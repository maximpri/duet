// SPDX-License-Identifier: GPL-3.0-or-later
//! Public but bulky results (`PublicBulky`): the content is kept whole under a
//! handle, and the frontier gets a deterministic preview — the first lines and
//! an outline — from which it reads further ranges with `read_raw`. A large
//! result would otherwise be re-sent with every later request of the run.
//!
//! Everything here is plain text processing; the engine sanitizes the preview
//! and adds a local summary when a local model is configured.

use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// Lines shown from the start of the content.
pub const HEAD_LINES: usize = 40;
/// Lines shown from the end of command output (where results and summaries are).
pub const TAIL_LINES: usize = 20;
/// Outline entries shown at most.
pub const MAX_OUTLINE: usize = 60;
/// Characters shown of one line.
const LINE_CHARS: usize = 200;

/// Declarations in common languages: Rust, TypeScript/JavaScript, Python, Go, Java/C#.
static DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^\s*(?:",
        // Items, optionally behind visibility and modifiers.
        r"(?:(?:pub(?:\([^)]*\))?|export|default|async|unsafe|extern|abstract|public|private|protected|static|final)\s+)*",
        r"(?:fn|struct|enum|trait|impl|mod|type|union|macro_rules!|class|interface|function|def|func|namespace)\b",
        // Constants (`pub const MAX: usize`) and exported bindings (`export const load =`).
        r"|(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+[A-Z][A-Z0-9_]*\s*:",
        r"|export\s+(?:default\s+)?(?:const|let|var)\b",
        r")",
    ))
    .expect("static regex")
});
static ISSUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(error|errors|panic|panicked|exception|traceback|failed|failures?|fatal|warning|warn)\b|^\s*-->")
        .expect("static regex")
});

/// What kind of text a bulky result is, which decides its outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A file: declarations with line numbers.
    Source,
    /// Command output: error and warning lines, then the last lines.
    Output,
    /// A file listing: file counts per directory.
    Listing,
    /// Search results: match counts per file.
    Matches,
}

/// Conservative token estimate for deciding what is bulky (3 characters per
/// token; code and logs usually tokenize at 3–4).
pub fn tokens(text: &str) -> usize {
    text.chars().count().div_ceil(3)
}

/// Whether `text` is over the threshold (`0` disables offloading).
pub fn is_bulky(text: &str, threshold_tokens: usize) -> bool {
    threshold_tokens > 0 && tokens(text) > threshold_tokens
}

/// Splits text numbered as `read_file` numbers it (`{n:>w}  {line}`, consecutive
/// numbers) into its first line number and the plain lines. `None` if any line
/// is not numbered that way.
pub fn strip_line_numbers(text: &str) -> Option<(usize, String)> {
    let mut first = None;
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        let n: usize = trimmed[..digits].parse().ok()?;
        let rest = &trimmed[digits..];
        let content = rest
            .strip_prefix("  ")
            .or((rest.is_empty()).then_some(""))?;
        let start = *first.get_or_insert(n);
        if n != start + i {
            return None;
        }
        out.push_str(content);
        out.push('\n');
    }
    first.map(|f| (f, out))
}

fn shown(line: &str) -> String {
    let mut s: String = line.chars().take(LINE_CHARS).collect();
    if line.chars().count() > LINE_CHARS {
        s.push('…');
    }
    s
}

fn numbered(n: usize, line: &str) -> String {
    format!("{n:>6}  {}\n", shown(line))
}

/// The preview of `text` (plain lines whose first line is `first_line`).
pub fn preview(text: &str, first_line: usize, shape: Shape) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let head = HEAD_LINES.min(lines.len());
    let mut out = format!("First {head} lines:\n");
    for (i, l) in lines.iter().enumerate().take(head) {
        out.push_str(&numbered(first_line + i, l));
    }
    let rest = &lines[head..];
    if rest.is_empty() {
        return out;
    }
    let after = |i: usize| first_line + head + i;
    match shape {
        Shape::Source => {
            let decls: Vec<(usize, &str)> = rest
                .iter()
                .enumerate()
                .filter(|(_, l)| DECLARATION.is_match(l))
                .map(|(i, l)| (after(i), *l))
                .collect();
            push_listed(&mut out, "Declarations after that", &decls);
        }
        Shape::Output => {
            let tail_from = rest.len().saturating_sub(TAIL_LINES);
            let issues: Vec<(usize, &str)> = rest[..tail_from]
                .iter()
                .enumerate()
                .filter(|(_, l)| ISSUE.is_match(l))
                .map(|(i, l)| (after(i), *l))
                .collect();
            push_listed(&mut out, "Error and warning lines after that", &issues);
            let tail: Vec<(usize, &str)> = rest[tail_from..]
                .iter()
                .enumerate()
                .map(|(i, l)| (after(tail_from + i), *l))
                .collect();
            push_listed(&mut out, "Last lines", &tail);
        }
        Shape::Listing => {
            let counts = count_by(&lines, |l| match l.split_once('/') {
                Some((dir, _)) => format!("{dir}/"),
                None => "(top level)".to_owned(),
            });
            push_counts(&mut out, "Files per top-level directory", &counts);
        }
        Shape::Matches => {
            let counts = count_by(&lines, |l| match l.split_once(':') {
                Some((path, _)) => path.to_owned(),
                None => "(other)".to_owned(),
            });
            push_counts(&mut out, "Matches per file", &counts);
        }
    }
    out
}

fn push_listed(out: &mut String, title: &str, entries: &[(usize, &str)]) {
    if entries.is_empty() {
        return;
    }
    out.push_str(&format!("{title}:\n"));
    for (n, l) in entries.iter().take(MAX_OUTLINE) {
        out.push_str(&numbered(*n, l));
    }
    if entries.len() > MAX_OUTLINE {
        out.push_str(&format!("  … {} more\n", entries.len() - MAX_OUTLINE));
    }
}

fn count_by(lines: &[&str], key: impl Fn(&str) -> String) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        *m.entry(key(l)).or_insert(0) += 1;
    }
    m
}

fn push_counts(out: &mut String, title: &str, counts: &BTreeMap<String, usize>) {
    out.push_str(&format!(
        "{title} (all {} lines):\n",
        counts.values().sum::<usize>()
    ));
    for (k, n) in counts.iter().take(MAX_OUTLINE) {
        out.push_str(&format!("  {} ({n})\n", shown(k)));
    }
    if counts.len() > MAX_OUTLINE {
        out.push_str(&format!("  … {} more\n", counts.len() - MAX_OUTLINE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_conservative_and_can_be_disabled() {
        let text = "x".repeat(6003);
        assert_eq!(tokens(&text), 2001);
        assert!(is_bulky(&text, 2000));
        assert!(!is_bulky(&"x".repeat(6000), 2000));
        assert!(!is_bulky(&text, 0));
    }

    #[test]
    fn strips_read_file_numbering() {
        assert_eq!(
            strip_line_numbers("  9  a\n 10  \n 11    b\n"),
            Some((9, "a\n\n  b\n".to_owned()))
        );
        assert_eq!(strip_line_numbers("1  a\n3  b\n"), None, "gap");
        assert_eq!(strip_line_numbers("plain text\n"), None);
    }

    #[test]
    fn source_outline_lists_declarations_after_the_head() {
        let mut text = String::new();
        for i in 0..200 {
            match i {
                100 => text.push_str("pub fn parse_ledger(input: &str) -> Ledger {\n"),
                150 => text.push_str("impl Display for Ledger {\n"),
                170 => text.push_str("export async function load() {\n"),
                180 => text.push_str("    def helper(self):\n"),
                _ => text.push_str("    let x = 1;\n"),
            }
        }
        let p = preview(&text, 1, Shape::Source);
        assert!(p.starts_with("First 40 lines:\n     1  "), "{p}");
        assert!(
            !p.contains("\n    41  "),
            "only the head is shown whole: {p}"
        );
        for line in [
            "   101  pub fn parse_ledger",
            "   151  impl Display",
            "   171  export async function load",
            "   181      def helper",
        ] {
            assert!(p.contains(line), "{line} missing: {p}");
        }
        assert_eq!(p.matches("let x").count(), 40);
    }

    #[test]
    fn output_outline_keeps_errors_and_the_tail() {
        let mut text = String::new();
        for i in 0..300 {
            match i {
                120 => text.push_str("error[E0308]: mismatched types\n"),
                121 => text.push_str("  --> src/lib.rs:4:5\n"),
                299 => text.push_str("test result: FAILED. 3 passed; 1 failed\n"),
                _ => text.push_str(&format!("compiling unit {i}\n")),
            }
        }
        let p = preview(&text, 1, Shape::Output);
        assert!(p.contains("   121  error[E0308]"), "{p}");
        assert!(p.contains("   122    --> src/lib.rs:4:5"), "{p}");
        assert!(p.contains("   300  test result: FAILED"), "{p}");
        assert!(
            p.contains("   281  compiling unit 280"),
            "tail starts 20 from the end: {p}"
        );
        assert!(!p.contains("compiling unit 200\n"), "{p}");
    }

    #[test]
    fn listings_and_matches_are_counted() {
        let mut list = String::new();
        for i in 0..100 {
            list.push_str(&format!("src/m{i}.rs\n"));
        }
        list.push_str("tests/a.rs\nREADME.md\n");
        let p = preview(&list, 1, Shape::Listing);
        assert!(
            p.contains("  src/ (100)") && p.contains("  tests/ (1)"),
            "{p}"
        );
        assert!(p.contains("(top level) (1)"), "{p}");
        let matches = "src/a.rs:1: x\n".repeat(30) + &"src/b.rs:9: x\n".repeat(20);
        let p = preview(&matches, 1, Shape::Matches);
        assert!(
            p.contains("  src/a.rs (30)") && p.contains("  src/b.rs (20)"),
            "{p}"
        );
    }
}

/// The class as the security engine applies it.
#[cfg(test)]
mod engine_tests {
    use crate::engine::Engine;
    use crate::local::LocalReader;
    use crate::policy::Policy;
    use crate::view::{Presenter, Source, ViewClass};
    use duet_provider::client::{HttpReply, Transport};
    use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
    use futures_util::StreamExt;
    use futures_util::future::BoxFuture;
    use serde_json::{Map, Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;

    const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";
    const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";

    fn policy() -> Policy {
        Policy {
            sensitive_globs: vec!["data/**".into(), "*.log".into()],
            command_output_sensitive: true,
            raw_ok_commands: vec!["cargo check".into()],
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            bulky_tokens: 2000,
            bulky_file_tokens: 2000,
            ..Policy::default()
        }
    }

    fn engine(local: Option<LocalReader>) -> (tempfile::TempDir, Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::open(d.path(), policy(), local).unwrap();
        (d, e)
    }

    /// `n` lines numbered as `read_file` numbers them, from `first`.
    fn numbered(lines: &[String], first: usize) -> String {
        let width = (first + lines.len()).to_string().len();
        lines
            .iter()
            .enumerate()
            .map(|(i, l)| format!("{:>width$}  {l}\n", first + i))
            .collect()
    }

    fn source_lines(n: usize) -> Vec<String> {
        (1..=n)
            .map(|i| match i {
                3 => format!("const KEY: &str = \"{KEY}\";"),
                150 => "pub fn reconcile(ledger: &Ledger) -> Report {".to_owned(),
                250 => format!("    // contact {EMAIL} for access"),
                _ => format!("    let value_{i} = compute(input, {i});"),
            })
            .collect()
    }

    fn file(path: &str, ranged: bool) -> Source {
        Source::File {
            path: PathBuf::from(path),
            ranged,
        }
    }

    fn args(v: Value) -> Map<String, Value> {
        let Value::Object(m) = v else { unreachable!() };
        m
    }

    fn read_raw(e: &Engine, v: Value) -> Result<String, String> {
        e.call_tool("read_raw", &args(v)).unwrap()
    }

    #[test]
    fn small_results_are_unchanged() {
        let (_d, e) = engine(None);
        let text = numbered(&source_lines(20), 1);
        let shown = e.present(&file("src/lib.rs", false), text.as_bytes());
        assert!(shown.starts_with(" 1  "), "{shown}");
        assert_eq!(shown.lines().count(), 20);
        assert!(!shown.contains(KEY), "{shown}");
        assert_eq!(e.take_view_class(), Some(ViewClass::Raw));
        assert_eq!(e.take_view_class(), None, "taken once");
    }

    #[test]
    fn a_bulky_file_becomes_a_handle_with_head_and_outline() {
        let (_d, e) = engine(None);
        let text = numbered(&source_lines(300), 1);
        let shown = e.present(&file("src/ledger.rs", false), text.as_bytes());
        assert_eq!(e.take_view_class(), Some(ViewClass::BulkyHandle));
        assert!(
            shown.starts_with("h1 (src/ledger.rs): lines 1-300"),
            "{shown}"
        );
        assert!(shown.contains("read_raw(handle=\"h1\""), "{shown}");
        assert!(shown.contains("    40      let value_40"), "head: {shown}");
        assert!(!shown.contains("value_41 "), "only the head: {shown}");
        assert!(
            shown.contains("   150  pub fn reconcile(ledger: &Ledger)"),
            "outline: {shown}"
        );
        assert!(!shown.contains(KEY), "{shown}");
        assert!(
            shown.len() < text.len() / 4,
            "{} vs {}",
            shown.len(),
            text.len()
        );
        assert!(!shown.contains("Summary by the local model"));

        // Ranges use the file's own line numbers and stay sanitized.
        let raw = read_raw(
            &e,
            json!({"handle": "h1", "start_line": 248, "end_line": 251}),
        )
        .unwrap();
        assert_eq!(e.take_view_class(), Some(ViewClass::Raw));
        assert!(
            raw.starts_with("h1 (src/ledger.rs), lines 248-251 of 1-300:\n"),
            "{raw}"
        );
        assert!(raw.contains("   249      let value_249"), "{raw}");
        assert!(raw.contains("   250      // contact"), "{raw}");
        assert!(!raw.contains(EMAIL), "{raw}");
        let head = read_raw(&e, json!({"handle": "h1", "end_line": 3})).unwrap();
        assert!(
            head.contains("     3  const KEY") && !head.contains(KEY),
            "{head}"
        );
    }

    #[test]
    fn a_ranged_read_is_shown_and_keeps_its_line_numbers_when_offloaded() {
        let (_d, e) = engine(None);
        let lines = source_lines(900);
        // 300 requested lines: shown, although over the token threshold.
        let range = numbered(&lines[99..399], 100);
        let shown = e.present(&file("src/ledger.rs", true), range.as_bytes());
        assert_eq!(e.take_view_class(), Some(ViewClass::Raw));
        assert!(shown.contains("value_398"), "{shown}");
        // 800 requested lines: more than one read_raw call returns.
        let range = numbered(&lines[99..899], 100);
        let shown = e.present(&file("src/ledger.rs", true), range.as_bytes());
        assert!(
            shown.starts_with("h1 (src/ledger.rs): lines 100-899"),
            "{shown}"
        );
        assert!(shown.contains("   100      let value_100"), "{shown}");
        let raw = read_raw(&e, json!({"handle": "h1", "start_line": 890})).unwrap();
        assert!(raw.contains("lines 890-899 of 100-899"), "{raw}");
        assert!(read_raw(&e, json!({"handle": "h1", "start_line": 950})).is_err());
        assert!(
            read_raw(
                &e,
                json!({"handle": "h1", "start_line": 300, "end_line": 200})
            )
            .is_err()
        );
        let capped = read_raw(
            &e,
            json!({"handle": "h1", "start_line": 100, "end_line": 899}),
        )
        .unwrap();
        assert!(capped.contains("continue from line 600"), "{capped}");
        assert!(!capped.contains("value_600"), "{capped}");
    }

    #[test]
    fn public_command_output_keeps_errors_and_tail_sensitive_output_does_not_change() {
        let (_d, e) = engine(None);
        let mut out = String::from("exit code 101\n--- stderr ---\n");
        for i in 0..400 {
            out.push_str(&format!("   Compiling crate_{i} v0.1.0\n"));
            if i == 200 {
                out.push_str(&format!("error: token {KEY} rejected\n"));
            }
        }
        out.push_str("error: could not compile `fx` due to 1 previous error\n");
        let shown = e.present(
            &Source::Command {
                command: "cargo check".into(),
                exit_code: Some(101),
            },
            out.as_bytes(),
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::BulkyHandle));
        assert!(shown.starts_with("h1 (output of `cargo check`)"), "{shown}");
        assert!(shown.contains("error: token"), "{shown}");
        assert!(!shown.contains(KEY), "{shown}");
        assert!(shown.contains("could not compile `fx`"), "tail: {shown}");
        assert!(!shown.contains("crate_150 "), "{shown}");

        // Output of other commands is still sensitive: a summary handle, no raw reads.
        let held = e.present(
            &Source::Command {
                command: "cargo test".into(),
                exit_code: Some(101),
            },
            out.as_bytes(),
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::HandleSummary));
        assert!(held.starts_with("h2 (output of `cargo test`)") && held.contains("ask_local"));
        assert!(
            read_raw(&e, json!({"handle": "h2"}))
                .unwrap_err()
                .contains("sensitive")
        );
    }

    #[test]
    fn bulky_search_keeps_sensitive_matches_masked() {
        let (_d, e) = engine(None);
        let mut text = String::new();
        for i in 0..150 {
            text.push_str(&format!(
                "src/m{}.rs:{i}: let total = amount * rate;\n",
                i % 7
            ));
            text.push_str(&format!("data/customers.csv:{i}: C-{i},{EMAIL},EUR\n"));
        }
        let shown = e.present(
            &Source::Search {
                pattern: "total".into(),
            },
            text.as_bytes(),
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::BulkyHandle));
        assert!(shown.starts_with("h1 (search for `total`)"), "{shown}");
        assert!(shown.contains("  data/customers.csv (150)"), "{shown}");
        let raw = read_raw(
            &e,
            json!({"handle": "h1", "start_line": 1, "end_line": 300}),
        )
        .unwrap();
        assert!(!raw.contains(EMAIL) && !raw.contains(",EUR"), "{raw}");
        assert!(raw.contains("[match in sensitive content"), "{raw}");
    }

    #[test]
    fn a_bulky_listing_is_counted_per_directory() {
        let (_d, e) = engine(None);
        let list: String = (0..700)
            .map(|i| format!("src/module_{i}/mod.rs\n"))
            .collect();
        let shown = e.present(&Source::FileList, list.as_bytes());
        assert!(shown.starts_with("h1 (file list)"), "{shown}");
        assert!(shown.contains("  src/ (700)"), "{shown}");
    }

    #[test]
    fn the_task_is_never_offloaded() {
        let (_d, e) = engine(None);
        let task = "Refactor the ledger module carefully. ".repeat(500);
        assert!(super::is_bulky(&task, 2000));
        assert!(e.sanitize_objective(&task).starts_with(&task));
    }

    /// A local server that answers every request with `content`.
    struct Replies(String);

    impl Transport for Replies {
        fn post(
            &self,
            _url: String,
            _headers: Vec<(String, String)>,
            _body: Vec<u8>,
        ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
            let delta = json!({"choices": [{"delta": {"content": self.0}}]});
            let chunks = vec![
                format!("data: {delta}\n\n"),
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n".to_owned(),
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":900,\"completion_tokens\":40}}\n\n"
                    .to_owned(),
                "data: [DONE]\n\n".to_owned(),
            ];
            Box::pin(async move {
                Ok(HttpReply {
                    status: 200,
                    headers: vec![],
                    body: futures_util::stream::iter(
                        chunks.into_iter().map(|c| Ok(bytes::Bytes::from(c))),
                    )
                    .boxed(),
                })
            })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_local_summary_is_added_and_sanitized() {
        let reply = json!({"summary": "Reconciliation code; the entry point is reconcile().",
                           "facts": [format!("maintainer {EMAIL}")]})
        .to_string();
        let cfg = ProviderConfig::new(
            "http://127.0.0.1:9/v1",
            "local",
            Role::Local {
                allowlist: vec![],
                allow_plaintext: false,
            },
        );
        let reader = LocalReader::new(ChatProvider::new(cfg, Box::new(Replies(reply))).unwrap());
        let (_d, e) = engine(Some(reader));
        // Bulky public command output gets a local summary ...
        let output: String = (0..900)
            .map(|i| format!("   Compiling crate_{i} v0.1.0\n"))
            .collect();
        let shown = e.present(
            &Source::Command {
                command: "cargo check".into(),
                exit_code: Some(0),
            },
            output.as_bytes(),
        );
        assert!(
            shown.contains("Summary by the local model: Reconciliation code"),
            "{shown}"
        );
        // ... bulky source only its outline: reading a range beats a summary.
        let text = numbered(&source_lines(300), 1);
        let source = e.present(&file("src/ledger.rs", false), text.as_bytes());
        assert!(
            source.contains("too long to show whole")
                && !source.contains("Summary by the local model"),
            "{source}"
        );
        assert!(
            shown.contains("- maintainer ") && !shown.contains(EMAIL),
            "{shown}"
        );
        // Listings get no local summary: their outline is exact.
        let list: String = (0..700)
            .map(|i| format!("src/module_{i}/mod.rs\n"))
            .collect();
        let shown = e.present(&Source::FileList, list.as_bytes());
        assert!(!shown.contains("Summary by the local model"), "{shown}");
    }
}
