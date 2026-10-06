// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings overlay's layout (inside the workspace): the screen bar, the
//! current screen, the key line with the edit prompt or the last status, and
//! the confirmation dialog.

use crate::app::{App, Mode, Tab};
use declass_config::{Origin, Proposal, Target};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Tabs, Wrap};
use toml::Value;

/// The frame of every settings screen's panel: rounded, muted (a panel that
/// shows focus or a warning sets its own border colour after it).
pub(crate) fn frame() -> Block<'static> {
    Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
}

pub(crate) fn origin_name(origin: Option<Origin>) -> &'static str {
    match origin {
        Some(Origin::Default) | None => "default",
        Some(Origin::Owner) => "owner",
        Some(Origin::Project) => "project",
        Some(Origin::Policy) => "policy",
    }
}

pub(crate) fn status_style(status: &str) -> Style {
    match status.to_ascii_lowercase().as_str() {
        "pass" => Style::new().fg(Color::Green),
        "warn" => Style::new().fg(Color::Yellow),
        "fail" => Style::new().fg(Color::Red),
        _ => Style::new().fg(Color::DarkGray),
    }
}

/// Draws the screens into `area` (the workspace's overlay).
pub(crate) fn draw_in(f: &mut Frame, app: &mut App, area: Rect) {
    // A terminal shrunk below the minimum while running shows why, not a
    // clipped screen.
    if let Err(message) = crate::check_size(area.width, area.height) {
        f.render_widget(
            Paragraph::new(format!("{message}; enlarge the window to continue"))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let [bar, body, keys] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .areas(area);
    let target = match app.target {
        Target::Owner => format!(
            " edits go to: owner config ({}) ",
            app.paths.owner.display()
        ),
        Target::Project => format!(
            " edits go to: project config ({}; tighten only) ",
            app.paths.project.display()
        ),
    };
    f.render_widget(
        Tabs::new(
            Tab::ALL
                .iter()
                .enumerate()
                .map(|(i, t)| format!("{} {}", i + 1, t.title())),
        )
        .select(app.tab.index())
        .highlight_style(
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(
            crate::ui::frame()
                .title(" declass settings · Esc back to the conversation ")
                .title_bottom(target),
        ),
        bar,
    );
    match app.tab {
        Tab::Ip => crate::ip::draw(f, app, body),
        Tab::Audit => crate::audit::draw(f, &mut app.audit, body),
        Tab::Runs => crate::runs::draw(f, &app.runs, body),
        _ => crate::settings::draw(f, app, body),
    }
    draw_keys(f, app, keys);
    match &app.mode {
        Mode::Confirm(p) => draw_confirm(f, app, p, area),
        Mode::ConfirmPurge(plan) => crate::data::draw_confirm(f, plan),
        _ => {}
    }
}

fn key_help(app: &App) -> &'static str {
    match (&app.mode, app.tab) {
        (Mode::Edit { .. }, _) => "Enter apply · Esc cancel",
        (Mode::Confirm(_), _) => "y apply this loosening change · n or Esc cancel",
        (Mode::Tester, _) => "type a workspace path · Enter or Esc done",
        (Mode::Sample, _) => "type sample text · Enter or Esc done",
        (Mode::ConfirmPurge(_), _) => "y delete · n or Esc cancel",
        (_, Tab::Models) => {
            "Enter edit · d doctor · o online checks · l detect local · [ ] u use · c cache probe · p owner/project · Esc back"
        }
        (_, Tab::Sensitivity) => {
            "Enter edit · a add entry · t test a path · s test text · p owner/project · Tab screens · Esc back"
        }
        (_, Tab::Data) => {
            "Enter edit · x purge old runs · X purge all runs · p owner/project · Tab screens · Esc back"
        }
        (_, Tab::Ip) => {
            "Enter open/close · i interface-only · s sealed · u unmark · p owner/project · Esc back"
        }
        (_, Tab::Audit) => "↑↓ select · Enter records · Esc runs · v verify · r reload · Esc back",
        (_, Tab::Runs) => "[ ] run · ↑↓ scroll/file · ←→ panel · J/K diff · f follow · Esc back",
        _ => "Enter edit · a add entry · p owner/project · Tab screens · Esc back",
    }
}

fn draw_keys(f: &mut Frame, app: &App, area: Rect) {
    if let Mode::Edit {
        key,
        buffer,
        append,
    } = &app.mode
    {
        let (what, hint) = match declass_config::setting(key).map(|s| s.kind) {
            _ if *append => ("add to", "one entry".to_owned()),
            Some(declass_config::Kind::List | declass_config::Kind::Patterns) => {
                ("set", "TOML array".to_owned())
            }
            Some(declass_config::Kind::Str) | None => ("set", String::new()),
            Some(k) => ("set", crate::settings::kind_text(k)),
        };
        return draw_prompt(f, area, what, key, buffer, &hint);
    }
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(app.status.clone(), Style::new().fg(Color::Cyan)),
            Line::styled(key_help(app), Style::new().fg(Color::DarkGray)),
        ]),
        area,
    );
}

fn draw_prompt(f: &mut Frame, area: Rect, what: &str, key: &str, buffer: &str, hint: &str) {
    let hint = if hint.is_empty() {
        String::new()
    } else {
        format!("  ({hint})")
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("{what} {key}: "),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::raw(buffer.to_owned()),
                Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            ]),
            Line::styled(
                format!("Enter apply · Esc cancel{hint}"),
                Style::new().fg(Color::DarkGray),
            ),
        ]),
        area,
    );
}

/// The policy diff of a change: removed and added list entries, or old and new.
pub(crate) fn diff(old: &Value, new: &Value) -> Vec<(char, String)> {
    match (old.as_array(), new.as_array()) {
        (Some(o), Some(n)) => o
            .iter()
            .filter(|e| !n.contains(e))
            .map(|e| ('-', e.to_string()))
            .chain(
                n.iter()
                    .filter(|e| !o.contains(e))
                    .map(|e| ('+', e.to_string())),
            )
            .collect(),
        _ => vec![('-', old.to_string()), ('+', new.to_string())],
    }
}

fn draw_confirm(f: &mut Frame, app: &App, p: &Proposal, within: Rect) {
    let [area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(within);
    let [area] = Layout::vertical([Constraint::Length(16)])
        .flex(Flex::Center)
        .areas(area);
    let mut lines = vec![
        Line::styled(
            "This change loosens privacy.",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::styled(p.key.clone(), Style::new().add_modifier(Modifier::BOLD)),
    ];
    for (sign, text) in diff(&p.old, &p.new) {
        let color = if sign == '-' {
            Color::Red
        } else {
            Color::Green
        };
        lines.push(Line::styled(
            format!("  {sign} {text}"),
            Style::new().fg(color),
        ));
    }
    lines.extend([
        Line::from(""),
        Line::from(format!(
            "weakens: {}",
            p.weakens.as_deref().unwrap_or_default()
        )),
        Line::from(format!("writes: {}", app.paths.owner.display())),
        Line::from(format!(
            "recorded in: {}",
            app.paths.config_audit().display()
        )),
        Line::from(""),
        Line::styled(
            "y apply    n cancel",
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            crate::ui::frame()
                .title(" confirm loosening ")
                .border_style(Style::new().fg(Color::Red)),
        ),
        area,
    );
}
