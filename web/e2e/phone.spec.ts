// The phone board (390px, touch): header facts, filters, cards, detail and the composer.
import { test, expect, live, card } from "./fixtures";

test("the phone header carries the title, connection, chips, router and status line", async ({ page }) => {
  await live(page);
  await expect(page.locator(".phone-brand")).toHaveText("forkdeck (demo)");
  await expect(page.locator(".phone-connection")).toHaveText("Live");
  await expect(page.locator(".agent-chip")).toHaveText(["CLAUDE ON CALL", "CODEX ON CALL", "COPILOT MISSING"]);
  const router = page.locator(".phone-router");
  await expect(router.locator("summary")).toHaveText("JEV (SIMULATED)");
  await router.locator("summary").tap();
  await expect(router.locator(".router-status")).toContainText("memory hits");
  await expect(page.locator(".board-status")).toContainText("issues: [on]");
  await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();
});

test("filters show active, attention, done and all jobs", async ({ page }) => {
  await live(page);
  const tabs = page.locator(".phone-tabs").getByRole("tab");
  await expect(tabs).toHaveText(["Active", "Attention", "Done", "All"]);
  await expect(tabs.first()).toHaveAttribute("aria-selected", "true");
  await expect(card(page, 72)).toBeVisible();
  await expect(card(page, 61)).toHaveCount(0);
  await tabs.nth(1).tap();
  await expect(tabs.nth(1)).toHaveAttribute("aria-selected", "true");
  await expect(card(page, 61)).toBeVisible();
  await expect(card(page, 72)).toHaveCount(0);
  await tabs.nth(2).tap();
  await expect(card(page, 70)).toBeVisible();
  await tabs.nth(3).tap();
  await expect(page.locator(".phone-card")).toHaveCount(await page.locator(".phone-card").count());
  await expect(card(page, 61)).toBeVisible();
  await expect(card(page, 72)).toBeVisible();
});

test.describe("custom lane names", () => {
  test.use({ daemonOptions: { config: '[web.lanes]\nattention = "Needs you"\ndone = "Finished"\n' } });
  test("filters and cards use [web.lanes]", async ({ page }) => {
    await live(page);
    await expect(page.locator(".phone-tabs").getByRole("tab")).toHaveText(["Active", "Needs you", "Finished", "All"]);
    await page.locator(".phone-tabs").getByRole("tab", { name: "Needs you" }).tap();
    await expect(card(page, 61).locator(".push-right")).toHaveText("Needs you");
  });
});

test("a card opens its detail with a way back; a plan card opens the plan", async ({ page }) => {
  await live(page);
  await card(page, 72).tap();
  await expect(page).toHaveURL(/\/jobs\/72$/);
  const back = page.getByRole("link", { name: "← Board" });
  await expect(back).toBeVisible();
  await expect(page.locator(".detail-title")).toHaveText("semantic-context#22 Home record API");
  await back.tap();
  await expect(page).toHaveURL(/\/$/);
  await page.locator(".phone-tabs").getByRole("tab", { name: "All" }).tap();
  await card(page, 100).tap();
  await expect(page).toHaveURL(/\/plans\/100$/);
});

test("the phone composer starts a task and replies work in the detail", async ({ page }) => {
  await live(page);
  await page.getByRole("textbox", { name: "New task", exact: true }).fill("From the phone");
  await page.getByRole("button", { name: "Start", exact: true }).tap();
  await expect(page).toHaveURL(/\/jobs\/\d+$/);
  await expect(page.locator(".detail-title")).toHaveText("From the phone");
  await expect(page.locator(".detail").getByLabel("Job output")).toContainText("Reading the task");
  const reply = page.locator(".reply-input");
  await reply.fill("Phone follow-up");
  await page.getByRole("button", { name: "Send now" }).tap();
  await expect(reply).toHaveValue("");
});

test("no page scrolls sideways on a phone", async ({ page }) => {
  for (const path of ["/", "/jobs/72", "/plans/100", "/repos", "/deploys", "/stats", "/configuration"]) {
    await page.goto(path);
    await expect(page.locator("h1, .phone-card").first()).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth), path).toBeLessThanOrEqual(0);
  }
});
