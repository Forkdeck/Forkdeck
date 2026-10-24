use crate::{orchestrator::Hub, remote::feed::Feed, state::{Job, JobStatus, StageState}};
use serde_json::{json, Value};

pub(super) const LANES: [&str; 6] = ["Queue", "Build", "Review", "Delivery", "Done", "Attention"];

pub fn snapshot(hub: &Hub, feed: &Feed) -> Value {
    let native = feed.snapshot(hub);
    let mut snap = serde_json::to_value(&native).unwrap();
    let st = hub.state.lock().unwrap();
    let observed = hub.schedule_views.lock().unwrap();
    snap["jobs"] = Value::Array(st.jobs.iter().filter(|j| j.issue_plan.is_none()).filter_map(|j| {
        let summary = native.jobs.iter().find(|s| s.id == j.id)?;
        let mut v = serde_json::to_value(summary).unwrap();
        let projection = projected(j, &st, &observed);
        for (key, value) in serde_json::to_value(projection).unwrap().as_object().unwrap() { v[key] = value.clone(); }
        Some(v)
    }).collect());
    snap["agents"] = json!({"claude": st.agent_ready(crate::state::AgentKind::Claude), "codex": st.agent_ready(crate::state::AgentKind::Codex), "copilot": st.copilot_usable()});
    snap["workspace"] = json!(std::env::current_dir().ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default());
    snap["workspaces"] = super::write::workspaces(hub);
    snap["capacity"] = json!(hub.cfg.automation.max_parallel);
    snap["busy"] = json!(st.jobs.iter().filter(|j| j.admitted).count());
    snap["router"] = json!({"kind":st.router.kind,"calls":st.router.calls,"last_ms":st.router.last_ms});
    // Jobs carry the lane's fixed id; what the board calls each lane is configured.
    snap["lanes"] = Value::Array(LANES.iter().zip(hub.cfg.web.lanes.in_order()).map(|(id, label)| json!({"id": id, "label": label})).collect());
    snap
}

pub(super) fn projected(j: &Job, st: &crate::state::State, observed: &std::collections::HashMap<String, crate::schedule::ReadView>) -> forkdeck_remote_proto::BoardProjection {
    let mut projection = board(j, st);
    if j.status == JobStatus::Queued {
        if let crate::state::Source::Issue { repo, number, .. } = &j.source {
            if let Some(status) = observed.values().filter(|v| &v.repo == repo).flat_map(|v| &v.tasks).find(|t| t["task"]["number"] == *number).and_then(|t| t["status"].as_str()).filter(|s| s.starts_with("waiting for ")) {
                projection.state = "waiting".into();
                projection.meta = format!("blocked by {} · not merged", status.trim_start_matches("waiting for "));
            }
        }
    }
    projection
}

pub fn detail(hub: &Hub, feed: &Feed, id: u32) -> Option<Value> {
    let detail = feed.detail(hub, id)?;
    let mut v = serde_json::to_value(&detail).ok()?;
    let board = snapshot(hub, feed);
    v["board"] = board["jobs"].as_array()?.iter().find(|j| j["id"] == id)?.clone();
    hub.job(id, |j| {
        v["revision"] = json!(super::write::revision(j));
        v["choices"] = json!(crate::choices::for_job(j).iter().map(|c| json!({"label":c.label,"detail":c.detail})).collect::<Vec<_>>());
        v["queued"] = json!(j.inbox);
        // A finished typed task continues where it ran; once that directory is gone nothing can.
        let gone = j.status.finished() && !j.workdir.is_dir();
        let manual = j.source.is_manual() && !gone;
        v["actions"] = json!({"reply":manual,"stop":!j.status.finished(),"resume":manual && j.status == JobStatus::Interrupted,"retry":manual && j.status.finished() && j.status != JobStatus::Interrupted});
        v["reply_reason"] = json!(if !j.source.is_manual() { "Only typed tasks take replies".to_string() }
            else if gone { format!("Its working directory {} no longer exists; start a new task instead", j.workdir.display()) }
            else { String::new() });
    });
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
        let tasks = task_views(observation.map(|v| v.tasks.clone()).unwrap_or(tasks), &st.jobs, &p.repo);
        let scheduler = observation.map(|v| match &v.error {
            Some(e) => format!("Scheduler needs attention: {e}"),
            None => scheduler_summary(v),
        }).unwrap_or_else(|| "No scheduler pass observed yet; forge status unknown".into());
        let decision = tasks.iter().find(|t| t["task"]["kind"] == "decision" && t["status"] == "needs you").map(|t| t["task"].clone());
        let unavailable = if hub.cfg.demo { Some("Issue publication is disabled in demo mode".to_string()) } else if !j.status.finished() { Some("Wait for the current turn to finish".to_string()) } else { None };
        let publish = unavailable.clone().or_else(|| p.clone().accept(!j.inbox.is_empty()).err());
        let ready = unavailable.clone().or_else(|| p.clone().request_ready(!j.inbox.is_empty()).err());
        let refine = if p.pending.is_some() || p.creating() { Some("Reconcile publication before refining".to_string()) } else if p.graph.is_some() && p.creation.as_ref().is_some_and(|c| c.done) { Some("Changes to a published task graph are not supported yet".to_string()) } else { None };
        Some(json!({"actions":{"publish":publish,"ready":ready,"refine":refine},"revision":super::write::revision(j),"id": j.id, "title": j.title, "status": plan_state(p), "plan": p, "draft_display": crate::taskgraph::strip(&p.draft), "tasks": tasks, "freshness": observation.map(|v| v.observed_unix), "scheduler": scheduler, "description": plan_description(p), "subtitle": format!("{}{} · r{} · {}", p.repo, p.issue.as_ref().map(|i| format!(" #{}", i.key)).unwrap_or_default(), p.revision, plan_state(p)), "decision": decision}))
    }).collect();
    for v in observed.values() {
        if st.jobs.iter().any(|j| j.issue_plan.as_ref().is_some_and(|p| p.repo == v.repo && p.issue.as_ref() == Some(&v.issue))) { continue; }
        let decision = v.tasks.iter().find(|t| t["task"]["kind"] == "decision" && t["status"] == "needs you").map(|t| t["task"].clone());
        plans.push(json!({"id":v.id,"title":v.title,"status":"scheduling","draft_display":crate::taskgraph::strip(&v.draft),"description":"Tasks start when their dependencies have merged.","plan":{"repo":v.repo,"revision":0,"issue":v.issue,"provider":v.issue.provider,"draft":v.draft,"review":"","graph":v.graph},"tasks":task_views(v.tasks.clone(), &st.jobs, &v.repo),"freshness":v.observed_unix,"scheduler":scheduler_summary(v),"decision":decision}));
    }
    plans
}

fn plan_description(p: &crate::planning::Plan) -> String {
    if let Some(g) = &p.graph {
        if !p.creation.as_ref().is_some_and(|c| c.done) { return format!("{} tasks in the graph · {}", g.tasks.len(), plan_state(p)); }
        format!("Accepted and created as {} sub-issues. Tasks start by themselves when what they depend on has merged.", g.tasks.iter().filter(|t| t.kind != crate::taskgraph::Kind::Group).count())
    } else { plan_state(p) }
}

fn duration(secs: u64) -> String {
    if secs >= 86400 { format!("{}d", secs / 86400) }
    else if secs >= 3600 { format!("{}h {}m", secs / 3600, secs % 3600 / 60) }
    else { format!("{}m {}s", secs / 60, secs % 60) }
}

fn stage(state: StageState) -> u8 {
    match state { StageState::Idle => 0, StageState::Running => 1, StageState::Ok => 2, StageState::Skipped => 3, StageState::Failed => 4 }
}

pub(super) fn board(j: &Job, st: &crate::state::State) -> forkdeck_remote_proto::BoardProjection {
    let lane = crate::ui::job_lane(j);
    let unverified = j.status == JobStatus::Done && j.checks.is_none() && j.issue_plan.is_none();
    let end = if j.status.finished() { j.finished.unwrap_or_else(std::time::Instant::now) } else { std::time::Instant::now() };
    let elapsed_secs = end.saturating_duration_since(j.created).as_secs();
    let elapsed = duration(elapsed_secs);
    let route = j.decision.as_ref().map(|d| {
        let mut line = format!("{} · {} · review {} ({}, {} ms)", d.agent.name(), d.tier.name(), if d.review { "yes" } else { "no" }, d.source, d.ms);
        if let Some(agent) = j.reviewer.agent { line.push_str(&format!(" · reviewer {}", agent.name())); }
        line
    }).unwrap_or_else(|| "not routed yet".into());
    let agent = j.decision.as_ref().map(|d| d.agent.name()).unwrap_or("unrouted");
    let tier = j.decision.as_ref().map(|d| d.tier.name()).unwrap_or("");
    let meta = if unverified {
        format!("{}nothing built or tested", if j.verdict == Some(true) { "review passed · " } else { "" })
    } else if j.status == JobStatus::Reviewing {
        format!("{} review · round {}", j.reviewer.agent.map(|a| a.name()).unwrap_or("pending"), j.rounds)
    } else if j.status == JobStatus::Planning {
        format!("plan first · {agent}")
    } else if !j.outcome.is_empty() { format!("{}{}", j.outcome, if j.status == JobStatus::Done && j.checks == Some(true) { " · checks passed" } else { "" }) }
    else if let Some(pr) = &j.pr_url {
        format!("PR #{} · {}", pr.rsplit('/').next().unwrap_or(pr), if j.status == JobStatus::Ci { "CI running" } else if j.pr_open == Some(false) { "closed" } else { "open" })
    } else { format!("{agent}{} · {}m", if tier.is_empty() { String::new() } else { format!(" · {tier}") }, elapsed_secs / 60) };
    let mut log: Vec<Vec<String>> = st.log.iter().filter(|l| {
        // Match whole job numbers: #7 must not pick up #72.
        l.msg.split(|c: char| c != '#' && !c.is_ascii_digit()).any(|word| word == format!("#{}", j.id))
    }).map(|l| vec![l.time.clone(), l.who.clone(), l.msg.clone()]).collect();
    log.extend(j.worker.tail.iter().enumerate().map(|(i, s)| vec![j.worker.tail_times.get(i).cloned().unwrap_or_default(), "worker".into(), s.clone()]));
    log.extend(j.reviewer.tail.iter().enumerate().map(|(i, s)| vec![j.reviewer.tail_times.get(i).cloned().unwrap_or_default(), "review".into(), s.clone()]));
    log.extend(j.turns.iter().flat_map(|t| [vec![String::new(), "queue".into(), t.ask.clone()], vec![String::new(), "issues".into(), t.answer.clone()]]));
    if !j.outcome.is_empty() { log.push(vec![String::new(), "queue".into(), j.outcome.clone()]); }
    let warn = if unverified {
        let refused = if j.worker.refused.is_empty() { String::new() } else { format!(" Worker was not allowed: {}.", j.worker.refused.join("; ")) };
        format!("Not built or tested: no checks ran. Review alone does not verify this work.{refused}")
    } else if j.status == JobStatus::Failed { j.outcome.clone() } else { String::new() };
    forkdeck_remote_proto::BoardProjection {
        lane: LANES[lane].into(),
        state: if unverified { "unverified" } else if lane == 3 && matches!(j.status, JobStatus::Done | JobStatus::Ci) { "PR open" } else { j.status.name() }.into(),
        meta, elapsed: if j.status == JobStatus::Queued { format!("queued {elapsed}") } else { elapsed }, route, log, warn,
        stages: vec![if j.decision.is_some() { 2 } else if j.status == JobStatus::Routing { 1 } else { 0 }, stage(j.worker.state), match j.checks { Some(true) => 2, Some(false) => 4, None if j.status.finished() => 3, None => 0 }, stage(j.reviewer.state), if j.status == JobStatus::Done && !unverified && lane == 4 { 2 } else if lane == 3 { 1 } else { 0 }],
    }
}


fn plan_state(p: &crate::planning::Plan) -> String {
    if let Some(id) = p.build_job { format!("building #{id}") }
    else if p.pending.is_some() { "publication pending".into() }
    else if p.creation.as_ref().is_some_and(|c| c.done) { "scheduling".into() }
    else if p.ready { "ready for build".into() }
    else if p.published == Some(p.revision) { "published".into() }
    else if p.reviewed == Some(p.revision) { "reviewed — accept or refine".into() }
    else { "planning — needs review".into() }
}

fn task_views(mut tasks: Vec<Value>, jobs: &[Job], repo: &str) -> Vec<Value> {
    let raw = tasks.clone();
    for row in &mut tasks {
        if row["task"]["kind"] == "group" {
            let children: Vec<_> = raw.iter().filter(|c| c["task"]["parent"] == row["task"]["id"]).collect();
            if children.iter().any(|c| c["status"].as_str().is_none_or(|s| s.is_empty() || s == "unknown")) { continue; }
            let merged = children.iter().filter(|c| c["status"].as_str().is_some_and(|s| s.starts_with("merged in PR ") || s == "done")).count();
            // Only the scheduler can attest that the whole group is complete.
            row["status"] = json!(format!("{merged} of {}{}", children.len(), if row["status"] == "done" { " merged" } else { "" }));
        } else if row["status"] == "working" {
            if let Some(job) = jobs.iter().find(|j| matches!(&j.source, crate::state::Source::Issue { repo: r, number, .. } if r == repo && Some(*number) == row["task"]["number"].as_u64()) && !j.status.finished()) {
                row["status"] = json!(format!("working · job #{}", job.id));
            }
        }
    }
    tasks
}

fn scheduler_summary(v: &crate::schedule::ReadView) -> String {
    let age = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs().saturating_sub(v.observed_unix);
    let counts = ["ready", "working", "waiting", "needs you"].map(|state| {
        let count = v.tasks.iter().filter(|t| t["task"]["kind"] != "group" && t["status"].as_str().is_some_and(|s| s.starts_with(state))).count();
        format!("{count} {state}")
    }).join(", ");
    format!("Last pass {} ago · {} · {counts}", duration(age), v.error.as_deref().unwrap_or("list matches its data (sha256 verified)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_jobs_use_the_real_projection_without_a_mode_flag() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut cfg = crate::config::Config { demo: true, ..Default::default() };
        crate::server::seed::configure(&mut cfg);
        let mut hub = Hub::new(cfg, rt.handle().clone());
        crate::server::seed::seed_demo(&hub).unwrap();
        // Opening history is avoided in this unit test; projection cannot depend on this flag.
        std::sync::Arc::make_mut(&mut hub.cfg).demo = false;
        {
            let st = hub.state.lock().unwrap();
            assert!(st.jobs.iter().all(|j| j.id < st.next_id), "new demo tasks must not reuse seeded IDs");
        }
        let feed = Feed::new();
        let snap = snapshot(&hub, &feed);
        let jobs = snap["jobs"].as_array().unwrap();
        let job = |id| jobs.iter().find(|j| j["id"] == id).unwrap();
        let working = job(72);
        assert_eq!(working["lane"], "Build");
        assert_eq!(working["state"], "working");
        assert_eq!(working["meta"], "claude · standard · 6m");
        assert_eq!(working["route"], "claude · standard · review yes (jev, 1436 ms)");
        assert_eq!(working["stages"], json!([2, 1, 0, 0, 0]));
        assert!(working["log"].as_array().unwrap().iter().any(|line| line[2] == "Adding the 404 path for an unknown record id…"));
        let unverified = job(62);
        assert_eq!(unverified["lane"], "Attention");
        assert_eq!(unverified["state"], "unverified");
        assert_eq!(unverified["meta"], "review passed · nothing built or tested");
        assert_eq!(unverified["stages"], json!([2, 2, 3, 2, 0]));
        assert!(unverified["warn"].as_str().unwrap().contains("Not built or tested"));
        assert_eq!(job(74)["state"], "waiting");
        assert_eq!(job(74)["meta"], "blocked by #22 · not merged");
        assert_eq!(job(60)["lane"], "Delivery");
        assert_eq!(job(60)["state"], "PR open");
        assert_eq!(job(70)["lane"], "Done");
        assert_eq!(job(61)["state"], "failed");
        assert_eq!(detail(&hub, &feed, 72).unwrap()["board"], *working);
        let plans = plans(&hub);
        assert_eq!(plans.len(), 3);
        let tasks = plans[0]["tasks"].as_array().unwrap();
        assert_eq!(tasks[1]["status"], "merged in PR #71");
        assert_eq!(tasks[3]["status"], "working · job #72");
        assert_eq!(tasks[4]["status"], "waiting for #22");
        assert_eq!(plans[0]["decision"]["number"], 25);
        // Mutating native state must change the projection immediately, even for a sample id.
        hub.job(72, |j| { j.status = JobStatus::Done; j.finished = Some(std::time::Instant::now()); j.checks = None; });
        let changed = snapshot(&hub, &feed);
        assert_eq!(changed["jobs"].as_array().unwrap().iter().find(|j| j["id"] == 72).unwrap()["state"], "unverified");
    }

    #[test]
    fn lane_names_come_from_the_configuration_and_ids_stay_fixed() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut cfg = crate::config::Config::default();
        cfg.web.lanes.queue = "Backlog".into();
        cfg.web.lanes.attention = "Needs a human".into();
        cfg.web.check().unwrap();
        let hub = Hub::new(cfg, rt.handle().clone());
        let snap = snapshot(&hub, &Feed::new());
        let lanes = snap["lanes"].as_array().unwrap();
        assert_eq!(lanes.len(), 6);
        assert_eq!(lanes[0], json!({"id": "Queue", "label": "Backlog"}));
        assert_eq!(lanes[1], json!({"id": "Build", "label": "Build"}));
        assert_eq!(lanes[5], json!({"id": "Attention", "label": "Needs a human"}));
        let mut blank = crate::config::Config::default();
        blank.web.lanes.done = "  ".into();
        assert!(blank.web.check().unwrap_err().to_string().contains("web.lanes.done"));
    }

    #[test]
    fn replies_are_offered_only_where_the_server_will_take_them() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut cfg = crate::config::Config { demo: true, ..Default::default() };
        crate::server::seed::configure(&mut cfg);
        let hub = Hub::new(cfg, rt.handle().clone());
        crate::server::seed::seed_demo(&hub).unwrap();
        let feed = Feed::new();
        let typed = detail(&hub, &feed, 70).unwrap();
        assert_eq!((typed["actions"]["reply"].as_bool(), typed["actions"]["retry"].as_bool()), (Some(true), Some(true)));
        assert_eq!(typed["reply_reason"], "");
        hub.job(70, |j| j.workdir = std::path::PathBuf::from("/nonexistent/forkdeck-workdir"));
        let gone = detail(&hub, &feed, 70).unwrap();
        assert_eq!((gone["actions"]["reply"].as_bool(), gone["actions"]["retry"].as_bool()), (Some(false), Some(false)));
        assert!(gone["reply_reason"].as_str().unwrap().contains("/nonexistent/forkdeck-workdir no longer exists"));
        assert_eq!(detail(&hub, &feed, 72).unwrap()["reply_reason"], "Only typed tasks take replies");
    }

    #[test]
    fn missing_checks_are_not_inferred_from_a_finished_worker_or_review() {
        let mut job = crate::history::JobRecord { id: 900, title: "Real task".into(), ..Default::default() }.job();
        job.status = JobStatus::Done;
        job.worker.state = StageState::Ok;
        job.reviewer.state = StageState::Ok;
        job.verdict = Some(true);
        let st = crate::state::State::default();
        let view = board(&job, &st);
        assert_eq!((view.lane.as_str(), view.state.as_str()), ("Attention", "unverified"));
        assert_eq!(view.stages, vec![0, 2, 3, 2, 0]);
        job.checks = Some(true);
        job.pr_url = Some("https://example.invalid/pull/1".into());
        let view = board(&job, &st);
        assert_eq!((view.lane.as_str(), view.state.as_str()), ("Delivery", "PR open"));
        assert!(view.warn.is_empty());
        job.pr_open = Some(false);
        assert_eq!(board(&job, &st).lane, "Done");
    }
}
