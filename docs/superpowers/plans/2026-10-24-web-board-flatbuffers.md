# Board over FlatBuffers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The browser board stops using JSON: its snapshot and live updates arrive as FlatBuffers over a WebSocket and the board reads fields straight from the received bytes (zero copy).

**Architecture:** One schema, `web/schema/board.fbs`, generates Rust and TypeScript code with a pinned `flatc`. The server encodes the board snapshot (`GET /api/v1/snapshot`, which stops serving JSON) and every feed event (`GET /api/v1/live`, WebSocket, binary frames) as a `Frame`. The client keeps the received `Snapshot`/`Job` tables and renders from their accessors; it never converts them to plain objects.

**Tech Stack:** Rust (axum 0.8 with `ws`, `flatbuffers` crate), Preact + TypeScript (`flatbuffers` npm), `flatc` 25.9.23.

**Spec:** this conversation's decision, summarised: avoid JSON for the board; JSON.parse and Fory are out (Fory's JS port builds codecs with `new Function`, which `script-src 'self'` blocks); FlatBuffers chosen for zero-copy reads and no `eval`. Scope is the board only; job detail, plans and the operational pages stay JSON for now.

## Global Constraints

- `flatc` 25.9.23, Rust crate `flatbuffers = "=25.9.23"`, npm `flatbuffers` exactly `25.9.23`. All three match.
- No `eval`/`new Function` anywhere in the shipped bundle; the Content-Security-Policy in `src/server/http.rs` does not change.
- Generated code is committed: Rust in `src/server/wire/board_generated.rs`, TypeScript under `web/src/wire/`. Nothing in `cargo build`, `cargo test` or `npm run build` needs `flatc`.
- Hand-formatted Rust: never run `cargo fmt` or `rustfmt`. Match the surrounding code's density and naming.
- Real data only in `web/`; no fixture or sample content (an existing test fails on that). Demo content comes from `src/server/seed.rs` through real Hub structures.
- The UI has no login; do not add one.
- Gate before each commit, unpiped, each must exit 0: `cargo clippy --all-targets -- -D warnings`, `cargo test`, and in `web/`: `npm run check`, `npm run build` (commit the rebuilt `web/dist`).
- Commit on the current branch (`fix/web-plan-default`); never push.

## Review Focus

1. **Cross-site WebSocket.** A page on another site can open `ws://127.0.0.1:7420/api/v1/live`; its Host header is the loopback name, so the existing Host check passes. The upgrade must be refused (403) when an `Origin` header is present and its host:port differs from the request's Host. No `Origin` (curl, tests) is allowed. Test in Task 3.
2. **Daemon restarted under an open tab.** The client resumes with a `since` above the new daemon's seq; `Feed::subscribe` already answers `Resync`. The client must then reload `/api/v1/snapshot` and resubscribe from the new seq, not loop. Test in Task 4 (reducer) and Task 3 (server sends Resync).
3. **Slow tab / lagged subscriber.** `RecvError::Lagged` must become a `Resync` frame, never a closed socket with no frame. Test in Task 3.
4. **A job the snapshot has not seen.** A `JobChanged` for an unknown id must add the job to the board in its lane, not be dropped. Test in Task 4.
5. **Non-ASCII and long text.** Titles and log lines with `—`, `·`, emoji, and a 500-line log must survive encode and decode unchanged. Test in Task 2.

---

## File Structure

- `scripts/flatc.sh` — downloads the pinned `flatc` into `.local/flatc/` (already git-ignored), verifies its sha256, prints its path.
- `web/schema/board.fbs` — the only schema.
- `src/server/wire/board_generated.rs` (generated), `src/server/wire.rs` — encoding the board and feed events into `Frame` bytes.
- `src/server/http.rs` — `GET /api/v1/snapshot` (now FlatBuffers), `GET /api/v1/live` (WebSocket); the SSE route `/api/v1/events` is removed.
- `web/src/wire/` (generated), `web/src/live.ts` — decoding, the board state signal, the WebSocket connection and the reducer.
- `web/src/state.ts`, `web/src/main.tsx`, `web/src/BoardWide.tsx`, `web/src/BoardPhone.tsx`, `web/src/ui.ts` — read the board from `live.ts` instead of JSON.
- `tests/web_wire.mjs` — reducer and decoding tests (Node 24 runs `.ts` imports directly, like `tests/web_markdown.mjs`).

### Task 1: Toolchain, schema and generated code

**Files:**
- Create: `scripts/flatc.sh`, `web/schema/board.fbs`, `src/server/wire.rs`, `src/server/wire/board_generated.rs` (generated), `web/src/wire/**` (generated)
- Modify: `Cargo.toml` (add `flatbuffers = "=25.9.23"`; axum features add `"ws"`), `src/server/mod.rs` (`mod wire;`), `web/package.json` (dependency `"flatbuffers": "25.9.23"`; scripts `"schema"` and `"schema:check"`), `web/package-lock.json`

**Interfaces:**
- Produces: schema types in namespace `forkdeck.web`, exactly:

```
table LogEntry { time: string; who: string; text: string; }
table Job { id: uint; title: string; lane: string; state: string; meta: string;
            elapsed: string; route: string; stages: [ubyte]; log: [LogEntry]; warn: string; }
table Lane { id: string; label: string; }
table Agents { claude: bool; codex: bool; copilot: bool; }
table Router { kind: string; calls: ulong; last_ms: ulong; }
table Snapshot { automation: bool; workspace: string; capacity: uint; busy: uint;
                 router: Router; agents: Agents; lanes: [Lane]; jobs: [Job]; }
table JobChanged { job: Job; }
table Tail { id: uint; worker: bool; from: ulong; lines: [string]; }
table Automation { on: bool; }
table Resync {}
table Changed {}
union Body { Snapshot, JobChanged, Tail, Automation, Resync, Changed }
table Frame { seq: ulong; body: Body; }
root_type Frame;
```

  `Job` carries exactly the fields the board renders today (`BoardJob` minus what no component reads). New fields go at the end of a table only; never reorder or remove.
- Produces: `npm run schema` (in `web/`) runs `../scripts/flatc.sh`, then `flatc --rust` into `src/server/wire/` and `flatc --ts` into `web/src/wire/`. `npm run schema:check` regenerates into a temp dir and fails if it differs from the committed files. Generated TS imports must resolve both under Vite and under Node 24's type stripping (pick the `flatc` TS options that make that true and record them in the script).

- [ ] **Step 1:** Write `scripts/flatc.sh`: download the official `flatc` v25.9.23 Linux release asset from github.com/google/flatbuffers, unzip into `.local/flatc/`, check a sha256 pinned in the script (record it from the first verified download), skip the download when the binary is present and matches. Run it; `flatc --version` prints `flatc version 25.9.23`.
- [ ] **Step 2:** Write `web/schema/board.fbs` as above, add the scripts and dependencies, run `npm install` in `web/` (the worktree has its own `node_modules`; `web/node_modules` must be a real directory, not a symlink), run `npm run schema`.
- [ ] **Step 3: Write the failing test** `wire::tests::a_frame_round_trips` in `src/server/wire.rs`: build a `Frame { seq: 7, body: Snapshot { workspace: "home", lanes: [Lane { id: "Queue", label: "Backlog" }], jobs: [Job { id: 72, title: "Home record API — sync ✓", stages: [2,1,0,0,0], log: [LogEntry { time: "09:04:02", who: "worker", text: "Adding the 404 path…" }] }] } }` with the generated builders, read it back with the generated root accessor, assert every value above.
- [ ] **Step 4:** Run `cargo test wire::` — fails until `mod wire;` and the generated module are wired in; make it compile and pass.
- [ ] **Step 5:** Run `npm run schema:check` (passes), the full gate, commit `feat(web): FlatBuffers schema and toolchain for the board`.

### Task 2: Encode the board and feed events

**Files:**
- Modify: `src/server/wire.rs`, `src/server/view.rs` (make `board` and `LANES` `pub(super)`), `src/server/http.rs` (the `/v1/snapshot` handler serves FlatBuffers)

**Interfaces:**
- Consumes: Task 1 generated types; `view::board(&Job, &State) -> BoardProjection`; `Feed::snapshot(&Hub).seq`; `hub.cfg.web.lanes.in_order()`.
- Produces:
  - `pub fn board(hub: &Hub, feed: &Feed) -> Vec<u8>` — a `Frame` whose `seq` is the feed snapshot's seq and whose body is `Snapshot`, built from the same data as `view::snapshot` (same jobs in the same order, same filter on `issue_plan`, same `waiting`/`blocked by` override, same `agents`, `workspace`, `capacity`, `busy`, `router`, `lanes`). Factor the shared job projection out of `view::snapshot` rather than duplicating it.
  - `pub fn event(hub: &Hub, seq: u64, event: &forkdeck_remote_proto::Event) -> Vec<u8>` — `Job { job }` becomes `JobChanged` with the full projection of that job looked up in hub state (a job no longer in state, or one with `issue_plan`, becomes `Changed`); `Tail` maps field for field (`worker` = `which == Which::Worker`); `Automation`, `Resync` map directly; `Stage` and `Log` become `Changed`.
  - `GET /api/v1/snapshot` → 200, body `wire::board(..)`; the JSON handler is removed, while `view::snapshot` stays as an internal function (`view::detail` and tests use it); `content-type: application/octet-stream`, `cache-control: no-store` (the existing `no_store` layer).

- [ ] **Step 1: Write the failing tests** in `src/server/wire.rs`, seeding like `view::tests::seeded_jobs_use_the_real_projection_without_a_mode_flag`:
  - `the_board_frame_matches_the_json_snapshot`: decode `board(&hub, &feed)`; for every job, `id`, `title`, `lane`, `state`, `meta`, `elapsed`, `route`, `stages`, `warn` and every log entry equal the corresponding values in `view::snapshot(&hub, &feed)["jobs"]`; job 72 has lane `"Build"`, state `"working"`, stages `[2,1,0,0,0]`; job 74 has state `"waiting"` and meta `"blocked by #22 · not merged"`; `lanes` equals the six configured `{id,label}` pairs; frame seq equals the snapshot's `seq`.
  - `events_become_frames`: one assertion per `Event` variant as specified in Produces, including `Job` for an unknown id → `Changed`.
  - `text_survives_encoding` (Review Focus 5): set a job title to `"Plans — sync · 🚀"` and push 500 worker tail lines; the decoded title is identical and the decoded log holds all of them in order.
- [ ] **Step 2:** Run `cargo test wire::` — fails (functions missing).
- [ ] **Step 3:** Implement `board` and `event`, add the route.
- [ ] **Step 4:** Change the `http.rs` tests that read `/api/v1/snapshot` as JSON to decode it instead: 200, `application/octet-stream`, `no-store`, and a `Snapshot` whose jobs equal `view::snapshot(..)["jobs"]` by id, lane and state. Run `cargo test` — passes.
- [ ] **Step 5:** Gate, commit `feat(server): encode the board and feed events as FlatBuffers`.

### Task 3: WebSocket live feed

**Files:**
- Modify: `src/server/http.rs` (route `GET /v1/live`, remove `/v1/events` and its `Cursor`/SSE code), `Cargo.toml` (dev-dependency `tokio-tungstenite`, version compatible with axum 0.8's)

**Interfaces:**
- Consumes: `wire::event`, `Feed::subscribe(since) -> (Vec<Numbered>, broadcast::Receiver<Numbered>)`.
- Produces: `GET /api/v1/live?since=<u64>` upgrades to a WebSocket. The server sends the backlog then live events, each as one binary message `wire::event(..)`; `RecvError::Lagged` sends a `Resync` frame and continues; `Closed` closes the socket. A WebSocket Ping every 10 s replaces the SSE heartbeat. Text or binary messages from the client are ignored (commands come in a later milestone). Upgrade refused with 403 when `Origin` is present and its authority (host[:port]) differs from the request's `Host` header; also subject to the existing `same_machine` Host check.

- [ ] **Step 1: Write the failing tests**, replacing `snapshot_and_sse_resume_without_a_login` with `snapshot_and_live_resume_without_a_login`:
  - connect to `/api/v1/live?since=<board seq>` with no Origin; change a job through the hub (as the existing test does); the next binary message decodes to a `Frame` with seq > since and body `JobChanged` for that job.
  - `since=1` after several events → backlog frames arrive in seq order, all > 1.
  - `since` greater than the current seq → first frame is `Resync` (Review Focus 2).
  - Origin `http://evil.test` → handshake fails with 403; Origin `http://127.0.0.1:<port>` → accepted (Review Focus 1).
  - a lagged receiver yields a `Resync` frame (Review Focus 3): subscribe, push more events than the broadcast capacity without reading, then read.
- [ ] **Step 2:** Run `cargo test http::` — fails.
- [ ] **Step 3:** Implement the route and the Origin check; delete the SSE route and code.
- [ ] **Step 4:** `cargo test` passes.
- [ ] **Step 5:** Gate, commit `feat(server): live board updates over a WebSocket`.

### Task 4: Client reads the board from FlatBuffers

**Files:**
- Create: `web/src/live.ts`, `tests/web_wire.mjs`
- Modify: `web/src/state.ts` (remove the EventSource, `reduce`, and the JSON snapshot fetch; keep `details`, `plans`, `selectJob` and their JSON fetches), `web/src/main.tsx`, `web/src/BoardWide.tsx`, `web/src/BoardPhone.tsx`, `web/src/ui.ts`, `tests/web_pages.mjs`, `docs/web-ui-operations.md`

**Interfaces:**
- Consumes: generated `Frame`, `Snapshot`, `Job`, `Body`, … from `web/src/wire/`.
- Produces in `web/src/live.ts`:
  - `export interface Board { seq: number; snapshot: Snapshot; jobs: Job[] }` — `jobs` is board order: snapshot jobs with `JobChanged` replacements applied in place and unknown ids appended. Tables are kept, never copied into objects.
  - `export const board = signal<Board | null>(null)`.
  - `export function apply(current: Board | null, frame: Frame): { next: Board | null; resync: boolean; tail?: { id: number; worker: boolean; from: number; lines: string[] } }` — pure reducer: `Snapshot` replaces the board; `JobChanged` replaces/appends; `Automation` is applied by keeping a separate `automation` override field on `Board` (add `automation: boolean` to `Board`, initialised from the snapshot); `Resync` → `resync: true`; `Tail` → returned for `state.ts` to merge into `details` as today; `Changed` → no change (the caller schedules the debounced refresh as now).
  - `export function connect(): () => void` — `fetch('/api/v1/snapshot')` as `arrayBuffer`, decode, set `board`, then open `new WebSocket(<ws or wss of location>/api/v1/live?since=<seq>)` with `binaryType = 'arraybuffer'`; on message decode and `apply`; on `resync` reload the board and reconnect from its seq; on close reconnect with backoff 1 s doubling to 10 s, resuming from the last seq seen; sets `connection` to `"Live"`, `"Reconnecting"`, `"Disconnected"` as the EventSource did; the debounced refresh of `plans` and the selected job's detail stays (100 ms) and is triggered by every frame except `Snapshot`.
- Components read accessors directly: `job.title()`, `job.lane()`, `job.stagesArray()`, `job.log(i)`, `snapshot.lanes(i).label()`, etc. `PresentedJob`/`present()` become a thin view over a `Job` table (the phone `group` is computed from `job.lane()`); no component receives plain-object copies of board jobs. The job detail page still takes its data from `details` (JSON) as today.

- [ ] **Step 1: Write the failing tests** in `tests/web_wire.mjs` (import `../web/src/live.ts` and the generated builders): build frames with the generated builders and assert:
  - `Snapshot` then `JobChanged` for job 72 with state `"done"` → `next.jobs` has job 72 with `state() === "done"` at the same index.
  - `JobChanged` for unknown id 900 with lane `"Queue"` → appended; still present after a following `Automation` frame (Review Focus 4).
  - `Resync` → `resync === true`, board unchanged.
  - `Tail` → returned with the same id/worker/from/lines.
  - `Automation { on: false }` → `next.automation === false`.
- [ ] **Step 2:** Run `node tests/web_wire.mjs` — fails (module missing).
- [ ] **Step 3:** Implement `live.ts`, switch `main.tsx` and both boards to it, trim `state.ts`.
- [ ] **Step 4:** `node tests/web_wire.mjs` passes; extend `tests/web_pages.mjs`: on the board page record every request URL; assert none matches `/api/v1/events`, that `/api/v1/snapshot` answered `application/octet-stream`, and that a WebSocket to `/api/v1/live` were made; existing 1440/390 checks still pass against `forkdeck --demo serve`.
- [ ] **Step 5:** Update `docs/web-ui-operations.md` (API list: `/api/v1/snapshot` is FlatBuffers, `/api/v1/live`; SSE gone; the schema, `npm run schema`, `npm run schema:check`). Gate plus `node tests/web_markdown.mjs`, `node tests/web_wire.mjs`, `tests/web_pages.mjs` against a demo server; commit `feat(web): the board reads FlatBuffers over a WebSocket`.
