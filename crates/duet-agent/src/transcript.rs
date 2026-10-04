// SPDX-License-Identifier: GPL-3.0-or-later
//! The run transcript: every conversation item and run event, appended and
//! synced as it happens. A crashed or interrupted run resumes from it.

use duet_boundary::model::{Item, Usage};
use duet_fs::FsError;
use duet_fs::host::HostWait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    ReviewUsage {
        usage: duet_boundary::review::UsageStats,
    },
    Start {
        objective: String,
        mode: String,
        frontier_model: String,
    },
    Item {
        item: Item,
    },
    Usage {
        turn: u64,
        usage: Usage,
        cost_usd: f64,
        interventions: Vec<String>,
    },
    /// Estimated usage of attempts of the request for `turn` that failed
    /// after output started; charged like billed usage.
    FailedAttempts {
        turn: u64,
        usage: Usage,
        cost_usd: f64,
    },
    /// The run was interrupted while this tool call ran; its command was
    /// stopped and its result is not recorded.
    Interrupted {
        call_id: String,
        tool: String,
    },
    /// Old tool results were replaced by stubs (`crate::context`).
    /// `positions` are the items masked, in the conversation as it was then
    /// (a resume masks the same ones; transcripts before they were recorded
    /// have none, and their resume masks again as needed).
    Masked {
        items: usize,
        tokens_before: u64,
        tokens_after: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        positions: Vec<usize>,
    },
    /// The local model condensed items `head..upto` of the conversation (as
    /// it was then) into one message, `text`, which replaced them
    /// (`crate::compaction`). `text` is kept whole, so a resume rebuilds the
    /// same conversation byte for byte.
    Compacted {
        head: usize,
        upto: usize,
        text: String,
        tokens_before: u64,
        tokens_after: u64,
        local_seconds: f64,
    },
    /// A compaction of a conversation of `tokens` failed (`reason`); the
    /// conversation was left as it was, and no new attempt is made before
    /// it reaches `retry_at`.
    CompactionFailed {
        tokens: u64,
        retry_at: u64,
        reason: String,
        local_seconds: f64,
    },
    /// How a tool result was shown to the frontier (for the cost ledger).
    Shown {
        call_id: String,
        class: duet_boundary::view::ViewClass,
    },
    /// (Session) An operator turn begins. `message` is the operator's text as
    /// typed, kept locally for the conversation view; the frontier receives
    /// the sanitized `Item` that follows. `journal_next` is the write
    /// journal's next record number, where undoing this turn starts.
    /// `exchange` counts operator turns (not frontier requests, which
    /// `Usage::turn` counts).
    TurnStart {
        exchange: u64,
        message: String,
        journal_next: u64,
    },
    /// (Session) Messages the operator sent while the turn ran, delivered
    /// at a safe point: after every result of request `after_request` was
    /// recorded, before the next request. The sanitized `Item` follows.
    Steered {
        exchange: u64,
        after_request: u64,
        messages: Vec<String>,
    },
    /// (Session) How an operator turn ended, and the seconds it worked.
    /// Texts are in their local form (placeholders restored).
    TurnEnd {
        exchange: u64,
        seconds: f64,
        end: crate::session::TurnEnd,
    },
    /// (Session) An explicit operator change to the planning capability
    /// boundary. Replayed before any recovery or model work on resume.
    PlanMode {
        enabled: bool,
    },
    /// (Session) The operator reverted the journaled writes of the turns from
    /// `exchange` on.
    Undone {
        exchange: u64,
        paths: Vec<PathBuf>,
    },
    End {
        terminal: crate::run::Terminal,
    },
    /// A sub-agent started (`delegate`). `child` is its id in the run (`a1`,
    /// `a2`, ...), `call_id` the parent's tool call it answers, `task` the
    /// task as the frontier wrote it, `paths` the globs a writing one may
    /// write, and `journal_next` where its writes begin in the write journal.
    SubagentStart {
        child: String,
        call_id: String,
        mode: String,
        task: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
        journal_next: u64,
    },
    /// One entry of a sub-agent's own conversation (its items, usage, ...),
    /// nested under its id; the parent's conversation never contains them.
    Subagent {
        child: String,
        entry: Box<Entry>,
    },
    /// A sub-agent ended in `terminal` (as the frontier wrote it), after
    /// `requests` frontier requests costing `cost_usd`. `written` are the
    /// files it wrote, and `journal_end` is where its writes end in the
    /// journal.
    SubagentEnd {
        child: String,
        call_id: String,
        terminal: crate::run::Terminal,
        requests: u64,
        cost_usd: f64,
        seconds: f64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        written: Vec<PathBuf>,
        journal_end: u64,
    },
    /// The parent never received this sub-agent's result (the run stopped
    /// before recording it), so on resume its writes were rolled back.
    SubagentReverted {
        child: String,
        paths: Vec<PathBuf>,
    },
    /// The local explorer answered the `explore` call `call_id`
    /// (`crate::explore`): what it did, never what it read or wrote (its
    /// report is the call's result). A resumed run charges its local time.
    Explored {
        call_id: String,
        stats: crate::explore::Stats,
    },
}

pub struct Transcript {
    path: PathBuf,
    wait: Option<Arc<dyn HostWait>>,
    /// A sub-agent's transcript: every entry is nested under its id.
    child: Option<String>,
}

impl Transcript {
    pub fn open(run_dir: &Path) -> Result<Self, FsError> {
        Self::open_waiting(run_dir, None)
    }

    /// Opens the transcript; its writes are retried in place while the disk
    /// is full and `wait` agrees (see `crate::host`).
    pub fn open_waiting(run_dir: &Path, wait: Option<Arc<dyn HostWait>>) -> Result<Self, FsError> {
        duet_fs::host::persist(wait.as_deref(), || {
            duet_fs::private::ensure_private_dir(run_dir)
        })?;
        Ok(Self {
            path: run_dir.join("transcript.jsonl"),
            wait,
            child: None,
        })
    }

    /// The same transcript, with every entry appended nested under the
    /// sub-agent `child` (`None`: at the top level).
    pub(crate) fn nested(mut self, child: Option<String>) -> Self {
        self.child = child;
        self
    }

    /// Appends one entry, whole or not at all.
    pub fn append(&self, entry: &Entry) -> Result<(), FsError> {
        // Match Entry::Subagent on disk without cloning the entire tool result
        // (or image payload) merely to wrap it in its child's envelope.
        #[derive(Serialize)]
        struct Nested<'a> {
            kind: &'static str,
            child: &'a str,
            entry: &'a Entry,
        }
        let line = match &self.child {
            Some(child) => serde_json::to_string(&Nested {
                kind: "subagent",
                child,
                entry,
            }),
            None => serde_json::to_string(entry),
        }
        .unwrap_or_default();
        duet_fs::host::persist(self.wait.as_deref(), || {
            duet_fs::private::append_line(&self.path, &line)
        })
    }

    pub fn read(run_dir: &Path) -> Result<Vec<Entry>, FsError> {
        Ok(
            duet_fs::private::read_lines_repairing(&run_dir.join("transcript.jsonl"))?
                .iter()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_entries_keep_the_replay_format() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = Transcript::open(dir.path())
            .unwrap()
            .nested(Some("a\"1".into()));
        let entry = Entry::Item {
            item: Item::ToolResult {
                call_id: "read-1".into(),
                content: "result\n\"quoted\" 日本語".repeat(1_024),
            },
        };
        transcript.append(&entry).unwrap();
        let expected = Entry::Subagent {
            child: "a\"1".into(),
            entry: Box::new(entry),
        };
        let line = serde_json::to_string(&expected).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("transcript.jsonl")).unwrap(),
            format!("{line}\n")
        );
        let restored = Transcript::read(dir.path()).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(serde_json::to_string(&restored[0]).unwrap(), line);
    }
}
