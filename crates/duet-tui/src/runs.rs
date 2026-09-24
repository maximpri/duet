// SPDX-License-Identifier: GPL-3.0-or-later
//! Run view (read-only): follows a run's transcript and audit log as they
//! grow — turns, tool calls and results — beside a feed of withheld content:
//! how results were shown, gate interventions and security events.

use crate::audit::list_runs;
use duet_agent::transcript::{Entry, Transcript};
use duet_boundary::audit::{self, AuditEvent, Line as Record};
use duet_boundary::model::Item;
use duet_boundary::view::ViewClass;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use std::path::{Path, PathBuf};

/// Characters shown of a message or result line.
const LINE_CHARS: usize = 160;

pub struct RunView {
    pub(crate) runs: Vec<String>,
    pub(crate) selected: usize,
    /// Sizes of the transcript and audit log when last read.
    seen: Option<(String, u64, u64)>,
    pub(crate) feed: Vec<String>,
    pub(crate) withheld: Vec<String>,
    pub(crate) follow: bool,
    scroll: usize,
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

    /// Re-lists runs and re-reads the selected one when its files grew.
    pub fn refresh(&mut self, ws: &Path) {
        let current = self.runs.get(self.selected).cloned();
        self.runs = list_runs(&ws.join(".duet/runs"), None);
        self.selected = current
            .and_then(|c| self.runs.iter().position(|r| *r == c))
            .unwrap_or(0);
        let Some(id) = self.runs.get(self.selected).cloned() else {
            self.feed.clear();
            self.withheld.clear();
            return;
        };
        let (run_dir, log) = Self::paths(ws, &id);
        let stamp = (
            id.clone(),
            len(&run_dir.join("transcript.jsonl")),
            len(&log),
        );
        if self.seen.as_ref() == Some(&stamp) {
            return;
        }
        self.seen = Some(stamp);
        let entries = Transcript::read(&run_dir).unwrap_or_default();
        let records = audit::read(&log).unwrap_or_default();
        (self.feed, self.withheld) = feeds(&entries, &records);
    }

    pub fn select_by(&mut self, d: isize, ws: &Path) {
        self.selected = self
            .selected
            .saturating_add_signed(d)
            .min(self.runs.len().saturating_sub(1));
        self.seen = None;
        self.scroll = 0;
        self.refresh(ws);
    }

    pub fn scroll_by(&mut self, d: isize) {
        self.follow = false;
        self.scroll = self
            .scroll
            .saturating_add_signed(d)
            .min(self.feed.len().saturating_sub(1));
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

pub(crate) fn draw(f: &mut Frame, view: &RunView, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);
    let title = match view.runs.get(view.selected) {
        None => " run: none yet (runs appear under .duet/runs) ".to_owned(),
        Some(id) => format!(
            " run {id} ({}/{}; [ ] switch; f follow: {}) ",
            view.selected + 1,
            view.runs.len(),
            if view.follow { "on" } else { "off" }
        ),
    };
    let height = left.height.saturating_sub(2) as usize;
    let scroll = if view.follow {
        view.feed.len().saturating_sub(height)
    } else {
        view.scroll
    };
    let feed: Vec<Line> = view.feed.iter().map(|l| Line::from(l.as_str())).collect();
    f.render_widget(
        Paragraph::new(feed)
            .scroll((scroll.min(u16::MAX as usize) as u16, 0))
            .block(Block::bordered().title(title)),
        left,
    );
    let withheld: Vec<Line> = view
        .withheld
        .iter()
        .map(|l| Line::styled(l.as_str(), Style::new().fg(Color::Yellow)))
        .collect();
    let skip = if view.follow {
        view.withheld
            .len()
            .saturating_sub(right.height.saturating_sub(2) as usize)
    } else {
        0
    };
    f.render_widget(
        Paragraph::new(withheld)
            .wrap(Wrap { trim: false })
            .scroll((skip.min(u16::MAX as usize) as u16, 0))
            .block(Block::bordered().title(" withheld and blocked ")),
        right,
    );
}
