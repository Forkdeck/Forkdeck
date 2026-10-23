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
    p.draft = "Goal\n\nPersist home records, expose them through the API, and keep peers in sync, delivered as reviewable pull requests that each merge on their own.\n\nSteps\n1. Storage: persisted home records with migrations.\n2. API: endpoint over the records, then the sync worker.\n3. Decision: which storage backend to commit to.".into();
    p.review = "codex · plan review · revision 3\n\nEvery work task is one reviewable pull request, and each after lists what must merge first. The sync worker correctly waits on the endpoint.\n\nVERDICT: PASS".into();
    p.revision = 3;
    p.reviewed = Some(3);
    p.accepted = Some(3);
    p.published = Some(3);
    p.creation = Some(crate::planning::Creation { done: true, ..Default::default() });
    let rows = [("storage", "Storage", "group", None, 26), ("records", "Persisted home records", "work", Some("storage"), 21), ("api", "API", "group", None, 27), ("endpoint", "Home record API", "work", Some("api"), 22), ("sync", "Home sync worker", "work", Some("api"), 24), ("backend", "Pick storage backend", "decision", None, 25), ("docs", "Docs for the home API", "work", None, 28)];
    let tasks: Vec<Value> = rows.iter().map(|(id, title, kind, parent, number)| json!({"id":id,"title":title,"kind":kind,"parent":parent,"number":number,"after":match *id { "api" => vec!["storage"], "sync" | "docs" => vec!["endpoint"], _ => vec![] }})).collect();
    p.graph = Some(serde_json::from_value(json!({"version":1,"tasks":tasks}))?);
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
        plan.creation = None;
        plan.build_job = build;
        let mut job = crate::history::JobRecord { id, title: title.into(), status: JobStatus::Done, ..Default::default() }.job();
        job.status = JobStatus::Done;
        job.issue_plan = Some(plan);
        st.jobs.push(job);
    }
    Ok(())
}
