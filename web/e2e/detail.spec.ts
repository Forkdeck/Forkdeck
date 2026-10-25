// The floating job detail: facts, stages, tabs and the output pane, against seeded and freshly run jobs.
import { test, expect, live, detail, type Daemon } from "./fixtures";

async function submitted(daemon: Daemon, text: string, extra = {}) {
  const result = await daemon.act({ action: "submit", text, workspace: "workspace", agent: null, plan_first: false, ...extra });
  expect(result.status, JSON.stringify(result.body)).toBe(200);
  return result.body.id as number;
}

test("an issue job shows its source, route, stages and why it takes no replies", async ({ page }) => {
  await live(page, "/jobs/72");
  const panel = detail(page);
  await expect(panel.locator(".detail-meta")).toContainText("#72");
  await expect(panel.locator(".detail-meta .pill")).toHaveText("working");
  await expect(panel.locator(".detail-source a", { hasText: "Source" })).toHaveAttribute("href", /github\.com\/o\/semantic-context\/issues\/22$/);
  await expect(panel.locator(".stage-chip")).toHaveCount(5);
  await expect(panel.locator(".stage-chip").first()).toContainText("Route");
  await expect(panel.getByRole("region", { name: "Worker" })).toContainText(/tools · \d+ in · \d+ out · \$\d+\.\d\d/);
  await expect(panel.getByRole("region", { name: "Reviewer" })).toBeVisible();
  await expect(panel.locator(".outcome h3")).toHaveText("Outcome");
  await expect(panel.locator(".action-notice")).toHaveText("Only typed tasks take replies.");
  await expect(panel.getByRole("textbox")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "Stop" })).toBeEnabled();
});

test("a delivery job links its pull request", async ({ page }) => {
  await live(page, "/jobs/60");
  await expect(detail(page).getByRole("link", { name: "Open PR" })).toHaveAttribute("href", /pull\/71$/);
});

test("a run fills Worker, Reviewer and Conversation tabs and the outcome", async ({ page, daemon }) => {
  await live(page);
  const id = await submitted(daemon, "Fix the retry loop");
  await page.goto(`/jobs/${id}`);
  const panel = detail(page);
  const output = panel.getByLabel("Job output");
  await expect(output).toContainText("Reading the task and the files it mentions");
  await expect(output).toContainText("▸ Edit src/net/client.rs");
  await expect(panel.locator(".detail-meta .pill")).toHaveText("unverified", { timeout: 30_000 });
  await expect(panel.locator(".outcome")).toContainText("Reset the backoff on success and added a regression test");
  await expect(panel.locator(".outcome")).toContainText("review passed");
  await expect(panel.locator(".detail-title")).toHaveText("Fix the retry loop");
  await expect(panel.locator(".routing-gate").first()).toBeVisible();
  await panel.getByRole("tab", { name: "Reviewer" }).click();
  await expect(panel.getByRole("tab", { name: "Reviewer" })).toHaveAttribute("aria-selected", "true");
  await expect(output).toContainText("VERDICT: PASS");
  await expect(output).not.toContainText("Reading the task");
  await panel.getByRole("tab", { name: "Conversation" }).click();
  await expect(output).toContainText("you  Fix the retry loop");
  await expect(output).toContainText("agent  Reset the backoff on success");
  await expect(output).not.toContainText("VERDICT");
});

test("a planning answer reads as its options, not raw markup", async ({ page, daemon }) => {
  await live(page);
  const id = await submitted(daemon, "Fix the retry loop", { plan_first: true });
  await page.goto(`/jobs/${id}`);
  const panel = detail(page);
  await expect(panel.getByRole("region", { name: "Choices" })).toBeVisible();
  await panel.getByRole("tab", { name: "Conversation" }).click();
  await expect(panel.getByLabel("Job output")).toContainText("Two ways to go.");
  await expect(panel.getByLabel("Job output")).not.toContainText("<options>");
});

test("the output follows new lines until scrolled up, then offers Jump to latest", async ({ page, daemon }) => {
  await page.setViewportSize({ width: 1440, height: 520 });
  await live(page);
  const id = await submitted(daemon, "A long run");
  await page.goto(`/jobs/${id}`);
  const output = detail(page).getByLabel("Job output");
  await expect(output).toContainText("▸ Read src/net/client.rs");
  const atBottom = () => output.evaluate(el => el.scrollHeight - el.clientHeight - el.scrollTop < 3);
  await expect.poll(atBottom).toBe(true);
  const scrollable = await output.evaluate(el => el.scrollHeight > el.clientHeight);
  test.skip(!scrollable, "the output fits; nothing to follow");
  await output.evaluate(el => { el.scrollTop = 0; el.dispatchEvent(new Event("scroll")); });
  const jump = detail(page).getByRole("button", { name: /lines back · Jump to latest/ });
  await expect(jump).toBeVisible();
  await jump.click();
  await expect(jump).toBeHidden();
  await expect.poll(atBottom).toBe(true);
});
