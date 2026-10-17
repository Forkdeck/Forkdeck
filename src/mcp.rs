//! A small Model Context Protocol client: enough to list a server's tools and call them.
//!
//! Two transports: a subprocess speaking newline-delimited JSON-RPC on stdin/stdout, and
//! Streamable HTTP (a POST per message, answered with JSON or a server-sent event stream).
//! The older HTTP+SSE transport is not supported.

use crate::config::McpServerCfg;
use crate::proc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

const PROTOCOL: &str = "2025-11-25";

pub struct ToolResult {
    /// All text content, plus any structured content serialised as JSON, exactly as the server sent
    /// it. Only for matching patterns against: a commit id or a run number must not be altered.
    pub text: String,
    /// The same with every value that came from the environment blanked out. Anything that is
    /// logged, shown to an agent, posted or passed to another tool uses this.
    pub shown: String,
    pub is_error: bool,
}

pub struct Tool {
    pub name: String,
    pub description: String,
}

enum Transport {
    Stdio { child: Child, stdin: ChildStdin, out: BufReader<ChildStdout>, _group: proc::Group },
    Http { client: reqwest::Client, url: String, headers: HashMap<String, String>, session: Option<String>, version: String },
}

pub struct Session {
    name: String,
    transport: Transport,
    next_id: u64,
    limit: Duration,
    /// Values taken from the environment for this server. Never shown.
    secrets: Vec<String>,
    /// The last of what a stdio server wrote to stderr, for when it dies without an answer.
    stderr_tail: Arc<Mutex<String>>,
}

/// What an error says when the server has dropped the session, so the caller can tell it apart
/// from an error that merely mentions a 404 and knows the call can be repeated safely.
pub const SESSION_EXPIRED: &str = "the session expired";

pub fn is_session_expired(server: &str, error: &str) -> bool {
    error == format!("{}: {}", server, SESSION_EXPIRED)
}

/// Longest single message accepted from a stdio server.
const MAX_LINE: usize = 16 * 1024 * 1024;

/// Replace `${NAME}` with the environment variable. Secrets belong in the environment, not in the config.
#[cfg(test)]
pub fn expand_env(s: &str) -> Result<String, String> {
    expand_into(s, &mut Vec::new())
}

/// As `expand_env`, and remembers every value it put in so it can be kept out of messages later.
fn expand_into(s: &str, secrets: &mut Vec<String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after.find('}').ok_or_else(|| format!("unclosed ${{ in {:?}", s))?;
        let var = &after[..end];
        let val = std::env::var(var).map_err(|_| format!("environment variable {} is not set", var))?;
        // Short values are settings, not secrets, and blanking them would mangle unrelated text.
        if val.len() >= 6 {
            secrets.push(val.clone());
        }
        out.push_str(&val);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Blank out every remembered secret. Errors and tool answers end up in logs, prompts and public
/// comments, and a server is free to echo its own URL, headers or environment.
///
/// This catches a value as it was given. It does not catch one that has been encoded (base64,
/// percent-encoding) on the way.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    // Longest first, so a value that begins with another one is not left half showing.
    let mut secrets: Vec<&String> = secrets.iter().collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for s in secrets {
        if out.contains(s.as_str()) {
            out = out.replace(s.as_str(), "***");
        }
    }
    out
}

/// Pull the JSON-RPC messages out of a server-sent event stream body.
pub fn sse_messages(body: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut data = String::new();
    for line in body.lines().chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !data.is_empty() {
                if let Ok(v) = serde_json::from_str::<Value>(&data) {
                    out.push(v);
                }
                data.clear();
            }
        } else if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.strip_prefix(' ').unwrap_or(d));
        }
    }
    out
}

fn result_of(name: &str, method: &str, msg: &Value) -> Result<Value, String> {
    if let Some(e) = msg.get("error") {
        let m = e.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
        return Err(format!("{}: {} failed: {}", name, method, m));
    }
    Ok(msg.get("result").cloned().unwrap_or(Value::Null))
}

impl Session {
    pub async fn open(name: &str, cfg: &McpServerCfg) -> Result<Session, String> {
        let mut secrets = Vec::new();
        let opened = Session::start(name, cfg, &mut secrets).await;
        // Whatever went wrong, the message must not carry a token.
        opened.map_err(|e| redact(&e, &secrets))
    }

    async fn start(name: &str, cfg: &McpServerCfg, secrets: &mut Vec<String>) -> Result<Session, String> {
        let limit = Duration::from_secs(cfg.timeout_secs.clamp(1, 3600));
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let transport = match (&cfg.command, &cfg.url) {
            (Some(cmd), None) => {
                let mut c = proc::command(cmd);
                let mut args = Vec::new();
                for a in &cfg.args {
                    args.push(expand_into(a, secrets)?);
                }
                c.args(args).env_clear();
                for k in ["PATH", "HOME", "USER", "LANG", "LC_ALL", "TMPDIR", "SYSTEMROOT"] {
                    if let Some(v) = std::env::var_os(k) { c.env(k, v); }
                }
                for (k, v) in &cfg.env {
                    c.env(k, expand_into(v, secrets)?);
                }
                c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
                let mut child = c.spawn().map_err(|e| format!("{}: cannot start {}: {}", name, cmd, e))?;
                let group = proc::Group::of(&child);
                let stdin = child.stdin.take().expect("piped stdin");
                let out = BufReader::new(child.stdout.take().expect("piped stdout"));
                // Servers log to stderr; it must be drained or a chatty one blocks. Only the end is kept.
                let mut err = child.stderr.take().expect("piped stderr");
                let tail = stderr_tail.clone();
                let hidden = secrets.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let mut raw = String::new();
                    while let Ok(n) = err.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        if let Ok(mut t) = tail.lock() {
                            raw.push_str(&String::from_utf8_lossy(&buf[..n]));
                            *t = redact(&raw, &hidden);
                            if t.len() > 2000 {
                                let cut = t.char_indices().map(|(i, _)| i).find(|i| t.len() - i <= 1500).unwrap_or(0);
                                *t = t[cut..].to_string();
                            }
                            // Preserve enough raw context to recognise a secret split across reads.
                            let keep = 4096 + hidden.iter().map(String::len).max().unwrap_or(0);
                            if raw.len() > keep * 2 {
                                let cut = raw.char_indices().map(|(i, _)| i).find(|i| raw.len() - i <= keep).unwrap_or(0);
                                raw = raw[cut..].to_string();
                            }
                        }
                    }
                });
                Transport::Stdio { child, stdin, out, _group: group }
            }
            (None, Some(url)) => {
                let mut headers = HashMap::new();
                for (k, v) in &cfg.headers {
                    headers.insert(k.clone(), expand_into(v, secrets)?);
                }
                let url = expand_into(url, secrets)?;
                // No redirects: only some credential headers are dropped when one crosses to another
                // host, and a token in a custom header would be sent along.
                let mut b = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
                // A server on this machine must not be reached through HTTP_PROXY: that would hand the proxy its credentials.
                if crate::orchestrator::is_loopback(&url) {
                    b = b.no_proxy();
                }
                Transport::Http { client: b.build().map_err(|e| e.to_string())?, url, headers, session: None, version: PROTOCOL.to_string() }
            }
            _ => return Err(format!("mcp.{}: set exactly one of command and url", name)),
        };
        let mut s = Session { name: name.to_string(), transport, next_id: 1, limit, secrets: secrets.clone(), stderr_tail };
        let init = s
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL,
                    "capabilities": {},
                    "clientInfo": { "name": "forkdeck", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        if let (Transport::Http { version, .. }, Some(v)) = (&mut s.transport, init.get("protocolVersion").and_then(|v| v.as_str())) {
            *version = v.to_string();
        }
        s.notify("notifications/initialized").await?;
        Ok(s)
    }

    pub async fn tools(&mut self) -> Result<Vec<Tool>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        // Bounded: a server that keeps handing out cursors must not hold us forever.
        for _ in 0..50 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let r = self.request("tools/list", params).await?;
            for t in r.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
                out.push(Tool {
                    name: t.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    description: t.get("description").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                });
            }
            cursor = r.get("nextCursor").and_then(|c| c.as_str()).map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    pub async fn call(&mut self, tool: &str, args: Value) -> Result<ToolResult, String> {
        let r = self.request("tools/call", json!({ "name": tool, "arguments": args })).await?;
        let mut text = String::new();
        for c in r.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default() {
            if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
        }
        if let Some(sc) = r.get("structuredContent").filter(|v| !v.is_null()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&sc.to_string());
        }
        // A tool may echo its environment or its URL; its answer goes into prompts and comments.
        let shown = redact(&text, &self.secrets);
        Ok(ToolResult { text, shown, is_error: r.get("isError").and_then(|e| e.as_bool()).unwrap_or(false) })
    }

    pub async fn close(mut self) {
        match &mut self.transport {
            Transport::Stdio { child, stdin, .. } => {
                let _ = stdin.shutdown().await;
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                // Dropping the session kills whatever is left of the process group.
            }
            Transport::Http { client, url, headers, session: Some(sid), version } => {
                let mut req = client.delete(url.as_str()).header("MCP-Session-Id", sid.as_str()).header("MCP-Protocol-Version", version.as_str());
                for (k, v) in headers.iter() {
                    req = req.header(k, v);
                }
                let _ = tokio::time::timeout(Duration::from_secs(5), req.send()).await;
            }
            Transport::Http { .. } => {}
        }
    }

    async fn notify(&mut self, method: &str) -> Result<(), String> {
        let msg = json!({ "jsonrpc": "2.0", "method": method });
        let name = self.name.clone();
        match &mut self.transport {
            Transport::Stdio { stdin, .. } => write_line(stdin, &msg).await.map_err(|e| format!("{}: {}", name, e)),
            Transport::Http { .. } => {
                let r = self.post(&msg, None).await.map(|_| ());
                r.map_err(|e| redact(&e, &self.secrets))
            }
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let res = self.request_raw(method, params).await;
        res.map_err(|e| redact(&e, &self.secrets))
    }

    async fn request_raw(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let name = self.name.clone();
        let limit = self.limit;
        let tail = self.stderr_tail.clone();
        let answer = match &mut self.transport {
            Transport::Stdio { stdin, out, .. } => {
                let exchange = async {
                    write_line(stdin, &msg).await?;
                    let mut raw = Vec::new();
                    loop {
                        raw.clear();
                        // Bounded: a server that never ends its line must not be read into memory without limit.
                        let n = (&mut *out).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut raw).await.map_err(|e| e.to_string())?;
                        if n == 0 {
                            let said = tail.lock().map(|t| t.trim().to_string()).unwrap_or_default();
                            let last = said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                            return Err(if last.is_empty() { "the server closed the connection".to_string() } else { format!("the server closed the connection: {}", last) });
                        }
                        if raw.len() > MAX_LINE {
                            return Err(format!("the server sent a message over {} MB", MAX_LINE / (1024 * 1024)));
                        }
                        let Ok(v) = serde_json::from_slice::<Value>(&raw) else { continue };
                        if v.get("id").and_then(|i| i.as_u64()) == Some(id) && v.get("method").is_none() {
                            return Ok(v);
                        }
                        // A request from the server to us: answer pings, decline everything else.
                        if let (Some(m), Some(rid)) = (v.get("method").and_then(|m| m.as_str()), v.get("id")) {
                            let reply = if m == "ping" {
                                json!({ "jsonrpc": "2.0", "id": rid, "result": {} })
                            } else {
                                json!({ "jsonrpc": "2.0", "id": rid, "error": { "code": -32601, "message": "not supported by this client" } })
                            };
                            write_line(stdin, &reply).await?;
                        }
                    }
                };
                match tokio::time::timeout(limit, exchange).await {
                    Err(_) => Err(format!("no answer within {} s", limit.as_secs())),
                    Ok(r) => r,
                }
                .map_err(|e| format!("{}: {}: {}", name, method, e))?
            }
            Transport::Http { .. } => self.post(&msg, Some(id)).await?.ok_or_else(|| format!("{}: {}: the server sent no response", name, method))?,
        };
        result_of(&name, method, &answer)
    }

    /// POST one message. For a request, returns the response with the matching id.
    async fn post(&mut self, msg: &Value, want: Option<u64>) -> Result<Option<Value>, String> {
        let limit = self.limit;
        let name = self.name.clone();
        let Transport::Http { client, url, headers, session, version } = &mut self.transport else { unreachable!() };
        let mut req = client
            .post(url.as_str())
            .timeout(limit)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .header("MCP-Protocol-Version", version.as_str())
            .body(msg.to_string());
        if let Some(sid) = session.as_deref() {
            req = req.header("MCP-Session-Id", sid);
        }
        for (k, v) in headers.iter() {
            req = req.header(k, v);
        }
        // reqwest puts the full URL in its messages, and a URL may carry a token.
        let mut resp = req.send().await.map_err(|e| {
            let why = if e.is_timeout() {
                format!("no answer within {} s", limit.as_secs())
            } else if e.is_connect() {
                "cannot connect to the server".to_string()
            } else {
                e.without_url().to_string()
            };
            format!("{}: {}", name, why)
        })?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(format!("{}: the server answered with a redirect (HTTP {}), which is not followed; configure the final URL", name, status.as_u16()));
        }
        if let Some(sid) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *session = Some(sid.to_string());
        }
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| format!("{}: {}", name, e.without_url()))? {
            if bytes.len() + chunk.len() > MAX_LINE { return Err(format!("{}: HTTP response exceeds {} bytes", name, MAX_LINE)); }
            bytes.extend_from_slice(&chunk);
            if status.is_success() && ctype.contains("text/event-stream") {
                if let Some(id) = want {
                    let body = String::from_utf8_lossy(&bytes);
                    // Only completed SSE frames can end the request.
                    if let Some(end) = body.rfind("\n\n").or_else(|| body.rfind("\r\n\r\n")) {
                        if let Some(v) = sse_messages(&body[..end]).into_iter().find(|v| v.get("id").and_then(|i| i.as_u64()) == Some(id) && v.get("method").is_none()) { return Ok(Some(v)); }
                    }
                }
            }
        }
        let body = String::from_utf8_lossy(&bytes);
        if status.as_u16() == 404 && session.is_some() {
            // The server no longer knows this session. Nothing was executed.
            return Err(format!("{}: {}", name, SESSION_EXPIRED));
        }
        if !status.is_success() {
            let short: String = redact(&body, &self.secrets).chars().take(200).collect();
            return Err(format!("{}: HTTP {} {}", name, status.as_u16(), short.replace('\n', " ")));
        }
        let Some(id) = want else { return Ok(None) };
        let msgs = if ctype.contains("text/event-stream") { sse_messages(&body) } else { serde_json::from_str::<Value>(&body).map(|v| vec![v]).unwrap_or_default() };
        Ok(msgs.into_iter().find(|v| v.get("id").and_then(|i| i.as_u64()) == Some(id) && v.get("method").is_none()))
    }
}

async fn write_line(stdin: &mut ChildStdin, msg: &Value) -> Result<(), String> {
    // serde_json never emits a raw newline, which is what the stdio framing requires.
    let mut line = msg.to_string();
    line.push('\n');
    stdin.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    stdin.flush().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn expands_environment_and_reports_missing_variables() {
        std::env::set_var("FORKDECK_TEST_TOKEN", "s3cret");
        assert_eq!(expand_env("Bearer ${FORKDECK_TEST_TOKEN}!").unwrap(), "Bearer s3cret!");
        assert_eq!(expand_env("plain $HOME {x}").unwrap(), "plain $HOME {x}");
        assert!(expand_env("${FORKDECK_TEST_UNSET_VAR}").unwrap_err().contains("FORKDECK_TEST_UNSET_VAR"));
        assert!(expand_env("${oops").is_err());
    }

    #[test]
    fn reads_event_streams() {
        let body = "id: 1\ndata:\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\ndata: {\"jsonrpc\":\"2.0\",\r\ndata: \"id\":7,\"result\":{}}\r\n\r\n";
        let m = sse_messages(body);
        assert_eq!(m.len(), 2);
        assert_eq!(m[1]["id"], 7);
    }

    async fn request(sock: &mut tokio::net::TcpStream) -> (String, String) {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let k = sock.read(&mut tmp).await.unwrap();
            buf.extend_from_slice(&tmp[..k]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some(at) = text.find("\r\n\r\n") {
                let head = text[..at].to_string();
                let len = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                if buf.len() >= at + 4 + len {
                    break (head, text[at + 4..].to_string());
                }
            }
            if k == 0 {
                break (text, String::new());
            }
        }
    }

    /// One HTTP exchange: read a request, hand it to `reply`, write what it returns.
    async fn serve(listener: TcpListener, n: usize, reply: fn(usize, &str, &str) -> String) {
        for i in 0..n {
            let (mut sock, _) = listener.accept().await.unwrap();
            let (head, body) = request(&mut sock).await;
            sock.write_all(reply(i, &head, &body).as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        }
    }

    #[tokio::test]
    async fn streamable_http_json_and_sse_with_session_and_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(serve(listener, 3, |i, head, body| {
            let h = head.to_lowercase();
            assert!(h.contains("authorization: bearer tok"), "{}", head);
            assert!(h.contains("accept: application/json, text/event-stream"));
            match i {
                0 => {
                    assert!(body.contains("\"method\":\"initialize\"") && !h.contains("mcp-session-id"));
                    let b = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"t","version":"0"}}}"#;
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: abc123\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", b.len(), b)
                }
                1 => {
                    assert!(body.contains("notifications/initialized") && h.contains("mcp-session-id: abc123"));
                    assert!(h.contains("mcp-protocol-version: 2025-06-18"), "uses the negotiated version: {}", head);
                    "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                }
                _ => {
                    assert!(body.contains("\"name\":\"status\"") && body.contains("\"app\":\"web\""));
                    let b = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"Healthy\"}],\"structuredContent\":{\"rev\":\"abc\"},\"isError\":false}}\n\n";
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", b.len(), b)
                }
            }
        }));
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer tok".to_string());
        let cfg = McpServerCfg { url: Some(url), headers, ..McpServerCfg::default() };
        let mut s = Session::open("t", &cfg).await.unwrap();
        let r = s.call("status", json!({ "app": "web" })).await.unwrap();
        assert_eq!(r.text, "Healthy\n{\"rev\":\"abc\"}");
        assert!(!r.is_error);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_sse_response_finishes_without_waiting_for_stream_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = McpServerCfg { url: Some(format!("http://{}/mcp", listener.local_addr().unwrap())), timeout_secs: 2, ..Default::default() };
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let _ = request(&mut first).await;
            let data = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{}}}\n\n";
            first.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n", data.len(), data).as_bytes()).await.unwrap();
            let (mut second, _) = listener.accept().await.unwrap();
            let _ = request(&mut second).await;
            second.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
            drop(first);
        });
        let result = tokio::time::timeout(Duration::from_secs(1), Session::open("stream", &cfg)).await;
        server.abort();
        assert!(matches!(result, Ok(Ok(_))), "matching response must end the request while the stream is still open");
    }

    #[tokio::test]
    async fn secrets_never_appear_in_errors_or_answers() {
        std::env::set_var("FORKDECK_TEST_SECRET", "hunter2SECRET");
        // Unreachable server, token in the URL.
        let cfg = McpServerCfg { url: Some("http://127.0.0.1:9/mcp?token=${FORKDECK_TEST_SECRET}".into()), timeout_secs: 3, ..McpServerCfg::default() };
        let e = Session::open("t", &cfg).await.err().unwrap();
        assert!(!e.contains("hunter2SECRET") && !e.contains("token="), "{}", e);

        // A server that echoes the token back, in an error body and in a tool answer.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(serve(listener, 4, |i, _, _| match i {
            0 => {
                let b = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{}}}"#;
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", b.len(), b)
            }
            1 => "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            2 => {
                let b = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"env: TOKEN=hunter2SECRET"}]}}"#;
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", b.len(), b)
            }
            _ => {
                let b = format!("bad key: {}hunter2SECRET !!!", "x".repeat(185));
                format!("HTTP/1.1 500 Oops\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", b.len(), b)
            }
        }));
        let mut headers = HashMap::new();
        headers.insert("X-Api-Key".to_string(), "${FORKDECK_TEST_SECRET}".to_string());
        let cfg = McpServerCfg { url: Some(url), headers, ..McpServerCfg::default() };
        let mut s = Session::open("t", &cfg).await.unwrap();
        let r = s.call("env", json!({})).await.unwrap();
        assert_eq!(r.shown, "env: TOKEN=***", "what is shown has the secret blanked");
        assert_eq!(r.text, "env: TOKEN=hunter2SECRET", "what patterns are matched against is untouched");
        let e = s.call("env", json!({})).await.err().unwrap();
        assert!(e.contains("500") && e.contains("bad key:") && e.contains("***") && !e.contains("hunter2"), "{}", e);
        server.await.unwrap();
    }

    #[test]
    fn redaction_handles_overlapping_values_and_ignores_short_ones() {
        let secrets = vec!["tokenAB".to_string(), "tokenABCDEF".to_string()];
        assert_eq!(redact("x tokenABCDEF y tokenAB z", &secrets), "x *** y *** z", "the longer value goes first");
        // A four-character setting such as a port is not remembered, so it cannot blank part of a commit id.
        std::env::set_var("FORKDECK_TEST_PORT", "3000");
        let mut got = Vec::new();
        assert_eq!(expand_into("${FORKDECK_TEST_PORT}", &mut got).unwrap(), "3000");
        assert!(got.is_empty());
        assert!(is_session_expired("ci", "ci: the session expired"));
        assert!(!is_session_expired("ci", "ci: HTTP 500 upstream said: the session expired"));
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        // Exactly one request is served: following the redirect would hang the test on a second one.
        let server = tokio::spawn(serve(listener, 1, |_, _, _| "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:9/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()));
        let cfg = McpServerCfg { url: Some(url), ..McpServerCfg::default() };
        let e = Session::open("t", &cfg).await.err().unwrap();
        assert!(e.contains("redirect") && e.contains("307"), "{}", e);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn http_errors_are_reported() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(serve(listener, 1, |_, _, _| "HTTP/1.1 401 Unauthorized\r\nContent-Length: 6\r\nConnection: close\r\n\r\ndenied".to_string()));
        let cfg = McpServerCfg { url: Some(url), ..McpServerCfg::default() };
        let e = Session::open("t", &cfg).await.err().unwrap();
        assert!(e.contains("401") && e.contains("denied"), "{}", e);
        server.await.unwrap();
    }
}
