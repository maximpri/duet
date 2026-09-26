// SPDX-License-Identifier: GPL-3.0-or-later
//! Scripted MCP servers for tests: one behaviour ([`Mock`]) served in process
//! over a pipe pair (stdio) or on a loopback HTTP port (JSON or event-stream
//! answers), plus a POSIX shell server ([`SH_SERVER`]) for tests that need a
//! real process in the sandbox.
//!
//! Tools: `echo` (read-only; returns its `text`), `write` (not read-only),
//! `fail` (answers `isError`), `boom` (a JSON-RPC error), `slow` (sleeps
//! `seconds`), `crash` (the server goes away), `picture` (an image block),
//! `canned` (returns [`Mock::canned`]; also answers as `web_search_prime`, the
//! tool of Z.ai's coding-plan search server).
//! Listing is paginated two tools per page. Every call's arguments are
//! recorded.

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// The scripted behaviour shared by both transports.
#[derive(Clone, Default)]
pub struct Mock {
    /// Description of `echo` (tests plant hostile or sensitive text here).
    pub echo_description: String,
    /// Protocol revision answered to `initialize` (default: this client's).
    pub version: Option<String>,
    /// What the `canned` tool returns.
    pub canned: String,
    /// Arguments of every `tools/call`, in order.
    pub calls: Arc<Mutex<Vec<Value>>>,
    /// Methods received, in order (requests and notifications).
    pub methods: Arc<Mutex<Vec<String>>>,
}

/// What the server does with one message.
pub enum Action {
    Reply(Value),
    Nothing,
    /// Wait, then reply.
    Delay(Duration, Value),
    /// Go away without answering.
    Crash,
}

fn tool(name: &str, description: &str, read_only: bool) -> Value {
    json!({"name": name, "description": description,
        "inputSchema": {"type": "object", "properties": {
            "text": {"type": "string", "description": "Text to use."},
            "seconds": {"type": "integer"}}},
        "annotations": {"readOnlyHint": read_only}})
}

impl Mock {
    pub fn calls(&self) -> Vec<Value> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn methods(&self) -> Vec<String> {
        self.methods.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn handle(&self, message: &Value) -> Action {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Ok(mut m) = self.methods.lock() {
            m.push(method.to_owned());
        }
        let Some(id) = message.get("id").cloned() else {
            return Action::Nothing;
        };
        if method.is_empty() {
            // An answer to a request of ours (none are made).
            return Action::Nothing;
        }
        let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
        let text = |t: &str| json!({"content": [{"type": "text", "text": t}]});
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => Action::Reply(ok(json!({
                "protocolVersion": self.version.clone().unwrap_or_else(|| crate::PROTOCOL_VERSION.into()),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock", "version": "1.0"}}))),
            "tools/list" => {
                let all = [
                    tool("echo", &self.echo_description, true),
                    tool("write", "Changes something.", false),
                    tool("fail", "Always fails.", true),
                    tool("boom", "Answers with a protocol error.", true),
                    tool("slow", "Sleeps.", true),
                    tool("crash", "Stops the server.", true),
                    tool("picture", "Returns an image.", true),
                    tool("canned", "Returns a fixed text.", true),
                ];
                let start: usize = params
                    .get("cursor")
                    .and_then(Value::as_str)
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                let end = (start + 2).min(all.len());
                let mut page = json!({"tools": all[start..end]});
                if end < all.len() {
                    page["nextCursor"] = json!(end.to_string());
                }
                Action::Reply(ok(page))
            }
            "tools/call" => {
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                if let Ok(mut c) = self.calls.lock() {
                    c.push(args.clone());
                }
                let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default();
                match params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "echo" | "write" => Action::Reply(ok(text(arg("text")))),
                    "fail" => Action::Reply(ok(json!({"isError": true,
                        "content": [{"type": "text", "text": format!("cannot do that: {}", arg("text"))}]}))),
                    "boom" => Action::Reply(json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32000, "message": format!("exploded on {}", arg("text"))}})),
                    "slow" => Action::Delay(
                        Duration::from_secs(
                            args.get("seconds").and_then(Value::as_u64).unwrap_or(5),
                        ),
                        ok(text("finally")),
                    ),
                    "crash" => Action::Crash,
                    "canned" | "web_search_prime" => Action::Reply(ok(text(&self.canned))),
                    "picture" => Action::Reply(ok(json!({"content": [
                        {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}]}))),
                    other => Action::Reply(json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32602, "message": format!("unknown tool {other}")}})),
                }
            }
            _ => Action::Reply(json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "method not found"}})),
        }
    }

    /// Serves over an in-process pipe pair: returns the client's ends
    /// (the server's input, the server's output). `ping_first` makes the
    /// server send a `ping` request and a log notification before each answer.
    pub fn stdio(
        &self,
        ping_first: bool,
    ) -> (
        Box<dyn AsyncWrite + Send + Unpin>,
        Box<dyn AsyncRead + Send + Unpin>,
    ) {
        let (client_in, server_in) = tokio::io::duplex(1 << 16);
        let (server_out, client_out) = tokio::io::duplex(1 << 16);
        let mock = self.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_in).lines();
            let mut out = server_out;
            let mut pings = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let reply = match mock.handle(&message) {
                    Action::Nothing => continue,
                    Action::Crash => return,
                    Action::Reply(r) => r,
                    Action::Delay(d, r) => {
                        tokio::time::sleep(d).await;
                        r
                    }
                };
                if ping_first {
                    pings += 1;
                    let extra = format!(
                        "{}\nnot json at all\n{}\n",
                        json!({"jsonrpc": "2.0", "id": format!("p{pings}"), "method": "ping"}),
                        json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info", "data": "x"}})
                    );
                    if out.write_all(extra.as_bytes()).await.is_err() {
                        return;
                    }
                }
                if out
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        (Box::new(client_in), Box::new(client_out))
    }
}

/// What the HTTP mock saw of one request.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub method: String,
    pub session: Option<String>,
    pub version: Option<String>,
    pub authorization: Option<String>,
    pub body: String,
}

/// How the HTTP mock answers requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Json,
    /// An event stream holding a `ping` request, then the answer.
    EventStream,
}

/// A loopback streamable-HTTP server for [`Mock`].
pub struct HttpMock {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    /// Answer the next session-bound request with 404 (a restarted server).
    pub expire_next: Arc<Mutex<bool>>,
}

impl HttpMock {
    pub async fn start(mock: Mock, answer: Answer) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let url = format!(
            "http://127.0.0.1:{}/mcp",
            listener.local_addr().expect("addr").port()
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let expire_next = Arc::new(Mutex::new(false));
        let (seen2, expire2) = (seen.clone(), expire_next.clone());
        let sessions = Arc::new(Mutex::new(0u32));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (mock, seen, expire, sessions) = (
                    mock.clone(),
                    seen2.clone(),
                    expire2.clone(),
                    sessions.clone(),
                );
                tokio::spawn(async move {
                    serve(stream, &mock, answer, &seen, &expire, &sessions).await;
                });
            }
        });
        Self {
            url,
            seen,
            expire_next,
        }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

async fn serve(
    stream: tokio::net::TcpStream,
    mock: &Mock,
    answer: Answer,
    seen: &Mutex<Vec<Seen>>,
    expire: &Mutex<bool>,
    sessions: &Mutex<u32>,
) {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await.unwrap_or(0) == 0 {
            return;
        }
        let method = request_line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        let mut s = Seen {
            method: method.clone(),
            ..Seen::default()
        };
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 || line.trim().is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').unwrap_or((&line, ""));
            let value = value.trim().to_owned();
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.parse().unwrap_or(0),
                "mcp-session-id" => s.session = Some(value),
                "mcp-protocol-version" => s.version = Some(value),
                "authorization" => s.authorization = Some(value),
                _ => {}
            }
        }
        let mut body = vec![0; length];
        if tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body)
            .await
            .is_err()
        {
            return;
        }
        s.body = String::from_utf8_lossy(&body).into_owned();
        let session = s.session.clone();
        if let Ok(mut v) = seen.lock() {
            v.push(s);
        }
        let respond = |status: &str, headers: &str, body: &str| {
            format!(
                "HTTP/1.1 {status}\r\n{headers}content-length: {}\r\n\r\n{body}",
                body.len()
            )
        };
        let out = if method == "DELETE" {
            respond("204 No Content", "", "")
        } else {
            let message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let is_init = message.get("method").and_then(Value::as_str) == Some("initialize");
            let expired = session.is_some() && {
                let mut e = expire
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                std::mem::replace(&mut *e, false)
            };
            if expired {
                respond("404 Not Found", "", "")
            } else if !is_init && session.is_none() {
                respond("400 Bad Request", "", "missing session")
            } else {
                let new_session = if is_init {
                    let mut n = sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *n += 1;
                    format!("mcp-session-id: session-{n}\r\n")
                } else {
                    String::new()
                };
                match mock.handle(&message) {
                    Action::Nothing => respond("202 Accepted", &new_session, ""),
                    Action::Crash => return,
                    action => {
                        let reply = match action {
                            Action::Delay(d, r) => {
                                tokio::time::sleep(d).await;
                                r
                            }
                            Action::Reply(r) => r,
                            _ => Value::Null,
                        };
                        match answer {
                            Answer::Json => respond(
                                "200 OK",
                                &format!("{new_session}content-type: application/json\r\n"),
                                &reply.to_string(),
                            ),
                            Answer::EventStream => {
                                let ping = json!({"jsonrpc": "2.0", "id": "s1", "method": "ping"});
                                let events = format!(
                                    ": opened\n\nevent: message\ndata: {ping}\n\nevent: message\ndata: {reply}\n\n"
                                );
                                respond(
                                    "200 OK",
                                    &format!("{new_session}content-type: text/event-stream\r\n"),
                                    &events,
                                )
                            }
                        }
                    }
                }
            }
        };
        if write.write_all(out.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// A stdio MCP server in POSIX shell, for tests that run a real process in
/// the sandbox (`/bin/sh -c SH_SERVER sh [description]`). Tools: `cat`
/// (read-only; reads `path` relative to its working directory and says
/// whether that was allowed), `note` (not read-only; appends `text` to
/// `received.txt` in its working directory and returns it; its description is
/// the first argument, if given), `env` (read-only; the values of
/// `CARGO_PKG_NAME` and `CARGO_MANIFEST_DIR` it sees) and `quit` (exits).
/// Values must not contain `"` or `\`.
pub const SH_SERVER: &str = r#"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"sh","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"cat","description":"Reads a file.","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]},"annotations":{"readOnlyHint":true}},{"name":"note","description":"%s","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}},{"name":"env","description":"Shows two variables.","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},{"name":"quit","description":"Exits.","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]}}\n' "$id" "${1:-Keeps a note.}" ;;
    *'"method":"tools/call"'*'"name":"cat"'*)
      path=$(printf '%s\n' "$line" | sed -n 's/.*"path":"\([^"]*\)".*/\1/p')
      if out=$(cat "$path" 2>&1); then state=readable; else state=denied; fi
      out=$(printf '%s' "$out" | tr -d '"\\' | tr '\n' ' ')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"%s: %s"}]}}\n' "$id" "$state" "$out" ;;
    *'"method":"tools/call"'*'"name":"note"'*)
      text=$(printf '%s\n' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
      printf '%s\n' "$text" >> received.txt
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"noted: %s"}]}}\n' "$id" "$text" ;;
    *'"method":"tools/call"'*'"name":"env"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"pkg=%s dir=%s"}]}}\n' "$id" "${CARGO_PKG_NAME:-unset}" "${CARGO_MANIFEST_DIR:-unset}" ;;
    *'"method":"tools/call"'*'"name":"quit"'*)
      exit 3 ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"unknown tool"}}\n' "$id" ;;
    *'"id":'*)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"method not found"}}\n' "$id" ;;
  esac
done
"#;
