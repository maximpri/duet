// SPDX-License-Identifier: GPL-3.0-or-later
//! Hash-chained log of every request sent to the frontier, and of the security
//! decisions taken during a run.
//!
//! Each line carries the SHA-256 of the previous line, so any edit, deletion or
//! reordering breaks verification. Requests are stored after the gate's
//! substitutions, and events carry names, paths and counts only: the log never
//! holds raw sensitive values.
//!
//! Tamper evidence: the chain proves the log is internally consistent, not that
//! it is the log that was written, since a rewritten log can be re-chained. An
//! anchored log therefore also writes its head (record count and last hash) to
//! an anchor file in the owner's state directory, outside the workspace, on
//! every append; [`check_anchor`] detects a log rewritten or truncated since.

use duet_fs::FsError;
use duet_fs::host::HostWait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One outbound request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditRecord {
    pub seq: u64,
    pub prev: String,
    pub unix_ms: u128,
    pub endpoint: String,
    pub model: String,
    /// SHA-256 of `request`.
    pub request_sha256: String,
    /// The exact JSON body sent (after substitutions).
    pub request: serde_json::Value,
    /// What the gate replaced or blocked before sending.
    pub interventions: Vec<String>,
}

/// A security decision. Fields are names, paths, counts and outcomes; never
/// content read from the workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditEvent {
    /// The run started; `boundary` is false in passthrough mode.
    RunStart { mode: String, boundary: bool },
    /// The run reached a terminal state.
    RunEnd { terminal: String },
    /// Why the local model endpoint was trusted (`host:port`, never the URL,
    /// which may carry credentials).
    EndpointTrust {
        role: String,
        host: String,
        trust: String,
    },
    /// A sandboxed command was refused access to a hidden path.
    SandboxDenial { command: String, access: String },
    /// A command ran with `sensitive_data`: its output was held locally and the
    /// files it wrote became sensitive.
    SensitiveCommand {
        command: String,
        exit_code: Option<i32>,
        derived_files: Vec<String>,
    },
    /// A request was blocked by an outbound check (the check's name only).
    BlockedSend { check: String },
    /// The local model changed protected source through `edit_protected`.
    ProtectedEdit {
        path: String,
        attempt: u32,
        checks_passed: bool,
    },
    /// The operator was asked to approve an action (`oversight.approve`). Holds
    /// the tool, the risk class and the target path of a write; never the
    /// command text, the content or the specification.
    Approval {
        tool: String,
        risk: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        approved: bool,
        /// `operator`, or `no_terminal` when nobody could be asked (denied).
        decided_by: String,
    },
    /// (Session) An operator message was added to the conversation. The
    /// message itself is in the next request record, sanitized; this holds
    /// only the turn number and how many values in it became placeholders.
    OperatorMessage { exchange: u64, placeholders: usize },
    /// A host-side web request made for the frontier (`web_fetch`,
    /// `web_search`): the host, never the URL's path or the query.
    WebRequest {
        tool: String,
        host: String,
        bytes: u64,
        /// `ok`, `truncated`, or why it failed (`refused`, `timeout`, ...).
        outcome: String,
    },
    /// Text for a third party was refused by the outbound check (a
    /// placeholder or a known sensitive value in it). Never holds the text.
    OutboundRefused {
        /// The tool or client that wanted to send it.
        channel: String,
        destination: String,
        reason: String,
    },
    /// `git_commit` recorded a commit in the user's repository: its hash and
    /// the paths it holds, never the message or content.
    GitCommit { hash: String, paths: Vec<String> },
    /// An owner or project setting changed (`duet config set`).
    ConfigChange {
        key: String,
        file: String,
        old: String,
        new: String,
        /// What the change weakened, when it loosened privacy.
        weakens: Option<String>,
        confirmed: bool,
    },
    /// An MCP server was started (or failed to start) for the run.
    McpServer {
        server: String,
        /// `stdio` or `http`.
        transport: String,
        /// `started` or `failed`.
        outcome: String,
        tools: usize,
    },
    /// A call to an MCP tool: server, tool, the server's trust class and the
    /// outcome (`ok`, `tool_error`, `error`, `timeout`, `server_stopped`,
    /// `refused_outbound`, `interrupted`); never arguments or results.
    McpCall {
        server: String,
        tool: String,
        trust: String,
        outcome: String,
        /// Placeholders in the arguments were resolved (sensitive stdio servers only).
        resolved_placeholders: bool,
    },
    /// A language-server tool call (`op` = `code_nav:<operation>` or
    /// `rename`), or a server's lifecycle (`op` = `server`, `outcome` =
    /// `started`, `restarted`, `crashed`, `refreshed` or `unavailable`).
    /// Holds the file, never its content, the query or the new name.
    LanguageServer {
        language: String,
        op: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// `ok`, `refused`, `unavailable`, `timeout` or `error` for a call.
        outcome: String,
        /// Results shown, and results the boundary dropped or withheld.
        #[serde(default)]
        shown: u32,
        #[serde(default)]
        withheld: u32,
    },
}

impl AuditEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            AuditEvent::RunStart { .. } => "run_start",
            AuditEvent::RunEnd { .. } => "run_end",
            AuditEvent::EndpointTrust { .. } => "endpoint_trust",
            AuditEvent::SandboxDenial { .. } => "sandbox_denial",
            AuditEvent::SensitiveCommand { .. } => "sensitive_command",
            AuditEvent::BlockedSend { .. } => "blocked_send",
            AuditEvent::ProtectedEdit { .. } => "protected_edit",
            AuditEvent::Approval { .. } => "approval",
            AuditEvent::ConfigChange { .. } => "config_change",
            AuditEvent::OperatorMessage { .. } => "operator_message",
            AuditEvent::WebRequest { .. } => "web_request",
            AuditEvent::OutboundRefused { .. } => "outbound_refused",
            AuditEvent::GitCommit { .. } => "git_commit",
            AuditEvent::McpServer { .. } => "mcp_server",
            AuditEvent::McpCall { .. } => "mcp_call",
            AuditEvent::LanguageServer { .. } => "language_server",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    pub seq: u64,
    pub prev: String,
    pub unix_ms: u128,
    pub event: AuditEvent,
}

/// One line of an audit log.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Request(AuditRecord),
    Event(EventRecord),
}

impl Line {
    pub fn seq(&self) -> u64 {
        match self {
            Line::Request(r) => r.seq,
            Line::Event(e) => e.seq,
        }
    }
    fn prev(&self) -> &str {
        match self {
            Line::Request(r) => &r.prev,
            Line::Event(e) => &e.prev,
        }
    }
}

/// Parses one line: an event record has an `event` field, a request does not.
pub fn parse_line(line: &str) -> Option<Line> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("event").is_some() {
        serde_json::from_value(v).ok().map(Line::Event)
    } else {
        serde_json::from_value(v).ok().map(Line::Request)
    }
}

/// Every parseable line of a log, in order.
pub fn read(path: &Path) -> Result<Vec<Line>, FsError> {
    let text = std::fs::read_to_string(path).map_err(|e| FsError::io("read", path, e))?;
    Ok(text
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(parse_line)
        .collect())
}

/// The chain link to a line: the SHA-256 of its exact bytes.
fn digest(line: &str) -> String {
    hex::encode(Sha256::digest(line.as_bytes()))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// The head of a log, written outside the workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Anchor {
    pub run_id: String,
    pub log: String,
    pub records: u64,
    pub head: String,
    pub unix_ms: u128,
}

/// Where the anchors of one run's audit log live under the owner state
/// directory.
///
/// An anchor is filed under the run id and the hash of the log's first record
/// (`audit-anchors/runs/<run-id>/<first>.json`). Both belong to the run itself,
/// so the anchor still applies after the workspace is moved or re-mounted.
/// Anchors written in the earlier layout, under a hash of the workspace path
/// (`audit-anchors/<workspace>/<run-id>.json`), are still read.
#[derive(Debug, Clone)]
pub struct RunAnchors {
    root: PathBuf,
    run_id: String,
    workspace: PathBuf,
}

/// The anchors of `run_id` under `state_dir`; `workspace` is used only to find
/// an anchor written in the earlier layout.
pub fn run_anchors(state_dir: &Path, workspace: &Path, run_id: &str) -> RunAnchors {
    RunAnchors {
        root: state_dir.join("audit-anchors"),
        run_id: run_id.to_owned(),
        workspace: workspace.to_path_buf(),
    }
}

/// Where the earlier layout filed the anchor of `run_id` in `workspace`: under
/// a hash of the workspace path, which changes when the workspace moves.
pub fn legacy_anchor_path(state_dir: &Path, workspace: &Path, run_id: &str) -> PathBuf {
    legacy_dir(&state_dir.join("audit-anchors"), workspace).join(format!("{run_id}.json"))
}

fn legacy_dir(root: &Path, workspace: &Path) -> PathBuf {
    let ws = hex::encode(Sha256::digest(workspace.to_string_lossy().as_bytes()));
    root.join(&ws[..16])
}

/// The anchor a log is compared with.
enum Found {
    /// This log's anchor (file and content).
    Anchor(PathBuf, Anchor),
    /// The run has anchors, but none for this log's first record: the start
    /// of the log was rewritten, or the log was emptied.
    Other(String),
    /// An anchor exists but cannot be read.
    Unreadable(String),
    None,
}

impl RunAnchors {
    fn run_dir(&self) -> PathBuf {
        self.root.join("runs").join(&self.run_id)
    }

    /// The anchor file of the log whose first record hashes to `first`.
    fn path_for(&self, first: &str) -> PathBuf {
        self.run_dir()
            .join(format!("{}.json", &first[..first.len().min(16)]))
    }

    /// Where this run's anchors are searched, for messages.
    pub fn describe(&self) -> String {
        self.run_dir().display().to_string()
    }

    /// The anchor of a log with these line digests: the current layout first,
    /// then the earlier one under this workspace, then the earlier one under
    /// any workspace (a workspace moved since the run).
    fn find(&self, digests: &[String]) -> Found {
        let current: Vec<PathBuf> = std::fs::read_dir(self.run_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        if let Some(first) = digests.first() {
            let own = self.path_for(first);
            if current.contains(&own) {
                return match read_anchor(&own) {
                    Ok(Some(a)) => Found::Anchor(own, a),
                    Ok(None) => Found::None,
                    Err(reason) => Found::Unreadable(reason),
                };
            }
        }
        if !current.is_empty() {
            return Found::Other(if digests.is_empty() {
                "truncated: the log is empty, but the run was anchored".into()
            } else {
                "rewritten: the first entry matches none of the run's anchors".into()
            });
        }
        let name = format!("{}.json", self.run_id);
        let own_legacy = legacy_dir(&self.root, &self.workspace).join(&name);
        let mut legacy = vec![own_legacy.clone()];
        legacy.extend(
            std::fs::read_dir(&self.root)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path().join(&name))
                .filter(|p| *p != own_legacy),
        );
        let mut fallback = None;
        for path in legacy {
            match read_anchor(&path) {
                Ok(Some(a)) => {
                    // Several workspaces may have used this run id: prefer
                    // the anchor this log agrees with.
                    if !matches!(compare(&a, digests), AnchorCheck::Mismatch(_)) {
                        return Found::Anchor(path, a);
                    }
                    fallback.get_or_insert(Found::Anchor(path, a));
                }
                Ok(None) => {}
                Err(reason) => {
                    fallback.get_or_insert(Found::Unreadable(reason));
                }
            }
        }
        fallback.unwrap_or(Found::None)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AnchorCheck {
    /// The log ends exactly at the anchored head.
    Matches,
    /// The anchored head is intact and records follow it that were never
    /// anchored (a crash between an append and its anchor write).
    Extends { unanchored: u64 },
    /// The log was rewritten or truncated after it was anchored.
    Mismatch(String),
    /// No anchor exists for this log.
    Missing,
}

fn compare(anchor: &Anchor, digests: &[String]) -> AnchorCheck {
    let n = digests.len() as u64;
    if anchor.records > n {
        return AnchorCheck::Mismatch(format!(
            "truncated: the anchor records {} entries, the log has {n}",
            anchor.records
        ));
    }
    let head = match anchor.records {
        0 => GENESIS,
        k => digests[k as usize - 1].as_str(),
    };
    if head != anchor.head {
        return AnchorCheck::Mismatch(format!(
            "rewritten: entry {} no longer matches the anchored head",
            anchor.records
        ));
    }
    match n - anchor.records {
        0 => AnchorCheck::Matches,
        unanchored => AnchorCheck::Extends { unanchored },
    }
}

fn read_anchor(path: &Path) -> Result<Option<Anchor>, String> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| format!("anchor {} is unreadable: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("anchor {}: {e}", path.display())),
    }
}

/// Checks the log at `path` against its anchor.
pub fn check_anchor(path: &Path, anchors: &RunAnchors) -> Result<AnchorCheck, FsError> {
    Ok(check_found(path, anchors)?.0)
}

/// The check, and the anchor file it compared with.
fn check_found(
    path: &Path,
    anchors: &RunAnchors,
) -> Result<(AnchorCheck, Option<PathBuf>), FsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(FsError::io("read", path, e)),
    };
    let digests: Vec<String> = text.lines().filter(|l| !l.is_empty()).map(digest).collect();
    Ok(match anchors.find(&digests) {
        Found::Anchor(file, a) => (compare(&a, &digests), Some(file)),
        Found::Other(reason) | Found::Unreadable(reason) => (AnchorCheck::Mismatch(reason), None),
        Found::None => (AnchorCheck::Missing, None),
    })
}

pub struct AuditLog {
    path: PathBuf,
    seq: u64,
    prev: String,
    /// Digest of the first record, under which the anchor is filed.
    first: Option<String>,
    /// The run's anchors, when the head is anchored.
    anchor: Option<RunAnchors>,
    /// Waits out a full disk so a write is retried in place (see `duet_fs::host`).
    wait: Option<Arc<dyn HostWait>>,
}

impl AuditLog {
    /// Opens (or creates) the log, continuing an existing chain.
    pub fn open(path: &Path) -> Result<Self, FsError> {
        if let Some(parent) = path.parent() {
            duet_fs::private::ensure_private_dir(parent)?;
        }
        let lines = duet_fs::private::read_lines_repairing(path)?;
        let first = lines.first().map(|l| digest(l));
        let (seq, prev) = match lines.last() {
            Some(last) => (
                parse_line(last).map_or(lines.len() as u64, |l| l.seq()),
                digest(last),
            ),
            None => (0, GENESIS.to_owned()),
        };
        Ok(Self {
            path: path.to_path_buf(),
            seq,
            prev,
            first,
            anchor: None,
            wait: None,
        })
    }

    /// Opens the log and anchors its head among the run's `anchors` after
    /// every append. Refuses to continue a log that no longer matches an
    /// existing anchor, so a rewritten log is never re-anchored as if it were
    /// genuine.
    pub fn open_anchored(path: &Path, anchors: &RunAnchors) -> Result<Self, FsError> {
        let mut log = Self::open(path)?;
        let digests: Vec<String> = duet_fs::private::read_lines_repairing(path)?
            .iter()
            .map(|l| digest(l))
            .collect();
        let refused = match anchors.find(&digests) {
            Found::Anchor(_, a) => match compare(&a, &digests) {
                AnchorCheck::Mismatch(reason) => Some(reason),
                _ => None,
            },
            Found::Other(reason) => Some(reason),
            Found::Unreadable(_) | Found::None => None,
        };
        if let Some(reason) = refused {
            return Err(FsError::io(
                "continue audit log",
                path,
                format!("it was rewritten or truncated since it was anchored ({reason})"),
            ));
        }
        log.anchor = Some(anchors.clone());
        log.write_anchor()?;
        Ok(log)
    }

    /// Retries writes that fail for lack of a host resource while `wait`
    /// agrees; `None` fails them at once.
    pub fn set_wait(&mut self, wait: Option<Arc<dyn HostWait>>) {
        self.wait = wait;
    }

    /// Records written so far and the hash of the last one.
    pub fn head(&self) -> (u64, &str) {
        (self.seq, &self.prev)
    }

    /// Writes the head to the anchor filed under the run id and the first
    /// record; an empty log has nothing to anchor yet.
    fn write_anchor(&self) -> Result<(), FsError> {
        let (Some(anchors), Some(first)) = (&self.anchor, &self.first) else {
            return Ok(());
        };
        let path = anchors.path_for(first);
        if let Some(parent) = path.parent() {
            duet_fs::private::ensure_private_dir(parent)?;
        }
        let anchor = Anchor {
            run_id: anchors.run_id.clone(),
            log: self.path.display().to_string(),
            records: self.seq,
            head: self.prev.clone(),
            unix_ms: now_ms(),
        };
        duet_fs::private::write_private(
            &path,
            &serde_json::to_vec_pretty(&anchor).unwrap_or_default(),
        )
    }

    /// Appends a line, then anchors the new head. Each step is retried in
    /// place while the disk is full (when a wait is set); the append is all
    /// or nothing, so a retried line is never written twice.
    fn write_line(&mut self, seq: u64, line: String) -> Result<(), FsError> {
        let wait = self.wait.clone();
        duet_fs::host::persist(wait.as_deref(), || {
            duet_fs::private::append_line(&self.path, &line)
        })?;
        self.seq = seq;
        self.prev = digest(&line);
        self.first.get_or_insert_with(|| self.prev.clone());
        duet_fs::host::persist(wait.as_deref(), || self.write_anchor())
    }

    pub fn append(
        &mut self,
        endpoint: &str,
        model: &str,
        request: serde_json::Value,
        interventions: Vec<String>,
    ) -> Result<AuditRecord, FsError> {
        let body = serde_json::to_vec(&request).unwrap_or_default();
        let record = AuditRecord {
            seq: self.seq + 1,
            prev: self.prev.clone(),
            unix_ms: now_ms(),
            endpoint: endpoint.to_owned(),
            model: model.to_owned(),
            request_sha256: hex::encode(Sha256::digest(&body)),
            request,
            interventions,
        };
        self.write_line(
            record.seq,
            serde_json::to_string(&record).unwrap_or_default(),
        )?;
        Ok(record)
    }

    pub fn event(&mut self, event: AuditEvent) -> Result<EventRecord, FsError> {
        let record = EventRecord {
            seq: self.seq + 1,
            prev: self.prev.clone(),
            unix_ms: now_ms(),
            event,
        };
        self.write_line(
            record.seq,
            serde_json::to_string(&record).unwrap_or_default(),
        )?;
        Ok(record)
    }
}

/// Appends an applied configuration change to the config audit log at `log`.
/// The one place a change is recorded: `duet config set` and the settings
/// screens both call it.
pub fn record_config_change(
    log: &Path,
    change: &duet_config::Change,
    target: duet_config::Target,
    confirmed: bool,
) -> Result<EventRecord, FsError> {
    AuditLog::open(log)?.event(AuditEvent::ConfigChange {
        key: change.key.clone(),
        file: target.as_str().into(),
        old: change.old.to_string(),
        new: change.new.to_string(),
        weakens: change.weakens.clone(),
        confirmed,
    })
}

/// A shared handle to one run's log: the gate appends requests, the agent and
/// the composition root append events.
#[derive(Clone)]
pub struct AuditHandle(Arc<Mutex<AuditLog>>);

impl AuditHandle {
    pub fn new(log: AuditLog) -> Self {
        Self(Arc::new(Mutex::new(log)))
    }

    /// The log, also after a panic elsewhere while it was held: the run must
    /// still record its end (a torn line is repaired on the next open, and
    /// `verify` reports a broken chain).
    fn log(&self) -> std::sync::MutexGuard<'_, AuditLog> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn append(
        &self,
        endpoint: &str,
        model: &str,
        request: serde_json::Value,
        interventions: Vec<String>,
    ) -> Result<AuditRecord, FsError> {
        self.log().append(endpoint, model, request, interventions)
    }

    /// See [`AuditLog::set_wait`].
    pub fn set_wait(&self, wait: Option<Arc<dyn HostWait>>) {
        self.log().set_wait(wait);
    }

    /// Appends an event. A failure is reported on stderr and does not stop the
    /// run: events record decisions already enforced elsewhere.
    pub fn record(&self, event: AuditEvent) {
        if let Err(e) = self.log().event(event) {
            eprintln!("warning: audit event not recorded: {e}");
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verification {
    Intact { records: u64 },
    Broken { at_seq: u64, reason: String },
}

/// Recomputes the chain.
pub fn verify(path: &Path) -> Result<Verification, FsError> {
    let text = std::fs::read_to_string(path).map_err(|e| FsError::io("read", path, e))?;
    let mut prev = GENESIS.to_owned();
    let mut expected_seq = 1;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let Some(parsed) = parse_line(line) else {
            return Ok(Verification::Broken {
                at_seq: expected_seq,
                reason: "unparseable record".into(),
            });
        };
        let body_mismatch = match &parsed {
            Line::Request(r) => {
                let body = serde_json::to_vec(&r.request).unwrap_or_default();
                hex::encode(Sha256::digest(&body)) != r.request_sha256
            }
            Line::Event(_) => false,
        };
        let reason = if parsed.seq() != expected_seq {
            Some(format!(
                "expected seq {expected_seq}, found {}",
                parsed.seq()
            ))
        } else if parsed.prev() != prev {
            Some("previous-record hash does not match".into())
        } else if body_mismatch {
            Some("request body does not match its hash".into())
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(Verification::Broken {
                at_seq: expected_seq,
                reason,
            });
        }
        prev = digest(line);
        expected_seq += 1;
    }
    Ok(Verification::Intact {
        records: expected_seq - 1,
    })
}

/// `unix_ms` as `YYYY-MM-DD HH:MM:SS` (UTC).
pub fn format_time(unix_ms: u128) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(unix_ms as i128 * 1_000_000)
        .map(|t| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            )
        })
        .unwrap_or_default()
}

/// One human-readable line for a log line (`duet audit show`, the TUI).
pub fn describe_line(line: &str) -> String {
    match parse_line(line) {
        Some(Line::Request(r)) => format!(
            "#{:<4} {}  request         {} {} ({} bytes){}",
            r.seq,
            format_time(r.unix_ms),
            r.endpoint,
            r.model,
            serde_json::to_vec(&r.request).map_or(0, |b| b.len()),
            if r.interventions.is_empty() {
                String::new()
            } else {
                format!("; {} intervention(s)", r.interventions.len())
            }
        ),
        Some(Line::Event(e)) => {
            let mut fields = serde_json::to_value(&e.event).unwrap_or_default();
            if let Some(m) = fields.as_object_mut() {
                m.remove("kind");
            }
            format!(
                "#{:<4} {}  {:<15} {fields}",
                e.seq,
                format_time(e.unix_ms),
                e.event.kind()
            )
        }
        None => format!("unparseable: {}", line.chars().take(80).collect::<String>()),
    }
}

/// The chain, then the anchor, as report lines and an exit code (0 intact and
/// anchored, 1 broken or rewritten, 2 no anchor to compare with).
pub fn verify_report(path: &Path, anchors: &RunAnchors) -> Result<(i32, Vec<String>), FsError> {
    let mut out = Vec::new();
    match verify(path)? {
        Verification::Intact { records } => out.push(format!("chain intact: {records} records")),
        Verification::Broken { at_seq, reason } => {
            out.push(format!("BROKEN at record {at_seq}: {reason}"));
            return Ok((1, out));
        }
    }
    let (check, file) = check_found(path, anchors)?;
    let file = file.map_or_else(|| anchors.describe(), |f| f.display().to_string());
    let (code, line) = match check {
        AnchorCheck::Matches => (0, format!("anchor matches ({file})")),
        AnchorCheck::Extends { unanchored } => (
            0,
            format!(
                "anchor matches; {unanchored} later record(s) were never anchored (an interrupted append)"
            ),
        ),
        AnchorCheck::Mismatch(reason) => (
            1,
            format!("REWRITTEN OR TRUNCATED since it was anchored: {reason}"),
        ),
        AnchorCheck::Missing => (
            2,
            format!(
                "no anchor at {file}: a rewritten log cannot be detected (run predates anchoring, or another state directory)"
            ),
        ),
    };
    out.push(line);
    Ok((code, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chain_verifies_and_detects_tampering() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit/r1.jsonl");
        let mut log = AuditLog::open(&p).unwrap();
        for i in 0..3 {
            log.append("https://f/v1", "m", json!({"n": i}), vec![])
                .unwrap();
        }
        // Reopening continues the same chain.
        let mut again = AuditLog::open(&p).unwrap();
        again
            .append("https://f/v1", "m", json!({"n": 3}), vec!["x".into()])
            .unwrap();
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 4 });

        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, text.replacen("\"n\":1", "\"n\":9", 1)).unwrap();
        assert!(matches!(
            verify(&p).unwrap(),
            Verification::Broken { at_seq: 2, .. }
        ));

        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&p, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(matches!(
            verify(&p).unwrap(),
            Verification::Broken { at_seq: 2, .. }
        ));
    }

    #[test]
    fn logs_written_before_events_still_verify() {
        // The chain link is the hash of the line as written, which for request
        // records equals the hash of their serialization used before events.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("old.jsonl");
        let mut prev = GENESIS.to_owned();
        let mut text = String::new();
        for i in 1..=2u64 {
            let r = AuditRecord {
                seq: i,
                prev: prev.clone(),
                unix_ms: 1,
                endpoint: "https://f/v1".into(),
                model: "m".into(),
                request_sha256: hex::encode(Sha256::digest(b"{}")),
                request: json!({}),
                interventions: vec![],
            };
            prev = hex::encode(Sha256::digest(serde_json::to_vec(&r).unwrap()));
            text.push_str(&serde_json::to_string(&r).unwrap());
            text.push('\n');
        }
        std::fs::write(&p, text).unwrap();
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 2 });
    }

    #[test]
    fn events_join_the_chain_without_content() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit/r2.jsonl");
        let handle = AuditHandle::new(AuditLog::open(&p).unwrap());
        handle.record(AuditEvent::EndpointTrust {
            role: "local".into(),
            host: "127.0.0.1:8080".into(),
            trust: "loopback".into(),
        });
        handle
            .append("https://f/v1", "m", json!({"n": 1}), vec![])
            .unwrap();
        handle.record(AuditEvent::BlockedSend {
            check: "known-values".into(),
        });
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 3 });
        let lines = read(&p).unwrap();
        assert!(matches!(&lines[0], Line::Event(e) if e.event.kind() == "endpoint_trust"));
        assert!(matches!(&lines[1], Line::Request(r) if r.seq == 2));
        assert!(matches!(&lines[2], Line::Event(e) if e.event.kind() == "blocked_send"));

        // Removing an event breaks the chain like removing a request.
        let text = std::fs::read_to_string(&p).unwrap();
        let without_first: Vec<&str> = text.lines().skip(1).collect();
        std::fs::write(&p, without_first.join("\n") + "\n").unwrap();
        assert!(matches!(
            verify(&p).unwrap(),
            Verification::Broken { at_seq: 1, .. }
        ));
    }

    #[test]
    fn anchor_detects_a_rewritten_or_truncated_log() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("ws/.duet/audit/r3.jsonl");
        let a = run_anchors(&d.path().join("state"), &d.path().join("ws"), "r3");
        let mut log = AuditLog::open_anchored(&p, &a).unwrap();
        for i in 0..3 {
            log.append("https://f/v1", "m", json!({"n": i}), vec![])
                .unwrap();
        }
        log.event(AuditEvent::RunEnd {
            terminal: "completed".into(),
        })
        .unwrap();
        assert_eq!(check_anchor(&p, &a).unwrap(), AnchorCheck::Matches);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(anchor_file(&d.path().join("state"), "r3"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let original = std::fs::read_to_string(&p).unwrap();

        // A fully re-chained rewrite passes verify() but not the anchor.
        std::fs::remove_file(&p).unwrap();
        let mut forged = AuditLog::open(&p).unwrap();
        for i in 0..3 {
            forged
                .append("https://f/v1", "m", json!({"n": i + 10}), vec![])
                .unwrap();
        }
        forged
            .event(AuditEvent::RunEnd {
                terminal: "completed".into(),
            })
            .unwrap();
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 4 });
        assert!(
            matches!(check_anchor(&p, &a).unwrap(), AnchorCheck::Mismatch(r) if r.starts_with("rewritten"))
        );
        // Continuing the forged log is refused rather than re-anchored.
        assert!(AuditLog::open_anchored(&p, &a).is_err());

        // Truncation, even at a record boundary.
        let first_two: Vec<&str> = original.lines().take(2).collect();
        std::fs::write(&p, first_two.join("\n") + "\n").unwrap();
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 2 });
        assert!(
            matches!(check_anchor(&p, &a).unwrap(), AnchorCheck::Mismatch(r) if r.starts_with("truncated"))
        );

        // A record after the anchored head (crash before the anchor write).
        std::fs::write(&p, &original).unwrap();
        let mut plain = AuditLog::open(&p).unwrap();
        plain
            .append("https://f/v1", "m", json!({"n": 4}), vec![])
            .unwrap();
        assert_eq!(
            check_anchor(&p, &a).unwrap(),
            AnchorCheck::Extends { unanchored: 1 }
        );
        assert_eq!(
            check_anchor(
                &p,
                &run_anchors(&d.path().join("other"), &d.path().join("ws"), "r3")
            )
            .unwrap(),
            AnchorCheck::Missing
        );
    }

    /// The single anchor file of `run_id` in the current layout.
    fn anchor_file(state: &Path, run_id: &str) -> PathBuf {
        let dir = state.join("audit-anchors/runs").join(run_id);
        let files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        files.into_iter().next().unwrap()
    }

    fn write_run(log: &Path, anchors: &RunAnchors, n: u64) {
        let mut a = AuditLog::open_anchored(log, anchors).unwrap();
        a.event(AuditEvent::RunStart {
            mode: "hybrid".into(),
            boundary: true,
        })
        .unwrap();
        for i in 0..n {
            a.append("https://f/v1", "m", json!({"n": i}), vec![])
                .unwrap();
        }
    }

    #[test]
    fn anchors_survive_a_moved_workspace() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join("state");
        let (old_ws, new_ws) = (d.path().join("ws"), d.path().join("moved/ws"));
        write_run(
            &old_ws.join(".duet/audit/r4.jsonl"),
            &run_anchors(&state, &old_ws, "r4"),
            2,
        );
        // The anchor is filed by run, not by workspace.
        anchor_file(&state, "r4");
        std::fs::create_dir_all(new_ws.parent().unwrap()).unwrap();
        std::fs::rename(&old_ws, &new_ws).unwrap();

        let log = new_ws.join(".duet/audit/r4.jsonl");
        let anchors = run_anchors(&state, &new_ws, "r4");
        assert_eq!(check_anchor(&log, &anchors).unwrap(), AnchorCheck::Matches);
        let (code, lines) = verify_report(&log, &anchors).unwrap();
        assert_eq!(code, 0, "{lines:?}");

        // Resuming in the new place continues the same anchor.
        AuditLog::open_anchored(&log, &anchors)
            .unwrap()
            .event(AuditEvent::RunEnd {
                terminal: "completed".into(),
            })
            .unwrap();
        assert_eq!(check_anchor(&log, &anchors).unwrap(), AnchorCheck::Matches);
        let original = std::fs::read_to_string(&log).unwrap();

        // A re-chained rewrite is still detected, whether it keeps the first
        // record or not.
        let first = original.lines().next().unwrap().to_owned();
        std::fs::write(&log, first + "\n").unwrap();
        let mut rechained = AuditLog::open(&log).unwrap();
        for i in 0..3 {
            rechained
                .append("https://f/v1", "m", json!({"n": i + 20}), vec![])
                .unwrap();
        }
        assert_eq!(verify(&log).unwrap(), Verification::Intact { records: 4 });
        assert!(matches!(
            check_anchor(&log, &anchors).unwrap(),
            AnchorCheck::Mismatch(r) if r.starts_with("rewritten")
        ));
        std::fs::remove_file(&log).unwrap();
        let mut forged = AuditLog::open(&log).unwrap();
        forged
            .append("https://f/v1", "m", json!({"n": 9}), vec![])
            .unwrap();
        assert_eq!(verify(&log).unwrap(), Verification::Intact { records: 1 });
        assert!(matches!(
            check_anchor(&log, &anchors).unwrap(),
            AnchorCheck::Mismatch(r) if r.starts_with("rewritten")
        ));
        let (code, _) = verify_report(&log, &anchors).unwrap();
        assert_eq!(code, 1);
        assert!(AuditLog::open_anchored(&log, &anchors).is_err());
        std::fs::write(&log, "").unwrap();
        assert!(matches!(
            check_anchor(&log, &anchors).unwrap(),
            AnchorCheck::Mismatch(r) if r.starts_with("truncated")
        ));
    }

    #[test]
    fn anchors_in_the_earlier_layout_are_still_read() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join("state");
        let ws = d.path().join("ws");
        let log = ws.join(".duet/audit/r6.jsonl");
        write_run(&log, &run_anchors(&state, &ws, "r6"), 2);
        // Move the anchor to where the earlier layout filed it.
        let legacy = legacy_anchor_path(&state, &ws, "r6");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::rename(anchor_file(&state, "r6"), &legacy).unwrap();
        std::fs::remove_dir(state.join("audit-anchors/runs/r6")).unwrap();

        let anchors = run_anchors(&state, &ws, "r6");
        assert_eq!(check_anchor(&log, &anchors).unwrap(), AnchorCheck::Matches);
        // Also after the workspace moved, since the run id finds it.
        let moved = d.path().join("elsewhere");
        std::fs::rename(&ws, &moved).unwrap();
        let log = moved.join(".duet/audit/r6.jsonl");
        let anchors = run_anchors(&state, &moved, "r6");
        assert_eq!(check_anchor(&log, &anchors).unwrap(), AnchorCheck::Matches);

        // A rewrite is detected against the earlier-layout anchor.
        let original = std::fs::read_to_string(&log).unwrap();
        let first: Vec<&str> = original.lines().take(1).collect();
        std::fs::write(&log, first.join("\n") + "\n").unwrap();
        assert!(matches!(
            check_anchor(&log, &anchors).unwrap(),
            AnchorCheck::Mismatch(r) if r.starts_with("truncated")
        ));
        std::fs::write(&log, &original).unwrap();

        // Resuming writes the current layout, which then takes precedence.
        AuditLog::open_anchored(&log, &anchors)
            .unwrap()
            .event(AuditEvent::RunEnd {
                terminal: "completed".into(),
            })
            .unwrap();
        anchor_file(&state, "r6");
        assert_eq!(check_anchor(&log, &anchors).unwrap(), AnchorCheck::Matches);
        std::fs::write(&log, &original).unwrap();
        assert!(matches!(
            check_anchor(&log, &anchors).unwrap(),
            AnchorCheck::Mismatch(r) if r.starts_with("truncated")
        ));
    }
}
