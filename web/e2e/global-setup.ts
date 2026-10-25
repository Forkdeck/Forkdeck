// The daemon embeds web/dist, so build it after the bundle; the auth test binary hosts the OIDC fixture.
import { execFileSync } from "node:child_process";
import { root } from "./daemon";

export default function setup() {
  if (!process.env.FORKDECK_BIN) execFileSync("cargo", ["build", "-q", "--bin", "forkdeck"], { cwd: root, stdio: "inherit" });
  if (!process.env.FORKDECK_AUTH_TEST_BIN) {
    const out = execFileSync("cargo", ["test", "-q", "--bin", "forkdeck", "--no-run", "--message-format=json"], { cwd: root, encoding: "utf8", maxBuffer: 1 << 28 });
    const artifact = out.split("\n").filter(Boolean).map(line => JSON.parse(line))
      .find(m => m.reason === "compiler-artifact" && m.target?.name === "forkdeck" && m.profile?.test && m.executable);
    if (!artifact) throw new Error("cargo did not report the forkdeck test binary");
    process.env.FORKDECK_AUTH_TEST_BIN = artifact.executable;
  }
}
