use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobStatus {
    Queued,
    Routing,
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
}

impl JobStatus {
    pub fn name(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Routing => "routing",
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
        }
    }
    pub fn finished(self) -> bool {
        matches!(self, JobStatus::Done | JobStatus::Failed | JobStatus::Cancelled)
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
    pub agent: Option<AgentKind>,
    /// Set when the agent ran on another CLI: "copilot" standing in, or a "copilot:<name>" profile.
    pub via: String,
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
}

impl Default for Stage {
    fn default() -> Self {
        Stage {
            agent: None,
            via: String::new(),
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
        }
    }
}

impl Stage {
    pub fn push(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.last = line.clone();
        self.tail.push_back(line);
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
#[derive(Clone, Debug, Serialize)]
pub struct Gate {
    pub name: String,
    pub answer: String,
    pub value: f64,
    pub sharp: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub agent: AgentKind,
    pub tier: Tier,
    pub review: bool,
    pub gates: Vec<Gate>,
    pub source: String,
    pub ms: u128,
    pub note: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
}

#[derive(Clone, Debug)]
pub enum Source {
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
    flag: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Cancel {
    pub fn new() -> Arc<Cancel> {
        Arc::new(Cancel { flag: AtomicBool::new(false), notify: tokio::sync::Notify::new() })
    }
    pub fn fire(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
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

pub struct Job {
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
    pub forced: Option<AgentKind>,
    pub decision: Option<Decision>,
    pub worker: Stage,
    pub reviewer: Stage,
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
    pub have_claude: bool,
    pub have_codex: bool,
    pub next_id: u32,
    pub team: TeamView,
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
    fn active(&self, key: Key) -> bool {
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
