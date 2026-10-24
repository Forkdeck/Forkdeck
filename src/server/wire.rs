// Keep flatc output untouched; its accessors are used as the board transport is wired in.
#[allow(dead_code, unused_imports, mismatched_lifetime_syntaxes, clippy::extra_unused_lifetimes)]
pub(super) mod board_generated;

use crate::{orchestrator::Hub, remote::feed::Feed};
use forkdeck_remote_proto::{BoardProjection, Event, Which};
use board_generated::forkdeck::web as fb;

fn log_entry<'a>(b: &mut flatbuffers::FlatBufferBuilder<'a>, time: &str, who: &str, text: &str) -> flatbuffers::WIPOffset<fb::LogEntry<'a>> {
    let time = b.create_string(time);
    let who = b.create_string(who);
    let text = b.create_string(text);
    fb::LogEntry::create(b, &fb::LogEntryArgs { time: Some(time), who: Some(who), text: Some(text) })
}

fn job<'a>(b: &mut flatbuffers::FlatBufferBuilder<'a>, j: &crate::state::Job, p: &BoardProjection) -> flatbuffers::WIPOffset<fb::Job<'a>> {
    let title = b.create_string(&j.title);
    let lane = b.create_string(&p.lane);
    let state = b.create_string(&p.state);
    let meta = b.create_string(&p.meta);
    let elapsed = b.create_string(&p.elapsed);
    let route = b.create_string(&p.route);
    let warn = b.create_string(&p.warn);
    let stages = b.create_vector(&p.stages);
    let log: Vec<_> = p.log.iter().map(|l| {
        let time = b.create_string(&l[0]);
        let who = b.create_string(&l[1]);
        let text = b.create_string(&l[2]);
        fb::LogEntry::create(b, &fb::LogEntryArgs { time: Some(time), who: Some(who), text: Some(text) })
    }).collect();
    let log = b.create_vector(&log);
    let kind = b.create_string(super::view::job_kind(j));
    let status = b.create_string(j.status.name());
    let agent = b.create_string(j.decision.as_ref().map(|d| d.agent.name()).unwrap_or(""));
    fb::Job::create(b, &fb::JobArgs { id: j.id, kind: Some(kind), status: Some(status), agent: Some(agent), queued: j.inbox.len() as u32, title: Some(title), lane: Some(lane), state: Some(state), meta: Some(meta), elapsed: Some(elapsed), route: Some(route), stages: Some(stages), log: Some(log), warn: Some(warn) })
}

pub fn board(hub: &Hub, feed: &Feed) -> Vec<u8> {
    let native = feed.snapshot(hub);
    let st = hub.state.lock().unwrap();
    let observed = hub.schedule_views.lock().unwrap();
    let mut b = flatbuffers::FlatBufferBuilder::new();
    let jobs: Vec<_> = st.jobs.iter().filter(|j| native.jobs.iter().any(|s| s.id == j.id)).map(|j| job(&mut b, j, &super::view::projected(j, &st, &observed))).collect();
    let jobs = b.create_vector(&jobs);
    let lanes: Vec<_> = super::view::LANES.iter().zip(hub.cfg.web.lanes.in_order()).map(|(id, label)| {
        let id = b.create_string(id);
        let label = b.create_string(label);
        fb::Lane::create(&mut b, &fb::LaneArgs { id: Some(id), label: Some(label) })
    }).collect();
    let lanes = b.create_vector(&lanes);
    // The same choices, and opaque ids, that the submit action accepts.
    let workspaces = super::write::workspaces(hub);
    let workspaces: Vec<_> = workspaces.as_array().into_iter().flatten().map(|w| {
        let id = b.create_string(w["id"].as_str().unwrap_or_default());
        let name = b.create_string(w["name"].as_str().unwrap_or_default());
        fb::Workspace::create(&mut b, &fb::WorkspaceArgs { id: Some(id), name: Some(name) })
    }).collect();
    let workspaces = b.create_vector(&workspaces);
    let workspace = std::env::current_dir().ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
    let workspace = b.create_string(&workspace);
    let kind = b.create_string(&st.router.kind);
    let label = b.create_string(&crate::ui::router_word(&st));
    let endpoint = b.create_string(if st.router.kind == "rules" { "" } else { &hub.cfg.router.jev.base_url });
    let router = fb::Router::create(&mut b, &fb::RouterArgs { kind: Some(kind), label: Some(label), endpoint: Some(endpoint), healthy: match st.router.healthy { None => 0, Some(true) => 1, Some(false) => 2 }, fallbacks: st.router.fallbacks.into(), remembered: st.router.remembered.into(), calls: st.router.calls.into(), last_ms: st.router.last_ms as u64 });
    let words: Vec<_> = crate::state::AgentKind::ALL.iter().map(|k| {
        let agent = b.create_string(k.name());
        let word = b.create_string(crate::ui::agent_word(&st, *k));
        fb::AgentWord::create(&mut b, &fb::AgentWordArgs { agent: Some(agent), word: Some(word) })
    }).collect();
    let words = b.create_vector(&words);
    let log: Vec<_> = st.log.iter().skip(st.log.len().saturating_sub(200)).map(|l| log_entry(&mut b, &l.time, &l.who, &l.msg)).collect();
    let log = b.create_vector(&log);
    let agents = fb::Agents::create(&mut b, &fb::AgentsArgs { claude: st.agent_ready(crate::state::AgentKind::Claude), codex: st.agent_ready(crate::state::AgentKind::Codex), copilot: st.copilot_usable(), words: Some(words) });
    let snapshot = fb::Snapshot::create(&mut b, &fb::SnapshotArgs { automation: native.automation, workspace: Some(workspace), capacity: hub.cfg.automation.max_parallel as u32, busy: st.jobs.iter().filter(|j| j.admitted).count() as u32, router: Some(router), agents: Some(agents), lanes: Some(lanes), jobs: Some(jobs), workspaces: Some(workspaces), running: st.jobs.iter().filter(|j| !j.status.finished() && j.status != crate::state::JobStatus::Queued).count() as u32, queued: st.jobs.iter().filter(|j| j.status == crate::state::JobStatus::Queued).count() as u32, spent_jobs: st.spent.jobs, spent_tokens: st.spent.tokens, demo: hub.cfg.demo, log: Some(log) });
    let frame = fb::Frame::create(&mut b, &fb::FrameArgs { seq: native.seq, body_type: fb::Body::Snapshot, body: Some(snapshot.as_union_value()) });
    fb::finish_frame_buffer(&mut b, frame);
    b.finished_data().to_vec()
}

pub fn event(hub: &Hub, seq: u64, event: &Event) -> Vec<u8> {
    let mut b = flatbuffers::FlatBufferBuilder::new();
    let (body_type, body) = match event {
        Event::Job { job: summary } => {
            let st = hub.state.lock().unwrap();
            let observed = hub.schedule_views.lock().unwrap();
            match st.jobs.iter().find(|j| j.id == summary.id) {
                Some(j) => {
                    let job = job(&mut b, j, &super::view::projected(j, &st, &observed));
                    (fb::Body::JobChanged, fb::JobChanged::create(&mut b, &fb::JobChangedArgs { job: Some(job) }).as_union_value())
                }
                None => (fb::Body::Changed, fb::Changed::create(&mut b, &fb::ChangedArgs {}).as_union_value()),
            }
        }
        Event::Tail { id, which, from, lines } => {
            let lines: Vec<_> = lines.iter().map(|line| b.create_string(line)).collect();
            let lines = b.create_vector(&lines);
            (fb::Body::Tail, fb::Tail::create(&mut b, &fb::TailArgs { id: *id, worker: *which == Which::Worker, from: *from, lines: Some(lines) }).as_union_value())
        }
        Event::Automation { on } => (fb::Body::Automation, fb::Automation::create(&mut b, &fb::AutomationArgs { on: *on }).as_union_value()),
        Event::Resync => (fb::Body::Resync, fb::Resync::create(&mut b, &fb::ResyncArgs {}).as_union_value()),
        Event::Log { lines } => {
            let lines: Vec<_> = lines.iter().map(|l| log_entry(&mut b, &l.time, &l.who, &l.msg)).collect();
            let lines = b.create_vector(&lines);
            (fb::Body::Log, fb::Log::create(&mut b, &fb::LogArgs { lines: Some(lines) }).as_union_value())
        }
        Event::Stage { .. } => (fb::Body::Changed, fb::Changed::create(&mut b, &fb::ChangedArgs {}).as_union_value()),
    };
    let frame = fb::Frame::create(&mut b, &fb::FrameArgs { seq, body_type, body: Some(body) });
    fb::finish_frame_buffer(&mut b, frame);
    b.finished_data().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::board_generated::forkdeck::web::*;
    use forkdeck_remote_proto::JobSummary;

    #[test]
    fn snapshot_carries_tui_state_and_recent_session_log() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let hub = seeded(&rt);
        rt.block_on(hub.reply(73, "queued reply".into())).unwrap();
        {
            let mut st = hub.state.lock().unwrap();
            st.log.clear();
            for i in 0..205 { st.log("test", format!("line {i}")); }
            st.router.kind = "jev".into();
            st.router.healthy = Some(false);
            st.router.fallbacks = 3;
            st.router.remembered = 4;
            st.spent.jobs = 5;
            st.spent.tokens = 6000;
        }
        let bytes = board(&hub, &Feed::new());
        let snap = root_as_frame(&bytes).unwrap().body_as_snapshot().unwrap();
        let st = hub.state.lock().unwrap();
        let jobs = snap.jobs().unwrap();
        let plan = jobs.iter().find(|j| j.id() == 100).expect("planning job on board");
        assert_eq!(plan.kind(), Some("plan"));
        assert_eq!(plan.lane(), Some(super::super::view::LANES[crate::ui::job_lane(st.jobs.iter().find(|j| j.id == 100).unwrap())]));
        for actual in jobs.iter() {
            let j = st.jobs.iter().find(|j| j.id == actual.id()).unwrap();
            assert_eq!(actual.status(), Some(j.status.name()));
            assert_eq!(actual.agent(), Some(j.decision.as_ref().map(|d| d.agent.name()).unwrap_or("")));
            assert_eq!(actual.queued(), j.inbox.len() as u32);
        }
        assert_eq!(jobs.iter().find(|j| j.id() == 73).unwrap().queued(), 1);
        assert_eq!((snap.running(), snap.queued()), (5, 1));
        assert_eq!(snap.running(), st.jobs.iter().filter(|j| !j.status.finished() && j.status != crate::state::JobStatus::Queued).count() as u32);
        assert_eq!(snap.queued(), st.jobs.iter().filter(|j| j.status == crate::state::JobStatus::Queued).count() as u32);
        assert_eq!((snap.spent_jobs(), snap.spent_tokens(), snap.demo()), (5, 6000, false));
        let log = snap.log().unwrap();
        assert_eq!(log.len(), 200);
        assert_eq!(log.get(0).text(), Some("line 5"));
        assert_eq!(log.get(199).text(), Some("line 204"));
        let router = snap.router().unwrap();
        assert_eq!((router.healthy(), router.fallbacks(), router.remembered()), (2, 3, 4));
        assert_eq!(router.label(), Some(crate::ui::router_word(&st).as_str()));
        assert_eq!(router.endpoint(), Some(hub.cfg.router.jev.base_url.as_str()));
        let words = snap.agents().unwrap().words().unwrap();
        assert_eq!(words.len(), 3);
        for (word, k) in words.iter().zip(crate::state::AgentKind::ALL) {
            assert_eq!((word.agent(), word.word()), (Some(k.name()), Some(crate::ui::agent_word(&st, k))));
        }
    }

    #[test]
    fn a_frame_round_trips() {
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let workspace = b.create_string("home");
        let id = b.create_string("Queue");
        let label = b.create_string("Backlog");
        let lane = Lane::create(&mut b, &LaneArgs { id: Some(id), label: Some(label) });
        let lanes = b.create_vector(&[lane]);
        let title = b.create_string("Home record API — sync ✓");
        let stages = b.create_vector(&[2u8,1,0,0,0]);
        let time = b.create_string("09:04:02");
        let who = b.create_string("worker");
        let text = b.create_string("Adding the 404 path…");
        let line = LogEntry::create(&mut b, &LogEntryArgs { time: Some(time), who: Some(who), text: Some(text) });
        let log = b.create_vector(&[line]);
        let job = Job::create(&mut b, &JobArgs { id: 72, title: Some(title), stages: Some(stages), log: Some(log), ..Default::default() });
        let jobs = b.create_vector(&[job]);
        let snap = Snapshot::create(&mut b, &SnapshotArgs { workspace: Some(workspace), lanes: Some(lanes), jobs: Some(jobs), ..Default::default() });
        let frame = Frame::create(&mut b, &FrameArgs { seq: 7, body_type: Body::Snapshot, body: Some(snap.as_union_value()) });
        finish_frame_buffer(&mut b, frame);
        let frame = root_as_frame(b.finished_data()).unwrap();
        assert_eq!(frame.seq(), 7);
        let snap = frame.body_as_snapshot().unwrap();
        assert_eq!(snap.workspace(), Some("home"));
        let lane = snap.lanes().unwrap().get(0);
        assert_eq!((lane.id(), lane.label()), (Some("Queue"), Some("Backlog")));
        let job = snap.jobs().unwrap().get(0);
        assert_eq!(job.id(), 72);
        assert_eq!(job.title(), Some("Home record API — sync ✓"));
        assert_eq!(job.stages().unwrap().bytes(), &[2,1,0,0,0]);
        let line = job.log().unwrap().get(0);
        assert_eq!((line.time(), line.who(), line.text()), (Some("09:04:02"), Some("worker"), Some("Adding the 404 path…")));
    }

    fn seeded(rt: &tokio::runtime::Runtime) -> Hub {
        let mut cfg = crate::config::Config { demo: true, ..Default::default() };
        crate::server::seed::configure(&mut cfg);
        let mut hub = Hub::new(cfg, rt.handle().clone());
        crate::server::seed::seed_demo(&hub).unwrap();
        std::sync::Arc::make_mut(&mut hub.cfg).demo = false;
        hub
    }

    #[test]
    fn the_board_frame_matches_the_json_snapshot() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let hub = seeded(&rt);
        let feed = Feed::new();
        let bytes = board(&hub, &feed);
        let frame = root_as_frame(&bytes).unwrap();
        let snap = frame.body_as_snapshot().unwrap();
        let json = crate::server::view::snapshot(&hub, &feed);
        assert_eq!(frame.seq(), json["seq"].as_u64().unwrap());
        assert_eq!(snap.automation(), json["automation"].as_bool().unwrap());
        assert_eq!(snap.workspace(), json["workspace"].as_str());
        assert_eq!(snap.capacity() as u64, json["capacity"].as_u64().unwrap());
        assert_eq!(snap.busy() as u64, json["busy"].as_u64().unwrap());
        let agents = snap.agents().unwrap();
        assert_eq!((agents.claude(), agents.codex(), agents.copilot()), (json["agents"]["claude"].as_bool().unwrap(), json["agents"]["codex"].as_bool().unwrap(), json["agents"]["copilot"].as_bool().unwrap()));
        let router = snap.router().unwrap();
        assert_eq!(router.kind(), json["router"]["kind"].as_str());
        assert_eq!(router.calls(), json["router"]["calls"].as_u64().unwrap());
        assert_eq!(router.last_ms(), json["router"]["last_ms"].as_u64().unwrap());
        let jobs = snap.jobs().unwrap();
        assert_eq!(jobs.len(), json["jobs"].as_array().unwrap().len());
        for (job, expected) in jobs.iter().zip(json["jobs"].as_array().unwrap()) {
            assert_eq!(job.id() as u64, expected["id"].as_u64().unwrap());
            for (value, key) in [(job.title(), "title"), (job.lane(), "lane"), (job.state(), "state"), (job.meta(), "meta"), (job.elapsed(), "elapsed"), (job.route(), "route"), (job.warn(), "warn")] {
                assert_eq!(value, expected[key].as_str(), "job {} {key}", job.id());
            }
            assert_eq!(serde_json::json!(job.stages().unwrap().bytes()), expected["stages"]);
            let log: Vec<_> = job.log().unwrap().iter().map(|l| serde_json::json!([l.time(), l.who(), l.text()])).collect();
            assert_eq!(serde_json::json!(log), expected["log"]);
        }
        let working = jobs.iter().find(|j| j.id() == 72).unwrap();
        assert_eq!((working.lane(), working.state()), (Some("Build"), Some("working")));
        assert_eq!(working.stages().unwrap().bytes(), &[2,1,0,0,0]);
        let waiting = jobs.iter().find(|j| j.id() == 74).unwrap();
        assert_eq!((waiting.state(), waiting.meta()), (Some("waiting"), Some("blocked by #22 · not merged")));
        let lanes: Vec<_> = snap.lanes().unwrap().iter().map(|l| serde_json::json!({"id": l.id(), "label": l.label()})).collect();
        assert_eq!(serde_json::json!(lanes), json["lanes"]);
        assert_eq!(lanes.len(), 6);
    }

    #[test]
    fn events_become_frames() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let hub = seeded(&rt);
        for (native, kind) in [
            (Event::Job { job: JobSummary { id: 72, title: "stale".into(), ..Default::default() } }, Body::JobChanged),
            (Event::Job { job: JobSummary { id: 999, ..Default::default() } }, Body::Changed),
            (Event::Job { job: JobSummary { id: 100, ..Default::default() } }, Body::JobChanged),
            (Event::Stage { id: 72, which: Which::Worker, stage: Default::default() }, Body::Changed),
            (Event::Log { lines: vec![forkdeck_remote_proto::LogLine { time: "12:00".into(), who: "issues".into(), msg: "hello".into(), ..Default::default() }] }, Body::Log),
            (Event::Resync, Body::Resync),
            (Event::Automation { on: true }, Body::Automation),
            (Event::Tail { id: 72, which: Which::Worker, from: 42, lines: vec!["— 🚀".into(), "next".into()] }, Body::Tail),
            (Event::Tail { id: 72, which: Which::Reviewer, from: 43, lines: vec![] }, Body::Tail),
        ] {
            let bytes = event(&hub, 19, &native);
            let frame = root_as_frame(&bytes).unwrap();
            assert_eq!(frame.seq(), 19);
            assert_eq!(frame.body_type(), kind);
            match native {
                Event::Job { job } if kind == Body::JobChanged => {
                    let actual = frame.body_as_job_changed().unwrap().job().unwrap();
                    assert_eq!(actual.id(), job.id);
                    if job.id == 72 {
                        assert_eq!(actual.title(), Some("semantic-context#22 Home record API"));
                        assert_eq!(actual.lane(), Some("Build"));
                    }
                }
                Event::Log { lines } => {
                    let actual = frame.body_as_log().unwrap().lines().unwrap();
                    assert_eq!(actual.len(), lines.len());
                    for (a, l) in actual.iter().zip(lines) { assert_eq!((a.time(), a.who(), a.text()), (Some(l.time.as_str()), Some(l.who.as_str()), Some(l.msg.as_str()))); }
                }
                Event::Tail { id, which, from, lines } => {
                    let tail = frame.body_as_tail().unwrap();
                    assert_eq!((tail.id(), tail.worker(), tail.from()), (id, which == Which::Worker, from));
                    assert_eq!(tail.lines().unwrap().iter().collect::<Vec<_>>(), lines);
                }
                Event::Automation { on } => assert_eq!(frame.body_as_automation().unwrap().on(), on),
                _ => {}
            }
        }
    }

    #[test]
    fn text_survives_encoding() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let hub = seeded(&rt);
        let lines: Vec<_> = (0..500).map(|i| format!("line {i} — sync · 🚀")).collect();
        hub.job(72, |j| {
            j.title = "Plans — sync · 🚀".into();
            j.worker.tail.clear();
            j.worker.tail_times.clear();
            j.worker.tail.extend(lines.iter().cloned());
        });
        let bytes = board(&hub, &Feed::new());
        let snap = root_as_frame(&bytes).unwrap().body_as_snapshot().unwrap();
        let job = snap.jobs().unwrap().iter().find(|j| j.id() == 72).unwrap();
        assert_eq!(job.title(), Some("Plans — sync · 🚀"));
        let actual: Vec<_> = job.log().unwrap().iter().filter(|l| l.who() == Some("worker")).map(|l| l.text().unwrap()).collect();
        assert_eq!(actual, lines);
    }

}
