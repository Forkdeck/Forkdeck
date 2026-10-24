// Keep flatc output untouched; its accessors are used as the board transport is wired in.
#[allow(dead_code, unused_imports, mismatched_lifetime_syntaxes, clippy::extra_unused_lifetimes)]
pub(super) mod board_generated;

#[cfg(test)]
mod tests {
    use super::board_generated::forkdeck::web::*;

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
}
