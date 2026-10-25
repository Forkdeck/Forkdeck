// Live board: binary snapshot and socket frames, other writers' changes, daemon restarts.
import { test, expect, live, card } from "./fixtures";

test("the snapshot and live frames are FlatBuffers, not JSON", async ({ page, daemon }) => {
  const frames: (string | Buffer)[] = [];
  page.on("websocket", ws => ws.on("framereceived", f => frames.push(f.payload)));
  const snapshot = page.waitForResponse(r => r.url().endsWith("/api/v1/snapshot"));
  await live(page);
  const response = await snapshot;
  expect(response.headers()["content-type"]).toBe("application/octet-stream");
  const body = (await response.body()).toString("utf8");
  expect(() => JSON.parse(body)).toThrow();
  await daemon.act({ action: "automation", on: false });
  await expect.poll(() => frames.length).toBeGreaterThan(0);
  expect(frames.every(f => Buffer.isBuffer(f))).toBe(true);
});

test("a job another client starts appears and moves through lanes without a reload", async ({ page, daemon }) => {
  await live(page);
  const result = await daemon.act({ action: "submit", text: "Started elsewhere", workspace: "workspace", agent: null, plan_first: false });
  const id = result.body.id as number;
  await expect(card(page, id)).toContainText("Started elsewhere");
  await expect(page.locator(".lane").filter({ hasText: "Build" })).toContainText(`#${id}`);
  await expect(page.locator(".lane").filter({ hasText: "Attention" })).toContainText(`#${id}`, { timeout: 30_000 });
});

test("intake changed elsewhere updates the header and status line", async ({ page, daemon }) => {
  await live(page);
  await daemon.act({ action: "automation", on: false });
  await expect(page.getByRole("button", { name: /Issue intake/ })).toHaveText("Issue intake paused");
  await expect(page.locator(".board-status")).toContainText("issues: [off]");
});

test("two browsers see each other's writes", async ({ page, browser, daemon }) => {
  const other = await browser.newPage();
  await other.goto(daemon.url);
  await live(page);
  await other.getByRole("textbox", { name: "New task", exact: true }).fill("From the second browser");
  await other.getByRole("button", { name: "Start", exact: true }).click();
  await expect(page.locator(".job-card", { hasText: "From the second browser" })).toBeVisible();
  await other.close();
});

test.describe("daemon restart", () => {
  // Chromium logs each refused reconnect attempt while the daemon is down.
  test.use({ allowConsole: [/WebSocket connection to .* failed/] });
  test("the board reconnects and keeps streaming", async ({ page, daemon }) => {
  await live(page);
  const connection = page.locator(".connection");
  await daemon.stop();
  await expect(connection).toContainText("Reconnecting");
  await daemon.start();
  await expect(connection).toContainText("Live", { timeout: 30_000 });
  const result = await daemon.act({ action: "submit", text: "After the restart", workspace: "workspace", agent: null, plan_first: false });
  await expect(card(page, result.body.id)).toContainText("After the restart");
  });
});

test("operational pages refresh when the board changes", async ({ page, daemon }) => {
  await live(page);
  await page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Repositories" }).click();
  const repo = page.locator(".operation-panel").filter({ hasText: "o/semantic-context" });
  await expect(repo.locator(".task-pill").first()).toHaveText("Automation on");
  await daemon.act({ action: "automation", on: false });
  await expect(repo.locator(".task-pill").first()).toHaveText("Paused");
});
