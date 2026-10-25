// Every write the browser offers, clicked for real against the demo daemon's Hub.
import { test, expect, live, card, detail, type Daemon } from "./fixtures";

async function submitted(daemon: Daemon, text: string, extra = {}) {
  const result = await daemon.act({ action: "submit", text, workspace: "workspace", agent: null, plan_first: false, ...extra });
  expect(result.status, JSON.stringify(result.body)).toBe(200);
  return result.body.id as number;
}
const state = (page: import("@playwright/test").Page) => detail(page).locator(".detail-meta .pill");

test("New task starts a job, opens it and resets the composer", async ({ page }) => {
  await live(page);
  const input = page.getByRole("textbox", { name: "New task", exact: true });
  await input.fill("Add a health endpoint");
  await page.getByRole("button", { name: "Start", exact: true }).click();
  await expect(page).toHaveURL(/\/jobs\/\d+$/);
  const id = Number(page.url().split("/").pop());
  await expect(detail(page).locator(".detail-title")).toHaveText("Add a health endpoint");
  await expect(card(page, id)).toContainText("Add a health endpoint");
  await expect(input).toHaveValue("");
  await expect(page.getByRole("checkbox", { name: "Plan first" })).not.toBeChecked();
  await expect(card(page, id)).toContainText("unverified", { timeout: 30_000 });
  await expect(page.locator(".lane").filter({ hasText: "Attention" })).toContainText(`#${id}`);
});

test("Enter submits; the chosen agent runs the task", async ({ page }) => {
  await live(page);
  await page.getByLabel("Agent").selectOption("codex");
  const copilot = await page.getByLabel("Agent").evaluate((s: HTMLSelectElement) => [...s.options].find(o => o.value === "copilot")?.disabled);
  expect(copilot, "copilot is not installed").toBe(true);
  await page.getByRole("textbox", { name: "New task", exact: true }).fill("Codex does this one");
  await page.getByRole("textbox", { name: "New task", exact: true }).press("Enter");
  await expect(page).toHaveURL(/\/jobs\/\d+$/);
  await expect(detail(page).locator(".mono-muted").first()).toContainText("codex", { timeout: 15_000 });
});

test("a task can start in a configured repository instead of the current workspace", async ({ page }) => {
  await live(page);
  await page.getByLabel("Workspace").selectOption({ label: "o/semantic-context" });
  await page.getByRole("textbox", { name: "New task", exact: true }).fill("Work in the repository");
  const sent = page.waitForRequest(r => r.url().endsWith("/api/v1/actions"));
  await page.getByRole("button", { name: "Start", exact: true }).click();
  expect((await sent).postDataJSON()).toMatchObject({ action: "submit", workspace: "repo-0", text: "Work in the repository" });
  await expect(page).toHaveURL(/\/jobs\/\d+$/);
  await expect(detail(page).locator(".detail-title")).toHaveText("Work in the repository");
});

test("Plan first offers choices; choosing an option carries on", async ({ page }) => {
  await live(page);
  await page.getByRole("textbox", { name: "New task", exact: true }).fill("Fix the retry loop");
  await page.getByRole("checkbox", { name: "Plan first" }).check();
  await page.getByRole("button", { name: "Start", exact: true }).click();
  await expect(page).toHaveURL(/\/jobs\/\d+$/);
  await expect(page.getByRole("checkbox", { name: "Plan first" })).not.toBeChecked();
  const choices = detail(page).getByRole("region", { name: "Choices" });
  await expect(choices.getByRole("button")).toHaveCount(2);
  await expect(choices).toContainText("Reset the backoff on success");
  await expect(state(page)).toHaveText("choose an option");
  await choices.getByRole("button", { name: /Reset the backoff on success/ }).click();
  await expect(choices).toBeHidden();
  await expect(state(page)).not.toHaveText("choose an option");
  await expect(detail(page).getByLabel("Job output")).toContainText("Reading the task");
});

test("Use my response sends a custom approach instead of an option", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Fix the retry loop", { plan_first: true });
  await live(page, `/jobs/${id}`);
  const panel = detail(page);
  await expect(panel.getByRole("region", { name: "Choices" })).toBeVisible();
  const reply = panel.getByRole("textbox").first();
  await reply.fill("Use exponential backoff with jitter");
  await panel.getByRole("button", { name: "Use my response" }).click();
  await expect(panel.getByRole("region", { name: "Choices" })).toBeHidden();
  await panel.getByRole("tab", { name: "Conversation" }).click();
  await expect(panel.getByLabel("Job output")).toContainText("Use exponential backoff with jitter");
});

test("Queue holds a reply for a running task and the card counts it", async ({ page, daemon }) => {
  const id = await submitted(daemon, "A long task");
  await live(page, `/jobs/${id}`);
  const panel = detail(page);
  // A queued message waits for the worker's current turn to end.
  await expect(panel.getByLabel("Job output")).toContainText("Reading the task");
  await panel.getByRole("textbox").fill("Also update the changelog");
  await panel.getByRole("button", { name: "Queue" }).click();
  await expect(panel.locator(".queued-messages")).toContainText("Also update the changelog");
  await expect(card(page, id)).toContainText(`#${id} ✉1`);
  await expect(panel.getByRole("textbox")).toHaveValue("");
});

test("Send now steers the running agent", async ({ page, daemon }) => {
  const id = await submitted(daemon, "A long task");
  await live(page, `/jobs/${id}`);
  const panel = detail(page);
  await expect(state(page)).toHaveText(/working|routing/);
  await panel.getByRole("textbox").fill("Prefer the smaller change");
  await panel.getByRole("button", { name: "Send now" }).click();
  await expect(panel.getByRole("textbox")).toHaveValue("");
  await panel.getByRole("tab", { name: "Conversation" }).click();
  await expect(panel.getByLabel("Job output")).toContainText("Prefer the smaller change");
});

test("Stop cancels a running task; Resume picks it up again", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Stop me");
  await live(page, `/jobs/${id}`);
  const panel = detail(page);
  await expect(panel.getByLabel("Job output")).toContainText("Reading the task");
  await panel.getByRole("button", { name: "Stop" }).click();
  await expect(state(page)).toHaveText(/cancelled|interrupted/);
  await expect(panel.getByRole("button", { name: "Stop" })).toHaveCount(0);
  await expect(page.locator(".lane").filter({ hasText: "Attention" })).toContainText(`#${id}`);
  const resume = panel.getByRole("button", { name: "Resume" });
  const retry = panel.getByRole("button", { name: "Retry" });
  await expect(resume.or(retry).first()).toBeVisible();
  await resume.or(retry).first().click();
  await expect(state(page)).toHaveText(/queued|routing|working/);
});

test("Retry runs a failed task again", async ({ page }) => {
  await live(page, "/jobs/61");
  const panel = detail(page);
  await expect(state(page)).toHaveText("failed");
  await panel.getByRole("button", { name: "Retry" }).click();
  await expect(state(page)).not.toHaveText("failed");
  await expect(panel.getByLabel("Job output")).toContainText("Reading the task", { timeout: 20_000 });
});

test("an empty Enter on a finished task carries on", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Finish then continue");
  await live(page, `/jobs/${id}`);
  await expect(state(page)).toHaveText("unverified", { timeout: 30_000 });
  await detail(page).getByRole("textbox").press("Enter");
  await expect(state(page)).not.toHaveText("unverified");
  await expect(state(page)).toHaveText("unverified", { timeout: 30_000 });
});

test("Plan as issue shows the daemon's refusal in demo mode", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Plan this later");
  await live(page, `/jobs/${id}`);
  await expect(state(page)).toHaveText("unverified", { timeout: 30_000 });
  const panel = detail(page);
  await panel.getByRole("button", { name: "Plan as issue…" }).click();
  await expect(panel.getByRole("textbox", { name: "Planning request" })).toHaveValue("Plan this later");
  await expect(panel.getByLabel("Repository")).toHaveValue("repo-0");
  await panel.getByRole("button", { name: "Start planning" }).click();
  await expect(panel.getByRole("alert")).toHaveText("Issue planning is disabled in demo mode");
  await expect(page).toHaveURL(new RegExp(`/jobs/${id}$`));
});

test("Issue intake toggles, the status line follows and Configuration audits it", async ({ page }) => {
  await live(page);
  const intake = page.getByRole("button", { name: /Issue intake/ });
  await expect(intake).toHaveText("Issue intake on");
  await intake.click();
  await expect(intake).toHaveText("Issue intake paused");
  await expect(page.locator(".board-status")).toContainText("issues: [off]");
  await intake.click();
  await expect(intake).toHaveText("Issue intake on");
  await page.goto("/configuration");
  const audit = page.locator(".operation-panel").filter({ hasText: "Action audit log" });
  await expect(audit).toContainText("automation");
  await expect(audit).toContainText("local");
});

test("a refused reply keeps the draft and shows why", async ({ page, daemon }) => {
  const id = await submitted(daemon, "Refuse me");
  await live(page, `/jobs/${id}`);
  const panel = detail(page);
  await expect(panel.getByLabel("Job output")).toContainText("Reading the task");
  await panel.getByRole("textbox").fill("too late");
  // The task changes between rendering and sending: the daemon refuses the stale revision.
  await daemon.act({ action: "reply", id, text: "first", expected_revision: null });
  await page.route("**/api/v1/actions", async route => {
    const body = route.request().postDataJSON();
    await route.continue({ postData: JSON.stringify({ ...body, expected_revision: "stale" }) });
  });
  await panel.getByRole("button", { name: "Queue" }).click();
  await expect(panel.getByRole("alert")).toHaveText("This task changed; review the current revision");
  await expect(panel.getByRole("textbox")).toHaveValue("too late");
});
