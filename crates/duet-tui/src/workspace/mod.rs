// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace: `duet` on a terminal, full screen.
//!
//! A status bar (mode, models, turns, cost against the session budget), the
//! conversation (the operator's messages, duet's progress and what the
//! boundary withheld, duet's replies streamed as Markdown, how each turn
//! ended), the input box, and a key line. The session itself runs in the CLI
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
mod panel;
mod terminal;
mod view;

use crate::term::editor::{Editor, Outcome};
use crate::term::feed::{Feed, Restore, Streamed, tint};
use crate::term::{Style as TStyle, colour, safe, styled};
use duet_agent::TurnEnd;
use duet_agent::transcript::Entry;
use duet_boundary::live::{StreamEvent, StreamTap};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::text::{Line, Span};
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
    /// `hybrid`, `local-only` or `passthrough`.
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
    pub fn start(hooks: Hooks, commands: &'static [&'static str]) -> std::io::Result<Arc<Self>> {
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
        let state = State::new(colour, hooks, commands);
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

/// Lines the conversation keeps; older ones are dropped.
const KEEP: usize = 50_000;

/// How often the status line moves while duet works.
const TICK: Duration = Duration::from_millis(250);

/// The workspace thread's state.
pub(crate) struct State {
    colour: bool,
    feed: Feed,
    editor: Editor,
    mode: Mode,
    answering: bool,
    status: Status,
    hooks: Hooks,
    commands: &'static [&'static str],
    /// The conversation, as logical lines (each with a lead its every row
    /// starts with) and as rows wrapped at `width`.
    lines: Vec<(Line<'static>, Option<Span<'static>>)>,
    rows: Vec<Line<'static>>,
    width: usize,
    /// Rows scrolled up from the bottom (0: the view follows new output).
    scroll: usize,
    /// Rows the conversation shows (set when drawn), for paging.
    page: usize,
    panel: panel::Panel,
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
            editor: Editor::default(),
            mode: Mode::Hidden,
            answering: false,
            status: Status::default(),
            hooks,
            commands,
            lines: Vec::new(),
            rows: Vec::new(),
            width: 80,
            scroll: 0,
            page: 20,
            panel: panel::Panel::default(),
            dirty: true,
            redraw: false,
            drawn_at: Instant::now(),
        }
    }

    /// Adds lines with duet's styles to the conversation.
    fn push(&mut self, lines: impl IntoIterator<Item = String>) {
        self.push_led(lines.into_iter().map(|l| (ansi::line(&l), None)));
    }

    /// Adds lines whose every row starts with its lead.
    fn push_led(
        &mut self,
        lines: impl IntoIterator<Item = (Line<'static>, Option<Span<'static>>)>,
    ) {
        let mut added = 0;
        for l in lines {
            let rows = rows_of(&l, self.width);
            added += rows.len();
            self.rows.extend(rows);
            self.lines.push(l);
        }
        if self.lines.len() > KEEP {
            let drop = self.lines.len() - KEEP;
            self.lines.drain(..drop);
            self.rewrap();
        }
        if self.scroll > 0 {
            // Scrolled up: the view stays where it is.
            self.scroll += added;
        }
        self.dirty = true;
    }

    fn rewrap(&mut self) {
        self.rows = self
            .lines
            .iter()
            .flat_map(|l| rows_of(l, self.width))
            .collect();
        self.scroll = self.scroll.min(self.rows.len());
    }

    /// The conversation is `width` columns wide now.
    fn set_width(&mut self, width: usize) {
        let width = width.max(10);
        if width != self.width {
            self.width = width;
            self.feed.resize(width);
            self.rewrap();
        }
    }

    fn scroll_by(&mut self, up: isize) {
        let max = self.rows.len().saturating_sub(1);
        self.scroll = if up >= 0 {
            (self.scroll + up.unsigned_abs()).min(max)
        } else {
            self.scroll.saturating_sub(up.unsigned_abs())
        };
        self.dirty = true;
    }

    fn message(&mut self, m: Msg) {
        match m {
            Msg::Lines(lines) => {
                let colour = self.colour;
                self.push(lines.iter().map(|l| tint(l, colour)).collect::<Vec<_>>());
            }
            Msg::Text(text) => {
                self.push(safe(&text).lines().map(str::to_owned).collect::<Vec<_>>())
            }
            Msg::Diff(text) => {
                let colour = self.colour;
                self.push(
                    safe(&text)
                        .lines()
                        .map(|l| diff_line(l, colour))
                        .collect::<Vec<_>>(),
                );
            }
            Msg::Entry(e) => {
                self.panel.entry(&e);
                let lines = self.feed.entry(&e);
                self.push(lines);
            }
            Msg::Stream(ev) => {
                let lines = self.feed.stream(&ev);
                self.push(lines);
            }
            Msg::Begin => {
                self.feed.begin(Instant::now());
                self.mode = Mode::Working;
            }
            Msg::End(end) => {
                let lines = self.feed.end(&end);
                self.push(lines);
                self.panel.refresh(true);
            }
            Msg::Mode(mode) => self.mode = mode,
            Msg::Status(s) => self.status = s,
            Msg::Attach {
                ws,
                run_dir,
                audit,
                policy,
            } => self.panel.attach(ws, run_dir, audit, *policy),
            Msg::Output(line) => {
                let line = safe(&line);
                if !line.trim().is_empty() {
                    // Every row of it marked as printed elsewhere.
                    let dim =
                        ratatui::style::Style::new().add_modifier(ratatui::style::Modifier::DIM);
                    self.push_led([(
                        Line::from(Span::styled(line, dim)),
                        Some(Span::styled("│ ", dim)),
                    )]);
                }
            }
            Msg::Remember(entries) => self.editor.remember(entries),
            Msg::Restore(restore) => self.feed.set_restore(restore),
            Msg::Sync(_) | Msg::Stop(_) => {}
        }
        self.dirty = true;
    }

    fn event(&mut self, ev: Event, tty: &mut File) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                // The side panel.
                KeyCode::Char('t' | 'T') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.panel.cycle();
                }
                KeyCode::Up if k.modifiers.contains(KeyModifiers::CONTROL) => self.panel.select(-1),
                KeyCode::Down if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.panel.select(1)
                }
                KeyCode::PageUp if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.panel.scroll_diff(-(self.page as isize));
                }
                KeyCode::PageDown if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.panel.scroll_diff(self.page as isize);
                }
                KeyCode::PageUp => self.scroll_by(self.page.saturating_sub(2).max(1) as isize),
                KeyCode::PageDown => {
                    self.scroll_by(-(self.page.saturating_sub(2).max(1) as isize));
                }
                _ => {
                    let outcome = self.editor.key(k);
                    self.outcome(outcome, tty);
                }
            },
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp => self.scroll_by(3),
                MouseEventKind::ScrollDown => self.scroll_by(-3),
                _ => return,
            },
            Event::Paste(text) => self.editor.insert(&text),
            Event::Resize(..) => {}
            _ => return,
        }
        self.dirty = true;
    }

    fn outcome(&mut self, outcome: Outcome, tty: &mut File) {
        match outcome {
            Outcome::Edited => {}
            Outcome::Submit(text) => {
                if !text.trim().is_empty() {
                    let mut lines = text.lines();
                    let mut shown = Vec::new();
                    if let Some(first) = lines.next() {
                        shown.push(tint(&format!("you> {first}"), self.colour));
                    }
                    shown.extend(lines.map(|l| format!("     {}", safe(l))));
                    self.push(shown);
                }
                self.scroll = 0;
                (self.hooks.line)(text);
            }
            Outcome::Eof => (self.hooks.eof)(),
            Outcome::Interrupt => {
                if let Some(notice) = (self.hooks.interrupt)() {
                    let line = styled(&notice, TStyle::WARN, self.colour);
                    self.push([line]);
                }
            }
            Outcome::Complete => {
                let cwd = std::env::current_dir().unwrap_or_default();
                let choices = self.editor.complete(&cwd, self.commands);
                if !choices.is_empty() {
                    let listed = styled(&safe(&choices.join("   ")), TStyle::DIM, self.colour);
                    self.push([listed]);
                }
            }
            // The screen is redrawn whole; the view goes to the bottom.
            Outcome::ClearScreen => self.scroll = 0,
            Outcome::Suspend => {
                terminal::suspend(tty);
                self.redraw = true;
            }
        }
    }
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
                        // Back from Ctrl-Z: the screen holds nothing of ours.
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
            s.dirty = true;
        }
        if s.mode == Mode::Working {
            // The files duet writes show as it writes them.
            s.panel.refresh(false);
        }
        let now = Instant::now();
        let ticking = s.mode == Mode::Working && s.feed.working();
        if s.dirty || (ticking && now.duration_since(s.drawn_at) >= TICK) {
            let _ = screen.draw(|f| view::draw(f, s, now));
            s.dirty = false;
            s.drawn_at = now;
        }
    }
}

/// A logical line's rows at `width`, each starting with its lead.
fn rows_of(
    (line, lead): &(Line<'static>, Option<Span<'static>>),
    width: usize,
) -> Vec<Line<'static>> {
    match lead {
        None => ansi::wrap(line, width),
        Some(lead) => {
            let room = width.saturating_sub(lead.width()).max(1);
            ansi::wrap(line, room)
                .into_iter()
                .map(|mut row| {
                    row.spans.insert(0, lead.clone());
                    row
                })
                .collect()
        }
    }
}

/// A line of `/diff`, coloured by what it is.
fn diff_line(line: &str, colour: bool) -> String {
    let style = if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
        TStyle::PLAIN.bold()
    } else if line.starts_with('+') {
        TStyle::GOOD
    } else if line.starts_with('-') {
        TStyle::BAD
    } else if line.starts_with("@@") {
        TStyle::ACCENT
    } else if line.starts_with("new file") || line.starts_with("changed ") {
        TStyle::DIM
    } else {
        TStyle::PLAIN
    };
    styled(line, style, colour)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
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
            &["/help", "/stop"],
        );
        (s, answering, sent)
    }

    fn screen(s: &mut State, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| view::draw(f, s, Instant::now())).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect())
            .collect()
    }

    #[test]
    fn the_layout_has_a_status_bar_the_conversation_the_input_and_keys() {
        let (mut s, _, _) = state();
        s.message(Msg::Status(Status {
            session: "20260926-1".into(),
            mode: "hybrid".into(),
            frontier: "glm-5.3-flash".into(),
            local: Some("omlx-coding".into()),
            turns: 2,
            cost_usd: 0.0123,
            budget_usd: 5.0,
            ..Status::default()
        }));
        s.message(Msg::Mode(Mode::Prompt));
        s.message(Msg::Lines(vec!["you> add hex literals".into()]));
        let rows = screen(&mut s, 80, 16);
        assert!(
            rows[0].contains("duet · hybrid · glm-5.3-flash ▸ omlx-coding"),
            "{}",
            rows[0]
        );
        assert!(rows[0].contains("turn 2 · $0.0123 of $5.00"), "{}", rows[0]);
        assert!(rows.iter().any(|r| r.contains("conversation")));
        assert!(rows.iter().any(|r| r.contains("you> add hex literals")));
        assert!(rows.iter().any(|r| r.contains("› ")), "the input prompt");
        assert!(rows[15].contains("⏎ send"), "{}", rows[15]);
    }

    #[test]
    fn an_approval_question_changes_the_input() {
        let (mut s, answering, _) = state();
        s.message(Msg::Mode(Mode::Working));
        answering.store(true, Ordering::SeqCst);
        s.answering = (s.hooks.answering)();
        let rows = screen(&mut s, 80, 16);
        assert!(rows.iter().any(|r| r.contains("approve? y/n ›")));
        assert!(rows.iter().any(|r| r.contains("approval: y or n")));
    }

    #[test]
    fn output_from_elsewhere_and_hostile_text_are_drawn_harmlessly() {
        let (mut s, _, _) = state();
        s.message(Msg::Output(
            "warning: \x1b]52;c;ZXZpbA==\x07clipboard".into(),
        ));
        s.message(Msg::Text("reply \x1b[2Jcleared?".into()));
        let rows = screen(&mut s, 80, 16);
        let all = rows.concat();
        assert!(all.contains("│ warning: clipboard"), "{all}");
        assert!(all.contains("reply cleared?"), "{all}");
        assert!(!all.contains('\x1b'));
    }

    #[test]
    fn the_view_follows_new_output_unless_scrolled_up() {
        let (mut s, _, _) = state();
        let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        s.message(Msg::Lines(lines));
        let rows = screen(&mut s, 80, 16);
        assert!(rows.iter().any(|r| r.contains("line 99")));
        s.scroll_by(20);
        s.message(Msg::Lines(vec!["line 100".into()]));
        let rows = screen(&mut s, 80, 16);
        assert!(!rows.iter().any(|r| r.contains("line 100")), "stays put");
        assert!(rows.iter().any(|r| r.contains("rows below")));
        s.scroll_by(-1000);
        let rows = screen(&mut s, 80, 16);
        assert!(rows.iter().any(|r| r.contains("line 100")));
    }

    #[test]
    fn a_sent_message_reaches_the_session_and_the_conversation() {
        let (mut s, _, sent) = state();
        let mut tty = tempfile::tempfile().unwrap();
        for c in "fix it".chars() {
            s.event(
                Event::Key(ratatui::crossterm::event::KeyEvent::from(KeyCode::Char(c))),
                &mut tty,
            );
        }
        s.event(
            Event::Key(ratatui::crossterm::event::KeyEvent::from(KeyCode::Enter)),
            &mut tty,
        );
        assert_eq!(*sent.lock().unwrap(), ["fix it"]);
        let rows = screen(&mut s, 80, 16);
        assert!(rows.iter().any(|r| r.contains("you> fix it")));
    }

    fn ctrl(c: char) -> Event {
        Event::Key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        ))
    }

    #[test]
    fn the_side_panel_opens_on_a_wide_screen_and_ctrl_t_cycles_it() {
        let (mut s, _, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        // Wide: open by itself, on Changes.
        let rows = screen(&mut s, 130, 20);
        let all = rows.concat();
        assert!(all.contains(" Changes "), "{all}");
        assert!(all.contains("changed files: none yet"), "{all}");
        // Ctrl-T: Privacy, Session, closed, Changes again.
        s.event(ctrl('t'), &mut tty);
        let all = screen(&mut s, 130, 20).concat();
        assert!(all.contains("nothing withheld so far"), "{all}");
        s.event(ctrl('t'), &mut tty);
        let all = screen(&mut s, 130, 20).concat();
        assert!(
            all.contains("the session starts with your first message"),
            "{all}"
        );
        s.event(ctrl('t'), &mut tty);
        let all = screen(&mut s, 130, 20).concat();
        assert!(!all.contains(" Changes "), "closed: {all}");
        // Narrow: closed until asked for (the operator's choice then holds).
        let (mut n, _, _) = state();
        assert!(!screen(&mut n, 100, 20).concat().contains(" Changes "));
        n.event(ctrl('t'), &mut tty);
        assert!(screen(&mut n, 100, 20).concat().contains(" Changes "));
    }

    #[test]
    fn the_privacy_view_counts_what_the_boundary_withheld() {
        let (mut s, _, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        for class in [
            duet_boundary::view::ViewClass::Tokenized,
            duet_boundary::view::ViewClass::Tokenized,
            duet_boundary::view::ViewClass::HandleSummary,
            duet_boundary::view::ViewClass::Raw,
        ] {
            s.message(Msg::Entry(Box::new(Entry::Shown {
                call_id: "c1".into(),
                class,
            })));
        }
        s.message(Msg::Entry(Box::new(Entry::Usage {
            turn: 3,
            usage: Default::default(),
            cost_usd: 0.0,
            interventions: vec!["2 values replaced (EMAIL, CARD)".into()],
        })));
        s.event(ctrl('t'), &mut tty);
        s.event(ctrl('t'), &mut tty);
        let all = screen(&mut s, 130, 24).concat();
        assert!(
            all.contains("2 tool result(s): values replaced by"),
            "{all}"
        );
        assert!(
            all.contains("1 tool result(s): sensitive, held locally"),
            "{all}"
        );
        assert!(
            all.contains("turn 3: 2 values replaced (EMAIL, CARD)"),
            "{all}"
        );
    }

    #[test]
    fn a_small_terminal_gets_a_notice() {
        let (mut s, _, _) = state();
        let rows = screen(&mut s, 30, 8);
        assert!(rows.concat().contains("needs at least 40x10"));
    }
}
