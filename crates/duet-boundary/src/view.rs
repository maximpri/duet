// SPDX-License-Identifier: GPL-3.0-or-later
//! What the frontier sees of a tool result.
//!
//! Every tool result is described by its [`Source`] and rendered by a
//! [`Presenter`]. In pass-through mode the content is shown as-is (size-capped);
//! the security engine replaces this with classification, placeholders and
//! handles.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File {
        path: PathBuf,
    },
    FileList,
    Search {
        pattern: String,
    },
    Command {
        command: String,
        exit_code: Option<i32>,
    },
    Diff,
    Checks,
}

pub trait Presenter: Send + Sync {
    /// Text shown to the frontier for `bytes` produced by `source`.
    fn present(&self, source: &Source, bytes: &[u8]) -> String;
    /// Whether the frontier may see this path's name and content at all
    /// (used to hide protected paths from listings and searches).
    fn path_visible(&self, _path: &std::path::Path) -> bool {
        true
    }
}

/// Shows content unchanged except for a size cap.
pub struct PassThrough {
    pub max_bytes: usize,
}

impl Presenter for PassThrough {
    fn present(&self, _source: &Source, bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        if text.len() <= self.max_bytes {
            return text.into_owned();
        }
        let mut cut = self.max_bytes;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        format!(
            "{}\n[truncated: showing {} of {} bytes; request a smaller range]",
            &text[..cut],
            cut,
            text.len()
        )
    }
}
