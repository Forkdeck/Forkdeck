// OIDC sign-in, roles and sign-out through the Rust auth tests' provider and seeded server.
import { test as base, expect } from "./fixtures";
import { OidcServer } from "./daemon";

const test = base.extend<{ admin: OidcServer; viewer: OidcServer }>({
  admin: async ({}, use) => { const s = await new OidcServer("admin").start(); await use(s); await s.cleanup(); },
  viewer: async ({}, use) => { const s = await new OidcServer("viewer").start(); await use(s); await s.cleanup(); },
  context: async ({ browser }, use) => {
    const context = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1440, height: 900 } });
    await use(context);
    await context.close();
  },
});

// Each fixture starts the Rust test binary; one at a time keeps start-up quick.
test.describe.configure({ mode: "default" });

test("signing in shows the member and enables writes", async ({ page, admin }) => {
  await page.goto(admin.url + "/auth/logout");
  await page.getByRole("link", { name: "Sign in", exact: true }).click();
  const account = page.locator(".signed-in");
  await expect(account).toContainText("Test member");
  await expect(account.locator("span")).toHaveAttribute("title", /user-1 · admin/);
  await expect(page.locator(".connection")).toContainText("Live");
  const intake = page.getByRole("button", { name: /Issue intake/ });
  await expect(intake).toBeEnabled();
  await intake.click();
  await expect(intake).toHaveText("Issue intake paused");
  await page.getByRole("textbox", { name: "New task", exact: true }).fill("Signed-in task");
  await page.getByRole("button", { name: "Start", exact: true }).click();
  await expect(page).toHaveURL(/\/jobs\/\d+$/);
  await page.goto(admin.url + "/configuration");
  await expect(page.locator(".operation-panel").filter({ hasText: "Action audit log" })).toContainText("user-1");
});

test("an unauthenticated visit goes through the provider and comes back signed in", async ({ page, admin }) => {
  await page.goto(admin.url + "/");
  await expect(page.locator(".signed-in")).toContainText("Test member");
  expect(new URL(page.url()).origin).toBe(admin.url);
});

test("signing out ends the session and closes the live socket", async ({ page, admin }) => {
  await page.goto(admin.url + "/");
  await expect(page.locator(".connection")).toContainText("Live");
  // A socket of the same session in a tab that stays put: signing out must close it from the
  // server. The signing-out page's own socket goes with its navigation, which proves nothing.
  const watcher = await page.context().newPage();
  await watcher.goto(admin.url + "/plans");
  await watcher.evaluate(url => new Promise<void>((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error("live socket refused"));
    socket.onclose = () => { (window as any).liveClosed = true; };
  }), admin.url.replace(/^https/, "wss") + "/api/v1/live");
  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("link", { name: "Sign in", exact: true })).toBeVisible();
  expect((await page.request.get(admin.url + "/api/v1/me")).status()).toBe(401);
  expect((await page.request.get(admin.url + "/api/v1/snapshot")).status()).toBe(401);
  await expect.poll(() => watcher.evaluate(() => (window as any).liveClosed === true)).toBe(true);
});

test.describe("a session ended elsewhere", () => {
  // Chromium logs the refused live socket before the board goes to sign in.
  test.use({ allowConsole: [/WebSocket connection to .* failed/] });
  test("sends the open board to sign in", async ({ page, admin }) => {
  await page.goto(admin.url + "/");
  await expect(page.locator(".connection")).toContainText("Live");
  const signIn = page.waitForRequest(r => new URL(r.url()).pathname === "/auth/login");
  // Same browser, another tab: the cookie is shared, so the open board's session ends too.
  const other = await page.context().newPage();
  await other.goto(admin.url + "/plans");
  await other.getByRole("button", { name: "Sign out" }).click();
  await expect(other.getByRole("link", { name: "Sign in", exact: true })).toBeVisible();
  await signIn;
  // The provider signs this browser straight back in; the board is live again.
  await expect(page.locator(".connection")).toContainText("Live");
  });
});

test("a viewer can read everything but every write control is off and refused", async ({ page, viewer }) => {
  await page.goto(viewer.url + "/");
  await expect(page.locator(".signed-in span")).toHaveAttribute("title", /viewer/);
  await expect(page.locator(".job-card").first()).toBeVisible();
  const reason = "Operator or admin role required";
  await expect(page.getByRole("textbox", { name: "New task", exact: true })).toBeDisabled();
  await expect(page.getByRole("textbox", { name: "New task", exact: true })).toHaveAttribute("title", reason);
  await expect(page.getByRole("button", { name: /Issue intake/ })).toBeDisabled();
  await expect(page.getByText(reason).first()).toBeVisible();
  await page.goto(viewer.url + "/jobs/73");
  await expect(page.getByRole("button", { name: "Stop" })).toBeDisabled();
  await page.goto(viewer.url + "/plans/101");
  await expect(page.getByRole("button", { name: "New plan" })).toBeDisabled();
  await expect(page.getByPlaceholder("Feedback to refine or re-plan…")).toBeDisabled();
  const refused = await page.request.post(viewer.url + "/api/v1/actions", {
    headers: { Origin: viewer.url, "Idempotency-Key": "viewer-write" }, data: { action: "automation", on: false },
  });
  expect(refused.status()).toBe(403);
  await page.goto(viewer.url + "/");
  await expect(page.getByRole("button", { name: /Issue intake/ })).toHaveText("Issue intake on");
});

test("writes from another origin are refused even when signed in", async ({ page, admin }) => {
  await page.goto(admin.url + "/");
  await expect(page.locator(".signed-in")).toContainText("Test member");
  for (const origin of ["https://evil.example", "http://" + new URL(admin.url).host]) {
    const response = await page.request.post(admin.url + "/api/v1/actions", {
      headers: { Origin: origin, "Idempotency-Key": `cross-${origin.length}` }, data: { action: "automation", on: false },
    });
    expect(response.status(), origin).toBe(403);
  }
});
