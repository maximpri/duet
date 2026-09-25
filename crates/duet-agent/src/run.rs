// SPDX-License-Identifier: GPL-3.0-or-later
//! The frontier loop: one continuous conversation until a terminal state.

use crate::context::{estimate, mask_if_needed};
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
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Terminal {
    Completed { summary: String },
    Failed { reason: String },
    BudgetStopped { which: String },
}

/// The reason of a run stopped by an interrupt (resumable).
pub const INTERRUPTED: &str = "interrupted; resume with `duet resume`";

impl Terminal {
    /// The state name, as in `summary.json` and the audit log's end event.
    pub fn state(&self) -> &'static str {
        match self {
            Terminal::Completed { .. } => "completed",
            Terminal::Failed { .. } => "failed",
            Terminal::BudgetStopped { .. } => "budget_stopped",
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
    pub sandbox: SandboxKind,
    pub network: bool,
    pub command_timeout: Duration,
    pub wall_clock: Duration,
    pub frontier_usd: f64,
    pub max_finish_attempts: u32,
    pub context_window: u64,
    pub mask_at: f64,
    pub max_output_tokens: u32,
    /// Sent as `reasoning_effort` unless `None`.
    pub reasoning_effort: Option<String>,
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
    /// Image settings and the operator's attachments (`frontier.vision`,
    /// `images.max_side`, `duet run --image`).
    pub images: crate::images::ImageConfig,
}

impl RunConfig {
    /// A run of `objective` in `workspace`, keeping its state in `run_dir`,
    /// with every other setting at its most restrictive value: no network,
    /// no checks, no web, MCP, language servers or sub-agents, no commit
    /// identity, no images for the frontier (`frontier.vision` off), usage
    /// priced at zero, oversight at its default (`off`: nothing is asked, so
    /// `git_commit` is not offered), the platform's sandbox and small limits
    /// (60 s wall clock, $5, 30 s per command, 2 finish attempts, 1000
    /// output tokens).
    ///
    /// The rule: a new field gets its restrictive default here, so code that
    /// does not set it can never loosen anything. Callers name only what they
    /// mean: `RunConfig { network: true, ..RunConfig::new(ws, dir, task) }`.
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
            // Either kind confines every command; one that is not installed
            // fails closed (`duet_sandbox::detect` finds the working one).
            sandbox: if cfg!(target_os = "linux") {
                SandboxKind::Bubblewrap
            } else {
                SandboxKind::Seatbelt
            },
            network: false,
            command_timeout: Duration::from_secs(30),
            wall_clock: Duration::from_secs(60),
            frontier_usd: 5.0,
            max_finish_attempts: 2,
            context_window: 200_000,
            mask_at: 0.7,
            max_output_tokens: 1000,
            reasoning_effort: None,
            price: Box::new(|_| 0.0),
            oversight: crate::oversight::Oversight::default(),
            web: None,
            git_author: None,
            mcp: None,
            lsp: None,
            subagents: None,
            images: crate::images::ImageConfig::default(),
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
    pub tool_calls: u64,
    pub masked_results: u64,
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
pub fn conclude(
    run_dir: &Path,
    run_id: &str,
    audit: Option<&AuditHandle>,
    audit_log: &Path,
    terminal: &Terminal,
    stats: &RunStats,
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
    duet_fs::host::persist(Some(wait.as_ref()), || {
        duet_fs::private::write_private(&run_dir.join("summary.json"), &bytes)
    })?;
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
    /// (Session) The operator's messages and stop request during a turn.
    pub(crate) steering: Option<Arc<crate::session::Steering>>,
    /// (Session) The current operator turn.
    pub(crate) exchange: u64,
    /// (Sub-agent) What makes this loop a sub-agent's: its id, tools, write
    /// scope and its parent's stop request.
    pub(crate) child: Option<Arc<crate::subagents::Child>>,
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
    // The ledger is rebuilt from the transcript; masking done before the
    // interruption is not replayed, so carried tokens are an upper bound.
    let mut calls: HashMap<String, ToolCall> = HashMap::new();
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
                    Item::User { .. } | Item::Images { .. } => {}
                }
                conv.items.push(item);
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
            Entry::TurnStart { .. } => drop_unfinished_turn(&mut conv.items),
            _ => {}
        }
    }
    drop_unfinished_turn(&mut conv.items);
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
    let mut conv = Conversation {
        system: system_prompt(&name, &cfg.checks),
        specs: tool_specs(cfg, presenter, git_tools.as_ref()),
        git_tools,
        items: Vec::new(),
        classes: HashMap::new(),
        interactive: false,
        steering: None,
        exchange: 0,
        child: None,
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
/// for the run (or session) and sorted, so the request prefix never changes.
pub(crate) fn tool_specs(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    git_tools: Option<&crate::git_tools::GitTools>,
) -> Vec<ToolSpec> {
    let mut extra = presenter.extra_tools();
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
    tools::specs_with(extra)
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
        let before = estimate(items, system);
        let masked = mask_if_needed(items, system, cfg.context_window, cfg.mask_at);
        if masked > 0 {
            stats.masked_results += masked as u64;
            noted!(
                host,
                transcript.append(&Entry::Masked {
                    items: masked,
                    tokens_before: before,
                    tokens_after: estimate(items, system),
                })
            );
        }
        let request = Request {
            system: system.clone(),
            items: items.clone(),
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
                let nudge = Item::User { text: "Your last response was cut off at the output limit, so none of its tool calls ran. Continue with smaller steps.".into() };
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
        delegated.clear();
        for (i, call) in response.tool_calls.iter().enumerate() {
            stats.tool_calls += 1;
            // The image a `read_file` call returns for the frontier itself.
            let mut attached = Vec::new();
            let mut ctx = Ctx {
                workspace: &cfg.workspace,
                run_dir: &cfg.run_dir,
                sandbox: cfg.sandbox,
                git,
                presenter,
                journal: &mut journal,
                command_timeout: cfg.command_timeout,
                network: cfg.network,
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
            } else if call.arguments.is_empty() && call.raw_arguments.trim() != "{}" {
                format!(
                    "error: arguments are not valid JSON: {}",
                    call.raw_arguments.chars().take(200).collect::<String>()
                )
            } else if let Some(refusal) = conv.child.as_ref().and_then(|c| c.refuse(call)) {
                format!("error: {refusal}")
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
                let outcome = if let Some(hub) = cfg.mcp.as_deref().filter(|h| h.owns(&call.name)) {
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
                        "finished; checks passed".to_owned()
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
            let class = presenter.take_view_class().unwrap_or(ViewClass::Raw);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_run_configuration_turns_every_capability_off() {
        let cfg = RunConfig::new("/ws", "/ws/.duet/runs/r", "task");
        assert!(!cfg.network);
        assert!(cfg.checks.is_empty());
        assert!(cfg.web.is_none() && cfg.mcp.is_none() && cfg.lsp.is_none());
        assert!(cfg.subagents.is_none() && cfg.git_author.is_none());
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
}
