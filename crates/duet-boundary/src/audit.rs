// SPDX-License-Identifier: GPL-3.0-or-later
//! Hash-chained log of every request sent to the frontier, and of the security
//! decisions taken during a run.
//!
//! Each line carries the SHA-256 of the previous line, so any edit, deletion or
//! reordering breaks verification. Requests are stored after the gate's
//! substitutions, and events carry names, paths and counts only. Request
//! bodies contain whatever cleared the active mode's boundary: a disclosure
//! bug can therefore leave sensitive values in the audit too. Top-clearance
//! requests to the local model can include raw sensitive content. Protect
//! the log as sensitive storage; integrity is not a privacy guarantee.
//!
//! Tamper evidence: the chain proves the log is internally consistent, not that
//! it is the log that was written, since a rewritten log can be re-chained. An
//! anchored log therefore also writes its head (record count and last hash) to
//! an anchor file in the owner's state directory, outside the workspace, on
//! every append; [`check_anchor`] detects a log rewritten or truncated since.
//!
//! A program that embeds Duet can follow a log as it is written: an
//! [`AuditSubscriber`] attached to it is told of every record appended after
//! (see [`AuditLog::subscribe`]), with its place in the chain, and never of
//! content: a request record reaches it as its endpoint, model, digest and
//! size, without the body.

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
    /// The exact JSON body sent (after substitutions), with each image's
    /// data replaced by its digest (`[image sha256:..., N bytes]`).
    pub request: serde_json::Value,
    /// What the gate replaced or blocked before sending.
    pub interventions: Vec<String>,
}

/// A security decision. Fields are names, paths, counts and outcomes; never
/// content read from the workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditEvent {
    /// Finish-time security review: counts and fixed rule ids, never code,
    /// file paths, hashes of private content, or the local model's prose.
    SecurityReview {
        changed_files: usize,
        candidates: usize,
        high: usize,
        medium: usize,
        local_reviewed: usize,
        local_unavailable: usize,
        #[serde(default)]
        frontier_reviewed: usize,
        #[serde(default)]
        frontier_unavailable: usize,
        #[serde(default)]
        external_candidates: usize,
        #[serde(default)]
        external_unavailable: usize,
        #[serde(default)]
        referenced_files: usize,
        incomplete: bool,
        blocked: bool,
        rule_ids: Vec<String>,
    },
    /// A review could not finish. The caller supplies only a fixed host
    /// reason, never a provider error or source content.
    SecurityReviewAborted { reason: String },
    /// A host-observed plan transition. Callers use fixed action names and
    /// validated revision/step/check IDs; never titles, commands, answers,
    /// notes or output. The digest binds the canonical private revision.
    Plan {
        action: String,
        revision: String,
        digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        check_id: Option<String>,
    },
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
    /// A request an outbound check refused after filtering was sent with
    /// `parts` parts of it withheld (the filters' stricter pass), and passed
    /// every check. What was sent is the next request record.
    SendWithheld { check: String, parts: u32 },
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
    /// An image entered the run (`read_file` on an image, an operator
    /// attachment): where it came from, its size, where it went and the rule
    /// that decided. `destination` is `frontier` (the image itself), `local`
    /// (described by the local model; the frontier got the description) or
    /// `none` (refused). `operator_public` marks the operator's decision to
    /// send it. Never the image or its description.
    Image {
        origin: String,
        bytes: u64,
        /// Digest of the prepared image (as in the request records).
        sha256: String,
        destination: String,
        decision: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        operator_public: bool,
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
    /// A sub-agent started (`delegate`): its id in the run, mode (`read` or
    /// `write`), the SHA-256 of its task (never the text), the path globs a
    /// writing one may write and the model that drives it.
    SubagentStart {
        child: String,
        mode: String,
        task_sha256: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
        model: String,
    },
    /// A sub-agent ended: its terminal state (`completed`, `failed`,
    /// `budget_stopped`), what it cost, its frontier requests and how many
    /// files it wrote.
    SubagentEnd {
        child: String,
        mode: String,
        outcome: String,
        cost_usd: f64,
        requests: u64,
        files_written: usize,
    },
    /// An `ask_local` question asked for characters of a sensitive value by
    /// position or piece (`rule` = `positional_question`: the local model
    /// was asked for the value's format instead), or its answer would have
    /// shown such characters (`characters_withheld`). `withheld` pieces were
    /// replaced; `count` is how many probes the handle has had in the run, so
    /// repeated probing shows as a rising count. Holds the handle id, never
    /// the question, the answer or a value.
    LocalProbe {
        handle: String,
        rule: String,
        withheld: u32,
        count: u32,
    },
    /// A command that read sensitive data (`sensitive_data`) printed a short
    /// output (a few lines): a probe of the data, like a narrow question to
    /// the local model. `count` is the run's probes so far; past `budget`
    /// (`sensitivity.output_probes`) the output is withheld (`shown` false)
    /// and the frontier's view does not depend on it. Holds counts, never
    /// the command or the output.
    OutputProbe {
        count: u32,
        budget: u32,
        shown: bool,
    },
    /// Masked output of a command that read sensitive data showed `shown`
    /// small numbers as written; `left` remain in the run's budget
    /// (`sensitivity.masked_numbers`). Never the numbers.
    MaskedNumbers {
        handle: String,
        shown: u32,
        left: u32,
    },
    /// A synthetic sample (twin) of the content under `handle` was asked
    /// for: `outcome` is `shown` (with `records` records), or `withheld`
    /// when a check found a known value or a copied span in it (nothing was
    /// shown), or `unsupported`. Never the sample.
    SyntheticSample {
        handle: String,
        records: u32,
        outcome: String,
    },
    /// Root, owner, or scoped instructions were given to the frontier.
    /// `origin` is `project` or `owner` for their legacy `DUET.md` files;
    /// other files append their relative name, e.g. `project:src/AGENTS.md`.
    /// Project content is presented through the same boundary as public files.
    /// Holds its size and digest, never its text.
    Instructions {
        origin: String,
        bytes: u64,
        sha256: String,
        truncated: bool,
    },
    /// A sandboxed command's connection through the egress proxy
    /// (`sandbox.network = "registries"`): the host it named (empty when it
    /// named none that could be read), the port, bytes each way and the
    /// outcome (`allowed`, `refused`, `failed`, with the reason). Never a
    /// path, a query or content.
    Egress {
        host: String,
        port: u16,
        bytes_up: u64,
        bytes_down: u64,
        outcome: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
    /// The local model condensed the older part of a conversation into a
    /// working summary (context compaction): `outcome` is `compacted` or
    /// `failed` (the conversation was left as it was). `child` names a
    /// sub-agent's conversation. Counts and sizes only, never the summary
    /// (the request that carries it is recorded like any other).
    Compaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child: Option<String>,
        outcome: String,
        items: usize,
        tokens_before: u64,
        tokens_after: u64,
        local_seconds: f64,
    },
    /// A hook of a program that embeds Duet failed (see
    /// [`AuditSubscriber`]): an audit subscriber when it was attached
    /// (`stage` = `open`) or on the record `seq` (`audit`), which it
    /// therefore missed, or an end hook (`end`). Holds the hook's name, never
    /// its message; the run is not affected.
    HookFailed {
        hook: String,
        stage: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        panicked: bool,
    },
    /// The local explorer answered an `explore` call: the SHA-256 of the
    /// question (never its text), `quick` or `thorough`, its steps (local
    /// model requests), the files and bytes it was shown, its local seconds,
    /// the references its report kept and the outcome (`reported`,
    /// `partial`: a cap ended it before it reported, `failed`). Never what
    /// it read or wrote.
    Explore {
        question_sha256: String,
        depth: String,
        steps: u32,
        files: u32,
        bytes_read: u64,
        local_seconds: f64,
        references: u32,
        outcome: String,
    },
}

impl AuditEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            AuditEvent::SecurityReview { .. } => "security_review",
            AuditEvent::SecurityReviewAborted { .. } => "security_review_aborted",
            AuditEvent::Plan { .. } => "plan",
            AuditEvent::RunStart { .. } => "run_start",
            AuditEvent::RunEnd { .. } => "run_end",
            AuditEvent::EndpointTrust { .. } => "endpoint_trust",
            AuditEvent::SandboxDenial { .. } => "sandbox_denial",
            AuditEvent::SensitiveCommand { .. } => "sensitive_command",
            AuditEvent::BlockedSend { .. } => "blocked_send",
            AuditEvent::SendWithheld { .. } => "send_withheld",
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
            AuditEvent::SubagentStart { .. } => "subagent_start",
            AuditEvent::SubagentEnd { .. } => "subagent_end",
            AuditEvent::Image { .. } => "image",
            AuditEvent::LocalProbe { .. } => "local_probe",
            AuditEvent::Instructions { .. } => "instructions",
            AuditEvent::Egress { .. } => "egress",
            AuditEvent::Compaction { .. } => "compaction",
            AuditEvent::OutputProbe { .. } => "output_probe",
            AuditEvent::MaskedNumbers { .. } => "masked_numbers",
            AuditEvent::SyntheticSample { .. } => "synthetic_sample",
            AuditEvent::Explore { .. } => "explore",
            AuditEvent::HookFailed { .. } => "hook_failed",
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
        let mut budget = 100_000;
        let current: Vec<PathBuf> = match anchor_entries(&self.root, &self.run_dir(), &mut budget) {
            Ok(paths) => paths
                .into_iter()
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect(),
            Err(reason) => return Found::Unreadable(reason),
        };
        if let Some(first) = digests.first() {
            let own = self.path_for(first);
            if current.contains(&own) {
                return match read_anchor(&self.root, &own) {
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
        let buckets = match anchor_entries(&self.root, &self.root, &mut budget) {
            Ok(paths) => paths,
            Err(reason) => return Found::Unreadable(reason),
        };
        legacy.extend(
            buckets
                .into_iter()
                .map(|p| p.join(&name))
                .filter(|p| *p != own_legacy),
        );
        let mut fallback = None;
        for path in legacy {
            match read_anchor(&self.root, &path) {
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

const ANCHOR_DIR_FLAGS: rustix::fs::OFlags = rustix::fs::OFlags::RDONLY
    .union(rustix::fs::OFlags::DIRECTORY)
    .union(rustix::fs::OFlags::NOFOLLOW)
    .union(rustix::fs::OFlags::CLOEXEC);

// Owner state is the trusted root. Descendants are opened component by
// component, so neither enumeration nor a later anchor read follows a link.
fn anchor_directory(root: &Path, path: &Path) -> Result<Option<std::fs::File>, String> {
    let state = root.parent().ok_or("anchor root has no parent")?;
    let relative = path
        .strip_prefix(state)
        .map_err(|_| "anchor outside state root")?;
    let mut dir: std::fs::File =
        match rustix::fs::open(state, ANCHOR_DIR_FLAGS, rustix::fs::Mode::empty()) {
            Ok(fd) => fd.into(),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(format!("cannot open anchor state root: {e}")),
        };
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            return Err("invalid anchor path".into());
        };
        dir = match rustix::fs::openat(&dir, name, ANCHOR_DIR_FLAGS, rustix::fs::Mode::empty()) {
            Ok(fd) => fd.into(),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "cannot open anchor directory without following links: {e}"
                ));
            }
        };
    }
    Ok(Some(dir))
}

fn anchor_entries(root: &Path, path: &Path, budget: &mut usize) -> Result<Vec<PathBuf>, String> {
    use std::os::unix::ffi::OsStrExt;
    let Some(dir) = anchor_directory(root, path)? else {
        return Ok(Vec::new());
    };
    let entries = rustix::fs::Dir::read_from(&dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        *budget = budget
            .checked_sub(1)
            .ok_or("anchor inventory exceeds 100000 entries")?;
        paths.push(path.join(std::ffi::OsStr::from_bytes(name)));
    }
    Ok(paths)
}

fn read_anchor(root: &Path, path: &Path) -> Result<Option<Anchor>, String> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Read;
    const LIMIT: u64 = 64 * 1024;
    let Some(dir) = anchor_directory(root, path.parent().ok_or("anchor has no parent")?)? else {
        return Ok(None);
    };
    let file: std::fs::File = match rustix::fs::openat(
        &dir,
        path.file_name().ok_or("anchor has no name")?,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd.into(),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(e) => return Err(format!("cannot open anchor without following links: {e}")),
    };
    let stat = file.metadata().map_err(|e| e.to_string())?;
    if !stat.is_file() || stat.len() > LIMIT {
        return Err("anchor is not a regular file within 64 KiB".into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("anchor exceeds 64 KiB".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "anchor is malformed".into())
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
    Ok(check_text_found(&text, anchors))
}

/// Checks an immutable log snapshot against the owner's anchors. Consumers
/// can verify and export the same bytes without reopening a changing log.
pub fn check_anchor_text(text: &str, anchors: &RunAnchors) -> AnchorCheck {
    check_text_found(text, anchors).0
}

fn check_text_found(text: &str, anchors: &RunAnchors) -> (AnchorCheck, Option<PathBuf>) {
    let digests: Vec<String> = text.lines().filter(|l| !l.is_empty()).map(digest).collect();
    match anchors.find(&digests) {
        Found::Anchor(file, a) => (compare(&a, &digests), Some(file)),
        Found::Other(reason) | Found::Unreadable(reason) => (AnchorCheck::Mismatch(reason), None),
        Found::None => (AnchorCheck::Missing, None),
    }
}

/// Follows an audit log as it is written: a program that embeds Duet
/// attaches one with [`AuditLog::subscribe`] (or [`AuditHandle::subscribe`])
/// to build its own record of a run.
///
/// Delivery: each record appended after the subscriber was attached, once,
/// in chain order, after its line is in the log (appended and synced) and
/// its anchor written (or the attempt failed). Calls are made on the thread
/// that appends, with the log locked: a subscriber must return quickly (hand
/// slow work such as network export to its own thread) and must not append
/// to the same log (that deadlocks).
///
/// Failure: an error or a panic never reaches the run. It is reported on
/// stderr and recorded as a `hook_failed` event naming the subscriber and the
/// record it missed, which every subscriber is then told of in turn (a
/// failure to deliver that event is not recorded again). A subscriber can
/// always rebuild what it missed from the log itself ([`read`]).
pub trait AuditSubscriber: Send + Sync {
    /// A short name (letters, digits, `.`, `_`, `-`; anything else becomes
    /// `_`), recorded when the subscriber fails.
    fn name(&self) -> &str;

    /// The subscriber was attached to `log`; the records already in it
    /// (a resumed run or session continues its chain) are not delivered.
    fn opened(&self, _log: &Opened) -> Result<(), String> {
        Ok(())
    }

    /// A record was appended.
    fn appended(&self, record: &Appended) -> Result<(), String>;
}

/// Where a log's chain stands when a subscriber is attached.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Opened {
    /// The log file.
    pub log: PathBuf,
    /// The run it belongs to, when it is anchored.
    pub run_id: Option<String>,
    /// Records already in the log.
    pub records: u64,
    /// SHA-256 of its last line ([`GENESIS`] when empty).
    pub head: String,
}

/// A record appended to a log, as a subscriber is told of it: its place in
/// the chain and what it records, never content.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Appended {
    /// Its position in the chain (the first record is 1).
    pub seq: u64,
    /// SHA-256 of the previous line ([`GENESIS`] for the first).
    pub prev: String,
    /// SHA-256 of this line as written: the chain's head after it.
    pub hash: String,
    pub unix_ms: u128,
    pub record: Recorded,
}

/// What an appended record holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "record", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Recorded {
    /// A security event, as recorded.
    Event { event: AuditEvent },
    /// A request sent to the frontier: the endpoint and model, the SHA-256
    /// and size in bytes of the body as recorded, and what the gate replaced
    /// or withheld (counts). Never the body.
    Request {
        endpoint: String,
        model: String,
        request_sha256: String,
        bytes: u64,
        interventions: Vec<String>,
    },
}

/// A log's head: records written, the hash of the last line, and the anchor
/// file outside the workspace that holds it (when the log is anchored and
/// has a record).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ChainHead {
    pub log: PathBuf,
    pub records: u64,
    pub head: String,
    pub anchor: Option<PathBuf>,
}

/// A subscriber's name as an audit event may hold it.
fn hook_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .take(64)
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    if clean.is_empty() {
        "unnamed".into()
    } else {
        clean
    }
}

/// Calls a hook of an embedding program: an error or a panic is reported on
/// stderr and returned (the hook's name as recorded, and whether it
/// panicked); it never propagates.
pub fn call_hook(
    name: &str,
    what: &str,
    f: impl FnOnce() -> Result<(), String>,
) -> Result<(), (String, bool)> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(why)) => {
            eprintln!("warning: {what} {name} failed: {why}");
            Err((hook_name(name), false))
        }
        Err(_) => {
            eprintln!("warning: {what} {name} panicked");
            Err((hook_name(name), true))
        }
    }
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
    /// Told of every record appended (see [`AuditSubscriber`]).
    subscribers: Vec<Arc<dyn AuditSubscriber>>,
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
            subscribers: Vec::new(),
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

    /// The head with the log's path and its anchor file.
    pub fn chain_head(&self) -> ChainHead {
        ChainHead {
            log: self.path.clone(),
            records: self.seq,
            head: self.prev.clone(),
            anchor: match (&self.anchor, &self.first) {
                (Some(a), Some(first)) => Some(a.path_for(first)),
                _ => None,
            },
        }
    }

    /// Attaches a subscriber: it is told where the chain stands now
    /// ([`AuditSubscriber::opened`]), then of every record appended.
    pub fn subscribe(&mut self, subscriber: Arc<dyn AuditSubscriber>) {
        let opened = Opened {
            log: self.path.clone(),
            run_id: self.anchor.as_ref().map(|a| a.run_id.clone()),
            records: self.seq,
            head: self.prev.clone(),
        };
        let outcome = call_hook(subscriber.name(), "audit subscriber", || {
            subscriber.opened(&opened)
        });
        self.subscribers.push(subscriber);
        if let Err((hook, panicked)) = outcome {
            self.hook_failed(hook, "open", None, panicked);
        }
    }

    /// Tells every subscriber of an appended record; each failure is recorded
    /// when `record_failures` (not for the record of a failure itself).
    fn deliver(&mut self, appended: &Appended, record_failures: bool) {
        let failures: Vec<(String, bool)> = self
            .subscribers
            .iter()
            .filter_map(|s| call_hook(s.name(), "audit subscriber", || s.appended(appended)).err())
            .collect();
        if record_failures {
            for (hook, panicked) in failures {
                self.hook_failed(hook, "audit", Some(appended.seq), panicked);
            }
        }
    }

    fn hook_failed(&mut self, hook: String, stage: &str, seq: Option<u64>, panicked: bool) {
        let event = AuditEvent::HookFailed {
            hook,
            stage: stage.into(),
            seq,
            panicked,
        };
        if let Err(e) = self.write_event(event, false) {
            eprintln!("warning: audit event not recorded: {e}");
        }
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
        let written = self.write_line(
            record.seq,
            serde_json::to_string(&record).unwrap_or_default(),
        );
        if self.seq == record.seq && !self.subscribers.is_empty() {
            let appended = Appended {
                seq: record.seq,
                prev: record.prev.clone(),
                hash: self.prev.clone(),
                unix_ms: record.unix_ms,
                record: Recorded::Request {
                    endpoint: record.endpoint.clone(),
                    model: record.model.clone(),
                    request_sha256: record.request_sha256.clone(),
                    bytes: body.len() as u64,
                    interventions: record.interventions.clone(),
                },
            };
            self.deliver(&appended, true);
        }
        written.map(|()| record)
    }

    pub fn event(&mut self, event: AuditEvent) -> Result<EventRecord, FsError> {
        self.write_event(event, true)
    }

    /// Appends an event and tells the subscribers (see [`AuditLog::deliver`]).
    fn write_event(
        &mut self,
        event: AuditEvent,
        record_failures: bool,
    ) -> Result<EventRecord, FsError> {
        let record = EventRecord {
            seq: self.seq + 1,
            prev: self.prev.clone(),
            unix_ms: now_ms(),
            event,
        };
        let written = self.write_line(
            record.seq,
            serde_json::to_string(&record).unwrap_or_default(),
        );
        // The line is in the log once the head moved, even if its anchor
        // could not be written.
        if self.seq == record.seq && !self.subscribers.is_empty() {
            let appended = Appended {
                seq: record.seq,
                prev: record.prev.clone(),
                hash: self.prev.clone(),
                unix_ms: record.unix_ms,
                record: Recorded::Event {
                    event: record.event.clone(),
                },
            };
            self.deliver(&appended, record_failures);
        }
        written.map(|()| record)
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

    /// See [`AuditLog::subscribe`].
    pub fn subscribe(&self, subscriber: Arc<dyn AuditSubscriber>) {
        self.log().subscribe(subscriber);
    }

    /// See [`AuditLog::chain_head`].
    pub fn chain_head(&self) -> ChainHead {
        self.log().chain_head()
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
    Ok(verify_text(&text))
}

/// Recomputes the chain of an immutable snapshot, including request-body
/// digests. Unknown or malformed records fail verification.
pub fn verify_text(text: &str) -> Verification {
    let mut prev = GENESIS.to_owned();
    let mut expected_seq = 1;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let Some(parsed) = parse_line(line) else {
            return Verification::Broken {
                at_seq: expected_seq,
                reason: "unparseable record".into(),
            };
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
            return Verification::Broken {
                at_seq: expected_seq,
                reason,
            };
        }
        prev = digest(line);
        expected_seq += 1;
    }
    Verification::Intact {
        records: expected_seq - 1,
    }
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
    fn plan_events_round_trip_only_revision_metadata_in_the_chain() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit/plan.jsonl");
        let event = AuditEvent::Plan {
            action: "verification_passed".into(),
            revision: "r2".into(),
            digest: "a".repeat(64),
            step_id: Some("s1".into()),
            check_id: Some("c1".into()),
        };
        let expected = json!({"kind":"plan","action":"verification_passed","revision":"r2","digest":"a".repeat(64),"step_id":"s1","check_id":"c1"});
        assert_eq!(serde_json::to_value(&event).unwrap(), expected);
        let mut log = AuditLog::open(&path).unwrap();
        log.event(event.clone()).unwrap();
        assert_eq!(verify(&path).unwrap(), Verification::Intact { records: 1 });
        let rows = read(&path).unwrap();
        assert!(
            matches!(&rows[0], Line::Event(row) if row.event == event && row.event.kind() == "plan")
        );
        let metadata: AuditEvent = serde_json::from_value(expected).unwrap();
        assert_eq!(metadata, event);
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

    #[test]
    fn anchor_checks_refuse_symlink_directories_and_oversized_files() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("ws/.duet/audit/r1.jsonl");
        let state = d.path().join("state");
        let anchors = run_anchors(&state, &d.path().join("ws"), "r1");
        write_run(&log, &anchors, 0);
        let text = std::fs::read_to_string(&log).unwrap();
        let parent = state.join("audit-anchors");
        let moved = state.join("moved");
        std::fs::rename(&parent, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &parent).unwrap();
        assert!(matches!(
            check_anchor_text(&text, &anchors),
            AnchorCheck::Mismatch(_)
        ));
        std::fs::remove_file(&parent).unwrap();
        std::fs::rename(&moved, &parent).unwrap();
        let first = digest(text.lines().next().unwrap());
        std::fs::File::create(anchors.path_for(&first))
            .unwrap()
            .set_len(65537)
            .unwrap();
        assert!(matches!(
            check_anchor_text(&text, &anchors),
            AnchorCheck::Mismatch(_)
        ));
    }

    /// Keeps what it is told.
    #[derive(Default)]
    struct Recorder {
        opened: Mutex<Vec<Opened>>,
        seen: Mutex<Vec<Appended>>,
        /// Fails (`Some(false)`) or panics (`Some(true)`) on every record.
        fails: Option<bool>,
        fails_on_open: bool,
    }

    impl AuditSubscriber for Recorder {
        fn name(&self) -> &str {
            match self.fails {
                None => "recorder",
                Some(false) => "failing recorder!",
                Some(true) => "panicking",
            }
        }
        fn opened(&self, log: &Opened) -> Result<(), String> {
            self.opened.lock().unwrap().push(log.clone());
            if self.fails_on_open {
                return Err("cannot start".into());
            }
            Ok(())
        }
        fn appended(&self, record: &Appended) -> Result<(), String> {
            self.seen.lock().unwrap().push(record.clone());
            match self.fails {
                None => Ok(()),
                Some(false) => Err("export queue full".into()),
                Some(true) => panic!("subscriber defect"),
            }
        }
    }

    #[test]
    fn subscribers_follow_every_record_in_chain_order() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("ws/.duet/audit/r7.jsonl");
        let anchors = run_anchors(&d.path().join("state"), &d.path().join("ws"), "r7");
        // A record from an earlier invocation: not delivered, but where the
        // chain stands is.
        write_run(&p, &anchors, 0);
        let earlier = digest(std::fs::read_to_string(&p).unwrap().lines().next().unwrap());

        let mut log = AuditLog::open_anchored(&p, &anchors).unwrap();
        let (a, b) = (Arc::new(Recorder::default()), Arc::new(Recorder::default()));
        log.subscribe(a.clone());
        log.subscribe(b.clone());
        let handle = AuditHandle::new(log);
        handle
            .append(
                "https://f/v1",
                "m",
                json!({"messages": ["BODY-MARKER-8841"]}),
                vec!["sanitize: replaced sensitive content (0 copied span(s))".into()],
            )
            .unwrap();
        handle.record(AuditEvent::BlockedSend {
            check: "known-values".into(),
        });
        handle.record(AuditEvent::RunEnd {
            terminal: "completed".into(),
        });

        let opened = a.opened.lock().unwrap().clone();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].records, 1);
        assert_eq!(opened[0].head, earlier);
        assert_eq!(opened[0].run_id.as_deref(), Some("r7"));
        assert_eq!(opened[0].log, p);

        let seen = a.seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            *b.seen.lock().unwrap(),
            "every subscriber is told the same"
        );
        let text = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(seen.len(), lines.len() - 1);
        let mut prev = earlier;
        for (n, r) in seen.iter().enumerate() {
            assert_eq!(r.seq, n as u64 + 2);
            assert_eq!(r.prev, prev);
            assert_eq!(
                r.hash,
                digest(lines[n + 1]),
                "the hash of the line as written"
            );
            prev = r.hash.clone();
        }
        let Recorded::Request {
            request_sha256,
            bytes,
            interventions,
            ..
        } = &seen[0].record
        else {
            panic!("{:?}", seen[0]);
        };
        let body = serde_json::to_vec(&json!({"messages": ["BODY-MARKER-8841"]})).unwrap();
        assert_eq!(*request_sha256, hex::encode(Sha256::digest(&body)));
        assert_eq!(*bytes, body.len() as u64);
        assert_eq!(interventions.len(), 1);
        // Never the content of a request.
        let told = serde_json::to_string(&seen).unwrap();
        assert!(!told.contains("BODY-MARKER"), "{told}");
        assert!(text.contains("BODY-MARKER"));
        assert!(matches!(
            &seen[1].record,
            Recorded::Event {
                event: AuditEvent::BlockedSend { .. }
            }
        ));

        let head = handle.chain_head();
        assert_eq!((head.records, head.head.as_str()), (4, prev.as_str()));
        let anchor: Anchor =
            serde_json::from_slice(&std::fs::read(head.anchor.unwrap()).unwrap()).unwrap();
        assert_eq!((anchor.records, anchor.head), (4, prev));
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 4 });
    }

    #[test]
    fn a_failing_subscriber_is_recorded_and_changes_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("audit/r8.jsonl");
        let mut log = AuditLog::open(&p).unwrap();
        let good = Arc::new(Recorder::default());
        let failing = Arc::new(Recorder {
            fails: Some(false),
            fails_on_open: true,
            ..Recorder::default()
        });
        let panicking = Arc::new(Recorder {
            fails: Some(true),
            ..Recorder::default()
        });
        log.subscribe(good.clone());
        log.subscribe(failing.clone());
        log.subscribe(panicking.clone());
        let handle = AuditHandle::new(log);
        handle
            .append("https://f/v1", "m", json!({"n": 1}), vec![])
            .unwrap();
        handle.record(AuditEvent::RunEnd {
            terminal: "completed".into(),
        });

        let events: Vec<AuditEvent> = read(&p)
            .unwrap()
            .into_iter()
            .filter_map(|l| match l {
                Line::Event(e) => Some(e.event),
                Line::Request(_) => None,
            })
            .collect();
        let failed =
            |stage: &str, seq: Option<u64>, hook: &str, panicked: bool| AuditEvent::HookFailed {
                hook: hook.into(),
                stage: stage.into(),
                seq,
                panicked,
            };
        assert_eq!(
            events,
            [
                // Attaching: the failing one's `opened`.
                failed("open", None, "failing_recorder_", false),
                // The request (record 2): both failed on it.
                failed("audit", Some(2), "failing_recorder_", false),
                failed("audit", Some(2), "panicking", true),
                AuditEvent::RunEnd {
                    terminal: "completed".into()
                },
                failed("audit", Some(5), "failing_recorder_", false),
                failed("audit", Some(5), "panicking", true),
            ]
        );
        // Every record reached every subscriber, the failure records too
        // (their own failures are not recorded again).
        let seqs = |r: &Recorder| {
            r.seen
                .lock()
                .unwrap()
                .iter()
                .map(|a| a.seq)
                .collect::<Vec<_>>()
        };
        assert_eq!(seqs(&good), [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(seqs(&failing), seqs(&good));
        // Attached after record 1 was written.
        assert_eq!(seqs(&panicking), [2, 3, 4, 5, 6, 7]);
        assert_eq!(verify(&p).unwrap(), Verification::Intact { records: 7 });
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
