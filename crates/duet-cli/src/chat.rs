// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet`: a session. On a terminal it is the workspace (full screen); without
//! one it is line by line.
//!
//! The operator types a message; duet works on it (progress lines show each
//! tool call and whatever the boundary withheld from the frontier) and the
//! turn ends with duet's reply, a question, a finished task or a stop. The
//! next message continues the same conversation. Lines starting with `/` are
//! commands for duet itself and never reach the frontier (`//` sends a
//! message that starts with `/`); a line ending with `\` continues on the
//! next line.
//!
//! Standard input is read by one thread into an inbox. A message typed while
//! duet works steers the running turn: it is delivered at the next safe
//! point (after the current step's results are recorded, before the next
//! frontier request; a running command is never cut short), and messages
//! queued meanwhile go together, in order. `/stop` ends the turn at that
//! point instead; Ctrl-C ends it now (a running command is killed). Either
//! way the session stays open. Approval questions (`oversight.approve`) are
//! answered with the next line typed after they are asked. When standard
//! input is not a terminal (the TUI drives a session through a pipe) nothing
//! is prompted and the output is the same.
//!
//! At the prompt, Ctrl-C twice leaves the session, like `/quit` or the end
//! of input; `duet --resume` continues it. `/close` ends it for good.
//!
//! On a terminal the workspace (`duet_tui::workspace`) reads the keyboard
//! instead of the reader thread: a status bar, the conversation with duet's
//! replies streamed and formatted as they arrive, and an input box with
//! history (the session's own messages) and completion. The lines it sends
//! and the Ctrl-C rules are the same.

use crate::approve;
use crate::term::words::{clip, describe_end, progress};
use crate::{
    Embedding, LocalOverride, Mode, Prepared, RunLimits, RunManifest, audit_log_path,
    checked_run_id, frontier_dialect, load_config, local_enabled, new_run_id, open_audit, prepare,
    setup,
};
use anyhow::{Context, Result, bail, ensure};
use duet_agent::oversight::Action;
use duet_agent::session::{Exchange, SessionLimits};
use duet_agent::transcript::Entry;
use duet_agent::{ApproveMode, Approver, RunStats, Session, Terminal, TurnEnd};
use duet_boundary::audit::AuditHandle;
use duet_boundary::view::{PassThrough, Presenter};
use duet_tui::workspace::{Hooks, Mode as Live, Status, Workspace};
use futures_util::FutureExt;
use futures_util::future::Either;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

mod attachment_queue;
mod plan_controls;
use plan_controls::{PlanAction, plan_action};

/// The session's arguments (`duet` without a command).
pub(crate) struct ChatArgs {
    pub message: Option<String>,
    pub goal: Option<String>,
    pub goal_turns: Option<u64>,
    pub plan_turns: Option<u64>,
    pub mode: Option<Mode>,
    pub frontier_url: Option<String>,
    pub frontier_model: Option<String>,
    pub no_privacy: bool,
    /// One-session acceptance commands, in addition to project checks.
    pub check: Vec<String>,
    /// `Some(None)`: the most recent open session.
    pub resume: Option<Option<String>>,
}

/// Lines typed by the operator, in order, each with a sequence number.
#[derive(Default)]
pub(crate) struct Inbox {
    queue: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Default)]
struct Queue {
    // Only operator lines have a sequence number and can answer a question.
    // Background attachment jobs and requeued commands must never answer one.
    lines: VecDeque<(Option<u64>, String)>,
    next: u64,
    closed: bool,
    /// An approval question waits for the operator's answer.
    answering: bool,
    /// Typed UI actions share the operator input sequence, never a text encoding.
    plan_actions: VecDeque<(Option<u64>, duet_tui::workspace::PlanAction)>,
    question_after: Option<u64>,
    question_id: Option<String>,
    question_held: Vec<String>,
}

enum OperatorInput {
    Line(Option<u64>, String),
    Plan(duet_tui::workspace::PlanAction),
}

impl Inbox {
    /// An inbox fed from standard input by a reader thread.
    fn stdin() -> Arc<Self> {
        let inbox = Arc::new(Self::default());
        let feed = inbox.clone();
        std::thread::spawn(move || feed.read_from(std::io::stdin().lock()));
        inbox
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads lines until the end of input; a line ending with `\` continues
    /// on the next one.
    pub(crate) fn read_from(&self, mut input: impl BufRead) {
        let mut pending = String::new();
        loop {
            let mut line = String::new();
            match input.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(part) = line.strip_suffix('\\') {
                pending.push_str(part);
                pending.push('\n');
                continue;
            }
            pending.push_str(line);
            self.push(std::mem::take(&mut pending));
        }
        if !pending.is_empty() {
            self.push(pending);
        }
        self.lock().closed = true;
        self.ready.notify_all();
    }

    fn push(&self, line: String) {
        let mut q = self.lock();
        let n = q.next;
        q.next += 1;
        q.lines.push_back((Some(n), line));
        drop(q);
        self.ready.notify_all();
    }

    /// Queue work from a background UI job without making it an approval answer.
    fn push_deferred(&self, line: String) {
        self.lock().lines.push_back((None, line));
        self.ready.notify_all();
    }

    fn pop(&self) -> Option<String> {
        self.pop_tagged().map(|(_, line)| line)
    }

    fn pop_tagged(&self) -> Option<(Option<u64>, String)> {
        let mut q = self.lock();
        if Self::action_ready(&q) {
            return None;
        }
        q.lines.pop_front()
    }

    fn action_ready(q: &Queue) -> bool {
        q.plan_actions.front().is_some_and(|(seq, _)| {
            seq.is_none()
                || q.lines.front().is_none_or(|(line_seq, _)| {
                    line_seq.is_some_and(|n| seq.is_some_and(|s| s < n))
                })
        })
    }

    fn push_plan_action(&self, action: duet_tui::workspace::PlanAction) {
        let mut q = self.lock();
        let seq = q.next;
        q.next += 1;
        q.plan_actions.push_back((Some(seq), action));
        self.ready.notify_all();
    }

    /// Replay an already selected control before later input after a runtime handoff.
    fn requeue_plan_action(&self, action: duet_tui::workspace::PlanAction) {
        self.lock().plan_actions.push_front((None, action));
        self.ready.notify_all();
    }

    fn pop_input(&self) -> Option<OperatorInput> {
        let mut q = self.lock();
        if Self::action_ready(&q) {
            q.plan_actions
                .pop_front()
                .map(|(_, action)| OperatorInput::Plan(action))
        } else {
            q.lines
                .pop_front()
                .map(|(seq, line)| OperatorInput::Line(seq, line))
        }
    }

    fn plan_action_ready(&self) -> bool {
        Self::action_ready(&self.lock())
    }

    fn reject_dependent_plan_action(&self) -> bool {
        let mut q = self.lock();
        if Self::action_ready(&q)
            && q.plan_actions
                .front()
                .is_some_and(|(_, action)| attachment_queue::plan_requests_turn(action))
        {
            q.plan_actions.pop_front();
            true
        } else {
            false
        }
    }

    fn question_mark(&self, id: &str) {
        let mut q = self.lock();
        if q.question_id.as_deref() == Some(id) {
            return;
        }
        q.question_after = Some(q.next);
        q.question_id = Some(id.to_owned());
    }

    fn fresh_answer(&self, seq: Option<u64>) -> bool {
        let q = self.lock();
        seq.zip(q.question_after)
            .is_some_and(|(seq, mark)| seq >= mark)
    }

    fn hold_for_question(&self, line: String) {
        self.lock().question_held.push(line);
    }

    fn release_question_input(&self) {
        let mut q = self.lock();
        q.question_after = None;
        q.question_id = None;
        let held = std::mem::take(&mut q.question_held);
        for line in held.into_iter().rev() {
            q.lines.push_front((None, line));
        }
    }

    fn discard_stale_question(&self, id: Option<&str>) -> Vec<String> {
        let mut q = self.lock();
        if q.question_id.as_deref().is_none_or(|old| Some(old) == id) {
            return Vec::new();
        }
        q.question_id = None;
        q.question_after = None;
        std::mem::take(&mut q.question_held)
    }

    /// The input ended (the workspace's Ctrl-D, or its terminal is gone).
    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// Whether an approval question waits for the next line.
    fn answering(&self) -> bool {
        self.lock().answering
    }

    /// Takes the next line unless an approval question is waiting for it.
    fn pop_unless_answering(&self) -> Option<String> {
        let mut q = self.lock();
        if q.answering || Self::action_ready(&q) {
            return None;
        }
        q.lines.pop_front().map(|(_, l)| l)
    }

    /// Puts lines back at the front, in order (commands held for later).
    fn requeue(&self, lines: Vec<String>) {
        let mut q = self.lock();
        for l in lines.into_iter().rev() {
            q.lines.push_front((None, l));
        }
    }

    /// Whether the input ended and every line was taken.
    fn drained(&self) -> bool {
        let q = self.lock();
        q.closed && q.lines.is_empty() && q.plan_actions.is_empty()
    }

    /// The sequence number the next line will get; from now on lines are
    /// held for the answer until [`Inbox::answer_after`] returns.
    fn mark(&self) -> u64 {
        let mut q = self.lock();
        q.answering = true;
        q.next
    }

    /// Waits for the first line typed at or after `mark` and takes it (lines
    /// typed before stay queued as messages). `None` when `stop` is raised or
    /// the input ends first.
    fn answer_after(&self, mark: u64, stop: &AtomicBool) -> Option<String> {
        let mut q = self.lock();
        loop {
            if !q.plan_actions.is_empty() {
                stop.store(true, Ordering::SeqCst);
                q.answering = false;
                return None;
            }
            if let Some(i) = q
                .lines
                .iter()
                .position(|(n, _)| n.is_some_and(|n| n >= mark))
            {
                let (_, line) = q.lines.remove(i).expect("selected approval answer exists");
                if let Command::Plan(raw) = parse(&line) {
                    // Keep the approval barrier until both the execution stop
                    // and the queued control are visible. Otherwise the turn
                    // driver could steer following input ahead of this switch.
                    if !plan_action(&raw).inspect_only() {
                        stop.store(true, Ordering::SeqCst);
                    }
                    q.lines.push_front((None, line.clone()));
                }
                q.answering = false;
                return Some(line);
            }
            if q.closed || stop.load(Ordering::SeqCst) {
                q.answering = false;
                return None;
            }
            q = self
                .ready
                .wait_timeout(q, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// Asks approval questions in the conversation: the answer is the next line
/// typed after the question. An interrupt or the end of input denies.
struct AskInline {
    mode: ApproveMode,
    inbox: Arc<Inbox>,
    interrupted: Arc<AtomicBool>,
    screen: Screen,
}

impl AskInline {
    fn ask(&self, action: &Action) -> bool {
        let mark = self.inbox.mark();
        self.screen.ask(&approve::describe(self.mode, action));
        let answer = self.inbox.answer_after(mark, &self.interrupted);
        if let Some(line) = answer.as_ref()
            && matches!(parse(line), Command::Plan(_))
        {
            // answer_after atomically preserved the command and stopped work
            // when needed before releasing the approval-input barrier.
            self.screen.line("denied; planning command queued");
            return false;
        }
        let yes = answer.as_deref().is_some_and(approve::is_yes);
        self.screen.line(if yes { "approved" } else { "denied" });
        yes
    }
}

impl Approver for AskInline {
    fn approve(&self, action: &Action) -> bool {
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => {
                tokio::task::block_in_place(|| self.ask(action))
            }
            _ => self.ask(action),
        }
    }
}

/// What a line typed at the prompt asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Message(String),
    Skills,
    Skill(String),
    Plugins,
    PluginPrompt(String),
    Goal(String),
    Plan(String),
    Answer(String),
    History(Option<String>),
    /// End the running turn after its current step.
    Stop,
    Status,
    Privacy,
    Diff,
    Undo,
    Help,
    /// Leave the session open (resumable).
    Quit,
    /// End the session for good.
    Close,
    /// Attach an image to the next message (`/image [--public] <path>`).
    Image {
        path: String,
        public: bool,
    },
    Attach(String),
    Attachments,
    Detach(String),
    /// Show the mode, or switch to top clearance (`/mode [top-clearance]`).
    Mode(Option<String>),
    Unknown(String),
    Empty,
}

/// The commands, for completion.
pub(crate) const COMMANDS: &[&str] = &[
    "/attach",
    "/close",
    "/diff",
    "/exit",
    "/goal",
    "/plan",
    "/answer",
    "/help",
    "/history",
    "/image",
    "/mode",
    "/quit",
    "/status",
    "/privacy",
    "/stop",
    "/undo",
    "/skills",
    "/skill",
    "/plugins",
    "/command",
    "/attachments",
    "/detach",
];

fn attachment_command(raw: &str) -> Command {
    if crate::images::named_image(raw) {
        Command::Image {
            path: raw.to_owned(),
            public: false,
        }
    } else {
        Command::Attach(raw.to_owned())
    }
}

pub(crate) fn parse(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Empty;
    }
    if let Some(rest) = trimmed.strip_prefix("//") {
        return Command::Message(format!("/{rest}"));
    }
    let Some(word) = trimmed.strip_prefix('/') else {
        if crate::attachments::is_path(trimmed) || crate::images::is_path(trimmed) {
            return attachment_command(trimmed);
        }
        return Command::Message(line.trim_end().to_owned());
    };
    for (prefix, kind) in [
        ("skill", Command::Skill as fn(String) -> Command),
        ("command", Command::PluginPrompt),
        ("plan", Command::Plan),
        ("answer", Command::Answer),
    ] {
        if let Some(rest) = word.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            return kind(rest.trim().to_owned());
        }
    }
    if let Some(rest) = word.strip_prefix("goal")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        return Command::Goal(rest.trim().to_owned());
    }
    if let Some(rest) = word.strip_prefix("history")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        let rest = rest.trim();
        return Command::History((!rest.is_empty()).then(|| rest.to_owned()));
    }
    if let Some(rest) = word.strip_prefix("attach")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        return attachment_command(rest.trim());
    }
    if let Some(rest) = word.strip_prefix("detach")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        return Command::Detach(rest.trim().to_owned());
    }
    if let Some(rest) = word.strip_prefix("image")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        let rest = rest.trim();
        let (public, path) = match rest.strip_prefix("--public") {
            Some(p) if p.is_empty() || p.starts_with(char::is_whitespace) => (true, p.trim()),
            _ => (false, rest),
        };
        return Command::Image {
            path: path.to_owned(),
            public,
        };
    }
    if let Some(rest) = word.strip_prefix("mode")
        && (rest.is_empty() || rest.starts_with(char::is_whitespace))
    {
        let rest = rest.trim();
        return Command::Mode((!rest.is_empty()).then(|| rest.to_owned()));
    }
    match word {
        "skills" => Command::Skills,
        "plugins" => Command::Plugins,
        "attachments" => Command::Attachments,
        "status" => Command::Status,
        "privacy" => Command::Privacy,
        "diff" => Command::Diff,
        "undo" => Command::Undo,
        "stop" => Command::Stop,
        "help" | "?" => Command::Help,
        "quit" | "exit" => Command::Quit,
        "close" => Command::Close,
        _ if crate::attachments::is_path(trimmed) => attachment_command(trimmed),
        other => Command::Unknown(other.to_owned()),
    }
}

const HELP: &str = "\
Type a message and press Enter; end a line with \\ to continue on the next.
While duet works, a message steers it: it arrives after the current step.
Workspace commands (task text and loaded guidance use the session privacy boundary):
  /skills                list portable workflows found on this machine and project
  /skill <name> [task]    use a workflow; instructions load only when needed
  /plugins               list installed extension packages
  /command plugin:name [arguments]  use a packaged command
  /plan [task]           plan with read-only tools; pause automatic goals
  /plan review|edit      review or edit the saved plan
  /plan revise <text>    request a new plan revision
  /plan approve rN       approve the displayed revision without executing
  /plan implement rN     approve and run all steps within the session budgets
  /plan pause|resume rN  pause or explicitly resume plan execution
  /plan status|off       inspect mode or leave planning without executing
  /answer QID 1          select a question option; --text <answer> supplies text
  /goal <objective>       work automatically; default 20 turns and session budgets
  /goal                  show progress and remaining turns
  /goal pause|resume     pause work or continue an unfinished goal
  /goal cancel           abandon a goal without marking it complete
  /history [session-id]  browse saved work or read a conversation
  /stop    end duet's turn after its current step (Ctrl-C ends it now)
  /privacy offline preview of file rules, destinations and policy exceptions
  /status  turns, tokens, cost and time against the budgets
  /diff    what changed in the workspace (sensitive files are only named)
  /undo    revert the file writes of the last turn (repeat for earlier turns)
  /attach <path>          attach a text file or image; dropped paths work too
  /attachments           list files and images waiting for your next message
  /detach ID|all         remove pending attachments (/attachments shows IDs)
  /image <path>           attach an image to your next message; in hybrid mode the
                          local model describes it (needs local.vision)
  /image --public <path>  attach an image the frontier may see itself (not scanned;
                          never from a sensitive path; needs frontier.vision)
  /mode    the session's mode (hybrid, top clearance or passthrough)
  /mode top-clearance     continue in top clearance: this session stays open and
                          a new one starts where only the local model works and
                          nothing leaves this machine (no frontier, web or network)
  /quit    leave; the session stays open for `duet --resume`
  /close   end the session for good
  //text   send a message that starts with /
Ctrl-C stops duet's turn at once (a running command is killed);
at the prompt, Ctrl-C twice leaves.";

#[derive(Debug, PartialEq, Eq)]
enum GoalAction<'a> {
    Status,
    Pause,
    Resume,
    Cancel,
    Start(&'a str),
}

fn goal_action(raw: &str) -> GoalAction<'_> {
    match raw.trim() {
        "" | "status" => GoalAction::Status,
        "pause" => GoalAction::Pause,
        "resume" => GoalAction::Resume,
        "cancel" => GoalAction::Cancel,
        "start" => GoalAction::Status,
        text => GoalAction::Start(text.strip_prefix("start ").unwrap_or(text).trim()),
    }
}

fn show_history(ws: &Path, id: Option<String>, screen: &Screen) {
    let result = match id {
        None => crate::history::summary(ws),
        Some(id) => crate::history::render(
            ws,
            crate::history::Args {
                id: Some(id),
                limit: 20,
                search: None,
                json: false,
            },
        ),
    };
    match result {
        Ok(text) => screen.line(&text),
        Err(e) => screen.line(&format!(
            "History: {}",
            duet_tui::term::safe(&format!("{e:#}"))
        )),
    }
}

fn plugin_command(cfg: &duet_config::Config, raw: &str) -> Result<String> {
    ensure!(
        cfg.bool("extensions.plugins_enabled")?,
        "plugins are disabled for this workspace"
    );
    let (name, arguments) = raw
        .trim()
        .split_once(char::is_whitespace)
        .unwrap_or((raw.trim(), ""));
    let packages = crate::plugins::active_packages(&crate::plugins::store()?)?;
    crate::plugins::command(&packages, name, arguments)
}

/// Validate an explicit workflow before bootstrap, provider requests or tool-server startup.
fn preflight_extension(ws: &Path, cfg: &duet_config::Config, text: &str) -> Result<()> {
    match parse(text) {
        Command::Skill(raw) => {
            ensure!(
                cfg.bool("extensions.skills_enabled")?,
                "skills are disabled for this workspace"
            );
            let runtime = crate::extensions::discover(ws, cfg)?;
            crate::extensions::invoke(&runtime.skills, &raw)?;
        }
        Command::PluginPrompt(raw) => {
            plugin_command(cfg, &raw)?;
        }
        _ => {}
    }
    Ok(())
}

/// Where the conversation is written: line by line on standard output
/// (without a terminal, or when it cannot be driven: exactly as it always
/// was), or the workspace.
#[derive(Clone)]
pub(crate) enum Screen {
    Plain { tty: bool },
    Live(Arc<Workspace>),
}

impl Screen {
    /// A line (or lines) of the conversation.
    fn line(&self, text: &str) {
        match self {
            Screen::Plain { .. } => println!("{text}"),
            Screen::Live(c) => c.lines(text),
        }
    }

    /// Text shown as it is (it ends its own lines).
    fn text(&self, text: &str) {
        match self {
            Screen::Plain { .. } => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            Screen::Live(c) => c.text(text),
        }
    }

    /// An approval question; the answer is the next line.
    fn ask(&self, question: &str) {
        match self {
            Screen::Plain { .. } => self.text(&format!("{question}approve? answer y or n: ")),
            // The input line asks (`approve? y/n>`).
            Screen::Live(c) => c.text(question),
        }
    }

    fn diff(&self, text: &str) {
        match self {
            Screen::Plain { .. } => print!("{text}"),
            Screen::Live(c) => c.diff(text),
        }
    }

    /// What Ctrl-C did.
    fn notice(&self, text: &str) {
        match self {
            Screen::Plain { .. } => eprintln!("\n{text}"),
            Screen::Live(c) => c.lines(text),
        }
    }

    /// How a turn ended.
    fn end(&self, end: &TurnEnd) {
        match self {
            Screen::Plain { .. } => println!("{}", describe_end(end)),
            Screen::Live(c) => c.end(end),
        }
    }

    /// Waits for the operator's next line.
    fn prompt(&self) {
        match self {
            Screen::Plain { tty: true } => {
                print!("you> ");
                let _ = std::io::stdout().flush();
            }
            Screen::Plain { tty: false } => {}
            Screen::Live(c) => c.mode(Live::Prompt),
        }
    }

    fn hide(&self) {
        if let Screen::Live(c) = self {
            c.mode(Live::Hidden);
        }
    }

    fn workspace(&self) -> Option<&Arc<Workspace>> {
        match self {
            Screen::Live(c) => Some(c),
            Screen::Plain { .. } => None,
        }
    }

    /// Gives the terminal back (the final lines are printed after it).
    fn close(&self) {
        if let Screen::Live(c) = self {
            c.stop();
        }
    }
}

/// The run id of the most recent session in `ws` that can be continued.
fn last_session(ws: &Path) -> Result<String> {
    let mut ids: Vec<String> = std::fs::read_dir(ws.join(".duet/runs"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.into_iter()
        .rev()
        .find(|id| {
            let dir = ws.join(".duet/runs").join(id);
            read_manifest(&dir).is_ok_and(|m| m.session) && duet_agent::resumable(&dir).is_ok()
        })
        .context("no open session in this workspace; start one with `duet`")
}

fn read_manifest(run_dir: &Path) -> Result<RunManifest> {
    Ok(serde_json::from_slice(
        &std::fs::read(run_dir.join("run.json")).context("no run.json")?,
    )?)
}

/// The session to continue: its manifest, after checking it is one.
fn resumed(ws: &Path, id: Option<String>) -> Result<RunManifest> {
    let id = match id {
        Some(id) => checked_run_id(&id)?.to_owned(),
        None => last_session(ws)?,
    };
    let run_dir = ws.join(".duet/runs").join(&id);
    let manifest = read_manifest(&run_dir).with_context(|| format!("no session {id}"))?;
    ensure!(
        manifest.run_id == id,
        "run {id}: run.json names another run"
    );
    ensure!(
        manifest.session,
        "run {id} is a one-shot run, not a session; continue it with `duet resume {id}`"
    );
    if let Err(why) = duet_agent::resumable(&run_dir) {
        bail!("session {id} cannot be continued: {why}");
    }
    Ok(manifest)
}

fn show_privacy(
    screen: &Screen,
    ws: &Path,
    cfg: &duet_config::Config,
    mode: Mode,
    frontier_url: Option<&str>,
    local_url: Option<&str>,
    files: bool,
) {
    // Keep piped session output stable; an explicit /privacy still prints its report.
    if !files && matches!(screen, Screen::Plain { tty: false }) {
        return;
    }
    match crate::privacy::collect(ws, cfg, mode, frontier_url, local_url) {
        Ok(report) => screen.line(&report.render(files)),
        Err(_) => screen
            .line("Privacy preview unavailable; run `duet privacy` to inspect the configuration."),
    }
}

/// Waits for the first message of a new session, and the images attached to
/// it (`/image`, checked against the rules as they are attached); `None`
/// when the operator leaves first.
#[allow(clippy::too_many_arguments)]
async fn first_message(
    inbox: &Inbox,
    leave: &AtomicBool,
    screen: &Screen,
    ws: &Path,
    cfg: &duet_config::Config,
    mode: Mode,
    frontier_url: Option<&str>,
    initial_planning: bool,
) -> Option<(
    String,
    Vec<crate::images::AttachedImage>,
    Vec<crate::attachments::Attachment>,
    bool,
    bool,
)> {
    if !matches!(screen, Screen::Plain { tty: false }) {
        screen.line("a new session starts with your first message; /help lists the commands");
    }
    screen.prompt();
    let mut planning = initial_planning;
    let mut images = Vec::new();
    let mut files = Vec::new();
    loop {
        if leave.load(Ordering::SeqCst) {
            return None;
        }
        let Some(line) = inbox.pop() else {
            if inbox.drained() {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        match parse(&line) {
            Command::Message(m) => return Some((if line.trim().starts_with("//") {line.trim().to_owned()}else{m}, images, files, false, planning)),
            Command::Skill(_) | Command::PluginPrompt(_) => match preflight_extension(ws, cfg, &line) {
                Ok(()) => return Some((line.trim().to_owned(), images, files, false, planning)),
                Err(error) => screen.line(&duet_tui::term::safe(&format!("{error:#}"))),
            },
            Command::Skills => match crate::extensions::discover(ws,cfg) {
                Ok(runtime)=>screen.line(&crate::extensions::list(&runtime.skills, cfg)),
                Err(e)=>screen.line(&format!("Skills: {e:#}")),
            },
            Command::Plugins => match crate::plugins::list() {
                Ok(text)=>screen.line(&text),Err(e)=>screen.line(&format!("Plugins: {e:#}")),
            },
            Command::Plan(raw) => {
                match plan_action(&raw) {
                    PlanAction::Status => {},
                    PlanAction::Off => planning = false,
                    PlanAction::On | PlanAction::Task(_) => planning = true,
                    PlanAction::Invalid(error) => { screen.line(error); continue; },
                    _ => { screen.line("There is no saved plan yet. Start with /plan <task>."); continue; },
                }
                screen.line(plan_status(planning));
                if let Some(w) = screen.workspace() { w.planning(planning); }
                if let PlanAction::Task(task) = plan_action(&raw) {
                    let message = if task.starts_with('/') { format!("/{task}") } else { task.to_owned() };
                    return Some((message, images, files, false, planning));
                }
            },
            Command::Goal(raw) => match goal_action(&raw) {
                GoalAction::Start(_) | GoalAction::Resume if planning => screen.line("Leave planning with /plan off before starting or resuming a goal."),
                GoalAction::Start(objective) => return Some((objective.to_owned(), images, files, true, planning)),
                _ => screen.line("Start a goal with /goal <objective>. It keeps working within your session budgets."),
            },
            Command::History(id) => show_history(ws, id, screen),
            Command::Privacy => show_privacy(screen, ws, cfg, mode, frontier_url, None, true),
            Command::Quit | Command::Close => return None,
            Command::Help => screen.text(&format!("{HELP}\n")),
            Command::Empty => {}
            Command::Attach(path) => match crate::attachments::add(&mut files, ws, &path) {
                Ok(notice) => screen.line(&notice),
                Err(e) => screen.line(&format!("attachment not added: {e:#}")),
            },
            Command::Attachments => screen.line(&attachment_queue::summary(&attachment_queue::chips(
                &files, images.iter().map(|i| (i.path.as_path(), i.public)),
            ))),
            Command::Detach(raw) => match attachment_queue::detach(&mut files, &mut images, &raw) {
                Ok(notice) => screen.line(&notice),
                Err(e) => screen.line(&format!("{e:#}")),
            },
            Command::Unknown(c) => {
                screen.line(&format!("unknown command /{c}; /help lists the commands"))
            }
            Command::Image { path, public } => {
                match crate::images::queue(&mut images, ws, cfg, mode, &path, public) {
                    Ok(notice) => screen.line(&notice),
                    Err(e) => screen.line(&format!("Image not attached: {e:#}")),
                }
            }
            _ => screen.line("nothing to show yet: the session starts with your first message"),
        }
        attachment_queue::publish(
            screen,
            attachment_queue::chips(&files, images.iter().map(|i| (i.path.as_path(), i.public))),
        );
        screen.prompt();
    }
}

/// `duet` without a command. Returns the exit code: 0 when the session was left open or
/// closed, 3 when a session budget is spent, 1 when it could not start.
pub(crate) async fn chat(ws: PathBuf, args: ChatArgs, emb: &Embedding) -> Result<i32> {
    ensure!(
        args.check.is_empty() || args.resume.is_none(),
        "--check belongs to a new session; resumed sessions use their original checks"
    );
    match (args.mode, args.no_privacy) {
        (Some(Mode::Passthrough), false) if args.resume.is_none() => bail!(
            "--mode passthrough turns the privacy boundary off: everything the model reads, \
including secrets and personal data, is sent to the frontier provider unfiltered. \
Add --no-privacy to confirm, or use --mode hybrid."
        ),
        (Some(Mode::Hybrid | Mode::TopClearance) | None, true) => {
            bail!("--no-privacy only applies to --mode passthrough")
        }
        _ => {}
    }
    let mut cfg = load_config(&ws, emb)?;
    let frontier_override = args.frontier_url.is_some() || args.frontier_model.is_some();
    let mut mode = match args.resume {
        None => crate::overrides::resolve_mode(&cfg, args.mode, frontier_override)?,
        // The session's own mode, from its record, once it is read.
        Some(_) => args.mode.unwrap_or(Mode::Hybrid),
    };
    let tty = std::io::stdin().is_terminal();
    // A first session in a terminal sets up its models before the screen opens.
    if args.resume.is_none() && !frontier_override && tty && std::io::stderr().is_terminal() {
        let frontier = mode != Mode::TopClearance;
        let local = mode != Mode::Passthrough && local_enabled(&cfg, mode)?;
        if crate::setup::needs_first_run(&cfg, frontier, local)? {
            let code = crate::setup::first_run(&mut cfg, frontier, local).await?;
            if code != 0 {
                return Ok(code);
            }
            cfg = load_config(&ws, emb)?;
        }
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    let working = Arc::new(AtomicBool::new(false));
    let leave = Arc::new(AtomicBool::new(false));
    let interrupts = Arc::new(Interrupts {
        working: working.clone(),
        interrupted: interrupted.clone(),
        leave: leave.clone(),
        idle_press: Mutex::new(None),
    });
    let clipboard = Arc::new(Mutex::new(crate::images::ClipboardImages::new(
        ws.clone(),
        cfg.int("images.max_side")? as u32,
    )));
    let (inbox, screen) = open_screen(tty, &interrupts, clipboard.clone(), || {
        crate::workspace_settings(&ws, emb)
    });
    // The terminal is given back on every way out.
    let _closing = Closing(screen.clone());
    // Refused before anything else when approval is on and nobody can answer.
    let oversight = approve::session_oversight(&cfg, |mode| {
        Arc::new(AskInline {
            mode,
            inbox: inbox.clone(),
            interrupted: interrupted.clone(),
            screen: screen.clone(),
        })
    })?;
    watch_interrupts(interrupts, screen.clone());
    let io = Io {
        inbox,
        tty,
        working,
        leave,
        screen: screen.clone(),
        _clipboard: clipboard,
    };
    let goal_turns = args.goal_turns.unwrap_or(crate::goals::DEFAULT_MAX_TURNS);
    let plan_turns = args.plan_turns.unwrap_or(crate::goals::DEFAULT_MAX_TURNS);
    let mut start_goal = args.goal.is_some();
    let mut message = args.goal.or(args.message);
    let mut resume = args.resume;
    let mut start_planning = false;
    let mut pending_attachments = PendingAttachments::default();
    let mut runtime_restart = false;
    // One session per pass; `/mode top-clearance` ends the pass with the
    // session left open and starts the next in top clearance.
    loop {
        if let Some(w) = screen.workspace() {
            w.status(Status {
                planning: start_planning,
                mode: mode_name(mode).to_owned(),
                frontier: args
                    .frontier_model
                    .clone()
                    .map_or_else(|| cfg.str("frontier.model"), Ok)
                    .unwrap_or_default(),
                local: (mode != Mode::Passthrough && local_enabled(&cfg, mode).unwrap_or(false))
                    .then(|| cfg.str("local.model").ok())
                    .flatten(),
                budget_usd: cfg.float("session.frontier_usd").unwrap_or(0.0),
                ..Status::default()
            });
        }
        let (manifest, resuming) = match resume.take() {
            Some(id) => {
                ensure!(
                    message.is_none(),
                    "--resume continues a session; send the message once it is open"
                );
                (resumed(&ws, id)?, true)
            }
            None => {
                show_privacy(
                    &screen,
                    &ws,
                    &cfg,
                    mode,
                    args.frontier_url.as_deref(),
                    None,
                    false,
                );
                if !start_goal
                    && message
                        .as_ref()
                        .is_some_and(|text| matches!(parse(text), Command::Plan(_)))
                {
                    io.inbox
                        .requeue(vec![message.take().expect("checked initial command")]);
                }
                let (first, first_images, first_files, is_goal, planning) = match message.take() {
                    Some(m) => (m, Vec::new(), Vec::new(), start_goal, start_planning),
                    None => {
                        match first_message(
                            &io.inbox,
                            &io.leave,
                            &screen,
                            &ws,
                            &cfg,
                            mode,
                            args.frontier_url.as_deref(),
                            start_planning,
                        )
                        .await
                        {
                            Some(m) => m,
                            None => return Ok(0),
                        }
                    }
                };
                start_goal = is_goal;
                start_planning = planning;
                if !is_goal && let Err(error) = preflight_extension(&ws, &cfg, &first) {
                    screen.line(&duet_tui::term::safe(&format!("{error:#}")));
                    continue;
                }
                // Setting up prints freely.
                screen.hide();
                // With the local model off nothing is probed for one.
                let local = match mode {
                    Mode::Hybrid | Mode::TopClearance if local_enabled(&cfg, mode)? => {
                        match setup::bootstrap(&cfg).await {
                            Ok(found) => found.map(|b| LocalOverride {
                                base_url: b.base_url,
                                model: b.model,
                                api_key_env: b.api_key_env,
                            }),
                            Err(code) => return Ok(code),
                        }
                    }
                    _ => None,
                };
                let manifest = RunManifest {
                    run_id: new_run_id(),
                    mode,
                    objective: first,
                    checks: Some({
                        let mut checks = cfg.list("checks.commands")?;
                        checks.extend(args.check.iter().cloned());
                        checks
                    }),
                    frontier_url: args
                        .frontier_url
                        .clone()
                        .map_or_else(|| cfg.str("frontier.base_url"), Ok)?,
                    frontier_model: args
                        .frontier_model
                        .clone()
                        .map_or_else(|| cfg.str("frontier.model"), Ok)?,
                    frontier_dialect: Some(frontier_dialect(&cfg)?.as_str().to_owned()),
                    local,
                    session: true,
                    images: first_images,
                    files: first_files,
                };
                (manifest, false)
            }
        };
        mode = manifest.mode;
        crate::overrides::check(&cfg, &manifest)?;
        if resuming || manifest.local.is_some() {
            show_privacy(
                &screen,
                &ws,
                &cfg,
                manifest.mode,
                Some(&manifest.frontier_url),
                manifest.local.as_ref().map(|l| l.base_url.as_str()),
                false,
            );
        }
        eprintln!("session {} ({:?})", manifest.run_id, manifest.mode);
        let lock = duet_fs::lock::WorkspaceLock::acquire(&ws)?;
        let run_dir = ws.join(".duet/runs").join(&manifest.run_id);
        duet_fs::private::ensure_private_dir(&run_dir)?;
        duet_fs::private::ensure_private_dir(&ws.join(".duet/tmp"))?;
        if resuming && !runtime_restart {
            let mut plans = duet_agent::plans::Store::load(&run_dir).map_err(anyhow::Error::msg)?;
            let unfinished = plans.snapshot().execution.as_ref().is_some_and(|e| {
                matches!(
                    e.status,
                    duet_agent::plans::ExecutionStatus::Active
                        | duet_agent::plans::ExecutionStatus::Waiting
                        | duet_agent::plans::ExecutionStatus::Paused
                )
            });
            plans.recover().map_err(anyhow::Error::msg)?;
            if unfinished {
                // Restore the restriction before recovery or optional services can run.
                duet_agent::transcript::Transcript::open(&run_dir)?
                    .append(&Entry::PlanMode { enabled: true })?;
            }
        }
        if !resuming {
            if start_planning {
                // Publish the permission restriction before the resumable manifest.
                duet_agent::transcript::Transcript::open(&run_dir)?
                    .append(&Entry::PlanMode { enabled: true })?;
            }
            duet_fs::private::write_private(
                &run_dir.join("run.json"),
                &serde_json::to_vec_pretty(&manifest)?,
            )?;
            if start_goal {
                crate::goals::Store::load(&run_dir)?.start(&manifest.objective, goal_turns)?;
            }
        }
        // Sessions last as long as the operator keeps them open: the provider's
        // own retry deadline is far away, and each turn sets its own.
        let limits = RunLimits {
            deadline: tokio::time::Instant::now() + Duration::from_secs(7 * 24 * 3600),
            interrupted: interrupted.clone(),
            local_meter: crate::pricing::meter(&cfg)?,
        };
        let mut audit = None;
        let driven = std::panic::AssertUnwindSafe(converse(
            &ws,
            &manifest,
            &cfg,
            oversight.clone(),
            &run_dir,
            resuming,
            &limits,
            &mut audit,
            &io,
            emb.hooks(),
            start_goal && !resuming,
            start_planning && !resuming,
            &mut pending_attachments,
            goal_turns,
            plan_turns,
        ))
        .catch_unwind()
        .await;
        let (terminal, stats, switch) = match driven {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) => (
                Terminal::Failed {
                    reason: format!("{e:#}"),
                },
                RunStats::default(),
                None,
            ),
            Err(panic) => (
                Terminal::internal_error(panic.as_ref()),
                RunStats::default(),
                None,
            ),
        };
        if switch.is_none() {
            screen.close();
        }
        let audit = match audit {
            Some(a) => Some(a),
            None => open_audit(&ws, &manifest.run_id, emb.hooks())
                .map(AuditHandle::new)
                .map_err(|e| eprintln!("warning: audit log not opened: {e:#}"))
                .ok(),
        };
        // `duet` itself embeds nothing, so it adds no hooks (see ARCHITECTURE.md,
        // "Embedding Duet").
        duet_agent::conclude_with(
            &run_dir,
            &manifest.run_id,
            audit.as_ref(),
            &audit_log_path(&ws, &manifest.run_id),
            &terminal,
            &stats,
            &duet_agent::Ending {
                kind: duet_agent::RunKind::Session,
                mode: manifest.mode.as_str(),
                resumed: resuming,
                policy: cfg.policy().map(duet_config::Policy::meta),
                hooks: emb.hooks(),
            },
        )?;
        drop(lock);
        let id = &manifest.run_id;
        if matches!(switch, Some(SessionChange::Planning)) {
            runtime_restart = true;
            resume = Some(Some(id.clone()));
            start_goal = false;
            start_planning = Session::persisted_planning(&run_dir).map_err(anyhow::Error::msg)?;
            continue;
        }
        if let Some(next) = switch {
            runtime_restart = false;
            screen.line(&format!(
                "session {id} ({}) left open (${:.4} so far); continue it with: duet --resume {id}",
                mode_name(mode),
                stats.cost_usd
            ));
            screen.line(
                "top clearance: a new session starts with your next message. Only the local \
model works in it, and nothing leaves this machine but its requests to the local model: no \
frontier, no web tools, no network for commands. It starts fresh, so nothing said here ever \
reaches the frontier; leaving top clearance needs a new session.",
            );
            mode = match next {
                SessionChange::Privacy { mode, planning } => {
                    start_planning = planning;
                    mode
                }
                SessionChange::Planning => unreachable!(),
            };
            start_goal = false;
            continue;
        }
        match &terminal {
            Terminal::Completed { .. } => {
                println!("session {id} closed (${:.4} in total)", stats.cost_usd);
            }
            Terminal::Open { .. } => {
                println!(
                    "session {id} left open (${:.4} so far); continue with: duet --resume {id}",
                    stats.cost_usd
                );
            }
            Terminal::Failed { reason } => {
                eprintln!("session {id} stopped: {reason}");
            }
            Terminal::BudgetStopped { which } => {
                println!(
                    "session {id}: {which} is spent; raise it to continue with duet --resume {id}"
                );
            }
        }
        return Ok(terminal.exit_code());
    }
}

/// The operator's side of the conversation.
struct Io {
    inbox: Arc<Inbox>,
    tty: bool,
    /// Raised while a turn runs (Ctrl-C then interrupts it).
    working: Arc<AtomicBool>,
    /// Raised by a second Ctrl-C at the prompt.
    leave: Arc<AtomicBool>,
    screen: Screen,
    /// Keeps private pasted-image files alive until the chat and its UI finish.
    _clipboard: Arc<Mutex<crate::images::ClipboardImages>>,
}

/// The workspace when standard input and output are a terminal that can take
/// it (its lines go to the inbox), else the reader thread and plain lines.
fn open_screen(
    tty: bool,
    interrupts: &Arc<Interrupts>,
    clipboard: Arc<Mutex<crate::images::ClipboardImages>>,
    settings: impl FnOnce() -> duet_tui::workspace::Settings,
) -> (Arc<Inbox>, Screen) {
    let plain = || (Inbox::stdin(), Screen::Plain { tty });
    if !(tty && std::io::stdout().is_terminal() && crate::term::capable()) {
        return plain();
    }
    let inbox = Arc::new(Inbox::default());
    let hooks = Hooks {
        paste_image: {
            let inbox = inbox.clone();
            Arc::new(move |bytes| {
                let path = clipboard
                    .lock()
                    .map_err(|_| "Clipboard image storage is unavailable".to_owned())?
                    .store(bytes)
                    .map_err(|e| e.to_string())?;
                inbox.push_deferred(format!("/image {}", crate::attachments::quoted(&path)));
                Ok("Image queued · sent with your next message".to_owned())
            })
        },
        plan_action: {
            let inbox = inbox.clone();
            Box::new(move |action| inbox.push_plan_action(action))
        },
        line: {
            let inbox = inbox.clone();
            Box::new(move |l| inbox.push(l))
        },
        eof: {
            let inbox = inbox.clone();
            Box::new(move || inbox.close())
        },
        interrupt: {
            let i = interrupts.clone();
            Box::new(move || i.press().map(str::to_owned))
        },
        answering: {
            let inbox = inbox.clone();
            Box::new(move || inbox.answering())
        },
    };
    match Workspace::start(hooks, COMMANDS, Some(settings())) {
        Ok(workspace) => (inbox, Screen::Live(workspace)),
        Err(error) => {
            eprintln!(
                "Full-screen UI unavailable: {}. Continuing in line mode.",
                duet_tui::term::safe(&error.to_string())
            );
            plain()
        }
    }
}

/// Closes the screen when dropped.
struct Closing(Screen);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Ctrl-C, as a signal or (in the workspace) a key.
struct Interrupts {
    working: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    leave: Arc<AtomicBool>,
    idle_press: Mutex<Option<std::time::Instant>>,
}

impl Interrupts {
    /// During a turn Ctrl-C interrupts it; at the prompt, twice within two
    /// seconds leaves the session. Returns what to tell the operator.
    fn press(&self) -> Option<&'static str> {
        if self.working.load(Ordering::SeqCst) {
            self.interrupted.store(true, Ordering::SeqCst);
            return Some("interrupt: stopping this turn; the session stays open");
        }
        let mut idle = self
            .idle_press
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if idle.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            self.leave.store(true, Ordering::SeqCst);
            None
        } else {
            *idle = Some(std::time::Instant::now());
            Some("Ctrl-C again leaves the session (it stays open to resume); /close ends it")
        }
    }
}

/// Ctrl-C sent as a signal (the workspace reads the key itself).
fn watch_interrupts(interrupts: Arc<Interrupts>, screen: Screen) {
    tokio::spawn(async move {
        while tokio::signal::ctrl_c().await.is_ok() {
            if let Some(notice) = interrupts.press() {
                screen.notice(notice);
            }
        }
    });
}

/// Sets the session up and holds the conversation until the operator leaves
/// or closes it.
#[allow(clippy::too_many_arguments)]
async fn converse(
    ws: &Path,
    manifest: &RunManifest,
    cfg: &duet_config::Config,
    oversight: duet_agent::Oversight,
    run_dir: &Path,
    resume: bool,
    limits: &RunLimits,
    audit: &mut Option<AuditHandle>,
    io: &Io,
    hooks: &duet_agent::Hooks,
    start_goal: bool,
    start_planning: bool,
    pending_attachments: &mut PendingAttachments,
    goal_turns: u64,
    plan_turns: u64,
) -> Result<(Terminal, RunStats, Option<SessionChange>)> {
    let planning = if resume {
        Session::persisted_planning(run_dir).map_err(anyhow::Error::msg)?
    } else {
        start_planning
    };
    let Prepared {
        git,
        engine,
        frontier,
        run_cfg,
    } = prepare(
        ws, manifest, cfg, oversight, run_dir, limits, audit, hooks, planning,
    )
    .await?;
    let passthrough = PassThrough { max_bytes: 60_000 };
    let presenter: &dyn Presenter = match &engine {
        Some(e) => e.as_ref(),
        None => &passthrough,
    };
    let session_limits = SessionLimits {
        frontier_usd: cfg.float("session.frontier_usd")?,
        working_time: Duration::from_secs(cfg.int("session.wall_clock_minutes")? as u64 * 60),
    };
    let mut session = Session::open(
        &run_cfg,
        &frontier,
        presenter,
        &git,
        limits.interrupted.clone(),
        session_limits,
        resume,
    )
    .map_err(anyhow::Error::msg)?;
    if start_planning {
        session.set_planning(true).map_err(anyhow::Error::msg)?;
    }
    let shown: Arc<dyn Fn(&str) -> String + Send + Sync> = match &engine {
        Some(e) => {
            let e = e.clone();
            Arc::new(move |t: &str| e.detokenize(t))
        }
        None => Arc::new(|t: &str| t.to_owned()),
    };
    if let Some(c) = io.screen.workspace() {
        c.restore(shown.clone());
        // History: the operator's own messages of this session.
        c.remember(
            session
                .history()
                .iter()
                .flat_map(|x| std::iter::once(x.message.clone()).chain(x.steered.clone()))
                .collect(),
        );
    }
    for image in std::mem::take(&mut pending_attachments.images) {
        // A failed recheck must stop before delivering the dependent request.
        session
            .attach(image.path, image.public)
            .map_err(anyhow::Error::msg)?;
    }
    let files = Mutex::new(if resume {
        std::mem::take(&mut pending_attachments.files)
    } else {
        manifest.files.clone()
    });
    let mut goals = crate::goals::Store::load(run_dir)?;
    goals.checkpoint()?;
    if start_goal {
        goals.resume()?;
    }
    let ctx = TurnCtx {
        cfg,
        mode: manifest.mode,
        skills: &run_cfg.skills,
        run_dir,
        io,
        shown: &shown,
        ws,
        git: &git,
        presenter,
        audit: frontier.audit(),
        files: &files,
        plan_turns,
    };
    if let Some(w) = io.screen.workspace() {
        // The side panel follows the files this session changes, holding
        // back what the engine holds back.
        w.attach(
            ws.to_path_buf(),
            run_dir.to_path_buf(),
            audit_log_path(ws, &manifest.run_id),
            crate::policy(cfg)?,
        );
    }
    let (closed, switch) = conduct(
        &mut session,
        manifest,
        cfg,
        &ctx,
        &mut goals,
        resume,
        goal_turns,
    )
    .await?;
    if matches!(switch, Some(SessionChange::Planning)) {
        pending_attachments.images = session.attached().to_vec();
        pending_attachments.files = std::mem::take(&mut *files.lock().unwrap());
    }
    let (terminal, mut stats) = session.end(closed);
    crate::mcp::stop(&run_cfg).await;
    // The session's language servers lived across its turns; stop them now.
    if let Some(lsp) = &run_cfg.lsp {
        lsp.shutdown().await;
    }
    stats.ledger.local = engine.as_ref().and_then(|e| e.take_local_stats());
    let local_cost = limits.local_meter.snapshot();
    stats.local_cost_usd = local_cost.cost_usd;
    stats.local_usage_unknown_requests = local_cost.unpriced_cancelled_requests;
    Ok((terminal, stats, switch))
}

#[derive(Default)]
struct PendingAttachments {
    images: Vec<duet_agent::images::Attachment>,
    files: Vec<crate::attachments::Attachment>,
}

#[derive(Debug, PartialEq, Eq)]
enum SessionChange {
    Privacy {
        mode: Mode,
        planning: bool,
    },
    /// Reopen the same session after stopping its runtime services.
    Planning,
}

/// Drives the operator controls and automatic goal turns. Provider construction
/// stays outside so the same driver can be exercised with an in-process frontier.
async fn conduct(
    session: &mut Session<'_>,
    manifest: &RunManifest,
    cfg: &duet_config::Config,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
    resume: bool,
    goal_turns: u64,
) -> Result<(bool, Option<SessionChange>)> {
    let TurnCtx {
        io,
        ws,
        git,
        run_dir,
        presenter,
        files,
        ..
    } = *ctx;
    if session.is_planning() {
        plan_controls::pause_goal(
            goals,
            "Planning mode pauses automatic goals. Use /plan off, then /goal resume to continue.",
        )?;
        io.screen.line(plan_status(true));
    }
    if resume
        && let Some(TurnEnd::Asked {
            options: Some(options),
            ..
        }) = session.history().last().and_then(|turn| turn.end.as_ref())
    {
        // This question predates this invocation; new stdin may intentionally answer it.
        let mut queue = io.inbox.lock();
        if queue.question_id.as_deref() != Some(&options.id) {
            queue.question_id = Some(options.id.clone());
            queue.question_after = Some(0);
        }
    }
    plan_controls::publish(ctx)?;
    plan_controls::publish_question(session, ctx)?;
    show_status(io, session, manifest, cfg, goals);
    if resume {
        if !session.is_planning() {
            io.screen.line(plan_status(false));
        }
        recap(session, &manifest.run_id, &io.screen);
        if goals.current().is_some() {
            io.screen.line(&goals.describe());
        }
    } else {
        let first = if let Some(prompt) = goals.prompt() {
            Ok(prompt)
        } else {
            match parse(&manifest.objective) {
                Command::Skill(raw) => crate::extensions::invoke(ctx.skills, &raw),
                Command::PluginPrompt(raw) => plugin_command(cfg, &raw),
                Command::Message(message) => Ok(message),
                _ => Ok(manifest.objective.clone()),
            }
        };
        match first {
            Ok(first) => turns(session, first, ctx, goals).await?,
            Err(error) => io.screen.line(&duet_tui::term::safe(&format!("{error:#}"))),
        }
        show_status(io, session, manifest, cfg, goals);
    }
    let (closed, switch) = loop {
        if io.leave.load(Ordering::SeqCst) {
            break (false, None);
        }
        if let Some(which) = session.spent() {
            io.screen.line(&format!(
                "the session budget {which} is spent; no further turns can start"
            ));
            break (false, None);
        }
        attachment_queue::publish(
            &io.screen,
            attachment_queue::chips(
                &files.lock().unwrap(),
                session
                    .attached()
                    .iter()
                    .map(|i| (i.path.as_path(), i.public)),
            ),
        );
        io.screen.prompt();
        let line = loop {
            if io.leave.load(Ordering::SeqCst) {
                break None;
            }
            match io.inbox.pop_input() {
                Some(input) => break Some(input),
                None if io.tty && io.inbox.drained() => break None,
                None if !session.is_planning() && plan_controls::active(run_dir)? => {
                    let next = plan_controls::prompt(run_dir)?
                        .context("Active plan has no continuation prompt")?;
                    turns(session, next, ctx, goals).await?;
                    show_status(io, session, manifest, cfg, goals);
                    if session.spent().is_some() {
                        break None;
                    }
                }
                None if goals.active() && !session.is_planning() => {
                    let next = goals.prompt().expect("active goal has a prompt");
                    turns(session, next, ctx, goals).await?;
                    show_status(io, session, manifest, cfg, goals);
                    if session.spent().is_some() {
                        break None;
                    }
                }
                None if io.inbox.drained() => break None,
                None => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };
        let Some(input) = line else {
            if matches!(io.screen, Screen::Plain { tty: true }) {
                println!();
            }
            break (false, None);
        };
        let (sequence, line) = match input {
            OperatorInput::Line(sequence, line) => (sequence, line),
            OperatorInput::Plan(action) => {
                let before = session.is_planning();
                let saving = matches!(&action, duet_tui::workspace::PlanAction::Save { .. });
                let effect = plan_controls::ui_action(action, session, ctx, goals).await;
                if saving
                    && let Err(error) = &effect
                    && let Some(workspace) = io.screen.workspace()
                {
                    workspace.plan_error(duet_tui::term::safe(&format!("{error:#}")));
                }
                if apply_plan_effect(effect, before, session, ctx, goals).await? {
                    break (false, Some(SessionChange::Planning));
                }
                show_status(io, session, manifest, cfg, goals);
                continue;
            }
        };
        let say = |text: &str| io.screen.line(text);
        match parse(&line) {
            Command::Empty => {}
            Command::Quit => break (false, None),
            Command::Close => break (true, None),
            Command::Help => io.screen.text(&format!("{HELP}\n")),
            Command::Skills => say(&crate::extensions::list(ctx.skills, cfg)),
            Command::Plugins => match crate::plugins::list() {
                Ok(text) => say(&text),
                Err(e) => say(&format!("Plugins: {e:#}")),
            },
            Command::Skill(raw) | Command::PluginPrompt(raw) => {
                let message = if matches!(parse(&line), Command::Skill(_)) {
                    crate::extensions::invoke(ctx.skills, &raw)
                } else {
                    plugin_command(cfg, &raw)
                };
                match message {
                    Ok(message) => {
                        turns(session, message, ctx, goals).await?;
                        show_status(io, session, manifest, cfg, goals);
                    }
                    Err(e) => say(&duet_tui::term::safe(&format!("Extension: {e:#}"))),
                }
            }
            Command::History(id) => show_history(ws, id, &io.screen),
            Command::Privacy => show_privacy(
                &io.screen,
                ws,
                cfg,
                manifest.mode,
                Some(&manifest.frontier_url),
                manifest.local.as_ref().map(|l| l.base_url.as_str()),
                true,
            ),
            Command::Plan(raw) => {
                let before = session.is_planning();
                let effect = plan_controls::text_action(&raw, session, ctx, goals).await;
                if apply_plan_effect(effect, before, session, ctx, goals).await? {
                    break (false, Some(SessionChange::Planning));
                }
                show_status(io, session, manifest, cfg, goals);
            }
            Command::Answer(raw) => {
                let before = session.is_planning();
                let effect = plan_controls::answer_text(&raw, session, ctx, goals).await;
                if apply_plan_effect(effect, before, session, ctx, goals).await? {
                    break (false, Some(SessionChange::Planning));
                }
                show_status(io, session, manifest, cfg, goals);
            }
            Command::Goal(raw) => {
                if matches!(goal_action(&raw), GoalAction::Start(_) | GoalAction::Resume)
                    && !session.is_planning()
                {
                    plan_controls::pause(
                        ctx,
                        "Plan paused while the goal runs. Use /plan resume rN to return.",
                    )?;
                }
                let result = match goal_action(&raw) {
                    GoalAction::Start(_) | GoalAction::Resume if session.is_planning() => {
                        Err(anyhow::anyhow!(
                            "Leave planning with /plan off before starting or resuming a goal."
                        ))
                    }
                    GoalAction::Status => Ok(()),
                    GoalAction::Pause => {
                        goals.pause("Paused by you. Use /goal resume to continue.")
                    }
                    GoalAction::Resume
                        if goals
                            .current()
                            .is_some_and(|g| g.state == crate::goals::State::Waiting) =>
                    {
                        Err(anyhow::anyhow!(
                            "Duet is waiting for your answer. Type a reply to continue the goal."
                        ))
                    }
                    GoalAction::Resume => goals.resume(),
                    GoalAction::Cancel => goals.cancel(),
                    GoalAction::Start(objective) => goals.start(objective, goal_turns),
                };
                if let Err(e) = result {
                    say(&format!("Goal: {e:#}"));
                }
                say(&goals.describe());
                show_status(io, session, manifest, cfg, goals);
            }
            Command::Status => say(&format!(
                "{}\n{}\n{}",
                status(session, &manifest.run_id, cfg),
                crate::pricing::status(run_dir, session.stats().cost_usd),
                goals.describe(),
            )),
            Command::Diff => io.screen.diff(&local_diff(git, ws, run_dir, presenter)),
            Command::Undo => match session.undo() {
                Ok((turn, paths)) if paths.is_empty() => {
                    say(&format!("turn {turn} wrote no files; nothing to revert"))
                }
                Ok((turn, paths)) => {
                    say(&format!("reverted the writes of turn {turn} and later:"));
                    for p in paths {
                        say(&format!("  {}", p.display()));
                    }
                    say("duet is told with your next message.");
                }
                Err(e) => say(&format!("undo: {e}")),
            },
            Command::Stop => {
                plan_controls::pause(ctx, "Stopped by you. Use /plan resume rN to continue.")?;
                plan_controls::pause_goal(goals, "Stopped by you. Use /goal resume to continue.")?;
                say("Work paused.");
            }
            Command::Unknown(c) => say(&format!("unknown command /{c}; /help lists the commands")),
            Command::Attach(path) => {
                match crate::attachments::add(&mut files.lock().unwrap(), ws, &path) {
                    Ok(notice) => say(&notice),
                    Err(e) => say(&format!("attachment not added: {e:#}")),
                }
            }
            Command::Attachments => say(&attachment_queue::summary(&attachment_queue::chips(
                &files.lock().unwrap(),
                session
                    .attached()
                    .iter()
                    .map(|i| (i.path.as_path(), i.public)),
            ))),
            Command::Detach(raw) => {
                match attachment_queue::detach_session(&mut files.lock().unwrap(), session, &raw) {
                    Ok(notice) => say(&notice),
                    Err(e) => say(&format!("{e:#}")),
                }
            }
            Command::Mode(asked) => match mode_change(manifest.mode, asked.as_deref()) {
                ModeChange::Switch(to) => {
                    break (
                        false,
                        Some(SessionChange::Privacy {
                            mode: to,
                            planning: session.is_planning(),
                        }),
                    );
                }
                ModeChange::Say(text) => say(&text),
            },
            Command::Image { path, .. } if path.is_empty() => say(&format!(
                "usage: /image [--public] <path>{}",
                match session.attached().len() {
                    0 => String::new(),
                    n => format!(" ({n} image(s) wait for your next message)"),
                }
            )),
            Command::Image { path, public } => {
                let attached = (|| -> Result<String> {
                    ensure!(
                        session.attached().len() < crate::images::MAX_PENDING,
                        "at most {} images can wait for one message",
                        crate::images::MAX_PENDING
                    );
                    let image = crate::images::selected(ws, &path, public)?;
                    session
                        .attach(image.path, image.public)
                        .map_err(anyhow::Error::msg)
                })();
                match attached {
                    Ok(said) => say(&format!("attached {said}")),
                    Err(e) => say(&format!("Image not attached: {e:#}")),
                }
            }
            Command::Message(m) => {
                if plan_controls::has_question(session, run_dir)? {
                    if !io.inbox.fresh_answer(sequence) {
                        io.inbox.hold_for_question(line);
                        say("Earlier queued text is held until you answer the current question.");
                        continue;
                    }
                    if !io.tty {
                        say(&format!("you> {}", m.replace('\n', "\n     ")));
                    }
                    let before = session.is_planning();
                    let effect = plan_controls::answer_freeform(&m, session, ctx, goals).await;
                    if apply_plan_effect(effect, before, session, ctx, goals).await? {
                        break (false, Some(SessionChange::Planning));
                    }
                    show_status(io, session, manifest, cfg, goals);
                    continue;
                }
                if !session.is_planning()
                    && goals
                        .current()
                        .is_some_and(|g| g.state == crate::goals::State::Waiting)
                    && let Err(e) = goals.resume()
                {
                    say(&format!("Goal: {e:#}"));
                    continue;
                }
                if !io.tty {
                    // Piped input is not on screen: the output keeps the conversation.
                    say(&format!("you> {}", m.replace('\n', "\n     ")));
                }
                turns(session, m, ctx, goals).await?;
                show_status(io, session, manifest, cfg, goals);
            }
        }
    };
    if !matches!(switch, Some(SessionChange::Planning)) {
        plan_controls::pause(
            ctx,
            "Session left open. Review the plan, then explicitly resume it.",
        )?;
    }
    if closed {
        goals.cancel()?;
    } else if goals.active() {
        goals.pause("Session left open. Resume the session, then use /goal resume.")?;
    }
    Ok((closed, switch))
}

/// What a turn needs besides the session.
async fn apply_plan_effect(
    effect: Result<plan_controls::Effect>,
    before: bool,
    session: &mut Session<'_>,
    ctx: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
) -> Result<bool> {
    let mut restart = before != session.is_planning();
    match effect {
        Ok(plan_controls::Effect::Restart) => restart = true,
        Ok(plan_controls::Effect::Prompt(message)) => {
            if restart {
                ctx.io.inbox.requeue(vec![if message.starts_with('/') {
                    format!("/{message}")
                } else {
                    message
                }]);
            } else {
                turns(session, message, ctx, goals).await?;
            }
        }
        Ok(plan_controls::Effect::Done) => {}
        Err(error) => {
            let error = duet_tui::term::safe(&format!("Plan: {error:#}"));
            ctx.io.screen.line(&error);
        }
    }
    plan_controls::publish(ctx)?;
    plan_controls::publish_question(session, ctx)?;
    if restart {
        ctx.io
            .screen
            .line("Restoring runtime services for the selected mode before continuing.");
    }
    Ok(restart)
}

struct TurnCtx<'a> {
    cfg: &'a duet_config::Config,
    mode: Mode,
    skills: &'a duet_agent::skills::Skills,
    run_dir: &'a Path,
    io: &'a Io,
    shown: &'a Arc<dyn Fn(&str) -> String + Send + Sync>,
    ws: &'a Path,
    git: &'a duet_git::Git,
    presenter: &'a dyn Presenter,
    audit: &'a AuditHandle,
    files: &'a Mutex<Vec<crate::attachments::Attachment>>,
    plan_turns: u64,
}

/// Runs a turn for `message`; a message that arrived as a turn ended (too
/// late to steer it) starts the next one, unless the operator stopped it.
async fn turns(
    session: &mut Session<'_>,
    message: String,
    t: &TurnCtx<'_>,
    goals: &mut crate::goals::Store,
) -> Result<()> {
    let plan_active = !session.is_planning() && plan_controls::active(t.run_dir)?;
    ensure!(
        !(plan_active && goals.active()),
        "Both a goal and a plan are active; pause one before continuing."
    );
    let active = goals.active() && !session.is_planning();
    if plan_active {
        plan_controls::begin_turn(t.run_dir)?;
    }
    if active {
        goals.begin_turn()?;
    }
    let m = crate::attachments::message(message, &mut t.files.lock().unwrap(), t.ws, t.presenter);
    let (end, late, stopped) = take_turn(session, &m, t, active || plan_active).await;
    if plan_active {
        plan_controls::finish_turn(t, &end)?;
        if stopped {
            plan_controls::pause(t, "Stopped by you. Use /plan resume rN to continue.")?;
        }
        t.io.screen
            .line(&duet_tui::term::safe(&(t.shown)(&plan_controls::describe(
                t.run_dir,
            )?)));
    }
    plan_controls::publish(t)?;
    plan_controls::publish_question(session, t)?;
    if active {
        goals.finish_turn(&end)?;
        if stopped {
            goals.pause("Stopped by you. Use /goal resume to continue.")?;
        }
        if let Some(status) = goals.status_line() {
            t.io.screen.line(&status);
        }
    }
    if stopped || matches!(end, TurnEnd::Stopped | TurnEnd::Interrupted) {
        for l in &late {
            t.io.screen.line(&format!(
                "not delivered (you stopped the turn): {}",
                clip(l)
            ));
        }
    } else if !late.is_empty() && session.spent().is_none() {
        t.io.screen
            .line("your message arrived as duet ended its turn; it starts the next one");
        // Append behind held commands: /quit and /goal pause take precedence.
        // Escape slash prefixes so steering text cannot become a command.
        let message = late.join("\n\n");
        t.io.inbox.push_deferred(if message.starts_with('/') {
            format!("/{message}")
        } else {
            message
        });
    }
    Ok(())
}

/// Runs one turn, following its progress in the transcript as it is
/// written. Meanwhile the operator's lines steer it (`/stop` ends it after
/// the current step, `/diff` and `/help` answer at once, other commands wait
/// for the turn to end). Returns how it ended and the steering messages it
/// ended before delivering.
async fn take_turn(
    session: &mut Session<'_>,
    message: &str,
    t: &TurnCtx<'_>,
    goal_active: bool,
) -> (TurnEnd, Vec<String>, bool) {
    let io = t.io;
    let path = t.run_dir.join("transcript.jsonl");
    let from = std::fs::metadata(&path).map_or(0, |m| m.len());
    let done = Arc::new(AtomicBool::new(false));
    let sink: Box<dyn FnMut(Entry) + Send> = match io.screen.workspace() {
        Some(c) => {
            let c = c.clone();
            Box::new(move |e| c.entry(e))
        }
        None => printed(t.shown.clone()),
    };
    let shared = Arc::new(Mutex::new(Follow::new(path, from, sink)));
    let follower = tokio::spawn(following(shared.clone(), done.clone()));
    let steering = session.steering();
    let mut held = Vec::new();
    let mut pending_images = Vec::new();
    let mut image_failed = false;
    let mut hold_messages = false;
    let mut plan_switch_queued = false;
    let mut attachments_committed = false;
    let mut last_plan_refresh = std::time::Instant::now();
    let mut stopped = false;
    attachment_queue::publish(&io.screen, Vec::new());
    io.working.store(true, Ordering::SeqCst);
    let say = |text: &str| io.screen.line(text);
    let end = {
        // In the workspace the frontier's responses show as they stream.
        let turn = match io.screen.workspace() {
            Some(c) => {
                let tap = Arc::new(Ordered {
                    follow: shared.clone(),
                    inner: c.tap(),
                });
                Either::Left(duet_boundary::live::observe(tap, session.turn(message)))
            }
            None => Either::Right(session.turn(message)),
        };
        if let Some(c) = io.screen.workspace() {
            c.begin();
        }
        let mut turn = std::pin::pin!(turn);
        loop {
            tokio::select! {
                end = &mut turn => break end,
                () = tokio::time::sleep(Duration::from_millis(50)) => {
                    if last_plan_refresh.elapsed() >= Duration::from_millis(500) {
                        if let Err(error) = plan_controls::publish(t) {
                            say(&duet_tui::term::safe(&format!("Plan state: {error:#}")));
                            stopped = true;
                            steering.stop();
                        }
                        last_plan_refresh = std::time::Instant::now();
                    }
                    if goal_active && io.tty && io.inbox.drained() {
                        stopped = true;
                        steering.stop();
                    }
                    if io.inbox.plan_action_ready() && !plan_switch_queued {
                        stopped = true;
                        steering.stop();
                        plan_switch_queued = true;
                        say("Plan action queued after the current step; following input will wait.");
                    }
                    while let Some(line) = io.inbox.pop_unless_answering() {
                        if plan_switch_queued {
                            held.push(line);
                            continue;
                        }
                        match parse(&line) {
                            Command::Plan(raw) => {
                                match plan_action(&raw) {
                                    PlanAction::Review(revision) => {
                                        if let Err(error) = plan_controls::review(t, revision, false) { say(&duet_tui::term::safe(&format!("Plan: {error:#}"))); }
                                    }
                                    PlanAction::Status => {
                                        match plan_controls::describe(t.run_dir) {
                                            Ok(status) => say(&duet_tui::term::safe(&(t.shown)(&status))),
                                            Err(error) => say(&duet_tui::term::safe(&format!("Plan: {error:#}"))),
                                        }
                                    }
                                    PlanAction::Invalid(error) => say(error),
                                    _ => {
                                        stopped = true;
                                        steering.stop();
                                        plan_switch_queued = true;
                                        say("Planning change queued after the current step; following input will wait.");
                                        held.push(line);
                                    }
                                }
                            }
                            Command::Answer(_) => { held.push(line); }
                            Command::Message(m) => {
                                if hold_messages {
                                    say("Message queued with your attachments for the next turn.");
                                    held.push(line);
                                    attachments_committed = true;
                                    continue;
                                }
                                say(&format!(
                                    "  ▸ for duet after the current step: {}",
                                    clip(&m)
                                ));
                                steering.steer(crate::attachments::message(m, &mut t.files.lock().unwrap(), t.ws, t.presenter));
                            }
                            Command::Attach(path) => {
                                if attachments_committed {
                                    held.push(line);
                                    continue;
                                }
                                match crate::attachments::add(&mut t.files.lock().unwrap(), t.ws, &path) {
                                    Ok(notice) => say(&notice),
                                    Err(e) => say(&format!("attachment not added: {e:#}")),
                                }
                            },
                            Command::Image { path, public } => {
                                if attachments_committed {
                                    held.push(line);
                                    continue;
                                }
                                // The following request belongs with this image even
                                // if validation fails; never steer it without its input.
                                hold_messages = true;
                                match crate::images::queue(&mut pending_images, t.ws, t.cfg, t.mode, &path, public) {
                                    Ok(notice) => say(&notice),
                                    Err(e) => {
                                        image_failed = true;
                                        say(&format!("Image not attached: {e:#}"));
                                    }
                                }
                            },
                            Command::Attachments => say(&attachment_queue::summary(&attachment_queue::chips(
                                &t.files.lock().unwrap(), pending_images.iter().map(|i| (i.path.as_path(), i.public)),
                            ))),
                            Command::Detach(raw) => {
                                if attachments_committed {
                                    held.push(line);
                                    continue;
                                }
                                match attachment_queue::detach(&mut t.files.lock().unwrap(), &mut pending_images, &raw) {
                                    Ok(notice) => say(&notice),
                                    Err(e) => say(&format!("{e:#}")),
                                }
                            },
                            Command::Stop => {
                                stopped = true;
                                steering.stop();
                                say("  ■ stopping after the current step");
                            }
                            Command::Diff => {
                                io.screen.diff(&local_diff(t.git, t.ws, t.run_dir, t.presenter))
                            }
                            Command::Help => io.screen.text(&format!("{HELP}\n")),
                            Command::Skills => say(&crate::extensions::list(t.skills, t.cfg)),
                            Command::Plugins => match crate::plugins::list() {
                                Ok(text)=>say(&text),Err(e)=>say(&format!("Plugins: {e:#}")),
                            },
                            Command::Empty => {}
                            Command::Unknown(c) => {
                                say(&format!("unknown command /{c}; /help lists the commands"))
                            }
                            Command::Quit
                            | Command::Close
                            | Command::Undo
                            | Command::Status
                            | Command::Privacy
                            | Command::Skill(_)
                            | Command::PluginPrompt(_)
                            | Command::History(_)
                            | Command::Goal(_)
                            | Command::Mode(_) => {
                                if hold_messages && attachment_queue::requests_turn(&parse(&line)) {
                                    attachments_committed = true;
                                }
                                let pauses = match parse(&line) {
                                    Command::Quit | Command::Close | Command::Undo | Command::Mode(_) => true,
                                    Command::Goal(raw) => !matches!(goal_action(&raw), GoalAction::Status),
                                    _ => false,
                                };
                                if goal_active && pauses {
                                    stopped = true;
                                    steering.stop();
                                }
                                say(&format!("  (after this turn: {})", line.trim()));
                                held.push(line);
                            }
                        }
                        attachment_queue::publish(&io.screen, attachment_queue::chips(
                            &t.files.lock().unwrap(), pending_images.iter().map(|i| (i.path.as_path(), i.public)),
                        ));
                    }
                }
            }
        }
    };
    io.working.store(false, Ordering::SeqCst);
    let previous_images = session.attached().len();
    if !image_failed {
        for image in pending_images {
            if let Err(why) = session.attach(image.path, image.public) {
                image_failed = true;
                say(&format!(
                    "Image could not remain attached: {why}. Attach it again before continuing."
                ));
                break;
            }
        }
    }
    if image_failed {
        stopped = true;
        // The queue is one message's attachment group. Reject it atomically so
        // a valid sibling cannot accidentally accompany a later queued request.
        // Preserve any images the session already had before this group's transfer.
        while session.attached().len() > previous_images {
            session.detach_image(previous_images);
        }
        // `turns` drained previous text attachments before starting this turn;
        // all text currently in this queue belongs to the rejected group.
        t.files.lock().unwrap().clear();
        say(
            "This request's attachment group was not sent. Attach its files again before resending.",
        );
        if let Some(draft) = attachment_queue::recover_dependent(&mut held) {
            say(&format!(
                "Not sent because its image could not be attached:\n{draft}"
            ));
            if let Some(workspace) = io.screen.workspace() {
                workspace.recover_draft(draft);
            } else {
                say(
                    "Your request is preserved above. Attach the image and send it again when ready.",
                );
            }
        } else if io.inbox.reject_dependent_plan_action() {
            say(
                "The queued plan action was not run because its image could not be attached. Attach the image and select the action again when ready.",
            );
        }
    }
    attachment_queue::publish(
        &io.screen,
        attachment_queue::chips(
            &t.files.lock().unwrap(),
            session
                .attached()
                .iter()
                .map(|i| (i.path.as_path(), i.public)),
        ),
    );
    done.store(true, Ordering::SeqCst);
    let _ = follower.await;
    if let TurnEnd::Asked {
        options: Some(options),
        ..
    } = &end
    {
        // Establish freshness before displaying the question, so a prompt reply
        // cannot be mistaken for input queued before the operator saw it.
        io.inbox.question_mark(&options.id);
    }
    io.screen.end(&end);
    io.inbox.requeue(held);
    (end, steering.take(), stopped)
}

fn plan_status(planning: bool) -> &'static str {
    if planning {
        "PLAN · read-only tools; automatic goals paused. /plan off leaves planning without executing."
    } else {
        "Planning off. Send an implementation request when ready; goals remain paused until /goal resume."
    }
}

/// What `/mode` does.
#[derive(Debug, PartialEq, Eq)]
enum ModeChange {
    /// End this session (left open) and continue in this mode.
    Switch(Mode),
    /// Only say something.
    Say(String),
}

/// `/mode [asked]` in a session of mode `now`. The only switch is up to top
/// clearance: from there, going back would send the frontier a conversation
/// in which the local model saw what the frontier may not, so leaving takes a
/// new session; and a session cannot turn its privacy boundary off.
fn mode_change(now: Mode, asked: Option<&str>) -> ModeChange {
    let say = |s: String| ModeChange::Say(s);
    let Some(asked) = asked else {
        return say(format!(
            "this session is {}. /mode top-clearance continues in top clearance: only the local \
model works and nothing leaves this machine (this session stays open)",
            mode_name(now)
        ));
    };
    let to = match asked.to_ascii_lowercase().as_str() {
        "top-clearance" | "top" | "top clearance" | "local-only" => Mode::TopClearance,
        "hybrid" => Mode::Hybrid,
        "passthrough" => Mode::Passthrough,
        other => {
            return say(format!(
                "unknown mode {other:?}: /mode top-clearance, or /mode to see this session's mode"
            ));
        }
    };
    match (now, to) {
        (a, b) if a == b => say(format!("this session is already {}", mode_name(now))),
        (_, Mode::TopClearance) => ModeChange::Switch(Mode::TopClearance),
        (Mode::TopClearance, _) => say(format!(
            "top clearance cannot be left in a session: its conversation holds what only the \
local model may see. /close this session, then start one with `duet --mode {}`",
            to.as_str()
        )),
        (_, Mode::Passthrough) => say(
            "a session cannot turn its privacy boundary off: start one with `duet --mode \
passthrough --no-privacy`"
                .to_owned(),
        ),
        (_, _) => say(format!(
            "a {} session continues as it is: start one with `duet --mode {}`",
            mode_name(now),
            to.as_str()
        )),
    }
}

/// How the mode reads in the status bar.
fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Hybrid => "hybrid",
        Mode::TopClearance => "top clearance",
        Mode::Passthrough => "passthrough",
    }
}

/// The session's state in the workspace's status bar.
fn show_status(
    io: &Io,
    session: &Session<'_>,
    manifest: &RunManifest,
    cfg: &duet_config::Config,
    goals: &crate::goals::Store,
) {
    let Some(w) = io.screen.workspace() else {
        return;
    };
    let local = match (&manifest.local, manifest.mode) {
        (_, Mode::Passthrough) => None,
        (Some(l), _) => Some(l.model.clone()),
        (None, mode) => local_enabled(cfg, mode)
            .unwrap_or(false)
            .then(|| cfg.str("local.model").ok())
            .flatten(),
    };
    let s = session.stats();
    w.status(Status {
        goal: goals.status_line(),
        planning: session.is_planning(),
        session: manifest.run_id.clone(),
        mode: mode_name(manifest.mode).to_owned(),
        frontier: manifest.frontier_model.clone(),
        local,
        turns: u32::try_from(session.turns()).unwrap_or(u32::MAX),
        cost_usd: s.cost_usd,
        budget_usd: cfg.float("session.frontier_usd").unwrap_or(0.0),
        requests: s.turns,
        tool_calls: s.tool_calls,
        tokens_in: s.usage.input + s.usage.cache_read + s.usage.cache_write,
        tokens_cached: s.usage.cache_read,
        tokens_out: s.usage.output,
        turn_budget_usd: cfg.float("limits.frontier_usd").unwrap_or(0.0),
        worked_secs: session.worked().as_secs(),
        budget_minutes: u64::try_from(cfg.int("session.wall_clock_minutes").unwrap_or(0))
            .unwrap_or(0),
    });
}

/// The last few turns of a resumed session.
fn recap(session: &Session<'_>, id: &str, screen: &Screen) {
    let history: &[Exchange] = session.history();
    screen.line(&format!(
        "session {id} resumed: {} turn(s) so far, ${:.4}",
        session.turns(),
        session.stats().cost_usd
    ));
    let skip = history.len().saturating_sub(5);
    if skip > 0 {
        screen.line(&format!("  ({skip} earlier turn(s) not shown)"));
    }
    for x in &history[skip..] {
        screen.line(&format!(
            "you> {}",
            x.message.trim().replace('\n', "\n     ")
        ));
        for m in &x.steered {
            screen.line(&format!(
                "you, while duet worked> {}",
                m.trim().replace('\n', "\n     ")
            ));
        }
        match &x.end {
            Some(end) => screen.end(end),
            None => screen.line("(this turn did not end; it counts as interrupted)"),
        }
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn minutes(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}m{:02}s", s / 60, s % 60)
}

/// `/status`.
fn status(session: &Session<'_>, id: &str, cfg: &duet_config::Config) -> String {
    let s = session.stats();
    let float = |k: &str| cfg.float(k).unwrap_or(0.0);
    let int = |k: &str| cfg.int(k).unwrap_or(0);
    format!(
        "session {id}\n{}\n\
turns {} · frontier requests {} · tool calls {}\n\
tokens {} in ({} cached), {} out\n\
cost ${:.4} of ${:.2} for the session (each turn at most ${:.2})\n\
agent working time {} of {}m (each turn at most {}m)",
        plan_status(session.is_planning()),
        session.turns(),
        s.turns,
        s.tool_calls,
        thousands(s.usage.input + s.usage.cache_read + s.usage.cache_write),
        thousands(s.usage.cache_read),
        thousands(s.usage.output),
        s.cost_usd,
        float("session.frontier_usd"),
        float("limits.frontier_usd"),
        minutes(session.worked()),
        int("session.wall_clock_minutes"),
        int("limits.wall_clock_minutes"),
    )
}

/// `/diff`: the workspace's changes against the last commit, shown to the
/// operator only. Sensitive files are named, never shown. Outside a git
/// repository: the files duet wrote in this session, against their content
/// before it (see `duet_agent::changes`).
fn local_diff(git: &duet_git::Git, ws: &Path, run_dir: &Path, presenter: &dyn Presenter) -> String {
    if !git.is_repository(ws) {
        return duet_agent::changes::for_operator(git, ws, run_dir, presenter);
    }
    let tracked = git.list_files(ws).unwrap_or_default();
    let (hidden, shown): (Vec<String>, Vec<String>) = tracked
        .into_iter()
        .partition(|f| presenter.path_sensitive(Path::new(f)));
    let mut out = git.diff_paths(ws, &shown).unwrap_or_default();
    let untracked = git
        .run(
            ws,
            &["ls-files", "--others", "--exclude-standard"],
            &[],
            None,
        )
        .map(|o| String::from_utf8_lossy(&o).into_owned())
        .unwrap_or_default();
    for f in untracked.lines().filter(|f| !f.is_empty()) {
        if presenter.path_sensitive(Path::new(f)) {
            out.push_str(&format!("new file {f}: sensitive, held locally\n"));
        } else {
            let lines = std::fs::read_to_string(ws.join(f)).map_or(0, |t| t.lines().count());
            let unit = if lines == 1 { "line" } else { "lines" };
            out.push_str(&format!("new file {f} ({lines} {unit})\n"));
        }
    }
    let changed_hidden: Vec<String> = git
        .diff_paths(ws, &hidden)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("+++ b/"))
        .map(str::to_owned)
        .collect();
    for f in changed_hidden {
        out.push_str(&format!("changed {f}: sensitive, held locally\n"));
    }
    if out.trim().is_empty() {
        "no changes against the last commit\n".into()
    } else {
        out
    }
}

/// Progress lines on standard output, with placeholders restored by `shown`.
fn printed(shown: Arc<dyn Fn(&str) -> String + Send + Sync>) -> Box<dyn FnMut(Entry) + Send> {
    let mut names: HashMap<String, String> = HashMap::new();
    Box::new(move |entry| {
        for l in progress(&entry, &mut names, shown.as_ref()) {
            println!("{l}");
        }
    })
}

/// The transcript from a byte offset on: each entry written since is handed
/// to the sink once, in order.
pub(crate) struct Follow {
    path: PathBuf,
    from: u64,
    partial: String,
    sink: Box<dyn FnMut(Entry) + Send>,
}

impl Follow {
    pub(crate) fn new(path: PathBuf, from: u64, sink: Box<dyn FnMut(Entry) + Send>) -> Self {
        Follow {
            path,
            from,
            partial: String::new(),
            sink,
        }
    }

    /// Hands every entry written so far to the sink.
    pub(crate) fn drain(&mut self) {
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() <= self.from) {
            return;
        }
        if let Ok(mut f) = std::fs::File::open(&self.path)
            && f.seek(SeekFrom::Start(self.from)).is_ok()
        {
            let mut chunk = String::new();
            if let Ok(n) = f.read_to_string(&mut chunk) {
                self.from += n as u64;
                self.partial.push_str(&chunk);
            }
        }
        while let Some(at) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=at).collect();
            if let Ok(entry) = serde_json::from_str::<Entry>(&line) {
                (self.sink)(entry);
            }
        }
    }
}

/// Follows the transcript from byte `from` as it is written, handing each
/// entry to `sink`, until `done` is raised (then reads what is left).
pub(crate) async fn follow(
    path: PathBuf,
    from: u64,
    done: Arc<AtomicBool>,
    sink: Box<dyn FnMut(Entry) + Send>,
) {
    following(Arc::new(Mutex::new(Follow::new(path, from, sink))), done).await;
}

/// [`follow`] over a follower others may drain too.
async fn following(f: Arc<Mutex<Follow>>, done: Arc<AtomicBool>) {
    loop {
        let last = done.load(Ordering::SeqCst);
        f.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain();
        if last {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Streamed responses shown after every step recorded before them: at the
/// start of each request the transcript is read up to its end first, so a
/// step's progress line never shows after the reply that followed it.
struct Ordered {
    follow: Arc<Mutex<Follow>>,
    inner: Arc<dyn duet_boundary::live::StreamTap>,
}

impl duet_boundary::live::StreamTap for Ordered {
    fn event(&self, event: duet_boundary::live::StreamEvent<'_>) {
        if matches!(event, duet_boundary::live::StreamEvent::Attempt(_)) {
            self.follow
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain();
        }
        self.inner.event(event);
    }
}

#[cfg(test)]
#[path = "chat/goal_tests.rs"]
mod goal_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn planning_before_first_message_preserves_mode_and_rejects_goals() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = duet_config::Config::load(&dir.path().join("owner.toml"), None).unwrap();
        for (lines, expected, planning) in [
            (
                vec![
                    "/plan",
                    "/goal do not start",
                    "/plan status",
                    "Inspect the design",
                ],
                "Inspect the design",
                true,
            ),
            (
                vec!["/plan on", "/plan off", "Implement it"],
                "Implement it",
                false,
            ),
            (vec!["/plan Inspect the design"], "Inspect the design", true),
            (vec!["/plan /literal task"], "//literal task", true),
        ] {
            let inbox = Inbox::default();
            for line in lines {
                inbox.push(line.to_owned());
            }
            inbox.close();
            let (message, _, _, goal, enabled) = first_message(
                &inbox,
                &AtomicBool::new(false),
                &Screen::Plain { tty: false },
                dir.path(),
                &cfg,
                Mode::Hybrid,
                None,
                false,
            )
            .await
            .unwrap();
            assert_eq!(message, expected);
            assert!(!goal);
            assert_eq!(enabled, planning);
        }
        assert_eq!(parse("/planet"), Command::Unknown("planet".into()));
        assert_eq!(parse("//plan"), Command::Message("/plan".into()));
    }

    #[tokio::test]
    async fn invalid_initial_workflows_are_reported_before_a_session_starts() {
        let directory = tempfile::tempdir().unwrap();
        let ws = directory.path().canonicalize().unwrap();
        let owner = ws.join("config.toml");
        std::fs::write(
            &owner,
            "[extensions]\nskills_enabled = false\nplugins_enabled = false\n",
        )
        .unwrap();
        let cfg = duet_config::Config::load(&owner, None).unwrap();
        let inbox = Inbox::default();
        inbox.push("/skill example task".into());
        inbox.push("/command example:review task".into());
        inbox.push("//skill this is literal text".into());
        inbox.close();
        let message = first_message(
            &inbox,
            &AtomicBool::new(false),
            &Screen::Plain { tty: false },
            &ws,
            &cfg,
            Mode::Hybrid,
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(message.0, "//skill this is literal text");
        assert!(!ws.join(".duet").exists());
        assert!(
            crate::extensions::list(&duet_agent::skills::Skills::default(), &cfg)
                .contains("disabled")
        );
    }

    #[tokio::test]
    async fn dropped_images_and_pending_removal_work_before_the_first_message() {
        let directory = tempfile::tempdir().unwrap();
        let ws = directory.path().canonicalize().unwrap();
        let image = ws.join("screen (1).PNG");
        std::fs::write(&image, duet_provider::image::solid_png(2, 2, [0, 0, 255])).unwrap();
        let mut cfg = duet_config::Config::load(&ws.join("owner.toml"), None).unwrap();
        cfg.set_owner("frontier.vision", toml::Value::Boolean(true))
            .unwrap();
        let inbox = Inbox::default();
        inbox.push(format!("/attach {}", crate::attachments::quoted(&image)));
        inbox.push("/attachments".into());
        inbox.push("/detach i1".into());
        inbox.push(crate::attachments::quoted(&image));
        inbox.push("Describe this screen".into());
        inbox.close();
        let (message, images, files, _, _) = first_message(
            &inbox,
            &AtomicBool::new(false),
            &Screen::Plain { tty: false },
            &ws,
            &cfg,
            Mode::Passthrough,
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(message, "Describe this screen");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].path, image);
        assert!(
            !images[0].public,
            "dropped images must never be promoted to public"
        );
        assert!(files.is_empty());
    }

    #[test]
    fn commands_and_messages() {
        assert_eq!(parse("  "), Command::Empty);
        assert_eq!(parse("/diff"), Command::Diff);
        assert_eq!(parse("/skills"), Command::Skills);
        assert_eq!(parse("/plugins"), Command::Plugins);
        assert_eq!(
            parse("/skill review task"),
            Command::Skill("review task".into())
        );
        assert_eq!(
            parse("/command kit:review task"),
            Command::PluginPrompt("kit:review task".into())
        );
        assert_eq!(
            parse("//skill review"),
            Command::Message("/skill review".into())
        );
        assert_eq!(parse(" /quit "), Command::Quit);
        assert_eq!(parse("/close"), Command::Close);
        assert_eq!(parse("/nope"), Command::Unknown("nope".into()));
        assert_eq!(parse("/mode"), Command::Mode(None));
        assert_eq!(
            parse("/mode  top-clearance "),
            Command::Mode(Some("top-clearance".into()))
        );
        assert_eq!(parse("/modes"), Command::Unknown("modes".into()));
        assert_eq!(
            parse("//etc is a dir"),
            Command::Message("/etc is a dir".into())
        );
        assert_eq!(parse("fix it  "), Command::Message("fix it".into()));
        let image = |path: &str, public| Command::Image {
            path: path.into(),
            public,
        };
        assert_eq!(parse("/image shots/a b.png"), image("shots/a b.png", false));
        assert_eq!(parse("/image --public ui.png "), image("ui.png", true));
        assert_eq!(parse("/image"), image("", false));
        assert_eq!(parse("/images"), Command::Unknown("images".into()));
        assert_eq!(
            parse("/attach './screen shot.png'"),
            image("'./screen shot.png'", false)
        );
        assert_eq!(parse("./screen.png"), image("./screen.png", false));
        assert_eq!(parse("screen.PNG"), image("screen.PNG", false));
        assert_eq!(parse("/attachments"), Command::Attachments);
        assert_eq!(parse("/detach all"), Command::Detach("all".into()));
    }

    #[test]
    fn mode_only_ever_switches_up_to_top_clearance() {
        use ModeChange::{Say, Switch};
        for now in [Mode::Hybrid, Mode::Passthrough] {
            for asked in ["top-clearance", "top", "Top-Clearance", "local-only"] {
                assert_eq!(
                    mode_change(now, Some(asked)),
                    Switch(Mode::TopClearance),
                    "{now:?} {asked}"
                );
            }
        }
        let says = |now, asked| match mode_change(now, asked) {
            Say(s) => s,
            Switch(m) => panic!("switched to {m:?}"),
        };
        assert!(says(Mode::Hybrid, None).contains("this session is hybrid"));
        assert!(says(Mode::TopClearance, Some("top")).contains("already top clearance"));
        // Top clearance is never left in a session.
        for asked in ["hybrid", "passthrough"] {
            let s = says(Mode::TopClearance, Some(asked));
            assert!(s.contains("cannot be left in a session"), "{s}");
            assert!(s.contains(&format!("duet --mode {asked}")), "{s}");
        }
        // Nor is the boundary turned off, or a mode changed sideways.
        assert!(says(Mode::Hybrid, Some("passthrough")).contains("cannot turn its privacy"));
        assert!(says(Mode::Passthrough, Some("hybrid")).contains("duet --mode hybrid"));
        assert!(says(Mode::Hybrid, Some("fast")).contains("unknown mode"));
    }

    #[test]
    fn a_trailing_backslash_continues_the_message() {
        let inbox = Inbox::default();
        inbox.read_from("one\\\ntwo\nthree\n".as_bytes());
        assert_eq!(inbox.pop().as_deref(), Some("one\ntwo"));
        assert_eq!(inbox.pop().as_deref(), Some("three"));
        assert!(inbox.drained());
    }

    #[test]
    fn ctrl_c_interrupts_a_turn_and_twice_at_the_prompt_leaves() {
        let flag = || Arc::new(AtomicBool::new(false));
        let i = Interrupts {
            working: flag(),
            interrupted: flag(),
            leave: flag(),
            idle_press: Mutex::new(None),
        };
        i.working.store(true, Ordering::SeqCst);
        assert_eq!(
            i.press(),
            Some("interrupt: stopping this turn; the session stays open")
        );
        assert!(i.interrupted.load(Ordering::SeqCst) && !i.leave.load(Ordering::SeqCst));
        i.working.store(false, Ordering::SeqCst);
        assert!(
            i.press()
                .unwrap()
                .starts_with("Ctrl-C again leaves the session")
        );
        assert!(!i.leave.load(Ordering::SeqCst));
        assert_eq!(i.press(), None);
        assert!(i.leave.load(Ordering::SeqCst));
        // A press long after the first one only warns again.
        i.leave.store(false, Ordering::SeqCst);
        *i.idle_press.lock().unwrap() = Some(std::time::Instant::now() - Duration::from_secs(3));
        assert!(i.press().is_some() && !i.leave.load(Ordering::SeqCst));
    }

    #[test]
    fn approval_input_requeues_planning_before_releasing_the_input_barrier() {
        for (line, interrupts) in [(" /plan inspect first ", true), ("/plan status", false)] {
            let inbox = Inbox::default();
            inbox.push("earlier queued message".to_owned());
            let mark = inbox.mark();
            inbox.push(line.to_owned());
            inbox.push("following message".to_owned());
            let stop = AtomicBool::new(false);
            assert!(inbox.pop_unless_answering().is_none());
            // No AskInline postprocessing runs: the inbox must already expose
            // the interrupt and the control first when the barrier is released.
            assert_eq!(inbox.answer_after(mark, &stop).as_deref(), Some(line));
            let q = inbox.lock();
            assert!(!q.answering);
            assert_eq!(stop.load(Ordering::SeqCst), interrupts);
            assert_eq!(q.lines.front(), Some(&(None, line.to_owned())));
            drop(q);
            assert_eq!(inbox.pop_unless_answering().as_deref(), Some(line));
            assert_eq!(inbox.pop().as_deref(), Some("earlier queued message"));
            assert_eq!(inbox.pop().as_deref(), Some("following message"));
            assert!(inbox.pop().is_none());
        }
    }

    #[test]
    fn planning_at_an_approval_prompt_is_preserved_and_denies_the_action() {
        for (answer, approved, interrupted, requeued) in [
            ("/plan", false, true, true),
            (" /plan on ", false, true, true),
            ("/plan off", false, true, true),
            ("/plan Inspect this first", false, true, true),
            ("/plan status", false, false, true),
            ("//plan", false, false, false),
            ("y", true, false, false),
        ] {
            let inbox = Arc::new(Inbox::default());
            let stop = Arc::new(AtomicBool::new(false));
            let feed = inbox.clone();
            let writer = std::thread::spawn(move || {
                let started = std::time::Instant::now();
                while !feed.answering() {
                    assert!(started.elapsed() < Duration::from_secs(5));
                    std::thread::yield_now();
                }
                feed.push(answer.to_owned());
                feed.push("following message".to_owned());
            });
            let ask = AskInline {
                mode: ApproveMode::All,
                inbox: inbox.clone(),
                interrupted: stop.clone(),
                screen: Screen::Plain { tty: false },
            };
            assert_eq!(
                ask.ask(&Action {
                    tool: "write_file".to_owned(),
                    risk: duet_agent::oversight::Risk::Write,
                    path: Some("source.rs".to_owned()),
                    command: None,
                    bytes: Some(1),
                }),
                approved,
                "{answer}"
            );
            writer.join().unwrap();
            assert_eq!(stop.load(Ordering::SeqCst), interrupted, "{answer}");
            if requeued {
                assert_eq!(inbox.pop().as_deref(), Some(answer));
            }
            assert_eq!(inbox.pop().as_deref(), Some("following message"));
            assert!(inbox.pop().is_none());
        }
    }

    #[test]
    fn an_approval_answer_is_the_first_line_after_the_question() {
        let inbox = Inbox::default();
        inbox.push("queued message".into());
        let mark = inbox.mark();
        inbox.push("y".into());
        let stop = AtomicBool::new(false);
        assert_eq!(inbox.answer_after(mark, &stop).as_deref(), Some("y"));
        // The message typed before the question stays queued.
        assert_eq!(inbox.pop().as_deref(), Some("queued message"));
        stop.store(true, Ordering::SeqCst);
        assert_eq!(inbox.answer_after(inbox.mark(), &stop), None);
    }

    #[test]
    fn background_images_and_requeued_commands_never_answer_approval() {
        let inbox = Inbox::default();
        // Include mark zero: it must not treat requeued work as an answer.
        let mark = inbox.mark();
        inbox.requeue(vec!["/status".into()]);
        inbox.push_deferred("/image 'clipboard.png'".into());
        assert_eq!(inbox.pop_unless_answering(), None);
        inbox.push("y".into());
        assert_eq!(
            inbox.answer_after(mark, &AtomicBool::new(false)).as_deref(),
            Some("y")
        );
        assert_eq!(inbox.pop().as_deref(), Some("/status"));
        assert_eq!(inbox.pop().as_deref(), Some("/image 'clipboard.png'"));

        let mark = inbox.mark();
        inbox.push_deferred("/image 'another.png'".into());
        inbox.close();
        assert_eq!(inbox.answer_after(mark, &AtomicBool::new(false)), None);
        assert_eq!(inbox.pop().as_deref(), Some("/image 'another.png'"));
    }

    #[test]
    fn typed_plan_actions_preserve_fifo_and_never_approve_tools() {
        use duet_tui::workspace::{PlanAction, PlanIdentity};
        let inbox = Inbox::default();
        let identity = PlanIdentity {
            revision: "r1".into(),
            digest: "digest".into(),
        };
        inbox.push("earlier message".into());
        inbox.push_plan_action(PlanAction::Approve { identity });
        inbox.push("later message".into());
        assert_eq!(inbox.pop().as_deref(), Some("earlier message"));
        assert!(inbox.pop().is_none());
        let mark = inbox.mark();
        let stopped = AtomicBool::new(false);
        assert!(inbox.answer_after(mark, &stopped).is_none());
        assert!(stopped.load(Ordering::SeqCst));
        assert!(matches!(
            inbox.pop_input(),
            Some(OperatorInput::Plan(PlanAction::Approve { .. }))
        ));
        assert_eq!(inbox.pop().as_deref(), Some("later message"));
    }

    #[test]
    fn superseded_question_returns_held_text_without_sending_it_to_a_later_question() {
        let inbox = Inbox::default();
        inbox.question_mark("old-question");
        inbox.hold_for_question("held text".into());
        assert!(
            inbox
                .discard_stale_question(Some("old-question"))
                .is_empty()
        );
        assert_eq!(
            inbox.discard_stale_question(Some("new-question")),
            vec!["held text"]
        );
        inbox.question_mark("new-question");
        inbox.release_question_input();
        assert!(inbox.pop().is_none());
    }
}
