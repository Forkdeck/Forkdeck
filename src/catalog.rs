//! The models a CLI offers this account, so the effort tiers use them instead of one default.

use serde_json::Value;

/// One model the CLI lists for the user.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub slug: String,
    pub description: String,
    pub efforts: Vec<String>,
    /// The CLI's own order: lower is newer and preferred.
    pub priority: i64,
}

/// The models in `codex debug models` output that are listed for the user, newest first.
pub fn codex_models(json: &str) -> Vec<Model> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return vec![] };
    let list = v.get("models").unwrap_or(&v).as_array().cloned().unwrap_or_default();
    let mut out: Vec<Model> = list.iter().filter(|m| m.get("visibility").and_then(Value::as_str) == Some("list")).filter_map(|m| {
        Some(Model {
            slug: m.get("slug")?.as_str()?.to_string(),
            description: m.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
            efforts: m.get("supported_reasoning_levels").and_then(Value::as_array).map(|l| {
                l.iter().filter_map(|e| e.get("effort").and_then(Value::as_str).map(String::from)).collect()
            }).unwrap_or_default(),
            priority: m.get("priority").and_then(Value::as_i64).unwrap_or(i64::MAX),
        })
    }).collect();
    out.sort_by_key(|m| m.priority);
    out
}

/// Which tier a model's own description puts it in. None when it says nothing that fits, or
/// that it is an older generation: an older model of the same kind is never the best choice.
fn tier_of(description: &str) -> Option<usize> {
    let d = description.to_ascii_lowercase();
    if d.contains("previous generation") || d.contains("older") || d.contains("legacy") {
        return None;
    }
    if ["frontier", "most demanding", "most capable", "hardest"].iter().any(|w| d.contains(w)) {
        Some(2)
    } else if ["fast", "affordable", "efficient", "easier", "lightweight", "mini"].iter().any(|w| d.contains(w)) {
        Some(0)
    } else if ["workhorse", "balanced", "everyday", "general"].iter().any(|w| d.contains(w)) {
        Some(1)
    } else {
        None
    }
}

/// The effort a tier asks for, if the model supports it, else the nearest one it does.
fn effort_for(tier: usize, efforts: &[String]) -> String {
    let wanted = ["low", "medium", "high"][tier];
    if efforts.is_empty() || efforts.iter().any(|e| e == wanted) {
        return wanted.to_string();
    }
    let order = ["minimal", "low", "medium", "high", "xhigh", "max"];
    let at = order.iter().position(|o| *o == wanted).unwrap_or(2) as i64;
    efforts.iter().min_by_key(|e| order.iter().position(|o| o == e).map(|p| (p as i64 - at).abs()).unwrap_or(99)).cloned().unwrap_or_else(|| wanted.to_string())
}

/// Model and effort for light, standard and heavy: the newest listed model whose description
/// fits each tier. A tier nothing fits keeps None, and the CLI's own default is used there.
pub fn tiers(models: &[Model]) -> [Option<(String, String)>; 3] {
    let mut out: [Option<(String, String)>; 3] = [None, None, None];
    for m in models {
        if let Some(t) = tier_of(&m.description) {
            if out[t].is_none() {
                out[t] = Some((m.slug.clone(), effort_for(t, &m.efforts)));
            }
        }
    }
    out
}

/// Ask the installed Codex for its catalog. Empty when it cannot say.
pub fn codex_installed(cmd: &str) -> Vec<Model> {
    std::process::Command::new(cmd).args(["debug", "models"]).stderr(std::process::Stdio::null()).output().ok()
        .filter(|o| o.status.success()).map(|o| codex_models(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `codex debug models`.
    const CATALOG: &str = r#"{"models":[
      {"slug":"gpt-6.1-sol","description":"Latest workhorse model for coding and everyday work.","visibility":"list","priority":1,
       "supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"}]},
      {"slug":"gpt-6-astra","description":"Frontier intelligence for the most demanding work.","visibility":"list","priority":2,
       "supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"max"}]},
      {"slug":"gpt-6-sol","description":"Previous generation workhorse model.","visibility":"list","priority":3,"supported_reasoning_levels":[]},
      {"slug":"gpt-6-luna","description":"Fast and affordable model for easier tasks.","visibility":"list","priority":4,
       "supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"}]},
      {"slug":"gpt-reserve","description":"Fast reserve.","visibility":"hide","priority":4,"supported_reasoning_levels":[]},
      {"slug":"gpt-5.6-luna","description":"Older fast and efficient model.","visibility":"list","priority":9,"supported_reasoning_levels":[]}
    ]}"#;

    #[test]
    fn each_tier_gets_the_newest_model_whose_description_fits() {
        let t = tiers(&codex_models(CATALOG));
        assert_eq!(t[0], Some(("gpt-6-luna".into(), "low".into())));
        assert_eq!(t[1], Some(("gpt-6.1-sol".into(), "medium".into())));
        assert_eq!(t[2], Some(("gpt-6-astra".into(), "high".into())));
    }

    #[test]
    fn hidden_and_older_models_are_never_picked() {
        let m = codex_models(CATALOG);
        assert!(!m.iter().any(|m| m.slug == "gpt-reserve"), "hidden");
        let t = tiers(&m);
        assert!(t.iter().flatten().all(|(s, _)| s != "gpt-6-sol" && s != "gpt-5.6-luna"), "{t:?}");
    }

    #[test]
    fn an_effort_the_model_lacks_becomes_the_nearest_it_has() {
        assert_eq!(effort_for(2, &["low".into(), "medium".into()]), "medium");
        assert_eq!(effort_for(0, &["medium".into(), "high".into()]), "medium");
        assert_eq!(effort_for(1, &[]), "medium");
    }

    #[test]
    fn nothing_readable_means_no_tiers() {
        assert!(codex_models("not json").is_empty());
        assert_eq!(tiers(&[]), [None, None, None]);
    }
}

/// Fill the Codex tiers the user left unset from the models Codex lists for this account, so
/// light, standard and heavy work go to different models instead of one default at three efforts.
/// Returns what was chosen, for the startup log.
pub fn fill_codex(cfg: &mut crate::config::Config) -> Vec<String> {
    let cmd = cfg.agents.codex.cmd.clone().unwrap_or_else(|| "codex".into());
    let found = tiers(&codex_installed(&cmd));
    let models = cfg.agents.codex.models.get_or_insert_with(Default::default);
    let unset = |v: &Option<String>| v.as_deref().is_none_or(str::is_empty);
    let mut chosen = Vec::new();
    let mut efforts = cfg.agents.codex.efforts.clone().unwrap_or_default();
    for (i, slot) in [&mut models.light, &mut models.standard, &mut models.heavy].into_iter().enumerate() {
        if let (true, Some((model, effort))) = (unset(slot), &found[i]) {
            *slot = Some(model.clone());
            let e = [&mut efforts.light, &mut efforts.standard, &mut efforts.heavy][i].get_or_insert_with(|| effort.clone()).clone();
            chosen.push(format!("{} {} ({})", ["light", "standard", "heavy"][i], model, e));
        }
    }
    cfg.agents.codex.efforts = Some(efforts);
    chosen
}
