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
    s.palette_popup = None;
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
    let chip_height = u16::from(!s.attachments.is_empty());
    let input_height = input_rows.len().clamp(1, INPUT_ROWS) as u16 + 2 + chip_height;
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
    if s.find.open {
        find_bar(f, s, input_area);
    } else {
        popups(f, s, input_area);
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
    s.plan.draw(f, area);
    if s.answering {
        approval(f, s, area);
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
        "Ctrl-V paste · Ctrl-F find · F1 help · F2 settings"
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
    if st.planning {
        spans.push(Span::styled("PLAN · read-only · ", palette::accent()));
    }
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
                "no frontier · web and command network off",
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
                "privacy boundary active",
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
            "The privacy boundary filters what the frontier sees. The Privacy panel shows sources, local questions and outbound records.",
            Style::new().fg(HELD),
        ));
    }
    lines.extend([
        Line::default(),
        key("@", "name a file of the workspace"),
        key("/", "commands (/models checks the connections, /settings)"),
        key("Ctrl-V", "paste text or an image from your clipboard"),
        key("Ctrl-F", "find in conversation · F4 command palette"),
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
    let transcript = s.transcript(width, now);
    let height = view.height as usize;
    // While scrolled up, the view holds on what is being read.
    if s.scroll > 0 && transcript.total > s.total {
        s.scroll += transcript.total - s.total;
    }
    s.total = transcript.total;
    s.page = height.max(1);
    let max = s.total.saturating_sub(height);
    s.scroll = s.scroll.min(max);
    let top = max - s.scroll;
    let end = (top + height).min(transcript.total);
    let mut shown = transcript.rows(&s.cells, top..end);
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
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0;
    for span in std::mem::take(&mut line.spans) {
        let mut part = String::new();
        let mut inside = None;
        for c in span.content.graphemes(true) {
            let now_inside = col < to && col + c.width() > from;
            if inside.is_some_and(|i| i != now_inside) && !part.is_empty() {
                let style = if inside == Some(true) {
                    span.style.add_modifier(Modifier::REVERSED)
                } else {
                    span.style
                };
                out.push(Span::styled(std::mem::take(&mut part), style));
            }
            inside = Some(now_inside);
            part.push_str(c);
            col += c.width();
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

fn find_bar(f: &mut Frame<'_>, s: &State, input_area: Rect) {
    let area = Rect {
        x: input_area.x,
        y: input_area.y.saturating_sub(3),
        width: input_area.width,
        height: 3,
    };
    let count = s.find.matches.len();
    let tally = if count == 0 {
        "no matches".to_owned()
    } else {
        format!("{} / {count}", s.find.current + 1)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(format!(" Find · {tally} "))
        .title_bottom(" Enter next · Shift-Enter previous · Esc close ");
    let inner = block.inner(area).inner(Margin::new(1, 0));
    let (rows, (row, col)) = s
        .find
        .query
        .display(&[], s.colour, inner.width.max(1) as usize, 1);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(rows.iter().map(|r| ansi::line(r)).collect::<Vec<_>>()),
        inner,
    );
    f.set_cursor_position(Position {
        x: inner.x + (col as u16).min(inner.width.saturating_sub(1)),
        y: inner.y + row as u16,
    });
}

fn composer(
    f: &mut Frame<'_>,
    s: &mut State,
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
            " Enter send · Alt-Enter line · Ctrl-V paste · F4 commands ",
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
    let mut inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if !s.attachments.is_empty() {
        let labels = s
            .attachments
            .iter()
            .map(|a| {
                format!(
                    "{} {}",
                    crate::term::safe(&a.id),
                    crate::term::safe(&a.label)
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Attached  ", palette::accent()),
                Span::raw(labels),
                Span::styled("  · /detach ID removes", palette::muted()),
            ])),
            Rect { height: 1, ..inner },
        );
        inner.y += 1;
        inner.height = inner.height.saturating_sub(1);
    }
    s.composer = inner;
    if s.editor.buffer().is_empty() {
        let placeholder = match s.mode {
            Mode::Working => "Add guidance for duet (it arrives after the current step)…",
            _ => "Describe a task or ask a question · @ names a file · / for commands…",
        };
        f.render_widget(
            Paragraph::new(Span::styled(placeholder, palette::muted())),
            inner,
        );
    } else {
        f.render_widget(Paragraph::new(rows), inner);
    }
    if s.overlay || s.help || s.answering || s.plan.active() || s.find.open {
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
    if st.planning {
        facts.push("PLAN · read-only".to_owned());
    }
    if let Some(plan) = s.plan.summary() {
        facts.push(crate::term::safe(&plan));
    }
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
    let right = match &s.toast {
        Some((_, text)) => format!("{text} "),
        None if s.details => "details on · F1 help ".to_owned(),
        None => "F1 help ".to_owned(),
    };
    let left = if s.toast.is_some() {
        " │ ".to_owned()
    } else {
        format!(" │ {}", facts.join(" · "))
    };
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
    if s.overlay || s.help || s.answering || s.plan.active() {
        return;
    }
    let palette_items = s.palette_items();
    if !palette_items.is_empty() {
        command_palette(f, s, input, &palette_items);
        return;
    }
    let picks = s.picks();
    let (title, lines, selected): (String, Vec<Line<'static>>, Option<usize>) = if !picks.is_empty()
    {
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

/// All commands fit on tall terminals; smaller windows keep the selected row
/// visible and explicitly state how much is hidden above and below.
fn command_palette(f: &mut Frame<'_>, s: &mut State, input: Rect, items: &[(&str, &str)]) {
    let top = f.area().y + if f.area().height >= 16 { 2 } else { 1 };
    let available = input.y.saturating_sub(top);
    let visible = usize::from(available.saturating_sub(3)).min(items.len());
    if visible == 0 {
        return;
    }
    s.palette = s.palette.min(items.len() - 1);
    s.palette_start = s.palette_start.min(items.len() - visible);
    if s.palette < s.palette_start {
        s.palette_start = s.palette;
    } else if s.palette >= s.palette_start + visible {
        s.palette_start = s.palette + 1 - visible;
    }
    let first = s.palette_start;
    let end = first + visible;
    let title = format!(" Commands · {} total ", items.len());
    let keys = if input.width >= 90 {
        " ↑↓ / wheel browse · PgUp/PgDn page · Enter run · Tab insert · Esc close "
    } else if input.width >= 60 {
        " ↑↓ / wheel · PgUp/PgDn · Enter run · Esc close "
    } else {
        " ↑↓ browse · Enter run · Esc close "
    };
    let label_width = items.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 2;
    let desired = items
        .iter()
        .map(|(_, description)| label_width + description.chars().count() + 5)
        .max()
        .unwrap_or(0)
        .max(keys.chars().count() + 2)
        .max(54);
    let width = (desired.min(u16::MAX as usize) as u16).min(input.width.saturating_sub(2));
    let height = visible as u16 + 3;
    let area = Rect {
        x: input.x + 1,
        y: input.y - height,
        width,
        height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(title, palette::accent()))
        .title_bottom(Span::styled(keys, palette::muted()));
    let inner = block.inner(area);
    let rows = Rect {
        height: visible as u16,
        width: inner.width.saturating_sub(1),
        ..inner
    };
    let lines: Vec<_> = items[first..end]
        .iter()
        .enumerate()
        .map(|(offset, (command, description))| {
            let selected = first + offset == s.palette;
            let style = if selected {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new()
            };
            Line::from(vec![
                Span::styled(if selected { "› " } else { "  " }, palette::accent()),
                Span::styled(format!("{command:<label_width$}"), style.fg(ACCENT)),
                Span::styled((*description).to_owned(), style),
            ])
        })
        .collect();
    let mut parts = vec![format!("{}–{end} of {}", first + 1, items.len())];
    if first > 0 {
        parts.push(format!("↑ {first} above"));
    }
    if end < items.len() {
        parts.push(format!("↓ {} more", items.len() - end));
    }
    let mut range = parts.join(" · ");
    if range.chars().count() + 18 <= inner.width as usize {
        range.push_str(" · type to filter");
    }
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), rows);
    f.render_widget(
        Paragraph::new(Span::styled(range, palette::accent())),
        Rect {
            y: inner.y + visible as u16,
            height: 1,
            ..inner
        },
    );
    if visible < items.len() {
        let mut scrollbar = ScrollbarState::new(items.len() - visible + 1)
            .position(first)
            .viewport_content_length(visible);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_style(palette::accent())
                .track_symbol(Some("│"))
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            Rect {
                height: visible as u16,
                ..inner
            },
            &mut scrollbar,
        );
    }
    s.palette_popup = Some(super::PalettePopup {
        area,
        rows,
        first,
        query: s.editor.buffer().to_owned(),
    });
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
fn help(f: &mut Frame<'_>, s: &mut State, area: Rect) {
    let head = |t: &str| Line::styled(t.to_owned(), palette::accent());
    let row = |k: &str, what: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {k:<30}  "),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(what.to_owned()),
        ])
    };
    let lines = vec![
        head("PLAN"),
        row("F5 / F6", "review saved plan / reopen clarification"),
        row(
            "Plan: Tab then Enter",
            "select action; Close is the default",
        ),
        row(
            "Plan editor: Ctrl-S",
            "save a revision; Enter inserts a line",
        ),
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
            "Shift + arrows · drag",
            "select message text (Ctrl/Alt + Shift: words)",
        ),
        row(
            "Ctrl-A · Ctrl-C · Ctrl-X",
            "select all input · copy selection · cut selection",
        ),
        row(
            "Ctrl-V · Shift-Insert",
            "paste text or an image; Enter sends afterward",
        ),
        row(
            "Ctrl-Z · Ctrl-Y",
            "undo / redo message edits (Ctrl-Shift-Z also redoes)",
        ),
        row(
            "Home / End · Ctrl-Home / End",
            "line / message start and end",
        ),
        row("Ctrl-P", "choose a workspace file"),
        row(
            "F4 · Ctrl-Shift-P",
            "command palette; Esc restores your draft",
        ),
        Line::default(),
        head("COPY, FIND AND IMAGES"),
        row(
            "Drag in conversation",
            "select text; Ctrl-C copies without interrupting",
        ),
        row(
            "Ctrl-Shift-C · /copy",
            "copy the latest reply when nothing is selected",
        ),
        row("Ctrl-F · /find", "find in the visible conversation text"),
        row(
            "Enter / Shift-Enter in Find",
            "next / previous match; Esc returns to your draft",
        ),
        row("F3 / Shift-F3", "next / previous match after closing Find"),
        row(
            "/paste · Ctrl-V",
            "read your local clipboard on this action only",
        ),
        row(
            "/image PATH · drop path",
            "attach an image for the next message",
        ),
        row(
            "/attachments · /detach ID",
            "list queued attachments · remove one (or all)",
        ),
        row(
            "SSH / unavailable clipboard",
            "use terminal text paste, or /image PATH on the host",
        ),
        row(
            "Ctrl-W · Ctrl-U · Ctrl-K",
            "cut a word, to the start, to the end",
        ),
        Line::default(),
        head("SESSION"),
        row(
            "Ctrl-C (no selection)",
            "stop duet's turn now (twice while idle: leave)",
        ),
        row("/stop", "stop after the current step"),
        row(
            "/plan [task]",
            "plan with read-only tools; pause automatic goals",
        ),
        row(
            "/plan status · /plan off",
            "inspect mode or leave; implementation needs a new request",
        ),
        row(
            "/goal start TEXT",
            "work toward a goal; 20 turns by default",
        ),
        row("/goal TEXT", "also starts a goal"),
        row(
            "/goal pause · /goal resume",
            "pause or continue automatic work",
        ),
        row(
            "/goal status · /goal cancel",
            "show progress or end the goal",
        ),
        row(
            "Goal limits",
            "session dollar and working-time caps still apply",
        ),
        row("Ctrl-O", "show or fold tool results"),
        row("PgUp · PgDn · wheel", "scroll the conversation"),
        row("Ctrl-End (empty input)", "back to the latest"),
        row("Ctrl-Alt-Z", "suspend the TUI; fg in the shell resumes"),
        row("Ctrl-D", "leave (the session stays open: duet --resume)"),
        Line::default(),
        head("PANEL AND SETTINGS"),
        row(
            "Tab · Shift-Tab",
            "next / previous panel tab (outside input completions)",
        ),
        row(
            "Ctrl-T",
            "the side panel: Changes, Privacy, Session, closed",
        ),
        row(
            "Ctrl-O in Privacy",
            "summary / exact audit record for the selected action",
        ),
        row(
            "Ctrl-↑ ↓ · Ctrl-PgUp PgDn",
            "pick a file or privacy event; scroll its details",
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
        row(
            "/history [ID]",
            "find earlier sessions and how to resume them",
        ),
        row(
            "/attach PATH",
            "attach a text file (dragged paths also work)",
        ),
        row("/quit · /close", "leave (resumable) · end the session"),
        Line::default(),
        Line::styled(
            "  Terminal shortcuts vary. Native Cmd-V pastes text; use Ctrl-V or /paste for images.",
            palette::muted(),
        ),
    ];
    let width = area.width.saturating_sub(6).min(100);
    let lines: Vec<_> = lines
        .iter()
        .flat_map(|line| ansi::wrap(line, width.saturating_sub(2) as usize))
        .collect();
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    s.help_scroll = s
        .help_scroll
        .min((lines.len() as u16).saturating_sub(height.saturating_sub(2)));
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
