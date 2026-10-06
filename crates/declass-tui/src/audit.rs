// SPDX-License-Identifier: GPL-3.0-or-later
//! Audit viewer: the workspace's run audit logs, each record (outbound request
//! summaries show the stored, placeholder-substituted text only), and chain +
//! anchor verification.

use crate::app::Paths;
use declass_boundary::audit::{
    Line as Record, describe_line, parse_line, run_anchors, verify_report,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use std::path::{Path, PathBuf};

/// Messages of a request shown in its summary (the latest ones).
const SHOWN_MESSAGES: usize = 6;
/// Characters shown per message.
const MESSAGE_CHARS: usize = 400;

/// Keep display summaries alongside their source, so repainting the list does
/// not parse every full request body again.
struct AuditLine {
    raw: String,
    summary: String,
}

#[derive(Default)]
pub struct AuditView {
    pub(crate) runs: Vec<String>,
    pub(crate) run: usize,
    /// Records of the selected run's log, with cached list summaries.
    lines: Vec<AuditLine>,
    loaded: Option<String>,
    /// Only the selected record needs its expanded detail prepared.
    detail: Option<(usize, Vec<String>)>,
    pub(crate) record: usize,
    pub(crate) focus_records: bool,
    /// Run id, exit code and report of the last verification.
    pub(crate) verified: Option<(String, i32, Vec<String>)>,
}

fn audit_dir(ws: &Path) -> PathBuf {
    ws.join(".declass/audit")
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
            self.loaded = None;
            self.detail = None;
            self.record = 0;
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
            .map(|line| AuditLine {
                raw: line.to_owned(),
                summary: describe_line(line),
            })
            .collect();
        self.record = 0;
        self.loaded = Some(id);
        self.detail = None;
    }

    fn selected_detail(&mut self) -> &[String] {
        if self.detail.as_ref().map(|(record, _)| *record) != Some(self.record) {
            self.detail = Some((
                self.record,
                self.lines
                    .get(self.record)
                    .map(|line| detail(&line.raw))
                    .unwrap_or_default(),
            ));
        }
        &self.detail.as_ref().unwrap().1
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
        let anchors = run_anchors(&paths.state, &paths.workspace, &id);
        self.verified = Some(match verify_report(&log, &anchors) {
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
            // `messages` (Chat Completions, Anthropic Messages) or `input` (Responses).
            let messages = r
                .request
                .get("messages")
                .or_else(|| r.request.get("input"))
                .and_then(|m| m.as_array())
                .map(Vec::as_slice)
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
                let role = m
                    .get("role")
                    .or_else(|| m.get("type"))
                    .and_then(|r| r.as_str())
                    .unwrap_or("?");
                let content = match m.get("content").or_else(|| m.get("output")) {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) if !other.is_null() => other.to_string(),
                    _ => String::new(),
                };
                // Chat Completions `tool_calls`, Anthropic `tool_use` blocks, a
                // Responses `function_call` item.
                let blocks = m.get("content").and_then(|c| c.as_array());
                let calls = m
                    .get("tool_calls")
                    .and_then(|c| c.as_array())
                    .map(|c| {
                        c.iter()
                            .filter_map(|c| c.pointer("/function/name").and_then(|n| n.as_str()))
                            .collect::<Vec<_>>()
                    })
                    .or_else(|| {
                        blocks.map(|b| {
                            b.iter()
                                .filter(|b| {
                                    b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                                })
                                .filter_map(|b| b.get("name").and_then(|n| n.as_str()))
                                .collect()
                        })
                    })
                    .or_else(|| {
                        (m.get("type").and_then(|t| t.as_str()) == Some("function_call"))
                            .then(|| m.get("name").and_then(|n| n.as_str()).into_iter().collect())
                    })
                    .map(|c: Vec<&str>| c.join(", "))
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
                crate::ui::frame()
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
        .map(|line| ListItem::new(line.summary.as_str()))
        .collect();
    let mut rstate = ListState::default().with_selected(Some(view.record));
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(
                crate::ui::frame()
                    .title(" records (Enter focuses, Esc returns) ")
                    .border_style(focus(view.focus_records)),
            ),
        records,
        &mut rstate,
    );
    let text: Vec<Line> = view
        .selected_detail()
        .iter()
        .map(|line| Line::from(line.as_str()))
        .collect();
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(crate::ui::frame().title(" record ")),
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
                .block(crate::ui::frame().title(format!(" verify {id} "))),
            verify_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use declass_boundary::audit::AuditLog;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn screen(view: &mut AuditView) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| draw(frame, view, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn cached_audit_view_tracks_navigation_refresh_and_removed_runs() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        std::fs::create_dir_all(audit_dir(ws)).unwrap();
        let first = audit_dir(ws).join("001.jsonl");
        let second = audit_dir(ws).join("002.jsonl");
        let mut log = AuditLog::open(&first).unwrap();
        for text in ["first message", "second message"] {
            log.append(
                "https://f.example/v1",
                "model",
                json!({"messages": [{"role": "user", "content": text}]}),
                vec![],
            )
            .unwrap();
        }
        let mut view = AuditView::default();
        view.refresh(ws);
        assert!(screen(&mut view).contains("first message"));
        // Repaints reuse both the prepared list and selected record detail.
        assert!(screen(&mut view).contains("first message"));
        view.focus_records = true;
        view.move_by(1, ws);
        assert!(screen(&mut view).contains("second message"));
        view.move_by(-1, ws);
        assert!(screen(&mut view).contains("first message"));

        AuditLog::open(&second)
            .unwrap()
            .append(
                "https://f.example/v1",
                "model",
                json!({"messages": [{"role": "user", "content": "new run"}]}),
                vec![],
            )
            .unwrap();
        view.refresh(ws);
        assert!(screen(&mut view).contains("new run"));
        view.focus_records = false;
        view.move_by(1, ws);
        assert!(screen(&mut view).contains("first message"));

        // A malformed record stays visible and can be selected after refresh.
        std::fs::write(&first, "invalid json\n").unwrap();
        view.refresh(ws);
        assert!(screen(&mut view).contains("unparseable record"));
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
        view.refresh(ws);
        let empty = screen(&mut view);
        assert!(empty.contains("runs: none yet"));
        assert!(!empty.contains("first message"));
        assert!(!empty.contains("unparseable record"));
        assert!(view.lines.is_empty());
    }

    fn shown(body: serde_json::Value) -> Vec<String> {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("a.jsonl");
        AuditLog::open(&path)
            .unwrap()
            .append("https://f.example/v1", "m", body, vec![])
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        detail(text.lines().next().unwrap())
    }

    #[test]
    fn requests_of_every_dialect_show_their_messages_and_calls() {
        let anthropic = shown(json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "fix it"}]},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "t", "name": "read_file", "input": {}}]}]}));
        assert!(
            anthropic.iter().any(|l| l.contains("2 message(s)")),
            "{anthropic:?}"
        );
        assert!(
            anthropic.iter().any(|l| l.contains("[calls read_file]")),
            "{anthropic:?}"
        );
        let responses = shown(json!({"input": [
            {"role": "user", "content": "fix it"},
            {"type": "function_call", "call_id": "c", "name": "run_command", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c", "output": "ok"}]}));
        assert!(
            responses.iter().any(|l| l.contains("3 message(s)")),
            "{responses:?}"
        );
        assert!(
            responses.iter().any(|l| l.contains("[calls run_command]")),
            "{responses:?}"
        );
        assert!(
            responses
                .iter()
                .any(|l| l.contains("function_call_output: ok")),
            "{responses:?}"
        );
    }
}
