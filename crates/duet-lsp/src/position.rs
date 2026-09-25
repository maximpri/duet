// SPDX-License-Identifier: GPL-3.0-or-later
//! Positions. The frontier counts lines from 1 (as `read_file` numbers them)
//! and columns from 1 in characters; the protocol counts both from 0 and
//! columns in UTF-16 code units (the only encoding every server supports, and
//! the only one this client offers).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// The text of each line, without its terminator (`\n` or `\r\n`).
fn line_of(text: &str, line: usize) -> Option<&str> {
    text.split('\n')
        .nth(line)
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// The protocol position of 1-based `line` and 1-based character `column`.
/// A column past the end of the line is clamped to the end.
pub fn to_lsp(text: &str, line: u32, column: u32) -> Result<Position, String> {
    if line == 0 || column == 0 {
        return Err("line and column count from 1".into());
    }
    let lines = text.split('\n').count();
    let l = line_of(text, line as usize - 1)
        .ok_or_else(|| format!("line {line} is outside the file ({lines} lines)"))?;
    let units: usize = l
        .chars()
        .take(column as usize - 1)
        .map(char::len_utf16)
        .sum();
    Ok(Position {
        line: line - 1,
        character: units as u32,
    })
}

/// 1-based line and character column of a protocol position. A position in
/// the middle of a surrogate pair counts as that character; one past the end
/// of the line is clamped to the end.
pub fn from_lsp(text: &str, pos: Position) -> (u32, u32) {
    let Some(l) = line_of(text, pos.line as usize) else {
        return (pos.line + 1, pos.character + 1);
    };
    let mut units = 0usize;
    let mut chars = 0u32;
    for c in l.chars() {
        let next = units + c.len_utf16();
        if next > pos.character as usize {
            break;
        }
        units = next;
        chars += 1;
    }
    (pos.line + 1, chars + 1)
}

/// The byte offset of a protocol position in `text`; `None` if the line does
/// not exist. A character past the end of the line means its end.
pub fn offset(text: &str, pos: Position) -> Option<usize> {
    let mut start = 0;
    for _ in 0..pos.line {
        start += text[start..].find('\n')? + 1;
    }
    let rest = &text[start..];
    let line = rest.split('\n').next().unwrap_or("");
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut units = 0usize;
    for (i, c) in line.char_indices() {
        if units >= pos.character as usize {
            return Some(start + i);
        }
        units += c.len_utf16();
    }
    Some(start + line.len())
}

/// The 1-based line `line` of `text` (for snippets), without its terminator.
pub fn line_text(text: &str, line: u32) -> Option<&str> {
    line.checked_sub(1).and_then(|l| line_of(text, l as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "fn a() {}\r\nlet s = \"é😀\"; call(x);\nlast";

    #[test]
    fn columns_convert_through_utf16() {
        // `c` of `call` is the 15th character; before it are 2 characters
        // outside the ASCII range, one of which needs a surrogate pair.
        let p = to_lsp(TEXT, 2, 15).unwrap();
        assert_eq!(
            p,
            Position {
                line: 1,
                character: 15
            }
        );
        assert_eq!(from_lsp(TEXT, p), (2, 15));
        let at = offset(TEXT, p).unwrap();
        assert!(TEXT[at..].starts_with("call"), "{}", &TEXT[at..]);
        // `\r\n` endings are not part of the line.
        assert_eq!(to_lsp(TEXT, 1, 99).unwrap().character, 9);
        assert_eq!(
            offset(
                TEXT,
                Position {
                    line: 0,
                    character: 99
                }
            ),
            Some(9)
        );
        assert_eq!(line_text(TEXT, 1), Some("fn a() {}"));
        // Inside the surrogate pair counts as the emoji itself.
        assert_eq!(
            from_lsp(
                TEXT,
                Position {
                    line: 1,
                    character: 10
                }
            ),
            (2, 11)
        );
        assert_eq!(
            from_lsp(
                TEXT,
                Position {
                    line: 1,
                    character: 11
                }
            ),
            (2, 11)
        );
        assert_eq!(
            from_lsp(
                TEXT,
                Position {
                    line: 1,
                    character: 12
                }
            ),
            (2, 12)
        );
    }

    #[test]
    fn positions_outside_the_file_are_reported() {
        assert!(to_lsp(TEXT, 4, 1).unwrap_err().contains("outside"));
        assert!(to_lsp(TEXT, 0, 1).is_err());
        assert_eq!(
            offset(
                TEXT,
                Position {
                    line: 3,
                    character: 0
                }
            ),
            None
        );
        assert_eq!(
            offset(
                TEXT,
                Position {
                    line: 2,
                    character: 2
                }
            ),
            Some(TEXT.len() - 2)
        );
    }
}
