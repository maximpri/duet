// SPDX-License-Identifier: GPL-3.0-or-later
//! Exercise the CI entry point in disposable repositories. Prerequisite stubs
//! record any attempted work, so rejected input cannot quietly start a build.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn executable(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Fixture {
    _temporary: tempfile::TempDir,
    repo: PathBuf,
    bin: PathBuf,
    out: PathBuf,
    cache: PathBuf,
    activity: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let repo = root.join("checkout");
        let bin = root.join("prerequisites");
        fs::create_dir_all(repo.join("tools")).unwrap();
        fs::create_dir(&bin).unwrap();
        fs::copy(repository().join("cicd.sh"), repo.join("cicd.sh")).unwrap();
        fs::copy(
            repository().join("tools/build-candidate.sh"),
            repo.join("tools/build-candidate.sh"),
        )
        .unwrap();
        fs::write(
            repo.join("Cargo.toml"),
            "[workspace.package]\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        for tool in ["gate.sh", "source-release.sh"] {
            executable(
                &repo.join("tools").join(tool),
                "printf 'unexpected-source-or-gate\\n' >> \"$CICD_TEST_ACTIVITY\"\nexit 97",
            );
        }
        executable(
            &bin.join("rustc"),
            "case \"$*\" in\n-vV) printf 'host: x86_64-unknown-linux-gnu\\n' ;;\n-V) printf 'rustc fixture\\n' ;;\n*) exit 96 ;;\nesac",
        );
        for tool in ["cargo", "docker"] {
            executable(
                &bin.join(tool),
                "printf 'unexpected-build-or-container\\n' >> \"$CICD_TEST_ACTIVITY\"\nexit 97",
            );
        }
        let f = Self {
            repo,
            bin,
            out: root.join("output"),
            cache: root.join("cache"),
            activity: root.join("activity"),
            _temporary: temporary,
        };
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=CI fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "Fixture",
            ],
        ] {
            let result = Command::new("git")
                .args(args)
                .current_dir(&f.repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(result.status.success(), "{}", text(&result));
        }
        f
    }

    fn command(&self, script: &str) -> Command {
        let mut command = Command::new("bash");
        let path = std::env::join_paths(std::iter::once(self.bin.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        command
            .arg(self.repo.join(script))
            .current_dir(&self.repo)
            .env("PATH", path)
            .env("CICD_TEST_ACTIVITY", &self.activity)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn build(&self, targets: &str) -> Output {
        self.command("cicd.sh")
            .args(["build", "--targets", targets, "--out"])
            .arg(&self.out)
            .arg("--cache-dir")
            .arg(&self.cache)
            .output()
            .unwrap()
    }

    fn refused_without_work(&self, result: Output, reason: &str) {
        assert!(!result.status.success(), "accepted invalid build request");
        assert!(text(&result).contains(reason), "{}", text(&result));
        assert!(
            !self.activity.exists(),
            "gate, packaging, cargo or Docker ran"
        );
        assert!(!self.cache.exists(), "preflight created build cache");
    }
}

#[test]
fn dirty_checkouts_refuse_before_packaging_or_building() {
    for name in ["Cargo.toml", "untracked.txt"] {
        let f = Fixture::new();
        fs::write(f.repo.join(name), "uncommitted content\n").unwrap();
        f.refused_without_work(f.build("host"), "clean, committed checkout");
        assert!(!f.out.exists());
        assert_eq!(
            fs::read_to_string(f.repo.join(name)).unwrap(),
            "uncommitted content\n"
        );
    }
}

#[test]
fn unsupported_duplicate_and_empty_targets_never_start_work() {
    for (targets, reason) in [
        ("not-a-target", "unsupported target"),
        (
            "x86_64-unknown-linux-gnu,x86_64-unknown-linux-gnu",
            "duplicate target",
        ),
        ("", "requires a value"),
        (",x86_64-unknown-linux-gnu", "empty target"),
        ("x86_64-unknown-linux-gnu,", "empty target"),
        (
            "x86_64-unknown-linux-gnu,,aarch64-unknown-linux-gnu",
            "empty target",
        ),
    ] {
        let f = Fixture::new();
        f.refused_without_work(f.build(targets), reason);
        assert!(!f.out.exists(), "created output for {targets:?}");
    }
}

#[test]
fn existing_output_is_preserved_without_starting_work() {
    for directory in [false, true] {
        let f = Fixture::new();
        let sentinel = if directory {
            fs::create_dir(&f.out).unwrap();
            f.out.join("valuable.txt")
        } else {
            f.out.clone()
        };
        fs::write(&sentinel, b"keep these exact bytes\0\xff").unwrap();
        f.refused_without_work(f.build("host"), "output already exists");
        assert_eq!(
            fs::read(&sentinel).unwrap(),
            b"keep these exact bytes\0\xff"
        );
        if directory {
            assert_eq!(fs::read_dir(&f.out).unwrap().count(), 1);
        }
    }
}

#[test]
fn dangling_output_symlink_is_not_followed_into_a_new_build() {
    let f = Fixture::new();
    let destination = f.out.with_file_name("missing-destination");
    symlink(&destination, &f.out).unwrap();
    let result = f.build("host");
    assert!(
        !result.status.success(),
        "accepted an existing output symlink"
    );
    assert!(!f.activity.exists(), "work started through output symlink");
    assert!(!f.cache.exists());
    assert!(!destination.exists());
    assert_eq!(fs::read_link(&f.out).unwrap(), destination);
}

#[test]
fn unsigned_worker_rejects_wrong_binary_version_before_help_or_sbom() {
    let f = Fixture::new();
    let target = "x86_64-unknown-linux-gnu";
    let target_dir = f.out.with_file_name("compiled");
    let binary_dir = target_dir.join(target).join("release");
    fs::create_dir_all(&binary_dir).unwrap();
    fs::create_dir(&f.out).unwrap();
    executable(
        &binary_dir.join("declass"),
        "case \"$1\" in\n--version) printf 'declass 9.9.9\\n' ;;\n*) printf 'unexpected-binary-execution\\n' >> \"$CICD_TEST_ACTIVITY\"; exit 98 ;;\nesac",
    );
    executable(
        &f.bin.join("cargo"),
        "case \"$1\" in\n-V) printf 'cargo fixture\\n' ;;\nbuild) printf 'build\\n' >> \"$CICD_TEST_ACTIVITY\" ;;\n*) printf 'unexpected-sbom-or-fetch\\n' >> \"$CICD_TEST_ACTIVITY\"; exit 99 ;;\nesac",
    );
    let result = f
        .command("tools/build-candidate.sh")
        .arg(&f.repo)
        .args([target, "1.2.3", "0123456789abcdef", "1700000000"])
        .arg(&f.out)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        text(&result).contains("unexpected binary version"),
        "{}",
        text(&result)
    );
    assert_eq!(fs::read_to_string(&f.activity).unwrap(), "build\n");
    assert!(!f.out.join("BUILDINFO.txt").exists());
    assert!(!f.out.join("declass-1.2.3.cdx.json").exists());
}
