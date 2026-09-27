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
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
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

const HELD: [&str; 5] = [
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
        let [head, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
        let mut spans = vec![Span::raw(" ")];
        for t in Tab::ALL {
            let style = if t == tab {
                Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::new().add_modifier(Modifier::DIM)
            };
            spans.push(Span::styled(format!(" {} ", t.title()), style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "Ctrl-T",
            Style::new().add_modifier(Modifier::DIM),
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), head);
        match tab {
            Tab::Changes => {
                changes::draw_files(f, &self.files, self.file, self.diff_scroll, false, body)
            }
            Tab::Privacy => self.draw_privacy(f, body),
            Tab::Session => draw_session(f, body, status),
        }
    }

    fn draw_privacy(&self, f: &mut Frame<'_>, area: Rect) {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let held_style = Style::new().fg(Color::Magenta);
        let mut lines = vec![Line::styled(
            "What the frontier did not see as it was:",
            Style::new().add_modifier(Modifier::BOLD),
        )];
        let total: usize = self.held.iter().sum();
        if total == 0 && self.withheld.is_empty() {
            lines.push(Line::styled(
                "  nothing withheld so far in this session",
                dim,
            ));
        }
        for (n, what) in self.held.iter().zip(HELD) {
            if *n > 0 {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {n:>4} "), held_style),
                    Span::raw(format!("tool result(s): {what}")),
                ]));
            }
        }
        if self.local_answers > 0 {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:>4} ", self.local_answers), held_style),
                Span::raw("question(s) answered by the local model"),
            ]));
        }
        if self.masked > 0 || self.compactions > 0 {
            lines.push(Line::styled(
                format!(
                    "  context: {} old result(s) shortened, {} condensation(s)",
                    self.masked, self.compactions
                ),
                dim,
            ));
        }
        if !self.withheld.is_empty() {
            lines.push(Line::default());
            lines.push(Line::styled(
                format!("Interventions ({}), newest first:", self.withheld.len()),
                Style::new().add_modifier(Modifier::BOLD),
            ));
            for w in self.withheld.iter().rev() {
                lines.push(Line::from(format!("  {}", crate::term::safe(w))));
            }
        }
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(dim)
                    .title(" privacy "),
            ),
            area,
        );
    }
}

fn draw_session(f: &mut Frame<'_>, area: Rect, s: &Status) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let row = |k: &str, v: String| {
        Line::from(vec![Span::styled(format!("  {k:<14}"), dim), Span::raw(v)])
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
    let lines = if s.session.is_empty() {
        vec![Line::styled(
            "  the session starts with your first message",
            dim,
        )]
    } else {
        vec![
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
                "tokens",
                format!(
                    "{} in ({} cached), {} out",
                    thousands(s.tokens_in),
                    thousands(s.tokens_cached),
                    thousands(s.tokens_out)
                ),
            ),
            row(
                "cost",
                format!(
                    "${:.4} of ${:.2} (each turn at most ${:.2})",
                    s.cost_usd, s.budget_usd, s.turn_budget_usd
                ),
            ),
            row(
                "working time",
                format!(
                    "{}m{:02}s of {}m",
                    s.worked_secs / 60,
                    s.worked_secs % 60,
                    s.budget_minutes
                ),
            ),
        ]
    };
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(dim)
                .title(" session "),
        ),
        area,
    );
}
