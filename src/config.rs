use crate::state::{AgentKind, Provider, Tier};
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
    pub jev: JevCfg,
    pub rules: RulesCfg,
}

impl Default for RouterCfg {
    fn default() -> Self {
        RouterCfg {
            kind: "rules".into(),
            default_agent: AgentKind::Claude,
            threshold: 0.6,
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
    pub timeout_mins: Option<u64>,
}

impl AgentsCfg {
    pub fn get(&self, k: AgentKind) -> AgentCfg {
        let (cfg, def) = match k {
            AgentKind::Claude => (&self.claude, AgentCfg::claude()),
            AgentKind::Codex => (&self.codex, AgentCfg::codex()),
        };
        AgentCfg {
            cmd: cfg.cmd.clone().or(def.cmd),
            models: Some(cfg.models.clone().unwrap_or_default().over(def.models)),
            efforts: Some(cfg.efforts.clone().unwrap_or_default().over(def.efforts)),
            write_args: cfg.write_args.clone().or(def.write_args),
            review_args: cfg.review_args.clone().or(def.review_args),
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
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewCfg {
    /// "auto" (router decides), "always" or "never"
    pub mode: String,
    /// "other" (the CLI that did not do the work) or "same"
    pub reviewer: String,
    pub max_fix_rounds: u32,
    /// Shell commands run after the worker finishes a typed task. Repositories have their own list.
    pub checks: Vec<String>,
    pub check_timeout_mins: u64,
    /// Which CLIs review a labelled pull request. Each one reviews independently.
    pub pr_reviewers: Vec<AgentKind>,
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
            max_fix_rounds: 1,
            checks: vec![],
            check_timeout_mins: 20,
            pr_reviewers: vec![AgentKind::Claude, AgentKind::Codex],
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
        if !(1..=24 * 60).contains(&self.review.check_timeout_mins) {
            anyhow::bail!("review.check_timeout_mins must be between 1 and 1440");
        }
        for k in [AgentKind::Claude, AgentKind::Codex] {
            if self.agents.get(k).cmd.unwrap_or_default().trim().is_empty() {
                anyhow::bail!("agents.{}.cmd must not be empty", k.name());
            }
        }
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
}
