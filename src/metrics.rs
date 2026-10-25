//! Versioned, content-free invocation snapshots. Atomic replacement makes retries idempotent.
use crate::state::{AgentKind, Job, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const ADAPTER: &str = "stream-v1";
pub fn id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
pub fn root() -> PathBuf {
    crate::cicd::state_dir().join("attempts-v1")
}

/// Stable pre-run proxies; prompt bytes are not the provider's full context size.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(default, deny_unknown_fields)]
pub struct Cohort {
    pub definition: String,
    pub category: String,
    pub complexity: String,
    pub context: String,
}
impl Cohort {
    pub fn classify(prompt: &str, tier: &str) -> Self {
        let words: Vec<_> = prompt.split(|c: char| !c.is_alphanumeric()).map(str::to_ascii_lowercase).collect();
        let category = if words.iter().any(|w| ["fix", "bug", "regression"].contains(&w.as_str())) {
            "fix"
        } else if words.iter().any(|w| ["docs", "document", "documentation"].contains(&w.as_str())) {
            "documentation"
        } else if words.iter().any(|w| ["test", "tests"].contains(&w.as_str())) {
            "tests"
        } else {
            "general"
        };
        Self {
            definition: "prompt-proxy-v1".into(),
            category: category.into(),
            complexity: tier.into(),
            context: match prompt.len() {
                0..=1024 => "prompt-small",
                1025..=4096 => "prompt-medium",
                _ => "prompt-large",
            }
            .into(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub input: Option<u64>,
    pub uncached: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub output: Option<u64>,
    pub reasoning: Option<u64>,
    pub cost_usd: Option<f64>,
}
impl Tokens {
    pub fn total(&self) -> Option<u64> {
        Some(self.input?.saturating_add(self.output?))
    }
}

/// One job/turn queue interval. Wall timestamps identify boundaries; only the
/// live process's monotonic clock measures wait, so restart downtime is excluded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueTiming {
    pub admitted_ms: i64,
    pub dispatched_ms: Option<i64>,
    pub wait_ms: Option<u64>,
    pub provenance: String,
}
impl QueueTiming {
    pub fn admitted() -> Self {
        Self { admitted_ms: chrono::Utc::now().timestamp_millis(), dispatched_ms: None,
            wait_ms: None, provenance: "process-monotonic-admission-v1".into() }
    }
    pub fn dispatch(&mut self, elapsed: std::time::Duration) {
        self.dispatched_ms = Some(chrono::Utc::now().timestamp_millis());
        self.wait_ms = Some(elapsed.as_millis().min(u64::MAX as u128) as u64);
    }
    pub fn persist(&self, job: &str, turn: usize) -> Result<(), String> {
        let root = crate::cicd::state_dir().join("queue-v1");
        write_snapshot(&root, &format!("{job}-{turn}"), self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub schema: u32,
    #[serde(default)]
    pub cohort: Cohort,
    #[serde(default)]
    pub provenance: String,
    #[serde(default)]
    pub first_output_ms: Option<u64>,
    #[serde(default)]
    pub routing_ms: Option<u64>,
    #[serde(default)]
    pub queue: Option<QueueTiming>,
    #[serde(default)]
    pub invocation_limit: String,
    pub id: String,
    pub job: String,
    pub turn: usize,
    #[serde(default)]
    pub sequence: usize,
    pub parent: Option<String>,
    pub role: String,
    pub retry: u32,
    pub repo: String,
    pub source: String,
    pub tier: String,
    pub cli: String,
    #[serde(default)]
    pub cli_version: Option<String>,
    pub configured_model: String,
    pub actual_model: Option<String>,
    pub effort: String,
    pub adapter: String,
    pub started: i64,
    pub ended: Option<i64>,
    pub wall_ms: Option<u64>,
    pub status: String,
    pub tokens: Tokens,
    pub terminal_usage: bool,
    pub decision: Option<String>,
}
impl Attempt {
    pub fn new(j: &Job, role: &str, kind: AgentKind, tier: &str, model: String, effort: String) -> Self {
        let (source, repo) = match &j.source {
            Source::Manual => ("task", j.workdir.to_string_lossy().into_owned()),
            Source::Issue { repo, .. } => ("issue", repo.clone()),
            Source::Pr { repo, .. } => ("review", repo.clone()),
            Source::Deploy { repo, .. } => ("deploy", repo.clone()),
        };
        Self {
            schema: 2,
            cohort: Cohort::classify(&j.prompt, tier),
            provenance: ADAPTER.into(),
            queue: Some(j.queue.clone()),
            first_output_ms: None,
            routing_ms: j.decision.as_ref().map(|d| d.ms.min(u64::MAX as u128) as u64),
            invocation_limit: "wall-time timeout; no hard token ceiling".into(),
            id: id(),
            job: j.metrics_id.clone(),
            turn: j.turns.len(),
            sequence: j.attempt_ids.len(),
            parent: j.attempt_ids.last().cloned(),
            role: role.into(),
            retry: j.rounds,
            repo,
            source: source.into(),
            tier: tier.into(),
            cli: kind.name().into(),
            cli_version: None,
            configured_model: model,
            actual_model: None,
            effort,
            adapter: ADAPTER.into(),
            started: crate::journal::now(),
            ended: None,
            wall_ms: None,
            status: "incomplete".into(),
            tokens: Tokens::default(),
            terminal_usage: false,
            decision: j.adaptive.as_ref().map(|d| d.id.clone()),
        }
    }
}

pub fn write_at(root: &Path, a: &Attempt) -> Result<(), String> {
    write_snapshot(root, &a.id, a)
}
fn write_snapshot(root: &Path, record_id: &str, value: &impl Serialize) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let tmp = root.join(format!("{}.{}.tmp", record_id, id()));
    let result = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(&serde_json::to_vec(value)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, root.join(format!("{}.json", record_id)))?;
        std::fs::File::open(root)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result.map_err(|e| e.to_string())
}
pub fn write(a: &Attempt) -> Result<(), String> {
    write_at(&root(), a)
}

/// Captures only metadata. Claude's final usage replaces intermediate messages;
/// Codex turn.completed is a turn delta. Duplicate terminal events within a turn are ignored.
#[derive(Default)]
pub struct Stream {
    pub first_output_ms: Option<u64>,
    pub tokens: Tokens,
    pub model: Option<String>,
    pub terminal: bool,
    closed_turn: bool,
    completed_turn: bool,
    mixed_model: bool,
    messages: BTreeMap<String, Tokens>,
}
impl Stream {
    pub fn observe(&mut self, kind: AgentKind, line: &str) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        let ty = v["type"].as_str().unwrap_or("");
        let mut changed = false;
        if let Some(m) = v.get("model").or_else(|| v.pointer("/message/model")).and_then(Value::as_str) {
            if self.model.as_deref().is_some_and(|old| old != m) {
                self.mixed_model = true;
            }
            self.model = if self.mixed_model { None } else { Some(m.to_string()) };
            changed = true;
        }
        if kind == AgentKind::Codex && ty == "turn.started" {
            self.closed_turn = false;
            self.terminal = false;
        }
        let terminal = (kind == AgentKind::Claude && ty == "result") || (kind == AgentKind::Codex && ty == "turn.completed");
        if kind == AgentKind::Claude && ty == "assistant" && !self.terminal {
            if let Some(id) = v.pointer("/message/id").and_then(Value::as_str) {
                let u = &v["message"]["usage"];
                let n = |k: &str| u[k].as_u64();
                let mut t = Tokens {
                    uncached: n("input_tokens"),
                    cache_read: n("cache_read_input_tokens"),
                    cache_write: n("cache_creation_input_tokens"),
                    output: n("output_tokens"),
                    ..Tokens::default()
                };
                t.input = t.uncached.zip(t.cache_read).zip(t.cache_write).map(|((a, b), c)| a.saturating_add(b).saturating_add(c));
                self.messages.insert(id.into(), t);
                let sum =
                    |get: fn(&Tokens) -> Option<u64>| self.messages.values().try_fold(0u64, |a, t| get(t).map(|b| a.saturating_add(b)));
                self.tokens = Tokens {
                    input: sum(|t| t.input),
                    uncached: sum(|t| t.uncached),
                    cache_read: sum(|t| t.cache_read),
                    cache_write: sum(|t| t.cache_write),
                    output: sum(|t| t.output),
                    ..Tokens::default()
                };
                changed = true;
            }
        }
        if !terminal || self.closed_turn {
            return changed;
        }
        self.closed_turn = true;
        let u = &v["usage"];
        let n = |key: &str| u[key].as_u64();
        let mut t = Tokens { input: n("input_tokens"), output: n("output_tokens"), ..Tokens::default() };
        if kind == AgentKind::Claude {
            t.uncached = t.input;
            t.cache_read = n("cache_read_input_tokens");
            t.cache_write = n("cache_creation_input_tokens");
            // Missing cache categories are not assumed to be zero in the ledger.
            t.input = match (t.uncached, t.cache_read, t.cache_write) {
                (Some(a), Some(b), Some(c)) => Some(a.saturating_add(b).saturating_add(c)),
                _ => None,
            };
            t.cost_usd = v["total_cost_usd"].as_f64().filter(|v| v.is_finite() && *v >= 0.0);
        } else {
            t.cache_read = n("cached_input_tokens");
            t.uncached = t.input.zip(t.cache_read).and_then(|(a, b)| a.checked_sub(b));
            t.reasoning = n("reasoning_output_tokens");
        }
        if kind == AgentKind::Codex && self.completed_turn {
            fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
                Some(a?.saturating_add(b?))
            }
            t.input = add(self.tokens.input, t.input);
            t.output = add(self.tokens.output, t.output);
            t.cache_read = add(self.tokens.cache_read, t.cache_read);
            t.uncached = add(self.tokens.uncached, t.uncached);
            t.reasoning = add(self.tokens.reasoning, t.reasoning);
        }
        self.tokens = t;
        self.terminal = true;
        self.completed_turn = true;
        true
    }
}

#[derive(Default, Serialize)]
pub struct Report {
    pub schema: u32,
    pub since: i64,
    pub damaged_records: usize,
    pub historical_jobs_without_attempts: usize,
    pub groups: Vec<Group>,
    pub tasks: Vec<TaskGroup>,
    pub note: String,
}
#[derive(Default, Serialize)]
pub struct Group {
    pub cli: String,
    pub model: String,
    pub effort: String,
    pub role: String,
    pub attempts: usize,
    pub complete_usage: usize,
    pub incomplete: usize,
    pub cancelled: usize,
    pub errors: usize,
    pub known_cost_usd: f64,
    pub cost_coverage: usize,
    pub known_reasoning: u64,
    pub reasoning_coverage: usize,
    pub known_input: u64,
    pub known_output: u64,
    pub known_cache_read: u64,
    pub known_cache_write: u64,
    pub cache_read_coverage: usize,
    pub cache_write_coverage: usize,
    pub median_tokens: Option<u64>,
    pub p90_tokens: Option<u64>,
    pub p90_wall_ms: Option<u64>,
}
pub fn read_at(root: &Path) -> Result<(Vec<Attempt>, usize), String> {
    let files = match std::fs::read_dir(root) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
        Err(e) => return Err(e.to_string()),
    };
    let mut out = vec![];
    let mut bad = 0;
    for f in files {
        let p = f.map_err(|e| e.to_string())?.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
        match serde_json::from_slice::<Attempt>(&bytes) {
            Ok(a) if matches!(a.schema, 1 | 2) => out.push(a),
            _ => bad += 1,
        }
    }
    Ok((out, bad))
}
pub fn percentile(v: &mut [u64], percent: usize) -> Option<u64> {
    v.sort_unstable();
    if v.is_empty() {
        None
    } else {
        Some(v[(v.len() * percent).div_ceil(100).saturating_sub(1)])
    }
}
pub fn report(since: i64, role: Option<&str>, repo: Option<&str>, entries: &[crate::journal::Entry]) -> Result<Report, String> {
    let (attempts, bad) = read_at(&root())?;
    report_from(attempts, bad, since, role, repo, entries)
}

/// Project already observed attempts without consulting storage (also used by the demo Hub).
pub fn report_from(attempts: Vec<Attempt>, bad: usize, since: i64, role: Option<&str>, repo: Option<&str>, entries: &[crate::journal::Entry]) -> Result<Report, String> {
    let task_groups = task_groups(&attempts, entries, repo);
    let mut groups: BTreeMap<(String, String, String, String), Vec<Attempt>> = BTreeMap::new();
    for a in attempts.into_iter().filter(|a| a.started >= since && role.is_none_or(|r| a.role == r) && repo.is_none_or(|r| a.repo == r)) {
        groups
            .entry((a.cli.clone(), a.actual_model.clone().unwrap_or_else(|| "unknown/default".into()), a.effort.clone(), a.role.clone()))
            .or_default()
            .push(a);
    }
    let mut report = Report { schema: 1, since, damaged_records: bad, historical_jobs_without_attempts: entries.iter().filter(|e| e.attempt_ids.is_empty() && repo.is_none_or(|r| e.repo == r)).count(), note: "Observational usage, not a model ranking. Missing usage makes totals lower bounds. Cache and reasoning are subsets; cross-provider tokens are different units. Historical job totals are not added to attempts.".into(), ..Report::default() };
    report.tasks = task_groups;
    for ((cli, model, effort, role), rows) in groups {
        let mut g = Group { cli, model, effort, role, attempts: rows.len(), ..Group::default() };
        let mut tokens = vec![];
        let mut times = vec![];
        for a in rows {
            if let Some(t) = a.tokens.total().filter(|_| a.terminal_usage && a.ended.is_some()) {
                g.complete_usage += 1;
                tokens.push(t);
            }
            g.incomplete += usize::from(a.ended.is_none() || !a.terminal_usage);
            g.cancelled += usize::from(a.status == "cancelled");
            g.errors += usize::from(a.status == "error" || a.status == "timeout");
            g.known_cost_usd += a.tokens.cost_usd.unwrap_or(0.0);
            g.cost_coverage += usize::from(a.tokens.cost_usd.is_some());
            g.known_reasoning += a.tokens.reasoning.unwrap_or(0);
            g.reasoning_coverage += usize::from(a.tokens.reasoning.is_some());
            g.known_input += a.tokens.input.unwrap_or(0);
            g.known_output += a.tokens.output.unwrap_or(0);
            g.known_cache_read += a.tokens.cache_read.unwrap_or(0);
            g.known_cache_write += a.tokens.cache_write.unwrap_or(0);
            g.cache_read_coverage += usize::from(a.tokens.cache_read.is_some());
            g.cache_write_coverage += usize::from(a.tokens.cache_write.is_some());
            if let Some(ms) = a.wall_ms {
                times.push(ms);
            }
        }
        g.median_tokens = percentile(&mut tokens, 50);
        g.p90_tokens = percentile(&mut tokens, 90);
        g.p90_wall_ms = percentile(&mut times, 90);
        report.groups.push(g);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cohort_proxies_are_deterministic_and_historical_values_stay_unknown() {
        assert_eq!(Cohort::classify("Fix a bug", "heavy").category, "fix");
        assert_eq!(Cohort::classify("Document behavior", "light").context, "prompt-small");
        assert_eq!(Cohort::classify(&"x".repeat(4097), "standard").context, "prompt-large");
        assert_eq!(Cohort::default().definition, "");
    }
    #[test]
    fn queue_restart_does_not_infer_wait_from_wall_clock() {
        const CHILD: &str = "FORKDECK_QUEUE_TEST_ROOT";
        if let Ok(root) = std::env::var(CHILD) {
            let old: QueueTiming = serde_json::from_slice(&std::fs::read(Path::new(&root).join("pending.json")).unwrap()).unwrap();
            assert!(old.dispatched_ms.is_none());
            assert!(old.wait_ms.is_none());
            let mut fresh = QueueTiming::admitted();
            fresh.dispatch(std::time::Duration::ZERO);
            assert_eq!(fresh.wait_ms, Some(0));
            assert!(fresh.admitted_ms > old.admitted_ms);
            write_snapshot(Path::new(&root), "fresh", &fresh).unwrap();
            return;
        }
        let root = std::env::temp_dir().join(id());
        let mut old = QueueTiming::admitted();
        old.admitted_ms -= 86_400_000;
        write_snapshot(&root, "pending", &old).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "metrics::tests::queue_restart_does_not_infer_wait_from_wall_clock"])
            .env(CHILD, &root).output().unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let fresh: QueueTiming = serde_json::from_slice(&std::fs::read(root.join("fresh.json")).unwrap()).unwrap();
        assert_eq!(fresh.wait_ms, Some(0));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_subsets_and_duplicate_terminal_events() {
        let mut s = Stream::default();
        let line = r#"{"type":"turn.completed","model":"x-1","usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":20,"reasoning_output_tokens":5}}"#;
        assert!(s.observe(AgentKind::Codex, line));
        s.observe(AgentKind::Codex, line);
        assert_eq!(s.tokens.total(), Some(120));
        assert_eq!(s.tokens.uncached, Some(20));
        assert_eq!(s.tokens.reasoning, Some(5));
        s.observe(AgentKind::Codex, r#"{"type":"turn.started"}"#);
        s.observe(AgentKind::Codex, line);
        assert_eq!(s.tokens.total(), Some(240));
        let mut c = Stream::default();
        c.observe(AgentKind::Claude,r#"{"type":"result","model":"c-1","total_cost_usd":0.2,"usage":{"input_tokens":10,"cache_read_input_tokens":80,"cache_creation_input_tokens":10,"output_tokens":20}}"#);
        assert_eq!(c.tokens.total(), Some(120));
        assert_eq!(c.tokens.cost_usd, Some(0.2));
    }
    #[test]
    fn unknown_is_not_zero_or_configured_model() {
        let mut s = Stream::default();
        s.observe(AgentKind::Claude, r#"{"type":"result","usage":{"input_tokens":10,"output_tokens":2}}"#);
        assert_eq!(s.tokens.input, None);
        assert_eq!(s.tokens.uncached, Some(10));
        assert_eq!(s.model, None);
        s.observe(AgentKind::Claude, r#"{"type":"assistant","message":{"model":"a"}}"#);
        s.observe(AgentKind::Claude, r#"{"type":"assistant","message":{"model":"b"}}"#);
        assert_eq!(s.model, None);
    }
    #[test]
    fn durable_snapshots_retry_without_double_counting_and_expose_corruption() {
        let root = std::env::temp_dir().join(id());
        let mut a = Attempt {
            schema: 2,
            cohort: Cohort::default(),
            provenance: ADAPTER.into(),
            first_output_ms: None,
            routing_ms: None,
            queue: None,
            invocation_limit: "wall-time timeout; no hard token ceiling".into(),
            id: id(),
            job: id(),
            turn: 1,
            sequence: 0,
            parent: None,
            role: "worker".into(),
            retry: 0,
            repo: String::new(),
            source: "task".into(),
            tier: "light".into(),
            cli: "codex".into(),
            cli_version: None,
            configured_model: "alias".into(),
            actual_model: None,
            effort: String::new(),
            adapter: ADAPTER.into(),
            started: 1,
            ended: None,
            wall_ms: None,
            status: "incomplete".into(),
            tokens: Default::default(),
            terminal_usage: false,
            decision: None,
        };
        let mut legacy = serde_json::to_value(&a).unwrap();
        legacy["schema"] = serde_json::json!(1);
        for key in ["cohort", "provenance", "first_output_ms", "routing_ms", "queue", "invocation_limit"] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        let old: Attempt = serde_json::from_value(legacy).unwrap();
        assert_eq!(old.cohort, Cohort::default());
        assert_eq!(old.routing_ms, None);
        assert!(old.queue.is_none());
        write_at(&root, &a).unwrap();
        a.tokens.input = Some(40);
        write_at(&root, &a).unwrap();
        write_at(&root, &a).unwrap();
        std::fs::write(root.join("bad.json"), "{").unwrap();
        let (rows, bad) = read_at(&root).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(bad, 1);
        assert_eq!(rows[0].tokens.input, Some(40));
        assert_eq!(rows[0].ended, None);
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Task efficiency is attributed once to the initial worker and its escalation path.
#[derive(Default, Serialize)]
pub struct TaskGroup {
    pub cohort: Cohort,
    pub complete_cost_tasks: usize,
    pub known_cost_usd: f64,
    pub reported_usd_per_accepted_task: Option<f64>,
    pub initial_model: String,
    pub effort: String,
    pub repo: String,
    pub source: String,
    pub tasks: usize,
    pub evaluated: usize,
    pub accepted: usize,
    pub first_pass: usize,
    pub unknown_quality: usize,
    pub cancelled: usize,
    pub complete_usage: usize,
    pub known_all_tokens: u64,
    pub known_evaluated_tokens: u64,
    pub known_excluded_tokens: u64,
    pub tokens_per_accepted_task_lower_bound: Option<f64>,
    pub partial_usage: bool,
    pub fix_rounds: u64,
    pub known_review_tokens: u64,
    pub known_rework_tokens: u64,
    pub median_tokens: Option<u64>,
    pub p90_tokens: Option<u64>,
    pub p90_task_secs: Option<u64>,
    pub queue_coverage: usize,
    pub p90_queue_wait_ms: Option<u64>,
}
fn task_groups(attempts: &[Attempt], entries: &[crate::journal::Entry], repo: Option<&str>) -> Vec<TaskGroup> {
    type TaskKey = (String, String, String, String, Cohort);
    type TaskRows<'a> = Vec<(&'a crate::journal::Entry, Vec<&'a Attempt>)>;
    let mut groups: BTreeMap<TaskKey, TaskRows<'_>> = BTreeMap::new();
    for entry in entries.iter().filter(|e| !e.job_id.is_empty()) {
        let mut rows: Vec<_> = attempts.iter().filter(|a| a.job == entry.job_id && a.turn == entry.turn).collect();
        rows.sort_by_key(|a| a.sequence);
        let first = rows.iter().find(|a| a.role == "worker");
        // Manual journal entries keep their legacy empty repo; attempts own metrics scope.
        let task_repo = first.map(|a| a.repo.as_str()).unwrap_or(&entry.repo);
        if repo.is_some_and(|r| r != task_repo) { continue; }
        let model = first.and_then(|a| a.actual_model.clone()).unwrap_or_else(|| "unknown/default".into());
        let effort = first.map(|a| a.effort.clone()).unwrap_or_default();
        groups
            .entry((model, effort, task_repo.to_owned(), entry.kind.clone(), first.map(|a| a.cohort.clone()).unwrap_or_default()))
            .or_default()
            .push((entry, rows));
    }
    let mut result = vec![];
    for ((initial_model, effort, repo, source, cohort), tasks) in groups {
        let mut g = TaskGroup { initial_model, effort, repo, source, cohort, ..TaskGroup::default() };
        let mut evaluated_cost = 0.0;
        let mut evaluated_cost_complete = true;
        let mut totals = vec![];
        let mut secs = vec![];
        let mut queue_times = vec![];
        for (e, rows) in tasks {
            g.tasks += 1;
            if let Some(ms) = rows.first().and_then(|a| a.queue.as_ref()).and_then(|q| q.wait_ms) {
                queue_times.push(ms);
                g.queue_coverage += 1;
            }
            g.fix_rounds += e.rounds as u64;
            secs.push(e.secs);
            let complete = !rows.is_empty()
                && rows.len() == e.attempt_ids.len()
                && rows.iter().all(|a| a.ended.is_some() && a.terminal_usage && a.tokens.total().is_some());
            let known = rows.iter().map(|a| a.tokens.input.unwrap_or(0).saturating_add(a.tokens.output.unwrap_or(0))).sum::<u64>();
            let cost = if complete {
                rows.iter().try_fold(0.0, |v, a| a.tokens.cost_usd.filter(|c| c.is_finite() && *c >= 0.0).map(|c| v + c))
            } else {
                None
            };
            g.complete_cost_tasks += usize::from(cost.is_some());
            g.known_cost_usd += rows.iter().filter_map(|a| a.tokens.cost_usd).sum::<f64>();
            g.known_all_tokens += known;
            g.complete_usage += usize::from(complete);
            if complete {
                totals.push(known);
            }
            g.known_review_tokens += rows.iter().filter(|a| a.role == "reviewer").filter_map(|a| a.tokens.total()).sum::<u64>();
            g.known_rework_tokens += rows.iter().filter(|a| a.retry > 0).filter_map(|a| a.tokens.total()).sum::<u64>();
            let cancelled = e.status == "cancelled";
            g.cancelled += usize::from(cancelled);
            let accepted = e.status == "done" && e.checks == Some(true) && e.verdict == Some(true);
            let evaluated = !cancelled && (accepted || e.checks == Some(false) || e.verdict == Some(false));
            if evaluated {
                g.evaluated += 1;
                evaluated_cost += cost.unwrap_or(0.0);
                evaluated_cost_complete &= cost.is_some();
                g.known_evaluated_tokens += known;
                if !complete {
                    g.partial_usage = true;
                }
            } else {
                g.unknown_quality += 1;
                g.known_excluded_tokens += known;
            }
            if accepted {
                g.accepted += 1;
                if e.rounds == 0 {
                    g.first_pass += 1;
                }
            }
        }
        g.reported_usd_per_accepted_task =
            if evaluated_cost_complete && g.accepted > 0 { Some(evaluated_cost / g.accepted as f64) } else { None };
        g.tokens_per_accepted_task_lower_bound =
            if g.accepted > 0 { Some(g.known_evaluated_tokens as f64 / g.accepted as f64) } else { None };
        g.median_tokens = percentile(&mut totals, 50);
        g.p90_tokens = percentile(&mut totals, 90);
        g.p90_task_secs = percentile(&mut secs, 90);
        g.p90_queue_wait_ms = percentile(&mut queue_times, 90);
        result.push(g);
    }
    result
}

#[cfg(test)]
mod outcome_tests {
    use super::*;
    #[test]
    fn errors_and_cancelled_turns_keep_known_usage_but_not_complete_coverage() {
        let mut s = Stream::default();
        let completed = r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":3}}"#;
        s.observe(AgentKind::Codex, completed);
        s.observe(AgentKind::Codex, completed);
        assert_eq!(s.tokens.total(), Some(13));
        s.observe(AgentKind::Codex, r#"{"type":"turn.started"}"#);
        s.observe(AgentKind::Codex, r#"{"type":"turn.failed","error":{"message":"cancelled"}}"#);
        assert_eq!(s.tokens.total(), Some(13));
        assert!(!s.terminal);
        s.observe(AgentKind::Codex, completed);
        assert_eq!(s.tokens.total(), Some(26));
        assert!(s.terminal);
        let mut c = Stream::default();
        for output in [2, 5] {
            c.observe(AgentKind::Claude, &serde_json::json!({"type":"assistant","message":{
                "id":"same", "usage":{"input_tokens":10,"cache_read_input_tokens":0,
                "cache_creation_input_tokens":0,"output_tokens":output}}}).to_string());
        }
        c.observe(AgentKind::Claude, r#"{"type":"error","error":{"type":"cancelled"}}"#);
        assert_eq!(c.tokens.total(), Some(15));
        assert!(!c.terminal);
        c.observe(AgentKind::Claude, r#"{"type":"result","is_error":true,"usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":6}}"#);
        assert_eq!(c.tokens.total(), Some(16));
        assert!(c.terminal); // Complete usage is separate from successful execution.
    }

    #[test]
    fn interrupted_message_snapshots_are_preserved_and_final_usage_replaces_them() {
        let mut s = Stream::default();
        let line = r#"{"type":"assistant","message":{"id":"msg-1","model":"c-1","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":2}}}"#;
        s.observe(AgentKind::Claude, line);
        s.observe(AgentKind::Claude, line);
        assert_eq!(s.tokens.total(), Some(32));
        assert!(!s.terminal);
        s.observe(AgentKind::Claude,r#"{"type":"result","usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":5}}"#);
        assert_eq!(s.tokens.total(), Some(35));
        assert!(s.terminal);
    }
    #[test]
    fn equal_end_to_end_efficiency_despite_different_acceptance_rates() {
        let mut attempts = vec![];
        let mut entries = vec![];
        for (model, tokens, accepted) in [("A", 10000, 80), ("B", 12000, 96)] {
            for n in 0..100 {
                let job = format!("{model}-{n}");
                let a:Attempt=serde_json::from_value(serde_json::json!({"schema":1,"id":job,"job":job,"turn":1,"sequence":0,"parent":null,"role":"worker","retry":0,"repo":"o/r","source":"issue","tier":"standard","cli":"claude","configured_model":model,"actual_model":model,"effort":"high","adapter":ADAPTER,"started":0,"ended":1,"wall_ms":1000,"status":"completed","tokens":{"input":tokens,"uncached":tokens,"cache_read":0,"cache_write":0,"output":0,"reasoning":null,"cost_usd":null},"terminal_usage":true,"decision":null})).unwrap();
                let e = crate::journal::Entry {
                    job_id: job.clone(),
                    turn: 1,
                    attempt_ids: vec![job],
                    repo: "o/r".into(),
                    kind: "issue".into(),
                    status: if n < accepted { "done" } else { "failed" }.into(),
                    checks: Some(n < accepted),
                    verdict: Some(n < accepted),
                    ..Default::default()
                };
                attempts.push(a);
                entries.push(e);
            }
        }
        let groups = task_groups(&attempts, &entries, None);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].accepted, 80);
        assert_eq!(groups[1].accepted, 96);
        assert!(groups.iter().all(|g| g.tokens_per_accepted_task_lower_bound == Some(12500.0) && !g.partial_usage));
        for e in &mut entries {
            e.checks = Some(false);
            e.verdict = Some(false);
            e.status = "failed".into();
        }
        assert!(task_groups(&attempts, &entries, None).iter().all(|g| g.tokens_per_accepted_task_lower_bound.is_none()));
    }
}
