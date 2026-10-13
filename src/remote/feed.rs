//! Turns the session state into numbered events for phones.
//!
//! Nothing in forkdeck announces changes (the interface just redraws), so the feed looks at
//! the state a few times a second and sends what differs from last time. Events are numbered;
//! a phone that reconnects asks for those after the last one it saw, and gets `Resync` when
//! they are no longer kept. Numbering starts at the time forkdeck started, in microseconds, so
//! a number from an earlier run is always too old and never mistaken for a current one.

use super::view;
use crate::orchestrator::Hub;
use crate::state::{Job, State};
use forkdeck_remote_proto::{Event, JobSummary, LogLine, RepoView, Snapshot, StageView, Which};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const KEEP: usize = 4096;
const SNAPSHOT_LOG: usize = 200;

struct Prev {
    summary: JobSummary,
    worker: StageView,
    reviewer: StageView,
}

struct Inner {
    seq: u64,
    ring: VecDeque<(u64, Event)>,
    jobs: HashMap<u32, Prev>,
    /// When each job started, in Unix seconds, fixed once so rounding cannot make it flicker.
    born: HashMap<u32, (Instant, u64)>,
    log_seq: u64,
    automation: bool,
}

/// An event with its sequence number.
pub type Numbered = (u64, Event);

pub struct Feed {
    inner: Mutex<Inner>,
    tx: broadcast::Sender<(u64, Event)>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Inner {
    fn created(&mut self, j: &Job) -> u64 {
        match self.born.get(&j.id) {
            Some((at, unix)) if *at == j.created => *unix,
            _ => {
                let unix = unix_now().saturating_sub(j.created.elapsed().as_secs());
                self.born.insert(j.id, (j.created, unix));
                unix
            }
        }
    }
}

impl Default for Feed {
    fn default() -> Self {
        Self::new()
    }
}

impl Feed {
    pub fn new() -> Feed {
        let start = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(1);
        let inner = Inner {
            seq: start,
            ring: VecDeque::new(),
            jobs: HashMap::new(),
            born: HashMap::new(),
            log_seq: 0,
            automation: false,
        };
        Feed { inner: Mutex::new(inner), tx: broadcast::channel(1024).0 }
    }

    /// Look at the state every `every` until the process ends.
    pub async fn run(self: std::sync::Arc<Self>, hub: Hub, every: Duration) {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            self.tick(&hub);
        }
    }

    pub fn tick(&self, hub: &Hub) {
        let mut inner = self.inner.lock().unwrap();
        let st = hub.state.lock().unwrap();
        self.step(&mut inner, &st);
    }

    fn step(&self, inner: &mut Inner, st: &State) {
        let mut out = Vec::new();
        if inner.automation != st.automation {
            inner.automation = st.automation;
            out.push(Event::Automation { on: st.automation });
        }
        if st.log_seq != inner.log_seq {
            let lines = view::log_after(st, inner.log_seq);
            inner.log_seq = st.log_seq;
            if !lines.is_empty() {
                out.push(Event::Log { lines });
            }
        }
        for j in &st.jobs {
            let created = inner.created(j);
            let cur = Prev {
                summary: view::summary(j, st.stopping, created),
                worker: view::stage(&j.worker),
                reviewer: view::stage(&j.reviewer),
            };
            let prev = inner.jobs.get(&j.id);
            if prev.map(|p| p.summary != cur.summary).unwrap_or(true) {
                out.push(Event::Job { job: cur.summary.clone() });
            }
            for (which, stage, now, before) in [
                (Which::Worker, &j.worker, &cur.worker, prev.map(|p| &p.worker)),
                (Which::Reviewer, &j.reviewer, &cur.reviewer, prev.map(|p| &p.reviewer)),
            ] {
                if before != Some(now) {
                    out.push(Event::Stage { id: j.id, which, stage: now.clone() });
                }
                if let Some((from, lines)) = view::tail_delta(before.map(|b| b.pushed).unwrap_or(0), stage) {
                    out.push(Event::Tail { id: j.id, which, from, lines });
                }
            }
            inner.jobs.insert(j.id, cur);
        }
        for e in out {
            inner.seq += 1;
            inner.ring.push_back((inner.seq, e.clone()));
            if inner.ring.len() > KEEP {
                inner.ring.pop_front();
            }
            let _ = self.tx.send((inner.seq, e));
        }
    }

    pub fn snapshot(&self, hub: &Hub) -> Snapshot {
        let mut inner = self.inner.lock().unwrap();
        let st = hub.state.lock().unwrap();
        self.step(&mut inner, &st);
        let jobs = st
            .jobs
            .iter()
            .map(|j| {
                let created = inner.created(j);
                view::summary(j, st.stopping, created)
            })
            .collect();
        let mut log: Vec<LogLine> = view::log_after(&st, 0);
        let drop = log.len().saturating_sub(SNAPSHOT_LOG);
        log.drain(..drop);
        Snapshot {
            seq: inner.seq,
            automation: st.automation,
            stopping: st.stopping,
            have_claude: st.has(crate::state::AgentKind::Claude),
            have_codex: st.has(crate::state::AgentKind::Codex),
            jobs,
            log,
            repos: st
                .repos
                .iter()
                .map(|r| RepoView { name: r.name.clone(), label: r.label.clone(), last_error: r.last_error.clone().unwrap_or_default() })
                .collect(),
        }
    }

    pub fn detail(&self, hub: &Hub, id: u32) -> Option<forkdeck_remote_proto::JobDetail> {
        let mut inner = self.inner.lock().unwrap();
        let st = hub.state.lock().unwrap();
        let j = st.jobs.iter().find(|j| j.id == id)?;
        let created = inner.created(j);
        Some(view::detail(j, st.stopping, created))
    }

    /// Events after `since` that are still kept, and a receiver for the ones to come.
    /// `since == 0` means only new ones. When the ones asked for are gone, the backlog is a
    /// single `Resync`.
    pub fn subscribe(&self, since: u64) -> (Vec<Numbered>, broadcast::Receiver<Numbered>) {
        let inner = self.inner.lock().unwrap();
        let rx = self.tx.subscribe();
        if since == 0 || since == inner.seq {
            return (Vec::new(), rx);
        }
        let oldest = inner.ring.front().map(|e| e.0).unwrap_or(inner.seq + 1);
        if since > inner.seq || since + 1 < oldest {
            return (vec![(inner.seq, Event::Resync)], rx);
        }
        (inner.ring.iter().filter(|e| e.0 > since).cloned().collect(), rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::JobStatus;
    use std::path::PathBuf;

    fn job(id: u32) -> Job {
        let rec = crate::history::JobRecord { id, title: format!("job {}", id), prompt: "p".into(), workdir: PathBuf::from("."), ..Default::default() };
        let mut j = rec.job();
        j.status = JobStatus::Working;
        j.finished = None;
        j
    }

    fn step(feed: &Feed, st: &State) {
        let mut inner = feed.inner.lock().unwrap();
        feed.step(&mut inner, st);
    }

    #[test]
    fn only_what_changed_is_sent_and_a_late_viewer_catches_up() {
        let feed = Feed::new();
        let mut st = State::default();
        st.jobs.push(job(1));
        step(&feed, &st);
        let (_, mut rx) = feed.subscribe(0);
        let mark = feed.inner.lock().unwrap().seq;

        step(&feed, &st);
        assert!(rx.try_recv().is_err(), "nothing changed, nothing sent");

        st.jobs[0].worker.push("compiling");
        st.log("queue", "hello");
        step(&feed, &st);
        let mut got = Vec::new();
        while let Ok((_, e)) = rx.try_recv() {
            got.push(e);
        }
        assert!(got.iter().any(|e| matches!(e, Event::Tail { id: 1, from: 0, lines, .. } if lines == &vec!["compiling".to_string()])));
        assert!(got.iter().any(|e| matches!(e, Event::Log { lines } if lines.len() == 1 && lines[0].msg == "hello")));
        assert!(!got.iter().any(|e| matches!(e, Event::Job { .. })), "the summary did not change");

        let (backlog, _) = feed.subscribe(mark);
        assert_eq!(backlog.len(), got.len());
        assert!(backlog.iter().all(|(seq, _)| *seq > mark));
    }

    #[test]
    fn a_number_from_another_run_or_one_long_gone_means_resync() {
        let feed = Feed::new();
        let mut st = State::default();
        st.jobs.push(job(1));
        step(&feed, &st);
        assert!(matches!(feed.subscribe(5).0.as_slice(), [(_, Event::Resync)]));
        assert!(matches!(feed.subscribe(u64::MAX - 1).0.as_slice(), [(_, Event::Resync)]));
        for i in 0..KEEP + 10 {
            st.log("t", format!("{}", i));
            step(&feed, &st);
        }
        let first = feed.inner.lock().unwrap().ring.front().unwrap().0;
        assert!(matches!(feed.subscribe(first - 5).0.as_slice(), [(_, Event::Resync)]));
        assert_eq!(feed.subscribe(first).0.len(), KEEP - 1);
    }
}
