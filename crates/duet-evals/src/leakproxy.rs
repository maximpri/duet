// SPDX-License-Identifier: GPL-3.0-or-later
//! Logging reverse proxy between an agent and its frontier provider.
//!
//! Each lane's model endpoint points at the proxy; the proxy forwards to the
//! real upstream (appending the request path) without TLS interception. Every
//! request body is stored and scanned for the run's canaries, independently of
//! anything the agent reports about itself. Responses stream through unchanged
//! and are stored so token usage can be recomputed from the provider's own
//! numbers.
//!
//! A request body the proxy cannot read as sent (any `Content-Encoding` other
//! than identity, such as a zstd-compressed request) is refused with 415 and
//! never forwarded: an unscannable request would be an unmeasured leak.
//!
//! WebSocket upgrades are forwarded without the client's
//! `Sec-WebSocket-Extensions`, so no compression is negotiated. After the 101
//! the proxy relays both directions byte for byte while reading the frames
//! ([`crate::wsframe`]). Each client message is a request: it is stored,
//! scanned and logged (with a `ws` marker) before its frames are forwarded.
//! Server messages are appended, one `data:` line each, to the response
//! capture of the client message they follow, so usage is read from them as
//! from a streamed HTTP response. Traffic the proxy cannot read (an extension
//! the upstream negotiated anyway, a frame that breaks the protocol) is still
//! relayed, and recorded in `ws.jsonl` as uninspected: the run's leak count is
//! then not a measurement.

use crate::canary::Manifest;
use crate::wsframe::{self, ControlKind, DataKind};
use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{BodyExt, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::header::HeaderMap;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

/// Largest WebSocket message the proxy reads; a larger one is uninspected.
const MAX_WS_MESSAGE: usize = 64 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub seq: u64,
    pub unix_ms: u128,
    pub method: String,
    pub path: String,
    pub bytes: usize,
    pub sha256: String,
    pub status: u16,
    pub leaked: Vec<String>,
    /// Set for a WebSocket handshake and for each client message of a session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws: Option<WsInfo>,
}

impl RequestRecord {
    /// A handshake that opened a WebSocket session: the session's messages,
    /// not the handshake, are its frontier requests.
    pub fn is_open_handshake(&self) -> bool {
        self.status == 101 && self.ws.as_ref().is_some_and(|w| w.role == WsRole::Upgrade)
    }

    fn is_ws_message(&self) -> bool {
        self.ws.as_ref().is_some_and(|w| w.role == WsRole::Message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WsRole {
    /// The HTTP upgrade request (`seq` is the session id).
    Upgrade,
    /// One client-to-server data message.
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsInfo {
    /// The `seq` of the session's upgrade request.
    pub session: u64,
    pub role: WsRole,
    /// `text` or `binary`, for messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opcode: Option<String>,
    /// The connection closed before the message's final fragment; the stored
    /// body is what arrived.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unfinished: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeakRecord {
    pub seq: u64,
    pub canary: String,
    pub kind: crate::canary::CanaryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Client,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsEventKind {
    /// The upstream negotiated an extension although none was offered.
    Extension,
    /// A frame the reader could not decode; the rest of that direction is
    /// relayed without reading.
    ProtocolError,
    Close,
    /// The upgraded connection could not be taken over.
    RelayFailed,
}

/// Session-level WebSocket events (`ws.jsonl`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEvent {
    pub session: u64,
    pub unix_ms: u128,
    pub kind: WsEventKind,
    pub side: Side,
    pub detail: String,
    /// Client traffic after this event was relayed without being scanned.
    pub uninspected: bool,
}

struct State {
    upstream: String,
    manifest: Manifest,
    log_dir: PathBuf,
    seq: AtomicU64,
    client: reqwest::Client,
    /// HTTP/1.1 only: WebSocket upgrades.
    upgrade_client: reqwest::Client,
}

pub struct ProxyHandle {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl ProxyHandle {
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn stop(self) {
        self.task.abort();
    }
}

/// Starts the proxy on `listen` (use port 0 for an ephemeral port).
pub async fn start(
    listen: SocketAddr,
    upstream: &str,
    manifest: Manifest,
    log_dir: &Path,
) -> Result<ProxyHandle> {
    fs::create_dir_all(log_dir.join("requests"))?;
    fs::create_dir_all(log_dir.join("responses"))?;
    let state = Arc::new(State {
        upstream: upstream.trim_end_matches('/').to_owned(),
        manifest,
        log_dir: log_dir.to_path_buf(),
        seq: AtomicU64::new(0),
        client: reqwest::Client::builder().build()?,
        upgrade_client: reqwest::Client::builder().http1_only().build()?,
    });
    let listener = TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| handle(state.clone(), req));
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    Ok(ProxyHandle { addr, task })
}

type ProxyBody = BoxBody<Bytes, std::io::Error>;

async fn handle(
    state: Arc<State>,
    req: Request<Incoming>,
) -> Result<Response<ProxyBody>, std::io::Error> {
    match forward(&state, req).await {
        Ok(resp) => Ok(resp),
        Err(err) => {
            let body = format!("duet-eval proxy error: {err:#}");
            let mut resp = Response::new(full(Bytes::from(body)));
            *resp.status_mut() = StatusCode::BAD_GATEWAY;
            Ok(resp)
        }
    }
}

fn full(bytes: Bytes) -> ProxyBody {
    http_body_util::Full::new(bytes)
        .map_err(|never| match never {})
        .boxed()
}

fn header_has_token(headers: &HeaderMap, name: hyper::header::HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|v| {
        String::from_utf8_lossy(v.as_bytes())
            .split(',')
            .any(|t| t.trim().eq_ignore_ascii_case(token))
    })
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    header_has_token(headers, hyper::header::CONNECTION, "upgrade")
        && header_has_token(headers, hyper::header::UPGRADE, "websocket")
}

async fn forward(state: &Arc<State>, mut req: Request<Incoming>) -> Result<Response<ProxyBody>> {
    let seq = state.seq.fetch_add(1, Ordering::SeqCst) + 1;
    let websocket = is_websocket_upgrade(req.headers());
    let downstream_upgrade = websocket.then(|| hyper::upgrade::on(&mut req));
    let (parts, body) = req.into_parts();
    let body = body
        .collect()
        .await
        .context("reading request body")?
        .to_bytes();
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_owned();
    let ws_info = websocket.then_some(WsInfo {
        session: seq,
        role: WsRole::Upgrade,
        opcode: None,
        unfinished: false,
    });

    fs::write(state.log_dir.join(format!("requests/{seq:05}.body")), &body)?;
    if let Some(encoding) = parts
        .headers
        .get(hyper::header::CONTENT_ENCODING)
        .map(|v| {
            String::from_utf8_lossy(v.as_bytes())
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|e| !e.is_empty() && e != "identity")
    {
        let status = StatusCode::UNSUPPORTED_MEDIA_TYPE;
        let mut record = new_record(
            seq,
            parts.method.as_str(),
            path,
            &body,
            status.as_u16(),
            Vec::new(),
        );
        record.ws = ws_info;
        log_request(state, &record)?;
        let mut resp = Response::new(full(Bytes::from(format!(
            "duet-eval proxy: request body with content-encoding {encoding} cannot be scanned; \
             disable request compression in the agent"
        ))));
        *resp.status_mut() = status;
        return Ok(resp);
    }

    // Scan and store before anything leaves the machine.
    let leaked_names = scan(state, seq, &body)?;

    let url = format!("{}{}", state.upstream, path);
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())?;
    let client = if websocket {
        &state.upgrade_client
    } else {
        &state.client
    };
    let mut upstream = client.request(method, &url).body(body.clone());
    for (name, value) in &parts.headers {
        let n = name.as_str();
        let hop = matches!(
            n,
            "host" | "content-length" | "connection" | "accept-encoding"
        );
        // Offered extensions are dropped so no compression is negotiated;
        // the upgrade headers themselves are forwarded.
        if (hop && !(websocket && n == "connection")) || n == "sec-websocket-extensions" {
            continue;
        }
        upstream = upstream.header(n, value.as_bytes());
    }
    let resp = upstream
        .send()
        .await
        .with_context(|| format!("forwarding to {url}"))?;
    let status = resp.status().as_u16();

    let mut record = new_record(
        seq,
        parts.method.as_str(),
        path.clone(),
        &body,
        status,
        leaked_names,
    );
    record.ws = ws_info;
    log_request(state, &record)?;

    let mut builder = Response::builder().status(status);
    let switching = status == 101 && downstream_upgrade.is_some();
    for (name, value) in resp.headers() {
        let n = name.as_str();
        if matches!(n, "content-length" | "transfer-encoding") || (n == "connection" && !switching)
        {
            continue;
        }
        builder = builder.header(n, value.as_bytes());
    }
    if let (true, Some(downstream)) = (switching, downstream_upgrade) {
        let extensions: Vec<String> = resp
            .headers()
            .get_all("sec-websocket-extensions")
            .iter()
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .collect();
        let session = Arc::new(Session {
            state: state.clone(),
            id: seq,
            path,
            reply_to: AtomicU64::new(seq),
        });
        let inspect = extensions.is_empty();
        if !inspect {
            session.event(
                WsEventKind::Extension,
                Side::Server,
                format!(
                    "upstream negotiated {}; frames relayed unread",
                    extensions.join(", ")
                ),
                true,
            )?;
        }
        tokio::spawn(async move {
            let upstream = match resp.upgrade().await {
                Ok(u) => u,
                Err(e) => {
                    let _ = session.event(
                        WsEventKind::RelayFailed,
                        Side::Server,
                        format!("{e:#}"),
                        false,
                    );
                    return;
                }
            };
            let downstream = match downstream.await {
                Ok(d) => TokioIo::new(d),
                Err(e) => {
                    let _ = session.event(
                        WsEventKind::RelayFailed,
                        Side::Client,
                        format!("{e:#}"),
                        false,
                    );
                    return;
                }
            };
            let (client_read, client_write) = tokio::io::split(downstream);
            let (server_read, server_write) = tokio::io::split(upstream);
            let _ = tokio::join!(
                relay_client(&session, client_read, server_write, inspect),
                relay_server(&session, server_read, client_write, inspect),
            );
        });
        return Ok(builder.body(full(Bytes::new()))?);
    }

    let capture = state.log_dir.join(format!("responses/{seq:05}.body"));
    let mut file = fs::File::create(&capture)?;
    let stream = resp
        .bytes_stream()
        .map_err(std::io::Error::other)
        .map(move |chunk| {
            if let Ok(bytes) = &chunk {
                let _ = file.write_all(bytes);
            }
            chunk.map(Frame::data)
        });
    Ok(builder.body(BodyExt::boxed(StreamBody::new(stream)))?)
}

/// Scans `body` for canaries and records any in `leaks.jsonl` under `seq`.
fn scan(state: &State, seq: u64, body: &[u8]) -> Result<Vec<String>> {
    let text = String::from_utf8_lossy(body);
    let leaked = state.manifest.find_in(&text);
    if !leaked.is_empty() {
        let mut f = append(&state.log_dir.join("leaks.jsonl"))?;
        for c in &leaked {
            let rec = LeakRecord {
                seq,
                canary: c.name.clone(),
                kind: c.kind,
            };
            writeln!(f, "{}", serde_json::to_string(&rec)?)?;
        }
    }
    Ok(leaked.iter().map(|c| c.name.clone()).collect())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

fn new_record(
    seq: u64,
    method: &str,
    path: String,
    body: &[u8],
    status: u16,
    leaked: Vec<String>,
) -> RequestRecord {
    RequestRecord {
        seq,
        unix_ms: unix_ms(),
        method: method.to_owned(),
        path,
        bytes: body.len(),
        sha256: hex::encode(Sha256::digest(body)),
        status,
        leaked,
        ws: None,
    }
}

fn log_request(state: &State, record: &RequestRecord) -> Result<()> {
    writeln!(
        append(&state.log_dir.join("requests.jsonl"))?,
        "{}",
        serde_json::to_string(record)?
    )?;
    Ok(())
}

fn append(path: &Path) -> Result<fs::File> {
    Ok(OpenOptions::new().create(true).append(true).open(path)?)
}

/// One relayed WebSocket session.
struct Session {
    state: Arc<State>,
    /// The upgrade request's `seq`.
    id: u64,
    path: String,
    /// The `seq` whose response capture receives server messages: the latest
    /// client message (the upgrade itself before the first one).
    reply_to: AtomicU64,
}

impl Session {
    fn event(
        &self,
        kind: WsEventKind,
        side: Side,
        detail: String,
        uninspected: bool,
    ) -> Result<()> {
        let event = WsEvent {
            session: self.id,
            unix_ms: unix_ms(),
            kind,
            side,
            detail,
            uninspected,
        };
        writeln!(
            append(&self.state.log_dir.join("ws.jsonl"))?,
            "{}",
            serde_json::to_string(&event)?
        )?;
        Ok(())
    }

    /// Stores, scans and logs one client message; later server messages are
    /// captured as its response.
    fn client_message(&self, message: &wsframe::Message, finished: bool) -> Result<()> {
        let state = &self.state;
        let seq = state.seq.fetch_add(1, Ordering::SeqCst) + 1;
        fs::write(
            state.log_dir.join(format!("requests/{seq:05}.body")),
            &message.payload,
        )?;
        let leaked = scan(state, seq, &message.payload)?;
        let mut record = new_record(seq, "WS", self.path.clone(), &message.payload, 101, leaked);
        record.ws = Some(WsInfo {
            session: self.id,
            role: WsRole::Message,
            opcode: Some(message.kind.name().to_owned()),
            unfinished: !finished,
        });
        log_request(state, &record)?;
        self.reply_to.store(seq, Ordering::SeqCst);
        Ok(())
    }

    /// Appends a server text message to the current response capture, one
    /// `data:` line per message (the shape usage parsing reads).
    fn server_message(&self, message: &wsframe::Message) -> Result<()> {
        if message.kind != DataKind::Text {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&message.payload);
        let text = if text.contains(['\n', '\r']) {
            match serde_json::from_str::<Value>(&text) {
                Ok(v) => serde_json::to_string(&v)?,
                Err(_) => text.lines().collect::<Vec<_>>().join("\ndata: "),
            }
        } else {
            text.into_owned()
        };
        let seq = self.reply_to.load(Ordering::SeqCst);
        let mut f = append(&self.state.log_dir.join(format!("responses/{seq:05}.body")))?;
        write!(f, "data: {text}\n\n")?;
        Ok(())
    }

    fn close(&self, side: Side, payload: &[u8]) -> Result<()> {
        let code = wsframe::close_code(payload).map_or("no code".into(), |c| c.to_string());
        self.event(WsEventKind::Close, side, code, false)
    }
}

/// Client to server. Frames of a data message are held until the message is
/// complete, then it is stored and scanned, then its bytes are forwarded
/// unchanged; control frames between fragments are held with them so the byte
/// order is kept. Without `inspect` bytes pass straight through.
async fn relay_client<R, W>(session: &Session, mut from: R, mut to: W, inspect: bool) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = inspect.then(|| wsframe::Reader::new(MAX_WS_MESSAGE));
    let mut held: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let Some(r) = reader.as_mut() else {
            to.write_all(&buf[..n]).await?;
            continue;
        };
        r.push(&buf[..n]);
        let mut broken = false;
        loop {
            match r.next_frame() {
                Ok(None) => break,
                Ok(Some(frame)) => {
                    held.extend_from_slice(&frame.raw);
                    match frame.content {
                        wsframe::Content::Data(Some(message)) => {
                            session.client_message(&message, true)?;
                        }
                        wsframe::Content::Control {
                            kind: ControlKind::Close,
                            payload,
                        } => {
                            if let Some(m) = r.unfinished() {
                                session.client_message(&m, false)?;
                            }
                            session.close(Side::Client, &payload)?;
                        }
                        _ if r.in_message() => continue,
                        _ => {}
                    }
                    to.write_all(&held).await?;
                    held.clear();
                }
                Err(e) => {
                    if let Some(m) = r.unfinished() {
                        session.client_message(&m, false)?;
                    }
                    session.event(WsEventKind::ProtocolError, Side::Client, e.0, true)?;
                    held.extend(r.take_buffered());
                    to.write_all(&held).await?;
                    held.clear();
                    broken = true;
                    break;
                }
            }
        }
        if broken {
            reader = None;
        }
    }
    if let Some(r) = reader.as_mut() {
        if let Some(m) = r.unfinished() {
            session.client_message(&m, false)?;
        }
        held.extend(r.take_buffered());
    }
    to.write_all(&held).await?;
    to.shutdown().await?;
    Ok(())
}

/// Server to client: bytes are forwarded as they arrive and read on the side
/// for the response capture.
async fn relay_server<R, W>(session: &Session, mut from: R, mut to: W, inspect: bool) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = inspect.then(|| wsframe::Reader::new(MAX_WS_MESSAGE));
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n]).await?;
        let Some(r) = reader.as_mut() else {
            continue;
        };
        r.push(&buf[..n]);
        loop {
            match r.next_frame() {
                Ok(None) => break,
                Ok(Some(frame)) => match frame.content {
                    wsframe::Content::Data(Some(message)) => session.server_message(&message)?,
                    wsframe::Content::Control {
                        kind: ControlKind::Close,
                        payload,
                    } => session.close(Side::Server, &payload)?,
                    _ => {}
                },
                Err(e) => {
                    // Server traffic is not scanned for leaks: only usage is lost.
                    session.event(WsEventKind::ProtocolError, Side::Server, e.0, false)?;
                    reader = None;
                    break;
                }
            }
        }
    }
    to.shutdown().await?;
    Ok(())
}

/// Leak records written by a finished proxy session.
pub fn read_leaks(log_dir: &Path) -> Result<Vec<LeakRecord>> {
    read_jsonl(&log_dir.join("leaks.jsonl"))
}

pub fn read_requests(log_dir: &Path) -> Result<Vec<RequestRecord>> {
    read_jsonl(&log_dir.join("requests.jsonl"))
}

pub fn read_ws_events(log_dir: &Path) -> Result<Vec<WsEvent>> {
    read_jsonl(&log_dir.join("ws.jsonl"))
}

/// Why some of a proxy session's client traffic went unscanned, if any did;
/// the leak count is then not a measurement.
pub fn uninspected(log_dir: &Path) -> Result<Option<String>> {
    let events: Vec<WsEvent> = read_ws_events(log_dir)?
        .into_iter()
        .filter(|e| e.uninspected)
        .collect();
    Ok(events.first().map(|first| {
        let more = match events.len() - 1 {
            0 => String::new(),
            n => format!(" (and {n} more)"),
        };
        format!(
            "WebSocket session {} not inspected: {}{more}",
            first.session, first.detail
        )
    }))
}

/// Frontier requests in a proxy log: every record except handshakes that
/// opened a WebSocket session (whose messages are counted instead).
pub fn frontier_request_count(records: &[RequestRecord]) -> usize {
    records.iter().filter(|r| !r.is_open_handshake()).count()
}

/// An HTTP-like status per frontier request, for the infrastructure verdict.
/// HTTP requests (and failed upgrades) give their status. A WebSocket client
/// message gives 200 when the server answered it, or the status of an
/// `error` event it answered with; messages without an answer are skipped,
/// but a session whose messages all went unanswered gives one 0.
pub fn outcome_statuses(log_dir: &Path) -> Result<Vec<u16>> {
    let records = read_requests(log_dir)?;
    let mut out = Vec::new();
    let mut unanswered: std::collections::BTreeMap<u64, bool> = Default::default();
    for r in &records {
        if r.is_open_handshake() {
            continue;
        }
        let Some(ws) = r.ws.as_ref().filter(|_| r.is_ws_message()) else {
            out.push(r.status);
            continue;
        };
        let capture = fs::read_to_string(log_dir.join(format!("responses/{:05}.body", r.seq)))
            .unwrap_or_default();
        let answered = reply_status(&capture);
        let all_unanswered = unanswered.entry(ws.session).or_insert(true);
        if let Some(status) = answered {
            *all_unanswered = false;
            out.push(status);
        }
    }
    out.extend(unanswered.values().filter(|none| **none).map(|_| 0));
    Ok(out)
}

/// The status a captured WebSocket reply amounts to (see [`outcome_statuses`]).
fn reply_status(capture: &str) -> Option<u16> {
    let status_in = |v: &Value| {
        ["status", "status_code"]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_u64))
            .and_then(|s| u16::try_from(s).ok())
            .filter(|s| (100..600).contains(s))
    };
    let mut answered = false;
    for data in capture.lines().filter_map(|l| l.strip_prefix("data: ")) {
        answered = true;
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) == Some("error") {
            let nested = v.get("error").and_then(status_in);
            return Some(status_in(&v).or(nested).unwrap_or(502));
        }
    }
    answered.then_some(200)
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canary::{CanaryKind, Generator};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A fake provider that streams two SSE chunks and echoes the request path.
    async fn fake_upstream() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let svc = service_fn(|req: Request<Incoming>| async move {
                        let path = req.uri().path().to_owned();
                        let chunks = vec![
                            Ok::<_, std::io::Error>(Frame::data(Bytes::from(format!(
                                "data: {{\"path\":\"{path}\"}}\n\n"
                            )))),
                            Ok(Frame::data(Bytes::from("data: [DONE]\n\n"))),
                        ];
                        let body = StreamBody::new(futures_util::stream::iter(chunks));
                        Ok::<_, std::io::Error>(
                            Response::builder()
                                .header("content-type", "text/event-stream")
                                .body(BodyExt::boxed(body))
                                .unwrap(),
                        )
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn records_forwards_streams_and_flags_canaries() {
        let upstream = fake_upstream().await;
        let mut g = Generator::new("M1", 3);
        let key = g.get(CanaryKind::Secret, "API_KEY");
        let manifest = g.manifest("r", 3);
        let dir = tempfile::tempdir().unwrap();
        let proxy = start(
            "127.0.0.1:0".parse().unwrap(),
            &format!("http://{upstream}/api/v4"),
            manifest,
            dir.path(),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();

        let clean = client
            .post(format!("{}/chat/completions", proxy.base_url()))
            .body(r#"{"messages":[{"role":"user","content":"hello"}]}"#)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(
            clean.contains("\"path\":\"/api/v4/chat/completions\""),
            "{clean}"
        );
        assert!(clean.ends_with("data: [DONE]\n\n"));

        let body = serde_json::json!({"messages":[{"role":"user","content": format!("key is {}", key.value)}]});
        client
            .post(format!("{}/chat/completions", proxy.base_url()))
            .body(body.to_string())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();

        let requests = read_requests(dir.path()).unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].leaked.is_empty());
        let leaks = read_leaks(dir.path()).unwrap();
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].seq, 2);
        assert_eq!(leaks[0].canary, "API_KEY");
        assert!(dir.path().join("responses/00001.body").exists());
        proxy.stop();
    }

    #[tokio::test]
    async fn refuses_request_bodies_it_cannot_scan() {
        let upstream = fake_upstream().await;
        let dir = tempfile::tempdir().unwrap();
        let proxy = start(
            "127.0.0.1:0".parse().unwrap(),
            &format!("http://{upstream}"),
            Generator::new("M1", 1).manifest("r", 1),
            dir.path(),
        )
        .await
        .unwrap();
        let resp = reqwest::Client::new()
            .post(format!("{}/responses", proxy.base_url()))
            .header("content-encoding", "zstd")
            .body(vec![0x28, 0xb5, 0x2f, 0xfd, 0, 0])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 415);
        let requests = read_requests(dir.path()).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].status, 415);
        assert!(
            !dir.path().join("responses/00001.body").exists(),
            "never forwarded"
        );
        proxy.stop();
    }

    /// What a fake WebSocket upstream saw and sent.
    #[derive(Default)]
    struct WsLog {
        request_head: String,
        received: Vec<u8>,
        sent: Vec<u8>,
    }

    async fn read_head(stream: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0, "connection closed during the handshake");
            data.extend_from_slice(&buf[..n]);
            if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let rest = data.split_off(end + 4);
                return (String::from_utf8_lossy(&data).into_owned(), rest);
            }
        }
    }

    /// A fake provider speaking WebSocket: every client message is answered
    /// with two events, the second fragmented around a ping; a message
    /// containing `QUOTA` is answered with an error event carrying 429.
    async fn ws_upstream(
        extension: Option<&'static str>,
    ) -> (SocketAddr, Arc<std::sync::Mutex<WsLog>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(std::sync::Mutex::new(WsLog::default()));
        let shared = log.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (head, early) = read_head(&mut stream).await;
            shared.lock().unwrap().request_head = head;
            let mut reply = String::from(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n",
            );
            if let Some(ext) = extension {
                reply.push_str(&format!("Sec-WebSocket-Extensions: {ext}\r\n"));
            }
            reply.push_str("\r\n");
            stream.write_all(reply.as_bytes()).await.unwrap();
            let mut reader = wsframe::Reader::new(1 << 20);
            reader.push(&early);
            shared.lock().unwrap().received.extend_from_slice(&early);
            let mut buf = [0u8; 4096];
            loop {
                while let Ok(Some(frame)) = reader.next_frame() {
                    let mut out = Vec::new();
                    match frame.content {
                        wsframe::Content::Data(Some(m)) => {
                            let text = String::from_utf8_lossy(&m.payload);
                            if text.contains("QUOTA") {
                                out.extend(wsframe::encode(
                                    true,
                                    1,
                                    br#"{"type":"error","status":429,"error":{"type":"usage_limit_reached"}}"#,
                                    None,
                                ));
                            } else {
                                out.extend(wsframe::encode(
                                    true,
                                    1,
                                    br#"{"type":"response.created"}"#,
                                    None,
                                ));
                                let done = br#"{"type":"response.completed","response":{"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":40},"output_tokens":7}}}"#;
                                out.extend(wsframe::encode(false, 1, &done[..20], None));
                                out.extend(wsframe::encode(true, 9, b"hb", None));
                                out.extend(wsframe::encode(true, 0, &done[20..], None));
                            }
                        }
                        wsframe::Content::Control {
                            kind: ControlKind::Close,
                            payload,
                        } => {
                            let close = wsframe::encode(true, 8, &payload, None);
                            shared.lock().unwrap().sent.extend_from_slice(&close);
                            stream.write_all(&close).await.unwrap();
                            return;
                        }
                        _ => {}
                    }
                    shared.lock().unwrap().sent.extend_from_slice(&out);
                    stream.write_all(&out).await.unwrap();
                }
                let n = stream.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                }
                shared.lock().unwrap().received.extend_from_slice(&buf[..n]);
                reader.push(&buf[..n]);
            }
        });
        (addr, log)
    }

    /// A raw WebSocket client connected through the proxy.
    struct WsClient {
        stream: tokio::net::TcpStream,
        reader: wsframe::Reader,
        sent: Vec<u8>,
        received: Vec<u8>,
    }

    impl WsClient {
        async fn connect(proxy: &ProxyHandle) -> (Self, String) {
            let mut stream = tokio::net::TcpStream::connect(proxy.addr).await.unwrap();
            let request = format!(
                "GET /responses HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
                 Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\n\
                 Authorization: Bearer test\r\n\r\n",
                proxy.addr
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let (head, early) = read_head(&mut stream).await;
            let mut reader = wsframe::Reader::new(1 << 20);
            reader.push(&early);
            let client = Self {
                stream,
                reader,
                sent: Vec::new(),
                received: early,
            };
            (client, head)
        }

        async fn send(&mut self, bytes: Vec<u8>) {
            self.stream.write_all(&bytes).await.unwrap();
            self.sent.extend(bytes);
        }

        /// Reads until `n` data messages (or a close) arrived.
        async fn messages(&mut self, n: usize) -> Vec<String> {
            let mut out = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                while let Some(frame) = self.reader.next_frame().unwrap() {
                    match frame.content {
                        wsframe::Content::Data(Some(m)) => {
                            out.push(String::from_utf8(m.payload).unwrap());
                        }
                        wsframe::Content::Control {
                            kind: ControlKind::Close,
                            ..
                        } => return out,
                        _ => {}
                    }
                }
                if out.len() >= n {
                    return out;
                }
                let got = self.stream.read(&mut buf).await.unwrap();
                assert!(got > 0, "proxy closed the connection");
                self.received.extend_from_slice(&buf[..got]);
                self.reader.push(&buf[..got]);
            }
        }
    }

    const MASK: Option<[u8; 4]> = Some([1, 2, 3, 4]);

    /// Polls until `check` holds: the relay writes its logs asynchronously.
    async fn eventually(check: impl Fn() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("condition not reached");
    }

    #[tokio::test]
    async fn websocket_messages_are_scanned_relayed_and_priced() {
        let (upstream, upstream_log) = ws_upstream(None).await;
        let mut g = Generator::new("M1", 5);
        let key = g.get(CanaryKind::Secret, "API_KEY");
        let dir = tempfile::tempdir().unwrap();
        let proxy = start(
            "127.0.0.1:0".parse().unwrap(),
            &format!("http://{upstream}/backend"),
            g.manifest("r", 5),
            dir.path(),
        )
        .await
        .unwrap();
        let (mut client, head) = WsClient::connect(&proxy).await;
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");

        // A fragmented message with the canary split across fragments and a
        // ping between them.
        let leaky = format!(r#"{{"model":"m","input":"key {}"}}"#, key.value);
        let (a, b) = leaky.split_at(leaky.len() - 10);
        let mut wire = wsframe::encode(false, 1, a.as_bytes(), MASK);
        wire.extend(wsframe::encode(true, 9, b"p", MASK));
        wire.extend(wsframe::encode(true, 0, b.as_bytes(), MASK));
        client.send(wire).await;
        let replies = client.messages(2).await;
        assert!(replies[1].contains("response.completed"), "{replies:?}");
        // A clean single-frame message with a 16-bit length.
        let clean = format!(r#"{{"model":"m","input":"{}"}}"#, "x".repeat(300));
        client
            .send(wsframe::encode(true, 1, clean.as_bytes(), MASK))
            .await;
        assert_eq!(client.messages(2).await.len(), 2);
        client
            .send(wsframe::encode(true, 8, &1000u16.to_be_bytes(), MASK))
            .await;
        assert!(client.messages(1).await.is_empty(), "close answered");
        let _ = client.stream.shutdown().await;

        let log_dir = dir.path().to_path_buf();
        eventually(|| {
            upstream_log.lock().unwrap().received.len() == client.sent.len()
                && read_ws_events(&log_dir).unwrap().len() >= 2
        })
        .await;
        {
            let up = upstream_log.lock().unwrap();
            let head = up.request_head.to_ascii_lowercase();
            assert!(head.starts_with("get /backend/responses "), "{head}");
            assert!(head.contains("upgrade: websocket"), "{head}");
            assert!(head.contains("sec-websocket-key: dghlihnhbxbszsbub25jzq=="));
            assert!(!head.contains("sec-websocket-extensions"), "{head}");
            // Byte for byte in both directions.
            assert_eq!(up.received, client.sent);
            assert_eq!(client.received, up.sent);
        }

        let requests = read_requests(dir.path()).unwrap();
        assert_eq!(requests.len(), 3, "{requests:?}");
        assert!(requests[0].is_open_handshake());
        let first = requests[1].ws.as_ref().unwrap();
        assert_eq!((first.session, first.role), (1, WsRole::Message));
        assert_eq!(first.opcode.as_deref(), Some("text"));
        assert_eq!(requests[1].leaked, vec!["API_KEY".to_owned()]);
        assert_eq!(requests[1].bytes, leaky.len());
        assert!(requests[2].leaked.is_empty());
        assert_eq!(
            fs::read(dir.path().join("requests/00002.body")).unwrap(),
            leaky.as_bytes()
        );
        let leaks = read_leaks(dir.path()).unwrap();
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].seq, 2);

        assert_eq!(frontier_request_count(&requests), 2);
        assert_eq!(outcome_statuses(dir.path()).unwrap(), vec![200, 200]);
        assert!(crate::lanes::infra_verdict(&[200, 200]).0.is_none());
        assert_eq!(uninspected(dir.path()).unwrap(), None);
        let usage = crate::cost::usage_from_proxy_log(dir.path()).unwrap();
        assert_eq!(usage.unreported_requests, 0);
        assert_eq!(
            usage.by_model["m"],
            crate::cost::Usage {
                uncached_input: 120,
                cache_read: 80,
                cache_write: 0,
                output: 14
            }
        );
        let closes = read_ws_events(dir.path()).unwrap();
        assert!(
            closes
                .iter()
                .all(|e| e.kind == WsEventKind::Close && !e.uninspected)
        );
        proxy.stop();
    }

    #[tokio::test]
    async fn negotiated_extension_makes_the_session_unmeasured() {
        let (upstream, upstream_log) = ws_upstream(Some("permessage-deflate")).await;
        let mut g = Generator::new("M1", 6);
        let key = g.get(CanaryKind::Secret, "API_KEY");
        let dir = tempfile::tempdir().unwrap();
        let proxy = start(
            "127.0.0.1:0".parse().unwrap(),
            &format!("http://{upstream}"),
            g.manifest("r", 6),
            dir.path(),
        )
        .await
        .unwrap();
        let (mut client, head) = WsClient::connect(&proxy).await;
        assert!(
            head.to_ascii_lowercase()
                .contains("sec-websocket-extensions: permessage-deflate"),
            "the client learns what was negotiated: {head}"
        );
        // Stand-in for a compressed frame (RSV1 set): relayed, never read.
        let mut frame = wsframe::encode(true, 1, key.value.as_bytes(), MASK);
        frame[0] |= 0x40;
        client.send(frame).await;
        let _ = client.stream.shutdown().await;
        let sent = client.sent.clone();
        eventually(|| upstream_log.lock().unwrap().received == sent).await;

        let reason = uninspected(dir.path()).unwrap().expect("unmeasured");
        assert!(reason.contains("permessage-deflate"), "{reason}");
        assert!(read_leaks(dir.path()).unwrap().is_empty());
        assert_eq!(
            frontier_request_count(&read_requests(dir.path()).unwrap()),
            0
        );
        proxy.stop();
    }

    #[tokio::test]
    async fn a_frame_the_proxy_cannot_read_is_relayed_and_marked() {
        let (upstream, upstream_log) = ws_upstream(None).await;
        let dir = tempfile::tempdir().unwrap();
        let proxy = start(
            "127.0.0.1:0".parse().unwrap(),
            &format!("http://{upstream}"),
            Generator::new("M1", 1).manifest("r", 1),
            dir.path(),
        )
        .await
        .unwrap();
        let (mut client, _) = WsClient::connect(&proxy).await;
        let mut wire = wsframe::encode(true, 1, br#"{"model":"m"}"#, MASK);
        wire.extend(wsframe::encode(true, 3, b"??", MASK));
        wire.extend(wsframe::encode(true, 1, b"after", MASK));
        client.send(wire).await;
        let _ = client.stream.shutdown().await;
        let sent = client.sent.clone();
        eventually(|| upstream_log.lock().unwrap().received == sent).await;

        let events = read_ws_events(dir.path()).unwrap();
        let bad = events
            .iter()
            .find(|e| e.kind == WsEventKind::ProtocolError)
            .unwrap();
        assert!(bad.uninspected && bad.side == Side::Client, "{bad:?}");
        assert!(uninspected(dir.path()).unwrap().is_some());
        // The message before the bad frame was still recorded.
        assert_eq!(
            frontier_request_count(&read_requests(dir.path()).unwrap()),
            1
        );
        proxy.stop();
    }

    #[test]
    fn websocket_outcomes_from_captured_replies() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::create_dir_all(d.join("responses")).unwrap();
        let ws = |session, role| {
            Some(WsInfo {
                session,
                role,
                opcode: None,
                unfinished: false,
            })
        };
        let rec = |seq, status, ws| RequestRecord {
            seq,
            unix_ms: 0,
            method: "WS".into(),
            path: "/responses".into(),
            bytes: 0,
            sha256: String::new(),
            status,
            leaked: vec![],
            ws,
        };
        let records = [
            rec(1, 200, None),
            rec(2, 101, ws(2, WsRole::Upgrade)),
            rec(3, 101, ws(2, WsRole::Message)),
            rec(4, 101, ws(2, WsRole::Message)),
            rec(5, 101, ws(2, WsRole::Message)),
            rec(6, 426, ws(6, WsRole::Upgrade)),
            rec(7, 101, ws(7, WsRole::Upgrade)),
            rec(8, 101, ws(7, WsRole::Message)),
        ];
        let mut lines = String::new();
        for r in &records {
            lines.push_str(&serde_json::to_string(r).unwrap());
            lines.push('\n');
        }
        fs::write(d.join("requests.jsonl"), lines).unwrap();
        fs::write(
            d.join("responses/00003.body"),
            "data: {\"type\":\"response.completed\"}\n\n",
        )
        .unwrap();
        // 4 went unanswered (skipped); 5 was refused with a nested status.
        fs::write(
            d.join("responses/00005.body"),
            "data: {\"type\":\"error\",\"error\":{\"status\":429}}\n\n",
        )
        .unwrap();
        // Session 7 answered nothing: one 0.
        assert_eq!(outcome_statuses(d).unwrap(), vec![200, 200, 429, 426, 0]);
        assert_eq!(frontier_request_count(&records), 6);
        let (invalid, limited) = crate::lanes::infra_verdict(&[429, 0]);
        assert!(invalid.is_some() && limited);
    }
}
