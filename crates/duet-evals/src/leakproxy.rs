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

use crate::canary::Manifest;
use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt};
use http_body_util::{BodyExt, StreamBody, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpListener;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeakRecord {
    pub seq: u64,
    pub canary: String,
    pub kind: crate::canary::CanaryKind,
}

struct State {
    upstream: String,
    manifest: Manifest,
    log_dir: PathBuf,
    seq: AtomicU64,
    client: reqwest::Client,
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

async fn forward(state: &State, req: Request<Incoming>) -> Result<Response<ProxyBody>> {
    let seq = state.seq.fetch_add(1, Ordering::SeqCst) + 1;
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
        log_request(
            state,
            seq,
            &parts.method,
            path,
            &body,
            status.as_u16(),
            Vec::new(),
        )?;
        let mut resp = Response::new(full(Bytes::from(format!(
            "duet-eval proxy: request body with content-encoding {encoding} cannot be scanned; \
             disable request compression in the agent"
        ))));
        *resp.status_mut() = status;
        return Ok(resp);
    }

    // Scan and store before anything leaves the machine.
    let text = String::from_utf8_lossy(&body);
    let leaked: Vec<_> = state.manifest.find_in(&text);
    let leaked_names: Vec<String> = leaked.iter().map(|c| c.name.clone()).collect();
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

    let url = format!("{}{}", state.upstream, path);
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())?;
    let mut upstream = state.client.request(method, &url).body(body.clone());
    for (name, value) in &parts.headers {
        let n = name.as_str();
        if matches!(
            n,
            "host" | "content-length" | "connection" | "accept-encoding"
        ) {
            continue;
        }
        upstream = upstream.header(n, value.as_bytes());
    }
    let resp = upstream
        .send()
        .await
        .with_context(|| format!("forwarding to {url}"))?;
    let status = resp.status().as_u16();

    log_request(state, seq, &parts.method, path, &body, status, leaked_names)?;

    let mut builder = Response::builder().status(status);
    for (name, value) in resp.headers() {
        let n = name.as_str();
        if matches!(n, "content-length" | "connection" | "transfer-encoding") {
            continue;
        }
        builder = builder.header(n, value.as_bytes());
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

fn log_request(
    state: &State,
    seq: u64,
    method: &hyper::Method,
    path: String,
    body: &[u8],
    status: u16,
    leaked: Vec<String>,
) -> Result<()> {
    let record = RequestRecord {
        seq,
        unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
        method: method.to_string(),
        path,
        bytes: body.len(),
        sha256: hex::encode(Sha256::digest(body)),
        status,
        leaked,
    };
    writeln!(
        append(&state.log_dir.join("requests.jsonl"))?,
        "{}",
        serde_json::to_string(&record)?
    )?;
    Ok(())
}

fn append(path: &Path) -> Result<fs::File> {
    Ok(OpenOptions::new().create(true).append(true).open(path)?)
}

/// Leak records written by a finished proxy session.
pub fn read_leaks(log_dir: &Path) -> Result<Vec<LeakRecord>> {
    read_jsonl(&log_dir.join("leaks.jsonl"))
}

pub fn read_requests(log_dir: &Path) -> Result<Vec<RequestRecord>> {
    read_jsonl(&log_dir.join("requests.jsonl"))
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
}
