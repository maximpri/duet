// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace's input: editing, history, search and completion,
//! independent of the terminal (the workspace feeds it keys and draws what
//! [`Editor::display`] returns).
//!
//! Keys: Shift-navigation selects; Ctrl-A selects all; Ctrl-C copies a
//! selection (otherwise interrupts), Ctrl-X cuts, Ctrl-V/Shift-Insert pastes.
//! Ctrl-Z undoes, Ctrl-Shift-Z/Ctrl-Y redoes; Ctrl-Alt-Z suspends. Home/End
//! move within a line, Ctrl-Home/End through the message, Ctrl-B/F move by
//! grapheme, and Ctrl/Alt-arrows or Alt-B/F move by word. Ctrl-W,
//! Alt-Backspace/D and Ctrl-K/U cut words/lines; Alt-Y restores the last cut.
//! Up/Down and Ctrl-P/N move through lines, then history (selection never
//! enters history). Ctrl-R searches history. Tab completes commands/paths.
//! Enter sends; a final `\` continues. Alt/Shift/Ctrl-Enter and Ctrl-J add
//! a newline. Pasting never sends. Ctrl-L clears the screen; Ctrl-D on an
//! empty input signals EOF.

use super::{Span, Style, paint};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::collections::VecDeque;
use std::ops::Range;
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

/// Maximum editable message size; oversized pastes stop at a grapheme boundary.
pub const MAX_INPUT_BYTES: usize = 256 * 1024;
const MAX_UNDO_ENTRIES: usize = 100;
const MAX_UNDO_BYTES: usize = 4 * 1024 * 1024;

/// What a key asks of the workspace.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Edited,
    /// A message or command to send (continuation lines joined).
    Submit(String),
    /// Ctrl-D on an empty input.
    Eof,
    /// Ctrl-C without a selection: cleared input and an interrupt request.
    Interrupt,
    /// Copy selected text to the clipboard (Ctrl-X already removed it).
    Copy(String),
    /// Request clipboard text; the host feeds the result to `insert`.
    Paste,
    /// Tab: the workspace knows the commands and the directory.
    Complete,
    ClearScreen,
    /// Ctrl-Alt-Z; Ctrl-Z belongs to editing undo.
    Suspend,
}

#[derive(Clone)]
struct Snapshot {
    buf: String,
    cursor: usize,
    anchor: Option<usize>,
}

#[derive(Default)]
struct UndoStack {
    entries: VecDeque<Snapshot>,
    bytes: usize,
}

impl UndoStack {
    fn push(&mut self, snapshot: Snapshot) {
        self.bytes += snapshot.buf.len();
        self.entries.push_back(snapshot);
        while self.entries.len() > MAX_UNDO_ENTRIES || self.bytes > MAX_UNDO_BYTES {
            if let Some(old) = self.entries.pop_front() {
                self.bytes -= old.buf.len();
            }
        }
    }

    fn pop(&mut self) -> Option<Snapshot> {
        let snapshot = self.entries.pop_back()?;
        self.bytes -= snapshot.buf.len();
        Some(snapshot)
    }
}

#[derive(Clone, Copy)]
enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    First,
    Last,
    Up,
    Down,
}

/// Ctrl-R's state.
struct Search {
    query: String,
    /// The history entry that matches, newest first.
    found: Option<usize>,
    /// The input before the search, restored when it is cancelled.
    before: Snapshot,
}

#[derive(Default)]
pub struct Editor {
    buf: String,
    /// Byte offset, on a grapheme boundary.
    cursor: usize,
    anchor: Option<usize>,
    undo: UndoStack,
    redo: UndoStack,
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
    pub fn buffer(&self) -> &str {
        &self.buf
    }

    /// Byte offset at an extended grapheme boundary.
    pub fn cursor(&self) -> usize {
        self.cursor
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

    /// Selected bytes, always at extended grapheme boundaries.
    pub fn selection_range(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection_range().map(|range| &self.buf[range])
    }

    pub fn select_all(&mut self) {
        self.accept_search();
        self.anchor = Some(0);
        self.cursor = self.buf.len();
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.anchor = None;
        self.browsing = None;
        self.search = None;
        // Submitted/interrupted messages do not reappear through undo.
        self.undo = UndoStack::default();
        self.redo = UndoStack::default();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            buf: self.buf.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.buf = snapshot.buf;
        self.cursor = snapshot.cursor;
        self.anchor = snapshot.anchor;
        self.search = None;
        self.browsing = None;
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.snapshot());
        self.redo = UndoStack::default();
    }

    fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(previous);
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(next);
        }
    }

    /// Insert typed/pasted text as one undoable edit, replacing any selection.
    /// Control characters except newlines/tabs are dropped, CRLF/CR becomes LF.
    /// Input is capped at MAX_INPUT_BYTES; a paste never submits the message.
    pub fn insert(&mut self, text: &str) {
        self.accept_search();
        let range = self.selection_range().unwrap_or(self.cursor..self.cursor);
        let available = MAX_INPUT_BYTES.saturating_sub(self.buf.len() - range.len());
        let clean = clean_text(text, available);
        if !clean.is_empty() {
            self.replace(range, &clean);
        }
    }

    fn replace(&mut self, range: Range<usize>, text: &str) {
        if self.buf[range.clone()] == *text {
            self.cursor = range.start + text.len();
            self.anchor = None;
            return;
        }
        self.checkpoint();
        self.buf.replace_range(range.clone(), text);
        // Insertion/deletion may join adjacent combining marks or emoji into
        // a new grapheme. Never leave a cursor in the middle of that cluster.
        let desired = range.start + text.len();
        self.cursor = self
            .buf
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|&i| i >= desired)
            .unwrap_or(self.buf.len());
        self.anchor = None;
        self.browsing = None;
    }

    /// Applies a key. Clipboard actions are returned to the host, not executed.
    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        if k.kind == KeyEventKind::Release {
            return Outcome::Edited;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        if ctrl && matches!(k.code, KeyCode::Char('c' | 'C')) {
            if let Some(text) = self.selected_text() {
                return Outcome::Copy(text.to_owned());
            }
            self.clear();
            return Outcome::Interrupt;
        }
        if self.search.is_some() {
            match self.search_key(k, ctrl) {
                Some(outcome) => return outcome,
                None => self.accept_search(),
            }
        }
        if ctrl && alt && matches!(k.code, KeyCode::Char('z' | 'Z')) {
            return Outcome::Suspend;
        }
        let motion = match k.code {
            KeyCode::Left if alt || ctrl => Some(Motion::WordLeft),
            KeyCode::Right if alt || ctrl => Some(Motion::WordRight),
            KeyCode::Left => Some(Motion::Left),
            KeyCode::Right => Some(Motion::Right),
            KeyCode::Home if ctrl => Some(Motion::First),
            KeyCode::End if ctrl => Some(Motion::Last),
            KeyCode::Home => Some(Motion::Home),
            KeyCode::End => Some(Motion::End),
            KeyCode::Up => Some(Motion::Up),
            KeyCode::Down => Some(Motion::Down),
            _ => None,
        };
        if let Some(motion) = motion {
            self.navigate(motion, shift);
            return Outcome::Edited;
        }
        match k.code {
            KeyCode::Char(c) if ctrl => return self.control(c, shift),
            KeyCode::Char(c) if alt => self.meta(c, shift),
            KeyCode::Char(c) => self.insert(&c.to_string()),
            KeyCode::Enter if alt || shift || ctrl => self.insert("\n"),
            KeyCode::Enter => return self.enter(),
            KeyCode::Insert if shift => return Outcome::Paste,
            KeyCode::Insert if ctrl => {
                if let Some(text) = self.selected_text() {
                    return Outcome::Copy(text.to_owned());
                }
            }
            KeyCode::Delete if shift => return self.copy_cut(),
            KeyCode::Tab => return Outcome::Complete,
            KeyCode::Backspace if alt || ctrl => self.cut_word_before(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete if alt || ctrl => self.cut_word_after(),
            KeyCode::Delete => self.delete(),
            KeyCode::Esc => self.anchor = None,
            _ => {}
        }
        Outcome::Edited
    }

    fn copy_cut(&mut self) -> Outcome {
        let Some(range) = self.selection_range() else {
            return Outcome::Edited;
        };
        let copied = self.buf[range.clone()].to_owned();
        self.cut_range(range.start, range.end);
        Outcome::Copy(copied)
    }

    fn control(&mut self, c: char, shift: bool) -> Outcome {
        match c.to_ascii_lowercase() {
            'a' => self.select_all(),
            'e' => self.navigate(Motion::End, shift),
            'b' => self.navigate(Motion::Left, shift),
            'f' => self.navigate(Motion::Right, shift),
            'p' => self.navigate(Motion::Up, shift),
            'n' => self.navigate(Motion::Down, shift),
            'x' => return self.copy_cut(),
            'v' => return Outcome::Paste,
            'z' if shift => self.redo(),
            'z' => self.undo(),
            'y' => self.redo(),
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
                let mut before = self.buf[..self.cursor]
                    .grapheme_indices(true)
                    .rev()
                    .peekable();
                let mut start = self.cursor;
                while let Some(&(i, g)) = before.peek() {
                    if !g.chars().all(char::is_whitespace) {
                        break;
                    }
                    start = i;
                    before.next();
                }
                for (i, g) in before {
                    if g.chars().all(char::is_whitespace) {
                        break;
                    }
                    start = i;
                }
                self.cut_range(start, self.cursor);
            }
            'r' => {
                let before = self.snapshot();
                self.anchor = None;
                self.search = Some(Search {
                    query: String::new(),
                    found: None,
                    before,
                });
            }
            'j' => self.insert("\n"),
            'l' => return Outcome::ClearScreen,
            _ => {}
        }
        Outcome::Edited
    }

    fn meta(&mut self, c: char, shift: bool) {
        match c.to_ascii_lowercase() {
            'b' => self.navigate(Motion::WordLeft, shift),
            'f' => self.navigate(Motion::WordRight, shift),
            'd' => self.cut_word_after(),
            'y' => {
                let cut = self.cut.clone();
                self.insert(&cut);
            }
            _ => {}
        }
    }

    fn navigate(&mut self, motion: Motion, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            if let Some(range) = self.selection_range() {
                match motion {
                    Motion::Left => {
                        self.cursor = range.start;
                        self.anchor = None;
                        return;
                    }
                    Motion::Right => {
                        self.cursor = range.end;
                        self.anchor = None;
                        return;
                    }
                    _ => {}
                }
            }
            self.anchor = None;
        }
        match motion {
            Motion::Left => self.left(),
            Motion::Right => self.right(),
            Motion::WordLeft => self.word_left(),
            Motion::WordRight => self.word_right(),
            Motion::Home => self.cursor = self.line_start(),
            Motion::End => self.cursor = self.line_end(),
            Motion::First => self.cursor = 0,
            Motion::Last => self.cursor = self.buf.len(),
            Motion::Up if extend && self.line_start() == 0 => self.cursor = 0,
            Motion::Down if extend && self.line_end() == self.buf.len() => {
                self.cursor = self.buf.len()
            }
            Motion::Up => self.up(),
            Motion::Down => self.down(),
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
        let range = self.selection_range().unwrap_or(from..to);
        if range.is_empty() {
            return;
        }
        self.cut = self.buf[range.clone()].to_owned();
        self.replace(range, "");
    }

    fn cut_word_before(&mut self) {
        let to = self.cursor;
        self.word_left();
        let from = self.cursor;
        self.cursor = to;
        self.cut_range(from, to);
    }

    fn cut_word_after(&mut self) {
        let from = self.cursor;
        self.word_right();
        let to = self.cursor;
        self.cursor = from;
        self.cut_range(from, to);
    }

    fn backspace(&mut self) {
        if let Some(range) = self.selection_range() {
            self.replace(range, "");
            return;
        }
        let to = self.cursor;
        self.left();
        let from = self.cursor;
        self.cursor = to;
        if from < to {
            self.replace(from..to, "");
        }
    }

    fn delete(&mut self) {
        if let Some(range) = self.selection_range() {
            self.replace(range, "");
            return;
        }
        let from = self.cursor;
        self.right();
        let to = self.cursor;
        self.cursor = from;
        if from < to {
            self.replace(from..to, "");
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
        let mut before = self.buf[..self.cursor]
            .grapheme_indices(true)
            .rev()
            .peekable();
        while let Some(&(i, g)) = before.peek() {
            if g.chars().any(is_word) {
                break;
            }
            self.cursor = i;
            before.next();
        }
        for (i, g) in before {
            if !g.chars().any(is_word) {
                break;
            }
            self.cursor = i;
        }
    }

    fn word_right(&mut self) {
        let mut after = self.buf[self.cursor..].grapheme_indices(true).peekable();
        let start = self.cursor;
        while let Some(&(i, g)) = after.peek() {
            if g.chars().any(is_word) {
                break;
            }
            self.cursor = start + i + g.len();
            after.next();
        }
        for (i, g) in after {
            if !g.chars().any(is_word) {
                break;
            }
            self.cursor = start + i + g.len();
        }
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
        self.checkpoint();
        self.buf = clean_text(&self.history[next], MAX_INPUT_BYTES);
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
        self.checkpoint();
        if i + 1 < self.history.len() {
            self.buf = clean_text(&self.history[i + 1], MAX_INPUT_BYTES);
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
                if s.query.len() + c.len_utf8() <= MAX_INPUT_BYTES {
                    s.query.push(c);
                }
                s.found = find(&self.history, &s.query, self.history.len());
            }
            KeyCode::Backspace => {
                if let Some((at, _)) = s.query.grapheme_indices(true).next_back() {
                    s.query.truncate(at);
                }
                s.found = find(&self.history, &s.query, self.history.len());
            }
            KeyCode::Enter => self.accept_search(),
            _ => return None,
        }
        Some(Outcome::Edited)
    }

    fn cancel_search(&mut self) {
        if let Some(s) = self.search.take() {
            self.restore(s.before);
        }
    }

    fn accept_search(&mut self) {
        if let Some(s) = self.search.take()
            && let Some(i) = s.found
        {
            self.checkpoint();
            self.buf = clean_text(&self.history[i], MAX_INPUT_BYTES);
            self.cursor = self.buf.len();
            self.anchor = None;
        }
    }

    /// Tab: completes the word before the cursor from `candidates` (given
    /// what precedes the cursor, the candidates and the length of the word
    /// they replace). Returns the choices to show when the word cannot be
    /// completed further.
    pub fn complete(&mut self, cwd: &Path, commands: &[&str]) -> Vec<String> {
        self.anchor = None;
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

    /// Move/extend selection at a displayed cell, using exactly `display`'s
    /// wrapping and cursor-following viewport. Row/column are zero-based in
    /// the input area, including the prompt. Click with `extend=false`, then
    /// drag with `extend=true`. A cell inside a wide grapheme maps to its
    /// nearest edge. Returns false during history search or if unchanged.
    pub fn select_at(
        &mut self,
        prompt: &[Span],
        columns: usize,
        max_rows: usize,
        row: usize,
        column: usize,
        extend: bool,
    ) -> bool {
        if self.search.is_some() {
            return false;
        }
        let indent: usize = prompt.iter().map(|s| display_width(&s.text)).sum();
        let room = columns.saturating_sub(indent + 1).max(1);
        let (rows, cursor) = layout(&self.buf, self.cursor, room);
        let window = visible_rows(rows.len(), cursor.0, max_rows);
        let row = &rows[(window.start + row).min(window.end - 1)];
        let column = column.saturating_sub(indent);
        let mut offset = row.end;
        let mut used = 0;
        for (i, g) in self.buf[row.clone()].grapheme_indices(true) {
            let width = display_width(g);
            if column < used + width {
                offset = row.start
                    + i
                    + if (column - used) * 2 >= width {
                        g.len()
                    } else {
                        0
                    };
                break;
            }
            used += width;
        }
        let before = (self.cursor, self.anchor);
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = offset;
        before != (self.cursor, self.anchor)
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
        let (rows, at) = layout(text, cursor, room);
        let selection = if self.search.is_none() {
            self.selection_range()
        } else {
            None
        };
        let mut painted: Vec<String> = rows
            .iter()
            .enumerate()
            .map(|(i, range)| {
                let mut row = if i == 0 {
                    paint(&prompt, colour)
                } else {
                    " ".repeat(indent)
                };
                for (offset, g) in text[range.clone()].grapheme_indices(true) {
                    let selected = selection
                        .as_ref()
                        .is_some_and(|s| s.contains(&(range.start + offset)));
                    let shown = if g == "\t" { "    " } else { g };
                    // Reverse video is a selection affordance, including NO_COLOR.
                    if selected {
                        row.push_str("\x1b[7m");
                    }
                    row.push_str(shown);
                    if selected {
                        row.push_str("\x1b[27m");
                    }
                }
                if text.as_bytes().get(range.end) == Some(&b'\n')
                    && selection.as_ref().is_some_and(|s| s.contains(&range.end))
                {
                    row.push_str("\x1b[7m \x1b[27m");
                }
                row
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
        let window = visible_rows(painted.len(), at.0, max_rows);
        (
            painted[window.clone()].to_vec(),
            (at.0 - window.start, indent + at.1),
        )
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
    text.split('\t').map(super::width).sum::<usize>()
        + text.bytes().filter(|&b| b == b'\t').count() * 4
}

/// Normalize without allocating an unbounded copy of a pasted clipboard.
fn clean_text(text: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max_bytes));
    for g in text.graphemes(true) {
        let start = out.len();
        if matches!(g, "\r\n" | "\r") {
            if out.len() == max_bytes {
                break;
            }
            out.push('\n');
            continue;
        }
        for c in g
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        {
            if out.len() + c.len_utf8() > max_bytes {
                out.truncate(start);
                return out;
            }
            out.push(c);
        }
    }
    out
}

/// Byte ranges of visual rows (excluding newline separators), and the cursor.
/// The cursor is placed after wrapping, so a cursor before a wide glyph
/// belongs to the row that actually displays that glyph.
fn layout(text: &str, cursor: usize, room: usize) -> (Vec<Range<usize>>, (usize, usize)) {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut column = 0;
    let mut at = (0, 0);
    for (offset, g) in text.grapheme_indices(true) {
        if g == "\n" {
            if offset == cursor {
                at = (rows.len(), column);
            }
            rows.push(start..offset);
            start = offset + 1;
            column = 0;
            continue;
        }
        let width = display_width(g);
        if column > 0 && column + width > room {
            rows.push(start..offset);
            start = offset;
            column = 0;
        }
        if offset == cursor {
            at = (rows.len(), column);
        }
        column += width;
    }
    rows.push(start..text.len());
    if cursor == text.len() {
        at = (rows.len() - 1, column);
    }
    (rows, at)
}

fn visible_rows(count: usize, cursor_row: usize, max_rows: usize) -> Range<usize> {
    let max_rows = max_rows.max(1);
    let start = (cursor_row + 1).saturating_sub(max_rows);
    start..(start + max_rows).min(count)
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
    // `@path` anywhere in a message: a file of the workspace named in it.
    let word = &before[before.rfind(char::is_whitespace).map_or(0, |i| i + 1)..];
    if let Some(path) = word.strip_prefix('@') {
        let choices = paths(path, cwd)?;
        return Some((path.len(), choices, " "));
    }
    let arg = before
        .strip_prefix("/image")
        .or_else(|| before.strip_prefix("/attach"))
        .filter(|r| r.starts_with(char::is_whitespace))?
        .trim_start();
    if before.starts_with("/image ") && arg.starts_with('-') && "--public".starts_with(arg) {
        return Some((arg.len(), vec!["--public".to_owned()], " "));
    }
    let path = match arg.strip_prefix("--public") {
        Some(rest) if before.starts_with("/image ") && rest.starts_with(char::is_whitespace) => {
            rest.trim_start()
        }
        _ => arg,
    };
    let choices = paths(path, cwd)?;
    Some((path.len(), choices, ""))
}

/// The entries of `path`'s directory that complete its last part, each the
/// whole path (a directory ends with `/`); hidden ones only when asked for.
fn paths(path: &str, cwd: &Path) -> Option<Vec<String>> {
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
    (!choices.is_empty()).then_some(choices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn at_names_a_file_anywhere_in_a_message() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/tokenizer.rs"), "").unwrap();
        std::fs::write(d.path().join("src/parser.rs"), "").unwrap();
        let mut e = Editor::default();
        e.insert("look at @src/tok");
        assert!(
            e.complete(d.path(), &[]).is_empty(),
            "one choice: completed"
        );
        assert_eq!(e.buffer(), "look at @src/tokenizer.rs ");
        e.insert("and @src/");
        let mut listed = e.complete(d.path(), &[]);
        listed.sort();
        assert_eq!(listed, ["parser.rs", "tokenizer.rs"]);
    }

    fn typed(e: &mut Editor, text: &str) {
        for c in text.chars() {
            e.key(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    #[test]
    fn selection_copy_cut_replace_and_undo_preserve_multiline_graphemes() {
        let mut e = Editor::default();
        let original = "a👩‍💻e\u{301}中\n文z";
        e.insert(original);
        for _ in 0..5 {
            e.key(key(KeyCode::Left, SHIFT));
        }
        let selected = "e\u{301}中\n文z";
        assert_eq!(e.selected_text(), Some(selected));
        assert_eq!(
            e.key(key(KeyCode::Char('c'), CTRL)),
            Outcome::Copy(selected.into())
        );
        assert_eq!(e.buffer(), original);
        assert_eq!(
            e.key(key(KeyCode::Char('x'), CTRL)),
            Outcome::Copy(selected.into())
        );
        assert_eq!(e.buffer(), "a👩‍💻");
        e.key(key(KeyCode::Char('z'), CTRL));
        assert_eq!(e.buffer(), original);
        assert_eq!(e.selected_text(), Some(selected));
        e.insert("Q");
        assert_eq!(e.buffer(), "a👩‍💻Q");
        e.key(key(KeyCode::Char('z'), CTRL));
        e.key(key(KeyCode::Delete, NONE));
        assert_eq!(e.buffer(), "a👩‍💻");
        assert_eq!(e.key(key(KeyCode::Char('c'), CTRL)), Outcome::Interrupt);
        assert_eq!(e.buffer(), "");
        e.key(key(KeyCode::Char('z'), CTRL));
        assert_eq!(e.buffer(), "", "interrupt clears undo state");
    }

    #[test]
    fn word_and_line_selection_never_split_combining_text_or_enter_history() {
        let mut e = Editor::default();
        e.remember(["older message".into()]);
        e.insert("e\u{301}clair 👩‍💻 中文_xyz!");
        e.key(key(KeyCode::Home, CTRL));
        e.key(key(KeyCode::Right, CTRL | SHIFT));
        assert_eq!(e.selected_text(), Some("e\u{301}clair"));
        e.key(key(KeyCode::Right, ALT | SHIFT));
        assert_eq!(e.selected_text(), Some("e\u{301}clair 👩‍💻 中文_xyz"));
        e.key(key(KeyCode::Right, NONE));
        assert!(e.selected_text().is_none());
        assert_eq!(&e.buffer()[e.cursor()..], "!");
        e.key(key(KeyCode::Char('a'), CTRL));
        e.insert("top\nα中");
        e.key(key(KeyCode::Home, SHIFT));
        assert_eq!(e.selected_text(), Some("α中"));
        e.key(key(KeyCode::Up, SHIFT));
        e.key(key(KeyCode::Up, SHIFT));
        assert_eq!(e.selected_text(), Some("top\nα中"));
        assert_eq!(e.buffer(), "top\nα中", "selection cannot recall history");
        e.key(key(KeyCode::End, CTRL | SHIFT));
        assert!(e.selected_text().is_none());
        e.key(key(KeyCode::Home, CTRL | SHIFT));
        assert_eq!(e.selected_text(), Some("top\nα中"));
    }

    #[test]
    fn paste_is_one_undoable_edit_and_never_implicitly_submits() {
        let mut e = Editor::default();
        e.insert("draft");
        e.select_all();
        assert_eq!(e.key(key(KeyCode::Char('v'), CTRL)), Outcome::Paste);
        assert_eq!(e.buffer(), "draft");
        e.insert("first\r\n中文\r👩‍💻\x1b\0\n");
        assert_eq!(e.buffer(), "first\n中文\n👩‍💻\n");
        assert_eq!(e.key(key(KeyCode::Char('z'), CTRL)), Outcome::Edited);
        assert_eq!(e.buffer(), "draft");
        assert_eq!(e.selected_text(), Some("draft"));
        e.key(key(KeyCode::Char('Z'), CTRL | SHIFT));
        assert_eq!(e.buffer(), "first\n中文\n👩‍💻\n");
        e.key(key(KeyCode::Char('z'), CTRL));
        e.key(key(KeyCode::Char('y'), CTRL));
        assert_eq!(e.buffer(), "first\n中文\n👩‍💻\n");
        e.key(key(KeyCode::Char('z'), CTRL));
        e.insert("replacement");
        e.key(key(KeyCode::Char('y'), CTRL));
        assert_eq!(e.buffer(), "replacement", "new edits invalidate redo");
        assert_eq!(e.key(key(KeyCode::Insert, SHIFT)), Outcome::Paste);
        assert_eq!(e.key(key(KeyCode::Char('z'), CTRL | ALT)), Outcome::Suspend);
        assert_eq!(
            e.key(key(KeyCode::Enter, NONE)),
            Outcome::Submit("replacement".into())
        );
        e.key(key(KeyCode::Char('z'), CTRL));
        assert_eq!(
            e.buffer(),
            "",
            "sent messages cannot be undone into the input"
        );
    }

    #[test]
    fn insertion_and_deletion_keep_cursor_at_new_grapheme_boundaries() {
        let mut e = Editor::default();
        e.insert("\u{301}x");
        e.key(key(KeyCode::Home, NONE));
        e.insert("e");
        assert_eq!(e.buffer(), "e\u{301}x");
        assert_eq!(e.cursor(), "e\u{301}".len());
        e.key(key(KeyCode::Backspace, NONE));
        assert_eq!(e.buffer(), "x");
        e.clear();
        e.insert("🇨 🇦");
        e.key(key(KeyCode::Home, NONE));
        e.key(key(KeyCode::Right, NONE));
        e.key(key(KeyCode::Delete, NONE));
        assert_eq!(e.buffer(), "🇨🇦");
        assert_eq!(e.cursor(), e.buffer().len());
        e.key(key(KeyCode::Backspace, NONE));
        assert_eq!(e.buffer(), "");
    }

    #[test]
    fn mouse_selection_matches_wrapping_wide_graphemes_and_visible_window() {
        let mut e = Editor::default();
        e.insert("ab中👩‍💻x\nz");
        let prompt = [Span::new("> ", Style::PLAIN)];
        let (rows, cursor) = e.display(&prompt, false, 9, 2);
        assert_eq!(rows, ["  x", "  z"]);
        assert_eq!(cursor, (1, 3));
        assert!(e.select_at(&prompt, 9, 2, 0, 2, false));
        // Moving to the first visible row changes the cursor-following
        // viewport. Use the current full layout for the following drag.
        assert!(e.select_at(&prompt, 9, 10, 2, 3, true));
        assert_eq!(e.selected_text(), Some("x\nz"));
        e.select_at(&prompt, 9, 10, 0, 7, false);
        assert_eq!(
            e.cursor(),
            "ab中👩‍💻".len(),
            "second emoji cell maps to its trailing edge"
        );
        e.select_at(&prompt, 9, 10, 0, 4, true);
        assert_eq!(e.selected_text(), Some("中👩‍💻"));
        let (rows, cursor) = e.display(&prompt, false, 9, 10);
        assert!(rows[0].contains("\x1b[7m中\x1b[27m\x1b[7m👩‍💻\x1b[27m"));
        assert_eq!(cursor, (0, 4));
        e.key(key(KeyCode::Right, NONE));
        let (_, cursor) = e.display(&prompt, false, 9, 10);
        assert_eq!(
            cursor,
            (1, 2),
            "cursor before a wrapped glyph is on that glyph's row"
        );
    }

    #[test]
    fn selection_marks_newlines_and_search_backspace_removes_whole_graphemes() {
        let mut e = Editor::default();
        e.insert("a\nb");
        e.select_all();
        let (rows, _) = e.display(&[], false, 20, 10);
        assert_eq!(
            rows,
            ["\x1b[7ma\x1b[27m\x1b[7m \x1b[27m", "\x1b[7mb\x1b[27m"]
        );
        e.key(key(KeyCode::Char('r'), CTRL));
        typed(&mut e, "e\u{301}");
        e.key(key(KeyCode::Backspace, NONE));
        let (rows, _) = e.display(&[], false, 80, 5);
        assert_eq!(rows, ["search: "]);
        e.key(key(KeyCode::Esc, NONE));
        assert_eq!(e.selected_text(), Some("a\nb"));
    }

    #[test]
    fn oversized_pastes_and_undo_history_are_bounded() {
        let mut e = Editor::default();
        e.insert(&format!("{}👩‍💻suffix", "a".repeat(MAX_INPUT_BYTES - 1)));
        assert_eq!(e.buffer().len(), MAX_INPUT_BYTES - 1);
        assert!(!e.buffer().contains('👩'));
        e.select_all();
        e.insert("中");
        assert_eq!(e.buffer(), "中", "selection frees capacity before paste");
        e.key(key(KeyCode::Char('z'), CTRL));
        assert_eq!(e.buffer().len(), MAX_INPUT_BYTES - 1);
        // Successive full replacements exercise the byte bound as well as
        // the entry bound, without retaining unbounded copies of a paste.
        for i in 0..20 {
            e.select_all();
            e.insert(&format!("{i:02}{}", "b".repeat(MAX_INPUT_BYTES - 2)));
        }
        assert!(e.undo.bytes <= MAX_UNDO_BYTES);
        assert!(e.undo.entries.len() <= MAX_UNDO_ENTRIES);
        e.clear();
        for _ in 0..MAX_UNDO_ENTRIES + 20 {
            e.insert("x");
        }
        for _ in 0..MAX_UNDO_ENTRIES + 20 {
            e.key(key(KeyCode::Char('z'), CTRL));
        }
        assert_eq!(e.buffer(), "x".repeat(20));
        assert!(e.redo.bytes <= MAX_UNDO_BYTES && e.redo.entries.len() <= MAX_UNDO_ENTRIES);
    }

    #[test]
    fn editing_moves_by_character_word_and_line() {
        let mut e = Editor::default();
        typed(&mut e, "fix the csv export");
        e.key(key(KeyCode::Home, NONE));
        e.key(key(KeyCode::Char('f'), ALT));
        e.key(key(KeyCode::Char('f'), ALT));
        e.key(key(KeyCode::Char('w'), CTRL));
        assert_eq!(e.buffer(), "fix  csv export");
        e.key(key(KeyCode::Char('y'), ALT));
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
        // Ctrl-C always reaches the workspace, even while searching.
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
