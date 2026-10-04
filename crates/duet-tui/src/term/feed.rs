// SPDX-License-Identifier: GPL-3.0-or-later
//! What the operator sees of duet's work: the transcript's entries become
//! progress lines, the frontier's streamed response becomes duet's text as
//! it arrives, and both keep a status (time, cost, what runs now).
//!
//! The same feed serves the `duet` workspace, `duet run` on a
//! terminal, and `duet run`'s compact log (no streamed text, placeholders
//! kept). Streamed text is shown restored for the operator (values in place
//! of placeholders); the frontier's view is never changed by it.

use super::markdown::Markdown;
use super::stream::{Detok, FieldReader};
use super::{Span, Style, clip_width, safe, styled};
use duet_agent::TurnEnd;
use duet_agent::transcript::Entry;
use duet_boundary::live::StreamEvent;
use duet_boundary::model::Item;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

/// Restores placeholders for the operator (identity without the boundary,
/// or in a log that must keep them).
pub type Restore = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// A stream event that can cross threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Streamed {
    Attempt(u32),
    Text(String),
    Reasoning(usize),
    Call { index: usize, name: String },
    Arguments { index: usize, delta: String },
    End,
}

impl From<StreamEvent<'_>> for Streamed {
    fn from(e: StreamEvent<'_>) -> Self {
        match e {
            StreamEvent::Attempt(n) => Streamed::Attempt(n),
            StreamEvent::Text(t) => Streamed::Text(t.to_owned()),
            StreamEvent::Reasoning(n) => Streamed::Reasoning(n),
            StreamEvent::Call { index, name } => Streamed::Call {
                index,
                name: name.to_owned(),
            },
            StreamEvent::Arguments { index, delta } => Streamed::Arguments {
                index,
                delta: delta.to_owned(),
            },
            StreamEvent::End => Streamed::End,
        }
    }
}

/// Columns of the indent under `duet: ` (as in the plain output).
const INDENT: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Activity {
    Waiting,
    Thinking,
    Writing(Option<String>),
    Running(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Text,
    Call(usize),
}

/// duet's text being shown as it streams.
struct Block {
    md: Markdown,
    source: Source,
    detok: Detok,
    /// The text shown so far, restored.
    text: String,
}

struct Call {
    name: String,
    reader: Option<FieldReader>,
}

pub struct Feed {
    colour: bool,
    columns: usize,
    /// Show the frontier's text (live views); a log only counts it.
    show_text: bool,
    restore: Restore,
    names: HashMap<String, String>,
    calls: HashMap<usize, Call>,
    block: Option<Block>,
    /// The last block of the turn (restored), to compare with its end.
    last_block: Option<String>,
    /// The current response's text was shown as it streamed.
    response_text: bool,
    /// Whether the current attempt showed anything.
    attempt_shown: bool,
    started: Option<Instant>,
    cost: f64,
    out_bytes: usize,
    activity: Activity,
    /// Tool calls of the last response still running, in order.
    running: VecDeque<(String, String)>,
}

impl Feed {
    pub fn new(colour: bool, columns: usize, show_text: bool, restore: Restore) -> Self {
        Self {
            colour,
            columns,
            show_text,
            restore,
            names: HashMap::new(),
            calls: HashMap::new(),
            block: None,
            last_block: None,
            response_text: false,
            attempt_shown: false,
            started: None,
            cost: 0.0,
            out_bytes: 0,
            activity: Activity::Waiting,
            running: VecDeque::new(),
        }
    }

    pub fn set_restore(&mut self, restore: Restore) {
        self.restore = restore;
    }

    pub fn resize(&mut self, columns: usize) {
        self.columns = columns;
    }

    /// Work starts (a turn, or a run): the status counts from `now`.
    pub fn begin(&mut self, now: Instant) {
        self.started = Some(now);
        self.cost = 0.0;
        self.out_bytes = 0;
        self.activity = Activity::Waiting;
        self.running.clear();
        self.last_block = None;
    }

    pub fn working(&self) -> bool {
        self.started.is_some()
    }

    fn label(&self, text: &str, style: Style) -> Vec<Span> {
        vec![Span::new(text, style)]
    }

    fn markdown(&self, label: Vec<Span>) -> Markdown {
        Markdown::new(self.columns, self.colour, label, INDENT)
    }

    /// A stream event of the response being received.
    pub fn stream(&mut self, event: &Streamed) -> Vec<String> {
        let mut out = Vec::new();
        match event {
            Streamed::Attempt(n) => {
                out.extend(self.close_block());
                if *n > 1 && self.attempt_shown {
                    out.push(tint(
                        "  ↻ the response was cut off; it starts again",
                        self.colour,
                    ));
                }
                self.calls.clear();
                self.response_text = false;
                self.attempt_shown = false;
                self.out_bytes = 0;
                self.activity = Activity::Waiting;
            }
            Streamed::Reasoning(n) => {
                self.out_bytes += n;
                self.activity = Activity::Thinking;
            }
            Streamed::Text(t) => {
                self.out_bytes += t.len();
                self.activity = Activity::Writing(None);
                if self.show_text {
                    out.extend(self.show(Source::Text, t));
                }
            }
            Streamed::Call { index, name } => {
                self.activity = Activity::Writing(Some(name.clone()));
                let field = match name.as_str() {
                    "reply" => Some("message"),
                    "ask_operator" => Some("question"),
                    _ => None,
                };
                self.calls.insert(
                    *index,
                    Call {
                        name: name.clone(),
                        reader: field.map(FieldReader::new),
                    },
                );
            }
            Streamed::Arguments { index, delta } => {
                self.out_bytes += delta.len();
                let text = self
                    .calls
                    .get_mut(index)
                    .and_then(|c| c.reader.as_mut())
                    .map(|r| r.push(delta));
                if let Some(text) = text.filter(|_| self.show_text) {
                    out.extend(self.show(Source::Call(*index), &text));
                }
            }
            Streamed::End => out.extend(self.close_block()),
        }
        out
    }

    /// Adds streamed text from `source` to the block it belongs to.
    fn show(&mut self, source: Source, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        if self.block.as_ref().is_some_and(|b| b.source != source) {
            out.extend(self.close_block());
        }
        if self.block.is_none() {
            if text.trim().is_empty() {
                return out;
            }
            let question = matches!(source, Source::Call(i)
                if self.calls.get(&i).is_some_and(|c| c.name == "ask_operator"));
            let label = if question {
                self.label("duet asks: ", Style::WARN.bold())
            } else {
                self.label("duet: ", Style::ACCENT.bold())
            };
            self.block = Some(Block {
                md: self.markdown(label),
                source,
                detok: Detok::default(),
                text: String::new(),
            });
        }
        let restore = self.restore.clone();
        let b = self.block.as_mut().expect("a block");
        let shown = b.detok.push(text, restore.as_ref());
        b.text.push_str(&shown);
        out.extend(b.md.push(&shown));
        if source == Source::Text {
            self.response_text = true;
        }
        self.attempt_shown = true;
        out
    }

    fn close_block(&mut self) -> Vec<String> {
        let Some(mut b) = self.block.take() else {
            return Vec::new();
        };
        let rest = b.detok.finish(self.restore.as_ref());
        b.text.push_str(&rest);
        let mut out = b.md.push(&rest);
        out.extend(b.md.finish());
        self.last_block = Some(b.text);
        out
    }

    /// The row being written, for the live region.
    pub fn partial(&self) -> Option<String> {
        self.block.as_ref().and_then(|b| b.md.partial())
    }

    /// A transcript entry: its progress lines.
    pub fn entry(&mut self, entry: &Entry) -> Vec<String> {
        let mut shown = None;
        match entry {
            Entry::Item {
                item:
                    Item::Assistant {
                        tool_calls,
                        text,
                        reasoning,
                        replay,
                    },
            } => {
                self.running = tool_calls
                    .iter()
                    .filter(|c| c.name != "reply" && c.name != "ask_operator")
                    .map(|c| (c.id.clone(), describe(c, self.restore.as_ref())))
                    .collect();
                self.activity = match self.running.front() {
                    Some((_, what)) => Activity::Running(what.clone()),
                    None => Activity::Waiting,
                };
                // Text shown as it streamed is not repeated.
                if std::mem::take(&mut self.response_text) && !text.is_empty() {
                    shown = Some(Entry::Item {
                        item: Item::Assistant {
                            text: String::new(),
                            reasoning: reasoning.clone(),
                            tool_calls: tool_calls.clone(),
                            replay: replay.clone(),
                        },
                    });
                }
            }
            Entry::Item {
                item: Item::ToolResult { call_id, .. },
            } => {
                self.running.retain(|(id, _)| id != call_id);
                self.activity = match self.running.front() {
                    Some((_, what)) => Activity::Running(what.clone()),
                    None => Activity::Waiting,
                };
            }
            Entry::Usage { cost_usd, .. } | Entry::FailedAttempts { cost_usd, .. } => {
                self.cost += cost_usd;
            }
            _ => {}
        }
        super::words::progress(
            shown.as_ref().unwrap_or(entry),
            &mut self.names,
            self.restore.as_ref(),
        )
        .iter()
        .map(|l| tint(l, self.colour))
        .collect()
    }

    /// The status of the work, one line (without the spinner): time, cost,
    /// what happens now.
    pub fn status_text(&self, now: Instant) -> Option<String> {
        let started = self.started?;
        let secs = now.saturating_duration_since(started).as_secs();
        let time = if secs >= 3600 {
            format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
        } else {
            format!("{}:{:02}", secs / 60, secs % 60)
        };
        let tokens = match self.out_bytes / 4 {
            0 => String::new(),
            n if n >= 1000 => format!(" (~{:.1}k tokens)", n as f64 / 1000.0),
            n => format!(" (~{n} tokens)"),
        };
        let what = match &self.activity {
            Activity::Waiting => "waiting for the frontier".to_owned(),
            Activity::Thinking => format!("frontier thinking{tokens}"),
            Activity::Writing(None) => format!("frontier writing{tokens}"),
            Activity::Writing(Some(tool)) => match tool.as_str() {
                "reply" => format!("frontier writing its reply{tokens}"),
                "ask_operator" => format!("frontier writing a question{tokens}"),
                t => format!("frontier writing a {t} call{tokens}"),
            },
            Activity::Running(what) => format!("running {what}"),
        };
        Some(format!("{time} · ${:.4} · {what}", self.cost))
    }

    /// The status line for the live region, `hint` at its end when it fits.
    pub fn status(&self, now: Instant, hint: &str) -> Option<String> {
        let text = safe(&self.status_text(now)?);
        let started = self.started?;
        let frame = ["◐", "◓", "◑", "◒"]
            [(now.saturating_duration_since(started).as_millis() / 250 % 4) as usize];
        let room = self.columns.saturating_sub(1);
        let mut line = format!(
            "{} {}",
            styled(frame, Style::ACCENT, self.colour),
            styled(&text, Style::DIM, self.colour)
        );
        if !hint.is_empty() && super::width(&text) + super::width(hint) + 5 <= room {
            line.push_str(&styled(&format!(" · {hint}"), Style::DIM, self.colour));
        }
        Some(clip_width(&line, room))
    }

    /// Work ended (a run): what is still open is shown.
    pub fn finish(&mut self) -> Vec<String> {
        self.started = None;
        self.close_block()
    }

    /// A turn ended: how, in the conversation. A reply or question already
    /// shown as it streamed is not repeated.
    pub fn end(&mut self, end: &TurnEnd) -> Vec<String> {
        let mut out = self.close_block();
        let streamed = self.last_block.take();
        self.started = None;
        self.calls.clear();
        self.response_text = false;
        let label = |text: &str, style: Style| vec![Span::new(text, style)];
        let indent = || label(&" ".repeat(INDENT), Style::PLAIN);
        match end {
            TurnEnd::Replied { message } => {
                if !streamed.is_some_and(|s| same(&s, message)) {
                    out.extend(
                        self.markdown(label("duet: ", Style::ACCENT.bold()))
                            .render(message.trim()),
                    );
                }
            }
            TurnEnd::Asked { question, .. } => {
                match streamed.and_then(|s| after(question, &s)) {
                    Some(rest) if rest.trim().is_empty() => {}
                    Some(rest) => out.extend(self.markdown(indent()).render(rest.trim())),
                    None => out.extend(
                        self.markdown(label("duet asks: ", Style::WARN.bold()))
                            .render(question.trim()),
                    ),
                }
                out.push(styled(
                    "      (your next message is the answer)",
                    Style::DIM,
                    self.colour,
                ));
            }
            TurnEnd::Completed { summary } => out.extend(
                self.markdown(label("duet finished: ", Style::GOOD.bold()))
                    .render(summary.trim()),
            ),
            other => {
                let style = match other {
                    TurnEnd::Failed { .. } => Style::BAD,
                    _ => Style::WARN,
                };
                for l in safe(&super::words::describe_end(other)).lines() {
                    out.push(styled(l, style, self.colour));
                }
            }
        }
        out
    }
}

/// A tool call as the status names it (restored for the operator).
fn describe(call: &duet_boundary::model::ToolCall, restore: &dyn Fn(&str) -> String) -> String {
    let arg = ["path", "command", "pattern", "dir", "task"]
        .iter()
        .find_map(|k| call.arguments.get(*k).and_then(|v| v.as_str()))
        .map(|a| restore(a).lines().next().unwrap_or("").to_owned())
        .unwrap_or_default();
    let what = format!("{} {arg}", call.name);
    clip_width(what.trim_end(), 60)
}

/// Whether two texts say the same (ignoring how whitespace was laid out).
fn same(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

/// What `full` says after `start`, if it starts with it.
fn after<'a>(full: &'a str, start: &str) -> Option<&'a str> {
    full.trim_start().strip_prefix(start.trim())
}

/// A progress or status line of duet's own, styled by its mark (the text
/// is made safe for the terminal first).
pub fn tint(line: &str, colour: bool) -> String {
    let line = safe(line);
    let style = if line.starts_with("  ◦") {
        Style::HELD
    } else if line.starts_with("  !") || line.starts_with("  ■") || line.starts_with("  ↻") {
        Style::WARN
    } else if line.starts_with("  ▸") {
        Style::ACCENT
    } else if line.starts_with("  ~") {
        Style::DIM.italic()
    } else if line.starts_with("  ") {
        Style::DIM
    } else if let Some(rest) = line.strip_prefix("you> ") {
        return format!("{}{rest}", styled("you> ", Style::PLAIN.bold(), colour));
    } else {
        Style::PLAIN
    };
    styled(&line, style, colour)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_boundary::model::ToolCall;
    use serde_json::{Value, json};

    fn restore() -> Restore {
        Arc::new(|t: &str| t.replace("⟨email:EMAIL#1⟩", "ana@example.org"))
    }

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        let Value::Object(arguments) = args else {
            unreachable!()
        };
        ToolCall {
            id: id.into(),
            name: name.into(),
            raw_arguments: String::new(),
            arguments,
        }
    }

    fn assistant(text: &str, calls: Vec<ToolCall>) -> Entry {
        Entry::Item {
            item: Item::Assistant {
                text: text.into(),
                reasoning: None,
                tool_calls: calls,
                replay: None,
            },
        }
    }

    fn feed() -> Feed {
        Feed::new(false, 60, true, restore())
    }

    #[test]
    fn a_streamed_reply_is_shown_as_it_arrives_and_not_repeated() {
        let mut f = feed();
        let t0 = Instant::now();
        f.begin(t0);
        let mut lines = Vec::new();
        lines.extend(f.stream(&Streamed::Attempt(1)));
        lines.extend(f.stream(&Streamed::Text("Looking at the ".into())));
        assert_eq!(f.partial().as_deref(), Some("duet: Looking at the"));
        lines.extend(f.stream(&Streamed::Text("notes of ⟨email:EM".into())));
        // The placeholder is held back until it is complete.
        assert_eq!(
            f.partial().as_deref(),
            Some("duet: Looking at the notes of")
        );
        lines.extend(f.stream(&Streamed::Text("AIL#1⟩.".into())));
        lines.extend(f.stream(&Streamed::End));
        assert_eq!(lines, ["duet: Looking at the notes of ana@example.org."]);
        // Its transcript entry does not repeat the text; the call shows.
        let read = call(
            "c1",
            "read_file",
            json!({"path": "notes/⟨email:EMAIL#1⟩.txt"}),
        );
        assert_eq!(
            f.entry(&assistant(
                "Looking at the notes of ⟨email:EMAIL#1⟩.",
                vec![read]
            )),
            ["  · read_file notes/ana@example.org.txt"]
        );
        assert_eq!(
            f.status_text(t0).as_deref(),
            Some("0:00 · $0.0000 · running read_file notes/ana@example.org.txt")
        );
        // The reply streams from the call's arguments.
        f.stream(&Streamed::Attempt(1));
        f.stream(&Streamed::Call {
            index: 0,
            name: "reply".into(),
        });
        let mut lines = Vec::new();
        for piece in [
            "{\"mess",
            "age\": \"Fixed.\\nThe ",
            "row count is right",
            "\"}",
        ] {
            lines.extend(f.stream(&Streamed::Arguments {
                index: 0,
                delta: piece.into(),
            }));
        }
        lines.extend(f.stream(&Streamed::End));
        assert_eq!(lines, ["duet: Fixed.", "      The row count is right"]);
        assert!(
            f.end(&TurnEnd::Replied {
                message: "Fixed.\nThe row count is right".into()
            })
            .is_empty()
        );
        assert!(f.status_text(t0).is_none(), "the turn ended");
    }

    #[test]
    fn a_question_shows_its_options_after_it_streamed() {
        let mut f = feed();
        f.begin(Instant::now());
        f.stream(&Streamed::Attempt(1));
        f.stream(&Streamed::Call {
            index: 0,
            name: "ask_operator".into(),
        });
        let mut lines = f.stream(&Streamed::Arguments {
            index: 0,
            delta: "{\"question\": \"Should b return a value?\", \"options\": [\"unit\", \"u32\"]}"
                .into(),
        });
        lines.extend(f.stream(&Streamed::End));
        lines.extend(f.end(&TurnEnd::Asked {
            options: None,
            question: "Should b return a value?\noptions: unit / u32".into(),
        }));
        assert_eq!(
            lines,
            [
                "duet asks: Should b return a value?",
                "      options: unit / u32",
                "      (your next message is the answer)",
            ]
        );
    }

    #[test]
    fn an_end_not_streamed_is_shown_whole_and_failures_read_as_before() {
        let mut f = feed();
        f.begin(Instant::now());
        assert_eq!(
            f.end(&TurnEnd::Completed {
                summary: "Added b.".into()
            }),
            ["duet finished: Added b."]
        );
        assert_eq!(
            f.end(&TurnEnd::Replied {
                message: "- one\n- two".into()
            }),
            ["duet: • one", "      • two"]
        );
        assert_eq!(
            f.end(&TurnEnd::Interrupted),
            [crate::term::words::describe_end(&TurnEnd::Interrupted)]
        );
    }

    #[test]
    fn a_retried_response_is_marked_and_shown_again() {
        let mut f = feed();
        f.begin(Instant::now());
        f.stream(&Streamed::Attempt(1));
        let mut lines = f.stream(&Streamed::Text("Partial answ".into()));
        lines.extend(f.stream(&Streamed::End));
        lines.extend(f.stream(&Streamed::Attempt(2)));
        lines.extend(f.stream(&Streamed::Text("Whole answer.".into())));
        lines.extend(f.stream(&Streamed::End));
        assert_eq!(
            lines,
            [
                "duet: Partial answ",
                "  ↻ the response was cut off; it starts again",
                "duet: Whole answer.",
            ]
        );
    }

    #[test]
    fn a_log_counts_the_stream_without_showing_it() {
        let mut f = Feed::new(false, 80, false, Arc::new(|t: &str| t.to_owned()));
        let t0 = Instant::now();
        f.begin(t0);
        f.stream(&Streamed::Attempt(1));
        f.stream(&Streamed::Call {
            index: 0,
            name: "write_file".into(),
        });
        let lines = f.stream(&Streamed::Arguments {
            index: 0,
            delta: "x".repeat(8400),
        });
        assert!(lines.is_empty() && f.partial().is_none());
        assert_eq!(
            f.status_text(t0 + std::time::Duration::from_secs(75))
                .as_deref(),
            Some("1:15 · $0.0000 · frontier writing a write_file call (~2.1k tokens)")
        );
        f.entry(&Entry::Usage {
            turn: 1,
            usage: Default::default(),
            cost_usd: 0.0123,
            interventions: vec![],
        });
        let lines = f.entry(&assistant(
            "",
            vec![call(
                "c1",
                "write_file",
                json!({"path": "src/⟨email:EMAIL#1⟩.rs"}),
            )],
        ));
        // A log keeps placeholders.
        assert_eq!(lines, ["  · write_file src/⟨email:EMAIL#1⟩.rs"]);
        let s = f.status(t0, "Ctrl-C stops").unwrap();
        assert_eq!(
            s,
            "◐ 0:00 · $0.0123 · running write_file src/⟨email:EMAIL#1⟩.rs · Ctrl-C stops"
        );
    }

    #[test]
    fn lines_are_tinted_by_their_mark_and_made_safe() {
        assert_eq!(
            tint("  ◦ withheld from the frontier: x", true),
            "\x1b[35m  ◦ withheld from the frontier: x\x1b[0m"
        );
        assert_eq!(tint("  · read_file a\x1b[2J", false), "  · read_file a");
        assert_eq!(tint("you> hi", true), "\x1b[1myou> \x1b[0mhi");
    }
}
