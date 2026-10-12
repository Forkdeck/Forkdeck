//! The real `copilot` binary against a fake OpenAI-compatible model server on localhost, through
//! its bring-your-own-key mode, so no Copilot subscription is needed. It checks what the parser and
//! the default arguments rely on: the JSONL events, the closing line, the usage file, and that a
//! reviewer cannot edit. Run with `cargo test --test copilot_byok -- --ignored` with copilot on PATH.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

// Must match AgentCfg::copilot() in src/config.rs.
const WRITE_ARGS: &[&str] = &["--allow-tool", "write", "--deny-tool", "shell"];
const REVIEW_ARGS: &[&str] = &["--deny-tool", "write", "--deny-tool", "shell", "--excluded-tools", "skill"];

#[derive(Clone)]
enum Answer {
    /// First a call to this tool with these arguments, then a final text.
    ToolThenText(&'static str, Value),
    /// Refuse every request the way GitHub refuses a model the organisation switched off.
    Refuse,
}

/// Serves chat completions on a free port until the test ends. Returns the base URL.
fn fake_server(answer: Answer) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let calls = Arc::new(Mutex::new(0u32));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if first.starts_with("GET") {
                let b = json!({"object": "list", "data": []}).to_string();
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", b.len(), b);
                continue;
            }
            let n = {
                let mut c = calls.lock().unwrap();
                *c += 1;
                *c
            };
            match &answer {
                Answer::Refuse => {
                    let b = json!({"error": {"message": "The requested model is not supported.", "code": "model_not_supported", "param": "model", "type": "invalid_request_error"}}).to_string();
                    let _ = write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", b.len(), b);
                }
                Answer::ToolThenText(tool, args) => {
                    let after_tool = req["messages"].as_array().is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
                    let offered = req["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["function"]["name"] == *tool));
                    let (delta, finish) = if !after_tool && offered {
                        (json!({"role": "assistant", "tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": tool, "arguments": args.to_string()}}]}), "tool_calls")
                    } else {
                        (json!({"role": "assistant", "content": "All good.\nVERDICT: PASS"}), "stop")
                    };
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                    for chunk in [
                        json!({"choices": [{"index": 0, "delta": delta, "finish_reason": null}]}),
                        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}),
                        json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 7, "total_tokens": 107}}),
                    ] {
                        let mut c = chunk;
                        c["id"] = json!(format!("chatcmpl-{}", n));
                        c["object"] = json!("chat.completion.chunk");
                        c["model"] = req["model"].clone();
                        let _ = write!(stream, "data: {}\n\n", c);
                    }
                    let _ = write!(stream, "data: [DONE]\n\n");
                }
            }
        }
    });
    url
}

struct Run {
    events: Vec<Value>,
    usage: Value,
    exit_ok: bool,
}

fn copilot(url: &str, dir: &Path, prompt: &str, role_args: &[&str]) -> Run {
    let usage = dir.join("usage.json");
    let mut args: Vec<String> = ["--output-format", "json", "--no-ask-user", "--no-auto-update", "--usage-output-file"].iter().map(|s| s.to_string()).collect();
    args.push(usage.display().to_string());
    args.extend(role_args.iter().map(|s| s.to_string()));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut child = Command::new(std::env::var("COPILOT_BIN").unwrap_or_else(|_| "copilot".into()))
        .args(&args)
        .current_dir(dir.join("work"))
        // A home of its own: no personal skills, instructions or settings.
        .env("HOME", &home)
        .env("COPILOT_HOME", home.join(".copilot"))
        .env("XDG_CACHE_HOME", std::env::var("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap()).join(".cache")))
        .env("COPILOT_OFFLINE", "true")
        .env("COPILOT_PROVIDER_BASE_URL", url)
        .env("COPILOT_MODEL", "fake-model")
        .env("COPILOT_PROVIDER_MAX_PROMPT_TOKENS", "100000")
        .env("COPILOT_PROVIDER_MAX_OUTPUT_TOKENS", "4000")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("copilot is on PATH (or set COPILOT_BIN)");
    child.stdin.take().unwrap().write_all(prompt.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let events = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let usage = std::fs::read_to_string(&usage).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    Run { events, usage, exit_ok: out.status.success() }
}

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("forkdeck-byok-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("work")).unwrap();
    std::fs::write(dir.join("work/notes.txt"), "hello\n").unwrap();
    dir
}

fn of_type<'a>(r: &'a Run, t: &str) -> Vec<&'a Value> {
    r.events.iter().filter(|e| e["type"] == t).collect()
}

#[test]
#[ignore]
fn a_worker_run_has_the_events_forkdeck_reads() {
    let dir = workspace("worker");
    let notes = dir.join("work/notes.txt");
    let url = fake_server(Answer::ToolThenText("view", json!({"path": notes})));
    let r = copilot(&url, &dir, "Read notes.txt and review it.", WRITE_ARGS);
    assert!(r.exit_ok, "{:?}", r.events);
    assert!(of_type(&r, "tool.execution_start").iter().any(|e| e["data"]["toolName"] == "view"), "{:?}", r.events);
    assert!(of_type(&r, "assistant.message").iter().any(|e| e["data"]["content"] == "All good.\nVERDICT: PASS"));
    let last = r.events.last().unwrap();
    assert_eq!((last["type"].as_str(), last["exitCode"].as_u64()), (Some("result"), Some(0)), "closing line: {}", last);
    assert!(r.usage["modelMetrics"]["fake-model"]["usage"]["inputTokens"].as_u64().unwrap_or(0) > 0, "usage file: {}", r.usage);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn a_worker_may_edit_files() {
    let dir = workspace("edit");
    let notes = dir.join("work/notes.txt");
    let url = fake_server(Answer::ToolThenText("edit", json!({"path": notes, "old_str": "hello", "new_str": "edited"})));
    let r = copilot(&url, &dir, "Change hello to edited in notes.txt.", WRITE_ARGS);
    assert!(r.exit_ok, "{:?}", r.events);
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "edited\n", "{:?}", of_type(&r, "tool.execution_complete"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn a_reviewer_cannot_edit_or_run_commands() {
    for (name, tool, args) in [
        ("review-edit", "edit", json!({"old_str": "hello", "new_str": "changed by the reviewer"})),
        ("review-shell", "bash", json!({"command": "echo changed > notes.txt", "description": "x"})),
    ] {
        let dir = workspace(name);
        let notes = dir.join("work/notes.txt");
        let mut args = args;
        if tool == "edit" {
            args["path"] = json!(notes);
        }
        let url = fake_server(Answer::ToolThenText(tool, args));
        let r = copilot(&url, &dir, "Review the change. Do not modify any file.", REVIEW_ARGS);
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello\n", "{} ran: {:?}", tool, r.events);
        assert!(of_type(&r, "tool.execution_start").iter().any(|e| e["data"]["toolName"] == tool), "the model asked for {}: {:?}", tool, r.events);
        assert!(denied(&r), "{} must be refused for a reviewer: {:?}", tool, r.events);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Copilot refused a tool on permission grounds (it does not ask: --no-ask-user, no allow rule).
fn denied(r: &Run) -> bool {
    of_type(r, "tool.execution_complete").iter().any(|e| e["data"]["success"] == false && e["data"]["error"]["code"] == "denied")
        && !of_type(r, "tool.execution_complete").iter().any(|e| e["data"]["success"] == true)
}

#[test]
#[ignore]
fn a_worker_cannot_run_commands_unattended() {
    let dir = workspace("worker-shell");
    let notes = dir.join("work/notes.txt");
    let url = fake_server(Answer::ToolThenText("bash", json!({"command": "echo changed > notes.txt", "description": "x"})));
    let r = copilot(&url, &dir, "Change notes.txt.", WRITE_ARGS);
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello\n", "{:?}", r.events);
    assert!(denied(&r), "{:?}", r.events);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn a_refused_model_is_reported_before_anything_runs() {
    let dir = workspace("refused");
    let url = fake_server(Answer::Refuse);
    let r = copilot(&url, &dir, "hi", WRITE_ARGS);
    assert!(!r.exit_ok);
    assert!(of_type(&r, "model.call_failure").iter().any(|e| e["data"]["errorCode"] == "model_not_supported"), "{:?}", r.events);
    assert!(of_type(&r, "tool.execution_start").is_empty());
    let last = r.events.last().unwrap();
    assert_eq!((last["type"].as_str(), last["exitCode"].as_u64()), (Some("result"), Some(1)), "{}", last);
    let _ = std::fs::remove_dir_all(&dir);
}
