// SPDX-License-Identifier: GPL-3.0-or-later
//! Sessions: one continuing conversation between the operator and the
//! frontier in one workspace. Each operator message starts a turn; the
//! frontier works with the same tools and loop as a run, and the turn ends
//! when it replies to the operator (`reply`, or a message without tool
//! calls), asks them a question (`ask_operator`), finishes a task (`finish`,
//! with the configured checks), or stops in a terminal state (failure,
//! budget, interrupt). The session itself stays open until the operator
//! closes it.
//!
//! Everything is appended to the run's transcript as it happens, so a
//! session resumes after exit, interrupt or crash with its full context.
//! Operator messages cross the boundary like task text: the first is
//! sanitized with the task's notes, later ones with
//! [`Presenter::sanitize_message`]; each is recorded in the audit log as an
//! `operator_message` event (counts only) before the request that carries it.

use crate::host::HostPolicy;
use crate::journal::{self, WriteJournal};
use crate::prompt::session_prompt;
use crate::run::{
    Conversation, INTERRUPTED, Limits, RunConfig, RunStats, Stop, Terminal, replay, work,
};
use crate::transcript::{Entry, Transcript};
use duet_boundary::GatedFrontier;
use duet_boundary::audit::AuditEvent;
use duet_boundary::model::{Item, ToolSpec};
use duet_boundary::vault::Vault;
use duet_boundary::view::Presenter;
use duet_git::Git;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// The tool result of `reply` and `ask_operator`.
pub(crate) const DELIVERED: &str =
    "delivered to the operator; this turn ends here and continues with their next message";

/// Why a session invocation that did not close the session ended.
pub const SESSION_LEFT: &str = "session left open; continue it with `duet --resume`";

/// How an operator turn ended. Texts are in their local form: placeholders
/// the frontier wrote are restored for the operator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TurnEnd {
    /// The frontier replied and waits for the operator's next message.
    Replied {
        message: String,
    },
    /// The frontier asked the operator a question and waits for the answer.
    Asked {
        question: String,
    },
    /// `finish` was called and every check passed.
    Completed {
        summary: String,
    },
    Failed {
        reason: String,
    },
    /// A per-turn or session budget stopped the turn; `which` names the
    /// setting (`limits.frontier_usd`, `session.wall_clock_minutes`, ...).
    BudgetStopped {
        which: String,
    },
    /// The operator interrupted the turn; a running command was stopped.
    Interrupted,
    /// The operator asked duet to stop after the step it was taking.
    Stopped,
}

impl TurnEnd {
    pub fn state(&self) -> &'static str {
        match self {
            TurnEnd::Replied { .. } => "replied",
            TurnEnd::Asked { .. } => "asked",
            TurnEnd::Completed { .. } => "completed",
            TurnEnd::Failed { .. } => "failed",
            TurnEnd::BudgetStopped { .. } => "budget_stopped",
            TurnEnd::Interrupted => "interrupted",
            TurnEnd::Stopped => "stopped",
        }
    }
}

/// The session's own budgets; each turn is also bounded by the run limits in
/// [`RunConfig`] (`wall_clock`, `frontier_usd`), whichever ends first.
#[derive(Debug, Clone, Copy)]
pub struct SessionLimits {
    /// Frontier spend over the whole session, across invocations.
    pub frontier_usd: f64,
    /// Time the agent works over the whole session (time spent waiting for
    /// the operator does not count).
    pub working_time: Duration,
}

/// What the operator sends while a turn runs: steering messages, delivered
/// together and in order at the next safe point (after the results of the
/// current step are recorded, before the next frontier request), and a
/// request to stop the turn at that point.
#[derive(Default)]
pub struct Steering {
    queue: std::sync::Mutex<std::collections::VecDeque<String>>,
    stop: AtomicBool,
    /// Messages delivered during the current turn.
    delivered: std::sync::Mutex<Vec<String>>,
}

impl Steering {
    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::VecDeque<String>> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queues a message for the running turn.
    pub fn steer(&self, message: impl Into<String>) {
        self.lock().push_back(message.into());
    }

    /// Asks the running turn to end after its current step.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Takes the queued messages (in the order they were sent) without
    /// delivering them: what a turn ended before reaching.
    pub fn take(&self) -> Vec<String> {
        self.lock().drain(..).collect()
    }

    /// Takes the queued messages to deliver them now.
    pub(crate) fn deliver(&self) -> Vec<String> {
        let messages = self.take();
        self.delivered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(messages.iter().cloned());
        messages
    }

    fn delivered(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .delivered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// The conversation item that delivers steering messages, sanitized like any
/// operator message and audited as one.
pub(crate) fn steering_item(
    presenter: &dyn Presenter,
    audit: &duet_boundary::audit::AuditHandle,
    exchange: u64,
    messages: &[String],
) -> Item {
    let text = format!(
        "[duet] The operator sent this while you were working; take it into account from your \
next step:\n\n{}",
        messages.join("\n\n")
    );
    let sanitized = presenter.sanitize_message(&text);
    let placeholders = Vault::tokens_in(&sanitized)
        .len()
        .saturating_sub(Vault::tokens_in(&text).len());
    audit.record(AuditEvent::OperatorMessage {
        exchange,
        placeholders,
    });
    Item::User { text: sanitized }
}

/// One operator turn as the conversation view shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    pub number: u64,
    /// The operator's message as typed.
    pub message: String,
    /// Messages the operator steered the turn with, as typed.
    pub steered: Vec<String>,
    /// `None` while it runs, or when the session stopped during it.
    pub end: Option<TurnEnd>,
}

/// The tools a session adds to the run's tools.
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "ask_operator".into(),
            description: "Ask the operator one question and wait for the answer. Ends the turn; \
their answer arrives as the next message."
                .into(),
            parameters: json!({"type": "object", "properties": {
                "question": {"type": "string"},
                "options": {"type": "array", "items": {"type": "string"},
                    "description": "Answers to choose from, when there are a few clear ones."}
            }, "required": ["question"]}),
        },
        ToolSpec {
            name: "reply".into(),
            description: "Send the operator a message and end the turn: what you did or found, \
or your answer. Their next message continues the conversation."
                .into(),
            parameters: json!({"type": "object", "properties": {
                "message": {"type": "string"}
            }, "required": ["message"]}),
        },
    ]
}

/// A call of `reply` or `ask_operator`: the text for the operator and whether
/// it is a question. `None` for any other tool.
pub(crate) fn to_operator(
    name: &str,
    args: &Map<String, Value>,
) -> Option<Result<(String, bool), String>> {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("`{key}` is required"))
    };
    match name {
        "reply" => Some(text("message").map(|m| (m, false))),
        "ask_operator" => Some(text("question").map(|q| {
            let options: Vec<&str> = args
                .get("options")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if options.is_empty() {
                (q, true)
            } else {
                (format!("{q}\noptions: {}", options.join(" / ")), true)
            }
        })),
        _ => None,
    }
}

/// Whether `run_dir` holds a session (its transcript has an operator turn).
pub fn is_session(run_dir: &Path) -> bool {
    Transcript::read(run_dir)
        .map(|entries| entries.iter().any(|e| matches!(e, Entry::TurnStart { .. })))
        .unwrap_or(false)
}

/// The operator turns of a session, from its transcript.
pub fn exchanges(entries: &[Entry]) -> Vec<Exchange> {
    let mut out: Vec<Exchange> = Vec::new();
    for e in entries {
        match e {
            Entry::TurnStart {
                exchange, message, ..
            } => out.push(Exchange {
                number: *exchange,
                message: message.clone(),
                steered: Vec::new(),
                end: None,
            }),
            Entry::Steered {
                exchange, messages, ..
            } => {
                if let Some(x) = out.iter_mut().rev().find(|x| x.number == *exchange) {
                    x.steered.extend(messages.iter().cloned());
                }
            }
            Entry::TurnEnd { exchange, end, .. } => {
                if let Some(x) = out.iter_mut().rev().find(|x| x.number == *exchange) {
                    x.end = Some(end.clone());
                }
            }
            _ => {}
        }
    }
    out
}

/// A note to the frontier about what happened between turns, prepended to
/// the operator's next message.
fn interrupted_note() -> String {
    "[duet] The operator interrupted your previous turn. Tool calls that were still running were \
stopped and their results are missing; files may have changed. Check with `diff` before relying \
on earlier results."
        .into()
}

fn undone_note(from: u64, paths: &[PathBuf], run_dir: &Path) -> String {
    let list = paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut note = format!(
        "[duet] The operator reverted the file changes you made since turn {from}: {list} are back \
to their earlier content (created files were removed). Re-read files before editing them."
    );
    if crate::git_tools::has_committed(run_dir) {
        note.push_str(
            " Commits are never undone: a reverted file you committed now differs from the commit \
(git_status shows it).",
        );
    }
    note
}

/// A live session. The run-level settings in `cfg` apply to every turn.
pub struct Session<'a> {
    cfg: &'a RunConfig,
    frontier: &'a GatedFrontier,
    presenter: &'a dyn Presenter,
    git: &'a Git,
    interrupted: Arc<AtomicBool>,
    limits: SessionLimits,
    conv: Conversation,
    stats: RunStats,
    /// Operator turns so far (before this invocation too).
    exchange: u64,
    worked: Duration,
    /// Where each turn began in the write journal, oldest first; `undo` pops.
    marks: Vec<(u64, u64)>,
    notes: Vec<String>,
    history: Vec<Exchange>,
    steering: Arc<Steering>,
    /// Images the operator attached for their next message.
    images: Vec<crate::images::Attachment>,
}

impl<'a> Session<'a> {
    /// Starts a session (its transcript begins with `cfg.objective`, the
    /// operator's first message, which [`Session::turn`] is then called with)
    /// or, with `resume`, continues the one in `cfg.run_dir`: interrupted
    /// writes are rolled back and the conversation is rebuilt from the
    /// transcript.
    pub fn open(
        cfg: &'a RunConfig,
        frontier: &'a GatedFrontier,
        presenter: &'a dyn Presenter,
        git: &'a Git,
        interrupted: Arc<AtomicBool>,
        limits: SessionLimits,
        resume: bool,
    ) -> Result<Self, String> {
        let name = cfg
            .workspace
            .file_name()
            .map_or("repository".into(), |n| n.to_string_lossy().into_owned());
        let git_tools = crate::git_tools::GitTools::for_run(git, cfg);
        let mut specs = crate::run::tool_specs(cfg, presenter, git_tools.as_ref());
        specs.extend(missing_from(&specs));
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        let steering = Arc::new(Steering::default());
        let mut session = Self {
            cfg,
            frontier,
            presenter,
            git,
            interrupted,
            limits,
            conv: Conversation {
                system: session_prompt(&name, &cfg.checks, &specs),
                specs,
                git_tools,
                items: Vec::new(),
                classes: HashMap::new(),
                interactive: true,
                steering: Some(steering.clone()),
                exchange: 0,
                child: None,
                context: Default::default(),
            },
            stats: RunStats::default(),
            exchange: 0,
            worked: Duration::ZERO,
            marks: Vec::new(),
            notes: Vec::new(),
            history: Vec::new(),
            steering,
            images: if resume {
                Vec::new()
            } else {
                cfg.images.attached.clone()
            },
        };
        let wait: Arc<dyn duet_fs::host::HostWait> = Arc::new(HostPolicy::finishing());
        let transcript = Transcript::open_waiting(&cfg.run_dir, Some(wait.clone()))
            .map_err(|e| e.to_string())?;
        if !resume {
            transcript
                .append(&Entry::Start {
                    objective: cfg.objective.clone(),
                    mode: cfg.mode.clone(),
                    frontier_model: frontier.model().to_owned(),
                })
                .map_err(|e| e.to_string())?;
            return Ok(session);
        }
        let restored = duet_fs::host::persist(Some(wait.as_ref()), || {
            WriteJournal::recover(&cfg.run_dir, &cfg.workspace)
        })
        .map_err(|e| e.to_string())?;
        if !restored.is_empty() {
            eprintln!("rolled back {} interrupted write(s)", restored.len());
        }
        let entries = Transcript::read(&cfg.run_dir).map_err(|e| e.to_string())?;
        session.history = exchanges(&entries);
        let mut open_turn = false;
        for e in &entries {
            match e {
                Entry::TurnStart {
                    exchange,
                    journal_next,
                    ..
                } => {
                    session.exchange = session.exchange.max(*exchange);
                    session.marks.push((*exchange, *journal_next));
                    // The turn's message carried the notes pending before it.
                    session.notes.clear();
                    open_turn = true;
                }
                Entry::TurnEnd { seconds, end, .. } => {
                    session.worked += Duration::from_secs_f64(seconds.max(0.0));
                    open_turn = false;
                    if *end == TurnEnd::Interrupted {
                        session.notes.push(interrupted_note());
                    }
                }
                Entry::Undone { exchange, paths } => {
                    session.marks.retain(|(x, _)| x < exchange);
                    if !paths.is_empty() {
                        session
                            .notes
                            .push(undone_note(*exchange, paths, &cfg.run_dir));
                    }
                }
                _ => {}
            }
        }
        // A turn the process stopped during (a crash, a closed terminal)
        // counts as interrupted.
        if open_turn {
            session.notes.push(interrupted_note());
            if let Some(x) = session.history.last_mut() {
                x.end.get_or_insert(TurnEnd::Interrupted);
            }
        }
        replay(entries, cfg, &mut session.conv, &mut session.stats);
        if session.stats.subagents.children > 0 {
            crate::run::drop_unfinished_turn(&mut session.conv.items);
            crate::subagents::recover(cfg, frontier.audit(), &session.conv.items)
                .map_err(|e| e.to_string())?;
        }
        session.stats.wall_seconds = session.worked.as_secs_f64();
        Ok(session)
    }

    /// Where the operator's messages and stop requests for a running turn
    /// go (shared with the input side).
    pub fn steering(&self) -> Arc<Steering> {
        self.steering.clone()
    }

    pub fn stats(&self) -> &RunStats {
        &self.stats
    }

    /// Operator turns so far.
    pub fn turns(&self) -> u64 {
        self.exchange
    }

    pub fn history(&self) -> &[Exchange] {
        &self.history
    }

    /// Time the agent has worked in this session.
    pub fn worked(&self) -> Duration {
        self.worked
    }

    /// The session budget that is spent, if one is (no further turn starts).
    pub fn spent(&self) -> Option<&'static str> {
        if self.stats.cost_usd >= self.limits.frontier_usd {
            Some("session.frontier_usd")
        } else if self.worked >= self.limits.working_time {
            Some("session.wall_clock_minutes")
        } else {
            None
        }
    }

    /// Attaches an image to the operator's next message (`/image`), marked
    /// public or not. It is checked now (a readable image, and where it may
    /// go); the answer says where it will go, or why it cannot be attached.
    pub fn attach(&mut self, path: PathBuf, public: bool) -> Result<String, String> {
        let a = crate::images::Attachment { path, public };
        let said = crate::images::check_attachment(self.cfg, self.presenter, &a)?;
        self.images.push(a);
        Ok(said)
    }

    /// Images waiting for the operator's next message.
    pub fn attached(&self) -> &[crate::images::Attachment] {
        &self.images
    }

    /// Restores placeholders in a frontier text for the operator.
    fn local(&self, text: &str) -> String {
        self.presenter.detokenize(text)
    }

    /// One operator turn: `message` is added to the conversation (sanitized)
    /// and the frontier works until the turn ends. Never fails: every failure
    /// ends the turn in a state, and the session stays open.
    pub async fn turn(&mut self, message: &str) -> TurnEnd {
        if let Some(which) = self.spent() {
            return TurnEnd::BudgetStopped {
                which: which.into(),
            };
        }
        let started = Instant::now();
        self.interrupted.store(false, Ordering::SeqCst);
        // The turn ends at the earlier of its own wall clock and what is left
        // of the session's working time.
        let left = self.limits.working_time.saturating_sub(self.worked);
        let session_bounds_time = left < self.cfg.wall_clock;
        let deadline = started + left.min(self.cfg.wall_clock);
        let session_bounds_usd =
            self.limits.frontier_usd < self.stats.cost_usd + self.cfg.frontier_usd;
        let limits = Limits {
            deadline,
            frontier_usd: self
                .limits
                .frontier_usd
                .min(self.stats.cost_usd + self.cfg.frontier_usd),
        };
        let host = Arc::new(HostPolicy::until(
            deadline.into_std(),
            Some(self.interrupted.clone()),
        ));
        self.frontier.audit().set_wait(Some(host.clone()));
        self.exchange += 1;
        let exchange = self.exchange;
        self.conv.exchange = exchange;
        // A stop asked for after the last turn ended is not carried over.
        self.steering.stop.store(false, Ordering::SeqCst);
        self.history.push(Exchange {
            number: exchange,
            message: message.to_owned(),
            steered: Vec::new(),
            end: None,
        });
        let first = self.conv.items.is_empty();
        let stop = match self.begin(exchange, message, first, &host) {
            Err(reason) => Err(reason),
            Ok(()) => {
                let worked = AssertUnwindSafe(work(
                    self.cfg,
                    self.frontier,
                    self.presenter,
                    self.git,
                    &self.interrupted,
                    &mut self.conv,
                    &mut self.stats,
                    &limits,
                    &host,
                ))
                .catch_unwind()
                .await;
                match worked {
                    Ok(r) => r,
                    Err(panic) => Ok(Stop::Terminal(Terminal::internal_error(panic.as_ref()))),
                }
            }
        };
        let end = match stop {
            Err(reason) => TurnEnd::Failed { reason },
            Ok(Stop::Stopped) => TurnEnd::Stopped,
            Ok(Stop::Reply { text, question }) => {
                let text = self.local(&text);
                if question {
                    TurnEnd::Asked { question: text }
                } else {
                    TurnEnd::Replied { message: text }
                }
            }
            Ok(Stop::Terminal(t)) => match t {
                Terminal::Completed { summary } => TurnEnd::Completed {
                    summary: self.local(&summary),
                },
                Terminal::Failed { reason } if reason == INTERRUPTED => TurnEnd::Interrupted,
                Terminal::Failed { reason } => TurnEnd::Failed { reason },
                Terminal::BudgetStopped { which } => TurnEnd::BudgetStopped {
                    which: match which.as_str() {
                        "frontier_usd" if session_bounds_usd => "session.frontier_usd",
                        "frontier_usd" => "limits.frontier_usd",
                        _ if session_bounds_time => "session.wall_clock_minutes",
                        _ => "limits.wall_clock_minutes",
                    }
                    .into(),
                },
            },
        };
        if end == TurnEnd::Interrupted {
            self.notes.push(interrupted_note());
        }
        let seconds = started.elapsed().as_secs_f64();
        self.worked += started.elapsed();
        self.stats.wall_seconds = self.worked.as_secs_f64();
        let steered = self.steering.delivered();
        if let Some(x) = self.history.last_mut() {
            x.end = Some(end.clone());
            x.steered = steered;
        }
        let finishing: Arc<dyn duet_fs::host::HostWait> = Arc::new(HostPolicy::finishing());
        if let Ok(t) = Transcript::open_waiting(&self.cfg.run_dir, Some(finishing)) {
            let _ = t.append(&Entry::TurnEnd {
                exchange,
                seconds,
                end: end.clone(),
            });
        }
        end
    }

    /// Records the start of a turn and adds the operator's message to the
    /// conversation, sanitized and audited.
    fn begin(
        &mut self,
        exchange: u64,
        message: &str,
        first: bool,
        host: &Arc<HostPolicy>,
    ) -> Result<(), String> {
        let wait: Arc<dyn duet_fs::host::HostWait> = host.clone();
        // Writes a crash or an interrupt left half-done are rolled back first.
        duet_fs::host::persist(Some(wait.as_ref()), || {
            WriteJournal::recover(&self.cfg.run_dir, &self.cfg.workspace)
        })
        .map_err(|e| e.to_string())?;
        crate::run::drop_unfinished_turn(&mut self.conv.items);
        // A sub-agent of an interrupted turn: ended, its writes rolled back.
        if self.stats.subagents.children > 0 {
            crate::subagents::recover(self.cfg, self.frontier.audit(), &self.conv.items)
                .map_err(|e| e.to_string())?;
        }
        let journal_next = WriteJournal::open_waiting(&self.cfg.run_dir, Some(wait.clone()))
            .map_err(|e| e.to_string())?
            .next_record();
        let transcript =
            Transcript::open_waiting(&self.cfg.run_dir, Some(wait)).map_err(|e| e.to_string())?;
        transcript
            .append(&Entry::TurnStart {
                exchange,
                message: message.to_owned(),
                journal_next,
            })
            .map_err(|e| e.to_string())?;
        self.marks.push((exchange, journal_next));
        let mut text = std::mem::take(&mut self.notes).join("\n\n");
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(message);
        // The sensitive-path note and the brief go with the first message only.
        let sanitized = if first {
            self.presenter.sanitize_objective(&text)
        } else {
            self.presenter.sanitize_message(&text)
        };
        let placeholders = Vault::tokens_in(&sanitized)
            .len()
            .saturating_sub(Vault::tokens_in(&text).len());
        self.frontier.audit().record(AuditEvent::OperatorMessage {
            exchange,
            placeholders,
        });
        // Attached images: described ones become notes in the message, the
        // others follow it. One that cannot be attached fails the turn.
        let mut sanitized = sanitized;
        let attachments = std::mem::take(&mut self.images);
        let images = crate::images::attach_all(
            self.cfg,
            self.presenter,
            Some(self.frontier.audit()),
            &attachments,
            &mut self.stats.ledger,
            &mut sanitized,
        )?;
        // The project instructions start the session's first message.
        if first
            && let Some(block) =
                crate::instructions::block(self.cfg, self.presenter, Some(self.frontier.audit()))
        {
            sanitized.insert_str(0, &block);
        }
        let item = Item::User { text: sanitized };
        transcript
            .append(&Entry::Item { item: item.clone() })
            .map_err(|e| e.to_string())?;
        self.conv.context.operator = Some(self.conv.items.len());
        self.conv.items.push(item);
        if !images.is_empty() {
            let item = Item::Images {
                call_id: None,
                images,
            };
            transcript
                .append(&Entry::Item { item: item.clone() })
                .map_err(|e| e.to_string())?;
            self.conv.items.push(item);
        }
        Ok(())
    }

    /// Reverts the journaled writes of the last turn not yet undone: every
    /// file it wrote gets its content from before the turn back (a file it
    /// created is removed). Files changed by commands are not journaled and
    /// are not reverted. Returns the paths restored and the turn number.
    pub fn undo(&mut self) -> Result<(u64, Vec<PathBuf>), String> {
        let Some(&(exchange, from)) = self.marks.last() else {
            return Err("no turn to undo".into());
        };
        let run_dir = &self.cfg.run_dir;
        let ws = &self.cfg.workspace;
        WriteJournal::recover(run_dir, ws).map_err(|e| e.to_string())?;
        let files = journal::written_since(run_dir, from);
        let mut paths = Vec::new();
        for w in &files {
            let before = match &w.before {
                Some(p) => {
                    Some(std::fs::read(p).map_err(|e| format!("{}: {e}", w.path.display()))?)
                }
                None => None,
            };
            let now = duet_fs::read_optional(ws, &w.path, 64 * 1024 * 1024)
                .map_err(|e| format!("{}: {e}", w.path.display()))?;
            if now == before {
                continue;
            }
            duet_fs::restore(ws, &w.path, before.as_deref())
                .map_err(|e| format!("{}: {e}", w.path.display()))?;
            paths.push(w.path.clone());
        }
        Transcript::open(run_dir)
            .and_then(|t| {
                t.append(&Entry::Undone {
                    exchange,
                    paths: paths.clone(),
                })
            })
            .map_err(|e| e.to_string())?;
        self.marks.pop();
        if !paths.is_empty() {
            self.notes.push(undone_note(exchange, &paths, run_dir));
        }
        Ok((exchange, paths))
    }

    /// Ends this invocation: `closed` ends the session for good
    /// (`Completed`); otherwise it is left open to resume (`Failed` with
    /// [`SESSION_LEFT`], or `BudgetStopped` when a session budget is spent).
    /// The transcript's end entry is written; the caller concludes the run.
    pub fn end(mut self, closed: bool) -> (Terminal, RunStats) {
        let terminal = if closed {
            Terminal::Completed {
                summary: format!(
                    "session closed by the operator after {} turn(s)",
                    self.exchange
                ),
            }
        } else if let Some(which) = self.spent() {
            Terminal::BudgetStopped {
                which: which.into(),
            }
        } else {
            Terminal::Failed {
                reason: SESSION_LEFT.into(),
            }
        };
        self.stats.ledger.finish();
        let wait: Arc<dyn duet_fs::host::HostWait> = Arc::new(HostPolicy::finishing());
        if let Ok(t) = Transcript::open_waiting(&self.cfg.run_dir, Some(wait)) {
            let _ = t.append(&Entry::End {
                terminal: terminal.clone(),
            });
        }
        (terminal, self.stats)
    }
}

/// The session tools not already in `have` (a presenter's extra tools win).
fn missing_from(have: &[ToolSpec]) -> Vec<ToolSpec> {
    specs()
        .into_iter()
        .filter(|s| !have.iter().any(|e| e.name == s.name))
        .collect()
}
