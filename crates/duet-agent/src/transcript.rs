// SPDX-License-Identifier: GPL-3.0-or-later
//! The run transcript: every conversation item and run event, appended and
//! synced as it happens. A crashed or interrupted run resumes from it.

use duet_boundary::model::{Item, Usage};
use duet_fs::FsError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    End {
        terminal: crate::run::Terminal,
    },
}

pub struct Transcript {
    path: PathBuf,
}

impl Transcript {
    pub fn open(run_dir: &Path) -> Result<Self, FsError> {
        duet_fs::private::ensure_private_dir(run_dir)?;
        Ok(Self {
            path: run_dir.join("transcript.jsonl"),
        })
    }

    pub fn append(&self, entry: &Entry) -> Result<(), FsError> {
        duet_fs::private::append_line(
            &self.path,
            &serde_json::to_string(entry).unwrap_or_default(),
        )
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
