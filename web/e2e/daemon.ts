// Real Forkdeck processes for browser tests: each test gets its own daemon, state and port.
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import http from "node:http";
import https from "node:https";
import net from "node:net";
import os from "node:os";
import path from "node:path";

export const root = path.resolve(import.meta.dirname, "../..");
export const binary = process.env.FORKDECK_BIN || path.join(root, "target/debug/forkdeck");

export async function freePort(): Promise<number> {
  const server = net.createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const port = (server.address() as net.AddressInfo).port;
  await new Promise(resolve => server.close(resolve));
  return port;
}

async function ready(url: string, child: ChildProcess, log: () => string) {
  for (let i = 0; i < 200; i++) {
    if (child.exitCode !== null) throw new Error(`forkdeck exited ${child.exitCode}\n${log()}`);
    try { await fetch(url); return; } catch { await new Promise(r => setTimeout(r, 50)); }
  }
  throw new Error(`forkdeck did not start\n${log()}`);
}

export interface DaemonOptions {
  /** Seeded native jobs and simulated agents; false runs the normal daemon with no agents installed. */
  demo?: boolean;
  /** TOML appended after `[web] port = …`; use subtables such as `[web.lanes]` or `[[repos]]`. `{port}` is the daemon's port. */
  config?: string;
  env?: Record<string, string>;
}

export class Daemon {
  readonly dir = fs.mkdtempSync(path.join(os.tmpdir(), "forkdeck-e2e-"));
  readonly workspace = path.join(this.dir, "workspace");
  readonly configPath = path.join(this.dir, "config.toml");
  readonly bin = path.join(this.dir, "bin");
  url = "";
  private child?: ChildProcess;
  private output = "";
  private port = 0;
  constructor(private options: DaemonOptions = {}) {
    fs.mkdirSync(this.workspace);
    fs.mkdirSync(this.bin);
  }
  get log() { return this.output; }
  async start() {
    this.port ||= await freePort();
    if (!fs.existsSync(this.configPath))
      fs.writeFileSync(this.configPath, `[web]\nport = ${this.port}\n${(this.options.config ?? "").replaceAll("{port}", String(this.port))}\n`);
    const args = [...(this.options.demo === false ? [] : ["--demo"]), "--config", this.configPath, "serve"];
    // Only the test's own bin directory and the system tools: no installed agent CLIs or forge logins.
    const env = {
      PATH: `${this.bin}:/usr/bin:/bin`, HOME: this.dir, LANG: "C.UTF-8",
      XDG_STATE_HOME: path.join(this.dir, "state"), XDG_CONFIG_HOME: path.join(this.dir, "config"),
      XDG_CACHE_HOME: path.join(this.dir, "cache"), ...this.options.env,
    };
    const child = this.child = spawn(binary, args, { cwd: this.workspace, env, stdio: ["ignore", "pipe", "pipe"] });
    child.stdout!.on("data", d => { this.output += d; });
    child.stderr!.on("data", d => { this.output += d; });
    this.url = `http://127.0.0.1:${this.port}`;
    await ready(this.url + "/", child, () => this.output);
    return this;
  }
  async stop() {
    const child = this.child;
    this.child = undefined;
    if (!child || child.exitCode !== null) return;
    const exited = once(child, "exit");
    child.kill("SIGTERM");
    const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
    await exited;
    clearTimeout(timer);
  }
  async restart() { await this.stop(); await this.start(); }
  /** Writes an executable into the daemon's PATH, e.g. a stand-in `gh`. */
  tool(name: string, script: string) {
    const file = path.join(this.bin, name);
    fs.writeFileSync(file, script, { mode: 0o755 });
  }
  /** A guarded write as the browser sends it: same Origin, fresh Idempotency-Key. */
  async act(command: Record<string, unknown>, key = randomBytes(16).toString("hex")) {
    const response = await fetch(this.url + "/api/v1/actions", {
      method: "POST",
      headers: { "Content-Type": "application/json", "Idempotency-Key": key, Origin: this.url },
      body: JSON.stringify(command),
    });
    return { status: response.status, body: await response.json().catch(() => ({})) };
  }
  async json<T = any>(api: string): Promise<T> {
    const response = await fetch(`${this.url}/api/v1/${api}`);
    if (!response.ok) throw new Error(`${api}: HTTP ${response.status}`);
    return response.json();
  }
  async cleanup() {
    await this.stop();
    fs.rmSync(this.dir, { recursive: true, force: true });
  }
}

/** A loopback GitHub or GitLab: authorization code with PKCE, token exchange and a paginated listing. */
export class FakeForge {
  readonly token = "e2e-provider-token-" + randomBytes(8).toString("hex");
  readonly secret = "e2e-forge-secret";
  url = "";
  exchanges = 0;
  listings: string[] = [];
  private codes = new Map<string, string>();
  private server?: http.Server;
  constructor(readonly provider: "github" | "gitlab", readonly repos: { repo: string; private: boolean; branch: string; description: string }[]) {}
  async start() {
    this.server = http.createServer((req, res) => void this.handle(req, res));
    this.server.listen(0, "127.0.0.1");
    await once(this.server, "listening");
    this.url = `http://127.0.0.1:${(this.server.address() as net.AddressInfo).port}/`;
    return this;
  }
  private async handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const url = new URL(req.url!, this.url);
    const github = this.provider === "github";
    if (url.pathname === (github ? "/login/oauth/authorize" : "/oauth/authorize")) {
      const code = randomBytes(8).toString("hex");
      this.codes.set(code, url.searchParams.get("code_challenge")!);
      const back = new URL(url.searchParams.get("redirect_uri")!);
      back.searchParams.set("code", code);
      back.searchParams.set("state", url.searchParams.get("state")!);
      res.writeHead(302, { location: back.toString() }).end();
    } else if (url.pathname === (github ? "/login/oauth/access_token" : "/oauth/token")) {
      let body = "";
      for await (const chunk of req) body += chunk;
      const form = new URLSearchParams(body);
      const challenge = this.codes.get(form.get("code")!);
      this.codes.delete(form.get("code")!);
      const verified = challenge && createHash("sha256").update(form.get("code_verifier")!).digest("base64url") === challenge;
      if (!verified || form.get("client_secret") !== this.secret) { res.writeHead(400).end("{}"); return; }
      this.exchanges++;
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ access_token: this.token, token_type: "bearer" }));
    } else if (url.pathname === (github ? "/user/repos" : "/api/v4/projects")) {
      this.listings.push(req.headers.authorization ?? "");
      if (req.headers.authorization !== `Bearer ${this.token}`) { res.writeHead(401).end("[]"); return; }
      const page = Number(url.searchParams.get("page") ?? 1);
      const items = this.repos.slice((page - 1) * 2, page * 2).map(r => github
        ? { full_name: r.repo, private: r.private, default_branch: r.branch, description: r.description }
        : { path_with_namespace: r.repo, visibility: r.private ? "private" : "public", default_branch: r.branch, description: r.description });
      const headers: Record<string, string> = { "content-type": "application/json" };
      if (page * 2 < this.repos.length) headers.link = `<${url.pathname}?page=${page + 1}>; rel="next"`;
      res.writeHead(200, headers).end(JSON.stringify(items));
    } else res.writeHead(404).end();
  }
  async stop() { if (this.server) await new Promise(resolve => this.server!.close(resolve)); }
}

/** The OIDC provider and seeded server from the Rust auth tests, behind a loopback HTTPS proxy for Secure cookies. */
export class OidcServer {
  readonly dir = fs.mkdtempSync(path.join(os.tmpdir(), "forkdeck-e2e-oidc-"));
  url = "";
  private proxy?: https.Server;
  private fixture?: ChildProcess;
  /** Upgraded live sockets: the server's closeAllConnections does not see them. */
  private tunnels = new Set<net.Socket>();
  constructor(private role: "admin" | "viewer") {}
  async start() {
    const test = process.env.FORKDECK_AUTH_TEST_BIN;
    if (!test) throw new Error("global setup did not build the auth test fixture");
    execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", path.join(this.dir, "key.pem"), "-out", path.join(this.dir, "cert.pem"), "-days", "1", "-subj", "/CN=localhost"], { stdio: "ignore" });
    let upstream = "";
    this.proxy = https.createServer({ key: fs.readFileSync(path.join(this.dir, "key.pem")), cert: fs.readFileSync(path.join(this.dir, "cert.pem")) }, (req, res) => {
      const forwarded = http.request(upstream + req.url, { method: req.method, headers: req.headers }, response => {
        res.writeHead(response.statusCode!, response.headers);
        response.pipe(res);
      });
      forwarded.on("error", error => res.destroy(error));
      req.pipe(forwarded);
    });
    // Upgrade requests (the live socket) are piped through unchanged.
    this.proxy.on("upgrade", (req, socket, head) => {
      const target = new URL(upstream);
      this.tunnels.add(socket as net.Socket);
      socket.on("close", () => this.tunnels.delete(socket as net.Socket));
      const backend = net.connect(Number(target.port), target.hostname, () => {
        backend.write(`${req.method} ${req.url} HTTP/1.1\r\n${Object.entries(req.headers).map(([k, v]) => `${k}: ${v}`).join("\r\n")}\r\n\r\n`);
        backend.write(head);
        socket.pipe(backend).pipe(socket);
      });
      backend.on("error", () => socket.destroy());
      socket.on("error", () => backend.destroy());
    });
    this.proxy.listen(0, "127.0.0.1");
    await once(this.proxy, "listening");
    this.url = `https://localhost:${(this.proxy.address() as net.AddressInfo).port}`;
    const fixture = this.fixture = spawn(test, ["server::auth::tests::screenshot_provider", "--exact", "--ignored", "--nocapture"], {
      cwd: root, stdio: ["pipe", "pipe", "inherit"],
      env: { ...process.env, FORKDECK_CAPTURE_ORIGIN: this.url, FORKDECK_CAPTURE_ROLE: this.role, XDG_STATE_HOME: path.join(this.dir, "state"), XDG_CONFIG_HOME: path.join(this.dir, "config") },
    });
    upstream = await new Promise<string>((resolve, reject) => {
      let output = "";
      const timeout = setTimeout(() => reject(new Error("OIDC fixture did not start")), 90000);
      fixture.once("exit", code => { clearTimeout(timeout); reject(new Error(`OIDC fixture exited ${code}`)); });
      fixture.stdout!.on("data", chunk => {
        output += chunk;
        const match = output.match(/Forkdeck capture fixture: (http:\/\/127\.0\.0\.1:\d+)/);
        if (match) { clearTimeout(timeout); resolve(match[1]); }
      });
    });
    return this;
  }
  async cleanup() {
    const fixture = this.fixture;
    if (fixture && fixture.exitCode === null) {
      const ended = once(fixture, "exit");
      fixture.stdin!.end("\n");
      await ended;
    }
    for (const tunnel of this.tunnels) tunnel.destroy();
    if (this.proxy) { this.proxy.closeAllConnections(); await new Promise(resolve => this.proxy!.close(resolve)); }
    fs.rmSync(this.dir, { recursive: true, force: true });
  }
}
