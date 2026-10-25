// Keyboard parity with the TUI, pressed for real against the demo daemon.
import { test, expect, live, card, detail, type Daemon } from "./fixtures";

const task = (page: import("@playwright/test").Page) => page.getByRole("textbox", { name: "New task", exact: true });
async function submitted(daemon: Daemon, text: string, extra = {}) {
  const result = await daemon.act({ action: "submit", text, workspace: "workspace", agent: null, plan_first: false, ...extra });
  expect(result.status).toBe(200);
  return result.body.id as number;
}

test("n focuses New task; Ctrl-P and Shift-Tab toggle Plan first; Esc clears", async ({ page }) => {
  await live(page);
  await page.locator("body").click();
  await page.keyboard.press("n");
  await expect(task(page)).toBeFocused();
  await expect(task(page)).toHaveValue("");
  const plan = page.getByRole("checkbox", { name: "Plan first" });
  await page.keyboard.press("Control+p");
  await expect(plan).toBeChecked();
  await page.keyboard.press("Shift+Tab");
  await expect(plan).not.toBeChecked();
  await page.keyboard.type("draft text");
  await page.keyboard.press("Control+p");
  await page.keyboard.press("Escape");
  await expect(task(page)).toHaveValue("");
  await expect(plan).not.toBeChecked();
  await page.keyboard.type("again");
  await page.keyboard.press("Control+u");
  await expect(task(page)).toHaveValue("");
});

test("↑ and ↓ recall sent tasks and restore the draft; history survives a reload", async ({ page }) => {
  await live(page);
  for (const text of ["first task", "second task"]) {
    await task(page).fill(text);
    await task(page).press("Enter");
    await expect(page).toHaveURL(/\/jobs\/\d+$/);
    await page.goto("/");
  }
  await task(page).fill("unfinished");
  await task(page).press("ArrowUp");
  await expect(task(page)).toHaveValue("second task");
  await task(page).press("ArrowUp");
  await expect(task(page)).toHaveValue("first task");
  await task(page).press("ArrowDown");
  await task(page).press("ArrowDown");
  await expect(task(page)).toHaveValue("unfinished");
  await page.reload();
  await expect(task(page)).toHaveValue("unfinished");
  await task(page).press("ArrowUp");
  await expect(task(page)).toHaveValue("second task");
});

test("Alt-↑/↓ step through jobs; Alt-←/→ jump lanes; F2 opens and closes", async ({ page }) => {
  await live(page);
  await page.locator("body").click();
  await page.keyboard.press("F2");
  await expect(page).toHaveURL(/\/jobs\/72$/);
  await page.keyboard.press("Alt+ArrowRight");
  await expect(page).toHaveURL(/\/jobs\/73$/);
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(page).toHaveURL(/\/jobs\/72$/);
  const first = page.url();
  await page.keyboard.press("Alt+ArrowDown");
  await expect(page).not.toHaveURL(first);
  await page.keyboard.press("Alt+ArrowUp");
  await expect(page).toHaveURL(first);
  await page.keyboard.press("F2");
  await expect(page).toHaveURL(/\/$/);
  await expect(detail(page)).not.toHaveClass(/open/);
});

test("F4 opens Repositories and F6 issue plans", async ({ page }) => {
  await live(page);
  await page.locator("body").click();
  await page.keyboard.press("F4");
  await expect(page).toHaveURL(/\/repos$/);
  await page.keyboard.press("F6");
  await expect(page).toHaveURL(/\/plans$/);
});

test("Ctrl-A toggles intake outside text fields and selects text inside them", async ({ page }) => {
  await live(page);
  const intake = page.getByRole("button", { name: /Issue intake/ });
  await page.locator("body").click();
  await page.keyboard.press("Control+a");
  await expect(intake).toHaveText("Issue intake paused");
  await task(page).fill("select me");
  await task(page).press("Control+a");
  await expect(intake).toHaveText("Issue intake paused");
  expect(await task(page).evaluate((t: HTMLTextAreaElement) => t.selectionEnd - t.selectionStart)).toBe("select me".length);
});

test("Ctrl-X stops the job whose panel is open, and nothing without a panel", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Stop with a key");
  await live(page);
  await page.locator("body").click();
  await page.keyboard.press("Control+x");
  await page.waitForTimeout(500);
  await expect(card(page, id)).not.toContainText("cancelled");
  await page.goto(`/jobs/${id}`);
  await expect(detail(page).getByLabel("Job output")).toContainText("Reading the task");
  await page.locator(".detail-title").click();
  await page.keyboard.press("Control+x");
  await expect(detail(page).locator(".detail-meta .pill")).toHaveText(/cancelled|interrupted/);
});

test("Enter queues a reply; Ctrl-S sends one now", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Steer by key");
  await live(page, `/jobs/${id}`);
  const panel = detail(page);
  await expect(panel.getByLabel("Job output")).toContainText("Reading the task");
  const reply = panel.getByRole("textbox");
  // Queue first: sending now starts a new turn, which takes anything queued straight away.
  await reply.fill("Wait your turn");
  await reply.press("Enter");
  await expect(panel.locator(".queued-messages")).toContainText("Wait your turn");
  await reply.fill("Sent at once");
  await reply.press("Control+s");
  await expect(reply).toHaveValue("");
  await panel.getByRole("tab", { name: "Conversation" }).click();
  await expect(panel.getByLabel("Job output")).toContainText("Sent at once");
});

test("F5 focuses the first choice; Enter on it chooses", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Choose by key", { plan_first: true });
  await live(page);
  await expect(card(page, id)).toContainText("choose an option");
  await page.goto(`/jobs/${id}`);
  const choices = detail(page).getByRole("region", { name: "Choices" });
  await expect(choices).toBeVisible();
  await page.locator(".detail-title").click();
  await page.keyboard.press("F5");
  await expect(choices.getByRole("button").first()).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(choices).toBeHidden();
});
