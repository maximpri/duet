// SPDX-License-Identifier: GPL-3.0-or-later
//! The conversation as cells: one per message, reply, tool call (with its
//! result), edit (with its diff), intervention of the boundary, note, and turn
//! end. Each has a title line (a mark, what it is, a detail, tags) and a body,
//! drawn indented under it; a tool's result is folded to its first lines until
//! details are shown (Ctrl-O), except a failure's.
//!
//! What is shown: the operator's own text; duet's replies with placeholders
//! restored (as the feed restores them); a tool call's arguments restored; a
//! tool's result as the frontier saw it (placeholders kept: this is what left
//! the machine); an edit of a sensitive file named only, never its content.
//! Everything from outside duet passes `term::safe` before it is stored.

use crate::term::markdown::Markdown;
use crate::term::stream::{Detok, FieldReader};
use crate::term::{Span as TSpan, safe};
use duet_agent::TurnEnd;
use duet_agent::transcript::Entry;
use duet_boundary::model::{Item, ToolCall};
use duet_boundary::view::ViewClass;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::HashMap;
use std::sync::Arc;

use super::ansi;
use crate::term::feed::Restore;
use crate::term::feed::Streamed;

/// Result lines shown while folded, and for a failure.
const FOLDED: usize = 2;
/// Lines of output printed elsewhere shown while folded.
const FOLDED_OUTPUT: usize = 3;
const FOLDED_FAILURE: usize = 8;
/// Lines of a result kept at all, and of an edit's diff.
const KEEP_RESULT: usize = 400;
const KEEP_DIFF: usize = 60;
/// The indent of a cell's body under its title.
const INDENT: usize = 2;

pub(super) mod palette {
    use ratatui::style::{Color, Modifier, Style};
    pub const ACCENT: Color = Color::Cyan;
    pub const MUTED: Color = Color::DarkGray;
    pub const GOOD: Color = Color::Green;
    pub const WARN: Color = Color::Yellow;
    pub const BAD: Color = Color::Red;
    /// What the boundary withheld from the frontier.
    pub const HELD: Color = Color::Magenta;
    pub fn accent() -> Style {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    }
    pub fn muted() -> Style {
        Style::new().fg(MUTED)
    }
}
use palette::{ACCENT, BAD, GOOD, HELD, WARN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    You,
    Duet,
    Asks,
    Done,
    Tool,
    Edit,
    Held,
    Steer,
    Note,
    Output,
    Warn,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Live,
    Ok,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Body {
    None,
    /// duet's text, rendered as light Markdown at the cell's width.
    Markdown(String),
    Lines(Vec<String>),
    /// An edit: removed and added lines.
    Diff(Vec<(char, String)>),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Cell {
    pub kind: Kind,
    pub state: State,
    pub title: String,
    pub detail: String,
    /// Marks after the title: what the boundary did with the result.
    pub tags: Vec<String>,
    pub body: Body,
    /// A tool's result, as the frontier saw it.
    pub result: Vec<String>,
    /// Bumped on every change (for the row cache).
    version: u64,
}

impl Cell {
    fn new(kind: Kind, title: impl Into<String>) -> Self {
        Cell {
            kind,
            state: State::Ok,
            title: safe(&title.into()),
            detail: String::new(),
            tags: Vec::new(),
            body: Body::None,
            result: Vec::new(),
            version: 0,
        }
    }

    fn with_lines(mut self, text: &str) -> Self {
        self.body = Body::Lines(safe(text).lines().map(str::to_owned).collect());
        self
    }

    fn touch(&mut self) {
        self.version += 1;
    }
}

/// A cell's rows for (width, details, version).
type Cached = ((usize, bool, u64), Vec<Line<'static>>);

/// Which part of a response is being shown as it streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Text,
    Call(usize),
}

struct Live {
    cell: usize,
    source: Source,
    detok: Detok,
}

/// The conversation, cell by cell.
pub(super) struct Cells {
    pub list: Vec<Cell>,
    /// Row cache: (width, details, version) → rows, per cell.
    cache: Vec<Option<Cached>>,
    by_call: HashMap<String, usize>,
    by_child: HashMap<String, usize>,
    restore: Restore,
    colour: bool,
    /// Whether a workspace path is held locally (sensitive by policy).
    held: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    calls: HashMap<usize, (String, Option<FieldReader>)>,
    live: Option<Live>,
    /// The response's text was shown as it streamed.
    response_text: bool,
    /// The last streamed block of the turn (restored), to compare with its end.
    last_streamed: Option<String>,
}

impl Cells {
    pub(super) fn new(colour: bool) -> Self {
        Cells {
            list: Vec::new(),
            cache: Vec::new(),
            by_call: HashMap::new(),
            by_child: HashMap::new(),
            restore: Arc::new(|t: &str| t.to_owned()),
            colour,
            held: Arc::new(|_| false),
            calls: HashMap::new(),
            live: None,
            response_text: false,
            last_streamed: None,
        }
    }

    pub(super) fn set_restore(&mut self, restore: Restore) {
        self.restore = restore;
    }

    pub(super) fn set_held(&mut self, held: Arc<dyn Fn(&str) -> bool + Send + Sync>) {
        self.held = held;
    }

    fn push(&mut self, cell: Cell) -> usize {
        self.list.push(cell);
        self.cache.push(None);
        self.list.len() - 1
    }

    fn get(&mut self, i: usize) -> Option<&mut Cell> {
        let c = self.list.get_mut(i)?;
        c.touch();
        Some(c)
    }

    /// Whether anything but notes is in the conversation yet.
    pub(super) fn is_empty(&self) -> bool {
        !self
            .list
            .iter()
            .any(|c| !matches!(c.kind, Kind::Note | Kind::Output))
    }

    /// A message the operator sent.
    pub(super) fn you(&mut self, text: &str) {
        self.push(Cell::new(Kind::You, "you").with_lines(text));
    }

    /// Lines of duet's own (from the session: notices, a recap, steering).
    pub(super) fn lines(&mut self, text: &str) {
        let mut note: Vec<String> = Vec::new();
        let flush = |cells: &mut Cells, note: &mut Vec<String>| {
            if !note.is_empty() {
                let cell = Cell::new(Kind::Note, "").with_lines(&note.join("\n"));
                cells.push(cell);
                note.clear();
            }
        };
        for line in text.lines() {
            if let Some(m) = line.strip_prefix("you> ") {
                flush(self, &mut note);
                self.you(m);
            } else if let Some(m) = line.strip_prefix("  ▸ ") {
                flush(self, &mut note);
                self.push(Cell::new(Kind::Steer, "steering").with_lines(m));
            } else if let Some(m) = line.strip_prefix("  ■ ") {
                flush(self, &mut note);
                self.push(Cell::new(Kind::Warn, m));
            } else if line.starts_with("     ") && note.is_empty() && self.last_is(Kind::You) {
                // A recap's continuation lines of the operator's message.
                if let Some(Cell {
                    body: Body::Lines(l),
                    ..
                }) = self.list.last_mut()
                {
                    l.push(safe(line.trim_start()));
                }
            } else {
                note.push(line.to_owned());
            }
        }
        flush(self, &mut note);
    }

    fn last_is(&self, kind: Kind) -> bool {
        self.list.last().is_some_and(|c| c.kind == kind)
    }

    /// Text shown as it is (help, an approval question).
    pub(super) fn text(&mut self, text: &str) {
        self.push(Cell::new(Kind::Note, "").with_lines(text.trim_end()));
    }

    /// `/diff`'s output.
    pub(super) fn diff(&mut self, text: &str) {
        let mut cell = Cell::new(Kind::Note, "changes");
        cell.body = Body::Lines(safe(text).lines().map(str::to_owned).collect());
        cell.detail = "against the last commit".into();
        cell.kind = Kind::Tool;
        cell.title = "diff".into();
        self.push(cell);
    }

    /// A line another part of the process printed: one dim block with the
    /// lines around it.
    pub(super) fn output(&mut self, line: &str) {
        let line = safe(line);
        if line.trim().is_empty() {
            return;
        }
        if let Some(i) = self.list.len().checked_sub(1)
            && self.list[i].kind == Kind::Output
            && let Some(c) = self.get(i)
            && let Body::Lines(l) = &mut c.body
        {
            l.push(line);
            return;
        }
        self.push(Cell::new(Kind::Output, "").with_lines(&line));
    }

    /// A turn starts.
    pub(super) fn begin(&mut self) {
        self.calls.clear();
        self.live = None;
        self.response_text = false;
        self.last_streamed = None;
    }

    /// A stream event of the response being received.
    pub(super) fn stream(&mut self, event: &Streamed) {
        match event {
            Streamed::Attempt(n) => {
                if *n > 1
                    && let Some(live) = self.live.take()
                {
                    // What the cut-off attempt showed is void.
                    if let Some(c) = self.get(live.cell) {
                        c.state = State::Stopped;
                        c.detail = "cut off; the response starts again".into();
                    }
                }
                self.close_live();
                self.calls.clear();
                self.response_text = false;
            }
            Streamed::Text(t) => self.show(Source::Text, t),
            Streamed::Call { index, name } => {
                let field = match name.as_str() {
                    "reply" => Some("message"),
                    "ask_operator" => Some("question"),
                    _ => None,
                };
                self.calls
                    .insert(*index, (name.clone(), field.map(FieldReader::new)));
            }
            Streamed::Arguments { index, delta } => {
                let text = self
                    .calls
                    .get_mut(index)
                    .and_then(|(_, r)| r.as_mut())
                    .map(|r| r.push(delta));
                if let Some(text) = text {
                    self.show(Source::Call(*index), &text);
                }
            }
            Streamed::Reasoning(_) => {}
            Streamed::End => self.close_live(),
        }
    }

    fn show(&mut self, source: Source, text: &str) {
        if self.live.as_ref().is_some_and(|l| l.source != source) {
            self.close_live();
        }
        if self.live.is_none() {
            if text.trim().is_empty() {
                return;
            }
            let asks = matches!(source, Source::Call(i)
                if self.calls.get(&i).is_some_and(|(n, _)| n == "ask_operator"));
            let mut cell = Cell::new(if asks { Kind::Asks } else { Kind::Duet }, "duet");
            if asks {
                cell.title = "duet asks".into();
            }
            cell.state = State::Live;
            cell.body = Body::Markdown(String::new());
            let i = self.push(cell);
            self.live = Some(Live {
                cell: i,
                source,
                detok: Detok::default(),
            });
        }
        let restore = self.restore.clone();
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let shown = live.detok.push(text, restore.as_ref());
        let i = live.cell;
        if let Some(c) = self.get(i)
            && let Body::Markdown(md) = &mut c.body
        {
            md.push_str(&shown);
        }
        if source == Source::Text {
            self.response_text = true;
        }
    }

    fn close_live(&mut self) {
        let Some(mut live) = self.live.take() else {
            return;
        };
        let rest = live.detok.finish(self.restore.as_ref());
        let mut streamed = None;
        if let Some(c) = self.get(live.cell) {
            if let Body::Markdown(md) = &mut c.body {
                md.push_str(&rest);
                streamed = Some(md.clone());
            }
            if c.state == State::Live {
                c.state = State::Ok;
            }
        }
        if streamed.is_some() {
            self.last_streamed = streamed;
        }
    }

    /// A transcript entry.
    pub(super) fn entry(&mut self, e: &Entry) {
        match e {
            Entry::Item {
                item: Item::Assistant {
                    text, tool_calls, ..
                },
            } => {
                let streamed = std::mem::take(&mut self.response_text);
                let narrates = tool_calls
                    .iter()
                    .any(|c| c.name != "reply" && c.name != "ask_operator");
                if !streamed && narrates && !text.trim().is_empty() {
                    let mut cell = Cell::new(Kind::Duet, "duet");
                    cell.body = Body::Markdown((self.restore)(text));
                    self.push(cell);
                }
                for c in tool_calls {
                    if c.name == "reply" || c.name == "ask_operator" {
                        continue;
                    }
                    let cell = self.call_cell(c);
                    let i = self.push(cell);
                    self.by_call.insert(c.id.clone(), i);
                }
            }
            Entry::Item {
                item: Item::ToolResult { call_id, content },
            } => {
                if let Some(&i) = self.by_call.get(call_id)
                    && let Some(c) = self.get(i)
                {
                    let content = safe(content);
                    let failed = content.starts_with("error:")
                        || content.starts_with("checks failed")
                        || content.starts_with("timed out")
                        || content
                            .lines()
                            .next()
                            .and_then(|l| l.strip_prefix("exit code "))
                            .is_some_and(|code| code.trim() != "0");
                    c.state = if failed { State::Failed } else { State::Ok };
                    c.result = content
                        .lines()
                        .take(KEEP_RESULT)
                        .map(str::to_owned)
                        .collect();
                    if content.lines().count() > KEEP_RESULT {
                        c.result.push(format!(
                            "… {} more lines",
                            content.lines().count() - KEEP_RESULT
                        ));
                    }
                }
            }
            Entry::Shown { call_id, class } => {
                let tag = match class {
                    ViewClass::Raw => return,
                    ViewClass::Tokenized => "values replaced by placeholders",
                    ViewClass::HandleSummary => "held locally, a summary was sent",
                    ViewClass::LocalAnswer => "answered by the local model",
                    ViewClass::BulkyHandle => "large, an outline was sent",
                    ViewClass::Protected => "protected, only its interface was sent",
                };
                if let Some(&i) = self.by_call.get(call_id)
                    && let Some(c) = self.get(i)
                {
                    c.tags.push(tag.into());
                }
            }
            Entry::Usage { interventions, .. } if !interventions.is_empty() => {
                let mut cell = Cell::new(Kind::Held, "withheld from the frontier");
                cell.body = Body::Lines(interventions.iter().map(|i| safe(i)).collect());
                self.push(cell);
            }
            Entry::Masked { items, .. } => {
                let mut cell = Cell::new(Kind::Note, "");
                cell.body = Body::Lines(vec![format!(
                    "context: {items} old tool result(s) shortened"
                )]);
                self.push(cell);
            }
            Entry::Compacted {
                head,
                upto,
                tokens_before,
                tokens_after,
                ..
            } => {
                let mut cell = Cell::new(Kind::Note, "");
                cell.body = Body::Lines(vec![format!(
                    "context: {} earlier item(s) condensed by the local model (~{tokens_before} → ~{tokens_after} tokens)",
                    upto - head
                )]);
                self.push(cell);
            }
            Entry::Interrupted { call_id, tool } => match self.by_call.get(call_id).copied() {
                Some(i) => {
                    if let Some(c) = self.get(i) {
                        c.state = State::Stopped;
                        c.tags.push("stopped".into());
                    }
                }
                None => {
                    self.push(Cell::new(Kind::Warn, format!("{tool} stopped")));
                }
            },
            Entry::Steered {
                after_request,
                messages,
                ..
            } => {
                let mut cell = Cell::new(Kind::Steer, "delivered to duet");
                cell.detail = format!("after request {after_request}");
                cell.body = Body::Lines(
                    messages
                        .iter()
                        .flat_map(|m| {
                            safe(&(self.restore)(m))
                                .lines()
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .collect(),
                );
                self.push(cell);
            }
            Entry::SubagentStart {
                child, mode, task, ..
            } => {
                let mut cell = Cell::new(Kind::Tool, format!("sub-agent {child}"));
                cell.state = State::Live;
                cell.detail = format!("({mode}) {}", first_line(&(self.restore)(task), 80));
                let i = self.push(cell);
                self.by_child.insert(child.clone(), i);
            }
            Entry::Subagent { child, entry } => {
                if let Some(&i) = self.by_child.get(child) {
                    let mut names = HashMap::new();
                    let lines =
                        crate::term::words::progress(entry, &mut names, self.restore.as_ref());
                    if let Some(c) = self.get(i) {
                        c.result.extend(lines.iter().map(|l| safe(l.trim_start())));
                    }
                }
            }
            Entry::SubagentEnd {
                child,
                terminal,
                cost_usd,
                written,
                ..
            } => {
                if let Some(&i) = self.by_child.get(child)
                    && let Some(c) = self.get(i)
                {
                    c.state = if terminal.state() == "completed" {
                        State::Ok
                    } else {
                        State::Failed
                    };
                    c.tags.push(format!(
                        "{} · ${cost_usd:.4} · {} file(s) written",
                        terminal.state(),
                        written.len()
                    ));
                }
            }
            _ => {}
        }
    }

    /// The cell a tool call opens (its result arrives later).
    fn call_cell(&self, c: &ToolCall) -> Cell {
        let arg = |k: &str| {
            c.arguments
                .get(k)
                .and_then(|v| v.as_str())
                .map(|a| first_line(&(self.restore)(a), 100))
                .unwrap_or_default()
        };
        let (title, detail) = match c.name.as_str() {
            "read_file" => ("read", arg("path")),
            "read_raw" => ("read", arg("handle")),
            "list_files" => ("list", {
                let d = arg("dir");
                if d.is_empty() { ".".into() } else { d }
            }),
            "search" => ("search", arg("pattern")),
            "code_nav" => ("navigate", {
                let s = arg("symbol");
                if s.is_empty() { arg("path") } else { s }
            }),
            "run_command" => {
                let sensitive = c
                    .arguments
                    .get("sensitive_data")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (
                    if sensitive {
                        "run on sensitive data"
                    } else {
                        "run"
                    },
                    arg("command"),
                )
            }
            "ask_local" => ("ask the local model", arg("question")),
            "explore" => ("explore", arg("question")),
            "web_fetch" => ("fetch", arg("url")),
            "web_search" => ("search the web", arg("query")),
            "synthetic_sample" => ("sample", arg("handle")),
            "diff" => ("diff", arg("path")),
            "finish" => ("finish", "running the checks".to_owned()),
            "delegate" => ("delegate", arg("task")),
            "edit_protected" => ("edit protected", arg("path")),
            n if n.starts_with("git_") => ("git", n.trim_start_matches("git_").to_owned()),
            "write_file" | "edit_file" => return self.edit_cell(c),
            other => (other, {
                c.arguments
                    .values()
                    .find_map(|v| v.as_str())
                    .map(|a| first_line(&(self.restore)(a), 100))
                    .unwrap_or_default()
            }),
        };
        let mut cell = Cell::new(Kind::Tool, title);
        cell.state = State::Live;
        cell.detail = detail;
        if c.name == "ask_local" {
            cell.tags.push("on this machine".into());
        }
        cell
    }

    /// A write or an edit, with what it changes (a sensitive file is named).
    fn edit_cell(&self, c: &ToolCall) -> Cell {
        let path = c
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .map(safe)
            .unwrap_or_default();
        let write = c.name == "write_file";
        let mut cell = Cell::new(Kind::Edit, if write { "write" } else { "edit" });
        cell.state = State::Live;
        cell.detail = path.clone();
        if (self.held)(&path) {
            cell.tags.push("sensitive: content not shown".into());
            return cell;
        }
        let restore = |t: &str| safe(&(self.restore)(t));
        let mut rows: Vec<(char, String)> = Vec::new();
        let (mut added, mut removed) = (0, 0);
        if write {
            let content = c
                .arguments
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            for l in restore(content).lines() {
                added += 1;
                rows.push(('+', l.to_owned()));
            }
        } else if let Some(edits) = c.arguments.get("edits").and_then(|v| v.as_array()) {
            for e in edits {
                let text = |k: &str| {
                    e.get(k)
                        .and_then(|v| v.as_str())
                        .map(restore)
                        .unwrap_or_default()
                };
                if !rows.is_empty() {
                    rows.push(('…', String::new()));
                }
                for l in text("old").lines() {
                    removed += 1;
                    rows.push(('-', l.to_owned()));
                }
                for l in text("new").lines() {
                    added += 1;
                    rows.push(('+', l.to_owned()));
                }
            }
        }
        if rows.len() > KEEP_DIFF {
            let more = rows.len() - KEEP_DIFF;
            rows.truncate(KEEP_DIFF);
            rows.push(('…', format!("{more} more lines")));
        }
        cell.tags.push(format!("+{added} −{removed}"));
        cell.body = Body::Diff(rows);
        cell
    }

    /// A turn ended: how. A reply or question shown as it streamed is not
    /// repeated.
    pub(super) fn end(&mut self, end: &TurnEnd) {
        self.close_live();
        let streamed = self.last_streamed.take();
        self.calls.clear();
        self.response_text = false;
        // Tool cells still open when the turn ended.
        for c in self.list.iter_mut().rev().take(64) {
            if c.state == State::Live {
                c.state = State::Stopped;
                c.touch();
            }
        }
        match end {
            TurnEnd::Replied { message } => {
                if !streamed.is_some_and(|s| same(&s, message)) {
                    let mut cell = Cell::new(Kind::Duet, "duet");
                    cell.body = Body::Markdown(message.trim().to_owned());
                    self.push(cell);
                }
            }
            TurnEnd::Asked { question } => {
                if !streamed.is_some_and(|s| same(&s, question)) {
                    let mut cell = Cell::new(Kind::Asks, "duet asks");
                    cell.body = Body::Markdown(question.trim().to_owned());
                    self.push(cell);
                }
                let mut hint = Cell::new(Kind::Note, "");
                hint.body = Body::Lines(vec!["your next message is the answer".into()]);
                self.push(hint);
            }
            TurnEnd::Completed { summary } => {
                let mut cell = Cell::new(Kind::Done, "done");
                cell.body = Body::Markdown(summary.trim().to_owned());
                self.push(cell);
            }
            other => {
                let kind = if matches!(other, TurnEnd::Failed { .. }) {
                    Kind::Failed
                } else {
                    Kind::Warn
                };
                let text = crate::term::words::describe_end(other);
                self.push(Cell::new(kind, "").with_lines(&text));
            }
        }
    }

    /// Every row of the conversation at `width` (cached per cell).
    pub(super) fn rows(&mut self, width: usize, details: bool) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for i in 0..self.list.len() {
            let key = (width, details, self.list[i].version);
            let fresh = !matches!(&self.cache[i], Some((k, _)) if *k == key);
            if fresh {
                let rows = self.render(&self.list[i], width, details);
                self.cache[i] = Some((key, rows));
            }
            let joined = i > 0 && joins(&self.list[i - 1], &self.list[i]);
            if i > 0 && !joined {
                out.push(Line::default());
            }
            if let Some((_, rows)) = &self.cache[i] {
                out.extend(rows.iter().cloned());
            }
        }
        out
    }

    fn render(&self, c: &Cell, width: usize, details: bool) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        let body_width = width.saturating_sub(INDENT).max(8);
        if let Some(title) = title_line(c) {
            rows.extend(ansi_wrap(title, width, INDENT));
        }
        let indent = || Span::raw(" ".repeat(INDENT));
        match &c.body {
            Body::None => {}
            Body::Markdown(md) => {
                let lines =
                    Markdown::new(body_width, self.colour, Vec::<TSpan>::new(), 0).render(md);
                for l in lines {
                    let mut line = ansi::line(&l);
                    line.spans.insert(0, indent());
                    rows.push(line);
                }
                if c.state == State::Live {
                    rows.push(Line::from(vec![
                        indent(),
                        Span::styled("▍", Style::new().fg(ACCENT)),
                    ]));
                }
            }
            Body::Lines(lines) => {
                let (lead, style) = match c.kind {
                    Kind::You => ("│ ", Style::new()),
                    Kind::Output => ("│ ", palette::muted()),
                    Kind::Held => ("  ", Style::new().fg(HELD)),
                    Kind::Note => ("", palette::muted()),
                    Kind::Warn => ("  ", Style::new().fg(WARN)),
                    Kind::Failed => ("  ", Style::new().fg(BAD)),
                    Kind::Steer => ("  ", Style::new().fg(ACCENT)),
                    _ => ("  ", Style::new()),
                };
                let lead_style = match c.kind {
                    Kind::You => Style::new().fg(ACCENT),
                    _ => palette::muted(),
                };
                let first_rows = c.kind == Kind::Note && c.title.is_empty();
                // Output printed elsewhere folds like a tool's result.
                let limit = if c.kind == Kind::Output && !details {
                    FOLDED_OUTPUT
                } else {
                    usize::MAX
                };
                for l in lines.iter().take(limit) {
                    let line = Line::from(Span::styled(l.clone(), style));
                    let lead = if first_rows { "· " } else { lead };
                    let lead_width = lead.chars().count();
                    for (j, mut row) in ansi::wrap(&line, width.saturating_sub(lead_width).max(8))
                        .into_iter()
                        .enumerate()
                    {
                        let shown = if j == 0 || c.kind == Kind::You || c.kind == Kind::Output {
                            lead.to_owned()
                        } else {
                            " ".repeat(lead_width)
                        };
                        row.spans.insert(0, Span::styled(shown, lead_style));
                        rows.push(row);
                    }
                }
                let hidden = lines.len().saturating_sub(limit);
                if hidden > 0 {
                    rows.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!(
                                "… {hidden} more line{} · Ctrl-O",
                                if hidden == 1 { "" } else { "s" }
                            ),
                            palette::muted().add_modifier(Modifier::ITALIC),
                        ),
                    ]));
                }
            }
            Body::Diff(diff) => {
                for (sign, text) in diff {
                    let (mark, style) = match sign {
                        '+' => ("+ ", Style::new().fg(GOOD).bg(Color::Rgb(16, 44, 24))),
                        '-' => ("- ", Style::new().fg(BAD).bg(Color::Rgb(52, 16, 16))),
                        _ => ("  ", palette::muted()),
                    };
                    let shown = if *sign == '…' {
                        format!("{mark}{}", if text.is_empty() { "⋯" } else { text })
                    } else {
                        format!("{mark}{text}")
                    };
                    for mut row in ansi::wrap(&Line::from(shown), body_width) {
                        // A full-width band: the row padded to the edge.
                        let used: usize = row.width();
                        row.spans
                            .push(Span::raw(" ".repeat(body_width.saturating_sub(used))));
                        for s in &mut row.spans {
                            s.style = style;
                        }
                        row.spans.insert(0, indent());
                        rows.push(row);
                    }
                }
            }
        }
        if !c.result.is_empty() {
            let limit = match (details, c.state) {
                (true, _) => usize::MAX,
                (false, State::Failed) => FOLDED_FAILURE,
                _ if c.kind == Kind::Edit => 1,
                _ => FOLDED,
            };
            let style = if c.state == State::Failed {
                Style::new().fg(BAD)
            } else {
                palette::muted()
            };
            for l in c.result.iter().take(limit) {
                // Every row of a result sits on the same rule.
                for mut row in ansi::wrap(
                    &Line::from(Span::styled(l.clone(), style)),
                    body_width.saturating_sub(2),
                ) {
                    row.spans.insert(0, Span::styled("  │ ", palette::muted()));
                    rows.push(row);
                }
            }
            let hidden = c.result.len().saturating_sub(limit);
            if hidden > 0 {
                rows.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        format!(
                            "… {hidden} more line{} · Ctrl-O",
                            if hidden == 1 { "" } else { "s" }
                        ),
                        palette::muted().add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
        }
        rows
    }
}

/// Cells drawn without a blank line between them (a note under what it
/// explains, captured output in one block).
fn joins(before: &Cell, after: &Cell) -> bool {
    matches!(
        (before.kind, after.kind),
        (Kind::Asks | Kind::Duet, Kind::Note) | (Kind::Output, Kind::Output)
    ) && !(after.kind == Kind::Note && !after.title.is_empty())
}

/// A cell's title line: its mark, what it is, a detail, tags.
fn title_line(c: &Cell) -> Option<Line<'static>> {
    let (mark, style) = match (c.kind, c.state) {
        (Kind::Note | Kind::Output, _) if c.title.is_empty() => return None,
        (_, State::Failed) => ("✗", Style::new().fg(BAD).add_modifier(Modifier::BOLD)),
        (_, State::Stopped) => ("■", Style::new().fg(WARN).add_modifier(Modifier::BOLD)),
        (Kind::You, _) => ("›", palette::accent()),
        (Kind::Duet, _) => ("◆", palette::accent()),
        (Kind::Asks, _) => ("◆", Style::new().fg(WARN).add_modifier(Modifier::BOLD)),
        (Kind::Done, _) => ("✓", Style::new().fg(GOOD).add_modifier(Modifier::BOLD)),
        (Kind::Tool | Kind::Edit, State::Live) => ("○", palette::muted()),
        (Kind::Tool | Kind::Edit, _) => ("●", Style::new().fg(GOOD)),
        (Kind::Held, _) => ("◦", Style::new().fg(HELD).add_modifier(Modifier::BOLD)),
        (Kind::Steer, _) => ("▸", palette::accent()),
        (Kind::Warn, _) => ("■", Style::new().fg(WARN).add_modifier(Modifier::BOLD)),
        (Kind::Failed, _) => ("✗", Style::new().fg(BAD).add_modifier(Modifier::BOLD)),
        (Kind::Note | Kind::Output, _) => ("·", palette::muted()),
    };
    let title_style = match c.kind {
        Kind::You | Kind::Duet | Kind::Steer => palette::accent(),
        Kind::Asks => Style::new().fg(WARN).add_modifier(Modifier::BOLD),
        Kind::Done => Style::new().fg(GOOD).add_modifier(Modifier::BOLD),
        Kind::Held => Style::new().fg(HELD).add_modifier(Modifier::BOLD),
        Kind::Warn => Style::new().fg(WARN),
        Kind::Failed => Style::new().fg(BAD),
        _ => Style::new().add_modifier(Modifier::BOLD),
    };
    let mut spans = vec![
        Span::styled(format!("{mark} "), style),
        Span::styled(c.title.clone(), title_style),
    ];
    if !c.detail.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::raw(c.detail.clone()));
    }
    for t in &c.tags {
        let style = if t.starts_with('+') {
            palette::muted()
        } else if t == "stopped" {
            Style::new().fg(WARN)
        } else {
            Style::new().fg(HELD)
        };
        spans.push(Span::styled(format!("  ◦ {t}"), style));
    }
    Some(Line::from(spans))
}

/// A line wrapped at `width`, continuation rows indented by `hang`.
fn ansi_wrap(line: Line<'static>, width: usize, hang: usize) -> Vec<Line<'static>> {
    let mut rows = ansi::wrap_widths(&line, width, width.saturating_sub(hang));
    for r in rows.iter_mut().skip(1) {
        r.spans.insert(0, Span::raw(" ".repeat(hang)));
    }
    rows
}

fn first_line(text: &str, max: usize) -> String {
    let line = safe(
        text.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim(),
    );
    if line.chars().count() > max {
        format!("{}…", line.chars().take(max).collect::<String>())
    } else {
        line
    }
}

/// Whether two texts say the same (ignoring how whitespace was laid out).
fn same(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}
