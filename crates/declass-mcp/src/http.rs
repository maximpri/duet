// SPDX-License-Identifier: GPL-3.0-or-later
//! The streamable HTTP transport: every message is a POST to the server's
//! endpoint. A request is answered either with a JSON body or with a
//! Server-Sent Events stream that carries the answer (and possibly requests
//! and notifications from the server before it). The session the server
//! assigns at initialization (`Mcp-Session-Id`) and the negotiated revision
//! (`MCP-Protocol-Version`) are sent with every later message.
//!
//! Every message (protocol messages, tool arguments, the answers to the
//! server's own requests, the closing `DELETE`) is checked in every part by
//! the guard the transport was opened with and sent by `declass-net`, which
//! sends nothing unchecked. The configured headers are the owner's
//! credentials: added by the client, never shown. Redirects are not followed
//! (they would carry the credentials to another address), no proxy from the
//! environment is used, bodies are size-capped and error bodies cut short.

use crate::sse::Events;
use crate::{MAX_MESSAGE_BYTES, McpError, answers, reply_to};
use declass_boundary::third_party::{Guard, Method, Outgoing};
use declass_net::{Client, Credentials, Options};
use serde_json::Value;
use std::time::Duration;

const SESSION: &str = "mcp-session-id";
const VERSION: &str = "mcp-protocol-version";

pub struct Http {
    client: Client,
    url: url::Url,
    credentials: Credentials,
    guard: Guard,
    pub(crate) session: Option<String>,
    pub(crate) version: Option<String>,
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Checks a server URL: `https`, or `http` to a loopback address only, and no
/// credentials in the URL itself (use `headers_env`).
pub fn check_url(url: &str) -> Result<url::Url, McpError> {
    let parsed = url::Url::parse(url).map_err(|e| McpError::Config(format!("invalid URL: {e}")))?;
    let host = parsed.host_str().unwrap_or_default();
    match parsed.scheme() {
        "https" => {}
        "http" if is_loopback(host) => {}
        "http" => {
            return Err(McpError::Config(
                "plain http is allowed only to a loopback address; use https".into(),
            ));
        }
        other => return Err(McpError::Config(format!("unsupported URL scheme {other}"))),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(McpError::Config(
            "the URL may not hold credentials; pass them with headers_env".into(),
        ));
    }
    Ok(parsed)
}

fn http_error(e: declass_net::NetError) -> McpError {
    // Connection failures mean the server is unreachable, not that one call failed.
    match e {
        declass_net::NetError::Connect(message) => {
            McpError::Closed(format!("cannot connect: {message}"))
        }
        other => McpError::Protocol(other.to_string()),
    }
}

/// Reads a whole body, refusing more than [`MAX_MESSAGE_BYTES`].
async fn body(mut response: declass_net::Response) -> Result<Vec<u8>, McpError> {
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http_error)? {
        out.extend_from_slice(&chunk);
        if out.len() > MAX_MESSAGE_BYTES {
            return Err(McpError::Protocol(format!(
                "a response was larger than {MAX_MESSAGE_BYTES} bytes"
            )));
        }
    }
    Ok(out)
}

fn valid_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        && value
            .bytes()
            .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
}

impl Http {
    pub fn new(url: &str, headers: &[(String, String)], guard: Guard) -> Result<Self, McpError> {
        let url = check_url(url)?;
        for (name, value) in headers {
            if !valid_header(name, "") {
                return Err(McpError::Config(format!("invalid header name {name}")));
            }
            // The value is a credential: never echoed in an error.
            if !valid_header(name, value) {
                return Err(McpError::Config(format!(
                    "the value for header {name} is not valid"
                )));
            }
        }
        let client = Client::new(Options {
            connect_timeout: Duration::from_secs(10),
            ..Options::default()
        })
        .map_err(|e| McpError::Config(e.to_string()))?;
        Ok(Self {
            client,
            url,
            credentials: Credentials::new(headers.to_vec()),
            guard,
            session: None,
            version: None,
        })
    }

    /// The server's name, as refusals and the audit log name it.
    fn destination(&self) -> String {
        format!("MCP server at {}", self.url.host_str().unwrap_or("?"))
    }

    /// Checks and sends one request to the endpoint.
    async fn send(
        &self,
        method: Method,
        message: Option<&Value>,
    ) -> Result<declass_net::Response, McpError> {
        let mut out = Outgoing {
            method,
            url: self.url.clone(),
            headers: Vec::new(),
            body: message.map(|m| declass_boundary::third_party::Body::Json(m.clone())),
        };
        if message.is_some() {
            out = out
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream");
        }
        if let Some(s) = &self.session {
            out = out.header(SESSION, s.as_str());
        }
        if let Some(v) = &self.version {
            out = out.header(VERSION, v.as_str());
        }
        let checked = self
            .guard
            .check("mcp", &self.destination(), out)
            .map_err(McpError::NotSent)?;
        self.client
            .send(&checked, &self.credentials)
            .await
            .map_err(http_error)
    }

    /// Checks the status of an answer; keeps the session id it assigns.
    async fn accepted(
        &mut self,
        response: declass_net::Response,
    ) -> Result<declass_net::Response, McpError> {
        let status = response.status();
        if status == 404 && self.session.is_some() {
            return Err(McpError::SessionExpired);
        }
        if !(200..300).contains(&status) {
            let text = body(response).await.unwrap_or_default();
            return Err(McpError::Http {
                status,
                body: String::from_utf8_lossy(&text).chars().take(300).collect(),
            });
        }
        if let Some(id) = response.header(SESSION)
            && !id.is_empty()
            && id.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            self.session = Some(id.to_owned());
        }
        Ok(response)
    }

    pub async fn notify(&mut self, message: &Value) -> Result<(), McpError> {
        let response = self.send(Method::Post, Some(message)).await?;
        self.accepted(response).await.map(drop)
    }

    /// Answers requests the server made while we wait for ours.
    async fn handle(&mut self, message: Value, id: u64) -> Result<Option<Value>, McpError> {
        let messages = match message {
            Value::Array(batch) => batch,
            single => vec![single],
        };
        let mut found = None;
        for m in messages {
            if answers(&m, id) {
                found = Some(m);
            } else if let Some(reply) = reply_to(&m) {
                self.notify(&reply).await?;
            }
        }
        Ok(found)
    }

    pub async fn request(&mut self, id: u64, message: &Value) -> Result<Value, McpError> {
        let response = self.send(Method::Post, Some(message)).await?;
        let mut response = self.accepted(response).await?;
        let kind = response
            .header("content-type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if kind.starts_with("text/event-stream") {
            let mut events = Events::default();
            loop {
                let chunk = response.chunk().await.map_err(http_error)?;
                let data = match &chunk {
                    Some(bytes) => events.push(bytes),
                    None => events.finish().into_iter().collect(),
                };
                if events.pending() > MAX_MESSAGE_BYTES {
                    return Err(McpError::Protocol(format!(
                        "an event was larger than {MAX_MESSAGE_BYTES} bytes"
                    )));
                }
                for d in data {
                    let Ok(parsed) = serde_json::from_str::<Value>(&d) else {
                        continue;
                    };
                    if let Some(found) = self.handle(parsed, id).await? {
                        return Ok(found);
                    }
                }
                if chunk.is_none() {
                    return Err(McpError::Protocol(
                        "the event stream ended without an answer".into(),
                    ));
                }
            }
        }
        let bytes = body(response).await?;
        if bytes.is_empty() {
            return Err(McpError::Protocol("the server sent no answer".into()));
        }
        let parsed: Value = serde_json::from_slice(&bytes)
            .map_err(|e| McpError::Protocol(format!("the answer is not JSON: {e}")))?;
        self.handle(parsed, id)
            .await?
            .ok_or_else(|| McpError::Protocol("the answer is for another request".into()))
    }

    /// Ends the session (`DELETE`); a server that does not support that is fine.
    pub async fn close(&mut self) {
        if self.session.is_none() {
            return;
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), self.send(Method::Delete, None)).await;
        self.session = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_need_tls_unless_loopback_and_hold_no_credentials() {
        assert!(check_url("https://mcp.example.com/mcp").is_ok());
        assert!(check_url("http://127.0.0.1:8123/mcp").is_ok());
        assert!(check_url("http://localhost/mcp").is_ok());
        assert!(check_url("http://[::1]:9/mcp").is_ok());
        assert!(check_url("http://mcp.example.com/mcp").is_err());
        assert!(check_url("http://192.168.1.4/mcp").is_err());
        assert!(check_url("https://user:pw@mcp.example.com/").is_err());
        assert!(check_url("file:///etc/passwd").is_err());
    }
}
