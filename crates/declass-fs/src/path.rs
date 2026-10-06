// SPDX-License-Identifier: GPL-3.0-or-later
//! Lexical validation of model-supplied paths.

use crate::error::FsError;
use std::path::{Component, Path, PathBuf};

/// Directories no tool may write, at any depth.
pub const RESERVED: [&str; 2] = [".git", ".declass"];

/// Normalizes a workspace-relative path: no absolute paths, no `..`, no empty
/// components. `.` components are dropped.
pub fn normalize_relative(input: &str) -> Result<PathBuf, FsError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(FsError::Outside(input.to_owned()));
    }
    let mut out = PathBuf::new();
    for c in Path::new(trimmed).components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::Outside(input.to_owned()));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(FsError::Outside(input.to_owned()));
    }
    Ok(out)
}

/// Whether `rel` is inside a reserved directory.
pub fn is_reserved(rel: &Path) -> bool {
    rel.components()
        .any(|c| matches!(c, Component::Normal(p) if RESERVED.iter().any(|r| p == *r)))
}

/// Normalizes a path that a tool intends to write.
pub fn writable_relative(input: &str) -> Result<PathBuf, FsError> {
    let rel = normalize_relative(input)?;
    if is_reserved(&rel) {
        return Err(FsError::Reserved(input.to_owned()));
    }
    Ok(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_rejects_escapes() {
        assert_eq!(
            normalize_relative("./src//lib.rs").unwrap(),
            PathBuf::from("src/lib.rs")
        );
        for bad in ["", "/etc/passwd", "../x", "a/../../b", ".", "  "] {
            assert!(normalize_relative(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn reserved_directories_at_any_depth() {
        assert!(writable_relative(".git/config").is_err());
        assert!(writable_relative("vendor/lib/.git/hooks/pre-commit").is_err());
        assert!(writable_relative(".declass/runs/x").is_err());
        assert!(writable_relative("src/.gitignore").is_ok());
        assert!(writable_relative("docs/declass.md").is_ok());
    }
}
