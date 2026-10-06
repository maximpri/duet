// SPDX-License-Identifier: GPL-3.0-or-later
//! The CLI scans existing code, backgrounds work, and confines external tools.
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn command(ws: &Path, owner: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_declass"))
        .arg("--workspace")
        .arg(ws)
        .arg("scan")
        .args(args)
        .env("DECLASS_CONFIG_HOME", owner)
        .output()
        .unwrap()
}

#[test]
fn fail_on_high_survives_a_full_advisory_report() {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    let owner = d.path().join("owner");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&owner).unwrap();
    std::fs::write(owner.join("config.toml"), "[local]\nenabled=false\n").unwrap();
    std::fs::write(
        ws.join("a.py"),
        format!("import random\n{}", "random.random()\n".repeat(64)),
    )
    .unwrap();
    std::fs::write(
        ws.join("z.py"),
        "import requests\nrequests.get(url, verify=False)\n",
    )
    .unwrap();
    let output = command(&ws, &owner, &["--rules-only", "--fail-on-high", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["blocked"], true);
    assert_eq!(report["incomplete"], true);
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 64);
    assert!(
        findings
            .iter()
            .any(|f| f["confirmed"] == true && f["rule"] == "tls-verification-disabled")
    );
}

#[test]
fn foreground_and_background_share_rules_and_scanners_cannot_modify_sources() {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    let owner = d.path().join("owner");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&owner).unwrap();
    let source = "import requests\nrequests.get(url, verify=False)\n";
    std::fs::write(ws.join("client.py"), source).unwrap();
    let git = declass_git::Git::locate().unwrap();
    git.run(&ws, &["init", "-q"], &[], None).unwrap();
    git.run(&ws, &["add", "client.py"], &[], None).unwrap();
    git.run(
        &ws,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.org",
            "commit",
            "-qm",
            "fixture",
        ],
        &[],
        None,
    )
    .unwrap();
    // A hostile installed tool tries to overwrite the read-only snapshot. It
    // can emit arbitrary prose, but only its checked location reaches a report.
    let script = "printf changed > client.py; printf '%s' '{\"results\":[{\"filename\":\"client.py\",\"line_number\":2,\"test_id\":\"synthetic\",\"issue_severity\":\"HIGH\",\"issue_text\":\"PRIVATE_SCANNER_OUTPUT_4891\"}]}'";
    let config = format!(
        "[local]\nenabled=false\n[review.scanners.fixture]\ncommand=\"/bin/sh\"\nformat=\"bandit\"\nargs={}\n",
        toml::Value::Array(vec!["-c".into(), script.into()])
    );
    std::fs::write(owner.join("config.toml"), config).unwrap();
    let output = command(&ws, &owner, &["--rules-only", "--fail-on-high", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["blocked"], true);
    assert_eq!(report["external_candidates"], 1);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_SCANNER_OUTPUT_4891"));
    assert_eq!(
        std::fs::read_to_string(ws.join("client.py")).unwrap(),
        source
    );
    let output = command(&ws, &owner, &["--background", "--rules-only", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let started: Value = serde_json::from_slice(&output.stdout).unwrap();
    let id = started["id"].as_str().unwrap();
    let start = Instant::now();
    let status = loop {
        let output = command(&ws, &owner, &["--status", id]);
        assert!(output.status.success());
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        if status["state"] == "completed" {
            break status;
        }
        assert_ne!(status["state"], "failed", "{status}");
        assert!(start.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status["dirty"], false);
    assert_eq!(status["commit"].as_str().unwrap().len(), 40);
    let dir = ws.join(".declass/runs").join(id);
    let metering: Value =
        serde_json::from_slice(&std::fs::read(dir.join("scan-usage.json")).unwrap()).unwrap();
    assert_eq!(metering["frontier"]["calls"], 0);
    let evidence = std::fs::read_to_string(dir.join("security-scanners/1-after.json")).unwrap();
    assert!(evidence.contains("PRIVATE_SCANNER_OUTPUT_4891"));
    let report: Value =
        serde_json::from_slice(&std::fs::read(dir.join("security-review.json")).unwrap()).unwrap();
    assert_eq!(report["findings"].as_array().unwrap().len(), 2);
    assert_eq!(
        std::fs::read_to_string(ws.join("client.py")).unwrap(),
        source
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.join("security-review.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
