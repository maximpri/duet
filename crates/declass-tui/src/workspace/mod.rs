// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace: `declass` on a terminal, full screen.
//!
//! A header (the workspace, the mode and models, where sensitive values go),
//! the conversation as cells (the operator's messages, declass's replies
//! streamed as Markdown, each tool call with its folded result, edits with
//! their diffs, what the boundary withheld, how each turn ended), the side
//! rail (Changes, Privacy, Session), the input, and a status line with a
//! badge. The palette, the `@` file picker, the approval dialog, help and the
//! settings open over it. The session itself runs in the CLI
//! (`declass-cli`'s chat loop); it talks to the workspace through this handle,
//! and hears from it through [`Hooks`]: the lines the operator sends, Ctrl-C,
//! the end of input.
//!
//! One thread owns the screen and the keyboard. The input is editable at all
//! times: typing while declass works steers it. Output from any other code (the
//! process's standard output and error) appears in the conversation, dimmed;
//! see [`terminal`]. Text from the frontier, tools and files reaches the
//! screen only through `term::safe` (the feed applies it), and the lines are
//! drawn through [`ansi`], which draws no escape sequence of its own.

mod ansi;
mod cells;
mod interaction;
mod panel;
mod plan;
pub use plan::{PlanAction, PlanIdentity, PlanQuestion, PlanSnapshot, PlanStepProgress};
mod privacy;
mod terminal;
mod view;

use crate::term::editor::{Editor, Outcome};
use crate::term::feed::{Feed, Restore, Streamed};
use crate::term::{colour, safe};
use declass_agent::TurnEnd;
use declass_agent::transcript::Entry;
use declass_boundary::live::{StreamEvent, StreamTap};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::fs::File;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the session is doing, for the input box and the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Setting up (before the first turn, or while the session opens).
    Hidden,
    /// Waiting for the operator's next message.
    Prompt,
    /// A turn runs: a message steers it.
    Working,
}

/// The session as the status bar shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// The session permits only planning tools.
    pub planning: bool,
    pub session: String,
    /// A short goal status supplied by the session (state, turns and task).
    pub goal: Option<String>,
    /// A goal is in progress (working, or waiting for the operator's answer):
    /// the session is in goal mode. A paused or finished goal is not.
    pub goal_running: bool,
    /// `hybrid`, `top clearance` or `passthrough`.
    pub mode: String,
    pub frontier: String,
    /// The local model, when the session has one.
    pub local: Option<String>,
    pub turns: u32,
    pub cost_usd: f64,
    pub budget_usd: f64,
    /// Frontier requests and tool calls so far.
    pub requests: u64,
    pub tool_calls: u64,
    /// Tokens sent (of them cached) and received.
    pub tokens_in: u64,
    pub tokens_cached: u64,
    pub tokens_out: u64,
    /// The most one turn may cost.
    pub turn_budget_usd: f64,
    /// Working time so far, and the session's budget in minutes.
    pub worked_secs: u64,
    pub budget_minutes: u64,
}

/// The settings overlay's sources: where the configuration lives (and the
/// embedding program's policy layer) and what the CLI does for the screens.
pub struct Settings {
    pub paths: crate::Paths,
    pub services: crate::Services,
}

/// Commands the workspace answers itself (never sent to the session).
pub const COMMANDS: &[&str] = &[
    "/copy",
    "/find",
    "/paste",
    "/audit",
    "/data",
    "/ip",
    "/limits",
    "/models",
    "/runs",
    "/sensitivity",
    "/settings",
];

/// What the workspace reports to the session.
pub struct Hooks {
    /// A revision-bound operator action; never interpreted as chat text.
    pub plan_action: Box<dyn Fn(PlanAction) + Send>,
    /// An explicitly pasted image, queued for the next message after validation.
    pub paste_image: Arc<dyn Fn(Vec<u8>) -> Result<String, String> + Send + Sync>,
    /// A line the operator sent (continuation lines joined).
    pub line: Box<dyn Fn(String) + Send>,
    /// Ctrl-D on an empty input.
    pub eof: Box<dyn Fn() + Send>,
    /// Ctrl-C: what to tell the operator, if anything.
    pub interrupt: Box<dyn Fn() -> Option<String> + Send>,
    /// Whether an approval question waits for the next line.
    pub answering: Box<dyn Fn() -> bool + Send>,
}

/// An attachment waiting for the next message. IDs are understood by `/detach`.
#[derive(Debug, Clone)]
pub struct AttachmentChip {
    pub id: String,
    pub label: String,
}

enum Msg {
    Plan(Option<Box<PlanSnapshot>>),
    OpenPlan(bool),
    PlanError(String),
    PlanSaved(Box<PlanSnapshot>),
    PlanQuestion(Option<PlanQuestion>),
    Attachments(Vec<AttachmentChip>),
    RecoverDraft(String),
    Lines(Vec<String>),
    Text(String),
    Diff(String),
    Entry(Box<Entry>),
    Stream(Streamed),
    Begin,
    End(TurnEnd),
    Mode(Mode),
    Status(Status),
    Planning(bool),
    /// The session's workspace, run directory, audit log and policy, for
    /// the side panel.
    Attach {
        ws: PathBuf,
        run_dir: PathBuf,
        audit: PathBuf,
        policy: Box<declass_boundary::policy::Policy>,
        history: Vec<Entry>,
    },
    /// A line another part of the process printed.
    Output(String),
    Remember(Vec<String>),
    Restore(Restore),
    /// Draws what is pending, then acknowledges.
    Sync(mpsc::Sender<()>),
    Stop(mpsc::Sender<()>),
}

/// The handle the session holds.
pub struct Workspace {
    tx: Sender<Msg>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Workspace {
    pub fn plan(&self, snapshot: Option<PlanSnapshot>) {
        self.send(Msg::Plan(snapshot.map(Box::new)));
    }
    pub fn plan_error(&self, error: String) {
        self.send(Msg::PlanError(error));
    }
    pub fn plan_saved(&self, snapshot: PlanSnapshot) {
        self.send(Msg::PlanSaved(Box::new(snapshot)));
    }
    pub fn open_plan(&self, edit: bool) {
        self.send(Msg::OpenPlan(edit));
    }
    pub fn plan_question(&self, question: Option<PlanQuestion>) {
        self.send(Msg::PlanQuestion(question));
    }
    /// Restore an unsent message after an attachment failed, preserving new input.
    pub fn recover_draft(&self, text: String) {
        self.send(Msg::RecoverDraft(text));
    }

    pub fn attachments(&self, attachments: Vec<AttachmentChip>) {
        self.send(Msg::Attachments(attachments));
    }
    /// Takes over the terminal. `commands` are completed after `/`.
    pub fn start(
        hooks: Hooks,
        commands: &'static [&'static str],
        settings: Option<Settings>,
    ) -> std::io::Result<Arc<Self>> {
        let mut tty = terminal::tty()?;
        let draw_to = tty.try_clone()?;
        terminal::enter(&mut tty).inspect_err(|_| terminal::give_back())?;
        let mut screen = match Terminal::new(CrosstermBackend::new(draw_to)) {
            Ok(t) => t,
            Err(e) => {
                terminal::give_back();
                return Err(e);
            }
        };
        let _ = screen.clear();
        let (tx, rx) = mpsc::channel();
        let captured = tx.clone();
        let capture = terminal::Capture::start(move |line| {
            if let Err(mpsc::SendError(Msg::Output(line))) = captured.send(Msg::Output(line)) {
                // The workspace is gone: the line goes where it was meant to.
                eprintln!("{line}");
            }
        })
        .inspect_err(|_| terminal::give_back())?;
        let colour = colour(true);
        let mut state = State::new(colour, hooks, commands);
        state.ws = settings.as_ref().map(|s| s.paths.workspace.clone());
        state.settings = settings;
        let thread = std::thread::Builder::new()
            .name("declass-workspace".into())
            .spawn(move || {
                let mut state = state;
                let run =
                    std::panic::AssertUnwindSafe(|| run(&mut state, &mut screen, &mut tty, &rx));
                let panicked = std::panic::catch_unwind(run).is_err();
                terminal::give_back();
                drop(capture);
                if panicked {
                    // The session is left open (the input ended): no work is lost.
                    (state.hooks.eof)();
                    while let Ok(m) = rx.recv() {
                        match m {
                            Msg::Stop(ack) => {
                                let _ = ack.send(());
                                return;
                            }
                            Msg::Sync(ack) => {
                                let _ = ack.send(());
                            }
                            _ => {}
                        }
                    }
                }
            })?;
        Ok(Arc::new(Workspace {
            tx,
            thread: Mutex::new(Some(thread)),
        }))
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
    }

    /// Lines of declass's own (tinted by their marks; made safe).
    pub fn lines(&self, text: &str) {
        self.send(Msg::Lines(text.lines().map(str::to_owned).collect()));
    }

    /// Text shown as is (made safe).
    pub fn text(&self, text: &str) {
        self.send(Msg::Text(text.to_owned()));
    }

    /// A diff, coloured by line.
    pub fn diff(&self, text: &str) {
        self.send(Msg::Diff(text.to_owned()));
    }

    pub fn entry(&self, entry: Entry) {
        self.send(Msg::Entry(Box::new(entry)));
    }

    pub fn begin(&self) {
        self.send(Msg::Begin);
    }

    pub fn end(&self, end: &TurnEnd) {
        self.send(Msg::End(end.clone()));
    }

    pub fn mode(&self, mode: Mode) {
        self.send(Msg::Mode(mode));
    }

    pub fn planning(&self, enabled: bool) {
        self.send(Msg::Planning(enabled));
    }

    pub fn status(&self, status: Status) {
        self.send(Msg::Status(status));
    }

    /// The session is open: the side panel follows its run directory (the
    /// files it changes) under `policy` (which files are held locally).
    pub fn attach(
        &self,
        ws: PathBuf,
        run_dir: PathBuf,
        audit: PathBuf,
        policy: declass_boundary::policy::Policy,
    ) {
        // Snapshot while the session is idle, before it starts publishing live
        // entries. Reading on the UI thread can race with queued live messages
        // and count the same usage/event twice.
        let history = crate::runs::read_transcript(&run_dir).unwrap_or_default();
        self.send(Msg::Attach {
            ws,
            run_dir,
            audit,
            policy: Box::new(policy),
            history,
        });
    }

    /// The operator's earlier messages, for the input's history.
    pub fn remember(&self, entries: Vec<String>) {
        self.send(Msg::Remember(entries));
    }

    /// How placeholders are restored for the operator (once the session's
    /// boundary is open).
    pub fn restore(&self, restore: Restore) {
        self.send(Msg::Restore(restore));
    }

    /// A tap that shows the frontier's responses as they stream.
    pub fn tap(self: &Arc<Self>) -> Arc<dyn StreamTap> {
        Arc::new(Tap(self.tx.clone()))
    }

    /// Draws everything sent so far.
    pub fn sync(&self) {
        let (ack, done) = mpsc::channel();
        if self.tx.send(Msg::Sync(ack)).is_ok() {
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
    }

    /// Gives the terminal back as it was (idempotent); standard output and
    /// error are the process's own again.
    pub fn stop(&self) {
        let (ack, done) = mpsc::channel();
        if self.tx.send(Msg::Stop(ack)).is_ok() {
            let _ = done.recv_timeout(Duration::from_secs(5));
        }
        if let Some(t) = self.thread.lock().ok().and_then(|mut t| t.take()) {
            let _ = t.join();
        }
        terminal::give_back();
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Tap(Sender<Msg>);

impl StreamTap for Tap {
    fn event(&self, event: StreamEvent<'_>) {
        let _ = self.0.send(Msg::Stream(event.into()));
    }
}

/// How often the screen moves while declass works (the spinner, the clock).
const TICK: Duration = Duration::from_millis(120);

/// Commands with what they do, for the palette (the session's and the
/// workspace's own).
const DESCRIBED: &[(&str, &str)] = &[
    ("/help", "keys and commands"),
    (
        "/plan",
        "plan with read-only tools; /plan off leaves without executing",
    ),
    ("/goal", "goal status; Tab chooses start, pause or resume"),
    ("/history", "find earlier sessions and how to resume them"),
    (
        "/status",
        "turns, tokens, cost and time against the budgets",
    ),
    ("/models", "frontier and local models; the doctor"),
    ("/settings", "the settings screens"),
    ("/copy", "copy the latest reply"),
    ("/paste", "paste text or an image from the clipboard"),
    ("/find", "search this conversation"),
    ("/attachments", "show files queued for your next message"),
    ("/detach", "remove a queued attachment: ID or all"),
    ("/skills", "discover reusable workflows"),
    ("/skill", "use a skill by name"),
    ("/plugins", "installed extension packages"),
    ("/command", "use a plugin command"),
    ("/diff", "what changed, against the last commit"),
    ("/undo", "revert the file writes of the last turn"),
    ("/stop", "end declass's turn after its current step"),
    ("/image", "attach an image to your next message"),
    ("/attach", "attach a text file to your next message"),
    (
        "/mode",
        "this session's mode; top-clearance continues with the local model only",
    ),
    ("/sensitivity", "what is sensitive; test a path or text"),
    ("/ip", "interface-only and sealed code"),
    ("/limits", "budgets and time limits"),
    ("/data", "retention; purge old runs"),
    ("/audit", "what was sent; verify the chain"),
    ("/runs", "browse recorded activity and changed files"),
    ("/quit", "leave; the session stays open"),
    ("/close", "end the session for good"),
];

/// Commands that take an argument: the palette puts them in the input.
const WITH_ARGUMENT: &[&str] = &[
    "/detach",
    "/image",
    "/attach",
    "/mode",
    "/goal start",
    "/skill",
    "/command",
];

const PLAN_COMMANDS: &[(&str, &str)] = &[
    (
        "/plan on",
        "enter read-only planning; pause automatic goals",
    ),
    (
        "/plan off",
        "leave planning; send an implementation request when ready",
    ),
    ("/plan status", "show whether planning is enabled"),
];

const GOAL_COMMANDS: &[(&str, &str)] = &[
    (
        "/goal start",
        "describe a goal; Declass continues up to 20 turns by default",
    ),
    ("/goal pause", "pause automatic work on the goal"),
    (
        "/goal resume",
        "continue the paused goal within its remaining limits",
    ),
    ("/goal cancel", "end the goal"),
    ("/goal status", "show progress and remaining goal turns"),
];

/// The `@` picker's matches shown at most.
const PICKS: usize = 8;

/// Last drawn palette geometry. A query change invalidates mouse hit testing.
struct PalettePopup {
    area: Rect,
    rows: Rect,
    first: usize,
    query: String,
}

/// The few rows outside the cached cells, plus the full conversation's height.
/// The viewport and clipboard request only the range they need.
struct TranscriptLayout {
    leading: Vec<Line<'static>>,
    trailing: Vec<Line<'static>>,
    total: usize,
}

impl TranscriptLayout {
    fn rows(&self, cells: &cells::Cells, range: Range<usize>) -> Vec<Line<'static>> {
        let chunks = std::iter::once(self.leading.as_slice())
            .chain(cells.row_chunks())
            .chain(std::iter::once(self.trailing.as_slice()));
        let mut offset = 0;
        let mut rows = Vec::new();
        for chunk in chunks {
            if offset >= range.end {
                break;
            }
            let start = range.start.saturating_sub(offset).min(chunk.len());
            let end = range.end.saturating_sub(offset).min(chunk.len());
            if start < end {
                rows.extend_from_slice(&chunk[start..end]);
            }
            offset += chunk.len();
        }
        rows
    }
}

/// The workspace thread's state.
pub(crate) struct State {
    colour: bool,
    /// Kept for the working status (time, cost, what runs now).
    feed: Feed,
    cells: cells::Cells,
    editor: Editor,
    mode: Mode,
    answering: bool,
    /// The last text shown: an approval question while one waits.
    question: Option<String>,
    /// The approval dialog's choice: false denies (the default).
    approve: bool,
    status: Status,
    /// How the last turn ended, for the status badge.
    ended: Option<TurnEnd>,
    hooks: Hooks,
    commands: &'static [&'static str],
    /// The workspace directory (for the header and the `@` picker).
    ws: Option<PathBuf>,
    /// The conversation's width, rows at the last draw, and scrolling (rows
    /// up from the bottom; 0 follows new output).
    width: usize,
    total: usize,
    scroll: usize,
    /// Rows the conversation shows (set when drawn), for paging.
    page: usize,
    /// Tool results unfolded (Ctrl-O).
    details: bool,
    help: bool,
    help_scroll: u16,
    panel: panel::Panel,
    plan: plan::Panel,
    /// Completions shown above the input until the next key.
    choices: Vec<String>,
    /// The palette's selected command; hidden after Esc until the input changes.
    palette: usize,
    palette_start: usize,
    palette_popup: Option<PalettePopup>,
    palette_off: Option<String>,
    /// The `@` picker: the workspace's files (read once), the selection.
    files: Option<Vec<String>>,
    pick: usize,
    /// A mouse selection in the conversation, (row, column) from its first
    /// row, while dragging and after (until the next key or click).
    sel: Option<((usize, usize), (usize, usize))>,
    selecting: bool,
    /// Where the conversation's text was drawn, and its first row there.
    view: Rect,
    view_top: usize,
    /// Temporary feedback for explicit clipboard actions.
    toast: Option<(Instant, String)>,
    clipboard: Option<interaction::ClipboardJob>,
    input_epoch: u64,
    composer: Rect,
    editor_selecting: bool,
    attachments: Vec<AttachmentChip>,
    find: interaction::Find,
    palette_draft: Option<Editor>,
    /// The settings overlay: its sources until first opened, then the app.
    settings: Option<Settings>,
    app: Option<crate::app::App>,
    overlay: bool,
    ticked: Instant,
    started: Instant,
    dirty: bool,
    /// The screen must be drawn whole (after Ctrl-Alt-Z).
    redraw: bool,
    drawn_at: Instant,
}

impl State {
    fn new(colour: bool, hooks: Hooks, commands: &'static [&'static str]) -> Self {
        State {
            colour,
            feed: Feed::new(colour, 80, true, Arc::new(|t: &str| t.to_owned())),
            cells: cells::Cells::new(colour),
            editor: Editor::default(),
            mode: Mode::Hidden,
            answering: false,
            question: None,
            approve: false,
            status: Status::default(),
            ended: None,
            hooks,
            commands,
            ws: None,
            width: 80,
            total: 0,
            scroll: 0,
            page: 20,
            details: false,
            help: false,
            help_scroll: 0,
            panel: panel::Panel::default(),
            plan: plan::Panel::default(),
            choices: Vec::new(),
            palette: 0,
            palette_start: 0,
            palette_popup: None,
            palette_off: None,
            files: None,
            pick: 0,
            sel: None,
            selecting: false,
            view: Rect::default(),
            view_top: 0,
            toast: None,
            clipboard: None,
            input_epoch: 0,
            composer: Rect::default(),
            editor_selecting: false,
            attachments: Vec::new(),
            find: interaction::Find::default(),
            palette_draft: None,
            settings: None,
            app: None,
            overlay: false,
            ticked: Instant::now(),
            started: Instant::now(),
            dirty: true,
            redraw: false,
            drawn_at: Instant::now(),
        }
    }

    /// The conversation is `width` columns wide now.
    fn set_width(&mut self, width: usize) {
        let width = width.max(10);
        if width != self.width {
            self.width = width;
            self.feed.resize(width);
        }
    }

    fn scroll_by(&mut self, up: isize) {
        let max = self.total.saturating_sub(self.page);
        self.scroll = if up >= 0 {
            (self.scroll + up.unsigned_abs()).min(max)
        } else {
            self.scroll.saturating_sub(up.unsigned_abs())
        };
        self.dirty = true;
    }

    fn refresh_answering(&mut self) {
        let answering = (self.hooks.answering)();
        if answering != self.answering {
            self.answering = answering;
            self.approve = false;
            self.input_epoch = self.input_epoch.wrapping_add(1);
            self.dirty = true;
        }
    }

    fn warn(&mut self, text: &str) {
        self.cells.lines(&format!("  ■ {text}"));
    }

    /// Opens the settings overlay, on `tab` when given.
    fn open_settings(&mut self, tab: Option<crate::app::Tab>) {
        if self.app.is_none() {
            let Some(Settings { paths, services }) = self.settings.take() else {
                self.warn("settings are not available here");
                return;
            };
            match crate::app::App::new(paths, services) {
                Ok(app) => self.app = Some(app),
                Err(e) => {
                    self.warn(&format!("settings: {}", safe(&format!("{e:#}"))));
                    return;
                }
            }
        }
        if let (Some(app), Some(tab)) = (self.app.as_mut(), tab) {
            app.enter_tab(tab);
        }
        self.overlay = true;
    }

    /// A key while the overlay is open: the screens take it; Esc (outside an
    /// edit or a dialog) and `q` come back to the conversation.
    fn overlay_key(&mut self, k: ratatui::crossterm::event::KeyEvent) {
        let Some(app) = self.app.as_mut() else {
            self.overlay = false;
            return;
        };
        let idle = matches!(app.mode, crate::app::Mode::Normal)
            && !(app.tab == crate::app::Tab::Audit && app.audit.focus_records);
        if k.code == KeyCode::Esc && idle {
            self.overlay = false;
            return;
        }
        app.key(k.code, k.modifiers);
        if app.quit {
            app.quit = false;
            self.overlay = false;
        }
    }

    /// A command the workspace answers itself; `false` sends it on.
    fn local_command(&mut self, text: &str) -> bool {
        use crate::app::Tab;
        let tab = match text.trim() {
            "/copy" => {
                self.copy_reply();
                return true;
            }
            "/paste" => {
                self.paste();
                return true;
            }
            "/find" => {
                self.open_find();
                return true;
            }
            "/settings" => None,
            "/models" => Some(Tab::Models),
            "/sensitivity" => Some(Tab::Sensitivity),
            "/ip" => Some(Tab::Ip),
            "/limits" => Some(Tab::Limits),
            "/data" => Some(Tab::Data),
            "/audit" => Some(Tab::Audit),
            "/runs" => Some(Tab::Runs),
            "/help" | "/?" => {
                self.help = true;
                self.help_scroll = 0;
                return true;
            }
            _ => return false,
        };
        self.open_settings(tab);
        true
    }

    /// The palette's commands for the input, when it is a command being typed.
    pub(crate) fn palette_items(&self) -> Vec<(&'static str, &'static str)> {
        let typed = self.editor.buffer();
        if self.answering || !typed.starts_with('/') || self.palette_off.as_deref() == Some(typed) {
            return Vec::new();
        }
        if typed.starts_with("/plan ") {
            return PLAN_COMMANDS
                .iter()
                .copied()
                .filter(|(command, _)| command.starts_with(typed))
                .collect();
        }
        if typed.starts_with("/goal ") {
            return GOAL_COMMANDS
                .iter()
                .copied()
                .filter(|(command, _)| command.starts_with(typed))
                .collect();
        }
        if typed.contains(char::is_whitespace) {
            return Vec::new();
        }
        let mut items: Vec<(&'static str, &'static str)> = DESCRIBED
            .iter()
            .copied()
            .filter(|(c, _)| c.starts_with(typed))
            .collect();
        for c in self.commands.iter().chain(COMMANDS) {
            if c.starts_with(typed) && !items.iter().any(|(d, _)| d == c) && *c != "/exit" {
                items.push((c, ""));
            }
        }
        items
    }

    /// The `@` word being typed at the end of the input, if any.
    fn at_word(&self) -> Option<&str> {
        if self.editor.cursor() != self.editor.buffer().len() {
            return None;
        }
        let typed = self.editor.buffer();
        let word = typed.rsplit(char::is_whitespace).next()?;
        (word.starts_with('@') && !typed.ends_with(char::is_whitespace)).then(|| &word[1..])
    }

    /// The workspace's files matching the `@` word, best first.
    pub(crate) fn picks(&mut self) -> Vec<String> {
        let Some(query) = self.at_word().map(str::to_lowercase) else {
            return Vec::new();
        };
        if self.files.is_none() {
            let ws = self
                .ws
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            self.files = Some(
                declass_git::Git::locate()
                    .ok()
                    .and_then(|g| g.list_files(&ws).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .take(50_000)
                    .collect(),
            );
        }
        let files = self.files.as_deref().unwrap_or_default();
        let mut scored: Vec<(i64, &String)> = files
            .iter()
            .filter_map(|f| fuzzy(&query, f).map(|score| (score, f)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
        scored
            .into_iter()
            .take(PICKS)
            .map(|(_, f)| safe(f))
            .collect()
    }

    /// Lay out the conversation at `width`; row text stays in the cell cache
    /// until the viewport or a clipboard selection asks for a range.
    fn transcript(&mut self, width: usize, now: Instant) -> TranscriptLayout {
        let mut leading = Vec::new();
        if self.cells.is_empty() && self.mode != Mode::Working {
            leading = view::welcome(self);
        }
        let cell_rows = self.cells.prepare_rows(width, self.details);
        let mut trailing = Vec::new();
        if self.mode == Mode::Working
            && let Some(text) = self.feed.status_text(now)
        {
            trailing.push(Line::default());
            trailing.extend(view::working_row(self, &text, width, now));
        }
        TranscriptLayout {
            total: leading.len() + cell_rows + trailing.len(),
            leading,
            trailing,
        }
    }

    /// The conversation's (row, column) under the mouse, if it is over it;
    /// `clamp` holds a drag outside it to its edge.
    fn at(&self, column: u16, row: u16, clamp: bool) -> Option<(usize, usize)> {
        let v = self.view;
        let inside = column >= v.x && column < v.right() && row >= v.y && row < v.bottom();
        if !inside && !clamp || v.width == 0 || v.height == 0 {
            return None;
        }
        let r = row.clamp(v.y, v.bottom() - 1) - v.y;
        let c = column.clamp(v.x, v.right()) - v.x;
        Some((self.view_top + r as usize, c as usize))
    }

    /// The selection's text, row by row.
    fn selected_text(&mut self) -> Option<String> {
        let (a, b) = self.sel?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let layout = self.transcript(self.width, Instant::now());
        let rows = layout.rows(&self.cells, a.0..b.0.saturating_add(1));
        let mut out = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let r = a.0 + i;
            let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
            let from = if r == a.0 { a.1 } else { 0 };
            let to = if r == b.0 { b.1 } else { usize::MAX };
            out.push(columns(&text, from, to).trim_end().to_owned());
        }
        let text = out.join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    fn message(&mut self, m: Msg) {
        match m {
            Msg::Plan(snapshot) => self.plan.snapshot(snapshot.map(|s| *s)),
            Msg::PlanError(error) => self.plan.error(error),
            Msg::PlanSaved(snapshot) => {
                self.input_epoch = self.input_epoch.wrapping_add(1);
                self.plan.saved(*snapshot);
            }
            Msg::OpenPlan(edit) => {
                self.input_epoch = self.input_epoch.wrapping_add(1);
                self.overlay = false;
                self.help = false;
                self.find.open = false;
                self.plan.show(edit);
            }
            Msg::PlanQuestion(question) => {
                let was_open = self.plan.question_active();
                let new_question = self.plan.question(question);
                if new_question || was_open != self.plan.question_active() {
                    // Cancel a clipboard read only when its target actually changes.
                    self.input_epoch = self.input_epoch.wrapping_add(1);
                }
                if new_question {
                    self.overlay = false;
                    self.help = false;
                    self.find.open = false;
                }
            }
            Msg::Attachments(chips) => self.attachments = chips,
            Msg::RecoverDraft(text) => {
                if self.editor.buffer().is_empty() {
                    self.editor.insert(&text);
                    self.input_epoch = self.input_epoch.wrapping_add(1);
                    self.notice("Message restored · fix the attachment before sending");
                } else {
                    self.cells
                        .text(&format!("Unsent message (attachment failed):\n{text}"));
                    self.notice("Unsent message saved in conversation · current draft preserved");
                }
            }
            Msg::Lines(lines) => self.cells.lines(&lines.join("\n")),
            Msg::Text(text) => {
                self.question = Some(safe(text.trim_end()));
                self.cells.text(&text);
            }
            Msg::Diff(text) => self.cells.diff(&text),
            Msg::Entry(e) => {
                self.panel.entry(&e);
                let _ = self.feed.entry(&e);
                self.cells.entry(&e);
            }
            Msg::Stream(ev) => {
                let _ = self.feed.stream(&ev);
                self.cells.stream(&ev);
            }
            Msg::Begin => {
                self.feed.begin(Instant::now());
                self.cells.begin();
                self.mode = Mode::Working;
                self.ended = None;
                self.started = Instant::now();
            }
            Msg::End(end) => {
                let _ = self.feed.end(&end);
                self.cells.end(&end);
                self.ended = Some(end);
                self.panel.refresh(true);
            }
            Msg::Mode(mode) => self.mode = mode,
            Msg::Status(s) => self.status = s,
            Msg::Planning(enabled) => self.status.planning = enabled,
            Msg::Attach {
                ws,
                run_dir,
                audit,
                policy,
                history,
            } => {
                let held_policy = (*policy).clone();
                self.cells.set_held(Arc::new(move |p: &str| {
                    held_policy.is_sensitive_path(std::path::Path::new(p))
                }));
                self.ws = Some(ws.clone());
                self.cells.set_run_dir(run_dir.clone());
                self.panel.attach(ws, run_dir, audit, *policy, &history);
            }
            Msg::Output(line) => self.cells.output(&line),
            Msg::Remember(entries) => self.editor.remember(entries),
            Msg::Restore(restore) => {
                self.feed.set_restore(restore.clone());
                self.cells.set_restore(restore);
            }
            Msg::Sync(_) | Msg::Stop(_) => {}
        }
        self.dirty = true;
    }

    fn event(&mut self, ev: Event, tty: &mut File) {
        self.dirty = true;
        // Permission questions take precedence over all review/edit overlays.
        if self.answering {
            let Event::Key(k) = ev else {
                return;
            };
            if k.kind == KeyEventKind::Release {
                return;
            }
            self.input_epoch = self.input_epoch.wrapping_add(1);
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            // The approval dialog: No is the default; Enter answers.
            match k.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    self.approve = !self.approve;
                }
                KeyCode::Char('y' | 'Y') if !ctrl => self.approve = true,
                KeyCode::Char('n' | 'N') if !ctrl => self.approve = false,
                KeyCode::Enter => {
                    let answer = if self.approve { "y" } else { "n" };
                    self.approve = false;
                    (self.hooks.line)(answer.to_owned());
                }
                KeyCode::Esc => {
                    self.approve = false;
                    (self.hooks.line)("n".to_owned());
                }
                _ if ctrl && matches!(k.code, KeyCode::Char('c' | 'C')) => {
                    let outcome = self.editor.key(k);
                    self.outcome(outcome, tty);
                }
                _ => {}
            }
            return;
        }
        if self.plan.active() {
            self.input_epoch = self.input_epoch.wrapping_add(1);
            match self.plan.event(ev) {
                plan::Effect::None => {}
                plan::Effect::Action(action) => (self.hooks.plan_action)(action),
                plan::Effect::Copy(text) => self.copy_text(text),
                plan::Effect::Paste => self.paste(),
                plan::Effect::Interrupt => {
                    if let Some(text) = (self.hooks.interrupt)() {
                        self.notice(&text);
                    }
                }
            }
            return;
        }
        if self.overlay {
            self.input_epoch = self.input_epoch.wrapping_add(1);
            match ev {
                Event::Key(k) if k.kind != KeyEventKind::Release => self.overlay_key(k),
                Event::Paste(text) => {
                    if let Some(app) = self.app.as_mut() {
                        app.paste(&text);
                    }
                }
                _ => {}
            }
            return;
        }
        let Event::Key(k) = ev else {
            match ev {
                Event::Mouse(m) => {
                    if matches!(m.kind, MouseEventKind::Down(_) | MouseEventKind::Drag(_)) {
                        self.input_epoch = self.input_epoch.wrapping_add(1);
                    }
                    if !self.palette_mouse(m) {
                        self.mouse(m);
                    }
                }
                Event::Resize(..) => self.palette_popup = None,
                Event::Paste(text) if !self.help && !self.answering => {
                    self.input_epoch = self.input_epoch.wrapping_add(1);
                    if self.find.open {
                        self.find_paste(&text);
                    } else {
                        self.insert_paste(&text);
                    }
                }
                _ => {}
            }
            return;
        };
        if k.kind == KeyEventKind::Release {
            return;
        }
        // Sending must wait for an explicitly pasted image to enter the queue.
        if !self.answering
            && !self.help
            && k.code == KeyCode::Enter
            && self.clipboard.as_ref().is_some_and(|j| j.receiving)
        {
            self.notice("Finishing paste… press Enter when the attachment is ready");
            return;
        }
        self.input_epoch = self.input_epoch.wrapping_add(1);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        if self.find.open {
            self.find_key(k);
            return;
        }
        if self.help {
            match k.code {
                KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('q') => self.help = false,
                KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(8),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(8),
                _ => {}
            }
            return;
        }
        if ctrl && matches!(k.code, KeyCode::Char('c' | 'C')) {
            if let Some(text) = self.selected_text() {
                self.copy_text(text);
                return;
            }
            if shift && self.editor.selected_text().is_none() {
                self.copy_reply();
                return;
            }
        }
        if ctrl && matches!(k.code, KeyCode::Char('f' | 'F')) {
            self.open_find();
            return;
        }
        if self.clipboard.is_some()
            && (ctrl && matches!(k.code, KeyCode::Char('x' | 'X'))
                || shift && k.code == KeyCode::Delete)
        {
            self.notice("Clipboard busy; selection kept, try cutting again in a moment");
            return;
        }
        if k.code == KeyCode::F(5) {
            self.plan.show(false);
            return;
        }
        if k.code == KeyCode::F(6) {
            self.plan.reopen_question();
            return;
        }
        if k.code == KeyCode::F(3) {
            self.next_find(shift);
            return;
        }
        if k.code == KeyCode::Esc
            && let Some(draft) = self.palette_draft.take()
        {
            self.editor = draft;
            self.palette_off = Some(self.editor.buffer().to_owned());
            return;
        }
        if k.code == KeyCode::F(4) || ctrl && shift && matches!(k.code, KeyCode::Char('p' | 'P')) {
            if self.palette_draft.is_none() {
                self.palette_draft = Some(std::mem::take(&mut self.editor));
            }
            self.editor.clear();
            self.editor.insert("/");
            self.palette_off = None;
            self.palette = 0;
            return;
        }
        if ctrl && matches!(k.code, KeyCode::Char('p' | 'P')) {
            self.editor
                .key(event::KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL));
            if !self.editor.buffer().is_empty()
                && !self.editor.buffer().ends_with(char::is_whitespace)
            {
                self.editor.insert(" ");
            }
            self.editor.insert("@");
            self.files = None;
            self.pick = 0;
            return;
        }
        self.sel = None;
        if k.code != KeyCode::Tab {
            self.choices.clear();
        }
        // The palette and the `@` picker take the arrows and Tab/Enter.
        let palette = self.palette_items();
        if !palette.is_empty() {
            let n = palette.len();
            self.palette = self.palette.min(n - 1);
            let plain = k.modifiers.is_empty();
            match k.code {
                KeyCode::Up if plain => {
                    self.palette = (self.palette + n - 1) % n;
                    return;
                }
                KeyCode::Down if plain => {
                    self.palette = (self.palette + 1) % n;
                    return;
                }
                KeyCode::PageUp | KeyCode::PageDown if plain => {
                    let step = self
                        .palette_popup
                        .as_ref()
                        .map_or(1, |p| p.rows.height as usize)
                        .max(1);
                    self.palette = if k.code == KeyCode::PageUp {
                        self.palette.saturating_sub(step)
                    } else {
                        (self.palette + step).min(n - 1)
                    };
                    return;
                }
                KeyCode::Home | KeyCode::End if plain => {
                    self.palette = if k.code == KeyCode::Home { 0 } else { n - 1 };
                    return;
                }
                KeyCode::Esc => {
                    self.palette_off = Some(self.editor.buffer().to_owned());
                    return;
                }
                KeyCode::Tab | KeyCode::Enter => {
                    let (command, _) = palette[self.palette.min(n - 1)];
                    self.palette = 0;
                    self.editor.clear();
                    if k.code == KeyCode::Tab || WITH_ARGUMENT.contains(&command) {
                        self.editor.insert(&format!("{command} "));
                    } else {
                        self.editor.insert(command);
                        let outcome = self.editor.key(k);
                        self.outcome(outcome, tty);
                    }
                    return;
                }
                _ => {}
            }
        }
        let picks = self.picks();
        if !picks.is_empty() {
            let n = picks.len();
            match k.code {
                KeyCode::Up => {
                    self.pick = (self.pick + n - 1) % n;
                    return;
                }
                KeyCode::Down => {
                    self.pick = (self.pick + 1) % n;
                    return;
                }
                KeyCode::Tab | KeyCode::Enter => {
                    let chosen = picks[self.pick.min(n - 1)].clone();
                    use unicode_segmentation::UnicodeSegmentation;
                    let typed = self.at_word().map_or(0, |w| w.graphemes(true).count());
                    for _ in 0..typed {
                        self.editor.key(ratatui::crossterm::event::KeyEvent::from(
                            KeyCode::Backspace,
                        ));
                    }
                    self.editor.insert(&format!("{chosen} "));
                    self.pick = 0;
                    return;
                }
                _ => {}
            }
        }
        match k.code {
            KeyCode::BackTab => self.panel.switch_tab(true),
            KeyCode::Tab if k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.panel.switch_tab(true)
            }
            KeyCode::Tab
                if !["/image ", "/attach "]
                    .iter()
                    .any(|p| self.editor.buffer().starts_with(p)) =>
            {
                self.panel.switch_tab(false)
            }
            KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::F(2) => self.open_settings(None),
            KeyCode::Char('o' | 'O') if ctrl => {
                if !self.panel.toggle_privacy_details() {
                    self.details = !self.details;
                }
            }
            KeyCode::Char('t' | 'T') if ctrl => self.panel.cycle(),
            KeyCode::Up if ctrl && !shift => self.panel.select(-1),
            KeyCode::Down if ctrl && !shift => self.panel.select(1),
            KeyCode::End if ctrl && !shift && self.editor.buffer().is_empty() => self.scroll = 0,
            KeyCode::PageUp if ctrl => self.panel.scroll_diff(-(self.page as isize)),
            KeyCode::PageDown if ctrl => self.panel.scroll_diff(self.page as isize),
            KeyCode::PageUp => self.scroll_by(self.page.saturating_sub(2).max(1) as isize),
            KeyCode::PageDown => self.scroll_by(-(self.page.saturating_sub(2).max(1) as isize)),
            _ => {
                let before = self.editor.buffer().to_owned();
                let outcome = self.editor.key(k);
                if self.editor.buffer() != before {
                    self.palette = 0;
                    self.pick = 0;
                }
                self.outcome(outcome, tty);
            }
        }
    }

    fn outcome(&mut self, outcome: Outcome, tty: &mut File) {
        match outcome {
            Outcome::Edited => {}
            Outcome::Copy(text) => self.copy_text(text),
            Outcome::Paste => self.paste(),
            Outcome::Submit(text) => {
                if let Some(draft) = self.palette_draft.take() {
                    self.editor = draft;
                }
                if self.local_command(&text) {
                    return;
                }
                if !text.trim().is_empty() {
                    self.cells.you(&text);
                }
                self.scroll = 0;
                (self.hooks.line)(text);
            }
            Outcome::Eof => (self.hooks.eof)(),
            Outcome::Interrupt => {
                if let Some(notice) = (self.hooks.interrupt)() {
                    self.warn(&notice);
                }
            }
            Outcome::Complete => {
                let cwd = std::env::current_dir().unwrap_or_default();
                let all: Vec<&str> = self.commands.iter().chain(COMMANDS).copied().collect();
                // Several: listed above the input until the next key.
                self.choices = self
                    .editor
                    .complete(&cwd, &all)
                    .iter()
                    .map(|c| safe(c))
                    .collect();
                self.choices.sort();
                self.choices.dedup();
            }
            // The screen is redrawn whole; the view goes to the bottom.
            Outcome::ClearScreen => {
                self.scroll = 0;
                self.redraw = true;
            }
            Outcome::Suspend => {
                terminal::suspend(tty);
                self.redraw = true;
            }
        }
    }
}

/// How well `query` matches `path` (its characters in order), or `None`:
/// contiguous runs and matches in the file name count more.
fn fuzzy(query: &str, path: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(-(path.len() as i64));
    }
    let lower = path.to_lowercase();
    let name_start = lower.rfind('/').map_or(0, |i| i + 1);
    let mut score = 0i64;
    let mut at = 0usize;
    let mut last: Option<usize> = None;
    for q in query.chars() {
        let found = lower[at..].find(q)? + at;
        score += 1;
        if last.is_some_and(|l| l + 1 == found) {
            score += 5;
        }
        if found >= name_start {
            score += 3;
        }
        last = Some(found);
        at = found + q.len_utf8();
    }
    if lower[name_start..].starts_with(query) {
        score += 20;
    }
    Some(score * 10 - lower.len() as i64 / 10)
}

/// The part of `text` between display columns `from` and `to`.
fn columns(text: &str, from: usize, to: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let mut col = 0;
    let mut out = String::new();
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        if col < to && col + width > from {
            out.push_str(grapheme);
        }
        col += width;
    }
    out
}

fn run(
    s: &mut State,
    screen: &mut Terminal<CrosstermBackend<File>>,
    tty: &mut File,
    rx: &Receiver<Msg>,
) {
    loop {
        loop {
            match rx.try_recv() {
                Ok(Msg::Stop(ack)) => {
                    let _ = ack.send(());
                    return;
                }
                Ok(Msg::Sync(ack)) => {
                    let _ = screen.draw(|f| view::draw(f, s, Instant::now()));
                    s.dirty = false;
                    let _ = ack.send(());
                }
                Ok(m) => s.message(m),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        match event::poll(Duration::from_millis(30)) {
            Ok(true) => {
                while let Ok(ev) = event::read() {
                    s.refresh_answering();
                    s.event(ev, tty);
                    if std::mem::take(&mut s.redraw) {
                        // Back from Ctrl-Alt-Z (or Ctrl-L): the screen is drawn whole.
                        let _ = screen.clear();
                    }
                    if !matches!(event::poll(Duration::ZERO), Ok(true)) {
                        break;
                    }
                }
            }
            Ok(false) => {}
            // The terminal is gone: nothing can be read any more.
            Err(_) => {
                (s.hooks.eof)();
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        s.refresh_answering();
        s.poll_clipboard(tty);
        if s.mode == Mode::Working {
            // The files declass writes show as it writes them.
            s.panel.refresh(false);
        }
        if s.overlay
            && s.ticked.elapsed() >= Duration::from_millis(500)
            && let Some(app) = s.app.as_mut()
        {
            // Background jobs (detection, the cache probe), the Runs view.
            app.tick();
            s.ticked = Instant::now();
            s.dirty = true;
        }
        let now = Instant::now();
        if s.toast
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() >= Duration::from_secs(6))
        {
            s.toast = None;
            s.dirty = true;
        }
        let ticking = s.mode == Mode::Working || s.mode == Mode::Hidden || s.clipboard.is_some();
        if s.dirty || (ticking && now.duration_since(s.drawn_at) >= TICK) {
            let _ = screen.draw(|f| view::draw(f, s, now));
            s.dirty = false;
            s.drawn_at = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use declass_boundary::model::{Item, ToolCall};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEvent;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn hooks(answering: Arc<AtomicBool>, sent: Arc<Mutex<Vec<String>>>) -> Hooks {
        Hooks {
            plan_action: Box::new(|_| {}),
            paste_image: Arc::new(|_| Err("clipboard unavailable in tests".into())),
            line: Box::new(move |l| sent.lock().unwrap().push(l)),
            eof: Box::new(|| {}),
            interrupt: Box::new(|| Some("interrupt: stopping this turn".into())),
            answering: Box::new(move || answering.load(Ordering::SeqCst)),
        }
    }

    fn state() -> (State, Arc<AtomicBool>, Arc<Mutex<Vec<String>>>) {
        let answering = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let s = State::new(
            false,
            hooks(answering.clone(), sent.clone()),
            &["/help", "/stop", "/status", "/image", "/quit"],
        );
        (s, answering, sent)
    }

    #[test]
    fn question_refresh_does_not_close_other_overlays_or_cancel_unrelated_paste() {
        let (mut s, _, _) = state();
        s.help = true;
        s.overlay = true;
        s.find.open = true;
        let epoch = s.input_epoch;
        s.message(Msg::PlanQuestion(None));
        assert!(s.help && s.overlay && s.find.open);
        assert_eq!(s.input_epoch, epoch);
        let question = PlanQuestion {
            id: "q1".into(),
            identity: None,
            question: "Choose?".into(),
            options: declass_agent::questions::QuestionOptions {
                id: "q1".into(),
                choices: vec![],
                allow_freeform: true,
                recommended: None,
            },
        };
        s.message(Msg::PlanQuestion(Some(question.clone())));
        assert!(!s.help && !s.overlay && !s.find.open);
        let mut tty = File::options()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap();
        s.event(key(KeyCode::Esc), &mut tty);
        assert!(!s.plan.question_active());
        s.help = true;
        s.overlay = true;
        s.find.open = true;
        let epoch = s.input_epoch;
        s.message(Msg::PlanQuestion(Some(question)));
        assert!(!s.plan.question_active());
        assert!(s.help && s.overlay && s.find.open);
        assert_eq!(s.input_epoch, epoch);
        s.plan.reopen_question();
        let epoch = s.input_epoch;
        s.message(Msg::PlanQuestion(None));
        assert!(!s.plan.question_active());
        assert!(s.help && s.overlay && s.find.open);
        assert_eq!(s.input_epoch, epoch.wrapping_add(1));
    }

    #[test]
    fn plan_overlay_preserves_composer_and_permission_modal_has_priority() {
        let (mut s, _, sent) = state();
        let actions = Arc::new(Mutex::new(Vec::new()));
        let target = actions.clone();
        s.hooks.plan_action = Box::new(move |action| target.lock().unwrap().push(action));
        s.editor.insert("Keep this unsent message");
        let identity = PlanIdentity {
            revision: "r1".into(),
            digest: "a".repeat(64),
        };
        s.message(Msg::Plan(Some(Box::new(PlanSnapshot {
            identity: identity.clone(),
            draft: declass_agent::plans::Draft {
                title: "Saved plan".into(),
                ..Default::default()
            },
            location: "plans/r1.md".into(),
            changes: None,
            verification: vec![],
            questions: vec![],
            status: "Draft".into(),
            approved: false,
            can_implement: true,
            can_resume: false,
            steps: vec![],
        }))));
        s.message(Msg::OpenPlan(true));
        let mut tty = File::options()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap();
        s.event(Event::Paste(" edit".into()), &mut tty);
        s.answering = true;
        s.question = Some("Allow tool?".into());
        let rendered = screen(&mut s, 80, 24);
        assert!(rendered.contains("approval"));
        s.event(key(KeyCode::Enter), &mut tty);
        assert_eq!(&*sent.lock().unwrap(), &["n"]);
        assert!(actions.lock().unwrap().is_empty());
        s.answering = false;
        s.event(ctrl('s'), &mut tty);
        assert!(
            matches!(&actions.lock().unwrap()[0], PlanAction::Save { identity: saved, draft } if saved==&identity && draft.title=="Saved plan edit")
        );
        assert_eq!(s.editor.buffer(), "Keep this unsent message");
        assert_eq!(&*sent.lock().unwrap(), &["n"]);
        s.message(Msg::PlanError("Test rejected save".into()));
        s.event(key(KeyCode::Esc), &mut tty);
        s.event(key(KeyCode::Esc), &mut tty);
        s.event(key(KeyCode::F(5)), &mut tty);
        s.event(key(KeyCode::Enter), &mut tty);
        assert_eq!(actions.lock().unwrap().len(), 1);
        assert_eq!(s.editor.buffer(), "Keep this unsent message");
    }

    fn screen(s: &mut State, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| view::draw(f, s, Instant::now())).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::from(code))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_text(s: &mut State, text: &str, tty: &mut File) {
        for c in text.chars() {
            s.event(key(KeyCode::Char(c)), tty);
        }
    }

    fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.as_object().unwrap().clone(),
            raw_arguments: args.to_string(),
        }
    }

    fn status() -> Status {
        Status {
            session: "20260927-1".into(),
            mode: "hybrid".into(),
            frontier: "glm-5.3-flash".into(),
            local: Some("omlx-coding".into()),
            turns: 2,
            cost_usd: 0.0123,
            budget_usd: 5.0,
            ..Status::default()
        }
    }

    /// A turn: a read (its result tokenized), an edit, a failing command,
    /// then a reply.
    fn a_turn(s: &mut State) {
        s.message(Msg::Status(status()));
        s.message(Msg::Mode(Mode::Working));
        s.message(Msg::Begin);
        s.message(Msg::Entry(Box::new(Entry::Item {
            item: Item::Assistant {
                text: String::new(),
                reasoning: None,
                replay: None,
                tool_calls: vec![
                    call("c1", "read_file", serde_json::json!({"path": "src/lib.rs"})),
                    call(
                        "c2",
                        "edit_file",
                        serde_json::json!({"path": "src/lib.rs", "edits": [{"old": "pub fn a() {}", "new": "pub fn a() {}\npub mod b;"}]}),
                    ),
                    call("c3", "run_command", serde_json::json!({"command": "cargo test"})),
                ],
            },
        })));
        let result = |id: &str, content: &str| {
            Msg::Entry(Box::new(Entry::Item {
                item: Item::ToolResult {
                    call_id: id.into(),
                    content: content.into(),
                },
            }))
        };
        s.message(result(
            "c1",
            "pub fn a() {}\nkey = ⟨secret#1⟩\nline 3\nline 4\nline 5",
        ));
        s.message(Msg::Entry(Box::new(Entry::Shown {
            call_id: "c1".into(),
            class: declass_boundary::view::ViewClass::Tokenized,
        })));
        s.message(result("c2", "edited src/lib.rs"));
        s.message(result(
            "c3",
            "exit code 101\n--- stdout ---\ntest b ... FAILED",
        ));
        s.message(Msg::End(TurnEnd::Replied {
            message: "Declared **b** in lib.rs; one test still fails.".into(),
        }));
        s.message(Msg::Mode(Mode::Prompt));
    }

    #[test]
    fn planning_is_visible_and_commands_are_discoverable() {
        let (mut s, _, _) = state();
        s.message(Msg::Planning(true));
        let rendered = screen(&mut s, 110, 30);
        assert!(rendered.contains("PLAN"), "{rendered}");
        assert!(DESCRIBED.iter().any(|(command, _)| *command == "/plan"));
        let mut tty = File::options().write(true).open("/dev/null").unwrap();
        type_text(&mut s, "/plan ", &mut tty);
        let commands: Vec<_> = s
            .palette_items()
            .into_iter()
            .map(|(command, _)| command)
            .collect();
        assert_eq!(commands, vec!["/plan on", "/plan off", "/plan status"]);
    }

    #[test]
    fn the_work_mode_is_named_in_the_header_on_the_input_and_on_the_status_line() {
        for (planning, goal_running, name, gist) in [
            (false, false, " BUILD ", "edits files and runs commands"),
            (
                true,
                false,
                " PLAN ",
                "read-only: investigates and saves a plan",
            ),
            (false, true, " GOAL ", "keeps working until the goal is met"),
            // Planning pauses a goal: the mode is planning.
            (true, true, " PLAN ", "read-only"),
        ] {
            let (mut s, _, _) = state();
            s.message(Msg::Status(Status {
                planning,
                goal_running,
                ..status()
            }));
            s.message(Msg::Mode(Mode::Prompt));
            let out = screen(&mut s, 140, 24);
            let rows: Vec<&str> = out.lines().collect();
            assert!(rows[1].trim_start().starts_with(name.trim()), "{out}");
            let input = rows
                .iter()
                .find(|r| r.contains(" Message declass "))
                .unwrap();
            assert!(input.contains(name) && input.contains(gist), "{out}");
            assert!(rows[23].contains(name.trim()), "{out}");
        }
    }

    #[test]
    fn the_layout_is_a_header_the_conversation_the_input_and_a_status_line() {
        let (mut s, _, _) = state();
        s.message(Msg::Status(status()));
        s.message(Msg::Mode(Mode::Prompt));
        let out = screen(&mut s, 100, 24);
        let rows: Vec<&str> = out.lines().collect();
        assert!(rows[0].contains("DECLASS  /"), "{out}");
        assert!(
            rows[1].contains("hybrid · frontier glm-5.3-flash · local omlx-coding"),
            "{out}"
        );
        assert!(rows[1].contains("privacy boundary active"), "{out}");
        assert!(
            out.contains("What are we working on?"),
            "the welcome: {out}"
        );
        assert!(out.contains(" Message declass "), "{out}");
        assert!(
            out.contains("Describe a task or ask a question"),
            "the placeholder: {out}"
        );
        assert!(
            rows[23].contains("READY") && rows[23].contains("turn 2 · $0.0123 of $5.00"),
            "{out}"
        );
    }

    #[test]
    fn a_turn_reads_as_cells() {
        let (mut s, _, _) = state();
        a_turn(&mut s);
        let out = screen(&mut s, 100, 40);
        assert!(
            !out.contains("What are we working on?"),
            "no welcome once work began"
        );
        assert!(
            out.contains("● read  src/lib.rs · done  ◦ values replaced by placeholders"),
            "{out}"
        );
        assert!(
            out.contains("│ key = ⟨secret#1⟩"),
            "what the frontier saw: {out}"
        );
        assert!(out.contains("more lines · Ctrl-O"), "folded: {out}");
        assert!(out.contains("edit  src/lib.rs · done  ◦ +2 −1"), "{out}");
        assert!(
            out.contains("+ pub mod b;") && out.contains("- pub fn a() {}"),
            "the diff: {out}"
        );
        assert!(
            out.contains("✗ run  cargo test"),
            "a failing command: {out}"
        );
        assert!(
            out.contains("test b ... FAILED"),
            "a failure is not folded: {out}"
        );
        assert!(
            out.contains("◆ declass") && out.contains("in lib.rs; one test still fails."),
            "{out}"
        );
    }

    #[test]
    fn ctrl_o_unfolds_tool_results() {
        let (mut s, _, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        a_turn(&mut s);
        assert!(!screen(&mut s, 100, 40).contains("line 5"));
        s.event(ctrl('o'), &mut tty);
        let out = screen(&mut s, 100, 40);
        assert!(
            out.contains("line 5") && !out.contains("more lines · Ctrl-O"),
            "{out}"
        );
    }

    #[test]
    fn journal_shows_local_questions_source_status_and_filtered_answer() {
        let (mut s, _, _) = state();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("handles")).unwrap();
        std::fs::write(dir.path().join("handles/h1.source"), "specs/game.md").unwrap();
        std::fs::write(dir.path().join("handles/h1"), "RAW_HANDLE_CANARY").unwrap();
        s.cells.set_run_dir(dir.path().to_owned());
        s.cells
            .set_restore(Arc::new(|t| t.replace("⟨secret#1⟩", "RESTORED_CANARY")));
        s.message(Msg::Entry(Box::new(Entry::Item {
            item: Item::Assistant {
                text: String::new(), reasoning: None, replay: None,
                tool_calls: vec![call("local1", "ask_local", serde_json::json!({
                    "handle":"h1", "questions":["What are the jump controls?", "Explain ⟨secret#1⟩\nand the treasure rules."]
                }))],
            },
        })));
        let pending = screen(&mut s, 100, 30);
        assert!(pending.contains("ask local  h1 · running"), "{pending}");
        assert!(pending.contains("Source: specs/game.md"), "{pending}");
        assert!(
            pending.contains("Q1: What are the jump controls?"),
            "{pending}"
        );
        assert!(pending.contains("Q2: Explain ⟨secret#1⟩"), "{pending}");
        assert!(pending.contains("and the treasure rules."), "{pending}");
        assert!(!pending.contains("Prepared answer"));
        assert!(!pending.contains("CANARY"));
        assert!(!pending.contains("on this machine"));
        s.message(Msg::Entry(Box::new(Entry::Item {
            item: Item::ToolResult {
                call_id: "local1".into(),
                content: "Space jumps.\n⟨redacted:copied-sensitive-text⟩".into(),
            },
        })));
        s.message(Msg::Entry(Box::new(Entry::Shown {
            call_id: "local1".into(),
            class: declass_boundary::view::ViewClass::LocalAnswer,
        })));
        let answered = screen(&mut s, 100, 30);
        assert!(answered.contains("ask local  h1 · done"), "{answered}");
        assert!(answered.contains("1 redaction(s) in result"), "{answered}");
        assert!(
            answered.contains("Prepared answer (filtered for the frontier)"),
            "{answered}"
        );
        assert!(
            answered.contains("⟨redacted:copied-sensitive-text⟩"),
            "{answered}"
        );
        assert!(!answered.contains("CANARY"));
        if let Ok(path) = std::env::var("DECLASS_JOURNAL_PREVIEW") {
            std::fs::write(path, answered).unwrap();
        }
    }

    #[test]
    fn journal_keeps_full_command_read_range_and_local_failure() {
        let (mut s, _, _) = state();
        let command = format!(
            "cat /{}\nprintf 'LAST_COMMAND_LINE'",
            "long_path/".repeat(12)
        );
        s.message(Msg::Entry(Box::new(Entry::Item {
            item: Item::Assistant {
                text: String::new(), reasoning: None, replay: None,
                tool_calls: vec![
                    call("cmd", "run_command", serde_json::json!({"command":command})),
                    call("read", "read_raw", serde_json::json!({"handle":"h1", "start_line":40, "end_line":90})),
                    call("ask", "ask_local", serde_json::json!({"handle":"h1", "question":"What controls are specified?"})),
                ],
            },
        })));
        s.message(Msg::Entry(Box::new(Entry::Item {
            item: Item::ToolResult {
                call_id: "ask".into(),
                content: "error: local model unavailable".into(),
            },
        })));
        let shown = screen(&mut s, 100, 40);
        assert!(shown.contains("LAST_COMMAND_LINE"), "{shown}");
        assert!(shown.contains("lines 40–90"), "{shown}");
        assert!(shown.contains("ask local  h1 · failed"), "{shown}");
        assert!(
            shown.contains("Question: What controls are specified?"),
            "{shown}"
        );
        assert!(shown.contains("error: local model unavailable"), "{shown}");
        assert!(!shown.contains("Prepared answer"));
    }

    #[test]
    fn an_edit_of_a_sensitive_file_is_named_never_shown() {
        let (mut s, _, _) = state();
        s.cells.set_held(Arc::new(|p: &str| p == ".env"));
        s.message(Msg::Entry(Box::new(Entry::Item {
            item: Item::Assistant {
                text: String::new(),
                reasoning: None,
                replay: None,
                tool_calls: vec![call(
                    "c1",
                    "write_file",
                    serde_json::json!({"path": ".env", "content": "API_KEY=sk_live_abc"}),
                )],
            },
        })));
        let out = screen(&mut s, 100, 30);
        assert!(
            out.contains("write  .env · running  ◦ sensitive: content not shown"),
            "{out}"
        );
        assert!(!out.contains("sk_live"), "{out}");
    }

    #[test]
    fn a_reply_shows_as_it_streams() {
        let (mut s, _, _) = state();
        s.message(Msg::Mode(Mode::Working));
        s.message(Msg::Begin);
        s.message(Msg::Stream(Streamed::Attempt(1)));
        s.message(Msg::Stream(Streamed::Call {
            index: 0,
            name: "reply".into(),
        }));
        s.message(Msg::Stream(Streamed::Arguments {
            index: 0,
            delta: "{\"message\": \"Half of the ".into(),
        }));
        let out = screen(&mut s, 100, 30);
        assert!(
            out.contains("◆ declass") && out.contains("Half of the"),
            "{out}"
        );
        assert!(out.contains("WORKING"), "{out}");
        s.message(Msg::Stream(Streamed::Arguments {
            index: 0,
            delta: "answer.\"}".into(),
        }));
        s.message(Msg::Stream(Streamed::End));
        s.message(Msg::End(TurnEnd::Replied {
            message: "Half of the answer.".into(),
        }));
        let out = screen(&mut s, 100, 30);
        assert_eq!(
            out.matches("Half of the answer.").count(),
            1,
            "not repeated: {out}"
        );
    }

    #[test]
    fn an_approval_is_a_dialog_that_defaults_to_no() {
        let (mut s, answering, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.message(Msg::Mode(Mode::Working));
        s.message(Msg::Text("commit 2 file(s): src/lib.rs, src/b.rs\n".into()));
        answering.store(true, Ordering::SeqCst);
        s.answering = true;
        let out = screen(&mut s, 100, 30);
        assert!(
            out.contains("! approval") && out.contains("commit 2 file(s)"),
            "{out}"
        );
        assert!(out.contains("› 1  No, deny"), "No is the default: {out}");
        s.event(key(KeyCode::Enter), &mut tty);
        s.event(key(KeyCode::Char('y')), &mut tty);
        s.event(key(KeyCode::Enter), &mut tty);
        assert_eq!(*sent.lock().unwrap(), ["n", "y"]);
    }

    #[test]
    fn the_palette_lists_commands_and_takes_the_arrows() {
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        type_text(&mut s, "/s", &mut tty);
        let out = screen(&mut s, 100, 30);
        assert!(out.contains("Commands ·"), "{out}");
        assert!(
            out.contains("/status") && out.contains("turns, tokens, cost"),
            "{out}"
        );
        assert!(out.contains("/settings"), "{out}");
        // Enter runs the selected command, independent of catalog ordering.
        s.palette = s
            .palette_items()
            .iter()
            .position(|(name, _)| *name == "/status")
            .unwrap();
        s.event(key(KeyCode::Enter), &mut tty);
        assert_eq!(*sent.lock().unwrap(), ["/status"]);
        // Tab puts a command that takes an argument into the input.
        type_text(&mut s, "/ima", &mut tty);
        s.event(key(KeyCode::Tab), &mut tty);
        assert_eq!(s.editor.buffer(), "/image ");
    }

    #[test]
    fn goal_commands_complete_and_history_reaches_the_session() {
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        type_text(&mut s, "/goal", &mut tty);
        s.event(key(KeyCode::Tab), &mut tty);
        assert_eq!(s.editor.buffer(), "/goal ");
        assert_eq!(s.palette_items().len(), 5);
        s.event(key(KeyCode::Enter), &mut tty);
        assert_eq!(s.editor.buffer(), "/goal start ");
        assert!(sent.lock().unwrap().is_empty());
        type_text(&mut s, "fix the tests", &mut tty);
        s.event(key(KeyCode::Enter), &mut tty);
        for command in [
            "/goal",
            "/goal pause",
            "/goal resume",
            "/goal cancel",
            "/goal status",
            "/goal improve the docs",
            "/history",
            "/history 20261001-test",
        ] {
            type_text(&mut s, command, &mut tty);
            s.event(key(KeyCode::Enter), &mut tty);
        }
        assert_eq!(
            *sent.lock().unwrap(),
            [
                "/goal start fix the tests",
                "/goal",
                "/goal pause",
                "/goal resume",
                "/goal cancel",
                "/goal status",
                "/goal improve the docs",
                "/history",
                "/history 20261001-test",
            ]
        );
        assert!(!s.overlay, "history is handled by the session");

        s.status = status();
        s.status.goal = Some("Active · 3/20 turns · Fix the tests".into());
        s.panel.tab = Some(panel::Tab::Session);
        assert!(screen(&mut s, 160, 40).contains("Active · 3/20 turns"));
    }

    #[test]
    fn tab_cycles_panel_tabs_in_both_directions_without_changing_the_message() {
        use panel::Tab;
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        screen(&mut s, 130, 30);
        type_text(&mut s, "continue building", &mut tty);
        for tab in [Tab::Privacy, Tab::Session, Tab::Changes] {
            s.event(key(KeyCode::Tab), &mut tty);
            assert_eq!(s.panel.tab, Some(tab));
        }
        for tab in [Tab::Session, Tab::Privacy, Tab::Changes] {
            s.event(key(KeyCode::BackTab), &mut tty);
            assert_eq!(s.panel.tab, Some(tab));
        }
        assert_eq!(s.editor.buffer(), "continue building");
        assert!(sent.lock().unwrap().is_empty());
        let (mut narrow, _, _) = state();
        screen(&mut narrow, 100, 30);
        assert_eq!(narrow.panel.tab, None);
        narrow.event(key(KeyCode::Tab), &mut tty);
        assert_eq!(narrow.panel.tab, Some(Tab::Changes));
    }

    #[test]
    fn command_menu_fits_available_height_and_keeps_its_last_command_visible() {
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        type_text(&mut s, "/", &mut tty);
        let count = s.palette_items().len();
        let tall = screen(&mut s, 120, 45);
        assert!(
            tall.contains(&format!("Commands · {count} total")),
            "{tall}"
        );
        assert!(tall.contains("/goal") && tall.contains("/close"), "{tall}");
        assert_eq!(
            s.palette_popup.as_ref().unwrap().rows.height as usize,
            count
        );
        let short = screen(&mut s, 40, 10);
        let visible = s.palette_popup.as_ref().unwrap().rows.height as usize;
        assert!(visible > 0 && visible < count);
        assert!(short.contains("more") && short.contains("↓"), "{short}");
        s.event(key(KeyCode::PageDown), &mut tty);
        assert_eq!(s.palette, visible);
        s.event(key(KeyCode::End), &mut tty);
        let end = screen(&mut s, 40, 10);
        assert_eq!(s.palette, count - 1);
        assert!(end.contains("/close") && end.contains("above"), "{end}");
        let popup = s.palette_popup.as_ref().unwrap();
        assert!(s.palette >= popup.first && s.palette < popup.first + popup.rows.height as usize);
        s.event(key(KeyCode::Home), &mut tty);
        assert_eq!(s.palette, 0);
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn menu_mouse_navigation_stays_in_menu_and_never_runs_a_clicked_command() {
        use ratatui::crossterm::event::MouseEvent;
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.editor.insert("/");
        screen(&mut s, 100, 18);
        let area = s.palette_popup.as_ref().unwrap().rows;
        let mouse = |kind, column, row| {
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        s.event(
            mouse(MouseEventKind::ScrollDown, area.x + 2, area.y),
            &mut tty,
        );
        assert_eq!(s.palette, 3);
        assert_eq!(s.scroll, 0);
        screen(&mut s, 100, 18);
        let popup = s.palette_popup.as_ref().unwrap();
        let target = popup.first + popup.rows.height as usize - 1;
        let x = popup.rows.x + 2;
        let y = popup.rows.bottom() - 1;
        s.event(
            mouse(MouseEventKind::Down(MouseButton::Left), x, y),
            &mut tty,
        );
        assert_eq!(s.palette, target);
        assert_eq!(s.editor.buffer(), "/");
        assert!(sent.lock().unwrap().is_empty());
        assert!(s.sel.is_none());
        // Summary/footer is not a command row and cannot change the selection.
        s.event(
            mouse(MouseEventKind::Down(MouseButton::Left), x, y + 1),
            &mut tty,
        );
        assert_eq!(s.palette, target);
        // A query can change before the next draw; old row geometry must not select it.
        s.editor.insert("zzzz");
        s.event(
            mouse(MouseEventKind::Down(MouseButton::Left), x, y),
            &mut tty,
        );
        assert_eq!(s.palette, target);
        screen(&mut s, 100, 18);
        assert!(s.palette_popup.is_none());
    }

    #[test]
    fn pasted_menu_filters_reset_selection_and_f4_paging_preserves_the_draft() {
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.editor.insert("draft\nsecond line");
        s.editor
            .key(event::KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        let cursor = s.editor.cursor();
        let selection = s.editor.selection_range();
        s.event(key(KeyCode::F(4)), &mut tty);
        screen(&mut s, 80, 18);
        s.event(key(KeyCode::End), &mut tty);
        s.event(Event::Paste("stat".into()), &mut tty);
        assert_eq!(s.palette, 0);
        assert_eq!(
            s.palette_items(),
            vec![(
                "/status",
                "turns, tokens, cost and time against the budgets"
            )]
        );
        s.event(key(KeyCode::PageDown), &mut tty);
        assert_eq!(s.palette, 0);
        let shown = screen(&mut s, 40, 10);
        assert!(shown.contains("/status"), "{shown}");
        s.event(key(KeyCode::Esc), &mut tty);
        assert_eq!(s.editor.buffer(), "draft\nsecond line");
        assert_eq!(s.editor.cursor(), cursor);
        assert_eq!(s.editor.selection_range(), selection);
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn at_picks_a_file_of_the_workspace() {
        let (mut s, _, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.files = Some(vec![
            "src/tokenizer.rs".into(),
            "src/parser.rs".into(),
            "README.md".into(),
        ]);
        type_text(&mut s, "look at @tok", &mut tty);
        let out = screen(&mut s, 100, 30);
        assert!(
            out.contains("files · Tab or Enter picks") && out.contains("src/tokenizer.rs"),
            "{out}"
        );
        s.event(key(KeyCode::Tab), &mut tty);
        assert_eq!(s.editor.buffer(), "look at @src/tokenizer.rs ");
    }

    #[test]
    fn output_from_elsewhere_and_hostile_text_are_drawn_harmlessly() {
        let (mut s, _, _) = state();
        s.message(Msg::Output(
            "warning: \x1b]52;c;ZXZpbA==\x07clipboard".into(),
        ));
        s.message(Msg::Text("reply \x1b[2Jcleared?".into()));
        let out = screen(&mut s, 100, 30);
        assert!(out.contains("│ warning: clipboard"), "{out}");
        assert!(out.contains("reply cleared?"), "{out}");
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn the_view_follows_new_output_unless_scrolled_up() {
        let (mut s, _, _) = state();
        s.message(Msg::Lines((0..100).map(|i| format!("line {i}")).collect()));
        assert!(screen(&mut s, 100, 24).contains("line 99"));
        s.scroll_by(20);
        s.message(Msg::Lines(vec!["line 100".into()]));
        let out = screen(&mut s, 100, 24);
        assert!(!out.contains("line 100"), "stays put: {out}");
        assert!(out.contains("more below · Ctrl-End"), "{out}");
        let mut tty = tempfile::tempfile().unwrap();
        s.event(
            Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL)),
            &mut tty,
        );
        assert!(screen(&mut s, 100, 24).contains("line 100"));
    }

    #[test]
    fn transcript_ranges_preserve_cell_boundaries_and_cross_cell_selection() {
        let (mut s, _, _) = state();
        let now = Instant::now();
        let welcome = s.transcript(80, now);
        let all = welcome.rows(&s.cells, 0..welcome.total);
        assert!(
            all.iter()
                .any(|line| line.to_string().contains("What are we working on?"))
        );
        assert_eq!(welcome.rows(&s.cells, 1..3), all[1..3]);

        for text in ["first user message", "第二个 message", "last user message"] {
            s.cells.you(text);
        }
        let layout = s.transcript(20, now);
        let all = layout.rows(&s.cells, 0..layout.total);
        assert_eq!(all.len(), layout.total);
        // Range boundaries can land in a title, wrapped body, or separator.
        for start in 0..=all.len() {
            let end = (start + 4).min(all.len());
            assert_eq!(layout.rows(&s.cells, start..end), all[start..end]);
        }
        let first = all
            .iter()
            .position(|line| line.to_string().contains("first user message"))
            .unwrap();
        let last = all
            .iter()
            .position(|line| line.to_string().contains("last user message"))
            .unwrap();
        s.width = 20;
        s.sel = Some(((first, 2), (last, usize::MAX)));
        let copied = s.selected_text().unwrap();
        assert!(copied.starts_with("first user message"), "{copied}");
        assert!(copied.contains("第二个 message"), "{copied}");
        assert!(copied.ends_with("last user message"), "{copied}");

        // Resizing refreshes the per-cell cache and the global row offsets.
        let narrow = s.transcript(10, now);
        assert!(narrow.total > layout.total);
        let widened = s.transcript(20, now);
        assert_eq!(widened.rows(&s.cells, 0..widened.total), all);
    }

    #[test]
    #[ignore = "manual viewport microbenchmark; run with --ignored --nocapture"]
    fn benchmark_cached_transcript_viewport() {
        use std::hint::black_box;
        let (mut s, _, _) = state();
        for i in 0..2_000 {
            s.cells.you(&format!(
                "Message {i}: {}",
                "cached conversation content ".repeat(4)
            ));
        }
        let total = s.transcript(80, Instant::now()).total;
        let viewport = total.saturating_sub(30)..total;
        let iterations = 100;
        let start = Instant::now();
        for _ in 0..iterations {
            let layout = s.transcript(80, Instant::now());
            // Previous draw path materialized the entire conversation, then
            // cloned the visible slice once more for selection highlighting.
            let all = layout.rows(&s.cells, 0..layout.total);
            black_box(all[viewport.clone()].to_vec());
        }
        let full = start.elapsed();
        let start = Instant::now();
        for _ in 0..iterations {
            let layout = s.transcript(80, Instant::now());
            black_box(layout.rows(&s.cells, viewport.clone()));
        }
        let visible = start.elapsed();
        eprintln!(
            "{iterations} cached draws; {total} total rows, 30 visible: whole transcript {full:?}; viewport {visible:?} ({:.1}x)",
            full.as_secs_f64() / visible.as_secs_f64()
        );
    }

    #[test]
    fn a_sent_message_reaches_the_session_and_the_conversation() {
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        type_text(&mut s, "fix it", &mut tty);
        s.event(key(KeyCode::Enter), &mut tty);
        assert_eq!(*sent.lock().unwrap(), ["fix it"]);
        let out = screen(&mut s, 100, 24);
        assert!(out.contains("› you") && out.contains("│ fix it"), "{out}");
    }

    #[test]
    fn the_rail_opens_on_a_wide_screen_and_ctrl_t_cycles_it() {
        let (mut s, _, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        let out = screen(&mut s, 130, 24);
        assert!(
            out.contains(" Changes ") && out.contains("No files changed yet."),
            "{out}"
        );
        s.event(ctrl('t'), &mut tty);
        assert!(screen(&mut s, 130, 24).contains("Privacy activity"));
        s.event(ctrl('t'), &mut tty);
        assert!(screen(&mut s, 130, 24).contains("starts with your first message"));
        s.event(ctrl('t'), &mut tty);
        assert!(!screen(&mut s, 130, 24).contains(" Changes "));
        let (mut n, _, _) = state();
        assert!(
            !screen(&mut n, 100, 24).contains(" Changes "),
            "narrow: closed"
        );
        n.event(ctrl('t'), &mut tty);
        assert!(screen(&mut n, 100, 24).contains(" Changes "));
    }

    #[test]
    fn journal_groups_repeated_filtering_but_keeps_the_first_concrete_description() {
        let (mut s, _, _) = state();
        let note = "sanitize: history item 2 · call c7 · edit_file · src/client.rs · arguments.new: filtered line(s) 2; replaced with ⟨secret:KEY#1⟩ (secret; first seen .env)";
        for turn in [3, 4] {
            s.message(Msg::Entry(Box::new(Entry::Usage {
                turn,
                usage: Default::default(),
                cost_usd: 0.0,
                interventions: vec![note.into()],
            })));
        }
        let out = screen(&mut s, 100, 40);
        assert_eq!(out.matches("call c7").count(), 1, "{out}");
        assert!(out.contains("src/client.rs"), "{out}");
        assert!(out.contains("first seen .env"), "{out}");
        assert!(out.contains("0 new details, 1 repeated"), "{out}");
        assert!(out.contains("history is checked on every request"), "{out}");
        if let Ok(path) = std::env::var("DECLASS_FILTER_PREVIEW") {
            std::fs::write(path, out).unwrap();
        }
    }

    #[test]
    fn the_privacy_view_shows_filtering_and_selected_read_details() {
        let (mut s, _, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        a_turn(&mut s);
        s.message(Msg::Entry(Box::new(Entry::Usage {
            turn: 3,
            usage: Default::default(),
            cost_usd: 0.0,
            interventions: vec!["2 values replaced (EMAIL, CARD)".into()],
        })));
        // The rail opens on Changes at this width; Ctrl-T goes on to Privacy.
        screen(&mut s, 130, 40);
        s.event(ctrl('t'), &mut tty);
        let out = screen(&mut s, 130, 40);
        assert!(out.contains("Privacy · action"), "{out}");
        assert!(out.contains("Checked model request"), "{out}");
        assert!(out.contains("◦ privacy filtering"), "the cell too: {out}");
        s.event(ctrl('o'), &mut tty);
        let expanded = screen(&mut s, 130, 40);
        assert!(
            expanded.contains("2 values replaced (EMAIL, CARD)"),
            "{expanded}"
        );
        s.event(ctrl('o'), &mut tty);
        s.event(
            ratatui::crossterm::event::Event::Key(KeyEvent::new(
                KeyCode::Down,
                KeyModifiers::CONTROL,
            )),
            &mut tty,
        );
        let selected = screen(&mut s, 130, 40);
        assert!(selected.contains("Ran command"), "{selected}");
        s.event(
            ratatui::crossterm::event::Event::Key(KeyEvent::new(
                KeyCode::Down,
                KeyModifiers::CONTROL,
            )),
            &mut tty,
        );
        let selected = screen(&mut s, 130, 40);
        assert!(selected.contains("src/lib.rs"), "{selected}");
    }

    #[test]
    fn settings_open_over_the_conversation_and_esc_comes_back() {
        let (mut s, _, sent) = state();
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join(".declass")).unwrap();
        s.settings = Some(Settings {
            paths: crate::Paths {
                owner: d.path().join("owner.toml"),
                project: ws.join(".declass/config.toml"),
                state: d.path().join("state"),
                workspace: ws,
                policy: None,
            },
            services: crate::tests::services(),
        });
        let mut tty = tempfile::tempfile().unwrap();
        type_text(&mut s, "/audit", &mut tty);
        s.palette_off = Some("/audit".into());
        s.event(key(KeyCode::Enter), &mut tty);
        assert!(s.overlay);
        assert!(sent.lock().unwrap().is_empty(), "never sent to the session");
        let out = screen(&mut s, 100, 30);
        assert!(
            out.contains("declass settings") && out.contains("runs: none yet"),
            "{out}"
        );
        s.event(key(KeyCode::Esc), &mut tty);
        assert!(!s.overlay);
        s.event(key(KeyCode::F(2)), &mut tty);
        assert!(
            screen(&mut s, 100, 30).contains("runs: none yet"),
            "F2 reopens where it was"
        );
    }

    #[test]
    fn f1_shows_the_keys() {
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.event(key(KeyCode::F(1)), &mut tty);
        let out = screen(&mut s, 110, 40);
        assert!(
            out.contains("keys and commands") && out.contains("Ctrl-V"),
            "{out}"
        );
        s.event(key(KeyCode::Esc), &mut tty);
        assert!(!s.help);
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn dragging_selects_text_of_the_conversation() {
        use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
        let (mut s, _, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.message(Msg::Lines(vec![
            "first line of notes".into(),
            "second line".into(),
        ]));
        let mut t = Terminal::new(TestBackend::new(100, 24)).unwrap();
        t.draw(|f| view::draw(f, &mut s, Instant::now())).unwrap();
        // Find where "second" is on the screen.
        let buf = t.backend().buffer().clone();
        let (x, y) = (0..24u16)
            .flat_map(|y| (0..94u16).map(move |x| (x, y)))
            .find(|&(x, y)| {
                (0..6).map(|i| buf[(x + i, y)].symbol()).collect::<String>() == "second"
            })
            .expect("drawn");
        let mouse = |kind, column, row| {
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        s.event(
            mouse(MouseEventKind::Down(MouseButton::Left), x, y),
            &mut tty,
        );
        s.event(
            mouse(MouseEventKind::Drag(MouseButton::Left), x + 6, y),
            &mut tty,
        );
        assert_eq!(s.selected_text().as_deref(), Some("second"));
        t.draw(|f| view::draw(f, &mut s, Instant::now())).unwrap();
        let buf = t.backend().buffer();
        assert!(
            buf[(x, y)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        assert!(
            !buf[(x + 7, y)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        // A key ends the selection.
        s.event(key(KeyCode::Char('a')), &mut tty);
        assert!(s.sel.is_none());
    }

    #[test]
    fn a_small_terminal_gets_a_notice() {
        let (mut s, _, _) = state();
        assert!(screen(&mut s, 30, 8).contains("needs at least 40x10"));
    }

    #[test]
    fn fuzzy_prefers_the_file_name() {
        assert!(fuzzy("tok", "src/tokenizer.rs") > fuzzy("tok", "docs/stock/list.md"));
        assert_eq!(fuzzy("zz", "src/lib.rs"), None);
    }
}
