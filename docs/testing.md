[Documentation](README.md) · [Front page](../README.md)

# Testing

## Commit gate

Run each command directly, without piping its output:

```sh
cargo clippy --all-targets -- -D warnings
cargo test
cd web
npm run check
npm run build
cd ..
```

Commit the rebuilt `web/dist` together with frontend changes. Rust stays hand-formatted.

## Documentation links

```sh
npm ci --prefix web
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

## Web captures

After building both frontend and binary, `node web/scripts/screenshots.mjs` starts
temporary servers on ephemeral ports and opens them in Chromium. It captures native
seeded jobs with `forkdeck serve --demo`, then regular `forkdeck serve` against a
private copy of `~/.local/state/forkdeck/forkdeck.db` and its WAL in a temporary
`XDG_STATE_HOME`. The real-history capture disables agent executables and issue
intake; original database hashes must remain unchanged. Both paths use the same API
views. Screenshots at 1440×900 and 390×844, including unverified job detail, are saved
under `docs/web-ui-mockup/screenshots/`. The script also checks that reply controls
stay inside the viewport. Set `CHROMIUM_PATH` if Chromium is installed elsewhere.
It renders the approved mockups with the same local fonts into `/tmp`; run
`python3 web/scripts/compare-screenshots.py` (Pillow required) to refresh the measured
comparison. See [capture notes](web-ui-mockup/screenshots/README.md) for the
remaining content differences from the references.

See [operational screen verification](web-ui-operations.md) for Markdown and
browser checks, and [development status](development.md#status) for live-integration gaps.
