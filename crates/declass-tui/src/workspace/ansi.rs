// SPDX-License-Identifier: GPL-3.0-or-later
//! declass's styled lines as ratatui lines.
//!
//! The feed, the Markdown renderer and the editor write lines with declass's own
//! SGR sequences (bold, dim, italic, the terminal's 16 colours, reset; see
//! `term::Style`). Text from outside declass has passed `term::safe` before it
//! is styled, so the only escape sequences here are declass's; any other escape
//! byte is dropped, never drawn.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// A line with declass's SGR sequences as a ratatui line.
pub(super) fn line(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut run = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut params = String::new();
                let mut fin = None;
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        fin = Some(d);
                        break;
                    }
                    params.push(d);
                }
                if fin == Some('m') {
                    if !run.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut run), style));
                    }
                    style = sgr(style, &params);
                }
            }
            continue;
        }
        if c.is_control() {
            continue;
        }
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, style));
    }
    Line::from(spans)
}

fn sgr(mut style: Style, params: &str) -> Style {
    if params.is_empty() {
        return Style::default();
    }
    for p in params.split(';') {
        style = match p.parse::<u8>() {
            Ok(0) => Style::default(),
            Ok(1) => style.add_modifier(Modifier::BOLD),
            Ok(2) => style.add_modifier(Modifier::DIM),
            Ok(3) => style.add_modifier(Modifier::ITALIC),
            Ok(7) => style.add_modifier(Modifier::REVERSED),
            Ok(22) => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            Ok(23) => style.remove_modifier(Modifier::ITALIC),
            Ok(27) => style.remove_modifier(Modifier::REVERSED),
            Ok(n @ 30..=37) => style.fg(colour(n - 30, false)),
            Ok(39) => style.fg(Color::Reset),
            Ok(n @ 90..=97) => style.fg(colour(n - 90, true)),
            _ => style,
        };
    }
    style
}

fn colour(n: u8, bright: bool) -> Color {
    match (n, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        _ => Color::White,
    }
}

/// `line` cut into rows of at most `width` columns, at spaces where it can
/// (a word longer than a row is cut by character); styles are kept. An empty
/// line is one empty row.
pub(super) fn wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    wrap_widths(line, width, width)
}

/// [`wrap`] with the first row `first` columns wide and the others `rest`.
pub(super) fn wrap_widths(line: &Line<'static>, first: usize, rest: usize) -> Vec<Line<'static>> {
    let (first, rest) = (first.max(1), rest.max(1));
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect();
    let w = |c: char| c.width().unwrap_or(0);
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let width = if rows.is_empty() { first } else { rest };
        // The longest run from `start` that fits.
        let mut used = 0;
        let mut end = start;
        while end < chars.len() && used + w(chars[end].0) <= width {
            used += w(chars[end].0);
            end += 1;
        }
        if end == start {
            // A character wider than the row: it goes alone.
            end = start + 1;
        }
        let mut next = end;
        if end < chars.len() {
            // Break after the last space in the run, if there is one past its start.
            if let Some(space) = (start + 1..end).rev().find(|&i| chars[i].0 == ' ') {
                end = space;
                next = space + 1;
            }
        }
        rows.push(row(&chars[start..end]));
        // A row does not start with the space it broke at.
        start = next;
    }
    if rows.is_empty() {
        rows.push(Line::default());
    }
    rows
}

/// Characters with styles as one line, a span per run of one style.
fn row(chars: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for &(c, style) in chars {
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), style)),
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn declasss_styles_become_ratatui_styles() {
        let l = line("\x1b[1;36mdeclass: \x1b[0mhi \x1b[2mdim\x1b[0m");
        assert_eq!(text(&l), "declass: hi dim");
        assert_eq!(l.spans[0].style.fg, Some(Color::Cyan));
        assert!(l.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(l.spans[1].style, Style::default());
        assert!(l.spans[2].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn other_escapes_and_controls_are_never_drawn() {
        // Only an SGR sequence changes the style; anything else vanishes.
        let l = line("a\x1b[2Jb\x1b]52;c;x\x07c\rd\x1b");
        assert!(!text(&l).contains('\x1b'));
        assert!(!text(&l).chars().any(char::is_control), "{:?}", text(&l));
    }

    #[test]
    fn wrapping_counts_columns_and_keeps_styles() {
        let l = line("\x1b[32mabcdef\x1b[0m中文");
        let rows = wrap(&l, 4);
        let texts: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(texts, ["abcd", "ef中", "文"]);
        assert_eq!(rows[1].spans[0].style.fg, Some(Color::Green));
        assert_eq!(wrap(&Line::default(), 10).len(), 1);
        // At spaces where it can; a longer word is cut.
        let texts: Vec<String> = wrap(&line("each source asked receives"), 12)
            .iter()
            .map(text)
            .collect();
        assert_eq!(texts, ["each source", "asked", "receives"]);
        let texts: Vec<String> = wrap(&line("a verylongword"), 5).iter().map(text).collect();
        assert_eq!(texts, ["a", "veryl", "ongwo", "rd"]);
    }
}
