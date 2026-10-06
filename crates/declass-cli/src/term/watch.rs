// SPDX-License-Identifier: GPL-3.0-or-later
//! `declass run`'s progress on standard error, so a long run never looks hung.
//! Standard output keeps only the final summary.
//!
//! On a terminal: tool steps as they happen, the frontier's text as it
//! streams (placeholders restored for the operator) and a status line
//! (elapsed time, cost, output received so far, what runs now), redrawn in
//! place at the bottom. Otherwise (a log, a pipe): one compact line per
//! step, with placeholders kept as the frontier saw them (a log is more
//! likely to be shared than a screen), no streamed text, and a heartbeat
//! line when nothing was printed for a while.

use super::feed::{Feed, Restore, Streamed, tint};
use super::{Region, Style, colour, frame, styled};
use declass_agent::transcript::Entry;
use declass_boundary::live::{StreamEvent, StreamTap};
use std::io::Write;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Msg {
    Entry(Box<Entry>),
    Stream(Streamed),
    Note(String),
    Restore(Restore),
    /// Stop drawing until [`Msg::Release`] (a question is asked on the
    /// terminal); acknowledged once the region is cleared.
    Hold(Sender<()>),
    Release,
    Stop(Sender<()>),
}

/// The progress view of one run.
pub(crate) struct Watch {
    tx: Sender<Msg>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// How often the live status line moves.
const TICK: Duration = Duration::from_millis(250);
/// A log gets a heartbeat line after this long without one.
const HEARTBEAT: Duration = Duration::from_secs(30);

impl Watch {
    /// Starts the view: live when standard error is a terminal that can
    /// take it, else the compact log. The status line waits for
    /// [`Watch::release`] (setup prints freely before).
    pub fn start(live: bool) -> Arc<Watch> {
        let (columns, _) = crossterm::terminal::size().unwrap_or((100, 24));
        let colour = live && colour(true);
        let identity: Restore = Arc::new(|t: &str| t.to_owned());
        let (tx, rx) = mpsc::channel();
        let view = View {
            live,
            colour,
            columns: columns as usize,
            feed: Feed::new(colour, (columns as usize).max(20), live, identity),
            region: Region::default(),
            held: true,
            printed_at: Instant::now(),
            drawn_at: Instant::now(),
        };
        let thread = std::thread::Builder::new()
            .name("declass-progress".into())
            .spawn(move || view.run(rx))
            .ok();
        Arc::new(Watch {
            tx,
            thread: Mutex::new(thread),
        })
    }

    pub fn tap(&self) -> Arc<dyn StreamTap> {
        Arc::new(Tap(self.tx.clone()))
    }

    pub fn entry(&self, entry: Entry) {
        let _ = self.tx.send(Msg::Entry(Box::new(entry)));
    }

    /// A line of declass's own (an interrupt notice).
    pub fn note(&self, text: &str) {
        let _ = self.tx.send(Msg::Note(text.to_owned()));
    }

    /// Placeholders restored for the operator on a terminal (the log keeps
    /// them).
    pub fn restore(&self, restore: Restore) {
        let _ = self.tx.send(Msg::Restore(restore));
    }

    /// Clears the status and holds it until [`Watch::release`], for a
    /// question asked on the same terminal.
    pub fn hold(&self) {
        let (ack, done) = mpsc::channel();
        if self.tx.send(Msg::Hold(ack)).is_ok() {
            let _ = done.recv_timeout(Duration::from_secs(2));
        }
    }

    pub fn release(&self) {
        let _ = self.tx.send(Msg::Release);
    }

    /// Ends the view: what is open is shown and the status line cleared.
    pub fn stop(&self) {
        let (ack, done) = mpsc::channel();
        if self.tx.send(Msg::Stop(ack)).is_ok() {
            let _ = done.recv_timeout(Duration::from_secs(5));
        }
        if let Some(t) = self.thread.lock().ok().and_then(|mut t| t.take()) {
            let _ = t.join();
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `approver` with the status line paused while it asks.
pub(crate) fn holding(
    approver: Arc<dyn declass_agent::Approver>,
    watch: Arc<Watch>,
) -> Arc<dyn declass_agent::Approver> {
    Arc::new(Holding { approver, watch })
}

struct Holding {
    approver: Arc<dyn declass_agent::Approver>,
    watch: Arc<Watch>,
}

impl declass_agent::Approver for Holding {
    fn approve(&self, action: &declass_agent::oversight::Action) -> bool {
        self.watch.hold();
        let yes = self.approver.approve(action);
        self.watch.release();
        yes
    }
}

struct Tap(Sender<Msg>);

impl StreamTap for Tap {
    fn event(&self, event: StreamEvent<'_>) {
        let _ = self.0.send(Msg::Stream(event.into()));
    }
}

struct View {
    live: bool,
    colour: bool,
    columns: usize,
    feed: Feed,
    region: Region,
    held: bool,
    printed_at: Instant,
    drawn_at: Instant,
}

impl View {
    fn run(mut self, rx: Receiver<Msg>) {
        self.feed.begin(Instant::now());
        loop {
            let mut lines = Vec::new();
            let mut changed = false;
            match rx.recv_timeout(TICK) {
                Ok(m) => {
                    let mut next = Some(m);
                    while let Some(m) = next.take() {
                        match m {
                            Msg::Stop(ack) => {
                                lines.extend(self.feed.finish());
                                self.show(&lines, false);
                                let _ = ack.send(());
                                return;
                            }
                            Msg::Hold(ack) => {
                                self.show(&lines, false);
                                lines.clear();
                                self.held = true;
                                let _ = ack.send(());
                            }
                            m => changed |= self.message(m, &mut lines),
                        }
                        next = rx.try_recv().ok();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            if !self.live
                && self.feed.working()
                && now.duration_since(self.printed_at) >= HEARTBEAT
                && let Some(status) = self.feed.status_text(now)
            {
                lines.push(format!("  … {status}"));
            }
            let tick = self.live && now.duration_since(self.drawn_at) >= TICK;
            if !lines.is_empty() || changed || tick {
                self.show(&lines, !self.held);
            }
        }
    }

    /// Applies a message; returns whether the live rows changed.
    fn message(&mut self, m: Msg, lines: &mut Vec<String>) -> bool {
        match m {
            Msg::Entry(e) => lines.extend(self.feed.entry(&e)),
            Msg::Stream(ev) => {
                lines.extend(self.feed.stream(&ev));
                return true;
            }
            Msg::Note(text) => lines.push(styled(&text, Style::WARN, self.colour)),
            Msg::Restore(r) => {
                if self.live {
                    self.feed.set_restore(r);
                }
            }
            Msg::Release => {
                self.held = false;
                return true;
            }
            Msg::Hold(_) | Msg::Stop(_) => {}
        }
        false
    }

    /// Prints `lines` (and, live, redraws the region below them).
    fn show(&mut self, lines: &[String], region: bool) {
        let now = Instant::now();
        if !lines.is_empty() {
            self.printed_at = now;
        }
        let mut err = std::io::stderr().lock();
        if !self.live {
            for l in lines {
                let _ = writeln!(err, "{}", tint(l, false));
            }
            return;
        }
        // A resized terminal has re-wrapped the status line.
        if let Ok((columns, _)) = crossterm::terminal::size()
            && columns as usize != self.columns
        {
            self.columns = columns as usize;
            self.region.resized(self.columns);
            self.feed.resize(self.columns.max(20));
        }
        let mut rows = Vec::new();
        if region {
            rows.extend(self.feed.partial());
            rows.extend(self.feed.status(now, "Ctrl-C stops the run"));
        }
        let mut body = String::new();
        self.region.print_above(&mut body, lines, &rows, None);
        let _ = err.write_all(frame(&body, rows.is_empty()).as_bytes());
        let _ = err.flush();
        self.drawn_at = now;
    }
}
