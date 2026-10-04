// SPDX-License-Identifier: GPL-3.0-or-later
//! Frontier loop, tools, transcript, context manager and cost ledger.

pub mod changes;
pub mod code_nav;
pub mod compaction;
pub mod context;
pub mod disclosure;
pub mod driver;
pub mod egress;
pub mod embed;
pub mod explore;
pub mod git_tools;
pub mod host;
pub mod images;
pub mod instructions;
pub mod journal;
pub mod ledger;
pub mod mcp;
pub mod oversight;
mod planning;
pub mod prompt;
pub mod protected;
pub mod purge;
pub mod review;
pub mod run;
pub mod session;
pub mod skills;
pub mod subagents;
pub mod tools;
pub mod transcript;
pub mod web;

pub use compaction::Compaction;
pub use embed::{EndHook, EndReport, Ending, Hooks, RunKind};
pub use oversight::{ApproveMode, Approver, Oversight};
pub use run::{RunConfig, RunStats, Terminal, conclude, conclude_with, resumable, run};
pub use session::{Session, SessionLimits, TurnEnd};
