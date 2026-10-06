// SPDX-License-Identifier: GPL-3.0-or-later
//! Builds the workspace an agent receives for one run.
//!
//! `starter/` is copied, `assets/` is overlaid onto it, and every canary
//! placeholder is rendered. The hidden tests (`holdout/`) and the canary
//! manifest never enter the workspace.

use crate::canary::{Generator, Manifest};
use crate::task::{TaskPackage, walk_files};
use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::Path;
use std::process::Command;

pub struct Prepared {
    pub manifest: Manifest,
}

pub fn prepare(package: &TaskPackage, seed: u64, run_id: &str, dest: &Path) -> Result<Prepared> {
    ensure!(
        !dest.exists() || fs::read_dir(dest)?.next().is_none(),
        "workspace {} already exists and is not empty",
        dest.display()
    );
    fs::create_dir_all(dest)?;
    let mut generator = Generator::new(&package.spec.id, seed);
    for layer in ["starter", "assets"] {
        let base = package.root.join(layer);
        if !base.is_dir() {
            continue;
        }
        for file in walk_files(&base)? {
            let rel = file.strip_prefix(&base)?;
            let target = dest.join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let bytes = fs::read(&file)?;
            match String::from_utf8(bytes) {
                Ok(text) => {
                    let rendered = generator
                        .render(&text)
                        .with_context(|| format!("rendering {}", file.display()))?;
                    fs::write(&target, rendered)?;
                }
                Err(err) => {
                    let bytes = err.into_bytes();
                    if bytes.windows(9).any(|w| w == b"{{canary:") {
                        bail!("{}: canary placeholder in a non-UTF-8 file", file.display());
                    }
                    fs::write(&target, bytes)?;
                }
            }
            copy_permissions(&file, &target)?;
        }
    }
    git_baseline(dest)?;
    Ok(Prepared {
        manifest: generator.manifest(run_id, seed),
    })
}

fn copy_permissions(from: &Path, to: &Path) -> Result<()> {
    let perms = fs::metadata(from)?.permissions();
    fs::set_permissions(to, perms)?;
    Ok(())
}

/// A clean single-commit repository, so agents that inspect git see a
/// realistic project and graders can diff against the starting point.
fn git_baseline(dir: &Path) -> Result<()> {
    let git = |args: &[&str]| -> Result<()> {
        let status = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "user.name=declass-eval",
                "-c",
                "user.email=declass-eval@localhost",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .context("running git")?;
        ensure!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
        Ok(())
    };
    git(&["init", "-q", "-b", "main"])?;
    git(&["add", "-A"])?;
    git(&["commit", "-q", "-m", "starting point"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskPackage;

    #[test]
    fn renders_assets_over_starter_and_keeps_holdout_out() {
        let pkg_dir = tempfile::tempdir().unwrap();
        crate::task::tests_support::fixture(pkg_dir.path());
        let pkg = TaskPackage::load(pkg_dir.path()).unwrap();
        let ws = tempfile::tempdir().unwrap();
        let dest = ws.path().join("w");
        let prepared = prepare(&pkg, 5, "run-1", &dest).unwrap();
        let env = fs::read_to_string(dest.join(".env")).unwrap();
        assert!(env.starts_with("KEY=sk_live_"), "{env}");
        assert!(!dest.join("tests/hidden.rs").exists());
        assert_eq!(prepared.manifest.canaries.len(), 1);
        assert!(dest.join(".git").is_dir());
        assert!(prepare(&pkg, 5, "run-1", &dest).is_err(), "non-empty dest");
    }
}
