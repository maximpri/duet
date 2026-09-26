// SPDX-License-Identifier: GPL-3.0-or-later
//! What a run (or session) changed in a workspace that is not a git
//! repository.
//!
//! With no commit to compare with, the run's write journal is the baseline:
//! each file the run wrote through its tools (`edit_file`, `write_file`,
//! `rename`, `edit_protected`, a sub-agent's writes) is compared with its
//! content before the run's first write to it, with Duet's hardened git
//! helper (`git diff --no-index`, which needs no repository). Files changed
//! only by commands are not journaled and do not appear. Everything here
//! reads the workspace; the only file written is a private scratch copy in
//! the run directory, removed again.

use crate::journal;
use duet_boundary::view::Presenter;
use duet_git::Git;
use std::path::{Path, PathBuf};

/// Largest file compared line by line.
const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;

/// What `diff` and `/diff` say about how they compare outside a repository.
pub const NOTE: &str = "This folder is not a git repository: the files written with the file tools \
are compared with their content before this run; changes made only by commands are not shown.";

/// One file the run changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    /// A git-style section: `diff --git a/<path> b/<path>`, the `---` and
    /// `+++` lines (relative to the workspace, `/dev/null` for a created or
    /// deleted file) and the hunks.
    pub diff: String,
}

/// The files the run in `run_dir` wrote that differ now from their content
/// before its first write to them, in the order they were first written.
pub fn written(git: &Git, workspace: &Path, run_dir: &Path) -> Vec<Change> {
    let mut out = Vec::new();
    for w in journal::written(run_dir) {
        let path = w.path.to_string_lossy().replace('\\', "/");
        let header = format!("diff --git a/{path} b/{path}\n");
        let before = match &w.before {
            Some(p) => match std::fs::read(p) {
                Ok(b) => Some(b),
                Err(e) => {
                    out.push(Change {
                        path: w.path.clone(),
                        diff: format!(
                            "{header}[not shown: its earlier content cannot be read: {e}]\n"
                        ),
                    });
                    continue;
                }
            },
            None => None,
        };
        let now = match duet_fs::read_optional(workspace, &w.path, MAX_DIFF_BYTES) {
            Ok(now) => now,
            Err(e) => {
                out.push(Change {
                    path: w.path.clone(),
                    diff: format!("{header}[not shown: {e}]\n"),
                });
                continue;
            }
        };
        if now == before {
            continue;
        }
        let raw = match compare(git, workspace, run_dir, w.before.as_deref(), now.as_deref()) {
            Ok(raw) => raw,
            Err(e) => {
                out.push(Change {
                    path: w.path.clone(),
                    diff: format!("{header}[not shown: {e}]\n"),
                });
                continue;
            }
        };
        out.push(Change {
            path: w.path.clone(),
            diff: relabel(&raw, &path, before.is_none(), now.is_none()),
        });
    }
    out
}

/// Git's unified diff of `before` (a saved file) and `now` (content read
/// from the workspace), through a private copy of `now` in `run_dir`: the
/// workspace path itself is never handed to git, so a symbolic link there
/// is never followed.
fn compare(
    git: &Git,
    workspace: &Path,
    run_dir: &Path,
    before: Option<&Path>,
    now: Option<&[u8]>,
) -> Result<String, String> {
    let copy = run_dir.join(format!("diff-{}.tmp", uuid::Uuid::new_v4().simple()));
    if let Some(bytes) = now {
        duet_fs::private::write_private(&copy, bytes).map_err(|e| e.to_string())?;
    }
    let diffed = git
        .diff_files(workspace, before, &copy)
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&copy);
    diffed
}

/// Replaces git's headers (absolute paths of the saved and copied files)
/// with ones naming `path` in the workspace.
fn relabel(raw: &str, path: &str, created: bool, deleted: bool) -> String {
    let from = if created {
        "/dev/null".to_owned()
    } else {
        format!("a/{path}")
    };
    let to = if deleted {
        "/dev/null".to_owned()
    } else {
        format!("b/{path}")
    };
    let mut out = format!("diff --git a/{path} b/{path}\n");
    if created {
        out.push_str("new file\n");
    } else if deleted {
        out.push_str("deleted file\n");
    }
    let mut lines = raw.split_inclusive('\n');
    for line in lines.by_ref() {
        if line.starts_with("Binary files ") {
            out.push_str(&format!("Binary files {from} and {to} differ\n"));
            return out;
        }
        if line.starts_with("@@ ") {
            out.push_str(&format!("--- {from}\n+++ {to}\n"));
            out.push_str(line);
            break;
        }
    }
    out.extend(lines);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// `/diff` in a session outside a repository, for the operator: every file
/// duet wrote, with sensitive ones named but never shown.
pub fn for_operator(
    git: &Git,
    workspace: &Path,
    run_dir: &Path,
    presenter: &dyn Presenter,
) -> String {
    let mut out = String::new();
    for c in written(git, workspace, run_dir) {
        if presenter.path_sensitive(&c.path) {
            out.push_str(&format!(
                "changed {}: sensitive, held locally\n",
                c.path.display()
            ));
        } else {
            out.push_str(&c.diff);
        }
    }
    if out.is_empty() {
        out.push_str("no changes by duet's file tools yet\n");
    }
    format!("{NOTE}\n{out}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_fs::Precondition;

    #[test]
    fn diffs_the_files_the_run_wrote_against_their_first_content() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = ws.join(".duet/runs/r1");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(ws.join("gone.txt"), "old\n").unwrap();
        std::fs::write(ws.join("same.txt"), "same\n").unwrap();
        let git = Git::locate().unwrap();
        let mut j = journal::WriteJournal::open(&run).unwrap();
        let w = |j: &mut journal::WriteJournal, p: &str, b: &[u8]| {
            j.write(&ws, Path::new(p), b, &Precondition::Any).unwrap();
        };
        w(&mut j, "a.txt", b"one\nthree\n");
        w(&mut j, "a.txt", b"one\nfour\n");
        w(&mut j, "new.txt", b"fresh\n");
        w(&mut j, "gone.txt", b"doomed\n");
        w(&mut j, "same.txt", b"changed\n");
        w(&mut j, "same.txt", b"same\n");
        std::fs::remove_file(ws.join("gone.txt")).unwrap();
        let changes = written(&git, &ws, &run);
        let paths: Vec<&Path> = changes.iter().map(|c| c.path.as_path()).collect();
        assert_eq!(
            paths,
            [
                Path::new("a.txt"),
                Path::new("new.txt"),
                Path::new("gone.txt")
            ]
        );
        let a = &changes[0].diff;
        assert!(
            a.starts_with("diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@"),
            "{a}"
        );
        assert!(a.contains("-two\n+four\n") && !a.contains("three"), "{a}");
        assert!(!a.contains(&*run.to_string_lossy()), "{a}");
        let new = &changes[1].diff;
        assert!(
            new.contains("new file\n--- /dev/null\n+++ b/new.txt\n") && new.contains("+fresh"),
            "{new}"
        );
        let gone = &changes[2].diff;
        assert!(
            gone.contains("--- a/gone.txt\n+++ /dev/null\n") && gone.contains("-old"),
            "{gone}"
        );
        // The scratch copies are gone.
        let left: Vec<String> = std::fs::read_dir(&run)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("diff-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_link_put_in_place_of_a_written_file_is_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let ws = root.join("ws");
        let run = ws.join(".duet/runs/r1");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(root.join("outside.txt"), "private-outside-text\n").unwrap();
        let git = Git::locate().unwrap();
        let mut j = journal::WriteJournal::open(&run).unwrap();
        j.write(&ws, Path::new("a.txt"), b"mine\n", &Precondition::Any)
            .unwrap();
        std::fs::remove_file(ws.join("a.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("outside.txt"), ws.join("a.txt")).unwrap();
        let changes = written(&git, &ws, &run);
        assert_eq!(changes.len(), 1);
        assert!(
            !changes[0].diff.contains("private-outside-text"),
            "{}",
            changes[0].diff
        );
    }
}
