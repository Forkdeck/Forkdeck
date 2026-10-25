[Documentation](README.md) · [Front page](../README.md)

# Testing

## Commit gate

Run each command directly, without piping its output:

```sh
cargo clippy --all-targets -- -D warnings
cargo test
cd web
pnpm run check
pnpm run build
pnpm run e2e
cd ..
```

Commit the rebuilt `web/dist` together with frontend changes. Rust stays hand-formatted.

## Documentation links

```sh
pnpm install --frozen-lockfile --dir web
node --test tests/doc_links.mjs
node scripts/check-doc-links.mjs
```

The checker walks `README.md` and every Markdown file under `docs/`, checks local
files and directories, and validates heading and explicit HTML anchors. It uses
Markdown parsing so fenced examples do not count as links. External URLs are
left alone. CI runs the same checks.

## Issue planning

Offline acceptance tests use fake agents and forge CLIs:

```sh
cargo test
python3 tests/tui_issue_plans.py target/debug/forkdeck
python3 tests/tui_exit.py target/debug/forkdeck
```

## Browser end-to-end tests

The Playwright suite in `web/e2e/` clicks through every web feature against real
daemons. Each test starts its own `forkdeck serve` on an ephemeral port with a
private `HOME`, `XDG_STATE_HOME` and `PATH`, so tests run in parallel and never touch
your state or installed CLIs.

```sh
pnpm --dir web exec playwright install chromium   # once
pnpm --dir web run e2e
```

`e2e` rebuilds the bundle; global setup then runs `cargo build` (the daemon embeds
`web/dist`) and builds the auth test binary for the OIDC fixture. Run one file or
test with `pnpm --dir web exec playwright test plans -g Refine`. Set
`FORKDECK_CHROMIUM` to use another Chromium, or `FORKDECK_BIN` to skip the build.

What runs where:

- `--demo` daemons cover the board, detail, every write (submit, plan-first choices,
  queue, send now, stop, resume, retry, carry on, plan as issue, intake), live
  updates across browsers and daemon restarts, keys, plans, the operational pages,
  phone layout and wide layouts. Simulated agents finish a task in about ten seconds.
- Normal daemons cover the empty board and plans, and the repository picker against
  a loopback GitHub and GitLab with a stand-in `gh`/`glab`.
- The ignored `screenshot_provider` Rust test hosts OIDC behind a loopback HTTPS
  proxy for sign-in, sign-out, viewer and admin roles and cross-origin writes.

Every test fails on a script error, CSP refusal or unexpected console error. Failed
tests keep a trace, screenshot and the daemon's log under `web/test-results/`.
CI runs the suite and fails when the committed `web/dist` differs from a rebuild.

## Web captures

After building both frontend and binary, `node web/scripts/screenshots.mjs` starts
temporary servers on ephemeral ports and opens them in Chromium. It captures native
seeded jobs with `forkdeck serve --demo`, then regular `forkdeck serve` against a
private copy of `~/.local/state/forkdeck/forkdeck.db` and its WAL in a temporary
`XDG_STATE_HOME`. The real-history capture disables agent executables and issue
intake; original database hashes must remain unchanged. Both paths use the same API
views. Captures include board/selected job, unverified detail, every plan tab,
repositories, deployments, stats and doctor at 1440×900; board, detail and all plan
tabs also run at 390×844. Screenshots are saved under `docs/screenshots/`.
The OIDC captures use the existing Rust test fake provider, an isolated session
database and a temporary loopback HTTPS proxy: sign-in landing page, then an actual
provider sign-in and the signed-in header on desktop and phone. Default local mode
still has no login. The fixture is an ignored Rust test started by the script.

The script checks document overflow, reply control visibility, browser errors and
original file hashes. Real history has no configured repositories in this isolated
capture config; its repository/deployment empty states are captured as observed.
Missing plans remain empty rather than receiving demo content. Journal, deployment
state and metrics attempts are also copied when present so real stats use real records.
Set `CHROMIUM_PATH` or `FORKDECK_BIN` to override executables. The script needs Cargo
and OpenSSL for the OIDC fixture and temporary certificate.

It renders the approved mockups with the same local fonts into `/tmp`; run
`python3 web/scripts/compare-screenshots.py` (Pillow required) to refresh the measured
comparison. See the [complete capture gallery](screenshots/README.md).

See [operational screen verification](web-ui-operations.md) for Markdown and
browser checks, and [development status](development.md#status) for live-integration gaps.
