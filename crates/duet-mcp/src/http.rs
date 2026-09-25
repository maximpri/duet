// SPDX-License-Identifier: GPL-3.0-or-later
//! The streamable HTTP transport: every message is a POST to the server's
//! endpoint. A request is answered either with a JSON body or with a
//! Server-Sent Events stream that carries the answer (and possibly requests
//! and notifications from the server before it). The session the server
//! assigns at initialization (`Mcp-Session-Id`) and the negotiated revision
//! (`MCP-Protocol-Version`) are sent with every later message.
//!
//! Redirects are not followed (they would carry the configured headers to
//! another address), bodies are size-capped and error bodies are cut short.

use crate::sse::Events;
use crate::{MAX_MESSAGE_BYTES, McpError, answers, reply_to};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::time::Duration;

const SESSION: &str = "mcp-session-id";
const VERSION: &str = "mcp-protocol-version";

pub struct Http {
    client: reqwest::Client,
    url: reqwest::Url,
    headers: HeaderMap,
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
pub fn check_url(url: &str) -> Result<reqwest::Url, McpError> {
    let parsed =
        reqwest::Url::parse(url).map_err(|e| McpError::Config(format!("invalid URL: {e}")))?;
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

fn http_error(e: reqwest::Error) -> McpError {
    // Connection failures mean the server is unreachable, not that one call failed.
    let message = e.to_string();
    if e.is_connect() {
        McpError::Closed(format!("cannot connect: {message}"))
    } else {
        McpError::Protocol(message)
    }
}

/// Reads a whole body, refusing more than [`MAX_MESSAGE_BYTES`].
async fn body(mut response: reqwest::Response) -> Result<Vec<u8>, McpError> {
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

impl Http {
    pub fn new(url: &str, headers: &[(String, String)]) -> Result<Self, McpError> {
        let url = check_url(url)?;
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| McpError::Config(format!("invalid header name {name}")))?;
            // The value is a credential: never echoed in an error.
            let mut value = HeaderValue::from_str(value).map_err(|_| {
                McpError::Config(format!("the value for header {name} is not valid"))
            })?;
            value.set_sensitive(true);
            map.insert(name, value);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| McpError::Config(e.to_string()))?;
        Ok(Self {
            client,
            url,
            headers: map,
            session: None,
            version: None,
        })
    }

    fn post(&self, message: &Value) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .post(self.url.clone())
            .headers(self.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(message.to_string());
        if let Some(s) = &self.session {
            r = r.header(SESSION, s);
        }
        if let Some(v) = &self.version {
            r = r.header(VERSION, v);
        }
        r
    }

    /// Checks the status of an answer; keeps the session id it assigns.
    async fn accepted(
        &mut self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, McpError> {
        let status = response.status();
        if status.as_u16() == 404 && self.session.is_some() {
            return Err(McpError::SessionExpired);
        }
        if !status.is_success() {
            let text = body(response).await.unwrap_or_default();
            return Err(McpError::Http {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&text).chars().take(300).collect(),
            });
        }
        if let Some(id) = response
            .headers()
            .get(SESSION)
            .and_then(|v| v.to_str().ok())
            && !id.is_empty()
            && id.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            self.session = Some(id.to_owned());
        }
        Ok(response)
    }

    pub async fn notify(&mut self, message: &Value) -> Result<(), McpError> {
        let response = self.post(message).send().await.map_err(http_error)?;
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
        let response = self.post(message).send().await.map_err(http_error)?;
        let mut response = self.accepted(response).await?;
        let kind = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
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
        let Some(s) = self.session.take() else {
            return;
        };
        let mut r = self
            .client
            .delete(self.url.clone())
            .headers(self.headers.clone())
            .header(SESSION, s);
        if let Some(v) = &self.version {
            r = r.header(VERSION, v);
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), r.send()).await;
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
