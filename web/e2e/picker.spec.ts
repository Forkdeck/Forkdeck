// Adding repositories from GitHub and GitLab: a normal daemon, a loopback forge and a stand-in clone tool.
import fs from "node:fs";
import { test as base, expect } from "./fixtures";
import { FakeForge } from "./daemon";

const repos = [
  { repo: "acme/api", private: true, branch: "main", description: "Service API" },
  { repo: "acme/web", private: false, branch: "trunk", description: "Browser client" },
  { repo: "acme/docs", private: false, branch: "main", description: "Handbook" },
];
const secret = "e2e-forge-secret";

// One forge per provider; the daemon is configured for both before it starts.
const test = base.extend<{ github: FakeForge; gitlab: FakeForge }>({
  github: async ({}, use) => { const f = await new FakeForge("github", [...repos]).start(); await use(f); await f.stop(); },
  gitlab: async ({}, use) => { const f = await new FakeForge("gitlab", repos.slice(0, 1)).start(); await use(f); await f.stop(); },
  daemonOptions: async ({ github, gitlab }, use, info) => {
    const clones = `${info.outputDir}/clones`;
    const forge = (name: string, f: FakeForge, host: string) => `[web.forges.${name}]\nhost = "${host}"\nclient_id = "e2e"\nclient_secret_env = "E2E_FORGE_SECRET"\nredirect_url = "http://127.0.0.1:{port}/forges/${name}/callback"\nprivate = true\nclone_dir = "${clones}"\nbase_url = "${f.url}"\n`;
    await use({ demo: false, env: { E2E_FORGE_SECRET: secret }, config: forge("github", github, "github.com") + forge("gitlab", gitlab, "gitlab.com") });
  },
});

// A clone tool that makes an empty checkout, or fails when told to; found on PATH when a clone runs.
test.beforeEach(async ({ daemon }) => {
  for (const tool of ["gh", "glab"])
    daemon.tool(tool, `#!/bin/sh\n[ "$3" = "acme/broken" ] && { echo "clone refused by forge" >&2; exit 1; }\nmkdir -p "$4/.git"\n`);
});

test("GitHub: connect, list every page, search and add a repository", async ({ page, daemon, github }) => {
  await page.goto("/repos");
  await page.getByRole("link", { name: "Add from GitHub" }).click();
  await expect(page).toHaveURL(/\/repos\/add\/[^/]+$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Add a repository from github.com");
  const rows = page.locator(".repo-picker-row");
  await expect(rows.getByRole("heading")).toHaveText(["acme/api", "acme/web", "acme/docs"]);
  expect(github.exchanges).toBe(1);
  expect(github.listings.length).toBe(2);
  await expect(rows.filter({ hasText: "acme/api" })).toContainText("Private");
  await expect(rows.filter({ hasText: "acme/web" })).toContainText("Default branch: trunk");
  await page.getByRole("searchbox", { name: "Search repositories" }).fill("handbook");
  await expect(rows.getByRole("heading")).toHaveText(["acme/docs"]);
  await page.getByRole("searchbox", { name: "Search repositories" }).fill("nothing like this");
  await expect(page.getByText("No repositories match your search.")).toBeVisible();
  await page.getByRole("searchbox", { name: "Search repositories" }).fill("");
  await rows.filter({ hasText: "acme/web" }).getByRole("button", { name: "Add" }).click();
  await expect(page.getByRole("status")).toContainText("Added acme/web at");
  await expect(page.getByRole("status")).toContainText("Restart Forkdeck to start watching it.");
  await expect(rows.filter({ hasText: "acme/web" })).toContainText("Already added");
  const config = fs.readFileSync(daemon.configPath, "utf8");
  expect(config).toMatch(/\[\[repos\]\][^[]*acme\/web/);
  // The provider token was used for listing only and never reached the browser or the config.
  expect(await page.content()).not.toContain(github.token);
  expect(config).not.toContain(github.token);
  expect(daemon.log).not.toContain(github.token);
});

test("GitLab: connect and add through the same picker", async ({ page }) => {
  await page.goto("/repos");
  await page.getByRole("link", { name: "Add from GitLab" }).click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Add a repository from gitlab.com");
  await page.locator(".repo-picker-row").getByRole("button", { name: "Add" }).click();
  await expect(page.getByRole("status")).toContainText("Added acme/api");
});

test("a repository already configured cannot be added twice", async ({ page }) => {
  await page.goto("/repos");
  await page.getByRole("link", { name: "Add from GitHub" }).click();
  const row = page.locator(".repo-picker-row").filter({ hasText: "acme/docs" });
  await row.getByRole("button", { name: "Add" }).click();
  await expect(row).toContainText("Already added");
  await page.goto("/repos");
  await page.getByRole("link", { name: "Add from GitHub" }).click();
  await expect(page.locator(".repo-picker-row").filter({ hasText: "acme/docs" })).toContainText("Already added");
});

test("an unknown or expired list asks to connect again", async ({ page }) => {
  await page.goto("/repos/add/no-such-picker");
  await expect(page.getByText("This list expired. Connect again.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Add from GitHub" })).toBeVisible();
});

test.describe("a failing clone", () => {
  test.use({ allowConsole: [] });
  test("shows the forge's message and adds nothing", async ({ page, daemon, github }) => {
    github.repos.push({ repo: "acme/broken", private: false, branch: "main", description: "Cannot clone" });
    await page.goto("/repos");
    await page.getByRole("link", { name: "Add from GitHub" }).click();
    const row = page.locator(".repo-picker-row").filter({ hasText: "acme/broken" });
    await row.getByRole("button", { name: "Add" }).click();
    await expect(page.getByRole("alert")).toHaveText("clone refused by forge");
    await expect(row.getByRole("button", { name: "Add" })).toBeVisible();
    expect(fs.readFileSync(daemon.configPath, "utf8")).not.toContain("acme/broken");
  });
});

test("Configuration redacts the forge secret", async ({ page }) => {
  await page.goto("/configuration");
  await expect(page.getByRole("heading", { level: 2, name: "Effective configuration · secrets redacted" })).toBeVisible();
  expect(await page.content()).not.toContain(secret);
});
