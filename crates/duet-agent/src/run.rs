// SPDX-License-Identifier: GPL-3.0-or-later
//! The frontier loop: one continuous conversation until a terminal state.

use crate::context::{apply_mask, estimate, mask_from_estimate};
use crate::driver::Driver;
use crate::host::HostPolicy;
use crate::journal::WriteJournal;
use crate::ledger::Ledger;
use crate::prompt::system_prompt;
use crate::tools::{self, Ctx, Outcome};
use crate::transcript::{Entry, Transcript};
use duet_boundary::audit::{AuditEvent, AuditHandle};
use duet_boundary::model::{ErrorKind, Item, Request, StopReason, ToolCall, ToolSpec, Usage};
use duet_boundary::view::{Presenter, ViewClass};
use duet_boundary::{GateError, GatedFrontier};
use duet_fs::FsError;
use duet_git::Git;
use duet_sandbox::SandboxKind;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// How a run ends. Every run ends in exactly one of these: infrastructure
/// failures are retried in place until a budget stops them, and a panic ends
/// the run as `Failed` with an internal-error reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", from = "TerminalRecord")]
pub enum Terminal {
    Completed {
        summary: String,
    },
    /// A session invocation ended normally and may be resumed.
    Open {
        reason: String,
    },
    Failed {
        reason: String,
    },
    BudgetStopped {
        which: String,
    },
}

/// Older releases recorded ordinary session exits as a failed terminal with
/// one exact sentinel. Normalize on read without rewriting historical records
/// or mistaking an actual failure that mentions that phrase for a normal exit.
#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum TerminalRecord {
    Completed { summary: String },
    Open { reason: String },
    Failed { reason: String },
    BudgetStopped { which: String },
}

impl From<TerminalRecord> for Terminal {
    fn from(record: TerminalRecord) -> Self {
        match record {
            TerminalRecord::Completed { summary } => Self::Completed { summary },
            TerminalRecord::Open { reason } => Self::Open { reason },
            TerminalRecord::Failed { reason } if reason == crate::session::SESSION_LEFT => {
                Self::Open { reason }
            }
            TerminalRecord::Failed { reason } => Self::Failed { reason },
            TerminalRecord::BudgetStopped { which } => Self::BudgetStopped { which },
        }
    }
}

/// The reason of a run stopped by an interrupt (resumable).
pub const INTERRUPTED: &str = "interrupted; resume with `duet resume`";

impl Terminal {
    /// The state name, as in `summary.json` and the audit log's end event.
    pub fn state(&self) -> &'static str {
        match self {
            Terminal::Completed { .. } => "completed",
            Terminal::Open { .. } => "open",
            Terminal::Failed { .. } => "failed",
            Terminal::BudgetStopped { .. } => "budget_stopped",
        }
    }

    /// Process exit status: leaving a resumable session is a normal exit.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Completed { .. } | Self::Open { .. } => 0,
            Self::Failed { .. } => 1,
            Self::BudgetStopped { .. } => 3,
        }
    }

    fn interrupted() -> Self {
        Terminal::Failed {
            reason: INTERRUPTED.into(),
        }
    }

    fn out_of_time() -> Self {
        Terminal::BudgetStopped {
            which: "wall_clock".into(),
        }
    }

    /// The same state with placeholders in its text restored: what the
    /// operator is shown (they may see their own values). The transcript and
    /// the audit log keep what the frontier saw.
    pub fn for_operator(&self, presenter: &dyn Presenter) -> Self {
        match self {
            Terminal::Completed { summary } => Terminal::Completed {
                summary: presenter.detokenize(summary),
            },
            Terminal::Open { reason } => Terminal::Open {
                reason: presenter.detokenize(reason),
            },
            Terminal::Failed { reason } => Terminal::Failed {
                reason: presenter.detokenize(reason),
            },
            Terminal::BudgetStopped { which } => Terminal::BudgetStopped {
                which: which.clone(),
            },
        }
    }

    /// `Failed` with the message of a caught panic.
    pub fn internal_error(panic: &(dyn std::any::Any + Send)) -> Self {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic without a message".into());
        Terminal::Failed {
            reason: format!("internal error: {message}"),
        }
    }
}

pub struct RunConfig {
    pub workspace: PathBuf,
    pub run_dir: PathBuf,
    pub objective: String,
    pub mode: String,
    pub checks: Vec<String>,
    pub review: crate::review::Settings,
    pub sandbox: SandboxKind,
    /// Commands' network (`sandbox.network`; see [`crate::egress`]).
    pub network: crate::egress::Network,
    pub command_timeout: Duration,
    pub wall_clock: Duration,
    pub frontier_usd: f64,
    pub max_finish_attempts: u32,
    pub context_window: u64,
    pub mask_at: f64,
    /// Context compaction by the local model (`context.compaction`,
    /// `context.compact_at`, `context.compact_to`); `None`: off. It needs a
    /// local model, so without one the run only masks.
    pub compaction: Option<crate::compaction::Compaction>,
    pub max_output_tokens: u32,
    /// Sent as `reasoning_effort` unless `None`.
    pub reasoning_effort: Option<String>,
    /// Whether earlier responses' reasoning is sent back to the frontier
    /// (`frontier.resend_reasoning`). The transcript keeps it either way.
    pub resend_reasoning: bool,
    /// Prices a response's usage in dollars.
    pub price: Box<dyn Fn(&Usage) -> f64 + Send + Sync>,
    /// Operator approval of risky actions (`oversight.approve`, `git.commit`).
    pub oversight: crate::oversight::Oversight,
    /// Host-side web access for the web tools; `None` when `web.enabled` is off.
    pub web: Option<Arc<duet_web::Web>>,
    /// The operator's identity for `git_commit` (`git.author`); `None` reads
    /// it from git configuration when a commit is made.
    pub git_author: Option<duet_git::Identity>,
    /// MCP servers started for the run, with their tools; `None` when none are configured.
    pub mcp: Option<Arc<crate::mcp::Hub>>,
    /// The run's language servers (`code_nav`, `rename`); `None` when
    /// `lsp.enabled` is off or none is installed. See
    /// [`crate::code_nav::language_servers`].
    pub lsp: Option<Arc<duet_lsp::Lsp>>,
    /// Sub-agents (`delegate`); `None` when `subagents.enabled` is off, and
    /// always in a sub-agent's own configuration (they cannot delegate).
    pub subagents: Option<crate::subagents::Subagents>,
    /// The local explorer (`explore`); `None` when `explore.enabled` is off
    /// or the run has no local model, and in a sub-agent's configuration.
    pub explore: Option<Arc<crate::explore::Explorer>>,
    /// Image settings and the operator's attachments (`frontier.vision`,
    /// `images.max_side`, `duet run --image`).
    pub images: crate::images::ImageConfig,
    /// The owner's own project instructions (`DUET.md` next to the owner
    /// config), read at the start if the file exists; `None`: not read. The
    /// repository's `DUET.md` is read in any case (see [`crate::instructions`]).
    pub owner_instructions: Option<PathBuf>,
    /// Discovered skill references and explicit operator activations.
    pub skills: Arc<crate::skills::Skills>,
}

impl RunConfig {
    /// A run of `objective` in `workspace`, keeping its state in `run_dir`,
    /// with every other setting at its most restrictive value: no network,
    /// no checks, no web, MCP, language servers, sub-agents or explorer, no commit
    /// identity, no owner instructions, no images for the frontier
    /// (`frontier.vision` off), usage priced at zero, oversight at its
    /// default (`off`: nothing is asked, so `git_commit` is not offered),
    /// the platform's sandbox and small limits (60 s wall clock, $5, 30 s
    /// per command, 2 finish attempts, 1000 output tokens).
    ///
    /// The rule: a new field gets its restrictive default here, so code that
    /// does not set it can never loosen anything. Callers name only what they
    /// mean: `RunConfig { network: Network::All, ..RunConfig::new(ws, dir, task) }`.
    /// The CLI's run configuration and a sub-agent's (`child_config`) stay
    /// full literals, so a new field is a compile error there until it is
    /// decided.
    pub fn new(
        workspace: impl Into<PathBuf>,
        run_dir: impl Into<PathBuf>,
        objective: impl Into<String>,
    ) -> Self {
        RunConfig {
            workspace: workspace.into(),
            run_dir: run_dir.into(),
            objective: objective.into(),
            // A label (the transcript's start entry): never claim a boundary.
            mode: "passthrough".into(),
            checks: Vec::new(),
            review: crate::review::Settings::default(),
            // Either kind confines every command; one that is not installed
            // fails closed (`duet_sandbox::detect` finds the working one).
            sandbox: if cfg!(target_os = "linux") {
                SandboxKind::Bubblewrap
            } else {
                SandboxKind::Seatbelt
            },
            network: crate::egress::Network::Off,
            command_timeout: Duration::from_secs(30),
            wall_clock: Duration::from_secs(60),
            frontier_usd: 5.0,
            max_finish_attempts: 2,
            context_window: 200_000,
            mask_at: 0.7,
            compaction: None,
            max_output_tokens: 1000,
            reasoning_effort: None,
            resend_reasoning: true,
            price: Box::new(|_| 0.0),
            oversight: crate::oversight::Oversight::default(),
            web: None,
            git_author: None,
            mcp: None,
            lsp: None,
            subagents: None,
            explore: None,
            images: crate::images::ImageConfig::default(),
            owner_instructions: None,
            skills: Arc::new(crate::skills::Skills::default()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunStats {
    pub turns: u64,
    /// Billed usage of the frontier responses.
    pub usage: Usage,
    /// Estimated usage of frontier attempts that failed after output started
    /// and were retried; not part of `usage`, but charged in `cost_usd`.
    #[serde(default, skip_serializing_if = "is_unused")]
    pub failed_attempt_usage: Usage,
    pub cost_usd: f64,
    /// User-priced local input/output tokens, separate from the frontier budget.
    #[serde(default)]
    pub local_cost_usd: f64,
    /// Canceled local requests whose final token usage was not reported.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub local_usage_unknown_requests: u64,
    pub tool_calls: u64,
    pub masked_results: u64,
    /// Times the local model condensed the conversation (see `compaction`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub compactions: u64,
    pub wall_seconds: f64,
    /// Where the frontier input went, by how tool results were shown (see `ledger`).
    #[serde(default)]
    pub ledger: Ledger,
    /// What sub-agents did; their usage and cost are also in `usage`,
    /// `failed_attempt_usage`, `cost_usd` and `ledger`.
    #[serde(default, skip_serializing_if = "crate::subagents::Totals::is_empty")]
    pub subagents: crate::subagents::Totals,
}

const MAX_TEXT_ONLY_TURNS: u32 = 3;
const MAX_LENGTH_STOPS: u32 = 3;

/// Provider-specific request fields for the configured reasoning effort.
fn reasoning_extra(effort: Option<&str>) -> serde_json::Map<String, serde_json::Value> {
    let mut extra = serde_json::Map::new();
    if let Some(e) = effort {
        extra.insert("reasoning_effort".into(), e.into());
    }
    extra
}

/// The conversation with no response's reasoning text: every request then
/// shares one prefix with the next, and past reasoning (two fifths of the
/// input the XL runs re-sent) is not read again. Provider state a dialect
/// must get back unchanged (signed reasoning) is kept.
fn without_reasoning(items: &[Item]) -> Vec<Item> {
    let mut items = items.to_vec();
    for item in &mut items {
        if let Item::Assistant { reasoning, .. } = item {
            *reasoning = None;
        }
    }
    items
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_unused(u: &Usage) -> bool {
    u.input + u.cache_read + u.cache_write + u.output == 0
}

pub(crate) fn add(a: &mut Usage, b: &Usage) {
    a.input += b.input;
    a.cache_read += b.cache_read;
    a.cache_write += b.cache_write;
    a.output += b.output;
    a.reasoning += b.reasoning;
}

/// Runs to a terminal state. `resume` continues an existing transcript.
///
/// The run ends at the earlier of `cfg.wall_clock` from now and the frontier
/// provider's own deadline. A panic anywhere in the run (engine, tools, gate)
/// is caught here and ends it as `Failed` with an internal-error reason; the
/// transcript's end entry is written either way.
///
/// A full disk pauses the run: its state writes (transcript, write journal,
/// audit log, workspace writes) are retried in place until they succeed, the
/// run is interrupted or the deadline ends it (see `crate::host`).
///
/// The terminal state is returned in the operator's form
/// ([`Terminal::for_operator`]); the transcript records the frontier's.
pub async fn run(
    cfg: &RunConfig,
    frontier: &GatedFrontier,
    presenter: &dyn Presenter,
    git: &Git,
    resume: bool,
    interrupted: &Arc<AtomicBool>,
) -> (Terminal, RunStats) {
    let started = Instant::now();
    let own = started + cfg.wall_clock;
    let deadline = frontier.deadline().map_or(own, |d| d.min(own));
    let host = Arc::new(HostPolicy::until(
        deadline.into_std(),
        Some(interrupted.clone()),
    ));
    frontier.audit().set_wait(Some(host.clone()));
    let mut stats = RunStats::default();
    let driven = AssertUnwindSafe(drive(
        cfg,
        frontier,
        presenter,
        git,
        resume,
        interrupted,
        &mut stats,
        deadline,
        &host,
    ))
    .catch_unwind()
    .await;
    let terminal = match driven {
        Ok(Ok(t)) => t,
        Ok(Err(reason)) => Terminal::Failed { reason },
        Err(panic) => Terminal::internal_error(panic.as_ref()),
    };
    stats.wall_seconds = started.elapsed().as_secs_f64();
    stats.ledger.finish();
    frontier
        .audit()
        .set_wait(Some(Arc::new(HostPolicy::finishing())));
    if let Ok(t) = Transcript::open_waiting(&cfg.run_dir, Some(Arc::new(HostPolicy::finishing()))) {
        let _ = t.append(&Entry::End {
            terminal: terminal.clone(),
        });
    }
    if let Some(lsp) = &cfg.lsp {
        lsp.shutdown().await;
    }
    (terminal.for_operator(presenter), stats)
}

/// Whether a run can be resumed: it has a transcript and did not complete.
pub fn resumable(run_dir: &Path) -> Result<(), String> {
    let entries = Transcript::read(run_dir).map_err(|e| e.to_string())?;
    if !entries.iter().any(|e| matches!(e, Entry::Item { .. })) {
        return Err("nothing to resume: the transcript is empty".into());
    }
    let last_end = entries.iter().rev().find_map(|e| match e {
        Entry::End { terminal } => Some(terminal),
        _ => None,
    });
    match last_end {
        Some(Terminal::Completed { .. }) => Err("the run already completed".into()),
        _ => Ok(()),
    }
}

/// Ends a run's records: the audit log's end event (which anchors its final
/// head), then `summary.json` in the run directory, written privately. Returns
/// the summary. `audit_log` is the log's path, read for the disclosure report.
/// Both writes wait out a full disk for up to [`crate::host::FINAL_GRACE`].
/// Without hooks; see [`conclude_with`].
pub fn conclude(
    run_dir: &Path,
    run_id: &str,
    audit: Option<&AuditHandle>,
    audit_log: &Path,
    terminal: &Terminal,
    stats: &RunStats,
) -> Result<serde_json::Value, duet_fs::FsError> {
    let hooks = crate::embed::Hooks::default();
    let ending = crate::embed::Ending {
        kind: crate::embed::RunKind::Run,
        mode: "",
        resumed: false,
        policy: None,
        hooks: &hooks,
    };
    conclude_with(run_dir, run_id, audit, audit_log, terminal, stats, &ending)
}

/// [`conclude`], then the end hooks of `ending.hooks`, called once with an
/// [`crate::embed::EndReport`] whether or not `summary.json` could be
/// written. Every run and session invocation that created its run directory
/// must end here exactly once (the CLI's `execute` and `chat` do).
pub fn conclude_with(
    run_dir: &Path,
    run_id: &str,
    audit: Option<&AuditHandle>,
    audit_log: &Path,
    terminal: &Terminal,
    stats: &RunStats,
    ending: &crate::embed::Ending<'_>,
) -> Result<serde_json::Value, duet_fs::FsError> {
    let wait = Arc::new(HostPolicy::finishing());
    if let Some(a) = audit {
        a.set_wait(Some(wait.clone()));
        a.record(AuditEvent::RunEnd {
            terminal: terminal.state().into(),
        });
    }
    // The report is best effort: a defect in it must not cost the summary.
    let disclosure = std::panic::catch_unwind(|| {
        let lines = duet_boundary::audit::read(audit_log).ok()?;
        Some(crate::disclosure::Disclosure::build(
            &lines,
            Some(&stats.ledger),
        ))
    })
    .unwrap_or(None);
    let summary = serde_json::json!({
        "run_id": run_id,
        "terminal": terminal,
        "stats": stats,
        "disclosure": disclosure,
    });
    let bytes = serde_json::to_vec_pretty(&summary).unwrap_or_default();
    let written = duet_fs::host::persist(Some(wait.as_ref()), || {
        duet_fs::private::write_private(&run_dir.join("summary.json"), &bytes)
    });
    if ending.hooks.has_end_hooks() {
        let mut report =
            crate::embed::EndReport::new(run_id, ending, terminal, stats, audit_log.to_path_buf());
        report.chain = audit.map(AuditHandle::chain_head);
        report.resumable = resumable(run_dir).is_ok();
        report.disclosure = disclosure;
        ending.hooks.ended(&report, audit);
    }
    written?;
    Ok(summary)
}

/// How a failed frontier request ends the run: an outage that outlasted the
/// wall clock is a budget stop, an interrupt during retries is resumable, a
/// full disk while auditing the request is waited out like any state write,
/// and anything else (credentials, an invalid request, a blocked send) fails it.
fn stop_for(e: GateError, host: &HostPolicy) -> Result<Terminal, String> {
    match e {
        GateError::Provider(p) if p.kind == ErrorKind::Deadline => Ok(Terminal::out_of_time()),
        GateError::Provider(p) if p.kind == ErrorKind::Cancelled => Ok(Terminal::interrupted()),
        GateError::Audit(e) if e.is_host_resource() => write_failed(e, host),
        e => Err(format!("frontier: {e}")),
    }
}

/// How a state write that failed ends the run: a full disk is waited out
/// until the run was interrupted or its wall clock ended, so it ends as that;
/// any other failure fails the run.
fn write_failed(e: FsError, host: &HostPolicy) -> Result<Terminal, String> {
    if !e.is_host_resource() {
        return Err(e.to_string());
    }
    Ok(if host.gave_up_on_interrupt() {
        Terminal::interrupted()
    } else {
        Terminal::out_of_time()
    })
}

/// Charges the estimated usage of attempts that failed after output started:
/// to the dollar budget, the cost ledger and the transcript. Billed usage is
/// charged separately, so nothing is counted twice.
fn charge_failed_attempts(
    cfg: &RunConfig,
    stats: &mut RunStats,
    transcript: &Transcript,
    turn: u64,
    usage: &Usage,
) -> Result<(), FsError> {
    if is_unused(usage) {
        return Ok(());
    }
    let cost = (cfg.price)(usage);
    stats.cost_usd += cost;
    add(&mut stats.failed_attempt_usage, usage);
    stats.ledger.on_failed_usage(usage, &*cfg.price);
    transcript.append(&Entry::FailedAttempts {
        turn,
        usage: *usage,
        cost_usd: cost,
    })
}

/// Resolves once `flag` is set.
pub(crate) async fn raised(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Unwraps a state write, or stops as [`write_failed`] says.
macro_rules! stored {
    ($host:expr, $write:expr) => {
        match $write {
            Ok(v) => v,
            Err(e) => return write_failed(e, $host).map(Into::into),
        }
    };
}

/// A state write whose other failures are tolerated: only a full disk that
/// was not waited out stops the loop.
macro_rules! noted {
    ($host:expr, $write:expr) => {
        if let Err(e) = $write
            && e.is_host_resource()
        {
            return write_failed(e, $host).map(Into::into);
        }
    };
}

/// Drops a trailing assistant turn whose tool calls did not all get results
/// (the run stopped mid-turn), with the results it did get, so the frontier
/// re-decides that turn.
pub(crate) fn drop_unfinished_turn(items: &mut Vec<Item>) {
    let Some(at) = items
        .iter()
        .rposition(|i| matches!(i, Item::Assistant { .. }))
    else {
        return;
    };
    let Item::Assistant { tool_calls, .. } = &items[at] else {
        return;
    };
    if tool_calls.is_empty() {
        return;
    }
    let answered: HashSet<&str> = items[at + 1..]
        .iter()
        .filter_map(|i| match i {
            Item::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    if tool_calls.iter().any(|c| !answered.contains(c.id.as_str())) {
        items.truncate(at);
    }
}

/// What the frontier has seen so far, and how the loop ends a turn.
pub(crate) struct Conversation {
    pub(crate) scoped_instructions: crate::instructions::ScopedInstructions,
    pub(crate) system: String,
    pub(crate) specs: Vec<ToolSpec>,
    /// The git tools (`None`: the workspace is not a git repository).
    pub(crate) git_tools: Option<crate::git_tools::GitTools>,
    pub(crate) items: Vec<Item>,
    /// How each tool result was shown, by call id (for the ledger).
    pub(crate) classes: HashMap<String, ViewClass>,
    /// A session: the frontier ends a turn by replying to the operator (a
    /// message without tool calls, `reply` or `ask_operator`).
    pub(crate) interactive: bool,
    /// The operator selected read-only planning for this session.
    pub(crate) planning: bool,
    /// (Session) The operator's messages and stop request during a turn.
    pub(crate) steering: Option<Arc<crate::session::Steering>>,
    /// (Session) The current operator turn.
    pub(crate) exchange: u64,
    /// (Sub-agent) What makes this loop a sub-agent's: its id, tools, write
    /// scope and its parent's stop request.
    pub(crate) child: Option<Arc<crate::subagents::Child>>,
    /// What context compaction keeps between its events.
    pub(crate) context: crate::compaction::State,
}

/// Why the loop stopped.
pub(crate) enum Stop {
    /// A terminal state (in a session: the state the turn ended in).
    Terminal(Terminal),
    /// (Session) the frontier replied to the operator, or asked them a
    /// question, and waits for their next message.
    Reply { text: String, question: bool },
    /// (Session) the operator asked to stop after the current step.
    Stopped,
}

impl From<Terminal> for Stop {
    fn from(t: Terminal) -> Self {
        Stop::Terminal(t)
    }
}

/// Rebuilds the conversation and the run's usage from its transcript. A
/// trailing assistant turn whose tool calls did not all get results cannot be
/// continued and is dropped so the frontier re-decides it; so is one that
/// precedes an operator turn (a session turn that was interrupted).
pub(crate) fn replay(
    entries: Vec<Entry>,
    cfg: &RunConfig,
    conv: &mut Conversation,
    stats: &mut RunStats,
) {
    // Sub-agents' spend counts for the run, whether they ended or not.
    crate::subagents::replay(&entries, cfg, stats);
    replay_priced(entries, &*cfg.price, conv, stats);
    // The transcript holds images by digest; their bytes are in the run's store.
    crate::images::load(&cfg.run_dir, &mut conv.items);
}

/// [`replay`] of one conversation, its usage priced by `price`.
pub(crate) fn replay_priced(
    entries: Vec<Entry>,
    price: &dyn Fn(&Usage) -> f64,
    conv: &mut Conversation,
    stats: &mut RunStats,
) {
    for e in &entries {
        if let Entry::Shown { call_id, class } = e {
            conv.classes.insert(call_id.clone(), *class);
        }
    }
    // Masking and compaction are replayed where they happened, so the
    // conversation (and the ledger's carried tokens) are the run's own.
    // Masking recorded before positions were is not replayed.
    let mut calls: HashMap<String, ToolCall> = HashMap::new();
    // The item after a turn start or steering is the operator's message.
    let mut operator_next = false;
    for e in entries {
        match e {
            Entry::Item { item } => {
                match &item {
                    Item::Assistant { tool_calls, .. } => {
                        for c in tool_calls {
                            calls.insert(c.id.clone(), c.clone());
                        }
                    }
                    Item::ToolResult { call_id, content } => {
                        if let Some(c) = calls.get(call_id) {
                            let class = conv.classes.get(call_id).copied();
                            stats
                                .ledger
                                .on_result(c, content, class.unwrap_or(ViewClass::Raw));
                        }
                    }
                    Item::User { .. } if operator_next => {
                        conv.context.operator = Some(conv.items.len());
                        operator_next = false;
                    }
                    Item::User { .. } | Item::Images { .. } => {}
                }
                conv.items.push(item);
            }
            Entry::Masked {
                items, positions, ..
            } => {
                apply_mask(&mut conv.items, &positions);
                stats.masked_results += items as u64;
            }
            Entry::Compacted {
                head, upto, text, ..
            } => {
                crate::compaction::replace(&mut conv.items, &mut conv.context, head, upto, &text);
                stats.compactions += 1;
            }
            Entry::CompactionFailed { retry_at, .. } => conv.context.retry_at = Some(retry_at),
            Entry::Steered { .. } => operator_next = true,
            Entry::ReviewUsage { usage } => {
                stats.cost_usd += usage.cost_usd;
                add(&mut stats.usage, &usage.usage);
                add(&mut stats.failed_attempt_usage, &usage.failed_usage);
                stats.ledger.on_usage(&usage.usage, price);
                stats.ledger.on_failed_usage(&usage.failed_usage, price);
                stats.ledger.review.merge(&usage);
            }
            Entry::Usage {
                usage, cost_usd, ..
            } => {
                stats
                    .ledger
                    .on_request(&conv.items, &conv.system, &conv.classes);
                stats.ledger.on_usage(&usage, price);
                add(&mut stats.usage, &usage);
                stats.cost_usd += cost_usd;
                stats.turns += 1;
            }
            Entry::FailedAttempts {
                usage, cost_usd, ..
            } => {
                stats.ledger.on_failed_usage(&usage, price);
                add(&mut stats.failed_attempt_usage, &usage);
                stats.cost_usd += cost_usd;
            }
            Entry::TurnStart { .. } => {
                drop_unfinished_turn(&mut conv.items);
                operator_next = true;
            }
            Entry::Explored { stats: s, .. } => stats.ledger.on_explore(&s),
            _ => {}
        }
    }
    drop_unfinished_turn(&mut conv.items);
    if conv.context.operator.is_some_and(|k| k >= conv.items.len()) {
        conv.context.operator = None;
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    cfg: &RunConfig,
    frontier: &GatedFrontier,
    presenter: &dyn Presenter,
    git: &Git,
    resume: bool,
    interrupted: &AtomicBool,
    stats: &mut RunStats,
    deadline: Instant,
    host: &Arc<HostPolicy>,
) -> Result<Terminal, String> {
    let wait: Arc<dyn duet_fs::host::HostWait> = host.clone();
    let transcript = stored!(
        host,
        Transcript::open_waiting(&cfg.run_dir, Some(wait.clone()))
    );
    let name = cfg
        .workspace
        .file_name()
        .map_or("repository".into(), |n| n.to_string_lossy().into_owned());
    let git_tools = crate::git_tools::GitTools::for_run(git, cfg);
    let specs = tool_specs(cfg, presenter, git_tools.as_ref());
    let mut conv = Conversation {
        system: system_prompt(&name, &cfg.checks, &specs),
        specs,
        git_tools,
        items: Vec::new(),
        classes: HashMap::new(),
        interactive: false,
        planning: false,
        steering: None,
        exchange: 0,
        child: None,
        context: Default::default(),
        scoped_instructions: Default::default(),
    };
    if resume {
        let restored = stored!(
            host,
            duet_fs::host::persist(Some(wait.as_ref()), || {
                WriteJournal::recover(&cfg.run_dir, &cfg.workspace)
            })
        );
        if !restored.is_empty() {
            eprintln!("rolled back {} interrupted write(s)", restored.len());
        }
        let entries = Transcript::read(&cfg.run_dir).map_err(|e| e.to_string())?;
        replay(entries, cfg, &mut conv, stats);
        if conv.items.is_empty() {
            return Err("nothing to resume: the transcript is empty".into());
        }
        // Sub-agents whose results the conversation lacks are ended, and
        // their writes rolled back, before the frontier re-decides.
        if stats.subagents.children > 0 {
            stored!(
                host,
                crate::subagents::recover(cfg, frontier.audit(), &conv.items)
            );
        }
    } else {
        stored!(
            host,
            transcript.append(&Entry::Start {
                objective: cfg.objective.clone(),
                mode: cfg.mode.clone(),
                frontier_model: frontier.model().to_owned(),
            })
        );
        let mut text = presenter.sanitize_objective(&cfg.objective);
        if let Some(block) = crate::skills::block(cfg, presenter) {
            text.insert_str(0, &block);
        }
        // Project instructions come first, framed; the whole message is
        // replayed from the transcript on resume, so it never changes.
        if let Some(block) = crate::instructions::block(cfg, presenter, Some(frontier.audit())) {
            text.insert_str(0, &block);
        }
        // The operator's images: described ones become notes in the text,
        // the others follow it as an image item.
        let images = crate::images::attach_all(
            cfg,
            presenter,
            Some(frontier.audit()),
            &cfg.images.attached,
            &mut stats.ledger,
            &mut text,
        )?;
        let first = Item::User { text };
        stored!(
            host,
            transcript.append(&Entry::Item {
                item: first.clone(),
            })
        );
        conv.items.push(first);
        if !images.is_empty() {
            let item = Item::Images {
                call_id: None,
                images,
            };
            stored!(host, transcript.append(&Entry::Item { item: item.clone() }));
            conv.items.push(item);
        }
    }
    let limits = Limits {
        deadline,
        frontier_usd: cfg.frontier_usd,
    };
    match work(
        cfg,
        frontier,
        presenter,
        git,
        interrupted,
        &mut conv,
        stats,
        &limits,
        host,
    )
    .await?
    {
        Stop::Terminal(t) => Ok(t),
        Stop::Reply { .. } | Stop::Stopped => {
            Err("internal error: a session stop outside a session".into())
        }
    }
}

/// The run's tools: built-in, the presenter's, the configured web tools, the
/// git tools, the MCP servers' tools and the language-server tools. Fixed
/// for the run and sorted. A session rebuilds its advertised subset when
/// the operator explicitly changes between planning and execution.
pub(crate) fn tool_specs(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    git_tools: Option<&crate::git_tools::GitTools>,
) -> Vec<ToolSpec> {
    let mut extra = presenter.extra_tools();
    extra.extend(crate::skills::specs(&cfg.skills));
    extra.extend(
        cfg.web
            .as_deref()
            .map(crate::web::specs)
            .unwrap_or_default(),
    );
    extra.extend(git_tools.map(|g| g.specs()).unwrap_or_default());
    extra.extend(
        cfg.mcp
            .as_deref()
            .map(crate::mcp::Hub::specs)
            .unwrap_or_default(),
    );
    if cfg.lsp.is_some() {
        extra.extend(crate::code_nav::specs());
    }
    if cfg.subagents.is_some() {
        extra.push(crate::subagents::spec());
    }
    if cfg.explore.is_some() {
        extra.push(crate::explore::spec());
    }
    let mut specs = tools::specs_with(extra);
    if cfg.review.enabled
        && let Some(finish) = specs.iter_mut().find(|s| s.name == "finish")
    {
        finish.description.push_str(" The host also runs a bounded security review of this run's changes; local opinions are advisory. Configured rule-confirmed high-severity findings must be fixed before completion.");
    }
    // The command tool says what network its commands have.
    if let Some(run) = specs.iter_mut().find(|s| s.name == "run_command") {
        run.description = tools::run_command_description(&cfg.network);
    }
    specs
}

/// What stops the loop besides the frontier: the wall-clock deadline and the
/// total frontier spend (of the run, or of the session) it may reach.
pub(crate) struct Limits {
    pub(crate) deadline: Instant,
    pub(crate) frontier_usd: f64,
}

/// The frontier loop: requests, tool calls and their results until a
/// terminal state, or (in a session) until the frontier replies to the
/// operator.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn work(
    cfg: &RunConfig,
    frontier: &dyn Driver,
    presenter: &dyn Presenter,
    git: &Git,
    interrupted: &AtomicBool,
    conv: &mut Conversation,
    stats: &mut RunStats,
    limits: &Limits,
    host: &Arc<HostPolicy>,
) -> Result<Stop, String> {
    let deadline = limits.deadline;
    let wait: Arc<dyn duet_fs::host::HostWait> = host.clone();
    let transcript = stored!(
        host,
        Transcript::open_waiting(&cfg.run_dir, Some(wait.clone()))
    )
    .nested(conv.child.as_ref().map(|c| c.id.clone()));
    let mut journal = stored!(
        host,
        WriteJournal::open_waiting(&cfg.run_dir, Some(wait.clone()))
    );
    if let Some(child) = &conv.child {
        journal.confine(child.scope());
    }
    if conv.planning {
        journal.confine(crate::journal::WriteScope::default());
    }
    let review = if cfg.review.enabled && conv.child.is_none() && !conv.planning {
        let baseline = crate::review::Baseline::open(
            &cfg.workspace,
            &cfg.run_dir,
            stats.turns > 0,
            Some(host.as_ref()),
        );
        if interrupted.load(Ordering::SeqCst) {
            return Ok(Terminal::interrupted().into());
        }
        if Instant::now() >= deadline {
            return Ok(Terminal::out_of_time().into());
        }
        Some(baseline?)
    } else {
        None
    };
    // Read-mode `delegate` calls run together; the results of those after
    // the first wait here, by call id, until their turn to be recorded.
    let mut delegated: HashMap<String, Outcome> = HashMap::new();
    // When the provider enforces the deadline itself, it is given a moment to
    // report a request cut off at it (with the usage of its failed attempts).
    let cutoff = if frontier.deadline().is_some() {
        deadline + Duration::from_secs(2)
    } else {
        deadline
    };
    let (mut text_only, mut length_stops, mut finish_attempts) = (0u32, 0u32, 0u32);
    // Compaction needs a local model to write the summary.
    let compaction = cfg.compaction.filter(|_| presenter.can_condense());
    let items = &mut conv.items;
    let system = &conv.system;
    let classes = &mut conv.classes;

    loop {
        if interrupted.load(Ordering::SeqCst) {
            return Ok(Terminal::interrupted().into());
        }
        // A sub-agent stops at the same point when the operator stops its
        // parent's turn.
        if conv.child.as_ref().is_some_and(|c| c.stop_requested()) {
            return Ok(Stop::Stopped);
        }
        // The safe point of a session: every result of the last response is
        // recorded and the next request is not sent yet. A stop ends the
        // turn here; steering messages join the conversation here.
        if let Some(steering) = conv.steering.clone() {
            if steering.stop_requested() {
                return Ok(Stop::Stopped);
            }
            let messages = steering.deliver();
            if !messages.is_empty() {
                let item = crate::session::steering_item(
                    presenter,
                    frontier.audit(),
                    conv.exchange,
                    &messages,
                );
                stored!(
                    host,
                    transcript.append(&Entry::Steered {
                        exchange: conv.exchange,
                        after_request: stats.turns,
                        messages,
                    })
                );
                stored!(host, transcript.append(&Entry::Item { item: item.clone() }));
                conv.context.operator = Some(items.len());
                items.push(item);
            }
        }
        if Instant::now() >= deadline {
            return Ok(Terminal::out_of_time().into());
        }
        if stats.cost_usd >= limits.frontier_usd {
            return Ok(Terminal::BudgetStopped {
                which: "frontier_usd".into(),
            }
            .into());
        }
        // Context compaction first (masking, else a local summary; see
        // `crate::compaction`), then the window's own masking.
        if let Some(c) = compaction {
            let as_sent = |older: &[Item]| {
                frontier
                    .as_sent(&Request {
                        items: older.to_vec(),
                        tools: conv.specs.clone(),
                        ..Request::default()
                    })
                    .items
            };
            let event =
                crate::compaction::decide(items, &conv.context, system, c, &as_sent, presenter);
            // A summary written while the run was interrupted may be cut short.
            if interrupted.load(Ordering::SeqCst) {
                return Ok(Terminal::interrupted().into());
            }
            if let Some(event) = event {
                // Recorded first: what is not recorded is not applied, so a
                // resumed run rebuilds the same conversation.
                match transcript.append(&event.entry()) {
                    Ok(()) => {
                        crate::compaction::apply(items, &mut conv.context, &event);
                        match &event {
                            crate::compaction::Event::Masked { positions, .. } => {
                                stats.masked_results += positions.len() as u64;
                                conv.scoped_instructions.clear();
                            }
                            crate::compaction::Event::Compacted { .. } => {
                                stats.compactions += 1;
                                conv.scoped_instructions.clear();
                            }
                            crate::compaction::Event::Failed { .. } => {}
                        }
                        if let Some(a) = event.audit(conv.child.as_ref().map(|c| c.id.clone())) {
                            frontier.audit().record(a);
                        }
                    }
                    Err(e) if e.is_host_resource() => return write_failed(e, host).map(Into::into),
                    Err(_) => {}
                }
            }
        }
        let before = estimate(items, system);
        let positions = mask_from_estimate(items, before, cfg.context_window, cfg.mask_at);
        if !positions.is_empty() {
            conv.scoped_instructions.clear();
            stats.masked_results += positions.len() as u64;
            noted!(
                host,
                transcript.append(&Entry::Masked {
                    items: positions.len(),
                    tokens_before: before,
                    tokens_after: estimate(items, system),
                    positions,
                })
            );
        }
        let request = Request {
            system: system.clone(),
            items: if cfg.resend_reasoning {
                items.clone()
            } else {
                without_reasoning(items)
            },
            tools: conv.specs.clone(),
            max_output_tokens: Some(cfg.max_output_tokens),
            extra: reasoning_extra(cfg.reasoning_effort.as_deref()),
            ..Request::default()
        };
        // Infrastructure failures are retried inside `create` until the
        // deadline; an interrupt stops the wait at once.
        let sent = tokio::select! {
            r = tokio::time::timeout_at(cutoff, frontier.create(&request)) => r,
            () = raised(interrupted) => return Ok(Terminal::interrupted().into()),
        };
        let (response, interventions) = match sent {
            Err(_) => return Ok(Terminal::out_of_time().into()),
            Ok(Err(e)) => {
                if let GateError::Provider(p) = &e {
                    let turn = stats.turns + 1;
                    noted!(
                        host,
                        charge_failed_attempts(cfg, stats, &transcript, turn, &p.failed_usage)
                    );
                }
                return stop_for(e, host).map(Into::into);
            }
            Ok(Ok(r)) => r,
        };
        let cost = (cfg.price)(&response.usage);
        stats.turns += 1;
        stats.cost_usd += cost;
        add(&mut stats.usage, &response.usage);
        stats.ledger.on_request(&request.items, system, classes);
        stats.ledger.on_usage(&response.usage, &*cfg.price);
        let turn = stats.turns;
        noted!(
            host,
            transcript.append(&Entry::Usage {
                turn,
                usage: response.usage,
                cost_usd: cost,
                interventions,
            })
        );
        noted!(
            host,
            charge_failed_attempts(
                cfg,
                stats,
                &transcript,
                turn,
                &response.attempts.estimated_failed
            )
        );

        let assistant = response.to_item();
        stored!(
            host,
            transcript.append(&Entry::Item {
                item: assistant.clone(),
            })
        );
        items.push(assistant);

        match response.stop {
            StopReason::Length => {
                length_stops += 1;
                if length_stops > MAX_LENGTH_STOPS {
                    return Ok(Terminal::Failed {
                        reason: "responses repeatedly hit the output limit".into(),
                    }
                    .into());
                }
                // Tool calls in a truncated response are never executed.
                let nudge = Item::User {
                    text: length_nudge(cfg.max_output_tokens),
                };
                stored!(
                    host,
                    transcript.append(&Entry::Item {
                        item: nudge.clone(),
                    })
                );
                items.push(nudge);
                continue;
            }
            StopReason::ContentFilter => {
                return Ok(Terminal::Failed {
                    reason: "the provider's content filter stopped the response".into(),
                }
                .into());
            }
            _ => {}
        }

        if response.tool_calls.is_empty() {
            // In a session a message without tool calls is the reply.
            if conv.interactive && !response.text.trim().is_empty() {
                return Ok(Stop::Reply {
                    text: response.text.clone(),
                    question: false,
                });
            }
            text_only += 1;
            if text_only > MAX_TEXT_ONLY_TURNS {
                return Ok(Terminal::Failed {
                    reason: "the model stopped calling tools without finishing".into(),
                }
                .into());
            }
            let text = if conv.interactive {
                "Continue working with the tools. When you are done, call `reply` with what you did, or `ask_operator` if you need a decision."
            } else {
                "Continue working with the tools. When the task is complete, call `finish`."
            };
            let nudge = Item::User { text: text.into() };
            stored!(
                host,
                transcript.append(&Entry::Item {
                    item: nudge.clone(),
                })
            );
            items.push(nudge);
            continue;
        }
        text_only = 0;

        let mut finished = None;
        let mut replied: Option<(String, bool)> = None;
        let mut scoped_given = false;
        delegated.clear();
        for (i, call) in response.tool_calls.iter().enumerate() {
            stats.tool_calls += 1;
            // The image a `read_file` call returns for the frontier itself.
            let mut attached = Vec::new();
            // How the result was shown, when the call decides it itself.
            let mut shown_as = None;
            let mut ctx = Ctx {
                workspace: &cfg.workspace,
                run_dir: &cfg.run_dir,
                sandbox: cfg.sandbox,
                git,
                presenter,
                journal: &mut journal,
                command_timeout: cfg.command_timeout,
                network: &cfg.network,
                checks: &cfg.checks,
                audit: Some(frontier.audit()),
                interrupted: Some(interrupted),
                web: cfg.web.as_deref(),
                git_tools: conv.git_tools.as_ref(),
                lsp: cfg.lsp.as_deref(),
            };
            // Only what this call shows counts for it.
            let _ = presenter.take_view_class();
            let content = if finished.is_some() {
                "not run: the task was already finished".to_owned()
            } else if replied.is_some() {
                "not run: you already ended this turn with a message to the operator".to_owned()
            } else if conv.planning && !crate::planning::allows(&call.name) {
                format!("error: {}", crate::planning::REFUSAL)
            } else if call.arguments.is_empty() && call.raw_arguments.trim() != "{}" {
                format!(
                    "error: arguments are not valid JSON: {}",
                    call.raw_arguments.chars().take(200).collect::<String>()
                )
            } else if let Some(refusal) = conv.child.as_ref().and_then(|c| c.refuse(call)) {
                format!("error: {refusal}")
            } else if matches!(
                call.name.as_str(),
                "read_file" | "edit_file" | "write_file" | "edit_protected" | "rename"
            ) && let Some(path) = call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                && let Some(notes) = conv.scoped_instructions.for_path(
                    cfg,
                    presenter,
                    Some(frontier.audit()),
                    Path::new(path),
                )
            {
                scoped_given = true;
                format!(
                    "{notes}\n[duet] Read these scoped instructions before continuing. This file operation was not performed; repeat it after applying the guidance."
                )
            } else if scoped_given
                && !matches!(
                    call.name.as_str(),
                    "read_file"
                        | "list_files"
                        | "search"
                        | "diff"
                        | "list_skills"
                        | "load_skill"
                        | "code_nav"
                        | "git_status"
                        | "git_log"
                        | "git_show"
                        | "git_blame"
                        | "web_search"
                        | "web_fetch"
                )
            {
                "not run: new scoped instructions were provided in this response; read them before retrying actions".to_owned()
            } else if conv.interactive
                && let Some(to_operator) = crate::session::to_operator(&call.name, &call.arguments)
            {
                match to_operator {
                    Ok((text, question)) => {
                        replied = Some((text, question));
                        crate::session::DELIVERED.to_owned()
                    }
                    Err(e) => format!("error: {e}"),
                }
            } else if let Err(e) = match cfg.mcp.as_deref().filter(|h| h.owns(&call.name)) {
                Some(hub) => crate::oversight::decide(
                    &cfg.oversight,
                    hub.action(cfg.oversight.mode, &call.name, &call.arguments),
                    Some(frontier.audit()),
                ),
                None => crate::oversight::review(
                    &cfg.oversight,
                    &call.name,
                    &call.arguments,
                    presenter,
                    Some(frontier.audit()),
                ),
            } {
                format!("error: {e}")
            } else {
                let outcome = if crate::skills::owns(&call.name) {
                    match crate::skills::call(
                        cfg,
                        presenter,
                        Some(frontier.audit()),
                        &call.name,
                        &call.arguments,
                    ) {
                        Ok(text) => Outcome::Result(text),
                        Err(error) => Outcome::Error(error),
                    }
                } else if let Some(hub) = cfg.mcp.as_deref().filter(|h| h.owns(&call.name)) {
                    match hub
                        .call(
                            &call.name,
                            &call.arguments,
                            presenter,
                            Some(frontier.audit()),
                            Some(interrupted),
                        )
                        .await
                    {
                        Ok(text) => Outcome::Result(text),
                        Err(e) => Outcome::Error(e),
                    }
                } else if call.name == crate::subagents::DELEGATE
                    && let Some(subagents) = &cfg.subagents
                {
                    let outcome = match delegated.remove(&call.id) {
                        Some(o) => o,
                        None => {
                            let parent = crate::subagents::Parent {
                                cfg,
                                subagents,
                                frontier,
                                presenter,
                                git,
                                interrupted,
                                limits,
                                host,
                                specs: &conv.specs,
                                git_tools: conv.git_tools.as_ref(),
                                stop: conv.steering.clone(),
                            };
                            let mut done = crate::subagents::delegate(
                                &parent,
                                &response.tool_calls[i..],
                                stats,
                            )
                            .await;
                            let first = done.remove(&call.id).unwrap_or_else(|| {
                                Outcome::Error("internal error: the sub-agent did not run".into())
                            });
                            delegated.extend(done);
                            first
                        }
                    };
                    // A writing sub-agent added records to the same journal.
                    noted!(host, journal.refresh());
                    // The report is shown as public text (whatever the
                    // sub-agents' own results were shown as).
                    let _ = presenter.take_view_class();
                    outcome
                } else if call.name == crate::explore::EXPLORE
                    && let Some(explorer) = &cfg.explore
                {
                    let done = crate::explore::explore(
                        &mut ctx,
                        explorer,
                        &call.arguments,
                        interrupted,
                        deadline,
                    )
                    .await;
                    if let Some(s) = &done.stats {
                        stats.ledger.on_explore(s);
                        noted!(
                            host,
                            transcript.append(&Entry::Explored {
                                call_id: call.id.clone(),
                                stats: s.clone(),
                            })
                        );
                    }
                    // A local answer, whatever its parts were shown as.
                    let _ = presenter.take_view_class();
                    shown_as = Some(ViewClass::LocalAnswer);
                    done.outcome
                } else if conv.child.is_some() && call.name == "run_command" {
                    crate::subagents::read_only_command(&ctx, &call.arguments).await
                } else if call.name == "read_file" && crate::images::wants(&call.arguments) {
                    let placed = crate::images::read(
                        cfg,
                        presenter,
                        Some(frontier.audit()),
                        &call.arguments,
                    );
                    stats.ledger.on_image(placed.destination, placed.bytes);
                    attached = placed.images;
                    match placed.text {
                        Ok(text) => Outcome::Result(text),
                        Err(e) => Outcome::Error(e),
                    }
                } else {
                    tools::dispatch(&mut ctx, &call.name, &call.arguments).await
                };
                let outcome = match (outcome, review.as_ref()) {
                    (Outcome::Finished { summary }, Some(base)) => {
                        let mut review_settings = cfg.review.clone();
                        review_settings.remaining_usd =
                            (limits.frontier_usd - stats.cost_usd).max(0.0);
                        if let Some(second) = &review_settings.second {
                            second.restore_spent(stats.ledger.review.cost_usd).await;
                        }
                        let reviewed = crate::review::finish(
                            base,
                            &cfg.run_dir,
                            review_settings,
                            presenter,
                            frontier.audit(),
                            || interrupted.load(Ordering::SeqCst) || Instant::now() >= deadline,
                            Some(host.as_ref()),
                        )
                        .await;
                        let usage = presenter.take_review_usage();
                        add(&mut stats.usage, &usage.usage);
                        add(&mut stats.failed_attempt_usage, &usage.failed_usage);
                        stats.cost_usd += usage.cost_usd;
                        stats.ledger.on_usage(&usage.usage, cfg.price.as_ref());
                        stats
                            .ledger
                            .on_failed_usage(&usage.failed_usage, cfg.price.as_ref());
                        stats.ledger.review.merge(&usage);
                        if usage.calls > 0 {
                            noted!(host, transcript.append(&Entry::ReviewUsage { usage }));
                        }
                        if let Err(reason) = &reviewed {
                            frontier.audit().record(AuditEvent::SecurityReviewAborted {
                                reason: reason.clone(),
                            });
                        }
                        if interrupted.load(Ordering::SeqCst) {
                            return Ok(Terminal::interrupted().into());
                        }
                        if Instant::now() >= deadline {
                            return Ok(Terminal::out_of_time().into());
                        }
                        match reviewed {
                            Ok(r) if r.blocked => Outcome::ChecksFailed(r.text),
                            Ok(r) => Outcome::Finished {
                                summary: format!("{summary}\n\n{}", r.text),
                            },
                            Err(e) => Outcome::ChecksFailed(e),
                        }
                    }
                    (outcome, _) => outcome,
                };
                // An interrupt stops a running command at once (its process
                // tree is killed); the call is recorded, its result is not.
                if interrupted.load(Ordering::SeqCst)
                    && !matches!(outcome, Outcome::Finished { .. })
                {
                    noted!(
                        host,
                        transcript.append(&Entry::Interrupted {
                            call_id: call.id.clone(),
                            tool: call.name.clone(),
                        })
                    );
                    return Ok(Terminal::interrupted().into());
                }
                match outcome {
                    Outcome::Result(text) => text,
                    Outcome::Error(e) => format!("error: {e}"),
                    Outcome::Finished { summary } => {
                        finished = Some(summary);
                        if cfg.checks.is_empty() {
                            "finished; no acceptance checks configured".to_owned()
                        } else {
                            "finished; checks passed".to_owned()
                        }
                    }
                    Outcome::ChecksFailed(report) => {
                        finish_attempts += 1;
                        if finish_attempts >= cfg.max_finish_attempts {
                            return Ok(Terminal::Failed {
                                reason: format!(
                                    "checks still failing after {finish_attempts} finish attempts"
                                ),
                            }
                            .into());
                        }
                        format!("checks failed; the task is not complete:\n{report}")
                    }
                }
            };
            // A result produced while the run was interrupted or ran out of
            // time may be degraded (a local model call cut short), so it is
            // not recorded: the turn is re-decided on resume. Its writes are
            // already journaled.
            if finished.is_none() {
                if interrupted.load(Ordering::SeqCst) {
                    return Ok(Terminal::interrupted().into());
                }
                if Instant::now() >= deadline {
                    return Ok(Terminal::out_of_time().into());
                }
            }
            let class = shown_as
                .or_else(|| presenter.take_view_class())
                .unwrap_or(ViewClass::Raw);
            stats.ledger.on_result(call, &content, class);
            classes.insert(call.id.clone(), class);
            let result = Item::ToolResult {
                call_id: call.id.clone(),
                content,
            };
            stored!(
                host,
                transcript.append(&Entry::Item {
                    item: result.clone(),
                })
            );
            noted!(
                host,
                transcript.append(&Entry::Shown {
                    call_id: call.id.clone(),
                    class,
                })
            );
            items.push(result);
            if !attached.is_empty() {
                let images = Item::Images {
                    call_id: Some(call.id.clone()),
                    images: attached,
                };
                stored!(
                    host,
                    transcript.append(&Entry::Item {
                        item: images.clone(),
                    })
                );
                items.push(images);
            }
        }
        if let Some(summary) = finished {
            return Ok(Terminal::Completed { summary }.into());
        }
        if let Some((text, question)) = replied {
            return Ok(Stop::Reply { text, question });
        }
    }
}

/// What the frontier is told after a response hit the output limit. Seen on a
/// one-page game: the whole page in one `write_file` was cut off, and "smaller
/// steps" alone did not say how to write one file in several.
fn length_nudge(limit: u32) -> String {
    format!(
        "Your last response was cut off at the output limit ({limit} tokens), so none of its tool \
calls ran and nothing was written. Continue with smaller steps. A large file, even one the brief \
asks for as a single file, is written in parts: create it with `write_file` holding the first part, \
then add each further part with `edit_file` in a later response, each response well under the limit."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_terminal_roundtrips_and_only_the_exact_legacy_sentinel_is_normalized() {
        let open = Terminal::Open {
            reason: crate::session::SESSION_LEFT.into(),
        };
        let json = serde_json::to_value(&open).unwrap();
        assert_eq!(json["state"], "open");
        assert_eq!(serde_json::from_value::<Terminal>(json).unwrap(), open);
        let legacy = serde_json::json!({
            "state":"failed", "reason":crate::session::SESSION_LEFT,
        });
        assert_eq!(serde_json::from_value::<Terminal>(legacy).unwrap(), open);
        assert_eq!(open.state(), "open");
        assert_eq!(open.exit_code(), 0);

        for reason in [
            "actual provider failure".to_owned(),
            INTERRUPTED.to_owned(),
            format!("write failed: {}", crate::session::SESSION_LEFT),
            format!("{}; unable to save", crate::session::SESSION_LEFT),
        ] {
            let failed: Terminal = serde_json::from_value(serde_json::json!({
                "state":"failed", "reason":reason,
            }))
            .unwrap();
            assert!(matches!(failed, Terminal::Failed { .. }));
            assert_eq!(failed.state(), "failed");
            assert_eq!(failed.exit_code(), 1);
        }
        assert_eq!(
            Terminal::Completed {
                summary: String::new()
            }
            .exit_code(),
            0
        );
        assert_eq!(
            Terminal::BudgetStopped {
                which: "session.frontier_usd".into()
            }
            .exit_code(),
            3
        );
    }

    #[test]
    fn resume_charges_review_calls_without_adding_them_to_working_context() {
        let review = duet_boundary::review::UsageStats {
            calls: 2,
            usage: Usage {
                input: 100,
                output: 20,
                ..Default::default()
            },
            failed_usage: Usage {
                output: 5,
                ..Default::default()
            },
            seconds: 3.0,
            cost_usd: 0.125,
        };
        let entries = vec![
            Entry::Item {
                item: Item::User {
                    text: "task".into(),
                },
            },
            Entry::ReviewUsage {
                usage: review.clone(),
            },
        ];
        let entries = serde_json::from_slice(&serde_json::to_vec(&entries).unwrap()).unwrap();
        let mut conv = Conversation {
            system: "system".into(),
            specs: vec![],
            git_tools: None,
            items: vec![],
            classes: HashMap::new(),
            interactive: false,
            planning: false,
            steering: None,
            exchange: 0,
            child: None,
            context: Default::default(),
            scoped_instructions: Default::default(),
        };
        let mut stats = RunStats::default();
        replay_priced(
            entries,
            &|u| (u.input + u.output) as f64 / 1000.0,
            &mut conv,
            &mut stats,
        );
        assert_eq!(
            conv.items,
            vec![Item::User {
                text: "task".into()
            }]
        );
        assert_eq!(stats.turns, 0);
        assert_eq!(stats.ledger.request_tokens, 0);
        assert_eq!(stats.usage, review.usage);
        assert_eq!(stats.failed_attempt_usage, review.failed_usage);
        assert_eq!(stats.ledger.review, review);
        assert_eq!(stats.cost_usd, 0.125);
        assert!((stats.ledger.input_usd + stats.ledger.output_usd - 0.125).abs() < 1e-12);
    }

    #[test]
    fn a_new_run_configuration_turns_every_capability_off() {
        let cfg = RunConfig::new("/ws", "/ws/.duet/runs/r", "task");
        assert!(matches!(cfg.network, crate::egress::Network::Off));
        assert!(cfg.checks.is_empty());
        assert!(cfg.web.is_none() && cfg.mcp.is_none() && cfg.lsp.is_none());
        assert!(cfg.subagents.is_none() && cfg.git_author.is_none());
        assert!(cfg.explore.is_none());
        assert!(!cfg.images.frontier_vision && cfg.images.attached.is_empty());
        assert_eq!(cfg.oversight.mode, crate::oversight::ApproveMode::Off);
        assert_eq!(cfg.mode, "passthrough");
        let usage = Usage {
            input: 1_000_000,
            output: 1_000_000,
            ..Usage::default()
        };
        assert_eq!((cfg.price)(&usage), 0.0);
        assert!(cfg.frontier_usd <= 5.0 && cfg.wall_clock <= Duration::from_secs(60));
    }

    #[test]
    fn without_reasoning_drops_only_the_reasoning_text() {
        let items = vec![
            Item::User {
                text: "task".into(),
            },
            Item::Assistant {
                text: "reading".into(),
                reasoning: Some("long private chain of thought".into()),
                tool_calls: vec![],
                replay: None,
            },
        ];
        let sent = without_reasoning(&items);
        assert_eq!(sent[0], items[0]);
        match &sent[1] {
            Item::Assistant {
                text, reasoning, ..
            } => {
                assert_eq!(text, "reading");
                assert!(reasoning.is_none());
            }
            other => panic!("{other:?}"),
        }
        let cfg = RunConfig::new("/ws", "/ws/.duet/runs/r", "task");
        assert!(cfg.resend_reasoning, "on unless the operator turns it off");
    }

    #[test]
    fn a_cut_off_response_is_told_the_limit_and_how_to_write_a_file_in_parts() {
        let nudge = length_nudge(32_768);
        for want in [
            "output limit (32768 tokens)",
            "nothing was written",
            "`write_file` holding the first part",
            "`edit_file` in a later response",
        ] {
            assert!(nudge.contains(want), "{want}: {nudge}");
        }
    }
}
