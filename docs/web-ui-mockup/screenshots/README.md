# Verified web UI captures

Captured from `forkdeck serve --demo` in Chromium after the one-use browser login:

- `board-1440x900.png` — `/jobs/72`, Worker tab selected.
- `plans-1440x900.png` — `/plans`, Tasks tab selected.
- `board-390x844.png` — `/`, responsive Active list.

All three match the corresponding approved Design Component markup rendered with
its default state and the same self-hosted IBM Plex fonts: **zero differing pixels**.
`comparison.json` records dimensions, pixel counts, and capture hashes. No visual
exceptions remain. Mutation controls are disabled while preserving their appearance.

Reproduce after `npm run build` in `web/` and `cargo build`:

```sh
node web/scripts/screenshots.mjs
```

The script starts and stops a demo daemon, checks login and browser interactions,
saves these screenshots, and renders `Main.dc.html`, `Plan.dc.html`, and
`Phone.dc.html` into `/tmp/forkdeck-reference-{Main,Plan,Phone}.png` for comparison.
It also writes computed text geometry into `/tmp` to diagnose fractional layout
differences. It uses no font CDN. `CHROMIUM_PATH` and `FORKDECK_BIN` can override
the browser and binary paths. Port 7420 must be free.
