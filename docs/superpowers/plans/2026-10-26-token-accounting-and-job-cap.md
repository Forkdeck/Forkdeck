# Token Accounting and Per-Job Token Cap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record cache reads and writes separately in the journal so limits and `forkdeck stats` can tell cached input from new input, put every run on the job's account even when it is stopped, and add `limits.tokens_per_job`, which stops an issue or review that spends too much.

**Architecture:** The per-attempt ledger (`metrics::Stream` → `metrics::Tokens`) already splits cache reads and writes for Claude and Codex. The journal (`journal::Usage`), the daily limit and `forkdeck stats` still merge them into `tokens_in`. Today a run is added to `j.used` only when `agents::run` returns, so a run whose future is dropped (a person's cancel, `job_mins`, shutdown) is never counted. This plan gives each run a `Meter`: a drop guard that keeps the run's live usage in `Job.running` while it works and moves it to `j.used` exactly once when it ends or is dropped. The per-job cap sums `j.used` and every entry in `j.running` (pull-request reviewers run at the same time) and stops the job through one stop-reason field shared with `job_mins`.

**Tech Stack:** Rust (tokio, serde), shell stand-in CLIs in `tests/automation.rs`.

**Spec:** No separate spec. The requirements come from the 2026-10-26 token-usage audit (items 1 and 2) and its review by gpt-6.1-sol, and are stated in full under Global Constraints. Read `docs/limits.md` and `docs/metrics-and-stats.md` for current behaviour.

## Global Constraints

- Rust is hand-formatted. Never run `cargo fmt` or rustfmt.
- Gate before every commit, run directly, never piped: `cargo clippy --all-targets -- -D warnings`, then `cargo test`, then `node scripts/check-doc-links.mjs` (run `pnpm install --frozen-lockfile` in `web/` first in a fresh worktree).
- `cargo test` takes one name filter. Run each filter below as its own command.
- `journal::Usage.tokens_in` keeps its meaning: all input tokens, cached ones included. Old journal lines, saved history and the web/remote views must read unchanged.
- New `journal::Usage` fields: `cache_read: u64`, `cache_write: u64`, `cache_known_in: u64` (the `tokens_in` of runs that reported cache reads). All `#[serde(default)]` through the struct's existing `#[serde(default)]`. Every struct literal of `Usage` must be updated, including `src/server/seed.rs:239`; the compiler lists them.
- Per vendor, `tokens_in` is: Claude `input_tokens + cache_read_input_tokens + cache_creation_input_tokens` (as today); Codex `input_tokens` (its `cached_input_tokens` is a subset, never added); Copilot the sum of `inputTokens` (it already includes `cacheReadTokens` and `cacheWriteTokens`, which today's `read_usage` adds a second time).
- New config `limits.cache_read_weight: f64`, default `1.0`, valid range `0.0..=1.0`; anything else fails `Config::validate` with `limits.cache_read_weight must be between 0 and 1`.
- Weighted tokens = `(tokens_in - cache_read) + round(cache_read × cache_read_weight) + tokens_out`. Cache writes count in full.
- `limits.tokens_per_day` and `limits.tokens_per_job` both count weighted tokens.
- New config `limits.tokens_per_job: u64`, default `0` (off). Like `job_mins`, it applies to issues and pull-request reviews only, never to typed tasks or deployments, is ignored in demo mode, and is never checked for the knowledge run (`Slot::Knowledge`), which runs after the outcome is settled. The knowledge run's tokens still go on the account.
- `Job.over_time: bool` is replaced by `Job.stopped: Option<state::Limit>`, `enum Limit { Time, Tokens(u64) }`. A limit names the stop only if it wins the job's cancel token (`Cancel::claim`, one compare-and-swap); a later limit, a person's cancel or a shutdown that got there first leaves it `None`.
- Every run is on the account before the job's journal line is written: when the job's future ends, `Job::settle` moves whatever is still in `j.running` (reviewer tasks aborted with their `JoinSet` drop later) into `j.used`; a meter dropped after that adds nothing.
- A job stopped by the cap: agent killed, status Failed, failure label (`agent:needs-human` for issues, `pr_fail_label`, default `review:changes`, for reviews), no retry scheduled. Outcome text exactly: `stopped after {spent} tokens, the limit for one job (limits.tokens_per_job). It was {state} at the time` (`{spent}` via `journal::kilo`; `{state}` as for `job_mins`). A job that reaches Done before the stop takes effect stays Done, as with `job_mins`.
- Log line when it fires: `#{id} stopped: it has used {spent} tokens (limits.tokens_per_job)` on the `limits` channel.
- `forkdeck stats` CLI line: `claude      71 run(s) · 9.8M tokens in, 83% from cache · 412.0k out` (`, NN% from cache` only when `cache_known_in > 0`; the percentage is `cache_read * 100 / cache_known_in`, rounded down).
- Out of scope: adaptive routing's `Objective::Tokens`, the reviewer-tier knob, and UI or web display of cache figures.

## Review Focus

1. **Pull-request reviewers run at once** (`src/orchestrator.rs:2539`). The cap sums every running meter, not just the one that saw the line. Test: `concurrent_reviewers_are_stopped_by_their_combined_spend` (Task 3).
2. **A run that is stopped from outside is dropped mid-await, possibly late** (an aborted `JoinSet` task). Its meter's `Drop` or `Job::settle`, whichever comes first, puts it on the account, once. Tests: `a_run_is_on_the_account_once_whether_it_ends_or_is_dropped`, `a_dropped_run_future_is_on_the_account`, `settle_counts_runs_still_going_once` (Task 2) and the journal assertion added to `a_job_over_its_time_limit_is_stopped_and_handed_to_a_person` (Task 2).
3. **Copilot reports only through its usage file, after it exits; Codex only at turn end.** The check runs again when the file is read, and a run never starts once the job is cancelled. Tests: `a_copilot_run_over_the_cap_stops_the_job_before_the_review`, `a_codex_run_over_the_cap_stops_the_job_after_it_reports` (Task 3).
4. **Two stop reasons racing** (`job_mins` timer and the cap, or a person's cancel and the cap, which fire outside the state lock). Test: `only_the_first_stop_names_the_reason` (Task 3).
5. **Exempt jobs** (typed tasks, deployments, demo mode, the knowledge run) must never be stopped. Tests: `the_cap_leaves_typed_tasks_deployments_knowledge_and_demo_alone` (unit) and `a_typed_task_is_not_stopped_by_the_job_cap` (Task 3).

---

### Task 1: Cache figures in `Usage`, weighted limits, stats and Copilot's count

**Files:**
- Modify: `src/journal.rs:16-36` (`Usage`), `src/journal.rs:228-249` (`Spent`, `spent`), `src/journal.rs:~425-430` (stats CLI line)
- Modify: `src/agents.rs:323-335` (`read_usage`), `src/agents.rs:~868-877` (its caller)
- Modify: `src/server/seed.rs:239` and any other `Usage { .. }` literal the compiler finds
- Modify: `src/config.rs:~549-559` (`LimitsCfg.cache_read_weight`), `src/config.rs:~1158` (validation)
- Modify: `src/issues.rs:401-427` (daily budget uses weighted tokens)
- Modify: `src/main.rs:~1005-1015` (doctor limits line: append ` · cache reads count {w}` when `w != 1.0`)
- Modify: `docs/limits.md`, `docs/metrics-and-stats.md`, `docs/AGENTS.md` (Copilot paragraph)
- Test: `src/journal.rs`, `src/agents.rs`, `src/config.rs` test modules

**Interfaces:**
- Produces:
  - `journal::Usage { runs, tokens_in, tokens_out, cost, cache_read: u64, cache_write: u64, cache_known_in: u64 }`; `Usage::add` sums all seven.
  - `impl Usage { pub fn weighted(&self, cache_read_weight: f64) -> u64; pub fn cache_share(&self) -> Option<u32> }`; `cache_share` is `None` when `cache_known_in == 0`.
  - `pub fn spent(entries: &[Entry], repo: Option<&str>, now: i64, cache_read_weight: f64) -> Spent` (new last parameter; `Spent.tokens` is weighted).
  - `#[derive(Debug, Default, PartialEq)] pub struct agents::CopilotUsage { pub input: u64, pub output: u64, pub cache_read: u64, pub cache_write: u64, pub cache_known: bool, pub models: Vec<String> }`; `pub fn read_usage(path: &Path) -> CopilotUsage`. `cache_known` is true when any model's usage has a `cacheReadTokens` key.
  - `config::LimitsCfg.cache_read_weight: f64` (default 1.0).

- [ ] **Step 1: Write the failing journal tests** in `src/journal.rs`'s test module:

```rust
#[test]
fn cache_reads_count_at_their_weight() {
    let u = Usage { runs: 1, tokens_in: 1000, tokens_out: 100, cache_read: 800, cache_known_in: 1000, ..Usage::default() };
    assert_eq!(u.weighted(1.0), 1100);
    assert_eq!(u.weighted(0.1), 200 + 80 + 100);
    assert_eq!(u.weighted(0.0), 300);
    assert_eq!(u.cache_share(), Some(80));
}

#[test]
fn an_old_line_has_no_cache_share() {
    let e: Entry = serde_json::from_str(r#"{"kind":"issue","claude":{"runs":1,"tokens_in":500,"tokens_out":5,"cost":0.0}}"#).unwrap();
    assert_eq!((e.claude.cache_read, e.claude.cache_known_in), (0, 0));
    assert_eq!(e.claude.cache_share(), None);
    assert_eq!(e.claude.weighted(0.1), 505, "unknown cache use counts in full");
}

#[test]
fn the_daily_count_weights_cache_reads() {
    let now = 10 * DAY;
    let mut e = Entry { ts: now - 60, kind: "issue".into(), ..Entry::default() };
    e.claude = Usage { runs: 1, tokens_in: 1000, cache_read: 900, cache_known_in: 1000, ..Usage::default() };
    assert_eq!(spent(&[e.clone()], None, now, 1.0).tokens, 1000);
    assert_eq!(spent(&[e], None, now, 0.1).tokens, 100 + 90);
}
```

Also extend the existing stats-text test (the one asserting the `run(s) · … tokens in` line, near `src/journal.rs:572`) with one Usage carrying `cache_read: 720, cache_known_in: 900` and assert the line contains `tokens in, 80% from cache · `, and that a Usage with `cache_known_in: 0` shows no `from cache`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib journal::`
Expected: compile errors (`no field cache_read`, `no method weighted`, `spent` takes 3 arguments).

- [ ] **Step 3: Implement in `src/journal.rs`.** Add the three fields, update `add`, add `weighted` and `cache_share` (`saturating_sub` for `tokens_in - cache_read`; `(cache_read as f64 * w).round() as u64`). Change `spent` to take `cache_read_weight` and sum `weighted(w)` over the three CLIs. Leave `Entry::tokens()` unweighted. Change the stats line to the Global Constraints format. Fix every `spent(` caller and `Usage { .. }` literal the compiler finds; callers pass `hub.cfg.limits.cache_read_weight`. In `src/issues.rs:406` and `:419`, use `u.weighted(w)` in place of `u.tokens()`.

- [ ] **Step 4: Add `cache_read_weight` to `LimitsCfg`** with default `1.0`, and in `validate` next to the `job_mins` check: `anyhow::ensure!((0.0..=1.0).contains(&self.limits.cache_read_weight), "limits.cache_read_weight must be between 0 and 1")`. Config test `cache_read_weight_must_be_a_fraction`: `1.5` fails with that message; omitted gives `1.0`.

- [ ] **Step 5: Run them to verify they pass**

Run: `cargo test --lib journal::`, then `cargo test --lib config::`
Expected: PASS.

- [ ] **Step 6: Write the failing Copilot usage test.** Rewrite `usage_file_gives_tokens_and_models` (`src/agents.rs:1344`) for the existing fixture (`inputTokens 200, outputTokens 14, cacheReadTokens 50`; second model `1/1`): `read_usage(&p)` with models sorted equals `CopilotUsage { input: 201, output: 15, cache_read: 50, cache_write: 0, cache_known: true, models: [..both..] }`; the missing file gives `CopilotUsage::default()`.

Before implementing, look in the Copilot SDK types (not a live run: there is no Copilot seat) for how `inputTokens` relates to the cache fields. If they state it excludes them, stop and report. If they are silent, go with the runtime measurements: `inputTokens` includes them.

- [ ] **Step 7: Implement `CopilotUsage` and `read_usage`.** At its caller (`src/agents.rs:~868`), keep setting `s.tokens_in`/`s.tokens_out` from `u.input`/`u.output` when `u.input + u.output > 0`; Task 2 records the cache fields.

- [ ] **Step 8: Run it**

Run: `cargo test --lib agents::tests::usage_file_gives_tokens_and_models`, then `cargo test --test automation copilot_usage_is_recorded_separately`
Expected: PASS (the stand-in writes no cache fields, so its counts and stats line are unchanged).

- [ ] **Step 9: Doctor line and docs.** `src/main.rs:~1010`: when `l.cache_read_weight != 1.0`, append ` · cache reads count {w}`. `docs/limits.md`: add `cache_read_weight = 1.0` to the TOML block with a bullet: cache reads are cheap for the provider (Anthropic bills them at a tenth of input), so `0.1` stops the daily limit overstating long, resumed sessions, and `1.0` counts them in full, as before. `docs/metrics-and-stats.md`: the sample `claude` line in the new format, and the share is over runs that reported cache use. `docs/AGENTS.md`: Copilot's `inputTokens` already includes cached input; earlier versions counted it twice.

- [ ] **Step 10: Full gate, then commit**

Run: `cargo clippy --all-targets -- -D warnings`, `cargo test`, `node scripts/check-doc-links.mjs`. All exit 0.

```bash
git add src/journal.rs src/agents.rs src/server/seed.rs src/config.rs src/issues.rs src/main.rs docs/limits.md docs/metrics-and-stats.md docs/AGENTS.md
git commit -m "feat(limits): weight cache reads and stop double counting Copilot input"
```

---

### Task 2: A meter per run, so every run is on the account once

**Files:**
- Modify: `src/state.rs` (`Job.running`, `Job::settle`), and every `Job { .. }` literal (`src/orchestrator.rs:465`, `src/history.rs:383`, `:819`)
- Modify: `src/orchestrator.rs:~986` (settle once the job's future has ended)
- Modify: `src/agents.rs` (`live_usage`, `Meter`, its use in `run` and `spawn_and_read`; remove the add at `:616-620`)
- Test: `src/agents.rs` tests, `tests/automation.rs`

**Interfaces:**
- Consumes: `journal::Usage` and `CopilotUsage` from Task 1; `metrics::Tokens { input, cache_read, cache_write, output, .. }` (`src/metrics.rs:61`).
- Produces:
  - `state::Job.running: std::collections::BTreeMap<u64, (AgentKind, journal::Usage)>`: the live usage of each run under way, keyed by run. Not saved.
  - `pub(crate) fn live_usage(kind: AgentKind, t: &metrics::Tokens, s: &Stage) -> journal::Usage`. Claude and Codex: `tokens_in`/`tokens_out` from the stage once it has totals (`s.tokens_in + s.tokens_out > 0`, set by the final event), otherwise `t.input`/`t.output`; `cache_read`/`cache_write` from `t`; `cache_known_in = tokens_in` when `t.cache_read` is `Some`. Copilot: the stage's tokens, no cache. `cost = s.cost`, `runs = 0`.
  - `pub(crate) struct Meter { hub: Hub, job: u32, slot: Slot, key: u64 }` with
    - `pub(crate) fn start(hub: &Hub, job: u32, slot: Slot, cli: AgentKind) -> Meter`: takes a key from a `static AtomicU64` and inserts `(cli, Usage::default())` into `j.running`.
    - `pub(crate) fn observe(&self, kind: AgentKind, t: &metrics::Tokens)`: in one `hub.job` closure, stores `live_usage(kind, t, j.stage(self.slot))` under its key.
    - `pub(crate) fn set(&self, u: journal::Usage)`: stores `u` under its key.
    - `impl Drop`: in one `hub.job` closure, removes its entry, if still there, and adds it to `j.used[journal::slot(cli)]` with `runs: 1`.
  - `impl Job { pub fn settle(&mut self) }` in `src/state.rs`: moves every entry of `running` into `used` the same way, leaving `running` empty. `Drop` and `settle` share one helper so a run is never added twice.

- [ ] **Step 1: Write the failing unit tests** in `src/agents.rs`:

```rust
#[test]
fn a_running_agent_has_a_live_count() {
    let mut t = crate::metrics::Stream::default();
    let line = |id: &str, out: u64| format!(r#"{{"type":"assistant","message":{{"id":"{id}","model":"m","usage":{{"input_tokens":10,"cache_read_input_tokens":90,"cache_creation_input_tokens":0,"output_tokens":{out}}}}}}}"#);
    t.observe(AgentKind::Claude, &line("a", 5));
    t.observe(AgentKind::Claude, &line("a", 7)); // the same message again, streamed further
    t.observe(AgentKind::Claude, &line("b", 3));
    let u = live_usage(AgentKind::Claude, &t.tokens, &Stage::default());
    assert_eq!((u.tokens_in, u.tokens_out, u.cache_read, u.cache_known_in, u.runs), (200, 10, 180, 200, 0));
    let done = Stage { tokens_in: 210, tokens_out: 12, cost: 0.5, ..Stage::default() };
    let u = live_usage(AgentKind::Claude, &t.tokens, &done);
    assert_eq!((u.tokens_in, u.tokens_out, u.cache_read, u.cost), (210, 12, 180, 0.5), "the final total wins, the cache split stays");
    let s = Stage { tokens_in: 40, tokens_out: 4, ..Stage::default() };
    let c = live_usage(AgentKind::Copilot, &crate::metrics::Tokens::default(), &s);
    assert_eq!((c.tokens_in, c.tokens_out, c.cache_known_in), (40, 4, 0));
}

#[test]
fn a_run_is_on_the_account_once_whether_it_ends_or_is_dropped() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let hub = Hub::new(Config::default(), rt.handle().clone());
    let id = rt.block_on(hub.submit("t".into(), "t".into(), Source::Manual, PathBuf::from("."), None, Planning::IfRouted)).unwrap();
    let a = Meter::start(&hub, id, Slot::Worker, AgentKind::Claude);
    let b = Meter::start(&hub, id, Slot::Reviewer, AgentKind::Codex);
    a.set(Usage { tokens_in: 100, cache_read: 60, cache_known_in: 100, ..Usage::default() });
    b.set(Usage { tokens_in: 7, ..Usage::default() });
    assert_eq!(hub.job(id, |j| (j.running.len(), j.used[0].runs)).unwrap(), (2, 0));
    drop(a);
    drop(b);
    let (running, claude, codex) = hub.job(id, |j| (j.running.len(), j.used[0], j.used[1])).unwrap();
    assert_eq!(running, 0);
    assert_eq!((claude.runs, claude.tokens_in, claude.cache_read), (1, 100, 60));
    assert_eq!((codex.runs, codex.tokens_in), (1, 7));
}

#[test]
fn a_dropped_run_future_is_on_the_account() {
    // Hub and job as above. A future holding a meter is dropped while it waits, as select! drops a run.
    let fut = async {
        let m = Meter::start(&hub, id, Slot::Worker, AgentKind::Claude);
        m.set(Usage { tokens_in: 42, ..Usage::default() });
        std::future::pending::<()>().await;
    };
    rt.block_on(async { let _ = tokio::time::timeout(Duration::from_millis(10), fut).await; });
    assert_eq!(hub.job(id, |j| (j.running.len(), j.used[0].runs, j.used[0].tokens_in)).unwrap(), (0, 1, 42));
}

#[test]
fn settle_counts_runs_still_going_once() {
    // Hub and job as above.
    let m = Meter::start(&hub, id, Slot::Reviewer, AgentKind::Copilot);
    m.set(Usage { tokens_in: 9, ..Usage::default() });
    hub.job(id, |j| j.settle());
    drop(m); // an aborted reviewer task, dropped after the job has been settled
    assert_eq!(hub.job(id, |j| (j.running.len(), j.used[2].runs, j.used[2].tokens_in)).unwrap(), (0, 1, 9));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib agents::tests::a_run`, then `cargo test --lib agents::tests::a_dropped_run`, then `cargo test --lib agents::tests::settle_counts`
Expected: compile errors (`live_usage`, `Meter`, `running` not found).

- [ ] **Step 3: Implement `Job.running`, `live_usage` and `Meter`** as in Interfaces. `Drop` must not panic and must not be reached while the state lock is held (the futures holding a meter are dropped by `select!` and `JoinSet`, outside it).

- [ ] **Step 4: Wire the meter into `run` and `spawn_and_read`.**
  - In `run`, right after the `cancel`/`steer` pair is fetched: if `cancel.is_set()`, return `RunOut { error: "cancelled".into(), ..Default::default() }` without starting anything. Then `let meter = Meter::start(hub, job, slot, e.cli);`.
  - Pass `&meter` to `spawn_and_read`. In its read loop, after the `for p in parsed` block (so a final event's totals are in the stage), call `meter.observe(kind, &telemetry.lock()….tokens)`.
  - After `read_usage`, when the file reported anything: `meter.set(Usage { tokens_in: u.input, tokens_out: u.output, cache_read: u.cache_read, cache_write: u.cache_write, cache_known_in: if u.cache_known { u.input } else { 0 }, ..Usage::default() })`.
  - In demo mode, after `demo::run` returns, `meter.observe(e.cli, &Default::default())` so its stage figures are kept.
  - Delete the `j.used[..].add(..)` block at `src/agents.rs:616-620`; the meter is dropped when `run` returns.
  - In `src/orchestrator.rs`, right after `drop(hub.unpark_lane(id));` (`:~986`), `hub.job(id, |j| j.settle());`, before the outcome and the journal line. `knowledge::learn` runs later and its meter ends normally, before the journal line.

- [ ] **Step 5: Write the failing end-to-end tests** in `tests/automation.rs`.
  - Extend the `CLAUDE` stand-in. Default (worker) branch: usage gains `"cache_read_input_tokens":${FAKE_CACHE_READ:-0}` next to `input_tokens`; and before `sh -c "${FAKE_WORK…}"`, when `FAKE_ASSISTANT_TOKENS` is set, print one event `{"type":"assistant","message":{"id":"m1","model":"m","content":[],"usage":{"input_tokens":$FAKE_ASSISTANT_TOKENS,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":0}}}`, then `sleep 300`. The `*"Do not modify any file"*` (review) branch does the same with `FAKE_REVIEW_ASSISTANT_TOKENS`, right after its `reviews` counter; Task 3 uses it. Keep it out of the lessons branch.
  - Change `a_job_over_its_time_limit_is_stopped_and_handed_to_a_person` to run with `("FAKE_ASSISTANT_TOKENS", "5000")` instead of `FAKE_WORK=sleep 300`, and add `assert_eq!(rig.journal()[0]["claude"]["tokens_in"], 5000, "what it used before the stop is on record");` (no new slow test).
  - Add:

```rust
#[test]
fn cache_reads_are_on_record_and_stats_show_their_share() {
    let rig = Rig::new("cache", "", "");
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_TOKENS", "200"), ("FAKE_CACHE_READ", "800")]);
    assert!(ok, "{}", out);
    let e = &rig.journal()[0];
    assert_eq!((e["claude"]["tokens_in"].as_u64(), e["claude"]["cache_read"].as_u64(), e["claude"]["cache_known_in"].as_u64()), (Some(1000), Some(800), Some(1000)));
    let (ok, out) = rig.run(&["stats"], &[]);
    assert!(ok && out.contains("1.0k tokens in, 80% from cache"), "{}", out);
}

#[test]
fn the_daily_limit_can_count_cache_reads_lightly() {
    let rig = Rig::new("cache-limit", "[limits]\ntokens_per_day = 1000\ncache_read_weight = 0.1", "");
    rig.issue();
    rig.once(&[("FAKE_TOKENS", "100"), ("FAKE_CACHE_READ", "4900")]);
    rig.issues(&[8]);
    let (ok, out) = rig.once(&[]);
    assert!(ok && !out.contains("token limit reached"), "5.0k in, 4.9k of it cached, counts as 590: {}", out);
}
```

Only the stand-in's worker branch reports usage (its review prints none), so these are one run's figures. Its `result` line has no `cache_creation_input_tokens`: the stage total is `200 + 800 + 0` from `parse_claude`, and the cache split comes from `metrics::Stream`, which reads `cache_read_input_tokens` from the `result` usage.

- [ ] **Step 6: Run them to verify they fail, then pass**

Run: `cargo test --test automation cache_`, then `cargo test --test automation a_job_over_its_time_limit`
Expected before Step 4 is wired: `cache_read` is null and the time-limit journal shows 0. After: PASS.

- [ ] **Step 7: Full gate, then commit**

```bash
git add src/state.rs src/orchestrator.rs src/history.rs src/agents.rs tests/automation.rs
git commit -m "fix(agents): count every run once, including runs that are stopped"
```

---

### Task 3: `limits.tokens_per_job`

**Files:**
- Modify: `src/config.rs` (`LimitsCfg.tokens_per_job: u64`, default 0)
- Modify: `src/state.rs` (`enum Limit`, `Job.stopped` replacing `over_time`, `Job::stop`, `Cancel::claim`)
- Modify: every `over_time` use: `src/orchestrator.rs:465`, `:575`, `:955`, `:990-1000`, `:1095`, `:3009`, `:3019`; `src/history.rs:383`, `:819`; `src/issues.rs:1383`
- Modify: `src/journal.rs` (`job_spend`)
- Modify: `src/agents.rs` (`cap_applies`, the check in `Meter::observe`/`Meter::set`)
- Modify: `src/main.rs:~1010` (doctor: `part(l.tokens_per_job, "tokens per job")`)
- Modify: `docs/limits.md`
- Test: `src/journal.rs`, `src/state.rs` or `src/orchestrator.rs`, `src/agents.rs` tests; `tests/automation.rs`

**Interfaces:**
- Consumes: `Usage::weighted`, `LimitsCfg.cache_read_weight` (Task 1); `Meter`, `Job.running` (Task 2).
- Produces:
  - `pub fn job_spend<'a>(used: &[Usage; 3], running: impl IntoIterator<Item = &'a Usage>, cache_read_weight: f64) -> u64` in `src/journal.rs`.
  - `#[derive(Clone, Copy, Debug, PartialEq)] pub enum state::Limit { Time, Tokens(u64) }`; `Job.stopped: Option<Limit>`, doc `/// Stopped by a limit: limits.job_mins, or limits.tokens_per_job having spent this many (weighted) tokens.`
  - `impl Cancel { pub fn claim(&self) -> bool }`: `fire`, returning whether this call set the flag (the `compare_exchange` succeeded). `fire` stays as it is.
  - `impl Job { pub fn stop(&mut self, why: Limit) -> bool }`: returns false and changes nothing when the job is finished or `self.stopped.is_some()`, or when `self.cancel.claim()` returns false (a cancel or shutdown got there first); otherwise sets `stopped` and returns true.
  - `pub(crate) fn cap_applies(j: &Job, slot: Slot, cfg: &Config) -> bool` in `src/agents.rs`: `cfg.limits.tokens_per_job > 0 && !cfg.demo && !j.source.is_manual() && !matches!(j.source, Source::Deploy { .. }) && slot != Slot::Knowledge`.
  - `config::LimitsCfg.tokens_per_job: u64`.

- [ ] **Step 1: Write the failing unit tests.**

In `src/journal.rs`:

```rust
#[test]
fn job_spend_counts_finished_and_running_runs() {
    let used = [Usage { runs: 1, tokens_in: 1000, tokens_out: 100, ..Usage::default() }, Usage::default(), Usage { runs: 1, tokens_in: 50, ..Usage::default() }];
    let a = Usage { tokens_in: 500, cache_read: 400, cache_known_in: 500, ..Usage::default() };
    let b = Usage { tokens_in: 30, ..Usage::default() };
    assert_eq!(job_spend(&used, [&a, &b], 1.0), 1100 + 50 + 500 + 30);
    assert_eq!(job_spend(&used, [&a, &b], 0.1), 1100 + 50 + 100 + 40 + 30);
}
```

Next to the existing `replies_keep_conversation_without_recounting_journal_usage` (`src/orchestrator.rs:~2998`, which shows how to make a Hub and a job):

```rust
#[test]
fn only_the_first_stop_names_the_reason() {
    // Make a hub and submit a job as the reply test does.
    hub.job(id, |j| {
        assert!(j.stop(Limit::Tokens(5)));
        assert!(!j.stop(Limit::Time));
        assert_eq!(j.stopped, Some(Limit::Tokens(5)));
        assert!(j.cancel.is_set());
    });
    let other = /* a second job */;
    hub.job(other, |j| {
        j.cancel.fire(); // a person's cancel
        assert!(!j.stop(Limit::Tokens(9)));
        assert_eq!(j.stopped, None);
    });
}
```

In `src/agents.rs`: `the_cap_leaves_typed_tasks_deployments_knowledge_and_demo_alone`. With `tokens_per_job = 1000`: an issue job and `Slot::Worker` → true; `Slot::Knowledge` → false; `Source::Manual` → false; `Source::Deploy { repo_idx: 0, repo: "o/r".into(), sha: "a".into() }` → false; the issue with `cfg.demo = true` → false; with `tokens_per_job = 0` → false. Build the issue source the way existing tests in that module do.

Also in `src/agents.rs`, `two_live_meters_are_summed_against_the_cap`: a Hub with `tokens_per_job = 1000` and a job from an issue source; `a = Meter::start(.., Slot::Worker, Claude)`, `b = Meter::start(.., Slot::Reviewer, Copilot)`; `a.set(600 in)` leaves `stopped == None`; `b.set(600 in)` gives `stopped == Some(Limit::Tokens(1200))` and a fired cancel, with both meters still alive.

Update the `over_time` lines of `replies_keep_conversation_without_recounting_journal_usage` to `j.stopped = Some(Limit::Time)` and `assert_eq!(j.stopped, None)`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib job_spend`, then `cargo test --lib only_the_first_stop`, then `cargo test --lib the_cap_leaves`, then `cargo test --lib two_live_meters`
Expected: compile errors. `two_live_meters_are_summed_against_the_cap` compiles once Step 3 is done and fails until Step 7.

- [ ] **Step 3: Implement** `job_spend`, `Limit`, `Job.stopped`, `Job::stop`, `cap_applies` and `tokens_per_job`. Replace `over_time` everywhere listed:
  - Literals and resets (`:465`, `:575`, history): `stopped: None`.
  - Watchdog (`src/orchestrator.rs:955`): `h.job(id, |j| j.stop(Limit::Time))`, log on true; `stop` fires the cancel, so drop the separate `c.fire()`.
  - Outcome (`:990-1000`): `match` on `stopped` when the status is not Done: `Time` keeps today's text; `Tokens(n)` uses the Global Constraints text with `journal::kilo(n)`.
  - Retry (`:1095`): `if stopped.is_none()`. Label (`src/issues.rs:1383`): `j.stopped.is_some()`.

- [ ] **Step 4: Run the unit tests to verify they pass**

The first three commands from Step 2. Expected: PASS (`two_live_meters` waits for Step 7).

- [ ] **Step 5: Write the failing end-to-end tests.** Stand-ins: in `FALLBACK_CODEX`'s last line (`tests/automation.rs:265`), `"input_tokens":${FAKE_CODEX_TOKENS:-10}`; in `COPILOT`'s usage line (`tests/automation.rs:~77`), `"inputTokens":${FAKE_COPILOT_TOKENS:-10}`. Quote them as the `CLAUDE` stand-in quotes `FAKE_TOKENS`.

```rust
#[test]
fn a_job_over_its_token_limit_is_stopped_and_handed_to_a_person() {
    let rig = Rig::new("limit-job-tokens", "[limits]\ntokens_per_job = 1000", "");
    rig.issue();
    let started = std::time::Instant::now();
    let (ok, out) = rig.once(&[("FAKE_ASSISTANT_TOKENS", "5000")]);
    assert!(!ok, "{}", out);
    assert!(started.elapsed().as_secs() < 60, "it was stopped, not waited for");
    assert!(out.contains("failed: stopped after 5.0k tokens, the limit for one job (limits.tokens_per_job)"), "{}", out);
    assert!(rig.calls().contains("--add-label agent:needs-human"), "{}", rig.calls());
    assert_eq!(rig.journal()[0]["status"], "failed");
    assert_eq!(rig.journal()[0]["claude"]["tokens_in"], 5000);
    let before = rig.calls();
    rig.once(&[]);
    assert_eq!(rig.calls(), before, "it is not retried");
}

#[test]
fn concurrent_reviewers_are_stopped_by_their_combined_spend() {
    // Each reviewer stays under the cap; together they pass it. Claude's reviewer is still running.
    // Which one reports last varies; two_live_meters_are_summed_against_the_cap pins the sum itself.
    let extra = "[agents.codex]\ncmd = \"absent-codex\"\n[agents.copilot.profiles.second]\nvendor = \"xai\"\nmodels = { light = \"grok-4.7\", standard = \"grok-4.7\", heavy = \"grok-4.7\" }\n[review]\npr_reviewers = [\"claude\", \"copilot:second\"]\n[limits]\ntokens_per_job = 1000";
    let rig = Rig::with_agents("limit-job-reviewers", extra, "", &[("claude", CLAUDE), ("copilot", COPILOT)]);
    rig.pull_request(5, "feat");
    rig.list_pull_request(5, "feat");
    let started = std::time::Instant::now();
    let (ok, out) = rig.once(&[("FAKE_REVIEW_ASSISTANT_TOKENS", "600"), ("FAKE_COPILOT_TOKENS", "600")]);
    assert!(!ok && out.contains("limits.tokens_per_job"), "{}", out);
    assert!(started.elapsed().as_secs() < 60, "it was stopped, not waited for");
    assert!(rig.calls().contains("--add-label review:changes"), "{}", rig.calls());
    let e = &rig.journal()[0];
    assert_eq!((e["claude"]["tokens_in"].as_u64(), e["copilot"]["tokens_in"].as_u64()), (Some(600), Some(600)), "both reviewers are on record");
    let before = rig.calls();
    rig.once(&[]);
    assert_eq!(rig.calls(), before, "the review is not asked for again");
}

#[test]
fn a_copilot_run_over_the_cap_stops_the_job_before_the_review() {
    let extra = format!("{}[limits]\ntokens_per_job = 1000", NO_REAL_CLIS);
    let rig = Rig::with_agents("limit-job-copilot", &extra, "", &[("copilot", COPILOT)]);
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_COPILOT_TOKENS", "5000")]);
    assert!(!ok && out.contains("limits.tokens_per_job"), "{}", out);
    assert_eq!(copilot_runs(&rig).len(), 1, "the review never started");
}

#[test]
fn a_codex_run_over_the_cap_stops_the_job_after_it_reports() {
    let rig = quota_rig_with("limit-job-codex", "[router]\nkind = \"rules\"\ndefault_agent = \"codex\"\n[limits]\ntokens_per_job = 1000");
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_CODEX_TOKENS", "5000")]);
    assert!(!ok && out.contains("limits.tokens_per_job"), "{}", out);
    assert_eq!(fs::read_to_string(rig.root.join("gh/attempts")).unwrap(), "codex\n", "only the worker ran");
}

#[test]
fn a_typed_task_is_not_stopped_by_the_job_cap() {
    let rig = Rig::new("limit-job-typed", "[limits]\ntokens_per_job = 1000", "");
    let (ok, out) = rig.run(&["run", "add a change file"], &[("FAKE_TOKENS", "5000")]);
    assert!(ok && !out.contains("limits.tokens_per_job"), "{}", out);
}
```

If the rules router does not send the Codex test's issue to Codex, force it the way the quota tests do rather than change the assertion. If the review rig's `once` prints the review's outcome without `failed:`, keep `!ok` only if `once` exits non-zero for a failed review; otherwise assert on the outcome text alone.

- [ ] **Step 6: Run them to verify they fail**

Run each: `cargo test --test automation token_limit`, `… concurrent_reviewers`, `… copilot_run_over_the_cap`, `… codex_run_over_the_cap`, `… typed_task_is_not_stopped`
Expected: all but the typed-task test FAIL (not stopped, or waits out `sleep 300`). The typed-task test may pass already and must keep passing.

- [ ] **Step 7: Implement the check.** In `Meter::observe` and `Meter::set`, in the same `hub.job` closure that stores the usage: if `cap_applies(j, self.slot, &hub.cfg)`, compute `job_spend(&j.used, j.running.values().map(|(_, u)| u), w)`; when it is `>= tokens_per_job` and `j.stop(Limit::Tokens(spent))` returns true, return `Some(spent)` from the closure and log the Global Constraints line after it. The stopped run stays in `j.running` until its meter drops, so it is counted once.

- [ ] **Step 8: Run the tests to verify they pass**

The five commands from Step 6, then `cargo test --lib`. Expected: PASS.

- [ ] **Step 9: Doctor and docs.** Doctor limits line: `part(l.tokens_per_job, "tokens per job")` after the daily tokens, and count `tokens_per_job > 0` in the `open`/`set` test. `docs/limits.md`: `tokens_per_job = 400000` in the TOML block and a bullet: it stops an issue or review once it has used this many tokens (weighted as above, all its agents together, reviewers running side by side included), kills its agents and hands the item to a person with the failure label, like `job_mins`. Typed tasks, deployments and the closing knowledge run are exempt. Claude is checked as it works; Codex reports when its turn ends and Copilot when it exits, so one such run can go past the limit before the job is stopped.

- [ ] **Step 10: Full gate, then commit**

Run: `cargo clippy --all-targets -- -D warnings`, `cargo test`, `node scripts/check-doc-links.mjs`. All exit 0.

```bash
git add src/config.rs src/state.rs src/history.rs src/journal.rs src/agents.rs src/orchestrator.rs src/issues.rs src/main.rs docs/limits.md tests/automation.rs
git commit -m "feat(limits): stop an issue or review that spends too many tokens"
```
