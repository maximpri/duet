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
//! - no proxy from the environment, no cookies, a User-Agent naming Declass.
//!
//! Searches go to the configured backend; the native one ([`search::native`])
//! asks public sources itself, through the same guarded client as a fetch.
//!
//! Every request, redirects and each native source's included, is checked in
//! every part by the run's presenter before a name is resolved or a byte
//! sent (the [`Guard`] the caller passes; see `declass_boundary::third_party`)
//! and sent by `declass-net`, which sends nothing else. Presenting what comes
//! back is the agent's job.

pub mod guard;
pub mod html;
pub mod search;

use declass_boundary::third_party::{Guard, Outgoing};
use declass_net::{Checked, Client, Credentials, Options};
use guard::Allowlist;
pub use search::{Backend, SearchResult, Searched, SourceReport};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use url::Url;

pub const MAX_REDIRECTS: usize = 5;
/// Names the software and where to learn about it, as API operators such as
/// Wikimedia ask of automated clients; nothing about the operator.
pub const USER_AGENT: &str = concat!(
    "declass/",
    env!("CARGO_PKG_VERSION"),
    " (coding agent; host-side fetch on behalf of a model; +",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);
/// Results a search returns when no count is given, and at most.
pub const DEFAULT_RESULTS: usize = 5;
pub const MAX_RESULTS: usize = 20;
/// Longest URL or query accepted.
pub const MAX_URL_CHARS: usize = 4096;
/// Least time between two Wikipedia searches: Wikimedia asks API clients to
/// send requests one at a time and gently.
pub const WIKIPEDIA_INTERVAL: Duration = Duration::from_secs(1);

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
    /// The boundary's check refused the request: nothing was sent.
    #[error("not sent: {0}")]
    NotSent(declass_boundary::third_party::Refusal),
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
            WebError::NotSent(_) => "refused_outbound",
        }
    }
}

/// Name resolution: the product's resolver lives in `declass-net`, the only
/// networking crate for third parties; re-exported for the egress proxy and
/// tests.
pub use declass_net::{Resolve, SystemResolver};

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
    /// When the last Wikipedia search ended; held during one, so they go one
    /// at a time (sub-agents share the run's `Web`).
    paced: tokio::sync::Mutex<Option<Instant>>,
    /// The session with an MCP search server, kept between searches.
    mcp: tokio::sync::Mutex<Option<declass_mcp::Client>>,
    /// The native backend's pacing per service and answers kept for the run.
    native: search::native::NativeState,
}

impl Web {
    pub fn new(cfg: WebConfig) -> Self {
        Self {
            cfg,
            resolver: Arc::new(SystemResolver),
            paced: tokio::sync::Mutex::new(None),
            mcp: tokio::sync::Mutex::new(None),
            native: search::native::NativeState::default(),
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

    /// `request` checked by `guard` for its host: nothing is sent, and no name
    /// is resolved, unless every part of it passed.
    fn checked(guard: &Guard, channel: &str, request: Outgoing) -> Result<Checked, WebError> {
        let host = request.url.host_str().unwrap_or("web").to_owned();
        guard
            .check(channel, &host, request)
            .map_err(WebError::NotSent)
    }

    /// The address to connect to for a checked request: its host resolved,
    /// every address checked, one chosen. `None` for an IP-literal host
    /// (connected as is).
    async fn checked_addr(
        &self,
        request: &Checked,
        deadline: Instant,
    ) -> Result<Option<SocketAddr>, WebError> {
        let url = request.url();
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
                let lookup = declass_net::lookup(self.resolver.as_ref(), request);
                let addrs = tokio::time::timeout_at(deadline, lookup)
                    .await
                    .map_err(|_| WebError::Timeout(self.cfg.timeout.as_secs()))?
                    .map_err(|e| WebError::Network(format!("cannot resolve {name}: {e}")))?
                    .unwrap_or_default();
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
    ) -> Result<Client, WebError> {
        Client::new(Options {
            timeout: Some(timeout),
            connect_timeout: timeout,
            user_agent: Some(USER_AGENT),
            pin: host.map(|(name, addr)| (name.to_owned(), addr)),
        })
        .map_err(|e| WebError::Network(e.to_string()))
    }

    fn remaining(&self, deadline: Instant) -> Result<Duration, WebError> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(WebError::Timeout(self.cfg.timeout.as_secs()));
        }
        Ok(left)
    }

    fn map_err(&self, e: declass_net::NetError) -> WebError {
        match e {
            declass_net::NetError::Timeout => WebError::Timeout(self.cfg.timeout.as_secs()),
            // Without the URL: it is already known to the caller.
            other => WebError::Network(format!("request failed: {other}")),
        }
    }

    /// Fetches `raw` with `GET`, following at most [`MAX_REDIRECTS`] checked
    /// redirects. `guard` checks every request (the URL the frontier gave and
    /// each redirect) before its name is resolved.
    pub async fn fetch(&self, guard: &Guard, raw: &str) -> Result<Page, WebError> {
        let deadline = Instant::now() + self.cfg.timeout;
        let mut url = Self::parse_url(raw)?;
        let mut redirects = 0;
        loop {
            Self::check_form(&url)?;
            if let Some(engine) = search_engine_page(&url) {
                return Err(WebError::Refused(format!(
                    "{engine} result pages are not fetched: search with web_search, which asks \
public sources directly"
                )));
            }
            let request = Self::checked(
                guard,
                "web_fetch",
                Outgoing::get(url.clone()).header(
                    "Accept",
                    "text/html, text/plain, application/json, */*;q=0.5",
                ),
            )?;
            let pinned = self.checked_addr(&request, deadline).await?;
            let host = url.host_str().unwrap_or_default().to_owned();
            let client = self.client(
                pinned.map(|a| (host.as_str(), a)),
                self.remaining(deadline)?,
            )?;
            let resp = client
                .send(&request, &Credentials::default())
                .await
                .map_err(|e| self.map_err(e))?;
            let status = resp.status();
            if (300..400).contains(&status) {
                let Some(location) = resp.header("location") else {
                    return Err(WebError::Network(format!("{status} without a Location")));
                };
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
        mut resp: declass_net::Response,
        deadline: Instant,
    ) -> Result<Page, WebError> {
        let status = resp.status();
        let content_type = resp.header("content-type").unwrap_or_default().to_owned();
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
        resp: &mut declass_net::Response,
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

    /// Searches with the configured backend; the results only.
    pub async fn search(
        &self,
        guard: &Guard,
        query: &str,
        count: usize,
    ) -> Result<Vec<SearchResult>, WebError> {
        self.search_with(guard, query, count, None)
            .await
            .map(|s| s.results)
    }

    /// Searches with the configured backend. A single backend is a fixed
    /// endpoint (the owner's own instance or a known provider), so it is not
    /// subject to the address check; the native backend's sources are
    /// (`sources` picks among them; `None`: its defaults). Either way every
    /// request is checked by `guard`, and the reply says what each source did.
    pub async fn search_with(
        &self,
        guard: &Guard,
        query: &str,
        count: usize,
        sources: Option<&[String]>,
    ) -> Result<Searched, WebError> {
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
        if let Backend::Native(native) = backend {
            return self
                .native_search(guard, native, query, count, sources)
                .await;
        }
        if sources.is_some_and(|s| !s.is_empty()) {
            return Err(WebError::Invalid(format!(
                "this run searches with {}, which has no sources to choose from",
                backend.name()
            )));
        }
        let (results, bytes) = if let Backend::Wikipedia { .. } = backend {
            // Checked before waiting its turn: a refused search costs no turn.
            let prepared = self.prepare(guard, backend, query, count)?;
            let mut last = self.paced.lock().await;
            if let Some(t) = *last {
                tokio::time::sleep_until(t + WIKIPEDIA_INTERVAL).await;
            }
            let out = self.send_prepared(guard, backend, prepared, count).await;
            *last = Some(Instant::now());
            out?
        } else {
            let prepared = self.prepare(guard, backend, query, count)?;
            self.send_prepared(guard, backend, prepared, count).await?
        };
        Ok(Searched {
            requests: vec![SourceReport {
                source: backend.name(),
                host: backend.host(),
                bytes,
                outcome: "ok",
                note: String::new(),
                hits: results.len(),
            }],
            results,
            labelled: false,
        })
    }

    /// One search on a single backend, built and checked: nothing is sent
    /// (and no turn of a paced backend is taken) when the check refuses.
    fn prepare(
        &self,
        guard: &Guard,
        backend: &Backend,
        query: &str,
        count: usize,
    ) -> Result<Prepared, WebError> {
        let Some(call) = backend.call(query, count) else {
            return Err(WebError::Search(format!(
                "{} is not a single endpoint",
                backend.name()
            )));
        };
        match call {
            // The headers a backend call names are the owner's credentials
            // (its key): added by the client, never part of what is checked.
            search::Call::Http { url, headers, body } => {
                let outgoing = match body {
                    Some(body) => Outgoing::post_json(url, body),
                    None => Outgoing::get(url),
                }
                .header("Accept", "application/json");
                Ok(Prepared::Http {
                    request: Self::checked(guard, "web_search", outgoing)?,
                    credentials: Credentials::new(
                        headers
                            .into_iter()
                            .map(|(k, v)| (k.to_owned(), v))
                            .collect(),
                    ),
                })
            }
            // The MCP transport checks every message it sends; the arguments
            // are checked here too, so a refusal opens no session.
            search::Call::Mcp {
                url,
                headers,
                tool,
                arguments,
            } => {
                guard
                    .check_value(
                        "web_search",
                        &backend.host(),
                        &serde_json::Value::Object(arguments.clone()),
                    )
                    .map_err(WebError::NotSent)?;
                Ok(Prepared::Mcp {
                    url,
                    headers,
                    tool,
                    arguments,
                })
            }
        }
    }

    /// Sends a prepared search: its results and the reply's size.
    async fn send_prepared(
        &self,
        guard: &Guard,
        backend: &Backend,
        prepared: Prepared,
        count: usize,
    ) -> Result<(Vec<SearchResult>, usize), WebError> {
        let (request, credentials) = match prepared {
            Prepared::Http {
                request,
                credentials,
            } => (request, credentials),
            Prepared::Mcp {
                url,
                headers,
                tool,
                arguments,
            } => {
                return self
                    .search_mcp(guard, backend, &url, &headers, tool, &arguments, count)
                    .await;
            }
        };
        let deadline = Instant::now() + self.cfg.timeout;
        let client = self.client(None, self.cfg.timeout)?;
        let mut resp = client
            .send(&request, &credentials)
            .await
            .map_err(|e| self.map_err(e))?;
        let status = resp.status();
        let (bytes, truncated) = self.read_capped(&mut resp, deadline).await?;
        if !(200..300).contains(&status) {
            // The reply's text is not shown: it would reach the frontier
            // outside the presenter.
            return Err(WebError::Search(format!(
                "{} answered {}{}",
                backend.name(),
                status_line(status),
                backend.refusal(status, &bytes)
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
        let results = backend.parse(&body, count).map_err(WebError::Search)?;
        Ok((results, bytes.len()))
    }

    /// One `tools/call` on an MCP search server, over the kept session (a
    /// new one after any failure). The server's own error text is not shown.
    /// Every message of the session is checked by the guard that opened it.
    #[allow(clippy::too_many_arguments)]
    async fn search_mcp(
        &self,
        guard: &Guard,
        backend: &Backend,
        url: &Url,
        headers: &[(String, String)],
        tool: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
        count: usize,
    ) -> Result<(Vec<SearchResult>, usize), WebError> {
        let timeout = self.cfg.timeout;
        let name = backend.name();
        let failed = |e: declass_mcp::McpError| match e {
            declass_mcp::McpError::Timeout(_) => WebError::Timeout(timeout.as_secs()),
            declass_mcp::McpError::Http { status, .. } => WebError::Search(format!(
                "{name} answered HTTP {status}{}",
                backend.refusal(status, b"")
            )),
            declass_mcp::McpError::Rpc { code, .. } => {
                WebError::Search(format!("{name} answered with error {code}"))
            }
            declass_mcp::McpError::Config(m) => WebError::Invalid(m),
            declass_mcp::McpError::NotSent(r) => WebError::NotSent(r),
            _ => WebError::Network(format!("{name} could not be reached or did not answer")),
        };
        let mut slot = self.mcp.lock().await;
        if slot.is_none() {
            let transport = declass_mcp::Transport::http(url.as_str(), headers, guard.clone())
                .map_err(failed)?;
            *slot = Some(
                declass_mcp::Client::connect(transport, timeout)
                    .await
                    .map_err(failed)?,
            );
        }
        let Some(client) = slot.as_mut() else {
            return Err(WebError::Network(format!("{name}: no session")));
        };
        let result = match client.call_tool(tool, arguments, timeout).await {
            Ok(r) => r,
            Err(declass_mcp::McpError::NotSent(r)) => {
                // Refused before it left: the session is still good.
                return Err(WebError::NotSent(r));
            }
            Err(e) => {
                *slot = None;
                return Err(failed(e));
            }
        };
        if result.is_error {
            return Err(WebError::Search(format!(
                "{name} reported an error (a spent quota, for example; `declass doctor` shows the search setup)"
            )));
        }
        if result.text.len() > self.cfg.max_bytes {
            return Err(WebError::Search(format!(
                "the {name} reply is larger than web.max_bytes"
            )));
        }
        let results = backend
            .parse_text(&result.text, count)
            .map_err(WebError::Search)?;
        Ok((results, result.text.len()))
    }
}

/// The search engine whose result page `url` is, if it is one. Searching is
/// `web_search`'s job, through sources with open APIs chosen by the owner
/// (SECURITY.md, Web tools): a result page fetched instead sends the query to
/// an engine nobody chose, and the engines' terms forbid automated queries.
/// Their other pages (a home page, documentation) are fetched as usual.
pub fn search_engine_page(url: &Url) -> Option<&'static str> {
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    let path = url.path().trim_end_matches('/');
    let query = |k: &str| url.query_pairs().any(|(name, _)| name == k);
    // `name` is a registered domain label (`google` in `www.google.co.uk`).
    let under = |name: &str| {
        host.split('.')
            .collect::<Vec<_>>()
            .windows(2)
            .any(|w| w[0] == name && !w[1].is_empty())
            && !host.ends_with(".test")
    };
    let is = |h: &str| host == h || host.ends_with(&format!(".{h}"));
    let engine = if under("google") && matches!(path, "/search" | "/webhp" | "/url") {
        "Google"
    } else if is("bing.com") && path == "/search" {
        "Bing"
    } else if is("duckduckgo.com")
        && (host.starts_with("html.") || host.starts_with("lite.") || query("q"))
    {
        "DuckDuckGo"
    } else if host == "search.yahoo.com" || (is("yahoo.co.jp") && host.starts_with("search.")) {
        "Yahoo"
    } else if (under("yandex") || is("ya.ru")) && path.starts_with("/search") {
        "Yandex"
    } else if is("baidu.com") && path == "/s" {
        "Baidu"
    } else if is("startpage.com") && (path.ends_with("/search") || query("query")) {
        "Startpage"
    } else if host == "search.brave.com" {
        "Brave Search"
    } else if is("ecosia.org") && path == "/search" {
        "Ecosia"
    } else if is("qwant.com") && query("q") {
        "Qwant"
    } else if is("mojeek.com") && path == "/search" {
        "Mojeek"
    } else if is("kagi.com") && path == "/search" {
        "Kagi"
    } else if host == "search.naver.com" {
        "Naver"
    } else if host == "search.seznam.cz" {
        "Seznam"
    } else if is("so.com") && path == "/s" {
        "360 Search"
    } else if is("sogou.com") && path == "/web" {
        "Sogou"
    } else {
        return None;
    };
    Some(engine)
}

/// A single backend's search, checked and ready to send.
enum Prepared {
    Http {
        request: Checked,
        credentials: Credentials,
    },
    /// An MCP search server's tool (its transport checks every message).
    Mcp {
        url: Url,
        headers: Vec<(String, String)>,
        tool: &'static str,
        arguments: serde_json::Map<String, serde_json::Value>,
    },
}

/// `404 Not Found`, as the HTTP client showed a status before.
fn status_line(status: u16) -> String {
    let reason = match status {
        400 => " Bad Request",
        401 => " Unauthorized",
        403 => " Forbidden",
        404 => " Not Found",
        429 => " Too Many Requests",
        500 => " Internal Server Error",
        502 => " Bad Gateway",
        503 => " Service Unavailable",
        _ => "",
    };
    format!("{status}{reason}")
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
    fn search_engine_result_pages_are_recognized_and_other_pages_are_not() {
        let page = |u: &str| search_engine_page(&Url::parse(u).unwrap());
        for serp in [
            "https://www.google.com/search?q=tokio",
            "https://www.google.co.uk/search?q=x",
            "https://html.duckduckgo.com/html/?q=x",
            "https://lite.duckduckgo.com/lite/",
            "https://duckduckgo.com/?q=x&ia=web",
            "https://www.bing.com/search?q=x",
            "https://search.yahoo.com/search?p=x",
            "https://yandex.ru/search/?text=x",
            "https://www.baidu.com/s?wd=x",
            "https://www.startpage.com/do/search?query=x",
            "https://search.brave.com/search?q=x",
            "https://www.ecosia.org/search?q=x",
        ] {
            assert!(page(serp).is_some(), "{serp}");
        }
        for other in [
            "https://www.google.com/",
            "https://developers.google.com/search/docs",
            "https://duckduckgo.com/about",
            "https://docs.rs/tokio/latest/tokio/",
            "https://github.com/search-engine/search",
            "https://en.wikipedia.org/wiki/Search_engine",
            "https://api.search.brave.test/search",
        ] {
            assert!(page(other).is_none(), "{other}");
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
