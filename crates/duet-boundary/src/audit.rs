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
            AuditEvent::ConfigChange { .. } => "config_change",
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

/// Where the anchor of `run_id` in `workspace` lives under the owner state directory.
pub fn anchor_path(state_dir: &Path, workspace: &Path, run_id: &str) -> PathBuf {
    let ws = hex::encode(Sha256::digest(workspace.to_string_lossy().as_bytes()));
    state_dir
        .join("audit-anchors")
        .join(&ws[..16])
        .join(format!("{run_id}.json"))
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
pub fn check_anchor(path: &Path, anchor: &Path) -> Result<AnchorCheck, FsError> {
    let anchor = match read_anchor(anchor) {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(AnchorCheck::Missing),
        Err(reason) => return Ok(AnchorCheck::Mismatch(reason)),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(FsError::io("read", path, e)),
    };
    let digests: Vec<String> = text.lines().filter(|l| !l.is_empty()).map(digest).collect();
    Ok(compare(&anchor, &digests))
}

pub struct AuditLog {
    path: PathBuf,
    seq: u64,
    prev: String,
    /// Anchor file and run id, when the head is anchored.
    anchor: Option<(PathBuf, String)>,
}

impl AuditLog {
    /// Opens (or creates) the log, continuing an existing chain.
    pub fn open(path: &Path) -> Result<Self, FsError> {
        if let Some(parent) = path.parent() {
            duet_fs::private::ensure_private_dir(parent)?;
        }
        let lines = duet_fs::private::read_lines_repairing(path)?;
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
            anchor: None,
        })
    }

    /// Opens the log and anchors its head at `anchor` after every append.
    /// Refuses to continue a log that no longer matches an existing anchor,
    /// so a rewritten log is never re-anchored as if it were genuine.
    pub fn open_anchored(path: &Path, anchor: &Path, run_id: &str) -> Result<Self, FsError> {
        let mut log = Self::open(path)?;
        if let Ok(Some(a)) = read_anchor(anchor) {
            let digests: Vec<String> = duet_fs::private::read_lines_repairing(path)?
                .iter()
                .map(|l| digest(l))
                .collect();
            if let AnchorCheck::Mismatch(reason) = compare(&a, &digests) {
                return Err(FsError::io(
                    "continue audit log",
                    path,
                    format!("it was rewritten or truncated since it was anchored ({reason})"),
                ));
            }
        }
        log.anchor = Some((anchor.to_path_buf(), run_id.to_owned()));
        log.write_anchor()?;
        Ok(log)
    }

    /// Records written so far and the hash of the last one.
    pub fn head(&self) -> (u64, &str) {
        (self.seq, &self.prev)
    }

    fn write_anchor(&self) -> Result<(), FsError> {
        let Some((path, run_id)) = &self.anchor else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            duet_fs::private::ensure_private_dir(parent)?;
        }
        let anchor = Anchor {
            run_id: run_id.clone(),
            log: self.path.display().to_string(),
            records: self.seq,
            head: self.prev.clone(),
            unix_ms: now_ms(),
        };
        duet_fs::private::write_private(
            path,
            &serde_json::to_vec_pretty(&anchor).unwrap_or_default(),
        )
    }

    fn write_line(&mut self, seq: u64, line: String) -> Result<(), FsError> {
        duet_fs::private::append_line(&self.path, &line)?;
        self.seq = seq;
        self.prev = digest(&line);
        self.write_anchor()
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

/// A shared handle to one run's log: the gate appends requests, the agent and
/// the composition root append events.
#[derive(Clone)]
pub struct AuditHandle(Arc<Mutex<AuditLog>>);

impl AuditHandle {
    pub fn new(log: AuditLog) -> Self {
        Self(Arc::new(Mutex::new(log)))
    }

    pub fn append(
        &self,
        endpoint: &str,
        model: &str,
        request: serde_json::Value,
        interventions: Vec<String>,
    ) -> Result<AuditRecord, FsError> {
        self.0
            .lock()
            .expect("audit lock")
            .append(endpoint, model, request, interventions)
    }

    /// Appends an event. A failure is reported on stderr and does not stop the
    /// run: events record decisions already enforced elsewhere.
    pub fn record(&self, event: AuditEvent) {
        if let Err(e) = self.0.lock().expect("audit lock").event(event) {
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
        let a = anchor_path(&d.path().join("state"), &d.path().join("ws"), "r3");
        let mut log = AuditLog::open_anchored(&p, &a, "r3").unwrap();
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
            std::fs::metadata(&a).unwrap().permissions().mode() & 0o777,
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
        assert!(AuditLog::open_anchored(&p, &a, "r3").is_err());

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
            check_anchor(&p, &d.path().join("none.json")).unwrap(),
            AnchorCheck::Missing
        );
    }
}
