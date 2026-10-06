// SPDX-License-Identifier: GPL-3.0-or-later
//! `declass-sbom` on this workspace: offline, the product's runtime closure with
//! licenses and hashes, no dev-only packages, no local paths.

use serde_json::Value;
use std::process::Command;

#[test]
fn the_sbom_of_declass_cli_lists_its_dependencies_with_licenses() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sbom.json");
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml");
    let o = Command::new(env!("CARGO_BIN_EXE_declass-sbom"))
        .args(["--manifest-path", manifest, "--out"])
        .arg(&out)
        .env("SOURCE_DATE_EPOCH", "1790000000")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = std::fs::read_to_string(&out).unwrap();
    let bom: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(bom["metadata"]["component"]["name"], "declass-cli");
    assert_eq!(bom["metadata"]["timestamp"], "2026-09-21T14:13:20Z");
    let components = bom["components"].as_array().unwrap();
    let named = |n: &str| components.iter().find(|c| c["name"] == n);
    for runtime in ["declass-boundary", "serde", "reqwest", "tokio"] {
        assert!(named(runtime).is_some(), "{runtime} missing");
    }
    let serde = named("serde").unwrap();
    assert!(
        serde["licenses"][0]["expression"]
            .as_str()
            .unwrap()
            .contains("MIT")
    );
    assert_eq!(serde["hashes"][0]["alg"], "SHA-256");
    // Dev-only dependencies and the harness are not part of the product.
    for absent in ["proptest", "declass-evals", "declass-release"] {
        assert!(named(absent).is_none(), "{absent} listed");
    }
    // Every component declares a license.
    for c in components {
        assert!(c["licenses"].is_array(), "{} has no license", c["name"]);
    }
    let root = env!("CARGO_MANIFEST_DIR");
    let home = std::env::var("HOME").unwrap_or_else(|_| "/nonexistent-home".into());
    assert!(!text.contains(root) && !text.contains(&home));
}
