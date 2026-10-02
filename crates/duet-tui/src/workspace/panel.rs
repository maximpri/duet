// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace's side panel: Tab / Shift-Tab cycle its tabs; Ctrl-T also closes it.
//!
//! - Changes: the files this session changed and the selected file's diff,
//!   from the session's write journal and git (see `crate::changes`). A file
//!   that is sensitive by policy, or derived from sensitive data by a command,
//!   is named, never shown, as everywhere else in duet.
//! - Privacy: selectable actions, questions, outcomes and outbound evidence.
//! - Session: turns, requests, tokens, cost and working time against the
//!   session's budgets.

use super::Status;
use super::cells::palette::{self, ACCENT, BAD, GOOD, HELD, MUTED, WARN};
use crate::changes::{self, ChangedFile};
use duet_agent::transcript::Entry;
use duet_boundary::audit::{self, AuditEvent, Line as Record};
use duet_boundary::policy::Policy;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph, Wrap};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Changes,
    Privacy,
    Session,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Changes, Tab::Privacy, Tab::Session];

    fn title(self) -> &'static str {
        match self {
            Tab::Changes => "Changes",
            Tab::Privacy => "Privacy",
            Tab::Session => "Session",
        }
    }
}

/// The session the panel follows.
struct Attached {
    ws: PathBuf,
    run_dir: PathBuf,
    audit: PathBuf,
    policy: Policy,
}

/// How often the changed files are looked at while duet works.
const EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct Panel {
    /// The view shown (`None`: the panel is closed).
    pub tab: Option<Tab>,
    /// Whether the operator opened or closed the panel (else it opens itself
    /// on a wide enough screen).
    chosen: bool,
    attached: Option<Attached>,
    files: Vec<ChangedFile>,
    file: usize,
    diff_scroll: usize,
    /// Select the file written last, until the operator picks one.
    picked: bool,
    derived_audit: Vec<String>,
    audit_len: u64,
    refreshed: Option<Instant>,
    privacy: super::privacy::Privacy,
    economics: Option<serde_json::Value>,
    session_scroll: usize,
    recorded_cost: f64,
}

impl Panel {
    /// Opens on a screen at least this wide, unless the operator chose.
    pub(super) const AUTO_WIDTH: u16 = 110;

    /// Follows the session in `run_dir` from now on.
    pub(super) fn attach(
        &mut self,
        ws: PathBuf,
        run_dir: PathBuf,
        audit: PathBuf,
        policy: Policy,
        history: &[Entry],
    ) {
        self.privacy = Default::default();
        self.audit_len = 0;
        self.economics = None;
        self.session_scroll = 0;
        self.recorded_cost = 0.0;
        for entry in history {
            self.entry(entry);
        }
        self.attached = Some(Attached {
            ws,
            run_dir,
            audit,
            policy,
        });
        self.refresh(true);
    }

    /// Ctrl-T: the next view, then closed.
    pub(super) fn cycle(&mut self) {
        self.chosen = true;
        self.tab = match self.tab {
            None => Some(Tab::Changes),
            Some(Tab::Changes) => Some(Tab::Privacy),
            Some(Tab::Privacy) => Some(Tab::Session),
            Some(Tab::Session) => None,
        };
    }

    /// Cycle only the three tabs; open a closed panel at the relevant end.
    pub(super) fn switch_tab(&mut self, backwards: bool) {
        self.chosen = true;
        self.tab = Some(match (self.tab, backwards) {
            (None, false) | (Some(Tab::Session), false) | (Some(Tab::Privacy), true) => {
                Tab::Changes
            }
            (Some(Tab::Changes), false) | (Some(Tab::Session), true) => Tab::Privacy,
            (None, true) | (Some(Tab::Privacy), false) | (Some(Tab::Changes), true) => Tab::Session,
        });
    }

    /// The panel's width on a screen `width` wide (0: not shown).
    pub(super) fn width(&mut self, width: u16) -> u16 {
        if !self.chosen {
            self.tab = (width >= Self::AUTO_WIDTH).then_some(self.tab.unwrap_or(Tab::Changes));
        }
        match self.tab {
            None => 0,
            Some(_) => (width * 2 / 5).clamp(32, 90).min(width.saturating_sub(30)),
        }
    }

    /// Ctrl-Up/Down: another file or privacy event.
    pub(super) fn select(&mut self, by: isize) {
        if self.tab == Some(Tab::Privacy) {
            self.privacy.select(by);
            return;
        }
        if self.files.is_empty() {
            return;
        }
        self.picked = true;
        let last = self.files.len() - 1;
        self.file = self.file.saturating_add_signed(by).min(last);
        self.diff_scroll = 0;
    }

    /// Ctrl-PgUp/PgDn: the active panel's details scroll.
    pub(super) fn scroll_diff(&mut self, by: isize) {
        if self.tab == Some(Tab::Privacy) {
            self.privacy.scroll(by);
            return;
        }
        if self.tab == Some(Tab::Session) {
            self.session_scroll = self.session_scroll.saturating_add_signed(by);
            return;
        }
        let rows = self.files.get(self.file).map_or(0, |f| f.rows.len());
        self.diff_scroll = self
            .diff_scroll
            .saturating_add_signed(by)
            .min(rows.saturating_sub(1));
    }

    /// Ctrl-O opens the selected privacy action's exact audit detail.
    /// Returns false when another panel tab is active.
    pub(super) fn toggle_privacy_details(&mut self) -> bool {
        if self.tab != Some(Tab::Privacy) {
            return false;
        }
        self.privacy.toggle_details();
        true
    }

    /// What a transcript entry adds to the Privacy view.
    pub(super) fn entry(&mut self, e: &Entry) {
        self.privacy.entry(e);
        self.recorded_cost += entry_cost(e);
    }

    /// Brings the changed files up to date: now when `force`, else at most
    /// every second.
    pub(super) fn refresh(&mut self, force: bool) {
        let Some(a) = &self.attached else {
            return;
        };
        if !force && self.refreshed.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.refreshed = Some(Instant::now());
        self.economics = duet_fs::read_file(
            &a.run_dir,
            std::path::Path::new("economics.json"),
            1024 * 1024,
        )
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
        let len = std::fs::metadata(&a.audit).map_or(0, |m| m.len());
        if len != self.audit_len {
            self.audit_len = len;
            let records = audit::read(&a.audit).unwrap_or_default();
            self.privacy.audit(&records);
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
        let derived: BTreeSet<String> = changes::derived_files(&a.run_dir, &self.derived_audit);
        changes::update(&mut self.files, &a.ws, &a.run_dir, &a.policy, &derived);
        if !self.picked
            && let Some(latest) = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.last > 0)
                .max_by_key(|(_, f)| f.last)
                .map(|(i, _)| i)
        {
            if latest != self.file {
                self.diff_scroll = 0;
            }
            self.file = latest;
        }
    }

    pub(super) fn draw(&mut self, f: &mut Frame<'_>, area: Rect, status: &Status) {
        let Some(tab) = self.tab else {
            return;
        };
        let [head, _, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .areas(area);
        let mut spans = Vec::new();
        for t in Tab::ALL {
            if t == tab {
                spans.push(Span::styled(
                    format!(" {} ", t.title()),
                    palette::accent().add_modifier(Modifier::REVERSED),
                ));
            } else {
                spans.push(Span::styled(format!(" {} ", t.title()), palette::muted()));
            }
        }
        spans.push(Span::styled("  Tab / Shift-Tab", palette::muted()));
        f.render_widget(Paragraph::new(Line::from(spans)), head);
        match tab {
            Tab::Changes => self.draw_changes(f, body),
            Tab::Privacy => {
                self.privacy
                    .draw(f, body, self.attached.as_ref().map(|a| a.run_dir.as_path()))
            }
            Tab::Session => {
                let mut live = status.clone();
                live.cost_usd = live.cost_usd.max(self.recorded_cost);
                draw_session(
                    f,
                    body,
                    &live,
                    self.economics.as_ref(),
                    &mut self.session_scroll,
                );
            }
        }
    }

    fn draw_changes(&self, f: &mut Frame<'_>, area: Rect) {
        let width = area.width as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();
        if self.files.is_empty() {
            lines.push(Line::styled("No files changed yet.", palette::muted()));
            lines.push(Line::default());
            lines.push(Line::styled(
                "The files duet writes appear here with their diffs. Sensitive ones are named, never shown.",
                palette::muted(),
            ));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
            return;
        }
        let (added, removed) = self
            .files
            .iter()
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} file(s)", self.files.len()),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  +{added}"), Style::new().fg(GOOD)),
            Span::styled(format!(" −{removed}"), Style::new().fg(BAD)),
            Span::styled("   Ctrl-↑↓ pick", palette::muted()),
        ]));
        let shown_files = (area.height as usize / 3).clamp(3, 12);
        let start = self.file.saturating_sub(shown_files - 1);
        for (i, file) in self.files.iter().enumerate().skip(start).take(shown_files) {
            let (mark, colour) = match (file.held, file.status) {
                (Some(_), _) => ("◦", HELD),
                (None, changes::Status::Created) => ("+", GOOD),
                (None, changes::Status::Deleted) => ("−", BAD),
                (None, changes::Status::Unchanged) => ("=", MUTED),
                (None, changes::Status::ByCommand) => ("⚙", WARN),
                (None, changes::Status::Edited) => ("●", ACCENT),
            };
            let mut spans = vec![
                Span::styled(if i == self.file { "› " } else { "  " }, palette::accent()),
                Span::styled(format!("{mark} "), Style::new().fg(colour)),
            ];
            let name_style = if i == self.file {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            spans.push(Span::styled(crate::term::safe(&file.path), name_style));
            if file.held.is_some() {
                spans.push(Span::styled("  held locally", Style::new().fg(HELD)));
            } else {
                if file.added > 0 {
                    spans.push(Span::styled(
                        format!("  +{}", file.added),
                        Style::new().fg(GOOD),
                    ));
                }
                if file.removed > 0 {
                    spans.push(Span::styled(
                        format!(" −{}", file.removed),
                        Style::new().fg(BAD),
                    ));
                }
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::styled("─".repeat(width), palette::muted()));
        if let Some(file) = self.files.get(self.file) {
            if let Some(held) = file.held {
                let why = match held {
                    changes::Held::Sensitive => "it matches the sensitivity rules",
                    changes::Held::Derived => "a command that could read sensitive data wrote it",
                };
                lines.push(Line::styled(
                    "held locally",
                    Style::new().fg(HELD).add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::styled(
                    format!("{why}; its content is not shown."),
                    palette::muted(),
                ));
            } else {
                let numbers = file
                    .rows
                    .iter()
                    .filter_map(|r| r.old.max(r.new))
                    .max()
                    .unwrap_or(1)
                    .to_string()
                    .len();
                // Paragraph clips each unwrapped row to one screen line. Only
                // format the rows that can actually fit below the file list.
                let visible_rows = (area.height as usize).saturating_sub(lines.len());
                for r in file.rows.iter().skip(self.diff_scroll).take(visible_rows) {
                    let n = r
                        .new
                        .or(r.old)
                        .map_or(" ".repeat(numbers), |n| format!("{n:>numbers$}"));
                    let text = crate::term::safe(&r.text);
                    let line = match r.kind {
                        changes::RowKind::Hunk => {
                            Line::styled(format!("{:┄<numbers$} {text}", ""), palette::muted())
                        }
                        changes::RowKind::Note => Line::styled(
                            format!("{n} {text}"),
                            palette::muted().add_modifier(Modifier::ITALIC),
                        ),
                        changes::RowKind::Added => {
                            band(&n, '+', &text, GOOD, Color::Rgb(16, 44, 24), width)
                        }
                        changes::RowKind::Removed => {
                            band(&n, '-', &text, BAD, Color::Rgb(52, 16, 16), width)
                        }
                        changes::RowKind::Context => Line::from(vec![
                            Span::styled(format!("{n} "), palette::muted()),
                            Span::raw(format!("  {text}")),
                        ]),
                    };
                    lines.push(line);
                }
            }
        }
        f.render_widget(Paragraph::new(lines), area);
    }
}

fn entry_cost(e: &Entry) -> f64 {
    let cost = match e {
        Entry::Usage { cost_usd, .. } | Entry::FailedAttempts { cost_usd, .. } => *cost_usd,
        Entry::ReviewUsage { usage } => usage.cost_usd,
        Entry::Subagent { entry, .. } => entry_cost(entry),
        _ => 0.0,
    };
    if cost.is_finite() && cost >= 0.0 {
        cost
    } else {
        0.0
    }
}

/// A diff row as a full-width band.
fn band(n: &str, sign: char, text: &str, fg: Color, bg: Color, width: usize) -> Line<'static> {
    let body = format!("{sign} {text}");
    let used = n.chars().count() + 1 + body.chars().count();
    Line::from(vec![
        Span::styled(format!("{n} "), palette::muted()),
        Span::styled(
            format!("{body}{}", " ".repeat(width.saturating_sub(used))),
            Style::new().fg(fg).bg(bg),
        ),
    ])
}

fn draw_session(
    f: &mut Frame<'_>,
    area: Rect,
    s: &Status,
    economics: Option<&serde_json::Value>,
    scroll: &mut usize,
) {
    if s.session.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled(
                "The session starts with your first message.",
                palette::muted(),
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let row = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<15}"), palette::muted()),
            Span::raw(v),
        ])
    };
    let thousands = |n: u64| {
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    };
    let [budget, time, facts] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .areas(area);
    let gauge = |f: &mut Frame<'_>, area: Rect, label: String, ratio: f64| {
        let colour = if ratio >= 0.9 {
            BAD
        } else if ratio >= 0.7 {
            WARN
        } else {
            GOOD
        };
        let [text, bar] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
        f.render_widget(Paragraph::new(label), text);
        f.render_widget(
            Gauge::default()
                .ratio(ratio.clamp(0.0, 1.0))
                .label("")
                .gauge_style(Style::new().fg(colour).bg(Color::Black)),
            bar,
        );
    };
    let cost_ratio = if s.budget_usd > 0.0 {
        s.cost_usd / s.budget_usd
    } else {
        0.0
    };
    gauge(
        f,
        budget,
        format!("frontier  ${:.4} of ${:.2}", s.cost_usd, s.budget_usd),
        cost_ratio,
    );
    let time_ratio = if s.budget_minutes > 0 {
        s.worked_secs as f64 / (s.budget_minutes as f64 * 60.0)
    } else {
        0.0
    };
    gauge(
        f,
        time,
        format!(
            "working time  {}m{:02}s of {}m",
            s.worked_secs / 60,
            s.worked_secs % 60,
            s.budget_minutes
        ),
        time_ratio,
    );
    let mut lines = Vec::new();
    if let Some(goal) = &s.goal {
        let safe = crate::term::safe(goal).replace('\n', " ");
        let mut chars = safe.chars();
        let mut summary: String = chars.by_ref().take(240).collect();
        if chars.next().is_some() {
            summary.push('…');
        }
        lines.push(row("goal", summary));
        lines.push(Line::default());
    }
    if let Some(e) = economics {
        let number = |path: &str| {
            e.pointer(path)
                .and_then(serde_json::Value::as_f64)
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(0.0)
        };
        let local = number("/local/cost_usd");
        lines.extend([
            row("local estimate", format!("${local:.6}")),
            row("combined", format!("${:.6}", s.cost_usd + local)),
            row(
                "local in/out",
                format!(
                    "{:.0} / {:.0} tokens",
                    number("/local/input_tokens"),
                    number("/local/output_tokens")
                ),
            ),
            row(
                "local $/1M",
                format!(
                    "{:.4} in / {:.4} out",
                    number("/local_rates/input_per_million"),
                    number("/local_rates/output_per_million")
                ),
            ),
            Line::from("Local costs are outside frontier budgets."),
            Line::default(),
        ]);
        if let Some(count) = e
            .pointer("/local/unpriced_cancelled_requests")
            .and_then(serde_json::Value::as_u64)
            .filter(|count| *count > 0)
        {
            lines.push(Line::styled(
                format!("Canceled local requests with unknown charges: {count}"),
                Style::new().fg(WARN),
            ));
        }
        if let Some(model) = e
            .pointer("/frontier/price/model")
            .and_then(serde_json::Value::as_str)
        {
            lines.push(row("priced model", crate::term::safe(model)));
            lines.push(row(
                "frontier $/1M",
                format!(
                    "{:.4} in / {:.4} out",
                    number("/frontier/price/input"),
                    number("/frontier/price/output")
                ),
            ));
            lines.push(row(
                "cache $/1M",
                format!(
                    "{:.4} read / {:.4} write",
                    number("/frontier/price/cache_read"),
                    number("/frontier/price/cache_write")
                ),
            ));
            if e.pointer("/frontier/overrides")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|tiers| !tiers.is_empty())
            {
                lines.push(Line::from("Context/time tiers apply to this model."));
            }
        } else if s.mode != "top clearance" {
            lines.push(Line::styled(
                "Frontier price UNKNOWN; combined is a subtotal.",
                Style::new().fg(WARN),
            ));
        }
        if let Some(source) = e.get("pricing_source").and_then(serde_json::Value::as_str) {
            lines.push(Line::from(crate::term::safe(source)));
        }
        if let Some(at) = e
            .pointer("/frontier/fetched_at")
            .and_then(serde_json::Value::as_u64)
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            lines.push(row(
                "catalog age",
                format!("{}h", now.saturating_sub(at) / 3600),
            ));
        }
        lines.push(Line::from("Token estimates; provider bills may differ."));
        lines.push(Line::styled(
            "Ctrl-PgUp/PgDn scroll details",
            palette::muted(),
        ));
        lines.push(Line::default());
    }
    lines.extend(vec![
        row("session", s.session.clone()),
        row("mode", s.mode.clone()),
        row("frontier", s.frontier.clone()),
        row(
            "local model",
            s.local.clone().unwrap_or_else(|| "none".into()),
        ),
        Line::default(),
        row("turns", s.turns.to_string()),
        row("requests", s.requests.to_string()),
        row("tool calls", s.tool_calls.to_string()),
        row(
            "tokens in",
            format!(
                "{} ({} cached)",
                thousands(s.tokens_in),
                thousands(s.tokens_cached)
            ),
        ),
        row("tokens out", thousands(s.tokens_out)),
        row("each turn", format!("at most ${:.2}", s.turn_budget_usd)),
    ]);
    let rows = lines
        .iter()
        .flat_map(|l| super::ansi::wrap(l, facts.width.max(1) as usize))
        .collect::<Vec<_>>();
    *scroll = (*scroll).min(rows.len().saturating_sub(facts.height as usize));
    let page = rows
        .into_iter()
        .skip(*scroll)
        .take(facts.height as usize)
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(page), facts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_agent::journal::WriteJournal;
    use duet_agent::transcript::Transcript;
    use duet_boundary::model::{Item, ToolCall};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    #[test]
    fn long_changes_view_keeps_scrolled_rows_and_last_line_reachable() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("workspace");
        let run_dir = dir.path().join("run");
        std::fs::create_dir_all(&ws).unwrap();
        let mut journal = WriteJournal::open(&run_dir).unwrap();
        let content = (1..=2_000)
            .map(|n| format!("unique row {n:04}\n"))
            .collect::<String>();
        journal
            .write(
                &ws,
                std::path::Path::new("long.txt"),
                content.as_bytes(),
                &duet_fs::Precondition::Any,
            )
            .unwrap();
        let mut panel = Panel::default();
        panel.attach(
            ws,
            run_dir,
            dir.path().join("audit.jsonl"),
            Policy::default(),
            &[],
        );
        panel.tab = Some(Tab::Changes);
        let mut terminal = Terminal::new(TestBackend::new(64, 20)).unwrap();
        let mut show = |panel: &mut Panel| {
            terminal
                .draw(|frame| panel.draw(frame, frame.area(), &Status::default()))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .chunks(64)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let first = show(&mut panel);
        assert!(
            first.contains("long.txt") && first.contains("unique row 0001"),
            "{first}"
        );
        assert!(!first.contains("unique row 0020"));
        panel.scroll_diff(1_000);
        let middle = show(&mut panel);
        assert!(middle.contains("unique row 1000"), "{middle}");
        panel.scroll_diff(isize::MAX);
        let last = show(&mut panel);
        assert!(
            last.contains("long.txt") && last.contains("unique row 2000"),
            "{last}"
        );
    }

    #[test]
    fn resume_loads_privacy_events_and_session_shows_local_prices_and_live_cost() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path()).unwrap();
        t.append(&Entry::Item {
            item: Item::Assistant {
                text: String::new(),
                reasoning: None,
                replay: None,
                tool_calls: vec![ToolCall {
                    id: "read1".into(),
                    name: "read_file".into(),
                    arguments: json!({"path":"spec.md"}).as_object().unwrap().clone(),
                    raw_arguments: String::new(),
                }],
            },
        })
        .unwrap();
        t.append(&Entry::Item {
            item: Item::ToolResult {
                call_id: "read1".into(),
                content: "specification".into(),
            },
        })
        .unwrap();
        std::fs::write(dir.path().join("economics.json"), json!({
            "local":{"cost_usd":0.25,"input_tokens":100,"output_tokens":50,"unpriced_cancelled_requests":2},
            "local_rates":{"input_per_million":2.0,"output_per_million":5.0},
            "frontier":{"price":{"model":"vendor/model","input":1.0,"output":4.0,"cache_read":0.1,"cache_write":1.25},"fetched_at":1},
            "pricing_source":"OpenRouter test catalog"
        }).to_string()).unwrap();
        let history = Transcript::read(dir.path()).unwrap();
        let live_usage = Entry::Usage {
            turn: 1,
            usage: Default::default(),
            cost_usd: 0.50,
            interventions: vec![],
        };
        // The session has already written this entry while its earlier Attach
        // message is still queued. The snapshot must not read it a second time.
        t.append(&live_usage).unwrap();
        let mut panel = Panel::default();
        panel.attach(
            dir.path().to_owned(),
            dir.path().to_owned(),
            dir.path().join("audit.jsonl"),
            Policy::default(),
            &history,
        );
        let mut terminal = Terminal::new(TestBackend::new(64, 38)).unwrap();
        let mut show = |panel: &mut Panel| {
            terminal
                .draw(|f| {
                    panel.draw(
                        f,
                        f.area(),
                        &Status {
                            session: "test".into(),
                            budget_usd: 5.0,
                            ..Default::default()
                        },
                    )
                })
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .chunks(64)
                .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        panel.tab = Some(Tab::Privacy);
        assert!(show(&mut panel).contains("spec.md"));
        panel.entry(&live_usage);
        panel.tab = Some(Tab::Session);
        let view = show(&mut panel);
        assert!(view.contains("frontier  $0.5000"), "{view}");
        assert!(view.contains("$0.250000"), "{view}");
        assert!(view.contains("$0.750000"), "{view}");
        assert!(view.contains("2.0000 in / 5.0000 out"), "{view}");
        assert!(view.contains("vendor/model"), "{view}");
        assert!(
            view.contains("Canceled local requests with unknown charges: 2"),
            "{view}"
        );
        if let Ok(path) = std::env::var("DUET_FINOPS_PREVIEW") {
            std::fs::write(path, view).unwrap();
        }
        panel.scroll_diff(isize::MAX);
        assert!(show(&mut panel).contains("each turn"));
    }
}
