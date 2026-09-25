// SPDX-License-Identifier: GPL-3.0-or-later
//! Frame layout: the screen bar, the current screen, the key line with the
//! edit prompt or the last status, and the confirmation dialog.

use crate::app::{App, Mode, Tab};
use duet_config::{Origin, Proposal, Target};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Tabs, Wrap};
use toml::Value;

pub(crate) fn origin_name(origin: Option<Origin>) -> &'static str {
    match origin {
        Some(Origin::Default) | None => "default",
        Some(Origin::Owner) => "owner",
        Some(Origin::Project) => "project",
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

pub(crate) fn draw(f: &mut Frame, app: &mut App) {
    // A terminal shrunk below the minimum while running shows why, not a
    // clipped screen.
    let area = f.area();
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
    .areas(f.area());
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
        .block(Block::bordered().title(" duet ").title_bottom(target)),
        bar,
    );
    match app.tab {
        Tab::Ip => crate::ip::draw(f, app, body),
        Tab::Audit => crate::audit::draw(f, &mut app.audit, body),
        Tab::Run => {
            let input = session_input(app);
            crate::runs::draw(f, &app.runs, body, input.as_ref())
        }
        _ => crate::settings::draw(f, app, body),
    }
    draw_keys(f, app, keys);
    match &app.mode {
        Mode::Confirm(p) => draw_confirm(f, app, p),
        Mode::ConfirmPurge(plan) => crate::data::draw_confirm(f, plan),
        Mode::Launch {
            objective,
            mode,
            session,
        } => crate::launch::draw_dialog(f, objective, *mode, *session),
        Mode::AckPassthrough { objective, session } => {
            crate::launch::draw_ack(f, objective, *session)
        }
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
        (Mode::Launch { session: true, .. }, _) => {
            "type the first message · ↑↓ mode · Enter start · Esc cancel"
        }
        (Mode::Launch { .. }, _) => "type the objective · ↑↓ mode · Enter start · Esc cancel",
        (Mode::Message { .. }, _) => {
            "Enter send (while duet works it steers the turn) · /stop after this step · Ctrl-C stop now · ↑↓ scroll · Esc done"
        }
        (Mode::AckPassthrough { .. }, _) => {
            "y start with the privacy boundary off · n or Esc cancel"
        }
        (_, Tab::Models) => {
            "Enter edit · d doctor · o online checks · l detect local · [ ] u use · c cache probe · p owner/project · q quit"
        }
        (_, Tab::Sensitivity) => {
            "Enter edit · a add entry · t test a path · s test text · p owner/project · Tab screens · q quit"
        }
        (_, Tab::Data) => {
            "Enter edit · x purge old runs · X purge all runs · p owner/project · Tab screens · q quit"
        }
        (_, Tab::Ip) => {
            "Enter open/close · i interface-only · s sealed · u unmark · p owner/project · q quit"
        }
        (_, Tab::Audit) => "↑↓ select · Enter records · Esc runs · v verify · r reload · q quit",
        (_, Tab::Run) => {
            "n new session · o one-shot run · r resume · i type · s stop after step · x stop now · ←→ panel · ↑↓ scroll/file · J/K diff · [ ] run · f follow · q quit"
        }
        _ => "Enter edit · a add entry · p owner/project · Tab screens · q quit",
    }
}

/// The Run view's input box, when the selected run is a session.
fn session_input(app: &App) -> Option<crate::runs::Input> {
    let state = app.runs.session.as_ref()?;
    let live = app.live_session().is_some();
    let (text, focused) = match &app.mode {
        Mode::Message { buffer } if live => (buffer.clone(), true),
        _ => (String::new(), false),
    };
    let hint = match (live, focused, state.working) {
        (true, true, true) => " duet is working: your message steers it after the current step ",
        (true, true, false) if state.asked => " duet asked you a question: type the answer ",
        (true, true, false) => " your message ",
        (true, false, true) => {
            " duet is working · i type to steer · s stop after step · x stop now "
        }
        (true, false, false) => " i type a message ",
        (false, _, _) if state.closed => " this session is closed · n starts a new one ",
        (false, _, _) => " not running here · r resumes this session ",
    };
    Some(crate::runs::Input {
        text,
        focused,
        hint: hint.into(),
    })
}

fn draw_keys(f: &mut Frame, app: &App, area: Rect) {
    if let Mode::Edit {
        key,
        buffer,
        append,
    } = &app.mode
    {
        let (what, hint) = match duet_config::setting(key).map(|s| s.kind) {
            _ if *append => ("add to", "one entry".to_owned()),
            Some(duet_config::Kind::List | duet_config::Kind::Patterns) => {
                ("set", "TOML array".to_owned())
            }
            Some(duet_config::Kind::Str) | None => ("set", String::new()),
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

fn draw_confirm(f: &mut Frame, app: &App, p: &Proposal) {
    let [area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(f.area());
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
            Block::bordered()
                .title(" confirm loosening ")
                .border_style(Style::new().fg(Color::Red)),
        ),
        area,
    );
}
