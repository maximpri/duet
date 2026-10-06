// SPDX-License-Identifier: GPL-3.0-or-later
//! Deleting raw run data (`.declass/runs/<id>`: handles, transcripts, vault,
//! write journal). Audit logs are kept separately and may contain sensitive
//! request bodies, especially in top clearance. Used by
//! `declass purge` and the TUI's Data screen.

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
    ws.join(".declass/runs")
}

/// Rejects a run id that could name a path outside the runs directory.
pub fn check_run_id(id: &str) -> Result<&str, String> {
    if id.is_empty()
        || id == "."
        || id.contains(['/', '\\'])
        || id.contains("..")
        || id.chars().any(char::is_control)
    {
        Err("invalid run id".into())
    } else {
        Ok(id)
    }
}

/// The runs `scope` selects, sorted. Reads only.
pub fn candidates(ws: &Path, scope: Scope) -> Vec<String> {
    try_candidates(ws, scope).unwrap_or_default()
}

/// Reports unreadable run data instead of silently treating it as expired.
/// Symlink directories are never followed. Activity includes nested files:
/// appending a transcript does not update its containing directory's mtime.
pub fn try_candidates(ws: &Path, scope: Scope) -> std::io::Result<Vec<String>> {
    let cutoff = match scope {
        Scope::OlderThan { days } => Some(
            SystemTime::now()
                .checked_sub(Duration::from_secs(
                    (days.max(0) as u64).saturating_mul(86_400),
                ))
                .unwrap_or(SystemTime::UNIX_EPOCH),
        ),
        Scope::All => None,
    };
    let root = runs_dir(ws);
    if !root.try_exists()? {
        return Ok(Vec::new());
    }
    safe_run_root(ws)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_dir() || check_run_id(&name).is_err() {
            continue;
        }
        let expired = match cutoff {
            Some(cutoff) => latest_activity(&entry.path())? < cutoff,
            None => true,
        };
        if expired {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

fn safe_run_root(ws: &Path) -> std::io::Result<()> {
    for path in [ws.join(".declass"), runs_dir(ws)] {
        if !std::fs::symlink_metadata(path)?.file_type().is_dir() {
            return Err(std::io::Error::other(
                "run data directory must not be a symlink",
            ));
        }
    }
    Ok(())
}

fn latest_activity(root: &Path) -> std::io::Result<SystemTime> {
    let mut latest = std::fs::symlink_metadata(root)?.modified()?;
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            entries += 1;
            if entries > 100_000 {
                return Err(std::io::Error::other(
                    "run activity scan exceeds 100000 entries",
                ));
            }
            let metadata = std::fs::symlink_metadata(entry.path())?;
            latest = latest.max(metadata.modified()?);
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(latest)
}

/// Deletes one run's raw data.
pub fn purge_run(ws: &Path, id: &str) -> std::io::Result<()> {
    let id = check_run_id(id).map_err(std::io::Error::other)?;
    safe_run_root(ws)?;
    if !std::fs::symlink_metadata(runs_dir(ws).join(id))?.is_dir() {
        return Err(std::io::Error::other("run data must be a real directory"));
    }
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
            std::fs::create_dir_all(ws.join(".declass/runs").join(id)).unwrap();
            std::fs::write(ws.join(".declass/runs").join(id).join("vault.json"), "{}").unwrap();
        }
        std::fs::create_dir_all(ws.join(".declass/audit")).unwrap();
        std::fs::write(ws.join(".declass/audit/r1.jsonl"), "").unwrap();
        assert!(candidates(ws, Scope::OlderThan { days: 14 }).is_empty());
        assert_eq!(
            candidates(ws, Scope::OlderThan { days: -1 }),
            vec!["r1", "r2"]
        );
        assert_eq!(candidates(ws, Scope::All), vec!["r1", "r2"]);
        purge_run(ws, "r1").unwrap();
        assert_eq!(candidates(ws, Scope::All), vec!["r2"]);
        assert!(
            ws.join(".declass/audit/r1.jsonl").exists(),
            "audit logs stay"
        );
        assert!(purge_run(ws, "../x").is_err());
        assert!(purge_run(ws, "").is_err());
        assert!(purge_run(ws, ".").is_err());
        assert!(ws.join(".declass/runs/r2").exists());
    }

    #[test]
    fn a_recent_nested_transcript_keeps_an_old_directory_out_of_retention() {
        let d = tempfile::tempdir().unwrap();
        let run = d.path().join(".declass/runs/r1");
        let nested = run.join("session");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("transcript.jsonl"), "new message").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 86_400);
        for dir in [&nested, &run] {
            std::fs::File::open(dir)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        assert!(
            try_candidates(d.path(), Scope::OlderThan { days: 1 })
                .unwrap()
                .is_empty()
        );
        std::fs::File::open(nested.join("transcript.jsonl"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        assert_eq!(
            try_candidates(d.path(), Scope::OlderThan { days: 1 }).unwrap(),
            vec!["r1"]
        );
    }

    #[test]
    fn a_symlinked_runs_directory_cannot_redirect_a_purge() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(outside.path().join("r1")).unwrap();
        std::fs::create_dir(d.path().join(".declass")).unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join(".declass/runs")).unwrap();
        assert!(try_candidates(d.path(), Scope::All).is_err());
        assert!(purge_run(d.path(), "r1").is_err());
        assert!(outside.path().join("r1").exists());
    }
}
