// SPDX-License-Identifier: GPL-3.0-or-later
//! The only place Duet spawns `git`.
//!
//! Every invocation clears the environment, pins the absolute git binary,
//! ignores global and system configuration, and disables hooks, fsmonitor,
//! optional locks and external diff/filter programs — so a repository cannot
//! make Duet execute code it did not choose.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git is not installed")]
    Missing,
    #[error("git {args}: {message}")]
    Failed { args: String, message: String },
    #[error(transparent)]
    Fs(#[from] duet_fs::FsError),
}

#[derive(Debug, Clone)]
pub struct Git {
    binary: PathBuf,
}

const HARDENING: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.untrackedCache=false",
    "-c",
    "diff.external=",
    "-c",
    "core.pager=cat",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "protocol.allow=never",
];

impl Git {
    /// Resolves git once to an absolute, canonical path.
    pub fn locate() -> Result<Self, GitError> {
        let candidates = [
            "/usr/bin/git",
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
        ];
        let binary = candidates
            .iter()
            .map(Path::new)
            .find(|p| p.is_file())
            .ok_or(GitError::Missing)?
            .canonicalize()
            .map_err(|_| GitError::Missing)?;
        Ok(Self { binary })
    }

    /// The resolved git binary.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Runs git with hardening flags; `env` adds variables after the environment is cleared.
    pub fn run(
        &self,
        cwd: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>, GitError> {
        let mut cmd = Command::new(&self.binary);
        cmd.args(HARDENING)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "duet")
            .env("GIT_AUTHOR_EMAIL", "duet@localhost")
            .env("GIT_COMMITTER_NAME", "duet")
            .env("GIT_COMMITTER_EMAIL", "duet@localhost")
            .envs(env.iter().copied())
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| GitError::Failed {
            args: args.join(" "),
            message: e.to_string(),
        })?;
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            use std::io::Write;
            let _ = pipe.write_all(input);
        }
        let out = child.wait_with_output().map_err(|e| GitError::Failed {
            args: args.join(" "),
            message: e.to_string(),
        })?;
        if !out.status.success() {
            return Err(GitError::Failed {
                args: args.join(" "),
                message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(out.stdout)
    }

    /// Keeps `.duet/` out of the user's `git status` through the repository-local
    /// exclude file, which is never committed.
    pub fn exclude_state_dir(&self, workspace: &Path) -> Result<(), GitError> {
        let Ok(out) = self.run(
            workspace,
            &["rev-parse", "--git-path", "info/exclude"],
            &[],
            None,
        ) else {
            return Ok(());
        };
        let rel = String::from_utf8_lossy(&out).trim().to_owned();
        let path = workspace.join(rel);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current.lines().any(|l| l.trim() == ".duet/") {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| duet_fs::FsError::io("create", parent, e))?;
        }
        let sep = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&path, format!("{current}{sep}.duet/\n"))
            .map_err(|e| duet_fs::FsError::io("write", &path, e))?;
        Ok(())
    }

    pub fn is_repository(&self, workspace: &Path) -> bool {
        self.run(
            workspace,
            &["rev-parse", "--is-inside-work-tree"],
            &[],
            None,
        )
        .is_ok()
    }

    /// Tracked and untracked-but-not-ignored files, relative to the workspace.
    /// Reserved directories are excluded. There is no file-count cap.
    pub fn list_files(&self, workspace: &Path) -> Result<Vec<String>, GitError> {
        let out = self.run(
            workspace,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--deduplicate",
            ],
            &[],
            None,
        )?;
        let mut files: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .filter(|p| !duet_fs::is_reserved(Path::new(p)))
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Working-tree diff of exactly `paths` (callers pass only public paths).
    pub fn diff_paths(&self, workspace: &Path, paths: &[String]) -> Result<String, GitError> {
        if paths.is_empty() {
            return Ok(String::new());
        }
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        let out = self.run(workspace, &args, &[], None)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

/// A private bare repository at `.duet/git` holding workspace snapshots, so a
/// run can be inspected and rolled back without touching the user's repository.
pub struct CheckpointStore {
    git: Git,
    workspace: PathBuf,
    store: PathBuf,
}

impl CheckpointStore {
    pub fn open(git: Git, workspace: &Path) -> Result<Self, GitError> {
        let store = workspace.join(".duet/git");
        git.exclude_state_dir(workspace)?;
        if !store.join("HEAD").exists() {
            duet_fs::private::ensure_private_dir(&store)?;
            git.run(
                &store,
                &[
                    "init",
                    "-q",
                    "--bare",
                    "--template=",
                    "--initial-branch=duet",
                ],
                &[],
                None,
            )?;
        }
        Ok(Self {
            git,
            workspace: workspace.to_path_buf(),
            store,
        })
    }

    fn env<'a>(&'a self, index: &'a str) -> Vec<(&'a str, &'a str)> {
        vec![
            ("GIT_DIR", self.store.to_str().unwrap_or_default()),
            ("GIT_WORK_TREE", self.workspace.to_str().unwrap_or_default()),
            ("GIT_INDEX_FILE", index),
        ]
    }

    /// Snapshots every non-reserved file and records it under `refs/duet/<name>`.
    pub fn snapshot(&self, name: &str, message: &str) -> Result<String, GitError> {
        let index = self.store.join("snapshot.index");
        let index_s = index.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&index);
        let env = self.env(&index_s);
        let files = self
            .git
            .list_files(&self.workspace)
            .or_else(|_| walk(&self.workspace))?;
        let mut list = Vec::new();
        for f in &files {
            list.extend_from_slice(f.as_bytes());
            list.push(0);
        }
        self.git.run(
            &self.workspace,
            &["add", "--pathspec-from-file=-", "--pathspec-file-nul", "-f"],
            &env,
            Some(&list),
        )?;
        let tree = self.git.run(&self.workspace, &["write-tree"], &env, None)?;
        let tree = String::from_utf8_lossy(&tree).trim().to_owned();
        let commit = self.git.run(
            &self.workspace,
            &["commit-tree", &tree, "-m", message],
            &env,
            None,
        )?;
        let commit = String::from_utf8_lossy(&commit).trim().to_owned();
        self.git.run(
            &self.workspace,
            &["update-ref", &format!("refs/duet/{name}"), &commit],
            &env,
            None,
        )?;
        let _ = std::fs::remove_file(&index);
        Ok(commit)
    }

    /// Content of `path` in snapshot `commit`, if present.
    pub fn read(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>, GitError> {
        let index = self.store.join("read.index").to_string_lossy().into_owned();
        match self.git.run(
            &self.workspace,
            &["show", &format!("{commit}:{path}")],
            &self.env(&index),
            None,
        ) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(GitError::Failed { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

fn walk(root: &Path) -> Result<Vec<String>, GitError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| duet_fs::FsError::io("list", &dir, e))?
            .flatten()
        {
            let p = entry.path();
            let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
            if duet_fs::is_reserved(&rel)
                || rel.starts_with("target")
                || rel.starts_with("node_modules")
            {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => out.push(rel.to_string_lossy().into_owned()),
                _ => {}
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, PathBuf, Git) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap();
        let git = Git::locate().unwrap();
        git.run(&ws, &["init", "-q", "-b", "main"], &[], None)
            .unwrap();
        std::fs::write(ws.join("a.txt"), "one\n").unwrap();
        std::fs::write(ws.join(".gitignore"), "ignored.log\n").unwrap();
        git.run(&ws, &["add", "-A"], &[], None).unwrap();
        git.run(&ws, &["commit", "-q", "-m", "init"], &[], None)
            .unwrap();
        (d, ws, git)
    }

    #[test]
    fn hostile_repository_config_cannot_run_hooks_or_fsmonitor() {
        let (_d, ws, git) = repo();
        let marker = ws.join("pwned");
        let hook = format!("#!/bin/sh\ntouch {}\n", marker.display());
        std::fs::write(ws.join(".git/hooks/post-commit"), &hook).unwrap();
        std::fs::write(ws.join("evil.sh"), &hook).unwrap();
        std::fs::set_permissions(
            ws.join(".git/hooks/post-commit"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        std::fs::set_permissions(
            ws.join("evil.sh"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        git.run(
            &ws,
            &[
                "config",
                "core.fsmonitor",
                ws.join("evil.sh").to_str().unwrap(),
            ],
            &[],
            None,
        )
        .unwrap();
        std::fs::write(ws.join("a.txt"), "two\n").unwrap();
        git.list_files(&ws).unwrap();
        git.run(&ws, &["commit", "-qam", "x"], &[], None).unwrap();
        assert!(!marker.exists(), "repository-controlled code ran");
    }

    #[test]
    fn lists_files_without_reserved_or_ignored() {
        let (_d, ws, git) = repo();
        std::fs::write(ws.join("new.rs"), "x").unwrap();
        std::fs::write(ws.join("ignored.log"), "x").unwrap();
        std::fs::create_dir_all(ws.join(".duet")).unwrap();
        std::fs::write(ws.join(".duet/state"), "x").unwrap();
        assert_eq!(
            git.list_files(&ws).unwrap(),
            vec![".gitignore", "a.txt", "new.rs"]
        );
    }

    #[test]
    fn diffs_only_requested_paths() {
        let (_d, ws, git) = repo();
        std::fs::write(ws.join("a.txt"), "changed\n").unwrap();
        let d = git.diff_paths(&ws, &["a.txt".into()]).unwrap();
        assert!(d.contains("+changed"));
        assert!(
            git.diff_paths(&ws, &[".gitignore".into()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn snapshots_are_private_and_readable() {
        let (_d, ws, git) = repo();
        let store = CheckpointStore::open(git.clone(), &ws).unwrap();
        let c1 = store.snapshot("runs/r1/0", "before").unwrap();
        std::fs::write(ws.join("a.txt"), "edited\n").unwrap();
        store.snapshot("runs/r1/1", "after").unwrap();
        assert_eq!(store.read(&c1, "a.txt").unwrap().unwrap(), b"one\n");
        assert_eq!(store.read(&c1, "missing").unwrap(), None);
        // The user's repository is untouched: no new commits, refs or staged changes.
        let log = git.run(&ws, &["log", "--oneline"], &[], None).unwrap();
        assert_eq!(String::from_utf8_lossy(&log).lines().count(), 1);
        let status = git.run(&ws, &["status", "--porcelain"], &[], None).unwrap();
        assert_eq!(String::from_utf8_lossy(&status).trim(), "M a.txt");
    }
}
