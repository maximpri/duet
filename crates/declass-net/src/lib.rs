// SPDX-License-Identifier: GPL-3.0-or-later
//! The product's one HTTP client and resolver for third parties: web pages,
//! search backends and MCP servers over HTTP (see SECURITY.md, Egress).
//!
//! It sends only a [`Checked`] request, one the run's presenter passed in
//! every part (host and labels, path, query, headers, body;
//! [`declass_boundary::third_party`]), and no type here builds one. The other
//! code that opens connections is the frontier and local-model client
//! (`declass-provider`, which the agent reaches only through the outbound gate)
//! and the commands' egress proxy (`declass-egress`, listed registries only);
//! `tools/gate.sh` refuses HTTP clients, sockets and resolver calls anywhere
//! else in the product.
//!
//! Every request: no redirect followed (a redirect is a new request, which
//! the caller checks), no proxy from the environment, no cookies, and
//! optionally the host pinned to an address the caller resolved with
//! [`lookup`] and checked, so a second resolution cannot answer differently.
//! Credentials the owner configured ([`Credentials`]) are added here, marked
//! sensitive, and never shown in errors.

pub use declass_boundary::third_party::{Body, Checked, Method};
use futures_util::future::BoxFuture;
use std::net::SocketAddr;
use std::time::Duration;
#[cfg(test)]
use url::Url;

/// Name resolution, replaceable in tests.
pub trait Resolve: Send + Sync {
    fn resolve(
        &self,
        host: String,
        port: u16,
    ) -> BoxFuture<'static, std::io::Result<Vec<SocketAddr>>>;
}

/// The system resolver: the product's only DNS lookup.
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

/// The addresses of a checked request's host: a name reaches a resolver only
/// after the check passed (a host label can carry data to whoever answers
/// for the domain). `Ok(None)` for a host that is an IP address.
pub async fn lookup(
    resolver: &dyn Resolve,
    request: &Checked,
) -> std::io::Result<Option<Vec<SocketAddr>>> {
    let url = request.url();
    let port = url.port_or_known_default().unwrap_or(80);
    match url.host() {
        Some(url::Host::Domain(name)) => resolver.resolve(name.to_owned(), port).await.map(Some),
        Some(_) => Ok(None),
        None => Err(std::io::Error::other("the URL has no host")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("timed out")]
    Timeout,
    /// The server could not be reached (refused, unreachable, TLS failed).
    #[error("cannot connect: {0}")]
    Connect(String),
    #[error("{0}")]
    Invalid(String),
    #[error("request failed: {0}")]
    Other(String),
}

impl NetError {
    fn of(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            NetError::Timeout
        } else if e.is_connect() {
            // Without the URL: the caller knows it.
            NetError::Connect(e.without_url().to_string())
        } else {
            NetError::Other(e.without_url().to_string())
        }
    }
}

/// How requests are made.
#[derive(Debug, Clone)]
pub struct Options {
    /// The whole request, body included; `None`: no limit here (the caller
    /// bounds it).
    pub timeout: Option<Duration>,
    pub connect_timeout: Duration,
    /// `User-Agent`, when the endpoint asks automated clients to name themselves.
    pub user_agent: Option<&'static str>,
    /// Connect to this address for this host name (one the caller resolved
    /// and checked); a request for another host is refused.
    pub pin: Option<(String, SocketAddr)>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: None,
            connect_timeout: Duration::from_secs(10),
            user_agent: None,
            pin: None,
        }
    }
}

/// Header values the owner configured for one endpoint (API keys read from
/// environment variables). Only names are ever shown.
#[derive(Clone, Default)]
pub struct Credentials(Vec<(String, String)>);

impl Credentials {
    pub fn new(headers: Vec<(String, String)>) -> Self {
        Self(headers)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(name, _)| format!("{name}: <withheld>")))
            .finish()
    }
}

/// A client for checked requests.
pub struct Client {
    inner: reqwest::Client,
    pin: Option<(String, SocketAddr)>,
}

impl Client {
    pub fn new(options: Options) -> Result<Self, NetError> {
        let mut b = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(options.connect_timeout);
        if let Some(t) = options.timeout {
            b = b.timeout(t);
        }
        if let Some(ua) = options.user_agent {
            b = b.user_agent(ua);
        }
        if let Some((name, addr)) = &options.pin {
            b = b.resolve(name, *addr);
        }
        Ok(Self {
            inner: b
                .build()
                .map_err(|e| NetError::Invalid(format!("client: {e}")))?,
            pin: options.pin,
        })
    }

    /// Sends `request` with `credentials` added. The response's body is read
    /// by the caller ([`Response::chunk`]).
    pub async fn send(
        &self,
        request: &Checked,
        credentials: &Credentials,
    ) -> Result<Response, NetError> {
        let r = request.request();
        if let Some((name, _)) = &self.pin
            && r.url.host_str() != Some(name.as_str())
        {
            return Err(NetError::Invalid(
                "the request is for another host than the checked address".into(),
            ));
        }
        let method = match r.method {
            Method::Get => reqwest::Method::GET,
            Method::Head => reqwest::Method::HEAD,
            Method::Post => reqwest::Method::POST,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut rb = self.inner.request(method, r.url.clone());
        for (name, value) in &r.headers {
            rb = rb.header(name.as_str(), value.as_str());
        }
        for (name, value) in &credentials.0 {
            let mut v = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
                NetError::Invalid(format!("the value for header {name} is not valid"))
            })?;
            v.set_sensitive(true);
            rb = rb.header(name.as_str(), v);
        }
        match &r.body {
            Some(Body::Json(v)) => {
                if !r
                    .headers
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
                {
                    rb = rb.header(reqwest::header::CONTENT_TYPE, "application/json");
                }
                rb = rb.body(v.to_string());
            }
            Some(Body::Text(s)) => rb = rb.body(s.clone()),
            None => {}
        }
        rb.send()
            .await
            .map(|inner| Response { inner })
            .map_err(NetError::of)
    }
}

/// A response whose body is read in pieces.
pub struct Response {
    inner: reqwest::Response,
}

impl Response {
    pub fn status(&self) -> u16 {
        self.inner.status().as_u16()
    }

    /// A header's value, when it is text.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.inner.headers().get(name).and_then(|v| v.to_str().ok())
    }

    /// The next piece of the body; `None` at its end.
    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, NetError> {
        self.inner
            .chunk()
            .await
            .map(|c| c.map(|b| b.to_vec()))
            .map_err(NetError::of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use declass_boundary::third_party::Outgoing;
    use declass_boundary::view::{PassThrough, Presenter};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One loopback server that records the request bytes it gets and answers 200.
    async fn server() -> (u16, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") || !got.ends_with(b"}") {
                match s.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                }
            }
            let _ = s
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-A: b\r\n\r\nok")
                .await;
            got
        });
        (port, task)
    }

    #[tokio::test]
    async fn sends_the_checked_request_with_credentials_and_pins_its_host() {
        let guard = PassThrough { max_bytes: 0 }.outbound_guard();
        let (port, task) = server().await;
        let url = Url::parse(&format!("http://api.test:{port}/q?x=1")).unwrap();
        let checked = guard
            .check(
                "test",
                "api.test",
                Outgoing::post_json(url, serde_json::json!({"a": 1})).header("Accept", "x/y"),
            )
            .unwrap();
        let client = Client::new(Options {
            pin: Some(("api.test".into(), SocketAddr::from(([127, 0, 0, 1], port)))),
            ..Options::default()
        })
        .unwrap();
        let creds = Credentials::new(vec![("Authorization".into(), "Bearer k-123".into())]);
        assert!(!format!("{creds:?}").contains("k-123"));
        let mut resp = client.send(&checked, &creds).await.unwrap();
        assert_eq!((resp.status(), resp.header("x-a")), (200, Some("b")));
        assert_eq!(resp.chunk().await.unwrap().unwrap(), b"ok");
        let got = String::from_utf8(task.await.unwrap()).unwrap();
        assert!(got.starts_with("POST /q?x=1 HTTP/1.1\r\n"), "{got}");
        for want in [
            "accept: x/y",
            "authorization: Bearer k-123",
            "content-type: application/json",
            "{\"a\":1}",
        ] {
            assert!(
                got.to_lowercase().contains(&want.to_lowercase()),
                "{want}: {got}"
            );
        }
        // A pinned client sends nothing for another host.
        let other = guard
            .check(
                "test",
                "x",
                Outgoing::get(Url::parse("http://other.test/").unwrap()),
            )
            .unwrap();
        assert!(matches!(
            client.send(&other, &Credentials::default()).await,
            Err(NetError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn lookup_resolves_only_a_checked_name() {
        struct Record(std::sync::Mutex<Vec<String>>);
        impl Resolve for Record {
            fn resolve(
                &self,
                host: String,
                port: u16,
            ) -> BoxFuture<'static, std::io::Result<Vec<SocketAddr>>> {
                self.0.lock().unwrap().push(host);
                Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
            }
        }
        let r = Record(Default::default());
        let guard = PassThrough { max_bytes: 0 }.outbound_guard();
        let named = guard
            .check(
                "t",
                "d",
                Outgoing::get(Url::parse("https://a.test/").unwrap()),
            )
            .unwrap();
        let ip = guard
            .check(
                "t",
                "d",
                Outgoing::get(Url::parse("https://10.1.2.3/").unwrap()),
            )
            .unwrap();
        assert_eq!(lookup(&r, &named).await.unwrap().unwrap()[0].port(), 443);
        assert!(lookup(&r, &ip).await.unwrap().is_none());
        assert_eq!(*r.0.lock().unwrap(), vec!["a.test".to_owned()]);
    }
}
