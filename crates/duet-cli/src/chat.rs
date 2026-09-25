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
//! Standard input is read by one thread into an inbox, so messages typed
//! while duet works wait their turn, and approval questions
//! (`oversight.approve`) are answered with the next line typed after they
//! are asked. When standard input is not a terminal (the TUI drives a
//! session through a pipe) nothing is prompted and the output is the same.
//!
//! Ctrl-C while duet works stops the turn (a running command is killed);
//! the session stays open. At the prompt, Ctrl-C twice leaves the session,
//! like `/quit` or the end of input; `duet chat --resume` continues it.
//! `/close` ends it for good.

use crate::approve;
use crate::{
    LocalOverride, Mode, Prepared, RunLimits, RunManifest, audit_log_path, checked_run_id,
    frontier_dialect, load_config, new_run_id, open_audit, prepare, setup,
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

    /// Whether the input ended and every line was taken.
    fn drained(&self) -> bool {
        let q = self.lock();
        q.closed && q.lines.is_empty()
    }

    /// The sequence number the next line will get.
    fn mark(&self) -> u64 {
        self.lock().next
    }

    /// Waits for the first line typed at or after `mark` and takes it (lines
    /// typed before stay queued as messages). `None` when `stop` is raised or
    /// the input ends first.
    fn answer_after(&self, mark: u64, stop: &AtomicBool) -> Option<String> {
        let mut q = self.lock();
        loop {
            if let Some(i) = q.lines.iter().position(|(n, _)| *n >= mark) {
                return q.lines.remove(i).map(|(_, l)| l);
            }
            if q.closed || stop.load(Ordering::SeqCst) {
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
}

impl AskInline {
    fn ask(&self, action: &Action) -> bool {
        let mark = self.inbox.mark();
        let mut out = std::io::stdout().lock();
        let _ = write!(
            out,
            "{}approve? answer y or n: ",
            approve::describe(self.mode, action)
        );
        let _ = out.flush();
        drop(out);
        let answer = self.inbox.answer_after(mark, &self.interrupted);
        let yes = answer.as_deref().is_some_and(approve::is_yes);
        println!("{}", if yes { "approved" } else { "denied" });
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
    Status,
    Diff,
    Undo,
    Help,
    /// Leave the session open (resumable).
    Quit,
    /// End the session for good.
    Close,
    Unknown(String),
    Empty,
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
        return Command::Message(line.trim_end().to_owned());
    };
    match word {
        "status" => Command::Status,
        "diff" => Command::Diff,
        "undo" => Command::Undo,
        "help" | "?" => Command::Help,
        "quit" | "exit" => Command::Quit,
        "close" => Command::Close,
        other => Command::Unknown(other.to_owned()),
    }
}

const HELP: &str = "\
Type a message and press Enter; end a line with \\ to continue on the next.
Commands (never sent to the model):
  /status  turns, tokens, cost and time against the budgets
  /diff    what changed in the workspace (sensitive files are only named)
  /undo    revert the file writes of the last turn (repeat for earlier turns)
  /quit    leave; the session stays open for `duet chat --resume`
  /close   end the session for good
  //text   send a message that starts with /
Ctrl-C stops duet's current turn; at the prompt, Ctrl-C twice leaves.";

fn prompt(tty: bool) {
    if tty {
        print!("you> ");
        let _ = std::io::stdout().flush();
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

/// Waits for the first message of a new session; `None` when the operator
/// leaves first.
async fn first_message(inbox: &Inbox, leave: &AtomicBool, tty: bool) -> Option<String> {
    if tty {
        println!("duet chat: a new session starts with your first message. /help lists commands.");
    }
    prompt(tty);
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
            Command::Message(m) => return Some(m),
            Command::Quit | Command::Close => return None,
            Command::Help => println!("{HELP}"),
            Command::Empty => {}
            _ => println!("nothing to show yet: the session starts with your first message"),
        }
        prompt(tty);
    }
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
    let inbox = Inbox::stdin();
    let interrupted = Arc::new(AtomicBool::new(false));
    // Refused before anything else when approval is on and nobody can answer.
    let oversight = approve::session_oversight(&cfg, |mode| {
        Arc::new(AskInline {
            mode,
            inbox: inbox.clone(),
            interrupted: interrupted.clone(),
        })
    })?;
    let working = Arc::new(AtomicBool::new(false));
    let leave = Arc::new(AtomicBool::new(false));
    watch_interrupts(working.clone(), interrupted.clone(), leave.clone());

    let (manifest, resume) = match args.resume {
        Some(id) => {
            ensure!(
                args.message.is_none(),
                "--resume continues a session; send the message once it is open"
            );
            (resumed(&ws, id)?, true)
        }
        None => {
            let first = match args.message {
                Some(m) => m,
                None => match first_message(&inbox, &leave, tty).await {
                    Some(m) => m,
                    None => return Ok(0),
                },
            };
            let local = match args.mode {
                Mode::Hybrid | Mode::LocalOnly => match setup::bootstrap(&cfg).await {
                    Ok(found) => found.map(|b| LocalOverride {
                        base_url: b.base_url,
                        model: b.model,
                    }),
                    Err(code) => return Ok(code),
                },
                Mode::Passthrough => None,
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
}

/// Ctrl-C during a turn interrupts it; at the prompt, twice within two
/// seconds leaves the session.
fn watch_interrupts(
    working: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    leave: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut idle_press: Option<std::time::Instant> = None;
        while tokio::signal::ctrl_c().await.is_ok() {
            if working.load(Ordering::SeqCst) {
                interrupted.store(true, Ordering::SeqCst);
                eprintln!("\ninterrupt: stopping this turn; the session stays open");
            } else if idle_press.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
                leave.store(true, Ordering::SeqCst);
            } else {
                idle_press = Some(std::time::Instant::now());
                eprintln!(
                    "\nCtrl-C again leaves the session (it stays open to resume); /close ends it"
                );
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
    } = prepare(ws, manifest, cfg, oversight, run_dir, limits, audit)?;
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
    if resume {
        recap(&session, &manifest.run_id);
    } else {
        take_turn(&mut session, &manifest.objective, run_dir, io, &shown).await;
    }
    let closed = loop {
        if io.leave.load(Ordering::SeqCst) {
            break false;
        }
        if let Some(which) = session.spent() {
            println!("the session budget {which} is spent; no further turns can start");
            break false;
        }
        prompt(io.tty);
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
            if io.tty {
                println!();
            }
            break false;
        };
        match parse(&line) {
            Command::Empty => {}
            Command::Quit => break false,
            Command::Close => break true,
            Command::Help => println!("{HELP}"),
            Command::Status => println!("{}", status(&session, &manifest.run_id, cfg)),
            Command::Diff => print!("{}", local_diff(&git, ws, presenter)),
            Command::Undo => match session.undo() {
                Ok((turn, paths)) if paths.is_empty() => {
                    println!("turn {turn} wrote no files; nothing to revert")
                }
                Ok((turn, paths)) => {
                    println!("reverted the writes of turn {turn} and later:");
                    for p in paths {
                        println!("  {}", p.display());
                    }
                    println!("duet is told with your next message.");
                }
                Err(e) => println!("undo: {e}"),
            },
            Command::Unknown(c) => println!("unknown command /{c}; /help lists the commands"),
            Command::Message(m) => {
                if !io.tty {
                    // Piped input is not on screen: the output keeps the conversation.
                    println!("you> {}", m.replace('\n', "\n     "));
                }
                take_turn(&mut session, &m, run_dir, io, &shown).await
            }
        }
    };
    let (terminal, mut stats) = session.end(closed);
    stats.ledger.local = engine.as_ref().and_then(|e| e.take_local_stats());
    Ok((terminal, stats))
}

/// Runs one turn, following its progress in the transcript as it is written.
async fn take_turn(
    session: &mut Session<'_>,
    message: &str,
    run_dir: &Path,
    io: &Io,
    shown: &Arc<dyn Fn(&str) -> String + Send + Sync>,
) {
    let path = run_dir.join("transcript.jsonl");
    let from = std::fs::metadata(&path).map_or(0, |m| m.len());
    let done = Arc::new(AtomicBool::new(false));
    let follower = tokio::spawn(follow(path, from, done.clone(), shown.clone()));
    io.working.store(true, Ordering::SeqCst);
    let end = session.turn(message).await;
    io.working.store(false, Ordering::SeqCst);
    done.store(true, Ordering::SeqCst);
    let _ = follower.await;
    println!("{}", describe_end(&end));
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
    }
}

/// The last few turns of a resumed session.
fn recap(session: &Session<'_>, id: &str) {
    let history: &[Exchange] = session.history();
    println!(
        "session {id} resumed: {} turn(s) so far, ${:.4}",
        session.turns(),
        session.stats().cost_usd
    );
    let skip = history.len().saturating_sub(5);
    if skip > 0 {
        println!("  ({skip} earlier turn(s) not shown)");
    }
    for x in &history[skip..] {
        println!("you> {}", x.message.trim().replace('\n', "\n     "));
        match &x.end {
            Some(end) => println!("{}", describe_end(end)),
            None => println!("(this turn did not end; it counts as interrupted)"),
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
/// operator only. Sensitive files are named, never shown.
fn local_diff(git: &duet_git::Git, ws: &Path, presenter: &dyn Presenter) -> String {
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

/// Prints the progress of a turn from the transcript, starting at byte
/// `from`, until `done` is raised (then reads what is left).
async fn follow(
    path: PathBuf,
    mut from: u64,
    done: Arc<AtomicBool>,
    shown: Arc<dyn Fn(&str) -> String + Send + Sync>,
) {
    let mut names: HashMap<String, String> = HashMap::new();
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
                for l in progress(&entry, &mut names, shown.as_ref()) {
                    println!("{l}");
                }
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
        Entry::Interrupted { tool, .. } => out.push(format!("  ■ {tool} stopped")),
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
    }

    #[test]
    fn a_trailing_backslash_continues_the_message() {
        let inbox = Inbox::default();
        inbox.read_from(std::io::Cursor::new("one\\\ntwo\nthree\n"));
        assert_eq!(inbox.pop().as_deref(), Some("one\ntwo"));
        assert_eq!(inbox.pop().as_deref(), Some("three"));
        assert!(inbox.drained());
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
