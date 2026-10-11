//! CI feedback and staging deployments, done by calling tools on MCP servers.
//!
//! forkdeck knows nothing about any particular CI or deployment system. The config names a server,
//! a tool, its arguments and how to read the answer; that is the whole integration.

use crate::agents::Slot;
use crate::config::{CallCfg, Config, DeployCfg, RepoCfg};
use crate::issues;
use crate::mcp::{Session, ToolResult};
use crate::orchestrator::{run_checks, Hub};
use crate::state::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub type Vars = HashMap<String, String>;

/// Placeholders that become JSON numbers when they are the whole value.
const NUMERIC: [&str; 2] = ["pr", "issue"];

fn fill(s: &str, vars: &Vars) -> String {
    let mut out = s.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{}}}", k), v);
    }
    out
}

/// Turn the configured arguments into the JSON a tool is called with.
///
/// A value that is nothing but one placeholder becomes a number when it stands for a pull request,
/// an issue or something captured from an earlier answer (`numeric`) and its value is all digits.
/// A commit never does, even one that happens to consist of digits.
pub fn render(v: &toml::Value, vars: &Vars, numeric: &[String]) -> Value {
    match v {
        toml::Value::String(s) => {
            for k in NUMERIC.iter().copied().chain(numeric.iter().map(|n| n.as_str())) {
                if s == &format!("{{{}}}", k) {
                    if let Some(n) = vars.get(k).and_then(|x| x.parse::<u64>().ok()) {
                        return Value::from(n);
                    }
                }
            }
            Value::String(fill(s, vars))
        }
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(*f),
        toml::Value::Boolean(b) => Value::from(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(|x| render(x, vars, numeric)).collect()),
        toml::Value::Table(t) => Value::Object(t.iter().map(|(k, x)| (k.clone(), render(x, vars, numeric))).collect()),
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Reading {
    Success,
    Failure,
    Pending,
}

fn matches(pattern: &Option<String>, text: &str) -> bool {
    pattern.as_deref().and_then(|p| Regex::new(p).ok()).map(|re| re.is_match(text)).unwrap_or(false)
}

fn same_revision(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    // Systems report full or abbreviated commits; seven characters is git's own short form.
    a.len() >= 7 && b.len() >= 7 && (a.starts_with(&b) || b.starts_with(&a))
}

/// Decide what a tool's answer means.
///
/// With a `revision` pattern, an answer about any other commit is "pending": a deployment system
/// that has not picked up the new commit yet is still describing the old one, healthy or not.
pub fn read(call: &CallCfg, text: &str, is_error: bool, want_rev: Option<&str>) -> Reading {
    if is_error {
        return Reading::Failure;
    }
    if let (Some(p), Some(want)) = (&call.revision, want_rev) {
        let got = Regex::new(p).ok().and_then(|re| re.captures(text).and_then(|c| c.get(1).map(|m| m.as_str().to_string())));
        match got {
            Some(g) if same_revision(&g, want) => {}
            _ => return Reading::Pending,
        }
    }
    if matches(&call.failure, text) {
        Reading::Failure
    } else if matches(&call.pending, text) {
        Reading::Pending
    } else if matches(&call.success, text) {
        Reading::Success
    } else {
        Reading::Pending
    }
}

/// Pull values out of an answer for later calls to use as placeholders: each `capture` entry is
/// a name and a regular expression whose first group is the value. Returns the names that matched.
pub fn capture(call: &CallCfg, text: &str, vars: &mut Vars) -> Vec<String> {
    let mut got = Vec::new();
    for (name, pattern) in &call.capture {
        let found = Regex::new(pattern).ok().and_then(|re| re.captures(text).and_then(|c| c.get(1).map(|m| m.as_str().to_string())));
        if let Some(v) = found {
            vars.insert(name.clone(), v);
            got.push(name.clone());
        }
    }
    got
}

/// The last `n` characters, on a character boundary.
pub fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        format!("…{}", s.chars().skip(count - n).collect::<String>())
    }
}

/// Open sessions, one per server, kept for the length of a stage.
struct Pool<'a> {
    cfg: &'a Config,
    open: HashMap<String, Session>,
    /// Names captured from earlier answers; these may be sent as numbers.
    captured: Vec<String>,
}

impl<'a> Pool<'a> {
    fn new(cfg: &'a Config) -> Pool<'a> {
        Pool { cfg, open: HashMap::new(), captured: Vec::new() }
    }

    async fn call(&mut self, stage_server: &str, call: &CallCfg, vars: &Vars) -> Result<ToolResult, String> {
        let name = call.server.as_deref().unwrap_or(stage_server).to_string();
        if !self.open.contains_key(&name) {
            let sc = self.cfg.mcp.get(&name).ok_or_else(|| format!("no [mcp.{}] server is configured", name))?;
            self.open.insert(name.clone(), Session::open(&name, sc).await?);
        }
        let args = render(&call.args, vars, &self.captured);
        let res = self.open.get_mut(&name).expect("just inserted").call(&call.tool, args).await;
        if res.is_err() {
            // The session may be broken; the next call starts a fresh one.
            self.open.remove(&name);
        }
        res
    }

    async fn close(self) {
        for (_, s) in self.open {
            s.close().await;
        }
    }
}

enum Waited {
    Success,
    Failure(String),
    /// Timed out, or the status could not be read.
    Unknown(String),
    Cancelled,
}

#[allow(clippy::too_many_arguments)]
async fn wait(
    pool: &mut Pool<'_>,
    server: &str,
    call: &CallCfg,
    vars: &Vars,
    want_rev: Option<&str>,
    interval: Duration,
    limit: Duration,
    cancel: &Cancel,
    mut tick: impl FnMut(&str),
) -> Waited {
    let start = Instant::now();
    let mut errors = 0;
    let mut last = String::new();
    loop {
        if cancel.is_set() {
            return Waited::Cancelled;
        }
        match pool.call(server, call, vars).await {
            Ok(r) => {
                errors = 0;
                last = r.text.clone();
                match read(call, &r.text, r.is_error, want_rev) {
                    Reading::Success => return Waited::Success,
                    Reading::Failure => return Waited::Failure(r.text),
                    Reading::Pending => tick(&format!("pending · {}", clip(&r.text.replace('\n', " "), 80))),
                }
            }
            Err(e) => {
                errors += 1;
                tick(&format!("status unreadable ({} of 5): {}", errors, clip(&e, 80)));
                if errors >= 5 {
                    return Waited::Unknown(format!("the status could not be read: {}", e));
                }
            }
        }
        if start.elapsed() >= limit {
            return Waited::Unknown(format!("no result after {} min; last answer: {}", limit.as_secs() / 60, clip(&last.replace('\n', " "), 200)));
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = cancel.wait() => return Waited::Cancelled,
        }
    }
}

fn repo_vars(r: &RepoCfg) -> Vars {
    let mut v: Vars = HashMap::new();
    let (owner, name) = r.repo.rsplit_once('/').unwrap_or(("", r.repo.as_str()));
    v.insert("repo".into(), r.repo.clone());
    v.insert("owner".into(), owner.to_string());
    v.insert("name".into(), name.to_string());
    v.insert("base".into(), r.base.clone());
    v
}

// ------------------------------------------------------------------ CI

pub enum Ci {
    Passed,
    /// With the output to show the worker.
    Failed(String),
    Unknown(String),
    Cancelled,
}

/// Wait for the pipeline on the job's pull request.
pub async fn ci_wait(hub: &Hub, id: u32, cancel: &Cancel) -> Ci {
    if hub.cfg.demo {
        tokio::time::sleep(Duration::from_millis(900)).await;
        hub.log("ci", format!("#{} pipeline passed", id));
        return Ci::Passed;
    }
    let found = hub
        .job(id, |j| match &j.source {
            Source::Issue { repo_idx, number, .. } => hub.cfg.repos.get(*repo_idx).cloned().map(|r| (r, *number, j.branch.clone(), j.base_sha.clone(), j.pr_url.clone())),
            _ => None,
        })
        .flatten();
    let Some((r, issue, branch, sha, pr_url)) = found else { return Ci::Unknown("not an issue job".into()) };
    let Some(ci) = r.ci.clone() else { return Ci::Passed };
    let mut vars = repo_vars(&r);
    vars.insert("issue".into(), issue.to_string());
    vars.insert("branch".into(), branch.unwrap_or_default());
    let sha = sha.unwrap_or_default();
    vars.insert("sha".into(), sha.clone());
    vars.insert("pr".into(), pr_url.as_deref().and_then(issues::pr_number).map(|n| n.to_string()).unwrap_or_default());

    hub.log("ci", format!("#{} waiting for CI on {} via {}.{}", id, &sha[..sha.len().min(10)], ci.status.server.as_deref().unwrap_or(&ci.server), ci.status.tool));
    let mut pool = Pool::new(&hub.cfg);
    let waited = wait(
        &mut pool,
        &ci.server,
        &ci.status,
        &vars,
        Some(&sha),
        Duration::from_secs(ci.interval_secs.max(1)),
        Duration::from_secs(ci.timeout_mins.max(1) * 60),
        cancel,
        |_| {},
    )
    .await;
    let out = match waited {
        Waited::Success => {
            hub.log("ci", format!("#{} pipeline passed", id));
            Ci::Passed
        }
        Waited::Cancelled => Ci::Cancelled,
        Waited::Unknown(why) => Ci::Unknown(why),
        Waited::Failure(status) => {
            hub.log("ci", format!("#{} pipeline failed", id));
            // The status answer usually names the failing run; the log call needs that.
            let got = capture(&ci.status, &status, &mut vars);
            pool.captured.extend(got);
            let log = match &ci.logs {
                None => status,
                Some(call) => match pool.call(&ci.server, call, &vars).await {
                    Ok(l) if !l.text.trim().is_empty() => l.text,
                    Ok(_) => status,
                    Err(e) => {
                        hub.log("ci", format!("#{} could not fetch the CI log ({}); using the status instead", id, clip(&e, 120)));
                        status
                    }
                },
            };
            Ci::Failed(log)
        }
    };
    pool.close().await;
    out
}

// ------------------------------------------------------------------ deploy

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Saved {
    /// The newest base commit a deployment was started for.
    pub seen: String,
    /// The newest commit that deployed and verified cleanly.
    pub last_good: String,
}

fn state_path() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME").ok().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/state")
    });
    base.join("forkdeck/deploy.json")
}

fn load_saved() -> HashMap<String, Saved> {
    std::fs::read_to_string(state_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn store_saved(key: &str, s: &Saved) -> Result<(), String> {
    let path = state_path();
    let mut all = load_saved();
    all.insert(key.to_string(), s.clone());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // Write beside and rename, so a crash cannot leave half a file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn saved_key(r: &RepoCfg, d: &DeployCfg) -> String {
    format!("{:?}:{}:{}", r.provider, r.repo, d.environment).to_lowercase()
}

/// Look at the base branch of every repository with a deploy stage and start a deployment when it has moved.
pub async fn deploy_poll(hub: &Hub) {
    if hub.cfg.demo {
        return;
    }
    for (idx, r) in hub.cfg.repos.iter().enumerate() {
        let Some(d) = &r.deploy else { continue };
        let busy = hub.state.lock().unwrap().jobs.iter().any(|j| !j.status.finished() && matches!(&j.source, Source::Deploy { repo_idx, .. } if *repo_idx == idx));
        if busy {
            continue;
        }
        let head = match issues::sh("git", &["ls-remote", "origin", &format!("refs/heads/{}", r.base)], Some(&r.path)).await {
            Ok(out) => out.split_whitespace().next().unwrap_or("").to_string(),
            Err(e) => {
                hub.log("deploy", format!("{}: cannot read {}: {}", r.repo, r.base, e));
                continue;
            }
        };
        if head.len() < 40 {
            continue;
        }
        let key = saved_key(r, d);
        let saved = load_saved().get(&key).cloned();
        match saved {
            None => {
                // First sight of this repository: note where it is, deploy nothing. What is there now is
                // taken to be what is running, which makes it the rollback target for the next deployment.
                let s = Saved { seen: head.clone(), last_good: head.clone() };
                match store_saved(&key, &s) {
                    Ok(()) => hub.log("deploy", format!("{}: watching {} from {}; the next commit will be deployed to {}", r.repo, r.base, &head[..10], d.environment)),
                    Err(e) => hub.log("deploy", format!("{}: cannot write {}: {}", r.repo, state_path().display(), e)),
                }
            }
            Some(s) if s.seen == head => {}
            Some(mut s) => {
                s.seen = head.clone();
                // Record first: if this cannot be written, deploying would repeat on every poll.
                if let Err(e) = store_saved(&key, &s) {
                    hub.log("deploy", format!("{}: cannot write {}: {}; not deploying", r.repo, state_path().display(), e));
                    continue;
                }
                hub.submit(
                    format!("{} deploy {} to {}", r.repo, &head[..10], d.environment),
                    String::new(),
                    Source::Deploy { repo_idx: idx, repo: r.repo.clone(), sha: head },
                    r.path.clone(),
                    None,
                );
            }
        }
    }
}

fn step(hub: &Hub, id: u32, slot: Slot, label: &str, state: StageState, line: &str) {
    hub.job(id, |j| {
        let s = if slot == Slot::Worker { &mut j.worker } else { &mut j.reviewer };
        if s.label != label {
            *s = Stage::default();
            s.label = label.to_string();
            s.started = Some(Instant::now());
        }
        if state != StageState::Running && s.elapsed.is_none() {
            s.elapsed = s.started.map(|t| t.elapsed());
        }
        s.state = state;
        if !line.is_empty() {
            s.push(line.to_string());
        }
    });
}

/// Deploy one commit: trigger, wait until healthy, verify, and roll back if any of that fails.
pub async fn deploy_run(hub: &Hub, id: u32, cancel: &Cancel) -> (JobStatus, String) {
    let found = hub
        .job(id, |j| match &j.source {
            Source::Deploy { repo_idx, sha, .. } => hub.cfg.repos.get(*repo_idx).cloned().map(|r| (r, sha.clone())),
            _ => None,
        })
        .flatten();
    let Some((r, sha)) = found else { return (JobStatus::Failed, "not a deploy job".into()) };
    let Some(d) = r.deploy.clone() else { return (JobStatus::Failed, "no deploy stage configured".into()) };
    let key = saved_key(&r, &d);
    let mut saved = load_saved().get(&key).cloned().unwrap_or_default();
    let short = |s: &str| s[..s.len().min(10)].to_string();
    let mut vars = repo_vars(&r);
    vars.insert("sha".into(), sha.clone());
    vars.insert("env".into(), d.environment.clone());
    vars.insert("last_good".into(), saved.last_good.clone());
    vars.insert("message".into(), String::new());
    hub.job(id, |j| j.reviewer.label = "verify".into());

    let interval = Duration::from_secs(d.interval_secs.max(1));
    let limit = Duration::from_secs(d.timeout_mins.max(1) * 60);
    let mut pool = Pool::new(&hub.cfg);
    set(hub, id, JobStatus::Deploying);

    // 1. Trigger, 2. wait until this commit is healthy, 3. verify.
    let mut problem: Option<String> = None;
    step(hub, id, Slot::Worker, "deploy", StageState::Running, &format!("{} → {}", short(&sha), d.environment));
    if let Some(t) = &d.trigger {
        match pool.call(&d.server, t, &vars).await {
            Ok(res) if !res.is_error => step(hub, id, Slot::Worker, "deploy", StageState::Running, &format!("triggered with {}", t.tool)),
            Ok(res) => problem = Some(format!("{} refused: {}", t.tool, clip(&res.text.replace('\n', " "), 200))),
            Err(e) => problem = Some(format!("could not trigger the deployment: {}", e)),
        }
    }
    // A trigger that never went out changed nothing, so there is nothing to roll back.
    let triggered = problem.is_none();
    if problem.is_none() {
        let waited = wait(&mut pool, &d.server, &d.status, &vars, Some(&sha), interval, limit, cancel, |l| step(hub, id, Slot::Worker, "deploy", StageState::Running, l)).await;
        match waited {
            Waited::Success => step(hub, id, Slot::Worker, "deploy", StageState::Ok, "healthy"),
            Waited::Failure(t) => problem = Some(format!("the deployment failed: {}", clip(&t.replace('\n', " "), 200))),
            Waited::Unknown(w) => problem = Some(format!("the deployment did not become healthy: {}", w)),
            Waited::Cancelled => {
                pool.close().await;
                step(hub, id, Slot::Worker, "deploy", StageState::Failed, "cancelled");
                return (JobStatus::Cancelled, format!("cancelled while {} was deploying to {}; its state there is unknown", short(&sha), d.environment));
            }
        }
    }
    if problem.is_none() && !d.verify.is_empty() {
        step(hub, id, Slot::Reviewer, "verify", StageState::Running, &d.verify.join(" ; "));
        match run_checks(hub, id, &d.verify, &r.path, cancel).await {
            Some((true, _)) | None => step(hub, id, Slot::Reviewer, "verify", StageState::Ok, "passed"),
            Some((false, out)) => {
                step(hub, id, Slot::Reviewer, "verify", StageState::Failed, "failed");
                problem = Some(format!("verification failed: {}", clip(out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(""), 200)));
            }
        }
    }

    let (status, outcome) = match problem {
        None => {
            saved.last_good = sha.clone();
            saved.seen = sha.clone();
            if let Err(e) = store_saved(&key, &saved) {
                hub.log("deploy", format!("{}: cannot record the deployment: {}", r.repo, e));
            }
            (JobStatus::Done, format!("{} is deployed to {}", short(&sha), d.environment))
        }
        Some(why) => {
            step(hub, id, Slot::Worker, "deploy", StageState::Failed, &why);
            hub.log("deploy", format!("#{} {}", id, why));
            let rolled = match &d.rollback {
                _ if !triggered && d.trigger.is_some() => "Nothing was deployed, so nothing was rolled back.".to_string(),
                None => "No rollback is configured.".to_string(),
                Some(_) if saved.last_good.is_empty() || saved.last_good == sha => "No earlier good commit is known, so nothing was rolled back.".to_string(),
                Some(rb) => {
                    set(hub, id, JobStatus::RollingBack);
                    step(hub, id, Slot::Reviewer, "rollback", StageState::Running, &format!("to {}", short(&saved.last_good)));
                    let back = rollback(&mut pool, &d, rb, &vars, &saved.last_good, interval, limit, cancel, |l| step(hub, id, Slot::Reviewer, "rollback", StageState::Running, l)).await;
                    match back {
                        Ok(()) => {
                            step(hub, id, Slot::Reviewer, "rollback", StageState::Ok, "healthy again");
                            format!("Rolled back to {}.", short(&saved.last_good))
                        }
                        Err(e) => {
                            step(hub, id, Slot::Reviewer, "rollback", StageState::Failed, &e);
                            format!("THE ROLLBACK TO {} ALSO FAILED: {}. {} needs attention now.", short(&saved.last_good), e, d.environment)
                        }
                    }
                }
            };
            (JobStatus::Failed, format!("deploying {} to {} failed: {} {}", short(&sha), d.environment, why, rolled))
        }
    };
    if let Some(n) = &d.notify {
        vars.insert("message".into(), outcome.clone());
        if let Err(e) = pool.call(&d.server, n, &vars).await {
            hub.log("deploy", format!("#{} could not send the notification: {}", id, clip(&e, 160)));
        }
    }
    pool.close().await;
    (status, outcome)
}

#[allow(clippy::too_many_arguments)]
async fn rollback(
    pool: &mut Pool<'_>,
    d: &DeployCfg,
    rb: &CallCfg,
    vars: &Vars,
    last_good: &str,
    interval: Duration,
    limit: Duration,
    cancel: &Cancel,
    tick: impl FnMut(&str),
) -> Result<(), String> {
    match pool.call(&d.server, rb, vars).await {
        Ok(res) if !res.is_error => {}
        Ok(res) => return Err(format!("{} refused: {}", rb.tool, clip(&res.text.replace('\n', " "), 200))),
        Err(e) => return Err(e),
    }
    // Now the healthy state to wait for is the old commit, not the one that failed.
    let mut back = vars.clone();
    back.insert("sha".into(), last_good.to_string());
    match wait(pool, &d.server, &d.status, &back, Some(last_good), interval, limit, cancel, tick).await {
        Waited::Success => Ok(()),
        Waited::Failure(t) => Err(format!("it reports failure: {}", clip(&t.replace('\n', " "), 160))),
        Waited::Unknown(w) => Err(w),
        Waited::Cancelled => Err("cancelled before it was confirmed".into()),
    }
}

fn set(hub: &Hub, id: u32, s: JobStatus) {
    hub.job(id, |j| j.status = s);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(success: &str, failure: &str, pending: &str, revision: Option<&str>) -> CallCfg {
        let opt = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
        CallCfg { server: None, tool: "t".into(), args: toml::Value::Table(Default::default()), success: opt(success), failure: opt(failure), pending: opt(pending), revision: revision.map(String::from), capture: Default::default() }
    }

    #[test]
    fn renders_placeholders_and_numbers() {
        let args: toml::Value = toml::from_str(
            r#"owner = "{owner}"
               pullNumber = "{pr}"
               ref = "refs/heads/{branch}"
               title = "PR {pr} of {repo}"
               dry = false
               retries = 3
               nested = { sha = "{sha}", list = ["{name}", "{unknown}"] }"#,
        )
        .unwrap();
        let mut vars: Vars = HashMap::new();
        for (k, v) in [("owner", "o"), ("name", "r"), ("repo", "o/r"), ("pr", "12"), ("branch", "forkdeck/issue-7"), ("sha", "abc")] {
            vars.insert(k.to_string(), v.to_string());
        }
        let j = render(&args, &vars, &[]);
        assert_eq!(j["owner"], "o");
        assert_eq!(j["pullNumber"], 12, "a bare numeric placeholder is a number");
        assert_eq!(j["title"], "PR 12 of o/r", "inside text it stays text");
        assert_eq!(j["ref"], "refs/heads/forkdeck/issue-7");
        assert_eq!(j["dry"], false);
        assert_eq!(j["retries"], 3);
        assert_eq!(j["nested"]["sha"], "abc");
        assert_eq!(j["nested"]["list"][1], "{unknown}", "unknown placeholders are left alone");
    }

    #[test]
    fn reads_check_run_style_answers() {
        // Patterns of the kind used for a list of check runs.
        let c = call(r#""conclusion":\s*"success""#, r#""conclusion":\s*"(failure|timed_out|cancelled)""#, r#""status":\s*"(queued|in_progress)""#, None);
        assert_eq!(read(&c, r#"{"total_count":0,"check_runs":[]}"#, false, None), Reading::Pending, "CI has not started yet");
        assert_eq!(read(&c, r#"[{"status":"in_progress","conclusion":null}]"#, false, None), Reading::Pending);
        assert_eq!(read(&c, r#"[{"status":"completed","conclusion":"success"}]"#, false, None), Reading::Success);
        assert_eq!(read(&c, r#"[{"status":"completed","conclusion":"success"},{"status":"queued"}]"#, false, None), Reading::Pending, "not done until all are");
        assert_eq!(read(&c, r#"[{"conclusion":"failure"},{"status":"in_progress"}]"#, false, None), Reading::Failure, "one failure is enough");
        assert_eq!(read(&c, "anything", true, None), Reading::Failure, "a tool error is a failure");
    }

    #[test]
    fn a_deployment_answer_about_another_commit_is_pending() {
        let c = call("Healthy", "Degraded", "Progressing", Some(r"revision: ([0-9a-f]+)"));
        let new = "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        // Still on the old commit: neither its health nor its failure says anything about ours.
        assert_eq!(read(&c, "revision: 0000000bbb health: Healthy", false, Some(new)), Reading::Pending);
        assert_eq!(read(&c, "revision: 0000000bbb health: Degraded", false, Some(new)), Reading::Pending);
        assert_eq!(read(&c, "health: Healthy", false, Some(new)), Reading::Pending, "no revision in the answer");
        assert_eq!(read(&c, "revision: 1111111 health: Progressing", false, Some(new)), Reading::Pending);
        assert_eq!(read(&c, "revision: 1111111 health: Healthy", false, Some(new)), Reading::Success, "abbreviated commits match");
        assert_eq!(read(&c, &format!("revision: {} health: Degraded", new), false, Some(new)), Reading::Failure);
        assert_eq!(read(&c, "revision: 111 health: Healthy", false, Some(new)), Reading::Pending, "too short to be sure");
    }

    #[test]
    fn captures_feed_later_calls() {
        let mut c = call("", "", "", None);
        c.capture.insert("run_id".into(), r"/actions/runs/(\d+)".into());
        c.capture.insert("missing".into(), r"nothing-(\d+)".into());
        let mut vars: Vars = HashMap::new();
        vars.insert("sha".into(), "1234567".into());
        let got = capture(&c, r#"{"details_url":"https://example.invalid/o/r/actions/runs/9876/job/5"}"#, &mut vars);
        assert_eq!(got, vec!["run_id"]);
        let args: toml::Value = toml::from_str("run = \"{run_id}\"\nsha = \"{sha}\"\nother = \"{missing}\"").unwrap();
        let j = render(&args, &vars, &got);
        assert_eq!(j["run"], 9876, "a captured id is sent as a number");
        assert_eq!(j["sha"], "1234567", "a commit made of digits stays text");
        assert_eq!(j["other"], "{missing}");
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail_chars("abcdef", 10), "abcdef");
        assert_eq!(tail_chars("abcdéf", 3), "…déf");
    }
}
