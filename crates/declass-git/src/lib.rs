// SPDX-License-Identifier: GPL-3.0-or-later
//! The only place Declass spawns `git`.
//!
//! Every invocation clears the environment, pins the absolute git binary,
//! ignores global and system configuration, and disables hooks, fsmonitor,
//! optional locks and external diff/filter programs — so a repository cannot
//! make Declass execute code it did not choose.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod commit;
pub mod walk;
pub use commit::{Committed, Identity, user_config_files};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git is not installed")]
    Missing,
    #[error("git {args}: {message}")]
    Failed { args: String, message: String },
    #[error(transparent)]
    Fs(#[from] declass_fs::FsError),
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
        self.run_accepting(cwd, args, env, stdin, &[0])
    }

    /// [`Git::run`], treating each exit code in `ok` as success (`git diff
    /// --no-index` exits 1 when the files differ).
    pub fn run_accepting(
        &self,
        cwd: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
        ok: &[i32],
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
            .env("GIT_AUTHOR_NAME", "declass")
            .env("GIT_AUTHOR_EMAIL", "declass@localhost")
            .env("GIT_COMMITTER_NAME", "declass")
            .env("GIT_COMMITTER_EMAIL", "declass@localhost")
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
        if !out.status.code().is_some_and(|c| ok.contains(&c)) {
            return Err(GitError::Failed {
                args: args.join(" "),
                message: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(out.stdout)
    }

    /// Keeps `.declass/` out of the user's `git status` through the repository-local
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
        if current.lines().any(|l| l.trim() == ".declass/") {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| declass_fs::FsError::io("create", parent, e))?;
        }
        let sep = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&path, format!("{current}{sep}.declass/\n"))
            .map_err(|e| declass_fs::FsError::io("write", &path, e))?;
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
    /// Outside a git repository, the files a walk finds that the workspace's
    /// `.gitignore` and `.ignore` files do not exclude ([`walk::list`]).
    /// Reserved directories are excluded. There is no file-count cap.
    pub fn list_files(&self, workspace: &Path) -> Result<Vec<String>, GitError> {
        let listed = self.run(
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
        );
        let out = match listed {
            Ok(out) => out,
            Err(GitError::Failed { .. }) if !self.is_repository(workspace) => {
                return walk::list(workspace);
            }
            Err(e) => return Err(e),
        };
        let mut files: Vec<String> = out
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .filter(|p| !declass_fs::is_reserved(Path::new(p)))
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Working-tree diff of exactly `paths` (callers pass only public paths).
    pub fn diff_paths(&self, workspace: &Path, paths: &[String]) -> Result<String, GitError> {
        self.diff_paths_from(workspace, "HEAD", paths)
    }

    /// Working-tree diff of exactly `paths` against the commit `base`.
    pub fn diff_paths_from(
        &self,
        workspace: &Path,
        base: &str,
        paths: &[String],
    ) -> Result<String, GitError> {
        if paths.is_empty() {
            return Ok(String::new());
        }
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--end-of-options",
            base,
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        let out = self.run(workspace, &args, &[], None)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Unified diff between two files outside any repository's index: `before`
    /// (`None`: absent) and `after` (a missing file counts as absent). Reads
    /// only. Empty when they are equal; binary files give git's one-line note.
    pub fn diff_files(
        &self,
        cwd: &Path,
        before: Option<&Path>,
        after: &Path,
    ) -> Result<String, GitError> {
        let null = Path::new("/dev/null");
        let before = before.unwrap_or(null);
        let after = if after.is_file() { after } else { null };
        if before == null && after == null {
            return Ok(String::new());
        }
        let (b, a) = (before.to_string_lossy(), after.to_string_lossy());
        let out = self.run_accepting(
            cwd,
            &[
                "diff",
                "--no-index",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=3",
                "--",
                &b,
                &a,
            ],
            &[],
            None,
            &[0, 1],
        )?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

/// A private bare repository at `.declass/git` holding workspace snapshots, so a
/// run can be inspected and rolled back without touching the user's repository.
pub struct CheckpointStore {
    git: Git,
    workspace: PathBuf,
    store: PathBuf,
}

impl CheckpointStore {
    pub fn open(git: Git, workspace: &Path) -> Result<Self, GitError> {
        let store = workspace.join(".declass/git");
        git.exclude_state_dir(workspace)?;
        if !store.join("HEAD").exists() {
            declass_fs::private::ensure_private_dir(&store)?;
            git.run(
                &store,
                &[
                    "init",
                    "-q",
                    "--bare",
                    "--template=",
                    "--initial-branch=declass",
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

    /// Snapshots every non-reserved file and records it under `refs/declass/<name>`.
    pub fn snapshot(&self, name: &str, message: &str) -> Result<String, GitError> {
        let index = self.store.join("snapshot.index");
        let index_s = index.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&index);
        let env = self.env(&index_s);
        let files = self.git.list_files(&self.workspace)?;
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
            &["update-ref", &format!("refs/declass/{name}"), &commit],
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
        std::fs::create_dir_all(ws.join(".declass")).unwrap();
        std::fs::write(ws.join(".declass/state"), "x").unwrap();
        assert_eq!(
            git.list_files(&ws).unwrap(),
            vec![".gitignore", "a.txt", "new.rs"]
        );
    }

    #[test]
    fn lists_files_outside_a_repository_by_walking() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap();
        let git = Git::locate().unwrap();
        std::fs::write(ws.join("a.txt"), "one\n").unwrap();
        std::fs::write(ws.join(".gitignore"), "ignored.log\n").unwrap();
        std::fs::write(ws.join("ignored.log"), "x").unwrap();
        std::fs::create_dir_all(ws.join(".declass")).unwrap();
        std::fs::write(ws.join(".declass/state"), "x").unwrap();
        assert!(!git.is_repository(&ws));
        assert_eq!(git.list_files(&ws).unwrap(), vec![".gitignore", "a.txt"]);
        // A snapshot of a folder that is no repository holds the same files.
        let store = CheckpointStore::open(git.clone(), &ws).unwrap();
        let c = store.snapshot("runs/r1/0", "before").unwrap();
        assert_eq!(store.read(&c, "a.txt").unwrap().unwrap(), b"one\n");
        assert_eq!(store.read(&c, "ignored.log").unwrap(), None);
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
    fn diffs_two_files_read_only() {
        let (_d, ws, git) = repo();
        let elsewhere = tempfile::tempdir().unwrap();
        let before = elsewhere.path().join("before.txt");
        std::fs::write(&before, "one\ntwo\n").unwrap();
        std::fs::write(ws.join("a.txt"), "one\nthree\n").unwrap();
        let out = git
            .diff_files(&ws, Some(&before), &ws.join("a.txt"))
            .unwrap();
        assert!(out.contains("-two\n+three"), "{out}");
        assert!(
            git.diff_files(&ws, Some(&before), &before)
                .unwrap()
                .is_empty()
        );
        let created = git.diff_files(&ws, None, &ws.join("a.txt")).unwrap();
        assert!(created.contains("+one\n+three"), "{created}");
        let deleted = git
            .diff_files(&ws, Some(&before), &ws.join("gone.txt"))
            .unwrap();
        assert!(deleted.contains("-one\n-two"), "{deleted}");
        assert!(
            git.diff_files(&ws, None, &ws.join("gone.txt"))
                .unwrap()
                .is_empty()
        );
        // Nothing was staged or written.
        let status = git.run(&ws, &["status", "--porcelain"], &[], None).unwrap();
        assert_eq!(String::from_utf8_lossy(&status).trim(), "M a.txt");
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
