# Web UI ↔ TUI Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Someone who uses the TUI daily finds the same information, actions and keys in the web UI, behaving the same way.

**Architecture:** The FlatBuffers board schema already carries the parity fields (commit `13bc194`; `web/schema/board.fbs`). The server fills them from the same helpers the TUI uses (factored out of `src/ui.rs`, never duplicated), adds a few detail fields and write behaviours, and the client renders them and adds the TUI's key bindings.

**Tech Stack:** Rust (axum, FlatBuffers), Preact + TypeScript, pnpm.

**Spec:** the TUI itself (`src/ui.rs`, `src/main.rs` key handling, `src/plan_ui.rs`), `docs/web-ui-server-mode-plan.md` §UI, `docs/web-ui.md`. The parity audit's gap matrix is summarised in the tasks below. Decisions already made by the person: the job detail stays a floating panel (not docked); "like the TUI" wins over the mockups where they differ (Plan first starts unchecked and resets after each submit); the plans page's left sidebar holds navigation only — the issue-plan list moves into the main area.

## Global Constraints

- Schema is append-only; `web/schema/board.fbs` is the contract. Regenerate with `scripts/schema.sh` (pinned flatc), commit generated code, `pnpm run schema:check` must pass.
- One source of truth: TUI words and states come from functions shared with `src/ui.rs` (`agent_word`, `router_word`, `prompt_title`, `job_lane`, `Plan::status`), not re-derived in the server or the browser.
- Hand-formatted Rust: never run `cargo fmt`/`rustfmt`. Real data only in `web/` (the `web_source` test). No CSP change. pnpm only, never npm.
- Gate before each commit, unpiped, each must exit 0: `cargo clippy --all-targets -- -D warnings`; `cargo test` (if it fails only under load, rerun with `RUST_TEST_THREADS=4` and say so); in `web/`: `pnpm run check`, `pnpm run build` (commit rebuilt `web/dist`); `node tests/web_wire.mjs`; `node tests/web_markdown.mjs`.

## Review Focus

1. **A key the browser owns.** Ctrl-T/Ctrl-N/Ctrl-W cannot be intercepted; Alt-←/→ may navigate history. Each TUI binding is either bound and tested, or documented in the help overlay with its browser substitute; none silently does the browser's thing *and* the app's.
2. **Keys while typing.** Shortcuts that are also text editing (Ctrl-A, Ctrl-U, Ctrl-X, Up/Down) act only where the TUI's equivalent would: Ctrl-A toggles intake only outside text fields; Ctrl-X stops only with no text selected; Up/Down recall history only at the first/last line of the composer.
3. **Planning jobs on the board.** Clicking one opens `/plans/<id>`, not an empty job panel; `/jobs/<plan id>` redirects there; `/jobs/<unknown>` says it is not on the board instead of showing another job.
4. **Log follow.** New output keeps the log pinned to the newest line only while the reader is at the bottom; scrolling up stops following and shows "↑ N lines back · Jump to latest"; End/Jump resumes.
5. **Phone.** Every view reachable on a phone has a way back (job detail especially).

---

## File ownership (parallel workers)

- **Server worker (Tasks 1–3):** `src/ui.rs` (extract helpers only), `src/server/{wire,view,write,operations}.rs`, `src/main.rs` (only to share `CARRY_ON` and the intake message), Rust tests.
- **Board worker (Task 4):** `web/src/{BoardWide,BoardPhone,PlansView,main,live,ui}.tsx/ts`, `web/src/style.css`, `tests/web_board_parity.mjs` (new), `tests/web_wire.mjs`.
- **Keys worker (Task 5):** `web/src/Actions.tsx`, `web/src/keys.ts` (new), `web/src/Help.tsx` (new), one mount line in `web/src/main.tsx` (`<Keys />` inside `App`), `tests/web_keys.mjs` (new).

### Task 1: Board snapshot and live frames (server)

**Files:** `src/ui.rs`, `src/server/wire.rs`, `src/server/view.rs`.

**Interfaces — Produces** (FlatBuffers, all fields already in the schema):
- `pub(crate) fn agent_word(st: &State, k: AgentKind) -> &'static str` and `pub(crate) fn router_word(st: &State) -> String` in `src/ui.rs`, extracted from the chip/router code at `ui.rs:343-363`; the TUI calls them.
- `Snapshot`: `agents.words` = three `AgentWord{agent:"claude"|"codex"|"copilot", word}`; `router.label` = `router_word`, `router.healthy` (0 unknown/1 ok/2 down from `st.router.healthy`), `fallbacks`, `remembered`, `endpoint` (`cfg.router.jev.base_url` only when kind ≠ "rules"); `running`/`queued` exactly as TUI `status()` (`ui.rs:803-806`); `spent_jobs`/`spent_tokens` from `st.spent`; `demo` = `cfg.demo`; `log` = the newest 200 session log lines (oldest first) as `LogEntry{time, who, text}`.
- Jobs: **planning jobs are included** (drop the `issue_plan.is_none()` filter in `wire::board`, `wire::event` and `view::snapshot`) and placed by `job_lane`; every `Job` gets `kind` ("manual"/"issue"/"pr"/"deploy", or "plan" when `issue_plan.is_some()`), `status` (native status name), `agent` (decision agent or ""), `queued` (`inbox.len()`).
- `Event::Log { lines }` becomes a `Log` frame (not `Changed`).

- [ ] **Step 1: Failing tests** in `wire.rs`/`view.rs`: seeded board has the plan job (id 100, kind "plan", lane per `job_lane`); words equal `ui::agent_word` for each agent and `router.label` equals `ui::router_word`; running/queued match a hand count; `log` holds the newest 200 lines oldest-first; `Event::Log` encodes as a `Log` frame with the same lines; job 73 (`inbox` non-empty after a queued reply) has `queued` = 1.
- [ ] **Step 2:** Run — fails. **Step 3:** Implement. **Step 4:** Pass. **Step 5:** Gate; commit `feat(server): the board carries what the TUI shows`.

### Task 2: Job detail and plan status (server)

**Files:** `src/server/view.rs`, `src/ui.rs`.

**Interfaces — Produces** (JSON on `GET /api/v1/jobs/{id}`, added beside existing fields):
- `prompt_title: string` — `pub(crate) fn prompt_title(job: &Job) -> String`, extracted from `ui.rs:783-789` (the `Some(j)` arms; the board is never the "new task" case).
- `source: {url: string, branch: string, workdir: string}` from the job (empty strings when absent).
- Board log turns tagged `"you"` (ask) and `"agent"` (answer) instead of `"queue"`/`"issues"` (`view.rs:140`); outcome lines stay `"queue"`.
- Plans: `status` from `Plan::status(st.automation)` (`planning.rs:133`) instead of `view::plan_state`.

- [ ] **Step 1: Failing tests:** `prompt_title` for a Choosing, Interrupted, running and finished job equals the TUI strings; detail of 72 has `source.url` = its issue URL; a job with one turn has board log entries `["", "you", ask]`, `["", "agent", answer]`; plan 100's status equals `Plan::status(true)`.
- [ ] **Steps 2–5:** fail, implement, pass, gate, commit `feat(server): job detail and plan status speak the TUI's words`.

### Task 3: Write behaviours (server)

**Files:** `src/server/write.rs`, `src/main.rs` (move `CARRY_ON` where both can use it).

**Interfaces — Produces:**
- `reply` with empty text: Interrupted → `hub.resume`; finished → `hub.reply(id, CARRY_ON)`; otherwise 400 "Enter a message" (TUI `submit_input`, `main.rs:1190-1202`).
- `choose` with both `option` and non-empty `text`: choose the option, then queue the text as a reply (TUI `main.rs:1289`).
- `automation` turned on with no `[[repos]]`: also logs `issues` / `no [[repos]] configured, nothing to poll` (TUI `main.rs:1424-1431`).
- `Command::LinkPlan { id: u32, workspace: String, issue: Option<String>, text: String, expected_revision: String }` (action `"link_plan"`) → `hub.link_plan` (`orchestrator.rs:643`), same revision and role checks as `plan`; 200 `{"id": <plan job id>}`.

- [ ] **Step 1: Failing tests** for each bullet (follow the existing `write.rs` test style). **Steps 2–5:** fail, implement, pass, gate, commit `feat(server): replies, choices, intake and plan linking behave as in the TUI`.

### Task 4: Board, detail and plans page (client)

**Interfaces — Consumes:** Task 1 schema fields (present now with defaults; real values after merge), Task 2 JSON fields (`prompt_title`, `source`, `"you"`/`"agent"` log tags; treat missing as empty so this branch works before the merge).

- [ ] **Step 1: Header and status line** (`BoardWide`, `BoardPhone`): agent chips use `agents.words` ("CLAUDE ON CALL" style, TUI colours: own colour for ON CALL, red NO ACCESS, blue VIA COPILOT, dim otherwise); router shows `router.label` with health/fallbacks/memory hits/endpoint in the side panel; a status line with running, queued, intake on/off, 24h jobs/tokens; "(demo)" next to the brand when `demo`.
- [ ] **Step 2: Cards:** plan jobs appear in their lane and link to `/plans/<id>`; a card shows ✉N when `queued` > 0.
- [ ] **Step 3: Routing:** `/jobs/<plan id>` redirects to `/plans/<id>`; `/jobs/<unknown>` shows "Job #N is not on the board" in the panel (never another job; fixes `BoardWide.tsx:62`).
- [ ] **Step 4: Detail panel:** source/PR links, branch and workdir under the title; routing gates from `detail.decision.gates` (name, value bar, sharp/split, answer) and `note` under the route line; per-stage rows (agent, model, effort, state, duration, tools, tokens in/out, cost) for worker and reviewer from `detail.worker`/`detail.reviewer`; an Outcome block with checks, review verdict and findings; the Conversation tab lists `"you"`/`"agent"` entries.
- [ ] **Step 5: Log follow** (Review Focus 4): pinned to newest while at bottom; "↑ N lines back · Jump to latest" when scrolled up; PgUp/PgDn scroll the log and End jumps to latest while the panel is open.
- [ ] **Step 6: Session log:** a collapsible "Session log" strip under the lanes showing `snapshot.log` plus `Log` frames (extend `live.ts` `apply` with `Body.Log`, capped at 200; add a `web_wire.mjs` test).
- [ ] **Step 7: Phone:** a visible "← Board" control in the job detail below 700 px; phone filter `aria-selected` compares ids (`BoardPhone.tsx:70`).
- [ ] **Step 8: Plans page:** the left sidebar holds navigation only; the issue-plan list moves into the main area above the selected plan (a compact list or selector, with status), on desktop and phone.
- [ ] **Step 9: Tests:** `tests/web_board_parity.mjs` against `forkdeck --demo serve` (port 17441): plan card → `/plans/100`; `/jobs/999` shows not-on-board; log follow and jump; phone back button; plans sidebar has no plan list; screenshots at 1440/960/390. Gate; commit `feat(web): board, detail and plans show what the TUI shows`.

### Task 5: Keys and composer (client)

**Interfaces — Consumes:** Task 2 `prompt_title`; Task 3 empty-reply, choose-with-text and `link_plan` behaviours (work after merge; until then the server answers 400 and the UI shows the error).

- [ ] **Step 1: Composer** (`Actions.tsx`): Plan first unchecked and reset after each successful submit; Ctrl-P and Shift-Tab toggle it while the composer has focus; Up/Down recall history (only at the first/last line), Esc and Ctrl-U clear; the draft and history are kept in `localStorage` per workspace (try/catch; works without storage).
- [ ] **Step 2: Reply box:** placeholder/label = `detail.prompt_title`; Enter queues (empty Enter allowed — the server resumes or carries on), Ctrl-S and Ctrl-Enter send now; choosing an option sends the typed draft along (`text`).
- [ ] **Step 3: Global keys** (`keys.ts`, mounted once in `App`): Ctrl-X stop the selected job (only with no text selection); Ctrl-A toggle intake (outside text fields); Alt-↑/↓ previous/next job, Alt-←/→ previous/next lane (newest job in that lane, like `ui.rs:262`); F2 open/close the detail panel; F4 → `/repos`; F5 focus the choices; F6 → `/plans`; `n` (outside text fields) focuses New task as the browser's stand-in for Ctrl-T/Ctrl-N; `?` opens the help overlay. Call `preventDefault` only for bindings the app handles.
- [ ] **Step 4: Help and status bar:** `Help.tsx` overlay listing every TUI key with its web equivalent, marking bindings the browser keeps (Ctrl-T/Ctrl-N/Ctrl-W and any that cannot be intercepted) and their substitutes; a one-line key hint bar at the bottom of the board like TUI `status()` (`ui.rs:815-819`), hidden below 700 px.
- [ ] **Step 5: Plan as issue:** a "Plan as issue…" control on finished typed tasks (repository, optional issue, request) sending `link_plan`, then routing to the plan.
- [ ] **Step 6: Tests:** `tests/web_keys.mjs` against `forkdeck --demo serve` (port 17442): each binding (Review Focus 1–2), Plan first reset, history recall, Ctrl-S send now, empty Enter on a finished typed task. Gate; commit `feat(web): the TUI's keys and composer in the browser`.
