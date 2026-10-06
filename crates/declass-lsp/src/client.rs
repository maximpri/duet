// SPDX-License-Identifier: GPL-3.0-or-later
//! One connection to a running server: requests with timeouts, notifications,
//! the few requests a server sends the client, and the diagnostics it
//! publishes.

use crate::framing::{encode, read_message};
use crate::position::Range;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use url::Url;

/// The byte streams of a started server.
pub struct Transport {
    pub read: Box<dyn AsyncRead + Send + Unpin>,
    pub write: Box<dyn AsyncWrite + Send + Unpin>,
    /// Its error output, if any: drained and dropped.
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    /// The sandboxed process, when there is one (its tree is killed when it
    /// is dropped).
    pub process: Option<declass_sandbox::Process>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The server exited or its stream broke.
    Closed,
    Timeout,
    /// The server answered with an error.
    Rpc {
        code: i64,
        message: String,
    },
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Closed => write!(f, "the language server stopped"),
            CallError::Timeout => write!(f, "the language server did not answer in time"),
            CallError::Rpc { code, message } => {
                let m: String = message.chars().take(200).collect();
                write!(f, "the language server refused the request ({code}: {m})")
            }
        }
    }
}

/// `ContentModified` and `ServerCancelled`: the answer was abandoned because
/// the server's view changed; asking again is expected to work.
pub const RETRYABLE: [i64; 2] = [-32801, -32802];

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    /// 1 error, 2 warning, 3 information, 4 hint.
    #[serde(default)]
    pub severity: Option<u8>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub source: Option<String>,
}

/// The latest diagnostics a server published for one file.
#[derive(Debug, Clone, Default)]
pub struct Published {
    /// Increases with every publication on this connection.
    pub seq: u64,
    pub version: Option<i64>,
    pub items: Vec<Diagnostic>,
}

#[derive(Default)]
struct DiagState {
    seq: u64,
    by_path: HashMap<PathBuf, Published>,
    /// Work-done progress the server has begun and not ended (indexing, a
    /// check run): while any is open, more diagnostics may be on the way.
    progress: std::collections::HashSet<String>,
}

/// How long diagnostics must stay unchanged, with no work in progress, to
/// count as settled (a server often publishes quick results first, then the
/// results of a slower check).
pub const SETTLE: Duration = Duration::from_millis(250);

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, CallError>>>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct Connection {
    out: mpsc::UnboundedSender<Vec<u8>>,
    pending: Pending,
    next: AtomicI64,
    alive: Arc<AtomicBool>,
    diags: Arc<Mutex<DiagState>>,
    diag_seq: watch::Receiver<u64>,
    process: Mutex<Option<declass_sandbox::Process>>,
    tasks: Vec<JoinHandle<()>>,
    /// The server's `capabilities` from `initialize`.
    pub capabilities: Value,
}

/// What the client answers to a request from the server.
fn answer(
    method: &str,
    params: &Value,
    root: &Url,
    settings: &Value,
) -> Result<Value, (i64, &'static str)> {
    match method {
        "workspace/configuration" => {
            let items = params.get("items").and_then(Value::as_array);
            Ok(Value::Array(
                items
                    .into_iter()
                    .flatten()
                    .map(|item| {
                        let section = item.get("section").and_then(Value::as_str).unwrap_or("");
                        section
                            .split('.')
                            .filter(|s| !s.is_empty())
                            .try_fold(settings, |v, key| v.get(key))
                            .filter(|_| !section.is_empty())
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            ))
        }
        "workspace/workspaceFolders" => Ok(json!([{"uri": root.as_str(), "name": "workspace"}])),
        // Edits reach the workspace only through declass's own write path.
        "workspace/applyEdit" => Ok(json!({
            "applied": false,
            "failureReason": "the client applies edits itself"
        })),
        "window/showDocument" => Ok(json!({"success": false})),
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability"
        | "window/showMessageRequest"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/inlineValue/refresh"
        | "workspace/codeLens/refresh"
        | "workspace/diagnostic/refresh"
        | "workspace/foldingRange/refresh" => Ok(Value::Null),
        _ => Err((-32601, "method not supported by this client")),
    }
}

impl Connection {
    /// Starts reading and writing `transport` and performs the `initialize`
    /// handshake.
    pub async fn start(
        transport: Transport,
        root: &Path,
        init_options: &Value,
        settings: &Value,
        timeout: Duration,
    ) -> Result<Self, String> {
        let root_uri = Url::from_directory_path(root)
            .map_err(|()| format!("{} is not absolute", root.display()))?;
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let pending: Pending = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let diags: Arc<Mutex<DiagState>> = Arc::default();
        let (seq_tx, seq_rx) = watch::channel(0u64);
        let Transport {
            read,
            mut write,
            stderr,
            process,
        } = transport;
        let mut tasks = Vec::new();
        let writer_alive = alive.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(bytes) = out_rx.recv().await {
                if write.write_all(&bytes).await.is_err() || write.flush().await.is_err() {
                    writer_alive.store(false, Ordering::SeqCst);
                    break;
                }
            }
        }));
        // Server diagnostics output is drained and dropped: it can quote the
        // code the server read, and nothing here shows it to anyone.
        if let Some(mut err) = stderr {
            tasks.push(tokio::spawn(async move {
                let mut sink = [0u8; 4096];
                while matches!(err.read(&mut sink).await, Ok(n) if n > 0) {}
            }));
        }
        {
            let (pending, alive, diags) = (pending.clone(), alive.clone(), diags.clone());
            let out = out_tx.clone();
            let root_uri = root_uri.clone();
            let settings = settings.clone();
            tasks.push(tokio::spawn(async move {
                let mut reader = BufReader::new(read);
                while let Ok(Some(msg)) = read_message(&mut reader).await {
                    let method = msg.get("method").and_then(Value::as_str);
                    let id = msg.get("id").cloned();
                    match (method, id) {
                        (None, Some(id)) => {
                            let Some(id) = id.as_i64() else { continue };
                            let Some(tx) = lock(&pending).remove(&id) else {
                                continue;
                            };
                            let reply = match msg.get("error") {
                                Some(e) => Err(CallError::Rpc {
                                    code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                                    message: e
                                        .get("message")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_owned(),
                                }),
                                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(reply);
                        }
                        (Some(method), Some(id)) => {
                            let params = msg.get("params").cloned().unwrap_or(Value::Null);
                            let reply = match answer(method, &params, &root_uri, &settings) {
                                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                                Err((code, message)) => json!({
                                    "jsonrpc": "2.0", "id": id,
                                    "error": {"code": code, "message": message}
                                }),
                            };
                            let _ = out.send(encode(&reply));
                        }
                        (Some("textDocument/publishDiagnostics"), None) => {
                            let Some(params) = msg.get("params") else {
                                continue;
                            };
                            let Some(path) = params
                                .get("uri")
                                .and_then(Value::as_str)
                                .and_then(|u| Url::parse(u).ok())
                                .and_then(|u| u.to_file_path().ok())
                            else {
                                continue;
                            };
                            let items: Vec<Diagnostic> = params
                                .get("diagnostics")
                                .and_then(|d| serde_json::from_value(d.clone()).ok())
                                .unwrap_or_default();
                            let seq = {
                                let mut st = lock(&diags);
                                st.seq += 1;
                                let seq = st.seq;
                                st.by_path.insert(
                                    path,
                                    Published {
                                        seq,
                                        version: params.get("version").and_then(Value::as_i64),
                                        items,
                                    },
                                );
                                seq
                            };
                            let _ = seq_tx.send(seq);
                        }
                        (Some("$/progress"), None) => {
                            let token = msg.pointer("/params/token").map(Value::to_string);
                            let kind = msg.pointer("/params/value/kind").and_then(Value::as_str);
                            if let Some(token) = token {
                                let mut st = lock(&diags);
                                match kind {
                                    Some("begin") => {
                                        st.progress.insert(token);
                                    }
                                    Some("end") => {
                                        st.progress.remove(&token);
                                    }
                                    _ => continue,
                                }
                                let seq = st.seq;
                                drop(st);
                                let _ = seq_tx.send(seq);
                            }
                        }
                        // Log and other notifications carry nothing declass uses.
                        _ => {}
                    }
                }
                alive.store(false, Ordering::SeqCst);
                for (_, tx) in lock(&pending).drain() {
                    let _ = tx.send(Err(CallError::Closed));
                }
                // Wakes anyone waiting for diagnostics.
                let _ = seq_tx.send(u64::MAX);
            }));
        }
        let conn = Self {
            out: out_tx,
            pending,
            next: AtomicI64::new(1),
            alive,
            diags,
            diag_seq: seq_rx,
            process: Mutex::new(process),
            tasks,
            capabilities: Value::Null,
        };
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "declass", "version": env!("CARGO_PKG_VERSION")},
            "locale": "en",
            "rootPath": root.display().to_string(),
            "rootUri": root_uri.as_str(),
            "workspaceFolders": [{"uri": root_uri.as_str(), "name": "workspace"}],
            "initializationOptions": init_options,
            "capabilities": {
                "general": {"positionEncodings": ["utf-16"]},
                "workspace": {
                    "applyEdit": false,
                    "configuration": true,
                    "workspaceFolders": true,
                    "symbol": {"dynamicRegistration": false},
                    "workspaceEdit": {"documentChanges": true, "resourceOperations": []}
                },
                "textDocument": {
                    "synchronization": {"dynamicRegistration": false, "didSave": true,
                        "willSave": false, "willSaveWaitUntil": false},
                    "hover": {"contentFormat": ["plaintext", "markdown"]},
                    "definition": {"linkSupport": true},
                    "references": {},
                    "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                    "rename": {"prepareSupport": false},
                    "publishDiagnostics": {"versionSupport": true, "relatedInformation": false}
                },
                "window": {"workDoneProgress": true}
            },
            "trace": "off"
        });
        let init = conn.request("initialize", params, timeout).await;
        let result = match init {
            Ok(r) => r,
            Err(e) => {
                conn.kill();
                return Err(format!("initialize: {e}"));
            }
        };
        let mut conn = conn;
        conn.capabilities = result.get("capabilities").cloned().unwrap_or(Value::Null);
        // A server that insists on another position encoding cannot be used:
        // every column would be wrong.
        if let Some(enc) = conn
            .capabilities
            .get("positionEncoding")
            .and_then(Value::as_str)
            && enc != "utf-16"
        {
            conn.kill();
            return Err(format!("the server uses the {enc} position encoding"));
        }
        conn.notify("initialized", json!({}));
        Ok(conn)
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn notify(&self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if self.out.send(encode(&msg)).is_err() {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    /// Sends a request and waits up to `timeout` for its answer; a request
    /// that times out is cancelled.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        if !self.alive() {
            return Err(CallError::Closed);
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.out.send(encode(&msg)).is_err() {
            lock(&self.pending).remove(&id);
            return Err(CallError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(CallError::Closed),
            Err(_) => {
                lock(&self.pending).remove(&id);
                self.notify("$/cancelRequest", json!({"id": id}));
                Err(CallError::Timeout)
            }
        }
    }

    /// The sequence number of the latest diagnostics publication.
    pub fn diagnostics_seq(&self) -> u64 {
        lock(&self.diags).seq
    }

    /// Whether the server has work in progress.
    pub fn busy(&self) -> bool {
        !lock(&self.diags).progress.is_empty()
    }

    /// The latest diagnostics of `path` published after `since`, waiting up to
    /// `wait`: once some have arrived, until they have settled (nothing new
    /// for [`SETTLE`] and no work in progress). `None` if none arrived in time.
    pub async fn diagnostics_after(
        &self,
        path: &Path,
        since: u64,
        wait: Duration,
    ) -> Option<Published> {
        let mut rx = self.diag_seq.clone();
        let deadline = tokio::time::Instant::now() + wait;
        let mut changed = tokio::time::Instant::now();
        loop {
            let found = lock(&self.diags)
                .by_path
                .get(path)
                .filter(|p| p.seq > since)
                .cloned();
            if !self.alive() {
                return found;
            }
            let until = match &found {
                Some(_) if !self.busy() => deadline.min(changed + SETTLE),
                _ => deadline,
            };
            match tokio::time::timeout_at(until, rx.changed()).await {
                Ok(Ok(())) => changed = tokio::time::Instant::now(),
                Ok(Err(_)) => return found,
                Err(_) if found.is_some() || tokio::time::Instant::now() >= deadline => {
                    return found;
                }
                Err(_) => {}
            }
        }
    }

    /// The latest diagnostics published for `path`, if any.
    pub fn diagnostics(&self, path: &Path) -> Option<Published> {
        lock(&self.diags).by_path.get(path).cloned()
    }

    /// Asks the server to stop (`shutdown`, then `exit`), then kills what is left.
    pub async fn shutdown(&self) {
        if self.alive() {
            let _ = self
                .request("shutdown", Value::Null, Duration::from_secs(2))
                .await;
            self.notify("exit", Value::Null);
            let process = lock(&self.process).take();
            if let Some(mut p) = process {
                // Waits for it to exit, then kills what is left of its tree.
                p.stop(Duration::from_secs(1)).await;
            }
        }
        self.kill();
    }

    /// Kills the server's process tree at once.
    pub fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Dropping the process kills its tree.
        drop(lock(&self.process).take());
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_requests_are_answered_by_section() {
        let root = Url::parse("file:///ws/").unwrap();
        let settings = json!({"rust-analyzer": {"cargo": {"targetDir": true}}});
        let got = answer(
            "workspace/configuration",
            &json!({"items": [{"section": "rust-analyzer"}, {"section": "rust-analyzer.cargo.targetDir"}, {"section": "other"}, {}]}),
            &root,
            &settings,
        )
        .unwrap();
        assert_eq!(
            got,
            json!([{"cargo": {"targetDir": true}}, true, null, null])
        );
        assert_eq!(
            answer("workspace/applyEdit", &Value::Null, &root, &settings).unwrap()["applied"],
            false
        );
        assert!(answer("x/unknown", &Value::Null, &root, &settings).is_err());
    }
}
