// Keep flatc output untouched; its accessors are used as the board transport is wired in.
#[allow(dead_code, unused_imports, mismatched_lifetime_syntaxes, clippy::extra_unused_lifetimes)]
pub(super) mod board_generated;

use crate::{orchestrator::Hub, remote::feed::Feed};
use forkdeck_remote_proto::{BoardProjection, Event, Which};
use board_generated::forkdeck::web as fb;

fn job<'a>(b: &mut flatbuffers::FlatBufferBuilder<'a>, id: u32, title: &str, p: &BoardProjection) -> flatbuffers::WIPOffset<fb::Job<'a>> {
    let title = b.create_string(title);
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
    fb::Job::create(b, &fb::JobArgs { id, title: Some(title), lane: Some(lane), state: Some(state), meta: Some(meta), elapsed: Some(elapsed), route: Some(route), stages: Some(stages), log: Some(log), warn: Some(warn) })
}

pub fn board(hub: &Hub, feed: &Feed) -> Vec<u8> {
    let native = feed.snapshot(hub);
    let st = hub.state.lock().unwrap();
    let observed = hub.schedule_views.lock().unwrap();
    let mut b = flatbuffers::FlatBufferBuilder::new();
    let jobs: Vec<_> = st.jobs.iter().filter(|j| j.issue_plan.is_none() && native.jobs.iter().any(|s| s.id == j.id)).map(|j| job(&mut b, j.id, &j.title, &super::view::projected(j, &st, &observed))).collect();
    let jobs = b.create_vector(&jobs);
    let lanes: Vec<_> = super::view::LANES.iter().zip(hub.cfg.web.lanes.in_order()).map(|(id, label)| {
        let id = b.create_string(id);
        let label = b.create_string(label);
        fb::Lane::create(&mut b, &fb::LaneArgs { id: Some(id), label: Some(label) })
    }).collect();
    let lanes = b.create_vector(&lanes);
    let workspace = std::env::current_dir().ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
    let workspace = b.create_string(&workspace);
    let kind = b.create_string(&st.router.kind);
    let router = fb::Router::create(&mut b, &fb::RouterArgs { kind: Some(kind), calls: st.router.calls.into(), last_ms: st.router.last_ms as u64 });
    let agents = fb::Agents::create(&mut b, &fb::AgentsArgs { claude: st.agent_ready(crate::state::AgentKind::Claude), codex: st.agent_ready(crate::state::AgentKind::Codex), copilot: st.copilot_usable() });
    let snapshot = fb::Snapshot::create(&mut b, &fb::SnapshotArgs { automation: native.automation, workspace: Some(workspace), capacity: hub.cfg.automation.max_parallel as u32, busy: st.jobs.iter().filter(|j| j.admitted).count() as u32, router: Some(router), agents: Some(agents), lanes: Some(lanes), jobs: Some(jobs) });
    let frame = fb::Frame::create(&mut b, &fb::FrameArgs { seq: native.seq, body_type: fb::Body::Snapshot, body: Some(snapshot.as_union_value()) });
    fb::finish_frame_buffer(&mut b, frame);
    b.finished_data().to_vec()
}

// The live transport uses this encoder in the next task.
#[allow(dead_code)]
pub fn event(hub: &Hub, seq: u64, event: &Event) -> Vec<u8> {
    let mut b = flatbuffers::FlatBufferBuilder::new();
    let (body_type, body) = match event {
        Event::Job { job: summary } => {
            let st = hub.state.lock().unwrap();
            let observed = hub.schedule_views.lock().unwrap();
            match st.jobs.iter().find(|j| j.id == summary.id && j.issue_plan.is_none()) {
                Some(j) => {
                    let job = job(&mut b, j.id, &j.title, &super::view::projected(j, &st, &observed));
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
        Event::Stage { .. } | Event::Log { .. } => (fb::Body::Changed, fb::Changed::create(&mut b, &fb::ChangedArgs {}).as_union_value()),
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
            (Event::Job { job: JobSummary { id: 100, ..Default::default() } }, Body::Changed),
            (Event::Stage { id: 72, which: Which::Worker, stage: Default::default() }, Body::Changed),
            (Event::Log { lines: vec![] }, Body::Changed),
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
                    assert_eq!(actual.title(), Some("semantic-context#22 Home record API"));
                    assert_eq!(actual.lane(), Some("Build"));
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
