//! Asks the copilot CLI who is logged in and which models the account may use, through its
//! headless JSON-RPC server (the protocol @github/copilot-sdk speaks). This costs no model request.

use crate::proc;
use crate::state::clip;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// One JSON-RPC message with the Content-Length header the server reads.
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
    /// Some(false): logged in, but the account cannot use Copilot from the CLI.
    pub seat: Option<bool>,
    /// Models whose policy allows them (or states none). None: the list could not be read.
    pub enabled: Option<HashSet<String>>,
    /// Models the account's policy switched off.
    pub disabled: Vec<String>,
    pub error: Option<String>,
}

pub fn models_from(result: &Value) -> (HashSet<String>, Vec<String>) {
    let mut enabled = HashSet::new();
    let mut disabled = Vec::new();
    for m in result.get("models").and_then(|m| m.as_array()).into_iter().flatten() {
        let Some(id) = m.get("id").and_then(|i| i.as_str()) else { continue };
        match m.pointer("/policy/state").and_then(|s| s.as_str()) {
            None | Some("enabled") => {
                enabled.insert(id.to_string());
            }
            Some(_) => disabled.push(id.to_string()),
        }
    }
    (enabled, disabled)
}

/// A 403 when listing models means the login works but the account has no Copilot access.
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
    // Dropping the guard stops copilot however this returns.
    let _group = proc::Group::of(&child);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut out = BufReader::new(child.stdout.take().expect("piped stdout"));
    let requests = [(1, "connect", json!({"supportedTaskKinds": ["agent"]})), (2, "auth.getStatus", json!({})), (3, "models.list", json!({}))];
    for (id, method, params) in requests {
        let msg = frame(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if stdin.write_all(&msg).await.is_err() || stdin.flush().await.is_err() {
            p.error = Some("copilot closed its input".into());
            return p;
        }
        // Notifications can come first; wait for the answer to this request.
        let answer = loop {
            match read_frame(&mut out).await {
                Some(v) if v.get("id").and_then(|i| i.as_u64()) == Some(id) => break v,
                Some(_) => continue,
                None => {
                    p.error = Some(format!("copilot stopped before answering {}", method));
                    return p;
                }
            }
        };
        if let Some(e) = answer.get("error") {
            if method == "models.list" {
                p.seat = seat_from_error(e);
            }
            if p.seat.is_none() {
                p.error = Some(format!("{}: {}", method, clip(e.get("message").and_then(|m| m.as_str()).unwrap_or("error"), 200)));
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
                let (enabled, disabled) = models_from(&r);
                p.seat = Some(!enabled.is_empty());
                p.enabled = Some(enabled);
                p.disabled = disabled;
            }
            _ => {}
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip() {
        let a = json!({"jsonrpc": "2.0", "id": 1, "method": "connect", "params": {}});
        let b = json!({"jsonrpc": "2.0", "id": 2, "result": {"ok": true, "text": "ünïcode"}});
        let mut bytes = frame(&a);
        bytes.extend(frame(&b));
        let mut r = tokio::io::BufReader::new(&bytes[..]);
        assert_eq!(read_frame(&mut r).await, Some(a));
        assert_eq!(read_frame(&mut r).await, Some(b));
        assert_eq!(read_frame(&mut r).await, None);
    }

    #[test]
    fn model_policy_and_seat() {
        let r = json!({"models": [
            {"id": "claude-sonnet-4.5", "policy": {"state": "enabled", "terms": ""}},
            {"id": "gpt-5.4"},
            {"id": "claude-opus-4.8", "policy": {"state": "disabled", "terms": ""}},
            {"id": "grok-4.7", "policy": {"state": "unconfigured", "terms": ""}}]});
        let (enabled, disabled) = models_from(&r);
        assert_eq!(enabled, ["claude-sonnet-4.5", "gpt-5.4"].iter().map(|s| s.to_string()).collect());
        assert_eq!(disabled, vec!["claude-opus-4.8".to_string(), "grok-4.7".to_string()]);
        // What an account without a Copilot seat got back on 2026-10-12.
        let e = json!({"code": -32603, "message": "Request models.list failed with message: GenericFailure, {\"kind\":\"http\",\"status\":403,\"statusText\":\"Forbidden\",\"body\":\"unauthorized: not authorized to use this Copilot feature\\n\"}"});
        assert_eq!(seat_from_error(&e), Some(false));
        assert_eq!(seat_from_error(&json!({"code": -32603, "message": "timeout"})), None);
    }
}
