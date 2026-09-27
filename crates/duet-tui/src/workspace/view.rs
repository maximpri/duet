// SPDX-License-Identifier: GPL-3.0-or-later
//! Drawing the workspace: the status bar, the conversation, the input box and
//! the key line.

use super::{Mode, State, ansi};
use crate::term::{Span as TSpan, Style as TStyle};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use std::time::Instant;

/// The smallest screen the layout is drawn on; below it, a notice.
const MIN: (u16, u16) = (40, 10);

/// The input box's most rows of text (it scrolls within them).
const INPUT_ROWS: usize = 8;

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
    let [top, body, input_area, keys] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);
    status_bar(f, s, top);
    let panel_width = s.panel.width(area.width);
    if panel_width > 0 {
        let [talk, side] =
            Layout::horizontal([Constraint::Min(30), Constraint::Length(panel_width)]).areas(body);
        conversation(f, s, talk, now);
        s.panel.draw(f, side, &s.status);
    } else {
        conversation(f, s, body, now);
    }
    input_box(f, s, input_area, input_rows, cursor);
    key_line(f, s, keys);
    choices(f, s, input_area);
    if s.overlay
        && let Some(app) = s.app.as_mut()
    {
        // Over everything but the status bar.
        let over = Rect {
            y: area.y + 1,
            height: area.height - 1,
            ..area
        };
        f.render_widget(ratatui::widgets::Clear, over);
        crate::ui::draw_in(f, app, over);
    }
}

fn status_bar(f: &mut Frame<'_>, s: &State, area: Rect) {
    let st = &s.status;
    let mut left = format!(
        " duet · {}",
        if st.mode.is_empty() { "…" } else { &st.mode }
    );
    if !st.frontier.is_empty() {
        left.push_str(&format!(" · {}", st.frontier));
    }
    if let Some(local) = &st.local {
        left.push_str(&format!(" ▸ {local}"));
    }
    let right = if st.session.is_empty() {
        String::from("new session ")
    } else {
        format!(
            "turn {} · ${:.4} of ${:.2} ",
            st.turns, st.cost_usd, st.budget_usd
        )
    };
    let pad = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let bar = Line::from(vec![
        Span::styled(left, Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::raw(right),
    ])
    .style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_widget(Paragraph::new(bar), area);
}

fn conversation(f: &mut Frame<'_>, s: &mut State, area: Rect, now: Instant) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().add_modifier(Modifier::DIM))
        .title(if s.scroll > 0 {
            format!(" conversation · {} rows below (PgDn) ", s.scroll)
        } else {
            " conversation ".to_owned()
        });
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    s.set_width(inner.width as usize);
    // Below the history: the reply row being streamed and the working status.
    let mut live: Vec<Line<'static>> = Vec::new();
    if let Some(p) = s.feed.partial() {
        live.extend(ansi::wrap(&ansi::line(&p), inner.width as usize));
    }
    if s.mode == Mode::Working
        && let Some(st) = s
            .feed
            .status(now, "type to steer · /stop after this step · Ctrl-C now")
    {
        live.extend(ansi::wrap(&ansi::line(&st), inner.width as usize));
    }
    let height = inner.height as usize;
    let live_rows = live.len().min(height);
    let room = height - live_rows;
    s.page = room.max(1);
    let end = s.rows.len().saturating_sub(s.scroll);
    let start = end.saturating_sub(room);
    let mut shown: Vec<Line<'static>> = s.rows[start..end].to_vec();
    if s.scroll == 0 {
        shown.extend(live.into_iter().take(live_rows));
    }
    f.render_widget(Paragraph::new(shown), inner);
}

/// The input's rows and the cursor within them.
fn input(s: &State, width: usize) -> (Vec<Line<'static>>, (usize, usize)) {
    let prompt = if s.answering {
        vec![TSpan::new("approve? y/n › ", TStyle::WARN.bold())]
    } else {
        vec![TSpan::new("› ", TStyle::PLAIN.bold())]
    };
    let (rows, cursor) = s
        .editor
        .display(&prompt, s.colour, width.max(10), INPUT_ROWS);
    (rows.iter().map(|r| ansi::line(r)).collect(), cursor)
}

fn input_box(
    f: &mut Frame<'_>,
    s: &State,
    area: Rect,
    rows: Vec<Line<'static>>,
    (row, col): (usize, usize),
) {
    let title = match (s.answering, s.mode) {
        (true, _) => " approval: y or n ",
        (false, Mode::Working) => " steer duet (it is working) ",
        (false, Mode::Hidden) => " message (duet is setting up) ",
        (false, Mode::Prompt) => " message ",
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    f.render_widget(Paragraph::new(rows), inner);
    if s.overlay {
        return;
    }
    f.set_cursor_position(Position {
        x: inner.x + (col as u16).min(inner.width.saturating_sub(1)),
        y: inner.y + (row as u16).min(inner.height.saturating_sub(1)),
    });
}

/// The completions, in a box just above the input.
fn choices(f: &mut Frame<'_>, s: &State, input: Rect) {
    const SHOWN: usize = 10;
    if s.choices.is_empty() {
        return;
    }
    let mut lines: Vec<Line<'static>> = s
        .choices
        .iter()
        .take(SHOWN)
        .map(|c| Line::from(c.clone()))
        .collect();
    if s.choices.len() > SHOWN {
        lines.push(Line::styled(
            format!("… {} more (type more of it)", s.choices.len() - SHOWN),
            Style::new().add_modifier(Modifier::DIM),
        ));
    }
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4;
    let height = lines.len() as u16 + 2;
    let area = Rect {
        x: input.x + 2,
        y: input.y.saturating_sub(height),
        width: width.min(input.width.saturating_sub(2)),
        height: height.min(input.y),
    };
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Tab completes "),
        ),
        area,
    );
}

fn key_line(f: &mut Frame<'_>, s: &State, area: Rect) {
    let keys = match s.mode {
        Mode::Working => {
            " ⏎ steer · /stop after this step · Ctrl-C stop now · PgUp/PgDn scroll · Ctrl-T panel · /help"
        }
        _ => {
            " ⏎ send · Alt-⏎ new line · Tab complete · PgUp/PgDn scroll · Ctrl-T panel · /help · Ctrl-D leave"
        }
    };
    f.render_widget(
        Paragraph::new(keys).style(Style::new().add_modifier(Modifier::DIM)),
        area,
    );
}
