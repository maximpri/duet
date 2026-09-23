// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("path {0} is outside the workspace")]
    Outside(String),
    #[error("path {0} is reserved (.git or .duet)")]
    Reserved(String),
    #[error("{0} is a symbolic link; links are never followed")]
    Symlink(PathBuf),
    #[error("{0} is not a regular file")]
    NotAFile(String),
    #[error("{0} already exists")]
    Exists(String),
    #[error("{path} changed since it was read (expected {expected}, found {actual})")]
    Precondition {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("cannot {op} {path}: {message}")]
    Io {
        op: &'static str,
        path: PathBuf,
        message: String,
    },
    #[error("workspace is locked by another duet process")]
    Locked,
}

impl FsError {
    pub fn io(op: &'static str, path: impl Into<PathBuf>, e: impl std::fmt::Display) -> Self {
        Self::Io {
            op,
            path: path.into(),
            message: e.to_string(),
        }
    }
}
