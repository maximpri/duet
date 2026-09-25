// SPDX-License-Identifier: GPL-3.0-or-later
//! The files a run changed, for the Run view's side panel: taken from the
//! run's write journal (each file's content before the run's first write to
//! it) and diffed against the workspace now with Duet's hardened git helper.
//! Everything here reads; nothing is written. Files that are sensitive by
//! policy, or derived from sensitive data by a command, are listed as held
//! locally and never diffed, so their content is not shown.

use duet_agent::journal;
use duet_boundary::policy::Policy;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Diff rows kept per file (a larger diff is cut with a note).
const MAX_ROWS: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Context,
    Added,
    Removed,
    /// The start of a hunk (`text` says where).
    Hunk,
    /// A remark from git or from the viewer (binary file, cut diff).
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: RowKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The run created it.
    Created,
    Edited,
    /// It existed before the run and is gone now.
    Deleted,
    /// Written, but back to its content before the run.
    Unchanged,
    /// Changed by a command that read sensitive data (not journaled).
    ByCommand,
}

/// Why a file's content is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Matches the sensitivity rules (globs, protected paths).
    Sensitive,
    /// Created or changed by a command that could read sensitive data.
    Derived,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangedFile {
    pub path: String,
    pub status: Status,
    pub held: Option<Held>,
    pub added: usize,
    pub removed: usize,
    pub rows: Vec<Row>,
    /// Journal number of the latest write (0: not journaled).
    pub last: u64,
    /// (modified, length) of the workspace file when last diffed.
    stamp: Option<(SystemTime, u64)>,
    diffed: bool,
    before: Option<PathBuf>,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
}

/// Parses `git diff` unified output into numbered rows and counts.
pub fn parse_unified(text: &str) -> (Vec<Row>, usize, usize) {
    let mut rows = Vec::new();
    let (mut added, mut removed) = (0, 0);
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            let Some((ranges, _)) = rest.split_once(" @@") else {
                continue;
            };
            let mut parts = ranges.split_whitespace();
            let start = |p: Option<&str>, sign: char| -> u32 {
                p.and_then(|p| p.strip_prefix(sign))
                    .and_then(|p| p.split(',').next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0)
            };
            old = start(parts.next(), '-');
            new = start(parts.next(), '+');
            in_hunk = true;
            rows.push(Row {
                old: None,
                new: None,
                kind: RowKind::Hunk,
                text: format!("from line {}", new.max(1)),
            });
            continue;
        }
        if !in_hunk {
            if line.starts_with("Binary files") {
                rows.push(Row {
                    old: None,
                    new: None,
                    kind: RowKind::Note,
                    text: "binary file: no line diff".into(),
                });
            }
            continue;
        }
        if rows.len() >= MAX_ROWS {
            rows.push(Row {
                old: None,
                new: None,
                kind: RowKind::Note,
                text: format!("diff cut at {MAX_ROWS} rows"),
            });
            break;
        }
        let (kind, body) = match line.chars().next() {
            Some('+') => (RowKind::Added, &line[1..]),
            Some('-') => (RowKind::Removed, &line[1..]),
            Some(' ') => (RowKind::Context, &line[1..]),
            Some('\\') => (RowKind::Note, line.trim_start_matches("\\ ")),
            _ => (RowKind::Context, line),
        };
        let row = match kind {
            RowKind::Added => {
                added += 1;
                new += 1;
                Row {
                    old: None,
                    new: Some(new - 1),
                    kind,
                    text: body.into(),
                }
            }
            RowKind::Removed => {
                removed += 1;
                old += 1;
                Row {
                    old: Some(old - 1),
                    new: None,
                    kind,
                    text: body.into(),
                }
            }
            RowKind::Context => {
                old += 1;
                new += 1;
                Row {
                    old: Some(old - 1),
                    new: Some(new - 1),
                    kind,
                    text: body.into(),
                }
            }
            _ => Row {
                old: None,
                new: None,
                kind,
                text: body.into(),
            },
        };
        rows.push(row);
    }
    (rows, added, removed)
}

/// Files derived from sensitive data in this run: `derived.json` (kept by
/// the security engine) and the audit log's sensitive-command events.
pub fn derived_files(run_dir: &Path, from_audit: &[String]) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = std::fs::read(run_dir.join("derived.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<PathBuf>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    out.extend(from_audit.iter().cloned());
    out
}

/// Brings `files` up to date with the run's journal and the workspace:
/// adds newly written files, re-diffs files whose content changed since the
/// last look, and lists derived files. Returns whether anything changed.
pub fn update(
    files: &mut Vec<ChangedFile>,
    ws: &Path,
    run_dir: &Path,
    policy: &Policy,
    derived: &BTreeSet<String>,
) -> bool {
    let git = duet_git::Git::locate().ok();
    let mut changed = false;
    let held = |path: &str| {
        if policy.is_sensitive_path(Path::new(path)) {
            Some(Held::Sensitive)
        } else if derived.contains(path) {
            Some(Held::Derived)
        } else {
            None
        }
    };
    for w in journal::written(run_dir) {
        let path = w.path.to_string_lossy().into_owned();
        let i = match files.iter().position(|f| f.path == path) {
            Some(i) => i,
            None => {
                files.push(ChangedFile {
                    path: path.clone(),
                    status: Status::Edited,
                    held: None,
                    added: 0,
                    removed: 0,
                    rows: Vec::new(),
                    last: 0,
                    stamp: None,
                    diffed: false,
                    before: w.before.clone(),
                });
                changed = true;
                files.len() - 1
            }
        };
        let f = &mut files[i];
        if f.last != w.last {
            f.last = w.last;
            changed = true;
        }
        f.held = held(&path);
        let now = stamp(&ws.join(&path));
        let status = match (&f.before, &now) {
            (None, Some(_)) => Status::Created,
            (Some(_), None) => Status::Deleted,
            _ => Status::Edited,
        };
        if f.held.is_some() {
            f.rows.clear();
            (f.added, f.removed, f.status) = (0, 0, status);
            continue;
        }
        if f.diffed && f.stamp == now {
            continue;
        }
        f.stamp = now;
        f.diffed = true;
        changed = true;
        let text = match &git {
            Some(g) => g
                .diff_files(ws, f.before.as_deref(), &ws.join(&path))
                .map_err(|e| e.to_string()),
            None => Err("git is not installed: no diff".into()),
        };
        match text {
            Ok(t) => {
                (f.rows, f.added, f.removed) = parse_unified(&t);
                f.status = if t.is_empty() {
                    Status::Unchanged
                } else {
                    status
                };
            }
            Err(e) => {
                f.rows = vec![Row {
                    old: None,
                    new: None,
                    kind: RowKind::Note,
                    text: e,
                }];
                f.status = status;
            }
        }
    }
    for path in derived {
        if !files.iter().any(|f| &f.path == path) {
            files.push(ChangedFile {
                path: path.clone(),
                status: Status::ByCommand,
                held: Some(Held::Derived),
                added: 0,
                removed: 0,
                rows: Vec::new(),
                last: 0,
                stamp: None,
                diffed: false,
                before: None,
            });
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunks_with_line_numbers_and_counts() {
        let text = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -2,3 +2,4 @@ fn f\n a\n-b\n+B\n+C\n c\n\\ No newline at end of file\n@@ -10 +11 @@\n-z\n+Z\n";
        let (rows, added, removed) = parse_unified(text);
        assert_eq!((added, removed), (3, 2));
        let shown: Vec<(Option<u32>, Option<u32>, RowKind, &str)> = rows
            .iter()
            .map(|r| (r.old, r.new, r.kind, r.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            vec![
                (None, None, RowKind::Hunk, "from line 2"),
                (Some(2), Some(2), RowKind::Context, "a"),
                (Some(3), None, RowKind::Removed, "b"),
                (None, Some(3), RowKind::Added, "B"),
                (None, Some(4), RowKind::Added, "C"),
                (Some(4), Some(5), RowKind::Context, "c"),
                (None, None, RowKind::Note, "No newline at end of file"),
                (None, None, RowKind::Hunk, "from line 11"),
                (Some(10), None, RowKind::Removed, "z"),
                (None, Some(11), RowKind::Added, "Z"),
            ]
        );
        let (rows, ..) = parse_unified("diff --git a/x b/x\nBinary files a/x and b/x differ\n");
        assert_eq!(rows[0].kind, RowKind::Note);
    }
}
