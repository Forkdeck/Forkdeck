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
    /// Models the account may use, as copilot reported them. None: not known, so try them all.
    pub enabled: Option<&'a HashSet<String>>,
    /// Copilot is installed, but the account cannot use it.
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
    /// The part it plays: claude or codex for a stand-in, else the CLI itself.
    pub family: AgentKind,
    /// "copilot" for a stand-in, "copilot:<name>" for a profile.
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
                return Some(Exec {
                    asked: r.clone(),
                    cli: *k,
                    family: *k,
                    via: None,
                    model: c.models.map(|m| m.get(tier).to_string()).unwrap_or_default(),
                    effort: c.efforts.map(|m| m.get(tier).to_string()).unwrap_or_default(),
                    vendor: if *k == AgentKind::Claude { Vendor::Anthropic } else { Vendor::OpenAI },
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
            // A pinned model the account may not use falls back to the CLI's own choice, once; it is never asked for again.
            let model = c.models.map(|m| m.get(tier).to_string()).filter(|m| m.is_empty() || m == "auto" || a.usable(m)).unwrap_or_default();
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

/// The worker for a task: what was asked for, else the other of claude and codex, else copilot itself.
pub fn worker_for(cfg: &Config, a: &Avail, wanted: &AgentRef, tier: Tier) -> Option<Exec> {
    let mut tries = vec![wanted.clone()];
    if let AgentRef::Kind(k @ (AgentKind::Claude | AgentKind::Codex)) = wanted {
        tries.push(AgentRef::Kind(k.other()));
        tries.push(AgentRef::Kind(AgentKind::Copilot));
    }
    tries.iter().find_map(|r| resolve(cfg, a, r, tier))
}

/// The reviewer. With `reviewer = "other"`, the first of `review.reviewer_order` that is available
/// and runs a different vendor's model than the worker. Otherwise, or when there is none, the
/// worker's own agent at the review tier. The flag says the review does not cross vendors.
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

#[cfg(test)]
mod tests {
    use super::*;
    use AgentKind::*;

    fn cfg(text: &str) -> Config {
        let c: Config = toml::from_str(text).unwrap();
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
        assert_eq!(vendor_of_model("opus"), Vendor::Unknown);
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
        assert_eq!((e.cli, e.via.clone(), e.model.as_str(), e.label()), (Claude, None, "sonnet", "claude".to_string()));

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
        let nothing = HashSet::new();
        let a = Avail { installed: &nothing, enabled: None, no_seat: false, rejected: &none };
        for r in [k(Claude), k(Codex), k(Copilot), AgentRef::Profile("claude".into())] {
            assert!(resolve(&c, &a, &r, Tier::Standard).is_none());
        }
    }

    #[test]
    fn org_policy_and_refusals_skip_models() {
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
        let second = AgentRef::Profile("second".into());
        let e = resolve(&c, &a, &second, Tier::Standard).unwrap();
        assert_eq!((e.model.as_str(), e.vendor.clone(), e.label()), ("grok-4.7", Vendor::Other("xai".into()), "copilot:second".to_string()));
        assert!(resolve(&c, &a, &second, Tier::Heavy).is_none(), "a tier without models");
        assert!(resolve(&c, &a, &AgentRef::Profile("nope".into()), Tier::Standard).is_none());
    }

    #[test]
    fn a_pinned_copilot_model_obeys_policy_and_refusals() {
        let c = cfg("[agents.copilot]\nmodels = { standard = \"gpt-5.5\" }");
        let only = set(&[Copilot]);
        let none = HashSet::new();
        let a = Avail { installed: &only, enabled: None, no_seat: false, rejected: &none };
        assert_eq!(resolve(&c, &a, &k(Copilot), Tier::Standard).unwrap().model, "gpt-5.5");
        let enabled = set(&["gpt-5.4".to_string()]);
        let a = Avail { installed: &only, enabled: Some(&enabled), no_seat: false, rejected: &none };
        assert_eq!(resolve(&c, &a, &k(Copilot), Tier::Standard).unwrap().model, "", "switched off: the CLI chooses");
        let rejected = set(&["gpt-5.5".to_string()]);
        let a = Avail { installed: &only, enabled: None, no_seat: false, rejected: &rejected };
        assert_eq!(resolve(&c, &a, &k(Copilot), Tier::Standard).unwrap().model, "", "refused once: never asked for again");
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
        assert_eq!((r.cli, same), (Codex, true), "only one CLI: it reviews its own work, flagged");

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
        assert_eq!((r.cli, r.effort.as_str(), same), (Codex, "high", true), "the same agent at the review tier");

        let all = set(&[Claude, Codex, Copilot]);
        let a = Avail { installed: &all, enabled: None, no_seat: false, rejected: &none };
        let w = worker_for(&c, &a, &k(Copilot), Tier::Standard).unwrap();
        assert!(reviewer_for(&c, &a, &w, Tier::Heavy).1, "copilot's own model has an unknown vendor");
    }
}
