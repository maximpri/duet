// SPDX-License-Identifier: GPL-3.0-or-later
//! The real CLI must preview policy without contacting endpoints or starting tools.
use serde_json::Value;
use std::io::Write;
use std::net::TcpListener;
use std::process::{Command, Stdio};

#[test]
fn preview_and_slash_command_are_offline_and_create_no_run() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("project");
    let owner = tmp.path().join("owner");
    std::fs::create_dir(&ws).unwrap();
    std::fs::create_dir(&owner).unwrap();
    let trap = TcpListener::bind("127.0.0.1:0").unwrap();
    trap.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", trap.local_addr().unwrap());
    std::fs::write(owner.join("config.toml"), format!(
        "[local]\nbase_url = {url:?}\n[frontier]\nbase_url = {url:?}\n[extensions]\nplugins_enabled = false\n[mcp.servers.trap]\nurl = {url:?}\nenabled = true\n"
    )).unwrap();
    std::fs::write(ws.join(".env"), "PRIVATE_CONTENT_NEVER_IN_PREVIEW").unwrap();
    std::fs::write(ws.join(".gitignore"), ".env\n").unwrap();
    let cmd = || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_declass"));
        c.arg("--workspace")
            .arg(&ws)
            .env("DECLASS_CONFIG_HOME", &owner)
            .env("NO_COLOR", "1");
        c
    };
    let output = cmd().args(["privacy", "--json"]).output().unwrap();
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["complete"], true);
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == ".env" && f["disposition"] == "sensitive")
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_CONTENT"));
    let mut child = cmd()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/privacy\n/quit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(".env"));
    assert_eq!(
        trap.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!ws.join(".declass").exists());
}

#[test]
fn pending_derived_classification_is_not_reported_as_public() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("project");
    let owner = tmp.path().join("owner");
    std::fs::create_dir_all(ws.join(".declass")).unwrap();
    std::fs::create_dir(&owner).unwrap();
    std::fs::write(ws.join(".declass/derived.pending"), "pending").unwrap();
    std::fs::write(ws.join("export.txt"), "PRIVATE_DERIVED_CONTENT").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_declass"))
        .arg("--workspace")
        .arg(&ws)
        .args(["privacy", "--json"])
        .env("DECLASS_CONFIG_HOME", &owner)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["complete"], false);
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "export.txt" && f["disposition"] == "unknown")
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_DERIVED_CONTENT"));
    assert!(!ws.join(".declass/runs").exists());
}
