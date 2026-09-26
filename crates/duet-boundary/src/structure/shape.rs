// SPDX-License-Identifier: GPL-3.0-or-later
//! Shape masks: a value written with its letters as `A`/`a`, its digits as
//! `9` and everything else kept (`2026-09-01` → `9999-99-99`,
//! `Amelia Stone` → `Aaaaaa Aaaaa`). A shape says how a value is written,
//! never which value it is.

/// How much of a value's writing a shape keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Every character by its class: lengths and positions kept.
    Exact,
    /// Letter runs collapsed to their case pattern (`a`, `A`, `Aa`, `aA`),
    /// digit runs kept digit by digit (a format lives in its digits:
    /// dates, fixed-width numbers), other characters kept. For identifying
    /// values (names, emails), whose letter counts describe a person.
    Runs,
    /// As [`Detail::Runs`], with each digit run as a single `9` too: for
    /// grouping free text into templates.
    Loose,
}

/// A character's class in a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Upper,
    Lower,
    /// A letter without case (CJK, Arabic, ...): shown as `a`.
    Caseless,
    Digit,
    /// A control character (never shown as itself).
    Control,
    /// Whitespace, punctuation and symbols: shown as themselves.
    Other,
}

pub fn class(c: char) -> Class {
    if c.is_numeric() {
        Class::Digit
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_lowercase() {
        Class::Lower
    } else if c.is_alphabetic() {
        Class::Caseless
    } else if c.is_control() && c != '\t' {
        Class::Control
    } else {
        Class::Other
    }
}

/// How one character is written in a shape.
fn masked(c: char) -> char {
    match class(c) {
        Class::Upper => 'A',
        Class::Lower | Class::Caseless => 'a',
        Class::Digit => '9',
        Class::Control => '?',
        Class::Other => c,
    }
}

/// The case pattern of a letter run: `a` (lower case), `A` (upper case),
/// `Aa` (capitalized) or `aA` (mixed).
fn run_pattern(run: &[char]) -> &'static str {
    let upper = |c: &char| class(*c) == Class::Upper;
    let first = run.first().is_some_and(upper);
    let rest_upper = run.iter().skip(1).filter(|c| upper(c)).count();
    match (first, rest_upper) {
        (false, 0) => "a",
        (true, n) if n == run.len() - 1 => "A",
        (true, 0) => "Aa",
        _ => "aA",
    }
}

/// `value`'s shape at `detail`.
pub fn shape(value: &str, detail: Detail) -> String {
    if detail == Detail::Exact {
        return value.chars().map(masked).collect();
    }
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match class(c) {
            Class::Upper | Class::Lower | Class::Caseless => {
                let start = i;
                while i < chars.len()
                    && matches!(
                        class(chars[i]),
                        Class::Upper | Class::Lower | Class::Caseless
                    )
                {
                    i += 1;
                }
                out.push_str(run_pattern(&chars[start..i]));
                continue;
            }
            Class::Digit if detail == Detail::Loose => {
                while i < chars.len() && class(chars[i]) == Class::Digit {
                    i += 1;
                }
                out.push('9');
                continue;
            }
            _ => out.push(masked(c)),
        }
        i += 1;
    }
    out
}

/// What a secret-like value may be described by: its length and the kinds
/// of characters it uses (`32 chars: A-Z a-z 0-9 _`). A shape would give
/// the class of every position, a real part of a random key's strength.
pub fn secret_summary(value: &str) -> String {
    let mut upper = false;
    let mut lower = false;
    let mut digit = false;
    let mut other: Vec<char> = Vec::new();
    for c in value.chars() {
        match class(c) {
            Class::Upper => upper = true,
            Class::Lower | Class::Caseless => lower = true,
            Class::Digit => digit = true,
            _ => {
                if !other.contains(&c) {
                    other.push(c);
                }
            }
        }
    }
    other.sort_unstable();
    let mut parts: Vec<String> = Vec::new();
    if upper {
        parts.push("A-Z".into());
    }
    if lower {
        parts.push("a-z".into());
    }
    if digit {
        parts.push("0-9".into());
    }
    if !other.is_empty() {
        parts.push(other.iter().map(|c| display_char(*c)).collect());
    }
    format!("{} chars: {}", value.chars().count(), parts.join(" "))
}

/// A character as it may be printed inside a view line.
pub fn display_char(c: char) -> String {
    match c {
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        c if c.is_control() => "?".into(),
        c => c.to_string(),
    }
}

/// `shape` made printable on one line (line breaks and tabs escaped).
pub fn printable(shape: &str) -> String {
    shape.chars().map(display_char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_shapes_keep_lengths_and_punctuation() {
        assert_eq!(shape("2026-09-01", Detail::Exact), "9999-99-99");
        assert_eq!(shape("Amelia Stone", Detail::Exact), "Aaaaaa Aaaaa");
        assert_eq!(shape("FH-0907", Detail::Exact), "AA-9999");
        assert_eq!(shape("ødegaard", Detail::Exact), "aaaaaaaa");
        assert_eq!(shape("a\u{7}b", Detail::Exact), "a?a");
    }

    #[test]
    fn run_shapes_collapse_letters_and_keep_digit_counts() {
        assert_eq!(
            shape("oscar.yorovvay01@mailbox-989.net", Detail::Runs),
            "a.a99@a-999.a"
        );
        assert_eq!(shape("Amelia McStone", Detail::Runs), "Aa aA");
        assert_eq!(shape("PSTN same-day", Detail::Runs), "A a-a");
        assert_eq!(
            shape("+1 (607) 914-6209", Detail::Runs),
            "+9 (999) 999-9999"
        );
        assert_eq!(shape("iPhone", Detail::Runs), "aA");
        assert_eq!(shape("port 8080 of 12", Detail::Loose), "a 9 a 9");
    }

    #[test]
    fn secrets_are_described_by_length_and_character_kinds() {
        assert_eq!(
            secret_summary("sk_live_Qm8vT2xW9pL4nR7k"),
            "24 chars: A-Z a-z 0-9 _"
        );
        assert_eq!(secret_summary("500"), "3 chars: 0-9");
    }
}
