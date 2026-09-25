// SPDX-License-Identifier: GPL-3.0-or-later
//! Run view (read-only), in two panels. The main panel follows the run's
//! transcript and audit log as they grow — turns, tool calls and results —
//! above a feed of withheld content (how results were shown, gate
//! interventions, security events). The side panel lists the files the run
//! changed with added/removed line counts, and below it the selected file's
//! diff with line numbers; while following, it moves to the file the run
//! wrote last. Files that are sensitive or derived from sensitive data are
//! listed as held locally and their content is never shown. A finished run
//! reads the same way.

use crate::audit::list_runs;
use crate::changes::{self, ChangedFile, Held, RowKind, Status};
use duet_agent::transcript::{Entry, Transcript};
use duet_boundary::audit::{self, AuditEvent, Line as Record};
use duet_boundary::model::Item;
use duet_boundary::policy::Policy;
use duet_boundary::view::ViewClass;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Characters shown of a message or result line.
const LINE_CHARS: usize = 160;

/// Which panel the arrow keys move in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Main,
    Side,
}

pub struct RunView {
    pub(crate) runs: Vec<String>,
    pub(crate) selected: usize,
    /// Sizes of the transcript and audit log when last read.
    seen: Option<(String, u64, u64)>,
    pub(crate) feed: Vec<String>,
    pub(crate) withheld: Vec<String>,
    pub(crate) follow: bool,
    scroll: usize,
    pub(crate) focus: Focus,
    /// The run whose files are listed, and the files.
    files_of: Option<String>,
    pub(crate) files: Vec<ChangedFile>,
    /// Derived files named by the audit log's sensitive-command events.
    derived_audit: Vec<String>,
    pub(crate) file: usize,
    pub(crate) diff_scroll: usize,
}

impl Default for RunView {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            selected: 0,
            seen: None,
            feed: Vec::new(),
            withheld: Vec::new(),
            follow: true,
            scroll: 0,
            focus: Focus::Main,
            files_of: None,
            files: Vec::new(),
            derived_audit: Vec::new(),
            file: 0,
            diff_scroll: 0,
        }
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.chars().count() > LINE_CHARS {
        format!("{}…", line.chars().take(LINE_CHARS).collect::<String>())
    } else {
        line.to_owned()
    }
}

fn len(p: &Path) -> u64 {
    std::fs::metadata(p).map_or(0, |m| m.len())
}

impl RunView {
    fn paths(ws: &Path, id: &str) -> (PathBuf, PathBuf) {
        (
            ws.join(".duet/runs").join(id),
            ws.join(".duet/audit").join(format!("{id}.jsonl")),
        )
    }

    /// Re-lists runs, re-reads the selected one when its files grew, and
    /// brings the changed-files panel up to date. `policy` decides which
    /// files are held locally.
    pub fn refresh(&mut self, ws: &Path, policy: &Policy) {
        let current = self.runs.get(self.selected).cloned();
        self.runs = list_runs(&ws.join(".duet/runs"), None);
        self.selected = current
            .and_then(|c| self.runs.iter().position(|r| *r == c))
            .unwrap_or(0);
        let Some(id) = self.runs.get(self.selected).cloned() else {
            self.feed.clear();
            self.withheld.clear();
            self.files.clear();
            self.files_of = None;
            return;
        };
        let (run_dir, log) = Self::paths(ws, &id);
        let stamp = (
            id.clone(),
            len(&run_dir.join("transcript.jsonl")),
            len(&log),
        );
        if self.seen.as_ref() != Some(&stamp) {
            self.seen = Some(stamp);
            let entries = Transcript::read(&run_dir).unwrap_or_default();
            let records = audit::read(&log).unwrap_or_default();
            (self.feed, self.withheld) = feeds(&entries, &records);
            self.derived_audit = records
                .iter()
                .filter_map(|r| match r {
                    Record::Event(e) => match &e.event {
                        AuditEvent::SensitiveCommand { derived_files, .. } => {
                            Some(derived_files.clone())
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .flatten()
                .collect();
        }
        if self.files_of.as_deref() != Some(id.as_str()) {
            self.files_of = Some(id.clone());
            self.files.clear();
            self.file = 0;
            self.diff_scroll = 0;
        }
        let derived: BTreeSet<String> = changes::derived_files(&run_dir, &self.derived_audit);
        changes::update(&mut self.files, ws, &run_dir, policy, &derived);
        if self.follow
            && let Some(latest) = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.last > 0)
                .max_by_key(|(_, f)| f.last)
                .map(|(i, _)| i)
            && latest != self.file
        {
            self.file = latest;
            self.diff_scroll = 0;
        }
        self.file = self.file.min(self.files.len().saturating_sub(1));
    }

    pub fn select_by(&mut self, d: isize, ws: &Path, policy: &Policy) {
        self.selected = self
            .selected
            .saturating_add_signed(d)
            .min(self.runs.len().saturating_sub(1));
        self.seen = None;
        self.scroll = 0;
        self.refresh(ws, policy);
    }

    /// Drops what was read, so the next refresh re-lists and re-reads.
    pub fn forget(&mut self) {
        self.seen = None;
        self.files_of = None;
    }

    pub fn scroll_by(&mut self, d: isize) {
        self.follow = false;
        self.scroll = self
            .scroll
            .saturating_add_signed(d)
            .min(self.feed.len().saturating_sub(1));
    }

    /// Arrow keys: in the main panel they scroll the feed; in the side panel
    /// one step selects a file and a page scrolls its diff.
    pub fn move_by(&mut self, d: isize) {
        match self.focus {
            Focus::Main => self.scroll_by(d),
            Focus::Side if d.abs() >= 10 => self.diff_by(d),
            Focus::Side => {
                self.follow = false;
                let next = self
                    .file
                    .saturating_add_signed(d)
                    .min(self.files.len().saturating_sub(1));
                if next != self.file {
                    self.file = next;
                    self.diff_scroll = 0;
                }
            }
        }
    }

    pub fn diff_by(&mut self, d: isize) {
        let rows = self.files.get(self.file).map_or(0, |f| f.rows.len());
        self.diff_scroll = self
            .diff_scroll
            .saturating_add_signed(d)
            .min(rows.saturating_sub(1));
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Main => Focus::Side,
            Focus::Side => Focus::Main,
        };
    }
}

/// The conversation feed and the withheld-content feed of a run.
pub(crate) fn feeds(entries: &[Entry], records: &[Record]) -> (Vec<String>, Vec<String>) {
    let mut feed = Vec::new();
    let mut withheld = Vec::new();
    let mut turn = 0;
    for e in entries {
        match e {
            Entry::Start {
                objective,
                mode,
                frontier_model,
            } => feed.push(format!(
                "start ({mode}, {frontier_model}): {}",
                first_line(objective)
            )),
            Entry::Item { item } => match item {
                Item::User { text } => feed.push(format!("user: {}", first_line(text))),
                Item::Assistant {
                    text, tool_calls, ..
                } => {
                    turn += 1;
                    feed.push(format!("turn {turn}: {}", first_line(text)));
                    for c in tool_calls {
                        feed.push(format!("  -> {} {}", c.name, first_line(&c.raw_arguments)));
                    }
                }
                Item::ToolResult { call_id, content } => feed.push(format!(
                    "  <- {call_id}: {} ({} lines)",
                    first_line(content),
                    content.lines().count()
                )),
            },
            Entry::Usage {
                turn: t,
                cost_usd,
                interventions,
                ..
            } => {
                feed.push(format!("  turn {t} cost ${cost_usd:.4}"));
                withheld.extend(interventions.iter().map(|i| format!("turn {t}: {i}")));
            }
            Entry::FailedAttempts {
                turn: t, cost_usd, ..
            } => feed.push(format!(
                "  turn {t} failed attempts (estimated) ${cost_usd:.4}"
            )),
            Entry::Interrupted { call_id, tool } => {
                feed.push(format!("  interrupted {tool} ({call_id})"))
            }
            Entry::Masked {
                items,
                tokens_before,
                tokens_after,
            } => feed.push(format!(
                "masked {items} old result(s): {tokens_before} -> {tokens_after} tokens"
            )),
            Entry::Shown { call_id, class } if *class != ViewClass::Raw => {
                withheld.push(format!("{call_id}: shown as {class:?}"))
            }
            Entry::Shown { .. } => {}
            Entry::End { terminal } => feed.push(format!(
                "end: {}",
                serde_json::to_string(terminal).unwrap_or_default()
            )),
        }
    }
    for r in records {
        match r {
            Record::Request(q) if !q.interventions.is_empty() => withheld.push(format!(
                "request #{}: {}",
                q.seq,
                q.interventions.join("; ")
            )),
            Record::Event(e) => match &e.event {
                AuditEvent::BlockedSend { check } => {
                    withheld.push(format!("#{} blocked send: {check}", e.seq))
                }
                AuditEvent::SensitiveCommand {
                    command,
                    derived_files,
                    ..
                } => withheld.push(format!(
                    "#{} sensitive command held locally: {command} ({} derived file(s))",
                    e.seq,
                    derived_files.len()
                )),
                AuditEvent::SandboxDenial { command, access } => {
                    withheld.push(format!("#{} sandbox denied {access}: {command}", e.seq))
                }
                AuditEvent::ProtectedEdit { path, attempt, .. } => withheld.push(format!(
                    "#{} protected edit {path} (attempt {attempt})",
                    e.seq
                )),
                _ => {}
            },
            _ => {}
        }
    }
    (feed, withheld)
}

fn focus_style(on: bool) -> Style {
    if on {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

pub(crate) fn draw(f: &mut Frame, view: &RunView, area: Rect) {
    let [main, side] =
        Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)]).areas(area);
    draw_main(f, view, main);
    draw_side(f, view, side);
}

fn draw_main(f: &mut Frame, view: &RunView, area: Rect) {
    let withheld_height = (area.height / 3).max(4);
    let [feed_area, withheld_area] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(withheld_height)]).areas(area);
    let title = match view.runs.get(view.selected) {
        None => " run: none yet (n starts one; runs appear under .duet/runs) ".to_owned(),
        Some(id) => format!(
            " run {id} ({}/{}, follow {}) ",
            view.selected + 1,
            view.runs.len(),
            if view.follow { "on" } else { "off" }
        ),
    };
    let height = feed_area.height.saturating_sub(2) as usize;
    let scroll = if view.follow {
        view.feed.len().saturating_sub(height)
    } else {
        view.scroll
    };
    let feed: Vec<Line> = view.feed.iter().map(|l| Line::from(l.as_str())).collect();
    f.render_widget(
        Paragraph::new(feed)
            .scroll((scroll.min(u16::MAX as usize) as u16, 0))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(focus_style(view.focus == Focus::Main)),
            ),
        feed_area,
    );
    let withheld: Vec<Line> = view
        .withheld
        .iter()
        .map(|l| Line::styled(l.as_str(), Style::new().fg(Color::Yellow)))
        .collect();
    let skip = if view.follow {
        view.withheld
            .len()
            .saturating_sub(withheld_area.height.saturating_sub(2) as usize)
    } else {
        0
    };
    f.render_widget(
        Paragraph::new(withheld)
            .wrap(Wrap { trim: false })
            .scroll((skip.min(u16::MAX as usize) as u16, 0))
            .block(Block::bordered().title(" withheld and blocked ")),
        withheld_area,
    );
}

fn status_mark(file: &ChangedFile) -> (&'static str, Color) {
    match (file.held, file.status) {
        (Some(_), _) => ("held", Color::Yellow),
        (None, Status::Created) => ("new ", Color::Green),
        (None, Status::Deleted) => ("gone", Color::Red),
        (None, Status::Unchanged) => ("same", Color::DarkGray),
        (None, Status::ByCommand) => ("cmd ", Color::Yellow),
        (None, Status::Edited) => ("edit", Color::Cyan),
    }
}

fn draw_side(f: &mut Frame, view: &RunView, area: Rect) {
    let list_height = (view.files.len() as u16 + 2).clamp(3, (area.height / 2).max(3));
    let [list_area, diff_area] =
        Layout::vertical([Constraint::Length(list_height), Constraint::Min(3)]).areas(area);
    let focused = view.focus == Focus::Side;
    let (added, removed) = view
        .files
        .iter()
        .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
    let items: Vec<ListItem> = view
        .files
        .iter()
        .map(|file| {
            let (mark, color) = status_mark(file);
            let mut spans = vec![
                Span::styled(format!("{mark} "), Style::new().fg(color)),
                Span::raw(file.path.clone()),
            ];
            if file.held.is_some() {
                spans.push(Span::styled(
                    "  held locally",
                    Style::new().fg(Color::Yellow),
                ));
            } else {
                if file.added > 0 {
                    spans.push(Span::styled(
                        format!("  +{}", file.added),
                        Style::new().fg(Color::Green),
                    ));
                }
                if file.removed > 0 {
                    spans.push(Span::styled(
                        format!("  -{}", file.removed),
                        Style::new().fg(Color::Red),
                    ));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let title = if view.files.is_empty() {
        " changed files: none yet ".to_owned()
    } else {
        format!(" changed files {} (+{added} -{removed}) ", view.files.len())
    };
    let mut state =
        ListState::default().with_selected((!view.files.is_empty()).then_some(view.file));
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(
                Block::bordered()
                    .title(title)
                    .border_style(focus_style(focused)),
            ),
        list_area,
        &mut state,
    );
    draw_diff(f, view, diff_area);
}

fn draw_diff(f: &mut Frame, view: &RunView, area: Rect) {
    let Some(file) = view.files.get(view.file) else {
        f.render_widget(
            Paragraph::new("The files this run writes appear here with their diffs.")
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" diff ")),
            area,
        );
        return;
    };
    if let Some(held) = file.held {
        let why = match held {
            Held::Sensitive => "it matches the sensitivity rules",
            Held::Derived => "a command that could read sensitive data created or changed it",
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::styled("held locally", Style::new().fg(Color::Yellow)),
                Line::from(format!("{}: {why}; its content is not shown.", file.path)),
            ])
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(format!(" {} ", file.path))),
            area,
        );
        return;
    }
    let width = file
        .rows
        .iter()
        .filter_map(|r| r.old.max(r.new))
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let number = |n: Option<u32>| n.map_or(" ".repeat(width), |n| format!("{n:>width$}"));
    let visible = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = file
        .rows
        .iter()
        .skip(view.diff_scroll)
        .take(visible)
        .map(|r| {
            let gutter = format!("{} {} ", number(r.old), number(r.new));
            match r.kind {
                RowKind::Hunk => Line::styled(
                    format!("{:┄<w$} {}", "", r.text, w = width * 2 + 1),
                    Style::new().fg(Color::DarkGray),
                ),
                RowKind::Note => Line::styled(
                    format!("{gutter}  {}", r.text),
                    Style::new()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
                RowKind::Added => Line::from(vec![
                    Span::styled(gutter, Style::new().fg(Color::DarkGray)),
                    Span::styled(
                        format!("+ {}", r.text),
                        Style::new().fg(Color::Green).bg(Color::Rgb(16, 48, 24)),
                    ),
                ]),
                RowKind::Removed => Line::from(vec![
                    Span::styled(gutter, Style::new().fg(Color::DarkGray)),
                    Span::styled(
                        format!("- {}", r.text),
                        Style::new().fg(Color::Red).bg(Color::Rgb(56, 16, 16)),
                    ),
                ]),
                RowKind::Context => Line::from(vec![
                    Span::styled(gutter, Style::new().fg(Color::DarkGray)),
                    Span::raw(format!("  {}", r.text)),
                ]),
            }
        })
        .collect();
    let what = match file.status {
        Status::Created => "created",
        Status::Deleted => "deleted",
        Status::Unchanged => "back to its content before the run",
        Status::ByCommand => "changed by a command",
        Status::Edited => "edited",
    };
    let position = if file.rows.is_empty() {
        String::new()
    } else {
        format!(
            "; rows {}-{} of {}",
            view.diff_scroll + 1,
            (view.diff_scroll + visible).min(file.rows.len()),
            file.rows.len()
        )
    };
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(format!(" {} ({what}{position}) ", file.path))
                .border_style(focus_style(view.focus == Focus::Side)),
        ),
        area,
    );
}
