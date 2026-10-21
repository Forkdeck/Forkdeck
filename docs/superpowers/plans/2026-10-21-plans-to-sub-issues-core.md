# Plans to Sub-issues — Core Implementation Plan (1 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An issue plan that needs more than one PR ends in a task list; accepting it creates real GitHub sub-issues with blocked-by links, and Forkdeck starts each task only once everything it depends on has merged.

**Architecture:** A pure `taskgraph` module owns the task list (parse, validate, render, read back). `subissues` turns an accepted plan into forge issues as a recoverable, step-by-step transaction. `schedule` runs on every poll: it reads each plan's task list from its parent issue (the authority), asks the forge what merged, and adds/removes the ready label; the same check runs again right before an issue is claimed.

All three plan sources enter through the existing F6 issue-plan lifecycle unchanged: an issue link, a new issue drafted from typed text, or typed text pointing the planner at a plan file in the repo.

**Tech Stack:** Rust (tokio, serde_json, rusqlite), `gh api` (REST) for GitHub.

**Spec:** `docs/superpowers/specs/2026-10-21-plans-to-sub-issues-design.md`

**Later plans (not here):** 2 — adopting existing issue trees (semantic-context #1), holding and reworking PRs built on unmerged dependencies, decision tasks answered by comment. 3 — re-planning with a shown difference, and the clarifying-question wait state for vague tasks.

## Global Constraints

- Hand-formatted Rust: never run `cargo fmt`. Match the surrounding density and naming.
- Gate before every commit, in its own call, unpiped: `cargo clippy --all-targets -- -D warnings` then `cargo test`. Read the result before `git commit`.
- Dependency means merged: a work task is done only when a PR from `forkdeck/issue-<n>` merged into `RepoCfg.base`. A closed issue or a closed-unmerged PR is not done.
- The task list in the parent issue is the authority; labels are output only.
- Remote writes only after the matching state is persisted (`hub.persist`), as `planning.rs` does.
- GitHub only in this plan. Every new forge call on GitLab returns `Err("Sub-issues are GitHub-only for now.")`, and the planner prompt omits the task-list instruction for GitLab repos.
- Installed `gh` is 2.45: use REST via `tracker::api`, never `gh issue create --parent`/`--blocked-by`.
- Block tag `forkdeck-tasks`; section markers `<!-- forkdeck:tasks:start -->` / `<!-- forkdeck:tasks:end -->`; task marker `<!-- forkdeck:task <token> <task-id> root=<n> -->`; JSON `version` 1.
- New `RepoCfg` labels: `plan_label` default `forkdeck:plan`, `human_label` default `needs-human`.

## Review Focus

1. A task carrying the ready label (stale, or applied by hand) while a dependency is unmerged must not start — Task 7 test `a_hand_labelled_blocked_task_is_not_claimed`.
2. A crash or forge timeout mid-creation must not duplicate issues on retry — Task 5 test `a_create_that_may_have_landed_is_found_not_repeated`.
3. A hand-edited task list must stop scheduling for that plan, not be guessed at — Task 2 test `a_hand_edited_list_is_refused`, Task 6 test `an_unreadable_list_changes_no_labels`.
4. A dependency whose PR was closed without merging keeps dependents blocked — Task 6 test `a_closed_unmerged_pr_is_not_done`.
5. A forge that cannot be read counts as "not ready", never "ready" — Task 6 test `unknown_facts_change_nothing`.

---

### Task 1: Task graph model, extraction and validation

**Files:**
- Create: `src/taskgraph.rs` (declare `mod taskgraph;` in `src/main.rs` beside `mod tracker;`)

**Interfaces:**
- Produces:
  ```rust
  pub const VERSION: u32 = 1;
  pub const TAG: &str = "forkdeck-tasks";
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
  pub enum Kind { Work, Group, Decision }
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
  pub struct Task { pub id: String, pub title: String, #[serde(default)] pub body: String, pub kind: Kind,
      #[serde(default)] pub parent: Option<String>, #[serde(default)] pub after: Vec<String>,
      #[serde(default)] pub checks: Vec<String>, #[serde(default)] pub number: Option<u64> }
  #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
  pub struct Graph { pub version: u32, pub tasks: Vec<Task> }
  impl Graph {
      pub fn task(&self, id: &str) -> Option<&Task>;
      pub fn children(&self, id: Option<&str>) -> Vec<&Task>;      // None = top level
      pub fn effective_after(&self, id: &str) -> Vec<String>;      // own `after` plus every ancestor group's, deduped
      pub fn creation_order(&self) -> Vec<usize>;                  // indices; every `after` target and parent before the task
  }
  pub fn extract(text: &str) -> Result<Option<Graph>, String>;     // the one ```forkdeck-tasks fenced block, if any
  pub fn strip(text: &str) -> String;                              // text without that block
  pub fn validate(g: &Graph) -> Result<(), String>;
  ```

- [ ] **Step 1: Write the failing tests** in `src/taskgraph.rs` `mod tests`:

```rust
const PLAN: &str = "Intro.\n\n```forkdeck-tasks\n{\"version\":1,\"tasks\":[\
 {\"id\":\"g0\",\"title\":\"Storage\",\"kind\":\"group\"},\
 {\"id\":\"a\",\"title\":\"Records\",\"kind\":\"work\",\"parent\":\"g0\"},\
 {\"id\":\"g1\",\"title\":\"API\",\"kind\":\"group\",\"after\":[\"g0\"]},\
 {\"id\":\"b\",\"title\":\"Endpoint\",\"kind\":\"work\",\"parent\":\"g1\"},\
 {\"id\":\"c\",\"title\":\"Sync\",\"kind\":\"work\",\"parent\":\"g1\",\"after\":[\"b\"]},\
 {\"id\":\"d\",\"title\":\"Pick backend\",\"kind\":\"decision\"}]}\n```\n\nPLAN: READY";

#[test]
fn a_plan_carries_one_task_list() {
    let g = extract(PLAN).unwrap().unwrap();
    assert_eq!(g.tasks.len(), 6);
    validate(&g).unwrap();
    assert_eq!(extract("No list here.\nPLAN: READY").unwrap(), None);
    assert!(extract(&format!("{PLAN}\n{PLAN}")).is_err(), "two lists");
    assert!(extract("```forkdeck-tasks\nnot json\n```").is_err());
    assert!(!strip(PLAN).contains("forkdeck-tasks") && strip(PLAN).contains("Intro."));
}

#[test]
fn group_dependencies_reach_everything_inside() {
    let g = extract(PLAN).unwrap().unwrap();
    assert_eq!(g.effective_after("c"), vec!["b".to_string(), "g0".to_string()]);
    assert_eq!(g.effective_after("a"), Vec::<String>::new());
    let order: Vec<&str> = g.creation_order().iter().map(|&i| g.tasks[i].id.as_str()).collect();
    let pos = |id| order.iter().position(|x| *x == id).unwrap();
    assert!(pos("g0") < pos("a") && pos("b") < pos("c") && pos("g1") < pos("b"));
}

#[test]
fn broken_lists_are_refused_with_a_reason() {
    let bad = |json: &str| validate(&serde_json::from_str::<Graph>(json).unwrap()).unwrap_err();
    assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["a"]}]}"#).contains("itself"));
    assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["zz"]}]}"#).contains("zz"));
    assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","after":["b"]},{"id":"b","title":"B","kind":"work","after":["a"]}]}"#).contains("cycle"));
    // A task waiting on the group it is inside can never start.
    assert!(bad(r#"{"version":1,"tasks":[{"id":"g","title":"G","kind":"group"},{"id":"a","title":"A","kind":"work","parent":"g","after":["g"]}]}"#).contains("cycle"));
    assert!(bad(r#"{"version":1,"tasks":[{"id":"g","title":"G","kind":"group"}]}"#).contains("no tasks"));
    assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work"},{"id":"a","title":"B","kind":"work"}]}"#).contains("twice"));
    assert!(bad(r#"{"version":1,"tasks":[{"id":"a","title":"A","kind":"work","parent":"b"},{"id":"b","title":"B","kind":"work"}]}"#).contains("group"));
    assert!(bad(r#"{"version":2,"tasks":[{"id":"a","title":"A","kind":"work"}]}"#).contains("version"));
    assert!(bad(r#"{"version":1,"tasks":[]}"#).contains("empty"));
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test taskgraph::` → FAIL (module missing).

- [ ] **Step 3: Implement.** `validate` errors, in this order, each naming the task id: version ≠ 1 ("version"), no tasks ("empty"), duplicate id ("twice"), empty title, `after`/`parent` naming an unknown id (the id), `after` naming itself ("itself"), parent that is not a group ("group"), group with no children ("no tasks"), then cycle detection by DFS over edges task → each `effective_after` target, where an edge to a group also reaches every descendant of that group, and a task inside group G depending on G is a cycle ("cycle"). `creation_order`: DFS emitting parents and `effective_after` targets first. `extract` finds fences opening with exactly ```` ```forkdeck-tasks ````.

- [ ] **Step 4: Run tests** — `cargo test taskgraph::` → PASS.

- [ ] **Step 5: Commit** — `git add src/taskgraph.rs src/main.rs && git commit -m "feat: a plan can carry a validated task list"`

---

### Task 2: The visible task list and task markers

**Files:**
- Modify: `src/taskgraph.rs`
- Modify: `src/tracker.rs:26-48` — generalize `section`/`replace` to `section_between(body, open, close)` / `replace_between(body, open, close, expected, replacement)`; keep `section`/`replace` as one-line wrappers over the plan markers.

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  ```rust
  pub const OPEN: &str = "<!-- forkdeck:tasks:start -->";
  pub const CLOSE: &str = "<!-- forkdeck:tasks:end -->";
  pub fn render(g: &Graph, status: &dyn Fn(&Task) -> String) -> String;   // whole managed section, markers included
  pub fn read(section: &str) -> Result<Graph, String>;                     // JSON from <details>, checked against the list
  pub fn task_marker(token: &str, id: &str, root: u64) -> String;
  pub fn parse_task_marker(body: &str) -> Option<(String, String, u64)>;  // (token, task id, root)
  pub fn task_body(t: &Task, g: &Graph, token: &str, root: u64) -> String;
  // tracker.rs
  pub fn section_between(body: &str, open: &str, close: &str) -> Result<Option<String>, String>;
  pub fn replace_between(body: &str, open: &str, close: &str, expected: &Option<String>, replacement: &str) -> Result<String, String>;
  ```

- [ ] **Step 1: Write the failing tests:**

```rust
fn numbered() -> Graph {
    let mut g = extract(PLAN).unwrap().unwrap();
    for (i, t) in g.tasks.iter_mut().enumerate() { t.number = Some(20 + i as u64); }
    g
}

#[test]
fn the_list_reads_like_the_design_and_reads_back() {
    let g = numbered();
    let s = render(&g, &|t| if t.id == "a" { "merged in PR #71".into() } else { String::new() });
    assert!(s.starts_with(OPEN) && s.ends_with(CLOSE));
    assert!(s.contains("- [x] #21 Records — merged in PR #71"));
    assert!(s.contains("  - [ ] #24 Sync · after #23"), "nested under its group, own deps only:\n{s}");
    assert!(s.contains("- [ ] #22 API · after #20"));
    assert!(s.contains("<details><summary>Plan data</summary>"));
    assert_eq!(read(&s).unwrap(), g);
    // Statuses change on every pass; they are not an edit.
    assert_eq!(read(&render(&g, &|_| "PR #9 open".into())).unwrap(), g);
}

#[test]
fn a_hand_edited_list_is_refused() {
    let s = render(&numbered(), &|_| String::new());
    assert!(read(&s.replace("#24 Sync", "#24 Sync faster")).unwrap_err().contains("edited"));
    assert!(read(&s.replace(" · after #23", "")).unwrap_err().contains("edited"));
    assert!(read(&s.replace("\"version\":1", "\"version\":1,")).is_err(), "broken JSON");
}

#[test]
fn task_issues_say_where_they_belong() {
    let g = numbered();
    let body = task_body(g.task("c").unwrap(), &g, "tok-3", 7);
    assert_eq!(parse_task_marker(&body), Some(("tok-3".into(), "c".into(), 7)));
    assert!(body.contains("Part of #7") && body.contains("Depends on #23"));
    assert_eq!(parse_task_marker("plain issue"), None);
}
```
Checked items: `- [x]` when the status starts with `merged` or equals `done`. A checks list renders under `**Acceptance checks**` in `task_body`. `Depends on` uses the task's own `after` numbers (groups by their own number).

- [ ] **Step 2: Run** `cargo test taskgraph:: tracker::` → FAIL.
- [ ] **Step 3: Implement.** List lines: indent two spaces per group depth; `- [x] #N Title` + (own `after` non-empty: ` · after #a, #b`) + (status non-empty: ` — status`). `read` rebuilds the skeleton by stripping `- [ ]`/`- [x]` and everything from ` — `, compares line-for-line against `render(&parsed, &|_| String::new())` stripped the same way, and errors `"The task list on the issue was edited by hand; re-plan to change it."` on any difference. JSON is compact (`serde_json::to_string`, so the test's `"version":1` matches) inside a ```` ```json ```` fence in `<details>`.
- [ ] **Step 4: Run** → PASS (the existing `preserve_human_text_and_detect_conflicts` still passes).
- [ ] **Step 5: Commit** — `feat: plans render a readable task list with their data folded in`

---

### Task 3: Config labels

**Files:**
- Modify: `src/config.rs` (`RepoCfg` near `:797`, defaults near `:839`, label validation `:1223`), `forkdeck.example.toml`, `README.md` (issues section)

**Interfaces:**
- Produces: `RepoCfg.plan_label: String` (default `"forkdeck:plan"`), `RepoCfg.human_label: String` (default `"needs-human"`). Both join the issue-label group in the distinctness check.

- [ ] **Step 1: Failing test** in config tests:

```rust
#[test]
fn plan_and_human_labels_default_and_stay_distinct() {
    let cfg: Config = toml::from_str("[[repos]]\nprovider='github'\nrepo='a/b'\npath='/tmp'\n").unwrap();
    assert_eq!((cfg.repos[0].plan_label.as_str(), cfg.repos[0].human_label.as_str()), ("forkdeck:plan", "needs-human"));
    for bad in ["plan_label = 'agent:ready'", "human_label = 'agent:working'", "plan_label = ''"] {
        assert!(check(bad).is_err(), "{bad}");   // the existing helper used by automation_labels_cannot_broaden_the_filter_or_conflict
    }
}
```
- [ ] **Step 2: Run** `cargo test config::` → FAIL.
- [ ] **Step 3: Implement** fields with `#[serde(default = ...)]`, add both to the issue label array in the `:1223` loop; document them in the example config (`#~` commented defaults, as the file does) and README.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `feat: labels for plan parents and tasks that need a person`

---

### Task 4: GitHub sub-issue, dependency and merge calls

**Files:**
- Modify: `src/tracker.rs`
- Create: `tests/fixtures/fake-forge` (python3, executable) — a stateful `gh api` stand-in used by Tasks 5–8

**Interfaces:**
- Produces (all GitHub-only; GitLab → `Err("Sub-issues are GitHub-only for now.")`):
  ```rust
  pub struct RemoteIssue { /* existing */ pub id: u64, pub completed: bool }   // "id"; state_reason == "completed"
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum PrState { None, Open(u64), Merged(u64), Closed(u64) }
  pub fn pr_state(r: &RepoCfg, json: &str) -> Result<PrState, String>;                // pure: pulls list → state
  pub async fn pr_for_branch(r: &RepoCfg, branch: &str) -> Result<PrState, String>;   // GET repos/{repo}/pulls?head={owner}:{branch}&state=all&per_page=100
  pub async fn sub_issues(r: &RepoCfg, key: &str) -> Result<Vec<RemoteIssue>, String>;        // GET …/issues/{n}/sub_issues?per_page=100 (--paginate)
  pub async fn add_sub_issue(r: &RepoCfg, parent: &str, child_id: u64) -> Result<(), String>; // POST …/sub_issues {"sub_issue_id"}
  pub async fn blocked_by(r: &RepoCfg, key: &str) -> Result<Vec<u64>, String>;                // GET …/dependencies/blocked_by → numbers
  pub async fn add_blocked_by(r: &RepoCfg, key: &str, blocker_id: u64) -> Result<(), String>; // POST …/dependencies/blocked_by {"issue_id"}
  pub async fn labelled(r: &RepoCfg, label: &str) -> Result<Vec<RemoteIssue>, String>;       // open issues with label, PRs skipped, paginated
  pub async fn set_label(r: &RepoCfg, key: &str, label: &str, on: bool) -> Result<(), String>; // `readiness(r, key, ready)` stays, as a one-line call to set_label(r, key, &r.label, ready)
  pub async fn close(r: &RepoCfg, key: &str) -> Result<(), String>;                           // PATCH {"state":"closed","state_reason":"completed"}
  ```

- [ ] **Step 1: Failing unit test** (pure):

```rust
#[test]
fn only_a_merge_into_the_base_branch_counts() {
    let cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='github'\nrepo='o/r'\npath='/tmp'\nbase='main'\n").unwrap();
    let r = &cfg.repos[0];
    let pr = |n: u64, state: &str, merged: bool, base: &str| format!(r#"{{"number":{n},"state":"{state}","merged_at":{},"base":{{"ref":"{base}"}}}}"#, if merged { "\"2026-10-01T00:00:00Z\"" } else { "null" });
    assert_eq!(pr_state(r, "[]").unwrap(), PrState::None);
    assert_eq!(pr_state(r, &format!("[{}]", pr(5, "open", false, "main"))).unwrap(), PrState::Open(5));
    assert_eq!(pr_state(r, &format!("[{}]", pr(5, "closed", false, "main"))).unwrap(), PrState::Closed(5));
    assert_eq!(pr_state(r, &format!("[{},{}]", pr(5, "closed", false, "main"), pr(6, "closed", true, "main"))).unwrap(), PrState::Merged(6));
    assert_eq!(pr_state(r, &format!("[{}]", pr(7, "closed", true, "release"))).unwrap(), PrState::Closed(7), "merged elsewhere");
}
```
- [ ] **Step 2: Run** `cargo test tracker::` → FAIL.
- [ ] **Step 3: Implement** the calls over `api()`; extend `parse` with `id` and `completed`. Precedence in `pr_state`: any merged-into-base → `Merged` (highest number); else any open → `Open`; else latest → `Closed`. Owner for `head=` is the part of `r.repo` before `/`.
- [ ] **Step 4: Write `tests/fixtures/fake-forge`**: stores state in `$FAKE_FORGE/state.json` (`issues` by number with id = 1000+number, labels, state, state_reason, body, title; `sub_issues` parent→[numbers]; `blocked_by` n→[numbers]; `pulls` list), logs each call to `$FAKE_FORGE/calls.log`, and implements exactly: `GET user`; `POST/GET/PATCH repos/o/r/issues[/N]`; `GET repos/o/r/issues?labels=…`; `POST/DELETE …/issues/N/labels[/L]`; `GET/POST …/sub_issues`; `GET/POST …/dependencies/blocked_by`; `GET repos/o/r/pulls?head=…`; `GET/POST …/issues/N/comments` (empty list). `$FAKE_FORGE/timeout_once` containing a `METHOD path` prefix makes that call apply its effect, then exit 1 with `timed out` (an uncertain write). Unknown routes exit 1 with `HTTP 404`.
- [ ] **Step 5: Run** → PASS. **Commit** — `feat: GitHub sub-issue, dependency and merge-state calls`

---

### Task 5: Accepting a task-list plan creates the sub-issues

**Files:**
- Create: `src/subissues.rs` (`mod subissues;` in `main.rs`)
- Modify: `src/planning.rs` (`Plan` fields, `accept`, `request_ready`, `status`), `src/orchestrator.rs:2302-2310` (Publish branch), `src/history.rs:14` (`UNRESOLVED_PLAN`)

**Interfaces:**
- Consumes: Tasks 1, 2, 4.
- Produces:
  ```rust
  // planning.rs
  #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
  pub struct Creation { pub token: String, pub attempted: BTreeSet<String>, pub ids: BTreeMap<String, u64>, pub linked: bool, pub listed: bool, pub done: bool }
  // Plan gains: #[serde(default)] pub graph: Option<taskgraph::Graph>, #[serde(default)] pub creation: Option<Creation>
  impl Plan { pub fn resolved(&self) -> bool }   // ready_for_intake() || creation.done
  // subissues.rs
  pub async fn materialize(hub: &Hub, id: u32) -> Result<String, String>;
  ```

Behaviour:
- `accept`: sets `graph = taskgraph::extract(&draft)?`; the published plan section uses `taskgraph::strip`. When `published == Some(revision)` and the graph's creation is not done, `accept` sets `action = Publish` and returns Ok (retry), instead of "already published".
- `request_ready` on a graph plan → `Err("This plan is split into sub-issues; they start by themselves once created.")`.
- Orchestrator Publish branch: run `planning::publish` only if `pending.is_some()`, then `subissues::materialize` if `graph.is_some()`; the Finish message is materialize's.
- `materialize` steps, persisting (`durable`) before each forge write and recording progress after it:
  1. For each index in `creation_order`, skip if numbered; if `attempted` holds the id, `find_created(task_marker)` → number, or `Err("A sub-issue create may still be in flight; retry later. Nothing was duplicated.")`; else `reachable`, mark attempted, create `task_body(...)`; a `refused` error unmarks. Store `number` in the graph and the API id in `ids`.
  2. `linked`: for every task, its parent is its group's issue or the root; add via `add_sub_issue` unless `sub_issues(parent)` already lists it. For every own `after` target, `add_blocked_by` unless `blocked_by` already lists it.
  3. `listed`: write `taskgraph::render(graph, |_| "")` into the root body with `replace_between` (expected = current section, re-read immediately before writing, as `publish` does), then `set_label(root, plan_label)`, and `set_label(task, human_label)` for decision tasks.
  4. `done = true`, persist. Returns `"Created N sub-issues under #root."` (Task 6 adds the immediate scheduling pass and the ready count).
- `UNRESOLVED_PLAN` treats `json_extract(issue_plan,'$.creation.done') IS 1` as resolved, matching `Plan::resolved`.

- [ ] **Step 1: Failing unit tests** in `planning.rs`:

```rust
#[test]
fn a_task_list_plan_is_created_not_marked_ready() {
    let cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='github'\nrepo='a/b'\npath='/tmp'\n").unwrap();
    let mut p = Plan::new(&cfg.repos[0], "").unwrap();
    p.revision = 1; p.reviewed = Some(1);
    p.draft = "Split.\n```forkdeck-tasks\n{\"version\":1,\"tasks\":[{\"id\":\"a\",\"title\":\"A\",\"kind\":\"work\"}]}\n```\nPLAN: READY".into();
    p.accept(false).unwrap();
    assert!(p.graph.is_some());
    assert!(!p.pending.as_ref().unwrap().section.contains("forkdeck-tasks"));
    p.pending = None; p.published = Some(1);
    assert!(p.request_ready(false).unwrap_err().contains("sub-issues"));
    assert!(!p.resolved());
    p.accept(false).unwrap();
    assert_eq!(p.action, Action::Publish, "an unfinished creation is retried, not refused");
    p.creation = Some(Creation { done: true, ..Default::default() });
    assert!(p.resolved());
}
```
And in `history.rs` tests: a job whose plan has `creation.done = true` is not kept by `UNRESOLVED_PLAN` past `KEEP`; one with `done = false` is.

- [ ] **Step 2: Failing integration test** `subissues::tests::accepting_creates_linked_sub_issues_in_order` using the child-process pattern of `orchestrator::tests::a_failed_plan_review_goes_back_to_the_planner` with `PATH` prefixed by a dir holding `gh` → `tests/fixtures/fake-forge`: a plan job linked to root issue #1 with `PLAN`'s graph from Task 1, reviewed, then `hub.plan_action(id, Action::Publish)`. Assert: six issues #2–#7 exist; `sub_issues(1)` = groups + decision, groups hold their tasks; `blocked_by` of "c" holds "b"'s number and "g1" holds "g0"'s; root body contains the rendered list and the `forkdeck:plan` label; the decision issue has `needs-human`; no task carries `agent:ready` yet (scheduling is Task 6).
- [ ] **Step 3: Failing test** `a_create_that_may_have_landed_is_found_not_repeated`: `timeout_once` = `POST repos/o/r/issues` on the third create; first `plan_action` fails; second succeeds; assert exactly six task issues exist.
- [ ] **Step 4: Run** → FAIL. **Step 5: Implement.** **Step 6: Run gate** → PASS.
- [ ] **Step 7: Commit** — `feat: accepting a plan creates its sub-issues and links`

---

### Task 6: The scheduler pass

**Files:**
- Create: `src/schedule.rs` (`mod schedule;`)
- Modify: `src/orchestrator.rs` `Hub` (add `pub merged: Arc<Mutex<HashSet<(String, u64)>>>`, initialised empty), `src/issues.rs:483` (`poll_once`: call `schedule::pass(hub, idx)` for each GitHub repo before listing issues, skipped in demo), `src/subissues.rs` (`materialize` ends with `schedule::pass` and returns `"Created N sub-issues under #root; K ready to start."`)

**Interfaces:**
- Consumes: Tasks 1, 2, 4.
- Produces:
  ```rust
  #[derive(Clone, Debug, Default)]
  pub struct Fact { pub open: bool, pub completed: bool, pub labels: Vec<String>, pub pr: Option<tracker::PrState> }  // pr None = not asked/unknown
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum Change { Label(u64), Unlabel(u64, String), Close(u64) }   // Unlabel carries the log reason
  pub fn done(g: &Graph, id: &str, facts: &HashMap<u64, Fact>) -> Option<bool>;   // None = cannot tell
  pub fn changes(r: &RepoCfg, g: &Graph, facts: &HashMap<u64, Fact>) -> Vec<Change>;
  pub fn status(r: &RepoCfg, g: &Graph, t: &Task, facts: &HashMap<u64, Fact>) -> String;
  pub async fn pass(hub: &Hub, idx: usize);
  ```

Rules for `changes` (pure): work done ⇔ `pr == Merged`; decision done ⇔ closed and completed; group done ⇔ every child done. A work task is ready ⇔ every `effective_after` target is `Some(true)`. Ready, open, not done, and carrying none of `label`/`working_label`/`done_label`/`failed_label`/`human_label` → `Label`. Carrying `label` while not ready → `Unlabel(n, "waits for #a, #b to merge")`. Open group (or the root, when every top-level task is done) that is done → `Close`. Any `None` on a task's path → no change for that task. `status`: `merged in PR #n` / `PR #n open` / `working` / `ready` / `waiting for #a, #b` / `needs you` / `PR #n closed unmerged — needs you` / `failed — needs you` / `done`.

`pass`: `labelled(plan_label)` roots → `section_between` + `taskgraph::read` (on error: log once per root per distinct error `"#root: scheduling stopped: <e>"`, skip) → facts from `sub_issues` of the root and of each group → `pr_for_branch("forkdeck/issue-<n>")` for work tasks that are closed or carry `done_label`/`working_label`, skipping ones already in `hub.merged` (cache inserts on `Merged`) → apply `changes` (log each) → re-render statuses; write only if the section changed, with the `replace_between` + re-read guard.

- [ ] **Step 1: Failing pure tests** in `schedule.rs`:

```rust
// facts for numbered() from taskgraph tests: g0=#20 a=#21 g1=#22 b=#23 c=#24 d=#25
#[test] fn only_tasks_with_merged_dependencies_get_the_label()      // a merged → b labelled, c not; c has label → Unlabel
#[test] fn a_closed_unmerged_pr_is_not_done()                        // a: Closed(5) → b stays waiting; status(a) = "PR #5 closed unmerged — needs you"
#[test] fn unknown_facts_change_nothing()                            // a.pr = None → no change for b or c
#[test] fn a_finished_group_and_plan_are_closed()                    // all work merged, d closed completed → Close(20), Close(22), Close(root)
#[test] fn labelled_working_or_failed_tasks_are_left_alone()         // b ready but has working_label → no Label
```
Write each with concrete `HashMap<u64, Fact>` inputs and `assert_eq!(changes(...), vec![...])`.
- [ ] **Step 2: Failing integration test** `an_unreadable_list_changes_no_labels`: fake-forge root with a hand-edited section and a task carrying `agent:ready`; `pass` leaves labels as they are and logs `scheduling stopped`.
- [ ] **Step 3: Failing integration tests.** Extend Task 5's `accepting_creates_linked_sub_issues_in_order`: after accepting, only #21 ("a") carries `agent:ready` (#23 "b" waits on g0 through its group). Then `a_merge_starts_what_it_unblocked`: after Task 5's setup, add a merged pull for head `forkdeck/issue-21` into `main`; `pass` labels #23, updates #21's line to `merged in PR #…`, and a second `pass` makes no forge write (`calls.log` has no new POST/PATCH/DELETE).
- [ ] **Step 4: Run** → FAIL. **Step 5: Implement.** **Step 6: Gate** → PASS.
- [ ] **Step 7: Commit** — `feat: tasks start when their dependencies merge`

---

### Task 7: Gate at claim time

**Files:**
- Modify: `src/schedule.rs` (add `gate`), `src/issues.rs:772` (`prepare`, right after `still_requested`)

**Interfaces:**
- Produces: `pub async fn gate(hub: &Hub, r: &RepoCfg, n: u64) -> Result<(), String>` — Ok for issues without a task marker. For a task: reads the root's list and the facts of its `effective_after` targets (same `done` rules); if any is not `Some(true)`, removes `r.label` from the issue and returns `Err(format!("{}#{n} waits for {list} to merge", team::TAKEN))`, which the existing TAKEN handling turns into "not started" without a failure report. An unreadable list or forge → the same Err with reason `"its task list cannot be read: <e>"`.

- [ ] **Step 1: Failing integration test** `a_hand_labelled_blocked_task_is_not_claimed`: Task 5 setup; add `agent:ready` to #24 by hand; run one `issues::poll_once` (pass removes it — assert) and separately call `issues::prepare` on a job for #24 with the label re-added just before: it returns an error starting with `team::TAKEN`, no `agent:working` label is added, no claim comment is posted, and #24 no longer carries `agent:ready`.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Gate** → PASS.
- [ ] **Step 5: Commit** — `fix: a task never starts ahead of its dependencies`

---

### Task 8: Planner and reviewer ask for and check the task list

**Files:**
- Modify: `src/orchestrator.rs:2331` (planner prompt), `:2357` (reviewer prompt), and the loop after `plan_ready` (`:2347-2350`)

**Interfaces:**
- Consumes: `taskgraph::{extract, validate, OPEN, CLOSE, TAG}`.

Prompt addition, GitHub repos only (exact copy):
> "If this needs more than one pull request, split it: after the plan, add one fenced block tagged forkdeck-tasks containing JSON {"version":1,"tasks":[{"id","title","body","kind":"work|group|decision","parent","after":[ids],"checks":[...]}]}. Each work task must be one pull request that can merge on its own once every task in its `after` has merged. Use group tasks to collect related work (a group's `after` applies to everything in it) and decision tasks only for choices the user must make. Leave the block out when one pull request is enough."

Reviewer addition: "If the plan has a forkdeck-tasks block, check that every work task is one reviewable pull request and that its `after` lists everything that must merge first."

Loop: reject reserved `taskgraph::OPEN`/`CLOSE` like the plan markers. After that, an `extract`/`validate` error is handled like a failed review: below `max_fix_rounds` push the turn `"The task list is invalid: <e>. Fix the forkdeck-tasks block."` and revise; at the limit finish with `"The task list is invalid: <e>. Reply to refine this task."`. The reviewer is not run for an invalid list.

- [ ] **Step 1: Failing test** `an_invalid_task_list_goes_back_to_the_planner`, copied from `a_failed_plan_review_goes_back_to_the_planner`: the fake planner emits a list with a cycle until its prompt contains `The task list is invalid`, then a valid one; assert two planner runs, one review, final message `Plan review passed…`, and the saved draft's graph validates.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Gate** → PASS.
- [ ] **Step 5: Commit** — `feat: planners split multi-PR work into a task list`

---

### Task 9: End-to-end check and install

- [ ] **Step 1:** Full gate: `cargo clippy --all-targets -- -D warnings`, then `cargo test` (all suites). Expect all pass; the known `history::concurrent_open_migrates_once…` flake passes when re-run alone.
- [ ] **Step 2:** `cargo build --release && install -m755 target/release/forkdeck ~/.local/bin/forkdeck`; `forkdeck --version` shows the new build.
- [ ] **Step 3:** Manual check against a scratch GitHub repo the user names (never semantic-context): F6 plan with two dependent tasks → accept → two sub-issues, a blocked-by link, the visible list on the parent, only the first labelled. Report what was seen.
