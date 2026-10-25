//! Fail-closed worker model selection. Evidence is paired benchmark data, never production rankings.
use crate::{
    metrics,
    orchestrator::Hub,
    state::{AgentKind, Decision as Route, JobStatus, Source},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    Advisory,
    Shadow,
    Adaptive,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    #[default]
    Tokens,
    ReportedCostUsd,
}
fn spend(objective: Objective, tokens: Option<u64>, cost: Option<f64>) -> Option<f64> {
    match objective {
        Objective::Tokens => tokens.map(|v| v as f64),
        Objective::ReportedCostUsd => cost.filter(|v| v.is_finite() && *v >= 0.0),
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The fingerprint of the configuration as written, taken before anything is filled in at
    /// run time (discovered models, the repository forkdeck was started in).
    #[serde(skip)]
    pub written: Option<String>,
    pub mode: Mode,
    pub objective: Objective,
    pub cohorts: Vec<metrics::Cohort>,
    pub invocation_max_usd: Option<f64>,
    pub policy: String,
    pub promotion_from: String,
    pub evidence: PathBuf,
    pub candidates: Vec<Choice>,
    pub repositories: Vec<String>,
    pub sources: Vec<String>,
    pub tiers: Vec<String>,
    pub max_prompt_bytes: usize,
    pub min_tasks: usize,
    pub min_coverage: f64,
    pub quality_floor: f64,
    pub noninferiority: f64,
    pub max_p90_ms: u64,
    pub trial_probability: f64,
    pub trial_tasks: usize,
    pub trial_tokens: u64,
    pub reserve_tokens: u64,
    pub window_secs: i64,
    pub monitor_tasks: usize,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub model: String,
    pub effort: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub task: String,
    pub baseline_accepted: bool,
    pub candidate_accepted: bool,
    pub baseline_tokens: Option<u64>,
    pub candidate_tokens: Option<u64>,
    pub candidate_ms: u64,
    #[serde(default)]
    pub baseline_cost_usd: Option<f64>,
    #[serde(default)]
    pub candidate_cost_usd: Option<f64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(default)]
    pub cohort: metrics::Cohort,
    #[serde(default)]
    pub fixture: bool,
    pub schema: u32,
    pub version: String,
    pub expires: i64,
    pub created: i64,
    pub fingerprint: String,
    pub cli: AgentKind,
    pub cli_version: String,
    pub reviewer_version: String,
    pub reviewer_actual_model: String,
    pub baseline: Choice,
    pub baseline_actual_model: String,
    pub candidate: Choice,
    pub candidate_actual_model: String,
    pub repo: String,
    pub source: String,
    pub tier: String,
    pub max_prompt_bytes: usize,
    /// Benchmark protocol identity, including tools, base revisions, context, checks and reviewer.
    pub protocol: String,
    pub observations: Vec<Observation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Qualification {
    #[serde(default)]
    pub objective: Objective,
    pub tasks: usize,
    pub coverage: f64,
    pub baseline_success: f64,
    pub candidate_success: f64,
    pub quality_difference_lower: f64,
    pub savings_lower: f64,
    pub savings_upper: f64,
    pub predicted_savings: f64,
    pub candidate_p90_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub job: String,
    pub turn: usize,
    pub policy: String,
    pub evidence: String,
    pub policy_snapshot: Config,
    pub evidence_snapshot: String,
    pub cohort: String,
    pub parameters: String,
    pub mode: Mode,
    pub baseline: Choice,
    pub proposed: Option<Choice>,
    pub executed: Choice,
    pub candidate_actual_model: String,
    pub baseline_actual_model: String,
    pub reviewer_actual_model: String,
    pub probability: f64,
    pub draw: f64,
    pub reason: String,
    pub qualification: Option<Qualification>,
    pub reserved: u64,
    pub candidate: bool,
    pub completed: bool,
    pub tokens: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    pub accepted: Option<bool>,
    pub wall_ms: Option<u64>,
    pub created: i64,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    pub schema: u32,
    #[serde(default)]
    pub disabled: bool,
    pub windows: BTreeMap<String, i64>,
    pub decisions: Vec<Decision>,
    pub suspended: BTreeMap<String, String>,
    pub validated: BTreeMap<String, usize>,
}

pub fn fingerprint(cfg: &crate::config::Config) -> String {
    if let Some(f) = &cfg.adaptive.written {
        return f.clone();
    }
    // Stable FNV-1a, not DefaultHasher (which has no stable-format guarantee).
    // Deliberately includes all prompt/check/review/adapter settings. No prompts or secrets stored.
    let input =
        format!("{:?}|{:?}|{:?}|{:?}|{}|{}", cfg.agents, cfg.review, cfg.knowledge, cfg.repos, metrics::ADAPTER, env!("CARGO_PKG_VERSION"));
    let mut h = 0xcbf29ce484222325u64;
    let input = format!("{input}|{:?}", cfg.adaptive.invocation_max_usd);
    for b in input
        .bytes()
        .chain(include_str!("orchestrator.rs").bytes())
        .chain(include_str!("agents.rs").bytes())
        .chain(include_str!("metrics.rs").bytes())
        .chain(include_str!("benchmark.rs").bytes())
    {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
fn evidence_digest(e: &Evidence) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in serde_json::to_vec(e).unwrap_or_default() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
fn wilson(p: f64, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let n = n as f64;
    let z = 1.96f64;
    let den = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / den;
    let radius = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / den;
    (center - radius, center + radius)
}
fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn ratio(rows: &[[f64; 4]], indices: impl Iterator<Item = usize>) -> Option<(f64, f64)> {
    let mut s = [0.0; 4];
    let mut n = 0;
    for i in indices {
        for (total, v) in s.iter_mut().zip(rows[i]) {
            *total += v;
        }
        n += 1;
    }
    if s[0] == 0.0 || s[1] == 0.0 || n == 0 {
        return None;
    }
    Some(((s[1] - s[0]) / n as f64, 1.0 - (s[3] / s[1]) / (s[2] / s[0])))
}
pub fn qualify(c: &Config, e: &Evidence, now: i64) -> Result<Qualification, String> {
    if !matches!(e.schema, 1 | 2)
        || e.version.is_empty()
        || e.protocol.is_empty()
        || e.created > now
        || e.expires <= now
        || e.expires <= e.created
    {
        return Err("missing or stale benchmark evidence".into());
    }
    if e.fixture {
        return Err("fixture evidence cannot qualify production models".into());
    }
    if !c.cohorts.is_empty() && !c.cohorts.contains(&e.cohort) {
        return Err("cohort definition incompatible".into());
    }
    if c.objective == Objective::ReportedCostUsd
        && e.observations.iter().any(|o| {
            spend(c.objective, o.baseline_tokens, o.baseline_cost_usd).is_none()
                || spend(c.objective, o.candidate_tokens, o.candidate_cost_usd).is_none()
        })
    {
        return Err("complete comparable reported USD costs required".into());
    }
    if e.cli_version.is_empty()
        || e.reviewer_version.is_empty()
        || e.reviewer_actual_model.is_empty()
        || e.baseline_actual_model.is_empty()
        || e.candidate_actual_model.is_empty()
        || e.baseline.model.is_empty()
        || e.candidate.model.is_empty()
    {
        return Err("unresolved benchmark model".into());
    }
    if c.min_tasks < 30
        || !(0.95..=1.0).contains(&c.min_coverage)
        || !(0.0..=0.05).contains(&c.noninferiority)
        || !(0.0..=1.0).contains(&c.quality_floor)
        || c.quality_floor == 0.0
        || c.max_p90_ms == 0
    {
        return Err("explicit quality, coverage, latency and sample gates required".into());
    }
    let mut grouped: BTreeMap<&str, Vec<&Observation>> = BTreeMap::new();
    for o in &e.observations {
        if o.task.is_empty() {
            return Err("empty benchmark task ID".into());
        }
        grouped.entry(&o.task).or_default().push(o);
    }
    if grouped.len() < c.min_tasks {
        return Err("insufficient distinct benchmark tasks".into());
    }
    let complete = e.observations.iter().filter(|o| o.baseline_tokens.is_some() && o.candidate_tokens.is_some()).count();
    let coverage = complete as f64 / e.observations.len() as f64;
    // Partial spend is a lower bound and cannot establish an efficiency winner.
    if coverage < c.min_coverage || complete != e.observations.len() {
        return Err("incomplete benchmark token coverage".into());
    }
    let mut rows = vec![];
    let mut latency = vec![];
    for values in grouped.values() {
        let n = values.len() as f64;
        rows.push([
            values.iter().filter(|o| o.baseline_accepted).count() as f64 / n,
            values.iter().filter(|o| o.candidate_accepted).count() as f64 / n,
            values.iter().map(|o| spend(c.objective, o.baseline_tokens, o.baseline_cost_usd).unwrap()).sum::<f64>() / n,
            values.iter().map(|o| spend(c.objective, o.candidate_tokens, o.candidate_cost_usd).unwrap()).sum::<f64>() / n,
        ]);
        latency.extend(values.iter().map(|o| o.candidate_ms));
    }
    if rows.iter().any(|r| r[2] <= 0.0 || r[3] <= 0.0) {
        return Err("zero benchmark spend is not qualifying evidence".into());
    }
    let base = rows.iter().map(|r| r[0]).sum::<f64>() / rows.len() as f64;
    let cand = rows.iter().map(|r| r[1]).sum::<f64>() / rows.len() as f64;
    let Some((_, predicted)) = ratio(&rows, 0..rows.len()) else {
        return Err("no accepted benchmark tasks".into());
    };
    let mut seed = 0x85ca_149e_a29d_083bu64;
    let mut quality = vec![];
    let mut savings = vec![];
    for _ in 0..2000 {
        let indices = (0..rows.len()).map(|_| rng(&mut seed) as usize % rows.len()).collect::<Vec<_>>();
        if let Some((q, s)) = ratio(&rows, indices.into_iter()) {
            quality.push(q);
            savings.push(s);
        } else {
            quality.push(-1.0);
            savings.push(-1.0);
        }
    }
    quality.sort_by(f64::total_cmp);
    savings.sort_by(f64::total_cmp);
    // Score bounds keep all-concordant small samples from claiming zero uncertainty.
    let positive = rows.iter().map(|r| (r[1] - r[0]).max(0.0)).sum::<f64>() / rows.len() as f64;
    let negative = rows.iter().map(|r| (r[0] - r[1]).max(0.0)).sum::<f64>() / rows.len() as f64;
    let paired_lower = wilson(positive, rows.len()).0 - wilson(negative, rows.len()).1;
    let q = Qualification {
        objective: c.objective,
        tasks: rows.len(),
        coverage,
        baseline_success: base,
        candidate_success: cand,
        quality_difference_lower: quality[49].min(paired_lower),
        savings_lower: savings[49],
        savings_upper: savings[1949],
        predicted_savings: predicted,
        candidate_p90_ms: metrics::percentile(&mut latency, 90).unwrap_or(u64::MAX),
    };
    if q.candidate_success < c.quality_floor || q.quality_difference_lower < -c.noninferiority {
        return Err("benchmark quality gate failed".into());
    }
    if q.candidate_p90_ms > c.max_p90_ms {
        return Err("benchmark latency gate failed".into());
    }
    if q.savings_lower <= 0.0 {
        return Err("benchmark savings inconclusive".into());
    }
    Ok(q)
}

pub fn state_path() -> PathBuf {
    crate::cicd::state_dir().join("adaptive-v1.json")
}
/// Serialize all state transactions across threads and processes, and fsync before dispatch.
pub fn transaction_at<T>(p: &Path, f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    let parent = p.parent().ok_or("no state parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(p.with_extension("lock"))
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    #[cfg(not(unix))]
    {
        return Err("adaptive durable locking requires Unix".into());
    }
    let mut store = match std::fs::read(p) {
        Ok(bytes) => serde_json::from_slice::<Store>(&bytes).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store { schema: 1, ..Store::default() },
        Err(e) => return Err(e.to_string()),
    };
    if store.schema != 1 {
        return Err("unknown adaptive state schema".into());
    }
    let result = f(&mut store)?;
    let tmp = p.with_extension(format!("{}.tmp", metrics::id()));
    let write = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(&serde_json::to_vec(&store)?)?;
        file.sync_all()?;
        std::fs::rename(&tmp, p)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(tmp);
        return Err(e.to_string());
    }
    drop(lock);
    Ok(result)
}
fn transaction<T>(f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    transaction_at(&state_path(), f)
}
fn key(d: &Decision) -> String {
    format!("{}|{}|{}|{}|{}", d.policy, d.evidence, d.cohort, d.parameters, d.evidence_snapshot)
}
fn read_evidence(p: &Path) -> Result<Evidence, String> {
    serde_json::from_slice(&std::fs::read(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn owner_gone(id: &str) -> bool {
    let Some(pid) = id.split('-').next().and_then(|s| s.parse::<i32>().ok()).filter(|p| *p > 1) else {
        return true;
    };
    // EPERM means a live process owned by another user, not a dead process.
    unsafe { libc::kill(pid, 0) != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) }
}
fn draw() -> Result<f64, String> {
    use std::io::Read;
    let mut b = [0u8; 8];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| e.to_string())?;
    Ok((u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64)
}
pub fn select(hub: &Hub, id: u32, baseline: &Route, manual: bool) {
    let c = &hub.cfg.adaptive;
    hub.job(id, |j| j.adaptive = None);
    if c.mode == Mode::Off || hub.cfg.demo {
        return;
    }
    let a = hub.cfg.agents.get(baseline.agent);
    let base = Choice {
        model: a.models.map(|m| m.get(baseline.tier).to_string()).unwrap_or_default(),
        effort: a.efforts.map(|m| m.get(baseline.tier).to_string()).unwrap_or_default(),
    };
    let Some((job, turn, repo, source, bytes)) = hub.job(id, |j| {
        let (repo, source) = match &j.source {
            Source::Issue { repo, .. } => (repo.clone(), "issue"),
            Source::Manual => (j.workdir.to_string_lossy().into_owned(), "task"),
            _ => (String::new(), "other"),
        };
        (j.metrics_id.clone(), j.turns.len(), repo, source.to_string(), j.prompt.len())
    }) else {
        return;
    };
    let cohort = hub.job(id, |j| metrics::Cohort::classify(&j.prompt, baseline.tier.name())).unwrap_or_default();
    let mut d = Decision {
        id: metrics::id(),
        job,
        turn,
        policy: c.policy.clone(),
        evidence: String::new(),
        policy_snapshot: c.clone(),
        evidence_snapshot: String::new(),
        cohort: format!("{}|{}|{}|{:?}", repo, source, baseline.tier.name(), cohort),
        parameters: format!(
            "{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            c.candidates,
            c.repositories,
            c.sources,
            c.tiers,
            c.max_prompt_bytes,
            c.min_tasks,
            c.min_coverage,
            c.quality_floor,
            c.noninferiority,
            c.max_p90_ms,
            c.trial_probability,
            c.trial_tasks,
            c.trial_tokens,
            c.reserve_tokens,
            c.window_secs,
            c.monitor_tasks
        ) + "|"
            + &c.promotion_from
            + &format!("|{:?}|{:?}|{:?}", c.objective, c.cohorts, c.invocation_max_usd),
        mode: c.mode,
        baseline: base.clone(),
        proposed: None,
        executed: base,
        candidate_actual_model: String::new(),
        baseline_actual_model: String::new(),
        reviewer_actual_model: String::new(),
        probability: 0.0,
        draw: 0.0,
        reason: String::new(),
        qualification: None,
        reserved: 0,
        candidate: false,
        completed: false,
        tokens: None,
        cost_usd: None,
        accepted: None,
        wall_ms: None,
        created: crate::journal::now(),
    };
    let qualify_route = (|| -> Result<Evidence, String> {
        if manual {
            return Err("manual override".into());
        }
        if c.policy.is_empty()
            || !c.repositories.contains(&repo)
            || !c.sources.contains(&source)
            || !c.tiers.iter().any(|t| t == baseline.tier.name())
            || bytes > c.max_prompt_bytes
            || c.max_prompt_bytes == 0
        {
            return Err("outside configured cohort/context limit".into());
        }
        // Agent extra arguments can override model/effort or sandbox settings; do not adapt those routes.
        if a.write_args.as_ref().is_some_and(|v| {
            v.iter().any(|s| {
                s == "--model"
                    || s == "-m"
                    || s.starts_with("--model=")
                    || s == "--effort"
                    || s.starts_with("--effort=")
                    || s == "-c"
                    || s == "--config"
                    || s.starts_with("--config=")
            })
        }) {
            return Err("explicit model/effort CLI override".into());
        }
        let e = read_evidence(&c.evidence)?;
        let reviewer_kind = if hub.cfg.review.reviewer == "other" { baseline.agent.other() } else { baseline.agent };
        let versions = hub.state.lock().unwrap().cli_versions.clone();
        if versions[crate::journal::slot(baseline.agent)].as_deref() != Some(e.cli_version.as_str())
            || versions[crate::journal::slot(reviewer_kind)].as_deref() != Some(e.reviewer_version.as_str())
        {
            return Err("worker/reviewer CLI version changed or unavailable".into());
        }
        if e.fingerprint != fingerprint(&hub.cfg)
            || (e.schema >= 2 && e.cohort != cohort)
            || e.cli != baseline.agent
            || e.baseline != d.baseline
            || e.repo != repo
            || e.source != source
            || e.tier != baseline.tier.name()
            || bytes > e.max_prompt_bytes
            || !c.candidates.contains(&e.candidate)
        {
            return Err("incompatible model, configuration or cohort evidence".into());
        }
        d.qualification = Some(qualify(c, &e, d.created)?);
        d.evidence_snapshot = evidence_digest(&e);
        d.evidence = e.version.clone();
        d.proposed = Some(e.candidate.clone());
        d.candidate_actual_model = e.candidate_actual_model.clone();
        d.baseline_actual_model = e.baseline_actual_model.clone();
        d.reviewer_actual_model = e.reviewer_actual_model.clone();
        Ok(e)
    })();
    if let Err(reason) = &qualify_route {
        d.reason = reason.clone();
    }
    let result = transaction(|s| {
        if s.disabled {
            d.reason = "disabled by operator".into();
        } else if qualify_route.is_ok() {
            let k = key(&d);
            if let Some(reason) = s.suspended.get(&k) {
                d.reason = format!("suspended: {reason}");
            } else if c.mode != Mode::Adaptive {
                d.reason = "qualified proposal; baseline executed".into();
            } else if !(0.0..=0.90).contains(&c.trial_probability)
                || c.trial_probability == 0.0
                || c.trial_tasks == 0
                || c.reserve_tokens == 0
                || c.trial_tokens < c.reserve_tokens
                || c.window_secs <= 0
                || c.monitor_tasks < 30
            {
                d.reason = "explicit bounded trial and monitoring limits required".into();
            } else if c.trial_probability > 0.10 && !promotion_ready(c, s, &d) {
                d.reason = "promotion requires a completed qualifying production window; baseline retained".into();
            } else if s.validated.get(&k).copied().unwrap_or(0) < c.min_tasks {
                d.reason = "insufficient completed advisory/shadow validation".into();
            } else {
                reserve(c, s, &mut d, draw()?);
            }
        }
        s.decisions.push(d.clone());
        Ok(())
    });
    if let Err(e) = result {
        hub.log("adaptive", format!("audit gap; baseline retained: {e}"));
        return;
    }
    hub.log("adaptive", format!("#{}: {}", id, d.reason));
    hub.job(id, |j| j.adaptive = Some(d));
}
fn reserve(c: &Config, s: &mut Store, d: &mut Decision, random: f64) {
    let k = key(d);
    let window_started = *s.windows.entry(k.clone()).or_insert(d.created);
    let active: Vec<_> = s
        .decisions
        .iter()
        .filter(|o| {
            key(o) == k
                && o.policy_snapshot.objective == c.objective
                && o.mode == Mode::Adaptive
                && o.created >= window_started
                && o.probability > 0.0
        })
        .collect();
    let ended_window = d.created - window_started >= c.window_secs;
    if ended_window {
        // No silent promotion or repeated significance tests: one declared window, explicit requalification.
        let report = production(c, &active);
        let reason = report.gate_failure.unwrap_or_else(|| {
            if report.observed_savings_lower.is_some_and(|v| v > 0.0) {
                "trial window complete; requalification and a new policy version required".into()
            } else {
                "production savings uncertainty inconclusive".into()
            }
        });
        s.suspended.insert(k.clone(), reason.clone());
        d.reason = reason;
    } else if active.iter().any(|o| o.completed && (o.tokens.is_none() || spend(c.objective, o.tokens, o.cost_usd).is_none())) {
        s.suspended.insert(k.clone(), "unknown trial usage".into());
        d.reason = "unknown trial usage".into();
    } else if active.iter().any(|o| !o.completed && owner_gone(&o.id)) {
        d.reason = "unfinished trial from a stopped process; reconciliation required".into();
        s.suspended.insert(k.clone(), d.reason.clone());
    } else {
        let candidates: Vec<_> = active.iter().filter(|o| o.candidate).collect();
        let spent = candidates.iter().fold(0u64, |n, o| n.saturating_add(o.tokens.unwrap_or(o.reserved)));
        d.probability = c.trial_probability;
        d.draw = random;
        if candidates.len() >= c.trial_tasks || spent.saturating_add(c.reserve_tokens) > c.trial_tokens {
            d.probability = 0.0;
            d.reason = "trial admission budget exhausted".into();
            s.suspended.insert(k.clone(), d.reason.clone());
        } else if d.draw < c.trial_probability {
            d.candidate = true;
            d.executed = d.proposed.clone().unwrap();
            d.reserved = c.reserve_tokens;
            d.reason = "qualified randomized trial".into();
        } else {
            d.reason = "randomized concurrent baseline holdout".into();
        }
    }
}
fn monitor(c: &Config, rows: &[&Decision]) -> Option<String> {
    if rows.iter().any(|d| !d.completed) {
        return Some("monitoring window has pending work".into());
    }
    if rows.iter().any(|d| d.accepted.is_none()) {
        return Some("monitoring quality evidence incomplete".into());
    }
    let candidates: Vec<_> = rows.iter().filter(|d| d.candidate && d.accepted.is_some()).collect();
    let baseline: Vec<_> = rows.iter().filter(|d| !d.candidate && d.accepted.is_some()).collect();
    if candidates.len() < c.monitor_tasks || baseline.len() < c.monitor_tasks {
        return Some("monitoring sample inconclusive".into());
    }
    if rows.iter().any(|d| d.tokens.is_none() || spend(c.objective, d.tokens, d.cost_usd).is_none()) {
        return Some("monitoring usage incomplete".into());
    }
    let p = |v: &Vec<&&Decision>| v.iter().filter(|d| d.accepted == Some(true)).count() as f64 / v.len() as f64;
    let pc = p(&candidates);
    let pb = p(&baseline);
    // Independent production arms: score bounds also retain uncertainty at 0% and 100%.
    let lower = wilson(pc, candidates.len()).0 - wilson(pb, baseline.len()).1;
    if pc < c.quality_floor || lower < -c.noninferiority {
        return Some("production quality gate failed".into());
    }
    let mut times: Vec<_> = candidates.iter().filter_map(|d| d.wall_ms).collect();
    if times.len() != candidates.len() || metrics::percentile(&mut times, 90).unwrap_or(u64::MAX) > c.max_p90_ms {
        return Some("production latency gate failed".into());
    }
    let spend = |v: &Vec<&&Decision>| {
        v.iter().map(|d| spend(c.objective, d.tokens, d.cost_usd).unwrap_or(0.0)).sum::<f64>()
            / v.iter().filter(|d| d.accepted == Some(true)).count().max(1) as f64
    };
    if spend(&candidates) >= spend(&baseline) {
        return Some("production savings gate failed".into());
    }
    None
}
pub fn suspend(hub: &Hub, id: u32, reason: &str) {
    let d = hub.job(id, |j| j.adaptive.clone()).flatten();
    if let Some(d) = d {
        if let Err(e) = transaction(|s| {
            s.suspended.insert(key(&d), reason.into());
            Ok(())
        }) {
            hub.log("adaptive", format!("suspension persistence failed: {e}"));
        }
    }
}
pub fn execution(hub: &Hub, id: u32) -> Option<Choice> {
    let (d, escalated) = hub.job(id, |j| (j.adaptive.clone(), j.escalated))?;
    let d = d?;
    if !d.candidate {
        return None;
    }
    if escalated {
        hub.log("adaptive", format!("#{id}: fix round uses baseline (bounded escalation)"));
        return None;
    }
    let e = match read_evidence(&hub.cfg.adaptive.evidence) {
        Ok(e) => e,
        Err(_) => {
            suspend(hub, id, "evidence unavailable before dispatch");
            fallback(hub, id, "evidence unavailable before dispatch");
            return None;
        }
    };
    if evidence_digest(&e) != d.evidence_snapshot
        || e.version != d.evidence
        || e.fingerprint != fingerprint(&hub.cfg)
        || qualify(&hub.cfg.adaptive, &e, crate::journal::now()).is_err()
    {
        suspend(hub, id, "evidence invalid before dispatch");
        fallback(hub, id, "evidence invalid before dispatch");
        return None;
    }
    match transaction(|s| {
        Ok(s.disabled || s.suspended.contains_key(&key(&d))
            || !s.decisions.iter().any(|o| o.id == d.id && o.candidate && !o.completed && o.executed == d.executed))
    }) {
        Ok(false) => Some(d.executed),
        _ => {
            fallback(hub, id, "suspended or missing durable dispatch record");
            None
        }
    }
}
pub fn settle(hub: &Hub, id: u32, status: JobStatus) {
    let Some((d, checks, verdict, rounds, ms, attempt_ids)) =
        hub.job(id, |j| (j.adaptive.clone(), j.checks, j.verdict, j.rounds, j.created.elapsed().as_millis() as u64, j.attempt_ids.clone()))
    else {
        return;
    };
    let Some(mut d) = d else { return };
    let result = (|| -> Result<(), String> {
        let (all, bad) = metrics::read_at(&metrics::root())?;
        let rows: Vec<_> = all.iter().filter(|a| a.job == d.job && a.turn == d.turn && attempt_ids.contains(&a.id)).collect();
        let complete = bad == 0
            && rows.len() == attempt_ids.len()
            && !rows.is_empty()
            && rows.iter().all(|a| a.ended.is_some() && a.terminal_usage && a.tokens.total().is_some());
        d.tokens = if complete { Some(rows.iter().map(|a| a.tokens.total().unwrap()).sum()) } else { None };
        d.cost_usd = if complete { rows.iter().try_fold(0.0, |v, a| a.tokens.cost_usd.map(|c| v + c)) } else { None };
        d.completed = true;
        d.wall_ms = Some(ms);
        // Workflow completion without required quality evidence is unknown, never proof of correctness.
        d.accepted = if status == JobStatus::Cancelled || !complete {
            None
        } else if checks == Some(false) || verdict == Some(false) {
            Some(false)
        } else if checks == Some(true) && verdict == Some(true) && status == JobStatus::Done {
            Some(true)
        } else {
            None
        };
        transaction(|s| {
            let k = key(&d);
            let already = s.decisions.iter().find(|o| o.id == d.id).is_some_and(|o| o.completed);
            let first = rows.iter().filter(|a| a.role == "worker").min_by_key(|a| a.sequence);
            let expected = if d.executed != d.baseline { &d.candidate_actual_model } else { &d.baseline_actual_model };
            if d.proposed.is_some() && first.is_none_or(|a| a.actual_model.as_deref() != Some(expected.as_str())) {
                s.suspended.insert(k.clone(), "executed model unresolved or changed".into());
            }
            if d.proposed.is_some()
                && rows.iter().filter(|a| a.role == "reviewer").any(|a| a.actual_model.as_deref() != Some(d.reviewer_actual_model.as_str()))
            {
                s.suspended.insert(k.clone(), "reviewer model unresolved or changed".into());
            }
            if (!complete || spend(d.policy_snapshot.objective, d.tokens, d.cost_usd).is_none()) && d.proposed.is_some() {
                s.suspended.insert(k.clone(), "missing attempt/usage coverage".into());
            }
            if d.candidate && d.tokens.is_some_and(|t| t > d.reserved) {
                s.suspended.insert(k.clone(), "trial reservation exceeded (in-flight admission overrun)".into());
            }
            if d.proposed.is_some()
                && matches!(d.mode, Mode::Advisory | Mode::Shadow)
                && complete
                && !already
                && !s.suspended.contains_key(&k)
            {
                *s.validated.entry(k).or_default() += 1;
            }
            if rounds > 0 && d.candidate {
                d.reason.push_str("; fix rounds escalated to baseline");
            }
            if let Some(old) = s.decisions.iter_mut().find(|o| o.id == d.id) {
                *old = d.clone();
            } else {
                return Err("missing durable decision".into());
            }
            Ok(())
        })
    })();
    if let Err(e) = result {
        hub.log("adaptive", format!("settlement audit gap: {e}"));
        suspend(hub, id, "settlement failed");
    }
    hub.job(id, |j| j.adaptive = Some(d));
}

pub fn status() -> Result<Store, String> {
    match std::fs::read(state_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store { schema: 1, ..Store::default() }),
        Err(e) => Err(e.to_string()),
    }
}
pub fn evaluate(cfg: &crate::config::Config, path: &Path) -> Result<Qualification, String> {
    let e = read_evidence(path)?;
    if e.fingerprint != fingerprint(cfg) {
        return Err("evidence configuration fingerprint does not match".into());
    }
    qualify(&cfg.adaptive, &e, crate::journal::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Config, Evidence) {
        let c =
            Config { min_tasks: 30, min_coverage: 0.95, quality_floor: 0.9, noninferiority: 0.05, max_p90_ms: 2000, ..Config::default() };
        let e = Evidence {
            schema: 1,
            cohort: metrics::Cohort::default(),
            fixture: false,
            version: "e1".into(),
            expires: 200,
            created: 50,
            fingerprint: "test".into(),
            cli: AgentKind::Claude,
            cli_version: "test-v1".into(),
            reviewer_version: "test-v1".into(),
            reviewer_actual_model: "base-1".into(),
            baseline: Choice { model: "base".into(), effort: "high".into() },
            baseline_actual_model: "base-1".into(),
            candidate: Choice { model: "small".into(), effort: "high".into() },
            candidate_actual_model: "small-1".into(),
            repo: "o/r".into(),
            source: "issue".into(),
            tier: "standard".into(),
            max_prompt_bytes: 1000,
            protocol: "fixed".into(),
            observations: (0..120)
                .map(|n| Observation {
                    task: n.to_string(),
                    baseline_accepted: true,
                    candidate_accepted: true,
                    baseline_tokens: Some(1000),
                    candidate_tokens: Some(500),
                    candidate_ms: 1000,
                    baseline_cost_usd: None,
                    candidate_cost_usd: None,
                })
                .collect(),
        };
        (c, e)
    }
    #[test]
    fn cost_objective_requires_complete_costs_and_counts_failed_work() {
        let (mut c, mut e) = fixture();
        c.objective = Objective::ReportedCostUsd;
        assert!(qualify(&c, &e, 100).unwrap_err().contains("costs"));
        for o in &mut e.observations {
            o.baseline_cost_usd = Some(2.0);
            o.candidate_cost_usd = Some(1.0);
        }
        assert_eq!(qualify(&c, &e, 100).unwrap().predicted_savings, 0.5);
        e.observations[0].candidate_accepted = false;
        e.observations[0].candidate_cost_usd = Some(1000.0);
        assert!(qualify(&c, &e, 100).is_err());
        e.observations[0].candidate_cost_usd = Some(-1.0);
        assert!(qualify(&c, &e, 100).unwrap_err().contains("costs"));
    }
    #[test]
    fn fixtures_and_changed_cohort_definitions_cannot_qualify() {
        let (mut c, mut e) = fixture();
        e.fixture = true;
        assert!(qualify(&c, &e, 100).unwrap_err().contains("fixture"));
        e.fixture = false;
        c.cohorts = vec![metrics::Cohort::classify("Fix a bug", "standard")];
        assert!(qualify(&c, &e, 100).unwrap_err().contains("cohort"));
        e.cohort = c.cohorts[0].clone();
        assert!(qualify(&c, &e, 100).is_ok());
        e.cohort.definition = "changed-v2".into();
        assert!(qualify(&c, &e, 100).is_err());
    }
    #[test]
    fn paired_evidence_uses_distinct_tasks_and_end_to_end_spend() {
        let (c, mut e) = fixture();
        let q = qualify(&c, &e, 100).unwrap();
        assert_eq!(q.tasks, 120);
        assert_eq!(q.predicted_savings, 0.5);
        for o in &mut e.observations {
            o.task = "one task repeated".into();
        }
        assert!(qualify(&c, &e, 100).unwrap_err().contains("distinct"));
        let (_, mut e) = fixture();
        for o in &mut e.observations {
            o.candidate_tokens = Some(1200);
        }
        assert!(qualify(&c, &e, 100).unwrap_err().contains("savings"));
    }
    #[test]
    fn stale_partial_unknown_and_quality_regressions_keep_baseline() {
        let (c, e) = fixture();
        assert!(qualify(&c, &e, 201).is_err());
        let mut e = e;
        e.observations[0].candidate_tokens = None;
        assert!(qualify(&c, &e, 100).is_err());
        e.observations[0].candidate_tokens = Some(500);
        e.candidate_actual_model.clear();
        assert!(qualify(&c, &e, 100).is_err());
        e.candidate_actual_model = "small-1".into();
        for o in e.observations.iter_mut().take(10) {
            o.candidate_accepted = false;
        }
        assert!(qualify(&c, &e, 100).unwrap_err().contains("quality"));
    }
    #[test]
    fn transaction_is_atomic_across_concurrent_writers_and_restarts() {
        let dir = std::env::temp_dir().join(metrics::id());
        let path = dir.join("state.json");
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || {
                    transaction_at(&p, |s| {
                        *s.validated.entry("cohort".into()).or_default() += 1;
                        Ok(())
                    })
                    .unwrap()
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        transaction_at(&path, |s| {
            assert_eq!(s.validated["cohort"], 12);
            s.suspended.insert("cohort".into(), "usage missing".into());
            Ok(())
        })
        .unwrap();
        transaction_at(&path, |s| {
            assert_eq!(s.suspended["cohort"], "usage missing");
            Ok(())
        })
        .unwrap();
        std::fs::write(&path, "broken").unwrap();
        assert!(transaction_at(&path, |_| Ok(())).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disabled_is_default_and_unknown_modes_are_rejected() {
        assert_eq!(Config::default().mode, Mode::Off);
        assert!(toml::from_str::<crate::config::Config>("[adaptive]\nmode='automatic'").is_err());
    }
}

#[derive(Serialize)]
pub struct ProductionReport {
    pub objective: Objective,
    pub policy: String,
    pub cohort: String,
    pub candidate_tasks: usize,
    pub baseline_tasks: usize,
    pub pending: usize,
    pub unknown_usage: usize,
    pub known_candidate_tokens: u64,
    pub reserved_tokens: u64,
    pub known_baseline_tokens: u64,
    pub observed_savings: Option<f64>,
    pub observed_savings_lower: Option<f64>,
    pub gate_failure: Option<String>,
}
fn production(c: &Config, rows: &[&Decision]) -> ProductionReport {
    let candidates: Vec<_> = rows.iter().filter(|d| d.candidate).collect();
    let baseline: Vec<_> = rows.iter().filter(|d| !d.candidate).collect();
    let candidate_tokens = candidates.iter().filter_map(|d| d.tokens).sum::<u64>();
    let baseline_tokens = baseline.iter().filter_map(|d| d.tokens).sum::<u64>();
    let a = candidates.iter().filter(|d| d.accepted == Some(true)).count();
    let b = baseline.iter().filter(|d| d.accepted == Some(true)).count();
    let all_complete = rows
        .iter()
        .all(|d| d.completed && d.tokens.is_some() && spend(c.objective, d.tokens, d.cost_usd).is_some() && d.accepted.is_some());
    let candidate_spend: f64 = candidates.iter().filter_map(|d| spend(c.objective, d.tokens, d.cost_usd)).sum();
    let baseline_spend: f64 = baseline.iter().filter_map(|d| spend(c.objective, d.tokens, d.cost_usd)).sum();
    let observed = if all_complete && a > 0 && b > 0 && baseline_spend > 0.0 {
        Some(1.0 - (candidate_spend / a as f64) / (baseline_spend / b as f64))
    } else {
        None
    };
    let mut lower = None;
    if observed.is_some() {
        let mut seed = 0x69a8_3421_f62c_45d1;
        let mut samples = vec![];
        for _ in 0..2000 {
            let mut sums = [0.0; 4];
            for (arm, idx) in [(&baseline, 0), (&candidates, 1)] {
                for _ in 0..arm.len() {
                    let d = arm[rng(&mut seed) as usize % arm.len()];
                    sums[idx] += f64::from(d.accepted == Some(true));
                    sums[idx + 2] += spend(c.objective, d.tokens, d.cost_usd).unwrap();
                }
            }
            samples.push(if sums[0] > 0.0 && sums[1] > 0.0 && sums[2] > 0.0 {
                1.0 - (sums[3] / sums[1]) / (sums[2] / sums[0])
            } else {
                -1.0
            });
        }
        samples.sort_by(f64::total_cmp);
        lower = Some(samples[49]);
    }
    ProductionReport {
        objective: c.objective,
        policy: rows.first().map(|d| d.policy.clone()).unwrap_or_default(),
        cohort: rows.first().map(|d| d.cohort.clone()).unwrap_or_default(),
        candidate_tasks: candidates.len(),
        baseline_tasks: baseline.len(),
        pending: rows.iter().filter(|d| !d.completed).count(),
        unknown_usage: rows.iter().filter(|d| d.completed && d.tokens.is_none()).count(),
        known_candidate_tokens: candidate_tokens,
        reserved_tokens: candidates.iter().filter(|d| !d.completed).map(|d| d.reserved).sum(),
        known_baseline_tokens: baseline_tokens,
        observed_savings: observed,
        observed_savings_lower: lower,
        gate_failure: monitor(c, rows),
    }
}
fn promotion_ready(c: &Config, s: &Store, d: &Decision) -> bool {
    if c.promotion_from.is_empty() || c.promotion_from == d.policy {
        return false;
    }
    let rows: Vec<_> = s
        .decisions
        .iter()
        .filter(|o| {
            o.policy == c.promotion_from
                && o.evidence == d.evidence
                && o.evidence_snapshot == d.evidence_snapshot
                && o.cohort == d.cohort
                && o.policy_snapshot.objective == c.objective
                && o.mode == Mode::Adaptive
                && o.probability > 0.0
        })
        .collect();
    let Some(first) = rows.first() else {
        return false;
    };
    if d.created - first.created < c.window_secs || rows.iter().any(|o| o.proposed != d.proposed || o.baseline != d.baseline) {
        return false;
    }
    if s.suspended.iter().any(|(k, reason)| {
        k.starts_with(&(c.promotion_from.clone() + "|"))
            && !reason.starts_with("trial window complete")
            && reason != "trial admission budget exhausted"
    }) {
        return false;
    }
    let p = production(c, &rows);
    p.gate_failure.is_none() && p.observed_savings_lower.is_some_and(|v| v > 0.0)
}
pub fn production_reports(c: &Config, s: &Store) -> Vec<ProductionReport> {
    let mut groups: BTreeMap<String, Vec<&Decision>> = BTreeMap::new();
    for d in s.decisions.iter().filter(|d| d.mode == Mode::Adaptive && d.probability > 0.0) {
        groups.entry(key(d)).or_default().push(d);
    }
    groups.values().map(|rows| production(rows.first().map(|d| &d.policy_snapshot).unwrap_or(c), rows)).collect()
}
pub fn set_disabled(cfg: &crate::config::Config, disabled: bool) -> Result<(), String> {
    if !disabled {
        evaluate(cfg, &cfg.adaptive.evidence)?;
    }
    transaction(|s| {
        s.disabled = disabled;
        Ok(())
    })
}

#[cfg(test)]
mod rollout_tests {
    use super::*;
    fn decision() -> Decision {
        let base = Choice { model: "large".into(), effort: "high".into() };
        Decision {
            id: metrics::id(),
            job: metrics::id(),
            turn: 1,
            policy: "p1".into(),
            evidence: "e1".into(),
            policy_snapshot: config(),
            evidence_snapshot: "snapshot".into(),
            cohort: "o/r|issue|standard".into(),
            parameters: "fixed".into(),
            mode: Mode::Adaptive,
            baseline: base.clone(),
            proposed: Some(Choice { model: "small".into(), effort: "high".into() }),
            executed: base,
            candidate_actual_model: "small".into(),
            baseline_actual_model: "large".into(),
            reviewer_actual_model: "large".into(),
            probability: 0.0,
            draw: 0.0,
            reason: String::new(),
            qualification: None,
            reserved: 0,
            candidate: false,
            completed: false,
            tokens: None,
            cost_usd: None,
            accepted: None,
            wall_ms: None,
            created: 100,
        }
    }
    fn config() -> Config {
        Config {
            trial_probability: 0.1,
            trial_tasks: 2,
            trial_tokens: 200,
            reserve_tokens: 100,
            window_secs: 1000,
            monitor_tasks: 30,
            quality_floor: 0.9,
            noninferiority: 0.05,
            max_p90_ms: 1000,
            ..Config::default()
        }
    }
    #[test]
    fn concurrent_trial_reservations_cannot_oversubscribe() {
        let dir = std::env::temp_dir().join(metrics::id());
        let path = dir.join("budget.json");
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    transaction_at(&path, |s| {
                        let mut d = decision();
                        if !s.suspended.contains_key(&key(&d)) {
                            reserve(&config(), s, &mut d, 0.01);
                        }
                        s.decisions.push(d);
                        Ok(())
                    })
                    .unwrap()
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        transaction_at(&path, |s| {
            assert_eq!(s.decisions.iter().filter(|d| d.candidate).count(), 2);
            assert_eq!(s.decisions.iter().map(|d| d.reserved).sum::<u64>(), 200);
            assert!(!s.suspended.is_empty());
            Ok(())
        })
        .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn holdout_unknown_usage_and_declared_window_are_preserved() {
        let c = config();
        let mut s = Store::default();
        let mut d = decision();
        reserve(&c, &mut s, &mut d, 0.5);
        assert!(!d.candidate);
        assert_eq!(d.executed, d.baseline);
        assert_eq!(d.probability, 0.1);
        d.completed = true;
        d.tokens = None;
        s.decisions.push(d);
        let mut next = decision();
        reserve(&c, &mut s, &mut next, 0.01);
        assert!(!next.candidate);
        assert!(next.reason.contains("unknown"));
        let mut s = Store::default();
        s.windows.insert(key(&next), 1);
        next.created = 2000;
        reserve(&c, &mut s, &mut next, 0.01);
        assert!(!next.candidate);
        assert!(next.reason.contains("inconclusive"));
    }
    #[test]
    fn cost_monitoring_and_promotion_require_complete_comparable_spend() {
        let mut c = config();
        c.objective = Objective::ReportedCostUsd;
        let mut decisions = vec![];
        for n in 0..240 {
            let mut d = decision();
            d.candidate = n < 120;
            d.completed = true;
            d.accepted = Some(true);
            d.tokens = Some(100);
            d.wall_ms = Some(500);
            d.cost_usd = Some(if d.candidate { 1.0 } else { 2.0 });
            decisions.push(d);
        }
        let report = production(&c, &decisions.iter().collect::<Vec<_>>());
        assert_eq!(report.observed_savings, Some(0.5));
        assert!(report.gate_failure.is_none());
        decisions[0].cost_usd = None;
        let report = production(&c, &decisions.iter().collect::<Vec<_>>());
        assert!(report.observed_savings.is_none());
        assert!(report.gate_failure.unwrap().contains("usage"));
    }
    #[test]
    fn quality_and_latency_windows_persist_suspension_and_completed_records() {
        for reason in ["quality", "latency"] {
            let root = std::env::temp_dir().join(metrics::id());
            let path = root.join("state.json");
            let c = config();
            let mut original = decision();
            original.created = 2000;
            transaction_at(&path, |s| {
                s.windows.insert(key(&original), 100);
                for i in 0..240 {
                    let mut d = decision();
                    d.candidate = i < 120;
                    d.probability = 0.1;
                    d.completed = true;
                    d.accepted = Some(!(d.candidate && reason == "quality"));
                    d.tokens = Some(if d.candidate { 50 } else { 100 });
                    d.wall_ms = Some(if d.candidate && reason == "latency" { 2000 } else { 500 });
                    s.decisions.push(d);
                }
                reserve(&c, s, &mut original, 0.0);
                assert!(!original.candidate);
                assert!(original.reason.contains(reason));
                Ok(())
            }).unwrap();
            transaction_at(&path, |s| {
                assert!(s.suspended.get(&key(&original)).unwrap().contains(reason));
                assert_eq!(s.decisions.len(), 240);
                assert!(s.decisions.iter().all(|d| d.completed));
                Ok(())
            }).unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn promotion_needs_both_arms_outcomes_and_credible_savings() {
        let mut c = config();
        c.promotion_from = "p1".into();
        let mut s = Store::default();
        for i in 0..240 {
            let mut d = decision();
            d.candidate = i < 120;
            d.completed = true;
            d.accepted = Some(true);
            d.wall_ms = Some(500);
            d.probability = 0.1;
            d.tokens = Some(if d.candidate { 500 } else { 1000 });
            s.decisions.push(d);
        }
        let mut next = decision();
        next.policy = "p2".into();
        next.created = 2000;
        assert!(promotion_ready(&c, &s, &next));
        s.decisions[0].completed = false;
        assert!(!promotion_ready(&c, &s, &next));
        s.decisions[0].completed = true;
        for d in s.decisions.iter_mut().take(120) {
            d.tokens = Some(1100);
        }
        assert!(!promotion_ready(&c, &s, &next));
    }
}

pub fn fallback(hub: &Hub, id: u32, reason: &str) {
    let Some(mut d) = hub.job(id, |j| j.adaptive.clone()).flatten() else {
        return;
    };
    d.executed = d.baseline.clone();
    d.reason.push_str(&format!("; baseline fallback: {reason}"));
    if let Err(e) = transaction(|s| {
        let old = s.decisions.iter_mut().find(|o| o.id == d.id).ok_or("missing decision")?;
        *old = d.clone();
        Ok(())
    }) {
        hub.log("adaptive", format!("fallback audit gap: {e}"));
    }
    hub.job(id, |j| j.adaptive = Some(d));
}
