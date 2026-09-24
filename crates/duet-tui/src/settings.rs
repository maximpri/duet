// SPDX-License-Identifier: GPL-3.0-or-later
//! Screens generated from the settings registry: which screen shows a key, the
//! settings table with each value's origin, the detail pane, the doctor pane
//! (Models) and the path tester (Sensitivity).

use crate::app::{App, Tab};
use crate::ui::{origin_name, status_style};
use duet_config::{Direction, Kind, REGISTRY, Scope, Setting};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState, Wrap};
use std::path::Path;

/// The screen that shows `key`, by its section. Every registered key has one
/// (a test enforces it), so a new setting appears without TUI changes.
pub fn screen_of(key: &str) -> Option<Tab> {
    match key.split('.').next()? {
        "frontier" | "local" => Some(Tab::Models),
        "sensitivity" => Some(Tab::Sensitivity),
        "ip" => Some(Tab::Ip),
        "limits" | "context" | "checks" | "sandbox" => Some(Tab::Limits),
        "data" => Some(Tab::Data),
        _ => None,
    }
}

/// The registry settings on `tab`, in registry order.
pub fn keys(tab: Tab) -> Vec<&'static Setting> {
    REGISTRY
        .iter()
        .filter(|s| screen_of(s.key) == Some(tab))
        .collect()
}

pub(crate) fn kind_text(kind: Kind) -> String {
    match kind {
        Kind::Bool => "true or false".into(),
        Kind::Int { min, max } => format!("whole number {min}..={max}"),
        Kind::Float { min, max } => format!("number {min}..={max}"),
        Kind::Str => "text".into(),
        Kind::List => "list of strings, as a TOML array".into(),
        Kind::Choice(opts) => format!("one of {}", opts.join(", ")),
    }
}

fn scope_text(s: &Setting) -> String {
    match (s.scope, s.direction) {
        (Scope::Owner, _) => "owner config only; a project file may never set it".into(),
        (Scope::Project, Direction::Any) => "owner or project config".into(),
        (Scope::Project, d) => format!(
            "owner or project config; a project may only {}",
            match d {
                Direction::AddOnly => "add entries",
                Direction::OnlyTrue => "turn it on",
                Direction::OnlyFalse => "turn it off",
                _ => "lower it",
            }
        ),
    }
}

pub(crate) fn draw(f: &mut Frame, app: &App, area: Rect) {
    let tab = app.tab;
    let [table_area, bottom] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(11)]).areas(area);
    let rows = keys(tab).into_iter().map(|s| {
        let value = app
            .cfg
            .value(s.key)
            .map(ToString::to_string)
            .unwrap_or_default();
        let origin = app.cfg.origin(s.key);
        let mut flags = String::new();
        if s.scope == Scope::Owner {
            flags.push_str("owner-only ");
        }
        if s.confirm {
            flags.push_str("confirm");
        }
        Row::new(vec![
            Cell::from(s.key),
            Cell::from(value),
            Cell::from(origin_name(origin)).style(match origin_name(origin) {
                "default" => Style::new().fg(Color::DarkGray),
                _ => Style::new().fg(Color::Cyan),
            }),
            Cell::from(flags).style(Style::new().fg(Color::Yellow)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(36),
            Constraint::Fill(1),
            Constraint::Length(8),
            Constraint::Length(19),
        ],
    )
    .header(
        Row::new(["setting", "value", "from", "rules"])
            .style(Style::new().add_modifier(Modifier::BOLD)),
    )
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
    .block(Block::bordered().title(format!(" {} settings ", tab.title())));
    let mut state = TableState::default().with_selected(Some(app.rows[tab.index()]));
    f.render_stateful_widget(table, table_area, &mut state);

    let [detail, side] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom);
    draw_detail(f, app, detail);
    match tab {
        Tab::Models => draw_doctor(f, app, side),
        Tab::Sensitivity => draw_tester(f, app, side),
        _ => f.render_widget(
            Paragraph::new(help_for(tab))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" about ")),
            side,
        ),
    }
}

fn help_for(tab: Tab) -> &'static str {
    match tab {
        Tab::Limits => {
            "Budgets and bounds for each run: frontier spend, wall clock, finish attempts, command timeouts, \
the checks run at finish, context masking and sandbox network access. A project may only lower budgets \
and may only turn the sandbox network off."
        }
        _ => {
            "Retention of raw run data (handles, transcripts, vault) and of audit logs. `duet purge` deletes \
runs older than data.retention_days. A project may only shorten raw-data retention."
        }
    }
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(s) = app.selected_setting() else {
        return;
    };
    let origin = app.cfg.origin(s.key);
    let file = match origin_name(origin) {
        "owner" => format!(", file {}", app.paths.owner.display()),
        "project" => format!(", file {}", app.paths.project.display()),
        _ => String::new(),
    };
    let mut lines = vec![
        Line::from(Span::styled(
            s.key,
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(s.help),
        Line::from(format!("type: {}", kind_text(s.kind))),
        Line::from(format!("default: {}", s.default)),
        Line::from(format!("from: {}{file}", origin_name(origin))),
        Line::from(format!("scope: {}", scope_text(s))),
    ];
    if s.confirm {
        lines.push(Line::styled(
            "loosening it needs confirmation, shows a policy diff and is audited",
            Style::new().fg(Color::Yellow),
        ));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" setting ")),
        area,
    );
}

fn draw_doctor(f: &mut Frame, app: &App, area: Rect) {
    let (title, lines) = match &app.doctor {
        None => (" doctor ".to_owned(), vec![Line::from("d runs the checks")]),
        Some((online, checks)) => {
            let mut lines = Vec::new();
            for c in checks {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<5}", c.status), status_style(&c.status)),
                    Span::raw(format!("{:<16} {}", c.name, c.detail)),
                ]));
                if let Some(fix) = &c.fix {
                    lines.push(Line::styled(
                        format!("      fix: {}", fix.replace('\n', "; ")),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
            }
            let mode = if *online {
                "online: listings only, never a model call"
            } else {
                "offline: no network; o adds --online"
            };
            (format!(" doctor ({mode}) "), lines)
        }
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title)),
        area,
    );
}

/// What the effective policy decides for `path`, with the patterns that matched.
pub(crate) fn tester_lines(app: &App, path: &str) -> Vec<String> {
    let policy = app.policy();
    let p = Path::new(path);
    let matching = |keys: &[&str]| -> String {
        let hits: Vec<String> = keys
            .iter()
            .flat_map(|k| app.cfg.list(k).unwrap_or_default())
            .filter(|g| duet_boundary::policy::glob_match(g, path))
            .map(|g| format!("\"{g}\""))
            .collect();
        if hits.is_empty() {
            String::new()
        } else {
            format!(" ({})", hits.join(", "))
        }
    };
    let yes = |b: bool| if b { "yes" } else { "no" };
    vec![
        format!("path: {path}"),
        format!(
            "sensitive: {}{}",
            yes(policy.is_sensitive_path(p)),
            matching(&["sensitivity.globs", "sensitivity.protected_paths"])
        ),
        format!(
            "secret sink: {}{}",
            yes(policy.is_secret_sink(p)),
            matching(&["sensitivity.secret_sinks"])
        ),
        format!(
            "secret-bearing name: {}",
            yes(duet_boundary::policy::is_secret_bearing(p))
        ),
        format!(
            "IP level: {}{}",
            crate::ip::level_name(policy.ip_level(p)),
            matching(&["ip.sealed", "ip.interface_only"])
        ),
    ]
}

fn draw_tester(f: &mut Frame, app: &App, area: Rect) {
    let editing = matches!(app.mode, crate::app::Mode::Tester);
    let lines: Vec<Line> = if app.tester.is_empty() && !editing {
        vec![Line::from(
            "t tests a path against the sensitivity globs, secret sinks and IP levels",
        )]
    } else {
        tester_lines(app, &app.tester)
            .into_iter()
            .map(Line::from)
            .collect()
    };
    let title = if editing {
        " path tester (typing; Enter or Esc ends) "
    } else {
        " path tester "
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title)),
        area,
    );
}
