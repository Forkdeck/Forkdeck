use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Copilot,
}

impl AgentKind {
    pub const ALL: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Codex, AgentKind::Copilot];

    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Copilot => "copilot",
        }
    }
    pub fn other(self) -> AgentKind {
        match self {
            AgentKind::Claude => AgentKind::Codex,
            AgentKind::Codex | AgentKind::Copilot => AgentKind::Claude,
        }
    }
}

/// What a task, a reviewer list or `--agent` asks for: a CLI by name, or a named copilot profile
/// (`copilot:<name>`). Which program and model actually run is decided later, in `pick`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum AgentRef {
    Kind(AgentKind),
    Profile(String),
}

impl AgentRef {
    pub fn parse(s: &str) -> Result<AgentRef, String> {
        let s = s.trim();
        if let Some(k) = AgentKind::ALL.into_iter().find(|k| k.name() == s) {
            return Ok(AgentRef::Kind(k));
        }
        match s.strip_prefix("copilot:") {
            Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') => {
                Ok(AgentRef::Profile(n.to_string()))
            }
            _ => Err(format!("unknown agent {:?}: use claude, codex, copilot or copilot:<profile>", s)),
        }
    }
    pub fn name(&self) -> String {
        match self {
            AgentRef::Kind(k) => k.name().to_string(),
            AgentRef::Profile(p) => format!("copilot:{}", p),
        }
    }
    /// The CLI this runs on when nothing stands in for anything.
    pub fn kind(&self) -> AgentKind {
        match self {
            AgentRef::Kind(k) => *k,
            AgentRef::Profile(_) => AgentKind::Copilot,
        }
    }
}

impl<'de> Deserialize<'de> for AgentRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        AgentRef::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for AgentRef {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.name())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Light,
    Standard,
    Heavy,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Light => "light",
            Tier::Standard => "standard",
            Tier::Heavy => "heavy",
        }
    }
    pub fn up(self) -> Tier {
        match self {
            Tier::Light => Tier::Standard,
            _ => Tier::Heavy,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    #[default]
    Queued,
    Routing,
    /// The agent is proposing ways to do the task.
    Planning,
    /// Waiting for the user to pick one of the proposed ways.
    Choosing,
    Working,
    Reviewing,
    Fixing,
    Publishing,
    /// Waiting for the project's CI on the pull request.
    Ci,
    Deploying,
    RollingBack,
    Done,
    Failed,
    Cancelled,
    /// forkdeck stopped while the job ran. A typed task continues on the next start.
    Interrupted,
}

impl JobStatus {
    pub fn name(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Routing => "routing",
            JobStatus::Planning => "planning",
            JobStatus::Choosing => "choose an option",
            JobStatus::Working => "working",
            JobStatus::Reviewing => "reviewing",
            JobStatus::Fixing => "fixing",
            JobStatus::Publishing => "publishing",
            JobStatus::Ci => "watching ci",
            JobStatus::Deploying => "deploying",
            JobStatus::RollingBack => "rolling back",
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
            JobStatus::Interrupted => "interrupted",
        }
    }
    pub fn finished(self) -> bool {
        matches!(self, JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled | JobStatus::Interrupted)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StageState {
    Idle,
    Running,
    Ok,
    Failed,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct Stage {
    pub unprinted: usize,
    pub agent: Option<AgentKind>,
    /// Set when the agent ran on another CLI: "copilot" standing in, or a "copilot:<name>" profile.
    pub via: String,
    /// The CLI's id for the conversation in this box, as soon as it is known.
    pub session: Option<String>,
    /// What the box shows when no agent runs in it (a deployment step).
    pub label: String,
    pub model: String,
    pub effort: String,
    pub state: StageState,
    pub last: String,
    pub tools: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: f64,
    pub started: Option<Instant>,
    pub elapsed: Option<Duration>,
    pub tail: VecDeque<String>,
    /// Lines ever pushed, so a remote viewer can ask for the ones it has not seen.
    pub pushed: u64,
}

impl Default for Stage {
    fn default() -> Self {
        Stage {
            agent: None,
            via: String::new(),
            session: None,
            label: String::new(),
            model: String::new(),
            effort: String::new(),
            state: StageState::Idle,
            last: String::new(),
            tools: 0,
            tokens_in: 0,
            tokens_out: 0,
            cost: 0.0,
            started: None,
            elapsed: None,
            tail: VecDeque::new(),
            unprinted: 0,
            pushed: 0,
        }
    }
}

impl Stage {
    pub fn push(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.last = line.clone();
        self.tail.push_back(line);
        self.unprinted = (self.unprinted + 1).min(200);
        self.pushed += 1;
        while self.tail.len() > 200 {
            self.tail.pop_front();
        }
    }
    pub fn secs(&self) -> u64 {
        match (self.elapsed, self.started) {
            (Some(d), _) => d.as_secs(),
            (None, Some(s)) => s.elapsed().as_secs(),
            _ => 0,
        }
    }
}

/// One typed answer from the decision layer, drawn as a bar in the fork-layer box.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gate {
    pub name: String,
    pub answer: String,
    pub value: f64,
    pub sharp: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub agent: AgentKind,
    pub tier: Tier,
    pub review: bool,
    pub gates: Vec<Gate>,
    pub source: String,
    pub ms: u128,
    pub note: String,
    /// The fork layer thinks the user should choose an approach before work starts.
    #[serde(default)]
    pub plan: bool,
}

/// Whether a job proposes approaches before it works.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Planning {
    /// When the fork layer says so.
    #[default]
    IfRouted,
    Always,
    Never,
}

/// One way of doing the task, as the agent proposed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanOption {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    /// One of the proposed options, counted from 0.
    Option(usize),
    /// The user's own approach instead.
    Own(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub options: Vec<PlanOption>,
    /// Everything the agent said while planning.
    pub text: String,
    pub chosen: Option<Choice>,
}

impl Choice {
    pub fn parse(text: &str, n: usize) -> Choice {
        match text.trim().parse::<usize>() {
            Ok(k) if (1..=n).contains(&k) => Choice::Option(k - 1),
            _ => Choice::Own(text.to_string()),
        }
    }
}

impl Plan {
    pub fn show_options(&self, stage: &mut Stage) {
        for (i, o) in self.options.iter().enumerate() { stage.push(format!("{}) {}{} — {}", i + 1, o.title, if o.recommended { " (recommended)" } else { "" }, o.summary)); }
    }
    /// The recommended option, else the first.
    pub fn default_choice(&self) -> usize {
        self.options.iter().position(|o| o.recommended).unwrap_or(0)
    }
    pub fn describe(&self, c: &Choice) -> String {
        match c {
            Choice::Option(i) => self.options.get(*i).map(|o| format!("option {}: {}", i + 1, o.title)).unwrap_or_else(|| format!("option {}", i + 1)),
            Choice::Own(t) => format!("own approach: {}", t),
        }
    }
}

/// The options in an agent's plan: the last `<options>{...}</options>` block. Earlier blocks (a
/// quoted example, a first draft) do not count. None when there is no usable block.
pub fn parse_options(text: &str) -> Option<Vec<PlanOption>> {
    let start = text.rfind("<options>")? + "<options>".len();
    let end = start + text[start..].find("</options>")?;
    #[derive(Deserialize)]
    struct Block {
        options: Vec<PlanOption>,
    }
    let mut opts = serde_json::from_str::<Block>(text[start..end].trim()).ok()?.options;
    opts.retain(|o| !o.title.trim().is_empty());
    if opts.is_empty() || opts.len() > 9 {
        return None;
    }
    // One recommendation at most: the first one marked.
    let mut seen = false;
    for o in &mut opts {
        if o.recommended && seen {
            o.recommended = false;
        }
        seen |= o.recommended;
    }
    Some(opts)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Source {
    #[default]
    Manual,
    Issue { repo_idx: usize, repo: String, number: u64, url: String },
    /// The base branch moved: deploy that commit.
    Deploy { repo_idx: usize, repo: String, sha: String },
    /// An existing pull or merge request that was labelled for review.
    Pr { repo_idx: usize, repo: String, number: u64, url: String, head: String, base: String },
}

impl Source {
    pub fn is_manual(&self) -> bool {
        matches!(self, Source::Manual)
    }
}

pub struct Cancel {
    flag: std::sync::atomic::AtomicU8,
    notify: tokio::sync::Notify,
}

impl Cancel {
    pub fn new() -> Arc<Cancel> {
        Arc::new(Cancel { flag: std::sync::atomic::AtomicU8::new(0), notify: tokio::sync::Notify::new() })
    }
    pub fn fire(&self) {
        let _ = self.flag.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn shutdown(&self) {
        let _ = self.flag.compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_shutdown(&self) -> bool { self.flag.load(Ordering::SeqCst) == 2 }
    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst) != 0
    }
    pub async fn wait(&self) {
        loop {
            if self.is_set() {
                return;
            }
            let n = self.notify.notified();
            if self.is_set() {
                return;
            }
            n.await;
        }
    }
}

/// The worker's own conversation in its CLI, continued on later turns instead of retelling it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub cli: AgentKind,
    pub id: String,
    pub model: String,
}

/// One exchange in a typed task's conversation: what the user asked and what the agent said back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub ask: String,
    /// Empty until the turn has produced a result.
    pub answer: String,
}

pub struct Job {
    pub restored: bool,
    pub active_secs: u64,
    pub active_started: Instant,
    pub worktree_lock: Option<Arc<crate::proc::FileLock>>,
    pub id: u32,
    pub title: String,
    pub prompt: String,
    pub source: Source,
    pub workdir: PathBuf,
    pub branch: Option<String>,
    /// The commit the work started from. The change under review is everything since this commit.
    pub base_sha: Option<String>,
    /// Tree of the agent's work as last staged. Only this tree may be committed.
    pub staged_tree: Option<String>,
    pub status: JobStatus,
    pub forced: Option<AgentRef>,
    pub decision: Option<Decision>,
    pub worker: Stage,
    pub reviewer: Stage,
    /// Pull request reviewers past the second, in configured order.
    pub extra: Vec<Stage>,
    pub verdict: Option<bool>,
    /// Result of the configured check commands; None when there are none or they have not run.
    pub checks: Option<bool>,
    pub outcome: String,
    pub pr_url: Option<String>,
    pub rounds: u32,
    pub created: Instant,
    pub finished: Option<Instant>,
    pub cancel: Arc<Cancel>,
    /// The status comment teammates see, while this job holds one.
    pub claim: Option<Arc<crate::team::Held>>,
    /// A teammate handed the item back to the queue: stop without touching its labels.
    pub released: bool,
    /// The conversation so far. The first turn is the task itself; each reply adds one.
    pub turns: Vec<Turn>,
    /// A teammate whose forkdeck announced the same deployment before this one did.
    pub peer: Option<String>,
    /// What each CLI was used for in this job so far: claude, codex, copilot (`journal::slot`).
    pub used: [crate::journal::Usage; 3],
    /// What checks and reviewers objected to, for the repository's notes.
    pub findings: Vec<String>,
    /// Stopped by `limits.job_mins`.
    pub over_time: bool,
    /// Has its run slot and is under way, though its status may still read Queued while the item
    /// is claimed and prepared. Not saved: a restored job starts over.
    pub admitted: bool,
    /// Planning tied to an issue on the forge: the plan is reviewed, then published to the issue
    /// and marked ready for build only when the user says so.
    pub issue_plan: Option<crate::planning::Plan>,
    /// The worker's conversation, once its CLI has named it.
    pub session: Option<Session>,
    /// Set on an interrupted typed task that is to continue: the stage it had reached.
    pub resume_from: Option<JobStatus>,
    /// Messages sent to a running typed task, for the agent once its current step ends.
    pub inbox: VecDeque<String>,
    /// Fired to stop the agent that is running now, so that it takes the first message at once.
    pub steer: Arc<Cancel>,
    /// Whether to propose approaches before working.
    pub planning: Planning,
    /// The approaches proposed, and the one chosen.
    pub plan: Option<Plan>,
    /// Woken when the user chooses.
    pub chose: Arc<tokio::sync::Notify>,
    /// The worker and reviewer as they last ran, when that differs from the first choice (a model
    /// the account refused was replaced). Later turns use these, so the refused model is not asked
    /// again and the conversation they started can continue.
    pub worker_ran: Option<crate::pick::Exec>,
    pub reviewer_ran: Option<crate::pick::Exec>,
    /// The database reserved this job's id, so it may be saved there. A job whose id could not be
    /// reserved is never saved: its number may belong to another job in the history.
    pub in_history: bool,
}

impl Job {
    pub fn stage(&mut self, slot: crate::agents::Slot) -> &mut Stage {
        use crate::agents::Slot;
        match slot {
            Slot::Worker => &mut self.worker,
            Slot::Reviewer => &mut self.reviewer,
            Slot::Extra(i) => {
                if self.extra.len() <= i {
                    self.extra.resize_with(i + 1, Stage::default);
                }
                &mut self.extra[i]
            }
        }
    }
}

pub struct LogLine {
    pub time: String,
    pub who: String,
    pub msg: String,
}

#[derive(Default)]
pub struct RouterStats {
    pub kind: String,
    pub endpoint: String,
    pub calls: u32,
    pub fallbacks: u32,
    pub last_ms: u128,
    pub healthy: Option<bool>,
}

#[derive(Default)]
pub struct RepoStatus {
    pub name: String,
    pub label: String,
    pub last_poll: Option<Instant>,
    pub last_error: Option<String>,
    pub picked: u32,
    /// Why nothing is being picked up from this repository right now, if a limit says so.
    pub hold: Option<String>,
    /// Rules waiting to be proposed are not retried before this.
    pub notes_retry: Option<Instant>,
    /// Where the next poll goes on asking about seen items a full listing left out.
    pub recheck_from: [usize; 2],
}

/// (repo index, is a pull request, number)
pub type Key = (usize, bool, u64);

pub struct Retry {
    /// Retries scheduled so far, including the pending one.
    pub attempts: u32,
    pub not_before: Instant,
    /// No more automatic retries; only removing and re-applying the label starts again.
    pub exhausted: bool,
}

/// How long to wait before retry number `attempt` (1-based): the base, tripling each time.
pub fn backoff(base_secs: u64, attempt: u32) -> Duration {
    Duration::from_secs(base_secs.saturating_mul(3u64.saturating_pow(attempt.saturating_sub(1))).min(24 * 3600))
}

#[derive(Default)]
pub struct State {
    pub jobs: Vec<Job>,
    pub log: VecDeque<LogLine>,
    /// Lines ever logged; the log holds the newest ones.
    pub log_seq: u64,
    pub router: RouterStats,
    pub repos: Vec<RepoStatus>,
    /// Items a job has been started for. An item in here is not taken again unless a retry is due.
    pub seen: HashSet<Key>,
    /// Items that came back without a result (review incomplete, head moved, could not run).
    pub retry: HashMap<Key, Retry>,
    pub automation: bool,
    /// Set once shutdown has begun: nothing new may start.
    pub stopping: bool,
    /// The last thing said about deployments, so it is said once and not on every poll.
    pub deploy_note: Option<String>,
    /// Deployments cancelled in this session (repo index, commit). They are not started again until forkdeck restarts.
    pub deploy_cancelled: HashSet<(usize, String)>,
    /// The agent CLIs found on PATH.
    pub installed: HashSet<AgentKind>,
    /// What copilot said about the login and the account's models.
    pub copilot: crate::copilot::Probe,
    /// Models copilot refused in this session; they are not tried again until restart.
    pub rejected_models: HashSet<String>,
    /// Copilot may play claude or codex when that CLI is missing (agents.copilot.stand_in).
    pub stand_in: bool,
    pub next_id: u32,
    pub team: TeamView,
    /// Finished jobs the journal would not take. They are written as soon as it works again, and
    /// until then nothing more is picked up.
    pub unrecorded: Vec<crate::journal::Entry>,
    /// Used in the last 24 hours, as of the last poll.
    pub spent: crate::journal::Spent,
    /// A CLI that reported a usage limit is passed over until then: claude, codex, copilot.
    pub agent_cooldowns: [Option<Instant>; 3],
}

/// What teammates are working on, as last read from the forge.
#[derive(Default)]
pub struct TeamView {
    pub rows: Vec<crate::team::Row>,
    pub at: Option<Instant>,
    pub error: Option<String>,
}

impl Source {
    pub fn key(&self) -> Option<Key> {
        match self {
            Source::Issue { repo_idx, number, .. } => Some((*repo_idx, false, *number)),
            Source::Pr { repo_idx, number, .. } => Some((*repo_idx, true, *number)),
            Source::Manual | Source::Deploy { .. } => None,
        }
    }
}

impl State {
    pub fn has(&self, k: AgentKind) -> bool {
        self.installed.contains(&k)
    }
    /// Installed and not resting after a usage limit.
    pub fn agent_ready(&self, k: AgentKind) -> bool {
        self.has(k) && self.agent_cooldowns[crate::journal::slot(k)].is_none_or(|until| Instant::now() >= until)
    }
    /// Copilot is installed and the account was not found unable to use it.
    pub fn copilot_usable(&self) -> bool {
        self.has(AgentKind::Copilot) && self.copilot.seat != Some(false)
    }

    pub fn active(&self, key: Key) -> bool {
        self.jobs.iter().any(|j| !j.status.finished() && j.source.key() == Some(key))
    }

    /// A labelled item was listed. Should a job start for it now?
    pub fn should_take(&mut self, key: Key, now: Instant) -> bool {
        if self.seen.insert(key) {
            return true;
        }
        if self.active(key) {
            return false;
        }
        match self.retry.get(&key) {
            Some(r) => !r.exhausted && now >= r.not_before,
            None => false,
        }
    }

    /// The label is gone from everything not in `listed`: forget those items, so that
    /// putting the label back starts from scratch. This is the manual retry.
    pub fn forget_unlisted(&mut self, repo_idx: usize, pr: bool, listed: &[u64]) {
        let gone: Vec<Key> = self
            .seen
            .iter()
            .filter(|k| k.0 == repo_idx && k.1 == pr && !listed.contains(&k.2))
            .copied()
            .filter(|k| !self.active(*k))
            .collect();
        for k in gone {
            self.seen.remove(&k);
            self.retry.remove(&k);
        }
    }

    /// What a retry scheduled now would be: Some(delay), or None when the allowance is used up.
    pub fn retry_preview(&self, key: Key, max: u32, base_secs: u64, immediate: bool) -> Option<Duration> {
        let next = self.retry.get(&key).map(|r| r.attempts).unwrap_or(0) + 1;
        (next <= max).then(|| if immediate { Duration::ZERO } else { backoff(base_secs, next) })
    }

    /// Record that the item needs another go. Returns the delay, or None when no retries are left.
    pub fn schedule_retry(&mut self, key: Key, max: u32, base_secs: u64, immediate: bool, now: Instant) -> Option<Duration> {
        let delay = self.retry_preview(key, max, base_secs, immediate);
        let r = self.retry.entry(key).or_insert(Retry { attempts: 0, not_before: now, exhausted: false });
        r.attempts += 1;
        match delay {
            Some(d) => r.not_before = now + d,
            None => r.exhausted = true,
        }
        delay
    }

    /// Leave the item alone for a while and then look at it again, without using up a retry.
    pub fn look_again(&mut self, key: Key, at: Instant) {
        let r = self.retry.entry(key).or_insert(Retry { attempts: 0, not_before: at, exhausted: false });
        r.not_before = at;
    }

    /// Done with the item: a later label means a new request.
    pub fn forget(&mut self, key: Key) {
        self.seen.remove(&key);
        self.retry.remove(&key);
    }

    /// Stop retrying without forgetting (a cancelled job must not come straight back).
    pub fn give_up(&mut self, key: Key, now: Instant) {
        let r = self.retry.entry(key).or_insert(Retry { attempts: 0, not_before: now, exhausted: true });
        r.exhausted = true;
    }

    pub fn log(&mut self, who: &str, msg: impl Into<String>) {
        let msg: String = msg.into();
        let msg = msg.replace('\n', " ");
        self.log.push_back(LogLine {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            who: who.to_string(),
            msg,
        });
        self.log_seq += 1;
        while self.log.len() > 500 {
            self.log.pop_front();
        }
    }
}

pub fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else if n == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(n - 1).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_cancellation_reason_survives_shutdown() {
        let user = Cancel::new();
        user.fire();
        user.shutdown();
        assert!(user.is_set() && !user.is_shutdown());
        let stopped = Cancel::new();
        stopped.shutdown();
        stopped.fire();
        assert!(stopped.is_shutdown());
    }

    #[test]
    fn retry_policy_is_bounded_and_reset_by_relabelling() {
        let mut st = State::default();
        let k: Key = (0, true, 5);
        let t0 = Instant::now();
        assert!(st.should_take(k, t0), "first sighting starts a job");
        assert!(!st.should_take(k, t0), "and only one");

        // Incomplete: retry 1 after the base delay, not before.
        assert_eq!(st.schedule_retry(k, 2, 60, false, t0), Some(Duration::from_secs(60)));
        assert!(!st.should_take(k, t0 + Duration::from_secs(59)));
        assert!(st.should_take(k, t0 + Duration::from_secs(60)));
        // Retry 2 backs off further; a third is refused.
        assert_eq!(st.schedule_retry(k, 2, 60, false, t0), Some(Duration::from_secs(180)));
        assert_eq!(st.schedule_retry(k, 2, 60, false, t0), None);
        assert!(!st.should_take(k, t0 + Duration::from_secs(100_000)), "no endless polling");

        // Label removed (not listed any more), then applied again: a fresh start.
        st.forget_unlisted(0, true, &[]);
        assert!(st.should_take(k, t0));
        // A head that moved is retried at once.
        assert_eq!(st.schedule_retry(k, 2, 60, true, t0), Some(Duration::ZERO));
        assert!(st.should_take(k, t0));
        // Other repos and issues with the same number are untouched by forget_unlisted.
        st.seen.insert((0, false, 5));
        st.seen.insert((1, true, 5));
        st.forget_unlisted(0, true, &[]);
        assert!(st.seen.contains(&(0, false, 5)) && st.seen.contains(&(1, true, 5)));
    }

    #[test]
    fn a_cancelled_item_does_not_come_back_until_relabelled() {
        let mut st = State::default();
        let k: Key = (0, false, 9);
        let t0 = Instant::now();
        assert!(st.should_take(k, t0));
        st.give_up(k, t0);
        assert!(!st.should_take(k, t0 + Duration::from_secs(10_000)));
        st.forget_unlisted(0, false, &[1, 2]);
        assert!(st.should_take(k, t0));
    }
    #[test]
    fn plan_options_are_read_from_the_last_block() {
        let text = "Here is an example: <options>{\"options\":[{\"title\":\"fake\"}]}</options>\n\nMy proposal:\n<options>\n{\"options\": [{\"title\": \"Small fix\", \"summary\": \"patch it\"}, {\"title\": \"Rewrite\", \"summary\": \"cleaner\", \"recommended\": true}, {\"title\": \"Both\", \"recommended\": true}]}\n</options>";
        let o = parse_options(text).unwrap();
        assert_eq!(o.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["Small fix", "Rewrite", "Both"]);
        assert_eq!(o.iter().map(|o| o.recommended).collect::<Vec<_>>(), [false, true, false], "one recommendation at most");
        let plan = Plan { options: o, text: String::new(), chosen: None };
        assert_eq!(plan.default_choice(), 1);
        assert_eq!(plan.describe(&Choice::Option(0)), "option 1: Small fix");
        for bad in ["no block at all", "<options>not json</options>", "<options>{\"options\":[]}</options>", "<options>{\"options\":[{\"title\":\"x\"}]}"] {
            assert_eq!(parse_options(bad), None, "{}", bad);
        }
    }

    #[test]
    fn agent_refs_round_trip() {
        for s in ["claude", "codex", "copilot", "copilot:second", "copilot:a_b-1"] {
            assert_eq!(AgentRef::parse(s).unwrap().name(), s);
        }
        assert_eq!(AgentRef::parse("copilot:x").unwrap().kind(), AgentKind::Copilot);
        assert_eq!(AgentRef::parse(" codex ").unwrap(), AgentRef::Kind(AgentKind::Codex));
        for bad in ["", "gpt", "copilot:", "copilot:Has Space", "claude:x"] {
            assert!(AgentRef::parse(bad).is_err(), "{:?}", bad);
        }
        #[derive(Deserialize)]
        struct T {
            r: Vec<AgentRef>,
        }
        let t: T = toml::from_str(r#"r = ["claude", "copilot:second"]"#).unwrap();
        assert_eq!(t.r, vec![AgentRef::Kind(AgentKind::Claude), AgentRef::Profile("second".into())]);
        assert!(toml::from_str::<T>(r#"r = ["nope"]"#).is_err());
    }
}
