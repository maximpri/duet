// SPDX-License-Identifier: GPL-3.0-or-later
//! A Model Context Protocol client, written from the public specification
//! (revision 2025-06-18; 2025-03-26 and 2024-11-05 servers are accepted).
//!
//! JSON-RPC 2.0 over two transports: a child process's standard input and
//! output (one JSON message per line) and streamable HTTP (each message is a
//! POST whose answer is JSON or a Server-Sent Events stream, with the
//! `Mcp-Session-Id` the server assigns). The client performs the
//! `initialize` handshake with version negotiation, lists tools (following
//! pagination) and calls them. It declares no client capabilities: a server
//! request other than `ping` is answered "method not found".
//!
//! The client is a protocol layer only. Which servers run, in which sandbox,
//! and what their tools may see or return is decided by the caller.

pub mod config;
pub mod content;
mod http;
#[cfg(any(test, feature = "test-support"))]
pub mod mock;
mod sse;
mod stdio;

pub use config::{Approve, Launch, ServerConfig, Trust};
pub use http::check_url;

use serde_json::{Map, Value, json};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// The revision this client implements and asks for.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions a server may answer with.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// Largest single message accepted from a server.
pub const MAX_MESSAGE_BYTES: usize = 8 << 20;
/// Pages of `tools/list` followed, and tools kept per server.
const MAX_PAGES: usize = 50;
pub const MAX_TOOLS: usize = 256;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum McpError {
    /// The transport is gone (the process exited, a pipe closed): the server
    /// cannot be used again.
    #[error("the server stopped: {0}")]
    Closed(String),
    #[error("no answer within {0} s")]
    Timeout(u64),
    #[error("protocol error: {0}")]
    Protocol(String),
    /// A JSON-RPC error answer.
    #[error("error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    /// The server no longer knows the HTTP session (it restarted); the client
    /// initializes again once.
    #[error("the server ended the session")]
    SessionExpired,
    #[error("{0}")]
    Config(String),
    /// The boundary's check refused a message: nothing was sent, and the
    /// session is as it was.
    #[error("not sent: {0}")]
    NotSent(declass_boundary::third_party::Refusal),
}

/// A tool as the server lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    /// The JSON Schema of the arguments (an object schema).
    pub input_schema: Value,
    /// The server declares the tool does not change its environment
    /// (`annotations.readOnlyHint`). A hint from the server, not a guarantee.
    pub read_only: bool,
}

/// A `tools/call` result, rendered as text.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    /// The tool reported a failure (`isError`); `text` explains it.
    pub is_error: bool,
    pub text: String,
}

/// How messages reach the server.
pub enum Transport {
    Stdio(stdio::Stdio),
    Http(http::Http),
}

impl Transport {
    /// Newline-delimited JSON over a process's standard input and output.
    pub fn stdio(
        input: Box<dyn AsyncWrite + Send + Unpin>,
        output: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Self {
        Transport::Stdio(stdio::Stdio::new(input, output))
    }

    /// Streamable HTTP to `url` with the owner's credential `headers` (name,
    /// value) on every request. Every message is checked by `guard` (see
    /// `declass_boundary::third_party`) before it is sent.
    pub fn http(
        url: &str,
        headers: &[(String, String)],
        guard: declass_boundary::third_party::Guard,
    ) -> Result<Self, McpError> {
        http::Http::new(url, headers, guard).map(Transport::Http)
    }

    async fn request(&mut self, id: u64, message: &Value) -> Result<Value, McpError> {
        match self {
            Transport::Stdio(t) => t.request(id, message).await,
            Transport::Http(t) => t.request(id, message).await,
        }
    }

    async fn notify(&mut self, message: &Value) -> Result<(), McpError> {
        match self {
            Transport::Stdio(t) => t.send(message).await,
            Transport::Http(t) => t.notify(message).await,
        }
    }

    fn set_version(&mut self, version: &str) {
        if let Transport::Http(t) = self {
            t.version = Some(version.to_owned());
        }
    }

    fn forget_session(&mut self) {
        if let Transport::Http(t) = self {
            t.session = None;
            t.version = None;
        }
    }

    async fn close(&mut self) {
        match self {
            Transport::Stdio(t) => t.close().await,
            Transport::Http(t) => t.close().await,
        }
    }
}

/// The answer to a request the server sent us: `ping` succeeds, anything
/// else is not offered by this client. `None` for notifications and
/// responses.
pub(crate) fn reply_to(message: &Value) -> Option<Value> {
    let method = message.get("method")?.as_str()?;
    let id = message.get("id")?.clone();
    Some(if method == "ping" {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        json!({"jsonrpc": "2.0", "id": id, "error": {
            "code": -32601, "message": "method not supported by this client"}})
    })
}

/// Whether `message` is the response to request `id`.
pub(crate) fn answers(message: &Value, id: u64) -> bool {
    message.get("method").is_none()
        && message.get("id").and_then(Value::as_u64) == Some(id)
        && (message.get("result").is_some() || message.get("error").is_some())
}

/// A connected, initialized server.
pub struct Client {
    transport: Transport,
    next_id: u64,
    /// The revision both sides speak.
    pub protocol_version: String,
    /// `serverInfo.name` and `version`, as the server states them.
    pub server_name: String,
    pub server_version: String,
}

fn secs(d: Duration) -> u64 {
    d.as_secs().max(1)
}

impl Client {
    /// Initializes the server: `initialize` with this client's revision, a
    /// check that the server's revision is one this client speaks, then
    /// `notifications/initialized`.
    pub async fn connect(transport: Transport, timeout: Duration) -> Result<Self, McpError> {
        let mut client = Self {
            transport,
            next_id: 0,
            protocol_version: String::new(),
            server_name: String::new(),
            server_version: String::new(),
        };
        client.initialize(timeout).await?;
        Ok(client)
    }

    async fn initialize(&mut self, timeout: Duration) -> Result<(), McpError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "declass", "version": env!("CARGO_PKG_VERSION")},
        });
        let result = self.exchange("initialize", params, timeout).await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::Protocol("initialize result has no protocolVersion".into()))?;
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(McpError::Protocol(format!(
                "the server speaks protocol revision {}, which this client does not (it speaks {})",
                version.chars().take(40).collect::<String>(),
                SUPPORTED_VERSIONS.join(", ")
            )));
        }
        self.protocol_version = version.to_owned();
        self.transport.set_version(version);
        let info = result.get("serverInfo");
        let field = |k: &str| {
            info.and_then(|i| i.get(k))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect::<String>()
        };
        self.server_name = field("name");
        self.server_version = field("version");
        self.transport
            .notify(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await
    }

    /// One request and its result, within `timeout`. A request that times out
    /// is cancelled (`notifications/cancelled`); an expired HTTP session is
    /// initialized again and the request repeated once.
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        match self.exchange(method, params.clone(), timeout).await {
            Err(McpError::SessionExpired) => {
                self.transport.forget_session();
                self.initialize(timeout).await?;
                self.exchange(method, params, timeout).await
            }
            other => other,
        }
    }

    async fn exchange(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let answer = match tokio::time::timeout(timeout, self.transport.request(id, &message)).await
        {
            Ok(r) => r?,
            Err(_) => {
                let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                    "params": {"requestId": id, "reason": "timed out"}});
                let _ =
                    tokio::time::timeout(Duration::from_secs(2), self.transport.notify(&cancel))
                        .await;
                return Err(McpError::Timeout(secs(timeout)));
            }
        };
        if let Some(e) = answer.get("error") {
            return Err(McpError::Rpc {
                code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no message")
                    .chars()
                    .take(2000)
                    .collect(),
            });
        }
        Ok(answer.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Every tool the server offers (all pages, at most [`MAX_TOOLS`]).
    pub async fn list_tools(&mut self, timeout: Duration) -> Result<Vec<Tool>, McpError> {
        let mut tools = Vec::new();
        let mut next: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &next {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let page = self.call("tools/list", params, timeout).await?;
            let listed = page
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| McpError::Protocol("tools/list result has no tools".into()))?;
            tools.extend(listed.iter().filter_map(parse_tool));
            next = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_owned);
            if next.is_none() || tools.len() >= MAX_TOOLS {
                break;
            }
        }
        tools.truncate(MAX_TOOLS);
        Ok(tools)
    }

    /// Calls `name` with `arguments`.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: &Map<String, Value>,
        timeout: Duration,
    ) -> Result<ToolResult, McpError> {
        let result = self
            .call(
                "tools/call",
                json!({"name": name, "arguments": arguments}),
                timeout,
            )
            .await?;
        Ok(ToolResult {
            is_error: result.get("isError").and_then(Value::as_bool) == Some(true),
            text: content::render(&result),
        })
    }

    /// Ends the session: closes a process's input (it should exit), or ends
    /// the HTTP session with `DELETE`.
    pub async fn close(mut self) {
        self.transport.close().await;
    }
}

fn parse_tool(v: &Value) -> Option<Tool> {
    let name = v.get("name")?.as_str()?.to_owned();
    if name.is_empty() {
        return None;
    }
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(Tool {
        name,
        title: text("title"),
        description: text("description").unwrap_or_default(),
        input_schema: v
            .get("inputSchema")
            .filter(|s| s.is_object())
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"})),
        read_only: v
            .get("annotations")
            .and_then(|a| a.get("readOnlyHint"))
            .and_then(Value::as_bool)
            == Some(true),
    })
}
