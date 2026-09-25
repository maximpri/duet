// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Statement fingerprints: SQL text with every constant replaced by `?`,
//! suitable for audit logs. See `docs/FINGERPRINT.md` for the rules.

#[cfg(not(feature = "std"))]
use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};

use crate::dialect::Dialect;
use crate::keywords::Keyword;
use crate::tokenizer::{Token, Tokenizer, TokenizerError, Whitespace};

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Constant,
    /// Unquoted word that is a keyword.
    Keyword(Keyword),
    /// Identifier (unquoted non-keyword word or quoted identifier).
    Identifier,
    Placeholder,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
struct Item {
    kind: Kind,
    text: String,
}

impl Item {
    fn constant() -> Self {
        Item {
            kind: Kind::Constant,
            text: "?".to_string(),
        }
    }

    fn is(&self, text: &str) -> bool {
        self.kind == Kind::Other && self.text == text
    }

    fn is_keyword(&self, keyword: Keyword) -> bool {
        self.kind == Kind::Keyword(keyword)
    }

    /// Whether a `-` or `+` directly after this item is a sign (part of a
    /// following number) rather than a binary operator.
    fn allows_sign(&self) -> bool {
        match self.kind {
            Kind::Constant | Kind::Identifier | Kind::Placeholder => false,
            Kind::Keyword(k) => SIGN_KEYWORDS.contains(&k),
            Kind::Other => self.text != ")" && self.text != "]",
        }
    }
}

/// Keywords after which a `-` or `+` is a sign.
const SIGN_KEYWORDS: &[Keyword] = &[
    Keyword::SELECT,
    Keyword::WHERE,
    Keyword::AND,
    Keyword::OR,
    Keyword::NOT,
    Keyword::ON,
    Keyword::HAVING,
    Keyword::WHEN,
    Keyword::THEN,
    Keyword::ELSE,
    Keyword::BETWEEN,
    Keyword::LIMIT,
    Keyword::OFFSET,
    Keyword::RETURNING,
];

/// Returns the fingerprint of `sql`: the statements' tokens with comments and
/// whitespace removed, constants replaced by `?`, lists of constants collapsed
/// and keywords upper-cased. See `docs/FINGERPRINT.md`.
pub fn fingerprint(dialect: &dyn Dialect, sql: &str) -> Result<String, TokenizerError> {
    let tokens = Tokenizer::new(dialect, sql).tokenize()?;
    let mut statements: Vec<Vec<Item>> = vec![Vec::new()];
    let mut previous_was_string = false;
    let mut newline_since_string = false;
    let continuation = dialect.supports_string_literal_continuation();
    for token in tokens {
        let current = statements.last_mut().expect("at least one statement");
        let single_quoted = matches!(token, Token::SingleQuotedString(_));
        let item = match token {
            Token::EOF => continue,
            Token::Whitespace(ws) => {
                match ws {
                    Whitespace::Newline | Whitespace::SingleLineComment { .. } => {
                        newline_since_string = true
                    }
                    Whitespace::MultiLineComment(_) => previous_was_string = false,
                    _ => {}
                }
                continue;
            }
            Token::SemiColon => {
                statements.push(Vec::new());
                previous_was_string = false;
                continue;
            }
            Token::SingleQuotedString(_)
                if continuation && previous_was_string && newline_since_string =>
            {
                // A continued string constant: already represented by one `?`.
                newline_since_string = false;
                continue;
            }
            Token::Number(..) => {
                // Fold a sign that does not follow an operand into the constant.
                let len = current.len();
                if len > 0 && (current[len - 1].is("-") || current[len - 1].is("+")) {
                    let sign_is_unary = len == 1 || current[len - 2].allows_sign();
                    if sign_is_unary {
                        current.pop();
                    }
                }
                Item::constant()
            }
            Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::TripleSingleQuotedString(_)
            | Token::TripleDoubleQuotedString(_)
            | Token::DollarQuotedString(_)
            | Token::SingleQuotedByteStringLiteral(_)
            | Token::DoubleQuotedByteStringLiteral(_)
            | Token::TripleSingleQuotedByteStringLiteral(_)
            | Token::TripleDoubleQuotedByteStringLiteral(_)
            | Token::SingleQuotedRawStringLiteral(_)
            | Token::DoubleQuotedRawStringLiteral(_)
            | Token::TripleSingleQuotedRawStringLiteral(_)
            | Token::TripleDoubleQuotedRawStringLiteral(_)
            | Token::NationalStringLiteral(_)
            | Token::EscapedStringLiteral(_)
            | Token::HexStringLiteral(_) => Item::constant(),
            Token::Word(w) => {
                if w.quote_style.is_some() {
                    Item {
                        kind: Kind::Identifier,
                        text: w.to_string(),
                    }
                } else if w.keyword != Keyword::NoKeyword {
                    Item {
                        kind: Kind::Keyword(w.keyword),
                        text: w.value.to_uppercase(),
                    }
                } else {
                    Item {
                        kind: Kind::Identifier,
                        text: w.value.to_lowercase(),
                    }
                }
            }
            Token::Placeholder(p) => Item {
                kind: Kind::Placeholder,
                text: p,
            },
            other => Item {
                kind: Kind::Other,
                text: other.to_string(),
            },
        };
        previous_was_string = single_quoted;
        newline_since_string = false;
        current.push(item);
    }
    let rendered: Vec<String> = statements
        .into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| render(&collapse(s)))
        .collect();
    Ok(rendered.join("; "))
}

/// Collapses `IN (?, ?)`, `ARRAY[?, ?]` and repeated `VALUES` rows.
fn collapse(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    let mut i = 0;
    while i < items.len() {
        let item = &items[i];
        out.push(item.clone());
        i += 1;
        if item.is_keyword(Keyword::IN) && i < items.len() && items[i].is("(") {
            if let Some(end) = constant_list_end(&items, i + 1, ")") {
                out.push(items[i].clone());
                out.push(Item::constant());
                out.push(items[end].clone());
                i = end + 1;
            }
        } else if item.is_keyword(Keyword::ARRAY) && i < items.len() && items[i].is("[") {
            if let Some(end) = constant_list_end(&items, i + 1, "]") {
                out.push(items[i].clone());
                out.push(Item::constant());
                out.push(items[end].clone());
                i = end + 1;
            }
        } else if item.is_keyword(Keyword::VALUES) {
            i = collapse_rows(&items, i, &mut out);
        }
    }
    out
}

/// If `items[start..]` is `?[, ?]* close`, the index of `close`.
fn constant_list_end(items: &[Item], start: usize, close: &str) -> Option<usize> {
    let mut i = start;
    loop {
        if items.get(i)?.kind != Kind::Constant {
            return None;
        }
        i += 1;
        let next = items.get(i)?;
        if next.is(close) {
            return Some(i);
        }
        if !next.is(",") {
            return None;
        }
        i += 1;
    }
}

/// Copies the rows following `VALUES` (starting at `start`) into `out`,
/// dropping each row that renders like the row before it. Returns the index
/// after the last row.
fn collapse_rows(items: &[Item], start: usize, out: &mut Vec<Item>) -> usize {
    let mut i = start;
    let mut last_row: Option<String> = None;
    loop {
        if !items.get(i).map_or(false, |t| t.is("(")) {
            return i;
        }
        let Some(end) = matching_paren(items, i) else {
            return i;
        };
        let row = collapse(items[i..=end].to_vec());
        let text = render(&row);
        let separator = if last_row.is_some() { 1 } else { 0 };
        if last_row.as_deref() != Some(text.as_str()) {
            if separator == 1 {
                out.push(items[i - 1].clone());
            }
            out.extend(row);
            last_row = Some(text);
        }
        i = end + 1;
        if items.get(i).map_or(false, |t| t.is(","))
            && items.get(i + 1).map_or(false, |t| t.is("("))
        {
            i += 1;
        } else {
            return i;
        }
    }
}

fn matching_paren(items: &[Item], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, item) in items.iter().enumerate().skip(open) {
        if item.is("(") {
            depth += 1;
        } else if item.is(")") {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn render(items: &[Item]) -> String {
    let mut s = String::new();
    let mut previous: Option<&Item> = None;
    for item in items {
        if let Some(prev) = previous {
            let tight_after = prev.kind == Kind::Other
                && matches!(prev.text.as_str(), "(" | "[" | "." | "::");
            let tight_before = item.kind == Kind::Other
                && matches!(item.text.as_str(), ")" | "]" | "," | "." | "::");
            if !tight_after && !tight_before {
                s.push(' ');
            }
        }
        s.push_str(&item.text);
        previous = Some(item);
    }
    s
}
