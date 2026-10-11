use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    pub fn name(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }
    pub fn other(self) -> AgentKind {
        match self {
            AgentKind::Claude => AgentKind::Codex,
            AgentKind::Codex => AgentKind::Claude,
        }
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

#[derive(Default)]
pub struct State {
    pub jobs: Vec<Job>,
    pub log: VecDeque<LogLine>,
    pub router: RouterStats,
    pub repos: Vec<RepoStatus>,
    /// (repo index, is a pull request, number)
    pub seen: HashSet<(usize, bool, u64)>,
    pub automation: bool,
    pub have_claude: bool,
    pub have_codex: bool,
    pub next_id: u32,
}

impl State {
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
