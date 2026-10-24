// Build web/ and the Rust binary first. Capture native seeded jobs and a private
// copy of real history through the same serve endpoints.
import { chromium } from "@playwright/test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHash } from "node:crypto";
import https from "node:https";
import http from "node:http";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
const root = path.resolve(import.meta.dirname, "../..");
const outputDir = path.join(root, "docs/screenshots");
const binary =
  process.env.FORKDECK_BIN || path.join(root, "target/debug/forkdeck");
fs.mkdirSync(outputDir, { recursive: true });
const browser = await chromium.launch({
  executablePath:
    process.env.CHROMIUM_PATH ||
    "/home/justin/.cache/ms-playwright/chromium-1187/chrome-linux/chrome",
  headless: true,
});
const captured = new Set();
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
    if (!fs.existsSync(source)) throw new Error(`Real history not found: ${source}`);
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
  if (real) {
    for (const name of ["journal.jsonl", "attempts-v1", "deploy.json"]) {
      const source = path.join(os.homedir(), ".local/state/forkdeck", name);
      if (!fs.existsSync(source)) continue;
      const files = fs.statSync(source).isDirectory()
        ? fs.readdirSync(source).map((name) => path.join(source, name)) : [source];
      for (const file of files) originals.push([file, hash(file)]);
      fs.cpSync(source, path.join(state, "forkdeck", name), { recursive: true });
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
    const address = await new Promise((resolve, reject) => {
      const timeout = setTimeout(
        () => reject(new Error("No server address")),
        30000,
      );
      server.once("exit", (code) => {
        clearTimeout(timeout);
        reject(new Error(`Server exited ${code}`));
      });
      server.stdout.on("data", (data) => {
        text += data;
        const match = text.match(
          /http:\/\/127\.0\.0\.1:\d+\//,
        );
        if (match) {
          clearTimeout(timeout);
          resolve(match[0]);
        }
      });
    });
    const base = new URL(address).origin;
    const page = await browser.newPage({
      viewport: { width: 1440, height: 900 },
    });
    const errors = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    await page.goto(base + "/");
    await page.waitForSelector(".job-card");
    const snapshot = await page.evaluate(async () =>
      (await fetch("/api/v1/snapshot")).json(),
    );
    const selected = real
      ? snapshot.jobs
          .filter((j) => j.state === "unverified")
          .sort((a, b) => b.id - a.id)[0]
      : snapshot.jobs.find((j) => j.kind === "manual" && ["working", "planning", "reviewing"].includes(j.state));
    if (!selected) throw new Error("No job for detail capture");
    if (!real) {
      // Exercise a native write before the Doctor capture; the audit is actual service data.
      for (const on of [!snapshot.automation, snapshot.automation]) {
        const response = await page.request.post(base + "/api/v1/actions", {
          headers: { "Idempotency-Key": `capture-intake-${on}` },
          data: { action: "automation", on },
        });
        if (!response.ok()) throw new Error(`Intake action: ${response.status()}`);
      }
    }
    const prefix = real ? "real-" : "";
    const detail = snapshot.jobs.find((j) => j.state === "unverified") || selected;
    const planList = await (await page.request.get(base + "/api/v1/plans")).json();
    const plan = planList.find((p) => p.plan.graph) || planList[0];
    const planUrl = plan ? `/plans/${plan.id}` : "/plans";
    const captures = [];
    for (const [width, height] of [[1440, 900], [390, 844]]) {
      captures.push(["board", width < 700 ? "/" : `/jobs/${selected.id}`, width, height]);
      captures.push(["detail", `/jobs/${detail.id}`, width, height]);
      for (const tab of ["Tasks", "Plan", "Review"])
        captures.push([`plans-${tab.toLowerCase()}`, planUrl, width, height, plan ? tab : null]);
    }
    for (const name of ["repos", "deploys", "stats", "doctor"])
      captures.push([name, `/${name}`, 1440, 900]);
    for (const [name, url, width, height, tab] of captures) {
      const file = `${prefix}${name}-${width}x${height}.png`;
      await page.setViewportSize({ width, height });
      await page.goto(base + url);
      if (["repos", "deploys", "stats", "doctor"].includes(name)) {
        await page.waitForFunction(() => !document.querySelector('[role="status"]'));
        await page.locator("main h1").waitFor();
      } else if (name.startsWith("plans")) {
        await page.getByRole("heading", { name: plan ? plan.title : "Issue plans", exact: true }).waitFor();
        if (tab) {
          await page.getByRole("tab", { name: tab, exact: true }).click();
          await page.locator(tab === "Tasks" ? ".task-list" : ".plan-prose").waitFor();
        }
      } else {
        await page.locator(width < 700 && url === "/" ? ".phone-card" : ".detail-title").first().waitFor();
        if (real && url === "/")
          await page.getByRole("tab", { name: "All", exact: true }).click();
        if (url.startsWith("/jobs")) {
          await page.waitForFunction((id) => document.querySelector(".detail-meta")?.textContent.startsWith(`#${id}`), Number(url.split("/").pop()));
          await page.locator(".job-card.selected").evaluate((node) => node.scrollIntoView({ block: "nearest", inline: "nearest" }));
        }
      }
      if (!real && name === "board" && width >= 700) {
        // Unsent drafts come from the selected native task; no response fixtures are introduced.
        await page.getByRole("textbox", { name: "New task", exact: true }).fill(selected.title);
        await page.getByRole("textbox", { name: "Reply to this task", exact: true }).fill(selected.title);
        if (!(await page.getByRole("button", { name: "Send now", exact: true }).isEnabled())) throw new Error("Writable controls disabled");
      }
      await save(page, file);
      if (!real) {
        const ref = name === "board" ? (width < 700 ? "Phone" : "Main") : name === "plans-tasks" && width === 1440 ? "Plan" : null;
        if (ref) await reference(page, ref);
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
async function save(page, file) {
  await page.evaluate(() => document.fonts.ready);
  await page.evaluate(() => new Promise(requestAnimationFrame));
  const problems = await page.evaluate(() => {
    const issues = [];
    if (document.documentElement.scrollWidth > innerWidth) issues.push("document overflow");
    const detail = document.querySelector(".detail-meta");
    const header = document.querySelector(".app-header");
    if (detail && detail.getBoundingClientRect().top < (header?.getBoundingClientRect().bottom ?? 0)) issues.push("detail heading clipped by header");
    for (const node of document.querySelectorAll(".reply-form input, .reply-form button")) {
      const r = node.getBoundingClientRect();
      if (r.x < 0 || r.y < 0 || r.right > innerWidth + 1 || r.bottom > innerHeight + 1)
        issues.push(`reply control outside viewport: ${node.textContent || "input"}`);
    }
    return issues;
  });
  if (problems.length) throw new Error(`${file}: ${problems.join(", ")}`);
  if (await page.locator(".connection, .phone-connection").count())
    await page.waitForFunction(() => [...document.querySelectorAll(".connection, .phone-connection")].some((node) => node.textContent.includes("Live")));
  await page.screenshot({ path: path.join(outputDir, file) });
  captured.add(file);
  console.log(file);
}

async function captureOidc() {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "forkdeck-oidc-capture-"));
  let fixture;
  let proxy;
  try {
    execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", path.join(temp, "key.pem"), "-out", path.join(temp, "cert.pem"), "-days", "1", "-subj", "/CN=localhost"], { stdio: "ignore" });
    let upstream;
    proxy = https.createServer({ key: fs.readFileSync(path.join(temp, "key.pem")), cert: fs.readFileSync(path.join(temp, "cert.pem")) }, (req, res) => {
      const forwarded = http.request(upstream + req.url, { method: req.method, headers: req.headers }, (response) => {
        res.writeHead(response.statusCode, response.headers);
        response.pipe(res);
      });
      forwarded.on("error", (error) => res.destroy(error));
      req.pipe(forwarded);
    });
    proxy.listen(0, "127.0.0.1");
    await once(proxy, "listening");
    const origin = `https://localhost:${proxy.address().port}`;
    fixture = spawn("cargo", ["test", "--bin", "forkdeck", "screenshot_provider", "--", "--ignored", "--nocapture"], {
      cwd: root, env: { ...process.env, FORKDECK_CAPTURE_ORIGIN: origin }, stdio: ["pipe", "pipe", "inherit"],
    });
    upstream = await new Promise((resolve, reject) => {
      let output = "";
      const timeout = setTimeout(() => reject(new Error("OIDC fixture did not start")), 120000);
      fixture.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`OIDC fixture exited ${code}`)); });
      fixture.stdout.on("data", (chunk) => {
        output += chunk;
        const match = output.match(/Forkdeck capture fixture: (http:\/\/127\.0\.0\.1:\d+)/);
        if (match) { clearTimeout(timeout); resolve(match[1]); }
      });
    });
    const context = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1440, height: 900 } });
    try {
      const page = await context.newPage();
      // Forkdeck's existing signed-out landing page; default local mode still has no login.
      await page.goto(origin + "/auth/logout");
      await page.getByRole("link", { name: "Sign in", exact: true }).waitFor();
      await save(page, "oidc-sign-in-1440x900.png");
      await page.getByRole("link", { name: "Sign in", exact: true }).click();
      await page.locator(".signed-in").getByText("Test member", { exact: true }).waitFor();
      await page.locator(".job-card.selected").waitFor();
      await save(page, "oidc-board-1440x900.png");
      await page.setViewportSize({ width: 390, height: 844 });
      await page.reload();
      await page.locator(".phone-card").first().waitFor();
      await save(page, "oidc-board-390x844.png");
    } finally { await context.close(); }
  } finally {
    if (fixture && fixture.exitCode === null) {
      const ended = once(fixture, "exit");
      fixture.stdin.end("\n");
      await ended;
    }
    if (proxy) await new Promise((resolve) => proxy.close(resolve));
    fs.rmSync(temp, { recursive: true, force: true });
  }
}

try {
  await capture(false);
  await capture(true);
  await captureOidc();
  for (const file of fs.readdirSync(outputDir)) {
    if (file.endsWith(".png") && !["terminal.png", "team.png"].includes(file) && !captured.has(file))
      fs.unlinkSync(path.join(outputDir, file));
  }
} finally {
  await browser.close();
}
