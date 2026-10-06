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
        .args(["-a", "256", "--"])
        .args(&names)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(sums.status.success());
    std::fs::write(dir.join("SHA256SUMS"), sums.stdout).unwrap();
    sign_sums(dir, key);
}

fn sign_sums(dir: &Path, key: &Path) {
    let _ = std::fs::remove_file(dir.join("SHA256SUMS.sig"));
    let signed = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", "declass-release", "-f"])
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
        format!("release@declass namespaces=\"declass-release\" {public}\n"),
    )
    .unwrap();
    let rel = root.join("declass-0.1.0");
    std::fs::create_dir(&rel).unwrap();
    std::fs::write(
        rel.join("declass-0.1.0-aarch64-apple-darwin"),
        b"\x7fbinary",
    )
    .unwrap();
    std::fs::write(rel.join("declass-0.1.0.cdx.json"), "{}\n").unwrap();
    std::fs::write(rel.join("BUILDINFO.txt"), "declass 0.1.0\n").unwrap();
    sum_and_sign(&rel, &key);

    let ok = verify(&rel, &signers);
    assert!(ok.status.success(), "{}", text(&ok));
    assert!(text(&ok).contains("signed by release@declass; 3 file(s) match"));

    // A changed file.
    std::fs::write(rel.join("BUILDINFO.txt"), "declass 0.1.1\n").unwrap();
    let bad = verify(&rel, &signers);
    assert!(!bad.status.success());
    assert!(text(&bad).contains("does not match"), "{}", text(&bad));
    std::fs::write(rel.join("BUILDINFO.txt"), "declass 0.1.0\n").unwrap();

    // A file added next to the listed ones.
    std::fs::write(rel.join("extra"), "x").unwrap();
    let bad = verify(&rel, &signers);
    assert!(text(&bad).contains("not in SHA256SUMS"), "{}", text(&bad));
    std::fs::remove_file(rel.join("extra")).unwrap();

    // Dot-prefixed and option-shaped names must be checked too.
    for name in [".hidden", "..hidden", "-extra"] {
        std::fs::write(rel.join(name), "unlisted").unwrap();
        assert!(!verify(&rel, &signers).status.success(), "accepted {name}");
        std::fs::remove_file(rel.join(name)).unwrap();
    }
    #[cfg(unix)]
    {
        // A dangling link is invisible to shell `-e`; it is still an artifact.
        std::os::unix::fs::symlink(root.join("missing-target"), rel.join("..link")).unwrap();
        assert!(!verify(&rel, &signers).status.success());
        std::fs::remove_file(rel.join("..link")).unwrap();

        // Even identical bytes must not allow a listed file to point outside
        // the release directory (the target can change after verification).
        let backing = root.join("buildinfo-copy");
        std::fs::copy(rel.join("BUILDINFO.txt"), &backing).unwrap();
        std::fs::remove_file(rel.join("BUILDINFO.txt")).unwrap();
        std::os::unix::fs::symlink(&backing, rel.join("BUILDINFO.txt")).unwrap();
        assert!(!verify(&rel, &signers).status.success());
        std::fs::remove_file(rel.join("BUILDINFO.txt")).unwrap();
        std::fs::create_dir(rel.join("BUILDINFO.txt")).unwrap();
        assert!(!verify(&rel, &signers).status.success());
        std::fs::remove_dir(rel.join("BUILDINFO.txt")).unwrap();
        std::fs::copy(backing, rel.join("BUILDINFO.txt")).unwrap();
    }

    // A signed, listed filename beginning with '-' is data, not a grep flag.
    std::fs::write(rel.join("-metadata"), "extra signed metadata").unwrap();
    sum_and_sign(&rel, &key);
    let ok = verify(&rel, &signers);
    assert!(ok.status.success(), "{}", text(&ok));
    std::fs::remove_file(rel.join("-metadata")).unwrap();
    sum_and_sign(&rel, &key);

    // shasum accepts valid hashes mixed with malformed lines, so the verifier
    // must reject a signed bare filename that otherwise escapes hashing.
    let valid_sums = std::fs::read_to_string(rel.join("SHA256SUMS")).unwrap();
    std::fs::write(rel.join("unchecked"), "bytes without a digest").unwrap();
    std::fs::write(rel.join("SHA256SUMS"), format!("{valid_sums}unchecked\n")).unwrap();
    sign_sums(&rel, &key);
    assert!(!verify(&rel, &signers).status.success());
    std::fs::remove_file(rel.join("unchecked")).unwrap();

    // Bash versions differ in how read handles embedded NUL bytes. Reject
    // raw control bytes before parsing so neither shell can repair a signed
    // malformed record into a different checksum line.
    for control in *b"\0\r\t" {
        let mut malformed = valid_sums.as_bytes().to_vec();
        malformed.insert(8, control);
        std::fs::write(rel.join("SHA256SUMS"), malformed).unwrap();
        sign_sums(&rel, &key);
        let bad = verify(&rel, &signers);
        assert!(!bad.status.success(), "accepted control byte {control}");
        assert!(text(&bad).contains("control"), "{}", text(&bad));
    }

    // Each artifact is listed exactly once, even under a valid signature.
    std::fs::write(
        rel.join("SHA256SUMS"),
        format!("{valid_sums}{}\n", valid_sums.lines().next().unwrap()),
    )
    .unwrap();
    sign_sums(&rel, &key);
    assert!(!verify(&rel, &signers).status.success());
    sum_and_sign(&rel, &key);

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
        format!("other@declass namespaces=\"git\" {other_public}\n"),
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
    c.args(args).env_remove("DECLASS_RELEASE_KEY");
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

    let missing = release(&[version, "--key", "/nonexistent/declass-key"], &[]);
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
    let env_key = release(&["9.9.9"], &[("DECLASS_RELEASE_KEY", key)]);
    assert!(
        text(&env_key).contains("does not match"),
        "{}",
        text(&env_key)
    );
}

/// Exercise the production packaging scripts with tiny build fixtures. The
/// source helper itself is real: it must reject a dirty tree, archive only the
/// committed tree, retain vendored notices, and work with a nonignored --out.
#[test]
#[cfg(unix)]
fn release_packages_committed_source_and_notices_with_signed_checksums() {
    use std::os::unix::fs::PermissionsExt;

    if !have_ssh_keygen() {
        eprintln!("skipped: no ssh-keygen");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(repo.join("tools")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    let write = |path: &Path, contents: &str| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    };
    let script = |path: &Path, contents: &str| {
        write(path, &format!("#!/usr/bin/env bash\nset -eu\n{contents}\n"));
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    for name in ["release.sh", "source-release.sh"] {
        std::fs::copy(tools().join(name), repo.join("tools").join(name)).unwrap();
    }
    for name in [
        "LICENSE",
        "NOTICE",
        "LICENSES.md",
        "crates/declass-boundary/rules/LICENSE.gitleaks",
        "crates/declass-boundary/rules/NOTICE",
    ] {
        let dest = repo.join(name);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(tools().join("..").join(name), dest).unwrap();
    }
    write(
        &repo.join("Cargo.toml"),
        "[workspace.package]\nversion = \"0.1.0\"\n",
    );
    write(&repo.join(".gitignore"), "/target\n/ignored-secret\n");
    write(&repo.join("original.rs"), "// committed source\n");
    write(&repo.join("ignored-secret"), "must not enter the archive\n");
    script(&repo.join("tools/gate.sh"), "exit 0");
    script(
        &repo.join("tools/sbom.sh"),
        "while [ $# -gt 0 ]; do\n if [ \"$1\" = --out ]; then echo '{}' >\"$2\"; exit; fi\n shift\ndone",
    );
    script(
        &bin.join("rustc"),
        "if [ \"$1\" = -vV ]; then echo 'host: test-target'; else echo 'rustc fixture'; fi",
    );
    script(
        &bin.join("cargo"),
        r#"case "$1" in
build) mkdir -p target/test-target/release; echo 'binary fixture' >target/test-target/release/declass ;;
metadata) printf '{"target_directory":"%s/target"}\n' "$PWD" ;;
vendor)
    mkdir -p vendor/dependency-1.0
    echo 'upstream notice fixture' >vendor/dependency-1.0/LICENSE
    if [ "$(uname -s)" = Darwin ]; then
        xattr -w org.declass.release-test metadata-fixture vendor/dependency-1.0/LICENSE
    fi
    printf '[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "vendor"\n'
    ;;
-V) echo 'cargo fixture' ;;
*) exit 1 ;;
esac"#,
    );
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=Release Test",
                "-c",
                "user.email=release@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out));
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "release fixture"]);
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let archive = temp.path().join("source.tar.gz");
    let source = || {
        Command::new(repo.join("tools/source-release.sh"))
            .arg(&archive)
            .env("PATH", &path)
            .output()
            .unwrap()
    };
    write(&repo.join("untracked.rs"), "// not committed\n");
    let dirty = source();
    assert!(!dirty.status.success());
    assert!(text(&dirty).contains("clean, committed checkout"));
    assert!(!archive.exists());
    std::fs::remove_file(repo.join("untracked.rs")).unwrap();
    write(&repo.join("original.rs"), "// uncommitted change\n");
    assert!(!source().status.success());
    git(&["checkout", "--", "original.rs"]);

    let (key, public) = keygen(temp.path(), "release");
    let signers = temp.path().join("allowed_signers");
    write(
        &signers,
        &format!("release@declass namespaces=\"declass-release\" {public}\n"),
    );
    // Not ignored: this catches populating --out before the source clean check.
    let out_dir = repo.join("custom output");
    let release = Command::new(repo.join("tools/release.sh"))
        .args(["0.1.0", "--key"])
        .arg(&key)
        .arg("--out")
        .arg(&out_dir)
        .env("PATH", &path)
        .env_remove("DECLASS_RELEASE_KEY")
        .env_remove("COPYFILE_DISABLE")
        .output()
        .unwrap();
    assert!(release.status.success(), "{}", text(&release));
    let checked = verify(&out_dir, &signers);
    assert!(checked.status.success(), "{}", text(&checked));
    assert!(text(&checked).contains("10 file(s) match"));
    for name in ["LICENSE", "NOTICE", "LICENSES.md"] {
        assert_eq!(
            std::fs::read(out_dir.join(name)).unwrap(),
            std::fs::read(repo.join(name)).unwrap()
        );
    }
    let source_archive = out_dir.join("declass-0.1.0-source.tar.gz");
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(&source_archive)
        .output()
        .unwrap();
    assert!(listing.status.success(), "{}", text(&listing));
    assert!(
        String::from_utf8_lossy(&listing.stdout)
            .lines()
            .all(|name| !name.split('/').any(|part| part.starts_with("._"))),
        "source archive contains platform metadata"
    );
    let unpacked = temp.path().join("unpacked");
    std::fs::create_dir(&unpacked).unwrap();
    let unpack = Command::new("tar")
        .arg("-xzf")
        .arg(source_archive)
        .arg("-C")
        .arg(&unpacked)
        .output()
        .unwrap();
    assert!(unpack.status.success(), "{}", text(&unpack));
    let source_tree = unpacked.join("declass-source");
    assert_eq!(
        std::fs::read_to_string(source_tree.join("original.rs")).unwrap(),
        "// committed source\n"
    );
    assert_eq!(
        std::fs::read_to_string(source_tree.join("vendor/dependency-1.0/LICENSE")).unwrap(),
        "upstream notice fixture\n"
    );
    assert!(source_tree.join(".cargo/config.toml").is_file());
    assert!(!source_tree.join("ignored-secret").exists());
    assert!(!source_tree.join("custom output").exists());
    assert!(!source_tree.join(".git").exists());
    std::fs::write(out_dir.join("NOTICE"), "tampered notice\n").unwrap();
    assert!(!verify(&out_dir, &signers).status.success());
}
