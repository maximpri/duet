// SPDX-License-Identifier: GPL-3.0-or-later
//! File-scoped instruction discovery, with conversation-local deduplication.

use super::*;
use std::collections::BTreeMap;

/// Descendant notes for a file, outer directory first. Root notes are excluded
/// because the opening message already contains them. Absolute paths are only
/// accepted inside the configured workspace; traversals and reserved paths
/// never cause discovery. References within a file are never expanded.
pub fn scoped(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    audit: Option<&AuditHandle>,
    path: &Path,
) -> Option<String> {
    let (_, files) = discover(cfg, presenter, path)?;
    render(&files, presenter, audit, MAX_SCOPED_BYTES)
}

/// What the current conversation has already seen for each target directory.
/// Changes to file content, overrides, or the set of available files reload
/// that scope; an unchanged second access produces no duplicate prompt text.
/// Initialize a fresh value on transcript replay so the next file access
/// refreshes current on-disk rules. This is instruction context, never policy.
#[derive(Default)]
pub struct ScopedInstructions {
    seen: BTreeMap<PathBuf, String>,
}

impl ScopedInstructions {
    /// Return newly applicable rules before a read/edit of `path`. A caller
    /// must defer a first edit until the model has seen the returned rules.
    pub fn for_path(
        &mut self,
        cfg: &RunConfig,
        presenter: &dyn Presenter,
        audit: Option<&AuditHandle>,
        path: &Path,
    ) -> Option<String> {
        let (scope, files) = discover(cfg, presenter, path)?;
        let identity: String = files
            .iter()
            .map(|file| format!("{}\0{}\n", file.path.display(), file.found.sha256))
            .collect();
        let digest = declass_fs::sha256_hex(identity.as_bytes());
        let previous = self.seen.get(&scope);
        if previous == Some(&digest) {
            return None;
        }
        let shown = render(&files, presenter, audit, MAX_SCOPED_BYTES).or_else(|| {
            previous.map(|_| format!(
                "[declass] The previously supplied descendant instruction files for {} are no longer available. Those descendant notes no longer apply to this scope; the repository root instructions and operator's request still apply.",
                scope.display()
            ))
        });
        if shown.is_some() {
            // Bound conversation metadata independently of the text budget.
            if self.seen.len() >= 1_024 {
                self.seen.clear();
            }
            self.seen.insert(scope, digest);
        }
        shown
    }

    /// Refresh rules after discarding earlier instruction-bearing context.
    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

fn discover(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    path: &Path,
) -> Option<(PathBuf, Vec<Instruction>)> {
    // Bound lexical work even when a model supplies a path the OS cannot open.
    if path.as_os_str().len() > 4_096 {
        return None;
    }
    let relative = if path.is_absolute() {
        path.strip_prefix(&cfg.workspace).ok()?
    } else {
        path
    };
    let relative = declass_fs::normalize_relative(relative.to_str()?).ok()?;
    if declass_fs::is_reserved(&relative) || !presenter.path_visible(&relative) {
        return None;
    }
    let scope = relative.parent()?.to_owned();
    let mut parents: Vec<_> = scope
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .collect();
    parents.reverse();
    let mut files = Vec::new();
    for directory in parents {
        files.extend(
            collect(directory, None, FILES, |path| read_at(&cfg.workspace, path))
                .into_iter()
                .filter(|file| presenter.path_visible(&file.path)),
        );
    }
    Some((scope, files))
}
