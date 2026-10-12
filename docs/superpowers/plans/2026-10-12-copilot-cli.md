# GitHub Copilot CLI Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the GitHub Copilot CLI (`copilot`) as a third agent. It stands in for a missing `claude`/`codex` by running Claude/GPT models, and named copilot profiles can serve as extra reviewers. Org model policies are respected.

**Architecture:** The router keeps choosing claude or codex. A new pure module `src/pick.rs` turns what was asked for (`AgentRef`: `claude`, `codex`, `copilot`, `copilot:<profile>`) into what runs (`Exec`: CLI, model, vendor), using installed CLIs, the copilot probe's enabled models, and models rejected this session. `src/copilot.rs` talks to `copilot --headless --stdio` (JSON-RPC) to learn login, seat and per-model policy. `agents.rs` gains a copilot command line, the `parse_copilot` stream parser, and usage-file reading. The orchestrator asks `pick` for workers and reviewers, and reruns once per model the account rejects.

**Tech Stack:** Rust 2021, tokio, serde/serde_json, toml, ratatui. No new crates.

**Design reference:** `docs/copilot-plan.md` (findings with sources, decisions). Read its "Step 0" table before Task 4.

## Global Constraints

- No new dependencies in `Cargo.toml`.
- Existing configs must load unchanged and behave exactly as today when `copilot` is not installed.
- Router stays two-way: `router.default_agent` must be `claude` or `codex`.
- Copilot is never run with `--allow-all`, `--allow-all-tools` or `--yolo`.
- Never pick model `auto` (or an empty model) for a profile; `auto` is allowed only in `[agents.copilot] models`.
- Tier names are `light`, `standard`, `heavy` (as in `Tiers`), not low/medium/high.
- Token/secret env vars are never logged.
- Gate for every task: `cargo clippy --all-targets -- -D warnings && cargo test` green. Never run `cargo fmt`: the repository is hand-formatted and rustfmt reformats every file.
- Code style: long lines as in the surrounding code; comments sparse, explaining why. Commit messages: Conventional Commits.
- Work happens in worktree `/mnt/e/work/Forkdeck-copilot` on branch `copilot-cli` (from `25e1769`). Uncommitted follow-up work on `main` is not here; don't port it.

---

## File map

| File | Change |
|---|---|
| `src/state.rs` | `AgentKind::Copilot`, `AgentKind::ALL`, `AgentRef`; `State.installed`, `State.copilot`, `State.rejected_models`; `Stage.via` |
| `src/config.rs` | `CopilotCfg`, `ProfileCfg`, `TierLists`, `OneOrMany`; `ReviewCfg.reviewer_order`; `pr_reviewers: Vec<AgentRef>`; validation |
| `src/pick.rs` (new) | `Vendor`, `vendor_of_model`, `Avail`, `Exec`, `resolve`, `worker_for`, `reviewer_for` |
| `src/copilot.rs` (new) | JSON-RPC framing, `Probe`, `probe(cmd)` |
| `src/agents.rs` | `RunSpec.exec`, copilot `command_line`, `parse_copilot`, usage file, `RunOut` fields |
| `src/orchestrator.rs` | `detect_agents` uses `installed` + probe; `decide`/workers/reviewers through `pick`; rejected-model rerun |
| `src/issues.rs` | `Review.by: String` |
| `src/main.rs` | `parse_agent` → `AgentRef`; Tab cycle; doctor rows |
| `src/ui.rs` | colors and chips for copilot; "via" label |
| `src/demo.rs` | `RunOut` construction |
| `forkdeck.example.toml`, `README.md`, `docs/copilot-plan.md` | docs |
| `tests/fixtures/copilot/*.jsonl` (new) | captured streams |
| `tests/automation.rs` | fake `copilot` script, copilot tests |
| `tests/copilot_byok.rs` (new) | `#[ignore]` real-binary test |

---

### Task 1: `AgentKind::Copilot`, `AgentRef`

**Files:**
- Modify: `src/state.rs`
- Modify (compile fixes only): `src/agents.rs`, `src/config.rs`, `src/ui.rs`, `src/main.rs`, `src/orchestrator.rs`, `src/router.rs`

**Interfaces:**
- Produces: `AgentKind::{Claude, Codex, Copilot}`, `AgentKind::ALL: [AgentKind; 3]`; `AgentRef::{Kind(AgentKind), Profile(String)}` with `parse(&str) -> Result<AgentRef, String>`, `name() -> String`, `kind() -> AgentKind`, serde as string. `AgentKind::other()` stays for now (removed in Task 7) and maps `Copilot => Claude`.

- [ ] **Step 1: Write failing tests** (append to `mod tests` in `src/state.rs`)

```rust
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
        #[derive(serde::Deserialize)]
        struct T {
            r: Vec<AgentRef>,
        }
        let t: T = toml::from_str(r#"r = ["claude", "copilot:second"]"#).unwrap();
        assert_eq!(t.r, vec![AgentRef::Kind(AgentKind::Claude), AgentRef::Profile("second".into())]);
        assert!(toml::from_str::<T>(r#"r = ["nope"]"#).is_err());
    }
```

- [ ] **Step 2: Run, expect compile failure** — `cargo test --lib agent_refs_round_trip` → error: cannot find `AgentRef`.

- [ ] **Step 3: Implement in `src/state.rs`**

Replace the `AgentKind` block with:

```rust
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
        let kind = AgentKind::ALL.into_iter().find(|k| k.name() == s);
        if let Some(k) = kind {
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
```

- [ ] **Step 4: Fix exhaustive matches** so it compiles with no behaviour change:
  - `src/agents.rs` `command_line`: add arm `AgentKind::Copilot => {}` (filled in Task 5); `spawn_and_read` dispatch: `AgentKind::Copilot => parse_codex(line)` temporarily — replaced in Task 4.
  - `src/config.rs` `AgentsCfg::get`: `AgentKind::Copilot => (&self.claude, AgentCfg::claude())` temporarily — replaced in Task 2.
  - `src/ui.rs` `agent_color`: `AgentKind::Copilot => BLUE` if a `BLUE` const exists, else add `const BLUE: Color = Color::Rgb(110, 160, 255);` next to the other colors.
  - `src/main.rs` Tab match: add `Some(AgentKind::Copilot) => None` (cycle extended in Task 8).
  - `src/orchestrator.rs` `has` closures: `match a { AgentKind::Claude => have_c, AgentKind::Codex => have_x, AgentKind::Copilot => false }`.

- [ ] **Step 5: Gate** — `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` → all pass, new test passes.

- [ ] **Step 6: Commit** — `git add -A src && git commit -m "feat: add copilot agent kind and agent references"`

---

### Task 2: Configuration

**Files:** Modify `src/config.rs`, `forkdeck.example.toml`

**Interfaces:**
- Consumes: `AgentKind`, `AgentRef` (Task 1).
- Produces:
  - `AgentsCfg.copilot: CopilotCfg`; `AgentsCfg::get(AgentKind) -> AgentCfg` covers copilot (cmd, own models/efforts, args).
  - `CopilotCfg { cmd, models: Option<Tiers>, efforts: Option<Tiers>, write_args, review_args, stand_in: Option<bool>, profiles: HashMap<String, ProfileCfg> }`; `CopilotCfg::stand_in() -> bool` (default true); `AgentsCfg::profile(&str) -> Option<ProfileCfg>` (built-in `claude`/`codex` merged with user overrides).
  - `ProfileCfg { vendor: Option<String>, models: Option<TierLists>, efforts: Option<Tiers> }`.
  - `TierLists { light, standard, heavy: Option<OneOrMany> }`, `TierLists::get(Tier) -> Vec<String>`.
  - `OneOrMany` (untagged: string or list) with `to_vec()`.
  - `ReviewCfg.reviewer_order: Vec<AgentRef>` default `[claude, codex]`; `ReviewCfg.pr_reviewers: Vec<AgentRef>`.

- [ ] **Step 1: Failing tests** (append to `mod tests` in `src/config.rs`)

```rust
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
        assert!(!claude.get(Tier::Heavy).is_empty(), "untouched tiers keep defaults");
        assert_eq!(cfg.agents.profile("second").unwrap().models.unwrap().get(Tier::Standard), vec!["grok-4.7", "grok-4.6"]);
        assert_eq!(cfg.review.pr_reviewers[1], AgentRef::Profile("second".into()));
    }

    #[test]
    fn copilot_profiles_are_validated() {
        let bad = |s: &str| toml::from_str::<Config>(s).map_err(|e| e.to_string()).and_then(|c| c.validate().map_err(|e| e.to_string()));
        assert!(bad("[agents.copilot.profiles.claude]\nmodels = { standard = \"gpt-5.4\" }").is_err(), "wrong vendor in claude");
        assert!(bad("[agents.copilot.profiles.x]\nmodels = { standard = \"grok-4.7\" }").is_err(), "unknown prefix needs vendor");
        assert!(bad("[agents.copilot.profiles.x]\nvendor = \"xai\"\nmodels = { standard = \"auto\" }").is_err(), "auto never in a profile");
        assert!(bad("[agents.copilot.profiles.x]\nvendor = \"xai\"\nmodels = { standard = [] }").is_err(), "empty list");
        assert!(bad("[agents.copilot.profiles.\"Bad Name\"]\nvendor = \"xai\"").is_err());
        assert!(bad("[review]\npr_reviewers = [\"copilot:missing\"]").is_err(), "undefined profile");
        assert!(bad("[router]\ndefault_agent = \"copilot\"").is_err(), "router stays two-way");
        assert!(bad("[agents.copilot]\ntypo = 1").is_err());
        assert!(bad("[agents.copilot]\nmodels = { standard = \"auto\" }").is_ok(), "auto is fine for plain copilot");
    }
```

- [ ] **Step 2: Run** — `cargo test --lib copilot_` → compile errors (no `profile`, `stand_in`, `reviewer_order`).

- [ ] **Step 3: Implement.** In `src/config.rs`:

```rust
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
    /// Run the `claude`/`codex` profiles on copilot when that CLI is not installed.
    pub stand_in: Option<bool>,
    pub profiles: HashMap<String, ProfileCfg>,
}

impl CopilotCfg {
    pub fn stand_in(&self) -> bool {
        self.stand_in.unwrap_or(true)
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileCfg {
    /// Needed when the model IDs do not say it (claude-* is anthropic, gpt-*/o<digit>* openai).
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

/// Built-in stand-in profiles. IDs follow GitHub's documented pattern (gpt-5.4, claude-haiku-4.5);
/// which of them an account may use is checked at run time, so a missing one is skipped.
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
    /// A copilot profile with built-in defaults under the user's settings, or None if it does not exist.
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
```

`AgentsCfg::get`: replace the temporary copilot arm. Build copilot from `CopilotCfg`:

```rust
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
```

`AgentCfg::copilot()`:

```rust
    fn copilot() -> AgentCfg {
        AgentCfg {
            cmd: Some("copilot".into()),
            // Empty: the CLI's own default model.
            models: Tiers::of("", "", ""),
            efforts: Tiers::of("low", "medium", "high"),
            // Like claude's acceptEdits: edit files, no shell commands unattended.
            write_args: strs(&["--allow-tool", "write", "--deny-tool", "shell"]),
            // Skills load even with --no-custom-instructions, so they are hidden from a reviewer.
            review_args: strs(&["--deny-tool", "write", "--deny-tool", "shell", "--excluded-tools", "skill"]),
        }
    }
```

`ReviewCfg`: `pub pr_reviewers: Vec<AgentRef>`, new `pub reviewer_order: Vec<AgentRef>` (doc comment: "Who reviews when `reviewer = \"other\"`: the first one that is available and comes from a different model vendor than the worker."); defaults `vec![AgentRef::Kind(AgentKind::Claude), AgentRef::Kind(AgentKind::Codex)]` for both. Import `AgentRef`.

Validation, in `validate()` after the cmd loop (and change that loop to `AgentKind::ALL`):

```rust
        if self.router.default_agent == AgentKind::Copilot {
            anyhow::bail!("router.default_agent must be claude or codex");
        }
        let mut names: Vec<String> = self.agents.copilot.profiles.keys().cloned().collect();
        names.extend(["claude".to_string(), "codex".to_string()]);
        for name in &names {
            if AgentRef::parse(&format!("copilot:{}", name)).is_err() {
                anyhow::bail!("agents.copilot.profiles.{:?}: names use a-z, 0-9, - and _", name);
            }
            let p = self.agents.profile(name).expect("listed");
            let models = p.models.clone().unwrap_or_default();
            for t in [Tier::Light, Tier::Standard, Tier::Heavy] {
                if models.get(t).is_empty() && (name == "claude" || name == "codex" || models.all().is_empty()) {
                    anyhow::bail!("agents.copilot.profiles.{}.models.{} must list at least one model", name, t.name());
                }
            }
            let want = match name.as_str() {
                "claude" => Some(crate::pick::Vendor::Anthropic),
                "codex" => Some(crate::pick::Vendor::OpenAI),
                _ => None,
            };
            for m in models.all() {
                if m.trim().is_empty() || m == "auto" {
                    anyhow::bail!("agents.copilot.profiles.{}: a profile names its models; \"auto\" could pick any vendor", name);
                }
                let v = crate::pick::vendor_of_model(&m);
                if let Some(w) = &want {
                    if &v != w {
                        anyhow::bail!("agents.copilot.profiles.{}: {} is not a {} model", name, m, name);
                    }
                } else if v == crate::pick::Vendor::Unknown && p.vendor.as_deref().unwrap_or("").trim().is_empty() {
                    anyhow::bail!("agents.copilot.profiles.{}: set vendor, {} does not say who makes it", name, m);
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
```

Note: the "empty tier" rule means a user profile must have at least one model somewhere; a tier with no list then resolves to `None` for that tier (handled by `pick`). `pick` is created in Task 3; to keep this task compiling, create `src/pick.rs` now with just `Vendor` and `vendor_of_model` from Task 3 Step 3 (the first code block), add `mod pick;` to `src/main.rs`, and include its `vendors_from_model_ids` test.

- [ ] **Step 4: Example config.** In `forkdeck.example.toml` after `[agents.codex]`:

```toml
[agents.copilot]
cmd = "copilot"
# Empty model names keep the copilot CLI's own default ("auto" lets Copilot pick).
models = { light = "", standard = "", heavy = "" }
efforts = { light = "low", standard = "medium", heavy = "high" }
# Edit files, no shell commands unattended. Never use --allow-all or --yolo here.
write_args = ["--allow-tool", "write", "--deny-tool", "shell"]
review_args = ["--deny-tool", "write", "--deny-tool", "shell", "--excluded-tools", "skill"]
# With claude or codex missing, run that role on copilot with a Claude or GPT model,
# so work and review still come from different vendors.
stand_in = true

# Models per tier, first one your account allows wins. Organisations switch models
# on and off; forkdeck asks copilot which are enabled and skips the rest.
# [agents.copilot.profiles.claude.models]
# standard = ["claude-sonnet-5.5", "claude-sonnet-4.5"]
# [agents.copilot.profiles.codex.models]
# standard = ["gpt-5.5", "gpt-5.4"]

# Any other model your `copilot` /model picker lists can be a profile, used as
# "copilot:<name>" in pr_reviewers, reviewer_order or --agent.
# [agents.copilot.profiles.second]
# vendor = "xai"
# models = { light = "<model id>", standard = "<model id>", heavy = "<model id>" }
```

In `[review]` after `reviewer = ...`:

```toml
# With reviewer = "other": the first of these that is available and comes from a
# different model vendor than the worker reviews.
reviewer_order = ["claude", "codex"]
```

and change the `pr_reviewers` comment to mention `"copilot:<profile>"`. The existing tests `example_config_parses` and `commented_examples_parse_once_uncommented` must stay green (the second uncomments examples: keep `<model id>` lines valid TOML strings; if that test validates, use `vendor = "xai"` and `models = { standard = "grok-4.7" }` instead of placeholders).

- [ ] **Step 5: Gate**, then **Commit** — `git commit -am "feat(config): copilot agent, profiles and reviewer order"` (add `src/pick.rs`).

---

### Task 3: `pick` — who runs, with which model

**Files:** Modify `src/pick.rs` (created in Task 2)

**Interfaces:**
- Consumes: `Config`, `AgentsCfg::profile`, `TierLists::get`, `AgentRef`, `AgentKind`, `Tier`.
- Produces:
```rust
pub enum Vendor { Anthropic, OpenAI, Other(String), Unknown }
pub fn vendor_of_model(id: &str) -> Vendor
pub fn differs(a: &Vendor, b: &Vendor) -> bool
pub struct Avail<'a> { pub installed: &'a HashSet<AgentKind>, pub enabled: Option<&'a HashSet<String>>, pub no_seat: bool, pub rejected: &'a HashSet<String> }
#[derive(Clone, Debug, PartialEq)]
pub struct Exec { pub asked: AgentRef, pub cli: AgentKind, pub family: AgentKind, pub via: Option<String>, pub model: String, pub effort: String, pub vendor: Vendor }
impl Exec { pub fn label(&self) -> String }
pub fn resolve(cfg: &Config, a: &Avail, r: &AgentRef, tier: Tier) -> Option<Exec>
pub fn worker_for(cfg: &Config, a: &Avail, wanted: &AgentRef, tier: Tier) -> Option<Exec>
pub fn reviewer_for(cfg: &Config, a: &Avail, worker: &Exec, tier: Tier) -> (Exec, bool /* same vendor */)
```

- [ ] **Step 1: Failing tests** — `mod tests` in `src/pick.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use AgentKind::*;

    fn cfg(extra: &str) -> Config {
        let c: Config = toml::from_str(extra).unwrap();
        c.validate().unwrap();
        c
    }
    fn set<T: Clone + Eq + std::hash::Hash>(v: &[T]) -> HashSet<T> {
        v.iter().cloned().collect()
    }
    fn k(x: AgentKind) -> AgentRef {
        AgentRef::Kind(x)
    }

    #[test]
    fn vendors_from_model_ids() {
        assert_eq!(vendor_of_model("claude-sonnet-4.5"), Vendor::Anthropic);
        assert_eq!(vendor_of_model("GPT-5.4"), Vendor::OpenAI);
        assert_eq!(vendor_of_model("o4-mini"), Vendor::OpenAI);
        assert_eq!(vendor_of_model("gemini-3.8-flash"), Vendor::Unknown);
        assert_eq!(vendor_of_model("auto"), Vendor::Unknown);
        assert_eq!(vendor_of_model(""), Vendor::Unknown);
        assert!(differs(&Vendor::Anthropic, &Vendor::OpenAI));
        assert!(!differs(&Vendor::Anthropic, &Vendor::Unknown), "unknown never counts as different");
        assert!(!differs(&Vendor::Unknown, &Vendor::Unknown));
        assert!(differs(&Vendor::Other("xai".into()), &Vendor::OpenAI));
    }

    #[test]
    fn real_cli_wins_and_copilot_stands_in() {
        let c = cfg("");
        let none = HashSet::new();
        let all = set(&[Claude, Codex, Copilot]);
        let a = Avail { installed: &all, enabled: None, no_seat: false, rejected: &none };
        let e = resolve(&c, &a, &k(Claude), Tier::Standard).unwrap();
        assert_eq!((e.cli, e.via.clone(), e.model.as_str()), (Claude, None, "sonnet"));

        let only = set(&[Copilot]);
        let a = Avail { installed: &only, enabled: None, no_seat: false, rejected: &none };
        let e = resolve(&c, &a, &k(Claude), Tier::Standard).unwrap();
        assert_eq!((e.cli, e.family, e.vendor.clone()), (Copilot, Claude, Vendor::Anthropic));
        assert_eq!(e.model, "claude-sonnet-5.5");
        assert_eq!(e.label(), "claude via copilot");
        let e = resolve(&c, &a, &k(Codex), Tier::Light).unwrap();
        assert_eq!((e.cli, e.vendor.clone()), (Copilot, Vendor::OpenAI));

        let off = cfg("[agents.copilot]\nstand_in = false");
        assert!(resolve(&off, &a, &k(Claude), Tier::Standard).is_none());
        let a = Avail { installed: &only, enabled: None, no_seat: true, rejected: &none };
        assert!(resolve(&c, &a, &k(Claude), Tier::Standard).is_none(), "no seat, no stand-in");
        assert!(resolve(&c, &a, &k(Copilot), Tier::Standard).is_none());
        let a = Avail { installed: &none, enabled: None, no_seat: false, rejected: &none };
        for r in [k(Claude), k(Codex), k(Copilot), AgentRef::Profile("claude".into())] {
            assert!(resolve(&c, &a, &r, Tier::Standard).is_none());
        }
    }

    #[test]
    fn org_policy_and_rejections_skip_models() {
        let c = cfg("");
        let only = set(&[Copilot]);
        let enabled = set(&["claude-sonnet-4.5".to_string(), "gpt-5.4".to_string()]);
        let none = HashSet::new();
        let a = Avail { installed: &only, enabled: Some(&enabled), no_seat: false, rejected: &none };
        assert_eq!(resolve(&c, &a, &k(Claude), Tier::Standard).unwrap().model, "claude-sonnet-4.5");
        assert!(resolve(&c, &a, &k(Claude), Tier::Heavy).is_none(), "no opus enabled");
        let rejected = set(&["claude-sonnet-5.5".to_string(), "claude-sonnet-5".to_string()]);
        let a = Avail { installed: &only, enabled: None, no_seat: false, rejected: &rejected };
        assert_eq!(resolve(&c, &a, &k(Claude), Tier::Standard).unwrap().model, "claude-sonnet-4.6");
    }

    #[test]
    fn plain_copilot_and_user_profiles() {
        let c = cfg("[agents.copilot]\nmodels = { standard = \"auto\" }\n[agents.copilot.profiles.second]\nvendor = \"xai\"\nmodels = { standard = \"grok-4.7\" }");
        let only = set(&[Copilot]);
        let none = HashSet::new();
        let a = Avail { installed: &only, enabled: None, no_seat: false, rejected: &none };
        let e = resolve(&c, &a, &k(Copilot), Tier::Standard).unwrap();
        assert_eq!((e.model.as_str(), e.vendor.clone(), e.label()), ("auto", Vendor::Unknown, "copilot".to_string()));
        let e = resolve(&c, &a, &AgentRef::Profile("second".into()), Tier::Standard).unwrap();
        assert_eq!((e.model.as_str(), e.vendor.clone(), e.label()), ("grok-4.7", Vendor::Other("xai".into()), "copilot:second".to_string()));
        assert!(resolve(&c, &a, &AgentRef::Profile("second".into()), Tier::Heavy).is_none(), "tier without models");
        assert!(resolve(&c, &a, &AgentRef::Profile("nope".into()), Tier::Standard).is_none());
    }

    #[test]
    fn workers_fall_back_and_reviewers_cross_vendors() {
        let c = cfg("");
        let none = HashSet::new();
        let codex_only = set(&[Codex]);
        let a = Avail { installed: &codex_only, enabled: None, no_seat: false, rejected: &none };
        assert_eq!(worker_for(&c, &a, &k(Claude), Tier::Standard).unwrap().cli, Codex, "missing claude falls back");
        let w = worker_for(&c, &a, &k(Codex), Tier::Standard).unwrap();
        let (r, same) = reviewer_for(&c, &a, &w, Tier::Heavy);
        assert_eq!((r.cli, same), (Codex, true), "only one CLI: reviews its own work, flagged");

        let only = set(&[Copilot]);
        let a = Avail { installed: &only, enabled: None, no_seat: false, rejected: &none };
        let w = worker_for(&c, &a, &k(Claude), Tier::Standard).unwrap();
        let (r, same) = reviewer_for(&c, &a, &w, Tier::Heavy);
        assert_eq!((r.family, r.vendor.clone(), same), (Codex, Vendor::OpenAI, false));

        let both = set(&[Claude, Codex]);
        let a = Avail { installed: &both, enabled: None, no_seat: false, rejected: &none };
        let w = worker_for(&c, &a, &k(Codex), Tier::Standard).unwrap();
        assert_eq!(reviewer_for(&c, &a, &w, Tier::Heavy).0.cli, Claude);

        let same_cfg = cfg("[review]\nreviewer = \"same\"");
        let w = worker_for(&same_cfg, &a, &k(Codex), Tier::Standard).unwrap();
        let (r, same) = reviewer_for(&same_cfg, &a, &w, Tier::Heavy);
        assert_eq!((r.cli, r.model.as_str(), same), (Codex, "", true), "same reviewer at the review tier");

        let all = set(&[Claude, Codex, Copilot]);
        let a = Avail { installed: &all, enabled: None, no_seat: false, rejected: &none };
        let w = worker_for(&c, &a, &k(Copilot), Tier::Standard).unwrap();
        assert!(reviewer_for(&c, &a, &w, Tier::Heavy).1, "copilot's own model has an unknown vendor");
    }
}
```

- [ ] **Step 2: Run** — `cargo test --lib pick::` → compile errors for `resolve` etc.

- [ ] **Step 3: Implement** — full `src/pick.rs`:

```rust
//! Turns what was asked for (claude, codex, copilot, copilot:<profile>) into what runs: which CLI,
//! which model, and whose model it is. Pure: everything it needs is passed in.

use crate::config::Config;
use crate::state::{AgentKind, AgentRef, Tier};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vendor {
    Anthropic,
    OpenAI,
    Other(String),
    Unknown,
}

pub fn vendor_of_model(id: &str) -> Vendor {
    let id = id.trim().to_ascii_lowercase();
    let mut c = id.chars();
    if id.starts_with("claude-") {
        Vendor::Anthropic
    } else if id.starts_with("gpt-") || (c.next() == Some('o') && c.next().is_some_and(|d| d.is_ascii_digit())) {
        Vendor::OpenAI
    } else {
        Vendor::Unknown
    }
}

fn vendor_named(s: &str) -> Vendor {
    match s.trim().to_ascii_lowercase().as_str() {
        "" => Vendor::Unknown,
        "anthropic" => Vendor::Anthropic,
        "openai" => Vendor::OpenAI,
        o => Vendor::Other(o.to_string()),
    }
}

/// Two models are only known to differ when both vendors are known.
pub fn differs(a: &Vendor, b: &Vendor) -> bool {
    *a != Vendor::Unknown && *b != Vendor::Unknown && a != b
}

/// What is available right now.
pub struct Avail<'a> {
    pub installed: &'a HashSet<AgentKind>,
    /// Models the account may use, from the copilot probe. None: not known, try them all.
    pub enabled: Option<&'a HashSet<String>>,
    /// Copilot is installed and logged in but the account has no Copilot access or no model.
    pub no_seat: bool,
    /// Models copilot refused in this session.
    pub rejected: &'a HashSet<String>,
}

impl Avail<'_> {
    fn copilot(&self) -> bool {
        self.installed.contains(&AgentKind::Copilot) && !self.no_seat
    }
    fn usable(&self, model: &str) -> bool {
        !self.rejected.contains(model) && self.enabled.is_none_or(|e| e.contains(model))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Exec {
    pub asked: AgentRef,
    /// The program that runs.
    pub cli: AgentKind,
    /// The role it plays: claude or codex for a stand-in, else the CLI itself.
    pub family: AgentKind,
    /// "copilot" for a stand-in, "copilot:<name>" for a user profile.
    pub via: Option<String>,
    pub model: String,
    pub effort: String,
    pub vendor: Vendor,
}

impl Exec {
    pub fn label(&self) -> String {
        match (&self.asked, &self.via) {
            (AgentRef::Profile(_), _) => self.asked.name(),
            (_, Some(v)) => format!("{} via {}", self.family.name(), v),
            _ => self.cli.name().to_string(),
        }
    }
}

pub fn resolve(cfg: &Config, a: &Avail, r: &AgentRef, tier: Tier) -> Option<Exec> {
    match r {
        AgentRef::Kind(k @ (AgentKind::Claude | AgentKind::Codex)) => {
            if a.installed.contains(k) {
                let c = cfg.agents.get(*k);
                let vendor = if *k == AgentKind::Claude { Vendor::Anthropic } else { Vendor::OpenAI };
                return Some(Exec {
                    asked: r.clone(),
                    cli: *k,
                    family: *k,
                    via: None,
                    model: c.models.map(|m| m.get(tier).to_string()).unwrap_or_default(),
                    effort: c.efforts.map(|m| m.get(tier).to_string()).unwrap_or_default(),
                    vendor,
                });
            }
            if !cfg.agents.copilot.stand_in() {
                return None;
            }
            let mut e = profile(cfg, a, k.name(), tier)?;
            e.asked = r.clone();
            e.family = *k;
            e.via = Some("copilot".into());
            Some(e)
        }
        AgentRef::Kind(AgentKind::Copilot) => {
            if !a.copilot() {
                return None;
            }
            let c = cfg.agents.get(AgentKind::Copilot);
            let model = c.models.map(|m| m.get(tier).to_string()).unwrap_or_default();
            Some(Exec {
                asked: r.clone(),
                cli: AgentKind::Copilot,
                family: AgentKind::Copilot,
                via: None,
                vendor: vendor_of_model(&model),
                model,
                effort: c.efforts.map(|m| m.get(tier).to_string()).unwrap_or_default(),
            })
        }
        AgentRef::Profile(name) => {
            let mut e = profile(cfg, a, name, tier)?;
            e.via = Some(r.name());
            Some(e)
        }
    }
}

fn profile(cfg: &Config, a: &Avail, name: &str, tier: Tier) -> Option<Exec> {
    if !a.copilot() {
        return None;
    }
    let p = cfg.agents.profile(name)?;
    let model = p.models.as_ref()?.get(tier).into_iter().find(|m| m != "auto" && !m.trim().is_empty() && a.usable(m))?;
    let vendor = match vendor_of_model(&model) {
        Vendor::Unknown => vendor_named(p.vendor.as_deref().unwrap_or("")),
        v => v,
    };
    Some(Exec {
        asked: AgentRef::Profile(name.to_string()),
        cli: AgentKind::Copilot,
        family: AgentKind::Copilot,
        via: None,
        model,
        effort: p.efforts.map(|m| m.get(tier).to_string()).unwrap_or_default(),
        vendor,
    })
}

/// The worker for a task: what was asked for, else the other family, else plain copilot.
pub fn worker_for(cfg: &Config, a: &Avail, wanted: &AgentRef, tier: Tier) -> Option<Exec> {
    let mut tries = vec![wanted.clone()];
    if let AgentRef::Kind(k @ (AgentKind::Claude | AgentKind::Codex)) = wanted {
        tries.push(AgentRef::Kind(k.other()));
        tries.push(AgentRef::Kind(AgentKind::Copilot));
    }
    tries.iter().find_map(|r| resolve(cfg, a, r, tier))
}

/// The reviewer: with `reviewer = "other"`, the first of `review.reviewer_order` that is
/// available and from a different vendor than the worker. Otherwise (or if none is) the worker's
/// own agent at the review tier. The flag says the review does not cross vendors.
pub fn reviewer_for(cfg: &Config, a: &Avail, worker: &Exec, tier: Tier) -> (Exec, bool) {
    if cfg.review.reviewer == "other" {
        for r in &cfg.review.reviewer_order {
            if let Some(e) = resolve(cfg, a, r, tier) {
                if differs(&worker.vendor, &e.vendor) {
                    return (e, false);
                }
            }
        }
    }
    let own = resolve(cfg, a, &worker.asked, tier).unwrap_or_else(|| worker.clone());
    (own, true)
}
```

(If `Option::is_none_or` is unavailable on the toolchain's MSRV, use `map_or(true, ...)`; toolchain is 1.96 so it exists.)

- [ ] **Step 4: Run** `cargo test --lib pick::` → PASS. **Gate.**
- [ ] **Step 5: Commit** — `git commit -am "feat: pick worker and reviewer across CLIs, copilot profiles and model vendors"`

---

### Task 4: Copilot stream parser and fixtures

**Files:** Modify `src/agents.rs`; create `tests/fixtures/copilot/worker.jsonl`, `tests/fixtures/copilot/rejected.jsonl`

**Interfaces:**
- Produces: `pub fn parse_copilot(line: &str) -> Vec<Parsed>`; new `Parsed::ModelRejected(String)` and `Parsed::NoModels(String)` variants; `spawn_and_read` dispatches `AgentKind::Copilot => parse_copilot(line)`.

- [ ] **Step 1: Fixtures.** Copy the captures from the scratchpad (`probe/run1.jsonl`, `probe/e400.jsonl`), dropping every `session.skills_loaded` line (it lists the developer's personal skills) and replacing the scratchpad path `/tmp/claude-1002/...probe/work/` with `/work/`:

```bash
S=/tmp/claude-1002/-mnt-e-work-Forkdeck/aa2c6492-dee9-4d69-b262-3c2eb13fd3ae/scratchpad/probe
mkdir -p tests/fixtures/copilot
for f in run1:worker e400:rejected; do
  grep -v '"session.skills_loaded"' $S/${f%%:*}.jsonl | sed "s#$S/work/#/work/#g" > tests/fixtures/copilot/${f##*:}.jsonl
done
grep -c . tests/fixtures/copilot/*.jsonl; grep -l "claude-1002\|justin" tests/fixtures/copilot/* && echo LEAK || echo clean
```
Expected: line counts > 10 each, `clean`.

- [ ] **Step 2: Failing tests** (append to `mod tests` in `src/agents.rs`)

```rust
    fn fixture(name: &str) -> Vec<Vec<Parsed>> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot").join(name);
        std::fs::read_to_string(p).unwrap().lines().filter(|l| !l.trim().is_empty()).map(parse_copilot).collect()
    }

    #[test]
    fn copilot_worker_stream() {
        let all: Vec<Parsed> = fixture("worker.jsonl").into_iter().flatten().filter(|p| *p != Parsed::Nothing).collect();
        assert_eq!(
            all,
            vec![
                Parsed::Tool("view /work/notes.txt".into()),
                Parsed::Text("All good.\nVERDICT: PASS".into()),
                Parsed::Final { ok: true, text: String::new(), tokens_in: 0, tokens_out: 0, cost: 0.0 },
            ]
        );
    }

    #[test]
    fn copilot_rejected_model_stream() {
        let all: Vec<Parsed> = fixture("rejected.jsonl").into_iter().flatten().filter(|p| *p != Parsed::Nothing).collect();
        assert_eq!(
            all,
            vec![
                Parsed::ModelRejected("fake-model".into()),
                Parsed::Error("400 The requested model is not supported.".into()),
                Parsed::Final { ok: false, text: String::new(), tokens_in: 0, tokens_out: 0, cost: 0.0 },
            ]
        );
    }

    #[test]
    fn copilot_other_lines() {
        assert_eq!(parse_copilot("plain"), vec![Parsed::Text("plain".into())]);
        assert_eq!(parse_copilot(r#"{"type":"model.call_start","data":{}}"#), vec![Parsed::Nothing]);
        assert_eq!(parse_copilot(r#"{"type":"assistant.message_delta","data":{"deltaContent":"x"},"ephemeral":true}"#), vec![Parsed::Nothing]);
        assert_eq!(parse_copilot(r#"{"type":"assistant.message","data":{"content":"","toolRequests":[{"name":"view"}]}}"#), vec![Parsed::Nothing]);
        assert_eq!(
            parse_copilot(r#"{"type":"tool.execution_complete","data":{"toolCallId":"1","success":false,"error":{"message":"denied"}}}"#),
            vec![Parsed::Note("tool failed: denied".into())]
        );
        assert_eq!(parse_copilot(r#"{"type":"session.warning","data":{"message":"slow","warningType":"x"}}"#), vec![Parsed::Note("warning: slow".into())]);
        assert_eq!(
            parse_copilot(r#"{"type":"session.model_change","data":{"newModel":"gpt-5.4","cause":"rate_limit_auto_switch"}}"#),
            vec![Parsed::Note("model switched to gpt-5.4 (rate_limit_auto_switch)".into())]
        );
        assert_eq!(parse_copilot(r#"{"type":"assistant.usage","data":{"model":"m","inputTokens":5,"outputTokens":2},"ephemeral":true}"#), vec![Parsed::Usage { tokens_in: 5, tokens_out: 2 }]);
        assert_eq!(
            parse_copilot(r#"{"type":"session.error","data":{"errorType":"authorization","message":"No model available. Check policy enablement under GitHub Settings > Copilot"}}"#),
            vec![Parsed::NoModels("No model available. Check policy enablement under GitHub Settings > Copilot".into())]
        );
        assert_eq!(
            parse_copilot(r#"{"type":"tool.execution_start","data":{"toolName":"bash","arguments":{"command":"cargo test\nmore"}}}"#),
            vec![Parsed::Tool("bash cargo test".into())]
        );
    }
```

- [ ] **Step 3: Run** → compile errors.

- [ ] **Step 4: Implement.** Add variants to `Parsed`:

```rust
    /// The account or provider refused the model before anything ran. The run may be retried with another one.
    ModelRejected(String),
    /// The account has no model it may use at all.
    NoModels(String),
```

Parser:

```rust
pub fn parse_copilot(line: &str) -> Vec<Parsed> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![Parsed::Text(line.to_string())],
    };
    let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |p: &str| v.pointer(p).and_then(|x| x.as_u64()).unwrap_or(0);
    let one = match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "assistant.message" if !s("/data/content").trim().is_empty() => Parsed::Text(s("/data/content")),
        "tool.execution_start" => Parsed::Tool(tool_summary(&s("/data/toolName"), v.pointer("/data/arguments").unwrap_or(&Value::Null))),
        "tool.execution_complete" if v.pointer("/data/success").and_then(|b| b.as_bool()) == Some(false) => {
            Parsed::Note(format!("tool failed: {}", s("/data/error/message")))
        }
        "assistant.usage" => Parsed::Usage { tokens_in: n("/data/inputTokens"), tokens_out: n("/data/outputTokens") },
        "model.call_failure" if s("/data/errorCode") == "model_not_supported" => Parsed::ModelRejected(s("/data/model")),
        "session.error" if s("/data/message").contains("No model available") => Parsed::NoModels(s("/data/message")),
        "session.error" => Parsed::Error(s("/data/message")),
        "session.warning" => Parsed::Note(format!("warning: {}", s("/data/message"))),
        "session.model_change" if !s("/data/cause").is_empty() => {
            Parsed::Note(format!("model switched to {} ({})", s("/data/newModel"), s("/data/cause")))
        }
        // The closing line. Tokens are not in it; they come from the usage file after exit.
        "result" => Parsed::Final { ok: n("/exitCode") == 0 && v.get("exitCode").is_some(), text: String::new(), tokens_in: 0, tokens_out: 0, cost: 0.0 },
        _ => Parsed::Nothing,
    };
    vec![one]
}
```

In `spawn_and_read`: dispatch `AgentKind::Copilot => parse_copilot(line)`. In the display `match &p`: `Parsed::ModelRejected(m) => s.push(format!("error: model {} is not available to this account", m))`, `Parsed::NoModels(e) => s.push(format!("error: {}", e))`. In the outcome `match p`: `Parsed::ModelRejected(m) => { if s_tools_seen == 0 { rejected = Some(m) } }` — track `let mut tools_seen = 0u32;` incremented on `Parsed::Tool`, and `let mut rejected: Option<String> = None; let mut no_models = false;`; `Parsed::NoModels(e) => { no_models = true; error = e }`. Also after the loop: `if stderr_text.contains("No model available") { no_models = true; }` and `if stderr_text.contains("The requested model is not supported") && tools_seen == 0 { rejected.get_or_insert_with(String::new); }`. These feed `RunOut` in Task 5; for now keep them as `let _ = (rejected, no_models);` to compile.

Text from `Parsed::Final` with empty text: existing code uses `final_text.filter(|t| !t.is_empty()).unwrap_or(last_text)` — already correct.

- [ ] **Step 5: Gate**, **Commit** — `git add tests/fixtures src/agents.rs && git commit -m "feat(agents): parse copilot JSONL output"`

---

### Task 5: Running an `Exec`: command line, usage file, rejected models

**Files:** Modify `src/agents.rs`, `src/demo.rs`, `src/orchestrator.rs` (call sites only)

**Interfaces:**
- Consumes: `pick::Exec` (Task 3), parser flags (Task 4).
- Produces:
```rust
pub struct RunSpec<'a> { pub exec: &'a Exec, pub prompt: &'a str, pub dir: &'a Path, pub read_only: bool }
#[derive(Default)] pub struct RunOut { pub ok: bool, pub text: String, pub error: String, pub rejected_model: Option<String>, pub no_models: bool, pub models_used: Vec<String> }
pub struct CmdLine { pub cmd: String, pub args: Vec<String>, pub usage_file: Option<PathBuf> }
pub fn command_line(hub: &Hub, spec: &RunSpec) -> CmdLine
pub fn read_usage(path: &Path) -> (u64, u64, Vec<String>)
```
`Stage.via: String` added in `src/state.rs`; `run` sets `s.agent = Some(exec.family)`, `s.via = exec.via.clone().unwrap_or_default()`, `s.model = exec.model`, `s.effort = exec.effort`.

- [ ] **Step 1: Failing tests** (in `src/agents.rs` tests; build a `Hub` the way existing orchestrator tests do — if none exists, test `copilot_args` as a pure helper instead, which is what the implementation exposes):

```rust
    #[test]
    fn copilot_command_lines() {
        let cfg: crate::config::Config = toml::from_str("").unwrap();
        let exec = crate::pick::Exec {
            asked: crate::state::AgentRef::Kind(AgentKind::Claude),
            cli: AgentKind::Copilot,
            family: AgentKind::Claude,
            via: Some("copilot".into()),
            model: "claude-sonnet-4.5".into(),
            effort: "medium".into(),
            vendor: crate::pick::Vendor::Anthropic,
        };
        let usage = std::path::Path::new("/tmp/u.json");
        let w = copilot_args(&cfg, &exec, false, usage);
        assert_eq!(&w[..5], ["--output-format", "json", "--no-ask-user", "--no-auto-update", "--usage-output-file"]);
        assert_eq!(w[5], "/tmp/u.json");
        assert!(w.windows(2).any(|p| p == ["--model", "claude-sonnet-4.5"]));
        assert!(w.windows(2).any(|p| p == ["--reasoning-effort", "medium"]));
        assert!(w.windows(2).any(|p| p == ["--allow-tool", "write"]));
        assert!(!w.iter().any(|a| a == "-p" || a.contains("allow-all") || a == "--yolo"), "prompt via stdin, never allow-all");
        let r = copilot_args(&cfg, &exec, true, usage);
        assert!(r.windows(2).any(|p| p == ["--deny-tool", "write"]) && r.windows(2).any(|p| p == ["--excluded-tools", "skill"]));
        let bare = crate::pick::Exec { model: String::new(), effort: String::new(), ..exec };
        assert!(!copilot_args(&cfg, &bare, false, usage).iter().any(|a| a == "--model" || a == "--reasoning-effort"));
    }

    #[test]
    fn usage_file_gives_tokens_and_models() {
        let dir = std::env::temp_dir().join(format!("forkdeck-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("u.json");
        std::fs::write(&p, r#"{"modelMetrics":{"claude-sonnet-4.5":{"usage":{"inputTokens":200,"outputTokens":14,"cacheReadTokens":50}},"gpt-5.4":{"usage":{"inputTokens":1,"outputTokens":1}}}}"#).unwrap();
        let (i, o, mut m) = read_usage(&p);
        m.sort();
        assert_eq!((i, o, m), (251, 15, vec!["claude-sonnet-4.5".to_string(), "gpt-5.4".to_string()]));
        assert_eq!(read_usage(&dir.join("missing.json")), (0, 0, vec![]));
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run** → compile errors.

- [ ] **Step 3: Implement.**

```rust
/// Arguments for one copilot run. The prompt goes to stdin, so it never needs `-p`.
pub fn copilot_args(cfg: &Config, exec: &Exec, read_only: bool, usage: &Path) -> Vec<String> {
    let a = cfg.agents.get(AgentKind::Copilot);
    let mut args: Vec<String> = ["--output-format", "json", "--no-ask-user", "--no-auto-update", "--usage-output-file"].map(String::from).to_vec();
    args.push(usage.display().to_string());
    if !exec.model.is_empty() {
        args.extend(["--model".to_string(), exec.model.clone()]);
    }
    if !exec.effort.is_empty() {
        args.extend(["--reasoning-effort".to_string(), exec.effort.clone()]);
    }
    args.extend(if read_only { a.review_args } else { a.write_args }.unwrap_or_default());
    args
}

/// Tokens (input incl. cache reads/writes, output) and the models that ran, from copilot's usage file.
pub fn read_usage(path: &Path) -> (u64, u64, Vec<String>) {
    let Ok(v) = std::fs::read_to_string(path).map_err(|_| ()).and_then(|s| serde_json::from_str::<Value>(&s).map_err(|_| ())) else {
        return (0, 0, vec![]);
    };
    let (mut i, mut o, mut models) = (0, 0, vec![]);
    if let Some(m) = v.get("modelMetrics").and_then(|m| m.as_object()) {
        for (name, x) in m {
            let n = |k: &str| x.pointer(&format!("/usage/{}", k)).and_then(|n| n.as_u64()).unwrap_or(0);
            i += n("inputTokens") + n("cacheReadTokens") + n("cacheWriteTokens");
            o += n("outputTokens");
            models.push(name.clone());
        }
    }
    (i, o, models)
}
```

`command_line` takes model/effort from `spec.exec` for claude/codex (instead of looking them up) and for copilot builds `usage_file = std::env::temp_dir().join(format!("forkdeck-copilot-{}-{}.json", std::process::id(), <counter>))` with a `static NEXT: AtomicU64`, args from `copilot_args`. `cmd` comes from `hub.cfg.agents.get(exec.cli).cmd`.

`run`: use `spec.exec.cli` as the parser kind; stage gets `agent = Some(exec.family)`, `via`, `model`, `effort`. `spawn_and_read` gets the `CmdLine`; after the child exits, if `usage_file` is set: `let (i, o, models) = read_usage(p); let _ = std::fs::remove_file(p);` and if `i + o > 0` set the stage's `tokens_in/out` to them; fill `RunOut { rejected_model, no_models, models_used: models, .. }`. A run with `rejected_model.is_some()` is never `ok`.

Replace every `RunOut { ok, text, error }` literal with `RunOut { ok, text, error, ..Default::default() }` (agents.rs, demo.rs).

Orchestrator call sites: build an `Exec` from the old `AgentKind` with a temporary helper so behaviour is unchanged until Task 7:

```rust
fn exec_of(hub: &Hub, k: AgentKind, tier: Tier) -> Exec {
    let st = hub.state.lock().unwrap();
    let installed: HashSet<AgentKind> = AgentKind::ALL.into_iter().filter(|_| true).collect();
    let none = HashSet::new();
    drop(st);
    pick::resolve(&hub.cfg, &pick::Avail { installed: &installed, enabled: None, no_seat: false, rejected: &none }, &AgentRef::Kind(k), tier).expect("claude/codex always resolve when installed")
}
```
(Task 7 deletes it.)

- [ ] **Step 4: Gate**, **Commit** — `git commit -am "feat(agents): run copilot with model, effort and usage file"`

---

### Task 6: Copilot probe (JSON-RPC)

**Files:** Create `src/copilot.rs`; modify `src/main.rs` (`mod copilot;`), `src/state.rs`, `src/orchestrator.rs` (`detect_agents`)

**Interfaces:**
- Produces:
```rust
pub fn frame(v: &Value) -> Vec<u8>
pub async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R) -> Option<Value>
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe { pub login: Option<String>, pub seat: Option<bool>, pub enabled: Option<HashSet<String>>, pub disabled: Vec<String>, pub error: Option<String> }
pub fn models_from(result: &Value) -> (HashSet<String>, Vec<String>)
pub fn seat_from_error(err: &Value) -> Option<bool>
pub async fn probe(cmd: &str) -> Probe
```
- `State`: replace `have_claude`/`have_codex` with `pub installed: HashSet<AgentKind>`; add `pub copilot: Probe`, `pub rejected_models: HashSet<String>`. Add `impl State { pub fn has(&self, k: AgentKind) -> bool }`. Update `ui.rs` and orchestrator readers (`st.installed.contains(&AgentKind::Claude)`).

- [ ] **Step 1: Failing tests** (`src/copilot.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn frames_round_trip() {
        let a = json!({"jsonrpc":"2.0","id":1,"method":"connect","params":{}});
        let b = json!({"jsonrpc":"2.0","id":2,"result":{"ok":true,"text":"ünïcode"}});
        let mut bytes = frame(&a);
        bytes.extend(frame(&b));
        let mut r = tokio::io::BufReader::new(&bytes[..]);
        assert_eq!(read_frame(&mut r).await, Some(a));
        assert_eq!(read_frame(&mut r).await, Some(b));
        assert_eq!(read_frame(&mut r).await, None);
    }

    #[test]
    fn model_policy_and_seat() {
        let r = json!({"models":[
            {"id":"claude-sonnet-4.5","policy":{"state":"enabled","terms":""}},
            {"id":"gpt-5.4"},
            {"id":"claude-opus-4.8","policy":{"state":"disabled","terms":""}},
            {"id":"grok-4.7","policy":{"state":"unconfigured","terms":""}}]});
        let (en, dis) = models_from(&r);
        assert_eq!(en, ["claude-sonnet-4.5", "gpt-5.4"].iter().map(|s| s.to_string()).collect());
        assert_eq!(dis, vec!["claude-opus-4.8".to_string(), "grok-4.7".to_string()]);
        // As seen from an account without a Copilot seat (2026-10-12).
        let e = json!({"code":-32603,"message":"Request models.list failed with message: GenericFailure, {\"kind\":\"http\",\"status\":403,\"statusText\":\"Forbidden\",\"body\":\"unauthorized: not authorized to use this Copilot feature\\n\"}"});
        assert_eq!(seat_from_error(&e), Some(false));
        assert_eq!(seat_from_error(&json!({"code":-32603,"message":"timeout"})), None);
    }
}
```

- [ ] **Step 2: Run** → compile errors.

- [ ] **Step 3: Implement** `src/copilot.rs`:

```rust
//! Asks the copilot CLI who is logged in and which models the account may use, through its
//! headless JSON-RPC server (the protocol @github/copilot-sdk speaks). Costs no model request.

use crate::proc;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

pub fn frame(v: &Value) -> Vec<u8> {
    let body = v.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body.into_bytes());
    out
}

pub async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R) -> Option<Value> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().ok();
            }
        }
    }
    let mut body = vec![0u8; len?];
    r.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub login: Option<String>,
    /// Some(false): logged in, but the account has no Copilot access for the CLI.
    pub seat: Option<bool>,
    /// Models with an enabled policy (or none stated). None: the list could not be read.
    pub enabled: Option<HashSet<String>>,
    pub disabled: Vec<String>,
    pub error: Option<String>,
}

pub fn models_from(result: &Value) -> (HashSet<String>, Vec<String>) {
    let mut en = HashSet::new();
    let mut dis = Vec::new();
    for m in result.get("models").and_then(|m| m.as_array()).into_iter().flatten() {
        let Some(id) = m.get("id").and_then(|i| i.as_str()) else { continue };
        match m.pointer("/policy/state").and_then(|s| s.as_str()) {
            None | Some("enabled") => {
                en.insert(id.to_string());
            }
            Some(_) => dis.push(id.to_string()),
        }
    }
    (en, dis)
}

pub fn seat_from_error(err: &Value) -> Option<bool> {
    let m = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
    (m.contains("\"status\":403") || m.contains("not authorized to use this Copilot feature")).then_some(false)
}

pub async fn probe(cmd: &str) -> Probe {
    match tokio::time::timeout(Duration::from_secs(15), talk(cmd)).await {
        Ok(p) => p,
        Err(_) => Probe { error: Some("copilot did not answer within 15 s".into()), ..Probe::default() },
    }
}

async fn talk(cmd: &str) -> Probe {
    let mut p = Probe::default();
    let mut child = match proc::command(cmd).args(["--headless", "--no-auto-update", "--stdio"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(c) => c,
        Err(e) => {
            p.error = Some(format!("cannot start {}: {}", cmd, e));
            return p;
        }
    };
    // Dropping the guard kills copilot however this function returns.
    let _group = proc::Group::of(&child);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut out = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut call = |id: u64, method: &str, params: Value| frame(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    let requests = [(1, "connect", json!({"supportedTaskKinds": ["agent"]})), (2, "auth.getStatus", json!({})), (3, "models.list", json!({}))];
    for (id, method, params) in requests {
        if stdin.write_all(&call(id, method, params)).await.is_err() {
            p.error = Some("copilot closed its input".into());
            return p;
        }
        let _ = stdin.flush().await;
        // Skip notifications until the answer to this request arrives.
        let answer = loop {
            match read_frame(&mut out).await {
                Some(v) if v.get("id").and_then(|i| i.as_u64()) == Some(id) => break v,
                Some(_) => continue,
                None => {
                    p.error = Some(format!("copilot ended before answering {}", method));
                    return p;
                }
            }
        };
        if let Some(e) = answer.get("error") {
            match method {
                "models.list" => p.seat = seat_from_error(e),
                _ => {}
            }
            if p.seat.is_none() {
                p.error = Some(format!("{}: {}", method, crate::state::clip(e.get("message").and_then(|m| m.as_str()).unwrap_or("error"), 200)));
            }
            return p;
        }
        let r = answer.get("result").cloned().unwrap_or(Value::Null);
        match method {
            "auth.getStatus" => {
                if r.get("isAuthenticated").and_then(|b| b.as_bool()) != Some(true) {
                    p.error = Some("copilot is not logged in (run `copilot login`)".into());
                    return p;
                }
                p.login = r.get("login").and_then(|l| l.as_str()).map(String::from);
            }
            "models.list" => {
                let (en, dis) = models_from(&r);
                p.seat = Some(!en.is_empty());
                p.enabled = Some(en);
                p.disabled = dis;
            }
            _ => {}
        }
    }
    p
}
```

(Drop the unused `mut` on `call` if clippy complains; the `match method` with one arm becomes an `if`.)

`detect_agents`:

```rust
    pub async fn detect_agents(&self) {
        let found: HashSet<AgentKind> = if self.cfg.demo {
            [AgentKind::Claude, AgentKind::Codex].into_iter().collect()
        } else {
            let checks = AgentKind::ALL.map(|k| {
                let cmd = self.cfg.agents.get(k).cmd.unwrap_or_default();
                async move { (k, agents::on_path(&cmd).await) }
            });
            futures_join_all(checks).await
        };
        ...
    }
```
No `futures` crate: use `tokio::join!` on the three calls explicitly. If copilot is installed, run `copilot::probe` right after (awaited: ~1-2 s, startup is already async) and store it in `st.copilot`. Log lines:
- copilot installed, probe `seat == Some(false)`: `"copilot: no Copilot access for <login>; it will not be used"`.
- probe error: `"copilot: could not list models ({e}); trying configured models as they come"`.
- neither claude nor codex installed but copilot resolves them: `"using copilot for claude and codex"`; nothing usable at all: keep the existing "neither claude nor codex was found on PATH" text extended to "... nor a usable copilot".

- [ ] **Step 4: Gate** (the tokio test needs `#[tokio::test]`; tokio "full" includes macros). **Commit** — `git commit -am "feat: probe copilot login, seat and enabled models"`

---

### Task 7: Orchestrator uses `pick` (workers, reviewers, PR reviewers, reruns)

**Files:** Modify `src/orchestrator.rs`, `src/issues.rs`, `src/main.rs` (submit signature), `src/state.rs`

**Interfaces:**
- Consumes: `pick::{worker_for, reviewer_for, resolve, Avail, Exec}`, `RunOut.rejected_model/no_models/models_used`, `State.{installed, copilot, rejected_models}`.
- Produces: `Job.forced: Option<AgentRef>`; `Hub::submit(.., forced: Option<AgentRef>)`; `decide(..) -> Result<(Decision, Exec), Finish>`; `Work.worker: Exec`, `Work.reviewer: Exec`, `Work.same_vendor: bool`; `issues::Review.by: String`; helper:
```rust
async fn run_agent(hub: &Hub, id: u32, slot: Slot, exec: Exec, tier: Tier, prompt: &str, dir: &Path, read_only: bool) -> (RunOut, Exec)
```
`AgentKind::other()` deleted; `available()` and `exec_of` deleted.

- [ ] **Step 1: Failing test** — orchestrator logic is covered by integration tests (Task 9). Unit-test the pure part: the avail snapshot.

```rust
    #[test]
    fn avail_snapshot_reflects_probe() {
        let mut st = State::default();
        st.installed.insert(AgentKind::Copilot);
        st.copilot.seat = Some(false);
        let snap = AvailOwned::of(&st);
        assert!(snap.borrow().no_seat);
        st.copilot.seat = Some(true);
        st.copilot.enabled = Some(["gpt-5.4".to_string()].into_iter().collect());
        let snap = AvailOwned::of(&st);
        assert!(!snap.borrow().no_seat && snap.borrow().enabled.unwrap().contains("gpt-5.4"));
    }
```

- [ ] **Step 2: Implement.**

`AvailOwned` (in orchestrator.rs) — an owned copy so the state lock is not held across awaits:

```rust
struct AvailOwned {
    installed: HashSet<AgentKind>,
    enabled: Option<HashSet<String>>,
    no_seat: bool,
    rejected: HashSet<String>,
}

impl AvailOwned {
    fn of(st: &State) -> AvailOwned {
        AvailOwned {
            installed: st.installed.clone(),
            enabled: st.copilot.enabled.clone(),
            no_seat: st.copilot.seat == Some(false),
            rejected: st.rejected_models.clone(),
        }
    }
    fn borrow(&self) -> pick::Avail<'_> {
        pick::Avail { installed: &self.installed, enabled: self.enabled.as_ref(), no_seat: self.no_seat, rejected: &self.rejected }
    }
}

fn avail(hub: &Hub) -> AvailOwned {
    if hub.cfg.demo {
        let installed = [AgentKind::Claude, AgentKind::Codex].into_iter().collect();
        return AvailOwned { installed, enabled: None, no_seat: false, rejected: HashSet::new() };
    }
    AvailOwned::of(&hub.state.lock().unwrap())
}
```

`run_agent`:

```rust
/// Run an agent; when copilot refuses the model before doing anything, remember that and try the
/// next model for the same role. Each pass removes one model, so this ends.
async fn run_agent(hub: &Hub, id: u32, slot: Slot, mut exec: Exec, tier: Tier, prompt: &str, dir: &Path, read_only: bool) -> (RunOut, Exec) {
    loop {
        let out = agents::run(hub, id, slot, RunSpec { exec: &exec, prompt, dir, read_only }).await;
        if out.no_models {
            hub.state.lock().unwrap().copilot.seat = Some(false);
            hub.log("agents", format!("copilot has no model this account may use: {}", out.error));
            return (out, exec);
        }
        let Some(m) = out.rejected_model.clone() else { return (out, exec) };
        let m = if m.is_empty() { exec.model.clone() } else { m };
        hub.state.lock().unwrap().rejected_models.insert(m.clone());
        hub.log("agents", format!("{} is not available to this copilot account, skipping it", m));
        match pick::resolve(&hub.cfg, &avail(hub).borrow(), &exec.asked, tier) {
            Some(next) if next.model != exec.model => exec = next,
            _ => return (out, exec),
        }
    }
}
```

`decide`: signature `forced: Option<AgentRef>`; router result `d.agent` stays claude/codex; `wanted = forced.clone().unwrap_or(AgentRef::Kind(d.agent))`; when forced, `d.note = format!("agent forced to {}", f.name())` and if `f.kind() != AgentKind::Copilot { d.agent = f.kind() }`. Then:

```rust
    let a = avail(hub);
    let Some(worker) = pick::worker_for(&hub.cfg, &a.borrow(), &wanted, d.tier) else {
        hub.job(id, |j| j.decision = Some(d.clone()));
        return Err(failed(format!("{} is not available, and nothing can stand in for it", wanted.name())));
    };
    if worker.asked != wanted {
        hub.log("router", format!("{} is not available, using {}", wanted.name(), worker.label()));
    }
```
Log line uses `worker.label()` instead of `d.agent.name()`; return `Ok((d, worker))`.

Typed/issue job (`run_task` area, line ~575 at base):

```rust
    let (d, worker) = match decide(hub, id, &prompt, is_issue, forced).await { Ok(x) => x, Err(f) => return f };
    let (reviewer, same_vendor) = pick::reviewer_for(&hub.cfg, &avail(hub).borrow(), &worker, d.tier.up());
    if d.review && same_vendor {
        hub.log("review", format!("#{} same-vendor review: {} reviews {}", id, reviewer.label(), worker.label()));
    }
```
`Work` gets `worker: Exec, reviewer: Exec, same_vendor: bool` instead of `reviewer: AgentKind`. `work_once` calls `run_agent(hub, id, Slot::Worker, w.worker.clone(), w.d.tier, prompt, w.dir, false)` and reports `w.worker.label()` in errors. In `refine`, the reviewer call becomes `run_agent(..., Slot::Reviewer, w.reviewer.clone(), w.d.tier.up(), &rp, w.dir, true)`. If `w.same_vendor`, the reviewer's text is prefixed for anything published: `format!("(Same-vendor review: {} reviewed {}.)\n\n{}", reviewer, worker, text)` — only where the review text is posted to an issue/PR, not in the verdict parsing input (parse the raw text).

**Vendor check after the run:** after each reviewer run, if any of `out.models_used` has `pick::vendor_of_model(m)` equal to the worker's vendor (and that vendor is known), treat as `same_vendor = true` and log `"#{} reviewer ran {} after a model switch: same-vendor review"`.

PR reviews: `reviewers: Vec<Exec>` from `hub.cfg.review.pr_reviewers` via `pick::resolve(&cfg, &a.borrow(), r, tier)`, dropping `None` with a log line `"{} is not available for review, skipped"`, deduplicated by `label()`. `decide` is still called for the tier (ignore its `Exec`). Note now shows labels. `issues::Review { by: e.label(), ... }`; in `src/issues.rs` change `pub by: AgentKind` → `pub by: String`, line 901 `r.by.name()` → `r.by`, and test literals `AgentKind::Claude` → `"claude".into()`, `AgentKind::Codex` → `"codex".into()`; orchestrator test `crashed_reviewer_is_incomplete_not_negative` likewise.

`submit`/`Job.forced`/`headless`/`Ui.forced` types → `Option<AgentRef>`; `parse_agent` returns `AgentRef` via `AgentRef::parse(s).map_err(anyhow::Error::msg)`; help text for `--agent`: "Force an agent: claude, codex, copilot or copilot:<profile>".

Delete `AgentKind::other()`, `available()`, `exec_of`. Grep must be clean: `grep -n "\.other()\|have_claude\|have_codex\|exec_of" src` → nothing.

- [ ] **Step 3: Gate** (all existing integration tests must still pass: behaviour with claude stand-in only is unchanged). **Commit** — `git commit -am "feat: choose workers and reviewers through pick, retry refused copilot models"`

---

### Task 8: Surface: TUI, doctor

**Files:** Modify `src/ui.rs`, `src/main.rs`

- [ ] **Step 1: Implement.**
  - Tab cycle (`main.rs`): `None → claude → codex → copilot → None` on `Option<AgentRef>` (profiles only via `--agent`).
  - `ui.rs` header: chips for CLAUDE, CODEX, COPILOT. For claude/codex: `"ON CALL"` if installed, `"VIA COPILOT"` (in BLUE) if not installed but `pick::resolve` gives a stand-in, else `"MISSING"`. COPILOT chip: `"ON CALL"`, `"NO ACCESS"` (seat false, RED), or `"MISSING"`. Legend gets `■ copilot` in BLUE.
  - Stage box first line: `a.name()` → `if s.via.is_empty() { a.name().to_string() } else { format!("{} via {}", a.name(), s.via) }`; same in the output pane title (`who`).
  - Prompt `[agent: …]` uses `AgentRef::name()` and `agent_color(r.kind())`.
  - `doctor` (`main.rs`): loop over `AgentKind::ALL`; for copilot when on PATH, print the probe:
    ```
    copilot   ok       GitHub Copilot CLI 1.0.95 · logged in as <login> · 14 models enabled
              profile claude    standard → claude-sonnet-4.5 (claude-sonnet-5.5 disabled)
              profile codex     standard → gpt-5.4
    ```
    or `copilot   no access  logged in as <login>, but this account cannot use Copilot CLI (github.com/settings/copilot)` / `copilot   error    <probe error>`. Profile lines: for each of `claude`, `codex` and user profiles, `standard` tier, first usable model or `none enabled`. `agents_found` counts claude/codex installed **or** standing in; the "one CLI only" message becomes "work and review use the same vendor" when `reviewer_for` reports same-vendor for the default agent.
- [ ] **Step 2: Manual check** — `cargo run -- doctor` on this machine (copilot installed, no seat) prints `copilot   no access  logged in as justinholmes ...`; `cargo run -- --demo` header shows three chips (screenshot not needed).
- [ ] **Step 3: Gate**, **Commit** — `git commit -am "feat(ui): show copilot, stand-ins and model access"`

---

### Task 9: Integration tests with a fake `copilot`

**Files:** Modify `tests/automation.rs`

- [ ] **Step 1: Add the stand-in script** next to `CLAUDE`:

```rust
/// `copilot`, speaking its JSONL. `FAKE_COPILOT_REJECT` lists models to refuse the way GitHub does
/// for a model the organisation has switched off. Every argv is appended to `$FAKE_GH/copilot.log`.
const COPILOT: &str = r#"#!/bin/sh
[ "$1" = "--version" ] && { echo "GitHub Copilot CLI stand-in"; exit 0; }
echo "$*" >> "$FAKE_GH/copilot.log"
[ "$1" = "--headless" ] && exit 1
model=""; usage=""
while [ $# -gt 0 ]; do
  case "$1" in --model) model=$2; shift 2 ;; --usage-output-file) usage=$2; shift 2 ;; *) shift ;; esac
done
prompt=$(cat)
for m in $FAKE_COPILOT_REJECT; do
  if [ "$m" = "$model" ]; then
    printf '%s\n' "{\"type\":\"model.call_failure\",\"data\":{\"model\":\"$model\",\"statusCode\":400,\"errorCode\":\"model_not_supported\"}}"
    printf '%s\n' '{"type":"session.error","data":{"errorType":"query","message":"400 The requested model is not supported."}}'
    printf '%s\n' '{"type":"result","exitCode":1,"usage":{"premiumRequests":0}}'
    exit 1
  fi
done
[ -n "$usage" ] && printf '{"modelMetrics":{"%s":{"usage":{"inputTokens":10,"outputTokens":2}}}}' "$model" > "$usage"
case "$prompt" in
  *"Do not modify any file"*)
    printf '%s\n' "{\"type\":\"assistant.message\",\"data\":{\"content\":\"Looked at it.\\nVERDICT: ${FAKE_VERDICT:-PASS}\",\"model\":\"$model\"}}" ;;
  *)
    printf '%s\n' '{"type":"tool.execution_start","data":{"toolName":"edit","arguments":{"path":"by_agent.txt"}}}'
    sh -c "${FAKE_WORK:-echo change >> by_agent.txt}" >/dev/null 2>&1
    printf '%s\n' "{\"type\":\"assistant.message\",\"data\":{\"content\":\"Did the work.\",\"model\":\"$model\"}}" ;;
esac
printf '%s\n' '{"type":"result","exitCode":0,"usage":{"premiumRequests":1}}'
"#;
```

`Rig::new` gets a sibling constructor `Rig::with_agents(name, extra_config, repo_config, agents: &[(&str, &str)])` that writes only the given agent scripts (plus `gh`, `mcp-stand-in`); `Rig::new` keeps writing `claude` only. The fake `--headless` exits 1, so the probe reports an error and every configured model is tried (the policy path is covered by `pick` unit tests).

- [ ] **Step 2: Tests**

```rust
fn copilot_log(rig: &Rig) -> String {
    fs::read_to_string(rig.root.join("gh/copilot.log")).unwrap_or_default()
}

#[test]
fn with_only_copilot_work_and_review_come_from_different_vendors() {
    let rig = Rig::with_agents("copilot-only", "", "", &[("copilot", COPILOT)]);
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let log = copilot_log(&rig);
    let runs: Vec<&str> = log.lines().filter(|l| !l.starts_with("--headless")).collect();
    assert_eq!(runs.len(), 2, "{}", log);
    assert!(runs[0].contains("--model claude-") && runs[0].contains("--allow-tool write"), "worker: {}", runs[0]);
    assert!(runs[1].contains("--model gpt-") && runs[1].contains("--deny-tool write"), "reviewer: {}", runs[1]);
    assert!(!log.contains("allow-all") && !log.contains("--yolo"));
    assert!(rig.calls().contains("pr create"), "{}", rig.calls());
}

#[test]
fn a_model_the_organisation_switched_off_is_skipped() {
    let rig = Rig::with_agents("copilot-reject", "", "", &[("copilot", COPILOT)]);
    rig.issue();
    let (ok, out) = rig.once(&[("FAKE_COPILOT_REJECT", "claude-sonnet-5.5 claude-sonnet-5")]);
    assert!(ok, "{}", out);
    let log = copilot_log(&rig);
    let worker_models: Vec<&str> = log.lines().filter(|l| l.contains("--allow-tool write")).map(|l| l.split("--model ").nth(1).unwrap().split(' ').next().unwrap()).collect();
    assert_eq!(worker_models, ["claude-sonnet-5.5", "claude-sonnet-5", "claude-sonnet-4.6"]);
    assert!(out.contains("not available to this copilot account"), "{}", out);
}

#[test]
fn with_claude_installed_copilot_reviews_as_codex() {
    let rig = Rig::with_agents("claude-plus-copilot", "", "", &[("claude", CLAUDE), ("copilot", COPILOT)]);
    rig.issue();
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    let runs: Vec<String> = copilot_log(&rig).lines().filter(|l| !l.starts_with("--headless")).map(String::from).collect();
    assert_eq!(runs.len(), 1, "only the review runs on copilot: {:?}", runs);
    assert!(runs[0].contains("--model gpt-") && runs[0].contains("--deny-tool write"));
}

#[test]
fn a_copilot_profile_reviews_pull_requests_next_to_claude() {
    let extra = "[agents.copilot.profiles.second]\nvendor = \"xai\"\nmodels = { light = \"grok-4.7\", standard = \"grok-4.7\", heavy = \"grok-4.7\" }\n[review]\npr_reviewers = [\"claude\", \"copilot:second\"]";
    let rig = Rig::with_agents("copilot-profile-pr", extra, "", &[("claude", CLAUDE), ("copilot", COPILOT)]);
    rig.pull_request(12, "feature");
    rig.list_pull_request(12, "feature");
    let (ok, out) = rig.once(&[]);
    assert!(ok, "{}", out);
    assert!(copilot_log(&rig).contains("--model grok-4.7"), "{}", copilot_log(&rig));
    let posted = rig.calls();
    assert!(posted.contains("copilot:second"), "the PR comment names the profile: {}", posted);
}

#[test]
fn a_truncated_copilot_stream_is_not_a_pass() {
    let rig = Rig::with_agents("copilot-truncated", "", "", &[("copilot", COPILOT)]);
    rig.issue();
    // The review prints its verdict but never the closing `result` line.
    let (_, out) = rig.once(&[("FAKE_DURING_REVIEW", "true")]);
    let _ = out; // replaced below
}
```

The truncated case needs a switch in the script: add `[ -n "$FAKE_COPILOT_TRUNCATE" ] && case "$prompt" in *"Do not modify any file"*) exit 0 ;; esac` right before the final `result` printf, then assert `rig.calls()` does **not** contain `pr create` and contains the label that marks an incomplete review (as `an_incomplete_review_goes_back_to_the_request_label` does), and `out` contains `exited before reporting a result`. Write the final version of that test with `FAKE_COPILOT_TRUNCATE=1` and those asserts; delete the placeholder body above.

PR-review test: check which existing helper exposes the posted comment body (see `a_pull_request_that_stays_put_is_passed_with_its_commit_named` and the `GH` script `--input`); assert on that body instead of `calls()` if `calls()` does not include it.

- [ ] **Step 3: Run** — `cargo test --test automation copilot` and `... profile` → PASS; full gate.
- [ ] **Step 4: Commit** — `git commit -am "test: copilot as stand-in, reviewer profile and refused models"`

---

### Task 10: Real-binary test against a fake model server

**Files:** Create `tests/copilot_byok.rs`

- [ ] **Step 1: Write the test** (`#[ignore]`: needs `copilot` on PATH; run with `cargo test --test copilot_byok -- --ignored`). It:
  1. Starts a `std::net::TcpListener` on `127.0.0.1:0` in a thread serving `POST /v1/chat/completions` (and `GET` → `{"object":"list","data":[]}`) with SSE as the scratchpad `fake_openai.py` does: first request without a `tool` message → one `tool_calls` chunk for the tool name in `FAKE_TOOL` (default `view`) with `{"path": "<file>"}`; otherwise text `"All good.\nVERDICT: PASS"`. Each response: `data: {...}\n\n` chunks, a usage chunk, `data: [DONE]\n\n`, `Connection: close`.
  2. Runs `copilot` with exactly the args from `agents::copilot_args` (call it: the test is in the same crate? No — integration tests can't reach private items; duplicate the default worker/reviewer arg lists in the test as constants and note they must match `AgentCfg::copilot()`), env `COPILOT_OFFLINE=true`, `COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:<port>/v1`, `COPILOT_MODEL=fake-model`, `COPILOT_PROVIDER_MAX_PROMPT_TOKENS=100000`, `COPILOT_PROVIDER_MAX_OUTPUT_TOKENS=4000`, `HOME` set to a temp dir (no personal skills), cwd a temp dir with `notes.txt`.
  3. Asserts on stdout lines: a `tool.execution_start` for `view`, an `assistant.message` with the verdict, last line `{"type":"result",...,"exitCode":0}`; usage file has `modelMetrics.fake-model.usage.inputTokens > 0`.
  4. Reviewer args with `FAKE_TOOL=edit` (server asks for an edit of notes.txt): the file is unchanged afterwards and a `tool.execution_complete` with `"success":false` or no `tool.execution_start` for `edit` appears.
  5. Server answering every request with HTTP 400 and the `model_not_supported` body (from the design doc) → `model.call_failure` with that `errorCode`, `result.exitCode` 1, no `tool.execution_start`.

- [ ] **Step 2: Run** — `cargo test --test copilot_byok -- --ignored --nocapture` → PASS on this machine (copilot 1.0.95 installed). If step 4 shows copilot *prompts* or hangs instead of denying, stop and report: the write/review args need changing.
- [ ] **Step 3: Gate** (ignored tests don't run in plain `cargo test`), **Commit** — `git commit -am "test: check the real copilot binary against a local fake model server"`

---

### Task 11: Docs

**Files:** `README.md`, `forkdeck.example.toml` (done in Task 2), `docs/copilot-plan.md`

- [ ] **Step 1: README**: intro and install section: "You need at least one of `claude`, `codex` or the GitHub Copilot CLI (`copilot`) on your PATH." New subsection "GitHub Copilot CLI" covering: stand-in behaviour, profiles + `copilot:<name>`, org model policies (forkdeck asks copilot which models are enabled and skips the rest; refused models are skipped for the session), no tokens in JSON output so usage comes from the usage file, cost column empty, `doctor` output, BYOK test command. `--agent` and Tab descriptions mention copilot. Remove the "planned" line if present.
- [ ] **Step 2: Design doc** fixes: tier names `light/standard/heavy`; router section: router stays two-way (drop `copilot_is_for`/`copilot_keywords`, jev third option); `write_args` = write only (no `shell(git:*)`); status line → "implemented on branch copilot-cli".
- [ ] **Step 3: Gate**, **Commit** — `git commit -am "docs: GitHub Copilot CLI support"`

---

## Self-review notes

- Spec coverage: stand-in (T3, T7, T9), profiles + reviewers (T2, T3, T7, T9), org policy probe (T6) + rejection rerun (T7, T9), no-auto (T2, T3), vendor flag incl. rate-limit switch (T4 note + T7 models_used check), usage tokens (T5), doctor/TUI (T8), skills excluded for reviewers (T2), BYOK drift test (T10), docs (T11). Router unchanged (constraint).
- Known soft spot: Task 9's truncated-stream and PR-comment assertions name the expected behaviour, but the exact helper for the posted PR body must be read from existing tests at implementation time.
