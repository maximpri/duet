// SPDX-License-Identifier: GPL-3.0-or-later
//! Audit viewer: the workspace's run audit logs, each record (outbound request
//! summaries show the stored, placeholder-substituted text only), and chain +
//! anchor verification.

use crate::app::Paths;
use duet_boundary::audit::{Line as Record, anchor_path, describe_line, parse_line, verify_report};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use std::path::{Path, PathBuf};

/// Messages of a request shown in its summary (the latest ones).
const SHOWN_MESSAGES: usize = 6;
/// Characters shown per message.
const MESSAGE_CHARS: usize = 400;

#[derive(Default)]
pub struct AuditView {
    pub(crate) runs: Vec<String>,
    pub(crate) run: usize,
    /// Raw lines of the selected run's log.
    lines: Vec<String>,
    loaded: Option<String>,
    pub(crate) record: usize,
    pub(crate) focus_records: bool,
    /// Run id, exit code and report of the last verification.
    pub(crate) verified: Option<(String, i32, Vec<String>)>,
}

fn audit_dir(ws: &Path) -> PathBuf {
    ws.join(".duet/audit")
}

/// Run ids with an audit log, newest first (run ids start with a timestamp).
pub(crate) fn list_runs(dir: &Path, ext: Option<&str>) -> Vec<String> {
    let mut runs: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            match ext {
                Some(x) if p.extension().is_some_and(|e| e == x) => {
                    p.file_stem().map(|s| s.to_string_lossy().into_owned())
                }
                None if p.is_dir() => p.file_name().map(|s| s.to_string_lossy().into_owned()),
                _ => None,
            }
        })
        .collect();
    runs.sort_unstable_by(|a, b| b.cmp(a));
    runs
}

impl AuditView {
    pub fn refresh(&mut self, ws: &Path) {
        self.runs = list_runs(&audit_dir(ws), Some("jsonl"));
        self.run = self.run.min(self.runs.len().saturating_sub(1));
        self.loaded = None;
        self.load(ws);
    }

    fn load(&mut self, ws: &Path) {
        let Some(id) = self.runs.get(self.run).cloned() else {
            self.lines.clear();
            return;
        };
        if self.loaded.as_ref() == Some(&id) {
            return;
        }
        let text =
            std::fs::read_to_string(audit_dir(ws).join(format!("{id}.jsonl"))).unwrap_or_default();
        self.lines = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();
        self.record = 0;
        self.loaded = Some(id);
    }

    pub fn move_by(&mut self, d: isize, ws: &Path) {
        let step = |cur: usize, len: usize| cur.saturating_add_signed(d).min(len.saturating_sub(1));
        if self.focus_records {
            self.record = step(self.record, self.lines.len());
        } else {
            self.run = step(self.run, self.runs.len());
            self.load(ws);
        }
    }

    pub fn verify(&mut self, paths: &Paths) {
        let Some(id) = self.runs.get(self.run).cloned() else {
            return;
        };
        let log = audit_dir(&paths.workspace).join(format!("{id}.jsonl"));
        let anchor = anchor_path(&paths.state, &paths.workspace, &id);
        self.verified = Some(match verify_report(&log, &anchor) {
            Ok((code, lines)) => (id, code, lines),
            Err(e) => (id, 1, vec![e.to_string()]),
        });
    }
}

fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max {
        format!("{}…", one.chars().take(max).collect::<String>())
    } else {
        one
    }
}

/// The detail of one record: a request's endpoint, hash, interventions and its
/// latest messages as stored (after the gate's substitutions); an event's fields.
pub(crate) fn detail(line: &str) -> Vec<String> {
    match parse_line(line) {
        Some(Record::Request(r)) => {
            let mut out = vec![
                format!("request #{} to {} ({})", r.seq, r.endpoint, r.model),
                format!("sha256 {}", r.request_sha256),
            ];
            if r.interventions.is_empty() {
                out.push("interventions: none".into());
            } else {
                out.push("interventions:".into());
                out.extend(r.interventions.iter().map(|i| format!("  {i}")));
            }
            let messages = r
                .request
                .get("messages")
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();
            out.push(format!(
                "{} message(s), {} tool(s); latest as sent:",
                messages.len(),
                r.request
                    .get("tools")
                    .and_then(|t| t.as_array())
                    .map_or(0, Vec::len)
            ));
            for m in messages
                .iter()
                .skip(messages.len().saturating_sub(SHOWN_MESSAGES))
            {
                let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("?");
                let content = match m.get("content") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) if !other.is_null() => other.to_string(),
                    _ => String::new(),
                };
                let calls = m
                    .get("tool_calls")
                    .and_then(|c| c.as_array())
                    .map(|c| {
                        c.iter()
                            .filter_map(|c| c.pointer("/function/name").and_then(|n| n.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|c| !c.is_empty())
                    .map_or(String::new(), |c| format!(" [calls {c}]"));
                out.push(format!(
                    "  {role}: {}{calls}",
                    clip(&content, MESSAGE_CHARS)
                ));
            }
            out
        }
        Some(Record::Event(e)) => {
            let fields = serde_json::to_string_pretty(&e.event).unwrap_or_default();
            std::iter::once(format!("event #{}: {}", e.seq, e.event.kind()))
                .chain(fields.lines().map(str::to_owned))
                .collect()
        }
        None => vec!["unparseable record".into()],
    }
}

pub(crate) fn draw(f: &mut Frame, view: &mut AuditView, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(30), Constraint::Min(20)]).areas(area);
    let focus = |on: bool| {
        if on {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new()
        }
    };
    let runs: Vec<ListItem> = view
        .runs
        .iter()
        .map(|r| ListItem::new(r.as_str()))
        .collect();
    let empty = runs.is_empty();
    let mut state = ListState::default().with_selected((!empty).then_some(view.run));
    f.render_stateful_widget(
        List::new(runs)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(
                Block::bordered()
                    .title(if empty { " runs: none yet " } else { " runs " })
                    .border_style(focus(!view.focus_records)),
            ),
        left,
        &mut state,
    );
    let shown = view.verified.as_ref().map(|(id, ..)| id) == view.runs.get(view.run);
    let verify_height = if shown && view.verified.is_some() {
        6
    } else {
        0
    };
    let [records, detail_area, verify_area] = Layout::vertical([
        Constraint::Percentage(45),
        Constraint::Min(4),
        Constraint::Length(verify_height),
    ])
    .areas(right);
    let items: Vec<ListItem> = view
        .lines
        .iter()
        .map(|l| ListItem::new(describe_line(l)))
        .collect();
    let mut rstate = ListState::default().with_selected(Some(view.record));
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(
                Block::bordered()
                    .title(" records (Enter focuses, Esc returns) ")
                    .border_style(focus(view.focus_records)),
            ),
        records,
        &mut rstate,
    );
    let text: Vec<Line> = view
        .lines
        .get(view.record)
        .map(|l| detail(l))
        .unwrap_or_default()
        .into_iter()
        .map(Line::from)
        .collect();
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" record ")),
        detail_area,
    );
    if let Some((id, code, lines)) = view
        .verified
        .as_ref()
        .filter(|(id, ..)| view.runs.get(view.run) == Some(id))
    {
        let color = match code {
            0 => Color::Green,
            2 => Color::Yellow,
            _ => Color::Red,
        };
        let text: Vec<Line> = lines
            .iter()
            .map(|l| Line::styled(l.clone(), Style::new().fg(color)))
            .collect();
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(format!(" verify {id} "))),
            verify_area,
        );
    }
}
