use crate::{orchestrator::Hub, remote::feed::Feed, state::JobStatus};
use serde_json::{json, Value};

const LANES: [&str; 6] = ["Queue", "Build", "Review", "Delivery", "Done", "Attention"];

pub fn demo_jobs() -> Vec<Value> { serde_json::from_str(include_str!("demo.json")).expect("embedded demo") }

pub fn seed_demo(hub: &Hub) -> anyhow::Result<()> {
    let mut st = hub.state.lock().unwrap();
    for v in demo_jobs() {
        let id = v["id"].as_u64().unwrap() as u32;
        let rec = crate::history::JobRecord { id, title: v["title"].as_str().unwrap().into(), prompt: v["title"].as_str().unwrap().into(), ..Default::default() };
        let mut job = rec.job();
        job.status = match v["state"].as_str().unwrap() {
            "working" => JobStatus::Working, "reviewing" => JobStatus::Reviewing,
            "planning" => JobStatus::Planning, "PR open" => JobStatus::Ci,
            "done" | "unverified" => JobStatus::Done, "failed" => JobStatus::Failed,
            _ => JobStatus::Queued,
        };
        job.checks = if v["state"] == "unverified" { None } else { Some(true) };
        for line in v["log"].as_array().unwrap() {
            if line[1] == "review" { job.reviewer.push(line[2].as_str().unwrap()); }
            else { job.worker.push(line[2].as_str().unwrap()); }
        }
        st.jobs.push(job);
    }
    let mut p = crate::planning::Plan {
        provider: crate::state::Provider::Github, repo: "o/semantic-context".into(), host: String::new(),
        issue: Some(crate::tracker::IssueRef { provider: crate::state::Provider::Github, repo: "o/semantic-context".into(), key: "20".into(), url: "https://github.com/o/semantic-context/issues/20".into() }),
        identity: "demo-home".into(), revision: 3, draft: String::new(), review: String::new(),
        reviewed: None, accepted: None, published: None, base: None, remote_context: String::new(),
        pending: None, action: crate::planning::Action::Draft, ready: false, build_job: None, graph: None, creation: None,
    };
    p.repo = "o/semantic-context".into();
    p.revision = 3;
    p.reviewed = Some(3);
    p.accepted = Some(3);
    p.published = Some(3);
    let rows = [("storage", "Storage", "group", None, 26), ("records", "Persisted home records", "work", Some("storage"), 21), ("api", "API", "group", None, 27), ("endpoint", "Home record API", "work", Some("api"), 22), ("sync", "Home sync worker", "work", Some("api"), 24), ("backend", "Pick storage backend", "decision", None, 25), ("docs", "Docs for the home API", "work", None, 28)];
    let tasks: Vec<Value> = rows.iter().map(|(id, title, kind, parent, number)| json!({"id":id,"title":title,"kind":kind,"parent":parent,"number":number,"after":match *id { "api" => vec!["storage"], "sync" | "docs" => vec!["endpoint"], _ => vec![] }})).collect();
    p.graph = Some(serde_json::from_value(json!({"version":1,"tasks":tasks}))?);
    let rec = crate::history::JobRecord { id: 100, title: "Home records and sync".into(), ..Default::default() };
    let mut job = rec.job();
    job.issue_plan = Some(p);
    st.jobs.push(job);
    Ok(())
}

pub fn snapshot(hub: &Hub, feed: &Feed) -> Value {
    let native = feed.snapshot(hub);
    let mut snap = serde_json::to_value(&native).unwrap();
    let st = hub.state.lock().unwrap();
    let demo = if hub.cfg.demo { demo_jobs() } else { vec![] };
    snap["jobs"] = Value::Array(st.jobs.iter().filter_map(|j| {
        if hub.cfg.demo && j.issue_plan.is_some() { return None; }
        let summary = native.jobs.iter().find(|s| s.id == j.id)?;
        let mut v = serde_json::to_value(summary).unwrap();
        let lane = crate::ui::job_lane(j);
        let unverified = j.status == JobStatus::Done && j.checks.is_none() && j.issue_plan.is_none() && lane == 5;
        v["lane"] = json!(LANES[lane]);
        v["state"] = json!(if unverified { "unverified" } else if lane == 3 && j.status == JobStatus::Done { "PR open" } else { j.status.name() });
        v["checks"] = json!(j.checks);
        v["meta"] = json!(format!("{} · {}", summary.agent, summary.tier));
        v["elapsed"] = json!(format!("{}m", j.created.elapsed().as_secs() / 60));
        if let Some(d) = demo.iter().find(|v| v["id"] == j.id) {
            for (key, value) in d.as_object().unwrap() { v[key] = value.clone(); }
        }
        Some(v)
    }).collect());
    snap["agents"] = json!({"claude": native.have_claude, "codex": native.have_codex, "copilot": st.has(crate::state::AgentKind::Copilot)});
    snap["demo"] = json!(hub.cfg.demo);
    snap
}

pub fn detail(hub: &Hub, feed: &Feed, id: u32) -> Option<Value> {
    let detail = feed.detail(hub, id)?;
    let mut v = serde_json::to_value(&detail).ok()?;
    let board = snapshot(hub, feed);
    v["board"] = board["jobs"].as_array()?.iter().find(|j| j["id"] == id)?.clone();
    Some(v)
}

pub fn plans(hub: &Hub) -> Vec<Value> {
    let st = hub.state.lock().unwrap();
    let observed = hub.schedule_views.lock().unwrap();
    let mut plans: Vec<Value> = st.jobs.iter().filter_map(|j| {
        let p = j.issue_plan.as_ref()?;
        // Unknown forge facts stay unknown; never infer a merge from a finished local job.
        let facts = std::collections::HashMap::new();
        let tasks: Vec<Value> = p.graph.as_ref().map(|g| g.tasks.iter().map(|t| {
            let status = hub.cfg.repos.iter().find(|r| p.is_for(r)).map(|r| crate::schedule::status(r, g, t, &facts)).unwrap_or_default();
            json!({"task": t, "after": g.effective_after(&t.id), "status": if status.is_empty() { "unknown" } else { &status }})
        }).collect()).unwrap_or_default();
        let observation = p.issue.as_ref().and_then(|i| observed.get(&crate::schedule::view_key(&p.host, &p.repo, &i.key)));
        let tasks = observation.map(|v| v.tasks.clone()).unwrap_or(tasks);
        let scheduler = observation.map(|v| match &v.error {
            Some(e) => format!("Scheduler needs attention: {e}"),
            None => format!("Last pass at Unix {} · task list validated", v.observed_unix),
        }).unwrap_or_else(|| "No scheduler pass observed yet; forge status unknown".into());
        let decision = tasks.iter().find(|t| t["task"]["kind"] == "decision" && t["status"] == "needs you").map(|t| t["task"].clone());
        Some(json!({"id": j.id, "title": j.title, "status": p.status(st.automation), "plan": p, "tasks": tasks, "freshness": observation.map(|v| v.observed_unix), "scheduler": scheduler, "demo": hub.cfg.demo, "subtitle": if hub.cfg.demo { Some("semantic-context #20 · r3 · scheduling") } else { None }, "decision": decision}))
    }).collect();
    for v in observed.values() {
        if st.jobs.iter().any(|j| j.issue_plan.as_ref().is_some_and(|p| p.repo == v.repo && p.issue.as_ref() == Some(&v.issue))) { continue; }
        let decision = v.tasks.iter().find(|t| t["task"]["kind"] == "decision" && t["status"] == "needs you").map(|t| t["task"].clone());
        plans.push(json!({"id":v.id,"title":v.title,"status":"scheduling","plan":{"repo":v.repo,"revision":0,"issue":v.issue,"provider":v.issue.provider,"draft":v.draft,"review":"","graph":v.graph},"tasks":v.tasks,"freshness":v.observed_unix,"scheduler":v.error.as_deref().unwrap_or("Task list validated; last observed forge facts shown"),"decision":decision}));
    }
    if hub.cfg.demo {
        for (id, title, subtitle) in [(101, "Web UI and server mode", "Forkdeck · r1 · reviewed — accept or refine"), (102, "Unverified work never reads as done", "Forkdeck #88 · r2 · building #73")] {
            let mut p = plans[0].clone();
            p["id"] = json!(id); p["title"] = json!(title); p["subtitle"] = json!(subtitle); p["demo"] = json!(false);
            plans.push(p);
        }
    }
    plans
}
