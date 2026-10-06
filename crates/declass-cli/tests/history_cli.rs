// SPDX-License-Identifier: GPL-3.0-or-later
//! Saved conversations through the public `declass history` command. Historical
//! session exits remain readable without changing their transcripts.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_LEFT: &str = "session left open; continue it with `declass --resume`";

fn saved(ws: &Path, id: &str, message: &str, terminal: Value) -> PathBuf {
    let dir = ws.join(".declass/runs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("run.json"),
        serde_json::to_vec(&json!({
            "run_id": id,
            "session": true,
            "objective": "Review saved work"
        }))
        .unwrap(),
    )
    .unwrap();
    let entries = [
        json!({"kind":"turn_start","exchange":1,"message":message}),
        json!({"kind":"item","item":{"type":"user","text":message}}),
        json!({"kind":"turn_end","end":{"state":"replied","message":"Ready."}}),
        json!({"kind":"end","terminal":terminal}),
    ];
    std::fs::write(
        dir.join("transcript.jsonl"),
        entries
            .iter()
            .map(|entry| format!("{entry}\n"))
            .collect::<String>(),
    )
    .unwrap();
    dir
}

fn history(ws: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_declass"))
        .arg("--workspace")
        .arg(ws)
        .arg("history")
        .args(args)
        .output()
        .unwrap()
}

fn record<'a>(listing: &'a Value, id: &str) -> &'a Value {
    listing["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == id)
        .unwrap_or_else(|| panic!("missing {id} from {listing}"))
}

#[test]
fn history_command_shows_old_and_current_open_sessions_without_rewriting_them() {
    let ws = tempfile::tempdir().unwrap();
    let current = "20261001-120000-current";
    let legacy = "20260928-120000-legacy";
    let failed = "20260927-120000-failed";
    let closed = "20260926-120000-closed";
    let dirs = [
        saved(
            ws.path(),
            current,
            "Check the current design",
            json!({"state":"open","reason":SESSION_LEFT}),
        ),
        saved(
            ws.path(),
            legacy,
            "Investigate the backfill anomaly",
            json!({"state":"failed","reason":SESSION_LEFT}),
        ),
        saved(
            ws.path(),
            failed,
            "Inspect the failed save",
            json!({"state":"failed","reason":format!("save failed: {SESSION_LEFT}")}),
        ),
        saved(
            ws.path(),
            closed,
            "Finish the prior review",
            json!({"state":"completed","summary":"Closed by operator"}),
        ),
    ];
    let before: Vec<Vec<u8>> = dirs
        .iter()
        .map(|dir| std::fs::read(dir.join("transcript.jsonl")).unwrap())
        .collect();

    let output = history(ws.path(), &["--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listing: Value = serde_json::from_slice(&output.stdout).unwrap();
    for id in [current, legacy] {
        let shown = record(&listing, id);
        assert_eq!(shown["kind"], "Session");
        assert_eq!(shown["status"], "Open");
        assert_eq!(shown["resume_command"], format!("declass --resume {id}"));
    }
    assert_eq!(record(&listing, failed)["status"], "Failed");
    assert_eq!(record(&listing, closed)["status"], "Closed");
    assert!(record(&listing, closed)["resume_command"].is_null());

    let output = history(ws.path(), &[legacy]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let detail = String::from_utf8(output.stdout).unwrap();
    assert!(detail.contains("Session · Open"), "{detail}");
    assert!(
        detail.contains(&format!("Continue: declass --resume {legacy}")),
        "{detail}"
    );
    assert!(
        detail.contains("Investigate the backfill anomaly"),
        "{detail}"
    );

    let output = history(ws.path(), &["--search", "BACKFILL", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let found: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(found["runs"].as_array().unwrap().len(), 1);
    assert_eq!(found["runs"][0]["id"], legacy);

    for (dir, bytes) in dirs.iter().zip(before) {
        assert_eq!(std::fs::read(dir.join("transcript.jsonl")).unwrap(), bytes);
    }
}
