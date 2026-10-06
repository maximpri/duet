// SPDX-License-Identifier: GPL-3.0-or-later
//! declass's replies as they read on a terminal: light Markdown (headings,
//! bullet and numbered lists, quotes, rules, fenced code, inline code and
//! bold) wrapped to the terminal's width, fed piece by piece as the reply
//! streams.
//!
//! A block starts with a label (`declass: `) and its later rows are indented
//! under it, as in the plain output. Rows are produced as soon as they are
//! complete; the row being written is available as [`Markdown::partial`]
//! for the live region. The result does not depend on how the text was cut
//! into pieces. Without colour the structure stays (bullets, wrapping,
//! indentation) and the inline marks (`` ` ``, `**`) are kept as written,
//! since nothing else would show them.

use super::{Span, Style, char_width, paint, safe, width};

/// Rows narrower than this are not wrapped further (a very narrow
/// terminal overflows instead).
const MIN_TEXT: usize = 10;
/// A line's start is decided within this many bytes.
const HEAD_MAX: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Paragraph,
    Heading,
    Bullet,
    Number,
    Quote,
    /// A line inside a fenced code block.
    Code,
    Table,
    Rule,
    Blank,
    FenceOpen(char, usize),
    FenceClose,
}

impl Kind {
    fn wraps(self) -> bool {
        matches!(
            self,
            Kind::Paragraph | Kind::Heading | Kind::Bullet | Kind::Number | Kind::Quote
        )
    }
}

/// Inline marks being read (colour only).
#[derive(Default, Clone, Copy)]
struct Inline {
    code: bool,
    bold: bool,
    /// A `*` that may start `**`.
    star: bool,
}

#[derive(Default)]
struct Line {
    /// The start of the line while its kind is undecided.
    head: String,
    kind: Option<Kind>,
    /// Text style of the line (a heading is bold, a quote italic).
    base: Style,
    /// What continuation rows of this line start with (inside the block).
    hang: Vec<Span>,
    row: Vec<Span>,
    row_width: usize,
    /// Width of the row's leading part (list marker, hang), after which a
    /// word is preceded by a space.
    row_base: usize,
    word: Vec<Span>,
    word_width: usize,
    space: bool,
    inline: Inline,
}

/// A block of declass's text being rendered.
pub struct Markdown {
    colour: bool,
    columns: usize,
    label: Vec<Span>,
    label_width: usize,
    indent: usize,
    rows: usize,
    fence: Option<(char, usize)>,
    blank: bool,
    line: Line,
}

impl Markdown {
    /// A block `columns` wide whose first row starts with `label` and whose
    /// other rows are indented by `indent` columns.
    pub fn new(columns: usize, colour: bool, label: Vec<Span>, indent: usize) -> Self {
        let label_width = label.iter().map(|s| width(&s.text)).sum();
        Self {
            colour,
            columns,
            label,
            label_width,
            indent,
            rows: 0,
            fence: None,
            blank: false,
            line: Line::default(),
        }
    }

    /// Renders a whole text at once.
    pub fn render(mut self, text: &str) -> Vec<String> {
        let mut rows = self.push(text);
        rows.extend(self.finish());
        rows
    }

    /// Adds text; returns the rows it completed.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for c in safe(text).chars() {
            self.char(c, &mut out);
        }
        out
    }

    /// Ends the block: the row being written is complete.
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.line.head.is_empty() || self.line.kind.is_some() {
            self.end_line(&mut out);
        }
        out
    }

    /// The row being written, as it would be shown now, if there is one.
    pub fn partial(&self) -> Option<String> {
        let l = &self.line;
        let mut spans = self.prefix();
        match l.kind {
            None if l.head.trim().is_empty() => return None,
            None => spans.push(Span::new(l.head.trim_start(), l.base)),
            Some(k) if !k.wraps() => {
                if l.row.is_empty() {
                    return None;
                }
                spans.extend(l.row.iter().cloned());
            }
            Some(_) => {
                if l.row_width <= l.row_base && l.word.is_empty() {
                    return None;
                }
                let gap = usize::from(l.space && l.row_width > l.row_base && !l.word.is_empty());
                if l.row_width + gap + l.word_width > self.capacity() && l.row_width > l.row_base {
                    spans.extend(l.hang.iter().cloned());
                } else {
                    spans.extend(l.row.iter().cloned());
                    if gap == 1 {
                        spans.push(Span::new(" ", l.base));
                    }
                }
                spans.extend(l.word.iter().cloned());
            }
        }
        let text = paint(&spans, self.colour);
        Some(super::clip_width(
            &text,
            self.columns.saturating_sub(1).max(1),
        ))
    }

    /// Columns for text in the current row.
    fn capacity(&self) -> usize {
        let prefix = if self.rows == 0 {
            self.label_width
        } else {
            self.indent
        };
        self.columns.saturating_sub(prefix).max(MIN_TEXT)
    }

    fn prefix(&self) -> Vec<Span> {
        if self.rows == 0 {
            self.label.clone()
        } else {
            vec![Span::new(" ".repeat(self.indent), Style::PLAIN)]
        }
    }

    fn emit(&mut self, content: Vec<Span>, out: &mut Vec<String>) {
        if self.blank && self.rows > 0 {
            out.push(String::new());
        }
        self.blank = false;
        let mut spans = self.prefix();
        spans.extend(content);
        let text = paint(&spans, self.colour);
        out.push(text.trim_end().to_owned());
        self.rows += 1;
    }

    fn char(&mut self, c: char, out: &mut Vec<String>) {
        if c == '\n' {
            self.end_line(out);
            return;
        }
        match self.line.kind {
            None => {
                self.line.head.push(c);
                if let Some(kind) = classify(&self.line.head, self.fence, false) {
                    self.start(kind, out);
                } else if self.line.head.len() > HEAD_MAX {
                    self.start(Kind::Paragraph, out);
                }
            }
            Some(k) if k.wraps() => self.text(c, out),
            Some(Kind::Code | Kind::Table) => self.raw(c),
            // A fence's info string, a rule's or a blank line's rest.
            Some(_) => {}
        }
    }

    /// Decides the line's kind from its start and feeds the rest of the
    /// start as text.
    fn start(&mut self, kind: Kind, out: &mut Vec<String>) {
        let head = std::mem::take(&mut self.line.head);
        self.line.kind = Some(kind);
        let lead = head.len() - head.trim_start_matches(' ').len();
        let rest = &head[lead..];
        let nest = || "  ".repeat((lead / 2).min(4));
        let (marker, content): (Vec<Span>, &str) = match kind {
            Kind::Paragraph => (vec![], rest),
            Kind::Heading => {
                self.line.base = Style::ACCENT.bold();
                let hashes = rest.len() - rest.trim_start_matches('#').len();
                if self.colour {
                    (vec![], rest[hashes..].trim_start())
                } else {
                    (vec![], rest)
                }
            }
            Kind::Bullet => (
                vec![
                    Span::new(nest(), Style::PLAIN),
                    Span::new("•", Style::ACCENT),
                    Span::new(" ", Style::PLAIN),
                ],
                &rest[2..],
            ),
            Kind::Number => {
                let end = rest.find(' ').unwrap_or(rest.len());
                (
                    vec![
                        Span::new(nest(), Style::PLAIN),
                        Span::new(&rest[..end], Style::ACCENT),
                        Span::new(" ", Style::PLAIN),
                    ],
                    rest[end..].trim_start_matches(' '),
                )
            }
            Kind::Quote => {
                self.line.base = Style::DIM.italic();
                (
                    vec![Span::new("│ ", Style::DIM)],
                    rest[1..].strip_prefix(' ').unwrap_or(&rest[1..]),
                )
            }
            Kind::Code => {
                self.line.row = vec![Span::new("│ ", Style::DIM)];
                for c in head.chars() {
                    self.raw(c);
                }
                return;
            }
            Kind::Table => {
                for c in head.chars() {
                    self.raw(c);
                }
                return;
            }
            Kind::FenceOpen(c, n) => {
                self.fence = Some((c, n));
                return;
            }
            Kind::FenceClose => {
                self.fence = None;
                return;
            }
            Kind::Rule => {
                let n = self.capacity().min(40);
                self.emit(vec![Span::new("─".repeat(n), Style::DIM)], out);
                return;
            }
            Kind::Blank => return,
        };
        let marker_width = marker.iter().map(|s| width(&s.text)).sum();
        self.line.hang = match kind {
            Kind::Quote => marker.clone(),
            _ => vec![Span::new(" ".repeat(marker_width), Style::PLAIN)],
        };
        self.line.row = marker;
        self.line.row_width = marker_width;
        self.line.row_base = marker_width;
        let content = content.to_owned();
        for c in content.chars() {
            self.text(c, out);
        }
    }

    /// A character of a line that is not wrapped (code, tables).
    fn raw(&mut self, c: char) {
        let style = match self.line.kind {
            Some(Kind::Code) => Style::CODE,
            _ => Style::PLAIN,
        };
        push_char(&mut self.line.row, c, style);
        self.line.row_width += char_width(c);
    }

    /// A character of a wrapped line.
    fn text(&mut self, c: char, out: &mut Vec<String>) {
        let l = &mut self.line;
        if self.colour {
            if l.inline.star && c != '*' {
                l.inline.star = false;
                self.word_char('*', out);
            }
            let l = &mut self.line;
            match c {
                '`' => {
                    l.inline.code = !l.inline.code;
                    return;
                }
                '*' if !l.inline.code => {
                    if l.inline.star {
                        l.inline.star = false;
                        l.inline.bold = !l.inline.bold;
                    } else {
                        l.inline.star = true;
                    }
                    return;
                }
                _ => {}
            }
        }
        if c == ' ' {
            self.flush_word(out);
            let l = &mut self.line;
            l.space = l.row_width > l.row_base;
            return;
        }
        self.word_char(c, out);
    }

    fn style(&self) -> Style {
        let i = self.line.inline;
        let mut s = self.line.base;
        if i.code {
            s = s.with(Style::CODE);
        }
        if i.bold {
            s = s.bold();
        }
        s
    }

    fn word_char(&mut self, c: char, out: &mut Vec<String>) {
        let style = self.style();
        push_char(&mut self.line.word, c, style);
        self.line.word_width += char_width(c);
        // A word longer than a whole row is broken where the row ends.
        if self.line.word_width + self.line.row_base >= self.capacity() {
            self.flush_word(out);
        }
    }

    /// Places the pending word: on this row, or on the next one.
    fn flush_word(&mut self, out: &mut Vec<String>) {
        if self.line.word.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.line.word);
        let word_width = std::mem::take(&mut self.line.word_width);
        let gap = usize::from(self.line.space && self.line.row_width > self.line.row_base);
        if self.line.row_width + gap + word_width > self.capacity()
            && self.line.row_width > self.line.row_base
        {
            self.wrap(out);
        } else if gap == 1 {
            let base = self.line.base;
            self.line.row.push(Span::new(" ", base));
            self.line.row_width += 1;
        }
        self.line.space = false;
        for span in word {
            for c in span.text.chars() {
                if self.line.row_width + char_width(c) > self.capacity()
                    && self.line.row_width > self.line.row_base
                {
                    self.wrap(out);
                }
                push_char(&mut self.line.row, c, span.style);
                self.line.row_width += char_width(c);
            }
        }
    }

    /// Ends the current row; the next one starts with the line's hang.
    fn wrap(&mut self, out: &mut Vec<String>) {
        let row = std::mem::take(&mut self.line.row);
        self.emit(row, out);
        let hang = self.line.hang.clone();
        self.line.row_width = hang.iter().map(|s| width(&s.text)).sum();
        self.line.row_base = self.line.row_width;
        self.line.row = hang;
        self.line.space = false;
    }

    fn end_line(&mut self, out: &mut Vec<String>) {
        if self.line.kind.is_none() {
            let kind = classify(&self.line.head, self.fence, true).unwrap_or(Kind::Paragraph);
            self.start(kind, out);
        }
        if self.line.inline.star {
            self.line.inline.star = false;
            self.word_char('*', out);
        }
        self.flush_word(out);
        match self.line.kind {
            Some(Kind::Blank) => self.blank = self.rows > 0,
            Some(Kind::FenceOpen(..) | Kind::FenceClose | Kind::Rule) => {}
            Some(Kind::Code) => {
                let row = std::mem::take(&mut self.line.row);
                self.emit(row, out);
            }
            _ => {
                if self.line.row_width > self.line.row_base || self.line.kind == Some(Kind::Table) {
                    let row = std::mem::take(&mut self.line.row);
                    self.emit(row, out);
                } else if self.line.row_base > 0 {
                    // An empty list item keeps its marker.
                    let row = std::mem::take(&mut self.line.row);
                    self.emit(row, out);
                }
            }
        }
        self.line = Line::default();
    }
}

fn push_char(spans: &mut Vec<Span>, c: char, style: Style) {
    match spans.last_mut() {
        Some(last) if last.style == style => last.text.push(c),
        _ => spans.push(Span::new(c.to_string(), style)),
    }
}

/// The kind of a line from its start `head`; `None` while undecided. `done`
/// says the line ended.
fn classify(head: &str, fence: Option<(char, usize)>, done: bool) -> Option<Kind> {
    let lead = head.len() - head.trim_start_matches(' ').len();
    let rest = &head[lead..];
    if let Some((fc, n)) = fence {
        if rest.is_empty() {
            return done.then_some(Kind::Code);
        }
        if lead > 3 {
            return Some(Kind::Code);
        }
        let fences = rest.chars().take_while(|c| *c == fc).count();
        let after = &rest[fences * fc.len_utf8()..];
        let closes = fences >= n && after.trim().is_empty();
        return match (closes, after.trim().is_empty(), done) {
            (true, _, true) => Some(Kind::FenceClose),
            (true, _, false) => None,
            (false, true, false) => None,
            _ => Some(Kind::Code),
        };
    }
    let Some(first) = rest.chars().next() else {
        return done.then_some(Kind::Blank);
    };
    let only = |set: &[char]| rest.chars().all(|c| set.contains(&c));
    Some(match first {
        '`' | '~' => {
            let n = rest.chars().take_while(|c| *c == first).count();
            if only(&[first]) && !done {
                return None;
            }
            if n >= 3 {
                Kind::FenceOpen(first, n)
            } else {
                Kind::Paragraph
            }
        }
        '#' => {
            let n = rest.chars().take_while(|c| *c == '#').count();
            if only(&['#']) && !done {
                return None;
            }
            if n <= 6 && rest[n..].starts_with(' ') {
                Kind::Heading
            } else {
                Kind::Paragraph
            }
        }
        '-' | '*' | '+' | '_' => {
            if only(&[first, ' ']) {
                if !done {
                    return None;
                }
                if first != '+' && rest.chars().filter(|c| *c == first).count() >= 3 {
                    return Some(Kind::Rule);
                }
            }
            if first != '_' && rest.chars().nth(1) == Some(' ') {
                Kind::Bullet
            } else {
                Kind::Paragraph
            }
        }
        '0'..='9' => {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            let after = &rest[digits..];
            let mut tail = after.chars();
            match (tail.next(), tail.next()) {
                _ if digits > 9 => Kind::Paragraph,
                (None, _) | (Some('.' | ')'), None) if !done => return None,
                (Some('.' | ')'), Some(' ')) => Kind::Number,
                _ => Kind::Paragraph,
            }
        }
        '>' => Kind::Quote,
        '|' => Kind::Table,
        _ => Kind::Paragraph,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(colour: bool, columns: usize) -> Markdown {
        Markdown::new(
            columns,
            colour,
            vec![Span::new("declass: ", Style::ACCENT.bold())],
            9,
        )
    }

    fn plain(text: &str, columns: usize) -> Vec<String> {
        block(false, columns).render(text)
    }

    #[test]
    fn paragraphs_wrap_under_the_label() {
        assert_eq!(
            plain(
                "The loop stopped one row early; it now reads to the end.",
                30
            ),
            [
                "declass: The loop stopped one",
                "         row early; it now",
                "         reads to the end.",
            ]
        );
        // A word longer than a row is broken.
        assert_eq!(
            plain("see aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa end", 26),
            [
                "declass: see",
                "         aaaaaaaaaaaaaaaaa",
                "         aaaaaaaaaaaaa end"
            ]
        );
    }

    #[test]
    fn lists_headings_quotes_and_rules_keep_their_shape() {
        let text = "## Plan\n\n- first step that is long enough to wrap\n  - nested\n2. second\n> quoted\n---\nend";
        assert_eq!(
            plain(text, 32),
            [
                "declass: ## Plan",
                "",
                "         • first step that is",
                "           long enough to wrap",
                "           • nested",
                "         2. second",
                "         │ quoted",
                "         ───────────────────────",
                "         end",
            ]
        );
    }

    #[test]
    fn code_blocks_are_not_wrapped_and_keep_their_indentation() {
        let text = "Run:\n```sh\ncargo test --workspace --locked\n    indented\n```\nDone.";
        assert_eq!(
            plain(text, 24),
            [
                "declass: Run:",
                "         │ cargo test --workspace --locked",
                "         │     indented",
                "         Done.",
            ]
        );
    }

    #[test]
    fn colour_styles_inline_code_bold_and_code_blocks() {
        let rows = block(true, 60).render("Use `cargo test` **now**.\n```\nx = 1\n```");
        assert_eq!(
            rows,
            [
                "\x1b[1;36mdeclass: \x1b[0mUse \x1b[33mcargo\x1b[0m \x1b[33mtest\x1b[0m \x1b[1mnow\x1b[0m.",
                "         \x1b[2m│ \x1b[0m\x1b[33mx = 1\x1b[0m",
            ]
        );
        // Without colour the marks stay as written.
        assert_eq!(
            plain("Use `cargo test` **now**.", 60),
            ["declass: Use `cargo test` **now**."]
        );
        let heading = block(true, 60).render("# Title");
        assert_eq!(
            heading,
            ["\x1b[1;36mdeclass: \x1b[0m\x1b[1;36mTitle\x1b[0m"]
        );
    }

    #[test]
    fn the_result_does_not_depend_on_how_the_text_was_cut() {
        let text = "## Findings\n\nThe `export_csv` loop **skips** the last row because it takes `n - 1` items.\n\n- fix the bound\n- add a test named `exports_trailing_row`\n10. numbered\n```rust\nfor row in rows {\n\tsend(row);\n}\n```\n> note\n***\nDone ⟨EMAIL#1⟩ 中文 done.";
        for colour in [false, true] {
            for columns in [24, 40, 80] {
                let whole = block(colour, columns).render(text);
                let chars: Vec<char> = text.chars().collect();
                for cut in [1, 2, 3, 5, 7] {
                    let mut md = block(colour, columns);
                    let mut rows = Vec::new();
                    for piece in chars.chunks(cut) {
                        rows.extend(md.push(&piece.iter().collect::<String>()));
                        // The live row is always narrower than the terminal.
                        if let Some(p) = md.partial() {
                            assert!(super::super::visible_width(&p) < columns, "{p:?}");
                        }
                    }
                    rows.extend(md.finish());
                    assert_eq!(rows, whole, "colour {colour}, {columns} columns, cut {cut}");
                }
                for row in &whole {
                    if !row.contains('│') {
                        assert!(super::super::visible_width(row) <= columns, "{row:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_row_being_written_shows_as_it_grows() {
        let mut md = block(false, 40);
        assert!(md.push("Hello wor").is_empty());
        assert_eq!(md.partial().as_deref(), Some("declass: Hello wor"));
        md.push("ld");
        assert_eq!(md.partial().as_deref(), Some("declass: Hello world"));
        assert_eq!(md.push("\n- it"), ["declass: Hello world"]);
        assert_eq!(md.partial().as_deref(), Some("         • it"));
        // An undecided start shows as typed.
        let mut md = block(false, 40);
        md.push("#");
        assert_eq!(md.partial().as_deref(), Some("declass: #"));
    }

    #[test]
    fn hostile_text_is_made_safe_before_rendering() {
        assert_eq!(
            plain("ok\x1b[2J\x1b]52;c;x\x07 fine", 40),
            ["declass: ok fine"]
        );
    }
}
