// SPDX-License-Identifier: GPL-3.0-or-later
//! What a program that embeds Duet adds to its runs and sessions
//! (ARCHITECTURE.md, "Embedding Duet"): audit subscribers, told of every
//! record of the run's audit log as it is written, and end hooks, called once
//! when a run or a session invocation ends. The third extension point, the
//! policy layer, is part of configuration ([`duet_config::policy`]) and is
//! re-exported here.
//!
//! Duet's own command line adds nothing ([`Hooks::default`]); it composes
//! runs through the same calls, so its behaviour is the same with or without
//! hooks. Hooks see names, counts, digests and chain positions, never content.

use crate::run::{INTERRUPTED, RunStats, Terminal};
use duet_boundary::audit::{AuditEvent, AuditHandle, AuditLog, call_hook};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

pub use crate::disclosure::Disclosure;
pub use duet_boundary::audit::{Appended, AuditSubscriber, ChainHead, Opened, Recorded};
pub use duet_config::policy::{Policy, PolicyError, PolicyFile, PolicyMeta, PolicySource};

/// The hooks of an embedding program for a run or session. The default has
/// none.
#[derive(Clone, Default)]
pub struct Hooks {
    audit: Vec<Arc<dyn AuditSubscriber>>,
    end: Vec<Arc<dyn EndHook>>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = |n: Vec<&str>| n.join(", ");
        f.debug_struct("Hooks")
            .field(
                "audit",
                &names(self.audit.iter().map(|s| s.name()).collect()),
            )
            .field("end", &names(self.end.iter().map(|h| h.name()).collect()))
            .finish()
    }
}

impl Hooks {
    /// Adds an audit subscriber (see [`AuditSubscriber`] for delivery and
    /// failure).
    pub fn subscribe(mut self, subscriber: Arc<dyn AuditSubscriber>) -> Self {
        self.audit.push(subscriber);
        self
    }

    /// Adds an end hook (see [`EndHook`]).
    pub fn on_end(mut self, hook: Arc<dyn EndHook>) -> Self {
        self.end.push(hook);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.audit.is_empty() && self.end.is_empty()
    }

    pub(crate) fn has_end_hooks(&self) -> bool {
        !self.end.is_empty()
    }

    /// Attaches the audit subscribers to a run's log, in the order they were
    /// added. Call it once, as soon as the log is open and before anything
    /// is recorded in this invocation.
    pub fn attach(&self, log: &mut AuditLog) {
        for s in &self.audit {
            log.subscribe(s.clone());
        }
    }

    /// Calls every end hook with `report`; a failure is reported on stderr
    /// and recorded in `audit` as a `hook_failed` event (stage `end`).
    pub(crate) fn ended(&self, report: &EndReport, audit: Option<&AuditHandle>) {
        for h in &self.end {
            if let Err((hook, panicked)) = call_hook(h.name(), "end hook", || h.ended(report))
                && let Some(a) = audit
            {
                a.record(AuditEvent::HookFailed {
                    hook,
                    stage: "end".into(),
                    seq: None,
                    panicked,
                });
            }
        }
    }
}

/// Told once when a run or a session invocation ends: every `duet run`,
/// `duet resume` and a `duet` session that got as far as creating its run
/// directory, whatever the terminal state (completed, failed, interrupted,
/// budget-stopped, a panic). A run interrupted and resumed is one call per
/// invocation; [`EndReport::resumable`] says whether another may follow.
///
/// It is called after the audit log's `run_end` event (which anchors the
/// log's head) and `summary.json` are written. An error or a panic never
/// reaches the run: it is reported on stderr and recorded as a `hook_failed`
/// event (stage `end`), which then follows the reported head in the log. A
/// process killed outright calls nothing.
pub trait EndHook: Send + Sync {
    /// A short name, recorded when the hook fails.
    fn name(&self) -> &str;

    fn ended(&self, report: &EndReport) -> Result<(), String>;
}

/// A one-shot run, or a conversation (a `duet` session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunKind {
    Run,
    Session,
}

/// What the caller of [`crate::conclude_with`] knows about the invocation
/// besides its records, for the end hooks.
pub struct Ending<'a> {
    pub kind: RunKind,
    /// The mode as the audit log's `run_start` event names it (`hybrid`,
    /// `passthrough`, `top-clearance`).
    pub mode: &'a str,
    /// This invocation continued an earlier one (`duet resume`,
    /// `duet --resume`).
    pub resumed: bool,
    /// The policy layer the run's configuration was loaded with.
    pub policy: Option<&'a PolicyMeta>,
    pub hooks: &'a Hooks,
}

/// How a run or session invocation ended, for an [`EndHook`]. Names, counts,
/// digests and paths only: no task, summary, failure reason or other text
/// written by a model or read from the workspace.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct EndReport {
    pub run_id: String,
    pub kind: RunKind,
    pub mode: String,
    pub resumed: bool,
    /// `completed`, `failed` or `budget_stopped`, as in the audit log's
    /// `run_end` event. A session is `completed` when the operator closed it
    /// and `failed` when they left it open to resume.
    pub state: String,
    /// The budget that stopped it (`budget_stopped`): `wall_clock`,
    /// `frontier_usd` or a session budget's setting name.
    pub budget: Option<String>,
    /// It was stopped by an interrupt (Ctrl-C).
    pub interrupted: bool,
    /// A later invocation may continue it: it did not complete, and it did
    /// something to continue.
    pub resumable: bool,
    pub audit_log: PathBuf,
    /// The log's head after the `run_end` event, with its anchor file;
    /// `None` when the log could not be opened.
    pub chain: Option<ChainHead>,
    /// The policy layer, when the configuration had one.
    pub policy: Option<PolicyMeta>,
    pub stats: EndStats,
    /// What the boundary withheld from the frontier, by class (counts and
    /// kinds, as in `summary.json`); `None` when the log could not be read.
    pub disclosure: Option<Disclosure>,
}

/// Counts of an invocation (a resumed run or session counts what came
/// before it too, as its `summary.json` does).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[non_exhaustive]
pub struct EndStats {
    pub turns: u64,
    pub tool_calls: u64,
    pub masked_results: u64,
    pub compactions: u64,
    /// Billed frontier tokens.
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    /// Frontier spend at list price, failed attempts included.
    pub cost_usd: f64,
    pub wall_seconds: f64,
    pub subagents: u64,
}

impl EndStats {
    fn of(s: &RunStats) -> Self {
        Self {
            turns: s.turns,
            tool_calls: s.tool_calls,
            masked_results: s.masked_results,
            compactions: s.compactions,
            input_tokens: s.usage.input,
            cache_read_tokens: s.usage.cache_read,
            cache_write_tokens: s.usage.cache_write,
            output_tokens: s.usage.output,
            cost_usd: s.cost_usd,
            wall_seconds: s.wall_seconds,
            subagents: s.subagents.children,
        }
    }
}

impl EndReport {
    /// The report of an invocation of `run_id` that ended in `terminal`;
    /// the log's head and the disclosure report are the caller's to add.
    pub(crate) fn new(
        run_id: &str,
        ending: &Ending<'_>,
        terminal: &Terminal,
        stats: &RunStats,
        audit_log: PathBuf,
    ) -> Self {
        Self {
            run_id: run_id.to_owned(),
            kind: ending.kind,
            mode: ending.mode.to_owned(),
            resumed: ending.resumed,
            state: terminal.state().to_owned(),
            budget: match terminal {
                Terminal::BudgetStopped { which } => Some(which.clone()),
                _ => None,
            },
            interrupted: matches!(terminal, Terminal::Failed { reason } if reason == INTERRUPTED),
            resumable: false,
            audit_log,
            chain: None,
            policy: ending.policy.cloned(),
            stats: EndStats::of(stats),
            disclosure: None,
        }
    }
}
