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
    if cfg.router.kind == "jev" || cfg.router.kind == "auto" {
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
                // "auto" asks a decision server only if one is running: none is not a failure.
                if cfg.router.kind == "auto" && e.starts_with("cannot connect") {
                    return Routed { decision: d, fell_back: None, asked_jev: false };
                }
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
    let mut body = json!({
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
            },
            "plan": {
                "type": "noul",
                "instructions": "The task is open enough that the user should choose between approaches before any work starts"
            }
        }
    });
    if cfg.router.plan == "never" {
        if let Some(q) = body.get_mut("questions").and_then(|q| q.as_object_mut()) {
            q.remove("plan");
        }
    }
    body
}

/// The development key scripts/local-jev.sh serves with unless FORKDECK_JEV_KEY says otherwise.
const LOCAL_DEV_KEY: &str = "forkdeck-local-dev";

async fn ask_jev(cfg: &Config, http: &reqwest::Client, task: &str) -> Result<Value, String> {
    let j = &cfg.router.jev;
    let url = format!("{}/v1/systemone", j.base_url.trim_end_matches('/'));
    let mut req = http
        .post(&url)
        .timeout(Duration::from_millis(j.timeout_ms))
        .json(&request_body(cfg, task));
    let key = Some(&j.api_key_env).filter(|n| !n.is_empty()).and_then(|n| std::env::var(n).ok()).filter(|k| !k.is_empty());
    // With no key set, "auto" on this machine uses the one scripts/local-jev.sh starts its server with.
    let key = key.or_else(|| (cfg.router.kind == "auto" && crate::orchestrator::is_loopback(&j.base_url)).then(|| LOCAL_DEV_KEY.to_string()));
    if let Some(key) = key {
        req = req.bearer_auth(key);
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

    let mut gates = vec![
        Gate { name: "which agent".into(), answer: picked.name().into(), value: conf, sharp: agent_sharp },
        Gate { name: "effort".into(), answer: tier.name().into(), value: econf, sharp: effort_sharp },
        Gate {
            name: "needs review".into(),
            answer: if review { "yes".into() } else { "no".into() },
            value: rv,
            sharp: review_sharp,
        },
    ];
    // A server that does not know the plan question leaves it out: then there is no plan.
    let mut plan = false;
    if cfg.router.plan != "never" {
        if let Some(pv) = a.get("plan").and_then(|r| r.get("noul")).and_then(|n| n.as_f64()) {
            let pv = unit(pv, "plan noul")?;
            let sharp = (pv - 0.5).abs() >= (th - 0.5).max(0.05);
            // Unlike a review, an unsure plan question is not worth a stop: only a clear yes asks the user.
            plan = pv >= 0.5 && sharp;
            gates.push(Gate { name: "plan first".into(), answer: if plan { "yes".into() } else { "no".into() }, value: pv, sharp });
        }
    }
    Ok(Decision { agent, tier, review, gates, source: "jev".into(), ms: 0, note: note.join("; "), plan, trial_from: None })
}

fn unit(v: f64, what: &str) -> Result<f64, String> {
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{} is {}, expected a value between 0 and 1", what, v))
    }
}

/// A word reduced to a common form, so "fixes", "fixed" and "fixing" all meet "fix", and
/// "renamed" meets "rename". Crude on purpose: both sides go through the same reduction.
fn stem(w: &str) -> String {
    let mut w = w.to_string();
    let n = w.len();
    let mut cut = false;
    for suffix in ["ing", "ed"] {
        if w.ends_with(suffix) && n - suffix.len() >= 3 {
            w.truncate(n - suffix.len());
            cut = true;
            break;
        }
    }
    if !cut {
        if ["sses", "xes", "ches", "shes"].iter().any(|s| w.ends_with(s)) {
            w.truncate(n - 2);
        } else if w.ends_with('s') && !w.ends_with("ss") && n > 3 {
            w.truncate(n - 1);
        }
    }
    // "planning" -> "plann" -> "plan"; "called" keeps its double l.
    let b = w.as_bytes();
    if cut && b.len() >= 4 && b[b.len() - 1] == b[b.len() - 2] && !b"aeiouslz".contains(&b[b.len() - 1]) {
        w.pop();
    }
    if w.len() > 3 && w.ends_with('e') {
        w.pop();
    }
    w
}

/// Lowercase words, split on anything that is not a letter or digit ("one-line" is two words).
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| stem(&t.to_lowercase())).collect()
}

/// How many of the keywords appear in the text. A keyword may be a phrase: its words must
/// appear next to each other.
fn hits(text: &str, keywords: &[String]) -> usize {
    let words = tokens(text);
    keywords
        .iter()
        .filter(|k| {
            let k = tokens(k);
            !k.is_empty() && words.windows(k.len()).any(|w| w == k.as_slice())
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
    let review_sharp = (rv - 0.5).abs() >= (th - 0.5).max(0.05);
    // Same rule as the Jev path: any unsure answer buys a review.
    let review = rv >= 0.5 || !review_sharp || !agent_sharp || !effort_sharp;
    // A task that asks how to go about something gets a choice of approaches first.
    let plan = cfg.router.plan != "never" && !is_issue && hits(&text, &r.plan_keywords) > 0;
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
        plan,
        trial_from: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decision server on loopback that answers every request and records the auth header.
    fn stand_in() -> (String, std::sync::Arc<std::sync::Mutex<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let s = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 65536];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                *s.lock().unwrap() = req.lines().find(|l| l.to_ascii_lowercase().starts_with("authorization:")).unwrap_or("").to_string();
                let body = r#"{"answers":{"agent":{"choice":"claude","confidence":0.9},"effort":{"score":1,"confidence":0.9},"review":{"noul":0.9}}}"#;
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            }
        });
        (url, seen)
    }

    fn auto(url: &str) -> Config {
        let mut cfg = Config::default();
        cfg.router.jev.base_url = url.into();
        cfg.router.jev.api_key_env = "FORKDECK_TEST_KEY_THAT_IS_NEVER_SET".into();
        cfg
    }

    #[tokio::test]
    async fn auto_uses_a_local_decision_server_when_one_answers() {
        let (url, seen) = stand_in();
        let cfg = auto(&url);
        assert_eq!(cfg.router.kind, "auto", "the default");
        let r = route(&cfg, &reqwest::Client::new(), "rename a field", false).await;
        assert!(r.asked_jev && r.fell_back.is_none(), "{:?}", r.fell_back);
        assert_eq!(r.decision.agent, AgentKind::Claude);
        assert!(seen.lock().unwrap().contains("Bearer forkdeck-local-dev"), "the local server's development key: {}", seen.lock().unwrap());
    }

    #[tokio::test]
    async fn auto_without_a_server_uses_the_rules_quietly() {
        let r = route(&auto("http://127.0.0.1:1"), &reqwest::Client::new(), "fix the failing test", false).await;
        assert!(!r.asked_jev && r.fell_back.is_none(), "{:?}", r.fell_back);
    }

    #[tokio::test]
    async fn jev_without_a_server_says_it_fell_back() {
        let mut cfg = auto("http://127.0.0.1:1");
        cfg.router.kind = "jev".into();
        let r = route(&cfg, &reqwest::Client::new(), "fix the failing test", false).await;
        assert!(r.fell_back.is_some_and(|e| e.contains("cannot connect")));
    }

    #[test]
    fn rules_review_confidence_obeys_the_threshold() {
        let mut cfg = Config::default();
        cfg.router.threshold = 0.9;
        let d = rules(&cfg, "fix bug test typo rename wording", false);
        assert!(d.review);
        assert!(!d.gates.iter().find(|g| g.name == "needs review").unwrap().sharp);
    }

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
    fn the_plan_question_is_optional_and_only_a_clear_yes_counts() {
        let answers = |plan: Option<f64>| {
            let mut a = json!({
                "agent": {"choice": "claude", "confidence": 0.9},
                "effort": {"score": 1.0, "confidence": 0.9},
                "review": {"noul": 0.9}
            });
            if let Some(p) = plan {
                a["plan"] = json!({"noul": p});
            }
            json!({"answers": a})
        };
        let cfg = Config::default();
        let d = from_answers(&cfg, &answers(Some(0.95))).unwrap();
        assert!(d.plan && d.gates.iter().any(|g| g.name == "plan first" && g.answer == "yes"));
        assert!(!from_answers(&cfg, &answers(Some(0.55))).unwrap().plan, "unsure is not a stop");
        let d = from_answers(&cfg, &answers(None)).unwrap();
        assert!(!d.plan && d.gates.len() == 3, "a server that does not know the question");
        let never: Config = toml::from_str("[router]\nplan = \"never\"").unwrap();
        assert!(!from_answers(&never, &answers(Some(0.99))).unwrap().plan);
        assert!(request_body(&cfg, "t")["questions"].get("plan").is_some());
        assert!(request_body(&never, "t")["questions"].get("plan").is_none());
        assert!(!rules(&cfg, "redesign the architecture", false).plan, "rules never ask for a plan");
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
    fn rules_match_phrases_and_word_forms() {
        let kw = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(hits("Renamed the field and fixed two failing tests", &kw(&["rename", "fix", "fail", "test"])), 4);
        assert_eq!(hits("Planning the races", &kw(&["plan", "race"])), 2);
        assert_eq!(hits("passed and called", &kw(&["pass", "call"])), 2);
        // A phrase needs its words together and in order.
        assert_eq!(hits("there is a race condition in the pool", &kw(&["race condition"])), 1);
        assert_eq!(hits("the condition of the race", &kw(&["race condition"])), 0);
        assert_eq!(hits("a one-line change", &kw(&["one line"])), 1);
        // Words are whole: "fixture" is not "fix".
        assert_eq!(hits("update the fixture", &kw(&["fix"])), 0);
    }

    #[test]
    fn rules_cover_common_phrasings() {
        let cfg = Config::default();
        let d = rules(&cfg, "The importer crashes with a stack trace when the file is empty", false);
        assert_eq!(d.agent, AgentKind::Codex);
        let d = rules(&cfg, "Clean up and document the config module", false);
        assert_eq!(d.agent, AgentKind::Claude);
        let d = rules(&cfg, "There is a race condition in the job queue", false);
        assert_eq!(d.tier, Tier::Heavy);
        let d = rules(&cfg, "Fix the spelling in the log message", false);
        assert_eq!(d.tier, Tier::Light);
    }

    #[test]
    fn rules_ask_for_a_plan_only_when_the_task_asks_how() {
        let mut cfg = Config::default();
        assert!(rules(&cfg, "How should we split the storage layer?", false).plan);
        assert!(!rules(&cfg, "Fix the failing parser test", false).plan);
        // An issue's text is someone else's: it never stops for a choice by itself.
        assert!(!rules(&cfg, "How should we split the storage layer?", true).plan);
        cfg.router.plan = "never".into();
        assert!(!rules(&cfg, "How should we split the storage layer?", false).plan);
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
