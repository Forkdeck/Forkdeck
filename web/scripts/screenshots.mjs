// Build web/ and the Rust binary first. Capture native seeded jobs and a private
// copy of real history through the same authenticated serve endpoints.
import { chromium } from "@playwright/test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { once } from "node:events";
const root = path.resolve(import.meta.dirname, "../..");
const outputDir = path.join(root, "docs/web-ui-mockup/screenshots");
const binary =
  process.env.FORKDECK_BIN || path.join(root, "target/debug/forkdeck");
const browser = await chromium.launch({
  executablePath:
    process.env.CHROMIUM_PATH ||
    "/home/justin/.cache/ms-playwright/chromium-1187/chrome-linux/chrome",
  headless: true,
});
const hash = (file) =>
  createHash("sha256").update(fs.readFileSync(file)).digest("hex");

async function reference(page, name) {
  const source = fs.readFileSync(
    path.join(root, `docs/web-ui-mockup/${name}.dc.html`),
    "utf8",
  );
  const css = fs.readFileSync(
    path.join(
      root,
      "web/dist/assets",
      fs
        .readdirSync(path.join(root, "web/dist/assets"))
        .find((f) => f.endsWith(".css")),
    ),
    "utf8",
  );
  await page.evaluate(
    ({ source, css }) => {
      const parsed = new DOMParser().parseFromString(source, "text/html");
      const logic = parsed.querySelector(
        'script[type="text/x-dc"]',
      ).textContent;
      class DCLogic {
        setState() {}
      }
      const values = new Function(
        "DCLogic",
        logic + ";return new Component({}).renderVals();",
      )(DCLogic);
      const template = parsed.querySelector("x-dc");
      template.querySelector("helmet").remove();
      const value = (text, ctx) =>
        text
          .trim()
          .split(".")
          .reduce((v, key) => v?.[key], ctx);
      const binding = (text, ctx) =>
        text.replace(
          /\{\{\s*(.*?)\s*\}\}/g,
          (_, expr) => value(expr, ctx) ?? "",
        );
      function walk(node, ctx) {
        if (node.nodeType === 3) {
          node.textContent = binding(node.textContent, ctx);
          return;
        }
        if (node.nodeType !== 1) return;
        if (node.tagName === "SC-FOR") {
          const list = value(node.getAttribute("list").slice(2, -2), ctx);
          for (const item of list)
            for (const child of [...node.childNodes]) {
              const clone = child.cloneNode(true);
              node.before(clone);
              walk(clone, { ...ctx, [node.getAttribute("as")]: item });
            }
          node.remove();
          return;
        }
        if (node.tagName === "SC-IF") {
          if (value(node.getAttribute("value").slice(2, -2), ctx)) {
            for (const child of [...node.childNodes]) {
              node.before(child);
              walk(child, ctx);
            }
          }
          node.remove();
          return;
        }
        for (const attr of [...node.attributes]) {
          if (attr.name.toLowerCase().startsWith("on"))
            node.removeAttribute(attr.name);
          else node.setAttribute(attr.name, binding(attr.value, ctx));
        }
        for (const child of [...node.childNodes]) walk(child, ctx);
      }
      for (const node of [...template.childNodes]) walk(node, values);
      const faces = [...css.matchAll(/@font-face\{[^}]+\}/g)]
        .map((m) => m[0])
        .join("");
      document.head.innerHTML =
        "<style>body{margin:0;background:#0F1115}a{color:#F2A541}" +
        faces +
        "</style>";
      document.body.innerHTML = template.innerHTML;
    },
    { source, css },
  );
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: `/tmp/forkdeck-reference-${name}.png` });
}

async function capture(real) {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "forkdeck-web-capture-"));
  const workspace = path.join(temp, "Forkdeck");
  const state = path.join(temp, "state");
  fs.mkdirSync(workspace);
  fs.mkdirSync(path.join(state, "forkdeck"), { recursive: true });
  const originals = [];
  if (real) {
    const source = path.join(os.homedir(), ".local/state/forkdeck/forkdeck.db");
    // Copy the WAL too: SQLite may hold the newest committed history there.
    for (const suffix of ["", "-wal"]) {
      const file = source + suffix;
      if (fs.existsSync(file)) {
        originals.push([file, hash(file)]);
        fs.copyFileSync(
          file,
          path.join(state, "forkdeck/forkdeck.db" + suffix),
        );
      }
    }
  }
  const config = path.join(temp, "capture.toml");
  fs.writeFileSync(
    config,
    `[web]
port = 0
[remote]
enabled = false
[automation]
enabled = ${!real}
max_parallel = 3
` +
      (real
        ? `[agents]
discover_models = false
[agents.claude]
cmd = "/unavailable-for-history-capture"
[agents.codex]
cmd = "/unavailable-for-history-capture"
[agents.copilot]
cmd = "/unavailable-for-history-capture"
`
        : ""),
  );
  const server = spawn(
    binary,
    ["--config", config, "serve", ...(real ? [] : ["--demo"])],
    {
      cwd: workspace,
      env: { ...process.env, XDG_STATE_HOME: state },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  server.stderr.on("data", (data) => process.stderr.write(data));
  let text = "";
  try {
    const login = await new Promise((resolve, reject) => {
      const timeout = setTimeout(
        () => reject(new Error("No login URL")),
        30000,
      );
      server.once("exit", (code) => {
        clearTimeout(timeout);
        reject(new Error(`Server exited ${code}`));
      });
      server.stdout.on("data", (data) => {
        text += data;
        const match = text.match(
          /http:\/\/127\.0\.0\.1:\d+\/auth\/login\?code=[a-f0-9]+/,
        );
        if (match) {
          clearTimeout(timeout);
          resolve(match[0]);
        }
      });
    });
    const base = new URL(login).origin;
    const page = await browser.newPage({
      viewport: { width: 1440, height: 900 },
    });
    const errors = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    await page.goto(login);
    await page.waitForURL(base + "/");
    await page.waitForSelector(".job-card");
    const snapshot = await page.evaluate(async () =>
      (await fetch("/api/v1/snapshot")).json(),
    );
    const selected = real
      ? snapshot.jobs
          .filter((j) => j.state === "unverified")
          .sort((a, b) => b.id - a.id)[0]
      : snapshot.jobs.find((j) => j.state === "working");
    if (!selected) throw new Error("No job for detail capture");
    const captures = real
      ? [
          ["real-board-1440x900.png", `/jobs/${selected.id}`, 1440, 900],
          ["real-board-390x844.png", "/", 390, 844],
          ["real-detail-390x844.png", `/jobs/${selected.id}`, 390, 844],
        ]
      : [
          ["board-1440x900.png", `/jobs/${selected.id}`, 1440, 900, "Main"],
          ["plans-1440x900.png", "/plans", 1440, 900, "Plan"],
          ["board-390x844.png", "/", 390, 844, "Phone"],
          [
            "unverified-1440x900.png",
            `/jobs/${snapshot.jobs.find((j) => j.state === "unverified").id}`,
            1440,
            900,
          ],
        ];
    for (const [file, url, width, height, name] of captures) {
      await page.setViewportSize({ width, height });
      await page.goto(base + url);
      await page.waitForSelector(
        url.startsWith("/plans")
          ? ".task-row"
          : url.startsWith("/jobs")
            ? ".detail-title"
            : ".phone-card",
      );
      if (
        !url.startsWith("/plans") &&
        !(width < 700 && url.startsWith("/jobs"))
      )
        await page
          .getByText("Live", { exact: width < 700 })
          .first()
          .waitFor();
      await page.evaluate(() => document.fonts.ready);
      if (real && url === "/")
        await page.getByRole("tab", { name: "All", exact: true }).click();
      if (url.startsWith("/jobs") && (real || file.startsWith("unverified")))
        await page.locator(".warning").waitFor();
      if (url.startsWith("/jobs")) {
        const reply = await page.locator(".reply-form").boundingBox();
        const controls = await page
          .locator(".reply-form input, .reply-form button")
          .evaluateAll((nodes) =>
            nodes.map((node) => {
              const r = node.getBoundingClientRect();
              return {
                label: node.textContent || "input",
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
              };
            }),
          );
        for (const control of controls) {
          if (
            control.x < 0 ||
            control.y < 0 ||
            control.x + control.width > width + 1 ||
            control.y + control.height > height + 1
          ) {
            throw new Error(
              `Reply control outside viewport in ${file}: ${control.label}`,
            );
          }
        }
        if (!reply || reply.y + reply.height > height + 1)
          throw new Error(`Reply bar outside viewport in ${file}`);
      }
      await page.screenshot({ path: path.join(outputDir, file) });
      console.log(file);
      if (name) await reference(page, name);
    }
    if (!real) {
      await page.goto(base + "/plans");
      for (const tab of ["Plan", "Review"]) {
        await page.getByRole("tab", { name: tab, exact: true }).click();
        if (!(await page.locator(".plan-prose").innerText()).trim())
          throw new Error(`Empty ${tab}`);
      }
    }
    if (errors.length) throw new Error(errors.join("\n"));
    console.log(
      `${real ? "Real history" : "Seeded"}: ${snapshot.jobs.length} jobs, selected #${selected.id}`,
    );
    await page.close();
  } finally {
    const ended = once(server, "exit");
    server.kill("SIGTERM");
    await ended;
    for (const [file, before] of originals) {
      if (hash(file) !== before)
        throw new Error(`Original history changed: ${file}`);
    }
    fs.rmSync(temp, { recursive: true, force: true });
  }
}
try {
  await capture(false);
  await capture(true);
} finally {
  await browser.close();
}
