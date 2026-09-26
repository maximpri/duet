// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet chat`: a session on the terminal, line by line.
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
//! of input; `duet chat --resume` continues it. `/close` ends it for good.
//!
//! On a terminal the console ([`crate::term::console`]) reads the keyboard
//! instead of the reader thread: a line editor with history (the session's
//! own messages) and completion, duet's replies streamed and formatted as
//! they arrive, and a status line while duet works. The lines it sends and
//! the Ctrl-C rules are the same.

use crate::approve;
use crate::term::console::{Console, Hooks, Mode as Live};
use crate::{
    LocalOverride, Mode, Prepared, RunLimits, RunManifest, audit_log_path, checked_run_id,
    frontier_dialect, load_config, local_enabled, new_run_id, open_audit, prepare, setup,
};
use anyhow::{Context, Result, bail, ensure};
use duet_agent::oversight::Action;
use duet_agent::session::{Exchange, SessionLimits};
use duet_agent::transcript::Entry;
use duet_agent::{ApproveMode, Approver, RunStats, Session, Terminal, TurnEnd};
use duet_boundary::audit::AuditHandle;
use duet_boundary::model::Item;
use duet_boundary::view::{PassThrough, Presenter, ViewClass};
use futures_util::FutureExt;
use futures_util::future::Either;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// `duet chat`'s arguments.
pub(crate) struct ChatArgs {
    pub message: Option<String>,
    pub mode: Mode,
    pub frontier_url: Option<String>,
    pub frontier_model: Option<String>,
    pub no_privacy: bool,
    /// `Some(None)`: the most recent open session.
    pub resume: Option<Option<String>>,
}

/// Characters of a progress line.
const LINE_CHARS: usize = 160;

/// Lines typed by the operator, in order, each with a sequence number.
#[derive(Default)]
pub(crate) struct Inbox {
    queue: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Default)]
struct Queue {
    lines: VecDeque<(u64, String)>,
    next: u64,
    closed: bool,
    /// An approval question waits for the operator's answer.
    answering: bool,
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
        q.lines.push_back((n, line));
        drop(q);
        self.ready.notify_all();
    }

    fn pop(&self) -> Option<String> {
        self.lock().lines.pop_front().map(|(_, l)| l)
    }

    /// The input ended (the console's Ctrl-D, or its terminal is gone).
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
        if q.answering {
            return None;
        }
        q.lines.pop_front().map(|(_, l)| l)
    }

    /// Puts lines back at the front, in order (commands held for later).
    fn requeue(&self, lines: Vec<String>) {
        let mut q = self.lock();
        for l in lines.into_iter().rev() {
            q.lines.push_front((0, l));
        }
    }

    /// Whether the input ended and every line was taken.
    fn drained(&self) -> bool {
        let q = self.lock();
        q.closed && q.lines.is_empty()
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
            if let Some(i) = q.lines.iter().position(|(n, _)| *n >= mark) {
                q.answering = false;
                return q.lines.remove(i).map(|(_, l)| l);
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
    /// End the running turn after its current step.
    Stop,
    Status,
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
    Unknown(String),
    Empty,
}

/// The commands, for completion.
pub(crate) const COMMANDS: &[&str] = &[
    "/close", "/diff", "/exit", "/help", "/image", "/quit", "/status", "/stop", "/undo",
];

pub(crate) fn parse(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Empty;
    }
    if let Some(rest) = trimmed.strip_prefix("//") {
        return Command::Message(format!("/{rest}"));
    }
    let Some(word) = trimmed.strip_prefix('/') else {
        return Command::Message(line.trim_end().to_owned());
    };
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
    match word {
        "status" => Command::Status,
        "diff" => Command::Diff,
        "undo" => Command::Undo,
        "stop" => Command::Stop,
        "help" | "?" => Command::Help,
        "quit" | "exit" => Command::Quit,
        "close" => Command::Close,
        other => Command::Unknown(other.to_owned()),
    }
}

const HELP: &str = "\
Type a message and press Enter; end a line with \\ to continue on the next.
While duet works, a message steers it: it arrives after the current step.
Commands (never sent to the model):
  /stop    end duet's turn after its current step (Ctrl-C ends it now)
  /status  turns, tokens, cost and time against the budgets
  /diff    what changed in the workspace (sensitive files are only named)
  /undo    revert the file writes of the last turn (repeat for earlier turns)
  /image <path>           attach an image to your next message; in hybrid mode the
                          local model describes it (needs local.vision)
  /image --public <path>  attach an image the frontier may see itself (not scanned;
                          never from a sensitive path; needs frontier.vision)
  /quit    leave; the session stays open for `duet chat --resume`
  /close   end the session for good
  //text   send a message that starts with /
Ctrl-C stops duet's turn at once (a running command is killed);
at the prompt, Ctrl-C twice leaves.";

/// Where the conversation is written: line by line on standard output
/// (without a terminal, or when it cannot be driven: exactly as it always
/// was), or the console.
#[derive(Clone)]
pub(crate) enum Screen {
    Plain { tty: bool },
    Live(Arc<Console>),
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

    fn console(&self) -> Option<&Arc<Console>> {
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
        .context("no open session in this workspace; start one with `duet chat`")
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

/// Waits for the first message of a new session, and the images attached to
/// it (`/image`, checked against the rules as they are attached); `None`
/// when the operator leaves first.
async fn first_message(
    inbox: &Inbox,
    leave: &AtomicBool,
    screen: &Screen,
    ws: &Path,
    cfg: &duet_config::Config,
    mode: Mode,
) -> Option<(String, Vec<crate::images::AttachedImage>)> {
    if !matches!(screen, Screen::Plain { tty: false }) {
        screen
            .line("duet chat: a new session starts with your first message. /help lists commands.");
    }
    screen.prompt();
    let mut images = Vec::new();
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
            Command::Message(m) => return Some((m, images)),
            Command::Quit | Command::Close => return None,
            Command::Help => screen.text(&format!("{HELP}\n")),
            Command::Empty => {}
            Command::Image { path, public } => {
                let a = crate::images::AttachedImage {
                    path: image_path(ws, &path),
                    public,
                };
                match crate::images::precheck(ws, cfg, mode, std::slice::from_ref(&a)) {
                    Ok(()) => {
                        screen.line(&format!(
                            "image {path} will be attached to your first message"
                        ));
                        images.push(a);
                    }
                    Err(e) => screen.line(&format!("{e:#}")),
                }
            }
            _ => screen.line("nothing to show yet: the session starts with your first message"),
        }
        screen.prompt();
    }
}

/// The file `/image` names: as given, else relative to the workspace.
fn image_path(ws: &Path, raw: &str) -> PathBuf {
    let raw = raw.trim().trim_matches(['"', '\'']);
    let given = std::env::current_dir()
        .map(|d| d.join(raw))
        .unwrap_or_else(|_| PathBuf::from(raw));
    if given.exists() { given } else { ws.join(raw) }
}

/// `duet chat`. Returns the exit code: 0 when the session was left open or
/// closed, 3 when a session budget is spent, 1 when it could not start.
pub(crate) async fn chat(ws: PathBuf, args: ChatArgs) -> Result<i32> {
    match (args.mode, args.no_privacy) {
        (Mode::Passthrough, false) if args.resume.is_none() => bail!(
            "--mode passthrough turns the privacy boundary off: everything the model reads, \
including secrets and personal data, is sent to the frontier provider unfiltered. \
Add --no-privacy to confirm, or use --mode hybrid."
        ),
        (Mode::Hybrid | Mode::LocalOnly, true) => {
            bail!("--no-privacy only applies to --mode passthrough")
        }
        _ => {}
    }
    let cfg = load_config(&ws)?;
    let tty = std::io::stdin().is_terminal();
    let interrupted = Arc::new(AtomicBool::new(false));
    let working = Arc::new(AtomicBool::new(false));
    let leave = Arc::new(AtomicBool::new(false));
    let interrupts = Arc::new(Interrupts {
        working: working.clone(),
        interrupted: interrupted.clone(),
        leave: leave.clone(),
        idle_press: Mutex::new(None),
    });
    let (inbox, screen) = open_screen(tty, &interrupts);
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

    let (manifest, resume) = match args.resume {
        Some(id) => {
            ensure!(
                args.message.is_none(),
                "--resume continues a session; send the message once it is open"
            );
            (resumed(&ws, id)?, true)
        }
        None => {
            let (first, first_images) = match args.message {
                Some(m) => (m, Vec::new()),
                None => match first_message(&inbox, &leave, &screen, &ws, &cfg, args.mode).await {
                    Some(m) => m,
                    None => return Ok(0),
                },
            };
            // Setting up prints freely.
            screen.hide();
            // With the local model off nothing is probed for one.
            let local = match args.mode {
                Mode::Hybrid | Mode::LocalOnly if local_enabled(&cfg, args.mode)? => {
                    match setup::bootstrap(&cfg).await {
                        Ok(found) => found.map(|b| LocalOverride {
                            base_url: b.base_url,
                            model: b.model,
                        }),
                        Err(code) => return Ok(code),
                    }
                }
                _ => None,
            };
            let manifest = RunManifest {
                run_id: new_run_id(),
                mode: args.mode,
                objective: first,
                frontier_url: args
                    .frontier_url
                    .map_or_else(|| cfg.str("frontier.base_url"), Ok)?,
                frontier_model: args
                    .frontier_model
                    .map_or_else(|| cfg.str("frontier.model"), Ok)?,
                frontier_dialect: Some(frontier_dialect(&cfg)?.as_str().to_owned()),
                local,
                session: true,
                images: first_images,
            };
            (manifest, false)
        }
    };
    eprintln!("session {} ({:?})", manifest.run_id, manifest.mode);
    let _lock = duet_fs::lock::WorkspaceLock::acquire(&ws)?;
    let run_dir = ws.join(".duet/runs").join(&manifest.run_id);
    duet_fs::private::ensure_private_dir(&run_dir)?;
    duet_fs::private::ensure_private_dir(&ws.join(".duet/tmp"))?;
    if !resume {
        duet_fs::private::write_private(
            &run_dir.join("run.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
    }
    // Sessions last as long as the operator keeps them open: the provider's
    // own retry deadline is far away, and each turn sets its own.
    let limits = RunLimits {
        deadline: tokio::time::Instant::now() + Duration::from_secs(7 * 24 * 3600),
        interrupted: interrupted.clone(),
    };
    let io = Io {
        inbox,
        tty,
        working,
        leave,
        screen: screen.clone(),
    };
    let mut audit = None;
    let driven = std::panic::AssertUnwindSafe(converse(
        &ws, &manifest, &cfg, oversight, &run_dir, resume, &limits, &mut audit, &io,
    ))
    .catch_unwind()
    .await;
    let (terminal, stats) = match driven {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => (
            Terminal::Failed {
                reason: format!("{e:#}"),
            },
            RunStats::default(),
        ),
        Err(panic) => (
            Terminal::internal_error(panic.as_ref()),
            RunStats::default(),
        ),
    };
    screen.close();
    let audit = match audit {
        Some(a) => Some(a),
        None => open_audit(&ws, &manifest.run_id)
            .map(AuditHandle::new)
            .map_err(|e| eprintln!("warning: audit log not opened: {e:#}"))
            .ok(),
    };
    duet_agent::conclude(
        &run_dir,
        &manifest.run_id,
        audit.as_ref(),
        &audit_log_path(&ws, &manifest.run_id),
        &terminal,
        &stats,
    )?;
    let id = &manifest.run_id;
    Ok(match &terminal {
        Terminal::Completed { .. } => {
            println!("session {id} closed (${:.4} in total)", stats.cost_usd);
            0
        }
        Terminal::Failed { reason } if reason == duet_agent::session::SESSION_LEFT => {
            println!(
                "session {id} left open (${:.4} so far); continue with: duet chat --resume {id}",
                stats.cost_usd
            );
            0
        }
        Terminal::Failed { reason } => {
            eprintln!("session {id} stopped: {reason}");
            1
        }
        Terminal::BudgetStopped { which } => {
            println!(
                "session {id}: {which} is spent; raise it to continue with duet chat --resume {id}"
            );
            3
        }
    })
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
}

/// The console when standard input and output are a terminal that can take
/// it (its lines go to the inbox), else the reader thread and plain lines.
fn open_screen(tty: bool, interrupts: &Arc<Interrupts>) -> (Arc<Inbox>, Screen) {
    let plain = || (Inbox::stdin(), Screen::Plain { tty });
    if !(tty && std::io::stdout().is_terminal() && crate::term::capable()) {
        return plain();
    }
    let inbox = Arc::new(Inbox::default());
    let hooks = Hooks {
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
    match Console::start(hooks) {
        Ok(console) => (inbox, Screen::Live(console)),
        Err(_) => plain(),
    }
}

/// Closes the screen when dropped.
struct Closing(Screen);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Ctrl-C, as a signal or (on the console) a key.
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

/// Ctrl-C sent as a signal (the console reads the key itself).
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
) -> Result<(Terminal, RunStats)> {
    let Prepared {
        git,
        engine,
        frontier,
        run_cfg,
    } = prepare(ws, manifest, cfg, oversight, run_dir, limits, audit).await?;
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
    let shown: Arc<dyn Fn(&str) -> String + Send + Sync> = match &engine {
        Some(e) => {
            let e = e.clone();
            Arc::new(move |t: &str| e.detokenize(t))
        }
        None => Arc::new(|t: &str| t.to_owned()),
    };
    if let Some(c) = io.screen.console() {
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
    let ctx = TurnCtx {
        run_dir,
        io,
        shown: &shown,
        ws,
        git: &git,
        presenter,
    };
    if resume {
        recap(&session, &manifest.run_id, &io.screen);
    } else {
        turns(&mut session, manifest.objective.clone(), &ctx).await;
    }
    let closed = loop {
        if io.leave.load(Ordering::SeqCst) {
            break false;
        }
        if let Some(which) = session.spent() {
            io.screen.line(&format!(
                "the session budget {which} is spent; no further turns can start"
            ));
            break false;
        }
        io.screen.prompt();
        let line = loop {
            if io.leave.load(Ordering::SeqCst) {
                break None;
            }
            match io.inbox.pop() {
                Some(l) => break Some(l),
                None if io.inbox.drained() => break None,
                None => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };
        let Some(line) = line else {
            if matches!(io.screen, Screen::Plain { tty: true }) {
                println!();
            }
            break false;
        };
        let say = |text: &str| io.screen.line(text);
        match parse(&line) {
            Command::Empty => {}
            Command::Quit => break false,
            Command::Close => break true,
            Command::Help => io.screen.text(&format!("{HELP}\n")),
            Command::Status => say(&status(&session, &manifest.run_id, cfg)),
            Command::Diff => io.screen.diff(&local_diff(&git, ws, run_dir, presenter)),
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
            Command::Stop => say("duet is not working; nothing to stop"),
            Command::Unknown(c) => say(&format!("unknown command /{c}; /help lists the commands")),
            Command::Image { path, .. } if path.is_empty() => say(&format!(
                "usage: /image [--public] <path>{}",
                match session.attached().len() {
                    0 => String::new(),
                    n => format!(" ({n} image(s) wait for your next message)"),
                }
            )),
            Command::Image { path, public } => {
                match session.attach(image_path(ws, &path), public) {
                    Ok(said) => say(&format!("attached {said}")),
                    Err(e) => say(&e.to_string()),
                }
            }
            Command::Message(m) => {
                if !io.tty {
                    // Piped input is not on screen: the output keeps the conversation.
                    say(&format!("you> {}", m.replace('\n', "\n     ")));
                }
                turns(&mut session, m, &ctx).await;
            }
        }
    };
    let (terminal, mut stats) = session.end(closed);
    crate::mcp::stop(&run_cfg).await;
    // The session's language servers lived across its turns; stop them now.
    if let Some(lsp) = &run_cfg.lsp {
        lsp.shutdown().await;
    }
    stats.ledger.local = engine.as_ref().and_then(|e| e.take_local_stats());
    Ok((terminal, stats))
}

/// What a turn needs besides the session.
struct TurnCtx<'a> {
    run_dir: &'a Path,
    io: &'a Io,
    shown: &'a Arc<dyn Fn(&str) -> String + Send + Sync>,
    ws: &'a Path,
    git: &'a duet_git::Git,
    presenter: &'a dyn Presenter,
}

/// Runs a turn for `message`; a message that arrived as a turn ended (too
/// late to steer it) starts the next one, unless the operator stopped it.
async fn turns(session: &mut Session<'_>, message: String, t: &TurnCtx<'_>) {
    let mut next = Some(message);
    while let Some(m) = next.take() {
        let (end, late) = take_turn(session, &m, t).await;
        if late.is_empty() {
            continue;
        }
        if matches!(end, TurnEnd::Stopped | TurnEnd::Interrupted) {
            for l in &late {
                t.io.screen.line(&format!(
                    "not delivered (you stopped the turn): {}",
                    clip(l)
                ));
            }
        } else if session.spent().is_none() {
            t.io.screen
                .line("your message arrived as duet ended its turn; it starts the next one");
            next = Some(late.join("\n\n"));
        }
    }
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
) -> (TurnEnd, Vec<String>) {
    let io = t.io;
    let path = t.run_dir.join("transcript.jsonl");
    let from = std::fs::metadata(&path).map_or(0, |m| m.len());
    let done = Arc::new(AtomicBool::new(false));
    let sink: Box<dyn FnMut(Entry) + Send> = match io.screen.console() {
        Some(c) => {
            let c = c.clone();
            Box::new(move |e| c.entry(e))
        }
        None => printed(t.shown.clone()),
    };
    let follower = tokio::spawn(follow(path, from, done.clone(), sink));
    let steering = session.steering();
    let mut held = Vec::new();
    io.working.store(true, Ordering::SeqCst);
    let say = |text: &str| io.screen.line(text);
    let end = {
        // On the console the frontier's responses show as they stream.
        let turn = match io.screen.console() {
            Some(c) => Either::Left(duet_boundary::live::observe(c.tap(), session.turn(message))),
            None => Either::Right(session.turn(message)),
        };
        if let Some(c) = io.screen.console() {
            c.begin();
        }
        let mut turn = std::pin::pin!(turn);
        loop {
            tokio::select! {
                end = &mut turn => break end,
                () = tokio::time::sleep(Duration::from_millis(50)) => {
                    while let Some(line) = io.inbox.pop_unless_answering() {
                        match parse(&line) {
                            Command::Message(m) => {
                                say(&format!(
                                    "  ▸ for duet after the current step: {}",
                                    clip(&m)
                                ));
                                steering.steer(m);
                            }
                            Command::Stop => {
                                steering.stop();
                                say("  ■ stopping after the current step");
                            }
                            Command::Diff => {
                                io.screen.diff(&local_diff(t.git, t.ws, t.run_dir, t.presenter))
                            }
                            Command::Help => io.screen.text(&format!("{HELP}\n")),
                            Command::Empty => {}
                            Command::Unknown(c) => {
                                say(&format!("unknown command /{c}; /help lists the commands"))
                            }
                            Command::Quit
                            | Command::Close
                            | Command::Undo
                            | Command::Status
                            | Command::Image { .. } => {
                                say(&format!("  (after this turn: {})", line.trim()));
                                held.push(line);
                            }
                        }
                    }
                }
            }
        }
    };
    io.working.store(false, Ordering::SeqCst);
    done.store(true, Ordering::SeqCst);
    let _ = follower.await;
    io.screen.end(&end);
    io.inbox.requeue(held);
    (end, steering.take())
}

/// How a turn's end reads in the conversation.
pub(crate) fn describe_end(end: &TurnEnd) -> String {
    let indent = |text: &str| text.trim().replace('\n', "\n      ");
    match end {
        TurnEnd::Replied { message } => format!("duet: {}", indent(message)),
        TurnEnd::Asked { question } => format!(
            "duet asks: {}\n      (your next message is the answer)",
            indent(question)
        ),
        TurnEnd::Completed { summary } => format!("duet finished: {}", indent(summary)),
        TurnEnd::Failed { reason } => format!(
            "this turn failed: {reason}\n      (the session is still open; your next message continues it)"
        ),
        TurnEnd::BudgetStopped { which } => format!(
            "this turn stopped: {which} reached (your next message continues within the limits)"
        ),
        TurnEnd::Interrupted => {
            "this turn was interrupted; your next message continues (duet is told)".into()
        }
        TurnEnd::Stopped => {
            "this turn stopped after a step, as you asked; your next message continues".into()
        }
    }
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
        "session {id}\n\
turns {} · frontier requests {} · tool calls {}\n\
tokens {} in ({} cached), {} out\n\
cost ${:.4} of ${:.2} for the session (each turn at most ${:.2})\n\
agent working time {} of {}m (each turn at most {}m)",
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

/// Follows the transcript from byte `from` as it is written, handing each
/// entry to `sink`, until `done` is raised (then reads what is left).
pub(crate) async fn follow(
    path: PathBuf,
    mut from: u64,
    done: Arc<AtomicBool>,
    mut sink: Box<dyn FnMut(Entry) + Send>,
) {
    let mut partial = String::new();
    loop {
        let last = done.load(Ordering::SeqCst);
        if let Ok(mut f) = std::fs::File::open(&path)
            && f.seek(SeekFrom::Start(from)).is_ok()
        {
            let mut chunk = String::new();
            if let Ok(n) = f.read_to_string(&mut chunk) {
                from += n as u64;
                partial.push_str(&chunk);
            }
        }
        while let Some(at) = partial.find('\n') {
            let line: String = partial.drain(..=at).collect();
            if let Ok(entry) = serde_json::from_str::<Entry>(&line) {
                sink(entry);
            }
        }
        if last {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

fn clip(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > LINE_CHARS {
        format!("{}…", line.chars().take(LINE_CHARS).collect::<String>())
    } else {
        line.to_owned()
    }
}

fn held(class: ViewClass) -> &'static str {
    match class {
        ViewClass::Raw => "shown as is",
        ViewClass::Tokenized => "values replaced by placeholders",
        ViewClass::HandleSummary => "sensitive, held locally (a summary was sent)",
        ViewClass::LocalAnswer => "answered by the local model",
        ViewClass::BulkyHandle => "large, held locally (an outline was sent)",
        ViewClass::Protected => "protected source (only its interface was sent)",
    }
}

/// The progress lines one transcript entry adds: tool calls, tool errors,
/// what the boundary withheld, context masking and interrupts.
pub(crate) fn progress(
    entry: &Entry,
    names: &mut HashMap<String, String>,
    shown: &dyn Fn(&str) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    match entry {
        Entry::Item {
            item: Item::Assistant {
                text, tool_calls, ..
            },
        } => {
            let to_operator = |n: &str| n == "reply" || n == "ask_operator";
            if !text.trim().is_empty() && tool_calls.iter().any(|c| !to_operator(&c.name)) {
                out.push(format!("  ~ {}", clip(&shown(text))));
            }
            for c in tool_calls {
                names.insert(c.id.clone(), c.name.clone());
                if to_operator(&c.name) {
                    continue;
                }
                let arg = ["path", "command", "pattern", "dir", "question"]
                    .iter()
                    .find_map(|k| c.arguments.get(*k).and_then(|v| v.as_str()))
                    .map(|a| clip(&shown(a)))
                    .unwrap_or_default();
                let what = if c.name == "finish" {
                    "finish (running the checks)".to_owned()
                } else {
                    format!("{} {arg}", c.name)
                };
                out.push(format!("  · {}", what.trim_end()));
            }
        }
        Entry::Item {
            item: Item::ToolResult { call_id, content },
        } if content.starts_with("error:") || content.starts_with("checks failed") => {
            let tool = names.get(call_id).map_or("tool", String::as_str);
            out.push(format!("  ! {tool}: {}", clip(&shown(content))));
        }
        Entry::Shown { call_id, class } if *class != ViewClass::Raw => {
            let tool = names.get(call_id).map_or("tool", String::as_str);
            out.push(format!(
                "  ◦ withheld from the frontier: {tool} result {}",
                held(*class)
            ));
        }
        Entry::Usage { interventions, .. } => {
            for i in interventions {
                out.push(format!("  ◦ withheld from the frontier: {}", clip(i)));
            }
        }
        Entry::Masked { items, .. } => {
            out.push(format!("  ◦ context: {items} old tool result(s) shortened"))
        }
        Entry::Compacted {
            head,
            upto,
            tokens_before,
            tokens_after,
            ..
        } => out.push(format!(
            "  ◦ context: {} earlier item(s) condensed by the local model (~{tokens_before} → ~{tokens_after} tokens)",
            upto - head
        )),
        Entry::Interrupted { tool, .. } => out.push(format!("  ■ {tool} stopped")),
        Entry::SubagentStart {
            child, mode, task, ..
        } => out.push(format!(
            "  ⇢ sub-agent {child} ({mode}): {}",
            clip(&shown(task))
        )),
        // A sub-agent's own steps, under its id.
        Entry::Subagent { child, entry } => out.extend(
            progress(entry, names, shown)
                .into_iter()
                .map(|l| format!("  [{child}]{l}")),
        ),
        Entry::SubagentEnd {
            child,
            terminal,
            cost_usd,
            written,
            ..
        } => out.push(format!(
            "  ⇠ sub-agent {child} {} (${cost_usd:.4}, {} file(s) written)",
            terminal.state(),
            written.len()
        )),
        Entry::Steered {
            after_request,
            messages,
            ..
        } => {
            for m in messages {
                out.push(format!(
                    "  ▸ delivered to duet after request {after_request}: {}",
                    clip(&shown(m))
                ));
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_boundary::model::ToolCall;
    use serde_json::json;

    #[test]
    fn commands_and_messages() {
        assert_eq!(parse("  "), Command::Empty);
        assert_eq!(parse("/diff"), Command::Diff);
        assert_eq!(parse(" /quit "), Command::Quit);
        assert_eq!(parse("/close"), Command::Close);
        assert_eq!(parse("/nope"), Command::Unknown("nope".into()));
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
    fn progress_shows_a_sub_agents_steps_under_its_id() {
        let mut names = HashMap::new();
        let shown = |t: &str| t.to_owned();
        let Value::Object(arguments) = json!({"path": "src/export/mod.rs"}) else {
            unreachable!()
        };
        let start = Entry::SubagentStart {
            child: "a1".into(),
            call_id: "c1".into(),
            mode: "read".into(),
            task: "Find the export.".into(),
            paths: vec![],
            journal_next: 1,
        };
        let step = Entry::Subagent {
            child: "a1".into(),
            entry: Box::new(Entry::Item {
                item: Item::Assistant {
                    text: String::new(),
                    reasoning: None,
                    tool_calls: vec![ToolCall {
                        id: "c2".into(),
                        name: "read_file".into(),
                        raw_arguments: String::new(),
                        arguments,
                    }],
                    replay: None,
                },
            }),
        };
        let end = Entry::SubagentEnd {
            child: "a1".into(),
            call_id: "c1".into(),
            terminal: duet_agent::Terminal::Completed {
                summary: "found".into(),
            },
            requests: 2,
            cost_usd: 0.002,
            seconds: 1.0,
            written: vec![],
            journal_end: 1,
        };
        let lines: Vec<String> = [start, step, end]
            .iter()
            .flat_map(|e| progress(e, &mut names, &shown))
            .collect();
        assert_eq!(
            lines,
            [
                "  ⇢ sub-agent a1 (read): Find the export.",
                "  [a1]  · read_file src/export/mod.rs",
                "  ⇠ sub-agent a1 completed ($0.0020, 0 file(s) written)",
            ]
        );
    }

    #[test]
    fn progress_shows_calls_errors_and_withheld_results_locally() {
        let mut names = HashMap::new();
        let shown = |t: &str| t.replace("⟨EMAIL#1⟩", "ana@example.org");
        let Value::Object(arguments) = json!({"path": "notes/⟨EMAIL#1⟩.txt"}) else {
            unreachable!()
        };
        let call = ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            raw_arguments: String::new(),
            arguments,
        };
        let lines = progress(
            &Entry::Item {
                item: Item::Assistant {
                    text: "Looking at the notes.".into(),
                    reasoning: None,
                    tool_calls: vec![call],
                    replay: None,
                },
            },
            &mut names,
            &shown,
        );
        assert_eq!(
            lines,
            vec![
                "  ~ Looking at the notes.".to_owned(),
                "  · read_file notes/ana@example.org.txt".to_owned()
            ]
        );
        let lines = progress(
            &Entry::Shown {
                call_id: "c1".into(),
                class: ViewClass::HandleSummary,
            },
            &mut names,
            &shown,
        );
        assert_eq!(
            lines,
            vec![
                "  ◦ withheld from the frontier: read_file result sensitive, held locally (a summary was sent)"
            ]
        );
        let lines = progress(
            &Entry::Item {
                item: Item::ToolResult {
                    call_id: "c1".into(),
                    content: "error: no such file".into(),
                },
            },
            &mut names,
            &shown,
        );
        assert_eq!(lines, vec!["  ! read_file: error: no such file"]);
    }

    use serde_json::Value;
}
