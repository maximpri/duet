// SPDX-License-Identifier: GPL-3.0-or-later
//! How declass's output looks on a terminal: styles, the feed of declass's work
//! (progress, streamed replies as Markdown, turn ends), the input editor, and
//! [`safe`], through which everything from the frontier, tools or files passes
//! before it is drawn (control characters and escape sequences could rewrite
//! the screen, set the clipboard or forge declass's own lines).
//!
//! The workspace (`crate::workspace`) draws these in full screen; `declass run`
//! shows the same progress on standard error, and the line-by-line session
//! (no terminal) prints it as it is.

pub mod editor;
pub mod feed;
pub mod markdown;
pub mod stream;
pub mod words;

use std::fmt::Write as _;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Whether output is styled with colour: a terminal, unless `NO_COLOR` is
/// set to anything (no-color.org) or the terminal is `dumb`.
pub fn colour(terminal: bool) -> bool {
    colour_for(
        terminal,
        std::env::var_os("NO_COLOR"),
        std::env::var("TERM").ok(),
    )
}

/// [`colour`] from the environment's values.
fn colour_for(terminal: bool, no_color: Option<std::ffi::OsString>, term: Option<String>) -> bool {
    terminal && no_color.is_none_or(|v| v.is_empty()) && term.as_deref() != Some("dumb")
}

/// Whether the live screen (cursor movement) can be used on a terminal.
pub fn capable() -> bool {
    std::env::var("TERM").map_or(true, |t| t != "dumb")
}

/// A text style. Colours are the terminal's own 16, so the theme decides
/// the exact shades.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<u8>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
}

impl Style {
    pub const PLAIN: Style = Style {
        fg: None,
        bold: false,
        dim: false,
        italic: false,
    };
    const fn fg(code: u8) -> Style {
        Style {
            fg: Some(code),
            ..Style::PLAIN
        }
    }
    pub const DIM: Style = Style {
        dim: true,
        ..Style::PLAIN
    };
    /// declass's name on its replies, headings, bullets.
    pub const ACCENT: Style = Style::fg(36);
    /// Inline code and code blocks.
    pub const CODE: Style = Style::fg(33);
    /// What the boundary withheld from the frontier.
    pub const HELD: Style = Style::fg(35);
    pub const GOOD: Style = Style::fg(32);
    pub const WARN: Style = Style::fg(33);
    pub const BAD: Style = Style::fg(31);

    pub const fn bold(self) -> Style {
        Style { bold: true, ..self }
    }
    pub const fn italic(self) -> Style {
        Style {
            italic: true,
            ..self
        }
    }

    /// `self` with `over`'s attributes added (its colour wins).
    pub fn with(self, over: Style) -> Style {
        Style {
            fg: over.fg.or(self.fg),
            bold: self.bold || over.bold,
            dim: self.dim || over.dim,
            italic: self.italic || over.italic,
        }
    }

    fn sgr(self) -> String {
        let mut codes = Vec::new();
        if self.bold {
            codes.push("1".to_owned());
        }
        if self.dim {
            codes.push("2".to_owned());
        }
        if self.italic {
            codes.push("3".to_owned());
        }
        if let Some(c) = self.fg {
            codes.push(c.to_string());
        }
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// A run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

/// Spans as terminal text: styled when `colour`, else the text alone.
pub fn paint(spans: &[Span], colour: bool) -> String {
    let mut out = String::new();
    for s in spans {
        if colour && s.style != Style::PLAIN && !s.text.is_empty() {
            let _ = write!(out, "{}{}\x1b[0m", s.style.sgr(), s.text);
        } else {
            out.push_str(&s.text);
        }
    }
    out
}

/// One styled piece of text as terminal text.
pub fn styled(text: &str, style: Style, colour: bool) -> String {
    paint(&[Span::new(text, style)], colour)
}

/// Display width of text without escape sequences.
pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Display width of text that may hold our own SGR sequences.
pub fn visible_width(text: &str) -> usize {
    width(&strip_sgr(text))
}

fn strip_sgr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `text` cut to `max` columns (an ellipsis marks the cut). Text with our
/// own SGR sequences keeps them, and the style is reset at the cut.
pub fn clip_width(text: &str, max: usize) -> String {
    if visible_width(text) <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            out.push(c);
            for d in chars.by_ref() {
                out.push(d);
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        let w = char_width(c);
        if used + w + 1 > max {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    if out.contains('\x1b') {
        out.push_str("\x1b[0m");
    }
    out
}

/// Text from outside declass (the frontier, tools, files, the operator) made
/// safe to write to a terminal: escape sequences, control characters and
/// text-direction overrides are removed, tabs become four spaces and
/// carriage returns are dropped. Newlines stay.
pub fn safe(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\x1b' => skip_escape(&mut chars),
            // C1 CSI and OSC introducers start sequences too.
            '\u{9b}' => skip_until_final(&mut chars),
            '\u{9d}' => skip_string(&mut chars),
            // Direction overrides could make a line read other than it is.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => skip_until_final(chars),
        Some(']' | 'P' | '_' | '^' | 'X') => skip_string(chars),
        // A two-character sequence (ESC and one more), or a lone ESC.
        _ => {}
    }
}

/// A CSI sequence: parameters up to a final byte `@`..`~`.
fn skip_until_final(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('@'..='~').contains(&c) {
            break;
        }
    }
}

/// A string sequence (OSC, DCS, ...): up to BEL or ST (`ESC \`).
fn skip_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        match c {
            '\x07' | '\u{9c}' => break,
            '\x1b' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
                break;
            }
            _ => {}
        }
    }
}

/// The live region at the bottom of the screen: rows drawn in place and
/// redrawn without entering the scrollback. Output goes above it.
#[derive(Default)]
pub struct Region {
    /// Rows between the region's first row and the cursor.
    up: usize,
    shown: bool,
    /// The drawn rows' widths and the cursor, for a resize.
    widths: Vec<usize>,
    cursor: (usize, usize),
}

impl Region {
    /// Moves to the region's first row and clears it and everything below.
    pub fn clear(&mut self, out: &mut String) {
        if self.shown {
            out.push('\r');
            if self.up > 0 {
                let _ = write!(out, "\x1b[{}A", self.up);
            }
            out.push_str("\x1b[J");
        }
        self.shown = false;
        self.up = 0;
    }

    /// The terminal is now `columns` wide and has re-wrapped the region's
    /// rows (as most terminals do): where its first row is now.
    pub fn resized(&mut self, columns: usize) {
        let columns = columns.max(1);
        let (row, col) = self.cursor;
        self.up = self
            .widths
            .iter()
            .take(row)
            .map(|w| w.div_ceil(columns).max(1))
            .sum::<usize>()
            + col / columns;
    }

    /// Draws `rows` (each at most the terminal's width) from the cursor's
    /// row, which must be the first of the region (after [`Region::clear`]),
    /// and puts the cursor at `cursor` (row, column), or after the last row.
    pub fn draw(&mut self, out: &mut String, rows: &[String], cursor: Option<(usize, usize)>) {
        if rows.is_empty() {
            return;
        }
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                out.push_str("\r\n");
            }
            out.push_str(row);
        }
        let last = rows.len() - 1;
        self.widths = rows.iter().map(|r| visible_width(r)).collect();
        match cursor {
            Some((row, col)) => {
                let row = row.min(last);
                if last > row {
                    let _ = write!(out, "\x1b[{}A", last - row);
                }
                out.push('\r');
                if col > 0 {
                    let _ = write!(out, "\x1b[{col}C");
                }
                self.up = row;
                self.cursor = (row, col);
            }
            None => {
                self.up = last;
                self.cursor = (last, self.widths[last]);
            }
        }
        self.shown = true;
    }

    /// Writes `lines` above the region, then draws it again.
    pub fn print_above(
        &mut self,
        out: &mut String,
        lines: &[String],
        rows: &[String],
        cursor: Option<(usize, usize)>,
    ) {
        self.clear(out);
        for l in lines {
            out.push_str(l);
            out.push_str("\x1b[0m\r\n");
        }
        self.draw(out, rows, cursor);
    }
}

/// A frame for the terminal: drawn at once where the terminal supports
/// synchronized output (others ignore the marks), with the cursor hidden
/// while it moves.
pub fn frame(body: &str, show_cursor: bool) -> String {
    format!(
        "\x1b[?2026h\x1b[?25l{body}{}\x1b[?2026l",
        if show_cursor { "\x1b[?25h" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_and_controls_never_reach_the_terminal() {
        let hostile =
            "ok\x1b[2J\x1b]52;c;ZXZpbA==\x07 red\x1b[31m done\x1bc\u{9b}1;1H\r\x08\ttab\nnext";
        assert_eq!(safe(hostile), "ok red done    tab\nnext");
        assert_eq!(safe("\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\"), "link");
        assert_eq!(safe("é⟨EMAIL#1⟩ 中文"), "é⟨EMAIL#1⟩ 中文");
        assert_eq!(safe("rm \u{202e}txt.exe"), "rm txt.exe");
    }

    #[test]
    fn no_color_or_a_dumb_terminal_turns_colour_off() {
        let term = || Some("xterm-256color".to_owned());
        assert!(colour_for(true, None, term()));
        assert!(!colour_for(true, Some("1".into()), term()));
        // Set but empty counts as unset (no-color.org).
        assert!(colour_for(true, Some("".into()), term()));
        assert!(!colour_for(true, None, Some("dumb".into())));
        assert!(!colour_for(false, None, term()));
    }

    #[test]
    fn painting_follows_the_colour_choice() {
        let spans = [
            Span::new("declass: ", Style::ACCENT.bold()),
            Span::new("hi", Style::PLAIN),
        ];
        assert_eq!(paint(&spans, false), "declass: hi");
        assert_eq!(paint(&spans, true), "\x1b[1;36mdeclass: \x1b[0mhi");
        assert_eq!(visible_width(&paint(&spans, true)), 11);
    }

    #[test]
    fn clipping_counts_columns_and_keeps_styles_closed() {
        assert_eq!(clip_width("abcdef", 4), "abc…");
        assert_eq!(clip_width("中文字", 4), "中…");
        let c = clip_width("\x1b[2mabcdef\x1b[0m", 4);
        assert_eq!(c, "\x1b[2mabc…\x1b[0m");
        assert_eq!(visible_width(&c), 4);
    }

    #[test]
    fn the_region_redraws_in_place() {
        let mut r = Region::default();
        let mut out = String::new();
        r.draw(&mut out, &["status".into(), "you> hi".into()], Some((1, 7)));
        assert_eq!(out, "status\r\nyou> hi\r\x1b[7C");
        out.clear();
        r.print_above(&mut out, &["line".into()], &["you> ".into()], Some((0, 5)));
        assert_eq!(out, "\r\x1b[1A\x1b[Jline\x1b[0m\r\nyou> \r\x1b[5C");
        out.clear();
        r.clear(&mut out);
        assert_eq!(out, "\r\x1b[J");
        // After a resize the terminal has re-wrapped the rows.
        r.draw(&mut out, &["a".repeat(30), "you> hi".into()], Some((1, 7)));
        r.resized(20);
        out.clear();
        r.clear(&mut out);
        assert_eq!(out, "\r\x1b[2A\x1b[J");
    }
}
