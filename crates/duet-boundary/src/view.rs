// SPDX-License-Identifier: GPL-3.0-or-later
//! What the frontier sees of a tool result.
//!
//! Every tool result is described by its [`Source`] and rendered by a
//! [`Presenter`]. In pass-through mode the content is shown as-is (size-capped);
//! the security engine replaces this with classification, placeholders and
//! handles.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File {
        path: PathBuf,
        /// The model asked for a line range (a deliberate read of those lines).
        ranged: bool,
    },
    FileList,
    Search {
        pattern: String,
    },
    Command {
        command: String,
        exit_code: Option<i32>,
    },
    /// A command the model ran with access to sensitive files: its output is
    /// derived from them, so it is sensitive whatever it looks like.
    SensitiveCommand {
        command: String,
        exit_code: Option<i32>,
    },
    Diff,
    Checks,
    Other {
        label: String,
    },
}

/// The form in which a tool result reached the frontier (for the cost ledger).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewClass {
    /// The content itself (public, scanned).
    Raw,
    /// Structure kept, values replaced by placeholders.
    Tokenized,
    /// Sensitive content held locally: handle, error lines, local summary.
    HandleSummary,
    /// An `ask_local` answer.
    LocalAnswer,
    /// Public but bulky content held locally: handle, head, outline.
    BulkyHandle,
}

impl ViewClass {
    pub const ALL: [ViewClass; 5] = [
        ViewClass::Raw,
        ViewClass::Tokenized,
        ViewClass::HandleSummary,
        ViewClass::LocalAnswer,
        ViewClass::BulkyHandle,
    ];
}

pub trait Presenter: Send + Sync {
    /// Text shown to the frontier for `bytes` produced by `source`.
    fn present(&self, source: &Source, bytes: &[u8]) -> String;
    /// How the latest `present` or `call_tool` result was shown, once; `None`
    /// when nothing was recorded (the content was shown as it is).
    fn take_view_class(&self) -> Option<ViewClass> {
        None
    }
    /// Whether the frontier may see this path's name and content at all
    /// (used to hide protected paths from listings and searches).
    fn path_visible(&self, _path: &std::path::Path) -> bool {
        true
    }
    /// Workspace paths ordinary commands may not read (enforced by the sandbox).
    fn hidden_from_commands(&self, _workspace: &std::path::Path) -> Vec<PathBuf> {
        Vec::new()
    }
    /// Files a sensitive command created or changed: they hold derived data from now on.
    /// `paths` are relative to `workspace`.
    fn mark_sensitive(&self, _workspace: &std::path::Path, _paths: &[PathBuf]) {}
    /// Additional tools this presenter offers (e.g. `ask_local`); fixed for a run.
    fn extra_tools(&self) -> Vec<crate::model::ToolSpec> {
        Vec::new()
    }
    /// Handles a call to one of `extra_tools`. `None` if the name is not ours.
    fn call_tool(
        &self,
        _name: &str,
        _args: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Result<String, String>> {
        None
    }
    /// Replaces placeholders the frontier copied from what it was shown, so edit
    /// anchors match the real file.
    fn detokenize(&self, text: &str) -> String {
        text.to_owned()
    }
    /// Content about to be written to `path`, with placeholders resolved locally.
    /// `Err` explains why a placeholder may not be written there.
    fn resolve_for_write(&self, _path: &std::path::Path, text: &str) -> Result<String, String> {
        Ok(text.to_owned())
    }
    /// The task text as it may be shown to the frontier.
    fn sanitize_objective(&self, text: &str) -> String {
        text.to_owned()
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
