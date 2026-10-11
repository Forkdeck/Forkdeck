//! The fork layer: a handful of typed questions answered before any agent starts.

use crate::config::Config;
use crate::state::{AgentKind, Decision, Gate, Tier};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub struct Routed {
    pub decision: Decision,
    /// Set when the Jev endpoint was asked and did not answer.
    pub fell_back: Option<String>,
    pub asked_jev: bool,
}

pub async fn route(cfg: &Config, http: &reqwest::Client, task: &str, is_issue: bool) -> Routed {
    let start = Instant::now();
    if cfg.demo {
        tokio::time::sleep(Duration::from_millis(350)).await;
        let mut d = rules(cfg, task, is_issue);
        d.source = "jev (simulated)".into();
        d.ms = start.elapsed().as_millis();
        return Routed { decision: d, fell_back: None, asked_jev: true };
    }
    if cfg.router.kind == "jev" {
        match ask_jev(cfg, http, task).await {
            Ok(v) => match from_answers(cfg, &v) {
                Ok(mut d) => {
                    d.ms = start.elapsed().as_millis();
                    return Routed { decision: d, fell_back: None, asked_jev: true };
                }
                Err(e) => {
                    let mut d = rules(cfg, task, is_issue);
                    d.ms = start.elapsed().as_millis();
                    return Routed { decision: d, fell_back: Some(e), asked_jev: true };
                }
            },
            Err(e) => {
                let mut d = rules(cfg, task, is_issue);
                d.ms = start.elapsed().as_millis();
                return Routed { decision: d, fell_back: Some(e), asked_jev: true };
            }
        }
    }
    let mut d = rules(cfg, task, is_issue);
    d.ms = start.elapsed().as_millis();
    Routed { decision: d, fell_back: None, asked_jev: false }
}

pub fn request_body(cfg: &Config, task: &str) -> Value {
    let j = &cfg.router.jev;
    // Keep the state short: decision servers have small context windows.
    let state: String = task.chars().take(6000).collect();
    json!({
        "state": state,
        "model": j.model,
        "questions": {
            "agent": {
                "type": "choice",
                "instructions": "Which coding agent should take this task?",
                "criteria": { "claude": j.claude_is_for, "codex": j.codex_is_for }
            },
            "effort": {
                "type": "score",
                "instructions": "How much reasoning effort does this task need?",
                "criteria": [
                    "Trivial: a typo, a rename, a one-line change",
                    "Routine: one feature or bug touching a few files",
                    "Hard: cross-cutting design, concurrency, data migration, security"
                ]
            },
            "review": {
                "type": "noul",
                "instructions": "This change is risky enough that someone else should review it before it is published"
            }
        }
    })
}

async fn ask_jev(cfg: &Config, http: &reqwest::Client, task: &str) -> Result<Value, String> {
    let j = &cfg.router.jev;
    let url = format!("{}/v1/systemone", j.base_url.trim_end_matches('/'));
    let mut req = http
        .post(&url)
        .timeout(Duration::from_millis(j.timeout_ms))
        .json(&request_body(cfg, task));
    if !j.api_key_env.is_empty() {
        if let Ok(key) = std::env::var(&j.api_key_env) {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }
    }
    let resp = req.send().await.map_err(|e| {
        if e.is_timeout() {
            format!("{} timed out after {} ms", url, j.timeout_ms)
        } else if e.is_connect() {
            format!("cannot connect to {}", url)
        } else {
            format!("{}: {}", url, e)
        }
    })?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let short: String = text.chars().take(160).collect();
        return Err(format!("{} answered {}: {}", url, status.as_u16(), short));
    }
    serde_json::from_str(&text).map_err(|e| format!("{} sent invalid JSON: {}", url, e))
}

/// Turn a `/v1/systemone` response into a decision.
pub fn from_answers(cfg: &Config, v: &Value) -> Result<Decision, String> {
    let th = cfg.router.threshold;
    let a = v.get("answers").ok_or("response has no \"answers\" object")?;
    let mut note = Vec::new();

    let agent_ans = a.get("agent").ok_or("no answer for \"agent\"")?;
    let choice = agent_ans.get("choice").and_then(|c| c.as_str()).ok_or("agent answer has no choice")?;
    let conf = unit(
        agent_ans
            .get("confidence")
            .and_then(|c| c.as_f64())
            .or_else(|| agent_ans.pointer(&format!("/probabilities/{}", choice)).and_then(|c| c.as_f64()))
            .unwrap_or(0.0),
        "agent confidence",
    )?;
    let picked = match choice {
        "claude" => AgentKind::Claude,
        "codex" => AgentKind::Codex,
        other => return Err(format!("agent answer was {:?}, expected claude or codex", other)),
    };
    let agent_sharp = conf >= th;
    let agent = if agent_sharp {
        picked
    } else {
        note.push(format!("agent split, using default {}", cfg.router.default_agent.name()));
        cfg.router.default_agent
    };

    let eff = a.get("effort").ok_or("no answer for \"effort\"")?;
    let score = eff.get("score").and_then(|s| s.as_f64()).ok_or("effort answer has no score")?;
    if !(0.0..=2.0).contains(&score) {
        return Err(format!("effort score is {}, expected 0 to 2", score));
    }
    // A missing confidence counts as unsure, which rounds the effort up rather than down.
    let econf = unit(eff.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0), "effort confidence")?;
    let effort_sharp = econf >= th;
    let mut tier = match score.round() as i64 {
        i64::MIN..=0 => Tier::Light,
        1 => Tier::Standard,
        _ => Tier::Heavy,
    };
    if !effort_sharp {
        tier = tier.up();
        note.push("effort split, one tier up".to_string());
    }

    let rv = unit(
        a.get("review").and_then(|r| r.get("noul")).and_then(|n| n.as_f64()).ok_or("no noul for \"review\"")?,
        "review noul",
    )?;
    // A yes/no probability is sharp when it is far from the middle.
    let review_sharp = (rv - 0.5).abs() >= (th - 0.5).max(0.05);
    // Any unsure answer buys a review.
    let review = rv >= 0.5 || !review_sharp || !agent_sharp || !effort_sharp;

    Ok(Decision {
        agent,
        tier,
        review,
        gates: vec![
            Gate { name: "which agent".into(), answer: picked.name().into(), value: conf, sharp: agent_sharp },
            Gate { name: "effort".into(), answer: tier.name().into(), value: econf, sharp: effort_sharp },
            Gate {
                name: "needs review".into(),
                answer: if review { "yes".into() } else { "no".into() },
                value: rv,
                sharp: review_sharp,
            },
        ],
        source: "jev".into(),
        ms: 0,
        note: note.join("; "),
    })
}

fn unit(v: f64, what: &str) -> Result<f64, String> {
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{} is {}, expected a value between 0 and 1", what, v))
    }
}

fn hits(text: &str, words: &[String]) -> usize {
    words
        .iter()
        .filter(|w| {
            let w = w.to_lowercase();
            text.split(|c: char| !c.is_alphanumeric()).any(|t| t == w)
        })
        .count()
}

/// Keyword rules. The numbers are evidence counts squashed into 0..1, not
/// calibrated probabilities; they exist so both routers draw the same bars.
pub fn rules(cfg: &Config, task: &str, is_issue: bool) -> Decision {
    let th = cfg.router.threshold;
    let r = &cfg.router.rules;
    let text = task.to_lowercase();
    let c = hits(&text, &r.claude_keywords) as f64;
    let x = hits(&text, &r.codex_keywords) as f64;
    let (picked, conf) = if c == x {
        (cfg.router.default_agent, 0.5)
    } else {
        let p = (0.5 + 0.2 * (c - x).abs()).min(0.95);
        (if c > x { AgentKind::Claude } else { AgentKind::Codex }, p)
    };
    let agent_sharp = conf >= th;
    let agent = if agent_sharp { picked } else { cfg.router.default_agent };

    let heavy = hits(&text, &r.heavy_keywords);
    let light = hits(&text, &r.light_keywords);
    let long = text.split_whitespace().count() > 120;
    let (mut tier, econf) = if heavy > 0 {
        (Tier::Heavy, (0.6 + 0.15 * heavy as f64).min(0.95))
    } else if light > 0 && !long {
        (Tier::Light, (0.6 + 0.15 * light as f64).min(0.95))
    } else {
        (Tier::Standard, 0.65)
    };
    let effort_sharp = econf >= th;
    let mut note: Vec<String> = Vec::new();
    if !agent_sharp {
        note.push(format!("no clear match, using default {}", agent.name()));
    }
    if !effort_sharp {
        tier = tier.up();
        note.push("effort split, one tier up".into());
    }

    let rv: f64 = if tier == Tier::Heavy {
        0.9
    } else if is_issue {
        0.8
    } else if tier == Tier::Light {
        0.15
    } else {
        0.6
    };
    let review_sharp = (rv - 0.5).abs() >= 0.09;
    // Same rule as the Jev path: any unsure answer buys a review.
    let review = rv >= 0.5 || !review_sharp || !agent_sharp || !effort_sharp;
    Decision {
        agent,
        tier,
        review,
        gates: vec![
            Gate { name: "which agent".into(), answer: agent.name().into(), value: conf, sharp: agent_sharp },
            Gate { name: "effort".into(), answer: tier.name().into(), value: econf, sharp: effort_sharp },
            Gate { name: "needs review".into(), answer: if review { "yes".into() } else { "no".into() }, value: rv, sharp: review_sharp },
        ],
        source: "rules".into(),
        ms: 0,
        note: note.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_response_shape() {
        let cfg = Config::default();
        let v = json!({
            "model": "jev-1.13.0",
            "answers": {
                "agent": {"type": "choice", "choice": "codex", "confidence": 0.85,
                          "probabilities": {"claude": 0.15, "codex": 0.85}},
                "effort": {"type": "score", "score": 2.0, "confidence": 0.9,
                           "probabilities": {"0": 0.0, "1": 0.1, "2": 0.9}},
                "review": {"type": "noul", "noul": 0.92}
            },
            "usage": {"input_tokens": 100, "output_tokens": 10}
        });
        let d = from_answers(&cfg, &v).unwrap();
        assert_eq!(d.agent, AgentKind::Codex);
        assert_eq!(d.tier, Tier::Heavy);
        assert!(d.review);
        assert!(d.gates.iter().all(|g| g.sharp));
    }

    #[test]
    fn split_answers_take_the_cautious_branch() {
        let cfg = Config::default();
        let v = json!({"answers": {
            "agent": {"choice": "codex", "confidence": 0.47},
            "effort": {"score": 0.0, "confidence": 0.4},
            "review": {"noul": 0.2}
        }});
        let d = from_answers(&cfg, &v).unwrap();
        assert_eq!(d.agent, AgentKind::Claude, "falls back to the default agent");
        assert_eq!(d.tier, Tier::Standard, "one tier up");
        assert!(d.review, "an unsure agent choice forces a review");
    }

    #[test]
    fn rejects_out_of_range_and_treats_missing_confidence_as_unsure() {
        let cfg = Config::default();
        let v = json!({"answers": {"agent": {"choice": "codex", "confidence": 3.0},
            "effort": {"score": 1.0, "confidence": 0.9}, "review": {"noul": 0.9}}});
        assert!(from_answers(&cfg, &v).is_err());
        let v = json!({"answers": {"agent": {"choice": "codex", "confidence": 0.9},
            "effort": {"score": -3.0, "confidence": 0.9}, "review": {"noul": 0.9}}});
        assert!(from_answers(&cfg, &v).is_err());
        let v = json!({"answers": {"agent": {"choice": "codex", "confidence": 0.9},
            "effort": {"score": 0.0}, "review": {"noul": 0.05}}});
        let d = from_answers(&cfg, &v).unwrap();
        assert_eq!(d.tier, Tier::Standard, "no effort confidence: one tier up");
        assert!(d.review, "an unsure effort answer forces a review");
    }

    #[test]
    fn rejects_unknown_agent() {
        let cfg = Config::default();
        let v = json!({"answers": {"agent": {"choice": "gemini", "confidence": 0.9},
            "effort": {"score": 1.0}, "review": {"noul": 0.5}}});
        assert!(from_answers(&cfg, &v).is_err());
    }

    #[test]
    fn rules_pick_by_keyword() {
        let cfg = Config::default();
        let d = rules(&cfg, "Fix the failing test for the crash in parser", false);
        assert_eq!(d.agent, AgentKind::Codex);
        let d = rules(&cfg, "Fix typo in heading", false);
        assert_eq!(d.tier, Tier::Light);
        assert!(!d.review);
        // No keyword for either agent: the choice is unsure, so a review is forced.
        let d = rules(&cfg, "rename foo", false);
        assert_eq!(d.tier, Tier::Light);
        assert!(d.review);
        let d = rules(&cfg, "Redesign the storage migration", true);
        assert_eq!(d.tier, Tier::Heavy);
        assert!(d.review);
    }
}
