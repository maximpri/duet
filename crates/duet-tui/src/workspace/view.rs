// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawing the workspace: the header, the conversation (and the welcome on an
//! empty session), the side rail, the input with its palette and file picker,
//! the status line, and what opens over them (the approval dialog, help, the
//! settings).

use super::cells::palette::{self, ACCENT, BAD, GOOD, HELD, MUTED, WARN};
use super::{Mode, State, ansi};
use crate::term::Span as TSpan;
use duet_agent::TurnEnd;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Wrap,
};
use std::time::Instant;

/// The smallest screen the layout is drawn on; below it, a notice.
const MIN: (u16, u16) = (40, 10);

/// The input's most rows of text (it scrolls within them).
const INPUT_ROWS: usize = 6;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn draw(f: &mut Frame<'_>, s: &mut State, now: Instant) {
    let area = f.area();
    if area.width < MIN.0 || area.height < MIN.1 {
        let note = Paragraph::new(format!(
            "duet needs at least {}x{}; this terminal is {}x{}",
            MIN.0, MIN.1, area.width, area.height
        ));
        f.render_widget(note, area);
        return;
    }
    let inner_width = area.width.saturating_sub(4) as usize;
    let (input_rows, cursor) = input(s, inner_width);
    let input_height = input_rows.len().clamp(1, INPUT_ROWS) as u16 + 2;
    let header_height = if area.height >= 16 { 2 } else { 1 };
    let [top, body, input_area, status] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(3),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);
    header(f, s, top);
    let rail = s.panel.width(area.width);
    if rail > 0 {
        let [talk, side] =
            Layout::horizontal([Constraint::Min(30), Constraint::Length(rail)]).areas(body);
        conversation(f, s, talk, now);
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_style(palette::muted());
        let inner = block.inner(side);
        f.render_widget(block, side);
        s.panel.draw(f, inner.inner(Margin::new(1, 0)), &s.status);
    } else {
        conversation(f, s, body, now);
    }
    composer(f, s, input_area, input_rows, cursor);
    status_line(f, s, status, now);
    popups(f, s, input_area);
    if s.answering {
        approval(f, s, area);
    }
    if s.help {
        help(f, s, area);
    }
    if s.overlay
        && let Some(app) = s.app.as_mut()
    {
        // Over everything but the header.
        let over = Rect {
            y: area.y + header_height,
            height: area.height - header_height,
            ..area
        };
        f.render_widget(Clear, over);
        crate::ui::draw_in(f, app, over);
    }
}

fn header(f: &mut Frame<'_>, s: &State, area: Rect) {
    let area = area.inner(Margin::new(2, 0));
    let place =
        s.ws.as_ref()
            .and_then(|w| w.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".into());
    let hint = if area.width >= 96 {
        "F1 help · Ctrl-O details · Ctrl-T panel · F2 settings"
    } else {
        "F1 help · / commands"
    };
    let name = format!("DUET  /  {}", crate::term::safe(&place));
    let gap = (area.width as usize).saturating_sub(name.chars().count() + hint.chars().count());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(name, palette::accent()),
            Span::raw(" ".repeat(gap)),
            Span::styled(hint, palette::muted()),
        ])),
        Rect { height: 1, ..area },
    );
    if area.height < 2 {
        return;
    }
    let st = &s.status;
    let mut spans = Vec::new();
    let dot = || Span::styled(" · ", palette::muted());
    match st.mode.as_str() {
        "" => spans.push(Span::styled("starting", palette::muted())),
        "passthrough" => {
            spans.push(Span::styled(
                "passthrough",
                Style::new().fg(BAD).add_modifier(Modifier::BOLD),
            ));
            spans.push(dot());
            spans.push(Span::styled(
                format!("frontier {}", st.frontier),
                palette::muted(),
            ));
            spans.push(dot());
            spans.push(Span::styled(
                "privacy boundary OFF: everything is sent as it is",
                Style::new().fg(BAD),
            ));
        }
        "top clearance" => {
            spans.push(Span::styled(
                "TOP CLEARANCE",
                Style::new().fg(HELD).add_modifier(Modifier::BOLD),
            ));
            spans.push(dot());
            match &st.local {
                Some(l) => spans.push(Span::styled(format!("local {l} only"), palette::muted())),
                None => spans.push(Span::styled("no local model", Style::new().fg(WARN))),
            }
            spans.push(dot());
            spans.push(Span::styled(
                "nothing leaves this machine: no frontier, web or network",
                Style::new().fg(HELD),
            ));
        }
        mode => {
            spans.push(Span::styled(
                mode.to_owned(),
                Style::new().fg(GOOD).add_modifier(Modifier::BOLD),
            ));
            spans.push(dot());
            spans.push(Span::styled(
                format!("frontier {}", st.frontier),
                palette::muted(),
            ));
            spans.push(dot());
            match &st.local {
                Some(l) => spans.push(Span::styled(format!("local {l}"), palette::muted())),
                None => spans.push(Span::styled("no local model", Style::new().fg(WARN))),
            }
            spans.push(dot());
            spans.push(Span::styled(
                "sensitive values stay on this machine",
                Style::new().fg(HELD),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
}

/// The welcome on an empty session.
pub(super) fn welcome(s: &State) -> Vec<Line<'static>> {
    let key = |k: &str, what: &str| {
        Line::from(vec![
            Span::styled(format!("{k:<10}"), palette::accent()),
            Span::raw(what.to_owned()),
        ])
    };
    let mut lines = vec![
        Line::styled("What are we working on?", palette::accent()),
        Line::default(),
        Line::from(
            "Describe the outcome. duet reads, edits and checks the code; you can steer it while it works.",
        ),
    ];
    if s.status.mode != "passthrough" {
        lines.push(Line::styled(
            "Secrets and personal data in this workspace are read on this machine; the frontier sees placeholders.",
            Style::new().fg(HELD),
        ));
    }
    lines.extend([
        Line::default(),
        key("@", "name a file of the workspace"),
        key("/", "commands (/models checks the connections, /settings)"),
        key("F1", "keys and commands"),
        Line::default(),
    ]);
    lines
}

fn conversation(f: &mut Frame<'_>, s: &mut State, area: Rect, now: Instant) {
    let view = area.inner(Margin::new(2, 1));
    if view.width < 4 || view.height == 0 {
        return;
    }
    // One column is kept for the scrollbar.
    let width = view.width.saturating_sub(1) as usize;
    s.set_width(width);
    let rows = s.transcript(width, now);
    let height = view.height as usize;
    // While scrolled up, the view holds on what is being read.
    if s.scroll > 0 && rows.len() > s.total {
        s.scroll += rows.len() - s.total;
    }
    s.total = rows.len();
    s.page = height.max(1);
    let max = s.total.saturating_sub(height);
    s.scroll = s.scroll.min(max);
    let top = max - s.scroll;
    let end = (top + height).min(rows.len());
    let mut shown: Vec<Line<'static>> = rows[top..end].to_vec();
    s.view = Rect {
        width: width as u16,
        ..view
    };
    s.view_top = top;
    if let Some((a, b)) = s.sel {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        for (i, line) in shown.iter_mut().enumerate() {
            let r = top + i;
            if r >= a.0 && r <= b.0 {
                let from = if r == a.0 { a.1 } else { 0 };
                let to = if r == b.0 { b.1 } else { usize::MAX };
                highlight(line, from, to);
            }
        }
    }
    f.render_widget(
        Paragraph::new(shown),
        Rect {
            width: width as u16,
            ..view
        },
    );
    if s.total > height {
        let mut state = ScrollbarState::new(max + 1)
            .position(top)
            .viewport_content_length(height);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("┃")
                .thumb_style(Style::new().fg(MUTED))
                .track_symbol(None)
                .begin_symbol(None)
                .end_symbol(None),
            view,
            &mut state,
        );
    }
    if s.scroll > 0 {
        let note = format!(" {} more below · Ctrl-End ", s.scroll);
        let w = note.chars().count() as u16;
        f.render_widget(
            Paragraph::new(Span::styled(note, Style::new().fg(Color::Black).bg(ACCENT))),
            Rect {
                x: view.x + view.width.saturating_sub(w + 2),
                y: view.y + view.height.saturating_sub(1),
                width: w.min(view.width),
                height: 1,
            },
        );
    }
}

/// What runs now, under the conversation while duet works.
pub(super) fn working_row(s: &State, text: &str, width: usize, now: Instant) -> Vec<Line<'static>> {
    let frame = SPINNER[(now.duration_since(s.started).as_millis() / 100 % 10) as usize];
    ansi::wrap(
        &Line::from(vec![
            Span::styled(format!("{frame} "), Style::new().fg(ACCENT)),
            Span::styled(crate::term::safe(text), palette::muted()),
        ]),
        width,
    )
}

/// Columns `from`..`to` of `line` shown selected.
fn highlight(line: &mut Line<'static>, from: usize, to: usize) {
    use unicode_width::UnicodeWidthChar;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0;
    for span in std::mem::take(&mut line.spans) {
        let mut part = String::new();
        let mut inside = None;
        for c in span.content.chars() {
            let now_inside = col >= from && col < to;
            if inside.is_some_and(|i| i != now_inside) && !part.is_empty() {
                let style = if inside == Some(true) {
                    span.style.add_modifier(Modifier::REVERSED)
                } else {
                    span.style
                };
                out.push(Span::styled(std::mem::take(&mut part), style));
            }
            inside = Some(now_inside);
            part.push(c);
            col += c.width().unwrap_or(0);
        }
        if !part.is_empty() {
            let style = if inside == Some(true) {
                span.style.add_modifier(Modifier::REVERSED)
            } else {
                span.style
            };
            out.push(Span::styled(part, style));
        }
    }
    line.spans = out;
}

/// The input's rows and the cursor within them.
fn input(s: &State, width: usize) -> (Vec<Line<'static>>, (usize, usize)) {
    let (rows, cursor) = s
        .editor
        .display(&[] as &[TSpan], s.colour, width.max(10), INPUT_ROWS);
    (rows.iter().map(|r| ansi::line(r)).collect(), cursor)
}

fn composer(
    f: &mut Frame<'_>,
    s: &State,
    area: Rect,
    rows: Vec<Line<'static>>,
    (row, col): (usize, usize),
) {
    let (title, hints, colour) = match s.mode {
        Mode::Working => (
            " Steer duet ",
            " Enter steer · /stop after this step · Ctrl-C stop now ",
            ACCENT,
        ),
        Mode::Hidden => (" duet is setting up ", " you can type your message ", MUTED),
        Mode::Prompt => (
            " Message duet ",
            " Enter send · Alt-Enter new line · @ file · / commands ",
            ACCENT,
        ),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(colour))
        .title(Span::styled(
            title,
            Style::new().fg(colour).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(hints, palette::muted()));
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if s.editor.buffer().is_empty() {
        let placeholder = match s.mode {
            Mode::Working => "Add guidance for duet (it arrives after the current step)…",
            _ => "Describe a task or ask a question · @ names a file · / for commands…",
        };
        f.render_widget(
            Paragraph::new(Span::styled(placeholder, palette::muted())),
            inner,
        );
        return;
    }
    f.render_widget(Paragraph::new(rows), inner);
    if s.overlay || s.help || s.answering {
        return;
    }
    f.set_cursor_position(Position {
        x: inner.x + (col as u16).min(inner.width.saturating_sub(1)),
        y: inner.y + (row as u16).min(inner.height.saturating_sub(1)),
    });
}

fn status_line(f: &mut Frame<'_>, s: &State, area: Rect, now: Instant) {
    let working = s.mode == Mode::Working;
    let spin = SPINNER[(now.duration_since(s.started).as_millis() / 100 % 10) as usize];
    let (badge, bg) = if s.answering {
        (" ! APPROVE ".to_owned(), WARN)
    } else if working {
        (format!(" {spin} WORKING "), ACCENT)
    } else if s.mode == Mode::Hidden {
        (format!(" {spin} STARTING "), MUTED)
    } else {
        match &s.ended {
            Some(TurnEnd::Completed { .. }) => (" DONE ".to_owned(), GOOD),
            Some(TurnEnd::Asked { .. }) => (" YOUR ANSWER ".to_owned(), WARN),
            Some(TurnEnd::Failed { .. }) => (" FAILED ".to_owned(), BAD),
            Some(TurnEnd::Interrupted | TurnEnd::Stopped) => (" STOPPED ".to_owned(), WARN),
            Some(TurnEnd::BudgetStopped { .. }) => (" LIMIT ".to_owned(), WARN),
            _ => (" READY ".to_owned(), GOOD),
        }
    };
    let st = &s.status;
    let mut facts = Vec::new();
    if st.session.is_empty() {
        facts.push("new session".to_owned());
    } else {
        facts.push(format!("turn {}", st.turns));
        facts.push(format!("${:.4} of ${:.2}", st.cost_usd, st.budget_usd));
        if st.tokens_in + st.tokens_out > 0 {
            facts.push(format!(
                "{} in · {} out",
                compact(st.tokens_in),
                compact(st.tokens_out)
            ));
        }
    }
    let copied = s
        .copied
        .filter(|(at, _)| now.duration_since(*at).as_secs() < 3)
        .map(|(_, n)| format!("copied {n} characters · "));
    let right = format!(
        "{}{}",
        copied.unwrap_or_default(),
        if s.details {
            "details on · F1 help "
        } else {
            "F1 help "
        }
    );
    let left = format!(" │ {}", facts.join(" · "));
    let used = badge.chars().count() + left.chars().count() + right.chars().count();
    let gap = (area.width as usize).saturating_sub(used);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                badge,
                Style::new()
                    .fg(Color::Black)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(left, palette::muted()),
            Span::raw(" ".repeat(gap)),
            Span::styled(right, palette::muted()),
        ])),
        area,
    );
}

fn compact(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// The command palette, the `@` picker, or completions, above the input.
fn popups(f: &mut Frame<'_>, s: &mut State, input: Rect) {
    if s.overlay || s.help || s.answering {
        return;
    }
    let palette_items = s.palette_items();
    let (title, lines, selected): (String, Vec<Line<'static>>, Option<usize>) =
        if !palette_items.is_empty() {
            let sel = s.palette.min(palette_items.len() - 1);
            let width = palette_items
                .iter()
                .map(|(c, _)| c.len())
                .max()
                .unwrap_or(8)
                + 3;
            (
                format!(" commands · {}/{} ", sel + 1, palette_items.len()),
                palette_items
                    .iter()
                    .map(|(c, d)| {
                        Line::from(vec![
                            Span::styled(format!("{c:<width$}"), palette::accent()),
                            Span::styled((*d).to_owned(), Style::new()),
                        ])
                    })
                    .collect(),
                Some(sel),
            )
        } else {
            let picks = s.picks();
            if !picks.is_empty() {
                let sel = s.pick.min(picks.len() - 1);
                (
                    " files · Tab or Enter picks ".to_owned(),
                    picks.into_iter().map(Line::from).collect(),
                    Some(sel),
                )
            } else if !s.choices.is_empty() {
                (
                    " Tab completes ".to_owned(),
                    s.choices.iter().take(10).cloned().map(Line::from).collect(),
                    None,
                )
            } else {
                return;
            }
        };
    let shown = 10usize.min(lines.len());
    let start = selected.map_or(0, |sel| sel.saturating_sub(shown - 1));
    let mut lines: Vec<Line<'static>> = lines.into_iter().skip(start).take(shown).collect();
    if let Some(sel) = selected {
        let i = sel - start;
        if let Some(l) = lines.get_mut(i) {
            l.spans.insert(0, Span::styled("› ", palette::accent()));
            for sp in l.spans.iter_mut().skip(1) {
                sp.style = sp.style.add_modifier(Modifier::REVERSED);
            }
        }
        for (j, l) in lines.iter_mut().enumerate() {
            if j != i {
                l.spans.insert(0, Span::raw("  "));
            }
        }
    }
    let width = (lines.iter().map(Line::width).max().unwrap_or(10) as u16 + 4)
        .max(title.chars().count() as u16 + 4)
        .min(input.width.saturating_sub(2));
    let height = lines.len() as u16 + 2;
    let area = Rect {
        x: input.x + 1,
        y: input.y.saturating_sub(height),
        width,
        height: height.min(input.y),
    };
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(Span::styled(title, palette::muted())),
        ),
        area,
    );
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// The approval dialog: the question, then No (the default) and Yes.
fn approval(f: &mut Frame<'_>, s: &State, area: Rect) {
    let question = s
        .question
        .clone()
        .unwrap_or_else(|| "duet asks for your approval".into());
    let width = area.width.saturating_sub(4).min(84);
    let body = Paragraph::new(question.clone()).wrap(Wrap { trim: false });
    let rows = question
        .lines()
        .map(|l| ansi::wrap(&Line::from(l.to_owned()), width.saturating_sub(4) as usize).len())
        .sum::<usize>() as u16;
    let height = (rows + 6).clamp(8, area.height.saturating_sub(2));
    let popup = centered(width, height, area);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(WARN))
        .title(Span::styled(
            " ! approval ",
            Style::new().fg(WARN).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    f.render_widget(block, popup);
    let [text, choices, keys] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(body, text);
    let choice = |n: &str, label: &str, on: bool| {
        if on {
            Line::from(Span::styled(
                format!("› {n}  {label}"),
                Style::new()
                    .fg(Color::Black)
                    .bg(WARN)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(format!("  {n}  {label}"))
        }
    };
    f.render_widget(
        Paragraph::new(vec![
            choice("1", "No, deny", !s.approve),
            choice("2", "Yes, allow", s.approve),
        ]),
        choices,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            "↑↓ or y/n choose · Enter answers · Esc denies",
            palette::muted(),
        )),
        keys,
    );
}

/// F1: the keys and commands.
fn help(f: &mut Frame<'_>, s: &State, area: Rect) {
    let head = |t: &str| Line::styled(t.to_owned(), palette::accent());
    let row = |k: &str, what: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {k:<28}"),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(what.to_owned()),
        ])
    };
    let lines = vec![
        head("MESSAGE"),
        row(
            "Enter",
            "send (while duet works: steer it after its current step)",
        ),
        row(
            "Alt-Enter · Shift-Enter · Ctrl-J",
            "a new line in the message",
        ),
        row("@", "name a file of the workspace (↑↓ pick, Tab or Enter)"),
        row("/", "commands (↑↓ pick, Tab or Enter)"),
        row("↑ ↓ · Ctrl-R", "earlier messages; search them"),
        row(
            "Ctrl-W · Ctrl-U · Ctrl-K",
            "cut a word, to the start, to the end",
        ),
        Line::default(),
        head("SESSION"),
        row("Ctrl-C", "stop duet's turn now (twice while idle: leave)"),
        row("/stop", "stop after the current step"),
        row("Ctrl-O", "show or fold tool results"),
        row("PgUp · PgDn · wheel", "scroll the conversation"),
        row("Ctrl-End", "back to the latest"),
        row("Ctrl-D", "leave (the session stays open: duet --resume)"),
        Line::default(),
        head("PANEL AND SETTINGS"),
        row(
            "Ctrl-T",
            "the side panel: Changes, Privacy, Session, closed",
        ),
        row(
            "Ctrl-↑ ↓ · Ctrl-PgUp PgDn",
            "pick a changed file; scroll its diff",
        ),
        row(
            "F2 · /settings",
            "the settings screens (/models /ip /audit /runs …)",
        ),
        Line::default(),
        head("COMMANDS"),
        row(
            "/status /diff /undo",
            "the budgets; what changed; revert the last turn",
        ),
        row("/image PATH", "attach an image to your next message"),
        row("/quit · /close", "leave (resumable) · end the session"),
        Line::default(),
        Line::styled(
            "  To select text with the mouse, hold Option (macOS) or Shift while dragging.",
            palette::muted(),
        ),
    ];
    let width = area.width.saturating_sub(6).min(96);
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = centered(width, height, area);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).scroll((s.help_scroll, 0)).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(Span::styled(
                    " duet · keys and commands ",
                    palette::accent(),
                ))
                .title_bottom(Span::styled(
                    " ↑↓ PgUp PgDn scroll · Esc or F1 closes ",
                    palette::muted(),
                )),
        ),
        popup,
    );
}
