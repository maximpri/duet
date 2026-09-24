// SPDX-License-Identifier: GPL-3.0-or-later
//! `tools/verify-release.sh` accepts a release signed and summed the way
//! `tools/release.sh` does it and rejects every tampering; `tools/release.sh`
//! refuses to start without an operator key or with a wrong version. The keys
//! are throwaway test keys in a temporary directory. Skipped without ssh-keygen.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools")
}

fn have_ssh_keygen() -> bool {
    Command::new("ssh-keygen")
        .arg("-?")
        .output()
        .map(|o| !o.stderr.is_empty() || !o.stdout.is_empty())
        .unwrap_or(false)
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn keygen(dir: &Path, name: &str) -> (PathBuf, String) {
    let key = dir.join(name);
    let ok = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
        .arg(&key)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    (key, public.trim().to_owned())
}

/// Sums and signs `dir` exactly as tools/release.sh does.
fn sum_and_sign(dir: &Path, key: &Path) {
    let names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with("SHA256SUMS"))
        .collect();
    let sums = Command::new("shasum")
        .args(["-a", "256"])
        .args(&names)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(sums.status.success());
    std::fs::write(dir.join("SHA256SUMS"), sums.stdout).unwrap();
    let _ = std::fs::remove_file(dir.join("SHA256SUMS.sig"));
    let signed = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", "duet-release", "-f"])
        .arg(key)
        .arg(dir.join("SHA256SUMS"))
        .output()
        .unwrap();
    assert!(signed.status.success(), "{}", text(&signed));
}

fn verify(dir: &Path, signers: &Path) -> Output {
    Command::new(tools().join("verify-release.sh"))
        .arg(dir)
        .arg("--signers")
        .arg(signers)
        .output()
        .unwrap()
}

#[test]
fn verify_release_accepts_a_signed_release_and_rejects_tampering() {
    if !have_ssh_keygen() {
        eprintln!("skipped: no ssh-keygen");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let (key, public) = keygen(root, "release");
    let (other_key, other_public) = keygen(root, "other");
    let signers = root.join("allowed_signers");
    std::fs::write(
        &signers,
        format!("release@duet namespaces=\"duet-release\" {public}\n"),
    )
    .unwrap();
    let rel = root.join("duet-0.1.0");
    std::fs::create_dir(&rel).unwrap();
    std::fs::write(rel.join("duet-0.1.0-aarch64-apple-darwin"), b"\x7fbinary").unwrap();
    std::fs::write(rel.join("duet-0.1.0.cdx.json"), "{}\n").unwrap();
    std::fs::write(rel.join("BUILDINFO.txt"), "duet 0.1.0\n").unwrap();
    sum_and_sign(&rel, &key);

    let ok = verify(&rel, &signers);
    assert!(ok.status.success(), "{}", text(&ok));
    assert!(text(&ok).contains("signed by release@duet; 3 file(s) match"));

    // A changed file.
    std::fs::write(rel.join("BUILDINFO.txt"), "duet 0.1.1\n").unwrap();
    let bad = verify(&rel, &signers);
    assert!(!bad.status.success());
    assert!(text(&bad).contains("does not match"), "{}", text(&bad));
    std::fs::write(rel.join("BUILDINFO.txt"), "duet 0.1.0\n").unwrap();

    // A file added next to the listed ones.
    std::fs::write(rel.join("extra"), "x").unwrap();
    let bad = verify(&rel, &signers);
    assert!(text(&bad).contains("not in SHA256SUMS"), "{}", text(&bad));
    std::fs::remove_file(rel.join("extra")).unwrap();

    // Edited sums (re-hashed after swapping a file) no longer match the signature.
    let sums = std::fs::read_to_string(rel.join("SHA256SUMS")).unwrap();
    std::fs::write(rel.join("SHA256SUMS"), sums.replacen('0', "1", 1)).unwrap();
    let bad = verify(&rel, &signers);
    assert!(!bad.status.success(), "{}", text(&bad));
    std::fs::write(rel.join("SHA256SUMS"), &sums).unwrap();

    // Signed by a key the operator does not trust, or in another namespace.
    sum_and_sign(&rel, &other_key);
    let bad = verify(&rel, &signers);
    assert!(
        text(&bad).contains("not signed by any key"),
        "{}",
        text(&bad)
    );
    let wrong_ns = root.join("wrong_ns");
    std::fs::write(
        &wrong_ns,
        format!("other@duet namespaces=\"git\" {other_public}\n"),
    )
    .unwrap();
    assert!(!verify(&rel, &wrong_ns).status.success());

    // Unsigned.
    std::fs::remove_file(rel.join("SHA256SUMS.sig")).unwrap();
    let bad = verify(&rel, &signers);
    assert!(text(&bad).contains("unsigned"), "{}", text(&bad));
    assert!(!verify(&rel, &root.join("missing")).status.success());
}

fn release(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut c = Command::new(tools().join("release.sh"));
    c.args(args).env_remove("DUET_RELEASE_KEY");
    for (k, v) in envs {
        c.env(k, v);
    }
    c.output().unwrap()
}

#[test]
fn release_refuses_to_start_without_an_operator_key_or_the_right_version() {
    let version = env!("CARGO_PKG_VERSION");
    let no_key = release(&[version], &[]);
    assert!(!no_key.status.success());
    assert!(
        text(&no_key).contains("signing key is required"),
        "{}",
        text(&no_key)
    );

    let missing = release(&[version, "--key", "/nonexistent/duet-key"], &[]);
    assert!(
        text(&missing).contains("not a readable file"),
        "{}",
        text(&missing)
    );

    let d = tempfile::tempdir().unwrap();
    let key = d.path().join("k");
    std::fs::write(&key, "not used").unwrap();
    let key = key.to_str().unwrap();
    let wrong = release(&["9.9.9", "--key", key], &[]);
    assert!(
        text(&wrong).contains("does not match the workspace version"),
        "{}",
        text(&wrong)
    );
    let malformed = release(&["v1", "--key", key], &[]);
    assert!(
        text(&malformed).contains("not x.y.z"),
        "{}",
        text(&malformed)
    );

    // A key inside the repository is refused (the manifest stands in for one).
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let inside = release(&[version, "--key", inside.to_str().unwrap()], &[]);
    assert!(
        text(&inside).contains("inside the repository"),
        "{}",
        text(&inside)
    );

    // The environment variable names the key too; the checks above still apply.
    let env_key = release(&["9.9.9"], &[("DUET_RELEASE_KEY", key)]);
    assert!(
        text(&env_key).contains("does not match"),
        "{}",
        text(&env_key)
    );
}
