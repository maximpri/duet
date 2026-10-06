// SPDX-License-Identifier: GPL-3.0-or-later
//! Listing a workspace that is not a git repository: a walk of the file
//! system that honours the `.gitignore` files in it (nested ones, negation,
//! directory patterns) as `git ls-files --exclude-standard` would, and
//! `.ignore` files the same way. Global and user-wide ignore files are not
//! read, just as Declass's git runs without them. Only regular files are
//! listed; symbolic links are neither listed nor followed.

use crate::GitError;
use std::path::Path;

/// Directories never listed, at any depth: git's and Declass's own state, and
/// the dependency and build-output directories a repository's `.gitignore`
/// usually excludes (a folder that is not a repository often has none).
pub const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".declass",
    "node_modules",
    "target",
    "dist",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".gradle",
    ".next",
    ".pytest_cache",
    ".mypy_cache",
];

/// The files under `root` that the ignore files there do not exclude,
/// relative to it, `/`-separated and sorted. Entries that cannot be read
/// (permissions) are left out; a `root` that cannot be read is an error.
pub fn list(root: &Path) -> Result<Vec<String>, GitError> {
    std::fs::read_dir(root).map_err(|e| declass_fs::FsError::io("list", root, e))?;
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .git_ignore(true)
        .ignore(true)
        // `.gitignore` applies although there is no repository.
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| {
            let skipped = e.depth() > 0
                && e.file_type().is_some_and(|t| t.is_dir())
                && SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref());
            !skipped
        })
        .build();
    let mut files: Vec<String> = walker
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?;
            if rel.as_os_str().is_empty() || declass_fs::is_reserved(rel) {
                return None;
            }
            Some(
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            )
        })
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn honours_ignore_files_and_skips_state_and_build_output() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path();
        write(ws, ".gitignore", "*.log\n/data/*.db\nbuild/\n!keep.log\n");
        write(ws, ".env", "KEY=value\n");
        write(ws, "src/main.rs", "fn main() {}\n");
        write(ws, "src/.hidden.rs", "\n");
        write(ws, "server.log", "x\n");
        write(ws, "keep.log", "x\n");
        write(ws, "data/app.db", "x\n");
        write(ws, "data/seed.sql", "x\n");
        write(ws, "build/out.o", "x\n");
        write(ws, "web/.gitignore", "cache/\n");
        write(ws, "web/cache/a.json", "x\n");
        write(ws, "web/app.ts", "x\n");
        write(ws, ".ignore", "scratch/\n");
        write(ws, "scratch/probe.py", "x\n");
        write(ws, "web/node_modules/pkg/index.js", "x\n");
        write(ws, "target/debug/app", "x\n");
        write(ws, ".declass/runs/r/transcript.jsonl", "x\n");
        write(ws, ".git/config", "x\n");
        std::os::unix::fs::symlink("/etc/hosts", ws.join("hosts")).unwrap();
        assert_eq!(
            list(ws).unwrap(),
            [
                ".env",
                ".gitignore",
                ".ignore",
                "data/seed.sql",
                "keep.log",
                "src/.hidden.rs",
                "src/main.rs",
                "web/.gitignore",
                "web/app.ts",
            ]
        );
    }

    #[test]
    fn a_workspace_named_like_a_skipped_directory_is_still_listed() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("target");
        write(&ws, "a.txt", "x\n");
        assert_eq!(list(&ws).unwrap(), ["a.txt"]);
        assert!(list(&d.path().join("missing")).is_err());
    }
}
