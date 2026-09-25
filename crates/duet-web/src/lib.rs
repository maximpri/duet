// SPDX-License-Identifier: GPL-3.0-or-later
//! Host-side web access for the frontier's `web_fetch` and `web_search` tools.
//!
//! Commands never get the network; these requests are made by the host, on the
//! frontier's behalf, under fixed rules:
//! - `GET` only, `http` and `https` only, no credentials in URLs;
//! - every address is checked after name resolution and the connection is
//!   pinned to the checked address ([`guard`]); every redirect is checked the
//!   same way, at most [`MAX_REDIRECTS`] of them;
//! - bodies are capped at `max_bytes`, requests at `timeout`;
//! - HTML becomes readable text ([`html`]), JSON and text stay as they are,
//!   anything else is refused;
//! - no proxy from the environment, no cookies, a User-Agent naming Duet.
//!
//! This crate only talks HTTP. Checking what is sent (the URL, the query) and
//! presenting what comes back is the agent's job, through its presenter.

pub mod guard;
pub mod html;
pub mod search;

use futures_util::future::BoxFuture;
use guard::Allowlist;
pub use search::{Backend, SearchResult};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use url::Url;

pub const MAX_REDIRECTS: usize = 5;
pub const USER_AGENT: &str = concat!(
    "duet/",
    env!("CARGO_PKG_VERSION"),
    " (coding agent; host-side fetch on behalf of a model)"
);
/// Results a search returns when no count is given, and at most.
pub const DEFAULT_RESULTS: usize = 5;
pub const MAX_RESULTS: usize = 20;
/// Longest URL or query accepted.
pub const MAX_URL_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebError {
    /// The request itself is not allowed (scheme, credentials, address).
    #[error("refused: {0}")]
    Refused(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("{0}")]
    Network(String),
    #[error("timed out after {0} seconds")]
    Timeout(u64),
    #[error("more than {MAX_REDIRECTS} redirects")]
    TooManyRedirects,
    #[error("binary content ({0}) is not shown")]
    Binary(String),
    #[error("search: {0}")]
    Search(String),
}

impl WebError {
    /// A short outcome name for the audit log.
    pub fn outcome(&self) -> &'static str {
        match self {
            WebError::Refused(_) => "refused",
            WebError::Invalid(_) => "invalid",
            WebError::Network(_) => "network_error",
            WebError::Timeout(_) => "timeout",
            WebError::TooManyRedirects => "too_many_redirects",
            WebError::Binary(_) => "binary",
            WebError::Search(_) => "search_error",
        }
    }
}

/// Name resolution, replaceable in tests.
pub trait Resolve: Send + Sync {
    fn resolve(
        &self,
        host: String,
        port: u16,
    ) -> BoxFuture<'static, std::io::Result<Vec<SocketAddr>>>;
}

/// The system resolver.
pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(
        &self,
        host: String,
        port: u16,
    ) -> BoxFuture<'static, std::io::Result<Vec<SocketAddr>>> {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), port))
                .await?
                .collect())
        })
    }
}

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub max_bytes: usize,
    pub timeout: Duration,
    pub allowlist: Allowlist,
    pub search: Option<Backend>,
}

/// A fetched page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The URL finally fetched (after redirects).
    pub url: String,
    pub status: u16,
    pub content_type: String,
    /// The content as text (HTML converted).
    pub text: String,
    /// Bytes received.
    pub bytes: usize,
    /// The body was longer than `max_bytes` and was cut there.
    pub truncated: bool,
}

/// What kind of body a response has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Body {
    Html,
    Text,
    /// Unknown: decided by looking at the bytes.
    Sniff,
    Binary,
}

fn body_kind(content_type: &str) -> Body {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "" | "application/octet-stream" => Body::Sniff,
        "text/html" | "application/xhtml+xml" => Body::Html,
        m if m.starts_with("text/")
            || m.ends_with("+json")
            || m.ends_with("+xml")
            || matches!(
                m,
                "application/json"
                    | "application/xml"
                    | "application/javascript"
                    | "application/x-javascript"
                    | "application/ecmascript"
                    | "application/toml"
                    | "application/yaml"
                    | "application/x-yaml"
                    | "application/x-sh"
                    | "application/sql"
                    | "application/graphql"
                    | "application/x-ndjson"
            ) =>
        {
            Body::Text
        }
        _ => Body::Binary,
    }
}

/// Decodes a body: UTF-8, or Latin-1 when the charset says so.
fn decode(bytes: &[u8], content_type: &str) -> String {
    let ct = content_type.to_ascii_lowercase();
    let latin1 = [
        "iso-8859-1",
        "latin1",
        "latin-1",
        "windows-1252",
        "us-ascii",
    ]
    .iter()
    .any(|c| ct.contains(&format!("charset={c}")) || ct.contains(&format!("charset=\"{c}\"")));
    if latin1 {
        bytes.iter().map(|&b| b as char).collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Whether bytes of unknown type look like text (no NUL, mostly valid UTF-8).
fn looks_textual(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(4096)];
    if head.contains(&0) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        // A multi-byte character cut at the end of the sample is fine.
        Err(e) => e.error_len().is_none(),
    }
}

fn looks_like_html(text: &str) -> bool {
    let start: String = text
        .trim_start()
        .chars()
        .take(64)
        .collect::<String>()
        .to_ascii_lowercase();
    start.starts_with("<!doctype html") || start.starts_with("<html")
}

pub struct Web {
    cfg: WebConfig,
    resolver: Arc<dyn Resolve>,
}

impl Web {
    pub fn new(cfg: WebConfig) -> Self {
        Self {
            cfg,
            resolver: Arc::new(SystemResolver),
        }
    }

    /// Uses `resolver` for names (tests).
    pub fn with_resolver(mut self, resolver: Arc<dyn Resolve>) -> Self {
        self.resolver = resolver;
        self
    }

    pub fn config(&self) -> &WebConfig {
        &self.cfg
    }

    /// The configured search backend, if any.
    pub fn search_backend(&self) -> Option<&Backend> {
        self.cfg.search.as_ref()
    }

    /// Checks a URL's form: scheme, credentials, host. Returns it parsed.
    pub fn parse_url(raw: &str) -> Result<Url, WebError> {
        let raw = raw.trim();
        if raw.len() > MAX_URL_CHARS {
            return Err(WebError::Invalid(format!(
                "URL longer than {MAX_URL_CHARS} characters"
            )));
        }
        let url = Url::parse(raw).map_err(|e| WebError::Invalid(format!("not a URL: {e}")))?;
        Self::check_form(&url)?;
        Ok(url)
    }

    fn check_form(url: &Url) -> Result<(), WebError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(WebError::Refused(format!(
                "only http and https URLs are fetched, not {}:",
                url.scheme()
            )));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(WebError::Refused(
                "URLs with credentials are not fetched".into(),
            ));
        }
        if url.host().is_none() {
            return Err(WebError::Invalid("the URL has no host".into()));
        }
        Ok(())
    }

    /// The address to connect to for `url`: its host resolved, every address
    /// checked, one chosen. `None` for an IP-literal host (connected as is).
    async fn checked_addr(
        &self,
        url: &Url,
        deadline: Instant,
    ) -> Result<Option<SocketAddr>, WebError> {
        let port = url.port_or_known_default().unwrap_or(80);
        let refuse = |host: &str, ip: IpAddr, why: &str| {
            WebError::Refused(format!(
                "{host} is {why} ({ip}); only public addresses are fetched (owner setting web.allowlist_private)"
            ))
        };
        match url.host() {
            Some(url::Host::Ipv4(v4)) => {
                let ip = IpAddr::V4(v4);
                match guard::refusal(ip, &self.cfg.allowlist, false) {
                    Some(why) => Err(refuse(&ip.to_string(), ip, why)),
                    None => Ok(None),
                }
            }
            Some(url::Host::Ipv6(v6)) => {
                let ip = IpAddr::V6(v6);
                match guard::refusal(ip, &self.cfg.allowlist, false) {
                    Some(why) => Err(refuse(&ip.to_string(), ip, why)),
                    None => Ok(None),
                }
            }
            Some(url::Host::Domain(name)) => {
                if guard::is_metadata_name(name) {
                    return Err(WebError::Refused(format!(
                        "{name} is a cloud metadata service"
                    )));
                }
                let named_ok = self.cfg.allowlist.has_name(name);
                let lookup = self.resolver.resolve(name.to_owned(), port);
                let addrs = tokio::time::timeout_at(deadline, lookup)
                    .await
                    .map_err(|_| WebError::Timeout(self.cfg.timeout.as_secs()))?
                    .map_err(|e| WebError::Network(format!("cannot resolve {name}: {e}")))?;
                if addrs.is_empty() {
                    return Err(WebError::Network(format!("{name} has no address")));
                }
                // Every address must pass: a name with one private address
                // among public ones is refused rather than raced.
                for a in &addrs {
                    if let Some(why) = guard::refusal(a.ip(), &self.cfg.allowlist, named_ok) {
                        return Err(refuse(name, a.ip(), why));
                    }
                }
                let chosen = addrs.iter().find(|a| a.is_ipv4()).unwrap_or(&addrs[0]);
                Ok(Some(SocketAddr::new(chosen.ip(), port)))
            }
            None => Err(WebError::Invalid("the URL has no host".into())),
        }
    }

    /// A client for one request: no redirects followed, no proxy, no cookies,
    /// and `host` pinned to `addr` when given.
    fn client(
        &self,
        host: Option<(&str, SocketAddr)>,
        timeout: Duration,
    ) -> Result<reqwest::Client, WebError> {
        let mut b = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .connect_timeout(timeout);
        if let Some((name, addr)) = host {
            b = b.resolve(name, addr);
        }
        b.build()
            .map_err(|e| WebError::Network(format!("client: {e}")))
    }

    fn remaining(&self, deadline: Instant) -> Result<Duration, WebError> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(WebError::Timeout(self.cfg.timeout.as_secs()));
        }
        Ok(left)
    }

    fn map_err(&self, e: reqwest::Error) -> WebError {
        if e.is_timeout() {
            WebError::Timeout(self.cfg.timeout.as_secs())
        } else {
            // Without the URL: it is already known to the caller.
            WebError::Network(format!("request failed: {}", e.without_url()))
        }
    }

    /// Fetches `raw` with `GET`, following at most [`MAX_REDIRECTS`] checked
    /// redirects.
    pub async fn fetch(&self, raw: &str) -> Result<Page, WebError> {
        let deadline = Instant::now() + self.cfg.timeout;
        let mut url = Self::parse_url(raw)?;
        let mut redirects = 0;
        loop {
            Self::check_form(&url)?;
            let pinned = self.checked_addr(&url, deadline).await?;
            let host = url.host_str().unwrap_or_default().to_owned();
            let client = self.client(
                pinned.map(|a| (host.as_str(), a)),
                self.remaining(deadline)?,
            )?;
            let resp = client
                .get(url.clone())
                .header(
                    reqwest::header::ACCEPT,
                    "text/html, text/plain, application/json, */*;q=0.5",
                )
                .send()
                .await
                .map_err(|e| self.map_err(e))?;
            let status = resp.status();
            if status.is_redirection() {
                let Some(location) = resp.headers().get(reqwest::header::LOCATION) else {
                    return Err(WebError::Network(format!("{status} without a Location")));
                };
                let location = location.to_str().map_err(|_| {
                    WebError::Invalid("a redirect Location that is not text".into())
                })?;
                redirects += 1;
                if redirects > MAX_REDIRECTS {
                    return Err(WebError::TooManyRedirects);
                }
                url = url
                    .join(location)
                    .map_err(|e| WebError::Invalid(format!("redirect to an invalid URL: {e}")))?;
                continue;
            }
            return self.read_page(url, resp, deadline).await;
        }
    }

    async fn read_page(
        &self,
        url: Url,
        mut resp: reqwest::Response,
        deadline: Instant,
    ) -> Result<Page, WebError> {
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let mut kind = body_kind(&content_type);
        if kind == Body::Binary {
            return Err(WebError::Binary(content_type));
        }
        let (bytes, truncated) = self.read_capped(&mut resp, deadline).await?;
        if kind == Body::Sniff {
            if !looks_textual(&bytes) {
                return Err(WebError::Binary(if content_type.is_empty() {
                    "unknown type".into()
                } else {
                    content_type
                }));
            }
            kind = if looks_like_html(&String::from_utf8_lossy(&bytes[..bytes.len().min(256)])) {
                Body::Html
            } else {
                Body::Text
            };
        }
        let raw = decode(&bytes, &content_type);
        let text = match kind {
            Body::Html => html::to_text(&raw, Some(&url)),
            _ => raw,
        };
        Ok(Page {
            url: url.to_string(),
            status,
            content_type: if content_type.is_empty() {
                "unknown".into()
            } else {
                content_type
            },
            text,
            bytes: bytes.len(),
            truncated,
        })
    }

    /// Reads the body up to `max_bytes`; the rest is not read.
    async fn read_capped(
        &self,
        resp: &mut reqwest::Response,
        deadline: Instant,
    ) -> Result<(Vec<u8>, bool), WebError> {
        let cap = self.cfg.max_bytes;
        let mut out = Vec::new();
        loop {
            let chunk = tokio::time::timeout_at(deadline, resp.chunk())
                .await
                .map_err(|_| WebError::Timeout(self.cfg.timeout.as_secs()))?
                .map_err(|e| self.map_err(e))?;
            let Some(chunk) = chunk else {
                return Ok((out, false));
            };
            let room = cap - out.len();
            if chunk.len() > room {
                out.extend_from_slice(&chunk[..room]);
                return Ok((out, true));
            }
            out.extend_from_slice(&chunk);
        }
    }

    /// Searches with the configured backend. The backend is the owner's own
    /// endpoint, so it is not subject to the address check.
    pub async fn search(&self, query: &str, count: usize) -> Result<Vec<SearchResult>, WebError> {
        let backend = self
            .cfg
            .search
            .as_ref()
            .ok_or_else(|| WebError::Search("no search backend is configured".into()))?;
        if query.trim().is_empty() {
            return Err(WebError::Invalid("the query is empty".into()));
        }
        if query.len() > MAX_URL_CHARS {
            return Err(WebError::Invalid(format!(
                "query longer than {MAX_URL_CHARS} characters"
            )));
        }
        let count = count.clamp(1, MAX_RESULTS);
        let deadline = Instant::now() + self.cfg.timeout;
        let (url, headers) = backend.request(query, count);
        let client = self.client(None, self.cfg.timeout)?;
        let mut req = client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json");
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let mut resp = req.send().await.map_err(|e| self.map_err(e))?;
        let status = resp.status();
        let (bytes, truncated) = self.read_capped(&mut resp, deadline).await?;
        if !status.is_success() {
            return Err(WebError::Search(format!(
                "{} answered {status}",
                backend.name()
            )));
        }
        if truncated {
            return Err(WebError::Search(format!(
                "the {} reply is larger than web.max_bytes",
                backend.name()
            )));
        }
        let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
            WebError::Search(format!("the {} reply is not JSON: {e}", backend.name()))
        })?;
        backend.parse(&body, count).map_err(WebError::Search)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_need_http_a_host_and_no_credentials() {
        assert!(Web::parse_url("https://example.org/a?b=c").is_ok());
        for bad in [
            "file:///etc/passwd",
            "ftp://example.org/",
            "gopher://example.org/",
            "https://user:pass@example.org/",
            "https://token@example.org/",
            "not a url",
            "data:text/plain,hi",
        ] {
            assert!(Web::parse_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn content_types_are_classified() {
        assert_eq!(body_kind("text/html; charset=utf-8"), Body::Html);
        assert_eq!(body_kind("application/json"), Body::Text);
        assert_eq!(body_kind("application/vnd.api+json"), Body::Text);
        assert_eq!(body_kind("text/plain"), Body::Text);
        assert_eq!(body_kind(""), Body::Sniff);
        assert_eq!(body_kind("image/png"), Body::Binary);
        assert_eq!(body_kind("application/pdf"), Body::Binary);
        assert!(looks_textual("héllo".as_bytes()));
        assert!(!looks_textual(b"\x89PNG\r\n\x1a\n\0\0"));
        assert_eq!(
            decode(&[0x63, 0x61, 0x66, 0xe9], "text/plain; charset=ISO-8859-1"),
            "café"
        );
    }
}
