// SPDX-License-Identifier: GPL-3.0-or-later
//! The input line of `duet chat`: editing, history, search and completion,
//! independent of the terminal (the console feeds it keys and draws what
//! [`Editor::display`] returns).
//!
//! Keys: arrows, Home/End, Ctrl-A/E (line start/end), Ctrl-B/F, Alt-B/F and
//! Ctrl/Alt-arrows (by word), Backspace/Delete, Ctrl-W and Alt-Backspace
//! (the word before), Alt-D (the word after), Ctrl-K/U (to the line's
//! end/start), Ctrl-Y (put back what was cut), Up/Down and Ctrl-P/N
//! (between lines of the message, then through history), Ctrl-R (search the
//! history; Enter takes the match into the input), Tab (complete commands
//! and `/image` paths), Enter (send; a line ending with `\` continues on
//! the next), Alt-Enter, Shift-Enter (where the terminal reports it) and
//! Ctrl-J (a new line in the message), Ctrl-L (clear the screen), Ctrl-D
//! (on an empty input: the end of input). Ctrl-C and Ctrl-Z are handed to
//! the console.

use super::{Span, Style, char_width, paint};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

/// What a key asks of the console.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Only the input changed (or nothing did).
    Edited,
    /// A message or command to send (continuation lines joined).
    Submit(String),
    /// Ctrl-D on an empty input.
    Eof,
    /// Ctrl-C: the input is cleared; the console applies the interrupt rules.
    Interrupt,
    /// Tab: the console completes (it knows the commands and the directory).
    Complete,
    ClearScreen,
    /// Ctrl-Z.
    Suspend,
}

/// Ctrl-R's state.
struct Search {
    query: String,
    /// The history entry that matches, newest first.
    found: Option<usize>,
    /// The input before the search, restored when it is cancelled.
    before: (String, usize),
}

#[derive(Default)]
pub struct Editor {
    buf: String,
    /// Byte offset, on a grapheme boundary.
    cursor: usize,
    history: Vec<String>,
    /// The history entry shown while browsing, and the input before it.
    browsing: Option<(usize, String)>,
    cut: String,
    search: Option<Search>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    #[cfg(test)]
    pub fn buffer(&self) -> &str {
        &self.buf
    }

    /// Adds entries to the history (oldest first), e.g. the operator's
    /// earlier messages of a resumed session.
    pub fn remember(&mut self, entries: impl IntoIterator<Item = String>) {
        for e in entries {
            let e = e.trim_end().to_owned();
            if !e.trim().is_empty() && self.history.last() != Some(&e) {
                self.history.push(e);
            }
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.browsing = None;
        self.search = None;
    }

    /// Inserts text (typed or pasted); control characters other than
    /// newlines and tabs are dropped, and pasted line ends are normalized.
    pub fn insert(&mut self, text: &str) {
        self.accept_search();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        self.buf.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
        self.browsing = None;
    }

    /// Applies a key.
    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        if k.kind == KeyEventKind::Release {
            return Outcome::Edited;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        if ctrl && matches!(k.code, KeyCode::Char('c' | 'C')) {
            self.clear();
            return Outcome::Interrupt;
        }
        if self.search.is_some() {
            match self.search_key(k, ctrl) {
                Some(outcome) => return outcome,
                // The key ends the search and then does what it does.
                None => self.accept_search(),
            }
        }
        match k.code {
            KeyCode::Char(c) if ctrl => return self.control(c),
            KeyCode::Char(c) if alt => self.meta(c),
            KeyCode::Char(c) => self.insert(&c.to_string()),
            KeyCode::Enter if alt || shift || ctrl => self.insert("\n"),
            KeyCode::Enter => return self.enter(),
            KeyCode::Tab => return Outcome::Complete,
            KeyCode::Backspace if alt || ctrl => self.cut_word_before(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left if alt || ctrl => self.word_left(),
            KeyCode::Right if alt || ctrl => self.word_right(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Up => self.up(),
            KeyCode::Down => self.down(),
            _ => {}
        }
        Outcome::Edited
    }

    fn control(&mut self, c: char) -> Outcome {
        match c.to_ascii_lowercase() {
            'a' => self.cursor = self.line_start(),
            'e' => self.cursor = self.line_end(),
            'b' => self.left(),
            'f' => self.right(),
            'p' => self.up(),
            'n' => self.down(),
            'h' => self.backspace(),
            'd' if self.buf.is_empty() => return Outcome::Eof,
            'd' => self.delete(),
            'k' => {
                let end = self.line_end();
                let end = if end == self.cursor && end < self.buf.len() {
                    end + 1
                } else {
                    end
                };
                self.cut_range(self.cursor, end);
            }
            'u' => self.cut_range(self.line_start(), self.cursor),
            'w' => {
                let before = &self.buf[..self.cursor];
                let trimmed = before.trim_end_matches(|c: char| c.is_whitespace());
                let start = trimmed.rfind(char::is_whitespace).map_or(0, |i| {
                    i + trimmed[i..].chars().next().map_or(1, char::len_utf8)
                });
                self.cut_range(start, self.cursor);
            }
            'y' => {
                let cut = self.cut.clone();
                self.insert(&cut);
            }
            'r' => {
                self.search = Some(Search {
                    query: String::new(),
                    found: None,
                    before: (self.buf.clone(), self.cursor),
                });
            }
            'j' => self.insert("\n"),
            'l' => return Outcome::ClearScreen,
            'z' => return Outcome::Suspend,
            _ => {}
        }
        Outcome::Edited
    }

    fn meta(&mut self, c: char) {
        match c {
            'b' => self.word_left(),
            'f' => self.word_right(),
            'd' => {
                let from = self.cursor;
                self.word_right();
                let to = self.cursor;
                self.cursor = from;
                self.cut_range(from, to);
            }
            _ => {}
        }
    }

    /// Enter sends, unless the line ends with `\` (then the message goes
    /// on on the next line, as without a terminal).
    fn enter(&mut self) -> Outcome {
        let end = self.cursor == self.buf.len();
        if end && self.buf.ends_with('\\') {
            self.insert("\n");
            return Outcome::Edited;
        }
        let text = self.buf.replace("\\\n", "\n");
        self.remember([self.buf.clone()]);
        self.clear();
        Outcome::Submit(text)
    }

    fn cut_range(&mut self, from: usize, to: usize) {
        if from >= to {
            return;
        }
        self.cut = self.buf[from..to].to_owned();
        self.buf.replace_range(from..to, "");
        self.cursor = from;
        self.browsing = None;
    }

    fn cut_word_before(&mut self) {
        let to = self.cursor;
        self.word_left();
        let from = self.cursor;
        self.cut_range(from, to);
    }

    fn backspace(&mut self) {
        let to = self.cursor;
        self.left();
        if self.cursor < to {
            self.buf.replace_range(self.cursor..to, "");
            self.browsing = None;
        }
    }

    fn delete(&mut self) {
        let from = self.cursor;
        self.right();
        if self.cursor > from {
            self.buf.replace_range(from..self.cursor, "");
            self.cursor = from;
            self.browsing = None;
        }
    }

    fn left(&mut self) {
        self.cursor = self.buf[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i);
    }

    fn right(&mut self) {
        if let Some(g) = self.buf[self.cursor..].graphemes(true).next() {
            self.cursor += g.len();
        }
    }

    fn word_left(&mut self) {
        let before: Vec<(usize, char)> = self.buf[..self.cursor].char_indices().collect();
        let mut i = before.len();
        while i > 0 && !is_word(before[i - 1].1) {
            i -= 1;
        }
        while i > 0 && is_word(before[i - 1].1) {
            i -= 1;
        }
        self.cursor = before.get(i).map_or(0, |(at, _)| *at);
    }

    fn word_right(&mut self) {
        let mut chars = self.buf[self.cursor..].char_indices().peekable();
        let mut end = self.buf.len() - self.cursor;
        while let Some(&(_, c)) = chars.peek() {
            if is_word(c) {
                break;
            }
            chars.next();
        }
        for (i, c) in chars {
            if !is_word(c) {
                end = i;
                break;
            }
        }
        self.cursor += end;
    }

    fn line_start(&self) -> usize {
        self.buf[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.buf[self.cursor..]
            .find('\n')
            .map_or(self.buf.len(), |i| self.cursor + i)
    }

    /// Column (in characters' display width) of the cursor in its line.
    fn column(&self) -> usize {
        display_width(&self.buf[self.line_start()..self.cursor])
    }

    /// The offset in the line starting at `start` closest to `column`.
    fn at_column(&self, start: usize, column: usize) -> usize {
        let end = self.buf[start..]
            .find('\n')
            .map_or(self.buf.len(), |i| start + i);
        let mut used = 0;
        for (i, g) in self.buf[start..end].grapheme_indices(true) {
            let w = display_width(g);
            if used + w > column {
                return start + i;
            }
            used += w;
        }
        end
    }

    fn up(&mut self) {
        let start = self.line_start();
        if start > 0 {
            let column = self.column();
            let prev = self.buf[..start - 1].rfind('\n').map_or(0, |i| i + 1);
            self.cursor = self.at_column(prev, column);
            return;
        }
        let next = match &self.browsing {
            Some((0, _)) => return,
            Some((i, _)) => i - 1,
            None if self.history.is_empty() => return,
            None => self.history.len() - 1,
        };
        let draft = match self.browsing.take() {
            Some((_, draft)) => draft,
            None => self.buf.clone(),
        };
        self.buf = self.history[next].clone();
        self.cursor = self.buf.len();
        self.browsing = Some((next, draft));
    }

    fn down(&mut self) {
        let end = self.line_end();
        if end < self.buf.len() {
            let column = self.column();
            self.cursor = self.at_column(end + 1, column);
            return;
        }
        let Some((i, draft)) = self.browsing.take() else {
            return;
        };
        if i + 1 < self.history.len() {
            self.buf = self.history[i + 1].clone();
            self.browsing = Some((i + 1, draft));
        } else {
            self.buf = draft;
        }
        self.cursor = self.buf.len();
    }

    /// A key while searching; `None` when the key ends the search and is
    /// then applied as usual.
    fn search_key(&mut self, k: KeyEvent, ctrl: bool) -> Option<Outcome> {
        let s = self.search.as_mut()?;
        match k.code {
            KeyCode::Char('r' | 'R') if ctrl => {
                let from = s.found.unwrap_or(self.history.len());
                s.found = find(&self.history, &s.query, from).or(s.found);
            }
            KeyCode::Char('g' | 'G') if ctrl => self.cancel_search(),
            KeyCode::Esc => self.cancel_search(),
            KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => {
                s.query.push(c);
                s.found = find(&self.history, &s.query, self.history.len());
            }
            KeyCode::Backspace => {
                s.query.pop();
                s.found = find(&self.history, &s.query, self.history.len());
            }
            KeyCode::Enter => self.accept_search(),
            _ => return None,
        }
        Some(Outcome::Edited)
    }

    fn cancel_search(&mut self) {
        if let Some(s) = self.search.take() {
            (self.buf, self.cursor) = s.before;
        }
    }

    fn accept_search(&mut self) {
        if let Some(s) = self.search.take()
            && let Some(i) = s.found
        {
            self.buf = self.history[i].clone();
            self.cursor = self.buf.len();
        }
    }

    /// Tab: completes the word before the cursor from `candidates` (given
    /// what precedes the cursor, the candidates and the length of the word
    /// they replace). Returns the choices to show when the word cannot be
    /// completed further.
    pub fn complete(&mut self, cwd: &Path, commands: &[&str]) -> Vec<String> {
        let Some((word_len, choices, suffix)) =
            completions(&self.buf[..self.cursor], cwd, commands)
        else {
            return Vec::new();
        };
        let word = self.buf[self.cursor - word_len..self.cursor].to_owned();
        if choices.len() == 1 {
            let add = format!(
                "{}{suffix}",
                &choices[0][word.len().min(choices[0].len())..]
            );
            let add = if choices[0].ends_with('/') {
                add.trim_end().to_owned()
            } else {
                add
            };
            self.insert(&add);
            return Vec::new();
        }
        let common = common_prefix(&choices);
        if common.len() > word.len() {
            self.insert(&common[word.len()..]);
            return Vec::new();
        }
        choices
            .iter()
            .map(|c| {
                c.rsplit_once('/')
                    .filter(|(dir, name)| !dir.is_empty() && !name.is_empty())
                    .map_or(c.as_str(), |(_, name)| name)
                    .to_owned()
            })
            .collect()
    }

    /// The input as it is drawn: `prompt` before the first row, later rows
    /// indented as far, wrapped at `columns`, at most `max_rows` rows (a
    /// window around the cursor). Returns the rows and the cursor's row and
    /// column.
    pub fn display(
        &self,
        prompt: &[Span],
        colour: bool,
        columns: usize,
        max_rows: usize,
    ) -> (Vec<String>, (usize, usize)) {
        let (prompt, text, cursor): (Vec<Span>, &str, usize) = match &self.search {
            Some(s) => {
                let label = if s.found.is_none() && !s.query.is_empty() {
                    "search (no match): "
                } else {
                    "search: "
                };
                (
                    vec![Span::new(label, Style::ACCENT)],
                    s.query.as_str(),
                    s.query.len(),
                )
            }
            None => (prompt.to_vec(), self.buf.as_str(), self.cursor),
        };
        let indent: usize = prompt.iter().map(|s| display_width(&s.text)).sum();
        let room = columns.saturating_sub(indent + 1).max(1);
        let mut rows: Vec<String> = vec![String::new()];
        let mut col = 0;
        let mut at = (0, 0);
        let mut placed = false;
        for (i, g) in text.grapheme_indices(true) {
            if i == cursor {
                at = (rows.len() - 1, col);
                placed = true;
            }
            if g == "\n" || g == "\r\n" {
                rows.push(String::new());
                col = 0;
                continue;
            }
            let (shown, w) = if g == "\t" {
                ("    ", 4)
            } else {
                (g, display_width(g))
            };
            if col + w > room {
                rows.push(String::new());
                col = 0;
            }
            rows.last_mut().expect("a row").push_str(shown);
            col += w;
        }
        // One column is kept free after a full row for the cursor.
        if !placed {
            at = (rows.len() - 1, col);
        }
        // With a search, the match follows the query on the first row.
        let mut painted: Vec<String> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if i == 0 {
                    format!("{}{r}", paint(&prompt, colour))
                } else {
                    format!("{}{r}", " ".repeat(indent))
                }
            })
            .collect();
        if let Some(s) = &self.search
            && let Some(found) = s.found
        {
            let first = self.history[found].lines().next().unwrap_or("");
            let shown = super::clip_width(
                &format!("  {first}"),
                room.saturating_sub(display_width(&s.query)).max(1),
            );
            painted[0].push_str(&paint(&[Span::new(shown, Style::DIM)], colour));
        }
        let max_rows = max_rows.max(1);
        let start = (at.0 + 1).saturating_sub(max_rows);
        let end = (start + max_rows).min(painted.len());
        let window = painted[start..end].to_vec();
        (window, (at.0 - start, indent + at.1))
    }
}

/// The history entry before `before` (newest first) that contains `query`
/// (ignoring case).
fn find(history: &[String], query: &str, before: usize) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    let q = query.to_lowercase();
    (0..before.min(history.len()))
        .rev()
        .find(|&i| history[i].to_lowercase().contains(&q))
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| if c == '\t' { 4 } else { char_width(c) })
        .sum()
}

fn common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut end = first.len();
    for item in &items[1..] {
        end = first
            .char_indices()
            .zip(item.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8())
            .min(end);
    }
    first[..end].to_owned()
}

/// Completions of the word before the cursor: its length, the candidates
/// (each the whole word) and what follows a unique one.
fn completions(
    before: &str,
    cwd: &Path,
    commands: &[&str],
) -> Option<(usize, Vec<String>, &'static str)> {
    if before.contains('\n') {
        return None;
    }
    if before.starts_with('/') && !before.contains(char::is_whitespace) {
        let choices: Vec<String> = commands
            .iter()
            .filter(|c| c.starts_with(before))
            .map(|c| (*c).to_owned())
            .collect();
        return (!choices.is_empty()).then_some((before.len(), choices, " "));
    }
    let arg = before
        .strip_prefix("/image")
        .filter(|r| r.starts_with(char::is_whitespace))?
        .trim_start();
    if arg.starts_with('-') && "--public".starts_with(arg) {
        return Some((arg.len(), vec!["--public".to_owned()], " "));
    }
    let path = match arg.strip_prefix("--public") {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => arg,
    };
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let listed = if dir.is_empty() {
        cwd.to_path_buf()
    } else if Path::new(dir).is_absolute() {
        Path::new(dir).to_path_buf()
    } else {
        cwd.join(dir)
    };
    let mut choices: Vec<String> = std::fs::read_dir(listed)
        .ok()?
        .flatten()
        .take(2000)
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let shown = n.starts_with(name) && (name.starts_with('.') || !n.starts_with('.'));
            shown.then(|| {
                let slash = if e.path().is_dir() { "/" } else { "" };
                format!("{dir}{n}{slash}")
            })
        })
        .collect();
    choices.sort();
    (!choices.is_empty()).then_some((path.len(), choices, ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn typed(e: &mut Editor, text: &str) {
        for c in text.chars() {
            e.key(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;

    #[test]
    fn editing_moves_by_character_word_and_line() {
        let mut e = Editor::default();
        typed(&mut e, "fix the csv export");
        e.key(key(KeyCode::Char('a'), CTRL));
        e.key(key(KeyCode::Char('f'), ALT));
        e.key(key(KeyCode::Char('f'), ALT));
        e.key(key(KeyCode::Char('w'), CTRL));
        assert_eq!(e.buffer(), "fix  csv export");
        e.key(key(KeyCode::Char('y'), CTRL));
        assert_eq!(e.buffer(), "fix the csv export");
        e.key(key(KeyCode::Char('e'), CTRL));
        e.key(key(KeyCode::Backspace, ALT));
        typed(&mut e, "importé");
        e.key(key(KeyCode::Left, NONE));
        e.key(key(KeyCode::Backspace, NONE));
        assert_eq!(e.buffer(), "fix the csv imporé");
        e.key(key(KeyCode::Home, NONE));
        e.key(key(KeyCode::Char('d'), ALT));
        e.key(key(KeyCode::Delete, NONE));
        assert_eq!(e.buffer(), "the csv imporé");
        e.key(key(KeyCode::Right, CTRL));
        e.key(key(KeyCode::Char('k'), CTRL));
        assert_eq!(e.buffer(), "the");
        e.key(key(KeyCode::Char('u'), CTRL));
        assert_eq!(e.buffer(), "");
        assert_eq!(e.key(key(KeyCode::Char('d'), CTRL)), Outcome::Eof);
    }

    #[test]
    fn enter_sends_and_multi_line_keys_add_lines() {
        let mut e = Editor::default();
        typed(&mut e, "one\\");
        assert_eq!(e.key(key(KeyCode::Enter, NONE)), Outcome::Edited);
        typed(&mut e, "two");
        e.key(key(KeyCode::Enter, ALT));
        typed(&mut e, "three");
        e.key(key(KeyCode::Char('j'), CTRL));
        typed(&mut e, "four");
        e.key(key(KeyCode::Enter, KeyModifiers::SHIFT));
        typed(&mut e, "five");
        assert_eq!(
            e.key(key(KeyCode::Enter, NONE)),
            Outcome::Submit("one\ntwo\nthree\nfour\nfive".into())
        );
        assert_eq!(e.buffer(), "");
        // Up and Down move between the lines of a message before history.
        e.insert("ab\ncdef");
        e.key(key(KeyCode::Up, NONE));
        typed(&mut e, "X");
        assert_eq!(e.buffer(), "abX\ncdef");
        e.key(key(KeyCode::Down, NONE));
        typed(&mut e, "Y");
        assert_eq!(e.buffer(), "abX\ncdeYf");
    }

    #[test]
    fn history_is_browsed_and_searched() {
        let mut e = Editor::default();
        e.remember(["Add a function b.".to_owned(), "u32 please".to_owned()]);
        typed(&mut e, "draft");
        e.key(key(KeyCode::Up, NONE));
        assert_eq!(e.buffer(), "u32 please");
        e.key(key(KeyCode::Up, NONE));
        e.key(key(KeyCode::Up, NONE));
        assert_eq!(e.buffer(), "Add a function b.");
        e.key(key(KeyCode::Down, NONE));
        e.key(key(KeyCode::Down, NONE));
        assert_eq!(e.buffer(), "draft");
        e.clear();
        e.key(key(KeyCode::Char('r'), CTRL));
        typed(&mut e, "FUNC");
        let (rows, _) = e.display(&[Span::new("you> ", Style::PLAIN)], false, 80, 5);
        assert_eq!(rows, ["search: FUNC  Add a function b."]);
        e.key(key(KeyCode::Enter, NONE));
        assert_eq!(e.buffer(), "Add a function b.");
        // Esc restores what was there.
        e.key(key(KeyCode::Char('r'), CTRL));
        typed(&mut e, "u32");
        e.key(key(KeyCode::Esc, NONE));
        assert_eq!(e.buffer(), "Add a function b.");
        // Ctrl-C always reaches the console, even while searching.
        e.key(key(KeyCode::Char('r'), CTRL));
        assert_eq!(e.key(key(KeyCode::Char('c'), CTRL)), Outcome::Interrupt);
        assert_eq!(e.buffer(), "");
        // Sent messages join the history.
        typed(&mut e, "again");
        e.key(key(KeyCode::Enter, NONE));
        e.key(key(KeyCode::Up, NONE));
        assert_eq!(e.buffer(), "again");
    }

    #[test]
    fn tab_completes_commands_and_image_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("shots")).unwrap();
        std::fs::write(dir.path().join("shots/login page.png"), b"x").unwrap();
        std::fs::write(dir.path().join("shots/logout.png"), b"x").unwrap();
        std::fs::write(dir.path().join(".hidden"), b"x").unwrap();
        let commands = ["/diff", "/image", "/status", "/stop"];
        let mut e = Editor::default();
        typed(&mut e, "/di");
        assert!(e.complete(dir.path(), &commands).is_empty());
        assert_eq!(e.buffer(), "/diff ");
        e.clear();
        typed(&mut e, "/st");
        assert_eq!(e.complete(dir.path(), &commands), ["/status", "/stop"]);
        e.clear();
        typed(&mut e, "/image --p");
        e.complete(dir.path(), &commands);
        assert_eq!(e.buffer(), "/image --public ");
        typed(&mut e, "sh");
        e.complete(dir.path(), &commands);
        assert_eq!(e.buffer(), "/image --public shots/");
        e.complete(dir.path(), &commands);
        assert_eq!(e.buffer(), "/image --public shots/log");
        assert_eq!(
            e.complete(dir.path(), &commands),
            ["login page.png", "logout.png"]
        );
        typed(&mut e, "i");
        e.complete(dir.path(), &commands);
        assert_eq!(e.buffer(), "/image --public shots/login page.png");
        // Nothing else is completed.
        e.clear();
        typed(&mut e, "fix sh");
        assert!(e.complete(dir.path(), &commands).is_empty());
        assert_eq!(e.buffer(), "fix sh");
    }

    #[test]
    fn the_input_wraps_under_the_prompt_and_keeps_the_cursor_in_view() {
        let mut e = Editor::default();
        e.insert("abcdefghij\nx\tz");
        let prompt = [Span::new("you> ", Style::PLAIN)];
        let (rows, cursor) = e.display(&prompt, false, 12, 10);
        assert_eq!(rows, ["you> abcdef", "     ghij", "     x    z"]);
        assert_eq!(cursor, (2, 11));
        e.key(key(KeyCode::Up, NONE));
        e.key(key(KeyCode::Home, NONE));
        let (rows, cursor) = e.display(&prompt, false, 12, 2);
        assert_eq!(rows, ["you> abcdef", "     ghij"]);
        assert_eq!(cursor, (0, 5));
        // Wide characters are not split across rows.
        let mut e = Editor::default();
        e.insert("中文中文");
        let (rows, _) = e.display(&prompt, false, 12, 10);
        assert_eq!(rows, ["you> 中文中", "     文"]);
        // Pasted control characters are dropped.
        let mut e = Editor::default();
        e.insert("a\x1b[31mb\r\nc");
        assert_eq!(e.buffer(), "a[31mb\nc");
    }
}
