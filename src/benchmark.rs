//! Explicit local benchmark execution. Each arm gets a disposable clone with no remotes.
use crate::{
    adaptive::{Choice, Evidence, Observation},
    agents::{self, RunSpec},
    metrics,
    orchestrator::Hub,
    proc,
    state::{AgentKind, Tier},
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub fixture: bool,
    #[serde(default)]
    pub max_invocations: Option<usize>,
    pub version: String,
    pub repository: PathBuf,
    pub cohort_repo: String,
    pub source: String,
    pub tier: String,
    pub cli: AgentKind,
    pub candidate: Choice,
    pub repeats: usize,
    pub lifetime_secs: i64,
    pub cache_state: String,
    pub tasks: Vec<Task>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub revision: String,
    pub prompt: String,
}
struct Arm {
    accepted: bool,
    tokens: Option<u64>,
    cost_usd: Option<f64>,
    ms: u64,
    actual: Option<String>,
}
async fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut c = proc::command("git");
    c.args(args);
    if let Some(d) = dir {
        c.current_dir(d);
    }
    let (ok, out, err) = proc::output(c, Duration::from_secs(120)).await?;
    if ok {
        Ok(out.trim().into())
    } else {
        Err(format!("benchmark git failed: {err}"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn invoke(
    hub: &Hub,
    kind: AgentKind,
    tier: Tier,
    choice: Option<&Choice>,
    dir: &Path,
    prompt: &str,
    read_only: bool,
    ledger: &Path,
    job: &str,
    round: u32,
    sequence: usize,
    deadline: Instant,
    cohort: &metrics::Cohort,
) -> Result<(bool, String, metrics::Attempt), String> {
    let installed: std::collections::HashSet<AgentKind> = [kind].into_iter().collect();
    let rejected = Default::default();
    let avail = crate::pick::Avail { installed: &installed, enabled: None, no_seat: false, rejected: &rejected };
    let mut exec = crate::pick::resolve(&hub.cfg, &avail, &crate::state::AgentRef::Kind(kind), tier).ok_or("benchmarks run claude or codex")?;
    if let Some(c) = choice {
        exec.model = c.model.clone();
        exec.effort = c.effort.clone();
    }
    let spec = RunSpec { exec: &exec, prompt, dir, read_only, resume: None, tier };
    let agents::CmdLine { cmd, mut args, .. } = agents::command_line(&hub.cfg, &spec);
    let (model, effort) = (exec.model.clone(), exec.effort.clone());
    let limit_description = agents::invocation_limit(&cmd, kind, &mut args, hub.cfg.adaptive.invocation_max_usd).await?;
    let mut a = metrics::Attempt {
        schema: 2,
        cohort: cohort.clone(),
        provenance: metrics::ADAPTER.into(),
        first_output_ms: None,
        routing_ms: None,
        queue: None,
        invocation_limit: limit_description,
        id: metrics::id(),
        job: job.into(),
        turn: 1,
        sequence,
        parent: None,
        role: if read_only { "reviewer" } else { "worker" }.into(),
        retry: round,
        repo: String::new(),
        source: "benchmark".into(),
        tier: tier.name().into(),
        cli: kind.name().into(),
        cli_version: agents::version(&cmd).await,
        configured_model: model,
        actual_model: None,
        effort,
        adapter: metrics::ADAPTER.into(),
        started: crate::journal::now(),
        ended: None,
        wall_ms: None,
        status: "incomplete".into(),
        tokens: Default::default(),
        terminal_usage: false,
        decision: None,
    };
    metrics::write_at(ledger, &a)?;
    let started = Instant::now();
    let mut command = proc::command(&cmd);
    if kind == AgentKind::Claude {
        command.env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1");
    }
    command.args(args).current_dir(dir);
    // The harness has no forge integration; remove common publishing credentials from children.
    for key in ["GH_TOKEN", "GITHUB_TOKEN", "GITLAB_TOKEN", "GL_TOKEN"] {
        command.env_remove(key);
    }
    let limit = deadline.saturating_duration_since(Instant::now());
    let result = proc::output_input(command, limit, Some(prompt.into())).await;
    let mut stream = metrics::Stream::default();
    let mut text = String::new();
    let mut terminal_ok = true;
    let mut ok = false;
    if let Ok((code, stdout, _)) = &result {
        for line in stdout.lines() {
            stream.observe(kind, line);
            for p in if kind == AgentKind::Claude { agents::parse_claude(line) } else { agents::parse_codex(line) } {
                match p {
                    agents::Parsed::Final { ok, text: t, .. } => {
                        terminal_ok = ok;
                        text = t;
                    }
                    agents::Parsed::Text(t) => text = t,
                    agents::Parsed::Error(_) => terminal_ok = false,
                    _ => {}
                }
            }
        }
        ok = *code == Some(0) && stream.terminal && terminal_ok;
    }
    a.tokens = stream.tokens;
    a.actual_model = stream.model;
    a.terminal_usage = stream.terminal;
    a.ended = Some(crate::journal::now());
    a.wall_ms = Some(started.elapsed().as_millis() as u64);
    a.status = if ok { "completed" } else { "error" }.into();
    metrics::write_at(ledger, &a)?;
    // Infrastructure errors invalidate the entire paired sample; don't label them quality failures.
    if result.is_err() || !a.terminal_usage {
        return Err("benchmark invocation incomplete; evidence not produced (attempt retained)".into());
    }
    Ok((ok, text, a))
}
#[allow(clippy::too_many_arguments)]
async fn arm(
    hub: &Hub,
    m: &Manifest,
    task: &Task,
    tier: Tier,
    choice: &Choice,
    baseline: &Choice,
    checks: &[String],
    dir: &Path,
    ledger: &Path,
) -> Result<Arm, String> {
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--no-hardlinks",
            "--no-checkout",
            m.repository.to_str().ok_or("non-UTF8 repository path")?,
            dir.to_str().ok_or("non-UTF8 benchmark path")?,
        ],
    )
    .await?;
    git(Some(dir), &["remote", "remove", "origin"]).await?;
    git(Some(dir), &["checkout", "--quiet", "--detach", &task.revision]).await?;
    let base = git(Some(dir), &["rev-parse", "HEAD"]).await?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(hub.cfg.agents.timeout_mins.unwrap_or(45).clamp(1, 1440) * 60);
    let mut prompt = format!(
        "Benchmark in a disposable working tree. Do not publish, push, create pull requests, or access production issues.\n\n{}",
        task.prompt
    );
    let cohort = metrics::Cohort::classify(&task.prompt, &m.tier);
    let job = metrics::id();
    let mut total = Some(0u64);
    let mut cost = Some(0.0);
    let mut actual = None;
    let mut accepted = false;
    let reviewer = if hub.cfg.review.reviewer == "other" { m.cli.other() } else { m.cli };
    let mut round = 0;
    let mut escalated = false;
    let mut sequence = 0;
    loop {
        let selected = if round == 0 && !escalated { choice } else { baseline };
        let (ok, text, a) = invoke(hub, m.cli, tier, Some(selected), dir, &prompt, false, ledger, &job, round, sequence, deadline, &cohort).await?;
        sequence += 1;
        cost = cost.zip(a.tokens.cost_usd).map(|(a, b)| a + b);
        total = total.zip(a.tokens.total()).map(|(a, b)| a.saturating_add(b));
        if round == 0 && !escalated {
            actual = a.actual_model;
        }
        if !ok {
            // Match production: only the initial candidate failure gets a baseline retry.
            // Escalation retains the tree and has its own allowance.
            if round == 0 && !escalated && choice != baseline {
                escalated = true;
                prompt = format!("{prompt}\n\nThe candidate worker failed. Continue from the existing working tree.\n{text}");
                continue;
            }
            break;
        }
        // Freeze the change before running checks; don't let tests change what gets reviewed.
        git(Some(dir), &["add", "--all"]).await?;
        let tree = git(Some(dir), &["write-tree"]).await?;
        let mut feedback = String::new();
        for check in checks {
            let mut c = proc::command("sh");
            c.arg("-c").arg(check).current_dir(dir);
            let (code, out, err) = proc::output_code(c, deadline.saturating_duration_since(Instant::now())).await?;
            if code.is_none() {
                return Err("benchmark check interrupted".into());
            }
            if code != Some(0) {
                feedback = format!("Check failed:\n{out}\n{err}");
                break;
            }
        }
        git(Some(dir), &["read-tree", "--reset", "-u", &tree]).await?;
        git(Some(dir), &["clean", "-fd"]).await?;
        if feedback.is_empty() {
            let rp = crate::orchestrator::review_prompt(&task.prompt, Some(&base), None);
            let (ok, text, a) = invoke(hub, reviewer, tier.up(), None, dir, &rp, true, ledger, &job, round, sequence, deadline, &cohort).await?;
            sequence += 1;
            cost = cost.zip(a.tokens.cost_usd).map(|(a, b)| a + b);
            total = total.zip(a.tokens.total()).map(|(a, b)| a.saturating_add(b));
            if !ok {
                return Err("benchmark reviewer did not complete".into());
            }
            match crate::orchestrator::parse_verdict(&text) {
                Some(true) => {
                    accepted = true;
                    break;
                }
                Some(false) => feedback = text,
                None => return Err("benchmark reviewer gave no verdict".into()),
            }
        }
        if round >= hub.cfg.review.max_fix_rounds {
            break;
        }
        round += 1;
        prompt = format!("{}\n\nYour previous attempt remains in this working tree. Fix these findings:\n{}", task.prompt, feedback);
    }
    Ok(Arm { accepted, tokens: total, cost_usd: cost, ms: start.elapsed().as_millis() as u64, actual })
}
pub async fn run(hub: &Hub, manifest: &Path, output: &Path, preflight: bool) -> Result<(), String> {
    let bytes = std::fs::read(manifest).map_err(|e| e.to_string())?;
    let mut m: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    m.repository = std::fs::canonicalize(&m.repository).map_err(|e| e.to_string())?;
    if m.tasks.is_empty() || m.repeats == 0 || m.repeats > 20 || m.lifetime_secs <= 0 || m.version.is_empty() || m.cache_state.is_empty() {
        return Err("benchmark needs tasks, 1–20 repeats, version, cache-state description and lifetime".into());
    }
    if hub.cfg.review.mode != "always" || hub.cfg.knowledge.enabled {
        return Err("benchmark requires review.mode=always and knowledge.enabled=false to match measured policy".into());
    }
    let mut ids = std::collections::HashSet::new();
    if m.tasks
        .iter()
        .any(|t| !ids.insert(&t.id) || t.id.is_empty() || t.revision.len() != 40 || !t.revision.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("task IDs must be unique and revisions must be full pinned commit hashes".into());
    }
    let tier = match m.tier.as_str() {
        "light" => Tier::Light,
        "standard" => Tier::Standard,
        "heavy" => Tier::Heavy,
        _ => return Err("unknown benchmark tier".into()),
    };
    if m.source == "task" {
        if m.cohort_repo.is_empty() {
            m.cohort_repo = m.repository.to_string_lossy().into_owned();
        }
        if Path::new(&m.cohort_repo) != m.repository {
            return Err("manual cohort must identify the canonical repository path".into());
        }
    } else if m.source != "issue" {
        return Err("benchmark source must be task or issue".into());
    }
    if !["same", "other"].contains(&hub.cfg.review.reviewer.as_str()) {
        return Err("reviewer must be same or other".into());
    }
    let reviewer_kind = if hub.cfg.review.reviewer == "other" { m.cli.other() } else { m.cli };
    if hub.cfg.agents.get(reviewer_kind).models.unwrap().get(tier.up()).is_empty() {
        return Err("pin an explicit reviewer model before benchmarking".into());
    }
    let checks = if m.source == "task" {
        hub.cfg.review.checks.clone()
    } else {
        hub.cfg.repos.iter().find(|r| r.repo == m.cohort_repo).ok_or("cohort repository not configured")?.checks.clone()
    };
    if checks.is_empty() {
        return Err("benchmark requires a fixed check suite".into());
    }
    let agent = hub.cfg.agents.get(m.cli);
    let baseline = Choice { model: agent.models.unwrap().get(tier).into(), effort: agent.efforts.unwrap().get(tier).into() };
    if baseline.model.is_empty() || m.candidate.model.is_empty() {
        return Err("explicit benchmark models required".into());
    }
    let expected = m
        .tasks
        .len()
        .checked_mul(m.repeats)
        .and_then(|n| n.checked_mul(4 * (hub.cfg.review.max_fix_rounds as usize + 1) + 1))
        .ok_or("invocation budget overflow")?;
    if !m.fixture && m.max_invocations.is_none() {
        return Err("set max_invocations to authorize the worst-case benchmark budget".into());
    }
    if m.max_invocations.is_some_and(|n| n < expected) {
        return Err(format!("max_invocations must cover {expected} worker/reviewer calls including fixes"));
    }
    let cohort = metrics::Cohort::classify(&m.tasks[0].prompt, &m.tier);
    if m.tasks.iter().any(|t| t.prompt.trim().is_empty() || metrics::Cohort::classify(&t.prompt, &m.tier) != cohort) {
        return Err("tasks must have nonempty prompts in one pre-run cohort; split the manifest by cohort".into());
    }
    for task in &m.tasks {
        git(Some(&m.repository), &["cat-file", "-e", &format!("{}^{{commit}}", task.revision)]).await?;
    }
    for kind in [m.cli, if hub.cfg.review.reviewer == "other" { m.cli.other() } else { m.cli }] {
        let version = agents::version(&hub.cfg.agents.get(kind).cmd.unwrap()).await.ok_or("benchmark CLI missing or --version failed")?;
        if !m.fixture && version.to_ascii_lowercase().contains("fixture") {
            return Err("fixture CLI requires fixture=true; its evidence cannot qualify".into());
        }
    }
    if preflight {
        println!("Benchmark preflight passed; no model invocations executed");
        return Ok(());
    }
    std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let run_dir = output.join(metrics::id());
    std::fs::create_dir(&run_dir).map_err(|e| e.to_string())?;
    std::fs::write(run_dir.join("manifest.json"), &bytes).map_err(|e| e.to_string())?;
    let mut observations = vec![];
    let mut base_actual = None;
    let mut cand_actual = None;
    for repeat in 0..m.repeats {
        for (i, task) in m.tasks.iter().enumerate() {
            eprintln!("benchmark {} repeat {}", task.id, repeat + 1);
            let mut arms: [Option<Arm>; 2] = [None, None];
            // OS randomness randomizes arm order; records include invocation timestamps and models.
            use std::io::Read;
            let mut b = [0u8; 1];
            std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| e.to_string())?;
            let order = if b[0] & 1 == 0 { [0, 1] } else { [1, 0] };
            for index in order {
                let dir = run_dir.join(format!("task-{i}-{repeat}-{index}"));
                let result = arm(
                    hub,
                    &m,
                    task,
                    tier,
                    if index == 0 { &baseline } else { &m.candidate },
                    &baseline,
                    &checks,
                    &dir,
                    &run_dir.join("attempts"),
                )
                .await;
                // Only remove the disposable clone allocated by this command.
                let _ = std::fs::remove_dir_all(&dir);
                arms[index] = Some(result?);
            }
            let b = arms[0].take().unwrap();
            let c = arms[1].take().unwrap();
            for (known, actual) in [(&mut base_actual, b.actual), (&mut cand_actual, c.actual)] {
                let actual = actual.ok_or("benchmark model unresolved; evidence cannot qualify")?;
                if known.as_ref().is_some_and(|old| old != &actual) {
                    return Err("benchmark model changed between runs".into());
                }
                *known = Some(actual);
            }
            observations.push(Observation {
                task: task.id.clone(),
                baseline_accepted: b.accepted,
                candidate_accepted: c.accepted,
                baseline_tokens: b.tokens,
                candidate_tokens: c.tokens,
                candidate_ms: c.ms,
                baseline_cost_usd: b.cost_usd,
                candidate_cost_usd: c.cost_usd,
            });
        }
    }
    let now = crate::journal::now();
    let cli_version = agents::version(&hub.cfg.agents.get(m.cli).cmd.unwrap()).await.ok_or("worker version unavailable")?;
    let reviewer_kind = if hub.cfg.review.reviewer == "other" { m.cli.other() } else { m.cli };
    let reviewer_version = agents::version(&hub.cfg.agents.get(reviewer_kind).cmd.unwrap()).await.ok_or("reviewer version unavailable")?;
    let (attempts, bad) = metrics::read_at(&run_dir.join("attempts"))?;
    let reviewer_actual_model =
        attempts.iter().find(|a| a.role == "reviewer").and_then(|a| a.actual_model.clone()).ok_or("benchmark reviewer model unresolved")?;
    if bad > 0
        || attempts.iter().any(|a| {
            let expected = if a.cli == m.cli.name() { &cli_version } else { &reviewer_version };
            a.cli_version.as_ref() != Some(expected) || (a.role == "reviewer" && a.actual_model.as_ref() != Some(&reviewer_actual_model))
        })
    {
        return Err("benchmark CLI or reviewer model changed between runs".into());
    }
    let e = Evidence {
        schema: 2,
        cohort: metrics::Cohort::classify(&m.tasks[0].prompt, &m.tier),
        fixture: m.fixture,
        cli_version,
        reviewer_version,
        reviewer_actual_model,
        version: m.version,
        created: now,
        expires: now.saturating_add(m.lifetime_secs),
        fingerprint: crate::adaptive::fingerprint(&hub.cfg),
        cli: m.cli,
        baseline,
        baseline_actual_model: base_actual.unwrap(),
        candidate: m.candidate,
        candidate_actual_model: cand_actual.unwrap(),
        repo: m.cohort_repo,
        source: m.source,
        tier: m.tier,
        max_prompt_bytes: m.tasks.iter().map(|t| t.prompt.len()).max().unwrap_or(0),
        protocol: format!(
            "isolated-clone-v1; fixed checks/review; baseline escalation; cache={}; repeats={}; revisions={}",
            m.cache_state,
            m.repeats,
            m.tasks.iter().map(|t| t.revision.as_str()).collect::<Vec<_>>().join(",")
        ),
        observations,
    };
    let path = run_dir.join("evidence.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&e).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    println!("{}", path.display());
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerManifest {
    pub version: String,
    pub repository: PathBuf,
    pub cli: AgentKind,
    pub baseline: Choice,
    pub candidate: Choice,
    pub repeats: usize,
    pub max_invocations: usize,
    pub cache_state: String,
    #[serde(default)]
    pub fixture: bool,
    pub cases: Vec<ReviewCase>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewCase {
    pub id: String,
    pub base: String,
    pub revision: String,
    pub task: String,
    /// One labeled defect per case; null identifies a clean change. Never sent to reviewer.
    pub defect: Option<String>,
}
#[derive(serde::Serialize)]
struct ReviewResult {
    case: String,
    repeat: usize,
    arm: String,
    defect: Option<String>,
    verdict_pass: Option<bool>,
    missed_defect: bool,
    false_positive: bool,
    attempt: metrics::Attempt,
}
#[derive(Default, serde::Serialize)]
struct ReviewSummary {
    cases: usize,
    defect_cases: usize,
    clean_cases: usize,
    missed_defects: usize,
    false_positives: usize,
    unknown_verdicts: usize,
    complete_usage: usize,
    known_tokens: u64,
    reported_cost_usd: Option<f64>,
    p90_ms: Option<u64>,
}
fn review_summary(rows: &[ReviewResult], arm: &str) -> ReviewSummary {
    let rows: Vec<_> = rows.iter().filter(|r| r.arm == arm).collect();
    let mut times: Vec<_> = rows.iter().filter_map(|r| r.attempt.wall_ms).collect();
    ReviewSummary {
        cases: rows.len(),
        defect_cases: rows.iter().filter(|r| r.defect.is_some()).count(),
        clean_cases: rows.iter().filter(|r| r.defect.is_none()).count(),
        missed_defects: rows.iter().filter(|r| r.missed_defect).count(),
        false_positives: rows.iter().filter(|r| r.false_positive).count(),
        unknown_verdicts: rows.iter().filter(|r| r.verdict_pass.is_none()).count(),
        complete_usage: rows.iter().filter(|r| r.attempt.terminal_usage && r.attempt.tokens.total().is_some()).count(),
        known_tokens: rows.iter().filter_map(|r| r.attempt.tokens.total()).sum(),
        reported_cost_usd: rows.iter().try_fold(0.0, |v, r| r.attempt.tokens.cost_usd.map(|c| v + c)),
        p90_ms: metrics::percentile(&mut times, 90),
    }
}
/// Descriptive labeled-change evaluation only: never produces worker routing evidence.
pub async fn reviewers(hub: &Hub, manifest: &Path, output: &Path, preflight: bool) -> Result<(), String> {
    let mut m: ReviewerManifest =
        serde_json::from_slice(&std::fs::read(manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    m.repository = std::fs::canonicalize(&m.repository).map_err(|e| e.to_string())?;
    if m.version.is_empty()
        || m.cache_state.is_empty()
        || m.baseline.model.is_empty()
        || m.candidate.model.is_empty()
        || m.repeats == 0
        || m.repeats > 20
        || m.cases.is_empty()
    {
        return Err("reviewer benchmark requires version, cache state, explicit models, cases and 1–20 repeats".into());
    }
    let calls = m.cases.len().checked_mul(m.repeats).and_then(|n| n.checked_mul(2)).ok_or("invocation budget overflow")?;
    if m.max_invocations < calls {
        return Err(format!("max_invocations must cover {calls} reviewer calls"));
    }
    if !m.cases.iter().any(|c| c.defect.is_some()) || !m.cases.iter().any(|c| c.defect.is_none()) {
        return Err("provide both labeled defects and clean changes".into());
    }
    let mut ids = std::collections::HashSet::new();
    for c in &m.cases {
        if c.id.is_empty() || !ids.insert(&c.id) || c.task.trim().is_empty() || c.defect.as_ref().is_some_and(|d| d.trim().is_empty()) {
            return Err("case IDs must be unique; tasks and defect labels must be nonempty".into());
        }
        for rev in [&c.base, &c.revision] {
            if rev.len() != 40 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("review cases require full pinned commit hashes".into());
            }
            git(Some(&m.repository), &["cat-file", "-e", &format!("{rev}^{{commit}}")]).await?;
        }
    }
    let version = agents::version(&hub.cfg.agents.get(m.cli).cmd.unwrap()).await.ok_or("reviewer CLI missing or --version failed")?;
    if !m.fixture && version.to_ascii_lowercase().contains("fixture") {
        return Err("fixture CLI requires fixture=true".into());
    }
    if preflight {
        println!("Benchmark preflight passed; no model invocations executed");
        return Ok(());
    }
    std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let run = output.join(metrics::id());
    std::fs::create_dir(&run).map_err(|e| e.to_string())?;
    std::fs::copy(manifest, run.join("manifest.json")).map_err(|e| e.to_string())?;
    let mut rows = vec![];
    for repeat in 0..m.repeats {
        for case in &m.cases {
            use std::io::Read;
            let mut random = [0u8; 1];
            std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut random)).map_err(|e| e.to_string())?;
            let order = if random[0] & 1 == 0 { ["baseline", "candidate"] } else { ["candidate", "baseline"] };
            for arm in order {
                let dir = run.join(format!("review-{}", metrics::id()));
                let result = async {
                    git(
                        None,
                        &[
                            "clone",
                            "--quiet",
                            "--no-hardlinks",
                            "--no-checkout",
                            m.repository.to_str().ok_or("non-UTF8 repository")?,
                            dir.to_str().ok_or("non-UTF8 output")?,
                        ],
                    )
                    .await?;
                    git(Some(&dir), &["remote", "remove", "origin"]).await?;
                    git(Some(&dir), &["checkout", "--quiet", "--detach", &case.revision]).await?;
                    let prompt = crate::orchestrator::review_prompt(&case.task, Some(&case.base), None);
                    let choice = if arm == "baseline" { &m.baseline } else { &m.candidate };
                    invoke(
                        hub,
                        m.cli,
                        Tier::Heavy,
                        Some(choice),
                        &dir,
                        &prompt,
                        true,
                        &run.join("attempts"),
                        &metrics::id(),
                        0,
                        0,
                        Instant::now() + Duration::from_secs(hub.cfg.agents.timeout_mins.unwrap_or(45).clamp(1, 1440) * 60),
                        &metrics::Cohort::classify(&case.task, "heavy"),
                    )
                    .await
                }
                .await;
                let _ = std::fs::remove_dir_all(&dir);
                let (ok, text, attempt) = result?;
                if attempt.cli_version.as_deref() != Some(&version) {
                    return Err("reviewer CLI changed during benchmark".into());
                }
                let verdict = if ok { crate::orchestrator::parse_verdict(&text) } else { None };
                rows.push(ReviewResult {
                    case: case.id.clone(),
                    repeat,
                    arm: arm.into(),
                    defect: case.defect.clone(),
                    verdict_pass: verdict,
                    missed_defect: case.defect.is_some() && verdict == Some(true),
                    false_positive: case.defect.is_none() && verdict == Some(false),
                    attempt,
                });
            }
        }
    }
    let report = serde_json::json!({"schema": 1, "kind": "reviewer-benchmark", "version": m.version,
        "fixture": m.fixture, "cache_state": m.cache_state, "cli_version": version,
        "baseline": review_summary(&rows, "baseline"), "candidate": review_summary(&rows, "candidate"),
        "note": "Case-level labeled-defect detection; unknown verdicts are separate. Descriptive only; cannot enable routing.", "results": rows});
    let path = run.join("reviewers.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    println!("{}", path.display());
    Ok(())
}

#[cfg(test)]
mod manifest_tests {
    use super::*;
    #[test]
    fn shipped_manifest_templates_have_valid_schemas() {
        let worker: Manifest = serde_json::from_str(include_str!("../docs/examples/worker-benchmark.json")).unwrap();
        let reviewer: ReviewerManifest = serde_json::from_str(include_str!("../docs/examples/reviewer-benchmark.json")).unwrap();
        assert!(worker.max_invocations.is_some());
        assert!(reviewer.cases.iter().any(|c| c.defect.is_none()));
        assert!(reviewer.cases.iter().any(|c| c.defect.is_some()));
    }
}
