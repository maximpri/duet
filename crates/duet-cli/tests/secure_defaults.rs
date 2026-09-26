// SPDX-License-Identifier: GPL-3.0-or-later
//! Secure defaults through the `duet` binary: loosening needs confirmation and
//! is audited, passthrough needs an explicit acknowledgement, a remote local
//! model over plain HTTP is refused, and `audit verify` detects a rewritten log.
//! Nothing here contacts a model server: every refusal happens before one.

use duet_boundary::audit::{AuditEvent, AuditLog, Line, Verification, read, run_anchors, verify};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    ws: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, ws) = (root.join("owner"), root.join("ws"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    Env {
        _dir: dir,
        home,
        ws,
    }
}

fn duet(e: &Env, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_duet"))
        .args(args)
        .arg("--workspace")
        .arg(&e.ws)
        .env("DUET_CONFIG_HOME", &e.home)
        .env_remove("ZAI_API_KEY")
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn config_audit(e: &Env) -> PathBuf {
    e.home.join("state/config-audit.jsonl")
}

#[test]
fn loosening_a_setting_needs_confirm_and_is_audited() {
    let e = env();
    let refused = duet(&e, &["config", "set", "sandbox.network", "\"all\""]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    let out = text(&refused);
    for shown in [
        "sandbox.network",
        "- \"registries\"",
        "+ \"all\"",
        "weakens",
        "--confirm",
    ] {
        assert!(out.contains(shown), "{shown}: {out}");
    }
    assert!(
        !e.home.join("config.toml").exists(),
        "applied without --confirm"
    );
    assert!(!config_audit(&e).exists());

    // Tightening needs no confirmation (and is recorded too).
    let tighten = duet(&e, &["config", "set", "limits.frontier_usd", "1.0"]);
    assert!(tighten.status.success(), "{}", text(&tighten));

    // The boolean of earlier versions still works, as "all", with a note.
    let applied = duet(
        &e,
        &["config", "set", "sandbox.network", "true", "--confirm"],
    );
    assert!(applied.status.success(), "{}", text(&applied));
    assert!(text(&applied).contains("deprecated"), "{}", text(&applied));
    let owner = std::fs::read_to_string(e.home.join("config.toml")).unwrap();
    assert!(owner.contains("network = \"all\""), "{owner}");

    let log = config_audit(&e);
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(verify(&log).unwrap(), Verification::Intact { records: 2 });
    let lines = read(&log).unwrap();
    let Line::Event(last) = &lines[1] else {
        panic!("expected an event")
    };
    match &last.event {
        AuditEvent::ConfigChange {
            key,
            file,
            old,
            new,
            weakens,
            confirmed,
        } => {
            assert_eq!((key.as_str(), file.as_str()), ("sandbox.network", "owner"));
            assert_eq!((old.as_str(), new.as_str()), ("\"registries\"", "\"all\""));
            assert!(weakens.is_some() && *confirmed);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn passthrough_needs_an_explicit_acknowledgement() {
    let e = env();
    let o = duet(&e, &["run", "--mode", "passthrough", "task"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("--no-privacy"), "{}", text(&o));
    assert!(!e.ws.join(".duet").exists(), "nothing may start");
    let o = duet(&e, &["run", "--mode", "hybrid", "--no-privacy", "task"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("only applies to --mode passthrough"));
}

#[test]
fn a_remote_local_model_over_plain_http_is_refused() {
    let e = env();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&e.ws)
        .status()
        .unwrap();
    assert!(git.success());
    // TEST-NET-1 (never routed); the run must stop before contacting it.
    std::fs::write(
        e.home.join("config.toml"),
        "[local]\nbase_url = \"http://192.0.2.1:9/v1\"\nallowlist = [\"192.0.2.1:9\"]\n",
    )
    .unwrap();
    let o = duet(&e, &["run", "--mode", "local-only", "task"]);
    assert!(!o.status.success());
    let out = text(&o);
    for needed in [
        "plain HTTP",
        "TLS",
        "SSH tunnel",
        "local.allow_plaintext = true",
    ] {
        assert!(out.contains(needed), "{needed}: {out}");
    }
    // A project config cannot opt in.
    std::fs::create_dir_all(e.ws.join(".duet")).unwrap();
    std::fs::write(
        e.ws.join(".duet/config.toml"),
        "[local]\nallow_plaintext = true\n",
    )
    .unwrap();
    let o = duet(&e, &["config", "get", "local.allow_plaintext"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("owner's config"), "{}", text(&o));
}

fn write_run_log(e: &Env, run_id: &str) -> PathBuf {
    let log = e.ws.join(".duet/audit").join(format!("{run_id}.jsonl"));
    let anchor = run_anchors(&e.home.join("state"), &e.ws, run_id);
    let mut a = AuditLog::open_anchored(&log, &anchor).unwrap();
    a.event(AuditEvent::RunStart {
        mode: "hybrid".into(),
        boundary: true,
    })
    .unwrap();
    a.append("https://f/v1", "m", serde_json::json!({"n": 1}), vec![])
        .unwrap();
    a.event(AuditEvent::RunEnd {
        terminal: "completed".into(),
    })
    .unwrap();
    log
}

fn rechain(log: &Path) {
    // A consistent forgery: a fresh chain with different content.
    std::fs::remove_file(log).unwrap();
    let mut forged = AuditLog::open(log).unwrap();
    forged
        .append("https://f/v1", "m", serde_json::json!({"n": 2}), vec![])
        .unwrap();
}

#[test]
fn audit_verify_checks_the_anchor_and_show_lists_events() {
    let e = env();
    let log = write_run_log(&e, "r1");
    let ok = duet(&e, &["audit", "verify", "r1"]);
    assert!(ok.status.success(), "{}", text(&ok));
    assert!(text(&ok).contains("anchor matches"));

    let show = duet(&e, &["audit", "show", "r1"]);
    let listed = text(&show);
    for kind in ["run_start", "request", "run_end"] {
        assert!(listed.contains(kind), "{kind}: {listed}");
    }

    rechain(&log);
    let bad = duet(&e, &["audit", "verify", "r1"]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(
        text(&bad).contains("REWRITTEN OR TRUNCATED"),
        "{}",
        text(&bad)
    );

    assert!(!duet(&e, &["audit", "verify", "../r1"]).status.success());
}
