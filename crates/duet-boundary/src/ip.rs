// SPDX-License-Identifier: GPL-3.0-or-later
//! Protected source code (IP levels): the pieces that catch copies of it.
//!
//! - [`ProtectedLines`]: every distinctive line of a withheld body. Outbound
//!   text containing one (a compiler snippet, a `cat` that slipped through, a
//!   local answer quoting code) has that line replaced. Lines that also occur
//!   in public files or in a skeleton are exempt.
//! - [`result_lines`]: from the output of a run that could read protected
//!   code, only lines with a recognised result shape (test outcomes, compiler
//!   diagnostics, panic locations, assertion values) are shown.
//! - [`distinctive_literals`]: string literals and identifier-like tokens of
//!   withheld bodies, which the engine adds to the placeholder vault so a
//!   quoted fragment is replaced wherever it appears.

use aho_corasick::AhoCorasick;
use regex::Regex;
use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

pub const LINE_WITHHELD: &str = "⟨protected code line withheld⟩";
/// Normalized length from which a line counts as distinctive.
pub const MIN_LINE_CHARS: usize = 16;
/// Characters of a result line shown.
const MAX_RESULT_CHARS: usize = 240;
/// Characters of a panic or assertion message shown.
const MAX_MESSAGE_CHARS: usize = 160;
/// Result lines shown per output.
pub const MAX_RESULT_LINES: usize = 60;

fn normalize(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn distinctive(norm: &str) -> bool {
    norm.chars().count() >= MIN_LINE_CHARS
        && norm
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .count()
            >= 3
}

#[derive(Default)]
pub struct ProtectedLines {
    protected: BTreeSet<String>,
    public: HashSet<String>,
    matcher: Option<AhoCorasick>,
    dirty: bool,
}

impl ProtectedLines {
    /// Indexes the distinctive lines of withheld text.
    pub fn add_protected(&mut self, text: &str) {
        for line in text.lines() {
            let n = normalize(line);
            if distinctive(&n) && self.protected.insert(n) {
                self.dirty = true;
            }
        }
    }

    /// Lines of public text (open files, skeletons) are never redacted.
    pub fn add_public(&mut self, text: &str) {
        for line in text.lines() {
            let n = normalize(line);
            if distinctive(&n) && self.public.insert(n) {
                self.dirty = true;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.protected.is_empty()
    }

    fn matcher(&mut self) -> Option<&AhoCorasick> {
        if self.dirty {
            let patterns: Vec<&String> = self
                .protected
                .iter()
                .filter(|l| !self.public.contains(*l))
                .collect();
            self.matcher = if patterns.is_empty() {
                None
            } else {
                AhoCorasick::new(patterns).ok()
            };
            self.dirty = false;
        }
        self.matcher.as_ref()
    }

    /// Replaces every line of `text` that contains a protected line. Returns the
    /// text and the number of lines replaced.
    pub fn redact(&mut self, text: &str) -> (String, usize) {
        let Some(m) = self.matcher() else {
            return (text.to_owned(), 0);
        };
        let mut out = String::with_capacity(text.len());
        let mut hits = 0;
        for line in text.split_inclusive('\n') {
            let body = line.trim_end_matches(['\n', '\r']);
            let norm = normalize(body);
            if norm.chars().count() >= MIN_LINE_CHARS && m.is_match(&norm) {
                hits += 1;
                let indent = &body[..body.len() - body.trim_start().len()];
                out.push_str(indent);
                out.push_str(LINE_WITHHELD);
                out.push_str(&line[body.len()..]);
            } else {
                out.push_str(line);
            }
        }
        (out, hits)
    }
}

static RESULT: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // Exit status as rendered by the command runner.
        r"^(exit code -?\d+|timed out after \d+s|terminated by a signal)$",
        r"^\$ .{1,200}$",
        // Test harness outcomes (libtest, TAP, jest-like, pytest-like).
        r"^test \S+ \.\.\. (ok|FAILED|ignored)$",
        r"^test result: ",
        r"^running \d+ tests?$",
        r"^failures:$",
        r"^---- \S+ stdout ----$",
        r"^\s{4}[\w:]+$",
        r"^(ok|not ok) \d+ ",
        r"^\s*(PASS|FAIL|✓|✕|✗|×) ",
        r"^(FAILED|ERROR|PASSED) \S+",
        r"^=+ .*\b(passed|failed|error|errors)\b.* =+$",
        r"^Tests?:\s+.*\b(passed|failed|total)\b",
        // Compiler diagnostics: header and location only.
        r"^(error|warning)(\[[A-Z]\d{4}\])?: ",
        r"^\s*--> \S+:\d+:\d+$",
        r"^\s*File .+, line \d+",
        r"^\S+\(\d+,\d+\): error TS\d+: ",
        // Panics and assertions.
        r"^thread '.*' panicked at \S+:\d+:\d+:?",
        r"^\s*(left|right)\s*: ",
        r"^assertion (`.+` )?failed",
        r"^\s*[A-Z][A-Za-z]*(Error|Exception): ",
        r"^E\s{2,}\S",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("static regex"))
    .collect()
});

static PANIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^thread '.*' panicked at \S+:\d+:\d+:$").expect("static regex"));

fn clip(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let cut: String = line.chars().take(max).collect();
    format!("{cut}…")
}

/// The recognised result lines of `output` (with the line after a panic
/// header, which holds its message), and how many other lines were withheld.
pub fn result_lines(output: &str) -> (Vec<String>, usize) {
    let mut kept = Vec::new();
    let mut withheld = 0;
    let mut after_panic = false;
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if after_panic {
            after_panic = false;
            kept.push(clip(line, MAX_MESSAGE_CHARS));
            continue;
        }
        if RESULT.iter().any(|r| r.is_match(line)) {
            after_panic = PANIC.is_match(line);
            let max = if line.trim_start().starts_with("left")
                || line.trim_start().starts_with("right")
            {
                MAX_MESSAGE_CHARS
            } else {
                MAX_RESULT_CHARS
            };
            kept.push(clip(line, max));
        } else {
            withheld += 1;
        }
    }
    if kept.len() > MAX_RESULT_LINES {
        withheld += kept.len() - MAX_RESULT_LINES;
        kept.truncate(MAX_RESULT_LINES);
    }
    (kept, withheld)
}

static STRING_LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""((?:[^"\\\n]|\\.){8,200})"|'((?:[^'\\\n]|\\.){8,200})'"#).expect("static regex")
});
static CODE_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9_]{10,}\b").expect("static regex"));
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}_]+").expect("static regex"));

/// String literals and long mixed letter/digit tokens of withheld text whose
/// words are not all public: fragments that identify protected code.
pub fn distinctive_literals(text: &str, public_words: &HashSet<String>) -> Vec<String> {
    let novel = |s: &str| {
        WORD.find_iter(s)
            .any(|w| !public_words.contains(&w.as_str().to_lowercase()))
    };
    let mut out = BTreeSet::new();
    for c in STRING_LITERAL.captures_iter(text) {
        if let Some(v) = c.get(1).or_else(|| c.get(2))
            && novel(v.as_str())
        {
            out.insert(v.as_str().to_owned());
        }
    }
    for m in CODE_TOKEN.find_iter(text) {
        let t = m.as_str();
        if t.bytes().any(|b| b.is_ascii_digit())
            && t.bytes().any(|b| b.is_ascii_alphabetic())
            && novel(t)
        {
            out.insert(t.to_owned());
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "    let uplift = base_rate * 0.8731 + seasonal(region);\n    \
        let floor = max(uplift, MIN_FLOOR_CENTS);\n    }\n    return floor;\n";

    #[test]
    fn lines_containing_protected_code_are_withheld() {
        let mut idx = ProtectedLines::default();
        idx.add_protected(BODY);
        let rustc = "error[E0308]: mismatched types\n  --> src/pricing/engine.rs:12:5\n\
            12 |     let uplift = base_rate *   0.8731 + seasonal(region);\n   |     ^^^^\n";
        let (out, n) = idx.redact(rustc);
        assert_eq!(n, 1, "{out}");
        assert!(
            !out.contains("0.8731") && out.contains("mismatched types"),
            "{out}"
        );
        // Short and common lines are never protected.
        let (same, n) = idx.redact("    return floor;\n}\n");
        assert_eq!((same.as_str(), n), ("    return floor;\n}\n", 0));
        // A protected line that is also public (e.g. in a skeleton) is exempt.
        idx.add_public("let floor = max(uplift, MIN_FLOOR_CENTS);");
        assert_eq!(
            idx.redact("x let floor = max(uplift, MIN_FLOOR_CENTS);").1,
            0
        );
    }

    #[test]
    fn only_result_shaped_lines_survive() {
        let out = "exit code 101\n--- stdout ---\nrunning 3 tests\n\
            test pricing::tests::breaks ... ok\ntest invoice::pooled ... FAILED\n\
            let secret = base * 0.8731; // printed by a test\n\
            ---- invoice::pooled stdout ----\n\
            thread 'invoice::pooled' panicked at tests/hidden.rs:40:5:\n\
            assertion `left == right` failed\n  left: 1180\n right: 1500\n\
            error[E0425]: cannot find value `rate` in this scope\n   --> src/invoice.rs:9:13\n\
            test result: FAILED. 1 passed; 1 failed; 0 ignored\n";
        let (kept, withheld) = result_lines(out);
        let joined = kept.join("\n");
        for want in [
            "exit code 101",
            "test invoice::pooled ... FAILED",
            "thread 'invoice::pooled' panicked at tests/hidden.rs:40:5:",
            "assertion `left == right` failed",
            "  left: 1180",
            "error[E0425]: cannot find value `rate` in this scope",
            "   --> src/invoice.rs:9:13",
            "test result: FAILED. 1 passed; 1 failed; 0 ignored",
        ] {
            assert!(joined.contains(want), "missing {want:?}:\n{joined}");
        }
        assert!(!joined.contains("0.8731"), "{joined}");
        assert_eq!(withheld, 2, "{joined}");
    }

    #[test]
    fn distinctive_literals_skip_public_words() {
        let public: HashSet<String> = ["discount", "exceeds", "maximum"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let text = r#"let tag = "zq4k2m9x8v7c6b5n"; let e = "discount exceeds maximum"; let k = rate_v2_2026x;"#;
        let lits = distinctive_literals(text, &public);
        assert!(lits.contains(&"zq4k2m9x8v7c6b5n".to_owned()), "{lits:?}");
        assert!(lits.contains(&"rate_v2_2026x".to_owned()), "{lits:?}");
        assert!(!lits.iter().any(|l| l.contains("discount")), "{lits:?}");
    }
}
