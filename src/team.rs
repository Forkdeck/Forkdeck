//! Who is working on what, kept on the forge so that teammates running their own forkdeck see it.
//!
//! A claim is one comment on the issue, pull request or commit. It starts with a hidden marker
//! (a line of JSON in an HTML comment) and is edited in place as the job moves on. Only metadata
//! goes into it: who, which machine, which agent, which stage, since when. No prompt, no agent
//! output, no logs. Typed tasks never leave the machine.
//!
//! The marker is read back from comments anyone may have written, so nothing in it is trusted
//! beyond what the forge itself vouches for: a marker counts only if the comment's author is the
//! person it names and the forge says that account may write to the repository, and its age is
//! the forge's own "last edited" time, not a clock in the marker.
//!
//! A claim is a lease, not a flag: everything forkdeck writes to a claimed item first asks
//! `standing`, and goes ahead only when this process holds the winning claim right now.

use crate::config::RepoCfg;
use crate::issues;
use crate::orchestrator::Hub;
use crate::state::{clip, JobStatus, Provider, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

const OPEN: &str = "<!-- forkdeck:claim ";
const CLOSE: &str = " -->";
/// Prefix of the error `issues::prepare` returns when somebody else holds the item.
pub const TAKEN: &str = "taken: ";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Marker {
    pub v: u32,
    /// Names this claim, so a later comment can end it.
    pub id: String,
    pub by: String,
    pub host: String,
    pub pid: u32,
    pub agent: String,
    /// active, done, failed, cancelled, yielded or released.
    pub state: String,
    pub stage: String,
    pub pr: String,
    pub started: i64,
    pub beat: i64,
    /// Claims this comment ends: a release, or a new claim replacing ones that had gone quiet.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ends: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    pub body: String,
    /// When the forge last saw this comment change, in seconds since the epoch. 0 if it did not say.
    pub updated: i64,
    /// The forge says the author may write to the repository. Only such comments can claim or release.
    pub trusted: bool,
    /// GitLab's numeric user id, which its membership lookup wants. 0 elsewhere.
    pub author_id: u64,
}

/// A marker found in a comment, with the time it was last touched.
#[derive(Clone, Debug)]
pub struct Seen {
    pub comment: u64,
    pub m: Marker,
    pub beat: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Issue(u64),
    Pr(u64),
    Commit(String),
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Text that came out of somebody's comment, made safe to print on a terminal.
fn plain(s: &str, max: usize) -> String {
    clip(&s.chars().filter(|c| !c.is_control() && *c != '\u{200b}').collect::<String>(), max)
}

fn clean(mut m: Marker) -> Marker {
    m.id = plain(&m.id, 40);
    m.by = plain(&m.by, 60);
    m.host = plain(&m.host, 60);
    m.agent = plain(&m.agent, 30);
    m.state = plain(&m.state, 20);
    m.stage = plain(&m.stage, 40);
    m.pr = if m.pr.starts_with("https://") || m.pr.starts_with("http://") { plain(&m.pr, 200) } else { String::new() };
    m.ends = m.ends.iter().take(20).map(|e| plain(e, 40)).collect();
    m
}

/// The marker a comment starts with, if it has one.
pub fn parse(body: &str) -> Option<Marker> {
    let rest = body.trim_start().strip_prefix(OPEN)?;
    let json = &rest[..rest.find(CLOSE)?];
    if json.len() > 4000 {
        return None;
    }
    let m: Marker = serde_json::from_str(json).ok()?;
    (m.v == 1 && !m.id.is_empty() && !m.by.is_empty()).then(|| clean(m))
}

fn hhmm(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default()
}

/// The whole comment: the marker and a line people can read.
pub fn render(m: &Marker, what: &str) -> String {
    // The JSON sits inside an HTML comment; nothing in it may be able to close that comment.
    let json = serde_json::to_string(m).unwrap_or_default().replace('<', "\\u003c").replace('>', "\\u003e");
    // Names go into Markdown other people read: letters, digits and a few marks only.
    let tame = |x: &str| x.chars().filter(|c| c.is_alphanumeric() || "-_.[] ".contains(*c)).collect::<String>();
    let who = format!("`{}`", tame(&m.by));
    let on = if tame(&m.host).trim().is_empty() { String::new() } else { format!(" on `{}`", tame(&m.host).trim()) };
    let pr = if m.pr.is_empty() { String::new() } else { format!(" · {}", m.pr) };
    let doing = match what {
        "pull request" => "reviewing this pull request",
        "deployment" => "deploying this commit",
        _ => "working on this issue",
    };
    let line = match m.state.as_str() {
        "active" => {
            let agent = if m.agent.is_empty() { String::new() } else { format!(" · agent: {}", m.agent) };
            format!(
                "**forkdeck** · {}{} is {} · stage: {}{}{} · started {} · last update {}",
                who,
                on,
                doing,
                if m.stage.is_empty() { "starting" } else { &m.stage },
                agent,
                pr,
                hhmm(m.started),
                hhmm(m.beat)
            )
        }
        "done" => format!("**forkdeck** · {}{} finished {}{} · {}", who, on, doing, pr, hhmm(m.beat)),
        "yielded" => format!("**forkdeck** · {}{} left this alone: someone else claimed it first.", who, on),
        "released" if !m.ends.is_empty() => format!("**forkdeck** · {} handed this back to the queue · {}", who, hhmm(m.beat)),
        other => format!("**forkdeck** · {}{} stopped {} ({}){} · {}", who, on, doing, other, pr, hhmm(m.beat)),
    };
    let foot = if m.state == "active" { "\n\n<sub>Updated in place while the job runs. `forkdeck team` lists everything in progress.</sub>" } else { "" };
    format!("{}{}{}\n{}{}", OPEN, json, CLOSE, line, foot)
}

/// Every marker in the comments that the forge vouches for, oldest comment first.
pub fn read_claims(comments: &[Comment]) -> Vec<Seen> {
    let mut out: Vec<Seen> = comments
        .iter()
        .filter_map(|c| {
            let m = parse(&c.body)?;
            // Anyone who can comment can type a marker. It only counts when its author is the person
            // it names, and the forge vouches that this account works on the repository: otherwise
            // a passer-by could hold issues, or end the claims of the people who do the work.
            if !c.trusted || !m.by.eq_ignore_ascii_case(&c.author) {
                return None;
            }
            let beat = if c.updated > 0 { c.updated } else { m.beat };
            Some(Seen { comment: c.id, m, beat })
        })
        .collect();
    out.sort_by_key(|s| s.comment);
    out
}

/// Ids of claims a later comment has ended.
pub fn ended(all: &[Seen]) -> HashSet<String> {
    let mut gone = HashSet::new();
    for s in all {
        for id in &s.m.ends {
            let Some(t) = all.iter().find(|o| &o.m.id == id) else { continue };
            if t.comment >= s.comment {
                continue;
            }
            // Every marker that got this far was written by an account with write access, which
            // could move the labels by hand anyway. A later one may end an earlier one.
            gone.insert(id.clone());
        }
    }
    gone
}

/// Claims that are being worked on right now, oldest first. The first one holds the item.
pub fn live(all: &[Seen], now: i64, stale_secs: i64) -> Vec<&Seen> {
    let gone = ended(all);
    all.iter().filter(|s| s.m.state == "active" && !gone.contains(&s.m.id) && now - s.beat <= stale_secs).collect()
}

/// Where a claim stands. Only `Owned` permits writing anything to the item.
#[derive(Debug, Clone, PartialEq)]
pub enum Standing {
    Owned,
    /// Somebody else holds the item, or ended this claim.
    Lost(String),
    /// Not refreshed within the limit. Nobody else holds the item, but anybody may take it.
    Expired,
    /// The claim comment is gone, or no longer says it is active.
    Missing,
    /// The forge could not be read.
    Unavailable(String),
}

impl Standing {
    pub fn words(&self) -> String {
        match self {
            Standing::Owned => "the claim is held".into(),
            Standing::Lost(who) => format!("{} has taken this over", who),
            Standing::Expired => "the claim went unrefreshed for too long and has expired".into(),
            Standing::Missing => "the claim comment is gone".into(),
            Standing::Unavailable(e) => format!("the claim could not be read ({})", e),
        }
    }
}

/// Does the claim in comment `mine` hold the item right now? The same rule decides a fresh claim
/// (`confirm`) and every later write, so two machines cannot both come out as the holder.
pub fn standing(all: &[Seen], mine: u64, now: i64, stale_secs: i64) -> Standing {
    let Some(me) = all.iter().find(|s| s.comment == mine) else { return Standing::Missing };
    if ended(all).contains(&me.m.id) {
        let who = all.iter().rev().find(|s| s.m.ends.contains(&me.m.id)).map(|s| s.m.by.clone()).unwrap_or_else(|| "someone".into());
        return Standing::Lost(who);
    }
    if me.m.state != "active" {
        return Standing::Missing;
    }
    match live(all, now, stale_secs).first() {
        Some(first) if first.comment == mine => Standing::Owned,
        Some(first) => Standing::Lost(first.m.by.clone()),
        None => Standing::Expired,
    }
}

pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{}s", s)
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d", s / 86_400)
    }
}

fn describe(s: &Seen, now: i64) -> String {
    let on = if s.m.host.is_empty() { String::new() } else { format!(" on {}", s.m.host) };
    format!("{}{} is already on it (last update {} ago)", s.m.by, on, ago(now - s.beat))
}

// ---------------------------------------------------------------------------------------------
// The forge

fn tool(r: &RepoCfg) -> &'static str {
    match r.provider {
        Provider::Github => "gh",
        Provider::Gitlab => "glab",
    }
}

fn gitlab_project(r: &RepoCfg) -> String {
    format!("projects/{}", r.repo.replace('/', "%2F"))
}

/// Where the comments of a target live. None when the forge has nothing forkdeck can edit there.
pub fn list_path(r: &RepoCfg, t: &Target) -> Option<String> {
    match (r.provider, t) {
        (Provider::Github, Target::Issue(n)) | (Provider::Github, Target::Pr(n)) => Some(format!("repos/{}/issues/{}/comments", r.repo, n)),
        (Provider::Github, Target::Commit(s)) => Some(format!("repos/{}/commits/{}/comments", r.repo, s)),
        (Provider::Gitlab, Target::Issue(n)) => Some(format!("{}/issues/{}/notes", gitlab_project(r), n)),
        (Provider::Gitlab, Target::Pr(n)) => Some(format!("{}/merge_requests/{}/notes", gitlab_project(r), n)),
        // GitLab's commit comments cannot be edited afterwards.
        (Provider::Gitlab, Target::Commit(_)) => None,
    }
}

fn edit_path(r: &RepoCfg, t: &Target, id: u64) -> Option<(&'static str, String)> {
    match (r.provider, t) {
        (Provider::Github, Target::Commit(_)) => Some(("PATCH", format!("repos/{}/comments/{}", r.repo, id))),
        (Provider::Github, _) => Some(("PATCH", format!("repos/{}/issues/comments/{}", r.repo, id))),
        (Provider::Gitlab, _) => list_path(r, t).map(|p| ("PUT", format!("{}/{}", p, id))),
    }
}

fn dir(r: &RepoCfg) -> Option<&std::path::Path> {
    // glab works out which GitLab host to talk to from the clone it runs in.
    (r.provider == Provider::Gitlab).then_some(r.path.as_path())
}

fn stamp(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map(|d| d.timestamp()).unwrap_or(0)
}

/// Comments as the API returns them. With paging the CLIs print one JSON array per page, back to back.
pub fn parse_comments(text: &str) -> Result<Vec<Comment>, String> {
    let mut out = Vec::new();
    for page in serde_json::Deserializer::from_str(text).into_iter::<Value>() {
        let page = page.map_err(|e| format!("the comment list is not JSON: {}", e))?;
        for c in page.as_array().ok_or("the comment list is not an array")? {
            let s = |p: &str| c.pointer(p).and_then(|x| x.as_str()).unwrap_or("");
            let Some(id) = c.get("id").and_then(|x| x.as_u64()) else { continue };
            let author = if s("/user/login").is_empty() { s("/author/username") } else { s("/user/login") };
            // GitHub says with every comment how its author relates to the repository.
            let trusted = matches!(s("/author_association"), "OWNER" | "MEMBER" | "COLLABORATOR");
            let author_id = c.pointer("/author/id").and_then(|x| x.as_u64()).unwrap_or(0);
            out.push(Comment { id, author: author.to_string(), body: s("/body").to_string(), updated: stamp(s("/updated_at")), trusted, author_id });
        }
    }
    Ok(out)
}

pub async fn comments(r: &RepoCfg, t: &Target) -> Result<Vec<Comment>, String> {
    let path = list_path(r, t).ok_or("this forge has no editable comments there")?;
    let path = format!("{}?per_page=100", path);
    let out = issues::sh(tool(r), &["api", "--paginate", &path], dir(r)).await?;
    let mut all = parse_comments(&out)?;
    if r.provider == Provider::Gitlab {
        // GitLab does not say it with the note, so ask about each account that wrote a marker.
        // Developer (30) and above can push and label. An answer that cannot be had is a no.
        let mut known: std::collections::HashMap<u64, bool> = Default::default();
        for c in all.iter_mut().filter(|c| c.author_id > 0 && parse(&c.body).is_some()) {
            let ok = match known.get(&c.author_id) {
                Some(k) => *k,
                None => {
                    let res = issues::sh("glab", &["api", &format!("{}/members/all/{}", gitlab_project(r), c.author_id)], dir(r)).await;
                    let level = res.ok().and_then(|o| serde_json::from_str::<Value>(&o).ok()).and_then(|v| v.get("access_level").and_then(|l| l.as_u64())).unwrap_or(0);
                    known.insert(c.author_id, level >= 30);
                    level >= 30
                }
            };
            c.trusted = ok;
        }
    }
    Ok(all)
}

/// Read the comments, trying a few times: what is decided from them should not hinge on one failed request.
async fn comments_surely(r: &RepoCfg, t: &Target) -> Result<Vec<Comment>, String> {
    let mut last = String::new();
    for wait in [0u64, 2, 5] {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        match comments(r, t).await {
            Ok(c) => return Ok(c),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn body_json(text: &str) -> String {
    serde_json::json!({ "body": text }).to_string()
}

async fn post(r: &RepoCfg, t: &Target, text: &str) -> Result<u64, String> {
    let path = list_path(r, t).ok_or("this forge has no editable comments there")?;
    let out = issues::sh_in(tool(r), &["api", "-X", "POST", &path, "--input", "-"], dir(r), &body_json(text)).await?;
    serde_json::from_str::<Value>(&out)
        .ok()
        .and_then(|v| v.get("id").and_then(|x| x.as_u64()))
        .ok_or_else(|| format!("the comment was posted but the answer names no id: {}", clip(&out, 120)))
}

async fn edit(r: &RepoCfg, t: &Target, id: u64, text: &str) -> Result<(), String> {
    let (method, path) = edit_path(r, t, id).ok_or("this forge has no editable comments there")?;
    issues::sh_in(tool(r), &["api", "-X", method, &path, "--input", "-"], dir(r), &body_json(text)).await.map(|_| ())
}

/// The account the forge CLI is logged in as.
pub async fn me(hub: &Hub, provider: Provider) -> Result<String, String> {
    let slot = (provider == Provider::Gitlab) as usize;
    if let Some(n) = hub.team_me.lock().unwrap()[slot].clone() {
        return Ok(n);
    }
    let name = match provider {
        Provider::Github => issues::sh("gh", &["api", "user", "--jq", ".login"], None).await?,
        Provider::Gitlab => {
            let first = hub.cfg.repos.iter().find(|r| r.provider == Provider::Gitlab).map(|r| r.path.clone());
            let out = issues::sh("glab", &["api", "user"], first.as_deref()).await?;
            serde_json::from_str::<Value>(&out).ok().and_then(|v| v.get("username").and_then(|x| x.as_str()).map(String::from)).unwrap_or_default()
        }
    };
    let name = plain(name.trim(), 60);
    if name.is_empty() {
        return Err("the forge CLI did not say who is logged in".into());
    }
    hub.team_me.lock().unwrap()[slot] = Some(name.clone());
    Ok(name)
}

pub fn host(hub: &Hub) -> String {
    if let Some(h) = &hub.cfg.team.host {
        return plain(h, 60);
    }
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is ours and its length is passed along.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    if ok {
        plain(&String::from_utf8_lossy(&buf[..end]), 60)
    } else {
        String::new()
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 delivers nothing; it only asks whether the process exists.
    pid > 1 && pid <= i32::MAX as u32 && unsafe { libc::kill(pid as i32, 0) == 0 || *libc::__errno_location() == libc::EPERM }
}

fn new_id() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}-{:x}", n, std::process::id())
}

// ---------------------------------------------------------------------------------------------
// Holding a claim

/// A claim this process holds.
pub struct Held {
    pub repo: RepoCfg,
    pub target: Target,
    pub what: &'static str,
    pub comment: u64,
    marker: std::sync::Mutex<Marker>,
    /// Serialises edits of the comment. True once the last one has been written.
    closed: tokio::sync::Mutex<bool>,
}

impl Held {
    /// Change the marker and write the comment, unless the final state is already written.
    async fn write(&self, last: bool, f: impl FnOnce(&mut Marker)) -> Result<(), String> {
        let mut closed = self.closed.lock().await;
        if *closed {
            return Ok(());
        }
        let text = {
            let mut m = self.marker.lock().unwrap();
            f(&mut m);
            m.beat = now();
            render(&m, self.what)
        };
        // The last word matters most: a comment left saying "active" blocks teammates until it
        // goes stale. Try it a few times, and count the claim as closed only once the forge has it.
        let waits: &[u64] = if last { &[0, 2, 6, 15] } else { &[0] };
        let mut res = Ok(());
        for w in waits {
            tokio::time::sleep(Duration::from_secs(*w)).await;
            res = edit(&self.repo, &self.target, self.comment, &text).await;
            if res.is_ok() {
                break;
            }
        }
        if last && res.is_ok() {
            *closed = true;
        }
        res
    }
}

pub enum Claimed {
    Ours(Arc<Held>),
    /// Somebody else holds it; the text says who.
    Taken(String),
}

pub fn on(hub: &Hub) -> bool {
    hub.cfg.team.enabled && !hub.cfg.demo
}

/// Say that this process is taking the item. Refuses when someone else is already on it.
/// The caller moves the label next and then calls `confirm`.
pub async fn claim(hub: &Hub, r: &RepoCfg, target: Target, what: &'static str) -> Result<Claimed, String> {
    let by = me(hub, r.provider).await.map_err(|e| format!("team: {}", e))?;
    let host = host(hub);
    let seen = read_claims(&comments_surely(r, &target).await.map_err(|e| format!("team: {}", e))?);
    let t = now();
    let stale = (hub.cfg.team.stale_mins * 60) as i64;
    // Claims that went quiet are ended by name, so that a machine waking up later finds its claim
    // gone instead of finding itself the oldest claimant again.
    let gone = ended(&seen);
    let mut ends: Vec<String> = seen.iter().filter(|s| s.m.state == "active" && !gone.contains(&s.m.id) && t - s.beat > stale).map(|s| s.m.id.clone()).collect();
    for s in live(&seen, t, stale) {
        // A claim this account left on this machine belongs to a run that has gone, unless that process is still there.
        // Without a machine name there is no telling this machine's claims from the account's other machines'.
        let mine = !host.is_empty() && s.m.by.eq_ignore_ascii_case(&by) && s.m.host == host && !(alive(s.m.pid) && s.m.pid != std::process::id());
        if !mine {
            return Ok(Claimed::Taken(describe(s, t)));
        }
        ends.push(s.m.id.clone());
    }
    let m = Marker { v: 1, id: new_id(), by, host, pid: std::process::id(), state: "active".into(), stage: "starting".into(), started: t, beat: t, ends, ..Marker::default() };
    let comment = post(r, &target, &render(&m, what)).await.map_err(|e| format!("team: {}", e))?;
    Ok(Claimed::Ours(Arc::new(Held { repo: r.clone(), target, what, comment, marker: std::sync::Mutex::new(m), closed: tokio::sync::Mutex::new(false) })))
}

/// After the label has moved: look again. If two machines claimed at the same moment, both see
/// both comments now, and the older comment wins on both. Returns who won when it was not us.
pub async fn confirm(hub: &Hub, held: &Arc<Held>) -> Result<Option<String>, String> {
    let seen = read_claims(&comments_surely(&held.repo, &held.target).await.map_err(|e| format!("team: {}", e))?);
    let t = now();
    let stale = (hub.cfg.team.stale_mins * 60) as i64;
    match standing(&seen, held.comment, t, stale) {
        Standing::Owned => Ok(None),
        Standing::Lost(who) => {
            let why = live(&seen, t, stale).first().map(|f| describe(f, t)).unwrap_or_else(|| format!("{} is already on it", who));
            let _ = held.write(true, |m| m.state = "yielded".into()).await;
            Ok(Some(why))
        }
        other => Err(format!("team: {}", other.words())),
    }
}

/// Close a claim that never became a running job.
pub async fn abandon(held: &Arc<Held>, state: &str) {
    let _ = held.write(true, |m| m.state = state.into()).await;
}

/// Put the account on the issue as assignee. Failing to is not a reason to stop.
pub async fn assign_if_on(hub: &Hub, r: &RepoCfg, n: u64) {
    if !on(hub) || !hub.cfg.team.assign {
        return;
    }
    let num = n.to_string();
    let res = match r.provider {
        Provider::Github => issues::sh("gh", &["issue", "edit", &num, "-R", &r.repo, "--add-assignee", "@me"], None).await,
        Provider::Gitlab => match me(hub, r.provider).await {
            Ok(u) => issues::sh("glab", &["issue", "update", &num, "-R", &r.repo, "--assignee", &format!("+{}", u)], dir(r)).await,
            Err(e) => Err(e),
        },
    };
    if let Err(e) = res {
        hub.log("team", format!("{}#{} could not be assigned: {}", r.repo, n, e));
    }
}

fn job_view(hub: &Hub, id: u32) -> Option<(JobStatus, String, String, bool)> {
    hub.job(id, |j| (j.status, j.worker.agent.map(|a| a.name().to_string()).unwrap_or_default(), j.pr_url.clone().unwrap_or_default(), j.claim.is_some()))
}

fn stage_name(s: JobStatus) -> &'static str {
    if s == JobStatus::Queued {
        "starting"
    } else {
        s.name()
    }
}

/// Tie the claim to its job and keep the comment current until the job ends.
pub fn attach(hub: &Hub, id: u32, held: Arc<Held>) {
    hub.job(id, |j| j.claim = Some(held.clone()));
    let hub = hub.clone();
    hub.rt.clone().spawn(async move {
        let beat = Duration::from_secs(hub.cfg.team.heartbeat_secs.max(30));
        let mut written = Instant::now();
        let mut shown = ("starting".to_string(), String::new(), String::new());
        let mut complained = false;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let Some((status, agent, pr, has)) = job_view(&hub, id) else { return };
            if !has || status.finished() {
                return;
            }
            let cur = (stage_name(status).to_string(), agent, pr);
            let due = written.elapsed() >= beat;
            if cur == shown && !due {
                continue;
            }
            // Never refresh a claim that is no longer the winning one: that would take the item
            // back from whoever has it now. Stop quietly instead; it is theirs.
            match check(&hub, &held).await {
                Standing::Owned => {}
                Standing::Unavailable(e) => {
                    if !complained {
                        complained = true;
                        hub.log("team", format!("#{} claim not refreshed: {}", id, e));
                    }
                    continue;
                }
                gone => {
                    hub.job(id, |j| j.released = true);
                    hub.log("team", format!("#{} stopped: {}", id, gone.words()));
                    hub.cancel(id);
                    return;
                }
            }
            let c = cur.clone();
            match held.write(false, move |m| (m.stage, m.agent, m.pr) = c).await {
                Ok(()) => complained = false,
                Err(e) if !complained => {
                    complained = true;
                    hub.log("team", format!("#{} status comment not updated: {}", id, e));
                }
                Err(_) => {}
            }
            shown = cur;
            written = Instant::now();
        }
    });
}

/// Read the forge and say where a held claim stands.
async fn check(hub: &Hub, held: &Arc<Held>) -> Standing {
    match comments_surely(&held.repo, &held.target).await {
        Ok(c) => standing(&read_claims(&c), held.comment, now(), (hub.cfg.team.stale_mins * 60) as i64),
        Err(e) => Standing::Unavailable(e),
    }
}

/// Before anything is written to the item (a push, a comment, a label): is it still ours?
/// A job without a claim (sharing off, typed tasks) always may. Anything short of holding the
/// winning claim right now is a no, including not being able to read the forge.
pub async fn guard(hub: &Hub, id: u32) -> Result<(), String> {
    let Some(held) = hub.job(id, |j| j.claim.clone()).flatten() else { return Ok(()) };
    if held.what == "deployment" {
        // Announced, not claimed: the deploy stage's own state decides who deploys.
        return Ok(());
    }
    match check(hub, &held).await {
        Standing::Owned => Ok(()),
        Standing::Unavailable(e) => Err(format!("team: could not confirm the claim is still ours, so nothing was written: {}", e)),
        gone => {
            hub.job(id, |j| j.released = true);
            Err(format!("{}; nothing more was written to it", gone.words()))
        }
    }
}

/// As `guard`, for the paths that only report: say why in the log and tell the caller to stay quiet.
pub async fn may_report(hub: &Hub, id: u32) -> bool {
    match guard(hub, id).await {
        Ok(()) => true,
        Err(e) => {
            hub.log("team", format!("#{} not reported on the forge: {}", id, e));
            false
        }
    }
}

/// A deployment is announced, not claimed: who deploys is decided by the deploy stage's own state.
pub async fn announce(hub: &Hub, id: u32) {
    if !on(hub) || !hub.cfg.team.deploys {
        return;
    }
    let found = hub
        .job(id, |j| match &j.source {
            Source::Deploy { repo_idx, sha, .. } => hub.cfg.repos.get(*repo_idx).cloned().map(|r| (r, sha.clone())),
            _ => None,
        })
        .flatten();
    let Some((r, sha)) = found else { return };
    let target = Target::Commit(sha.clone());
    if list_path(&r, &target).is_none() {
        return;
    }
    let made = async {
        let by = me(hub, r.provider).await?;
        let t = now();
        let seen = read_claims(&comments(&r, &target).await?);
        if let Some(other) = live(&seen, t, (hub.cfg.team.stale_mins * 60) as i64).into_iter().find(|s| !s.m.by.eq_ignore_ascii_case(&by)) {
            hub.log("team", format!("{} {}: {}", r.repo, clip(&sha, 12), describe(other, t)));
        }
        let m = Marker { v: 1, id: new_id(), by, host: host(hub), pid: std::process::id(), state: "active".into(), stage: "deploying".into(), started: t, beat: t, ..Marker::default() };
        let comment = post(&r, &target, &render(&m, "deployment")).await?;
        Ok::<_, String>(Arc::new(Held { repo: r.clone(), target: target.clone(), what: "deployment", comment, marker: std::sync::Mutex::new(m), closed: tokio::sync::Mutex::new(false) }))
    };
    match made.await {
        Ok(h) => attach(hub, id, h),
        Err(e) => hub.log("team", format!("{} deployment not announced: {}", r.repo, e)),
    }
}

/// The job is over: write how it ended. After this the comment is not touched again.
pub async fn finish(hub: &Hub, id: u32, status: JobStatus) {
    let Some((held, released, pr)) = hub.job(id, |j| j.claim.take().map(|h| (h, j.released, j.pr_url.clone().unwrap_or_default()))).flatten() else { return };
    let state = match status {
        _ if released => "released",
        JobStatus::Done => "done",
        JobStatus::Cancelled => "cancelled",
        _ => "failed",
    };
    if let Err(e) = held.write(true, |m| {
        m.state = state.into();
        m.stage = String::new();
        if !pr.is_empty() {
            m.pr = pr;
        }
    })
    .await
    {
        hub.log("team", format!("#{} final status not written: {}", id, e));
    }
}

// ---------------------------------------------------------------------------------------------
// Looking at what everybody is doing

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Row {
    pub repo: String,
    /// issue, pr or deploy
    pub kind: &'static str,
    /// `#7`, `!5` or a short commit
    pub item: String,
    pub title: String,
    pub url: String,
    pub who: String,
    pub host: String,
    pub agent: String,
    pub stage: String,
    pub pr: String,
    /// Seconds since the claim started and since it was last updated; None when there is no claim.
    pub running_secs: Option<i64>,
    pub idle_secs: Option<i64>,
    pub stale: bool,
    pub mine: bool,
}

struct Listed {
    number: u64,
    title: String,
    url: String,
    assignees: Vec<String>,
}

fn parse_listed(provider: Provider, json: &str) -> Result<Vec<Listed>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("the list is not JSON: {}", e))?;
    let s = |i: &Value, p: &str| i.pointer(p).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (num, url, name) = match provider {
        Provider::Github => ("number", "/url", "/login"),
        Provider::Gitlab => ("iid", "/web_url", "/username"),
    };
    Ok(v.as_array()
        .ok_or("the list is not an array")?
        .iter()
        .filter_map(|i| {
            Some(Listed {
                number: i.get(num)?.as_u64()?,
                title: plain(&s(i, "/title"), 200),
                url: s(i, url),
                assignees: i.get("assignees").and_then(|a| a.as_array()).map(|a| a.iter().map(|u| plain(&s(u, name), 60)).filter(|n| !n.is_empty()).collect()).unwrap_or_default(),
            })
        })
        .collect())
}

async fn listed(r: &RepoCfg, pr: bool, label: &str) -> Result<Vec<Listed>, String> {
    let out = match (r.provider, pr) {
        (Provider::Github, false) => issues::sh("gh", &["issue", "list", "-R", &r.repo, "--label", label, "--state", "open", "--limit", "30", "--json", "number,title,url,assignees"], None).await?,
        (Provider::Github, true) => issues::sh("gh", &["pr", "list", "-R", &r.repo, "--label", label, "--state", "open", "--limit", "30", "--json", "number,title,url,assignees"], None).await?,
        (Provider::Gitlab, false) => issues::sh("glab", &["issue", "list", "-R", &r.repo, "--label", label, "--per-page", "30", "--output", "json"], dir(r)).await?,
        (Provider::Gitlab, true) => issues::sh("glab", &["mr", "list", "-R", &r.repo, "--label", label, "--per-page", "30", "--output", "json"], dir(r)).await?,
    };
    parse_listed(r.provider, &out)
}

/// One line of the overview, from an item and the markers on it.
#[allow(clippy::too_many_arguments)]
pub fn row(repo: &str, kind: &'static str, item: String, title: &str, url: &str, assignees: &[String], seen: &[Seen], me: &str, now: i64, stale_secs: i64, fallback: &str) -> Row {
    let gone = ended(seen);
    // Whoever holds it now; failing that, the last thing anybody's forkdeck said about it.
    let pick = live(seen, now, stale_secs)
        .into_iter()
        .next()
        .or_else(|| seen.iter().rev().find(|s| s.m.state == "active" && !gone.contains(&s.m.id)))
        .or_else(|| seen.iter().rev().find(|s| s.m.ends.is_empty()));
    let mut r = Row {
        repo: repo.to_string(),
        kind,
        item,
        title: title.to_string(),
        url: url.to_string(),
        who: String::new(),
        host: String::new(),
        agent: String::new(),
        stage: fallback.to_string(),
        pr: String::new(),
        running_secs: None,
        idle_secs: None,
        stale: false,
        mine: false,
    };
    match pick {
        Some(s) => {
            let active = s.m.state == "active";
            r.who = s.m.by.clone();
            r.host = s.m.host.clone();
            r.agent = s.m.agent.clone();
            r.pr = s.m.pr.clone();
            r.stage = match (active, s.m.stage.is_empty()) {
                (true, false) => s.m.stage.clone(),
                (true, true) => "starting".into(),
                (false, _) if kind == "issue" && s.m.state == "done" => "pull request open".into(),
                (false, _) => s.m.state.clone(),
            };
            r.running_secs = Some(now - s.m.started);
            r.idle_secs = Some(now - s.beat);
            r.stale = active && now - s.beat > stale_secs;
            r.mine = s.m.by.eq_ignore_ascii_case(me);
        }
        None => {
            r.who = assignees.join(", ");
            r.mine = assignees.iter().any(|a| a.eq_ignore_ascii_case(me)) && assignees.len() == 1;
        }
    }
    r
}

/// Everything in progress in the configured repositories, and what could not be read.
pub async fn gather(hub: &Hub, all: bool) -> (Vec<Row>, Vec<String>) {
    if hub.cfg.demo {
        return (demo_rows(), Vec::new());
    }
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    let stale = (hub.cfg.team.stale_mins * 60) as i64;
    let mut done_repos: HashSet<(bool, &str)> = HashSet::new();
    for r in &hub.cfg.repos {
        // Two entries for the same repository (two environments) are one repository here.
        if !done_repos.insert((r.provider == Provider::Github, r.repo.as_str())) {
            continue;
        }
        let who = me(hub, r.provider).await.unwrap_or_default();
        let mut wanted: Vec<(bool, &str, &str)> = vec![(false, r.working_label.as_str(), "claimed, no forkdeck status"), (true, r.pr_working_label.as_str(), "in review, no forkdeck status")];
        // An issue whose pull request is open is still being worked on while its CI is watched.
        if all || r.ci.is_some() {
            wanted.push((false, r.done_label.as_str(), "pull request open"));
        }
        for (pr, label, fallback) in wanted {
            let items = match listed(r, pr, label).await {
                Ok(i) => i,
                Err(e) => {
                    errors.push(format!("{}: {}", r.repo, e));
                    continue;
                }
            };
            for i in items {
                let target = if pr { Target::Pr(i.number) } else { Target::Issue(i.number) };
                let seen = match comments(r, &target).await {
                    Ok(c) => read_claims(&c),
                    Err(e) => {
                        errors.push(format!("{}: {}", r.repo, e));
                        Vec::new()
                    }
                };
                if !all && !pr && label == r.done_label && live(&seen, now(), stale).is_empty() {
                    continue;
                }
                let item = format!("{}{}", if pr { "!" } else { "#" }, i.number);
                rows.push(row(&r.repo, if pr { "pr" } else { "issue" }, item, &i.title, &i.url, &i.assignees, &seen, &who, now(), stale, fallback));
            }
        }
        if r.deploy.is_some() && r.provider == Provider::Github {
            match deploys(r, &who, stale).await {
                Ok(mut d) => rows.append(&mut d),
                Err(e) => errors.push(format!("{}: {}", r.repo, e)),
            }
        }
    }
    (rows, errors)
}

/// Deployments announced on the latest commits of the base branch.
async fn deploys(r: &RepoCfg, me: &str, stale: i64) -> Result<Vec<Row>, String> {
    let out = issues::sh("gh", &["api", &format!("repos/{}/commits?sha={}&per_page=10", r.repo, r.base)], None).await?;
    let v: Value = serde_json::from_str(&out).map_err(|e| format!("the commit list is not JSON: {}", e))?;
    let mut rows = Vec::new();
    for c in v.as_array().map(|a| a.as_slice()).unwrap_or_default() {
        if c.pointer("/commit/comment_count").and_then(|n| n.as_u64()).unwrap_or(0) == 0 {
            continue;
        }
        let Some(sha) = c.get("sha").and_then(|s| s.as_str()) else { continue };
        let seen = read_claims(&comments(r, &Target::Commit(sha.to_string())).await?);
        let t = now();
        // Only deployments under way: a finished one is in the deployment system's own history.
        if live(&seen, t, i64::MAX / 4).is_empty() {
            continue;
        }
        let title = c.pointer("/commit/message").and_then(|m| m.as_str()).unwrap_or("").lines().next().unwrap_or("").to_string();
        let url = c.get("html_url").and_then(|u| u.as_str()).unwrap_or("");
        rows.push(row(&r.repo, "deploy", sha.chars().take(7).collect(), &plain(&title, 200), url, &[], &seen, me, t, stale, "deploying"));
    }
    Ok(rows)
}

fn demo_rows() -> Vec<Row> {
    let mk = |item: &str, kind: &'static str, title: &str, who: &str, host: &str, agent: &str, stage: &str, run: i64, idle: i64, stale: bool, mine: bool| Row {
        repo: "acme/payments".into(),
        kind,
        item: item.into(),
        title: title.into(),
        url: String::new(),
        who: who.into(),
        host: host.into(),
        agent: agent.into(),
        stage: stage.into(),
        pr: String::new(),
        running_secs: Some(run),
        idle_secs: Some(idle),
        stale,
        mine,
    };
    vec![
        mk("#41", "issue", "Retry loop keeps the long backoff after a success", "you", "this machine", "claude", "working", 240, 20, false, true),
        mk("#38", "issue", "Webhook signature check rejects rotated keys", "maria", "mbp-maria", "codex", "watching ci", 1900, 140, false, false),
        mk("!57", "pr", "Add --since flag to the export command", "maria", "mbp-maria", "claude", "reviewing", 300, 60, false, false),
        mk("#35", "issue", "Paginate the ledger export", "tomasz", "build-02", "claude", "working", 9200, 4100, true, false),
        mk("9f3c2ab", "deploy", "Merge pull request #52", "tomasz", "build-02", "", "deploying", 180, 15, false, false),
    ]
}

/// The overview as text, one block per person, the reader's own work first.
pub fn text(rows: &[Row], width: usize) -> Vec<String> {
    let mut out = Vec::new();
    if rows.is_empty() {
        out.push("nothing in progress".to_string());
        return out;
    }
    let mut people: Vec<(bool, String)> = Vec::new();
    for r in rows {
        let key = (r.mine, group(r));
        if !people.contains(&key) {
            people.push(key);
        }
    }
    // Own work first, then people by name, and what nobody's forkdeck has spoken for last.
    people.sort_by_key(|p| (!p.0, p.1.is_empty(), p.1.to_lowercase()));
    for (mine, name) in people {
        let head = match (mine, name.is_empty()) {
            (_, true) => "nobody assigned".to_string(),
            (true, _) => format!("{} (you)", name),
            (false, _) => name.clone(),
        };
        out.push(head);
        for r in rows.iter().filter(|r| r.mine == mine && group(r) == name) {
            let time = r.running_secs.map(ago).unwrap_or_default();
            let left = format!("  {:<18} {:<18} {:<7} {:>7}  ", clip(&format!("{}{}", short_repo(&r.repo), sep(r)), 18), clip(&r.stage, 18), clip(&r.agent, 7), time);
            let room = width.saturating_sub(left.chars().count()).max(10);
            out.push(format!("{}{}", left, clip(&r.title, room)));
            if r.stale {
                let hint = if r.kind == "deploy" { String::new() } else { format!(" · forkdeck team release {}{}", r.repo, r.item) };
                out.push(format!("  {:<18} no update for {}{}", "", r.idle_secs.map(ago).unwrap_or_default(), hint));
            }
            if !r.pr.is_empty() {
                out.push(format!("  {:<18} {}", "", clip(&r.pr, width.saturating_sub(21).max(20))));
            }
        }
    }
    out
}

fn sep(r: &Row) -> String {
    if r.kind == "deploy" {
        format!("@{}", r.item)
    } else {
        r.item.clone()
    }
}

fn short_repo(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

fn group(r: &Row) -> String {
    if r.who.is_empty() || r.host.is_empty() {
        r.who.clone()
    } else {
        format!("{} on {}", r.who, r.host)
    }
}

/// Keep the overview in the interface fresh.
pub async fn watch_loop(hub: Hub) {
    if !hub.cfg.demo && (!hub.cfg.team.enabled || hub.cfg.repos.is_empty()) {
        return;
    }
    loop {
        let (rows, errors) = gather(&hub, false).await;
        {
            let mut st = hub.state.lock().unwrap();
            st.team.rows = rows;
            st.team.at = Some(Instant::now());
            st.team.error = errors.into_iter().next();
        }
        let wait = if hub.cfg.demo { 30 } else { hub.cfg.team.refresh_secs.max(30) };
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}

/// `owner/repo#12` or `owner/repo!5`.
pub fn parse_ref(s: &str) -> Option<(String, bool, u64)> {
    let at = s.rfind(['#', '!'])?;
    let n: u64 = s[at + 1..].parse().ok()?;
    let repo = &s[..at];
    (!repo.is_empty()).then(|| (repo.to_string(), s.as_bytes()[at] == b'!', n))
}

/// Hand a claimed item back to the queue so that someone (or this machine) can pick it up.
/// A claim that is still being updated is only ended with `force`.
async fn labels(r: &RepoCfg, pr: bool, n: u64) -> Result<Vec<String>, String> {
    let num = n.to_string();
    let out = match (r.provider, pr) {
        (Provider::Github, false) => issues::sh("gh", &["issue", "view", &num, "-R", &r.repo, "--json", "labels"], None).await?,
        (Provider::Github, true) => issues::sh("gh", &["pr", "view", &num, "-R", &r.repo, "--json", "labels"], None).await?,
        (Provider::Gitlab, false) => issues::sh("glab", &["issue", "view", &num, "-R", &r.repo, "--output", "json"], dir(r)).await?,
        (Provider::Gitlab, true) => issues::sh("glab", &["mr", "view", &num, "-R", &r.repo, "--output", "json"], dir(r)).await?,
    };
    parse_labels(&out)
}

/// Label names from `gh ... --json labels` (objects with a name) or `glab ... --output json` (plain strings).
pub fn parse_labels(json: &str) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("the item is not JSON: {}", e))?;
    let arr = v.get("labels").and_then(|l| l.as_array()).ok_or("the item lists no labels")?;
    Ok(arr.iter().filter_map(|l| l.as_str().or_else(|| l.get("name").and_then(|n| n.as_str())).map(String::from)).collect())
}

pub async fn release(hub: &Hub, what: &str, force: bool, unclaimed: bool) -> Result<String, String> {
    let (repo, pr, n) = parse_ref(what).ok_or("name the item as owner/repo#12 (an issue) or owner/repo!5 (a pull request)")?;
    let r = hub.cfg.repos.iter().find(|r| r.repo.eq_ignore_ascii_case(&repo)).ok_or_else(|| format!("{} is not one of the configured [[repos]]", repo))?;
    let target = if pr { Target::Pr(n) } else { Target::Issue(n) };
    let by = me(hub, r.provider).await?;
    let all = comments(r, &target).await?;
    // Our own word has to count with the others, or the release would be ignored by everyone.
    if let Some(c) = all.iter().rev().find(|c| c.author.eq_ignore_ascii_case(&by)) {
        if !c.trusted {
            return Err(format!("{} does not have write access to {}, so teammates' forkdecks would ignore the release", by, r.repo));
        }
    }
    let seen = read_claims(&all);
    let t = now();
    let stale = (hub.cfg.team.stale_mins * 60) as i64;
    let gone = ended(&seen);
    let open: Vec<&Seen> = seen.iter().filter(|s| s.m.state == "active" && !gone.contains(&s.m.id)).collect();
    if let Some(s) = live(&seen, t, stale).into_iter().find(|s| !s.m.by.eq_ignore_ascii_case(&by)) {
        if !force {
            return Err(format!("{}. That is recent, so it was left alone; --force ends it anyway and stops their job", describe(s, t)));
        }
    }
    // Only an item that is marked as being worked on can be handed back. Without this, releasing
    // something already finished would put its request label back and start the work again.
    let (from, to) = if pr { (&r.pr_working_label, &r.pr_label) } else { (&r.working_label, &r.label) };
    let labels = labels(r, pr, n).await?;
    if !labels.iter().any(|l| l == from) {
        return Err(format!("{}{}{} does not carry the label {:?}, so there is nothing to hand back", r.repo, if pr { "!" } else { "#" }, n, from));
    }
    if open.is_empty() && !unclaimed {
        return Err(format!("no forkdeck claim is open on it. If it was claimed by hand or by a forkdeck with sharing off, --unclaimed moves the label back to {:?} anyway", to));
    }
    if let Some(s) = open.iter().find(|s| !pr && !s.m.pr.is_empty()) {
        return Err(format!("{}'s forkdeck has already opened {} for this issue. Close or take over that pull request; there is nothing to hand back", s.m.by, s.m.pr));
    }
    let said = if open.is_empty() {
        "no forkdeck claim was open on it".to_string()
    } else {
        let m = Marker { v: 1, id: new_id(), by: by.clone(), host: host(hub), state: "released".into(), started: t, beat: t, ends: open.iter().map(|s| s.m.id.clone()).collect(), ..Marker::default() };
        post(r, &target, &render(&m, "")).await?;
        let names: Vec<String> = open.iter().map(|s| s.m.by.clone()).collect();
        format!("ended the claim of {}", names.join(", "))
    };
    issues::relabel(r, pr, n, from, to).await?;
    Ok(format!("{}{}{}: {}, label back to {}", r.repo, if pr { "!" } else { "#" }, n, said, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(id: &str, by: &str, state: &str) -> Marker {
        Marker { v: 1, id: id.into(), by: by.into(), host: "box".into(), pid: 1, state: state.into(), stage: "working".into(), started: 1000, beat: 1000, ..Marker::default() }
    }

    fn c(id: u64, author: &str, m: &Marker, updated: i64) -> Comment {
        Comment { id, author: author.into(), body: render(m, "issue"), updated, trusted: true, author_id: 0 }
    }

    #[test]
    fn a_marker_survives_the_round_trip_and_cannot_close_its_own_comment() {
        let mut m = marker("a1", "alice", "active");
        m.host = "evil --> <b>x</b>".into();
        m.pr = "https://example.invalid/o/r/pull/3".into();
        let body = render(&m, "issue");
        assert_eq!(body.matches("-->").count(), 1, "{}", body);
        assert!(body.starts_with(OPEN));
        let back = parse(&body).unwrap();
        assert_eq!(back, m);
        assert!(body.contains("`alice`") && !body.contains("@alice"), "nobody is notified: {}", body);
    }

    #[test]
    fn only_a_comment_that_starts_with_a_marker_by_its_own_author_counts() {
        let m = marker("a1", "alice", "active");
        let quoted = Comment { id: 1, author: "bob".into(), body: format!("> {}", render(&m, "issue")), updated: 1000, trusted: true, author_id: 0 };
        let forged = c(2, "mallory", &m, 1000);
        let real = c(3, "Alice", &m, 1000);
        // Her own name on it, but the forge does not say she works on the repository.
        let mut passer_by = c(4, "eve", &marker("e1", "eve", "active"), 1000);
        passer_by.trusted = false;
        let seen = read_claims(&[quoted, forged, real, passer_by]);
        assert_eq!(seen.iter().map(|s| s.comment).collect::<Vec<_>>(), vec![3]);
        assert!(parse("<!-- forkdeck:claim {\"v\":2,\"id\":\"x\",\"by\":\"a\"} -->").is_none(), "an unknown version is not guessed at");
        assert!(parse("<!-- forkdeck:claim not json -->").is_none());
    }

    #[test]
    fn what_a_marker_says_is_cleaned_before_it_is_shown() {
        let body = "<!-- forkdeck:claim {\"v\":1,\"id\":\"x\",\"by\":\"al\\u001b[31mice\",\"state\":\"active\",\"pr\":\"javascript:alert(1)\",\"stage\":\"a\\nb\"} -->";
        let m = parse(body).unwrap();
        assert_eq!(m.by, "al[31mice");
        assert_eq!(m.pr, "");
        assert_eq!(m.stage, "ab");
    }

    #[test]
    fn the_oldest_fresh_claim_holds_the_item_and_age_is_the_forges_clock() {
        let a = marker("a", "alice", "active");
        let mut b = marker("b", "bob", "active");
        // Bob's marker claims a time far in the future; what counts is when the forge saw the comment change.
        b.beat = 9_999_999;
        let seen = read_claims(&[c(20, "bob", &b, 1100), c(10, "alice", &a, 1000)]);
        let now = 1200;
        assert_eq!(live(&seen, now, 600).iter().map(|s| s.m.by.as_str()).collect::<Vec<_>>(), vec!["alice", "bob"]);
        // Alice goes quiet: after the limit only Bob is left.
        assert_eq!(live(&seen, 1650, 600).iter().map(|s| s.m.by.as_str()).collect::<Vec<_>>(), vec!["bob"]);
        assert!(live(&seen, 5000, 600).is_empty());
        // A finished claim holds nothing.
        let done = read_claims(&[c(10, "alice", &marker("a", "alice", "done"), 1000)]);
        assert!(live(&done, 1001, 600).is_empty());
    }

    #[test]
    fn a_later_marker_from_someone_with_write_access_ends_a_claim_and_nobody_elses_does() {
        let a = marker("a", "alice", "active");
        let mut rel = marker("r", "bob", "released");
        rel.ends = vec!["a".into()];
        let seen = read_claims(&[c(1, "alice", &a, 1000), c(2, "bob", &rel, 1000)]);
        assert!(ended(&seen).contains("a"));
        assert!(live(&seen, 1001, 600).is_empty());
        assert_eq!(standing(&seen, 1, 1001, 600), Standing::Lost("bob".into()));

        // The same comment from an account without write access is not a marker at all.
        let mut outside = c(2, "bob", &rel, 1000);
        outside.trusted = false;
        let seen = read_claims(&[c(1, "alice", &a, 1000), outside]);
        assert!(ended(&seen).is_empty());
        assert_eq!(standing(&seen, 1, 1001, 600), Standing::Owned);

        // A release written before the claim it names does nothing to it.
        let seen = read_claims(&[c(5, "alice", &a, 1000), c(2, "bob", &rel, 1000)]);
        assert!(ended(&seen).is_empty());
    }

    #[test]
    fn only_the_winning_claim_within_its_lease_is_owned() {
        let a = marker("a", "alice", "active");
        let b = marker("b", "bob", "active");
        let both = read_claims(&[c(1, "alice", &a, 1000), c(2, "bob", &b, 1000)]);
        assert_eq!(standing(&both, 1, 1100, 600), Standing::Owned);
        assert_eq!(standing(&both, 2, 1100, 600), Standing::Lost("alice".into()));
        assert_eq!(standing(&both, 3, 1100, 600), Standing::Missing);

        // Alice's machine slept past the limit. Nobody took over: expired, not owned.
        let alone = read_claims(&[c(1, "alice", &a, 1000)]);
        assert_eq!(standing(&alone, 1, 5000, 600), Standing::Expired);
        // Bob took over meanwhile without naming her claim: his is the only live one.
        let taken = read_claims(&[c(1, "alice", &a, 1000), c(2, "bob", &b, 4900)]);
        assert_eq!(standing(&taken, 1, 5000, 600), Standing::Lost("bob".into()));
        // Bob's claim names hers, as forkdeck's does. Even if her comment is refreshed afterwards
        // she does not become the oldest claimant again.
        let mut over = marker("b", "bob", "active");
        over.ends = vec!["a".into()];
        let woke = read_claims(&[c(1, "alice", &a, 4990), c(2, "bob", &over, 4900)]);
        assert_eq!(standing(&woke, 1, 5000, 600), Standing::Lost("bob".into()));
        assert_eq!(standing(&woke, 2, 5000, 600), Standing::Owned);
        // A claim that says it is finished owns nothing.
        let done = read_claims(&[c(1, "alice", &marker("a", "alice", "done"), 1000)]);
        assert_eq!(standing(&done, 1, 1001, 600), Standing::Missing);
    }

    #[test]
    fn reads_labels_from_both_clis() {
        assert_eq!(parse_labels(r#"{"labels":[{"name":"agent:working"},{"name":"bug"}]}"#).unwrap(), vec!["agent:working", "bug"]);
        assert_eq!(parse_labels(r#"{"iid":3,"labels":["agent:working"]}"#).unwrap(), vec!["agent:working"]);
        assert!(parse_labels("{}").is_err() && parse_labels("nope").is_err());
    }

    #[test]
    fn reads_paged_comment_lists_from_both_forges() {
        let gh = r#"[{"id":1,"body":"hi","user":{"login":"alice"},"author_association":"MEMBER","updated_at":"2026-01-01T00:00:10Z"}][{"id":2,"body":"x","user":{"login":"bob"},"author_association":"CONTRIBUTOR","updated_at":"bad"}]"#;
        let v = parse_comments(gh).unwrap();
        assert_eq!((v.len(), v[0].author.as_str(), v[0].updated, v[1].updated), (2, "alice", 1_767_225_610, 0));
        assert!(v[0].trusted && !v[1].trusted, "having contributed before is not write access");
        let gl = r#"[{"id":7,"body":"n","author":{"username":"carol"},"updated_at":"2026-01-01T00:00:10.000+00:00"}]"#;
        let gl = gl.replace(r#"{"username":"carol"}"#, r#"{"username":"carol","id":42}"#);
        let v = parse_comments(&gl).unwrap();
        assert_eq!((v[0].author.as_str(), v[0].author_id, v[0].trusted), ("carol", 42, false), "GitLab notes are vouched for by a membership lookup");
        assert!(parse_comments("<html>").is_err());
        assert!(parse_comments("").unwrap().is_empty());
    }

    #[test]
    fn rows_show_the_holder_and_flag_what_went_quiet() {
        let mut a = marker("a", "alice", "active");
        a.agent = "claude".into();
        let seen = read_claims(&[c(1, "alice", &a, 1000)]);
        let r = row("o/r", "issue", "#7".into(), "T", "u", &[], &seen, "ALICE", 1300, 600, "x");
        assert_eq!((r.who.as_str(), r.stage.as_str(), r.stale, r.mine, r.running_secs, r.idle_secs), ("alice", "working", false, true, Some(300), Some(300)));
        let r = row("o/r", "issue", "#7".into(), "T", "u", &[], &seen, "bob", 5000, 600, "x");
        assert!(r.stale && !r.mine);
        // No marker: fall back to the assignee and say that nothing is known.
        let r = row("o/r", "issue", "#7".into(), "T", "u", &["carol".into()], &[], "bob", 5000, 600, "claimed, no forkdeck status");
        assert_eq!((r.who.as_str(), r.stage.as_str(), r.running_secs), ("carol", "claimed, no forkdeck status", None));
        let lines = text(&[r], 100).join("\n");
        assert!(lines.starts_with("carol\n") && lines.contains("r#7"), "{}", lines);
    }

    #[test]
    fn the_overview_groups_by_person_with_own_work_first() {
        let rows = demo_rows();
        let t = text(&rows, 110);
        assert_eq!(t[0], "you on this machine (you)");
        let heads: Vec<&String> = t.iter().filter(|l| !l.starts_with(' ')).collect();
        assert_eq!(heads.len(), 3, "{:?}", heads);
        let joined = t.join("\n");
        assert!(joined.contains("no update for 1h 8m · forkdeck team release acme/payments#35"), "{}", joined);
        assert!(joined.contains("payments@9f3c2ab"), "{}", joined);
        assert_eq!(text(&[], 80), vec!["nothing in progress"]);
    }

    #[test]
    fn parses_item_references() {
        assert_eq!(parse_ref("o/r#12"), Some(("o/r".into(), false, 12)));
        assert_eq!(parse_ref("group/sub/p!5"), Some(("group/sub/p".into(), true, 5)));
        assert_eq!(parse_ref("o/r"), None);
        assert_eq!(parse_ref("#3"), None);
        assert_eq!(parse_ref("o/r#x"), None);
    }

    #[test]
    fn knows_where_comments_live_on_each_forge() {
        let mut r: RepoCfg = toml::from_str("provider = \"github\"\nrepo = \"o/r\"\npath = \".\"").unwrap();
        assert_eq!(list_path(&r, &Target::Pr(5)).unwrap(), "repos/o/r/issues/5/comments");
        assert_eq!(edit_path(&r, &Target::Commit("abc".into()), 9).unwrap(), ("PATCH", "repos/o/r/comments/9".to_string()));
        r.provider = Provider::Gitlab;
        r.repo = "g/sub/p".into();
        assert_eq!(list_path(&r, &Target::Pr(5)).unwrap(), "projects/g%2Fsub%2Fp/merge_requests/5/notes");
        assert_eq!(edit_path(&r, &Target::Issue(5), 9).unwrap(), ("PUT", "projects/g%2Fsub%2Fp/issues/5/notes/9".to_string()));
        assert!(list_path(&r, &Target::Commit("abc".into())).is_none());
    }
}
