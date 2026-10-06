// SPDX-License-Identifier: GPL-3.0-or-later
//! Requests to third parties: every recipient other than the frontier
//! provider and the local model (web pages, search backends, MCP servers
//! over HTTP, package registries through the egress proxy).
//!
//! What the frontier's tools send to a third party is text the frontier
//! chose (a URL, a query, tool arguments), so every byte of such a request is
//! checked by the run's presenter before it leaves, as every frontier request
//! passes the outbound gate. The check is proven by types, the way the gate
//! is (the agent holds a [`crate::GatedFrontier`], never a provider):
//! - a [`Guard`] is the presenter's check, owned, so a request made later
//!   or in another task (the egress proxy's) is checked against what the run
//!   knows then. Only a presenter makes one
//!   ([`crate::view::Presenter::outbound_guard`]);
//! - a [`Checked`] request is one every part of which a guard passed. Only
//!   [`Guard::check`] makes one;
//! - the product's HTTP client for third parties (`declass-net`) sends only a
//!   [`Checked`] request, and `tools/gate.sh` refuses networking code
//!   anywhere else.
//!
//! The parts read: the host and each of its labels (the name goes to a
//! resolver before anything connects: DNS is a channel), the port, the path
//! and each decoded segment, the query and each decoded key and value, every
//! header name and value, and every string and key of a JSON body (JSON
//! inside a string decoded too). The parts are also read joined, and the
//! values alone joined, so a value cut into consecutive parts is found.
//! Credentials the owner configured (API keys read from environment
//! variables) are not part of the request: the client adds them, and they
//! go only to the endpoint they were configured for.

use serde_json::Value;
use std::sync::Arc;
use url::Url;

/// Levels of JSON inside a JSON string that are read.
const NESTED_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Delete => "DELETE",
        }
    }
}

/// A request body.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Sent as `application/json`.
    Json(Value),
    Text(String),
}

/// A request for a third party, before the check.
#[derive(Debug, Clone, PartialEq)]
pub struct Outgoing {
    pub method: Method,
    pub url: Url,
    /// Every header the request carries besides the client's own fixed ones
    /// (`User-Agent`, `Content-Length`, ...) and the owner's credentials.
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
}

impl Outgoing {
    pub fn get(url: Url) -> Self {
        Self {
            method: Method::Get,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn post_json(url: Url, body: Value) -> Self {
        Self {
            method: Method::Post,
            url,
            headers: Vec::new(),
            body: Some(Body::Json(body)),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// Why nothing was sent. The reason names what was found (a kind of value
/// and where it came from, a placeholder), never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: String,
    /// The destination as it may be named in errors and the audit log
    /// ([`Guard::name`]): a host name can itself carry the value.
    pub destination: String,
}

/// Names a destination that itself holds what the check refuses.
pub const WITHHELD_DESTINATION: &str = "a host whose name holds a withheld value";

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

/// A request every part of which passed a [`Guard`]. Only [`Guard::check`]
/// makes one; it cannot be changed after the check.
#[derive(Debug)]
pub struct Checked {
    request: Outgoing,
}

impl Checked {
    pub fn request(&self) -> &Outgoing {
        &self.request
    }

    pub fn url(&self) -> &Url {
        &self.request.url
    }
}

/// The texts of one request, as a check reads them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Texts {
    /// Every part on its own.
    pub parts: Vec<String>,
    /// The parts joined in order, and the values alone joined in order
    /// (host labels, path segments, query values, header values, JSON
    /// string values): a value cut into consecutive parts.
    pub joined: Vec<String>,
}

impl Texts {
    /// One text (a query, tool arguments written as text).
    pub fn of(text: &str) -> Self {
        Self {
            parts: vec![text.to_owned()],
            joined: Vec::new(),
        }
    }

    /// Every string and key of `value` (JSON inside a string too).
    pub fn of_value(value: &Value) -> Self {
        let mut t = Collect::default();
        t.value(value, 0);
        t.finish()
    }

    /// Every part of `request`.
    pub fn of_request(request: &Outgoing) -> Self {
        let mut t = Collect::default();
        let url = &request.url;
        if let Some(host) = url.host_str() {
            t.part(host);
            for label in host.split('.') {
                t.value_part(label);
            }
        }
        if let Some(port) = url.port() {
            t.part(&port.to_string());
        }
        if !url.username().is_empty() {
            t.value_part(url.username());
        }
        if let Some(p) = url.password() {
            t.value_part(p);
        }
        t.part(url.path());
        for segment in url.path().split('/').filter(|s| !s.is_empty()) {
            t.value_part(&crate::reencoded::percent_decode(segment));
        }
        if let Some(q) = url.query() {
            t.part(q);
            for (k, v) in url.query_pairs() {
                t.part(&k);
                t.value_part(&v);
            }
        }
        if let Some(f) = url.fragment() {
            t.value_part(f);
        }
        for (name, value) in &request.headers {
            t.part(name);
            t.value_part(value);
        }
        match &request.body {
            Some(Body::Json(v)) => t.value(v, 0),
            Some(Body::Text(s)) => t.value_part(s),
            None => {}
        }
        t.finish()
    }
}

#[derive(Default)]
struct Collect {
    parts: Vec<String>,
    all: Vec<String>,
    values: Vec<String>,
}

impl Collect {
    fn part(&mut self, s: &str) {
        if !s.is_empty() {
            self.parts.push(s.to_owned());
            self.all.push(s.to_owned());
        }
    }

    fn value_part(&mut self, s: &str) {
        if !s.is_empty() {
            self.part(s);
            self.values.push(s.to_owned());
        }
    }

    fn value(&mut self, v: &Value, depth: usize) {
        match v {
            Value::String(s) => {
                self.value_part(s);
                if depth < NESTED_DEPTH
                    && s.trim_start().starts_with(['{', '[', '"'])
                    && let Ok(inner) = serde_json::from_str::<Value>(s)
                    && !inner.is_string()
                {
                    self.value(&inner, depth + 1);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| self.value(x, depth)),
            Value::Object(m) => {
                for (k, x) in m {
                    self.part(k);
                    self.value(x, depth);
                }
            }
            Value::Number(n) => self.value_part(&n.to_string()),
            Value::Bool(_) | Value::Null => {}
        }
    }

    fn finish(self) -> Texts {
        let mut joined = Vec::new();
        if self.all.len() > 1 {
            joined.push(self.all.join(" "));
        }
        if self.values.len() > 1 {
            joined.push(self.values.join(" "));
        }
        Texts {
            parts: self.parts,
            joined,
        }
    }
}

/// What a presenter checks third-party text against.
pub(crate) trait Policy: Send + Sync {
    /// Why `texts` may not be sent to `destination`; `None`: they may.
    fn refusal(&self, destination: &str, texts: &Texts) -> Option<String>;
    /// A refusal, for the run's audit log (an `outbound_refused` event).
    fn refused(&self, channel: &str, destination: &str, reason: &str);
}

/// A presenter's check of what goes to third parties, owned and cheap to
/// clone. Without the boundary (pass-through) every request passes.
#[derive(Clone)]
pub struct Guard {
    policy: Option<Arc<dyn Policy>>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.policy.is_some() {
            "Guard(checked)"
        } else {
            "Guard(open)"
        })
    }
}

impl Guard {
    /// No boundary: nothing is refused (pass-through mode).
    pub(crate) fn open() -> Self {
        Self { policy: None }
    }

    pub(crate) fn new(policy: Arc<dyn Policy>) -> Self {
        Self {
            policy: Some(policy),
        }
    }

    /// Whether this guard refuses nothing (pass-through mode).
    pub fn is_open(&self) -> bool {
        self.policy.is_none()
    }

    /// `destination` as it may be named in errors and the audit log: as it
    /// is, unless the check refuses the name itself (a host label that
    /// spells a withheld value), which is then named by
    /// [`WITHHELD_DESTINATION`]. A refused name is never written anywhere.
    pub fn name(&self, destination: &str) -> String {
        match &self.policy {
            Some(p) if p.refusal("-", &Texts::of(destination)).is_some() => {
                WITHHELD_DESTINATION.to_owned()
            }
            _ => destination.to_owned(),
        }
    }

    fn decide(&self, channel: &str, destination: &str, texts: &Texts) -> Result<(), Refusal> {
        let Some(policy) = &self.policy else {
            return Ok(());
        };
        if policy.refusal(destination, texts).is_none() {
            return Ok(());
        }
        let destination = self.name(destination);
        let reason = policy
            .refusal(&destination, texts)
            .unwrap_or_else(|| "it holds a withheld value".into());
        policy.refused(channel, &destination, &reason);
        Err(Refusal {
            reason,
            destination,
        })
    }

    /// `request` checked in every part for `destination` (a host or a server
    /// name), on behalf of `channel` (the tool or client). A refusal is
    /// recorded in the run's audit log and nothing may be sent.
    pub fn check(
        &self,
        channel: &str,
        destination: &str,
        request: Outgoing,
    ) -> Result<Checked, Refusal> {
        self.decide(channel, destination, &Texts::of_request(&request))?;
        Ok(Checked { request })
    }

    /// Text for a third party that is not an HTTP request (arguments for a
    /// local MCP server that has network, a command line that would have it).
    pub fn check_text(&self, channel: &str, destination: &str, text: &str) -> Result<(), Refusal> {
        self.decide(channel, destination, &Texts::of(text))
    }

    /// A JSON value for a third party: every string and key.
    pub fn check_value(
        &self,
        channel: &str,
        destination: &str,
        value: &Value,
    ) -> Result<(), Refusal> {
        self.decide(channel, destination, &Texts::of_value(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_part_of_a_request_is_read() {
        let url =
            Url::parse("https://quartz.otter.evil.test:4539/a/sk%5Flive/b?q=one+two&k%20x=z#frag")
                .unwrap();
        let request = Outgoing::post_json(
            url,
            json!({"params": {"arguments": {"text": "{\"inner\": \"deep\"}", "n": 12}}}),
        )
        .header("X-Note", "hdr");
        let t = Texts::of_request(&request);
        for want in [
            "quartz.otter.evil.test",
            "otter",
            "4539",
            "sk_live",
            "one two",
            "k x",
            "frag",
            "X-Note",
            "hdr",
            "arguments",
            "deep",
            "12",
        ] {
            assert!(t.parts.iter().any(|p| p == want), "{want}: {:?}", t.parts);
        }
        // Values alone, joined: labels, segments, query values, header
        // values, JSON strings, in order.
        let values = t.joined.last().unwrap();
        assert!(
            values.starts_with("quartz otter evil test a sk_live b one two z frag hdr"),
            "{values}"
        );
        assert!(values.contains("deep"), "{values}");
    }

    #[test]
    fn an_open_guard_passes_everything_and_a_policy_decides_otherwise() {
        struct No;
        impl Policy for No {
            fn refusal(&self, _: &str, t: &Texts) -> Option<String> {
                t.parts
                    .iter()
                    .any(|p| p.contains("no"))
                    .then(|| "said no".into())
            }
            fn refused(&self, _: &str, _: &str, _: &str) {}
        }
        let url = Url::parse("https://example.test/no").unwrap();
        assert!(
            Guard::open()
                .check("t", "d", Outgoing::get(url.clone()))
                .is_ok()
        );
        let g = Guard::new(Arc::new(No));
        assert_eq!(
            g.check("t", "d", Outgoing::get(url)).unwrap_err().reason,
            "said no"
        );
        assert!(g.check_text("t", "d", "yes").is_ok());
        assert!(g.check_value("t", "d", &json!({"no": 1})).is_err());
    }
}
