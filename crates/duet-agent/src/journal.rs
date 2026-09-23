// SPDX-License-Identifier: GPL-3.0-or-later
//! Write journal: every file write is recorded as pending (with the previous
//! content saved) before it happens and as applied after. On resume, pending
//! writes without an applied record are rolled back, so an interrupted run
//! never leaves a half-applied change.

use duet_fs::{FsError, Precondition, WriteReceipt};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
}

impl WriteJournal {
    pub fn open(run_dir: &Path) -> Result<Self, FsError> {
        let dir = run_dir.join("writes");
        duet_fs::private::ensure_private_dir(&dir)?;
        let lines = duet_fs::private::read_lines_repairing(&run_dir.join("writes.jsonl"))?;
        let next = lines.len() as u64 + 1;
        Ok(Self {
            dir,
            next,
            paths: Vec::new(),
        })
    }

    fn log(&self) -> PathBuf {
        self.dir.with_file_name("writes.jsonl")
    }

    /// Writes `bytes` to `rel` under `workspace` with journal protection.
    pub fn write(
        &mut self,
        workspace: &Path,
        rel: &Path,
        bytes: &[u8],
        pre: &Precondition,
    ) -> Result<WriteReceipt, FsError> {
        let n = self.next;
        self.next += 1;
        let before = duet_fs::read_optional(workspace, rel, 64 * 1024 * 1024)?;
        if let Some(b) = &before {
            duet_fs::private::write_private(&self.dir.join(format!("{n}.before")), b)?;
        }
        let pending = Record::Pending {
            n,
            path: rel.to_path_buf(),
            existed: before.is_some(),
        };
        duet_fs::private::append_line(
            &self.log(),
            &serde_json::to_string(&pending).unwrap_or_default(),
        )?;
        let receipt = duet_fs::atomic_write(workspace, rel, bytes, pre, 0o644)?;
        duet_fs::private::append_line(
            &self.log(),
            &serde_json::to_string(&Record::Applied { n }).unwrap_or_default(),
        )?;
        if !self.paths.contains(&rel.to_path_buf()) {
            self.paths.push(rel.to_path_buf());
        }
        Ok(receipt)
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.paths.clone()
    }

    /// Rolls back writes that were started but not recorded as applied.
    /// Returns the paths restored.
    pub fn recover(run_dir: &Path, workspace: &Path) -> Result<Vec<PathBuf>, FsError> {
        let lines = duet_fs::private::read_lines_repairing(&run_dir.join("writes.jsonl"))?;
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
                duet_fs::restore(workspace, path, before.as_deref())?;
                duet_fs::private::append_line(
                    &run_dir.join("writes.jsonl"),
                    &serde_json::to_string(&Record::Applied { n: *n }).unwrap_or_default(),
                )?;
                restored.push(path.clone());
            }
        }
        Ok(restored)
    }
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
        duet_fs::private::append_line(
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
}
