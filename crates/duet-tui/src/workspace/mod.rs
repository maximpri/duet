// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace: `duet` on a terminal, full screen.
//!
//! A header (the workspace, the mode and models, where sensitive values go),
//! the conversation as cells (the operator's messages, duet's replies
//! streamed as Markdown, each tool call with its folded result, edits with
//! their diffs, what the boundary withheld, how each turn ended), the side
//! rail (Changes, Privacy, Session), the input, and a status line with a
//! badge. The palette, the `@` file picker, the approval dialog, help and the
//! settings open over it. The session itself runs in the CLI
//! (`duet-cli`'s chat loop); it talks to the workspace through this handle,
//! and hears from it through [`Hooks`]: the lines the operator sends, Ctrl-C,
//! the end of input.
//!
//! One thread owns the screen and the keyboard. The input is editable at all
//! times: typing while duet works steers it. Output from any other code (the
//! process's standard output and error) appears in the conversation, dimmed;
//! see [`terminal`]. Text from the frontier, tools and files reaches the
//! screen only through `term::safe` (the feed applies it), and the lines are
//! drawn through [`ansi`], which draws no escape sequence of its own.

mod ansi;
mod cells;
mod panel;
mod terminal;
mod view;

use crate::term::editor::{Editor, Outcome};
use crate::term::feed::{Feed, Restore, Streamed};
use crate::term::{colour, safe};
use duet_agent::TurnEnd;
use duet_agent::transcript::Entry;
use duet_boundary::live::{StreamEvent, StreamTap};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::fs::File;
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
    pub session: String,
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
    /// A line the operator sent (continuation lines joined).
    pub line: Box<dyn Fn(String) + Send>,
    /// Ctrl-D on an empty input.
    pub eof: Box<dyn Fn() + Send>,
    /// Ctrl-C: what to tell the operator, if anything.
    pub interrupt: Box<dyn Fn() -> Option<String> + Send>,
    /// Whether an approval question waits for the next line.
    pub answering: Box<dyn Fn() -> bool + Send>,
}

enum Msg {
    Lines(Vec<String>),
    Text(String),
    Diff(String),
    Entry(Box<Entry>),
    Stream(Streamed),
    Begin,
    End(TurnEnd),
    Mode(Mode),
    Status(Status),
    /// The session's workspace, run directory, audit log and policy, for
    /// the side panel.
    Attach {
        ws: PathBuf,
        run_dir: PathBuf,
        audit: PathBuf,
        policy: Box<duet_boundary::policy::Policy>,
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
    /// Takes over the terminal. `commands` are completed after `/`.
    pub fn start(
        hooks: Hooks,
        commands: &'static [&'static str],
        settings: Option<Settings>,
    ) -> std::io::Result<Arc<Self>> {
        let mut tty = terminal::tty()?;
        let draw_to = tty.try_clone()?;
        terminal::enter(&mut tty)?;
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
            .name("duet-workspace".into())
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

    /// Lines of duet's own (tinted by their marks; made safe).
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
        policy: duet_boundary::policy::Policy,
    ) {
        self.send(Msg::Attach {
            ws,
            run_dir,
            audit,
            policy: Box::new(policy),
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

/// How often the screen moves while duet works (the spinner, the clock).
const TICK: Duration = Duration::from_millis(120);

/// Commands with what they do, for the palette (the session's and the
/// workspace's own).
const DESCRIBED: &[(&str, &str)] = &[
    ("/help", "keys and commands"),
    (
        "/status",
        "turns, tokens, cost and time against the budgets",
    ),
    ("/diff", "what changed, against the last commit"),
    ("/undo", "revert the file writes of the last turn"),
    ("/stop", "end duet's turn after its current step"),
    ("/image", "attach an image to your next message"),
    (
        "/mode",
        "this session's mode; top-clearance continues with the local model only",
    ),
    ("/settings", "the settings screens"),
    ("/models", "frontier and local models; the doctor"),
    ("/sensitivity", "what is sensitive; test a path or text"),
    ("/ip", "interface-only and sealed code"),
    ("/limits", "budgets and time limits"),
    ("/data", "retention; purge old runs"),
    ("/audit", "what was sent; verify the chain"),
    ("/runs", "earlier runs and sessions"),
    ("/quit", "leave; the session stays open"),
    ("/close", "end the session for good"),
];

/// Commands that take an argument: the palette puts them in the input.
const WITH_ARGUMENT: &[&str] = &["/image", "/mode"];

/// The `@` picker's matches shown at most.
const PICKS: usize = 8;

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
    /// Completions shown above the input until the next key.
    choices: Vec<String>,
    /// The palette's selected command; hidden after Esc until the input changes.
    palette: usize,
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
    /// When something was copied, and how many characters.
    copied: Option<(Instant, usize)>,
    /// The settings overlay: its sources until first opened, then the app.
    settings: Option<Settings>,
    app: Option<crate::app::App>,
    overlay: bool,
    ticked: Instant,
    started: Instant,
    dirty: bool,
    /// The screen must be drawn whole (after Ctrl-Z).
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
            choices: Vec::new(),
            palette: 0,
            palette_off: None,
            files: None,
            pick: 0,
            sel: None,
            selecting: false,
            view: Rect::default(),
            view_top: 0,
            copied: None,
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
        if self.answering
            || !typed.starts_with('/')
            || typed.contains(char::is_whitespace)
            || self.palette_off.as_deref() == Some(typed)
        {
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
                duet_git::Git::locate()
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

    /// The conversation's rows at `width`: the welcome on an empty
    /// session, the cells, and what runs now while duet works.
    pub(crate) fn transcript(&mut self, width: usize, now: Instant) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        if self.cells.is_empty() && self.mode != Mode::Working {
            rows.extend(view::welcome(self));
        }
        rows.extend(self.cells.rows(width, self.details));
        if self.mode == Mode::Working
            && let Some(text) = self.feed.status_text(now)
        {
            rows.push(Line::default());
            rows.extend(view::working_row(self, &text, width, now));
        }
        rows
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
        let rows = self.transcript(self.width, Instant::now());
        let mut out = Vec::new();
        for (r, row) in rows.iter().enumerate().take(b.0 + 1).skip(a.0) {
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
            Msg::Attach {
                ws,
                run_dir,
                audit,
                policy,
            } => {
                let held_policy = (*policy).clone();
                self.cells.set_held(Arc::new(move |p: &str| {
                    held_policy.is_sensitive_path(std::path::Path::new(p))
                }));
                self.ws = Some(ws.clone());
                self.panel.attach(ws, run_dir, audit, *policy);
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
        if self.overlay {
            if let Event::Key(k) = ev
                && k.kind != KeyEventKind::Release
            {
                self.overlay_key(k);
            }
            return;
        }
        let Event::Key(k) = ev else {
            match ev {
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollUp => self.scroll_by(3),
                    MouseEventKind::ScrollDown => self.scroll_by(-3),
                    MouseEventKind::Down(MouseButton::Left) => {
                        self.sel = self.at(m.column, m.row, false).map(|p| (p, p));
                        self.selecting = self.sel.is_some();
                    }
                    MouseEventKind::Drag(MouseButton::Left) if self.selecting => {
                        // Past the edge the conversation scrolls on.
                        if m.row < self.view.y {
                            self.scroll_by(1);
                        } else if m.row >= self.view.bottom() {
                            self.scroll_by(-1);
                        }
                        if let (Some((a, _)), Some(p)) = (self.sel, self.at(m.column, m.row, true))
                        {
                            self.sel = Some((a, p));
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) if self.selecting => {
                        self.selecting = false;
                        match self.selected_text() {
                            Some(text) if self.sel.is_some_and(|(a, b)| a != b) => {
                                let n = text.chars().count();
                                terminal::copy(&text, tty);
                                self.copied = Some((Instant::now(), n));
                            }
                            _ => self.sel = None,
                        }
                    }
                    _ => {}
                },
                Event::Paste(text) => self.editor.insert(&text),
                _ => {}
            }
            return;
        };
        if k.kind == KeyEventKind::Release {
            return;
        }
        // A key ends a selection (it stays copied).
        self.sel = None;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
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
        if self.answering {
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
        if k.code != KeyCode::Tab {
            self.choices.clear();
        }
        // The palette and the `@` picker take the arrows and Tab/Enter.
        let palette = self.palette_items();
        if !palette.is_empty() {
            let n = palette.len();
            match k.code {
                KeyCode::Up => {
                    self.palette = (self.palette + n - 1) % n;
                    return;
                }
                KeyCode::Down => {
                    self.palette = (self.palette + 1) % n;
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
                    let typed = self.at_word().map_or(0, |w| w.chars().count());
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
            KeyCode::F(1) => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::F(2) => self.open_settings(None),
            KeyCode::Char('o' | 'O') if ctrl => self.details = !self.details,
            KeyCode::Char('t' | 'T') if ctrl => self.panel.cycle(),
            KeyCode::Up if ctrl => self.panel.select(-1),
            KeyCode::Down if ctrl => self.panel.select(1),
            KeyCode::End if ctrl => self.scroll = 0,
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
            Outcome::Submit(text) => {
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
    use unicode_width::UnicodeWidthChar;
    let mut col = 0;
    let mut out = String::new();
    for c in text.chars() {
        if col >= from && col < to {
            out.push(c);
        }
        col += c.width().unwrap_or(0);
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
                    s.event(ev, tty);
                    if std::mem::take(&mut s.redraw) {
                        // Back from Ctrl-Z (or Ctrl-L): the screen is drawn whole.
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
        let answering = (s.hooks.answering)();
        if answering != s.answering {
            s.answering = answering;
            s.approve = false;
            s.dirty = true;
        }
        if s.mode == Mode::Working {
            // The files duet writes show as it writes them.
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
        let ticking = s.mode == Mode::Working || s.mode == Mode::Hidden;
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
    use duet_boundary::model::{Item, ToolCall};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyEvent;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn hooks(answering: Arc<AtomicBool>, sent: Arc<Mutex<Vec<String>>>) -> Hooks {
        Hooks {
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
            class: duet_boundary::view::ViewClass::Tokenized,
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
    fn the_layout_is_a_header_the_conversation_the_input_and_a_status_line() {
        let (mut s, _, _) = state();
        s.message(Msg::Status(status()));
        s.message(Msg::Mode(Mode::Prompt));
        let out = screen(&mut s, 100, 24);
        let rows: Vec<&str> = out.lines().collect();
        assert!(rows[0].contains("DUET  /"), "{out}");
        assert!(
            rows[1].contains("hybrid · frontier glm-5.3-flash · local omlx-coding"),
            "{out}"
        );
        assert!(
            rows[1].contains("sensitive values stay on this machine"),
            "{out}"
        );
        assert!(
            out.contains("What are we working on?"),
            "the welcome: {out}"
        );
        assert!(out.contains(" Message duet "), "{out}");
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
            out.contains("● read  src/lib.rs  ◦ values replaced by placeholders"),
            "{out}"
        );
        assert!(
            out.contains("│ key = ⟨secret#1⟩"),
            "what the frontier saw: {out}"
        );
        assert!(out.contains("more lines · Ctrl-O"), "folded: {out}");
        assert!(out.contains("edit  src/lib.rs  ◦ +2 −1"), "{out}");
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
            out.contains("◆ duet") && out.contains("in lib.rs; one test still fails."),
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
            out.contains("write  .env  ◦ sensitive: content not shown"),
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
            out.contains("◆ duet") && out.contains("Half of the"),
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
        assert!(out.contains("commands · 1/"), "{out}");
        assert!(
            out.contains("/status") && out.contains("turns, tokens, cost"),
            "{out}"
        );
        assert!(out.contains("/settings"), "{out}");
        // Enter runs the selected command.
        s.event(key(KeyCode::Enter), &mut tty);
        assert_eq!(*sent.lock().unwrap(), ["/status"]);
        // Tab puts a command that takes an argument into the input.
        type_text(&mut s, "/ima", &mut tty);
        s.event(key(KeyCode::Tab), &mut tty);
        assert_eq!(s.editor.buffer(), "/image ");
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
        assert!(screen(&mut s, 130, 24).contains("nothing withheld so far"));
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
    fn the_privacy_view_counts_what_the_boundary_withheld() {
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
        assert!(out.contains("1 result(s): values replaced by"), "{out}");
        assert!(out.contains("turn 3: 2 values replaced"), "{out}");
        assert!(
            out.contains("◦ withheld from the frontier"),
            "the cell too: {out}"
        );
    }

    #[test]
    fn settings_open_over_the_conversation_and_esc_comes_back() {
        let (mut s, _, sent) = state();
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join(".duet")).unwrap();
        s.settings = Some(Settings {
            paths: crate::Paths {
                owner: d.path().join("owner.toml"),
                project: ws.join(".duet/config.toml"),
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
            out.contains("duet settings") && out.contains("runs: none yet"),
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
            out.contains("keys and commands") && out.contains("Ctrl-T"),
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
