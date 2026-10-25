// Issue plans: list, facts, tabs, Markdown, decision links and the plan controls with their reasons.
import { test, expect } from "./fixtures";

test("the plan list is in the main area and selects plans by address", async ({ page }) => {
  await page.goto("/plans");
  const list = page.getByRole("region", { name: "Issue plans" });
  await expect(list.getByRole("link")).toHaveCount(3);
  await expect(list.getByRole("link").first()).toHaveAttribute("aria-current", "page");
  await expect(page.locator(".plans-sidebar").getByRole("link", { name: /Home records/ })).toHaveCount(0);
  await list.getByRole("link", { name: /Web UI and server mode/ }).click();
  await expect(page).toHaveURL(/\/plans\/101$/);
  await expect(page.locator(".plan-title")).toHaveText("Web UI and server mode");
  await expect(list.getByRole("link", { name: /Web UI and server mode/ })).toHaveAttribute("aria-current", "page");
});

test("a published plan shows its issue, revision, flags, tasks, scheduler and decision", async ({ page }) => {
  await page.goto("/plans/100");
  await expect(page.locator(".plan-heading .mono-muted")).toHaveText(/^o\/semantic-context#20 · revision 3 · /);
  await expect(page.locator(".plan-subtitle")).toContainText("Accepted and created as 5 sub-issues");
  await expect(page.getByRole("link", { name: "Open #20 on GitHub" })).toHaveAttribute("href", "https://github.com/o/semantic-context/issues/20");
  await expect(page.getByLabel("Revision status").locator(".task-pill")).toHaveText(["reviewed: yes", "accepted: yes", "published: yes"]);
  const tasks = page.getByRole("region", { name: "Task list" }).locator(".task-row");
  await expect(tasks).toHaveCount(7);
  await expect(tasks.filter({ hasText: "after #" }).first()).toBeVisible();
  await expect(page.getByRole("region", { name: "Scheduler" })).toContainText("Scheduler");
  await expect(page.locator(".decision-heading")).toHaveText("#25 needs you: Pick storage backend");
  await expect(page.getByRole("link", { name: "Answer #25 on GitHub" })).toHaveAttribute("href", "https://github.com/o/semantic-context/issues/25");
});

test("Plan and Review tabs render the draft and review as Markdown", async ({ page }) => {
  await page.goto("/plans/100");
  await page.getByRole("tab", { name: "Plan" }).click();
  const prose = page.locator(".plan-prose");
  await expect(prose.getByRole("heading", { level: 1 })).toHaveText("Home records and sync");
  await expect(prose.locator("pre code")).toContainText("fn find_home");
  await expect(prose.getByRole("link", { name: "SQLite transactions" })).toHaveAttribute("href", "https://www.sqlite.org/lang_transaction.html");
  await expect(prose.locator("script, iframe")).toHaveCount(0);
  await page.getByRole("tab", { name: "Review" }).click();
  await expect(page.getByRole("tab", { name: "Review" })).toHaveAttribute("aria-selected", "true");
  await expect(prose.getByRole("heading", { level: 1 })).toHaveText("Plan review");
  await page.getByRole("tab", { name: "Tasks" }).click();
  await expect(page.getByRole("region", { name: "Task list" })).toBeVisible();
});

test("controls explain why they are off and Refine follows typed feedback", async ({ page }) => {
  await page.goto("/plans/100");
  const accept = page.getByRole("button", { name: "Accept and publish" });
  await expect(accept).toBeDisabled();
  await expect(accept).toHaveAttribute("title", "Issue publication is disabled in demo mode");
  await expect(page.getByRole("button", { name: "Ready for build" })).toHaveAttribute("title", "Issue publication is disabled in demo mode");
  await page.getByPlaceholder("Feedback to refine or re-plan…").fill("Split the API task");
  await expect(page.getByRole("button", { name: "Refine" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Refine" })).toHaveAttribute("title", "Changes to a published task graph are not supported yet");
  await page.goto("/plans/101");
  const feedback = page.getByPlaceholder("Feedback to refine or re-plan…");
  await expect(page.getByRole("button", { name: "Refine" })).toBeDisabled();
  await feedback.fill("Cover the phone layout too");
  await expect(page.getByRole("button", { name: "Refine" })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Re-plan…" })).toBeEnabled();
});

test("Refine sends the feedback to the planner and the plan goes back to planning", async ({ page }) => {
  await page.goto("/plans/101");
  await expect(page.locator(".plan-heading .mono-muted")).toContainText("Reviewed — accept and publish or refine");
  const feedback = page.getByPlaceholder("Feedback to refine or re-plan…");
  await feedback.fill("Cover the phone layout too");
  await page.getByRole("button", { name: "Refine" }).click();
  await expect(feedback).toHaveValue("");
  await expect(page.locator(".plan-heading .mono-muted")).toContainText("Planning — needs review");
  await expect(page.getByRole("region", { name: "Issue plans" }).getByRole("link", { name: /Web UI and server mode/ })).toContainText("Planning — needs review");
});

test("New plan opens the planning composer; the daemon's refusal is shown", async ({ page }) => {
  await page.goto("/plans");
  await page.getByRole("button", { name: "New plan" }).click();
  const request = page.getByRole("textbox", { name: "Planning request" });
  await expect(request).toBeVisible();
  await expect(page.getByLabel("Workspace")).toHaveValue("repo-0");
  await request.fill("Plan a sync feature");
  await page.getByRole("textbox", { name: "Existing issue" }).fill("#30");
  await page.getByRole("button", { name: "Start", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText("Issue planning is disabled in demo mode");
});

test("an unknown plan says so", async ({ page }) => {
  await page.goto("/plans/999");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Plan not found");
});

test.describe("a daemon with no plans", () => {
  test.use({ daemonOptions: { demo: false } });
  test("offers the planning composer", async ({ page }) => {
    await page.goto("/plans");
    await expect(page.getByText("No issue plans yet.")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Planning request" })).toBeVisible();
  });
});
