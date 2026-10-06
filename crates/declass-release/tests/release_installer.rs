// SPDX-License-Identifier: GPL-3.0-or-later
//! Real signatures, checksums, packaging and installation, with only the HTTPS
//! transport and host architecture substituted. No production signing key.
#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
                "release-test namespaces=\"declass-release\" {}",
                std::fs::read_to_string(key.with_extension("pub")).unwrap()
            ),
        );
        let payload = root.join("payload");
        let stem = format!("declass-0.1.0-{target}");
        script(
            &payload.join(&stem),
            &format!(
                "printf 'executed\\n' >>\"$DECLASS_TEST_RUN_MARKER\"\nprintf 'declass {reported_version}\\n'"
            ),
        );
        for name in [
            "declass-0.1.0.cdx.json",
            "LICENSE",
            "NOTICE",
            "LICENSES.md",
            "LICENSE.gitleaks",
            "NOTICE.gitleaks",
            "declass-0.1.0-source.tar.gz",
            "SOURCE.txt",
        ] {
            write(&payload.join(name), "signed fixture\n");
        }
        write(
            &payload.join("BUILDINFO.txt"),
            &format!("declass 0.1.0\ntarget {target}\n"),
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
        #[cfg(target_os = "macos")]
        checked(
            Command::new("xattr")
                .args(["-w", "org.declass.release-test", "metadata-fixture"])
                .arg(f.payload.join("LICENSE")),
        );
        f.sign_payload();
        checked(
            Command::new(tools().join("package-release.sh"))
                .env_remove("COPYFILE_DISABLE")
                .arg(&f.payload)
                .arg("--key")
                .arg(&f.key)
                .arg("--signers")
                .arg(&f.signers)
                .arg("--out")
                .arg(&f.assets),
        );
        let listing = checked(
            Command::new("tar")
                .arg("-tzf")
                .arg(f.assets.join(format!("{}.tar.gz", f.stem))),
        );
        assert!(
            String::from_utf8_lossy(&listing.stdout)
                .lines()
                .all(|name| !name.split('/').any(|part| part.starts_with("._"))),
            "release archive contains platform metadata"
        );
        let bin = f.root.join("transport");
        script(
            &bin.join("uname"),
            "case \"$1\" in -s) echo \"$DECLASS_TEST_OS\" ;; -m) echo \"$DECLASS_TEST_ARCH\" ;; *) exit 2 ;; esac",
        );
        script(
            &bin.join("sysctl"),
            "printf '%s\\n' \"${DECLASS_TEST_TRANSLATED:-0}\"",
        );
        for forbidden in ["python", "python3", "rustc", "cargo"] {
            script(
                &bin.join(forbidden),
                "printf 'forbidden dependency\\n' >>\"$DECLASS_TEST_FORBIDDEN\"; exit 99",
            );
        }
        script(
            &bin.join("curl"),
            r#"destination=''
lookup=false
writeout=''
secure=false
secure_redirect=false
while [ $# -gt 0 ]; do
    case "$1" in
    --output) destination="$2"; shift 2 ;;
    --head) lookup=true; shift ;;
    --write-out) writeout="$2"; shift 2 ;;
    --proto) [ "$2" = '=https' ]; secure=true; shift 2 ;;
    --proto-redir) [ "$2" = '=https' ]; secure_redirect=true; shift 2 ;;
    --tlsv1.2 | --fail | --silent | --show-error | --location) shift ;;
    --max-redirs | --connect-timeout | --max-time) shift 2 ;;
    https://github.com/maximpri/duet/releases/download/v0.1.0/*) url="$1"; shift ;;
    https://github.com/maximpri/duet/releases/latest) url="$1"; shift ;;
    *) exit 2 ;;
    esac
done
$secure && $secure_redirect
printf '%s\n' "$url" >> "$DECLASS_TEST_DOWNLOADS"
if $lookup; then
    [ "$url" = https://github.com/maximpri/duet/releases/latest ]
    [ "$destination" = /dev/null ] && [ "$writeout" = '%{url_effective}' ]
    [ "${DECLASS_TEST_LATEST_FAILURE:-false}" != true ] || exit 22
    printf '%s' "${DECLASS_TEST_LATEST_URL:-https://github.com/maximpri/duet/releases/tag/v0.1.0}"
    exit 0
fi
name=${url##*/}
[ "$name" != "${DECLASS_TEST_FAIL_DOWNLOAD:-}" ] || exit 22
cp "$DECLASS_TEST_ASSETS/$name" "$destination""#,
        );
        f
    }
    fn sign(&self, path: &Path) {
        let sig = PathBuf::from(format!("{}.sig", path.display()));
        let _ = std::fs::remove_file(sig);
        checked(
            Command::new("ssh-keygen")
                .args(["-Y", "sign", "-n", "declass-release", "-f"])
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
    fn package_unsigned(&self) {
        self.sign_payload();
        std::fs::remove_file(self.payload.join("SHA256SUMS.sig")).unwrap();
        self.repack_unsigned_outer();
    }
    fn repack_unsigned_outer(&self) {
        let archive = self.assets.join(format!("{}-unsigned.tar.gz", self.stem));
        checked(
            Command::new("tar")
                .env("COPYFILE_DISABLE", "1")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&self.payload)
                .arg("."),
        );
        write(
            &self
                .assets
                .join(format!("{}-unsigned.SHA256SUMS", self.stem)),
            &format!(
                "{:x}  {}-unsigned.tar.gz\n",
                Sha256::digest(std::fs::read(archive).unwrap()),
                self.stem
            ),
        );
    }
    fn installer(&self, os: &str, arch: &str) -> Command {
        let bin = self.root.join("install/bin");
        write(&bin.join("declass"), "previous installation\n");
        let mut command =
            self.configured_command(Command::new(tools().join("install-release.sh")), os, arch);
        command.args(["0.1.0", "--signers"]).arg(&self.signers);
        command
    }
    fn configured_command(&self, mut command: Command, os: &str, arch: &str) -> Command {
        let path = std::env::join_paths(
            std::iter::once(self.root.join("transport"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        command
            .env("PATH", path)
            .env("DECLASS_TEST_OS", os)
            .env("DECLASS_TEST_ARCH", arch)
            .env("DECLASS_TEST_ASSETS", &self.assets)
            .env("DECLASS_INSTALL_DIR", self.root.join("install/bin"))
            .env("DECLASS_DATA_DIR", self.root.join("install/share"))
            .env("DECLASS_TEST_RUN_MARKER", self.root.join("executed"))
            .env("DECLASS_CONFIG_HOME", self.root.join("config"))
            .env("HOME", self.root.join("home"))
            .env("DECLASS_TEST_DOWNLOADS", self.root.join("downloads"))
            .env("DECLASS_TEST_FORBIDDEN", self.root.join("forbidden"));
        command
    }
    fn bootstrap(&self, os: &str, arch: &str) -> Command {
        write(
            &self.root.join("install/bin/declass"),
            "previous installation\n",
        );
        let mut command = self.configured_command(Command::new("bash"), os, arch);
        command
            .args(["-s", "--"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn run_bootstrap(&self, mut command: Command) -> Output {
        let source = std::fs::read(tools().join("../install.sh")).unwrap();
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(&source).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            !self.root.join("forbidden").exists(),
            "bootstrap invoked Python/Rust"
        );
        output
    }
    fn assert_previous_retained(&self) {
        assert_eq!(
            std::fs::read_to_string(self.root.join("install/bin/declass")).unwrap(),
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
            std::fs::read(f.root.join("install/bin/declass")).unwrap(),
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
            "declass-0.1.0-source.tar.gz",
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
                command.env("DECLASS_TEST_FAIL_DOWNLOAD", format!("{}.tar.gz", f.stem));
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
                        "other namespaces=\"declass-release\" {}",
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
                command.env("DECLASS_TEST_OS", "Unsupported");
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
        .env("DECLASS_DATA_DIR", blocked)
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

#[test]
fn piped_bootstrap_resolves_latest_once_and_uses_default_trust() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    write(
        &f.root.join("config/allowed_signers"),
        &std::fs::read_to_string(&f.signers).unwrap(),
    );
    let mut command = f.bootstrap("Linux", "aarch64");
    command.env("SHELL", "/bin/bash");
    let result = f.run_bootstrap(command);
    assert!(result.status.success(), "{}", describe(&result));
    let calls = std::fs::read_to_string(f.root.join("downloads")).unwrap();
    let calls: Vec<_> = calls.lines().collect();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0], "https://github.com/maximpri/duet/releases/latest");
    assert!(
        calls[1..].iter().all(
            |url| url.starts_with("https://github.com/maximpri/duet/releases/download/v0.1.0/")
        )
    );
    assert!(f.root.join("executed").exists());
    let records = std::fs::read_dir(f.root.join("install/share/releases"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(records.join("declass-0.1.0-source.tar.gz").is_file());
    assert!(records.join("SHA256SUMS.sig").is_file());
    let rc = std::fs::read_to_string(f.root.join("home/.bashrc")).unwrap();
    assert!(rc.contains(&path_line(&f)), "{rc}");
    assert!(!f.root.join("home/.profile").exists());
    assert!(!f.root.join("home/.zshrc").exists());
}

fn path_line(f: &Fixture) -> String {
    format!(
        "export PATH=\"{}:$PATH\" # added by the declass installer",
        f.root.join("install/bin").display()
    )
}

#[test]
fn path_is_added_to_the_shell_profile_once_and_never_when_declined() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    for _ in 0..2 {
        let out = f
            .installer("Linux", "aarch64")
            .env("SHELL", "/usr/bin/zsh")
            .env_remove("ZDOTDIR")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        assert!(describe(&out).contains("Open a new terminal"));
    }
    let rc = std::fs::read_to_string(f.root.join("home/.zshrc")).unwrap();
    assert_eq!(rc.matches(&path_line(&f)).count(), 1, "{rc}");

    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    let out = f
        .installer("Linux", "aarch64")
        .env("SHELL", "/usr/bin/fish")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let fish =
        std::fs::read_to_string(f.root.join("home/.config/fish/conf.d/declass.fish")).unwrap();
    assert!(fish.contains("fish_add_path -g"), "{fish}");

    for decline in [&["--no-modify-path"][..], &[]] {
        let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
        let mut command = f.installer("Linux", "aarch64");
        command.args(decline).env("SHELL", "/bin/bash");
        if decline.is_empty() {
            command.env("DECLASS_NO_MODIFY_PATH", "1");
        }
        let out = command.output().unwrap();
        assert!(out.status.success(), "{}", describe(&out));
        assert!(describe(&out).contains("no shell profile was changed"));
        assert!(
            !f.root.join("home").exists()
                || std::fs::read_dir(f.root.join("home"))
                    .unwrap()
                    .next()
                    .is_none()
        );
    }
}

#[test]
fn piped_bootstrap_uses_native_arm64_under_rosetta_and_explicit_version() {
    let f = Fixture::new("aarch64-apple-darwin", "0.1.0");
    let mut command = f.bootstrap("Darwin", "x86_64");
    command
        .args(["--version", "0.1.0", "--signers"])
        .arg(&f.signers)
        .env("DECLASS_TEST_TRANSLATED", "1");
    let result = f.run_bootstrap(command);
    assert!(result.status.success(), "{}", describe(&result));
    let calls = std::fs::read_to_string(f.root.join("downloads")).unwrap();
    assert_eq!(calls.lines().count(), 3);
    assert!(!calls.contains("/latest"));
    assert!(!calls.contains("x86_64"));
    assert!(calls.contains("aarch64-apple-darwin"));
}

#[test]
fn bootstrap_without_a_trust_file_installs_the_unsigned_preview_with_a_notice() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    f.package_unsigned();
    let result = f.run_bootstrap(f.bootstrap("Linux", "aarch64"));
    assert!(result.status.success(), "{}", describe(&result));
    assert!(describe(&result).contains("not publisher identity"));
    let calls = std::fs::read_to_string(f.root.join("downloads")).unwrap();
    assert!(calls.lines().skip(1).all(|url| url.contains("-unsigned.")));
    assert!(f.root.join("executed").exists());
}

#[test]
fn bootstrap_with_a_named_but_missing_trust_file_makes_no_requests_or_install_changes() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    f.package_unsigned();
    let mut command = f.bootstrap("Linux", "aarch64");
    command.arg("--signers").arg(f.root.join("missing_signers"));
    let result = f.run_bootstrap(command);
    assert!(!result.status.success());
    assert!(describe(&result).contains("independent trusted channel"));
    assert!(!f.root.join("downloads").exists());
    f.assert_not_executed();
    f.assert_previous_retained();
}

#[test]
fn absent_or_invalid_latest_never_downloads_a_payload_or_changes_installation() {
    for destination in [
        "network-failure",
        "https://github.com/maximpri/duet/releases",
        "https://github.com/maximpri/duet/releases/tag/v0.1.0-rc.1",
        "https://github.com/maximpri/duet/releases/tag/vnested/v0.1.0",
        "https://other.example/releases/tag/v0.1.0",
        "http://github.com/maximpri/duet/releases/tag/v0.1.0",
    ] {
        let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
        let mut command = f.bootstrap("Linux", "aarch64");
        command.arg("--signers").arg(&f.signers);
        if destination == "network-failure" {
            command.env("DECLASS_TEST_LATEST_FAILURE", "true");
        } else {
            command.env("DECLASS_TEST_LATEST_URL", destination);
        }
        let result = f.run_bootstrap(command);
        assert!(!result.status.success(), "accepted {destination}");
        assert!(describe(&result).contains("release"));
        let calls = std::fs::read_to_string(f.root.join("downloads")).unwrap();
        assert_eq!(calls.lines().count(), 1);
        f.assert_previous_retained();
        f.assert_not_executed();
    }
}

#[test]
fn unsigned_preview_with_allow_unsigned_retains_source() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    f.package_unsigned();
    let mut command = f.bootstrap("Linux", "aarch64");
    command.arg("--allow-unsigned"); // No trusted key/config is provided.
    let result = f.run_bootstrap(command);
    assert!(result.status.success(), "{}", describe(&result));
    assert!(describe(&result).contains("not publisher identity"));
    let calls = std::fs::read_to_string(f.root.join("downloads")).unwrap();
    assert_eq!(calls.lines().count(), 3); // One lookup, manifest, archive; no key/signature.
    assert!(calls.lines().skip(1).all(|url| url.contains("-unsigned.")));
    let records = std::fs::read_dir(f.root.join("install/share/releases"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(records.join("declass-0.1.0-source.tar.gz").is_file());
    assert!(records.join("LICENSES.md").is_file());
    assert!(!records.join("SHA256SUMS.sig").exists());
}

#[test]
fn unsigned_corruption_extra_files_and_signatures_refuse_before_execution() {
    for fault in ["archive", "inner", "extra", "link", "signature"] {
        let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
        f.package_unsigned();
        match fault {
            "archive" => write(
                &f.assets.join(format!("{}-unsigned.tar.gz", f.stem)),
                "changed",
            ),
            "inner" => {
                write(&f.payload.join(&f.stem), "changed binary");
                f.repack_unsigned_outer();
            }
            "extra" => {
                write(&f.payload.join("unlisted"), "extra");
                f.repack_unsigned_outer();
            }
            "link" => {
                std::fs::remove_file(f.payload.join(&f.stem)).unwrap();
                std::os::unix::fs::symlink(f.root.join("outside"), f.payload.join(&f.stem))
                    .unwrap();
                f.repack_unsigned_outer();
            }
            "signature" => {
                write(&f.payload.join("SHA256SUMS.sig"), "untrusted signature");
                f.repack_unsigned_outer();
            }
            _ => unreachable!(),
        }
        let mut command = f.bootstrap("Linux", "aarch64");
        command.args(["--allow-unsigned", "0.1.0"]);
        let result = f.run_bootstrap(command);
        assert!(!result.status.success(), "accepted {fault}");
        f.assert_previous_retained();
        f.assert_not_executed();
        assert!(!f.root.join("outside").exists());
    }
}

#[test]
fn bad_signature_never_falls_back_to_available_unsigned_preview() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    f.package_unsigned();
    write(
        &f.assets.join(format!("{}.SHA256SUMS.sig", f.stem)),
        "invalid signature",
    );
    let mut command = f.bootstrap("Linux", "aarch64");
    command.args(["0.1.0", "--signers"]).arg(&f.signers);
    let result = f.run_bootstrap(command);
    assert!(!result.status.success());
    let calls = std::fs::read_to_string(f.root.join("downloads")).unwrap();
    assert!(!calls.contains("-unsigned"));
    f.assert_previous_retained();
    f.assert_not_executed();
}

#[test]
fn incomplete_piped_bootstrap_cannot_begin_downloads_or_installation() {
    let f = Fixture::new("aarch64-unknown-linux-gnu", "0.1.0");
    let source = std::fs::read(tools().join("../install.sh")).unwrap();
    let mut command = f.bootstrap("Linux", "aarch64");
    command.arg("--allow-unsigned");
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&source[..source.len() / 2])
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    assert!(!f.root.join("downloads").exists());
    f.assert_previous_retained();
    f.assert_not_executed();
}
