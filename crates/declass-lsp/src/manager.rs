// SPDX-License-Identifier: GPL-3.0-or-later
//! The servers of one run: started lazily, one per language, kept across tool
//! calls, restarted once after a crash and then reported unavailable.
//!
//! A server reads the workspace itself (sandboxed, with the paths the caller
//! hides denied); documents the run reads or edits are also sent to it
//! (`didOpen`, `didChange`, `didSave`) so it answers against their current
//! text. The caller decides which documents may be sent: this module never
//! reads a file on its own.

use crate::client::{CallError, Connection, Published, RETRYABLE, Transport};
use crate::servers::{Detected, Settings, language_id};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Starts at most this many server processes per language in a run (the
/// first, and one restart after a crash).
pub const MAX_STARTS: u32 = 2;

/// A server being started.
pub type Launching<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Transport, String>> + Send + 'a>>;

/// Starts a server's process.
pub trait Launcher: Send + Sync {
    /// Starts `server` with `deny_read` unreadable to it.
    fn launch<'a>(&'a self, server: &'a Detected, deny_read: &'a [PathBuf]) -> Launching<'a>;
}

/// Starts servers in the OS sandbox: no network, a cleared environment (the
/// sandbox allowlist plus the server's named variables), writes only in the
/// workspace and a scratch directory.
pub struct SandboxLauncher {
    pub kind: declass_sandbox::SandboxKind,
    pub workspace: PathBuf,
    pub scratch: PathBuf,
}

impl Launcher for SandboxLauncher {
    fn launch<'a>(&'a self, server: &'a Detected, deny_read: &'a [PathBuf]) -> Launching<'a> {
        Box::pin(async move {
            let spec = declass_sandbox::Spec {
                workspace: self.workspace.clone(),
                scratch: self.scratch.clone(),
                network: declass_sandbox::Network::Off,
                timeout: Duration::MAX,
                output_cap: 0,
                spill_file: None,
                extra_env: server
                    .spec
                    .env
                    .iter()
                    .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
                    .collect(),
                deny_read: deny_read.to_vec(),
                read_only: false,
            };
            let mut argv = vec![server.program.display().to_string()];
            argv.extend(server.args.iter().cloned());
            let mut process = declass_sandbox::spawn(self.kind, &spec, &argv, &self.workspace)
                .await
                .map_err(|e| e.to_string())?;
            let (write, read, stderr) = process
                .take_streams()
                .ok_or("the server has no standard streams")?;
            Ok(Transport {
                read: Box::new(read),
                write,
                stderr: Some(Box::new(stderr)),
                process: Some(process),
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspError {
    /// No configured server handles this file type.
    NoServer(String),
    /// The server could not be started or crashed after its restart.
    Unavailable {
        language: String,
        reason: String,
    },
    Call(CallError),
}

impl std::fmt::Display for LspError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LspError::NoServer(what) => write!(f, "no language server handles {what}"),
            LspError::Unavailable { language, reason } => write!(
                f,
                "the {language} language server is unavailable for the rest of this run ({reason}); \
use read_file and search instead"
            ),
            LspError::Call(e) => write!(f, "{e}"),
        }
    }
}

/// A lifecycle event, for the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub language: String,
    /// `started`, `restarted`, `crashed`, `unavailable` or `refreshed` (restarted
    /// because the hidden paths changed).
    pub what: String,
}

struct Doc {
    version: i64,
    text: String,
}

#[derive(Default)]
struct Slot {
    conn: Option<Arc<Connection>>,
    starts: u32,
    deny: Vec<PathBuf>,
    docs: HashMap<PathBuf, Doc>,
    unavailable: Option<String>,
}

/// The hidden paths as they are now (called when a server starts or before
/// each use, so a server started before a path became hidden is restarted).
pub type Hidden<'a> = &'a (dyn Fn() -> Vec<PathBuf> + Send + Sync);

pub struct Lsp {
    root: PathBuf,
    servers: Vec<Detected>,
    launcher: Box<dyn Launcher>,
    pub settings: Settings,
    slots: tokio::sync::Mutex<HashMap<String, Slot>>,
    events: std::sync::Mutex<Vec<Event>>,
}

fn ext_of(path: &Path) -> Option<&str> {
    path.extension().and_then(|e| e.to_str())
}

impl Lsp {
    /// `root` is the canonical workspace; `servers` those found installed.
    pub fn new(
        root: PathBuf,
        servers: Vec<Detected>,
        launcher: Box<dyn Launcher>,
        settings: Settings,
    ) -> Self {
        Self {
            root,
            servers,
            launcher,
            settings,
            slots: Default::default(),
            events: Default::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn servers(&self) -> &[Detected] {
        &self.servers
    }

    /// The server for a workspace-relative file, by extension.
    pub fn server_for(&self, rel: &Path) -> Option<&Detected> {
        let ext = ext_of(rel)?;
        self.servers
            .iter()
            .find(|s| s.spec.extensions.iter().any(|e| e == ext))
    }

    /// Lifecycle events since the last call.
    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(
            &mut *self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn event(&self, language: &str, what: &str) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Event {
                language: language.into(),
                what: what.into(),
            });
    }

    /// Whether the server of `language` is running.
    pub async fn active(&self, language: &str) -> bool {
        self.slots
            .lock()
            .await
            .get(language)
            .and_then(|s| s.conn.as_ref())
            .is_some_and(|c| c.alive())
    }

    /// The running connection of `slot`, starting or restarting it as needed.
    async fn ensure(
        &self,
        slot: &mut Slot,
        server: &Detected,
        hidden: Hidden<'_>,
    ) -> Result<Arc<Connection>, LspError> {
        let language = &server.spec.language;
        let unavailable = |reason: &str| LspError::Unavailable {
            language: language.clone(),
            reason: reason.to_owned(),
        };
        if let Some(reason) = &slot.unavailable {
            return Err(unavailable(reason));
        }
        let mut deny = hidden();
        deny.sort();
        deny.dedup();
        if let Some(conn) = &slot.conn {
            if conn.alive() && deny == slot.deny {
                return Ok(conn.clone());
            }
            if conn.alive() {
                // A path became hidden while the server could read it: start
                // over with the new set (not counted as a crash).
                conn.shutdown().await;
                slot.starts = slot.starts.saturating_sub(1);
                self.event(language, "refreshed");
            } else {
                self.event(language, "crashed");
            }
            slot.conn = None;
            slot.docs.clear();
        }
        let mut last = String::new();
        while slot.starts < MAX_STARTS {
            slot.starts += 1;
            let started = match self.launcher.launch(server, &deny).await {
                Ok(t) => {
                    Connection::start(
                        t,
                        &self.root,
                        &server.spec.init_options,
                        &server.spec.settings,
                        self.settings.request_timeout,
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            match started {
                Ok(conn) => {
                    let conn = Arc::new(conn);
                    slot.conn = Some(conn.clone());
                    slot.deny = deny;
                    self.event(
                        language,
                        if slot.starts == 1 {
                            "started"
                        } else {
                            "restarted"
                        },
                    );
                    return Ok(conn);
                }
                Err(e) => last = e,
            }
        }
        let reason = if last.is_empty() {
            "it stopped twice".to_owned()
        } else {
            format!(
                "it stopped twice; last start: {}",
                last.chars().take(160).collect::<String>()
            )
        };
        slot.unavailable = Some(reason.clone());
        self.event(language, "unavailable");
        Err(unavailable(&reason))
    }

    /// Sends `text` as the current content of `abs` (open, or change).
    /// Returns whether anything was sent.
    fn sync(conn: &Connection, slot: &mut Slot, server: &Detected, abs: &Path, text: &str) -> bool {
        let Ok(uri) = Url::from_file_path(abs) else {
            return false;
        };
        match slot.docs.get_mut(abs) {
            Some(doc) if doc.text == text => false,
            Some(doc) => {
                doc.version += 1;
                doc.text = text.to_owned();
                conn.notify(
                    "textDocument/didChange",
                    json!({"textDocument": {"uri": uri.as_str(), "version": doc.version},
                           "contentChanges": [{"text": text}]}),
                );
                true
            }
            None => {
                let lang = language_id(&server.spec.language, ext_of(abs).unwrap_or(""));
                conn.notify(
                    "textDocument/didOpen",
                    json!({"textDocument": {"uri": uri.as_str(), "languageId": lang, "version": 1, "text": text}}),
                );
                slot.docs.insert(
                    abs.to_path_buf(),
                    Doc {
                        version: 1,
                        text: text.to_owned(),
                    },
                );
                true
            }
        }
    }

    fn did_save(conn: &Connection, abs: &Path, text: &str) {
        let Ok(uri) = Url::from_file_path(abs) else {
            return;
        };
        let sync = conn.capabilities.get("textDocumentSync");
        let include = sync
            .and_then(|s| s.get("save"))
            .and_then(|s| s.get("includeText"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut params = json!({"textDocument": {"uri": uri.as_str()}});
        if include {
            params["text"] = Value::String(text.to_owned());
        }
        conn.notify("textDocument/didSave", params);
    }

    /// Sends a request about the workspace file `rel` whose current content is
    /// `text` (which the caller has cleared to be sent). `params` builds the
    /// request from the document's URI. A server that stopped is restarted
    /// once; an abandoned answer (`ContentModified`) is asked for once more.
    pub async fn request(
        &self,
        rel: &Path,
        text: &str,
        hidden: Hidden<'_>,
        method: &str,
        params: &(dyn Fn(&str) -> Value + Send + Sync),
    ) -> Result<Value, LspError> {
        let server = self.server_for(rel).ok_or_else(|| {
            LspError::NoServer(format!(
                "{} files",
                ext_of(rel).map_or("these".into(), |e| format!(".{e}"))
            ))
        })?;
        let abs = self.root.join(rel);
        let uri = Url::from_file_path(&abs)
            .map_err(|()| LspError::NoServer(rel.display().to_string()))?;
        let mut slots = self.slots.lock().await;
        let slot = slots.entry(server.spec.language.clone()).or_default();
        let mut retried = false;
        loop {
            let conn = self.ensure(slot, server, hidden).await?;
            Self::sync(&conn, slot, server, &abs, text);
            match conn
                .request(method, params(uri.as_str()), self.settings.request_timeout)
                .await
            {
                Ok(v) => return Ok(v),
                Err(CallError::Closed) => {
                    // `ensure` restarts it, or reports it unavailable.
                    continue;
                }
                Err(CallError::Rpc { code, .. }) if RETRYABLE.contains(&code) && !retried => {
                    retried = true;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Err(e) => return Err(LspError::Call(e)),
            }
        }
    }

    /// A request that is not about one file (`workspace/symbol`), to the
    /// server of `language`.
    pub async fn workspace_request(
        &self,
        language: &str,
        hidden: Hidden<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, LspError> {
        let server = self
            .servers
            .iter()
            .find(|s| s.spec.language == language)
            .ok_or_else(|| LspError::NoServer(language.to_owned()))?;
        let mut slots = self.slots.lock().await;
        let slot = slots.entry(language.to_owned()).or_default();
        loop {
            let conn = self.ensure(slot, server, hidden).await?;
            match conn
                .request(method, params.clone(), self.settings.request_timeout)
                .await
            {
                Ok(v) => return Ok(v),
                Err(CallError::Closed) => continue,
                Err(e) => return Err(LspError::Call(e)),
            }
        }
    }

    /// The diagnostics of `rel` after sending `text` as its current, saved
    /// content (`didChange`/`didOpen`, then `didSave`), waiting up to `wait`
    /// for the server to publish them. Only when `start` is set is a server
    /// started for this; otherwise `None` unless one is running. Also `None`
    /// when nothing was published in time.
    pub async fn diagnostics(
        &self,
        rel: &Path,
        text: &str,
        hidden: Hidden<'_>,
        start: bool,
        wait: Duration,
    ) -> Result<Option<Published>, LspError> {
        let server = self
            .server_for(rel)
            .ok_or_else(|| LspError::NoServer(rel.display().to_string()))?;
        let abs = self.root.join(rel);
        let conn = {
            let mut slots = self.slots.lock().await;
            let slot = slots.entry(server.spec.language.clone()).or_default();
            let running = slot.conn.as_ref().is_some_and(|c| c.alive());
            if !running && !start {
                return Ok(None);
            }
            let conn = self.ensure(slot, server, hidden).await?;
            let since = conn.diagnostics_seq();
            let changed = Self::sync(&conn, slot, server, &abs, text);
            let known = conn.diagnostics(&abs);
            if changed {
                Self::did_save(&conn, &abs, text);
            } else if known.is_some() && !conn.busy() {
                return Ok(known);
            }
            (conn, since, if changed { None } else { known })
        };
        let (conn, since, known) = conn;
        Ok(conn.diagnostics_after(&abs, since, wait).await.or(known))
    }

    /// Stops every server (`shutdown` and `exit`, then kills what is left).
    pub async fn shutdown(&self) {
        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            if let Some(conn) = slot.conn.take() {
                conn.shutdown().await;
            }
        }
    }
}
