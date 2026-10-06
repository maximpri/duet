// SPDX-License-Identifier: GPL-3.0-or-later
//! The host-side egress proxy for sandboxed commands
//! (`sandbox.network = "registries"`).
//!
//! A command with the proxy route can reach nothing but this proxy (and the
//! servers it starts itself on loopback). The proxy speaks HTTP/1.1 proxy
//! requests: `CONNECT host:port` for HTTPS and plain-HTTP `GET`/`HEAD` in
//! absolute form. For each connection it:
//! - allows the target only if its host is on the allowlist
//!   (`sandbox.registries`, [`Hosts`]) with an allowed port;
//! - resolves the name itself and refuses it when any address is private,
//!   loopback, link-local or a cloud metadata address (the same classes as
//!   the web tools, `declass_web::guard`), then connects to a checked address;
//! - for a tunnel, reads the TLS ClientHello that opens it and refuses it
//!   unless it names the same host ([`tls`]); nothing else of the tunnel is
//!   looked at (no TLS interception);
//! - for plain HTTP, sends one request with `Host` set from the target and
//!   no body, then only the response, and only after the run's outbound check
//!   passed every part of that request (`declass_boundary::third_party`: its
//!   path, query and headers can carry what the command was given); a
//!   tunnel's host passes the same check;
//! - reports one [`Event`] (host, port, bytes each way, outcome), never a
//!   path, a query or content, and never a host name the check refuses.
//!
//! One [`Route`] serves one command: a loopback listener under Seatbelt, a
//! bridge channel under bubblewrap (see `declass_sandbox::bridge`). It stops
//! accepting when dropped.

pub mod head;
pub mod hosts;
pub mod tls;

use declass_boundary::third_party::{Guard, Method, Outgoing};
use declass_sandbox::{ProxyRoute, SandboxKind};
use declass_web::{Resolve, guard};
pub use hosts::Hosts;
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// How long a client may take to send its request head, or a tunnel's
/// first TLS record.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(15);

/// What the proxy allows.
#[derive(Debug, Clone)]
pub struct Rules {
    pub hosts: Hosts,
    /// Addresses exempt from the private-address rule. Declass leaves it empty;
    /// tests route an allowed name to a server on loopback with it. Cloud
    /// metadata addresses stay refused even so.
    pub exempt: guard::Allowlist,
    pub connect_timeout: Duration,
}

impl Rules {
    pub fn new(hosts: Hosts) -> Self {
        Self {
            hosts,
            exempt: guard::Allowlist::default(),
            connect_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Connected; bytes flowed as counted.
    Allowed,
    /// The request broke a rule.
    Refused,
    /// An allowed target could not be reached (name, connection).
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Allowed => "allowed",
            Outcome::Refused => "refused",
            Outcome::Failed => "failed",
        }
    }
}

/// One connection through the proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The target host as a normalized name; empty when the request named
    /// none that could be read.
    pub host: String,
    pub port: u16,
    /// Bytes from the command to the target, and back.
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub outcome: Outcome,
    /// Why it was refused or failed (empty when allowed).
    pub reason: String,
}

/// Receives every [`Event`] (the run's audit log).
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// The proxy of one run or session: its rules, resolver and event sink.
/// Cheap to clone; it listens only through [`Proxy::route`].
#[derive(Clone)]
pub struct Proxy {
    inner: Arc<Inner>,
}

struct Inner {
    rules: Rules,
    resolver: Arc<dyn Resolve>,
    sink: Sink,
    /// The run's outbound check (see `declass_boundary::third_party`).
    guard: Guard,
}

/// The targets one route refused, for the command's output.
#[derive(Debug, Default)]
pub struct Refusals(Mutex<Vec<String>>);

impl Refusals {
    fn add(&self, target: String) {
        let mut seen = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !seen.contains(&target) && seen.len() < 20 {
            seen.push(target);
        }
    }
}

/// One command's way to the proxy. Dropping it stops accepting new
/// connections; open ones run to their end.
pub struct Route {
    /// Put this in the command's `declass_sandbox::Spec`.
    pub network: ProxyRoute,
    refusals: Arc<Refusals>,
    accept: tokio::task::JoinHandle<()>,
}

impl Route {
    /// `host:port (reason)` of every refused request so far (at most 20).
    pub fn refused(&self) -> Vec<String> {
        self.refusals
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl Proxy {
    /// A proxy that allows `rules`, resolves with `resolver`, reports to
    /// `sink` and checks what it forwards with `guard` (the run's presenter's).
    pub fn new(rules: Rules, resolver: Arc<dyn Resolve>, sink: Sink, guard: Guard) -> Self {
        Self {
            inner: Arc::new(Inner {
                rules,
                resolver,
                sink,
                guard,
            }),
        }
    }

    /// A route for one command under `kind`'s sandbox. Bubblewrap needs the
    /// bridge helper's command line (`helper`, e.g. `declass __sandbox-bridge`).
    pub async fn route(&self, kind: SandboxKind, helper: &[OsString]) -> std::io::Result<Route> {
        let refusals = Arc::new(Refusals::default());
        let (proxy, log) = (self.clone(), refusals.clone());
        let (network, accept) = match kind {
            SandboxKind::Seatbelt => {
                let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
                let port = listener.local_addr()?.port();
                let accept = tokio::spawn(async move {
                    while let Ok((client, _)) = listener.accept().await {
                        let (proxy, log) = (proxy.clone(), log.clone());
                        tokio::spawn(async move { proxy.serve(client, &log).await });
                    }
                });
                (ProxyRoute::Loopback { port }, accept)
            }
            SandboxKind::Bubblewrap => {
                if helper.is_empty() {
                    return Err(std::io::Error::other(
                        "the egress bridge needs its helper program",
                    ));
                }
                let (network, incoming) = declass_sandbox::bridge::channel(helper.to_vec())?;
                let accept = tokio::spawn(async move {
                    while let Some(Ok(client)) = incoming.next().await {
                        let (proxy, log) = (proxy.clone(), log.clone());
                        tokio::spawn(async move { proxy.serve(client, &log).await });
                    }
                });
                (network, accept)
            }
        };
        Ok(Route {
            network,
            refusals,
            accept,
        })
    }

    /// Serves one client connection to its end.
    pub async fn serve<S>(&self, mut client: S, refusals: &Refusals)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        let mut record = Record::new(self.inner.sink.clone());
        let mut buf = Vec::with_capacity(4096);
        let end = loop {
            if let Some(end) = head::head_end(&buf) {
                break end;
            }
            if buf.len() > head::MAX_HEAD {
                record.decide(Outcome::Refused, "the request head is too long");
                respond(&mut client, 431, "the request head is too long").await;
                return;
            }
            match tokio::time::timeout(HEAD_TIMEOUT, client.read_buf(&mut buf)).await {
                Ok(Ok(n)) if n > 0 => {}
                // Closed or silent before a whole request: nothing to record.
                _ => {
                    if buf.is_empty() {
                        record.silent = true;
                    } else {
                        record.decide(Outcome::Refused, "an incomplete request");
                    }
                    return;
                }
            }
        };
        let request = match head::parse(&buf[..end]) {
            Ok(r) => r,
            Err(refusal) => {
                if let Some((host, port)) = &refusal.target {
                    record.target(&self.inner.guard.name(host), *port);
                }
                self.refuse(
                    &mut client,
                    &mut record,
                    refusals,
                    refusal.status,
                    &refusal.reason,
                )
                .await;
                return;
            }
        };
        let (raw_host, port) = request.target();
        let Some(host) = hosts::normalize(raw_host) else {
            record.port = port;
            let why = "the target is not a host name";
            self.refuse(&mut client, &mut record, refusals, 403, why)
                .await;
            return;
        };
        record.target(&self.inner.guard.name(&host), port);
        // What leaves: a tunnel's host, or every part of a plain request.
        let checked = match &request {
            head::Request::Connect { .. } => {
                self.inner.guard.check_text("egress proxy", &host, &host)
            }
            head::Request::Forward { head, .. } => match outgoing(&host, port, head) {
                Some(out) => self.inner.guard.check("egress proxy", &host, out).map(drop),
                None => {
                    let why = "a request head that cannot be checked";
                    self.refuse(&mut client, &mut record, refusals, 400, why)
                        .await;
                    return;
                }
            },
        };
        if let Err(r) = checked {
            let why = format!("not sent: {}", r.reason);
            self.refuse(&mut client, &mut record, refusals, 403, &why)
                .await;
            return;
        }
        let upstream = match self.connect(&host, port).await {
            Ok(s) => s,
            Err((outcome, status, why)) => {
                if outcome == Outcome::Refused {
                    self.refuse(&mut client, &mut record, refusals, status, &why)
                        .await;
                } else {
                    record.decide(outcome, &why);
                    respond(&mut client, status, &format!("{host}:{port}: {why}")).await;
                }
                return;
            }
        };
        let rest = buf[end..].to_vec();
        match request {
            head::Request::Connect { .. } => {
                self.tunnel(client, upstream, rest, &host, port, &mut record, refusals)
                    .await;
            }
            head::Request::Forward { head, .. } => {
                forward(client, upstream, &head, &mut record).await;
            }
        }
    }

    async fn refuse<S: AsyncWrite + Unpin>(
        &self,
        client: &mut S,
        record: &mut Record,
        refusals: &Refusals,
        status: u16,
        why: &str,
    ) {
        record.decide(Outcome::Refused, why);
        let target = if record.host.is_empty() {
            why.to_owned()
        } else {
            format!("{}:{} ({why})", record.host, record.port)
        };
        refusals.add(target);
        let message = if record.host.is_empty() {
            why.to_owned()
        } else {
            format!("{}:{}: {why}", record.host, record.port)
        };
        respond(client, status, &message).await;
    }

    /// Checks `host:port` and connects to it. The error says whether the
    /// request was refused or the target failed, the status and why.
    async fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Result<tokio::net::TcpStream, (Outcome, u16, String)> {
        let rules = &self.inner.rules;
        if !rules.hosts.allows(host, port) {
            return Err((
                Outcome::Refused,
                403,
                "not an allowed package registry (the operator can add it to sandbox.registries)"
                    .into(),
            ));
        }
        if guard::is_metadata_name(host) {
            return Err((Outcome::Refused, 403, "a cloud metadata service".into()));
        }
        let lookup = self.inner.resolver.resolve(host.to_owned(), port);
        let addrs = match tokio::time::timeout(RESOLVE_TIMEOUT, lookup).await {
            Ok(Ok(a)) if !a.is_empty() => a,
            Ok(Ok(_)) => return Err((Outcome::Failed, 502, "the name has no address".into())),
            Ok(Err(e)) => return Err((Outcome::Failed, 502, format!("cannot resolve: {e}"))),
            Err(_) => return Err((Outcome::Failed, 504, "resolving timed out".into())),
        };
        // Every address must pass: a name with one private address among
        // public ones is refused rather than raced.
        for a in &addrs {
            if let Some(why) = guard::refusal(a.ip(), &rules.exempt, false) {
                return Err((Outcome::Refused, 403, format!("the name resolves to {why}")));
            }
        }
        let mut ordered: Vec<SocketAddr> = addrs.iter().filter(|a| a.is_ipv4()).copied().collect();
        ordered.extend(addrs.iter().filter(|a| a.is_ipv6()));
        let mut last = String::new();
        for addr in ordered {
            let addr = SocketAddr::new(addr.ip(), port);
            match tokio::time::timeout(rules.connect_timeout, tokio::net::TcpStream::connect(addr))
                .await
            {
                Ok(Ok(s)) => return Ok(s),
                Ok(Err(e)) => last = format!("cannot connect: {e}"),
                Err(_) => last = "connecting timed out".into(),
            }
        }
        Err((Outcome::Failed, 502, last))
    }

    #[allow(clippy::too_many_arguments)]
    async fn tunnel<S>(
        &self,
        mut client: S,
        mut upstream: tokio::net::TcpStream,
        mut first: Vec<u8>,
        host: &str,
        port: u16,
        record: &mut Record,
        refusals: &Refusals,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send,
    {
        if client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .is_err()
        {
            record.decide(Outcome::Failed, "the client left");
            return;
        }
        let hello = loop {
            match tls::client_hello(&first) {
                tls::Hello::Incomplete if first.len() < tls::MAX_RECORD => {}
                tls::Hello::Incomplete => break tls::Hello::Unnamed,
                other => break other,
            }
            match tokio::time::timeout(HEAD_TIMEOUT, client.read_buf(&mut first)).await {
                Ok(Ok(n)) if n > 0 => {}
                _ => break tls::Hello::Incomplete,
            }
        };
        let why = match hello {
            tls::Hello::Name(name) if name == host => None,
            tls::Hello::Name(_) => Some("the TLS session is for another host"),
            tls::Hello::Unnamed => Some("a tunnel carries only TLS naming its host"),
            tls::Hello::Incomplete => {
                // Clients open tunnels they never use (connection pools): not
                // a refusal, and nothing crossed.
                record.decide(Outcome::Failed, "the client closed the tunnel unused");
                return;
            }
        };
        if let Some(why) = why {
            // The tunnel is already open: refuse by closing it.
            record.decide(Outcome::Refused, why);
            refusals.add(format!("{host}:{port} ({why})"));
            return;
        }
        record.decide(Outcome::Allowed, "");
        if upstream.write_all(&first).await.is_err() {
            return;
        }
        record.up.fetch_add(first.len() as u64, Ordering::Relaxed);
        let (cr, cw) = tokio::io::split(client);
        let (ur, uw) = upstream.into_split();
        tokio::join!(pump(cr, uw, &record.up), pump(ur, cw, &record.down));
    }
}

/// The plain-HTTP request `head` (as it would be sent to `host:port`) for
/// the outbound check: its method, URL (path and query) and every header.
fn outgoing(host: &str, port: u16, head: &[u8]) -> Option<Outgoing> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = match first.next()? {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        _ => return None,
    };
    let url = url::Url::parse(&format!("http://{host}:{port}{}", first.next()?)).ok()?;
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Some(Outgoing {
        method,
        url,
        headers,
        body: None,
    })
}

/// Sends a plain-HTTP request head and relays the response. Nothing more
/// the client sends is forwarded: one request per connection.
async fn forward<S>(
    mut client: S,
    mut upstream: tokio::net::TcpStream,
    head: &[u8],
    record: &mut Record,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    record.decide(Outcome::Allowed, "");
    if upstream.write_all(head).await.is_err() {
        return;
    }
    record.up.fetch_add(head.len() as u64, Ordering::Relaxed);
    let (ur, _uw) = upstream.into_split();
    let (_cr, cw) = tokio::io::split(&mut client);
    pump(ur, cw, &record.down).await;
}

/// Copies until the reader ends, counting, then ends the writer.
async fn pump<R, W>(mut r: R, mut w: W, count: &AtomicU64)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match r.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if w.write_all(&buf[..n]).await.is_err() {
                    break;
                }
                count.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
    }
    let _ = w.shutdown().await;
}

async fn respond<S: AsyncWrite + Unpin>(client: &mut S, status: u16, message: &str) {
    let reason = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let body = format!("declass egress proxy: {message}\n");
    let text = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = client.write_all(text.as_bytes()).await;
    let _ = client.shutdown().await;
}

/// One connection's event, sent when the connection ends however it ends
/// (also when its task is cancelled).
struct Record {
    sink: Sink,
    host: String,
    port: u16,
    up: AtomicU64,
    down: AtomicU64,
    decided: Option<(Outcome, String)>,
    /// Closed before sending anything: no event.
    silent: bool,
}

impl Record {
    fn new(sink: Sink) -> Self {
        Self {
            sink,
            host: String::new(),
            port: 0,
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            decided: None,
            silent: false,
        }
    }

    fn target(&mut self, host: &str, port: u16) {
        self.host = hosts::normalize(host).unwrap_or_default();
        self.port = port;
    }

    fn decide(&mut self, outcome: Outcome, why: &str) {
        self.decided = Some((outcome, why.to_owned()));
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        if self.silent {
            return;
        }
        let (outcome, reason) = self
            .decided
            .take()
            .unwrap_or((Outcome::Failed, "the connection ended early".into()));
        (self.sink)(Event {
            host: std::mem::take(&mut self.host),
            port: self.port,
            bytes_up: self.up.load(Ordering::Relaxed),
            bytes_down: self.down.load(Ordering::Relaxed),
            outcome,
            reason,
        });
    }
}

/// Addresses exempt from the private-address rule, for tests: `ips` as
/// single-address networks.
pub fn exempt(ips: &[IpAddr]) -> guard::Allowlist {
    guard::Allowlist::parse(&ips.iter().map(ToString::to_string).collect::<Vec<_>>())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
