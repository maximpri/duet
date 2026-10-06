// SPDX-License-Identifier: GPL-3.0-or-later
//! Persistent path classifications, shared by every run of one workspace.
//! These contain paths only. They must outlive run retention and purge.

use declass_fs::{FsError, pinned::PinnedParent};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LEGACY_RUNS: usize = 10_000;

fn invalid(path: &Path, message: &str) -> FsError {
    FsError::io("load derived classifications", path, message.to_owned())
}

fn missing(error: &FsError) -> bool {
    matches!(error, FsError::Io { errno: Some(n), .. } if *n == declass_fs::fault::Errno::NOENT.raw_os_error())
}

fn load_at(root: &Path, rel: &Path, budget: &mut u64) -> Result<HashSet<PathBuf>, FsError> {
    let store = match Store::open(root, rel) {
        Ok(store) => store,
        Err(e) if missing(&e) => return Ok(HashSet::new()),
        Err(e) => return Err(e),
    };
    if store.pending.file_type()?.is_some() {
        return Err(invalid(
            store.pending.path(),
            "a sensitive command did not finish classification; owner review is required before another run",
        ));
    }
    if store.manifest.file_type()?.is_none() {
        return Ok(HashSet::new());
    }
    let limit = MAX_MANIFEST_BYTES.min(*budget);
    let mut bytes = Vec::new();
    store
        .manifest
        .open_read()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| FsError::io("read classifications", store.manifest.path(), e))?;
    if bytes.len() as u64 > limit {
        return Err(invalid(
            store.manifest.path(),
            "classification state exceeds the review limit",
        ));
    }
    *budget -= bytes.len() as u64;
    let paths: HashSet<PathBuf> = serde_json::from_slice(&bytes).map_err(|_| {
        invalid(
            store.manifest.path(),
            "invalid classification manifest; owner review is required",
        )
    })?;
    if paths.iter().any(|p| {
        p.as_os_str().is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_)))
    }) {
        return Err(invalid(
            store.manifest.path(),
            "classification manifest contains an invalid relative path",
        ));
    }
    Ok(paths)
}

pub(crate) fn load(path: &Path) -> Result<HashSet<PathBuf>, FsError> {
    let mut budget = MAX_TOTAL_BYTES;
    load_at(
        path.parent().unwrap(),
        Path::new(path.file_name().unwrap()),
        &mut budget,
    )
}

/// Reads current and retained legacy run classifications without creating state
/// or reading classified file contents. Pending commands, malformed manifests,
/// symlinks and I/O errors refuse the preview/startup. Reads are capped at
/// 10,000 retained entries and 64 MiB in total (16 MiB per manifest).
pub fn workspace_paths(workspace: &Path) -> Result<HashSet<PathBuf>, FsError> {
    let mut budget = MAX_TOTAL_BYTES;
    let mut paths = load_at(workspace, Path::new(".declass/derived.json"), &mut budget)?;
    let runs = workspace.join(".declass/runs");
    // Pin descendants from the workspace root before inspecting any manifest.
    // Directory enumeration supplies names only, never a new trusted root.
    match PinnedParent::open(workspace, Path::new(".declass/runs/derived.json"), false) {
        Ok(_) => {}
        Err(e) if missing(&e) => return Ok(paths),
        Err(e) => return Err(e),
    }
    let entries = match std::fs::read_dir(&runs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
        Err(e) => return Err(FsError::io("read directory", runs, e)),
    };
    for (count, entry) in entries.enumerate() {
        if count >= MAX_LEGACY_RUNS {
            return Err(invalid(&runs, "too many retained runs to inspect safely"));
        }
        let entry = entry.map_err(|e| FsError::io("read directory", &runs, e))?;
        let kind = entry
            .file_type()
            .map_err(|e| FsError::io("inspect", entry.path(), e))?;
        if kind.is_symlink() {
            return Err(FsError::Symlink(entry.path()));
        }
        if kind.is_dir() {
            paths.extend(load_at(
                workspace,
                &Path::new(".declass/runs")
                    .join(entry.file_name())
                    .join("derived.json"),
                &mut budget,
            )?);
        }
    }
    Ok(paths)
}

pub(crate) struct Store {
    manifest: PinnedParent,
    pending: PinnedParent,
}
impl Store {
    pub(crate) fn open(root: &Path, rel: &Path) -> Result<Self, FsError> {
        Ok(Self {
            manifest: PinnedParent::open(root, rel, false)?,
            pending: PinnedParent::open(root, &rel.with_extension("pending"), false)?,
        })
    }
    pub(crate) fn save(&self, paths: &HashSet<PathBuf>) -> Result<(), FsError> {
        let mut sorted: Vec<_> = paths.iter().collect();
        sorted.sort();
        let bytes = serde_json::to_vec(&sorted)
            .map_err(|e| FsError::io("encode classifications", self.manifest.path(), e))?;
        declass_fs::private::write_private_pinned(&self.manifest, &bytes)
    }
    pub(crate) fn begin(&self) -> Result<(), FsError> {
        if self.pending.file_type()?.is_some() {
            return Err(invalid(
                self.pending.path(),
                "a sensitive command is already pending classification",
            ));
        }
        declass_fs::private::write_private_pinned(
            &self.pending,
            b"Sensitive command classification is incomplete.\n",
        )
    }
    pub(crate) fn finish(&self) -> Result<(), FsError> {
        if self.pending.file_type()?.is_some() {
            self.pending.remove()?;
            self.pending.sync()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(path: &Path) -> Store {
        Store::open(path.parent().unwrap(), Path::new(path.file_name().unwrap())).unwrap()
    }

    #[test]
    fn readonly_preview_does_not_create_state_and_imports_retained_legacy_paths() {
        let d = tempfile::tempdir().unwrap();
        assert!(workspace_paths(d.path()).unwrap().is_empty());
        assert!(!d.path().join(".declass").exists());
        let run = d.path().join(".declass/runs/old");
        declass_fs::private::ensure_private_dir(&run).unwrap();
        store(&run.join("derived.json"))
            .save(&HashSet::from(["export.txt".into()]))
            .unwrap();
        assert_eq!(
            workspace_paths(d.path()).unwrap(),
            HashSet::from(["export.txt".into()])
        );
        assert!(!d.path().join(".declass/derived.json").exists());
    }

    #[test]
    fn unfinished_command_and_corrupt_classifications_refuse_a_fresh_run() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join(".declass");
        declass_fs::private::ensure_private_dir(&state).unwrap();
        let manifest = state.join("derived.json");
        store(&manifest).begin().unwrap();
        assert!(workspace_paths(d.path()).is_err());
        assert!(store(&manifest).begin().is_err());
        store(&manifest)
            .save(&HashSet::from(["export.txt".into()]))
            .unwrap();
        // A persisted path list alone is insufficient: the pending marker
        // survives a crash between the list write and classification commit.
        assert!(workspace_paths(d.path()).is_err());
        store(&manifest).finish().unwrap();
        assert!(
            workspace_paths(d.path())
                .unwrap()
                .contains(Path::new("export.txt"))
        );
        for invalid in [
            "broken json",
            "[\"../outside\"]",
            "[\"/absolute\"]",
            "[\"\"]",
        ] {
            std::fs::write(&manifest, invalid).unwrap();
            assert!(workspace_paths(d.path()).is_err());
        }
    }

    #[test]
    fn classification_and_pending_symlinks_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join(".declass");
        declass_fs::private::ensure_private_dir(&state).unwrap();
        let manifest = state.join("derived.json");
        std::os::unix::fs::symlink("absent", &manifest).unwrap();
        assert!(workspace_paths(d.path()).is_err());
        std::fs::remove_file(&manifest).unwrap();
        std::os::unix::fs::symlink("absent", manifest.with_extension("pending")).unwrap();
        assert!(workspace_paths(d.path()).is_err());
        assert!(store(&manifest).begin().is_err());
    }

    #[test]
    fn legacy_parent_symlinks_and_exhausted_read_budget_fail_closed() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join(".declass");
        declass_fs::private::ensure_private_dir(&state).unwrap();
        let outside = d.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, state.join("runs")).unwrap();
        assert!(workspace_paths(d.path()).is_err());
        std::fs::remove_file(state.join("runs")).unwrap();
        store(&state.join("derived.json"))
            .save(&HashSet::from(["export.txt".into()]))
            .unwrap();
        assert!(load_at(d.path(), Path::new(".declass/derived.json"), &mut 1).is_err());
    }

    #[test]
    fn failed_classification_write_keeps_the_pending_marker() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().canonicalize().unwrap().join(".declass");
        declass_fs::private::ensure_private_dir(&state).unwrap();
        let manifest = state.join("derived.json");
        store(&manifest).begin().unwrap();
        let _guard = declass_fs::fault::inject(&state, |_, _| {
            Some(declass_fs::fault::Fault {
                after_bytes: 0,
                errno: declass_fs::fault::Errno::NOSPC,
            })
        });
        assert!(
            store(&manifest)
                .save(&HashSet::from(["export.txt".into()]))
                .is_err()
        );
        assert!(workspace_paths(d.path()).is_err());
    }
}
