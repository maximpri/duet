// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only operational reports. Output is an allowlist of fixed labels,
//! counts, timestamps and hashed run identifiers, never stored event strings.

use anyhow::Result;
use duet_boundary::audit::{self, Anchor, AnchorCheck, Line, Verification};
use duet_config::Config;
use rustix::fs::{Dir, Mode as FsMode, OFlags};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INDEX_ENTRIES: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Intact,
    MissingLog,
    MissingAnchor,
    BrokenChain,
    Truncated,
    Rewritten,
    Unanchored,
    IncompleteRecord,
    ChangedDuringCheck,
    Unreadable,
    UnsafePath,
    TooLarge,
    InvalidRunId,
}

#[derive(Debug, Serialize)]
struct RunReport {
    run_ref: String,
    status: Status,
    records: u64,
    bytes: u64,
    unanchored_records: u64,
    first_unix_ms: Option<u64>,
    last_unix_ms: Option<u64>,
}

#[derive(Serialize)]
struct MetadataRecord {
    seq: u64,
    unix_ms: Option<u64>,
    kind: &'static str,
    metrics: BTreeMap<String, Value>,
}

struct Inspection {
    report: RunReport,
    metadata: Vec<MetadataRecord>,
}

struct Snapshot {
    text: String,
    stat: fs::Metadata,
}

fn run_ref(id: &str) -> String {
    hex::encode(Sha256::digest(id.as_bytes()))
}

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn path_error(error: rustix::io::Errno) -> Status {
    match error {
        rustix::io::Errno::NOENT => Status::MissingLog,
        rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR => Status::UnsafePath,
        _ => Status::Unreadable,
    }
}

// Pin every directory component, including the root. A renamed directory stays
// attached to its handle; a symlink replacement cannot redirect later reads.
fn directory(root: &Path, relative: &Path) -> std::result::Result<File, Status> {
    let mut dir: File = rustix::fs::open(root, DIR_FLAGS, FsMode::empty())
        .map_err(path_error)?
        .into();
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err(Status::UnsafePath);
        };
        dir = rustix::fs::openat(&dir, name, DIR_FLAGS, FsMode::empty())
            .map_err(path_error)?
            .into();
    }
    Ok(dir)
}

fn regular(root: &Path, relative: &Path) -> std::result::Result<File, Status> {
    let name = relative.file_name().ok_or(Status::UnsafePath)?;
    let dir = directory(root, relative.parent().ok_or(Status::UnsafePath)?)?;
    let file: File = rustix::fs::openat(
        &dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        FsMode::empty(),
    )
    .map_err(path_error)?
    .into();
    if !file.metadata().map_err(|_| Status::Unreadable)?.is_file() {
        return Err(Status::UnsafePath);
    }
    Ok(file)
}

fn snapshot(root: &Path, relative: &Path, limit: u64) -> std::result::Result<Snapshot, Status> {
    let file = regular(root, relative)?;
    let stat = file.metadata().map_err(|_| Status::Unreadable)?;
    if !stat.is_file() {
        return Err(Status::UnsafePath);
    }
    if stat.len() > limit {
        return Err(Status::TooLarge);
    }
    let mut text = String::new();
    file.take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|_| Status::Unreadable)?;
    if text.len() as u64 > limit {
        return Err(Status::TooLarge);
    }
    Ok(Snapshot { text, stat })
}

fn unchanged(root: &Path, relative: &Path, prior: &fs::Metadata) -> bool {
    regular(root, relative)
        .ok()
        .and_then(|f| f.metadata().ok())
        .is_some_and(|now| {
            now.is_file()
                && now.len() == prior.len()
                && now.ino() == prior.ino()
                && now.dev() == prior.dev()
                && now.modified().ok() == prior.modified().ok()
        })
}

// Only these field names, and only numeric/boolean values, may leave an event.
// In particular, outcome/reason/tool/model/host/path and every digest are absent.
const METRICS: &[&str] = &[
    "changed_files",
    "candidates",
    "high",
    "medium",
    "local_reviewed",
    "local_unavailable",
    "frontier_reviewed",
    "frontier_unavailable",
    "external_candidates",
    "external_unavailable",
    "referenced_files",
    "incomplete",
    "blocked",
    "boundary",
    "exit_code",
    "parts",
    "attempt",
    "checks_passed",
    "approved",
    "exchange",
    "placeholders",
    "bytes",
    "tools",
    "resolved_placeholders",
    "operator_public",
    "shown",
    "withheld",
    "cost_usd",
    "requests",
    "files_written",
    "count",
    "budget",
    "left",
    "records",
    "truncated",
    "bytes_up",
    "bytes_down",
    "items",
    "steps",
    "files",
    "bytes_read",
    "local_seconds",
    "references",
    "confirmed",
    "panicked",
];

fn metadata(line: Line) -> MetadataRecord {
    match line {
        Line::Request(r) => MetadataRecord {
            seq: r.seq,
            unix_ms: r.unix_ms.try_into().ok(),
            kind: "request",
            metrics: BTreeMap::from([
                (
                    "request_bytes".into(),
                    json!(serde_json::to_vec(&r.request).map_or(0, |b| b.len())),
                ),
                ("interventions".into(), json!(r.interventions.len())),
            ]),
        },
        Line::Event(r) => {
            let values = serde_json::to_value(&r.event).unwrap_or(Value::Null);
            let metrics = METRICS
                .iter()
                .filter_map(|key| {
                    let v = values.get(key)?;
                    (v.is_number() || v.is_boolean()).then(|| ((*key).into(), v.clone()))
                })
                .collect();
            MetadataRecord {
                seq: r.seq,
                unix_ms: r.unix_ms.try_into().ok(),
                kind: r.event.kind(),
                metrics,
            }
        }
    }
}

fn inspect(ws: &Path, state: &Path, id: &str, export_records: bool) -> Inspection {
    let mut out = Inspection {
        report: RunReport {
            run_ref: run_ref(id),
            status: Status::Unreadable,
            records: 0,
            bytes: 0,
            unanchored_records: 0,
            first_unix_ms: None,
            last_unix_ms: None,
        },
        metadata: Vec::new(),
    };
    if duet_agent::purge::check_run_id(id).is_err() {
        out.report.status = Status::InvalidRunId;
        return out;
    }
    let path = Path::new(".duet/audit").join(format!("{id}.jsonl"));
    let data = match snapshot(ws, &path, MAX_LOG_BYTES) {
        Ok(data) => data,
        Err(status) => {
            out.report.status = status;
            return out;
        }
    };
    out.report.bytes = data.text.len() as u64;
    match audit::verify_text(&data.text) {
        Verification::Intact { records } => out.report.records = records,
        Verification::Broken { .. } => {
            out.report.status = Status::BrokenChain;
            return out;
        }
    }
    out.report.status =
        match audit::check_anchor_text(&data.text, &audit::run_anchors(state, ws, id)) {
            AnchorCheck::Matches => Status::Intact,
            AnchorCheck::Extends { unanchored } => {
                out.report.unanchored_records = unanchored;
                Status::Unanchored
            }
            AnchorCheck::Mismatch(reason) if reason.starts_with("truncated:") => Status::Truncated,
            AnchorCheck::Mismatch(_) => Status::Rewritten,
            AnchorCheck::Missing => Status::MissingAnchor,
        };
    if !data.text.is_empty() && !data.text.ends_with('\n') {
        out.report.status = Status::IncompleteRecord;
    }
    if !unchanged(ws, &path, &data.stat) {
        out.report.status = Status::ChangedDuringCheck;
    }
    if out.report.status != Status::Intact {
        return out;
    }
    for line in data.text.lines().filter(|line| !line.is_empty()) {
        // verify_text parsed every record already; never skip an unknown record.
        let Some(parsed) = audit::parse_line(line) else {
            out.report.status = Status::BrokenChain;
            out.metadata.clear();
            return out;
        };
        let when = match &parsed {
            Line::Request(r) => r.unix_ms.try_into().ok(),
            Line::Event(r) => r.unix_ms.try_into().ok(),
        };
        if out.report.first_unix_ms.is_none() {
            out.report.first_unix_ms = when;
        }
        out.report.last_unix_ms = when;
        if export_records {
            out.metadata.push(metadata(parsed));
        }
    }
    out
}

#[derive(Default)]
struct Inventory {
    ids: BTreeSet<String>,
    errors: usize,
    visited: usize,
}

impl Inventory {
    fn entries(&mut self, root: &Path, relative: &Path) -> Vec<PathBuf> {
        let fd = match directory(root, relative) {
            Ok(fd) => fd,
            Err(Status::MissingLog) => return Vec::new(),
            Err(_) => {
                self.errors += 1;
                return Vec::new();
            }
        };
        let Ok(dir) = Dir::read_from(&fd) else {
            self.errors += 1;
            return Vec::new();
        };
        let mut paths = Vec::new();
        for entry in dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    self.errors += 1;
                    continue;
                }
            };
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            self.visited += 1;
            if self.visited > MAX_INDEX_ENTRIES {
                self.errors += 1;
                break;
            }
            paths.push(relative.join(std::ffi::OsStr::from_bytes(name)));
        }
        paths
    }

    fn add(&mut self, id: &str) {
        if duet_agent::purge::check_run_id(id).is_ok() {
            self.ids.insert(id.into());
        } else {
            self.errors += 1;
        }
    }

    fn anchor(&mut self, state: &Path, path: &Path, audit_dir: &Path) {
        if path.extension().is_none_or(|ext| ext != "json") {
            return;
        }
        let anchor = snapshot(state, path, 64 * 1024)
            .ok()
            .and_then(|s| serde_json::from_str::<Anchor>(&s.text).ok());
        let Some(anchor) = anchor else {
            self.errors += 1;
            return;
        };
        let log = Path::new(&anchor.log);
        if log.parent() == Some(audit_dir) {
            if log
                .file_name()
                .is_some_and(|name| name == format!("{}.jsonl", anchor.run_id).as_str())
            {
                self.add(&anchor.run_id);
            } else {
                self.errors += 1;
            }
        }
    }
}

fn inventory(ws: &Path, state: &Path) -> Inventory {
    let mut out = Inventory::default();
    for path in out.entries(ws, Path::new(".duet/audit")) {
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                out.add(id);
            } else {
                out.errors += 1;
            }
        }
    }
    for path in out.entries(ws, Path::new(".duet/runs")) {
        if let Some(id) = path.file_name().and_then(|s| s.to_str()) {
            out.add(id);
        } else {
            out.errors += 1;
        }
    }
    // Modern and legacy anchors contain the original log path. This discovers
    // a deleted log even after its raw run directory was legitimately purged.
    let audit_dir = ws
        .canonicalize()
        .unwrap_or_else(|_| ws.to_path_buf())
        .join(".duet/audit");
    for bucket in out.entries(state, Path::new("audit-anchors")) {
        if bucket.file_name().is_some_and(|n| n == "runs") {
            for run in out.entries(state, &bucket) {
                for anchor in out.entries(state, &run) {
                    out.anchor(state, &anchor, &audit_dir);
                }
            }
        } else {
            for anchor in out.entries(state, &bucket) {
                out.anchor(state, &anchor, &audit_dir);
            }
        }
    }
    out
}

pub(crate) fn export(ws: &Path, run_id: &str) -> Result<i32> {
    let inspected = inspect(ws, &duet_config::owner_state_dir(), run_id, true);
    let code = i32::from(inspected.report.status != Status::Intact);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version": 1, "kind": "duet_audit_metadata", "run": inspected.report,
            "records": inspected.metadata,
        }))?
    );
    Ok(code)
}

pub(crate) fn check(ws: &Path, run_id: Option<&str>) -> Result<i32> {
    let state = duet_config::owner_state_dir();
    let inventory = match run_id {
        Some(id) => Inventory {
            ids: BTreeSet::from([id.to_owned()]),
            ..Inventory::default()
        },
        None => inventory(ws, &state),
    };
    let runs: Vec<_> = inventory
        .ids
        .iter()
        .map(|id| inspect(ws, &state, id, false).report)
        .collect();
    let healthy = inventory.errors == 0 && runs.iter().all(|run| run.status == Status::Intact);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version": 1, "kind": "duet_audit_health", "healthy": healthy,
            "inventory_errors": inventory.errors, "runs": runs,
        }))?
    );
    Ok(i32::from(!healthy))
}

pub(crate) fn retention(ws: &Path, cfg: &Config) -> Result<i32> {
    let raw_days = cfg.int("data.retention_days")?;
    let audit_days = cfg.int("data.audit_retention_days")?;
    let raw = duet_agent::purge::try_candidates(
        ws,
        duet_agent::purge::Scope::OlderThan { days: raw_days },
    );
    let mut inventory = inventory(ws, &duet_config::owner_state_dir());
    let cutoff =
        SystemTime::now().checked_sub(Duration::from_secs(audit_days.max(0) as u64 * 86_400));
    let mut audit_expired = Vec::new();
    for id in &inventory.ids {
        let path = Path::new(".duet/audit").join(format!("{id}.jsonl"));
        match regular(ws, &path)
            .ok()
            .and_then(|f| f.metadata().ok())
            .and_then(|m| m.modified().ok())
        {
            Some(t) if cutoff.is_some_and(|cutoff| t < cutoff) => audit_expired.push(run_ref(id)),
            Some(_) => {}
            None => inventory.errors += 1,
        }
    }
    let raw_error = raw.is_err();
    let raw_expired: Vec<_> = raw
        .unwrap_or_default()
        .iter()
        .map(|id| run_ref(id))
        .collect();
    let attention =
        raw_error || inventory.errors > 0 || !raw_expired.is_empty() || !audit_expired.is_empty();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version": 1, "kind": "duet_retention_review", "attention": attention,
            "raw_retention_days": raw_days, "audit_review_days": audit_days,
            "raw_expired_run_refs": raw_expired, "audit_expired_run_refs": audit_expired,
            "raw_scan_error": raw_error, "inventory_errors": inventory.errors,
            "audit_action": "review_and_archive_manually", "deleted": false,
        }))?
    );
    Ok(i32::from(attention))
}

#[cfg(test)]
mod tests {
    use super::*;
    use audit::{AuditEvent, AuditLog};

    struct Fixture {
        _temp: tempfile::TempDir,
        ws: PathBuf,
        state: PathBuf,
        log: PathBuf,
    }

    fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let ws = root.join("workspace");
        let state = root.join("state");
        let log = ws.join(".duet/audit/r1.jsonl");
        let mut writer =
            AuditLog::open_anchored(&log, &audit::run_anchors(&state, &ws, "r1")).unwrap();
        writer
            .event(AuditEvent::RunStart {
                mode: "top_clearance".into(),
                boundary: true,
            })
            .unwrap();
        writer
            .event(AuditEvent::RunEnd {
                terminal: "completed".into(),
            })
            .unwrap();
        Fixture {
            _temp: temp,
            ws,
            state,
            log,
        }
    }

    #[test]
    fn directory_replacement_cannot_redirect_snapshots_or_inventory() {
        let f = fixture();
        let relative = Path::new(".duet/audit/r1.jsonl");
        let old = snapshot(&f.ws, relative, MAX_LOG_BYTES).unwrap();
        let outside = f.state.join("outside");
        fs::rename(f.ws.join(".duet"), &outside).unwrap();
        std::os::unix::fs::symlink(&outside, f.ws.join(".duet")).unwrap();
        assert!(matches!(
            snapshot(&f.ws, relative, MAX_LOG_BYTES),
            Err(Status::UnsafePath)
        ));
        assert!(!unchanged(&f.ws, relative, &old.stat));
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", true).report.status,
            Status::UnsafePath
        );
        assert!(inventory(&f.ws, &f.state).errors > 0);
        // Owner-state anchor enumeration must not follow a substituted bucket.
        fs::remove_file(f.ws.join(".duet")).unwrap();
        fs::rename(outside, f.ws.join(".duet")).unwrap();
        let anchors = f.state.join("audit-anchors");
        fs::rename(&anchors, f.state.join("old-anchors")).unwrap();
        std::os::unix::fs::symlink(f.state.join("old-anchors"), anchors).unwrap();
        assert!(inventory(&f.ws, &f.state).errors > 0);
        assert!(inspect(&f.ws, &f.state, "r1", true).metadata.is_empty());
    }

    #[test]
    fn export_never_copies_request_or_event_strings_or_content_digests() {
        let f = fixture();
        let canary = "fictional-secret-operations-canary";
        let mut writer =
            AuditLog::open_anchored(&f.log, &audit::run_anchors(&f.state, &f.ws, "r1")).unwrap();
        writer
            .append(
                &format!("https://{canary}/path?key={canary}"),
                canary,
                json!({"messages":[{"content":canary}]}),
                vec![canary.into()],
            )
            .unwrap();
        writer
            .event(AuditEvent::SensitiveCommand {
                command: canary.into(),
                exit_code: Some(0),
                derived_files: vec![canary.into()],
            })
            .unwrap();
        writer
            .event(AuditEvent::ConfigChange {
                key: canary.into(),
                file: canary.into(),
                old: canary.into(),
                new: canary.into(),
                weakens: Some(canary.into()),
                confirmed: true,
            })
            .unwrap();
        writer
            .event(AuditEvent::EndpointTrust {
                role: canary.into(),
                host: canary.into(),
                trust: canary.into(),
            })
            .unwrap();
        let checked = inspect(&f.ws, &f.state, "r1", true);
        assert_eq!(checked.report.status, Status::Intact);
        assert_eq!(checked.metadata.len(), 6);
        let output = serde_json::to_string(&checked.metadata).unwrap();
        assert!(!output.contains(canary), "{output}");
        for forbidden in [
            "https:",
            "endpoint",
            "request_sha256",
            "model",
            "derived_files",
            "weakens",
        ] {
            // Event kinds are fixed schema labels; field values never survive.
            if forbidden != "endpoint" {
                assert!(!output.contains(forbidden), "{output}");
            }
        }
        let request = checked
            .metadata
            .iter()
            .find(|r| r.kind == "request")
            .unwrap();
        assert_eq!(request.metrics["interventions"], 1);
        assert!(request.metrics["request_bytes"].as_u64().unwrap() > 0);
        assert_eq!(checked.metadata[4].metrics["confirmed"], true);
    }

    #[test]
    fn health_finds_deleted_logs_from_anchors_after_raw_data_is_gone() {
        let f = fixture();
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::Intact
        );
        fs::remove_file(&f.log).unwrap();
        let index = inventory(&f.ws, &f.state);
        assert_eq!(index.errors, 0);
        assert!(index.ids.contains("r1"));
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::MissingLog
        );
    }

    #[test]
    fn export_refuses_tampered_truncated_and_unanchored_snapshots() {
        let f = fixture();
        let original = fs::read_to_string(&f.log).unwrap();
        fs::write(
            &f.log,
            original.replacen("\"boundary\":true", "\"boundary\":false", 1),
        )
        .unwrap();
        let bad = inspect(&f.ws, &f.state, "r1", true);
        assert_eq!(bad.report.status, Status::BrokenChain);
        assert!(bad.metadata.is_empty());
        fs::write(&f.log, format!("{}\n", original.lines().next().unwrap())).unwrap();
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::Truncated
        );
        fs::write(&f.log, original.trim_end()).unwrap();
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::IncompleteRecord
        );
        fs::write(&f.log, &original).unwrap();
        let mut unanchored = AuditLog::open(&f.log).unwrap();
        unanchored
            .event(AuditEvent::RunEnd {
                terminal: "new-tail".into(),
            })
            .unwrap();
        let checked = inspect(&f.ws, &f.state, "r1", true);
        assert_eq!(checked.report.status, Status::Unanchored);
        assert_eq!(checked.report.unanchored_records, 1);
        assert!(checked.metadata.is_empty());
        assert_eq!(
            inspect(&f.ws, &f.state.join("other"), "r1", false)
                .report
                .status,
            Status::MissingAnchor
        );
    }

    #[test]
    fn a_rechained_rewrite_is_not_a_healthy_export() {
        let f = fixture();
        let rewritten = f.ws.join("other.jsonl");
        let mut writer = AuditLog::open(&rewritten).unwrap();
        writer
            .event(AuditEvent::RunStart {
                mode: "rewritten".into(),
                boundary: false,
            })
            .unwrap();
        writer
            .event(AuditEvent::RunEnd {
                terminal: "completed".into(),
            })
            .unwrap();
        fs::copy(rewritten, &f.log).unwrap();
        assert!(matches!(
            audit::verify(&f.log).unwrap(),
            Verification::Intact { .. }
        ));
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::Rewritten
        );
    }

    #[test]
    fn unreadable_anchor_indexes_symlinks_and_large_logs_fail_closed() {
        let f = fixture();
        let anchors = f.state.join("audit-anchors/runs/r1");
        let anchor = fs::read_dir(&anchors)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(anchor, "not an anchor").unwrap();
        assert!(inventory(&f.ws, &f.state).errors > 0);
        assert_ne!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::Intact
        );
        fs::remove_file(&f.log).unwrap();
        std::os::unix::fs::symlink(f.ws.join("missing"), &f.log).unwrap();
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::UnsafePath
        );
        fs::remove_file(&f.log).unwrap();
        fs::File::create(&f.log)
            .unwrap()
            .set_len(MAX_LOG_BYTES + 1)
            .unwrap();
        assert_eq!(
            inspect(&f.ws, &f.state, "r1", false).report.status,
            Status::TooLarge
        );
        assert_eq!(
            inspect(&f.ws, &f.state, ".", false).report.status,
            Status::InvalidRunId
        );
    }
}
