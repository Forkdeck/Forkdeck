//! Jobs kept across restarts, in a SQLite database next to the deployment state. Several forkdecks
//! may share it: it runs in WAL mode, and job ids come from the database so they never clash.

use crate::state::{clip, AgentKind, Decision, Job, JobStatus, Plan, Planning, Session, Source, Stage, StageState, Turn};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How many jobs are kept; older ones are removed when forkdeck starts.
pub const KEEP: usize = 200;

/// Schema changes, in order. `PRAGMA user_version` says how many have been applied.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE jobs (
        id INTEGER PRIMARY KEY,
        title TEXT NOT NULL DEFAULT '',
        prompt TEXT NOT NULL DEFAULT '',
        source TEXT NOT NULL DEFAULT 'null',
        workdir TEXT NOT NULL DEFAULT '',
        branch TEXT,
        base_sha TEXT,
        status TEXT NOT NULL DEFAULT 'queued',
        resume_from TEXT,
        outcome TEXT NOT NULL DEFAULT '',
        decision TEXT,
        forced TEXT,
        session TEXT,
        pr_url TEXT,
        rounds INTEGER NOT NULL DEFAULT 0,
        verdict INTEGER,
        checks INTEGER,
        created INTEGER NOT NULL DEFAULT 0,
        finished INTEGER,
        worker TEXT,
        reviewer TEXT
    );
    CREATE TABLE turns (
        job INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
        n INTEGER NOT NULL,
        ask TEXT NOT NULL,
        answer TEXT NOT NULL,
        PRIMARY KEY (job, n)
    );
", "
    ALTER TABLE jobs ADD COLUMN planning TEXT;
    ALTER TABLE jobs ADD COLUMN plan TEXT;
", "
    ALTER TABLE jobs ADD COLUMN inbox TEXT;
    ALTER TABLE jobs ADD COLUMN owner TEXT;
"];

/// Who is running: this process, told apart from a later process that reuses its id by its start time.
pub fn owner_token() -> String {
    let pid = std::process::id();
    format!("{}:{}", pid, start_time(pid).unwrap_or_default())
}

/// The kernel's start time of a process (field 22 of /proc/<pid>/stat), or None where there is no /proc.
fn start_time(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // The command name is in parentheses and may hold spaces; the fields after it are plain.
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split_whitespace().nth(19).map(String::from)
}

/// Whether the process that wrote this owner token is still running.
pub fn owner_alive(token: &str) -> bool {
    let Some((pid, start)) = token.split_once(':') else { return false };
    // 0 is not a process (kill(0, ..) would ask about our own process group).
    let Ok(pid) = pid.parse::<u32>() else { return false };
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    match start_time(pid) {
        // Same id, same start: the same process.
        Some(now) => start.is_empty() || now == start,
        // No /proc: ask the kernel whether the id is in use.
        None => unsafe { libc::kill(pid as i32, 0) == 0 },
    }
}

pub fn path() -> PathBuf {
    crate::cicd::state_dir().join("forkdeck.db")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// An `Instant` from earlier in this run, as Unix seconds.
fn secs_of(at: Instant) -> u64 {
    now_secs().saturating_sub(at.elapsed().as_secs())
}

/// Unix seconds back to an `Instant`, as far back as this process can represent.
fn instant_of(secs: u64) -> Instant {
    let ago = Duration::from_secs(now_secs().saturating_sub(secs));
    Instant::now().checked_sub(ago).unwrap_or_else(Instant::now)
}

/// What a stage box shows, saved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StageRecord {
    pub agent: Option<AgentKind>,
    pub via: String,
    pub label: String,
    pub model: String,
    pub effort: String,
    pub state: String,
    pub tools: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: f64,
    pub secs: u64,
    pub tail: Vec<String>,
}

impl StageRecord {
    fn of(s: &Stage) -> StageRecord {
        let state = match s.state {
            StageState::Idle => "idle",
            StageState::Running => "running",
            StageState::Ok => "ok",
            StageState::Failed => "failed",
            StageState::Skipped => "skipped",
        };
        StageRecord {
            agent: s.agent,
            via: s.via.clone(),
            label: s.label.clone(),
            model: s.model.clone(),
            effort: s.effort.clone(),
            state: state.into(),
            tools: s.tools,
            tokens_in: s.tokens_in,
            tokens_out: s.tokens_out,
            cost: s.cost,
            secs: s.secs(),
            tail: s.tail.iter().cloned().collect(),
        }
    }
    fn stage(self) -> Stage {
        let state = match self.state.as_str() {
            "ok" => StageState::Ok,
            "skipped" => StageState::Skipped,
            "idle" => StageState::Idle,
            // A stage that was running when forkdeck stopped did not finish.
            _ => StageState::Failed,
        };
        Stage {
            agent: self.agent,
            via: self.via,
            session: None,
            label: self.label,
            model: self.model,
            effort: self.effort,
            state,
            last: self.tail.last().cloned().unwrap_or_default(),
            tools: self.tools,
            tokens_in: self.tokens_in,
            tokens_out: self.tokens_out,
            cost: self.cost,
            started: None,
            elapsed: (self.secs > 0).then(|| Duration::from_secs(self.secs)),
            tail: self.tail.into(),
        }
    }
}

/// Everything about a job that outlives the process.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRecord {
    pub id: u32,
    pub title: String,
    pub prompt: String,
    pub source: Source,
    pub workdir: PathBuf,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub status: JobStatus,
    pub resume_from: Option<JobStatus>,
    pub outcome: String,
    pub decision: Option<String>,
    pub forced: Option<String>,
    pub session: Option<Session>,
    pub pr_url: Option<String>,
    pub rounds: u32,
    pub verdict: Option<bool>,
    pub checks: Option<bool>,
    pub created: u64,
    pub finished: Option<u64>,
    pub worker: StageRecord,
    pub reviewer: StageRecord,
    pub turns: Vec<Turn>,
    pub planning: Planning,
    pub plan: Option<Plan>,
    /// Messages accepted for the agent and not taken yet.
    pub inbox: Vec<String>,
    /// The forkdeck process that runs the job, as `owner_token` wrote it.
    pub owner: Option<String>,
}

impl JobRecord {
    pub fn of(j: &Job) -> JobRecord {
        JobRecord {
            id: j.id,
            title: j.title.clone(),
            prompt: j.prompt.clone(),
            source: j.source.clone(),
            workdir: j.workdir.clone(),
            branch: j.branch.clone(),
            base_sha: j.base_sha.clone(),
            status: j.status,
            resume_from: j.resume_from,
            outcome: j.outcome.clone(),
            decision: j.decision.as_ref().and_then(|d| serde_json::to_string(d).ok()),
            forced: j.forced.as_ref().map(|r| r.name()),
            session: j.session.clone(),
            pr_url: j.pr_url.clone(),
            rounds: j.rounds,
            verdict: j.verdict,
            checks: j.checks,
            created: secs_of(j.created),
            finished: j.finished.map(secs_of),
            worker: StageRecord::of(&j.worker),
            reviewer: StageRecord::of(&j.reviewer),
            turns: j.turns.clone(),
            planning: j.planning,
            plan: j.plan.clone(),
            inbox: j.inbox.iter().cloned().collect(),
            owner: Some(owner_token()),
        }
    }

    /// The job as it was saved. One that was still running when forkdeck went away (killed, or
    /// out of time while stopping) did not finish: it comes back interrupted, from where it was.
    pub fn job(self) -> Job {
        let (status, resume_from) = if self.status.finished() { (self.status, self.resume_from) } else { (JobStatus::Interrupted, Some(self.status)) };
        let mut outcome = self.outcome;
        if status == JobStatus::Interrupted && outcome.is_empty() {
            outcome = "interrupted: forkdeck stopped while this ran".into();
        }
        Job {
            id: self.id,
            title: self.title,
            prompt: self.prompt,
            source: self.source,
            workdir: self.workdir,
            branch: self.branch,
            base_sha: self.base_sha,
            staged_tree: None,
            status,
            forced: self.forced.and_then(|f| crate::state::AgentRef::parse(&f).ok()),
            decision: self.decision.and_then(|d| serde_json::from_str::<Decision>(&d).ok()),
            worker: self.worker.stage(),
            reviewer: self.reviewer.stage(),
            extra: Vec::new(),
            verdict: self.verdict,
            checks: self.checks,
            outcome,
            pr_url: self.pr_url,
            rounds: self.rounds,
            created: instant_of(self.created),
            finished: Some(instant_of(self.finished.unwrap_or(self.created))),
            cancel: crate::state::Cancel::new(),
            claim: None,
            released: false,
            turns: self.turns,
            session: self.session,
            resume_from,
            inbox: self.inbox.into(),
            steer: crate::state::Cancel::new(),
            planning: self.planning,
            plan: self.plan,
            chose: Default::default(),
            worker_ran: None,
            reviewer_ran: None,
            peer: None,
            used: Default::default(),
            findings: Vec::new(),
            over_time: false,
        }
    }
}

fn status_text(s: JobStatus) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

fn status_of(s: &str) -> JobStatus {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or(JobStatus::Interrupted)
}

pub struct History {
    db: Mutex<Connection>,
}

impl History {
    pub fn open(path: &Path) -> Result<History, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        }
        let db = Connection::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        History::setup(db)
    }

    #[cfg(test)]
    pub fn in_memory() -> History {
        History::setup(Connection::open_in_memory().expect("in-memory database")).expect("schema")
    }

    fn setup(db: Connection) -> Result<History, String> {
        let e = |e: rusqlite::Error| e.to_string();
        db.busy_timeout(Duration::from_secs(5)).map_err(e)?;
        // WAL lets several forkdecks read and write the same file.
        let _: String = db.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0)).map_err(e)?;
        db.execute_batch("PRAGMA foreign_keys = ON").map_err(e)?;
        let done: usize = db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).map_err(e)? as usize;
        if done > MIGRATIONS.len() {
            return Err(format!("the history was written by a newer forkdeck (schema {}, this one knows {})", done, MIGRATIONS.len()));
        }
        for (i, m) in MIGRATIONS.iter().enumerate().skip(done) {
            db.execute_batch(&format!("BEGIN; {} PRAGMA user_version = {}; COMMIT;", m, i + 1)).map_err(e)?;
        }
        Ok(History { db: Mutex::new(db) })
    }

    /// A new job id, reserved with an empty row so another forkdeck cannot take it.
    pub fn new_id(&self) -> Result<u32, String> {
        let db = self.db.lock().unwrap();
        db.execute("INSERT INTO jobs (created) VALUES (?1)", params![now_secs() as i64]).map_err(|e| e.to_string())?;
        Ok(db.last_insert_rowid() as u32)
    }

    pub fn save(&self, r: &JobRecord) -> Result<(), String> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let json = |v: &dyn erased::Json| v.json();
        tx.execute(
            "INSERT INTO jobs (id, title, prompt, source, workdir, branch, base_sha, status, resume_from, outcome, decision, forced, session, pr_url, rounds, verdict, checks, created, finished, worker, reviewer, planning, plan, inbox, owner)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, prompt = excluded.prompt, source = excluded.source, workdir = excluded.workdir,
               branch = excluded.branch, base_sha = excluded.base_sha, status = excluded.status, resume_from = excluded.resume_from,
               outcome = excluded.outcome, decision = excluded.decision, forced = excluded.forced, session = excluded.session,
               pr_url = excluded.pr_url, rounds = excluded.rounds, verdict = excluded.verdict, checks = excluded.checks,
               created = excluded.created, finished = excluded.finished, worker = excluded.worker, reviewer = excluded.reviewer,
               planning = excluded.planning, plan = excluded.plan, inbox = excluded.inbox, owner = excluded.owner",
            params![
                r.id,
                r.title,
                r.prompt,
                json(&r.source),
                r.workdir.to_string_lossy(),
                r.branch,
                r.base_sha,
                status_text(r.status),
                r.resume_from.map(status_text),
                r.outcome,
                r.decision,
                r.forced,
                r.session.as_ref().map(|s| json(s)),
                r.pr_url,
                r.rounds,
                r.verdict,
                r.checks,
                r.created as i64,
                r.finished.map(|f| f as i64),
                json(&r.worker),
                json(&r.reviewer),
                json(&r.planning),
                r.plan.as_ref().map(|p| json(p)),
                json(&r.inbox),
                r.owner,
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM turns WHERE job = ?1", params![r.id]).map_err(|e| e.to_string())?;
        for (n, t) in r.turns.iter().enumerate() {
            tx.execute("INSERT INTO turns (job, n, ask, answer) VALUES (?1, ?2, ?3, ?4)", params![r.id, n as i64, t.ask, t.answer]).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// The newest `limit` jobs, oldest first. Rows that were reserved but never saved are skipped.
    pub fn load(&self, limit: usize) -> Result<Vec<JobRecord>, String> {
        let db = self.db.lock().unwrap();
        let e = |e: rusqlite::Error| e.to_string();
        let mut q = db
            .prepare(
                "SELECT id, title, prompt, source, workdir, branch, base_sha, status, resume_from, outcome, decision, forced, session, pr_url, rounds, verdict, checks, created, finished, worker, reviewer, planning, plan, inbox, owner
                 FROM jobs WHERE source != 'null' ORDER BY id DESC LIMIT ?1",
            )
            .map_err(e)?;
        let rows = q
            .query_map(params![limit as i64], |r| {
                let text = |i: usize| r.get::<_, Option<String>>(i).map(|v| v.unwrap_or_default());
                Ok(JobRecord {
                    id: r.get(0)?,
                    title: text(1)?,
                    prompt: text(2)?,
                    source: serde_json::from_str(&text(3)?).unwrap_or(Source::Manual),
                    workdir: PathBuf::from(text(4)?),
                    branch: r.get(5)?,
                    base_sha: r.get(6)?,
                    status: status_of(&text(7)?),
                    resume_from: r.get::<_, Option<String>>(8)?.map(|s| status_of(&s)),
                    outcome: text(9)?,
                    decision: r.get(10)?,
                    forced: r.get(11)?,
                    session: r.get::<_, Option<String>>(12)?.and_then(|s| serde_json::from_str(&s).ok()),
                    pr_url: r.get(13)?,
                    rounds: r.get(14)?,
                    verdict: r.get(15)?,
                    checks: r.get(16)?,
                    created: r.get::<_, i64>(17)? as u64,
                    finished: r.get::<_, Option<i64>>(18)?.map(|f| f as u64),
                    worker: serde_json::from_str(&text(19)?).unwrap_or_default(),
                    reviewer: serde_json::from_str(&text(20)?).unwrap_or_default(),
                    turns: Vec::new(),
                    planning: serde_json::from_str(&text(21)?).unwrap_or_default(),
                    plan: r.get::<_, Option<String>>(22)?.and_then(|p| serde_json::from_str(&p).ok()),
                    inbox: r.get::<_, Option<String>>(23)?.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or_default(),
                    owner: r.get(24)?,
                })
            })
            .map_err(e)?;
        let mut out: Vec<JobRecord> = rows.collect::<Result<_, _>>().map_err(e)?;
        let mut tq = db.prepare("SELECT ask, answer FROM turns WHERE job = ?1 ORDER BY n").map_err(e)?;
        for r in &mut out {
            r.turns = tq.query_map(params![r.id], |t| Ok(Turn { ask: t.get(0)?, answer: t.get(1)? })).map_err(e)?.collect::<Result<_, _>>().map_err(e)?;
        }
        out.reverse();
        Ok(out)
    }

    /// Take a job over for this process, unless another forkdeck that is still running owns it.
    /// One transaction, so two processes starting at once cannot both take it.
    pub fn claim(&self, id: u32) -> Result<bool, String> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let owner: Option<Option<String>> = tx.query_row("SELECT owner FROM jobs WHERE id = ?1", params![id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        let me = owner_token();
        match owner {
            None => return Ok(false),
            Some(Some(o)) if o != me && owner_alive(&o) => return Ok(false),
            _ => {}
        }
        tx.execute("UPDATE jobs SET owner = ?1 WHERE id = ?2", params![me, id]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// Keep the newest `keep` jobs.
    pub fn prune(&self, keep: usize) -> Result<usize, String> {
        let db = self.db.lock().unwrap();
        let cut: Option<i64> = db.query_row("SELECT id FROM jobs ORDER BY id DESC LIMIT 1 OFFSET ?1", params![keep as i64], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        match cut {
            Some(c) => db.execute("DELETE FROM jobs WHERE id <= ?1", params![c]).map_err(|e| e.to_string()),
            None => Ok(0),
        }
    }

    pub fn clear(&self) -> Result<usize, String> {
        self.db.lock().unwrap().execute("DELETE FROM jobs", []).map_err(|e| e.to_string())
    }
}

/// One line per saved job, for `forkdeck history`.
pub fn summary(r: &JobRecord) -> String {
    let when = chrono::DateTime::from_timestamp(r.created as i64, 0).map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
    format!("#{:<5} {}  {:<12} {}", r.id, when, r.status.name(), clip(&r.title, 70))
}

mod erased {
    /// Serialise anything for a TEXT column; an error leaves the column as JSON null.
    pub trait Json {
        fn json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap_or_else(|_| "null".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AgentRef, Cancel, Gate, Tier};

    fn job(id: u32, source: Source, status: JobStatus) -> Job {
        let mut worker = Stage { agent: Some(AgentKind::Claude), via: "copilot".into(), model: "claude-sonnet-4.5".into(), state: StageState::Ok, tools: 3, tokens_in: 10, tokens_out: 2, ..Stage::default() };
        worker.push("▸ Edit src/a.rs");
        worker.push("Done.");
        Job {
            id,
            title: format!("job {}", id),
            prompt: "fix it".into(),
            source,
            workdir: PathBuf::from("/tmp/w"),
            branch: Some("b".into()),
            base_sha: Some("abc".into()),
            staged_tree: None,
            status,
            forced: Some(AgentRef::Profile("second".into())),
            decision: Some(Decision { agent: AgentKind::Codex, tier: Tier::Heavy, review: true, gates: vec![Gate { name: "effort".into(), answer: "heavy".into(), value: 0.9, sharp: true }], source: "rules".into(), ms: 3, note: String::new(), plan: true }),
            worker,
            reviewer: Stage::default(),
            extra: Vec::new(),
            verdict: Some(true),
            checks: None,
            outcome: "ok".into(),
            pr_url: None,
            rounds: 1,
            created: Instant::now(),
            finished: Some(Instant::now()),
            cancel: Cancel::new(),
            claim: None,
            released: false,
            turns: vec![Turn { ask: "fix it".into(), answer: "fixed".into() }, Turn { ask: "and the test".into(), answer: String::new() }],
            session: Some(Session { cli: AgentKind::Claude, id: "s1".into(), model: "sonnet".into() }),
            resume_from: None,
            inbox: vec!["and the docs too".to_string()].into(),
            steer: Cancel::new(),
            peer: None,
            used: Default::default(),
            findings: Vec::new(),
            over_time: false,
            planning: Planning::Always,
            plan: Some(Plan { options: vec![crate::state::PlanOption { title: "a".into(), summary: "b".into(), recommended: true }], text: "t".into(), chosen: Some(crate::state::Choice::Own("mine".into())) }),
            chose: Default::default(),
            worker_ran: None,
            reviewer_ran: None,
        }
    }

    #[test]
    fn jobs_survive_a_round_trip() {
        let h = History::in_memory();
        let sources = [
            Source::Manual,
            Source::Issue { repo_idx: 0, repo: "o/r".into(), number: 7, url: "u".into() },
            Source::Deploy { repo_idx: 1, repo: "o/r".into(), sha: "s".into() },
            Source::Pr { repo_idx: 0, repo: "o/r".into(), number: 5, url: "u".into(), head: "h".into(), base: "main".into() },
        ];
        let mut saved = Vec::new();
        for (i, src) in sources.into_iter().enumerate() {
            let id = h.new_id().unwrap();
            let status = [JobStatus::Done, JobStatus::Failed, JobStatus::Interrupted, JobStatus::Cancelled][i];
            let r = JobRecord::of(&job(id, src, status));
            h.save(&r).unwrap();
            saved.push(r);
        }
        let back = h.load(10).unwrap();
        assert_eq!(back, saved);
        // Saving again replaces, it does not add.
        h.save(&saved[0]).unwrap();
        assert_eq!(h.load(10).unwrap().len(), 4);
        let j = back[0].clone().job();
        assert_eq!((j.turns.len(), j.worker.tail.len(), j.worker.via.as_str(), j.forced.map(|f| f.name())), (2, 2, "copilot", Some("copilot:second".to_string())));
        assert_eq!(j.decision.unwrap().tier, Tier::Heavy);
        assert_eq!(j.inbox.iter().collect::<Vec<_>>(), ["and the docs too"], "a queued message survives a restart");
    }

    #[test]
    fn a_job_that_was_running_comes_back_interrupted_from_where_it_was() {
        let h = History::in_memory();
        let id = h.new_id().unwrap();
        let mut j = job(id, Source::Manual, JobStatus::Reviewing);
        j.reviewer.state = StageState::Running;
        h.save(&JobRecord::of(&j)).unwrap();
        let back = h.load(10).unwrap().remove(0).job();
        assert_eq!((back.status, back.resume_from, back.reviewer.state), (JobStatus::Interrupted, Some(JobStatus::Reviewing), StageState::Failed));
        assert!(back.outcome.contains("ok") || back.outcome.contains("interrupted"));
    }

    #[test]
    fn a_job_is_claimed_only_from_an_owner_that_is_gone() {
        let h = History::in_memory();
        let id = h.new_id().unwrap();
        let mut r = JobRecord::of(&job(id, Source::Manual, JobStatus::Interrupted));
        // A process that cannot exist: beyond any pid_max.
        r.owner = Some("999999999:1".into());
        h.save(&r).unwrap();
        assert!(h.claim(id).unwrap(), "the owner is gone");
        assert_eq!(h.load(10).unwrap()[0].owner.as_deref(), Some(owner_token().as_str()));
        assert!(h.claim(id).unwrap(), "claiming again from this process is fine");
        // Owned by a live process that is not this one: init (pid 1) is always running.
        let other = format!("1:{}", start_time(1).unwrap_or_default());
        r.owner = Some(other.clone());
        h.save(&r).unwrap();
        assert!(owner_alive(&other));
        assert!(!h.claim(id).unwrap(), "a running forkdeck keeps its job");
        assert!(!h.claim(9999).unwrap(), "no such job");
    }

    #[test]
    fn ids_are_unique_and_old_jobs_are_pruned() {
        let h = History::in_memory();
        let ids: Vec<u32> = (0..5).map(|_| h.new_id().unwrap()).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
        assert!(h.load(10).unwrap().is_empty(), "reserved but never saved");
        for id in &ids {
            h.save(&JobRecord::of(&job(*id, Source::Manual, JobStatus::Done))).unwrap();
        }
        assert_eq!(h.prune(3).unwrap(), 2);
        assert_eq!(h.load(10).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), vec![3, 4, 5]);
        assert_eq!(h.load(2).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), vec![4, 5], "the newest, oldest first");
        assert_eq!(h.clear().unwrap(), 3);
    }

    #[test]
    fn a_file_database_is_reopened_and_a_newer_schema_is_refused() {
        let dir = std::env::temp_dir().join(format!("forkdeck-history-test-{}", std::process::id()));
        let p = dir.join("h.db");
        {
            let h = History::open(&p).unwrap();
            let id = h.new_id().unwrap();
            h.save(&JobRecord::of(&job(id, Source::Manual, JobStatus::Done))).unwrap();
        }
        assert_eq!(History::open(&p).unwrap().load(10).unwrap().len(), 1);
        Connection::open(&p).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(History::open(&p).err().unwrap().contains("newer forkdeck"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
