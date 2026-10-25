//! Route memory: earlier fork decisions and how their jobs turned out. A task much like earlier
//! ones that all went well is routed the way they were, without asking the decision server; once
//! a kind of task keeps going well, the next one tries a tier lower to spend fewer tokens.
//!
//! Only word fingerprints are kept, never the task's text.

use crate::config::MemoryCfg;
use crate::state::{AgentKind, Decision, Tier};

/// One routed job, as remembered.
#[derive(Clone, Debug)]
pub struct Route {
    pub job: u32,
    pub words: Vec<u64>,
    pub agent: AgentKind,
    pub tier: Tier,
    pub decision: Decision,
    /// Where the decision came from: "jev", "memory" or "rules".
    pub source: String,
    /// Set when the job tried a tier lower than its group's: the tier it stepped down from.
    pub trial_from: Option<Tier>,
    /// Done, with checks and review passing, no fix rounds and no escalation.
    pub accepted: bool,
    pub tokens: Option<u64>,
    /// Unix seconds.
    pub finished: u64,
}

/// What memory says about a new task.
#[derive(Clone, Debug)]
pub struct Recall {
    pub decision: Decision,
    /// Try this tier instead of the decision's, to use fewer tokens.
    pub step_down: Option<Tier>,
}

/// Words too common to say anything about a task.
const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "onto", "are", "was", "were", "has", "have", "had", "but",
    "not", "you", "your", "our", "its", "it's", "can", "could", "would", "should", "will", "please", "make", "sure", "also",
    "then", "when", "there", "here", "some", "any", "all", "out", "about", "only", "just", "need", "needs", "want", "so",
];

/// FNV-1a: stable across builds, unlike the standard hasher.
fn hash(w: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in w.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The task's words, hashed: numbers, hashes and paths stand in for any of their kind, so
/// "line 12 of src/a.rs" and "line 40 of src/b.rs" read the same.
pub fn fingerprint(task: &str) -> Vec<u64> {
    let mut out: Vec<u64> = task
        .split_whitespace()
        .flat_map(|raw| {
            let w = raw.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            let shape = if w.contains('/') || (w.contains('.') && w.chars().any(|c| c.is_alphabetic())) {
                Some("<path>".to_string())
            } else if !w.is_empty() && w.chars().all(|c| c.is_ascii_digit()) {
                Some("<number>".to_string())
            } else if w.len() >= 7 && w.chars().all(|c| c.is_ascii_hexdigit()) && w.chars().any(|c| c.is_ascii_digit()) {
                Some("<hash>".to_string())
            } else {
                None
            };
            match shape {
                Some(s) => vec![s],
                None => w.split(|c: char| !c.is_alphanumeric()).filter(|t| t.len() > 1 && !STOP.contains(t)).map(String::from).collect(),
            }
        })
        .map(|w| hash(&w))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Shared words over all words (Jaccard). Both lists are sorted and without repeats.
pub fn similarity(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut shared) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    shared as f64 / (a.len() + b.len() - shared) as f64
}

/// How long a failed step-down keeps its group at the tier it stepped down from.
pub const TRIAL_FLOOR_SECS: u64 = 30 * 24 * 3600;

/// Earlier routes in the same repository and of the same kind of job, against a new task.
/// `trial_running`: a step-down for a task like this one is already under way.
pub fn recall(cfg: &MemoryCfg, past: &[Route], words: &[u64], now: u64, trial_running: bool) -> Option<Recall> {
    if !cfg.enabled {
        return None;
    }
    // Keyword rules are what memory is meant to improve on; their routes are not evidence.
    let mut near: Vec<(f64, &Route)> = past
        .iter()
        .filter(|r| r.source != "rules")
        .map(|r| (similarity(words, &r.words), r))
        .filter(|(s, _)| *s >= cfg.similarity)
        .collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.finished.cmp(&a.1.finished)));
    near.truncate(10);
    // A like task that did not go well: the decision server is asked again. A step-down that did
    // not work is the exception; it only stops further step-downs for a while.
    if near.iter().any(|(_, r)| !r.accepted && r.trial_from.is_none()) {
        return None;
    }
    let good: Vec<&(f64, &Route)> = near.iter().filter(|(_, r)| r.accepted).collect();
    let (best, top) = (good.first()?.0, good.first()?.1);
    if good.len() < cfg.min_matches || good.iter().any(|(_, r)| r.agent != top.agent) {
        return None;
    }
    // The cheapest tier that has worked for tasks like this.
    let tier = good.iter().map(|(_, r)| r.tier).min()?;
    let at_tier = good.iter().filter(|(_, r)| r.tier == tier).count();
    let floor = near.iter().any(|(_, r)| !r.accepted && r.trial_from == Some(tier) && now.saturating_sub(r.finished) < TRIAL_FLOOR_SECS);
    let step_down = (cfg.step_down && tier > Tier::Light && at_tier >= cfg.step_down_after && !floor && !trial_running).then(|| tier.down());
    let mut decision = top.decision.clone();
    decision.agent = top.agent;
    decision.tier = tier;
    decision.source = "memory".into();
    decision.ms = 0;
    decision.trial_from = None;
    let jobs: Vec<String> = good.iter().take(5).map(|(_, r)| format!("#{}", r.job)).collect();
    decision.note = format!("{} similar, best {:.2} ({})", good.len(), best, jobs.join(" "));
    Some(Recall { decision, step_down })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MemoryCfg {
        MemoryCfg { enabled: true, similarity: 0.6, min_matches: 2, step_down: true, step_down_after: 3 }
    }

    fn decision(agent: AgentKind, tier: Tier) -> Decision {
        Decision { agent, tier, review: true, gates: vec![], source: "jev".into(), ms: 40, note: String::new(), plan: false, trial_from: None }
    }

    fn route(job: u32, task: &str, tier: Tier, accepted: bool) -> Route {
        Route {
            job,
            words: fingerprint(task),
            agent: AgentKind::Claude,
            tier,
            decision: decision(AgentKind::Claude, tier),
            source: "jev".into(),
            trial_from: None,
            accepted,
            tokens: Some(1000),
            finished: 1_000,
        }
    }

    const TASK: &str = "Fix the typo in the README heading";

    #[test]
    fn fingerprints_ignore_case_numbers_paths_and_filler() {
        assert_eq!(fingerprint("Fix line 12 in src/a.rs"), fingerprint("fix LINE 40 in docs/b.md please"));
        assert_ne!(fingerprint("Fix the parser"), fingerprint("Fix the lexer"));
        assert!(similarity(&fingerprint(TASK), &fingerprint("Fix a typo in the README heading")) > 0.8);
        assert!(similarity(&fingerprint(TASK), &fingerprint("Add retries to the HTTP client")) < 0.2);
    }

    #[test]
    fn enough_accepted_neighbours_answer_for_the_decision_server() {
        let past = [route(1, TASK, Tier::Light, true), route(2, "fix typo in README heading", Tier::Light, true)];
        let r = recall(&cfg(), &past, &fingerprint("Fix a typo in the README heading"), 2_000, false).unwrap();
        assert_eq!((r.decision.agent, r.decision.tier, r.decision.source.as_str()), (AgentKind::Claude, Tier::Light, "memory"));
        assert!(r.decision.note.starts_with("2 similar"), "{}", r.decision.note);
        assert_eq!(r.step_down, None, "nothing below light");
        // One is not enough.
        assert!(recall(&cfg(), &past[..1], &fingerprint(TASK), 2_000, false).is_none());
        // Nor is anything when memory is off, or the task is unlike them.
        assert!(recall(&MemoryCfg { enabled: false, ..cfg() }, &past, &fingerprint(TASK), 2_000, false).is_none());
        assert!(recall(&cfg(), &past, &fingerprint("Add retries to the HTTP client"), 2_000, false).is_none());
    }

    #[test]
    fn one_neighbour_that_went_badly_sends_it_back_to_the_server() {
        let past = [route(1, TASK, Tier::Light, true), route(2, TASK, Tier::Light, true), route(3, TASK, Tier::Light, false)];
        assert!(recall(&cfg(), &past, &fingerprint(TASK), 2_000, false).is_none());
    }

    #[test]
    fn rules_routes_are_not_evidence_and_agents_must_agree() {
        let mut past = vec![route(1, TASK, Tier::Light, true), route(2, TASK, Tier::Light, true)];
        past[1].source = "rules".into();
        assert!(recall(&cfg(), &past, &fingerprint(TASK), 2_000, false).is_none());
        past[1].source = "jev".into();
        past[1].agent = AgentKind::Codex;
        assert!(recall(&cfg(), &past, &fingerprint(TASK), 2_000, false).is_none());
    }

    #[test]
    fn a_group_that_keeps_passing_tries_a_tier_lower_once_at_a_time() {
        let past: Vec<Route> = (1..=3).map(|i| route(i, TASK, Tier::Standard, true)).collect();
        let r = recall(&cfg(), &past, &fingerprint(TASK), 2_000, false).unwrap();
        assert_eq!((r.decision.tier, r.step_down), (Tier::Standard, Some(Tier::Light)));
        assert_eq!(recall(&cfg(), &past[..2], &fingerprint(TASK), 2_000, false).unwrap().step_down, None, "not after two");
        assert_eq!(recall(&cfg(), &past, &fingerprint(TASK), 2_000, true).unwrap().step_down, None, "one trial at a time");
        assert_eq!(recall(&MemoryCfg { step_down: false, ..cfg() }, &past, &fingerprint(TASK), 2_000, false).unwrap().step_down, None);
    }

    #[test]
    fn a_step_down_that_worked_becomes_the_tier_and_one_that_failed_holds_the_floor() {
        let mut past: Vec<Route> = (1..=3).map(|i| route(i, TASK, Tier::Standard, true)).collect();
        let mut trial = route(4, TASK, Tier::Light, true);
        trial.trial_from = Some(Tier::Standard);
        past.push(trial.clone());
        let r = recall(&cfg(), &past, &fingerprint(TASK), 2_000, false).unwrap();
        assert_eq!(r.decision.tier, Tier::Light, "the cheaper tier worked");

        past.pop();
        trial.accepted = false;
        past.push(trial);
        let r = recall(&cfg(), &past, &fingerprint(TASK), 2_000, false).unwrap();
        assert_eq!((r.decision.tier, r.step_down), (Tier::Standard, None), "a failed trial is not a failed task, but no more trials");
        let r = recall(&cfg(), &past, &fingerprint(TASK), 1_000 + TRIAL_FLOOR_SECS, false).unwrap();
        assert_eq!(r.step_down, Some(Tier::Light), "until the floor expires");
    }
}
