// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet chat` on a terminal: one thread owns the screen and the keyboard.
//!
//! Output (progress, duet's text as it streams, command results) is printed
//! into the scrollback; below it a live region holds the row of duet's text
//! being written, a status line while duet works, and the input line, which
//! is editable at all times (typing while duet works steers it). The region
//! is redrawn in place and never enters the scrollback.
//!
//! The keyboard is read raw, so Ctrl-C arrives as a key: it is handed to
//! the same interrupt rules as the signal (it never just edits the line).
//! Output processing stays on, so a line printed by other code still starts
//! at the left edge. Nothing is stored: history comes from the session's
//! transcript and lives in memory.

use super::editor::{Editor, Outcome};
use super::feed::{Feed, Restore, Streamed, tint};
use super::{Region, Span, Style, colour, frame, safe, styled};
use crossterm::event::{self, Event, KeyEventKind, KeyboardEnhancementFlags};
use duet_agent::TurnEnd;
use duet_agent::transcript::Entry;
use duet_boundary::live::{StreamEvent, StreamTap};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

/// What the chat side is doing, for the live region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Nothing drawn (setup prints freely); keys are still read.
    Hidden,
    /// Waiting for the operator's next message.
    Prompt,
    /// A turn runs: the status line shows, and a message steers it.
    Working,
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
    /// Draws what is pending, then acknowledges.
    Sync(Sender<()>),
    Remember(Vec<String>),
    Restore(Restore),
    Stop(Sender<()>),
}

/// What the console reports to the chat.
pub(crate) struct Hooks {
    /// A line the operator sent (continuation lines joined).
    pub line: Box<dyn Fn(String) + Send>,
    /// Ctrl-D on an empty input.
    pub eof: Box<dyn Fn() + Send>,
    /// Ctrl-C: what to tell the operator, if anything.
    pub interrupt: Box<dyn Fn() -> Option<String> + Send>,
    /// Whether an approval question waits for the next line.
    pub answering: Box<dyn Fn() -> bool + Send>,
}

pub(crate) struct Console {
    tx: Sender<Msg>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The terminal state the console changed, restored on exit and on panic.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static KEYBOARD: AtomicBool = AtomicBool::new(false);

fn restore_terminal() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = std::io::stdout();
    if KEYBOARD.swap(false, Ordering::SeqCst) {
        let _ = crossterm::execute!(out, event::PopKeyboardEnhancementFlags);
    }
    let _ = write!(out, "\x1b[?2004l\x1b[?25h");
    let _ = out.flush();
    let _ = crossterm::terminal::disable_raw_mode();
}

/// Raw keyboard input (no line editing, echo or signals from keys) with the
/// terminal's output processing left on.
fn enter_terminal() -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let stdin = std::io::stdin();
    if let Ok(mut t) = rustix::termios::tcgetattr(&stdin) {
        t.output_modes |= rustix::termios::OutputModes::OPOST | rustix::termios::OutputModes::ONLCR;
        let _ = rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &t);
    }
    let mut out = std::io::stdout();
    // Shift-Enter can only be told from Enter where the terminal reports
    // keys unambiguously.
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && crossterm::execute!(
            out,
            event::PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        )
        .is_ok()
    {
        KEYBOARD.store(true, Ordering::SeqCst);
    }
    write!(out, "\x1b[?2004h")?;
    out.flush()
}

impl Console {
    /// Takes over the terminal. The chat learns what the operator does
    /// through `hooks`.
    pub fn start(hooks: Hooks) -> std::io::Result<Arc<Console>> {
        static HOOK: Once = Once::new();
        HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                previous(info);
            }));
        });
        enter_terminal()?;
        let (columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let colour = colour(true);
        let (tx, rx) = mpsc::channel();
        let state = Live {
            colour,
            columns: columns.max(20) as usize,
            rows: rows.max(4) as usize,
            feed: Feed::new(
                colour,
                columns.max(20) as usize,
                true,
                Arc::new(|t: &str| t.to_owned()),
            ),
            editor: Editor::default(),
            region: Region::default(),
            mode: Mode::Hidden,
            pending: Vec::new(),
            dirty: true,
            answering: false,
            drawn_at: Instant::now(),
            hooks,
        };
        let thread = std::thread::Builder::new()
            .name("duet-console".into())
            .spawn(move || {
                let mut state = state;
                let run = std::panic::AssertUnwindSafe(|| run(&mut state, &rx));
                if std::panic::catch_unwind(run).is_err() {
                    // The terminal is given back and the session is left
                    // open (the input ended), so no work is lost.
                    restore_terminal();
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
        Ok(Arc::new(Console {
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
        if mode == Mode::Hidden {
            // Other code prints next: the region is gone first.
            let (ack, done) = mpsc::channel();
            self.send(Msg::Sync(ack));
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
    }

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

    /// Gives the terminal back as it was (idempotent). Everything sent
    /// before is shown first.
    pub fn stop(&self) {
        let (ack, done) = mpsc::channel();
        if self.tx.send(Msg::Stop(ack)).is_ok() {
            let _ = done.recv_timeout(Duration::from_secs(5));
        }
        if let Some(t) = self.thread.lock().ok().and_then(|mut t| t.take()) {
            let _ = t.join();
        }
        restore_terminal();
    }
}

impl Drop for Console {
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

/// The console thread's state.
struct Live {
    colour: bool,
    columns: usize,
    rows: usize,
    feed: Feed,
    editor: Editor,
    region: Region,
    mode: Mode,
    /// Lines to print above the region at the next frame.
    pending: Vec<String>,
    dirty: bool,
    answering: bool,
    drawn_at: Instant,
    hooks: Hooks,
}

/// How often the status line moves while duet works.
const TICK: Duration = Duration::from_millis(250);

fn run(s: &mut Live, rx: &Receiver<Msg>) {
    loop {
        loop {
            match rx.try_recv() {
                Ok(Msg::Stop(ack)) => {
                    s.close();
                    let _ = ack.send(());
                    return;
                }
                Ok(Msg::Sync(ack)) => {
                    s.render(Instant::now());
                    let _ = ack.send(());
                }
                Ok(m) => s.message(m),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    s.close();
                    return;
                }
            }
        }
        match event::poll(Duration::from_millis(30)) {
            Ok(true) => {
                while let Ok(ev) = event::read() {
                    s.event(ev);
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
        s.render(Instant::now());
    }
}

impl Live {
    fn message(&mut self, m: Msg) {
        match m {
            Msg::Lines(lines) => {
                let colour = self.colour;
                self.pending.extend(lines.iter().map(|l| tint(l, colour)));
            }
            Msg::Text(text) => self.pending.extend(safe(&text).lines().map(str::to_owned)),
            Msg::Diff(text) => {
                let colour = self.colour;
                self.pending
                    .extend(safe(&text).lines().map(|l| diff_line(l, colour)));
            }
            Msg::Entry(e) => {
                let lines = self.feed.entry(&e);
                self.pending.extend(lines);
            }
            Msg::Stream(ev) => {
                let lines = self.feed.stream(&ev);
                self.pending.extend(lines);
                self.dirty = true;
            }
            Msg::Begin => {
                self.feed.begin(Instant::now());
                self.mode = Mode::Working;
                self.dirty = true;
            }
            Msg::End(end) => {
                let lines = self.feed.end(&end);
                self.pending.extend(lines);
                self.dirty = true;
            }
            Msg::Mode(mode) => {
                self.mode = mode;
                self.dirty = true;
            }
            Msg::Remember(entries) => self.editor.remember(entries),
            Msg::Restore(restore) => self.feed.set_restore(restore),
            Msg::Stop(_) | Msg::Sync(_) => {}
        }
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                let outcome = self.editor.key(k);
                self.outcome(outcome);
            }
            Event::Paste(text) => self.editor.insert(&text),
            Event::Resize(columns, rows) => {
                let mut out = String::new();
                self.region.resized(columns as usize);
                self.region.clear(&mut out);
                self.write(&out);
                self.columns = (columns as usize).max(20);
                self.rows = (rows as usize).max(4);
                self.feed.resize(self.columns);
            }
            _ => return,
        }
        self.dirty = true;
    }

    fn outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Edited => {}
            Outcome::Submit(text) => {
                if !text.trim().is_empty() {
                    let mut lines = text.lines();
                    if let Some(first) = lines.next() {
                        self.pending
                            .push(tint(&format!("you> {first}"), self.colour));
                    }
                    self.pending
                        .extend(lines.map(|l| format!("     {}", safe(l))));
                }
                (self.hooks.line)(text);
            }
            Outcome::Eof => (self.hooks.eof)(),
            Outcome::Interrupt => {
                if let Some(notice) = (self.hooks.interrupt)() {
                    self.pending.push(styled(&notice, Style::WARN, self.colour));
                }
            }
            Outcome::Complete => {
                let cwd = std::env::current_dir().unwrap_or_default();
                let choices = self.editor.complete(&cwd, crate::chat::COMMANDS);
                if !choices.is_empty() {
                    let listed = safe(&choices.join("   "));
                    self.pending.push(styled(&listed, Style::DIM, self.colour));
                }
            }
            Outcome::ClearScreen => {
                self.region = Region::default();
                self.write("\x1b[2J\x1b[H");
            }
            Outcome::Suspend => self.suspend(),
        }
    }

    /// Ctrl-Z: the terminal is given back and the job stops, as the shell
    /// expects; it is taken again when the job continues.
    fn suspend(&mut self) {
        let mut out = String::new();
        self.region.clear(&mut out);
        self.write(&out);
        restore_terminal();
        let _ = rustix::process::kill_current_process_group(rustix::process::Signal::Tstp);
        let _ = enter_terminal();
        self.dirty = true;
    }

    fn write(&self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }

    /// The live region's rows and where the cursor goes.
    fn region_rows(&self, now: Instant) -> (Vec<String>, Option<(usize, usize)>) {
        if self.mode == Mode::Hidden {
            return (Vec::new(), None);
        }
        let mut rows = Vec::new();
        if let Some(p) = self.feed.partial() {
            rows.push(p);
        }
        if self.mode == Mode::Working
            && let Some(status) = self
                .feed
                .status(now, "type to steer · /stop after this step · Ctrl-C now")
        {
            rows.push(status);
        }
        let prompt = if self.answering {
            vec![Span::new("approve? y/n> ", Style::WARN.bold())]
        } else {
            vec![Span::new("you> ", Style::PLAIN.bold())]
        };
        let room = self.rows.saturating_sub(rows.len() + 1).max(1);
        let (input, (r, c)) = self
            .editor
            .display(&prompt, self.colour, self.columns, room);
        let cursor = (rows.len() + r, c);
        rows.extend(input);
        (rows, Some(cursor))
    }

    fn render(&mut self, now: Instant) {
        let answering = (self.hooks.answering)();
        if answering != self.answering {
            self.answering = answering;
            self.dirty = true;
        }
        let ticking = self.mode == Mode::Working && self.feed.working();
        if self.pending.is_empty()
            && !self.dirty
            && !(ticking && now.duration_since(self.drawn_at) >= TICK)
        {
            return;
        }
        let (rows, cursor) = self.region_rows(now);
        let mut body = String::new();
        if self.pending.is_empty() {
            self.region.clear(&mut body);
            self.region.draw(&mut body, &rows, cursor);
        } else {
            let lines = std::mem::take(&mut self.pending);
            self.region.print_above(&mut body, &lines, &rows, cursor);
        }
        self.write(&frame(&body, cursor.is_some() || self.mode == Mode::Hidden));
        self.dirty = false;
        self.drawn_at = now;
    }

    /// The region is cleared and whatever is pending is printed.
    fn close(&mut self) {
        let mut body = String::new();
        let lines = std::mem::take(&mut self.pending);
        self.region.print_above(&mut body, &lines, &[], None);
        self.write(&frame(&body, true));
    }
}

/// A line of `/diff`, coloured by what it is.
fn diff_line(line: &str, colour: bool) -> String {
    let style = if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
        Style::PLAIN.bold()
    } else if line.starts_with('+') {
        Style::GOOD
    } else if line.starts_with('-') {
        Style::BAD
    } else if line.starts_with("@@") {
        Style::ACCENT
    } else if line.starts_with("new file") || line.starts_with("changed ") {
        Style::DIM
    } else {
        Style::PLAIN
    };
    styled(line, style, colour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lines_are_coloured_by_kind() {
        assert_eq!(diff_line("+pub mod b;", true), "\x1b[32m+pub mod b;\x1b[0m");
        assert_eq!(diff_line("-old", true), "\x1b[31m-old\x1b[0m");
        assert_eq!(
            diff_line("+++ b/src/lib.rs", true),
            "\x1b[1m+++ b/src/lib.rs\x1b[0m"
        );
        assert_eq!(diff_line("+pub mod b;", false), "+pub mod b;");
    }
}
