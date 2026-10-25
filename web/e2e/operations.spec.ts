// Repositories, Deployments, Stats and Configuration read the daemon's own observations.
import { test, expect } from "./fixtures";

const panel = (page: import("@playwright/test").Page, title: string | RegExp) =>
  page.locator(".operation-panel").filter({ has: page.getByRole("heading", { level: 2, name: title, exact: true }) });

test("Repositories shows intake, poll facts, limits, teammate claims and plan links", async ({ page }) => {
  await page.goto("/repos");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Repositories");
  const repo = panel(page, "o/semantic-context");
  await expect(repo.locator(".task-pill").first()).toHaveText("Automation on");
  await expect(repo).toContainText("github");
  await expect(repo).toContainText("agent:ready");
  await expect(repo).toContainText("Forge rate limit on previous poll; recovered");
  await expect(repo).toContainText("waiting for the single recovery probe");
  await expect(repo).toContainText("Anyone");
  await expect(repo.getByRole("heading", { name: "Limits · zero means unlimited" })).toBeVisible();
  await expect(repo).toContainText("repo jobs per day");
  await expect(repo.getByRole("heading", { name: "Teammate claims" })).toBeVisible();
  await expect(repo).toContainText("alex");
  await expect(repo).toContainText("Home record API");
  await expect(page.locator(".operations-header, .app-header")).toContainText(/Observed \d/);
  await repo.getByRole("link", { name: "Home records and sync" }).click();
  await expect(page).toHaveURL(/\/plans\/100$/);
});

test("Deployments shows revisions, rollout state and each deployment job", async ({ page }) => {
  await page.goto("/deploys");
  const env = panel(page, "o/semantic-context · staging");
  await expect(env).toContainText("a06de31");
  await expect(env).toContainText("c78f0a2");
  await expect(env).toContainText("Job #110");
  await expect(env).toContainText("Health check failed; rolled back to a06de31");
  await expect(env).toContainText("Job #111");
  await expect(env).toContainText("Rollout: 2 of 3 replicas ready");
  const saved = env.locator("details");
  await expect(saved.locator("dl")).toBeHidden();
  await saved.locator("summary", { hasText: "Persistent rollout state" }).click();
  await expect(saved).toContainText("in progress");
});

test("Stats shows outcome charts, pull request rates, usage and the router", async ({ page }) => {
  await page.goto("/stats");
  await expect(page.getByText(/^Last 7 days/)).toBeVisible();
  for (const kind of ["issues", "tasks", "reviews", "deploys"])
    await expect(panel(page, new RegExp(`^${kind} · \\d+ jobs$`)).getByRole("img")).toHaveAttribute("aria-label", /^\d+ done, \d+ failed, \d+ cancelled$/);
  await expect(panel(page, "Pull request rates & time")).toContainText("Durations in seconds");
  await expect(page.locator(".metrics-note")).toContainText("Observational usage, not a model ranking");
  await expect(panel(page, "Per model, CLI, effort & role")).toContainText("sonnet");
  await expect(panel(page, "Task cohorts · quality, cost coverage & queue wait")).toContainText("prompt-medium");
  await expect(panel(page, "Router · current daemon session")).toBeVisible();
});

test("Configuration shows each check, the audit log and redacted settings", async ({ page, daemon }) => {
  await daemon.act({ action: "automation", on: false });
  await page.goto("/configuration");
  await expect(panel(page, "claude").locator(".task-pill")).toHaveText("ok");
  await expect(panel(page, "copilot").locator(".task-pill")).toHaveText("failure");
  await expect(panel(page, "decision endpoint")).toContainText("jev (simulated)");
  await expect(panel(page, "configuration")).toContainText(daemon.configPath);
  const audit = panel(page, "Action audit log");
  await expect(audit).toContainText("automation");
  await expect(audit).toContainText("200");
  await expect(panel(page, "Effective configuration · secrets redacted")).toBeVisible();
});
