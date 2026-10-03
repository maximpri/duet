// SPDX-License-Identifier: GPL-3.0-or-later
//! Real signatures, checksums, packaging and installation, with only the HTTPS
//! transport and host architecture substituted. No production signing key.
#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools")
}
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
fn script(path: &Path, text: &str) {
    write(path, &format!("#!/usr/bin/env bash\nset -eu\n{text}\n"));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn describe(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
fn checked(cmd: &mut Command) -> Output {
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    out
}
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    key: PathBuf,
    signers: PathBuf,
    payload: PathBuf,
    assets: PathBuf,
    stem: String,
}
impl Fixture {
    fn new(target: &str, reported_version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let key = root.join("key");
        checked(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key),
        );
        let signers = root.join("allowed_signers");
        write(
            &signers,
            &format!(
                "release-test namespaces=\"duet-release\" {}",
                std::fs::read_to_string(key.with_extension("pub")).unwrap()
            ),
        );
        let payload = root.join("payload");
        let stem = format!("duet-0.1.0-{target}");
        script(
            &payload.join(&stem),
            &format!(
                "printf 'executed\\n' >>\"$DUET_TEST_RUN_MARKER\"\nprintf 'duet {reported_version}\\n'"
            ),
        );
        for name in [
            "duet-0.1.0.cdx.json",
            "LICENSE",
            "NOTICE",
            "LICENSES.md",
            "LICENSE.gitleaks",
            "NOTICE.gitleaks",
            "duet-0.1.0-source.tar.gz",
            "SOURCE.txt",
        ] {
            write(&payload.join(name), "signed fixture\n");
        }
        write(
            &payload.join("BUILDINFO.txt"),
            &format!("duet 0.1.0\ntarget {target}\n"),
        );
        let assets = root.join("assets");
        let f = Self {
            _dir: dir,
            root,
            key,
            signers,
            payload,
            assets,
            stem,
        };
        f.sign_payload();
        checked(
            Command::new(tools().join("package-release.sh"))
                .arg(&f.payload)
                .arg("--key")
                .arg(&f.key)
                .arg("--signers")
                .arg(&f.signers)
                .arg("--out")
                .arg(&f.assets),
        );
        let bin = f.root.join("transport");
        script(
            &bin.join("uname"),
            "case \"$1\" in -s) echo \"$DUET_TEST_OS\" ;; -m) echo \"$DUET_TEST_ARCH\" ;; *) exit 2 ;; esac",
        );
        script(
            &bin.join("curl"),
            r#"destination=''
secure=false
secure_redirect=false
while [ $# -gt 0 ]; do
    case "$1" in
    --output) destination="$2"; shift 2 ;;
    --proto) [ "$2" = '=https' ]; secure=true; shift 2 ;;
    --proto-redir) [ "$2" = '=https' ]; secure_redirect=true; shift 2 ;;
    --tlsv1.2 | --fail | --silent | --show-error | --location) shift ;;
    --max-redirs | --connect-timeout | --max-time) shift 2 ;;
    https://github.com/maximpri/duet/releases/download/v0.1.0/*) url="$1"; shift ;;
    *) exit 2 ;;
    esac
done
$secure && $secure_redirect
name=${url##*/}
[ "$name" != "${DUET_TEST_FAIL_DOWNLOAD:-}" ] || exit 22
cp "$DUET_TEST_ASSETS/$name" "$destination""#,
        );
        f
    }
    fn sign(&self, path: &Path) {
        let sig = PathBuf::from(format!("{}.sig", path.display()));
        let _ = std::fs::remove_file(sig);
        checked(
            Command::new("ssh-keygen")
                .args(["-Y", "sign", "-n", "duet-release", "-f"])
                .arg(&self.key)
                .arg(path),
        );
    }
    fn sign_payload(&self) {
        let mut sums = String::new();
        for entry in std::fs::read_dir(&self.payload).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap();
            if name.starts_with("SHA256SUMS") {
                continue;
            }
            sums.push_str(&format!(
                "{:x}  {name}\n",
                Sha256::digest(std::fs::read(&path).unwrap())
            ));
        }
        write(&self.payload.join("SHA256SUMS"), &sums);
        self.sign(&self.payload.join("SHA256SUMS"));
    }
    /// Authenticated archive containing an independently invalid inner payload.
    fn repack_and_sign_outer(&self) {
        let archive = self.assets.join(format!("{}.tar.gz", self.stem));
        let files: Vec<_> = std::fs::read_dir(&self.payload)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        checked(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&self.payload)
                .args(files),
        );
        let manifest = self.assets.join(format!("{}.SHA256SUMS", self.stem));
        write(
            &manifest,
            &format!(
                "{:x}  {}.tar.gz\n",
                Sha256::digest(std::fs::read(archive).unwrap()),
                self.stem
            ),
        );
        self.sign(&manifest);
    }
    fn installer(&self, os: &str, arch: &str) -> Command {
        let bin = self.root.join("install/bin");
        write(&bin.join("duet"), "previous installation\n");
        let mut command = Command::new(tools().join("install-release.sh"));
        let path = std::env::join_paths(
            std::iter::once(self.root.join("transport"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        command
            .arg("0.1.0")
            .arg("--signers")
            .arg(&self.signers)
            .env("PATH", path)
            .env("DUET_TEST_OS", os)
            .env("DUET_TEST_ARCH", arch)
            .env("DUET_TEST_ASSETS", &self.assets)
            .env("DUET_INSTALL_DIR", &bin)
            .env("DUET_DATA_DIR", self.root.join("install/share"))
            .env("DUET_TEST_RUN_MARKER", self.root.join("executed"));
        command
    }
    fn assert_previous_retained(&self) {
        assert_eq!(
            std::fs::read_to_string(self.root.join("install/bin/duet")).unwrap(),
            "previous installation\n"
        );
    }
    fn assert_not_executed(&self) {
        assert!(!self.root.join("executed").exists());
    }
}

#[test]
fn selects_and_installs_all_four_platforms_and_retains_signed_source() {
    for (os, arch, target) in [
        ("Darwin", "arm64", "aarch64-apple-darwin"),
        ("Darwin", "x86_64", "x86_64-apple-darwin"),
        ("Linux", "aarch64", "aarch64-unknown-linux-gnu"),
        ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
    ] {
        let f = Fixture::new(target, "0.1.0");
        checked(&mut f.installer(os, arch));
        assert_eq!(
            std::fs::read(f.root.join("install/bin/duet")).unwrap(),
            std::fs::read(f.payload.join(&f.stem)).unwrap()
        );
        assert!(f.root.join("executed").exists());
        let records: Vec<_> = std::fs::read_dir(f.root.join("install/share/releases"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(records.len(), 1);
        for name in [
            "LICENSE",
            "NOTICE",
            "SOURCE.txt",
            "duet-0.1.0-source.tar.gz",
            "SHA256SUMS.sig",
        ] {
            assert!(records[0].join(name).is_file(), "missing {name}");
        }
    }
}

#[test]
fn download_and_authentication_failures_never_execute_or_replace_the_old_binary() {
    for failure in [
        "download",
        "archive",
        "signature",
        "missing-signature",
        "missing-signers",
        "untrusted-signer",
        "inner-payload",
        "inner-link",
        "http",
        "unsupported",
    ] {
        let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
        let mut command = f.installer("Linux", "aarch64");
        match failure {
            "download" => {
                command.env("DUET_TEST_FAIL_DOWNLOAD", format!("{}.tar.gz", f.stem));
            }
            "archive" => write(&f.assets.join(format!("{}.tar.gz", f.stem)), "tampered"),
            "signature" => write(
                &f.assets.join(format!("{}.SHA256SUMS.sig", f.stem)),
                "tampered",
            ),
            "missing-signature" => {
                std::fs::remove_file(f.assets.join(format!("{}.SHA256SUMS.sig", f.stem))).unwrap()
            }
            "missing-signers" => std::fs::remove_file(&f.signers).unwrap(),
            "untrusted-signer" => {
                let other = f.root.join("other-key");
                checked(
                    Command::new("ssh-keygen")
                        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                        .arg(&other),
                );
                write(
                    &f.signers,
                    &format!(
                        "other namespaces=\"duet-release\" {}",
                        std::fs::read_to_string(other.with_extension("pub")).unwrap()
                    ),
                );
            }
            "inner-payload" => {
                write(&f.payload.join(&f.stem), "tampered executable");
                f.repack_and_sign_outer();
            }
            "inner-link" => {
                std::fs::remove_file(f.payload.join(&f.stem)).unwrap();
                std::os::unix::fs::symlink(f.root.join("outside"), f.payload.join(&f.stem))
                    .unwrap();
                f.repack_and_sign_outer();
            }
            "http" => {
                command.args(["--url", "http://example.invalid/releases"]);
            }
            "unsupported" => {
                command.env("DUET_TEST_OS", "Unsupported");
            }
            _ => unreachable!(),
        }
        let result = command.output().unwrap();
        assert!(
            !result.status.success(),
            "accepted {failure}: {}",
            describe(&result)
        );
        f.assert_previous_retained();
        f.assert_not_executed();
        assert!(!f.root.join("outside").exists());
    }
}

#[test]
fn authenticated_but_unusable_binary_leaves_previous_installation_intact() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "9.9.9");
    let result = f.installer("Linux", "aarch64").output().unwrap();
    assert!(!result.status.success());
    assert!(
        describe(&result).contains("unexpected version"),
        "{}",
        describe(&result)
    );
    f.assert_previous_retained();
    assert!(f.root.join("executed").exists()); // Authenticated code only.
}

#[test]
fn failure_retaining_source_keeps_old_binary_and_removes_staging_file() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    let blocked = f.root.join("not-a-directory");
    write(&blocked, "file blocks metadata directory creation");
    let out = f
        .installer("Linux", "aarch64")
        .env("DUET_DATA_DIR", blocked)
        .output()
        .unwrap();
    assert!(!out.status.success());
    f.assert_previous_retained();
    assert!(f.root.join("executed").exists());
    assert_eq!(
        std::fs::read_dir(f.root.join("install/bin"))
            .unwrap()
            .count(),
        1
    );
}
