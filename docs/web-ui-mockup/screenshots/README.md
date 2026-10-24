# Web UI captures

All captures use Chromium, the embedded production bundle, default local access
without a browser login, and the real `/api/v1` endpoints. There are no browser fixtures or response
interceptors. Reproduce with `npm run build --prefix web`, `cargo build`, then
`node web/scripts/screenshots.mjs` (`CHROMIUM_PATH` and `FORKDECK_BIN` override the
executables). After capture, `python3 web/scripts/compare-screenshots.py`
(requires Pillow) refreshes the comparison measurements.

## Seeded Hub

`forkdeck serve --demo` creates native jobs, decisions, stages, checks, outcomes,
plans and a task graph. Its scheduler observation is calculated with
`schedule::status()` from seeded forge facts. The graph passes the same rendered
checksum round trip as a scheduler observation. The browser uses the same views
as a regular server.

- `board-1440x900.png` — working job #72, Worker tab.
- `plans-1440x900.png` — Tasks tab, scheduler observation and decision #25.
- `board-390x844.png` — the responsive Active filter.
- `unverified-1440x900.png` — finished job #62 without checks.

## Real history

The script copies `~/.local/state/forkdeck/forkdeck.db` **and its WAL** into a
private temporary `XDG_STATE_HOME`, then starts **`forkdeck serve` without
`--demo`**. This capture restores 69 real jobs; selected job #70 is finished with
no recorded checks. It neither edits the copied job records to create a desired
state nor writes to the original database. SHA-256 checks verify that the original
database and WAL remain unchanged after capture.

The temporary config disables issue intake, native remote transport and agent
executables, preventing restored work from starting real agents. Agent availability
and slot capacity shown in these captures reflect that config. The temporary
workspace, config and database copy are deleted after the server stops.

- `real-board-1440x900.png` — real lanes, state pills, job #70 detail and warning.
- `real-board-390x844.png` — all real jobs, with scrolling and full-size cards.
- `real-detail-390x844.png` — job #70 detail and unverified warning on a phone.

## Differences from the reference

The colours, fonts, spacing, radii, controls and six-lane design use the approved
reference styles, now expressed as CSS classes. These are **not pixel-identical
content comparisons**. `comparison.json` records the actual reference differences;
the previous zero-difference claim no longer applies.

- Workspace name/initial, agent availability, intake, busy slots and capacity come
  from the server. The Agents box additionally shows actual session router stats.
- Elapsed times and observation age advance. Native seeded stage lines retain the
  reference timestamps; old real history has no per-line timestamps, so those
  timestamps remain blank rather than being invented.
- The graph makes sync task #24 depend on #22. Its queue card therefore says #22,
  correcting the board reference's inconsistent #23. Effective dependencies also
  show inherited #26 where applicable.
- The actual graph has **0 ready**, 1 working, 2 waiting and 1 decision needing an
  answer. The reference's extra ready task is not fabricated. The scheduler's
  checksum claim comes from the validated observation.
- The native tier is `heavy`, rather than the reference's unsupported `deep`.
  GitHub pull requests use `#71`, rather than the reference's `!71`. Route text
  includes its recorded source, latency and reviewer when known.
- Plan headings show the current plan state (`scheduling` in this capture);
  draft/review tabs render API text in the styled prose card, instead of separate
  hard-coded reference articles. The decision input has a generic placeholder.
- Phone filters include every API job matching that filter. Active contains the
  five active seeded jobs, rather than the reference's fixed seven-job list that
  included Attention and Done jobs. Cards use the same meta as desktop.
- Large real histories scroll within lanes and the phone list. Selected desktop
  cards scroll into view, and the detail log scrolls while the reply bar remains
  visible. These behaviours preserve the design with more than the sample jobs.

Mutation controls remain disabled as in the original read-only milestone.
