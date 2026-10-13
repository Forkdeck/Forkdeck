//! What a phone is shown: the session state turned into wire types.

use crate::state::{Job, JobStatus, Source, Stage, StageState, State};
use forkdeck_remote_proto as p;

pub fn status(s: JobStatus) -> p::Status {
    match s {
        JobStatus::Queued => p::Status::Queued,
        // The app knows no planning stage: a plan being made is still being worked out, one waiting
        // for its choice waits, and an interrupted task has stopped.
        JobStatus::Routing | JobStatus::Planning => p::Status::Routing,
        JobStatus::Choosing => p::Status::Queued,
        JobStatus::Interrupted => p::Status::Cancelled,
        JobStatus::Working => p::Status::Working,
        JobStatus::Reviewing => p::Status::Reviewing,
        JobStatus::Fixing => p::Status::Fixing,
        JobStatus::Publishing => p::Status::Publishing,
        JobStatus::Ci => p::Status::Ci,
        JobStatus::Deploying => p::Status::Deploying,
        JobStatus::RollingBack => p::Status::RollingBack,
        JobStatus::Done => p::Status::Done,
        JobStatus::Failed => p::Status::Failed,
        JobStatus::Cancelled => p::Status::Cancelled,
    }
}

fn stage_state(s: StageState) -> p::StageState {
    match s {
        StageState::Idle => p::StageState::Idle,
        StageState::Running => p::StageState::Running,
        StageState::Ok => p::StageState::Ok,
        StageState::Failed => p::StageState::Failed,
        StageState::Skipped => p::StageState::Skipped,
    }
}

fn tri(b: Option<bool>) -> u8 {
    match b {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    }
}

pub fn summary(j: &Job, stopping: bool, created_unix: u64) -> p::JobSummary {
    let (kind, origin, url) = match &j.source {
        Source::Manual => ("manual", String::new(), String::new()),
        Source::Issue { repo, number, url, .. } => ("issue", format!("{}#{}", repo, number), url.clone()),
        Source::Pr { repo, number, url, .. } => ("pr", format!("{}!{}", repo, number), url.clone()),
        Source::Deploy { repo, sha, .. } => ("deploy", format!("{}@{}", repo, sha.chars().take(7).collect::<String>()), String::new()),
    };
    let agent = j.decision.as_ref().map(|d| d.agent).or_else(|| j.forced.as_ref().map(|r| r.kind())).map(|a| a.name().to_string()).unwrap_or_default();
    p::JobSummary {
        id: j.id,
        title: j.title.clone(),
        kind: kind.into(),
        origin,
        url,
        status: status(j.status),
        agent,
        tier: j.decision.as_ref().map(|d| d.tier.name().to_string()).unwrap_or_default(),
        created_unix,
        verdict: tri(j.verdict),
        pr_url: j.pr_url.clone().unwrap_or_default(),
        rounds: j.rounds,
        can_reply: j.source.is_manual() && j.status.finished() && !stopping,
        outcome: j.outcome.clone(),
    }
}

pub fn stage(s: &Stage) -> p::StageView {
    p::StageView {
        agent: s.agent.map(|a| a.name().to_string()).unwrap_or_default(),
        label: s.label.clone(),
        model: s.model.clone(),
        effort: s.effort.clone(),
        state: stage_state(s.state),
        last: s.last.clone(),
        tools: s.tools,
        tokens_in: s.tokens_in,
        tokens_out: s.tokens_out,
        cost: s.cost,
        secs: s.secs(),
        pushed: s.pushed,
    }
}

pub fn detail(j: &Job, stopping: bool, created_unix: u64) -> p::JobDetail {
    let decision = j.decision.as_ref().map(|d| p::DecisionView {
        agent: d.agent.name().into(),
        tier: d.tier.name().into(),
        review: d.review,
        gates: d
            .gates
            .iter()
            .map(|g| p::GateView { name: g.name.clone(), answer: g.answer.clone(), value: g.value, sharp: g.sharp })
            .collect(),
        source: d.source.clone(),
        ms: d.ms.min(u64::MAX as u128) as u64,
        note: d.note.clone(),
    });
    p::JobDetail {
        summary: summary(j, stopping, created_unix),
        prompt: j.prompt.clone(),
        has_decision: decision.is_some(),
        decision: decision.unwrap_or_default(),
        worker: stage(&j.worker),
        reviewer: stage(&j.reviewer),
        worker_tail: j.worker.tail.iter().cloned().collect(),
        reviewer_tail: j.reviewer.tail.iter().cloned().collect(),
        turns: j.turns.iter().map(|t| p::TurnView { ask: t.ask.clone(), answer: t.answer.clone() }).collect(),
        checks: tri(j.checks),
        findings: j.findings.clone(),
    }
}

/// Log lines numbered after `after`, oldest first. Lines already dropped from the log are gone.
pub fn log_after(st: &State, after: u64) -> Vec<p::LogLine> {
    let first = st.log_seq.saturating_sub(st.log.len() as u64) + 1;
    st.log
        .iter()
        .enumerate()
        .map(|(i, l)| (first + i as u64, l))
        .filter(|(seq, _)| *seq > after)
        .map(|(seq, l)| p::LogLine { seq, time: l.time.clone(), who: l.who.clone(), msg: l.msg.clone() })
        .collect()
}

/// New tail lines for a viewer that has seen `known` of them: `(from, lines)`. When the
/// stage was reset or the viewer is too far behind, the whole tail is sent and `from` is
/// where it starts; a viewer whose count differs from `from` replaces its copy.
pub fn tail_delta(known: u64, s: &Stage) -> Option<(u64, Vec<String>)> {
    if s.pushed == known {
        return None;
    }
    // Saturating: this runs under the state lock, and a panic there would take forkdeck down.
    let start = s.pushed.saturating_sub(s.tail.len() as u64);
    if s.pushed > known && known >= start {
        Some((known, s.tail.iter().skip((known - start) as usize).cloned().collect()))
    } else {
        Some((start, s.tail.iter().cloned().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_are_sent_as_increments_until_the_viewer_falls_behind() {
        let mut s = Stage::default();
        assert_eq!(tail_delta(0, &s), None);
        s.push("a");
        s.push("b");
        assert_eq!(tail_delta(0, &s), Some((0, vec!["a".into(), "b".into()])));
        assert_eq!(tail_delta(1, &s), Some((1, vec!["b".into()])));
        for i in 0..300 {
            s.push(format!("l{}", i));
        }
        // 302 pushed, the newest 200 kept: a viewer at 1 gets everything there is.
        let (from, lines) = tail_delta(1, &s).unwrap();
        assert_eq!((from, lines.len(), lines[0].as_str()), (102, 200, "l100"));
        assert_eq!(tail_delta(300, &s).unwrap().1, vec!["l298".to_string(), "l299".to_string()]);
        // Reset to a fresh stage: the viewer replaces its copy with nothing.
        let fresh = Stage::default();
        assert_eq!(tail_delta(302, &fresh), Some((0, vec![])));
        // A tail whose count went missing is sent whole rather than panicking.
        let mut odd = Stage::default();
        odd.tail.push_back("kept".into());
        assert_eq!(tail_delta(5, &odd), Some((0, vec!["kept".to_string()])));
    }

    #[test]
    fn log_lines_carry_their_position_in_the_whole_session() {
        let mut st = State::default();
        for i in 0..510 {
            st.log("t", format!("m{}", i));
        }
        let all = log_after(&st, 0);
        assert_eq!((all.len(), all[0].seq, all[0].msg.as_str()), (500, 11, "m10"));
        let new = log_after(&st, 508);
        assert_eq!(new.iter().map(|l| l.seq).collect::<Vec<_>>(), vec![509, 510]);
    }
}
