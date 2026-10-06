// SPDX-License-Identifier: GPL-3.0-or-later
//! Operational CLI reports are offline, content-free, and non-destructive.

use declass_boundary::audit::{AuditEvent, AuditLog, run_anchors};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

struct Fixture {
    _temp: tempfile::TempDir,
    ws: PathBuf,
    owner: PathBuf,
    log: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let ws = root.join("workspace");
    let owner = root.join("owner");
    std::fs::create_dir_all(ws.join(".declass/runs/r1")).unwrap();
    std::fs::create_dir_all(&owner).unwrap();
    let log = ws.join(".declass/audit/r1.jsonl");
    let mut writer =
        AuditLog::open_anchored(&log, &run_anchors(&owner.join("state"), &ws, "r1")).unwrap();
    writer
        .event(AuditEvent::RunStart {
            mode: "top_clearance".into(),
            boundary: true,
        })
        .unwrap();
    writer
        .append(
            "https://secret-host/private?token=OPERATIONS_CANARY",
            "OPERATIONS_CANARY",
            json!({"messages":[{"content":"OPERATIONS_CANARY"}]}),
            vec![],
        )
        .unwrap();
    Fixture {
        _temp: temp,
        ws,
        owner,
        log,
    }
}

fn run(f: &Fixture, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_declass"))
        .args(args)
        .arg("--workspace")
        .arg(&f.ws)
        .env("DECLASS_CONFIG_HOME", &f.owner)
        .output()
        .unwrap()
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn export_is_metadata_only_and_health_alerts_on_deleted_anchored_logs() {
    let f = fixture();
    let exported = run(&f, &["audit", "export", "r1"]);
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    assert_eq!(report(&exported)["records"].as_array().unwrap().len(), 2);
    let text = String::from_utf8_lossy(&exported.stdout);
    for forbidden in [
        "OPERATIONS_CANARY",
        "secret-host",
        "messages",
        "request_sha256",
        "https://",
    ] {
        assert!(!text.contains(forbidden), "{text}");
    }
    let healthy = run(&f, &["audit", "check"]);
    assert!(healthy.status.success());
    assert_eq!(report(&healthy)["healthy"], true);
    std::fs::remove_dir_all(f.ws.join(".declass/runs/r1")).unwrap();
    std::fs::remove_file(&f.log).unwrap();
    let missing = run(&f, &["audit", "check"]);
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(report(&missing)["runs"][0]["status"], "missing_log");
    assert_eq!(report(&missing)["healthy"], false);
}

#[test]
fn retention_is_read_only_and_purge_respects_active_runs_and_forensic_state() {
    let f = fixture();
    std::fs::write(
        f.owner.join("config.toml"),
        "[data]\nretention_days = 0\naudit_retention_days = 1\n",
    )
    .unwrap();
    std::fs::write(
        f.ws.join(".declass/runs/r1/private.txt"),
        "OPERATIONS_CANARY",
    )
    .unwrap();
    for marker in ["derived.json", "derived.pending"] {
        std::fs::write(
            f.ws.join(".declass").join(marker),
            "classification persists",
        )
        .unwrap();
    }
    let old = SystemTime::now() - Duration::from_secs(2 * 86_400);
    std::fs::File::open(&f.log)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    let original = std::fs::read(&f.log).unwrap();
    let retention = run(&f, &["audit", "retention"]);
    assert_eq!(retention.status.code(), Some(1));
    let r = report(&retention);
    assert_eq!(r["raw_expired_run_refs"].as_array().unwrap().len(), 1);
    assert_eq!(r["audit_expired_run_refs"].as_array().unwrap().len(), 1);
    assert_eq!(r["deleted"], false);
    let preview = run(&f, &["purge", "r1", "--dry-run"]);
    assert!(preview.status.success());
    assert!(f.ws.join(".declass/runs/r1/private.txt").exists());
    let lock = declass_fs::lock::WorkspaceLock::acquire(&f.ws).unwrap();
    assert!(!run(&f, &["purge", "r1"]).status.success());
    assert!(f.ws.join(".declass/runs/r1/private.txt").exists());
    drop(lock);
    assert!(!run(&f, &["purge", "."]).status.success());
    assert!(!run(&f, &["purge", "r1", "--all"]).status.success());
    assert!(run(&f, &["purge", "r1"]).status.success());
    assert!(!f.ws.join(".declass/runs/r1").exists());
    assert_eq!(std::fs::read(&f.log).unwrap(), original);
    for marker in ["derived.json", "derived.pending"] {
        assert!(f.ws.join(".declass").join(marker).exists());
    }
    assert!(run(&f, &["audit", "check"]).status.success());
}
