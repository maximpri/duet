// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline DMG/installer contracts. hdiutil is replaced by a capture shim;
//! real SSH signatures, checksums, file installation and source retention run.
#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TARGET: &str = "aarch64-apple-darwin";

fn tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools")
}
fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn sums(dir: &Path) {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            !p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("SHA256SUMS")
        })
        .collect();
    files.sort();
    let text: String = files
        .iter()
        .map(|p| {
            format!(
                "{:x}  {}\n",
                Sha256::digest(std::fs::read(p).unwrap()),
                p.file_name().unwrap().to_str().unwrap()
            )
        })
        .collect();
    std::fs::write(dir.join("SHA256SUMS"), text).unwrap();
}
fn assert_ok(o: Output) {
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    release: PathBuf,
    home: PathBuf,
    capture: PathBuf,
    image: PathBuf,
    path: std::ffi::OsString,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let release = root.join("release with spaces");
        let home = root.join("home with spaces");
        let stubs = root.join("stubs");
        for dir in [&release, &home, &stubs] {
            std::fs::create_dir(dir).unwrap();
        }
        executable(
            &stubs.join("uname"),
            "#!/bin/sh\ncase \"$1\" in -s) echo Darwin;; -m) echo \"${DMG_TEST_ARCH:-arm64}\";; *) exit 1;; esac\n",
        );
        executable(
            &stubs.join("sysctl"),
            "#!/bin/sh\n[ \"$*\" = '-in sysctl.proc_translated' ] || exit 1\necho \"${DMG_TEST_TRANSLATED:-0}\"\n",
        );
        executable(
            &stubs.join("hdiutil"),
            "#!/bin/bash\nset -eu\n[ \"$1\" = create ]\nsrc=\"\"\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = -srcfolder ]; then src=$2; shift; fi\n  dest=$1; shift\ndone\ncp -pR \"$src\" \"$DMG_CAPTURE\"\nprintf 'fixture disk image\\n' >\"$dest\"\n",
        );
        for name in [
            "LICENSE",
            "NOTICE",
            "LICENSES.md",
            "LICENSE.gitleaks",
            "NOTICE.gitleaks",
            "SOURCE.txt",
            "declass-0.1.0-source.tar.gz",
            "declass-0.1.0.cdx.json",
        ] {
            std::fs::write(release.join(name), format!("fixture {name}\n")).unwrap();
        }
        std::fs::write(
            release.join("BUILDINFO.txt"),
            format!("declass 0.1.0\ntarget {TARGET}\n"),
        )
        .unwrap();
        executable(
            &release.join(format!("declass-0.1.0-{TARGET}")),
            "#!/bin/sh\necho 'declass 0.1.0'\n",
        );
        sums(&release);
        let path = std::env::join_paths(
            std::iter::once(stubs).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        Self {
            _temp: temp,
            release,
            home,
            capture: root.join("mounted"),
            image: root.join("Declass candidate.dmg"),
            path,
            root,
        }
    }
    fn package(&self) -> Command {
        let mut c = Command::new("/bin/bash");
        c.arg(tools().join("package-dmg.sh"))
            .arg(&self.release)
            .arg("--out")
            .arg(&self.image)
            .env("PATH", &self.path)
            .env("HOME", &self.home)
            .env("DMG_CAPTURE", &self.capture)
            .env("TMPDIR", &self.root)
            .env_remove("DECLASS_CONFIG_HOME");
        c
    }
    fn install(&self) -> Command {
        let mut c = Command::new("/bin/bash");
        c.arg(self.capture.join("Install Declass.command"))
            .env("PATH", &self.path)
            .env("HOME", &self.home)
            .env_remove("DECLASS_CONFIG_HOME");
        c
    }
}

#[test]
fn unsigned_dmg_installs_offline_preserves_source_and_refuses_tampering() {
    let f = Fixture::new();
    assert_ok(f.package().output().unwrap());
    assert_eq!(
        std::fs::read_to_string(f.capture.join("INSTALL-MODE")).unwrap(),
        "unsigned\n"
    );
    assert!(f.image.with_extension("dmg.SHA256SUMS").is_file());
    assert!(!f.image.with_extension("dmg.SHA256SUMS.sig").exists());
    for (name, mode) in [
        ("", 0o755),
        ("payload", 0o755),
        ("README.txt", 0o644),
        ("INSTALL-MODE", 0o644),
        ("Install Declass.command", 0o755),
        ("verify-release.sh", 0o755),
        ("verify-checksums.sh", 0o755),
        ("payload/declass-0.1.0-aarch64-apple-darwin", 0o755),
        ("payload/declass-0.1.0-source.tar.gz", 0o644),
        ("payload/SHA256SUMS", 0o644),
    ] {
        assert_eq!(
            std::fs::metadata(f.capture.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            mode,
            "mounted mode for {name}"
        );
    }
    assert!(!f.install().output().unwrap().status.success());
    assert!(!f.home.join(".local/bin/declass").exists());
    assert_ok(f.install().arg("--unsigned").output().unwrap());
    let installed = f.home.join(".local/bin/declass");
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        std::fs::read(f.release.join(format!("declass-0.1.0-{TARGET}"))).unwrap()
    );
    let records: Vec<_> = std::fs::read_dir(f.home.join(".local/share/declass/releases"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(records.len(), 1);
    for entry in std::fs::read_dir(&f.release).unwrap() {
        let p = entry.unwrap().path();
        assert_eq!(
            std::fs::read(&p).unwrap(),
            std::fs::read(records[0].join(p.file_name().unwrap())).unwrap()
        );
    }
    assert_ok(f.install().arg("--unsigned").output().unwrap());
    let original_image = std::fs::read(&f.image).unwrap();
    assert!(!f.package().output().unwrap().status.success());
    assert_eq!(std::fs::read(&f.image).unwrap(), original_image);

    let untouched = f.root.join("untouched");
    std::fs::write(&untouched, "outside\n").unwrap();
    std::fs::remove_file(&installed).unwrap();
    symlink(&untouched, &installed).unwrap();
    assert!(
        !f.install()
            .arg("--unsigned")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(std::fs::read_to_string(&untouched).unwrap(), "outside\n");
    std::fs::remove_file(&installed).unwrap();
    std::fs::write(f.capture.join("payload/NOTICE"), "tampered\n").unwrap();
    assert!(
        !f.install()
            .arg("--unsigned")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!installed.exists());
}

#[test]
fn rosetta_installer_uses_native_apple_silicon_architecture() {
    let f = Fixture::new();
    assert_ok(f.package().output().unwrap());
    assert_ok(
        f.install()
            .arg("--unsigned")
            .env("DMG_TEST_ARCH", "x86_64")
            .env("DMG_TEST_TRANSLATED", "1")
            .output()
            .unwrap(),
    );
    assert!(f.home.join(".local/bin/declass").is_file());
}

#[test]
fn incompatible_candidates_preserve_the_existing_binary_but_verify_without_execution() {
    for body in ["#!/bin/sh\nexit 1\n", "#!/bin/sh\necho 'declass 9.9.9'\n"] {
        let f = Fixture::new();
        assert_ok(f.package().output().unwrap());
        assert_ok(f.install().arg("--unsigned").output().unwrap());
        let installed = f.home.join(".local/bin/declass");
        let original = std::fs::read(&installed).unwrap();
        let payload = f.capture.join("payload");
        executable(&payload.join(format!("declass-0.1.0-{TARGET}")), body);
        sums(&payload);
        assert_ok(
            f.install()
                .args(["--unsigned", "--verify-only"])
                .output()
                .unwrap(),
        );
        assert!(
            !f.install()
                .arg("--unsigned")
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(std::fs::read(&installed).unwrap(), original);
        assert!(
            !f.home
                .join(".local/share/declass/releases/.install-lock")
                .exists()
        );
    }
}

#[test]
fn unsigned_dmg_refuses_signatures_missing_source_and_nonregular_files() {
    for kind in ["signature", "source", "symlink", "malformed"] {
        let f = Fixture::new();
        match kind {
            "signature" => {
                std::fs::write(f.release.join("SHA256SUMS.sig"), "invalid signature\n").unwrap()
            }
            "source" => {
                std::fs::remove_file(f.release.join("declass-0.1.0-source.tar.gz")).unwrap();
                sums(&f.release);
            }
            "symlink" => {
                std::fs::remove_file(f.release.join("NOTICE")).unwrap();
                symlink(f.root.join("missing"), f.release.join("NOTICE")).unwrap();
            }
            "malformed" => {
                let p = f.release.join("SHA256SUMS");
                let text = std::fs::read_to_string(&p).unwrap();
                std::fs::write(f.release.join("unchecked"), "not hashed").unwrap();
                std::fs::write(p, format!("{text}unchecked\n")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            !f.package().output().unwrap().status.success(),
            "accepted {kind}"
        );
        assert!(!f.image.exists());
        assert!(!f.capture.exists(), "invalid payload reached hdiutil");
    }
}

#[test]
fn signed_dmg_authenticates_payload_and_outer_image_with_external_trust() {
    if Command::new("ssh-keygen").arg("-?").output().is_err() {
        return;
    }
    let f = Fixture::new();
    let key = f.root.join("test-key");
    assert_ok(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output()
            .unwrap(),
    );
    let signers = f.root.join("allowed_signers");
    std::fs::write(
        &signers,
        format!(
            "dmg-test namespaces=\"declass-release\" {}",
            std::fs::read_to_string(key.with_extension("pub")).unwrap()
        ),
    )
    .unwrap();
    assert_ok(
        Command::new("ssh-keygen")
            .args(["-Y", "sign", "-n", "declass-release", "-f"])
            .arg(&key)
            .arg(f.release.join("SHA256SUMS"))
            .output()
            .unwrap(),
    );
    assert!(
        !f.package()
            .arg("--signers")
            .arg(&signers)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_ok(
        f.package()
            .arg("--key")
            .arg(&key)
            .arg("--signers")
            .arg(&signers)
            .output()
            .unwrap(),
    );
    let sig = f.image.with_extension("dmg.SHA256SUMS.sig");
    let manifest = f.image.with_extension("dmg.SHA256SUMS");
    assert_ok(
        Command::new("ssh-keygen")
            .args([
                "-Y",
                "verify",
                "-n",
                "declass-release",
                "-I",
                "dmg-test",
                "-f",
            ])
            .arg(&signers)
            .arg("-s")
            .arg(sig)
            .stdin(std::fs::File::open(manifest).unwrap())
            .output()
            .unwrap(),
    );
    assert!(
        !f.install()
            .arg("--verify-only")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_ok(
        f.install()
            .arg("--verify-only")
            .arg("--signers")
            .arg(&signers)
            .output()
            .unwrap(),
    );
    assert!(
        !f.install()
            .arg("--unsigned")
            .arg("--verify-only")
            .output()
            .unwrap()
            .status
            .success()
    );
    std::fs::write(
        f.capture.join("payload/declass-0.1.0-aarch64-apple-darwin"),
        "modified after signing\n",
    )
    .unwrap();
    assert!(
        !f.install()
            .arg("--verify-only")
            .arg("--signers")
            .arg(&signers)
            .output()
            .unwrap()
            .status
            .success()
    );
}
