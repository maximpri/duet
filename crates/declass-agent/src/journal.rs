// SPDX-License-Identifier: GPL-3.0-or-later
//! Write journal: every file write is recorded as pending (with the previous
//! content saved) before it happens and as applied after. On resume, pending
//! writes without an applied record are rolled back, so an interrupted run
//! never leaves a half-applied change.

use declass_fs::host::{HostWait, persist};
use declass_fs::{FsError, Precondition, WriteReceipt};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Record {
    Pending {
        n: u64,
        path: PathBuf,
        existed: bool,
    },
    Applied {
        n: u64,
    },
}

pub struct WriteJournal {
    dir: PathBuf,
    next: u64,
    paths: Vec<PathBuf>,
    wait: Option<Arc<dyn HostWait>>,
    /// The only paths writes may go to (a sub-agent's); `None`: any.
    scope: Option<WriteScope>,
}

/// The workspace paths a journal may write: globs over `/`-separated
/// relative paths (see [`declass_boundary::policy::glob_match`]). Every tool
/// write goes through the journal, so a scope confines all of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteScope {
    globs: Vec<String>,
}

impl WriteScope {
    /// Writes only to paths matching one of `globs`; no globs, no writes.
    pub fn only(globs: Vec<String>) -> Self {
        Self { globs }
    }

    pub fn allows(&self, rel: &Path) -> bool {
        let path = rel.to_string_lossy();
        self.globs
            .iter()
            .any(|g| declass_boundary::policy::glob_match(g, &path))
    }

    pub fn globs(&self) -> &[String] {
        &self.globs
    }
}

impl WriteJournal {
    pub fn open(run_dir: &Path) -> Result<Self, FsError> {
        Self::open_waiting(run_dir, None)
    }

    /// Opens the journal; each step of a write is retried in place while the
    /// disk is full and `wait` agrees (see `crate::host`).
    pub fn open_waiting(run_dir: &Path, wait: Option<Arc<dyn HostWait>>) -> Result<Self, FsError> {
        let dir = run_dir.join("writes");
        persist(wait.as_deref(), || {
            declass_fs::private::ensure_private_dir(&dir)
        })?;
        let lines = persist(wait.as_deref(), || {
            declass_fs::private::read_lines_repairing(&run_dir.join("writes.jsonl"))
        })?;
        let next = lines.len() as u64 + 1;
        Ok(Self {
            dir,
            next,
            paths: Vec::new(),
            wait,
            scope: None,
        })
    }

    /// Refuses every later write outside `scope` (before anything is saved or
    /// recorded).
    pub fn confine(&mut self, scope: WriteScope) {
        self.scope = Some(scope);
    }

    /// Takes up numbering after writes another journal on the same run made
    /// meanwhile (a sub-agent's), so record numbers stay unique.
    pub fn refresh(&mut self) -> Result<(), FsError> {
        let lines = persist(self.wait.as_deref(), || {
            declass_fs::private::read_lines_repairing(&self.log())
        })?;
        self.next = self.next.max(lines.len() as u64 + 1);
        Ok(())
    }

    fn log(&self) -> PathBuf {
        self.dir.with_file_name("writes.jsonl")
    }

    /// Writes `bytes` to `rel` under `workspace` with journal protection.
    ///
    /// Each step (saving the previous content, the pending record, the write,
    /// the applied record) is retried on its own while the disk is full, so a
    /// retry never leaves a second pending record for the same write.
    pub fn write(
        &mut self,
        workspace: &Path,
        rel: &Path,
        bytes: &[u8],
        pre: &Precondition,
    ) -> Result<WriteReceipt, FsError> {
        if let Some(scope) = &self.scope
            && !scope.allows(rel)
        {
            let allowed = if scope.globs.is_empty() {
                "none".to_owned()
            } else {
                scope.globs.join(", ")
            };
            return Err(FsError::Io {
                op: "write",
                path: rel.to_path_buf(),
                message: format!("outside the paths this sub-agent may write ({allowed})"),
                errno: None,
            });
        }
        let wait = self.wait.clone();
        let wait = wait.as_deref();
        let n = self.next;
        self.next += 1;
        let before = persist(wait, || {
            declass_fs::read_optional(workspace, rel, 64 * 1024 * 1024)
        })?;
        if let Some(b) = &before {
            let saved = self.dir.join(format!("{n}.before"));
            persist(wait, || declass_fs::private::write_private(&saved, b))?;
        }
        let pending = serde_json::to_string(&Record::Pending {
            n,
            path: rel.to_path_buf(),
            existed: before.is_some(),
        })
        .unwrap_or_default();
        let log = self.log();
        persist(wait, || declass_fs::private::append_line(&log, &pending))?;
        let mut retry = false;
        let receipt = persist(wait, || {
            // An attempt that failed only after replacing the file (syncing
            // its directory) must not be repeated against the new content.
            if std::mem::replace(&mut retry, true)
                && declass_fs::read_optional(workspace, rel, 64 * 1024 * 1024)?.as_deref()
                    == Some(bytes)
            {
                return Ok(WriteReceipt {
                    before: before.as_deref().map(declass_fs::sha256_hex),
                    after: declass_fs::sha256_hex(bytes),
                    before_bytes: before.clone(),
                });
            }
            declass_fs::atomic_write(workspace, rel, bytes, pre, 0o644)
        })?;
        let applied = serde_json::to_string(&Record::Applied { n }).unwrap_or_default();
        persist(wait, || declass_fs::private::append_line(&log, &applied))?;
        if !self.paths.contains(&rel.to_path_buf()) {
            self.paths.push(rel.to_path_buf());
        }
        Ok(receipt)
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.paths.clone()
    }

    /// The number the next write will be recorded under (a session marks
    /// where each operator turn begins with it, for undo).
    pub fn next_record(&self) -> u64 {
        self.next
    }

    /// Whether recovery would restore project files. Planning may inspect
    /// this state, but must not perform the rollback itself.
    pub(crate) fn has_pending(run_dir: &Path) -> Result<bool, FsError> {
        let path = run_dir.join("writes.jsonl");
        let records = declass_fs::private::read_lines_repairing(&path)?
            .iter()
            .map(|line| serde_json::from_str::<Record>(line))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| FsError::Io {
                op: "read write journal",
                path,
                message: e.to_string(),
                errno: None,
            })?;
        let applied: std::collections::HashSet<u64> = records
            .iter()
            .filter_map(|r| match r {
                Record::Applied { n } => Some(*n),
                Record::Pending { .. } => None,
            })
            .collect();
        Ok(records
            .iter()
            .any(|r| matches!(r, Record::Pending { n, .. } if !applied.contains(n))))
    }

    /// Rolls back writes that were started but not recorded as applied.
    /// Returns the paths restored.
    pub fn recover(run_dir: &Path, workspace: &Path) -> Result<Vec<PathBuf>, FsError> {
        let lines = declass_fs::private::read_lines_repairing(&run_dir.join("writes.jsonl"))?;
        let records: Vec<Record> = lines
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let applied: Vec<u64> = records
            .iter()
            .filter_map(|r| match r {
                Record::Applied { n } => Some(*n),
                Record::Pending { .. } => None,
            })
            .collect();
        let mut restored = Vec::new();
        for r in &records {
            if let Record::Pending { n, path, existed } = r {
                if applied.contains(n) {
                    continue;
                }
                let before = if *existed {
                    Some(
                        std::fs::read(run_dir.join(format!("writes/{n}.before")))
                            .map_err(|e| FsError::io("read", path, e))?,
                    )
                } else {
                    None
                };
                declass_fs::restore(workspace, path, before.as_deref())?;
                declass_fs::private::append_line(
                    &run_dir.join("writes.jsonl"),
                    &serde_json::to_string(&Record::Applied { n: *n }).unwrap_or_default(),
                )?;
                restored.push(path.clone());
            }
        }
        Ok(restored)
    }
}

/// A file the run wrote, as its journal records it (read-only view for the
/// run viewer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// Workspace-relative path.
    pub path: PathBuf,
    /// The content before the run's first write to it (`None`: the run
    /// created it). This file holds that content when `existed`.
    pub before: Option<PathBuf>,
    /// Journal number of the latest write to it (orders files by recency).
    pub last: u64,
}

/// The files `run_dir`'s journal records, in order of first write. Reads
/// only; a missing journal is an empty list.
pub fn written(run_dir: &Path) -> Vec<Written> {
    written_since(run_dir, 0)
}

/// The files written by records numbered `from` or later, each with its
/// content before the first of those writes (what undoing them restores).
pub fn written_since(run_dir: &Path, from: u64) -> Vec<Written> {
    written_between(run_dir, from, None)
}

/// [`written_since`], limited to records numbered below `to` when given.
pub fn written_between(run_dir: &Path, from: u64, to: Option<u64>) -> Vec<Written> {
    let text = std::fs::read_to_string(run_dir.join("writes.jsonl")).unwrap_or_default();
    let mut out: Vec<Written> = Vec::new();
    for record in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Record>(l).ok())
    {
        let Record::Pending { n, path, existed } = record else {
            continue;
        };
        if n < from || to.is_some_and(|t| n >= t) {
            continue;
        }
        match out.iter_mut().find(|w| w.path == path) {
            Some(w) => w.last = n,
            None => out.push(Written {
                before: existed.then(|| run_dir.join(format!("writes/{n}.before"))),
                path,
                last: n,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_write_is_rolled_back() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = d.path().join("run");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("a.txt"), "original").unwrap();
        let mut j = WriteJournal::open(&run).unwrap();
        j.write(&ws, Path::new("a.txt"), b"first edit", &Precondition::Any)
            .unwrap();
        // Simulate a crash between "pending" and "applied": the file changed but no applied record.
        std::fs::write(run.join("writes/2.before"), "first edit").unwrap();
        declass_fs::private::append_line(
            &run.join("writes.jsonl"),
            r#"{"state":"pending","n":2,"path":"a.txt","existed":true}"#,
        )
        .unwrap();
        std::fs::write(ws.join("a.txt"), "half-written").unwrap();
        let restored = WriteJournal::recover(&run, &ws).unwrap();
        assert_eq!(restored, vec![PathBuf::from("a.txt")]);
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).unwrap(),
            "first edit"
        );
        assert!(
            WriteJournal::recover(&run, &ws).unwrap().is_empty(),
            "recovery is idempotent"
        );
    }

    #[test]
    fn a_confined_journal_writes_only_its_paths_and_numbers_stay_unique() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = d.path().join("run");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("a.txt"), "original").unwrap();
        let mut parent = WriteJournal::open(&run).unwrap();
        parent
            .write(&ws, Path::new("a.txt"), b"parent", &Precondition::Any)
            .unwrap();

        // A sub-agent's journal on the same run, confined to src/.
        let mut child = WriteJournal::open(&run).unwrap();
        let mark = child.next_record();
        child.confine(WriteScope::only(vec!["src/**".into()]));
        let refused = child
            .write(&ws, Path::new("a.txt"), b"child", &Precondition::Any)
            .unwrap_err();
        assert!(refused.to_string().contains("src/**"), "{refused}");
        assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "parent");
        child
            .write(&ws, Path::new("src/x.rs"), b"fn x() {}", &Precondition::Any)
            .unwrap();
        let end = child.next_record();
        let mut none = WriteJournal::open(&run).unwrap();
        none.confine(WriteScope::default());
        assert!(
            none.write(&ws, Path::new("src/y.rs"), b"y", &Precondition::Any)
                .is_err()
        );

        // The parent takes up numbering after the child's records.
        parent.refresh().unwrap();
        parent
            .write(&ws, Path::new("a.txt"), b"again", &Precondition::Any)
            .unwrap();
        let text = std::fs::read_to_string(run.join("writes.jsonl")).unwrap();
        let mut pending: Vec<u64> = text
            .lines()
            .filter_map(|l| match serde_json::from_str::<Record>(l).ok()? {
                Record::Pending { n, .. } => Some(n),
                Record::Applied { .. } => None,
            })
            .collect();
        let count = pending.len();
        pending.sort_unstable();
        pending.dedup();
        assert_eq!((count, pending.len()), (3, 3), "{text}");
        // The child's range holds its own write only.
        let theirs = written_between(&run, mark, Some(end));
        assert_eq!(theirs.len(), 1);
        assert_eq!(theirs[0].path, PathBuf::from("src/x.rs"));
    }

    #[test]
    fn written_lists_each_file_once_with_its_first_content() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        let run = d.path().join("run");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("a.txt"), "original").unwrap();
        assert!(written(&run).is_empty());
        let mut j = WriteJournal::open(&run).unwrap();
        j.write(&ws, Path::new("a.txt"), b"one", &Precondition::Any)
            .unwrap();
        j.write(&ws, Path::new("new.txt"), b"x", &Precondition::Any)
            .unwrap();
        j.write(&ws, Path::new("a.txt"), b"two", &Precondition::Any)
            .unwrap();
        let w = written(&run);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].path, PathBuf::from("a.txt"));
        assert_eq!(
            std::fs::read_to_string(w[0].before.as_ref().unwrap()).unwrap(),
            "original"
        );
        assert_eq!(
            (w[1].path.as_path(), w[1].before.as_ref()),
            (Path::new("new.txt"), None)
        );
        assert!(w[0].last > w[1].last, "a.txt was written last");
    }
}
