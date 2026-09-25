// SPDX-License-Identifier: GPL-3.0-or-later
//! Deleting raw run data (`.duet/runs/<id>`: handles, transcripts, vault,
//! write journal). Audit logs are kept; they hold no raw values. Used by
//! `duet purge` and the TUI's Data screen.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Which runs to delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Runs last modified more than this many days ago.
    OlderThan { days: i64 },
    /// Every run.
    All,
}

fn runs_dir(ws: &Path) -> PathBuf {
    ws.join(".duet/runs")
}

/// Rejects a run id that could name a path outside the runs directory.
pub fn check_run_id(id: &str) -> Result<&str, String> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        Err("invalid run id".into())
    } else {
        Ok(id)
    }
}

/// The runs `scope` selects, sorted. Reads only.
pub fn candidates(ws: &Path, scope: Scope) -> Vec<String> {
    let cutoff = match scope {
        Scope::OlderThan { days } => {
            Some(SystemTime::now() - Duration::from_secs(days.max(0) as u64 * 86_400))
        }
        Scope::All => None,
    };
    let mut out: Vec<String> = std::fs::read_dir(runs_dir(ws))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| match cutoff {
            None => true,
            Some(cutoff) => entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| t < cutoff)
                .unwrap_or(false),
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

/// Deletes one run's raw data.
pub fn purge_run(ws: &Path, id: &str) -> std::io::Result<()> {
    let id = check_run_id(id).map_err(std::io::Error::other)?;
    std::fs::remove_dir_all(runs_dir(ws).join(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_by_age_and_deletes_only_run_directories() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path();
        for id in ["r1", "r2"] {
            std::fs::create_dir_all(ws.join(".duet/runs").join(id)).unwrap();
            std::fs::write(ws.join(".duet/runs").join(id).join("vault.json"), "{}").unwrap();
        }
        std::fs::create_dir_all(ws.join(".duet/audit")).unwrap();
        std::fs::write(ws.join(".duet/audit/r1.jsonl"), "").unwrap();
        assert!(candidates(ws, Scope::OlderThan { days: 14 }).is_empty());
        assert_eq!(
            candidates(ws, Scope::OlderThan { days: -1 }),
            vec!["r1", "r2"]
        );
        assert_eq!(candidates(ws, Scope::All), vec!["r1", "r2"]);
        purge_run(ws, "r1").unwrap();
        assert_eq!(candidates(ws, Scope::All), vec!["r2"]);
        assert!(ws.join(".duet/audit/r1.jsonl").exists(), "audit logs stay");
        assert!(purge_run(ws, "../x").is_err());
        assert!(purge_run(ws, "").is_err());
    }
}
