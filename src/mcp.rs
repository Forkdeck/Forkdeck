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
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

const PROTOCOL: &str = "2025-11-25";

pub struct ToolResult {
    /// All text content, plus any structured content serialised as JSON.
    pub text: String,
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
}

/// Replace `${NAME}` with the environment variable. Secrets belong in the environment, not in the config.
pub fn expand_env(s: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after.find('}').ok_or_else(|| format!("unclosed ${{ in {:?}", s))?;
        let var = &after[..end];
        let val = std::env::var(var).map_err(|_| format!("environment variable {} is not set", var))?;
        out.push_str(&val);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
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
        let limit = Duration::from_secs(cfg.timeout_secs.clamp(1, 3600));
        let transport = match (&cfg.command, &cfg.url) {
            (Some(cmd), None) => {
                let mut c = proc::command(cmd);
                c.args(cfg.args.iter().map(|a| expand_env(a)).collect::<Result<Vec<_>, _>>()?);
                for (k, v) in &cfg.env {
                    c.env(k, expand_env(v)?);
                }
                c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
                let mut child = c.spawn().map_err(|e| format!("{}: cannot start {}: {}", name, cmd, e))?;
                let group = proc::Group::of(&child);
                let stdin = child.stdin.take().expect("piped stdin");
                let out = BufReader::new(child.stdout.take().expect("piped stdout"));
                // Servers log to stderr; it must be drained or a chatty one blocks.
                let mut err = child.stderr.take().expect("piped stderr");
                tokio::spawn(async move {
                    let mut sink = Vec::new();
                    let _ = err.read_to_end(&mut sink).await;
                });
                Transport::Stdio { child, stdin, out, _group: group }
            }
            (None, Some(url)) => {
                let mut headers = HashMap::new();
                for (k, v) in &cfg.headers {
                    headers.insert(k.clone(), expand_env(v)?);
                }
                let url = expand_env(url)?;
                // A server on this machine must not be reached through HTTP_PROXY: that would hand the proxy its credentials.
                let mut b = reqwest::Client::builder();
                if crate::orchestrator::is_loopback(&url) {
                    b = b.no_proxy();
                }
                Transport::Http { client: b.build().map_err(|e| e.to_string())?, url, headers, session: None, version: PROTOCOL.to_string() }
            }
            _ => return Err(format!("mcp.{}: set exactly one of command and url", name)),
        };
        let mut s = Session { name: name.to_string(), transport, next_id: 1, limit };
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
        Ok(ToolResult { text, is_error: r.get("isError").and_then(|e| e.as_bool()).unwrap_or(false) })
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
            Transport::Http { .. } => self.post(&msg, None).await.map(|_| ()),
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let name = self.name.clone();
        let limit = self.limit;
        let answer = match &mut self.transport {
            Transport::Stdio { stdin, out, .. } => {
                let exchange = async {
                    write_line(stdin, &msg).await?;
                    let mut raw = Vec::new();
                    loop {
                        raw.clear();
                        let n = out.read_until(b'\n', &mut raw).await.map_err(|e| e.to_string())?;
                        if n == 0 {
                            return Err("the server closed the connection".to_string());
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
        let resp = req.send().await.map_err(|e| format!("{}: {}", name, if e.is_timeout() { format!("no answer within {} s", limit.as_secs()) } else { e.to_string() }))?;
        let status = resp.status();
        if let Some(sid) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
            *session = Some(sid.to_string());
        }
        let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
        let body = resp.text().await.map_err(|e| format!("{}: {}", name, e))?;
        if !status.is_success() {
            let short: String = body.chars().take(200).collect();
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

    /// One HTTP exchange: read a request, hand it to `reply`, write what it returns.
    async fn serve(listener: TcpListener, n: usize, reply: fn(usize, &str, &str) -> String) {
        for i in 0..n {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let (head, body) = loop {
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
            };
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
