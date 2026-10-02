// SPDX-License-Identifier: GPL-3.0-or-later
//! Operator-only data-flow history, built from transcripts and outbound audit
//! records. Never reads handle contents or restores placeholder values.

use super::cells::palette;
use crate::term::safe;
use duet_agent::transcript::Entry;
use duet_boundary::audit::{AuditEvent, Line as Record};
use duet_boundary::model::Item;
use duet_boundary::view::ViewClass;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

const KEEP: usize = 500;
const TEXT_LIMIT: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct FilterHistory(VecDeque<String>);

impl FilterHistory {
    pub(super) fn split(&mut self, scope: &str, notes: &[String]) -> (Vec<String>, Vec<String>) {
        let (mut new, mut repeated) = (Vec::new(), Vec::new());
        for note in notes {
            let note = bounded(note);
            // Counts from older releases have no source identity. Equal counts
            // do not establish that the same message was filtered again.
            if note.contains("model's own message") {
                new.push(note);
                continue;
            }
            let key = format!("{scope}\0{note}");
            if self.0.contains(&key) {
                repeated.push(note);
            } else {
                self.0.push_back(key);
                if self.0.len() > KEEP {
                    self.0.pop_front();
                }
                new.push(note);
            }
        }
        (new, repeated)
    }
}

pub(super) fn bounded(text: &str) -> String {
    let mut end = text.len().min(TEXT_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = safe(&text[..end]);
    if end < text.len() {
        out.push_str(
            "\n[Preview limited to 64 KiB; inspect the transcript or /audit for the full record.]",
        );
    }
    out
}

#[derive(Default)]
struct Activity {
    number: u64,
    scope: String,
    call_id: Option<String>,
    tool: String,
    request: u64,
    target: String,
    handle: Option<String>,
    inputs: Vec<(String, String)>,
    class: Option<ViewClass>,
    result: Option<String>,
    interrupted: bool,
    notice: bool,
}

impl Activity {
    fn visible(&self) -> bool {
        self.notice
            || self.class.is_some_and(|c| c != ViewClass::Raw)
            || matches!(
                self.tool.as_str(),
                "read_file"
                    | "read_raw"
                    | "ask_local"
                    | "run_command"
                    | "synthetic_sample"
                    | "explore"
                    | "edit_protected"
                    | "web_fetch"
                    | "web_search"
                    | "code_nav"
            )
    }

    fn outcome(&self) -> &'static str {
        if self.notice {
            return "Recorded decision";
        }
        if self.interrupted {
            return "Interrupted; no result recorded";
        }
        let Some(result) = &self.result else {
            return "Waiting for a result";
        };
        if result.starts_with("error:")
            || result.starts_with("checks failed")
            || result.starts_with("timed out")
        {
            return "Failed; see recorded error";
        }
        if let Some(code) = result
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("exit code "))
            && code.trim() != "0"
        {
            return "Command failed; see output";
        }
        "Result recorded"
    }
}

struct Sent {
    seq: u64,
    unix_ms: u128,
    model: String,
    text: String,
    interventions: Vec<String>,
}

#[derive(Default)]
pub(super) struct Privacy {
    events: VecDeque<Activity>,
    next: u64,
    selected: Option<u64>,
    scroll: usize,
    max_scroll: usize,
    expanded: bool,
    requests: BTreeMap<String, u64>,
    sent: BTreeMap<String, Sent>,
    ambiguous: BTreeSet<String>,
    audit_seq: u64,
    dropped: usize,
    filtering: FilterHistory,
}

impl Privacy {
    fn push(&mut self, mut event: Activity) {
        self.next += 1;
        event.number = self.next;
        self.events.push_back(event);
        if self.events.len() > KEEP {
            self.events.pop_front();
            self.dropped += 1;
        }
    }

    fn call_mut(&mut self, scope: &str, id: &str) -> Option<&mut Activity> {
        self.events
            .iter_mut()
            .rev()
            .find(|e| e.scope == scope && e.call_id.as_deref() == Some(id))
    }

    fn note(&mut self, scope: &str, title: &str, detail: String) {
        self.push(Activity {
            scope: scope.into(),
            tool: title.into(),
            result: Some(bounded(&detail)),
            notice: true,
            ..Default::default()
        });
    }

    pub(super) fn entry(&mut self, entry: &Entry) {
        self.scoped_entry("", entry);
    }

    fn scoped_entry(&mut self, scope: &str, entry: &Entry) {
        match entry {
            Entry::Subagent { child, entry } => self.scoped_entry(child, entry),
            Entry::Usage { turn, interventions, .. } => {
                self.requests.insert(scope.into(), *turn);
                if !interventions.is_empty() {
                    let (new, repeated) = self.filtering.split(scope, interventions);
                    let title = format!("Request {turn}: {} new, {} repeated details", new.len(), repeated.len());
                    let mut detail = String::new();
                    if !new.is_empty() { detail.push_str(&format!("New filtering details in this view:\n{}\n", new.join("\n\n"))); }
                    if !repeated.is_empty() { detail.push_str(&format!("\nSame recorded filtering details seen earlier (history is checked on every request):\n{}", repeated.join("\n\n"))); }
                    if interventions.iter().any(|s| s.contains("model's own message")) {
                        detail.push_str("\n\nThis older record does not identify the affected message or field; those details cannot be reconstructed from the count alone.");
                    }
                    self.note(scope, &title, detail);
                }
            }
            Entry::Item { item: Item::Assistant { tool_calls, .. } } => {
                for call in tool_calls {
                    if self.events.iter().any(|e| e.call_id.as_deref() == Some(&call.id)) {
                        self.ambiguous.insert(call.id.clone());
                    }
                    let a = &call.arguments;
                    let get = |key: &str| a.get(key).and_then(Value::as_str).unwrap_or("");
                    let handle = get("handle");
                    let target = [get("path"), handle, get("url"), get("server")].into_iter()
                        .find(|s| !s.is_empty()).unwrap_or("");
                    let mut inputs = Vec::new();
                    for (key, label) in [("question", "Question"), ("questions", "Questions"),
                        ("command", "Command"), ("pattern", "Search"), ("start_line", "First line"),
                        ("end_line", "Last line"), ("sensitive_data", "Sensitive-data access"), ("spec", "Requested change")] {
                        if let Some(v) = a.get(key) {
                            let text = if let Some(s) = v.as_str() { s.to_owned() }
                                else if let Some(list) = v.as_array() {
                                    list.iter().enumerate().map(|(i,v)| format!("{}. {}", i+1, v.as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n")
                                } else { v.to_string() };
                            inputs.push((label.into(), bounded(&text)));
                        }
                    }
                    self.push(Activity { scope: scope.into(), call_id: Some(call.id.clone()), tool: safe(&call.name),
                        request: self.requests.get(scope).copied().unwrap_or(0), target: bounded(target),
                        handle: (!handle.is_empty()).then(|| handle.to_owned()), inputs, ..Default::default() });
                }
            }
            Entry::Item { item: Item::ToolResult { call_id, content } } => {
                if let Some(e) = self.call_mut(scope, call_id) { e.result = Some(bounded(content)); }
            }
            Entry::Shown { call_id, class } => {
                if let Some(e) = self.call_mut(scope, call_id) { e.class = Some(*class); }
                else {
                    self.push(Activity { scope: scope.into(), call_id: Some(call_id.clone()),
                        tool: "Tool result (call details unavailable)".into(), class: Some(*class), ..Default::default() });
                }
            }
            Entry::Interrupted { call_id, .. } => {
                if let Some(e) = self.call_mut(scope, call_id) { e.interrupted = true; }
            }
            Entry::TurnEnd { .. } | Entry::End { .. } => {
                for event in &mut self.events {
                    if event.scope == scope && event.result.is_none() {
                        event.interrupted = true;
                    }
                }
            }
            Entry::Masked { items, tokens_before, tokens_after, .. } => self.note(scope, "Older results shortened",
                format!("{items} earlier tool results were replaced by stubs for subsequent requests.\nContext tokens: {tokens_before} → {tokens_after}.\nEarlier outbound records remain in /audit.")),
            Entry::Compacted { text, tokens_before, tokens_after, .. } => self.note(scope, "Conversation condensed locally",
                format!("Context tokens: {tokens_before} → {tokens_after}.\nReplacement summary:\n{text}")),
            Entry::CompactionFailed { reason, .. } => self.note(scope, "Condensation failed", format!("Conversation retained.\n{reason}")),
            _ => {}
        }
    }

    pub(super) fn audit(&mut self, records: &[Record]) {
        for record in records {
            match record {
                Record::Request(r) if r.seq > self.audit_seq => {
                    for (id, text) in results(&r.request).into_iter().rev().take(KEEP) {
                        self.sent.entry(id).or_insert_with(|| Sent {
                            seq: r.seq,
                            unix_ms: r.unix_ms,
                            model: safe(&r.model),
                            text: bounded(&text),
                            interventions: r.interventions.iter().map(|s| bounded(s)).collect(),
                        });
                    }
                    while self.sent.len() > KEEP {
                        let oldest = self
                            .sent
                            .iter()
                            .min_by_key(|(_, s)| s.seq)
                            .map(|(id, _)| id.clone())
                            .unwrap();
                        self.sent.remove(&oldest);
                    }
                    self.audit_seq = r.seq;
                }
                Record::Event(r) if r.seq > self.audit_seq => {
                    let note = match &r.event {
                        AuditEvent::EndpointTrust { role, host, trust } => Some((
                            "Model endpoint",
                            format!("Role: {role}\nHost: {host}\nTrust decision: {trust}"),
                        )),
                        AuditEvent::BlockedSend { check } => Some((
                            "Outbound request blocked",
                            format!("Check: {check}\nThis request was stopped by the boundary."),
                        )),
                        AuditEvent::SendWithheld { check, parts } => Some((
                            "Outbound content withheld",
                            format!(
                                "Check: {check}\n{parts} request parts were withheld before the request passed the checks."
                            ),
                        )),
                        AuditEvent::SensitiveCommand {
                            command,
                            exit_code,
                            derived_files,
                        } => Some((
                            "Command used sensitive data",
                            format!(
                                "Command: {command}\nExit: {exit_code:?}\nOutput kept local; derived files marked sensitive:\n{}",
                                derived_files.join("\n")
                            ),
                        )),
                        AuditEvent::Image {
                            origin,
                            destination,
                            decision,
                            ..
                        } => Some((
                            "Image routing",
                            format!(
                                "Source: {origin}\nDestination: {destination}\nDecision: {decision}"
                            ),
                        )),
                        AuditEvent::LocalProbe {
                            handle,
                            rule,
                            withheld,
                            count,
                        } => Some((
                            "Local question filtered",
                            format!(
                                "Handle: {handle}\nRule: {rule}\nPieces withheld: {withheld}\nProbe number: {count}"
                            ),
                        )),
                        AuditEvent::OutputProbe {
                            count,
                            budget,
                            shown,
                        } => Some((
                            "Sensitive command output checked",
                            format!("Probe: {count} / {budget}\nOutput shown: {shown}"),
                        )),
                        AuditEvent::OutboundRefused {
                            channel,
                            destination,
                            reason,
                        } => Some((
                            "External send refused",
                            format!(
                                "Action: {channel}\nDestination: {destination}\nReason: {reason}"
                            ),
                        )),
                        AuditEvent::SandboxDenial { command, access } => Some((
                            "File access denied",
                            format!("Command: {command}\nAccess: {access}"),
                        )),
                        _ => None,
                    };
                    if let Some((title, detail)) = note {
                        self.note(
                            "",
                            title,
                            format!("Audit #{} · {} UTC\n{detail}", r.seq, time(r.unix_ms)),
                        );
                    }
                    self.audit_seq = r.seq;
                }
                _ => {}
            }
        }
    }

    pub(super) fn select(&mut self, by: isize) {
        let visible: Vec<_> = self.events.iter().rev().filter(|e| e.visible()).collect();
        if visible.is_empty() {
            return;
        }
        let index = self
            .selected
            .and_then(|id| visible.iter().position(|e| e.number == id))
            .unwrap_or(0);
        self.selected =
            Some(visible[index.saturating_add_signed(by).min(visible.len() - 1)].number);
        self.scroll = 0;
        self.expanded = false;
    }

    pub(super) fn toggle_details(&mut self) {
        self.expanded = !self.expanded;
        self.scroll = 0;
    }

    pub(super) fn scroll(&mut self, by: isize) {
        if self.selected.is_none() {
            self.selected = self
                .events
                .iter()
                .rev()
                .find(|e| e.visible())
                .map(|e| e.number);
        }
        self.scroll = self.scroll.saturating_add_signed(by).min(self.max_scroll);
    }

    fn details(&self, e: &Activity, run_dir: Option<&Path>) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        let mut add = |label: &str, text: &str| {
            out.push(Line::styled(label.to_owned(), palette::muted()));
            out.extend(safe(text).lines().map(|s| Line::raw(s.to_owned())));
            out.push(Line::default());
        };
        if e.notice {
            add(&e.tool, e.result.as_deref().unwrap_or(""));
            return out;
        }
        let source = e.handle.as_ref().and_then(|h| handle_source(run_dir?, h));
        if let Some(source) = source {
            add("Source", &format!("{} · {}", e.target, source));
        } else if !e.target.is_empty() {
            add("Source", &e.target);
        }
        for (label, text) in &e.inputs {
            add(label, text);
        }
        add("Outcome", e.outcome());
        if let Some(class) = e.class {
            let handling = match class {
                ViewClass::Raw => {
                    "Tool result prepared without a handle or placeholder transformation. The outbound checks still apply."
                }
                ViewClass::Tokenized => {
                    "Values were replaced by placeholders or redactions in the prepared view. Original values were not restored here."
                }
                ViewClass::HandleSummary => {
                    "Source classified as sensitive. Raw content retained locally; a filtered view and handle were prepared."
                }
                ViewClass::LocalAnswer => {
                    "The local-model service handled the question against the stored source. The answer was filtered for the frontier."
                }
                ViewClass::BulkyHandle => {
                    "Source was too large for the inline view. An outline and handle were prepared; public ranges can be read with read_raw."
                }
                ViewClass::Protected => {
                    "Protected-source policy applied. Only the permitted interface or a sealed-file notice was prepared."
                }
            };
            add("Handling", handling);
        }
        let id = e.call_id.as_deref().unwrap_or("");
        if !self.ambiguous.contains(id)
            && let Some(sent) = self.sent.get(id)
        {
            add(
                "Outbound record",
                &format!(
                    "Audit #{} · {} UTC · {}\nPassed outbound checks; provider receipt is not confirmed by this record.",
                    sent.seq,
                    time(sent.unix_ms),
                    sent.model
                ),
            );
            if !sent.interventions.is_empty() {
                add("Outbound filtering", &sent.interventions.join("\n"));
            }
            add("Recorded outbound view", &sent.text);
            if e.result.as_ref().is_some_and(|r| *r != sent.text) {
                add(
                    "Prepared view before outbound filtering",
                    e.result.as_deref().unwrap_or(""),
                );
            }
        } else {
            add(
                "Outbound record",
                if self.ambiguous.contains(id) {
                    "Call ID was reused; a unique outbound match is unavailable. Inspect /audit."
                } else {
                    "No matching outbound record yet. A prepared result alone does not confirm it was sent."
                },
            );
            if let Some(result) = &e.result {
                add("Prepared view / error", result);
            }
        }
        add(
            "Trace",
            &format!(
                "{}{}{}",
                if e.scope.is_empty() {
                    String::new()
                } else {
                    format!("Agent {} · ", e.scope)
                },
                if e.request == 0 {
                    String::new()
                } else {
                    format!("request {} · ", e.request)
                },
                id
            ),
        );
        out
    }

    fn summary(&self, e: &Activity, run_dir: Option<&Path>) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        let mut add = |label: &str, message: String| {
            out.push(Line::styled(label.to_owned(), palette::accent()));
            out.extend(
                safe(&message)
                    .lines()
                    .map(|line| Line::raw(line.to_owned())),
            );
            out.push(Line::default());
        };
        if e.notice {
            let message = match e.tool.as_str() {
                "Outbound request blocked" => {
                    "Duet stopped this request before it left the privacy boundary."
                }
                "Outbound content withheld" => {
                    "Duet removed content before allowing the request to continue."
                }
                "Command used sensitive data" => {
                    "The command used sensitive data. Its output stayed on this machine."
                }
                "Local question filtered" => {
                    "A local answer was checked before anything could be shared with the cloud model."
                }
                "External send refused" => "Duet refused this external request.",
                "File access denied" => "Duet blocked access to this file.",
                "Image routing" => "Duet recorded where this image was allowed to go.",
                _ if e.tool.starts_with("Request ") => {
                    "Duet checked this model request for sensitive content."
                }
                _ => "Duet recorded a privacy decision.",
            };
            add("What happened", message.into());
            if e.tool.starts_with("Request ") {
                add("Filtering", e.tool.clone());
            }
            add(
                "For the exact decision and reason",
                "Press Ctrl-O for the full record.".into(),
            );
            return out;
        }

        let id = e.call_id.as_deref().unwrap_or("");
        let sent = (!self.ambiguous.contains(id))
            .then(|| self.sent.get(id))
            .flatten();
        let destination = if let Some(sent) = sent {
            format!(
                "Duet allowed this text to go to {}. The audit record does not show whether the service received it.",
                sent.model
            )
        } else if self.ambiguous.contains(id) {
            "A matching cloud record cannot be identified. Open /audit for the complete history."
                .into()
        } else {
            "No matching cloud send is recorded for this result yet.".into()
        };
        add("Cloud sharing", destination);

        let source = source_name(e, run_dir);
        let action = match e.tool.as_str() {
            "read_raw" => "Read another section of a source",
            "read_file" => "Read a file",
            "web_fetch" => "Fetched a web page",
            "web_search" => "Searched the web",
            "ask_local" => "Asked the local model about a source",
            "run_command" => "Ran a command",
            "synthetic_sample" => "Created a synthetic sample",
            "explore" => "Explored the workspace",
            "edit_protected" => "Edited protected code locally",
            "code_nav" => "Looked up code",
            _ => "Used a tool",
        };
        let mut what = action.to_owned();
        if !source.is_empty() {
            what.push_str(&format!("\n{}", short(&source, 220)));
        }
        let first = e.inputs.iter().find(|(label, _)| label == "First line");
        let last = e.inputs.iter().find(|(label, _)| label == "Last line");
        if let (Some((_, first)), Some((_, last))) = (first, last) {
            what.push_str(&format!("\nLines {first}–{last}"));
        }
        if let Some((label, value)) = e
            .inputs
            .iter()
            .find(|(label, _)| matches!(label.as_str(), "Question" | "Command"))
        {
            what.push_str(&format!("\n{label}: {}", short(value, 180)));
        }
        add("What Duet did", what);

        let handling = match e.class {
            Some(ViewClass::Tokenized) => "Sensitive values were replaced with placeholders.",
            Some(ViewClass::HandleSummary) => {
                "The original content stayed local. The cloud model could receive a filtered summary and reference."
            }
            Some(ViewClass::LocalAnswer) => {
                "The local model answered using the stored source. Its answer was filtered before sharing."
            }
            Some(ViewClass::Protected) => {
                "Protected code stayed local. Only the permitted interface was prepared for sharing."
            }
            Some(ViewClass::BulkyHandle) => {
                "A short outline was prepared. The full source was not included here."
            }
            Some(ViewClass::Raw) => {
                "The result was kept as written. Duet still checked the outgoing request for sensitive content."
            }
            None => "No privacy handling has been recorded for this result yet.",
        };
        add("How it was handled", handling.into());

        let result = e.outcome();
        if result != "Result recorded" {
            let mut status = result.to_owned();
            if result.contains("failed")
                && let Some(error) = e.result.as_deref().and_then(|text| text.lines().next())
            {
                status.push_str(&format!("\n{}", short(error, 180)));
            }
            add("Result", status);
        }
        if let Some(sent) = sent {
            add(
                "Exact text",
                format!(
                    "The outbound record contains {} line(s). Press Ctrl-O to inspect the text and audit details.",
                    sent.text.lines().count()
                ),
            );
        } else {
            add(
                "More detail",
                "Press Ctrl-O to inspect the prepared result and audit status.".into(),
            );
        }
        out
    }

    pub(super) fn draw(&mut self, f: &mut Frame<'_>, area: Rect, run_dir: Option<&Path>) {
        let visible: Vec<_> = self.events.iter().rev().filter(|e| e.visible()).collect();
        if visible.is_empty() {
            f.render_widget(Paragraph::new("Privacy activity\n\nDuet's file reads, local checks and cloud sharing will appear here. Ctrl-O opens the full record for a selected action.").wrap(Wrap { trim: false }), area);
            return;
        }
        let index = self
            .selected
            .and_then(|id| visible.iter().position(|e| e.number == id))
            .unwrap_or(0);
        let selected = visible[index];
        let count = (area.height.saturating_sub(16) as usize / 6)
            .clamp(1, 3)
            .min(visible.len());
        let [head, list, detail, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length((count * 2 + 1) as u16),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    format!("Privacy · action {} of {}", index + 1, visible.len()),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Line::styled("Ctrl-↑↓ choose · Ctrl-O full record", palette::muted()),
            ]),
            head,
        );
        let start = index.saturating_sub(count - 1);
        let mut rows = Vec::new();
        for event in visible.iter().skip(start).take(count) {
            let current = event.number == selected.number;
            rows.push(Line::from(vec![
                Span::styled(if current { "› " } else { "  " }, palette::accent()),
                Span::styled(
                    format!(
                        "{}  {}",
                        plain_title(event),
                        short(
                            &source_name(event, run_dir),
                            area.width.saturating_sub(22) as usize
                        )
                    ),
                    if current {
                        palette::accent()
                    } else {
                        Style::default()
                    },
                ),
            ]));
            let matched = !self
                .ambiguous
                .contains(event.call_id.as_deref().unwrap_or(""))
                && event
                    .call_id
                    .as_ref()
                    .is_some_and(|id| self.sent.contains_key(id));
            rows.push(Line::styled(
                format!("  {}", plain_status(event, matched)),
                palette::muted(),
            ));
        }
        rows.push(Line::styled(
            "─".repeat(area.width as usize),
            palette::muted(),
        ));
        f.render_widget(Paragraph::new(rows), list);
        let lines = if self.expanded {
            self.details(selected, run_dir)
        } else {
            self.summary(selected, run_dir)
        };
        // Use the same Unicode wrapping for measurement and drawing, so the
        // last line remains reachable and resizing cannot leave a blank pane.
        let rows: Vec<_> = lines
            .iter()
            .flat_map(|line| super::ansi::wrap(line, detail.width.max(1) as usize))
            .collect();
        self.max_scroll = rows.len().saturating_sub(detail.height as usize);
        self.scroll = self.scroll.min(self.max_scroll);
        let page = rows
            .into_iter()
            .skip(self.scroll)
            .take(detail.height as usize)
            .collect::<Vec<_>>();
        f.render_widget(Paragraph::new(page), detail);
        f.render_widget(
            Paragraph::new(Line::styled(
                format!(
                    "{} {}/{} · /audit{}",
                    if self.expanded {
                        "Full record · Ctrl-O summary"
                    } else {
                        "Summary · Ctrl-O full record"
                    },
                    self.scroll + 1,
                    self.max_scroll + 1,
                    if self.dropped > 0 {
                        " · older activity omitted"
                    } else {
                        ""
                    }
                ),
                palette::muted(),
            )),
            footer,
        );
    }
}

fn plain_title(event: &Activity) -> &str {
    match event.tool.as_str() {
        "read_raw" => "Read source excerpt",
        "read_file" => "Read file",
        "web_fetch" => "Fetched web page",
        "web_search" => "Searched web",
        "ask_local" => "Asked local model",
        "run_command" => "Ran command",
        "synthetic_sample" => "Made sample",
        "explore" => "Explored code",
        "edit_protected" => "Edited protected code",
        "code_nav" => "Looked up code",
        name if name.starts_with("Request ") => "Checked model request",
        name => name,
    }
}

fn plain_status(event: &Activity, matched_cloud_record: bool) -> &str {
    if event.notice {
        return match event.tool.as_str() {
            "Outbound request blocked" | "External send refused" => "Blocked before sending",
            "Outbound content withheld" => "Content removed before sending",
            "Command used sensitive data" => "Command output kept local",
            "File access denied" => "Access blocked",
            name if name.starts_with("Request ") => "Sensitive text filtered",
            _ => "Privacy decision recorded",
        };
    }
    if matched_cloud_record {
        "Allowed for cloud request"
    } else {
        event.outcome()
    }
}

fn source_name(event: &Activity, run_dir: Option<&Path>) -> String {
    let source = event
        .handle
        .as_ref()
        .and_then(|handle| run_dir.and_then(|dir| handle_source(dir, handle)))
        .unwrap_or_else(|| {
            if event.handle.is_some() {
                "Stored source".into()
            } else {
                event.target.clone()
            }
        });
    source
        .trim_start_matches("web content from ")
        .trim_end_matches(" (untrusted)")
        .to_owned()
}

fn short(text: &str, max: usize) -> String {
    let text = text.replace('\n', " ");
    let mut chars = text.chars();
    let first: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{first}…")
    } else {
        first
    }
}

fn time(unix_ms: u128) -> String {
    let s = unix_ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

pub(super) fn handle_source(run_dir: &Path, handle: &str) -> Option<String> {
    let digits = handle.strip_prefix('h')?;
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Metadata only: never open the raw handle, vault, or original source.
    let bytes = duet_fs::read_file(
        run_dir,
        Path::new(&format!("handles/{handle}.source")),
        TEXT_LIMIT as u64,
    )
    .ok()?;
    Some(bounded(std::str::from_utf8(&bytes).ok()?))
}

fn content(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_owned();
    }
    value
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .map(|p| {
                    p["text"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| "[non-text content; inspect /audit]".into())
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Tool-result locations in each supported provider dialect. Other content,
/// especially system/user messages and image payloads, is not captured here.
fn results(request: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(messages) = request
        .get("messages")
        .or_else(|| request.get("input"))
        .and_then(Value::as_array)
    {
        for m in messages {
            if m["role"] == "tool"
                && let Some(id) = m["tool_call_id"].as_str()
            {
                out.push((id.into(), content(&m["content"])));
            } else if m["type"] == "function_call_output"
                && let Some(id) = m["call_id"].as_str()
            {
                out.push((id.into(), content(&m["output"])));
            } else if let Some(parts) = m["content"].as_array() {
                for part in parts {
                    if part["type"] == "tool_result"
                        && let Some(id) = part["tool_use_id"].as_str()
                    {
                        out.push((id.into(), content(&part["content"])));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_boundary::audit::{AuditLog, read};
    use duet_boundary::model::ToolCall;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    #[test]
    fn repeated_details_are_scoped_and_legacy_counts_are_never_treated_as_identity() {
        let mut history = FilterHistory::default();
        let notes = vec!["sanitize: history item 2 · call edit42 · edit_file · src/client.rs · arguments.new: filtered line(s) 2; replaced with ⟨secret:KEY#1⟩".into()];
        assert_eq!(history.split("parent", &notes), (notes.clone(), vec![]));
        assert_eq!(history.split("parent", &notes), (vec![], notes.clone()));
        assert_eq!(history.split("child", &notes), (notes.clone(), vec![]));
        let old = vec!["sanitize: replaced 1 known value(s) in the model's own message".into()];
        history.split("parent", &old);
        assert_eq!(history.split("parent", &old), (old, vec![]));
    }

    fn call(id: &str, name: &str, args: Value) -> Entry {
        Entry::Item {
            item: Item::Assistant {
                text: String::new(),
                reasoning: None,
                replay: None,
                tool_calls: vec![ToolCall {
                    id: id.into(),
                    name: name.into(),
                    arguments: args.as_object().unwrap().clone(),
                    raw_arguments: args.to_string(),
                }],
            },
        }
    }
    fn result(id: &str, text: &str) -> Entry {
        Entry::Item {
            item: Item::ToolResult {
                call_id: id.into(),
                content: text.into(),
            },
        }
    }
    fn text(p: &Privacy, run: Option<&Path>) -> String {
        let e = p.events.iter().rev().find(|e| e.visible()).unwrap();
        p.details(e, run)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn screen(p: &mut Privacy, width: u16, height: u16, run: Option<&Path>) -> String {
        let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
        t.draw(|f| p.draw(f, f.area(), run)).unwrap();
        t.backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn questions_sources_errors_and_outbound_evidence_survive_replay() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("handles")).unwrap();
        std::fs::write(d.path().join("handles/h1.source"), "specs/game.md").unwrap();
        std::fs::write(
            d.path().join("handles/h1"),
            "RAW_HANDLE_CANARY_MUST_NOT_BE_READ",
        )
        .unwrap();
        let entries = [
            call(
                "c1",
                "ask_local",
                json!({"handle":"h1","questions":["What are the jump controls?","Which treasures are required?"]}),
            ),
            result(
                "c1",
                "Space jumps. Collect three treasures. ⟨redacted:copied-sensitive-text⟩",
            ),
            Entry::Shown {
                call_id: "c1".into(),
                class: ViewClass::LocalAnswer,
            },
        ];
        let mut p = Privacy::default();
        for entry in &entries {
            p.entry(entry);
        }
        let prepared = text(&p, Some(d.path()));
        assert!(prepared.contains("What are the jump controls?"));
        assert!(prepared.contains("Which treasures are required?"));
        assert!(prepared.contains("specs/game.md"));
        assert!(prepared.contains("No matching outbound record yet"));
        let mut log = AuditLog::open(&d.path().join("audit.jsonl")).unwrap();
        log.append("https://frontier.example", "test-model", json!({"messages":[{"role":"tool","tool_call_id":"c1","content":"Space jumps. Collect three treasures."}]}), vec!["copied text withheld".into()]).unwrap();
        let records = read(&d.path().join("audit.jsonl")).unwrap();
        p.audit(&records);
        let detail = text(&p, Some(d.path()));
        assert!(detail.contains("Recorded outbound view\nSpace jumps. Collect three treasures."));
        assert!(detail.contains("copied text withheld"));
        assert!(!detail.contains("RAW_HANDLE_CANARY"));
        let mut replay = Privacy::default();
        replay.audit(&records); // audit may load before the transcript recap
        for entry in &entries {
            replay.entry(entry);
        }
        assert_eq!(text(&replay, Some(d.path())), detail);
        p.entry(&call(
            "c2",
            "ask_local",
            json!({"handle":"h1","question":"What is the map size?"}),
        ));
        assert!(text(&p, None).contains("Waiting for a result"));
        p.entry(&result("c2", "error: local model unavailable"));
        assert!(text(&p, None).contains("Failed; see recorded error"));
        assert!(!text(&p, None).contains("local answer prepared"));
        if let Ok(path) = std::env::var("DUET_PRIVACY_PREVIEW") {
            std::fs::write(path, screen(&mut replay, 76, 42, Some(d.path()))).unwrap();
        }
    }

    #[test]
    fn outbound_dialects_keep_only_tool_result_text() {
        for request in [
            json!({"messages":[{"role":"tool","tool_call_id":"c1","content":"filtered"}]}),
            json!({"input":[{"type":"function_call_output","call_id":"c1","output":[{"type":"input_text","text":"filtered"}]}]}),
            json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":[{"type":"text","text":"filtered"}]}]}]}),
        ] {
            assert_eq!(results(&request), vec![("c1".into(), "filtered".into())]);
        }
        assert!(results(&json!({"messages":[{"role":"system","content":"PRIVATE_SYSTEM"},{"role":"user","content":"PRIVATE_USER"}]})).is_empty());
    }

    #[test]
    fn bounded_history_scrolls_to_end_and_namespaces_children() {
        let mut p = Privacy::default();
        p.entry(&call(
            "same",
            "ask_local",
            json!({"handle":"h1","question":"Parent"}),
        ));
        p.entry(&Entry::Subagent {
            child: "a1".into(),
            entry: Box::new(call(
                "same",
                "ask_local",
                json!({"handle":"h2","question":"Child"}),
            )),
        });
        p.entry(&Entry::Subagent {
            child: "a1".into(),
            entry: Box::new(result("same", "child result")),
        });
        assert!(p.events[0].result.is_none());
        assert_eq!(p.events[1].result.as_deref(), Some("child result"));
        assert!(text(&p, None).contains("Call ID was reused"));
        for i in 0..(KEEP + 3) {
            p.entry(&call(
                &format!("call-{i}"),
                "read_file",
                json!({"path":format!("file-{i}.rs")}),
            ));
        }
        assert_eq!(p.events.len(), KEEP);
        p.entry(&result(
            &format!("call-{}", KEEP + 2),
            &format!(
                "{}\nLAST_LINE_REACHABLE",
                "A long Unicode line: 界 é 🌲\n".repeat(150)
            ),
        ));
        let first = screen(&mut p, 32, 22, None);
        assert!(first.contains("Privacy · action"));
        assert!(!first.contains("LAST_LINE_REACHABLE"));
        p.toggle_details();
        screen(&mut p, 32, 22, None);
        p.scroll(isize::MAX);
        let last = screen(&mut p, 32, 22, None);
        assert!(last.contains("LAST_LINE_REACHABLE"), "{last}");
        assert!(p.selected.is_some()); // new arrivals cannot move a scrolled view
        assert!(handle_source(Path::new("/tmp"), "../../vault").is_none());
        let escaped = bounded("before\x1b]52;c;INJECTED\x07after");
        assert!(!escaped.contains('\x1b'));
    }

    #[test]
    fn compact_view_explains_a_web_excerpt_without_dumping_the_outbound_text() {
        let mut p = Privacy::default();
        p.entry(&call(
            "c1",
            "read_raw",
            json!({"handle":"h3","start_line":40,"end_line":108}),
        ));
        p.entry(&result("c1", "Lantern — needed to win the game"));
        p.entry(&Entry::Shown {
            call_id: "c1".into(),
            class: ViewClass::Raw,
        });
        p.sent.insert(
            "c1".into(),
            Sent {
                seq: 19,
                unix_ms: 0,
                model: "glm-test".into(),
                text: "Lantern — needed to win the game".into(),
                interventions: vec![],
            },
        );
        let compact = screen(&mut p, 76, 32, None);
        assert!(compact.contains("Cloud sharing"), "{compact}");
        assert!(compact.contains("Lines 40–108"), "{compact}");
        assert!(compact.contains("service received it"), "{compact}");
        assert!(!compact.contains("Lantern — needed"), "{compact}");
        let narrow = screen(&mut p, 50, 25, None);
        assert!(narrow.contains("Cloud sharing"), "{narrow}");
        assert!(!narrow.contains("Lantern — needed"), "{narrow}");
        p.toggle_details();
        let expanded = screen(&mut p, 76, 32, None);
        assert!(expanded.contains("Full record"), "{expanded}");
        assert!(text(&p, None).contains("Lantern — needed to win"));
    }
}
