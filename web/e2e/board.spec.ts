// The wide board against a seeded demo daemon: header, status, lanes, cards, panel, session log, help.
import { test, expect, live, card, detail } from "./fixtures";

const lanes = ["Queue", "Build", "Review", "Delivery", "Done", "Attention"];

test("header shows the TUI's title, connection, slots, intake and agent chips", async ({ page }) => {
  await live(page);
  const header = page.locator(".app-header");
  await expect(header.locator(".brand-name")).toHaveText("forkdeck (demo)");
  await expect(header.locator(".workspace-label")).toHaveText("workspace");
  await expect(header.locator(".connection")).toHaveText(/^Live · \d+ slots of \d+ busy$/);
  await expect(header.getByRole("button", { name: /Issue intake on/ })).toBeEnabled();
  await expect(header.locator(".agent-chip")).toHaveText(["CLAUDE ON CALL", "CODEX ON CALL", "COPILOT MISSING"]);
  await expect(header.locator(".router-label")).toHaveText("JEV (SIMULATED)");
  await expect(page.locator(".board-status")).toHaveText(/^running: \[\d+\]  queued: \[\d+\]  issues: \[on\] · 24h: \d+ jobs \/ \d+ tokens$/);
  await expect(page.locator(".router-status")).toContainText("fork layer");
  await expect(page.locator(".router-status")).toContainText(/calls \d+ · fallbacks \d+ · last \d+ms/);
  await expect(page.locator(".router-status")).toContainText(/memory hits \d+/);
});

test("every lane is shown in order and its count matches its cards", async ({ page }) => {
  await live(page);
  await expect(page.locator(".lane h2")).toHaveText(lanes);
  for (const lane of await page.locator(".lane").all()) {
    const count = Number(await lane.locator(".lane-heading .mono-muted").textContent());
    await expect(lane.locator(".job-card")).toHaveCount(count);
  }
  // Seeded jobs land where the TUI puts them.
  await expect(page.locator(".lane").filter({ hasText: "Build" }).locator(".job-card")).toContainText(["#72"]);
  await expect(card(page, 61)).toContainText("failed");
  await expect(page.locator(".lane").filter({ hasText: "Attention" })).toContainText("#61");
});

test.describe("custom lane names", () => {
  test.use({ daemonOptions: { config: '[web.lanes]\nqueue = "Inbox"\nbuild = "Doing"\nreview = "Checking"\ndelivery = "Shipping"\ndone = "Finished"\nattention = "Needs you"\n' } });
  test("lane headings use [web.lanes]", async ({ page }) => {
    await live(page);
    await expect(page.locator(".lane h2")).toHaveText(["Inbox", "Doing", "Checking", "Shipping", "Finished", "Needs you"]);
  });
});

test("clicking a card floats its detail; close button and Esc return to the board", async ({ page }) => {
  await live(page);
  await card(page, 72).click();
  await expect(page).toHaveURL(/\/jobs\/72$/);
  await expect(detail(page)).toHaveClass(/open/);
  await expect(card(page, 72)).toHaveClass(/selected/);
  await expect(detail(page).locator(".detail-title")).toHaveText("semantic-context#22 Home record API");
  await detail(page).getByRole("button", { name: "Close job detail" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(detail(page)).not.toHaveClass(/open/);
  await card(page, 73).click();
  await expect(page).toHaveURL(/\/jobs\/73$/);
  await page.keyboard.press("Escape");
  await expect(page).toHaveURL(/\/$/);
});

test("a planning job's card opens its plan", async ({ page }) => {
  await live(page);
  await card(page, 100).click();
  await expect(page).toHaveURL(/\/plans\/100$/);
  await expect(page.locator(".plan-title")).toHaveText("Home records and sync");
  // A plan job's /jobs URL is the same plan.
  await page.goto("/jobs/101");
  await expect(page).toHaveURL(/\/plans\/101$/);
});

test("an unknown job never shows another job", async ({ page }) => {
  await live(page, "/jobs/999");
  await expect(detail(page).locator(".detail-title")).toHaveText("Job #999 is not on the board");
  await expect(page.locator(".job-card.selected")).toHaveCount(0);
});

test("the sidebar reaches every page", async ({ page }) => {
  await live(page);
  for (const [name, path, heading] of [["Plans", "/plans", "Home records and sync"], ["Repositories", "/repos", "Repositories"], ["Deployments", "/deploys", "Deployments"], ["Stats", "/stats", "Stats"], ["Configuration", "/configuration", "Configuration"]]) {
    await page.goto("/");
    await page.getByRole("navigation", { name: "Main" }).getByRole("link", { name, exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`${path}$`));
    await expect(page.getByRole("heading", { level: 1 })).toHaveText(heading);
  }
});

test("old Doctor links land on Configuration; unknown pages say so", async ({ page }) => {
  await page.goto("/doctor");
  await expect(page).toHaveURL(/\/configuration$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Configuration");
  await page.goto("/no-such-page");
  await expect(page.getByRole("status")).toContainText("Page not found");
});

test("the session log opens and streams new lines", async ({ page, daemon }) => {
  await live(page);
  const log = page.locator(".session-log");
  await log.locator("summary").click();
  await expect(log).toHaveAttribute("open", "");
  const rows = log.locator(".log-row");
  const before = await rows.count();
  expect((await daemon.act({ action: "submit", text: "Session log line", workspace: "workspace", agent: null, plan_first: false })).status).toBe(200);
  await expect(log).toContainText("Session log line");
  expect(await rows.count()).toBeGreaterThan(before);
});

test("? and the key bar open the shortcut help; Esc closes it", async ({ page }) => {
  await live(page);
  await expect(page.getByRole("contentinfo", { name: "Keyboard shortcuts" })).toContainText("F2 view");
  await page.keyboard.press("?");
  const help = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(help).toBeVisible();
  await expect(help).toContainText("Ctrl-S / Ctrl-Enter");
  await page.keyboard.press("Escape");
  await expect(help).toBeHidden();
  await page.getByRole("button", { name: "? Help" }).click();
  await expect(help).toBeVisible();
  await help.getByRole("button", { name: "Close help" }).click();
  await expect(help).toBeHidden();
});

test.describe("a daemon with no jobs", () => {
  test.use({ daemonOptions: { demo: false } });
  test("shows the empty board with a working composer", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("No jobs yet");
    await expect(page.getByRole("textbox", { name: "New task", exact: true })).toBeEnabled();
    await expect(page.getByRole("button", { name: /Issue intake/ })).toBeVisible();
    await expect(page.locator(".brand-name")).toHaveCount(0);
  });
});
