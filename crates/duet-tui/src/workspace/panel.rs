// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace's side panel, cycled with Ctrl-T:
//!
//! - Changes: the files this session changed and the selected file's diff,
//!   from the session's write journal and git (see `crate::changes`). A file
//!   that is sensitive by policy, or derived from sensitive data by a command,
//!   is named, never shown, as everywhere else in duet.
//! - Privacy: what the boundary withheld from the frontier, as the transcript
//!   records it (results shown other than as they are, by kind; the values
//!   and lines it replaced; local-model answers), counted and newest first.
//! - Session: turns, requests, tokens, cost and working time against the
//!   session's budgets.

use super::Status;
use super::cells::palette::{self, ACCENT, BAD, GOOD, HELD, MUTED, WARN};
use crate::changes::{self, ChangedFile};
use duet_agent::transcript::Entry;
use duet_boundary::audit::{self, AuditEvent, Line as Record};
use duet_boundary::model::Item;
use duet_boundary::policy::Policy;
use duet_boundary::view::ViewClass;
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

/// Interventions kept for the Privacy view.
const KEEP: usize = 500;

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
    /// Results shown other than as they are, by how.
    held: [usize; 5],
    /// The boundary's interventions, oldest first.
    withheld: Vec<String>,
    local_answers: usize,
    masked: usize,
    compactions: usize,
}

fn held_index(class: ViewClass) -> Option<usize> {
    match class {
        ViewClass::Raw => None,
        ViewClass::Tokenized => Some(0),
        ViewClass::HandleSummary => Some(1),
        ViewClass::LocalAnswer => Some(2),
        ViewClass::BulkyHandle => Some(3),
        ViewClass::Protected => Some(4),
    }
}

const HELD_TEXT: [&str; 5] = [
    "values replaced by placeholders",
    "sensitive, held locally (a summary was sent)",
    "answered by the local model",
    "large, held locally (an outline was sent)",
    "protected source (only its interface was sent)",
];

impl Panel {
    /// Opens on a screen at least this wide, unless the operator chose.
    pub(super) const AUTO_WIDTH: u16 = 110;

    /// Follows the session in `run_dir` from now on.
    pub(super) fn attach(&mut self, ws: PathBuf, run_dir: PathBuf, audit: PathBuf, policy: Policy) {
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

    /// Ctrl-Up/Down: another file.
    pub(super) fn select(&mut self, by: isize) {
        if self.files.is_empty() {
            return;
        }
        self.picked = true;
        let last = self.files.len() - 1;
        self.file = self.file.saturating_add_signed(by).min(last);
        self.diff_scroll = 0;
    }

    /// Ctrl-PgUp/PgDn: the diff scrolls.
    pub(super) fn scroll_diff(&mut self, by: isize) {
        let rows = self.files.get(self.file).map_or(0, |f| f.rows.len());
        self.diff_scroll = self
            .diff_scroll
            .saturating_add_signed(by)
            .min(rows.saturating_sub(1));
    }

    /// What a transcript entry adds to the Privacy view.
    pub(super) fn entry(&mut self, e: &Entry) {
        match e {
            Entry::Shown { class, .. } => {
                if let Some(i) = held_index(*class) {
                    self.held[i] += 1;
                }
            }
            Entry::Usage {
                turn,
                interventions,
                ..
            } => {
                self.withheld
                    .extend(interventions.iter().map(|i| format!("turn {turn}: {i}")));
                if self.withheld.len() > KEEP {
                    let drop = self.withheld.len() - KEEP;
                    self.withheld.drain(..drop);
                }
            }
            Entry::Item {
                item: Item::Assistant { tool_calls, .. },
            } => {
                self.local_answers += tool_calls.iter().filter(|c| c.name == "ask_local").count();
            }
            Entry::Masked { items, .. } => self.masked += items,
            Entry::Compacted { .. } => self.compactions += 1,
            Entry::Subagent { entry, .. } => self.entry(entry),
            _ => {}
        }
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
        let len = std::fs::metadata(&a.audit).map_or(0, |m| m.len());
        if len != self.audit_len {
            self.audit_len = len;
            self.derived_audit = audit::read(&a.audit)
                .unwrap_or_default()
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

    pub(super) fn draw(&self, f: &mut Frame<'_>, area: Rect, status: &Status) {
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
        spans.push(Span::styled("  Ctrl-T", palette::muted()));
        f.render_widget(Paragraph::new(Line::from(spans)), head);
        match tab {
            Tab::Changes => self.draw_changes(f, body),
            Tab::Privacy => self.draw_privacy(f, body),
            Tab::Session => draw_session(f, body, status),
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
                for r in file.rows.iter().skip(self.diff_scroll) {
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

    fn draw_privacy(&self, f: &mut Frame<'_>, area: Rect) {
        let heading =
            |t: &str| Line::styled(t.to_owned(), Style::new().add_modifier(Modifier::BOLD));
        let mut lines = vec![heading("What the frontier did not see as it was")];
        let total: usize = self.held.iter().sum();
        if total == 0 && self.withheld.is_empty() && self.local_answers == 0 {
            lines.push(Line::styled(
                "nothing withheld so far in this session",
                palette::muted(),
            ));
        }
        for (n, what) in self.held.iter().zip(HELD_TEXT) {
            if *n > 0 {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{n:>4} "),
                        Style::new().fg(HELD).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("result(s): {what}")),
                ]));
            }
        }
        if self.local_answers > 0 {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>4} ", self.local_answers),
                    Style::new().fg(HELD).add_modifier(Modifier::BOLD),
                ),
                Span::raw("question(s) answered by the local model"),
            ]));
        }
        if self.masked > 0 || self.compactions > 0 {
            lines.push(Line::styled(
                format!(
                    "context: {} old result(s) shortened, {} condensation(s)",
                    self.masked, self.compactions
                ),
                palette::muted(),
            ));
        }
        if !self.withheld.is_empty() {
            lines.push(Line::default());
            lines.push(heading(&format!(
                "Interventions ({}), newest first",
                self.withheld.len()
            )));
            for w in self.withheld.iter().rev() {
                lines.push(Line::from(vec![
                    Span::styled("◦ ", Style::new().fg(HELD)),
                    Span::raw(crate::term::safe(w)),
                ]));
            }
        }
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
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

fn draw_session(f: &mut Frame<'_>, area: Rect, s: &Status) {
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
            Span::styled(format!("{k:<13}"), palette::muted()),
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
        format!("cost  ${:.4} of ${:.2}", s.cost_usd, s.budget_usd),
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
    let lines = vec![
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
    ];
    f.render_widget(Paragraph::new(lines), facts);
}
