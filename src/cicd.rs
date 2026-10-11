//! CI feedback and staging deployments, done by calling tools on MCP servers.
//!
//! forkdeck knows nothing about any particular CI or deployment system. The config names a server,
//! a tool, its arguments and how to read the answer; that is the whole integration.
//!
//! One rule runs through the deploy stage: **it only rolls back on evidence of failure**. A status
//! that says the commit being deployed is unhealthy, or a verify command that fails, is evidence.
//! A timeout, an unreadable status, a tool error, a cancel or a newer commit taking over is not:
//! those end the job with the state reported as unknown and nothing is touched.

use crate::agents::Slot;
use crate::config::{CallCfg, Config, DeployCfg, RepoCfg};
use crate::issues;
use crate::mcp::{self, Session, ToolResult};
use crate::orchestrator::{run_checks_detailed, Checked, Hub};
use crate::state::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub type Vars = HashMap<String, String>;

/// Placeholders that become JSON numbers when they are the whole value.
const NUMERIC: [&str; 2] = ["pr", "issue"];

/// Replace `{name}` with its value, in one pass: text that arrives inside a value is never looked
/// at again, so a value containing `{sha}` stays as it is. Unknown names are left alone.
pub fn fill(s: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let name_len = after.find(|c: char| !(c.is_ascii_lowercase() || c == '_')).unwrap_or(after.len());
        let closed = after[name_len..].starts_with('}');
        match vars.get(&after[..name_len]).filter(|_| closed && name_len > 0) {
            Some(v) => {
                out.push_str(v);
                rest = &after[name_len + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
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
    /// The deployment system gave up on the rollout and restored the old version by itself.
    Reverted,
    /// The rollout is waiting for a person.
    Paused,
}

fn matches(pattern: &Option<String>, text: &str) -> bool {
    pattern.as_deref().and_then(|p| Regex::new(p).ok()).map(|re| re.is_match(text)).unwrap_or(false)
}

fn same_revision(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    // Systems report full or abbreviated commits; seven characters is git's own short form.
    a.len() >= 7 && b.len() >= 7 && (a.starts_with(&b) || b.starts_with(&a))
}

/// Is this answer about the commit being waited for? True when the stage has no way to tell.
pub fn about(call: &CallCfg, text: &str, want_rev: Option<&str>) -> bool {
    match (&call.revision, want_rev) {
        (Some(p), Some(want)) => {
            let got = Regex::new(p).ok().and_then(|re| re.captures(text).and_then(|c| c.get(1).map(|m| m.as_str().to_string())));
            matches!(got, Some(g) if same_revision(&g, want))
        }
        _ => true,
    }
}

/// Decide what a tool's answer means. A tool error is not an answer; callers deal with that.
///
/// With a `revision` pattern, an answer about any other commit is "pending": a system that has
/// not picked up the new commit yet is still describing the old one, healthy or not.
///
/// The one exception is `reverted`. A system that aborts a rollout goes back to reporting the old
/// commit, so that answer is no longer about ours. It is believed once ours has been seen rolling
/// out (`seen_ours`); before that, it is left over from some earlier deployment.
pub fn read(call: &CallCfg, text: &str, want_rev: Option<&str>, seen_ours: bool) -> Reading {
    let ours = about(call, text, want_rev);
    if matches(&call.reverted, text) && (ours || seen_ours) {
        return Reading::Reverted;
    }
    if !ours {
        return Reading::Pending;
    }
    if matches(&call.failure, text) {
        Reading::Failure
    } else if matches(&call.paused, text) {
        Reading::Paused
    } else if matches(&call.pending, text) {
        Reading::Pending
    } else if matches(&call.success, text) {
        Reading::Success
    } else {
        Reading::Pending
    }
}

/// How far the rollout says it has got, if the stage says where to look.
pub fn progress(call: &CallCfg, text: &str) -> Option<String> {
    let re = Regex::new(call.progress.as_deref()?).ok()?;
    re.captures(text)?.get(1).map(|m| one_line(m.as_str(), 60)).filter(|s| !s.is_empty())
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

fn one_line(s: &str, n: usize) -> String {
    clip(&s.replace('\n', " "), n)
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

    async fn once(&mut self, name: &str, call: &CallCfg, args: &Value) -> Result<ToolResult, String> {
        if !self.open.contains_key(name) {
            let sc = self.cfg.mcp.get(name).ok_or_else(|| format!("no [mcp.{}] server is configured", name))?;
            self.open.insert(name.to_string(), Session::open(name, sc).await?);
        }
        let res = self.open.get_mut(name).expect("just inserted").call(&call.tool, args.clone()).await;
        if res.is_err() {
            // The session may be broken; the next call starts a fresh one.
            self.open.remove(name);
        }
        res
    }

    async fn call(&mut self, stage_server: &str, call: &CallCfg, vars: &Vars) -> Result<ToolResult, String> {
        let name = call.server.as_deref().unwrap_or(stage_server).to_string();
        let args = render(&call.args, vars, &self.captured);
        match self.once(&name, call, &args).await {
            // The server forgot our session. Nothing was executed, so asking again is safe.
            Err(e) if mcp::is_session_expired(&name, &e) => self.once(&name, call, &args).await,
            other => other,
        }
    }

    async fn close(self) {
        for (_, s) in self.open {
            s.close().await;
        }
    }
}

#[derive(Debug)]
enum Waited {
    Success,
    /// The answer said so. This is the only outcome that counts as evidence of failure. Carries the
    /// answer as it may be shown, and the values captured from it as it was sent.
    Failure(String, Vars),
    /// Timed out, or the status could not be read. Nothing is known.
    Unknown(String),
    /// The branch has a newer commit; waiting for this one no longer makes sense.
    Superseded(String),
    /// The deployment system stopped the rollout and restored the old version itself.
    Reverted(String),
    Cancelled,
}

struct WaitOpts<'a> {
    interval: Duration,
    limit: Duration,
    /// A failure reported this soon, before anything was seen running, may be left over from an
    /// earlier run. It is not believed until this much time has passed. Only without `revision`.
    settle: Duration,
    /// (local clone, branch, commit): stop when the branch no longer points at the commit.
    superseded_by: Option<(&'a Path, &'a str, &'a str)>,
    /// How long the rollout may wait for a person. That time is not charged to `limit`.
    pause_limit: Duration,
}

/// How many times in a row the status may be unreadable before giving up.
const MAX_ERRORS: u32 = 5;

#[allow(clippy::too_many_arguments)]
async fn wait(pool: &mut Pool<'_>, server: &str, call: &CallCfg, vars: &Vars, want_rev: Option<&str>, o: &WaitOpts<'_>, cancel: &Cancel, mut tick: impl FnMut(&str)) -> Waited {
    let start = Instant::now();
    let mut errors = 0;
    let mut last = String::new();
    let mut saw_running = false;
    let mut seen_ours = false;
    // Time the rollout has spent waiting for a person, and when the current wait began.
    let mut paused_total = Duration::ZERO;
    let mut paused_since: Option<Instant> = None;
    loop {
        if cancel.is_set() {
            return Waited::Cancelled;
        }
        // A tool that reports an error has not answered the question. One bad gateway or rate limit
        // must not be taken for a failed pipeline or a failed deployment, so it is counted like a
        // call that did not get through.
        let answer = match pool.call(server, call, vars).await {
            Ok(r) if r.is_error => Err(format!("{} reported an error: {}", call.tool, one_line(&r.shown, 160))),
            other => other,
        };
        match answer {
            Ok(r) => {
                errors = 0;
                last = r.shown.clone();
                // Patterns see the answer as it was sent; everything that is shown uses the blanked copy.
                let reading = read(call, &r.text, want_rev, seen_ours);
                seen_ours |= call.revision.is_some() && about(call, &r.text, want_rev);
                if reading != Reading::Paused {
                    if let Some(t) = paused_since.take() {
                        paused_total += t.elapsed();
                    }
                }
                // What the system says about how far it has got, or failing that the answer itself.
                let where_at = progress(call, &r.text).unwrap_or_else(|| one_line(&r.shown, 80));
                match reading {
                    Reading::Success => return Waited::Success,
                    Reading::Reverted => return Waited::Reverted(r.shown),
                    Reading::Paused => {
                        saw_running = true;
                        let since = *paused_since.get_or_insert_with(Instant::now);
                        if paused_total + since.elapsed() >= o.pause_limit {
                            return Waited::Unknown(format!("it has been waiting for someone to promote it for {} min; last answer: {}", o.pause_limit.as_secs() / 60, one_line(&last, 200)));
                        }
                        tick(&format!("paused, waiting for a person · {}", where_at));
                    }
                    Reading::Failure => {
                        let fresh = call.revision.is_some() || saw_running || start.elapsed() >= o.settle;
                        if fresh {
                            let mut got = Vars::new();
                            capture(call, &r.text, &mut got);
                            return Waited::Failure(r.shown, got);
                        }
                        tick("failure reported, possibly from an earlier run; waiting to see");
                    }
                    Reading::Pending => {
                        saw_running = true;
                        tick(&format!("pending · {}", where_at));
                    }
                }
            }
            Err(e) => {
                errors += 1;
                tick(&format!("status unreadable ({} of {}): {}", errors, MAX_ERRORS, clip(&e, 80)));
                if errors >= MAX_ERRORS {
                    return Waited::Unknown(format!("the status could not be read {} times in a row: {}", MAX_ERRORS, e));
                }
            }
        }
        if let Some((clone, branch, sha)) = o.superseded_by {
            if let Ok(out) = issues::sh("git", &["ls-remote", "origin", &format!("refs/heads/{}", branch)], Some(clone)).await {
                let head = out.split_whitespace().next().unwrap_or("");
                if head.len() >= 40 && head != sha {
                    return Waited::Superseded(head.to_string());
                }
            }
        }
        let paused_now = paused_total + paused_since.map(|t| t.elapsed()).unwrap_or_default();
        if start.elapsed().saturating_sub(paused_now) >= o.limit {
            return Waited::Unknown(format!("no result after {} min; last answer: {}", o.limit.as_secs() / 60, one_line(&last, 200)));
        }
        tokio::select! {
            _ = tokio::time::sleep(o.interval) => {}
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
    let Some(pr) = pr_url.as_deref().and_then(issues::pr_number) else {
        // Calling the tool with an empty number would produce an error that looks like a red pipeline.
        return Ci::Unknown(format!("could not tell the pull request number from {:?}", pr_url.unwrap_or_default()));
    };
    let mut vars = repo_vars(&r);
    vars.insert("issue".into(), issue.to_string());
    vars.insert("branch".into(), branch.unwrap_or_default());
    let sha = sha.unwrap_or_default();
    vars.insert("sha".into(), sha.clone());
    vars.insert("pr".into(), pr.to_string());

    hub.log("ci", format!("#{} waiting for CI on {} via {}.{}", id, &sha[..sha.len().min(10)], ci.status.server.as_deref().unwrap_or(&ci.server), ci.status.tool));
    let mut pool = Pool::new(&hub.cfg);
    let opts = WaitOpts {
        interval: Duration::from_secs(ci.interval_secs.max(1)),
        limit: Duration::from_secs(ci.timeout_mins.max(1) * 60),
        settle: Duration::from_secs(ci.settle_secs),
        superseded_by: None,
        pause_limit: Duration::from_secs(ci.timeout_mins.max(1) * 60),
    };
    let waited = wait(&mut pool, &ci.server, &ci.status, &vars, Some(&sha), &opts, cancel, |_| {}).await;
    let out = match waited {
        Waited::Success => {
            hub.log("ci", format!("#{} pipeline passed", id));
            Ci::Passed
        }
        Waited::Cancelled => Ci::Cancelled,
        Waited::Unknown(why) | Waited::Superseded(why) => Ci::Unknown(why),
        Waited::Reverted(status) => Ci::Failed(status),
        Waited::Failure(status, got) => {
            hub.log("ci", format!("#{} pipeline failed", id));
            // The status answer usually names the failing run; the log call needs that.
            pool.captured.extend(got.keys().cloned());
            vars.extend(got);
            let log = match &ci.logs {
                None => status,
                Some(call) => match pool.call(&ci.server, call, &vars).await {
                    Ok(l) if !l.is_error && !l.shown.trim().is_empty() => l.shown,
                    Ok(l) => {
                        if l.is_error {
                            hub.log("ci", format!("#{} the log tool reported an error; using the status instead", id));
                        }
                        status
                    }
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
#[serde(default)]
pub struct Saved {
    /// The newest base commit whose deployment ran to a conclusion, good or bad.
    pub seen: String,
    /// The newest commit that deployed and verified cleanly.
    pub last_good: String,
    /// A deployment that was started and has not reported back. If forkdeck starts and finds this
    /// set, the previous run was stopped or killed in the middle.
    pub in_progress: Option<String>,
    /// How many times that deployment has been started. An interrupted one is picked up again, but
    /// not for ever: something that keeps stopping forkdeck mid-deployment would otherwise trigger
    /// the same commit again and again.
    pub tries: u32,
    /// A rollback that was started and has not reported back.
    pub rolling_back: Option<String>,
}

/// How often one commit's deployment is started before a person has to look.
const MAX_TRIES: u32 = 3;

fn state_dir() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME").ok().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/state")
    });
    base.join("forkdeck")
}

fn state_path() -> PathBuf {
    state_dir().join("deploy.json")
}

/// Only one forkdeck may run deployments from a state directory: two of them (a cron `once` over a
/// running interface, say) would each wait on, and roll back, the other's work. Once taken, the lock
/// is held until the process exits. `Err` says why it could not be taken; a later call tries again,
/// so a process that lost to a short-lived one takes over when that one is gone.
fn deploy_lock() -> Result<(), String> {
    use std::os::fd::AsRawFd;
    static LOCK: Mutex<Option<std::fs::File>> = Mutex::new(None);
    let mut held = LOCK.lock().map_err(|_| "lock state poisoned".to_string())?;
    if held.is_some() {
        return Ok(());
    }
    let path = state_dir().join("deploy.lock");
    let cannot = |e: std::io::Error| format!("cannot use {} to make sure only one forkdeck deploys ({}); this one will not deploy", path.display(), e);
    std::fs::create_dir_all(state_dir()).map_err(cannot)?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(cannot)?;
    // SAFETY: plain syscall on a descriptor we own.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let e = std::io::Error::last_os_error();
        return Err(if e.kind() == std::io::ErrorKind::WouldBlock {
            format!("another forkdeck is running deployments from {}; this one will not deploy while that one runs", state_dir().display())
        } else {
            cannot(e)
        });
    }
    *held = Some(f);
    Ok(())
}

/// Read the state file. A file that exists but cannot be understood is an error, not "no state":
/// treating it as empty would make the commit that is there now the rollback target.
fn load_saved() -> Result<HashMap<String, Saved>, String> {
    let path = state_path();
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(format!("cannot read {}: {}", path.display(), e)),
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{} is damaged ({}). Repair it or delete it; until then nothing is deployed", path.display(), e)),
    }
}

/// Change one repository's entry. Read, modify and write happen under one lock, and the write is a
/// rename, so a crash or a second job cannot leave half a file or lose the other's update.
fn update_saved(key: &str, change: impl FnOnce(&mut Saved)) -> Result<Saved, String> {
    static WRITE: Mutex<()> = Mutex::new(());
    let _held = WRITE.lock().map_err(|_| "state lock poisoned".to_string())?;
    let path = state_path();
    let mut all = load_saved()?;
    let entry = all.entry(key.to_string()).or_default();
    change(entry);
    let now = entry.clone();
    std::fs::create_dir_all(state_dir()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    Ok(now)
}

fn saved_key(r: &RepoCfg, d: &DeployCfg) -> String {
    format!("{:?}:{}:{}", r.provider, r.repo, d.environment).to_lowercase()
}

/// Say something once, not on every poll.
fn note_once(hub: &Hub, msg: String) {
    let mut st = hub.state.lock().unwrap();
    if st.deploy_note.as_deref() != Some(msg.as_str()) {
        st.log("deploy", msg.clone());
        st.deploy_note = Some(msg);
    }
}

/// What to do about one repository, given what is recorded and where its branch is now.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Never seen: note the commit, deploy nothing.
    Adopt,
    Nothing,
    /// Start a deployment; the number is which try this is for that commit.
    Deploy(u32),
    /// The same commit was interrupted too often. Stop and say so.
    GiveUp(u32),
}

pub fn plan(saved: Option<&Saved>, head: &str) -> Plan {
    let Some(s) = saved else { return Plan::Adopt };
    match &s.in_progress {
        None if s.seen == head => Plan::Nothing,
        None => Plan::Deploy(1),
        Some(p) if p == head && s.tries >= MAX_TRIES => Plan::GiveUp(s.tries),
        Some(p) if p == head => Plan::Deploy(s.tries + 1),
        // The interrupted one was for an older commit; the head is a fresh start.
        Some(_) => Plan::Deploy(1),
    }
}

/// Look at the base branch of every repository with a deploy stage and start a deployment when it has moved.
pub async fn deploy_poll(hub: &Hub) {
    if hub.cfg.demo || !hub.cfg.repos.iter().any(|r| r.deploy.is_some()) {
        return;
    }
    if let Err(why) = deploy_lock() {
        return note_once(hub, why);
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
        // Read the record now, not once for the whole pass: a job for another repository may have
        // finished and written while this one's branch was being looked up.
        let saved = match load_saved() {
            Ok(all) => all.get(&key).cloned(),
            Err(e) => return note_once(hub, e),
        };
        if let Some(target) = saved.as_ref().and_then(|s| s.rolling_back.clone()) {
            hub.log("deploy", format!("{}: a rollback to {} was under way when forkdeck last stopped and was never confirmed. Check {} by hand", r.repo, short(&target), d.environment));
            record(hub, r, &key, |s| s.rolling_back = None);
        }
        // The user cancelled this one in this session; do not start it again behind their back.
        if hub.state.lock().unwrap().deploy_cancelled.contains(&(idx, head.clone())) {
            continue;
        }
        match plan(saved.as_ref(), &head) {
            Plan::Nothing => {}
            Plan::Adopt => {
                // Nothing is deployed on first sight. Whether there is something to roll back to is a
                // separate question: the branch head is only a rollback target if the deployment
                // system itself says that commit is what is running, and healthy.
                let baseline = running_and_healthy(hub, r, d, &head).await;
                let last_good = if baseline.is_ok() { head.clone() } else { String::new() };
                match update_saved(&key, |s| *s = Saved { seen: head.clone(), last_good, ..Saved::default() }) {
                    Ok(_) => match baseline {
                        Ok(()) => hub.log("deploy", format!("{}: watching {} from {}, which {} reports healthy; the next commit will be deployed there", r.repo, r.base, short(&head), d.environment)),
                        Err(why) => hub.log(
                            "deploy",
                            format!(
                                "{}: watching {} from {}. {} could not be confirmed to be running that commit healthily ({}), so there is nothing to roll back to until a deployment succeeds",
                                r.repo,
                                r.base,
                                short(&head),
                                d.environment,
                                why
                            ),
                        ),
                    },
                    Err(e) => note_once(hub, format!("{}: {}", r.repo, e)),
                }
            }
            Plan::GiveUp(tries) => {
                hub.log("deploy", format!("{}: the deployment of {} to {} was interrupted {} times and will not be started again. Its state there is unknown; check it by hand", r.repo, short(&head), d.environment, tries));
                record(hub, r, &key, |s| {
                    s.seen = head.clone();
                    s.in_progress = None;
                    s.tries = 0;
                });
            }
            Plan::Deploy(n) => {
                if let Some(p) = saved.as_ref().and_then(|s| s.in_progress.clone()) {
                    hub.log("deploy", format!("{}: the deployment of {} was started and never finished (was forkdeck stopped?). Its state in {} is unknown; deploying {} now (try {} of {})", r.repo, short(&p), d.environment, short(&head), n, MAX_TRIES));
                }
                // Record first: if this cannot be written, a crash would lose track of a running deployment.
                if let Err(e) = update_saved(&key, |s| {
                    s.in_progress = Some(head.clone());
                    s.tries = n;
                }) {
                    note_once(hub, format!("{}: {}; not deploying", r.repo, e));
                    continue;
                }
                hub.submit(
                    format!("{} deploy {} to {}", r.repo, short(&head), d.environment),
                    String::new(),
                    Source::Deploy { repo_idx: idx, repo: r.repo.clone(), sha: head },
                    r.path.clone(),
                    None,
                );
            }
        }
    }
}

/// Ask the deployment system, once, whether `sha` is what is running and healthy.
async fn running_and_healthy(hub: &Hub, r: &RepoCfg, d: &DeployCfg, sha: &str) -> Result<(), String> {
    let mut vars = repo_vars(r);
    vars.insert("sha".into(), sha.to_string());
    vars.insert("env".into(), d.environment.clone());
    vars.insert("last_good".into(), String::new());
    vars.insert("message".into(), String::new());
    let mut pool = Pool::new(&hub.cfg);
    let answer = pool.call(&d.server, &d.status, &vars).await;
    pool.close().await;
    match answer {
        Err(e) => Err(format!("the status could not be read: {}", clip(&e, 160))),
        Ok(res) if res.is_error => Err(format!("{} reported an error", d.status.tool)),
        Ok(res) => match read(&d.status, &res.text, Some(sha), false) {
            Reading::Success => Ok(()),
            _ if !about(&d.status, &res.text, Some(sha)) => Err("it is running a different commit".to_string()),
            other => Err(format!("its status reads {:?}", other).to_lowercase()),
        },
    }
}

/// Where the branch points on the remote right now, if that can be found out.
async fn branch_head(r: &RepoCfg) -> Option<String> {
    let out = issues::sh("git", &["ls-remote", "origin", &format!("refs/heads/{}", r.base)], Some(&r.path)).await.ok()?;
    out.split_whitespace().next().filter(|h| h.len() >= 40).map(String::from)
}

fn step(hub: &Hub, id: u32, slot: Slot, label: &str, state: StageState, line: &str) {
    hub.job(id, |j| {
        let s = if slot == Slot::Worker { &mut j.worker } else { &mut j.reviewer };
        if s.label != label {
            *s = Stage::default();
            s.label = label.to_string();
            s.started = Some(Instant::now());
        }
        if s.started.is_none() {
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

fn short(s: &str) -> String {
    s[..s.len().min(10)].to_string()
}

/// How a deployment ended, before any rollback.
enum End {
    Good,
    /// Evidence that this commit is bad where it was deployed. The only case that rolls back.
    Bad(String),
    /// The trigger tool said no. Nothing changed, so there is nothing to undo.
    Refused(String),
    /// No evidence either way.
    Unknown(String),
    /// The deployment system judged the rollout bad and restored the old version itself.
    Reverted(String),
    Superseded(String),
    Cancelled(&'static str),
}

/// Deploy one commit: trigger, wait until healthy, verify, and roll back on evidence of failure.
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
    let saved = match load_saved() {
        Ok(all) => all.get(&key).cloned().unwrap_or_default(),
        Err(e) => return (JobStatus::Failed, format!("nothing was deployed: {}", e)),
    };
    let env = d.environment.clone();
    let mut vars = repo_vars(&r);
    vars.insert("sha".into(), sha.clone());
    vars.insert("env".into(), env.clone());
    vars.insert("last_good".into(), saved.last_good.clone());
    vars.insert("message".into(), String::new());
    hub.job(id, |j| j.reviewer.label = "verify".into());

    let mut pool = Pool::new(&hub.cfg);
    // State that could not be written. The deployment still happened; forkdeck's record of it did not.
    let mut unrecorded: Vec<String> = Vec::new();
    set(hub, id, JobStatus::Deploying);
    step(hub, id, Slot::Worker, "deploy", StageState::Running, &format!("{} → {}", short(&sha), env));
    let end = attempt(hub, id, &mut pool, &r, &d, &vars, &sha, cancel).await;

    let (status, outcome) = match end {
        End::Good => {
            step(hub, id, Slot::Worker, "deploy", StageState::Ok, "healthy");
            record_into(&mut unrecorded, hub, &r, &key, |s| {
                s.last_good = sha.clone();
                s.seen = sha.clone();
                s.in_progress = None;
                s.tries = 0;
            });
            (JobStatus::Done, format!("{} is deployed to {}", short(&sha), env))
        }
        End::Superseded(newer) => {
            step(hub, id, Slot::Worker, "deploy", StageState::Skipped, "superseded");
            // Forget what was seen, so the next poll deploys wherever the branch is then. That also
            // covers a branch forced back to a commit that was deployed before: this one was
            // triggered, so the environment is no longer known to be on the old one.
            record_into(&mut unrecorded, hub, &r, &key, |s| {
                s.seen = String::new();
                s.in_progress = None;
                s.tries = 0;
            });
            (JobStatus::Done, format!("stopped waiting for {}: {} now points at {}, which is deployed next. Nothing was rolled back", short(&sha), r.base, short(&newer)))
        }
        End::Cancelled(during) => {
            step(hub, id, Slot::Worker, "deploy", StageState::Failed, "cancelled");
            // `in_progress` is left set on purpose: the next start of forkdeck picks this up again.
            (JobStatus::Cancelled, format!("cancelled while {} of {} to {}; its state there is unknown and nothing was rolled back", during, short(&sha), env))
        }
        End::Refused(why) => {
            step(hub, id, Slot::Worker, "deploy", StageState::Failed, &why);
            record_into(&mut unrecorded, hub, &r, &key, |s| {
                s.seen = sha.clone();
                s.in_progress = None;
                s.tries = 0;
            });
            (JobStatus::Failed, format!("{} was not deployed to {}: {}", short(&sha), env, why))
        }
        End::Reverted(said) => {
            // The system that runs the rollout decided, and has already acted. Calling a rollback on
            // top of its own would be two hands on the same wheel.
            step(hub, id, Slot::Worker, "deploy", StageState::Failed, "stopped and reverted by the deployment system");
            record_into(&mut unrecorded, hub, &r, &key, |s| {
                s.seen = sha.clone();
                s.in_progress = None;
                s.tries = 0;
            });
            (
                JobStatus::Failed,
                format!("deploying {} to {} failed: the deployment system stopped the rollout and restored the previous version itself ({}). forkdeck called no rollback", short(&sha), env, said),
            )
        }
        End::Unknown(why) => {
            step(hub, id, Slot::Worker, "deploy", StageState::Failed, &why);
            record_into(&mut unrecorded, hub, &r, &key, |s| {
                s.seen = sha.clone();
                s.in_progress = None;
                s.tries = 0;
            });
            (JobStatus::Failed, format!("the state of {} in {} is UNKNOWN: {}. Nothing was rolled back; check {} by hand", short(&sha), env, why, env))
        }
        End::Bad(why) => {
            step(hub, id, Slot::Worker, "deploy", StageState::Failed, &why);
            hub.log("deploy", format!("#{} {}", id, why));
            record_into(&mut unrecorded, hub, &r, &key, |s| {
                s.seen = sha.clone();
                s.in_progress = None;
                s.tries = 0;
            });
            // The evidence is about this commit. If the branch has moved since, the environment is
            // on its way to something else, and rolling back now would undo that instead.
            // That has to be known, not assumed: if the branch cannot be read, a newer commit may be
            // rolling out right now, and a rollback is not something to do on a guess.
            let head_now = if d.rollback.is_some() { branch_head(&r).await } else { Some(sha.clone()) };
            let rolled = match (&d.rollback, &head_now) {
                (Some(_), None) => format!(
                    "Nothing was rolled back, because {} could not be read to make sure no newer commit is on its way. {} is left as it is and needs attention.",
                    r.base, env
                ),
                (Some(_), Some(to)) if to != &sha => {
                    record_into(&mut unrecorded, hub, &r, &key, |s| s.seen = String::new());
                    format!("Nothing was rolled back, because {} already points at {}, which is deployed next.", r.base, short(to))
                }
                _ => match &d.rollback {
                None => "No rollback is configured.".to_string(),
                Some(_) if saved.last_good.is_empty() || saved.last_good == sha => "No earlier good commit is known, so nothing was rolled back.".to_string(),
                Some(rb) => {
                    set(hub, id, JobStatus::RollingBack);
                    step(hub, id, Slot::Reviewer, "rollback", StageState::Running, &format!("to {}", short(&saved.last_good)));
                    // Written down before it starts: if forkdeck dies now, the next start says so.
                    record_into(&mut unrecorded, hub, &r, &key, |s| s.rolling_back = Some(saved.last_good.clone()));
                    let back = rollback(&mut pool, &d, rb, &vars, &saved.last_good, cancel, |l| step(hub, id, Slot::Reviewer, "rollback", StageState::Running, l)).await;
                    // Either way it has reported back; an unconfirmed one is in the outcome below.
                    record_into(&mut unrecorded, hub, &r, &key, |s| s.rolling_back = None);
                    match back {
                        Ok(()) => {
                            step(hub, id, Slot::Reviewer, "rollback", StageState::Ok, "healthy again");
                            format!("Rolled back to {}.", short(&saved.last_good))
                        }
                        Err(e) => {
                            step(hub, id, Slot::Reviewer, "rollback", StageState::Failed, &e);
                            format!("THE ROLLBACK TO {} WAS NOT CONFIRMED: {}. {} needs attention now.", short(&saved.last_good), e, env)
                        }
                    }
                }
                },
            };
            (JobStatus::Failed, format!("deploying {} to {} failed: {} {}", short(&sha), env, why, rolled))
        }
    };
    // What happened out there and what forkdeck has on record must not silently part ways.
    let (status, outcome) = match unrecorded.first() {
        None => (status, outcome),
        Some(e) => (
            if status == JobStatus::Done { JobStatus::Failed } else { status },
            format!(
                "{}. THIS COULD NOT BE RECORDED ({}): forkdeck's record of {} is out of date, so its next decision there cannot be trusted. Repair {} and check it before the next deployment",
                outcome,
                e,
                env,
                state_path().display()
            ),
        ),
    };
    if let Some(n) = d.notify.as_ref().filter(|_| status != JobStatus::Cancelled) {
        // Text from tools and commands goes out to a chat or a ticket: no stray mentions.
        vars.insert("message".into(), issues::quiet(&outcome));
        match pool.call(&d.server, n, &vars).await {
            Ok(res) if res.is_error => hub.log("deploy", format!("#{} the notification was refused: {}", id, one_line(&res.shown, 160))),
            Ok(_) => {}
            Err(e) => hub.log("deploy", format!("#{} could not send the notification: {}", id, clip(&e, 160))),
        }
    }
    pool.close().await;
    (status, outcome)
}

/// As `record`, and keeps the reason when the state could not be written.
fn record_into(failed: &mut Vec<String>, hub: &Hub, r: &RepoCfg, key: &str, change: impl FnOnce(&mut Saved)) {
    if let Err(e) = update_saved(key, change) {
        hub.log("deploy", format!("{}: cannot record the deployment: {}", r.repo, e));
        failed.push(e);
    }
}

fn record(hub: &Hub, r: &RepoCfg, key: &str, change: impl FnOnce(&mut Saved)) {
    if let Err(e) = update_saved(key, change) {
        hub.log("deploy", format!("{}: cannot record the deployment: {}", r.repo, e));
    }
}

#[allow(clippy::too_many_arguments)]
async fn attempt(hub: &Hub, id: u32, pool: &mut Pool<'_>, r: &RepoCfg, d: &DeployCfg, vars: &Vars, sha: &str, cancel: &Cancel) -> End {
    if let Some(t) = &d.trigger {
        match pool.call(&d.server, t, vars).await {
            Ok(res) if !res.is_error => step(hub, id, Slot::Worker, "deploy", StageState::Running, &format!("triggered with {}", t.tool)),
            Ok(res) => return End::Refused(format!("{} refused: {}", t.tool, one_line(&res.shown, 200))),
            // No answer is not a no: the server may well have started. Keep watching.
            Err(e) => {
                hub.log("deploy", format!("#{} the trigger call did not return ({}); watching the status anyway", id, clip(&e, 120)));
                step(hub, id, Slot::Worker, "deploy", StageState::Running, "trigger unanswered, watching");
            }
        }
    }
    if cancel.is_set() {
        return End::Cancelled("starting the deployment");
    }
    let opts = WaitOpts {
        interval: Duration::from_secs(d.interval_secs.max(1)),
        limit: Duration::from_secs(d.timeout_mins.max(1) * 60),
        settle: Duration::ZERO,
        superseded_by: Some((r.path.as_path(), r.base.as_str(), sha)),
        pause_limit: Duration::from_secs(d.pause_timeout_mins.max(1) * 60),
    };
    match wait(pool, &d.server, &d.status, vars, Some(sha), &opts, cancel, |l| step(hub, id, Slot::Worker, "deploy", StageState::Running, l)).await {
        Waited::Success => {}
        Waited::Failure(t, _) => return End::Bad(format!("{} reports failure: {}", d.status.tool, one_line(&t, 200))),
        Waited::Unknown(w) => return End::Unknown(w),
        Waited::Reverted(t) => return End::Reverted(one_line(&t, 200)),
        Waited::Superseded(newer) => return End::Superseded(newer),
        Waited::Cancelled => return End::Cancelled("waiting for the deployment"),
    }
    if !d.verify.is_empty() {
        step(hub, id, Slot::Reviewer, "verify", StageState::Running, &d.verify.join(" ; "));
        let res = run_checks_detailed(hub, id, &d.verify, &r.path, cancel).await;
        // These are not the project's checks; do not show them as such.
        hub.job(id, |j| j.checks = None);
        // A verify command that was stopped has not failed. Rolling back on a quit would undo a good deployment.
        if cancel.is_set() {
            step(hub, id, Slot::Reviewer, "verify", StageState::Failed, "cancelled");
            return End::Cancelled("verifying the deployment");
        }
        let last_line = |out: &str| clip(out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(""), 200);
        match res {
            Some(Checked::Passed) | None => step(hub, id, Slot::Reviewer, "verify", StageState::Ok, "passed"),
            // The command ran and said no: that is evidence about the deployment.
            Some(Checked::Failed(out)) => {
                step(hub, id, Slot::Reviewer, "verify", StageState::Failed, "failed");
                return End::Bad(format!("verification failed: {}", last_line(&out)));
            }
            // The command never gave an answer (it hung, could not start, was killed). That is a
            // problem with the check, and says nothing about what was deployed.
            Some(Checked::Errored(out)) => {
                step(hub, id, Slot::Reviewer, "verify", StageState::Failed, "gave no result");
                return End::Unknown(format!("it reported healthy, but verification gave no result: {}", last_line(&out)));
            }
        }
    }
    End::Good
}

async fn rollback(pool: &mut Pool<'_>, d: &DeployCfg, rb: &CallCfg, vars: &Vars, last_good: &str, cancel: &Cancel, tick: impl FnMut(&str)) -> Result<(), String> {
    match pool.call(&d.server, rb, vars).await {
        Ok(res) if !res.is_error => {}
        Ok(res) => return Err(format!("{} refused: {}", rb.tool, one_line(&res.shown, 200))),
        Err(e) => return Err(e),
    }
    // Now the healthy state to wait for is the old commit, not the one that failed.
    let mut back = vars.clone();
    back.insert("sha".into(), last_good.to_string());
    let limit = Duration::from_secs(d.timeout_mins.max(1) * 60);
    let opts = WaitOpts { interval: Duration::from_secs(d.interval_secs.max(1)), limit, settle: Duration::ZERO, superseded_by: None, pause_limit: limit };
    match wait(pool, &d.server, &d.status, &back, Some(last_good), &opts, cancel, tick).await {
        Waited::Success => Ok(()),
        Waited::Failure(t, _) => Err(format!("it reports failure: {}", one_line(&t, 160))),
        Waited::Unknown(w) | Waited::Superseded(w) => Err(w),
        Waited::Reverted(t) => Err(format!("the deployment system reports it reverted: {}", one_line(&t, 160))),
        Waited::Cancelled => Err("forkdeck was stopped before it was confirmed".into()),
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
        CallCfg {
            server: None,
            tool: "t".into(),
            args: toml::Value::Table(Default::default()),
            success: opt(success),
            failure: opt(failure),
            pending: opt(pending),
            revision: revision.map(String::from),
            reverted: None,
            paused: None,
            progress: None,
            capture: Default::default(),
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
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
        let v = vars(&[("owner", "o"), ("name", "r"), ("repo", "o/r"), ("pr", "12"), ("branch", "forkdeck/issue-7"), ("sha", "abc")]);
        let j = render(&args, &v, &[]);
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
    fn substitution_is_a_single_pass() {
        // Text that arrives inside a value is data. It is never expanded again, whatever it looks like.
        let v = vars(&[("message", "smoke test printed {env} {repo} {message}"), ("env", "staging"), ("repo", "o/r")]);
        for _ in 0..20 {
            assert_eq!(fill("{env}: {message}", &v), "staging: smoke test printed {env} {repo} {message}");
        }
        assert_eq!(fill("{", &v), "{");
        assert_eq!(fill("a {env", &v), "a {env");
        assert_eq!(fill("{{env}}", &v), "{staging}");
        assert_eq!(fill("{ENV} {} {env}{repo}", &v), "{ENV} {} stagingo/r");
    }

    #[test]
    fn reads_check_run_style_answers() {
        // Patterns of the kind used for a list of check runs.
        let c = call(r#""conclusion":\s*"success""#, r#""conclusion":\s*"(failure|timed_out|cancelled)""#, r#""status":\s*"(queued|in_progress)""#, None);
        assert_eq!(read(&c, r#"{"total_count":0,"check_runs":[]}"#, None, false), Reading::Pending, "CI has not started yet");
        assert_eq!(read(&c, r#"[{"status":"in_progress","conclusion":null}]"#, None, false), Reading::Pending);
        assert_eq!(read(&c, r#"[{"status":"completed","conclusion":"success"}]"#, None, false), Reading::Success);
        assert_eq!(read(&c, r#"[{"status":"completed","conclusion":"success"},{"status":"queued"}]"#, None, false), Reading::Pending, "not done until all are");
        assert_eq!(read(&c, r#"[{"conclusion":"failure"},{"status":"in_progress"}]"#, None, false), Reading::Failure, "one failure is enough");
    }

    #[test]
    fn a_deployment_answer_about_another_commit_is_pending() {
        let c = call("Healthy", "Degraded", "Progressing", Some(r"revision: ([0-9a-f]+)"));
        let new = "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        // Still on the old commit: neither its health nor its failure says anything about ours.
        assert_eq!(read(&c, "revision: 0000000bbb health: Healthy", Some(new), false), Reading::Pending);
        assert_eq!(read(&c, "revision: 0000000bbb health: Degraded", Some(new), false), Reading::Pending);
        assert_eq!(read(&c, "health: Healthy", Some(new), false), Reading::Pending, "no revision in the answer");
        assert_eq!(read(&c, "revision: 1111111 health: Progressing", Some(new), false), Reading::Pending);
        assert_eq!(read(&c, "revision: 1111111 health: Healthy", Some(new), false), Reading::Success, "abbreviated commits match");
        assert_eq!(read(&c, &format!("revision: {} health: Degraded", new), Some(new), false), Reading::Failure);
        assert_eq!(read(&c, "revision: 111 health: Healthy", Some(new), false), Reading::Pending, "too short to be sure");
    }

    #[test]
    fn a_progressive_rollout_is_read_step_by_step() {
        let mut c = call("phase=Healthy", "phase=Degraded", "phase=Progressing", Some(r"rev=([0-9a-f]+)"));
        c.reverted = Some("phase=Aborted".into());
        c.paused = Some("phase=Paused".into());
        c.progress = Some(r"step=(.+?)(?: phase|$)".into());
        let new = "1111111aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        // The strategy is the system's: forkdeck only reads how far it says it has got.
        assert_eq!(read(&c, "rev=1111111 step=canary 20% phase=Progressing", Some(new), false), Reading::Pending);
        assert_eq!(progress(&c, "rev=1111111 step=canary 20% phase=Progressing").as_deref(), Some("canary 20%"));
        assert_eq!(progress(&c, "rev=1111111 step=eu-west 2 of 5 regions phase=Paused").as_deref(), Some("eu-west 2 of 5 regions"));
        assert_eq!(progress(&c, "rev=1111111 phase=Progressing"), None);
        assert_eq!(read(&c, "rev=1111111 step=canary 50% phase=Paused", Some(new), true), Reading::Paused);
        assert_eq!(read(&c, "rev=1111111 phase=Healthy", Some(new), true), Reading::Success);

        // Aborted: the system is back on the old commit and says so.
        assert_eq!(read(&c, "rev=0000000 phase=Aborted", Some(new), true), Reading::Reverted, "believed once ours was seen rolling out");
        assert_eq!(read(&c, "rev=0000000 phase=Aborted", Some(new), false), Reading::Pending, "before that it is left over from an earlier deployment");
        assert_eq!(read(&c, "rev=1111111 phase=Aborted", Some(new), false), Reading::Reverted, "an abort that still names ours is ours");
        // A paused rollout of some other commit says nothing about ours.
        assert_eq!(read(&c, "rev=0000000 phase=Paused", Some(new), true), Reading::Pending);
    }

    #[test]
    fn captures_feed_later_calls() {
        let mut c = call("", "", "", None);
        c.capture.insert("run_id".into(), r"/actions/runs/(\d+)".into());
        c.capture.insert("missing".into(), r"nothing-(\d+)".into());
        let mut v = vars(&[("sha", "1234567")]);
        let got = capture(&c, r#"{"details_url":"https://example.invalid/o/r/actions/runs/9876/job/5"}"#, &mut v);
        assert_eq!(got, vec!["run_id"]);
        let args: toml::Value = toml::from_str("run = \"{run_id}\"\nsha = \"{sha}\"\nother = \"{missing}\"").unwrap();
        let j = render(&args, &v, &got);
        assert_eq!(j["run"], 9876, "a captured id is sent as a number");
        assert_eq!(j["sha"], "1234567", "a commit made of digits stays text");
        assert_eq!(j["other"], "{missing}");
    }

    #[test]
    fn deployment_plan() {
        let s = |seen: &str, prog: Option<&str>, tries: u32| Saved { seen: seen.into(), last_good: "g".into(), in_progress: prog.map(String::from), tries, rolling_back: None };
        assert_eq!(plan(None, "a"), Plan::Adopt, "first sight deploys nothing");
        assert_eq!(plan(Some(&s("a", None, 0)), "a"), Plan::Nothing);
        assert_eq!(plan(Some(&s("a", None, 0)), "b"), Plan::Deploy(1));
        assert_eq!(plan(Some(&s("", None, 0)), "a"), Plan::Deploy(1), "after a superseded one, wherever the branch is gets deployed");
        // Interrupted: picked up again, a bounded number of times.
        assert_eq!(plan(Some(&s("a", Some("b"), 1)), "b"), Plan::Deploy(2));
        assert_eq!(plan(Some(&s("a", Some("b"), 2)), "b"), Plan::Deploy(3));
        assert_eq!(plan(Some(&s("a", Some("b"), 3)), "b"), Plan::GiveUp(3), "not triggered for ever");
        assert_eq!(plan(Some(&s("a", Some("b"), 3)), "c"), Plan::Deploy(1), "a newer commit starts afresh");
    }

    #[test]
    fn old_state_files_still_load() {
        let old = r#"{"github:o/r:staging":{"seen":"a","last_good":"a","in_progress":null}}"#;
        let all: HashMap<String, Saved> = serde_json::from_str(old).unwrap();
        assert_eq!(all["github:o/r:staging"].tries, 0);
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail_chars("abcdef", 10), "abcdef");
        assert_eq!(tail_chars("abcdéf", 3), "…déf");
    }
}
