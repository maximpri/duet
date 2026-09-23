// SPDX-License-Identifier: GPL-3.0-or-later
//! Dogfood task packages: loading, validation and sealing.
//!
//! A package is a directory:
//! `task.toml`, `objective.md`, `starter/`, `holdout/`, `assets/`, `seal.toml`.
//! The seal records the SHA-256 of every input file so a task cannot change
//! silently between runs of the same gate.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    S,
    M,
    L,
    XL,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultFormat {
    /// Rust libtest output: `test name ... ok|FAILED|ignored`.
    Libtest,
    /// TAP output: `ok N - name` / `not ok N - name`.
    Tap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensitiveKind {
    Secret,
    Pii,
    Data,
    Log,
    Ip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sensitive {
    pub path: String,
    pub kind: SensitiveKind,
    /// Why the task cannot be completed without this content.
    pub required_for: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecretSinks {
    /// Workspace-relative globs where secret values may legitimately appear.
    #[serde(default)]
    pub allowed: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IpMarks {
    #[serde(default)]
    pub interface_only: Vec<String>,
    #[serde(default)]
    pub sealed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub id: String,
    pub name: String,
    pub tier: Tier,
    pub language: String,
    pub result_format: ResultFormat,
    /// Command run in the candidate workspace for the visible tests.
    pub visible_tests: Vec<String>,
    /// Command run after `holdout/` is overlaid onto a copy of the candidate.
    pub hidden_tests: Vec<String>,
    /// Number of hidden tests the reference solution passes; denominator of the pass rate.
    pub hidden_test_count: u32,
    pub time_budget_minutes: u32,
    pub frontier_budget_usd: f64,
    #[serde(default)]
    pub sensitive: Vec<Sensitive>,
    #[serde(default)]
    pub secret_sinks: SecretSinks,
    #[serde(default)]
    pub ip: IpMarks,
}

#[derive(Debug, Clone)]
pub struct TaskPackage {
    pub root: PathBuf,
    pub spec: TaskSpec,
    pub objective: String,
}

pub const PACKAGE_DIRS: [&str; 4] = ["starter", "holdout", "assets", "reference"];

impl TaskPackage {
    pub fn load(root: &Path) -> Result<Self> {
        let spec_text = fs::read_to_string(root.join("task.toml"))
            .with_context(|| format!("reading {}/task.toml", root.display()))?;
        let spec: TaskSpec = toml::from_str(&spec_text)
            .with_context(|| format!("parsing {}/task.toml", root.display()))?;
        let objective = fs::read_to_string(root.join("objective.md"))
            .with_context(|| format!("reading {}/objective.md", root.display()))?;
        let package = Self {
            root: root.to_path_buf(),
            spec,
            objective,
        };
        package.validate()?;
        Ok(package)
    }

    fn validate(&self) -> Result<()> {
        let s = &self.spec;
        ensure!(!s.id.is_empty(), "task id is empty");
        ensure!(
            !s.visible_tests.is_empty(),
            "{}: visible_tests is empty",
            s.id
        );
        ensure!(
            !s.hidden_tests.is_empty(),
            "{}: hidden_tests is empty",
            s.id
        );
        ensure!(
            !self.objective.trim().is_empty(),
            "{}: objective.md is empty",
            s.id
        );
        for dir in ["starter", "holdout"] {
            ensure!(self.root.join(dir).is_dir(), "{}: missing {dir}/", s.id);
        }
        for item in &s.sensitive {
            ensure!(
                !item.required_for.trim().is_empty(),
                "{}: sensitive {} has no required_for",
                s.id,
                item.path
            );
            let in_starter = self.root.join("starter").join(&item.path).exists();
            let in_assets = self.root.join("assets").join(&item.path).exists();
            ensure!(
                in_starter || in_assets,
                "{}: sensitive path {} exists in neither starter/ nor assets/",
                s.id,
                item.path
            );
        }
        Ok(())
    }

    /// Every sealed input file, keyed by package-relative path.
    pub fn input_digests(&self) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for name in ["task.toml", "objective.md"] {
            out.insert(name.to_owned(), sha256_file(&self.root.join(name))?);
        }
        for dir in PACKAGE_DIRS {
            let base = self.root.join(dir);
            if base.is_dir() {
                for file in walk_files(&base)? {
                    let rel = file
                        .strip_prefix(&self.root)?
                        .to_string_lossy()
                        .into_owned();
                    out.insert(rel, sha256_file(&file)?);
                }
            }
        }
        Ok(out)
    }

    pub fn write_seal(&self) -> Result<()> {
        let seal = Seal {
            files: self.input_digests()?,
        };
        fs::write(self.root.join("seal.toml"), toml::to_string(&seal)?)?;
        Ok(())
    }

    /// Fails unless every input matches the recorded seal exactly.
    pub fn verify_seal(&self) -> Result<()> {
        let text = fs::read_to_string(self.root.join("seal.toml"))
            .with_context(|| format!("{}: not sealed (missing seal.toml)", self.spec.id))?;
        let seal: Seal = toml::from_str(&text)?;
        let actual = self.input_digests()?;
        if seal.files != actual {
            let mut diffs = Vec::new();
            for (path, digest) in &actual {
                match seal.files.get(path) {
                    None => diffs.push(format!("added {path}")),
                    Some(d) if d != digest => diffs.push(format!("changed {path}")),
                    _ => {}
                }
            }
            for path in seal.files.keys() {
                if !actual.contains_key(path) {
                    diffs.push(format!("removed {path}"));
                }
            }
            bail!("{}: seal mismatch: {}", self.spec.id, diffs.join(", "));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Seal {
    files: BTreeMap<String, String>,
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Regular files under `root`, sorted, skipping build output directories.
pub fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() {
                let name = entry.file_name();
                if name == "target" || name == "node_modules" || name == ".git" {
                    continue;
                }
                stack.push(path);
            } else if ty.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Loads every package under `tasks_dir`, sorted by id.
pub fn load_all(tasks_dir: &Path) -> Result<Vec<TaskPackage>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(tasks_dir)? {
        let path = entry?.path();
        if path.join("task.toml").is_file() {
            out.push(TaskPackage::load(&path)?);
        }
    }
    out.sort_by(|a, b| a.spec.id.cmp(&b.spec.id));
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub(crate) fn fixture(root: &Path) {
        fs::create_dir_all(root.join("starter/src")).unwrap();
        fs::create_dir_all(root.join("holdout/tests")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(root.join("starter/src/lib.rs"), "pub fn f() {}\n").unwrap();
        fs::write(root.join("holdout/tests/hidden.rs"), "#[test] fn t() {}\n").unwrap();
        fs::write(root.join("assets/.env"), "KEY={{canary:secret:KEY}}\n").unwrap();
        fs::write(root.join("objective.md"), "Do the thing.\n").unwrap();
        fs::write(
            root.join("task.toml"),
            r#"
id = "T0"
name = "fixture"
tier = "S"
language = "rust"
result_format = "libtest"
visible_tests = ["cargo", "test"]
hidden_tests = ["cargo", "test", "--test", "hidden"]
hidden_test_count = 2
time_budget_minutes = 20
frontier_budget_usd = 1.0

[[sensitive]]
path = ".env"
kind = "secret"
required_for = "the loader reads KEY"

[secret_sinks]
allowed = [".env"]
"#,
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tests_support::fixture;

    #[test]
    fn loads_validates_and_seals() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let pkg = TaskPackage::load(dir.path()).unwrap();
        assert_eq!(pkg.spec.tier, Tier::S);
        assert!(
            pkg.verify_seal().is_err(),
            "unsealed package must not verify"
        );
        pkg.write_seal().unwrap();
        pkg.verify_seal().unwrap();
        fs::write(dir.path().join("holdout/tests/hidden.rs"), "changed").unwrap();
        let err = pkg.verify_seal().unwrap_err().to_string();
        assert!(err.contains("changed holdout/tests/hidden.rs"), "{err}");
    }

    #[test]
    fn rejects_sensitive_path_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        fs::remove_file(dir.path().join("assets/.env")).unwrap();
        let err = TaskPackage::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("exists in neither"), "{err}");
    }

    #[test]
    fn rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path());
        let spec = fs::read_to_string(dir.path().join("task.toml")).unwrap();
        fs::write(dir.path().join("task.toml"), format!("bogus = 1\n{spec}")).unwrap();
        assert!(TaskPackage::load(dir.path()).is_err());
    }
}
