//! Runs the `claude`, `codex` and `copilot` CLIs headless and turns their JSONL streams into stage updates.

use crate::config::Config;
use crate::orchestrator::Hub;
use crate::pick::Exec;
use crate::state::{clip, AgentKind, Stage, StageState};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::process::Stdio;
use std::time::{Duration, Instant};
use crate::proc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Worker,
    Reviewer,
    /// A pull request reviewer past the second.
    Extra(usize),
}

pub struct RunSpec<'a> {
    pub exec: &'a Exec,
    pub prompt: &'a str,
    pub dir: &'a Path,
    pub read_only: bool,
}

#[derive(Default)]
pub struct RunOut {
    pub ok: bool,
    pub text: String,
    pub error: String,
    /// Copilot refused the model before doing anything (the model, or empty when only stderr said so).
    pub rejected_model: Option<String>,
    /// Copilot said the account has no model it may use.
    pub no_models: bool,
    /// The models that actually answered, from copilot's usage file.
    pub models_used: Vec<String>,
}

pub struct CmdLine {
    pub cmd: String,
    pub args: Vec<String>,
    /// Where copilot writes its usage statistics when it exits.
    pub usage_file: Option<PathBuf>,
}

/// Only terminal failures are checked, never successful model text mentioning limits.
pub fn quota_error(error: &str) -> bool {
    let error = error.to_lowercase().replace('’', "'");
    ["you've hit your session limit", "you've hit your limit", "you have hit your usage limit",
     "you've hit your usage limit", "usage limit reached", "rate limit reached",
     "rate limit exceeded", "rate_limit_exceeded", "insufficient_quota"]
        .iter().any(|phrase| error.contains(phrase))
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
    /// The account or provider refused this model. Before any tool ran, another model may be tried.
    ModelRejected(String),
    /// The account has no model it may use at all.
    NoModels(String),
    Nothing,
}

/// The prompt is not part of the command line: it is written to the agent's stdin, so its size
/// and its first character (a leading `-` would read as an option) do not matter.
pub fn command_line(hub: &Hub, spec: &RunSpec) -> CmdLine {
    let e = spec.exec;
    let a = hub.cfg.agents.get(e.cli);
    let cmd = a.cmd.unwrap_or_else(|| e.cli.name().to_string());
    let extra = if spec.read_only { a.review_args } else { a.write_args }.unwrap_or_default();
    let mut args: Vec<String> = Vec::new();
    let mut usage_file = None;
    match e.cli {
        AgentKind::Claude => {
            args.extend(["-p", "--output-format", "stream-json", "--verbose"].map(String::from));
            if !e.model.is_empty() {
                args.extend(["--model".to_string(), e.model.clone()]);
            }
            if !e.effort.is_empty() {
                args.extend(["--effort".to_string(), e.effort.clone()]);
            }
            args.extend(extra);
        }
        AgentKind::Codex => {
            args.extend(["exec", "--json", "--skip-git-repo-check"].map(String::from));
            if !e.model.is_empty() {
                args.extend(["--model".to_string(), e.model.clone()]);
            }
            if !e.effort.is_empty() {
                args.extend(["-c".to_string(), format!("model_reasoning_effort=\"{}\"", e.effort)]);
            }
            args.extend(extra);
            // "-" makes codex read the prompt from stdin.
            args.push("-".to_string());
        }
        AgentKind::Copilot => {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let f = std::env::temp_dir().join(format!("forkdeck-copilot-usage-{}-{}.json", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            args = copilot_args(&hub.cfg, e, spec.read_only, &f);
            usage_file = Some(f);
        }
    }
    CmdLine { cmd, args, usage_file }
}

/// Arguments for one copilot run. The prompt goes to stdin, so it never needs `-p`. Tokens are not
/// in its JSON output, so they come from the usage file it writes on exit.
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

/// Tokens in (cache reads and writes included) and out, and the models that ran, from copilot's usage file.
pub fn read_usage(path: &Path) -> (u64, u64, Vec<String>) {
    let Some(v) = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
        return (0, 0, vec![]);
    };
    let (mut i, mut o, mut models) = (0, 0, vec![]);
    for (name, m) in v.get("modelMetrics").and_then(|m| m.as_object()).into_iter().flatten() {
        let n = |k: &str| m.pointer(&format!("/usage/{}", k)).and_then(|n| n.as_u64()).unwrap_or(0);
        i += n("inputTokens") + n("cacheReadTokens") + n("cacheWriteTokens");
        o += n("outputTokens");
        models.push(name.clone());
    }
    (i, o, models)
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

pub fn parse_copilot(line: &str) -> Vec<Parsed> {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![Parsed::Text(line.to_string())],
    };
    let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let n = |p: &str| v.pointer(p).and_then(|x| x.as_u64()).unwrap_or(0);
    let one = match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        // A message that only asks for tools has no text; the tools arrive as their own events.
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
        "session.model_change" if !s("/data/cause").is_empty() => Parsed::Note(format!("model switched to {} ({})", s("/data/newModel"), s("/data/cause"))),
        // The closing line. It carries no token counts; those come from the usage file.
        "result" => Parsed::Final { ok: v.get("exitCode").and_then(|c| c.as_u64()) == Some(0), text: String::new(), tokens_in: 0, tokens_out: 0, cost: 0.0 },
        _ => Parsed::Nothing,
    };
    vec![one]
}

pub async fn run(hub: &Hub, job: u32, slot: Slot, spec: RunSpec<'_>) -> RunOut {
    let line = command_line(hub, &spec);
    let start = Instant::now();
    let e = spec.exec;
    let stage = |f: &mut dyn FnMut(&mut Stage)| {
        hub.job(job, |j| f(j.stage(slot)));
    };
    stage(&mut |s| {
        let keep = std::mem::take(&mut s.tail);
        *s = Stage::default();
        s.tail = keep;
        s.agent = Some(e.family);
        s.via = e.via.clone().unwrap_or_default();
        s.model = e.model.clone();
        s.effort = e.effort.clone();
        s.state = StageState::Running;
        s.started = Some(start);
        s.last = "starting".into();
    });
    let cancel = match hub.job(job, |j| j.cancel.clone()) {
        Some(c) => c,
        None => return RunOut { error: "job vanished".into(), ..Default::default() },
    };

    let out = if hub.cfg.demo {
        tokio::select! {
            o = crate::demo::run(hub, job, slot, &spec) => o,
            _ = cancel.wait() => RunOut { error: "cancelled".into(), ..Default::default() },
        }
    } else {
        let limit = Duration::from_secs(hub.cfg.agents.timeout_mins.unwrap_or(45).clamp(1, 24 * 60) * 60);
        // Dropping the losing branch drops the process group guard, which kills the agent and its children.
        tokio::select! {
            o = spawn_and_read(hub, job, slot, e.cli, &line, spec.prompt, spec.dir) => o,
            _ = cancel.wait() => RunOut { error: "cancelled".into(), ..Default::default() },
            _ = tokio::time::sleep(limit) => RunOut { error: format!("timed out after {} min", limit.as_secs() / 60), ..Default::default() },
        }
    };
    // What this run used goes on the job's account, whichever box it was shown in.
    if !out.ok && quota_error(&out.error) && !cancel.is_set() && !hub.cfg.demo {
        hub.state.lock().unwrap().agent_cooldowns[crate::journal::slot(e.cli)] = Some(Instant::now() + Duration::from_secs(15 * 60));
        hub.log("agents", format!("{} reported a usage limit; skipping it for 15 minutes: {}", e.cli.name(), clip(&out.error, 200)));
    }
    hub.job(job, |j| {
        let s = if slot == Slot::Worker { &j.worker } else { &j.reviewer };
        let add = crate::journal::Usage { runs: 1, tokens_in: s.tokens_in, tokens_out: s.tokens_out, cost: s.cost };
        j.used[crate::journal::slot(e.cli)].add(&add);
    });
    stage(&mut |s| {
        s.elapsed = Some(start.elapsed());
        s.state = if out.ok { StageState::Ok } else { StageState::Failed };
        if !out.ok && !out.error.is_empty() {
            s.push(format!("error: {}", out.error));
        }
    });
    out
}

pub(crate) fn record_event(s: &mut crate::state::Stage, p: &Parsed) {
    match p {
        Parsed::Text(t) | Parsed::Note(t) => {
            for l in t.lines().filter(|l| !l.trim().is_empty()) {
                s.push(l);
            }
        }
        Parsed::Tool(t) => {
            s.tools += 1;
            s.push(format!("▸ {}", t));
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
        Parsed::Error(e) | Parsed::NoModels(e) => s.push(format!("error: {}", e)),
        Parsed::ModelRejected(m) => s.push(format!("error: model {} is not available to this account", m)),
        Parsed::Nothing => {}
    }
}

async fn spawn_and_read(hub: &Hub, job: u32, slot: Slot, kind: AgentKind, line: &CmdLine, prompt: &str, dir: &Path) -> RunOut {
    let (cmd, args) = (line.cmd.as_str(), &line.args);
    let fail = |e: String| RunOut { error: e, ..Default::default() };
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
    let group = proc::Group::of(&child);
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
    let mut tools_seen = 0u32;
    let mut rejected: Option<String> = None;
    let mut no_models = false;
    let mut status: Option<std::io::Result<std::process::ExitStatus>> = None;
    loop {
        // Bytes, not lines(): one invalid UTF-8 sequence must not end the loop while the agent is still writing.
        // read_until keeps what it has read in `raw` if the other branch wins, so nothing is lost.
        let n = tokio::select! {
            r = reader.read_until(b'\n', &mut raw) => r,
            s = child.wait(), if status.is_none() => {
                // The agent itself is done. Anything it left running would hold the pipe open
                // and keep us here until the timeout, so stop it now.
                status = Some(s);
                group.kill();
                continue;
            }
        };
        match n {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                error = format!("reading {} output failed: {}", cmd, e);
                break;
            }
        }
        let owned = String::from_utf8_lossy(&raw).trim().to_string();
        raw.clear();
        let line = owned.as_str();
        if line.is_empty() {
            continue;
        }
        let parsed = match kind {
            AgentKind::Claude => parse_claude(line),
            AgentKind::Codex => parse_codex(line),
            AgentKind::Copilot => parse_copilot(line),
        };
        for p in parsed {
            hub.job(job, |j| {
                let s = j.stage(slot);
                record_event(s, &p);
            });
            match p {
                Parsed::Text(t) => last_text = t,
                Parsed::Tool(_) => tools_seen += 1,
                // Only a refusal before anything was done can be retried with another model.
                Parsed::ModelRejected(m) if tools_seen == 0 => rejected = Some(m),
                Parsed::NoModels(e) => {
                    no_models = true;
                    error = e;
                }
                Parsed::Final { ok, text, .. } => {
                    finished = true;
                    final_ok = Some(ok);
                    if !ok && error.is_empty() {
                        error = first_line(&text);
                    }
                    final_text = Some(text);
                }
                Parsed::Usage { .. } => {
                    // A model call that completed supersedes an error reported earlier in the stream (a retry that worked).
                    error.clear();
                    // Codex's turn.completed ends the run. Copilot reports usage per model call; only its result line ends the run.
                    if kind == AgentKind::Codex {
                        finished = true;
                    }
                }
                Parsed::Error(e) => error = e,
                _ => {}
            }
        }
    }
    let status = match status {
        Some(s) => s,
        None => child.wait().await,
    };
    let stderr_text = err_task.await.unwrap_or_default();
    if stderr_text.contains("No model available") {
        no_models = true;
    }
    if stderr_text.contains("The requested model is not supported") && tools_seen == 0 {
        rejected.get_or_insert_with(String::new);
    }
    let mut models_used = Vec::new();
    if let Some(f) = &line.usage_file {
        let (tokens_in, tokens_out, models) = read_usage(f);
        let _ = std::fs::remove_file(f);
        if tokens_in + tokens_out > 0 {
            hub.job(job, |j| {
                let s = j.stage(slot);
                s.tokens_in = tokens_in;
                s.tokens_out = tokens_out;
            });
        }
        models_used = models;
    }
    let exit_ok = status.as_ref().map(|s| s.success()).unwrap_or(false);
    // A clean exit is not enough: the stream has to have reached its closing event.
    let ok = exit_ok && finished && final_ok.unwrap_or(true) && error.is_empty() && rejected.is_none() && !no_models;
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
    let text = final_text.filter(|t| !t.is_empty()).unwrap_or(last_text);
    RunOut { ok, text, error, rejected_model: rejected, no_models, models_used }
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
    fn quota_messages_are_specific() {
        assert!(quota_error("You've hit your session limit · resets 2pm (Europe/Berlin)"));
        assert!(quota_error("You’ve hit your limit"));
        assert!(quota_error("rate_limit_exceeded"));
        assert!(!quota_error("connection refused"));
        assert!(!quota_error("token limit for this prompt"));
    }

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

    fn fixture(name: &str) -> Vec<Parsed> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot").join(name);
        std::fs::read_to_string(p).unwrap().lines().filter(|l| !l.trim().is_empty()).flat_map(parse_copilot).filter(|p| *p != Parsed::Nothing).collect()
    }

    #[test]
    fn copilot_worker_stream() {
        assert_eq!(
            fixture("worker.jsonl"),
            vec![
                Parsed::Tool("view /work/notes.txt".into()),
                Parsed::Text("All good.\nVERDICT: PASS".into()),
                Parsed::Final { ok: true, text: String::new(), tokens_in: 0, tokens_out: 0, cost: 0.0 },
            ]
        );
    }

    #[test]
    fn copilot_refused_model_stream() {
        assert_eq!(
            fixture("rejected.jsonl"),
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
        let l = r#"{"type":"tool.execution_complete","data":{"toolCallId":"1","success":false,"error":{"message":"denied"}}}"#;
        assert_eq!(parse_copilot(l), vec![Parsed::Note("tool failed: denied".into())]);
        assert_eq!(parse_copilot(r#"{"type":"session.warning","data":{"message":"slow","warningType":"x"}}"#), vec![Parsed::Note("warning: slow".into())]);
        let l = r#"{"type":"session.model_change","data":{"newModel":"gpt-5.4","cause":"rate_limit_auto_switch"}}"#;
        assert_eq!(parse_copilot(l), vec![Parsed::Note("model switched to gpt-5.4 (rate_limit_auto_switch)".into())]);
        let l = r#"{"type":"assistant.usage","data":{"model":"m","inputTokens":5,"outputTokens":2},"ephemeral":true}"#;
        assert_eq!(parse_copilot(l), vec![Parsed::Usage { tokens_in: 5, tokens_out: 2 }]);
        let none = "No model available. Check policy enablement under GitHub Settings > Copilot";
        let l = format!(r#"{{"type":"session.error","data":{{"errorType":"authorization","message":"{}"}}}}"#, none);
        assert_eq!(parse_copilot(&l), vec![Parsed::NoModels(none.into())]);
        let l = r#"{"type":"tool.execution_start","data":{"toolName":"bash","arguments":{"command":"cargo test\nmore"}}}"#;
        assert_eq!(parse_copilot(l), vec![Parsed::Tool("bash cargo test".into())]);
        let l = r#"{"type":"result","sessionId":"x","usage":{}}"#;
        assert_eq!(parse_copilot(l), vec![Parsed::Final { ok: false, text: String::new(), tokens_in: 0, tokens_out: 0, cost: 0.0 }]);
    }

    #[test]
    fn copilot_command_lines() {
        let cfg: Config = toml::from_str("").unwrap();
        let exec = Exec {
            asked: crate::state::AgentRef::Kind(AgentKind::Claude),
            cli: AgentKind::Copilot,
            family: AgentKind::Claude,
            via: Some("copilot".into()),
            model: "claude-sonnet-4.5".into(),
            effort: "medium".into(),
            vendor: crate::pick::Vendor::Anthropic,
        };
        let usage = Path::new("/tmp/u.json");
        let has = |a: &[String], pair: [&str; 2]| a.windows(2).any(|p| p[0] == pair[0] && p[1] == pair[1]);
        let w = copilot_args(&cfg, &exec, false, usage);
        assert_eq!(&w[..6], ["--output-format", "json", "--no-ask-user", "--no-auto-update", "--usage-output-file", "/tmp/u.json"]);
        assert!(has(&w, ["--model", "claude-sonnet-4.5"]) && has(&w, ["--reasoning-effort", "medium"]));
        assert!(has(&w, ["--allow-tool", "write"]) && has(&w, ["--deny-tool", "shell"]));
        assert!(!w.iter().any(|a| a == "-p" || a.contains("allow-all") || a == "--yolo"), "prompt on stdin, never allow-all: {:?}", w);
        let r = copilot_args(&cfg, &exec, true, usage);
        assert!(has(&r, ["--deny-tool", "write"]) && has(&r, ["--excluded-tools", "skill"]), "{:?}", r);
        let bare = Exec { model: String::new(), effort: String::new(), ..exec };
        assert!(!copilot_args(&cfg, &bare, false, usage).iter().any(|a| a == "--model" || a == "--reasoning-effort"));
    }

    #[test]
    fn usage_file_gives_tokens_and_models() {
        let dir = std::env::temp_dir().join(format!("forkdeck-usage-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("u.json");
        std::fs::write(&p, r#"{"modelMetrics":{"claude-sonnet-4.5":{"usage":{"inputTokens":200,"outputTokens":14,"cacheReadTokens":50}},"gpt-5.4":{"usage":{"inputTokens":1,"outputTokens":1}}}}"#).unwrap();
        let (i, o, mut m) = read_usage(&p);
        m.sort();
        assert_eq!((i, o, m), (251, 15, vec!["claude-sonnet-4.5".to_string(), "gpt-5.4".to_string()]));
        assert_eq!(read_usage(&dir.join("missing.json")), (0, 0, vec![]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(parse_codex("hello"), vec![Parsed::Text("hello".into())]);
    }
}
