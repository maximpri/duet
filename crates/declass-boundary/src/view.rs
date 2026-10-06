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
    /// Content fetched from the web (a page, search results): public but
    /// untrusted. Scanned like public content, offloaded when bulky, and never
    /// instructions to the model.
    Web {
        url: String,
    },
    /// Git history: one path's content or diff at a revision (classified by
    /// the path as it is now, like a file), or commit metadata (messages,
    /// authors, status) when `path` is `None`.
    GitHistory {
        rev: String,
        path: Option<PathBuf>,
    },
    /// A language server's answer about the workspace file `path`: a line of
    /// it (a definition or reference), or, with `signature`, declaration text
    /// (hover, symbol names). Classified exactly like the file: nothing of a
    /// sensitive or sealed file's content, and of an interface-only file only
    /// declarations.
    CodeNav {
        path: PathBuf,
        signature: bool,
    },
    Other {
        label: String,
    },
    /// An image's bytes (PNG or JPEG, prepared), which cannot be shown as
    /// text: the local model describes it and the frontier gets the cleaned
    /// description. `origin` is its path or file name.
    Image {
        origin: String,
    },
    /// A tool result (or error text) from an MCP server; `trust` is the
    /// server's configured class.
    Mcp {
        server: String,
        tool: String,
        trust: ServerTrust,
    },
    /// A sub-agent's report to the agent that delegated to it (`delegate`):
    /// text a model wrote after seeing only what the boundary presented to
    /// it. Scanned like public text and never offloaded; framed as data.
    Subagent {
        child: String,
    },
    /// The local explorer's report (`explore`): text the local model wrote
    /// after reading the workspace as it is, sensitive and protected files
    /// included. `question` is the question as the frontier asked it; `read`
    /// is everything the explorer was shown. Cleaned as local-model output
    /// about `read` (copied spans, re-encodings, digits of withheld numbers,
    /// the per-value budget, protected code) and never offloaded.
    Explore {
        question: String,
        read: Explored,
    },
}

/// What the local explorer was shown, each text with the workspace file it
/// came from (`None`: output not tied to one file, such as a listing or
/// commit metadata). The text is raw workspace content: `Debug` shows only
/// the paths and sizes.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Explored(pub std::sync::Arc<Vec<(Option<PathBuf>, String)>>);

impl std::fmt::Debug for Explored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(p, t)| {
                format!(
                    "{} ({} bytes)",
                    p.as_ref().map_or("-".into(), |p| p.display().to_string()),
                    t.len()
                )
            }))
            .finish()
    }
}

/// How an external server's results are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerTrust {
    /// Untrusted public data: scanned and shown (bulky results offloaded).
    Public,
    /// Sensitive data: held locally as a handle with a local summary.
    Sensitive,
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
    /// Protected source: an interface-only skeleton or a sealed-file notice.
    Protected,
}

impl ViewClass {
    pub const ALL: [ViewClass; 6] = [
        ViewClass::Raw,
        ViewClass::Tokenized,
        ViewClass::HandleSummary,
        ViewClass::LocalAnswer,
        ViewClass::BulkyHandle,
        ViewClass::Protected,
    ];
}

/// What the frontier asked to change in a protected file.
#[derive(Debug, Clone, Copy)]
pub struct ImplementRequest<'a> {
    /// The change, as the frontier described it.
    pub spec: &'a str,
    /// Test code the change must pass, if the frontier wrote some.
    pub tests: Option<&'a str>,
    /// Raw output of the failed checks of a previous attempt (stays local).
    pub feedback: Option<&'a str>,
}

/// A protected file's new content and what the frontier may be told about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Implemented {
    pub content: String,
    /// Which items changed, in terms of the interface only.
    pub summary: String,
}

/// A working summary the local model wrote of the older part of a
/// conversation (context compaction), and the local seconds it took.
#[derive(Debug, Clone, PartialEq)]
pub struct Condensed {
    /// The summary, cleaned as local-model output; `Err` says why there is none.
    pub summary: Result<String, String>,
    pub seconds: f64,
}

pub trait Presenter: Send + Sync {
    /// A second opinion is optional and must independently enforce open-code,
    /// non-privacy eligibility. `paths` includes every contextual source.
    fn review_second(
        &self,
        _candidate: &declass_review::Candidate,
        _paths: &[PathBuf],
        _diff: &str,
        _remaining_usd: f64,
    ) -> Option<Result<declass_review::Judgment, String>> {
        None
    }
    fn take_review_usage(&self) -> crate::review::UsageStats {
        Default::default()
    }
    /// Local-only judgment of a rule candidate. No tools or frontier fallback.
    /// `None` means this presenter has no local reviewer.
    fn review_candidate(
        &self,
        _candidate: &declass_review::Candidate,
    ) -> Option<Result<declass_review::Judgment, String>> {
        None
    }
    /// Names from sensitive schemas, for advisory privacy-flow candidates.
    /// Never the values of those fields.
    fn review_fields(&self) -> Vec<String> {
        Vec::new()
    }
    /// Byte ranges of locally recognized private values in source. The review
    /// layer receives offsets, never the vault's values or protected-code tokens.
    fn review_value_spans(&self, _source: &str) -> Vec<(usize, usize)> {
        Vec::new()
    }
    /// Text shown to the frontier for `bytes` produced by `source`.
    fn present(&self, source: &Source, bytes: &[u8]) -> String;
    /// How the latest `present` or `call_tool` result was shown, once; `None`
    /// when nothing was recorded (the content was shown as it is).
    fn take_view_class(&self) -> Option<ViewClass> {
        None
    }
    /// Security events decided since the last call, for the run's audit log
    /// (a local-model question that probed a value piece by piece).
    fn take_events(&self) -> Vec<crate::audit::AuditEvent> {
        Vec::new()
    }
    /// Whether the frontier may see this path's name and content at all
    /// (used to hide protected paths from listings and searches).
    fn path_visible(&self, _path: &std::path::Path) -> bool {
        true
    }
    /// The path's protected-source level (IP levels), if it has one.
    fn protection(&self, _path: &std::path::Path) -> Option<crate::policy::IpLevel> {
        None
    }
    /// Whether the content of this path is sensitive (policy globs, or derived
    /// from sensitive data during the run).
    fn path_sensitive(&self, _path: &std::path::Path) -> bool {
        false
    }
    /// Workspace paths ordinary commands may not read (enforced by the sandbox).
    fn hidden_from_commands(&self, _workspace: &std::path::Path) -> Vec<PathBuf> {
        Vec::new()
    }
    /// Workspace paths the host's checks may not read. Checks build and test
    /// the project, so protected source stays readable to them; their output
    /// is presented as [`Source::Checks`].
    fn hidden_from_checks(&self, workspace: &std::path::Path) -> Vec<PathBuf> {
        self.hidden_from_commands(workspace)
    }
    /// A change to protected source, made where the source may be read (by the
    /// local model). `None` if `path` is not protected.
    fn implement_protected(
        &self,
        _path: &std::path::Path,
        _current: &str,
        _request: &ImplementRequest<'_>,
    ) -> Option<Result<Implemented, String>> {
        None
    }
    /// Persist a write-ahead marker before a command may read sensitive data.
    fn begin_sensitive_command(&self) -> Result<(), declass_fs::FsError> {
        Ok(())
    }
    /// Files a sensitive command created or changed: they hold derived data from now on.
    /// `paths` are relative to `workspace`.
    fn mark_sensitive(
        &self,
        _workspace: &std::path::Path,
        _paths: &[PathBuf],
    ) -> Result<(), declass_fs::FsError> {
        Ok(())
    }
    /// Text the frontier itself wrote into a file (`write_file` content, an
    /// edit's new text), before placeholders are resolved.
    fn note_authored(&self, _text: &str) {}
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
    /// What the task text is given besides itself (the sensitive paths, the
    /// protected ones, a local brief), for a sub-agent's task: the same
    /// knowledge of the boundary its parent was given. Empty when there is
    /// nothing to say.
    fn task_notes(&self) -> String {
        String::new()
    }
    /// A later operator message in a session, as it may be shown to the
    /// frontier: sanitized like the task, without the task's notes (the
    /// sensitive-path note and brief are given once per session).
    fn sanitize_message(&self, text: &str) -> String {
        text.to_owned()
    }
    /// Text the frontier wants sent to a third party (`destination`: a host
    /// or server name), such as a URL, a search query or tool arguments.
    /// `Ok` holds the text to send; `Err` says why nothing may be sent.
    /// Placeholders are never resolved here: the third party is not local.
    fn check_outbound(&self, _destination: &str, text: &str) -> Result<String, String> {
        Ok(text.to_owned())
    }
    /// The check every request to a third party passes, owned (see
    /// [`crate::third_party`]): the only maker of the [`Checked`] requests
    /// the product's HTTP client sends. It reads every part of a request as
    /// [`Self::check_outbound`] reads a text, against what the run knows at
    /// the time of the request, and records each refusal. Without the
    /// boundary every request passes.
    ///
    /// [`Checked`]: crate::third_party::Checked
    fn outbound_guard(&self) -> crate::third_party::Guard {
        crate::third_party::Guard::open()
    }
    /// Whether [`Self::condense`] can write a summary (a local model is
    /// configured). Pass-through has none.
    fn can_condense(&self) -> bool {
        false
    }
    /// A working summary of `conversation`, the older part of a frontier
    /// conversation in the form it was sent (after the outbound filter),
    /// written by the local model and cleaned as local-model output (see
    /// `declass_agent::compaction`). `None` when there is no local model.
    fn condense(&self, _conversation: &str) -> Option<Condensed> {
        None
    }
    /// Where an image goes (see [`crate::images`]). Without the boundary: to
    /// the frontier when it accepts images. The engine records an image it
    /// lets through by digest, and its outbound check refuses any other.
    fn route_image(&self, image: &crate::images::ImageRequest<'_>) -> crate::images::Route {
        crate::images::route(&crate::images::Facts {
            boundary: false,
            frontier_vision: image.frontier_vision,
            local_vision: false,
            to_frontier: crate::images::ToFrontier::Never,
            attached: image.attached,
            operator_public: image.operator_public,
            in_workspace: matches!(image.origin, crate::images::Origin::Workspace(_)),
            sensitive: false,
            protected: false,
        })
    }
}

/// How a language-server answer from a file the frontier may not see the
/// content of is shown in its place (see [`Source::CodeNav`]).
pub const CODE_NAV_WITHHELD: &str = "⟨withheld:";

/// Shows content unchanged except for a size cap.
pub struct PassThrough {
    pub max_bytes: usize,
}

impl Presenter for PassThrough {
    fn present(&self, source: &Source, bytes: &[u8]) -> String {
        if let Source::Image { origin } = source {
            // Pass-through sends images themselves (or refuses them); their
            // bytes are never shown as text.
            return format!("[image {origin}: not shown as text]");
        }
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
