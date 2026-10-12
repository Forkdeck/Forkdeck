use crate::state::{AgentKind, AgentRef, Provider, Tier};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const EXAMPLE: &str = include_str!("../forkdeck.example.toml");

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub router: RouterCfg,
    pub agents: AgentsCfg,
    pub review: ReviewCfg,
    pub automation: AutoCfg,
    pub team: TeamCfg,
    /// MCP servers by name. CI and deployment stages call tools on these.
    pub mcp: HashMap<String, McpServerCfg>,
    pub repos: Vec<RepoCfg>,
    #[serde(skip)]
    pub demo: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RouterCfg {
    /// "rules" or "jev"
    pub kind: String,
    pub default_agent: AgentKind,
    /// Answers below this confidence are "split": the safer branch is taken.
    pub threshold: f64,
    /// "auto": the decision server may ask for a plan first on open-ended tasks; "never": it is
    /// not asked. Ctrl-P and `run --plan` ask for one either way.
    pub plan: String,
    pub jev: JevCfg,
    pub rules: RulesCfg,
}

impl Default for RouterCfg {
    fn default() -> Self {
        RouterCfg {
            kind: "rules".into(),
            default_agent: AgentKind::Claude,
            threshold: 0.6,
            plan: "auto".into(),
            jev: JevCfg::default(),
            rules: RulesCfg::default(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct JevCfg {
    pub base_url: String,
    pub model: String,
    /// Name of the environment variable holding the bearer key. Empty or unset: no auth header.
    pub api_key_env: String,
    pub timeout_ms: u64,
    pub claude_is_for: String,
    pub codex_is_for: String,
}

impl Default for JevCfg {
    fn default() -> Self {
        JevCfg {
            base_url: "http://127.0.0.1:8000".into(),
            model: "jev-latest".into(),
            api_key_env: "FORKDECK_JEV_KEY".into(),
            timeout_ms: 4000,
            claude_is_for: "Open-ended or ambiguous work: multi-file refactors, design changes, documentation".into(),
            codex_is_for: "Well-specified work: a bug with a clear reproduction, a contained feature, tests, scripts".into(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RulesCfg {
    pub claude_keywords: Vec<String>,
    pub codex_keywords: Vec<String>,
    pub heavy_keywords: Vec<String>,
    pub light_keywords: Vec<String>,
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| w.to_string()).collect()
}

impl Default for RulesCfg {
    fn default() -> Self {
        RulesCfg {
            claude_keywords: words("refactor design architecture document docs readme explain plan restructure"),
            codex_keywords: words("bug fix test tests script failing crash regression implement endpoint"),
            heavy_keywords: words("architecture migrate migration concurrency race security rewrite redesign deadlock distributed"),
            light_keywords: words("typo rename comment bump format lint wording spelling"),
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsCfg {
    pub claude: AgentCfg,
    pub codex: AgentCfg,
    pub copilot: CopilotCfg,
    pub timeout_mins: Option<u64>,
}

/// `[agents.copilot]`: the CLI's own settings plus the profiles it can run.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CopilotCfg {
    pub cmd: Option<String>,
    /// Models for `copilot` itself (forced with --agent copilot). May be "auto".
    pub models: Option<Tiers>,
    pub efforts: Option<Tiers>,
    pub write_args: Option<Vec<String>>,
    pub review_args: Option<Vec<String>>,
    /// Run the `claude` and `codex` profiles on copilot when that CLI is not installed.
    pub stand_in: Option<bool>,
    pub profiles: HashMap<String, ProfileCfg>,
}

impl CopilotCfg {
    pub fn stand_in(&self) -> bool {
        self.stand_in.unwrap_or(true)
    }
}

/// A set of models copilot runs under one name, used as `copilot:<name>`.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileCfg {
    /// Needed when the model IDs do not say it (claude-* is anthropic, gpt-* and o<digit>* openai).
    pub vendor: Option<String>,
    pub models: Option<TierLists>,
    pub efforts: Option<Tiers>,
}

/// A model preference list per tier: the first one the account allows is used.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct TierLists {
    pub light: Option<OneOrMany>,
    pub standard: Option<OneOrMany>,
    pub heavy: Option<OneOrMany>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

impl TierLists {
    pub fn get(&self, t: Tier) -> Vec<String> {
        match t {
            Tier::Light => &self.light,
            Tier::Standard => &self.standard,
            Tier::Heavy => &self.heavy,
        }
        .as_ref()
        .map(|l| l.to_vec())
        .unwrap_or_default()
    }
    fn of(l: &[&str], s: &[&str], h: &[&str]) -> TierLists {
        let m = |v: &[&str]| Some(OneOrMany::Many(v.iter().map(|x| x.to_string()).collect()));
        TierLists { light: m(l), standard: m(s), heavy: m(h) }
    }
    fn over(self, d: TierLists) -> TierLists {
        TierLists { light: self.light.or(d.light), standard: self.standard.or(d.standard), heavy: self.heavy.or(d.heavy) }
    }
    fn all(&self) -> Vec<String> {
        [Tier::Light, Tier::Standard, Tier::Heavy].into_iter().flat_map(|t| self.get(t)).collect()
    }
}

/// The stand-in profiles. IDs follow GitHub's documented pattern (gpt-5.4, claude-haiku-4.5);
/// which of them an account may use is checked when they run, and a missing one is skipped.
fn builtin_profile(name: &str) -> Option<ProfileCfg> {
    let models = match name {
        "claude" => TierLists::of(
            &["claude-haiku-5.5", "claude-haiku-4.5"],
            &["claude-sonnet-5.5", "claude-sonnet-5", "claude-sonnet-4.6", "claude-sonnet-4.5"],
            &["claude-opus-5.5", "claude-opus-5", "claude-opus-4.8"],
        ),
        "codex" => TierLists::of(
            &["gpt-5.4-mini", "gpt-5-mini", "gpt-5.3-codex"],
            &["gpt-5.5", "gpt-5.4", "gpt-5.3-codex"],
            &["gpt-5.5", "gpt-5.4", "gpt-5.3-codex"],
        ),
        _ => return None,
    };
    Some(ProfileCfg { vendor: None, models: Some(models), efforts: Tiers::of("low", "medium", "high") })
}

impl AgentsCfg {
    pub fn get(&self, k: AgentKind) -> AgentCfg {
        let (cfg, def) = match k {
            AgentKind::Claude => (self.claude.clone(), AgentCfg::claude()),
            AgentKind::Codex => (self.codex.clone(), AgentCfg::codex()),
            AgentKind::Copilot => {
                let c = &self.copilot;
                let cfg = AgentCfg { cmd: c.cmd.clone(), models: c.models.clone(), efforts: c.efforts.clone(), write_args: c.write_args.clone(), review_args: c.review_args.clone() };
                (cfg, AgentCfg::copilot())
            }
        };
        AgentCfg {
            cmd: cfg.cmd.or(def.cmd),
            models: Some(cfg.models.unwrap_or_default().over(def.models)),
            efforts: Some(cfg.efforts.unwrap_or_default().over(def.efforts)),
            write_args: cfg.write_args.or(def.write_args),
            review_args: cfg.review_args.or(def.review_args),
        }
    }

    /// A copilot profile, the built-in values under the user's, or None if there is no such profile.
    pub fn profile(&self, name: &str) -> Option<ProfileCfg> {
        let user = self.copilot.profiles.get(name).cloned();
        match (user, builtin_profile(name)) {
            (None, None) => None,
            (Some(u), None) => Some(u),
            (None, Some(b)) => Some(b),
            (Some(u), Some(b)) => Some(ProfileCfg {
                vendor: u.vendor.or(b.vendor),
                models: Some(u.models.unwrap_or_default().over(b.models.unwrap_or_default())),
                efforts: Some(u.efforts.unwrap_or_default().over(b.efforts)),
            }),
        }
    }
}

/// A value per effort tier. A tier left out of the config keeps its built-in value.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Tiers {
    pub light: Option<String>,
    pub standard: Option<String>,
    pub heavy: Option<String>,
}

impl Tiers {
    pub fn get(&self, t: Tier) -> &str {
        match t {
            Tier::Light => self.light.as_deref(),
            Tier::Standard => self.standard.as_deref(),
            Tier::Heavy => self.heavy.as_deref(),
        }
        .unwrap_or("")
    }
    fn of(l: &str, s: &str, h: &str) -> Option<Tiers> {
        Some(Tiers { light: Some(l.into()), standard: Some(s.into()), heavy: Some(h.into()) })
    }
    fn over(self, def: Option<Tiers>) -> Tiers {
        let d = def.unwrap_or_default();
        Tiers { light: self.light.or(d.light), standard: self.standard.or(d.standard), heavy: self.heavy.or(d.heavy) }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentCfg {
    pub cmd: Option<String>,
    /// Model per tier. An empty string leaves the CLI's own default in place.
    pub models: Option<Tiers>,
    pub efforts: Option<Tiers>,
    /// Extra arguments when the agent may edit the working tree.
    pub write_args: Option<Vec<String>>,
    /// Extra arguments when the agent only reviews.
    pub review_args: Option<Vec<String>>,
}

fn strs(v: &[&str]) -> Option<Vec<String>> {
    Some(v.iter().map(|s| s.to_string()).collect())
}

impl AgentCfg {
    fn claude() -> AgentCfg {
        AgentCfg {
            cmd: Some("claude".into()),
            models: Tiers::of("haiku", "sonnet", "opus"),
            efforts: Tiers::of("low", "medium", "high"),
            write_args: strs(&["--permission-mode", "acceptEdits"]),
            review_args: strs(&["--permission-mode", "plan"]),
        }
    }
    fn codex() -> AgentCfg {
        AgentCfg {
            cmd: Some("codex".into()),
            models: Tiers::of("", "", ""),
            efforts: Tiers::of("low", "medium", "high"),
            write_args: strs(&["--sandbox", "workspace-write"]),
            review_args: strs(&["--sandbox", "read-only"]),
        }
    }
    fn copilot() -> AgentCfg {
        AgentCfg {
            cmd: Some("copilot".into()),
            models: Tiers::of("", "", ""),
            efforts: Tiers::of("low", "medium", "high"),
            // Like claude's acceptEdits: edit files, no shell commands unattended.
            write_args: strs(&["--allow-tool", "write", "--deny-tool", "shell"]),
            // Skills load even with --no-custom-instructions, so a reviewer does not get them.
            review_args: strs(&["--deny-tool", "write", "--deny-tool", "shell", "--excluded-tools", "skill"]),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewCfg {
    /// "auto" (router decides), "always" or "never"
    pub mode: String,
    /// "other" (the CLI that did not do the work) or "same"
    pub reviewer: String,
    /// Who reviews when `reviewer` is "other": the first one that is available and runs a model
    /// from a different vendor than the worker's.
    pub reviewer_order: Vec<AgentRef>,
    /// How many times a failed review or failed checks go back to the worker. It keeps fixing
    /// until the review passes; this only caps runaway loops.
    pub max_fix_rounds: u32,
    /// Shell commands run after the worker finishes a typed task. Repositories have their own list.
    pub checks: Vec<String>,
    pub check_timeout_mins: u64,
    /// Which agents review a labelled pull request. Each one reviews independently.
    pub pr_reviewers: Vec<AgentRef>,
    /// How often a pull request review that produced no result (a reviewer did not finish, the
    /// head moved, the review could not run) is retried automatically.
    pub max_retries: u32,
    /// Wait before the first such retry; it triples for each later one.
    pub retry_secs: u64,
}

impl Default for ReviewCfg {
    fn default() -> Self {
        ReviewCfg {
            mode: "auto".into(),
            reviewer: "other".into(),
            reviewer_order: vec![AgentRef::Kind(AgentKind::Claude), AgentRef::Kind(AgentKind::Codex)],
            max_fix_rounds: 5,
            checks: vec![],
            check_timeout_mins: 20,
            pr_reviewers: vec![AgentRef::Kind(AgentKind::Claude), AgentRef::Kind(AgentKind::Codex)],
            max_retries: 3,
            retry_secs: 120,
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct AutoCfg {
    /// Start with issue polling switched on.
    pub enabled: bool,
    pub max_parallel: usize,
    pub poll_secs: u64,
    pub worktree_root: Option<PathBuf>,
    pub draft: bool,
    /// Open a pull request even when the review did not pass.
    pub publish_on_failed_review: bool,
}

impl Default for AutoCfg {
    fn default() -> Self {
        AutoCfg {
            enabled: false,
            max_parallel: 2,
            poll_secs: 60,
            worktree_root: None,
            draft: true,
            publish_on_failed_review: false,
        }
    }
}

/// Sharing what this machine is working on with teammates, through comments on the forge.
#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct TeamCfg {
    /// Claim issues and reviews with a status comment, and refuse what a teammate already holds.
    pub enabled: bool,
    /// A claim not updated for this long is shown as stale and no longer blocks anyone.
    pub stale_mins: u64,
    /// How often a running job refreshes its status comment.
    pub heartbeat_secs: u64,
    /// How often the team pane re-reads the forge.
    pub refresh_secs: u64,
    /// Machine name shown to teammates. Defaults to the hostname on private repositories and to
    /// nothing on public ones. A name set here is used everywhere; "" shows none anywhere.
    pub host: Option<String>,
    /// Assign a claimed issue to the logged-in account.
    pub assign: bool,
    /// Announce deployments with a comment on the commit (GitHub only).
    pub deploys: bool,
}

impl Default for TeamCfg {
    fn default() -> Self {
        TeamCfg { enabled: false, stale_mins: 30, heartbeat_secs: 300, refresh_secs: 120, host: None, assign: true, deploys: true }
    }
}

impl AutoCfg {
    pub fn root(&self) -> PathBuf {
        if let Some(p) = &self.worktree_root {
            return expand(p);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".cache/forkdeck/worktrees")
    }
}

/// How to reach one MCP server: a command to run (stdio) or a URL (Streamable HTTP).
/// `${NAME}` in args, env values, the url and header values is replaced from the environment.
#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerCfg {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
    pub headers: HashMap<String, String>,
    /// Longest wait for one answer.
    pub timeout_secs: u64,
}

impl Default for McpServerCfg {
    fn default() -> Self {
        McpServerCfg { command: None, args: vec![], env: HashMap::new(), url: None, headers: HashMap::new(), timeout_secs: 60 }
    }
}

/// One tool call and how to read its answer.
///
/// `args` may contain placeholders such as `{owner}`, `{name}`, `{repo}`, `{branch}`, `{base}`,
/// `{sha}`, `{pr}`, `{issue}`, `{env}`, `{last_good}` and `{message}`. A value that is nothing but
/// a numeric placeholder (`"{pr}"`) is sent as a number.
///
/// The answer is read as text. `failure`, `pending` and `success` are regular expressions tried in
/// that order; an answer none of them matches counts as pending. A tool error is a failure.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CallCfg {
    /// Defaults to the stage's server.
    #[serde(default)]
    pub server: Option<String>,
    pub tool: String,
    #[serde(default = "empty_table")]
    pub args: toml::Value,
    #[serde(default)]
    pub success: Option<String>,
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub pending: Option<String>,
    /// Optional: a regular expression whose first group is the revision the answer describes.
    /// Success then also requires that revision to be the commit being waited for.
    #[serde(default)]
    pub revision: Option<String>,
    /// Optional, for a deployment status: the deployment system has stopped the rollout and put the
    /// old version back by itself (an aborted canary, a failed analysis). The deployment has failed,
    /// and forkdeck's own rollback tool is not called on top of it.
    #[serde(default)]
    pub reverted: Option<String>,
    /// Optional, for a deployment status: the rollout is waiting for a person (a manual promotion
    /// or an approval gate). Time spent like this does not count against `timeout_mins`.
    #[serde(default)]
    pub paused: Option<String>,
    /// Optional: a regular expression whose first group says how far a rollout has got, such as
    /// "canary 20%" or a region. Shown while waiting; it decides nothing.
    #[serde(default)]
    pub progress: Option<String>,
    /// Optional: values to take from the answer for later calls in the same stage, such as the id of
    /// the failing run for the log call. Name = regular expression; the first group is the value,
    /// usable as `{name}`.
    #[serde(default)]
    pub capture: HashMap<String, String>,
}

fn empty_table() -> toml::Value {
    toml::Value::Table(Default::default())
}

/// Watch CI on a pull request forkdeck opened, and send failures back to the worker.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CiCfg {
    pub server: String,
    #[serde(default = "d_30")]
    pub interval_secs: u64,
    #[serde(default = "d_30")]
    pub timeout_mins: u64,
    /// How many times a CI failure is sent back to the worker before a person is asked.
    #[serde(default = "d_1")]
    pub max_fix_rounds: u32,
    /// A failure reported within this many seconds, before the pipeline was seen running, may be
    /// the previous run's and is not believed yet. Not used when `status.revision` is set.
    #[serde(default = "d_60")]
    pub settle_secs: u64,
    pub status: CallCfg,
    /// Optional: fetch the failing output to show the worker. Without it the status answer is used.
    #[serde(default)]
    pub logs: Option<CallCfg>,
}

/// Deploy the base branch when it moves, watch the result, and roll back if it goes wrong.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DeployCfg {
    pub server: String,
    #[serde(default = "d_staging")]
    pub environment: String,
    #[serde(default = "d_15")]
    pub interval_secs: u64,
    #[serde(default = "d_15")]
    pub timeout_mins: u64,
    /// Optional: start the deployment. Leave out when merging already deploys (GitOps auto-sync).
    #[serde(default)]
    pub trigger: Option<CallCfg>,
    pub status: CallCfg,
    /// How long a rollout may sit waiting for a person (see `status.paused`) before forkdeck stops
    /// watching it. Nothing is rolled back when it does.
    #[serde(default = "d_day")]
    pub pause_timeout_mins: u64,
    /// Shell commands run after the deployment reports healthy, such as a smoke test.
    #[serde(default)]
    pub verify: Vec<String>,
    /// Optional: undo a failed deployment. `{last_good}` is the last commit that deployed cleanly.
    #[serde(default)]
    pub rollback: Option<CallCfg>,
    /// Optional: tell someone how it went. `{message}` is a one-line result.
    #[serde(default)]
    pub notify: Option<CallCfg>,
}

fn d_30() -> u64 {
    30
}
fn d_day() -> u64 {
    24 * 60
}
fn d_60() -> u64 {
    60
}
fn d_15() -> u64 {
    15
}
fn d_1() -> u32 {
    1
}
fn d_staging() -> String {
    "staging".into()
}

impl CallCfg {
    pub fn patterns(&self) -> [(&'static str, &Option<String>); 7] {
        [
            ("success", &self.success),
            ("failure", &self.failure),
            ("pending", &self.pending),
            ("revision", &self.revision),
            ("reverted", &self.reverted),
            ("paused", &self.paused),
            ("progress", &self.progress),
        ]
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RepoCfg {
    pub provider: Provider,
    /// "owner/name" on GitHub, "group/project" on GitLab
    pub repo: String,
    /// Local clone that worktrees are created from
    pub path: PathBuf,
    #[serde(default = "d_base")]
    pub base: String,
    #[serde(default = "d_ready")]
    pub label: String,
    #[serde(default = "d_working")]
    pub working_label: String,
    #[serde(default = "d_review")]
    pub done_label: String,
    #[serde(default = "d_failed")]
    pub failed_label: String,
    /// Only pick up issues and pull requests opened by these accounts. Empty: anyone.
    #[serde(default)]
    pub allowed_authors: Vec<String>,
    /// Shell commands run in the worktree before review, for example the test suite.
    #[serde(default)]
    pub checks: Vec<String>,
    /// Pull requests carrying this label get reviewed by every CLI in review.pr_reviewers.
    #[serde(default = "d_pr_ready")]
    pub pr_label: String,
    #[serde(default = "d_pr_running")]
    pub pr_working_label: String,
    #[serde(default = "d_pr_pass")]
    pub pr_pass_label: String,
    #[serde(default = "d_pr_fail")]
    pub pr_fail_label: String,
    /// Run `checks` on labelled pull requests too. Off by default: it executes the pull request's code as you.
    #[serde(default)]
    pub pr_checks: bool,
    #[serde(default)]
    pub ci: Option<CiCfg>,
    #[serde(default)]
    pub deploy: Option<DeployCfg>,
}

fn d_base() -> String {
    "main".into()
}
fn d_ready() -> String {
    "agent:ready".into()
}
fn d_working() -> String {
    "agent:working".into()
}
fn d_review() -> String {
    "agent:pr-open".into()
}
fn d_pr_ready() -> String {
    "review:ready".into()
}
fn d_pr_running() -> String {
    "review:running".into()
}
fn d_pr_pass() -> String {
    "review:passed".into()
}
fn d_pr_fail() -> String {
    "review:changes".into()
}
fn d_failed() -> String {
    "agent:needs-human".into()
}

pub fn expand(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Ok(home) = std::env::var("HOME") {
        if s == "~" {
            return PathBuf::from(home);
        }
        if let Some(rest) = s.strip_prefix("~/") {
            return PathBuf::from(home).join(rest);
        }
    }
    p.to_path_buf()
}

pub fn user_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/forkdeck/config.toml")
}

pub fn load(explicit: Option<&Path>) -> anyhow::Result<(Config, Option<PathBuf>)> {
    // Never read a config from the working directory: agents edit files there, and the config names commands to run.
    let candidates: Vec<PathBuf> = match explicit {
        Some(p) => vec![p.to_path_buf()],
        None => vec![user_path()],
    };
    for c in candidates {
        if c.exists() {
            let text = std::fs::read_to_string(&c)?;
            let mut cfg: Config =
                toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {}", c.display(), e))?;
            for r in &mut cfg.repos {
                r.path = expand(&r.path);
            }
            cfg.validate()?;
            return Ok((cfg, Some(c)));
        } else if explicit.is_some() {
            anyhow::bail!("config file {} not found", c.display());
        }
    }
    Ok((Config::default(), None))
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !matches!(self.router.kind.as_str(), "rules" | "jev") {
            anyhow::bail!("router.kind must be \"rules\" or \"jev\", got {:?}", self.router.kind);
        }
        if !matches!(self.review.mode.as_str(), "auto" | "always" | "never") {
            anyhow::bail!("review.mode must be auto, always or never");
        }
        if !matches!(self.review.reviewer.as_str(), "other" | "same") {
            anyhow::bail!("review.reviewer must be other or same");
        }
        if !matches!(self.router.plan.as_str(), "auto" | "never") {
            anyhow::bail!("router.plan must be auto or never");
        }
        if !(0.5..=1.0).contains(&self.router.threshold) {
            anyhow::bail!("router.threshold must be between 0.5 and 1.0, got {}", self.router.threshold);
        }
        if self.router.jev.timeout_ms < 100 {
            anyhow::bail!("router.jev.timeout_ms must be at least 100");
        }
        if let Some(m) = self.agents.timeout_mins {
            if !(1..=24 * 60).contains(&m) {
                anyhow::bail!("agents.timeout_mins must be between 1 and 1440, got {}", m);
            }
        }
        if !(30..=3600).contains(&self.team.heartbeat_secs) {
            anyhow::bail!("team.heartbeat_secs must be between 30 and 3600, got {}", self.team.heartbeat_secs);
        }
        if self.team.stale_mins * 60 < self.team.heartbeat_secs * 3 || self.team.stale_mins > 7 * 24 * 60 {
            anyhow::bail!("team.stale_mins must cover at least three heartbeats and at most a week, got {}", self.team.stale_mins);
        }
        if self.team.refresh_secs < 30 {
            anyhow::bail!("team.refresh_secs must be at least 30");
        }
        if !(1..=24 * 60).contains(&self.review.check_timeout_mins) {
            anyhow::bail!("review.check_timeout_mins must be between 1 and 1440");
        }
        for k in AgentKind::ALL {
            if self.agents.get(k).cmd.unwrap_or_default().trim().is_empty() {
                anyhow::bail!("agents.{}.cmd must not be empty", k.name());
            }
        }
        if self.router.default_agent == AgentKind::Copilot {
            anyhow::bail!("router.default_agent must be claude or codex");
        }
        self.validate_profiles()?;
        // Two stages deploying the same repository to the same environment would each wait on, and
        // roll back, the other's work.
        let mut targets = std::collections::HashSet::new();
        for r in &self.repos {
            if let Some(d) = &r.deploy {
                let key = format!("{:?}:{}:{}", r.provider, r.repo, d.environment).to_lowercase();
                anyhow::ensure!(targets.insert(key), "repos: {} has more than one deploy stage for {:?}", r.repo, d.environment);
            }
        }
        for (name, m) in &self.mcp {
            if m.command.is_some() == m.url.is_some() {
                anyhow::bail!("mcp.{}: set exactly one of command and url", name);
            }
        }
        for r in &self.repos {
            let mut calls: Vec<(String, &str, &CallCfg)> = Vec::new();
            if let Some(ci) = &r.ci {
                anyhow::ensure!(ci.interval_secs >= 1 && ci.timeout_mins >= 1, "repos.ci for {}: interval_secs and timeout_mins must be at least 1", r.repo);
                calls.push(("ci.status".into(), &ci.server, &ci.status));
                calls.extend(ci.logs.iter().map(|c| ("ci.logs".to_string(), ci.server.as_str(), c)));
                anyhow::ensure!(ci.status.success.is_some(), "repos.ci.status for {}: a success pattern is required", r.repo);
                // "Passed" has to mean this commit passed. Either the answer says which commit it is
                // about, or the question names the commit. A result for the previous push must not count.
                anyhow::ensure!(
                    ci.status.revision.is_some() || ci.status.args.to_string().contains("{sha}"),
                    "repos.ci.status for {}: set a revision pattern, or pass {{sha}} in args so the tool is asked about the exact commit. Otherwise a result from the previous push could be taken for the new one",
                    r.repo
                );
            }
            if let Some(d) = &r.deploy {
                anyhow::ensure!(d.interval_secs >= 1 && d.timeout_mins >= 1, "repos.deploy for {}: interval_secs and timeout_mins must be at least 1", r.repo);
                // A name is all there is to go on, so this is a guard against accidents, not a control.
                if d.environment.to_lowercase().contains("prod") {
                    anyhow::bail!("repos.deploy for {}: environment {:?} looks like production. Unattended production deployments are not supported.", r.repo, d.environment);
                }
                calls.push(("deploy.status".into(), &d.server, &d.status));
                anyhow::ensure!(d.status.success.is_some(), "repos.deploy.status for {}: a success pattern is required", r.repo);
                // Without it, "healthy" from a system still running the previous commit would be recorded
                // as this commit deployed, and would become the target of the next rollback.
                anyhow::ensure!(
                    d.status.revision.is_some(),
                    "repos.deploy.status for {}: a revision pattern is required, so that an answer about the previous commit is not taken for this one",
                    r.repo
                );
                for (n, c) in [("deploy.trigger", &d.trigger), ("deploy.rollback", &d.rollback), ("deploy.notify", &d.notify)] {
                    calls.extend(c.iter().map(|c| (n.to_string(), d.server.as_str(), c)));
                }
            }
            for (what, stage_server, c) in calls {
                let server = c.server.as_deref().unwrap_or(stage_server);
                anyhow::ensure!(self.mcp.contains_key(server), "repos.{} for {}: no [mcp.{}] server is configured", what, r.repo, server);
                anyhow::ensure!(!c.tool.trim().is_empty(), "repos.{} for {}: tool must not be empty", what, r.repo);
                anyhow::ensure!(c.args.is_table(), "repos.{} for {}: args must be a table", what, r.repo);
                for (pname, p) in c.patterns() {
                    if let Some(p) = p {
                        regex::Regex::new(p).map_err(|e| anyhow::anyhow!("repos.{}.{} for {}: {}", what, pname, r.repo, e))?;
                    }
                }
                for (cname, p) in &c.capture {
                    let re = regex::Regex::new(p).map_err(|e| anyhow::anyhow!("repos.{}.capture.{} for {}: {}", what, cname, r.repo, e))?;
                    anyhow::ensure!(re.captures_len() >= 2, "repos.{}.capture.{} for {}: the pattern needs a group for the value", what, cname, r.repo);
                }
                if let Some(p) = &c.progress {
                    anyhow::ensure!(regex::Regex::new(p).map(|re| re.captures_len() >= 2).unwrap_or(false), "repos.{}.progress for {}: the pattern needs a group for what to show", what, r.repo);
                }
                if let Some(p) = &c.revision {
                    anyhow::ensure!(regex::Regex::new(p).map(|re| re.captures_len() >= 2).unwrap_or(false), "repos.{}.revision for {}: the pattern needs a group for the commit", what, r.repo);
                }
            }
            if !r.path.is_absolute() {
                anyhow::bail!("repos: path for {} must be absolute or start with ~/, got {}", r.repo, r.path.display());
            }
            for (name, v) in [("repo", &r.repo), ("base", &r.base)] {
                if v.is_empty() || v.starts_with('-') || v.contains(char::is_whitespace) {
                    anyhow::bail!("repos: {} {:?} is not a valid name", name, v);
                }
            }
        }
        Ok(())
    }

    fn validate_profiles(&self) -> anyhow::Result<()> {
        use crate::pick::{vendor_of_model, Vendor};
        let mut names: Vec<String> = self.agents.copilot.profiles.keys().cloned().collect();
        names.sort();
        for b in ["claude", "codex"] {
            if !names.iter().any(|n| n == b) {
                names.push(b.to_string());
            }
        }
        for name in &names {
            if AgentRef::parse(&format!("copilot:{}", name)).is_err() {
                anyhow::bail!("agents.copilot.profiles.{:?}: profile names use a-z, 0-9, - and _", name);
            }
            let p = self.agents.profile(name).expect("listed above");
            let models = p.models.clone().unwrap_or_default();
            if models.all().is_empty() {
                anyhow::bail!("agents.copilot.profiles.{}: list at least one model", name);
            }
            let want = match name.as_str() {
                "claude" => Some(Vendor::Anthropic),
                "codex" => Some(Vendor::OpenAI),
                _ => None,
            };
            for m in models.all() {
                if m.trim().is_empty() || m == "auto" {
                    anyhow::bail!("agents.copilot.profiles.{}: name the models; \"auto\" could pick any vendor's", name);
                }
                let v = vendor_of_model(&m);
                match &want {
                    Some(w) if &v != w => anyhow::bail!("agents.copilot.profiles.{}: {} is not a {} model", name, m, name),
                    None if v == Vendor::Unknown && p.vendor.as_deref().unwrap_or("").trim().is_empty() => {
                        anyhow::bail!("agents.copilot.profiles.{}: set vendor, the name {} does not say whose model it is", name, m)
                    }
                    _ => {}
                }
            }
        }
        for r in self.review.pr_reviewers.iter().chain(&self.review.reviewer_order) {
            if let AgentRef::Profile(p) = r {
                if self.agents.profile(p).is_none() {
                    anyhow::bail!("{} is not defined under [agents.copilot.profiles]", r.name());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let cfg: Config = toml::from_str(EXAMPLE).expect("example parses");
        cfg.validate().unwrap();
        // What `init` writes has to behave like having no config at all.
        assert_eq!(cfg.router.kind, "rules");
        assert!(cfg.repos.is_empty());
        assert!(!cfg.automation.enabled);
        assert_eq!(cfg.agents.get(AgentKind::Claude).models.unwrap().get(Tier::Heavy), "opus");
        assert_eq!(cfg.agents.get(AgentKind::Codex).cmd.unwrap(), "codex");
    }

    #[test]
    fn commented_examples_parse_once_uncommented() {
        // The jev and [[repos]] blocks ship commented out; make sure they are still valid.
        let text: String = EXAMPLE.lines().map(|l| l.strip_prefix("#~ ").unwrap_or(l)).collect::<Vec<_>>().join("\n");
        let text = text.replace("kind = \"rules\"", "kind = \"jev\"");
        let mut cfg: Config = toml::from_str(&text).expect("uncommented example parses");
        for r in &mut cfg.repos {
            r.path = expand(&r.path);
        }
        cfg.validate().unwrap();
        assert_eq!(cfg.repos.len(), 2);
    }

    #[test]
    fn partial_tier_table_keeps_the_other_defaults() {
        let cfg: Config = toml::from_str("[agents.claude]\nmodels = { heavy = \"big\" }").unwrap();
        let m = cfg.agents.get(AgentKind::Claude).models.unwrap();
        assert_eq!((m.get(Tier::Light), m.get(Tier::Heavy)), ("haiku", "big"));
    }

    const WITH_STAGES: &str = r#"
[mcp.ci]
command = "ci-mcp"
[mcp.cd]
url = "http://127.0.0.1:3000/mcp"
headers = { Authorization = "Bearer ${TOKEN}" }
[[repos]]
provider = "github"
repo = "o/r"
path = "/src/r"
[repos.ci]
server = "ci"
[repos.ci.status]
tool = "status"
args = { owner = "{owner}", pr = "{pr}", nested = { ref = "{branch}", commit = "{sha}" } }
success = "ok"
failure = "bad"
[repos.deploy]
server = "cd"
ENV
[repos.deploy.status]
tool = "get_application"
success = "Healthy"
revision = 'rev: ([0-9a-f]+)'
[repos.deploy.rollback]
server = "ci"
tool = "rollback"
"#;

    #[test]
    fn stages_parse_and_production_is_refused() {
        let cfg: Config = toml::from_str(&WITH_STAGES.replace("ENV", "")).unwrap();
        cfg.validate().unwrap();
        let r = &cfg.repos[0];
        assert_eq!(r.ci.as_ref().unwrap().max_fix_rounds, 1);
        assert_eq!(r.deploy.as_ref().unwrap().environment, "staging");
        for env in ["production", "Prod-EU"] {
            let cfg: Config = toml::from_str(&WITH_STAGES.replace("ENV", &format!("environment = \"{}\"", env))).unwrap();
            assert!(cfg.validate().unwrap_err().to_string().contains("production"), "{}", env);
        }
        let cfg: Config = toml::from_str(&WITH_STAGES.replace("ENV", "").replace("server = \"cd\"", "server = \"nope\"")).unwrap();
        assert!(cfg.validate().unwrap_err().to_string().contains("[mcp.nope]"));
        let cfg: Config = toml::from_str(&WITH_STAGES.replace("ENV", "").replace("success = \"ok\"", "success = \"(\"")).unwrap();
        assert!(cfg.validate().is_err(), "a pattern that does not compile is rejected");
        let cfg: Config = toml::from_str("[mcp.x]\ncommand = \"a\"\nurl = \"b\"").unwrap();
        assert!(cfg.validate().is_err());
        // A CI status that neither says nor is asked which commit it is about could pass on an old result.
        let cfg: Config = toml::from_str(&WITH_STAGES.replace("ENV", "").replace(", commit = \"{sha}\"", "")).unwrap();
        assert!(cfg.validate().unwrap_err().to_string().contains("previous push"));
        let cfg: Config = toml::from_str(&WITH_STAGES.replace("ENV", "").replace(", commit = \"{sha}\"", "").replace("failure = \"bad\"", "failure = \"bad\"\nrevision = 'sha=([0-9a-f]+)'")).unwrap();
        cfg.validate().unwrap();
        let twice = WITH_STAGES.replace("ENV", "");
        let twice = format!("{}{}", twice, &twice[twice.find("[[repos]]").unwrap()..]);
        let cfg: Config = toml::from_str(&twice).unwrap();
        assert!(cfg.validate().unwrap_err().to_string().contains("more than one deploy stage"));
        // A deploy status that cannot say which commit it is about would record commits as deployed that never were.
        let cfg: Config = toml::from_str(&WITH_STAGES.replace("ENV", "").replace("revision = 'rev: ([0-9a-f]+)'", "")).unwrap();
        assert!(cfg.validate().unwrap_err().to_string().contains("revision pattern is required"));
    }

    #[test]
    fn rejects_bad_values() {
        for bad in ["[router]\nthreshold = 7", "[agents]\ntimeout_mins = 0", "[agents.claude]\ncmd = \"\"",
            "[[repos]]\nprovider = \"github\"\nrepo = \"o/r\"\npath = \"relative\""] {
            let cfg: Config = toml::from_str(bad).unwrap();
            assert!(cfg.validate().is_err(), "{}", bad);
        }
    }

    #[test]
    fn empty_config_is_valid() {
        let cfg: Config = toml::from_str("").unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.router.kind, "rules");
        assert!(!cfg.automation.enabled);
    }

    #[test]
    fn copilot_defaults_and_profiles() {
        let cfg: Config = toml::from_str("").unwrap();
        let c = cfg.agents.get(AgentKind::Copilot);
        assert_eq!(c.cmd.as_deref(), Some("copilot"));
        assert!(c.write_args.unwrap().iter().all(|a| !a.contains("allow-all") && a != "--yolo"));
        assert!(cfg.agents.copilot.stand_in());
        let p = cfg.agents.profile("claude").unwrap();
        assert!(p.models.unwrap().get(Tier::Standard).iter().all(|m| m.starts_with("claude-")));
        assert!(cfg.agents.profile("codex").unwrap().models.unwrap().get(Tier::Light).iter().all(|m| m.starts_with("gpt-")));
        assert!(cfg.agents.profile("second").is_none());
        assert_eq!(cfg.review.reviewer_order, vec![AgentRef::Kind(AgentKind::Claude), AgentRef::Kind(AgentKind::Codex)]);

        let cfg: Config = toml::from_str(
            r#"
            [agents.copilot]
            stand_in = false
            [agents.copilot.profiles.claude.models]
            standard = "claude-sonnet-4.5"
            [agents.copilot.profiles.second]
            vendor = "xai"
            models = { standard = ["grok-4.7", "grok-4.6"] }
            [review]
            pr_reviewers = ["claude", "copilot:second"]
            "#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert!(!cfg.agents.copilot.stand_in());
        let claude = cfg.agents.profile("claude").unwrap().models.unwrap();
        assert_eq!(claude.get(Tier::Standard), vec!["claude-sonnet-4.5".to_string()]);
        assert!(!claude.get(Tier::Heavy).is_empty(), "untouched tiers keep their defaults");
        assert_eq!(cfg.agents.profile("second").unwrap().models.unwrap().get(Tier::Standard), vec!["grok-4.7", "grok-4.6"]);
        assert_eq!(cfg.review.pr_reviewers[1], AgentRef::Profile("second".into()));
    }

    #[test]
    fn copilot_profiles_are_validated() {
        let check = |s: &str| toml::from_str::<Config>(s).map_err(|e| e.to_string()).and_then(|c| c.validate().map_err(|e| e.to_string()));
        assert!(check("[agents.copilot.profiles.claude]\nmodels = { standard = \"gpt-5.4\" }").is_err(), "wrong vendor for claude");
        assert!(check("[agents.copilot.profiles.x]\nmodels = { standard = \"grok-4.7\" }").is_err(), "unknown prefix needs a vendor");
        assert!(check("[agents.copilot.profiles.x]\nvendor = \"xai\"\nmodels = { standard = \"auto\" }").is_err(), "never auto in a profile");
        assert!(check("[agents.copilot.profiles.x]\nvendor = \"xai\"\nmodels = { standard = [] }").is_err(), "no models at all");
        assert!(check("[agents.copilot.profiles.\"Bad Name\"]\nvendor = \"xai\"\nmodels = { standard = \"grok-4.7\" }").is_err());
        assert!(check("[review]\npr_reviewers = [\"copilot:missing\"]").is_err(), "undefined profile");
        assert!(check("[review]\nreviewer_order = [\"copilot:missing\"]").is_err(), "undefined profile");
        assert!(check("[router]\ndefault_agent = \"copilot\"").is_err(), "the router stays two-way");
        assert!(check("[agents.copilot]\ntypo = 1").is_err());
        assert!(check("[agents.copilot]\nmodels = { standard = \"auto\" }").is_ok(), "auto is fine for copilot itself");
    }
}
