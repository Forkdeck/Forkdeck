[Documentation](README.md) · [Front page](../README.md)

# Development

Forkdeck is a Rust daemon and terminal interface with a Preact frontend in `web/`.
The browser and paired phone read the same Hub and event feed.

Rust is hand-formatted here. Do not run `cargo fmt` or `rustfmt`.

## Web development

Build the frontend before compiling Rust after UI changes:

```sh
cd web
pnpm install --frozen-lockfile
pnpm run check
pnpm run build
cd ..
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
```

`web/dist` is checked in so a Rust-only checkout builds without Node. Both debug and
release binaries embed the built assets, including local IBM Plex Sans/Mono fonts;
hashed assets receive immutable caching. Commit rebuilt `web/dist` with source changes.
No font CDN is used at runtime. `pnpm run contracts` in `web/` regenerates TypeScript
contracts from `crates/remote-proto`; the Rust protocol tests check for drift.

For frontend development, run `forkdeck serve --demo`, then `cd web && pnpm run dev` and
open http://localhost:5173. Vite proxies `/api` to the daemon on 7420; HMR and frontend
source stay on Vite.

## Releases

A release is cut by pushing a tag `vX.Y.Z` that matches the version in
`Cargo.toml`; `.github/workflows/release.yaml` tests, builds, writes the SBOMs
and publishes the release.

See [testing](testing.md) for the commit gate, browser checks and capture commands.

## Status

Early. The MCP client has been run against the reference MCP server over both
transports, and the CI and deploy stages are tested against a stand-in server;
no stage has been run against a real CI or deployment system yet. The claude
path has been run against the real CLI. The GitHub flow, the
codex stream parser and the Jev wire format are tested against stand-ins that
follow the documented formats, and the `glab` commands have not been run
against a live GitLab yet. The team feature is tested against a stand-in for
`gh api`; its `gh` calls follow the documented REST endpoints and have not
been run by two real machines against one repository. Expect to adjust
`agents.codex` arguments for your codex version. The copilot path is tested
against the real `copilot` binary talking to a stand-in model server (its
bring-your-own-key mode; `cargo test --test copilot_byok -- --ignored`), and
its login and model probe against a real account without a Copilot seat. It
has not run against Copilot's own models yet.

Not there yet: feeding review comments on an open pull request back to the
worker for another round. Interrupted typed tasks already resume as described above.

## License

MIT
