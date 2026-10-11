//! Runs the `claude` and `codex` CLIs headless and turns their JSONL streams into stage updates.

use crate::orchestrator::Hub;
use crate::state::{clip, AgentKind, Stage, StageState, Tier};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use crate::proc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Worker,
    Reviewer,
}

pub struct RunSpec<'a> {
    pub kind: AgentKind,
    pub tier: Tier,
    pub prompt: &'a str,
    pub dir: &'a Path,
    pub read_only: bool,
}

pub struct RunOut {
    pub ok: bool,
    pub text: String,
    pub error: String,
}

/// What one line of agent output means for the display.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    Text(String),
    Tool(String),
    Final { ok: bool, text: String, tokens_in: u64, tokens_out: u64, cost: f64 },
    Usage { tokens_in: u64, tokens_out: u64 },
    Error(String),
    /// Shown in the output pane but never taken as the agent's answer.
    Note(String),
    Nothing,
}

/// The prompt is not part of the command line: it is written to the agent's stdin, so its size
/// and its first character (a leading `-` would read as an option) do not matter.
pub fn command_line(hub: &Hub, spec: &RunSpec) -> (String, Vec<String>, String, String) {
    let a = hub.cfg.agents.get(spec.kind);
    let cmd = a.cmd.unwrap_or_else(|| spec.kind.name().to_string());
    let model = a.models.map(|m| m.get(spec.tier).to_string()).unwrap_or_default();
    let effort = a.efforts.map(|m| m.get(spec.tier).to_string()).unwrap_or_default();
    let extra = if spec.read_only { a.review_args } else { a.write_args }.unwrap_or_default();
    let mut args: Vec<String> = Vec::new();
    match spec.kind {
        AgentKind::Claude => {
            args.extend(["-p", "--output-format", "stream-json", "--verbose"].map(String::from));
            if !model.is_empty() {
                args.extend(["--model".to_string(), model.clone()]);
            }
            if !effort.is_empty() {
                args.extend(["--effort".to_string(), effort.clone()]);
            }
            args.extend(extra);
        }
        AgentKind::Codex => {
            args.extend(["exec", "--json", "--skip-git-repo-check"].map(String::from));
            if !model.is_empty() {
                args.extend(["--model".to_string(), model.clone()]);
            }
            if !effort.is_empty() {
                args.extend(["-c".to_string(), format!("model_reasoning_effort=\"{}\"", effort)]);
            }
            args.extend(extra);
            // "-" makes codex read the prompt from stdin.
            args.push("-".to_string());
        }
    }
    (cmd, args, model, effort)
}

fn first_line(s: &str) -> String {
    s.lines().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

fn tool_summary(name: &str, input: &Value) -> String {
    let detail = ["command", "file_path", "path", "pattern", "query", "url", "description"]
        .iter()
        .find_map(|k| input.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("");
    if detail.is_empty() {
        name.to_string()
    } else {
        format!("{} {}", name, first_line(detail))
    }
}

pub fn parse_claude(line: &str) -> Vec<Parsed> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![Parsed::Text(line.to_string())],
    };
    let mut out = Vec::new();
    match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "assistant" => {
            if let Some(items) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for it in items {
                    match it.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            let t = it.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if !t.trim().is_empty() {
                                out.push(Parsed::Text(t.to_string()));
                            }
                        }
                        "tool_use" => {
                            let name = it.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                            out.push(Parsed::Tool(tool_summary(name, it.get("input").unwrap_or(&Value::Null))));
                        }
                        _ => {}
                    }
                }
            }
        }
        "result" => {
            let u = v.get("usage").cloned().unwrap_or(Value::Null);
            let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            out.push(Parsed::Final {
                ok: !v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
                text: v.get("result").and_then(|r| r.as_str()).unwrap_or("").to_string(),
                tokens_in: n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"),
                tokens_out: n("output_tokens"),
                cost: v.get("total_cost_usd").and_then(|c| c.as_f64()).unwrap_or(0.0),
            });
        }
        _ => {}
    }
    if out.is_empty() {
        out.push(Parsed::Nothing);
    }
    out
}

pub fn parse_codex(line: &str) -> Vec<Parsed> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![Parsed::Text(line.to_string())],
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let one = match ty {
        "item.started" | "item.completed" => {
            let done = ty == "item.completed";
            match s("/item/type").as_str() {
                "agent_message" if done => Parsed::Text(s("/item/text")),
                "command_execution" if !done => Parsed::Tool(format!("shell {}", first_line(&s("/item/command")))),
                "file_change" if done => {
                    let files: Vec<String> = v
                        .pointer("/item/changes")
                        .and_then(|c| c.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|c| c.get("path").and_then(|p| p.as_str()).map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    Parsed::Tool(format!("edit {}", files.join(", ")))
                }
                "mcp_tool_call" if !done => Parsed::Tool(format!("mcp {}", s("/item/tool"))),
                "web_search" if done => Parsed::Tool(format!("search {}", s("/item/query"))),
                // Item-level errors are warnings; only turn.failed and a top-level error end the run.
                "error" if done => Parsed::Note(format!("warning: {}", s("/item/message"))),
                _ => Parsed::Nothing,
            }
        }
        "turn.completed" => {
            let n = |k: &str| v.pointer(&format!("/usage/{}", k)).and_then(|x| x.as_u64()).unwrap_or(0);
            Parsed::Usage { tokens_in: n("input_tokens"), tokens_out: n("output_tokens") }
        }
        "turn.failed" => Parsed::Error(s("/error/message")),
        "error" => Parsed::Error(s("/message")),
        _ => Parsed::Nothing,
    };
    vec![one]
}

pub async fn run(hub: &Hub, job: u32, slot: Slot, spec: RunSpec<'_>) -> RunOut {
    let (cmd, args, model, effort) = command_line(hub, &spec);
    let start = Instant::now();
    let kind = spec.kind;
    let stage = |f: &mut dyn FnMut(&mut Stage)| {
        hub.job(job, |j| f(if slot == Slot::Worker { &mut j.worker } else { &mut j.reviewer }));
    };
    stage(&mut |s| {
        let keep = std::mem::take(&mut s.tail);
        *s = Stage::default();
        s.tail = keep;
        s.agent = Some(kind);
        s.model = model.clone();
        s.effort = effort.clone();
        s.state = StageState::Running;
        s.started = Some(start);
        s.last = "starting".into();
    });
    let cancel = match hub.job(job, |j| j.cancel.clone()) {
        Some(c) => c,
        None => return RunOut { ok: false, text: String::new(), error: "job vanished".into() },
    };

    let out = if hub.cfg.demo {
        tokio::select! {
            o = crate::demo::run(hub, job, slot, &spec) => o,
            _ = cancel.wait() => RunOut { ok: false, text: String::new(), error: "cancelled".into() },
        }
    } else {
        let limit = Duration::from_secs(hub.cfg.agents.timeout_mins.unwrap_or(45).clamp(1, 24 * 60) * 60);
        // Dropping the losing branch drops the process group guard, which kills the agent and its children.
        tokio::select! {
            o = spawn_and_read(hub, job, slot, kind, &cmd, &args, spec.prompt, spec.dir) => o,
            _ = cancel.wait() => RunOut { ok: false, text: String::new(), error: "cancelled".into() },
            _ = tokio::time::sleep(limit) => RunOut { ok: false, text: String::new(), error: format!("timed out after {} min", limit.as_secs() / 60) },
        }
    };
    stage(&mut |s| {
        s.elapsed = Some(start.elapsed());
        s.state = if out.ok { StageState::Ok } else { StageState::Failed };
        if !out.ok && !out.error.is_empty() {
            s.push(format!("error: {}", out.error));
        }
    });
    out
}

#[allow(clippy::too_many_arguments)]
async fn spawn_and_read(
    hub: &Hub,
    job: u32,
    slot: Slot,
    kind: AgentKind,
    cmd: &str,
    args: &[String],
    prompt: &str,
    dir: &Path,
) -> RunOut {
    let fail = |e: String| RunOut { ok: false, text: String::new(), error: e };
    let mut child = match proc::command(cmd)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return fail(format!("cannot start {}: {}", cmd, e)),
    };
    let _group = proc::Group::of(&child);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let prompt_bytes = prompt.as_bytes().to_vec();
    tokio::spawn(async move {
        let _ = stdin.write_all(&prompt_bytes).await;
        let _ = stdin.shutdown().await;
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).to_string()
    });

    let mut reader = BufReader::new(stdout);
    let mut raw: Vec<u8> = Vec::new();
    let mut last_text = String::new();
    let mut final_text: Option<String> = None;
    let mut final_ok: Option<bool> = None;
    let mut finished = false;
    let mut error = String::new();
    loop {
        raw.clear();
        // Bytes, not lines(): one invalid UTF-8 sequence must not end the loop while the agent is still writing.
        match reader.read_until(b'\n', &mut raw).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                error = format!("reading {} output failed: {}", cmd, e);
                break;
            }
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed = match kind {
            AgentKind::Claude => parse_claude(line),
            AgentKind::Codex => parse_codex(line),
        };
        for p in parsed {
            hub.job(job, |j| {
                let s = if slot == Slot::Worker { &mut j.worker } else { &mut j.reviewer };
                match &p {
                    Parsed::Text(t) | Parsed::Note(t) => {
                        for l in t.lines().filter(|l| !l.trim().is_empty()) {
                            s.push(clip(l, 400));
                        }
                    }
                    Parsed::Tool(t) => {
                        s.tools += 1;
                        s.push(format!("▸ {}", clip(t, 160)));
                    }
                    Parsed::Final { tokens_in, tokens_out, cost, .. } => {
                        s.tokens_in = *tokens_in;
                        s.tokens_out = *tokens_out;
                        s.cost = *cost;
                    }
                    Parsed::Usage { tokens_in, tokens_out } => {
                        s.tokens_in += *tokens_in;
                        s.tokens_out += *tokens_out;
                    }
                    Parsed::Error(e) => s.push(format!("error: {}", e)),
                    Parsed::Nothing => {}
                }
            });
            match p {
                Parsed::Text(t) => last_text = t,
                Parsed::Final { ok, text, .. } => {
                    finished = true;
                    final_ok = Some(ok);
                    if !ok && error.is_empty() {
                        error = first_line(&text);
                    }
                    final_text = Some(text);
                }
                Parsed::Usage { .. } => finished = true,
                Parsed::Error(e) => error = e,
                _ => {}
            }
        }
    }
    let status = child.wait().await;
    let stderr_text = err_task.await.unwrap_or_default();
    let exit_ok = status.as_ref().map(|s| s.success()).unwrap_or(false);
    // A clean exit is not enough: the stream has to have reached its closing event.
    let ok = exit_ok && finished && final_ok.unwrap_or(true) && error.is_empty();
    if !ok && error.is_empty() {
        let tail: Vec<&str> = stderr_text.lines().rev().filter(|l| !l.trim().is_empty()).take(3).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        error = if !tail.is_empty() {
            clip(&tail.join(" | "), 400)
        } else if exit_ok {
            format!("{} exited before reporting a result", cmd)
        } else {
            match status {
                Ok(s) => format!("{} exited with {}", cmd, s),
                Err(e) => e.to_string(),
            }
        };
    }
    RunOut { ok, text: final_text.filter(|t| !t.is_empty()).unwrap_or(last_text), error }
}

pub async fn on_path(cmd: &str) -> bool {
    if cmd.trim().is_empty() {
        return false;
    }
    let mut c = proc::command(cmd);
    c.arg("--version");
    matches!(proc::output(c, Duration::from_secs(15)).await, Ok((true, _, _)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_stream_lines() {
        let l = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking at it"},{"type":"tool_use","name":"Edit","input":{"file_path":"src/a.rs"}}]}}"#;
        assert_eq!(
            parse_claude(l),
            vec![Parsed::Text("Looking at it".into()), Parsed::Tool("Edit src/a.rs".into())]
        );
        let l = r#"{"type":"result","is_error":false,"result":"done","total_cost_usd":0.12,"usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":7}}"#;
        assert_eq!(
            parse_claude(l),
            vec![Parsed::Final { ok: true, text: "done".into(), tokens_in: 100, tokens_out: 7, cost: 0.12 }]
        );
        assert_eq!(parse_claude(r#"{"type":"system","subtype":"init"}"#), vec![Parsed::Nothing]);
    }

    #[test]
    fn codex_stream_lines() {
        let l = r#"{"type":"item.started","item":{"id":"1","type":"command_execution","command":"cargo test"}}"#;
        assert_eq!(parse_codex(l), vec![Parsed::Tool("shell cargo test".into())]);
        let l = r#"{"type":"item.completed","item":{"id":"2","type":"agent_message","text":"Fixed."}}"#;
        assert_eq!(parse_codex(l), vec![Parsed::Text("Fixed.".into())]);
        let l = r#"{"type":"turn.completed","usage":{"input_tokens":5,"cached_input_tokens":2,"output_tokens":3}}"#;
        assert_eq!(parse_codex(l), vec![Parsed::Usage { tokens_in: 5, tokens_out: 3 }]);
        let l = r#"{"type":"item.completed","item":{"id":"9","type":"error","message":"slow down"}}"#;
        assert_eq!(parse_codex(l), vec![Parsed::Note("warning: slow down".into())]);
        let l = r#"{"type":"turn.failed","error":{"message":"quota"}}"#;
        assert_eq!(parse_codex(l), vec![Parsed::Error("quota".into())]);
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(parse_codex("hello"), vec![Parsed::Text("hello".into())]);
    }
}
