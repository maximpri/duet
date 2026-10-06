// SPDX-License-Identifier: GPL-3.0-or-later
//! Text with every value replaced by its shape.
//!
//! A word is kept as written only when it is vocabulary the frontier
//! already has: a word of the public files or the task, a word of
//! toolchains' own output ([`TOOL_WORDS`]: `passed`, `panicked`,
//! `expected`), and for a command's output a word of the command itself;
//! and not a word that occurs as a value of sensitive data. Everything else is a value: known
//! and detected values, other words and every number become shapes (`Aa`,
//! `a.a99@a-999.a`, `9999`). Dates keep their layout (`9999-99-99T99:99:99Z`).
//!
//! **Numbers** carry information one comparison at a time: a command can
//! print a count, or a digit of a value, as a small number, the same
//! channel as a yes/no question to the local model. So a number is shown as
//! written only if it is a small whole number (0 to 99) standing alone, and
//! only while the run's budget of such numbers lasts
//! (`sensitivity.masked_numbers`); every other number is its shape.

use super::Knowledge;
use super::shape::{self, Detail};
use crate::detect::Kind;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Words of test runners', compilers' and shells' own output: vocabulary,
/// not data (a repository too small to use them keeps them still). A word
/// that is a value of the data is masked all the same.
pub const TOOL_WORDS: &[&str] = &[
    "a",
    "actual",
    "an",
    "and",
    "as",
    "assert",
    "assertion",
    "at",
    "be",
    "by",
    "call",
    "can",
    "cannot",
    "caused",
    "code",
    "col",
    "column",
    "compiled",
    "compiling",
    "debug",
    "debuginfo",
    "denied",
    "dev",
    "directory",
    "done",
    "duration",
    "error",
    "errors",
    "exception",
    "exit",
    "expected",
    "fail",
    "failed",
    "failure",
    "failures",
    "false",
    "fatal",
    "file",
    "filtered",
    "finished",
    "for",
    "found",
    "from",
    "got",
    "has",
    "ignored",
    "in",
    "info",
    "invalid",
    "is",
    "it",
    "last",
    "left",
    "line",
    "main",
    "measured",
    "missing",
    "ms",
    "nan",
    "no",
    "none",
    "not",
    "null",
    "of",
    "ok",
    "on",
    "open",
    "optimized",
    "or",
    "out",
    "panic",
    "panicked",
    "pass",
    "passed",
    "passing",
    "pending",
    "permission",
    "profile",
    "property",
    "read",
    "received",
    "recent",
    "release",
    "required",
    "result",
    "results",
    "right",
    "run",
    "running",
    "skip",
    "skipped",
    "stack",
    "status",
    "succeeded",
    "success",
    "such",
    "suite",
    "suites",
    "syntax",
    "target",
    "test",
    "tests",
    "the",
    "thread",
    "time",
    "to",
    "todo",
    "token",
    "total",
    "trace",
    "traceback",
    "true",
    "type",
    "undefined",
    "unexpected",
    "unknown",
    "unoptimized",
    "value",
    "want",
    "warn",
    "warning",
    "warnings",
    "was",
    "with",
];

/// A number shown as written must be at most this many digits.
pub const SMALL_NUMBER_DIGITS: usize = 2;
/// Characters of a line shown in a template.
const TEMPLATE_CHARS: usize = 200;
/// Templates listed.
const TEMPLATES_SHOWN: usize = 20;

/// A date-time written with digits (`2026-09-01T16:28:24.3Z`): kept as a
/// layout, its letters (`T`, `Z`) as written.
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{4}-\d{2}-\d{2}(?:[T ]\d{2}:\d{2}(?::\d{2}(?:[.,]\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?\b|\b\d{2}:\d{2}:\d{2}(?:[.,]\d+)?\b")
        .expect("static regex")
});
/// A token: a number with a decimal point or grouping, a word (letters,
/// digits, `_`), or one other character.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+(?:[.,]\d+)+|[\p{L}\p{N}_]+|(?s:.)").expect("static regex"));
/// Lines Declass itself writes around a command's output (see [`framing`]).
static FRAMING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:exit code -?\d+|terminated by a signal|timed out after \d+s|--- (?:stdout|stderr) ---|\[(?:stdout|stderr) truncated: \d+ of \d+ bytes shown\])$")
        .expect("static regex")
});

/// Whether `line` is one Declass writes around a command's output (`exit code
/// 0`, `--- stdout ---`), not the program's.
pub fn framing(line: &str) -> bool {
    FRAMING.is_match(line)
}

/// How numbers standing alone are shown.
pub trait Numbers {
    /// `Some(digits)` to show the number as written, `None` for its shape.
    fn show(&mut self, digits: &str) -> bool;
}

/// Every number as its shape.
pub struct NoNumbers;
impl Numbers for NoNumbers {
    fn show(&mut self, _: &str) -> bool {
        false
    }
}

/// Small numbers shown while a budget lasts; counts what it did.
pub struct Budget {
    pub left: u32,
    pub shown: u32,
    pub masked: u32,
}

impl Numbers for Budget {
    fn show(&mut self, digits: &str) -> bool {
        if digits.len() <= SMALL_NUMBER_DIGITS && self.left > 0 {
            self.left -= 1;
            self.shown += 1;
            true
        } else {
            self.masked += 1;
            false
        }
    }
}

/// One line masked: what is shown, and the key lines of one template share.
struct Masked {
    shown: String,
    key: String,
}

fn mask_line(
    line: &str,
    k: &dyn Knowledge,
    extra: &HashSet<String>,
    numbers: &mut dyn Numbers,
) -> Masked {
    // Values (with their kind) and dates (`None`), overlaps merged.
    let mut spans: Vec<(usize, usize, Option<Kind>)> = k
        .values(line)
        .into_iter()
        .map(|(s, e, kind)| (s, e, Some(kind)))
        .chain(DATE.find_iter(line).map(|m| (m.start(), m.end(), None)))
        .collect();
    spans.sort_by_key(|s| (s.0, std::cmp::Reverse(s.1)));
    let mut merged: Vec<(usize, usize, Option<Kind>)> = Vec::new();
    for s in spans {
        match merged.last_mut() {
            Some(last) if s.0 < last.1 => {
                last.1 = last.1.max(s.1);
                last.2 = last.2.or(s.2);
            }
            _ => merged.push(s),
        }
    }
    let mut shown = String::with_capacity(line.len());
    let mut key = String::with_capacity(line.len());
    let mut at = 0;
    let mut words = |text: &str, shown: &mut String, key: &mut String| {
        for m in TOKEN.find_iter(text) {
            let t = m.as_str();
            if t.bytes().all(|b| b.is_ascii_digit()) {
                if numbers.show(t) {
                    shown.push_str(t);
                } else {
                    shown.push_str(&shape::shape(t, Detail::Exact));
                }
                key.push('9');
            } else if t.chars().next().is_some_and(char::is_numeric) && t.contains(['.', ',']) {
                shown.push_str(&shape::shape(t, Detail::Exact));
                key.push('9');
            } else if t.chars().any(char::is_alphanumeric) {
                let lower = t.to_lowercase();
                let known = k.public_word(&lower) || TOOL_WORDS.contains(&lower.as_str());
                let keep = extra.contains(&lower) || (known && !k.data_word(&lower));
                if keep {
                    shown.push_str(t);
                    key.push_str(t);
                } else {
                    shown.push_str(&shape::shape(t, Detail::Runs));
                    key.push_str(&shape::shape(t, Detail::Loose));
                }
            } else {
                shown.push_str(&shape::display_char(t.chars().next().unwrap_or(' ')));
                key.push_str(t);
            }
        }
    };
    for (s, e, kind) in merged {
        words(&line[at..s], &mut shown, &mut key);
        let v = &line[s..e];
        if let Some(kind) = kind {
            shown.push_str(&value_shape(v, kind));
            key.push_str(&format!("⟨{}⟩", kind.tag()));
        } else {
            let date: String = v
                .chars()
                .map(|c| if c.is_ascii_digit() { '9' } else { c })
                .collect();
            shown.push_str(&date);
            key.push_str(&date);
        }
        at = e;
    }
    words(&line[at..], &mut shown, &mut key);
    Masked { shown, key }
}

/// How a value shows in masked text: a secret as `•••` (the class of each
/// of its characters is a part of its strength; a URL keeps its layout,
/// its password withheld), any other value by its runs, a date inside it
/// by its layout.
fn value_shape(v: &str, kind: Kind) -> String {
    if kind == Kind::Secret {
        return super::profile::url_shape(v.trim()).unwrap_or_else(|| "•••".to_owned());
    }
    let mut shown = String::new();
    let mut last = 0;
    for d in DATE.find_iter(v) {
        shown.push_str(&shape::shape(&v[last..d.start()], Detail::Runs));
        shown.extend(
            d.as_str()
                .chars()
                .map(|c| if c.is_ascii_digit() { '9' } else { c }),
        );
        last = d.end();
    }
    shown.push_str(&shape::shape(&v[last..], Detail::Runs));
    shown
}

/// A command's output with every value masked. `command` is the command
/// line the frontier wrote: its words are the frontier's own and are kept.
/// Declass's framing (`exit code 0`, `--- stdout ---`) is kept as written.
pub fn mask(text: &str, command: &str, k: &dyn Knowledge, numbers: &mut dyn Numbers) -> String {
    let extra: HashSet<String> = TOKEN
        .find_iter(command)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| w.chars().any(char::is_alphabetic))
        .collect();
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if framing(line) {
            out.push_str(line);
        } else {
            out.push_str(&mask_line(line, k, &extra, numbers).shown);
        }
        out.push('\n');
    }
    out
}

/// Lines that share a masked shape.
#[derive(Debug, Clone)]
pub struct Template {
    /// The first such line, masked (numbers as their shapes).
    pub shown: String,
    pub count: usize,
    /// 1-based line number of the first.
    pub first: usize,
}

/// The templates of `lines` that two or more lines share, most frequent
/// first. A line only one line has is left out: one-off prose is content,
/// not structure.
pub fn templates(lines: &[&str], k: &dyn Knowledge) -> Vec<Template> {
    let none = HashSet::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Template> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let cut: String = line.chars().take(TEMPLATE_CHARS * 2).collect();
        let m = mask_line(&cut, k, &none, &mut NoNumbers);
        let key = m.key.split_whitespace().collect::<Vec<_>>().join(" ");
        match by_key.get(&key) {
            Some(&t) => out[t].count += 1,
            None => {
                by_key.insert(key, out.len());
                let shown: String = m
                    .shown
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(TEMPLATE_CHARS)
                    .collect();
                out.push(Template {
                    shown,
                    count: 1,
                    first: i + 1,
                });
            }
        }
    }
    out.retain(|t| t.count >= 2);
    out.sort_by(|a, b| b.count.cmp(&a.count).then(a.first.cmp(&b.first)));
    out
}

/// The templates as lines of a view.
pub fn render_templates(templates: &[Template], lines: usize) -> String {
    let covered: usize = templates.iter().map(|t| t.count).sum();
    let mut out = format!(
        "  Line templates ({} shapes covering {covered} of {lines} lines; words of the public files kept, other words and values as shapes; digits as in the first line of each):\n",
        templates.len()
    );
    for t in templates.iter().take(TEMPLATES_SHOWN) {
        out.push_str(&format!(
            "    ×{:<5} from line {:<6} {}\n",
            t.count, t.first, t.shown
        ));
    }
    if templates.len() > TEMPLATES_SHOWN {
        out.push_str(&format!(
            "    … {} more templates\n",
            templates.len() - TEMPLATES_SHOWN
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Public words: a small vocabulary; emails are values; `settled` is data.
    struct Toy;
    impl Knowledge for Toy {
        fn values(&self, text: &str) -> Vec<(usize, usize, Kind)> {
            let re = Regex::new(r"\S+@\S+\.\w+").unwrap();
            re.find_iter(text)
                .map(|m| (m.start(), m.end(), Kind::Email))
                .collect()
        }
        fn public_word(&self, w: &str) -> bool {
            [
                "error", "invalid", "token", "at", "position", "entries", "status", "settled",
                "orders", "from", "info", "query",
            ]
            .contains(&w)
        }
        fn data_word(&self, w: &str) -> bool {
            w == "settled"
        }
    }

    #[test]
    fn values_words_and_numbers_become_shapes() {
        let mut b = Budget {
            left: 1,
            shown: 0,
            masked: 0,
        };
        let out = mask(
            "exit code 1\n--- stdout ---\nentries: 67\nError: invalid token at position 345 for Quenanlek <amelia@mail-9.net>\nstatus settled 12.50 7\n",
            "node x.js",
            &Toy,
            &mut b,
        );
        assert_eq!(
            out,
            "exit code 1\n--- stdout ---\nentries: 67\nError: invalid token at position 999 for Aa <a@a-9.a>\nstatus a 99.99 9\n"
        );
        assert_eq!((b.shown, b.masked, b.left), (1, 2, 0));
    }

    #[test]
    fn toolchain_words_are_kept_unless_they_are_values_of_the_data() {
        struct Pending;
        impl Knowledge for Pending {
            fn values(&self, _: &str) -> Vec<(usize, usize, Kind)> {
                Vec::new()
            }
            fn public_word(&self, _: &str) -> bool {
                false
            }
            fn data_word(&self, w: &str) -> bool {
                w == "pending"
            }
        }
        let mut b = Budget {
            left: 9,
            shown: 0,
            masked: 0,
        };
        let out = mask(
            "test result: ok. 1 passed; 0 failed; 2 pending\nstatus pending\n",
            "cargo test",
            &Pending,
            &mut b,
        );
        assert_eq!(out, "test result: ok. 1 passed; 0 failed; 2 a\nstatus a\n");
        let mut sorted = TOOL_WORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, TOOL_WORDS, "kept sorted");
    }

    #[test]
    fn the_commands_own_words_are_kept() {
        let out = mask(
            "accounting entries: 67\n",
            "console.log('accounting entries:')",
            &Toy,
            &mut NoNumbers,
        );
        assert_eq!(out, "accounting entries: 99\n");
    }

    #[test]
    fn repeated_lines_group_into_templates_with_dates_kept_as_layouts() {
        let lines = [
            "2026-09-01T12:47:20Z INFO query from Amelia <amelia@mail-9.net> 12",
            "2026-09-02T08:36:59Z INFO query from Kenji <kenji.v@mail-505.net> 3",
            "one-off prose line about Northwind",
        ];
        let t = templates(&lines, &Toy);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].count, 2);
        assert_eq!(
            t[0].shown,
            "9999-99-99T99:99:99Z INFO query from Aa <a@a-9.a> 99"
        );
        let view = render_templates(&t, 3);
        assert!(
            !view.contains("Northwind") && !view.contains("Amelia"),
            "{view}"
        );
    }
}
