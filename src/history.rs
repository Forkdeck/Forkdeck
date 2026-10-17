//! Jobs kept across restarts, in a SQLite database next to the deployment state. Several forkdecks
//! may share it: it runs in WAL mode, and job ids come from the database so they never clash.

use crate::state::{clip, AgentKind, Decision, Job, JobStatus, Plan, Planning, Session, Source, Stage, StageState, Turn};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A job whose issue plan is still in progress: a publication pending, or not yet marked ready for build.
/// Resolved is what `Plan::ready_for_intake` says: ready, nothing pending, and the current revision
/// reviewed, accepted and published.
const UNRESOLVED_PLAN: &str = "(issue_plan IS NOT NULL AND NOT (json_extract(issue_plan, '$.ready') IS 1 \
    AND json_extract(issue_plan, '$.pending') IS NULL \
    AND json_extract(issue_plan, '$.reviewed') IS json_extract(issue_plan, '$.revision') \
    AND json_extract(issue_plan, '$.accepted') IS json_extract(issue_plan, '$.revision') \
    AND json_extract(issue_plan, '$.published') IS json_extract(issue_plan, '$.revision')))";

/// How many jobs are kept; older ones are removed when forkdeck starts. Issue plans still in
/// progress are kept as well, whatever their age.
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
", "
    CREATE TABLE ids (last INTEGER NOT NULL);
    INSERT INTO ids SELECT COALESCE(MAX(id), 0) FROM jobs;
", "
    ALTER TABLE jobs ADD COLUMN active_secs INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE jobs ADD COLUMN used TEXT;
", "
    ALTER TABLE jobs ADD COLUMN issue_plan TEXT;
    CREATE TABLE composer (workdir TEXT PRIMARY KEY, state TEXT NOT NULL);
"];

/// Who is running: this process, told apart from a later process that reuses its id by its start time.
pub fn owner_token() -> String {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| { let pid = std::process::id(); format!("{}:{}", pid, start_time(pid).unwrap_or_default()) }).clone()
}

/// The kernel's start time of a process (field 22 of /proc/<pid>/stat), or None where there is no /proc.
#[cfg(target_os = "linux")]
fn start_time(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split_whitespace().nth(19).map(String::from)
}

#[cfg(not(target_os = "linux"))]
fn start_time(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps").args(["-o", "lstart=", "-p", &pid.to_string()]).output().ok()?;
    let started = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !started.is_empty()).then_some(started)
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
        None => crate::team::alive(pid),
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

fn instant_of(secs: u64) -> Instant {
    Instant::now().checked_sub(Duration::from_secs(now_secs().saturating_sub(secs))).unwrap_or_else(Instant::now)
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
            unprinted: 0,
            // As if the restored lines had just been pushed: a viewer's offsets stay inside the tail.
            pushed: self.tail.len() as u64,
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
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JobRecord {
    pub active_secs: u64,
    pub used: [crate::journal::Usage; 3],
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
    pub issue_plan: Option<crate::planning::Plan>,
}

impl JobRecord {
    pub fn of(j: &Job) -> JobRecord {
        JobRecord {
            active_secs: j.active_secs + j.finished.unwrap_or_else(Instant::now).saturating_duration_since(j.active_started).as_secs(),
            used: j.used,
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
            issue_plan: j.issue_plan.clone(),
        }
    }

    /// The job as it was saved. One that was still running when forkdeck went away (killed, or
    /// out of time while stopping) did not finish: it comes back interrupted, from where it was.
    pub fn job(self) -> Job {
        let (status, resume_from) = if self.status.finished() { (self.status, self.resume_from) } else { (JobStatus::Interrupted, if self.status == JobStatus::Queued { self.resume_from.or(Some(self.status)) } else { Some(self.status) }) };
        let mut outcome = self.outcome;
        if status == JobStatus::Interrupted && outcome.is_empty() {
            outcome = "interrupted: forkdeck stopped while this ran".into();
        }
        Job {
            active_secs: self.active_secs,
            active_started: Instant::now(),
            restored: true,
            worktree_lock: None,
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
            in_history: true,
            peer: None,
            used: self.used,
            findings: Vec::new(),
            over_time: false,
            admitted: false,
            issue_plan: self.issue_plan.map(|mut p| { p.build_job = None; p }),
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
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        if let Some(dir) = path.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        }
        std::fs::OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(path).map_err(|e| e.to_string())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
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
        for attempt in 0..=50 {
            match db.query_row::<String, _, _>("PRAGMA journal_mode = WAL", [], |r| r.get(0)) {
                Ok(_) => break,
                Err(err) if attempt < 50 && matches!(err.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)) => std::thread::sleep(Duration::from_millis(100)),
                Err(err) => return Err(err.to_string()),
            }
        }
        db.execute_batch("PRAGMA foreign_keys = ON").map_err(e)?;
        db.execute_batch("BEGIN IMMEDIATE").map_err(e)?;
        let done: usize = db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).map_err(e)? as usize;
        if done > MIGRATIONS.len() {
            return Err(format!("the history was written by a newer forkdeck (schema {}, this one knows {})", done, MIGRATIONS.len()));
        }
        for (i, m) in MIGRATIONS.iter().enumerate().skip(done) {
            db.execute_batch(&format!("{} PRAGMA user_version = {};", m, i + 1)).map_err(e)?;
        }
        db.execute_batch("COMMIT").map_err(e)?;
        Ok(History { db: Mutex::new(db) })
    }

    /// A new job id, reserved with an empty row so another forkdeck cannot take it.
    /// Ids only ever go up, also across `clear`: a forkdeck that is still open keeps jobs with the
    /// old ids, and a new job must never get one of them.
    pub fn new_id(&self) -> Result<u32, String> {
        let mut db = self.db.lock().unwrap();
        let e = |e: rusqlite::Error| e.to_string();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        let last: i64 = tx.query_row("SELECT MAX((SELECT last FROM ids), COALESCE((SELECT MAX(id) FROM jobs), 0))", [], |r| r.get(0)).map_err(e)?;
        let id = last + 1;
        if id > u32::MAX as i64 { return Err("job ids exhausted".into()); }
        tx.execute("INSERT INTO jobs (id, created) VALUES (?1, ?2)", params![id, now_secs() as i64]).map_err(e)?;
        tx.execute("UPDATE ids SET last = ?1", params![id]).map_err(e)?;
        tx.commit().map_err(e)?;
        Ok(id as u32)
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
        tx.execute("UPDATE jobs SET active_secs = ?1, used = ?2, issue_plan = ?3 WHERE id = ?4", params![r.active_secs, json(&r.used), r.issue_plan.as_ref().map(|p| json(p)), r.id]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM turns WHERE job = ?1", params![r.id]).map_err(|e| e.to_string())?;
        for (n, t) in r.turns.iter().enumerate() {
            tx.execute("INSERT INTO turns (job, n, ask, answer) VALUES (?1, ?2, ?3, ?4)", params![r.id, n as i64, t.ask, t.answer]).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// What the input line held in a working directory when the interface last closed (JSON).
    pub fn composer(&self, workdir: &str) -> Option<String> {
        self.db.lock().unwrap().query_row("SELECT state FROM composer WHERE workdir = ?1", params![workdir], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn save_composer(&self, workdir: &str, state: &str) -> Result<(), String> {
        self.db.lock().unwrap().execute("INSERT INTO composer (workdir, state) VALUES (?1, ?2) ON CONFLICT(workdir) DO UPDATE SET state = excluded.state", params![workdir, state]).map(|_| ()).map_err(|e| e.to_string())
    }

    /// The newest `limit` jobs, oldest first. Rows that were reserved but never saved are skipped.
    /// Also every issue plan still in progress, however old: it is working state, holding drafts and
    /// the tokens that reconcile an uncertain write, not history.
    pub fn load(&self, limit: usize) -> Result<Vec<JobRecord>, String> {
        let mut out = self.load_where("1 = ?1", 1, limit)?;
        let newest_kept = out.first().map(|r| r.id as i64).unwrap_or(i64::MAX);
        let mut older = self.load_where(&format!("id < ?1 AND {}", UNRESOLVED_PLAN), newest_kept, usize::MAX >> 1)?;
        older.append(&mut out);
        Ok(older)
    }

    fn load_where(&self, filter: &str, arg: i64, limit: usize) -> Result<Vec<JobRecord>, String> {
        let db = self.db.lock().unwrap();
        Self::read_where(&db, filter, arg, limit)
    }

    fn read_where(db: &Connection, filter: &str, arg: i64, limit: usize) -> Result<Vec<JobRecord>, String> {
        let e = |e: rusqlite::Error| e.to_string();
        let mut q = db
            .prepare(&format!(
                "SELECT id, title, prompt, source, workdir, branch, base_sha, status, resume_from, outcome, decision, forced, session, pr_url, rounds, verdict, checks, created, finished, worker, reviewer, planning, plan, inbox, owner, active_secs, used, issue_plan
                 FROM jobs WHERE source != 'null' AND {} ORDER BY id DESC LIMIT ?2",
                filter
            ))
            .map_err(e)?;
        let rows = q
            .query_map(params![arg, limit as i64], |r| {
                let text = |i: usize| r.get::<_, Option<String>>(i).map(|v| v.unwrap_or_default());
                Ok(JobRecord {
                    active_secs: r.get(25)?,
                    used: serde_json::from_str(&text(26)?).unwrap_or_default(),
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
                    issue_plan: r.get::<_, Option<String>>(27)?.and_then(|p| serde_json::from_str(&p).ok()),
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
    pub fn take_over(&self, id: u32, resume: bool) -> Result<Option<JobRecord>, String> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let record = Self::read_where(&tx, "id = ?1", id as i64, 1)?.into_iter().next();
        let me = owner_token();
        let owner: Option<Option<String>> = tx.query_row("SELECT owner FROM jobs WHERE id = ?1", params![id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        if owner.as_ref().and_then(|o| o.as_deref()).is_some_and(|o| o != me && owner_alive(o)) { return Err("another forkdeck that is still running has it".into()); }
        if owner.is_none() { tx.execute("INSERT INTO jobs (id, created, owner) VALUES (?1, ?2, ?3)", params![id, now_secs() as i64, me]).map_err(|e| e.to_string())?; }
        if let Some(r) = &record {
            if r.owner.as_deref().is_some_and(|o| o != me && owner_alive(o)) { return Err("another forkdeck that is still running has it".into()); }
            if resume && r.status.finished() && r.status != JobStatus::Interrupted { return Err("the saved job is no longer interrupted".into()); }
        }
        tx.execute("UPDATE jobs SET owner = ?1 WHERE id = ?2", params![me, id]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(record)
    }

    #[cfg(test)]
    pub fn claim(&self, id: u32) -> Result<bool, String> { self.take_over(id, false).map(|_| true).or_else(|e| if e.contains("still running") { Ok(false) } else { Err(e) }) }

    /// One saved job, as it is in the database now.
    #[cfg(test)]
    pub fn get(&self, id: u32) -> Result<Option<JobRecord>, String> {
        Ok(self.load_where("id = ?1", id as i64, 1)?.into_iter().next())
    }

    /// Keep the newest `keep` jobs.
    pub fn prune(&self, keep: usize) -> Result<usize, String> {
        let db = self.db.lock().unwrap();
        let cut: Option<i64> = db.query_row("SELECT id FROM jobs ORDER BY id DESC LIMIT 1 OFFSET ?1", params![keep as i64], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
        match cut {
            Some(c) => db.execute(&format!("DELETE FROM jobs WHERE id <= ?1 AND NOT ({})", UNRESOLVED_PLAN), params![c]).map_err(|e| e.to_string()),
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
            active_secs: 0,
            active_started: Instant::now(),
            restored: false,
            worktree_lock: None,
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
            admitted: false,
            issue_plan: None,
            planning: Planning::Always,
            plan: Some(Plan { options: vec![crate::state::PlanOption { title: "a".into(), summary: "b".into(), recommended: true }], text: "t".into(), chosen: Some(crate::state::Choice::Own("mine".into())) }),
            chose: Default::default(),
            worker_ran: None,
            reviewer_ran: None,
            in_history: true,
        }
    }

    #[tokio::test]
    async fn failed_id_reservation_does_not_create_a_runtime_job_and_saves_keep_the_latest_state() {
        let history = std::sync::Arc::new(History::in_memory());
        let mut hub = crate::orchestrator::Hub::new(crate::config::Config::default(), tokio::runtime::Handle::current());
        hub.history = Some(history.clone());
        let _permit = hub.sem.clone().acquire_many_owned(hub.cfg.automation.max_parallel as u32).await.unwrap();
        history.db.lock().unwrap().execute("UPDATE ids SET last = ?1", params![u32::MAX]).unwrap();
        assert!(hub.submit("x".into(), "x".into(), Source::Manual, PathBuf::from("."), None, Planning::Never).await.is_err());
        assert!(hub.state.lock().unwrap().jobs.is_empty());
        history.db.lock().unwrap().execute("UPDATE ids SET last = 0", []).unwrap();
        let id = hub.submit("x".into(), "x".into(), Source::Manual, PathBuf::from("."), None, Planning::Never).await.unwrap();
        assert_eq!(id, 1);
        for i in 0..20 {
            hub.job(id, |j| { j.outcome = i.to_string(); });
            hub.save(id);
        }
        hub.job(id, |j| { j.status = JobStatus::Done; j.outcome = "final".into(); });
        hub.save(id);
        for _ in 0..200 {
            if history.get(id).unwrap().is_some_and(|r| r.status == JobStatus::Done && r.outcome == "final") { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let r = history.get(id).unwrap().unwrap();
        assert_eq!((r.status, r.outcome.as_str()), (JobStatus::Done, "final"));
    }

    #[test]
    fn an_issue_plan_in_progress_outlives_trimming() {
        let h = History::in_memory();
        let cfg: crate::config::Config = toml::from_str("[[repos]]\nprovider='github'\nrepo='a/b'\npath='/tmp'\n").unwrap();
        let plan = crate::planning::Plan::new(&cfg.repos[0], "7").unwrap();
        let mut ready = plan.clone();
        (ready.ready, ready.revision, ready.reviewed, ready.accepted, ready.published) = (true, 2, Some(2), Some(2), Some(2));
        assert!(ready.ready_for_intake());
        for (plan, title) in [(Some(plan), "unresolved"), (Some(ready), "ready"), (None, "a"), (None, "b"), (None, "c")] {
            let id = h.new_id().unwrap();
            let mut j = job(id, Source::Manual, JobStatus::Done);
            j.title = title.into();
            j.issue_plan = plan;
            h.save(&JobRecord::of(&j)).unwrap();
        }
        h.prune(2).unwrap();
        let titles: Vec<String> = h.load(2).unwrap().into_iter().map(|r| r.title).collect();
        assert_eq!(titles, ["unresolved", "b", "c"], "the plan in progress is kept and loaded; the finished one is history");
    }

    #[test]
    fn queued_resumes_keep_the_stage_and_usage_without_downtime() {
        let mut j = job(1, Source::Manual, JobStatus::Queued);
        j.resume_from = Some(JobStatus::Reviewing);
        j.active_secs = 17;
        j.used[0].tokens_in = 91;
        let mut record = JobRecord::of(&j);
        record.created = now_secs() - 86400;
        let back = record.job();
        assert_eq!(back.resume_from, Some(JobStatus::Reviewing));
        assert_eq!(back.used[0].tokens_in, 91);
        assert_eq!(crate::journal::entry(&back, JobStatus::Done, "done", 0).secs, 17);
    }

    #[test]
    fn takeover_reads_the_latest_record_and_refuses_completed_work() {
        let h = History::in_memory();
        let id = h.new_id().unwrap();
        let mut r = JobRecord::of(&job(id, Source::Manual, JobStatus::Interrupted));
        h.save(&r).unwrap();
        r.turns.push(Turn { ask: "newer request".into(), answer: "new answer".into() });
        h.save(&r).unwrap();
        assert_eq!(h.take_over(id, true).unwrap().unwrap().turns.len(), 3);
        r.status = JobStatus::Done;
        h.save(&r).unwrap();
        assert!(h.take_over(id, true).unwrap_err().contains("no longer interrupted"));
        assert!(h.take_over(id, false).unwrap().is_some());
    }

    #[test]
    fn concurrent_open_migrates_once_and_creates_private_files() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = crate::proc::Scratch::new().unwrap();
        let path = scratch.0.join("state/history.db");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4).map(|_| {
            let (p, b) = (path.clone(), barrier.clone());
            std::thread::spawn(move || { b.wait(); History::open(&p).unwrap().new_id().unwrap() })
        }).collect();
        let mut ids: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        ids.sort();
        assert_eq!(ids, [1, 2, 3, 4]);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
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
        assert!(h.claim(9999).unwrap(), "a missing job has nobody to refuse takeover");
        assert_eq!(h.get(id).unwrap().map(|r| r.id), Some(id));
        assert!(h.get(9999).unwrap().is_none());
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
        assert_eq!(h.new_id().unwrap(), 6, "clearing never hands an old id out again");
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
