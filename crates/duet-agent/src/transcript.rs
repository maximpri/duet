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
    Masked {
        items: usize,
        tokens_before: u64,
        tokens_after: u64,
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
    /// (Session) How an operator turn ended, and the seconds it worked.
    /// Texts are in their local form (placeholders restored).
    TurnEnd {
        exchange: u64,
        seconds: f64,
        end: crate::session::TurnEnd,
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
}

pub struct Transcript {
    path: PathBuf,
    wait: Option<Arc<dyn HostWait>>,
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
        })
    }

    /// Appends one entry, whole or not at all.
    pub fn append(&self, entry: &Entry) -> Result<(), FsError> {
        let line = serde_json::to_string(entry).unwrap_or_default();
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
