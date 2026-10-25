// Browser-facing protections of the no-login daemon: CSP, write guards, idempotency, socket origin.
import http from "node:http";
import { test, expect, live } from "./fixtures";

/** The status a WebSocket upgrade gets with the given Origin. */
function upgrade(url: string, origin: string) {
  return new Promise<number>((resolve, reject) => {
    const request = http.get(url.replace(/\/$/, "") + "/api/v1/live?since=0", {
      headers: { Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", Origin: origin },
    });
    request.on("upgrade", (response, socket) => { socket.destroy(); resolve(response.statusCode!); });
    request.on("response", response => { response.resume(); resolve(response.statusCode!); });
    request.on("error", reject);
  });
}

test("pages carry a strict CSP and run without eval", async ({ page }) => {
  const response = await page.goto("/");
  const csp = response!.headers()["content-security-policy"] ?? "";
  expect(csp).toContain("script-src 'self'");
  expect(csp).not.toContain("unsafe-eval");
  expect(csp).not.toContain("unsafe-inline' 'unsafe-eval");
  await expect(page.locator(".job-card").first()).toBeVisible();
  // A CSP refusal is a console error, which the page fixture turns into a failure.
});

test("writes need the page's origin and an idempotency key", async ({ daemon }) => {
  const post = (headers: Record<string, string>) => fetch(daemon.url + "/api/v1/actions", {
    method: "POST", headers: { "Content-Type": "application/json", ...headers }, body: JSON.stringify({ action: "automation", on: false }),
  });
  expect((await post({ Origin: daemon.url })).status, "no key").toBe(400);
  expect((await post({ Origin: "http://evil.example", "Idempotency-Key": "a" })).status, "foreign origin").toBe(403);
  expect((await post({ Origin: daemon.url, "Idempotency-Key": "c", "Sec-Fetch-Site": "cross-site" })).status, "cross-site fetch").toBe(403);
  const intake = await fetch(daemon.url + "/api/v1/doctor").then(r => r.json());
  expect(intake.audit.filter((a: any) => a.action === "automation" && a.status === "200")).toHaveLength(0);
});

test("a repeated idempotency key replays the first answer instead of starting a second job", async ({ daemon }) => {
  const command = { action: "submit", text: "Only once", workspace: "workspace", agent: null, plan_first: false };
  const first = await daemon.act(command, "same-key");
  const second = await daemon.act(command, "same-key");
  expect(first.status).toBe(200);
  expect(second).toEqual(first);
  const jobs = await fetch(daemon.url + "/api/v1/jobs/" + (first.body.id + 1));
  expect(jobs.status).toBe(404);
});

test("the live socket refuses another origin", async ({ page, daemon }) => {
  await live(page);
  expect(await upgrade(daemon.url, daemon.url), "same origin").toBe(101);
  expect(await upgrade(daemon.url, "http://evil.example"), "foreign origin").toBe(403);
});
