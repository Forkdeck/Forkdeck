import { test as base, expect, type Page } from "@playwright/test";
import { Daemon, type DaemonOptions } from "./daemon";

interface Fixtures {
  daemonOptions: DaemonOptions;
  daemon: Daemon;
  /** Console errors a test expects, beyond the network failures the browser logs for refused writes. */
  allowConsole: RegExp[];
}

export const test = base.extend<Fixtures>({
  daemonOptions: [{}, { option: true }],
  allowConsole: [[], { option: true }],
  daemon: async ({ daemonOptions }, use, info) => {
    const daemon = new Daemon(daemonOptions);
    try {
      await daemon.start();
      await use(daemon);
    } finally {
      if (info.status !== info.expectedStatus) await info.attach("daemon.log", { body: daemon.log });
      await daemon.cleanup();
    }
  },
  baseURL: async ({ daemon }, use) => use(daemon.url),
  // Any script error, CSP refusal or unexpected console error fails the test.
  page: async ({ page, allowConsole }, use) => {
    const errors: string[] = [];
    page.on("pageerror", e => errors.push(String(e)));
    page.on("console", m => {
      // Refused writes log a failed resource; leaving a page mid-handshake logs an aborted socket.
      if (m.type() !== "error" || /^Failed to load resource|Connection closed before receiving a handshake response/.test(m.text())) return;
      if (!allowConsole.some(r => r.test(m.text()))) errors.push(m.text());
    });
    await use(page);
    expect(errors, "browser errors").toEqual([]);
  },
});
export { expect };
export type { Daemon } from "./daemon";

/** The board has its snapshot and an open live socket. */
export async function live(page: Page, path = "/") {
  await page.goto(path);
  await expect(page.locator(".connection, .phone-connection").first()).toContainText("Live");
}
export const card = (page: Page, id: number) => page.locator(".job-card, .phone-card").filter({ hasText: new RegExp(`^#${id}(?!\\d)`) });
export const detail = (page: Page) => page.getByRole("complementary", { name: "Job detail" });
