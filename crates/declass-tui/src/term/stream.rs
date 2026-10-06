// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading the frontier's text as it streams: placeholders restored for the
//! operator even when a delta ends inside one, and the message of a `reply`
//! (or the question of `ask_operator`) read out of the call's JSON arguments
//! as they arrive.

use declass_boundary::vault::{CLOSE, OPEN};

/// Restores placeholders in streamed text. A placeholder is held back until
/// it is complete, so the operator sees exactly what restoring the whole
/// text at once shows: text is only restored cut just before an opening
/// bracket that has no closing one yet, and placeholders never contain an
/// opening bracket or a newline, so no placeholder spans a cut.
#[derive(Default)]
pub struct Detok {
    held: String,
}

/// The longest text held back waiting for a placeholder to close (real
/// placeholders are far shorter).
const HOLD_MAX: usize = 256;

impl Detok {
    /// Adds `delta`; returns what can be shown now, restored by `restore`.
    pub fn push(&mut self, delta: &str, restore: &dyn Fn(&str) -> String) -> String {
        self.held.push_str(delta);
        let cut = match self.held.rfind(OPEN) {
            Some(at) => {
                let tail = &self.held[at..];
                let open = !tail.contains(CLOSE) && !tail.contains('\n');
                if open && tail.len() <= HOLD_MAX {
                    at
                } else {
                    self.held.len()
                }
            }
            None => self.held.len(),
        };
        if cut == 0 {
            return String::new();
        }
        let ready: String = self.held.drain(..cut).collect();
        restore(&ready)
    }

    /// Whatever is still held (the text ended).
    pub fn finish(&mut self, restore: &dyn Fn(&str) -> String) -> String {
        let rest = std::mem::take(&mut self.held);
        if rest.is_empty() {
            String::new()
        } else {
            restore(&rest)
        }
    }
}

/// Reads one top-level string field out of a JSON object that arrives in
/// pieces, decoding escapes (including `\u` pairs split across pieces), and
/// returns the field's text as it arrives. Everything else is skipped.
pub struct FieldReader {
    field: &'static str,
    state: State,
    key: String,
    depth: usize,
    in_string: bool,
    escaped: bool,
    unicode: String,
    high: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Before the object's `{`.
    Start,
    /// Where a key (or the object's end) comes next.
    Key,
    InKey,
    Colon,
    Value,
    /// Inside the wanted string.
    Field,
    FieldEscape,
    FieldUnicode,
    /// Skipping another value (a string, number, literal, object or array).
    Skip,
    Done,
}

impl FieldReader {
    pub fn new(field: &'static str) -> Self {
        Self {
            field,
            state: State::Start,
            key: String::new(),
            depth: 0,
            in_string: false,
            escaped: false,
            unicode: String::new(),
            high: None,
        }
    }

    /// Adds a piece of the arguments; returns the field's text it carried.
    pub fn push(&mut self, piece: &str) -> String {
        let mut out = String::new();
        for c in piece.chars() {
            self.char(c, &mut out);
        }
        out
    }

    fn char(&mut self, c: char, out: &mut String) {
        match self.state {
            State::Start => {
                if c == '{' {
                    self.state = State::Key;
                }
            }
            State::Key => match c {
                '"' => {
                    self.key.clear();
                    self.state = State::InKey;
                }
                '}' => self.state = State::Done,
                _ => {}
            },
            State::InKey => {
                if self.escaped {
                    self.escaped = false;
                    self.key.push(c);
                } else if c == '\\' {
                    self.escaped = true;
                } else if c == '"' {
                    self.state = State::Colon;
                } else {
                    self.key.push(c);
                }
            }
            State::Colon => {
                if c == ':' {
                    self.state = State::Value;
                }
            }
            State::Value => match c {
                c if c.is_whitespace() => {}
                '"' if self.key == self.field => self.state = State::Field,
                _ => {
                    self.state = State::Skip;
                    self.depth = 0;
                    self.in_string = false;
                    self.skip(c);
                }
            },
            State::Field => match c {
                '\\' => self.state = State::FieldEscape,
                '"' => self.state = State::Done,
                c => self.emit(c, out),
            },
            State::FieldEscape => {
                self.state = State::Field;
                match c {
                    'n' => self.emit('\n', out),
                    't' => self.emit('\t', out),
                    'r' => self.emit('\r', out),
                    'b' | 'f' => {}
                    'u' => {
                        self.unicode.clear();
                        self.state = State::FieldUnicode;
                    }
                    c => self.emit(c, out),
                }
            }
            State::FieldUnicode => {
                self.unicode.push(c);
                if self.unicode.len() == 4 {
                    self.state = State::Field;
                    if let Ok(unit) = u16::from_str_radix(&self.unicode, 16) {
                        self.unit(unit, out);
                    }
                }
            }
            State::Skip => self.skip(c),
            State::Done => {}
        }
    }

    fn emit(&mut self, c: char, out: &mut String) {
        if self.high.take().is_some() {
            out.push(char::REPLACEMENT_CHARACTER);
        }
        out.push(c);
    }

    /// A `\u` escape: a surrogate pair's halves arrive as two escapes.
    fn unit(&mut self, unit: u16, out: &mut String) {
        match (self.high.take(), unit) {
            (None, 0xD800..=0xDBFF) => self.high = Some(unit),
            (Some(high), 0xDC00..=0xDFFF) => {
                let code =
                    0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            (high, unit) => {
                if high.is_some() {
                    out.push(char::REPLACEMENT_CHARACTER);
                }
                if (0xD800..=0xDFFF).contains(&unit) {
                    if (0xD800..=0xDBFF).contains(&unit) {
                        self.high = Some(unit);
                    } else {
                        out.push(char::REPLACEMENT_CHARACTER);
                    }
                } else {
                    out.push(
                        char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER),
                    );
                }
            }
        }
    }

    /// Skips a value that is not the wanted field, up to the `,` or `}`
    /// after it.
    fn skip(&mut self, c: char) {
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if c == '\\' {
                self.escaped = true;
            } else if c == '"' {
                self.in_string = false;
            }
            return;
        }
        match c {
            '"' => self.in_string = true,
            '{' | '[' => self.depth += 1,
            '}' | ']' if self.depth > 0 => self.depth -= 1,
            '}' => self.state = State::Done,
            ',' if self.depth == 0 => self.state = State::Key,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vault() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("⟨email:EMAIL#1⟩", "ana@example.org"),
            ("⟨secret:API_KEY#2⟩", "sk-live-51Hx"),
        ])
    }

    /// Restoring like the vault: known placeholders become their values.
    fn restore(text: &str) -> String {
        let mut out = text.to_owned();
        for (token, value) in vault() {
            out = out.replace(token, value);
        }
        out
    }

    #[test]
    fn a_placeholder_split_across_deltas_is_restored_whole() {
        let text = "Mail ⟨email:EMAIL#1⟩ about ⟨secret:API_KEY#2⟩; ⟨unknown⟩ stays, a stray ⟨ too.\nDone ⟨email:EMAIL#1⟩";
        let whole = restore(text);
        let chars: Vec<char> = text.chars().collect();
        for cut in 1..=chars.len() {
            for offset in 0..cut.min(4) {
                let mut d = Detok::default();
                let mut shown = String::new();
                let mut pieces = vec![chars[..offset].iter().collect::<String>()];
                pieces.extend(chars[offset..].chunks(cut).map(|p| p.iter().collect()));
                for p in &pieces {
                    let now = d.push(p, &restore);
                    // Never a piece of a placeholder on screen.
                    assert!(
                        !now.contains("⟨email") && !now.contains("⟨secret"),
                        "{now:?}"
                    );
                    shown.push_str(&now);
                }
                shown.push_str(&d.finish(&restore));
                assert_eq!(shown, whole, "cut {cut} offset {offset}");
            }
        }
        assert!(whole.contains("ana@example.org") && whole.contains("sk-live-51Hx"));
    }

    #[test]
    fn an_opening_bracket_that_never_closes_is_not_held_forever() {
        let mut d = Detok::default();
        assert_eq!(d.push("a ⟨ b", &restore), "a ");
        assert_eq!(d.push(" c\nd", &restore), "⟨ b c\nd");
        let long = "x".repeat(HOLD_MAX);
        assert_eq!(d.push(&format!("⟨{long}"), &restore), format!("⟨{long}"));
    }

    #[test]
    fn the_reply_message_is_read_as_the_arguments_arrive() {
        let args = r#"{"note": {"a": [1, "}\""]}, "n": 3, "message": "Line one\nsaid \"hi\" \\ café 😀 ⟨email:EMAIL#1⟩ end", "after": "x"}"#;
        let expected = "Line one\nsaid \"hi\" \\ café 😀 ⟨email:EMAIL#1⟩ end";
        let chars: Vec<char> = args.chars().collect();
        for cut in 1..=12 {
            let mut r = FieldReader::new("message");
            let got: String = chars
                .chunks(cut)
                .map(|p| r.push(&p.iter().collect::<String>()))
                .collect();
            assert_eq!(got, expected, "cut {cut}");
        }
        // Streamed through placeholder restoration, the operator sees values.
        let mut r = FieldReader::new("message");
        let mut d = Detok::default();
        let mut shown = String::new();
        for p in chars.chunks(3) {
            shown.push_str(&d.push(&r.push(&p.iter().collect::<String>()), &restore));
        }
        shown.push_str(&d.finish(&restore));
        assert!(shown.ends_with("ana@example.org end"), "{shown}");
        // Another field, or none, gives nothing.
        assert_eq!(FieldReader::new("question").push(args), "");
        assert_eq!(FieldReader::new("message").push(r#"{"message": 5}"#), "");
    }
}
