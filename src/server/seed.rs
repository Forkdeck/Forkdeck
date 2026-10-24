//! Sample runs expressed exclusively as native Hub values.
use crate::{orchestrator::Hub, state::{JobStatus, StageState}};
use serde_json::{json, Value};

pub fn seed_demo(hub: &Hub) -> anyhow::Result<()> {
    use crate::state::{AgentKind, Decision, Tier};
    use std::time::{Duration, Instant};
    let mut st = hub.state.lock().unwrap();
    let mut job = crate::history::JobRecord { id: 74, title: "semantic-context#24 Home sync worker".into(), prompt: "semantic-context#24 Home sync worker".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.source = crate::state::Source::Issue { repo_idx: 0, repo: "o/semantic-context".into(), number: 24, url: "https://github.com/o/semantic-context/issues/24".into() };
    job.status = JobStatus::Queued;
    job.created = Instant::now() - Duration::from_secs(720);
    job.worker.state = StageState::Idle;
    job.reviewer.state = StageState::Idle;
    job.outcome = "#24 waits for #22 to merge; removed agent:ready".into();
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 75, title: "Web UI: read-only board (milestone 2)".into(), prompt: "Web UI: read-only board (milestone 2)".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.status = JobStatus::Planning;
    job.created = Instant::now() - Duration::from_secs(180);
    job.decision = Some(Decision { agent: AgentKind::Claude, tier: Tier::Heavy, review: true, gates: vec![], source: "memory".into(), ms: 0, note: String::new(), plan: true, trial_from: None });
    job.worker.state = StageState::Running;
    job.reviewer.state = StageState::Idle;
    st.log("router", "#75 → claude · heavy · review yes (memory, 0 ms)");
    st.log.back_mut().unwrap().time = "09:10:41".into();
    job.worker.push_at("09:10:43", "Reading src/remote/feed.rs and crates/remote-proto…");
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 72, title: "semantic-context#22 Home record API".into(), prompt: "semantic-context#22 Home record API".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.source = crate::state::Source::Issue { repo_idx: 0, repo: "o/semantic-context".into(), number: 22, url: "https://github.com/o/semantic-context/issues/22".into() };
    job.status = JobStatus::Working;
    job.admitted = true;
    job.created = Instant::now() - Duration::from_secs(372);
    job.decision = Some(Decision { agent: AgentKind::Claude, tier: Tier::Standard, review: true, gates: vec![], source: "jev".into(), ms: 1436, note: String::new(), plan: false, trial_from: None });
    job.worker.state = StageState::Running;
    job.reviewer.state = StageState::Idle;
    st.log("issues", "#72 o/semantic-context#22 claimed, working on forkdeck/issue-22");
    st.log.back_mut().unwrap().time = "09:04:02".into();
    st.log("router", "#72 → claude · standard · review yes (jev, 1436 ms)");
    st.log.back_mut().unwrap().time = "09:04:03".into();
    job.worker.push_at("09:04:20", "▸ Read crates/helix-ha/src/home/records.rs");
    job.worker.push_at("09:05:11", "▸ Edit crates/helix-ha/src/home/api.rs");
    job.worker.push_at("09:06:40", "▸ Bash cargo test --offline --locked home::api");
    job.worker.push_at("09:07:58", "test result: ok. 14 passed; 0 failed");
    job.worker.push_at("09:08:30", "▸ Bash cargo clippy --offline --locked --all-targets -- -D warnings");
    job.worker.push_at("09:10:14", "Adding the 404 path for an unknown record id…");
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 73, title: "Forkdeck: unverified work never reads as done".into(), prompt: "Forkdeck: unverified work never reads as done".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.status = JobStatus::Reviewing;
    job.admitted = true;
    job.created = Instant::now() - Duration::from_secs(1080);
    job.decision = Some(Decision { agent: AgentKind::Claude, tier: Tier::Standard, review: true, gates: vec![], source: "jev".into(), ms: 0, note: String::new(), plan: false, trial_from: None });
    job.worker.state = StageState::Ok;
    job.reviewer.state = StageState::Running;
    job.reviewer.agent = Some(AgentKind::Codex);
    job.checks = Some(true);
    job.rounds = 1;
    st.log("checks", "#73 cargo clippy … && cargo test --bin forkdeck · passed in 41s");
    st.log.back_mut().unwrap().time = "08:58:01".into();
    job.reviewer.push_at("09:09:12", "I’ll inspect all changes since the starting commit…");
    job.reviewer.push_at("09:10:30", "▸ shell git diff 843aa9a");
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 60, title: "semantic-context#21 Persisted home records".into(), prompt: "semantic-context#21 Persisted home records".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.source = crate::state::Source::Issue { repo_idx: 0, repo: "o/semantic-context".into(), number: 21, url: "https://github.com/o/semantic-context/issues/21".into() };
    job.status = JobStatus::Ci;
    job.created = Instant::now() - Duration::from_secs(86400);
    job.decision = Some(Decision { agent: AgentKind::Codex, tier: Tier::Standard, review: true, gates: vec![], source: "jev".into(), ms: 0, note: String::new(), plan: false, trial_from: None });
    job.worker.state = StageState::Ok;
    job.reviewer.state = StageState::Ok;
    job.reviewer.agent = Some(AgentKind::Codex);
    job.verdict = Some(true);
    job.checks = Some(true);
    job.pr_url = Some("https://github.com/o/semantic-context/pull/71".into());
    job.pr_open = Some(true);
    st.log("issues", "#60 PR !71 opened, draft");
    st.log.back_mut().unwrap().time = "yesterday".into();
    st.log("ci", "#60 pipeline 4411 running: build, test, adapter");
    st.log.back_mut().unwrap().time = "08:40:02".into();
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 70, title: "Fix four ways a task could start early".into(), prompt: "Fix four ways a task could start early".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.status = JobStatus::Done;
    job.created = Instant::now() - Duration::from_secs(1080);
    job.finished = Some(Instant::now());
    job.decision = Some(Decision { agent: AgentKind::Claude, tier: Tier::Standard, review: true, gates: vec![], source: "jev".into(), ms: 0, note: String::new(), plan: false, trial_from: None });
    job.worker.state = StageState::Ok;
    job.reviewer.state = StageState::Ok;
    job.reviewer.agent = Some(AgentKind::Codex);
    job.verdict = Some(true);
    job.checks = Some(true);
    job.outcome = "committed 2803c77".into();
    job.reviewer.push_at("00:13:53", "codex: PASS");
    st.log("queue", "#70 done");
    st.log.back_mut().unwrap().time = "00:13:53".into();
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 69, title: "Planners split multi-PR work into a task list".into(), prompt: "Planners split multi-PR work into a task list".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.status = JobStatus::Done;
    job.created = Instant::now() - Duration::from_secs(540);
    job.finished = Some(Instant::now());
    job.decision = Some(Decision { agent: AgentKind::Claude, tier: Tier::Standard, review: true, gates: vec![], source: "jev".into(), ms: 0, note: String::new(), plan: false, trial_from: None });
    job.worker.state = StageState::Ok;
    job.reviewer.state = StageState::Ok;
    job.reviewer.agent = Some(AgentKind::Codex);
    job.verdict = Some(true);
    job.checks = Some(true);
    job.outcome = "committed 0e6ecf1".into();
    job.reviewer.push_at("00:13:53", "codex: PASS");
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 62, title: "Plans render a readable task list (first try)".into(), prompt: "Plans render a readable task list (first try)".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.status = JobStatus::Done;
    job.created = Instant::now() - Duration::from_secs(240);
    job.finished = Some(Instant::now());
    job.decision = Some(Decision { agent: AgentKind::Claude, tier: Tier::Standard, review: true, gates: vec![], source: "jev".into(), ms: 0, note: String::new(), plan: false, trial_from: None });
    job.worker.state = StageState::Ok;
    job.reviewer.state = StageState::Ok;
    job.reviewer.agent = Some(AgentKind::Codex);
    job.verdict = Some(true);
    job.worker.refused.push("claude was not allowed: Bash: export PATH=… cargo test".into());
    job.reviewer.push_at("21:07:43", "codex: PASS (gate results remain unverified)");
    st.log("queue", "#62 needs you: unverified");
    st.log.back_mut().unwrap().time = "21:07:43".into();
    st.jobs.push(job);
    let mut job = crate::history::JobRecord { id: 61, title: "A plan can carry a validated task list".into(), prompt: "A plan can carry a validated task list".into(), status: JobStatus::Done, ..Default::default() }.job();
    job.status = JobStatus::Failed;
    job.created = Instant::now() - Duration::from_secs(1500);
    job.finished = Some(Instant::now());
    job.decision = Some(Decision { agent: AgentKind::Claude, tier: Tier::Standard, review: true, gates: vec![], source: "jev".into(), ms: 0, note: String::new(), plan: false, trial_from: None });
    job.worker.state = StageState::Ok;
    job.reviewer.state = StageState::Failed;
    job.reviewer.agent = Some(AgentKind::Codex);
    job.verdict = Some(false);
    job.outcome = "review did not pass · fix changed nothing".into();
    job.reviewer.push_at("20:54:20", "codex: FAIL");
    job.reviewer.push_at("20:55:58", "the fix changed nothing, stopping");
    st.jobs.push(job);
    let mut p = crate::planning::Plan {
        provider: crate::state::Provider::Github, repo: "o/semantic-context".into(), host: String::new(),
        issue: Some(crate::tracker::IssueRef { provider: crate::state::Provider::Github, repo: "o/semantic-context".into(), key: "20".into(), url: "https://github.com/o/semantic-context/issues/20".into() }),
        identity: "demo-home".into(), revision: 3, draft: String::new(), review: String::new(),
        reviewed: None, accepted: None, published: None, base: None, remote_context: String::new(),
        pending: None, action: crate::planning::Action::Draft, ready: false, build_job: None, graph: None, creation: None,
    };
    p.draft = "# Home records and sync\n\n## Goal\nPersist home records and expose a stable API before adding peer synchronisation. Each task is an independently reviewable pull request.\n\n## Storage and migration\n- Add a versioned records table with an idempotent migration.\n- Keep existing installations readable during upgrade.\n- Test a second migration run against the same database.\n\n## API contract\n1. Add `GET /homes/:id` over persisted records.\n2. Return a documented 404 for an unknown record.\n3. Add the sync worker after the endpoint has merged.\n\n```rust\nfn find_home(id: HomeId) -> Result<Option<Home>, StoreError>;\n```\n\n## Verification\nRun storage migration tests, API contract tests and an interrupted-sync recovery test. See [SQLite transactions](https://www.sqlite.org/lang_transaction.html).\n\n## Rollout and risks\nShip storage first, then the API. Keep sync disabled until the backend decision is closed. Retain the prior database backup until verification completes.\n".into();
    p.review = "# Plan review\n\n## Scope and ordering\nEvery work task is one reviewable pull request. The sync worker waits for the endpoint and the backend decision.\n\n## Verification\n- Migration replay is covered.\n- Unknown-record behaviour is explicit.\n- Recovery from interrupted sync is an acceptance check.\n\n## Risk\nKeep the previous database backup until rollout verification passes.\n\nVERDICT: PASS".into();
    p.revision = 3;
    p.reviewed = Some(3);
    p.accepted = Some(3);
    p.published = Some(3);
    p.creation = Some(crate::planning::Creation { done: true, ..Default::default() });
    let rows = [("storage", "Storage", "group", None, 26), ("records", "Persisted home records", "work", Some("storage"), 21), ("api", "API", "group", None, 27), ("endpoint", "Home record API", "work", Some("api"), 22), ("sync", "Home sync worker", "work", Some("api"), 24), ("backend", "Pick storage backend", "decision", None, 25), ("docs", "Docs for the home API", "work", None, 28)];
    let tasks: Vec<Value> = rows.iter().map(|(id, title, kind, parent, number)| json!({"id":id,"title":title,"kind":kind,"parent":parent,"number":number,"after":match *id { "api" => vec!["storage"], "sync" | "docs" => vec!["endpoint"], _ => vec![] }})).collect();
    p.graph = Some(serde_json::from_value(json!({"version":1,"tasks":tasks}))?);
    p.draft.push_str(&format!("\n```forkdeck-tasks\n{}\n```\n", serde_json::to_string(p.graph.as_ref().unwrap())?));
    let rec = crate::history::JobRecord { id: 100, title: "Home records and sync".into(), status: JobStatus::Done, ..Default::default() };
    let mut job = rec.job();
    let graph = crate::taskgraph::read(&crate::taskgraph::render(p.graph.as_ref().unwrap(), &|_| String::new())).map_err(anyhow::Error::msg)?;
    let repo: crate::config::RepoCfg = toml::from_str(r#"provider = "github"
repo = "o/semantic-context"
path = "."
"#)?;
    let mut facts = std::collections::HashMap::new();
    for task in &graph.tasks {
        let number = task.number.unwrap();
        facts.insert(number, crate::schedule::Fact { open: true, completed: false, labels: if number == 22 { vec![repo.working_label.clone()] } else { vec![] }, pr: Some(if number == 21 { crate::tracker::PrState::Merged(71) } else { crate::tracker::PrState::None }) });
    }
    let tasks = graph.tasks.iter().map(|t| json!({"task":t,"after":graph.effective_after(&t.id),"status":crate::schedule::status(&repo, &graph, t, &facts)})).collect();
    hub.schedule_views.lock().unwrap().insert(crate::schedule::view_key(&p.host, &p.repo, "20"), crate::schedule::ReadView {
        id: "home".into(), title: job.title.clone(), repo: p.repo.clone(), issue: p.issue.clone().unwrap(), graph, draft: p.draft.clone(), tasks,
        observed_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() - 40, error: None,
    });
    job.status = JobStatus::Done;
    job.issue_plan = Some(p.clone());
    st.jobs.push(job);
    for (id, title, revision, reviewed, build) in [(101, "Web UI and server mode", 1, true, None), (102, "Unverified work never reads as done", 2, true, Some(73))] {
        let mut plan = p.clone();
        plan.repo = "Forkdeck".into();
        plan.identity = format!("seed-{id}");
        plan.issue = None;
        plan.revision = revision;
        plan.reviewed = reviewed.then_some(revision);
        plan.accepted = None;
        plan.published = None;
        plan.graph = None;
        plan.draft = format!("# {title}\n\n## Scope\nDeliver the next reviewable milestone while preserving existing workflows.\n\n## Implementation\n1. Inspect the current state and shared contracts.\n2. Add the missing projection and user-facing behaviour.\n3. Verify failure paths and restart behaviour.\n\n## Verification\nRun the Rust gates and the browser checks before accepting revision {revision}.\n");
        plan.review = format!("# Revision {revision} review\n\nThe scope is bounded and the verification covers the affected workflows.\n\nVERDICT: PASS");
        plan.creation = None;
        plan.build_job = build;
        let mut job = crate::history::JobRecord { id, title: title.into(), status: JobStatus::Done, ..Default::default() }.job();
        job.status = JobStatus::Done;
        job.issue_plan = Some(plan);
        st.jobs.push(job);
    }
    st.repos[0].last_poll = Some(Instant::now() - Duration::from_secs(40));
    st.repos[0].hold = Some("waiting for the single recovery probe".into());
    st.repos[0].last_error = Some("Forge rate limit on previous poll; recovered".into());
    st.team.at = Some(Instant::now() - Duration::from_secs(30));
    st.team.rows.push(crate::team::Row { repo: "o/semantic-context".into(), kind: "issue", item: "#22".into(), title: "Home record API".into(), url: "https://github.com/o/semantic-context/issues/22".into(), who: "alex".into(), host: "workstation".into(), agent: "claude".into(), stage: "working".into(), pr: String::new(), running_secs: Some(372), idle_secs: Some(20), stale: false, mine: false });
    for (id, sha, status, outcome, seconds) in [(110,"b41c92e",JobStatus::Failed,"Health check failed; rolled back to a06de31",3600),(111,"c78f0a2",JobStatus::Deploying,"Rollout: 2 of 3 replicas ready",180)] {
        let mut j = crate::history::JobRecord { id, title: format!("staging: {sha}"), status: JobStatus::Done, ..Default::default() }.job();
        j.source = crate::state::Source::Deploy { repo_idx: 0, repo: "o/semantic-context".into(), sha: sha.into() };
        j.status = status;
        j.created = Instant::now() - Duration::from_secs(seconds);
        j.finished = (status == JobStatus::Failed).then(|| Instant::now() - Duration::from_secs(3000));
        j.checks = (status == JobStatus::Failed).then_some(false);
        j.worker.label = "pipeline".into(); j.worker.state = StageState::Ok;
        j.worker.push_at("08:20:00", "Build and contract tests passed");
        j.reviewer.label = if status == JobStatus::Failed { "rollback" } else { "rollout" }.into();
        j.reviewer.state = if status == JobStatus::Failed { StageState::Ok } else { StageState::Running };
        j.reviewer.push_at("08:30:00", outcome);
        j.outcome = outcome.into();
        st.jobs.push(j);
    }
    let mut read_records = super::ReadRecords::default();
    read_records.deployments.insert(crate::cicd::saved_key(&hub.cfg.repos[0],hub.cfg.repos[0].deploy.as_ref().unwrap()),crate::cicd::Saved { seen: "b41c92e".into(),last_good:"a06de31".into(),in_progress:Some("c78f0a2".into()),tries:1,rolling_back:None });
    for (id, status, issue, pr, rounds) in [(112,JobStatus::Done,Some(19),true,0),(113,JobStatus::Failed,Some(18),false,2),(114,JobStatus::Cancelled,None,false,0)] {
        let mut j = crate::history::JobRecord { id, title: "Validate recovery and migration".into(), status: JobStatus::Done, ..Default::default() }.job();
        if let Some(number) = issue { j.source = crate::state::Source::Issue {repo_idx:0,repo:hub.cfg.repos[0].repo.clone(),number,url:format!("https://github.com/o/semantic-context/issues/{number}")}; }
        j.status = status;
        j.finished = Some(Instant::now() - Duration::from_secs(600));
        j.created = Instant::now() - Duration::from_secs(4800);
        j.active_started = j.created;
        j.verdict = (status == JobStatus::Done).then_some(true);
        j.rounds = rounds;
        if pr { j.pr_url = Some("https://github.com/o/semantic-context/pull/70".into()); }
        st.jobs.push(j);
    }
    // The journal and attempt reports consume native records, just as normal operation does.
    st.next_id = st.jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    let records = st.jobs.iter().filter(|j| j.status.finished() && j.issue_plan.is_none()).map(|j| crate::journal::entry(j,j.status,&j.outcome,0)).collect::<Vec<_>>();
    read_records.entries = records;
    let now = crate::journal::now();
    for (index,e) in read_records.entries.iter_mut().enumerate() {
        e.job_id = format!("demo-{index}");
        let id = format!("demo-attempt-{index}");
        e.attempt_ids = vec![id.clone()];
        let mut a: crate::metrics::Attempt = serde_json::from_value(json!({"schema":2,"id":id,"job":e.job_id,"turn":e.turn,"parent":null,"role":"worker","retry":e.rounds,"repo":e.repo,"source":e.kind,"tier":"standard","cli":"claude","configured_model":"sonnet","actual_model":"sonnet","effort":"medium","adapter":"demo","started":now-1200,"ended":now-60,"wall_ms":1140000,"status":"done","tokens":{"input":120000,"output":3400,"cost_usd":null},"terminal_usage":true,"decision":null}))?;
        a.queue = Some(crate::metrics::QueueTiming {admitted_ms:(now-1201)*1000,dispatched_ms:Some((now-1200)*1000),wait_ms:Some(1000),provenance:"process-monotonic-admission-v1".into()});
        a.cohort = crate::metrics::Cohort {definition:"prompt-proxy-v1".into(),category:"fix".into(),complexity:"standard".into(),context:"prompt-medium".into()};
        e.claude = crate::journal::Usage {runs:1,tokens_in:120000,tokens_out:3400,cost:0.0};
        read_records.attempts.push(a);
    }
    *hub.read_records.lock().unwrap() = Some(read_records);
    Ok(())
}

/// Configure the demo before Hub builds repository state. No fixture data lives in views.
pub fn configure(cfg: &mut crate::config::Config) {
    let repo: crate::config::RepoCfg = toml::from_str(r#"
provider = "github"
repo = "o/semantic-context"
path = "."
base = "main"
jobs_per_day = 12
[deploy]
server = "delivery"
environment = "staging"
[deploy.status]
tool = "rollout_status"
success = "healthy"
"#).expect("valid demo repository");
    cfg.repos = vec![repo];
    cfg.automation.enabled = true;
}
