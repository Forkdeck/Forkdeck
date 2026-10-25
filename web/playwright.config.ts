import { defineConfig } from "@playwright/test";

// Every test starts its own daemon (see e2e/fixtures.ts), so tests run in parallel safely.
export default defineConfig({
  testDir: "e2e",
  globalSetup: "./e2e/global-setup.ts",
  timeout: 60_000,
  expect: { timeout: 15_000 },
  fullyParallel: true,
  workers: process.env.CI ? 2 : 4,
  retries: 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    launchOptions: { executablePath: process.env.FORKDECK_CHROMIUM || undefined },
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "desktop", testIgnore: /phone\.spec/, use: { viewport: { width: 1440, height: 900 } } },
    { name: "phone", testMatch: /phone\.spec/, use: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
  ],
});
