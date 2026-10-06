// SPDX-License-Identifier: GPL-3.0-or-later
//! Condensed command output (`context.condense_output`): what a test run, a
//! build or an install printed, reduced to what the reader acts on.
//!
//! Deterministic and line by line. Each recognized format's rules mark lines
//! kept or omitted, and why; a run of omitted lines becomes one marker naming
//! the lines and what they held, so any of them can be read back from the
//! handle the whole output is kept under. Kept: failing tests with their
//! assertion messages, the first diagnostic of each kind with its location
//! and a few lines of context (later ones of the kind by location and label),
//! summary counts, exit codes, and every line no rule recognizes. Omitted:
//! passing tests, build, download and progress lines, repeated warnings, the
//! middle of long failure output and deep stack traces.
//!
//! Output in no recognized format is not condensed, nor is output the
//! condensed view would not make much shorter: [`condense`] returns `None`
//! and the caller shows it as before. The engine condenses only output the
//! frontier may already see (never output held as sensitive), and sanitizes
//! the whole output before condensing it.

use crate::bulky;
use regex::Regex;
use std::collections::BTreeMap;
use std::ops::Range;

mod generic;
mod go;
mod js;
mod jvm;
mod packages;
mod python;
mod rust;

/// Output shorter than this (estimated tokens) is shown as it is.
pub const MIN_TOKENS: usize = 300;
/// A condensed view (with its handle line and note) is used only when it is
/// at most this share of the original, in percent.
pub const MAX_PERCENT: usize = 70;
/// Estimated tokens of the handle line and the closing note.
const NOTE_TOKENS: usize = 45;
/// Characters shown of one kept line.
const LINE_CHARS: usize = 400;
/// Lines kept inside a block beyond its head and tail because they carry
/// the message (an assertion, an expected/actual pair).
const MAX_IMPORTANT: usize = 20;

macro_rules! re {
    ($(#[$doc:meta])* $name:ident, $pattern:expr) => {
        $(#[$doc])*
        static $name: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new($pattern).expect("static regex"));
    };
}
pub(crate) use re;

/// Why a line was omitted; the marker counts lines per reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Why {
    Passing,
    Skipped,
    Build,
    Progress,
    Repeat,
    Detail,
    Frames,
    Routine,
}

impl Why {
    fn label(self, n: usize) -> String {
        let (one, many) = match self {
            Why::Passing => ("passing test", "passing tests"),
            Why::Skipped => ("skipped test", "skipped tests"),
            Why::Build => ("build or download line", "build and download lines"),
            Why::Progress => ("progress line", "progress lines"),
            Why::Repeat => ("repeated line", "repeated lines"),
            Why::Detail => ("detail line", "detail lines"),
            Why::Frames => ("stack frame", "stack frames"),
            Why::Routine => ("routine line", "routine lines"),
        };
        format!("{n} {}", if n == 1 { one } else { many })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// No rule decided: shown.
    Unset,
    Keep,
    Omit(Why),
}

/// Lines kept at the start and the end of a block.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cap {
    pub head: usize,
    pub tail: usize,
}

/// The output's lines and what the rules decided about each.
pub(crate) struct Plan {
    /// The lines as shown: escape sequences removed, a line rewritten in
    /// place (`\r`) reduced to its last state.
    lines: Vec<String>,
    marks: Vec<Mark>,
    formats: Vec<&'static str>,
    /// Lines a rule adds after an original line (a sum of omitted results).
    notes: BTreeMap<usize, Vec<String>>,
}

re!(
    ANSI,
    r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]"
);
re!(
    /// Lines `run_command` and the checks write around the output itself,
    /// and the sandbox's notes (a refused download, a removed `.git`).
    STRUCTURE,
    r"^(?:exit code -?\d+|timed out after \S+|terminated by a signal|--- (?:stdout|stderr) ---|\[(?:stdout|stderr) truncated: .*\]|\$ .+|could not run: .*|\[sandbox\] .*)$"
);

impl Plan {
    fn new(text: &str) -> Self {
        let lines: Vec<String> = text
            .lines()
            .map(|l| {
                let l = l.rsplit('\r').find(|s| !s.is_empty()).unwrap_or("");
                ANSI.replace_all(l, "").into_owned()
            })
            .collect();
        let marks = vec![Mark::Unset; lines.len()];
        Self {
            lines,
            marks,
            formats: Vec::new(),
            notes: BTreeMap::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn line(&self, i: usize) -> &str {
        &self.lines[i]
    }

    pub(crate) fn blank(&self, i: usize) -> bool {
        self.lines[i].trim().is_empty()
    }

    pub(crate) fn is_unset(&self, i: usize) -> bool {
        self.marks[i] == Mark::Unset
    }

    /// Records that a format was recognized (for the view's header).
    pub(crate) fn found(&mut self, format: &'static str) {
        if !self.formats.contains(&format) {
            self.formats.push(format);
        }
    }

    pub(crate) fn any_found(&self) -> bool {
        !self.formats.is_empty()
    }

    /// Adds `text` (in brackets: it is not the tool's own output) after line `after`.
    pub(crate) fn note(&mut self, after: usize, text: String) {
        self.notes
            .entry(after)
            .or_default()
            .push(format!("[{text}]"));
    }

    /// Keeps line `i` unless a rule already decided it.
    pub(crate) fn keep(&mut self, i: usize) {
        if self.is_unset(i) {
            self.marks[i] = Mark::Keep;
        }
    }

    /// Omits line `i` unless a rule already decided it.
    pub(crate) fn omit(&mut self, i: usize, why: Why) {
        if self.is_unset(i) {
            self.marks[i] = Mark::Omit(why);
        }
    }

    /// Keeps line `i` whatever was decided (a header whose section turned
    /// out to hold a failure).
    pub(crate) fn force_keep(&mut self, i: usize) {
        self.marks[i] = Mark::Keep;
    }

    /// Omits every undecided line of `range`.
    pub(crate) fn omit_all(&mut self, range: Range<usize>, why: Why) {
        for i in range {
            if !self.blank(i) {
                self.omit(i, why);
            }
        }
    }

    /// A block (a failure's output, a diagnostic): its first and last lines
    /// are kept, and between them the lines `important` picks, up to
    /// [`MAX_IMPORTANT`]; the rest are detail. Decided lines stay as decided.
    pub(crate) fn block(
        &mut self,
        range: Range<usize>,
        cap: Cap,
        important: &dyn Fn(&str) -> bool,
    ) {
        let (start, end) = (range.start, range.end);
        let mut extra = 0;
        for i in range {
            if !self.is_unset(i) || self.blank(i) {
                continue;
            }
            if i < start + cap.head || i + cap.tail >= end {
                self.keep(i);
            } else if extra < MAX_IMPORTANT && important(&self.lines[i]) {
                self.keep(i);
                extra += 1;
            } else {
                self.omit(i, Why::Detail);
            }
        }
    }

    /// The end of the block starting at `start`: the first line after it
    /// for which `ends` holds, or the end of the output.
    pub(crate) fn until(&self, start: usize, ends: impl Fn(&str) -> bool) -> usize {
        (start + 1..self.len())
            .find(|&i| ends(&self.lines[i]))
            .unwrap_or(self.len())
    }

    /// Lines of `range` matching `re`.
    pub(crate) fn matching(&self, range: Range<usize>, re: &Regex) -> Vec<usize> {
        range.filter(|&i| re.is_match(&self.lines[i])).collect()
    }
}

re!(
    /// JavaScript and JVM stack frames; the first frame in the project's own code
    /// is kept, the rest omitted.
    JS_FRAME, r"^\s+at \S");
re!(
    /// Frames and paths in dependencies and the runtime rather than the project.
    DEPENDENCY,
    r"node_modules|node:internal|\(internal/|<anonymous>\)$|site-packages|dist-packages|/rustlib/|\.cargo/registry|/usr/lib/|java\.base/|jdk\.internal|org\.junit\.|org\.apache\.maven|sun\.reflect|java\.lang\.reflect|java\.util\.|kotlin\.|org\.gradle\."
);

/// Omits JavaScript/JVM stack frames in `range` but, in each trace (a run of
/// frames), the first one in the project's own code.
pub(crate) fn js_frames(plan: &mut Plan, range: Range<usize>) {
    let mut kept = false;
    for i in range {
        if !JS_FRAME.is_match(plan.line(i)) {
            kept = false;
            continue;
        }
        if !plan.is_unset(i) {
            continue;
        }
        if !kept && !DEPENDENCY.is_match(plan.line(i)) {
            plan.keep(i);
            kept = true;
        } else {
            plan.omit(i, Why::Frames);
        }
    }
}

re!(PY_FRAME, r#"^\s*File "[^"]+", line \d+"#);

/// Omits the frames of a Python traceback in `range` but the last two (where
/// it failed); a frame is its `File "…", line N` line and the source under it.
pub(crate) fn python_frames(plan: &mut Plan, range: Range<usize>) {
    let starts = plan.matching(range.clone(), &PY_FRAME);
    if starts.len() <= 2 {
        return;
    }
    let indent = |l: &str| l.len() - l.trim_start().len();
    for (k, &s) in starts.iter().enumerate().take(starts.len() - 2) {
        let depth = indent(plan.line(s));
        let next = starts.get(k + 1).copied().unwrap_or(range.end);
        plan.omit(s, Why::Frames);
        for i in s + 1..next {
            if plan.blank(i) || indent(plan.line(i)) <= depth {
                break;
            }
            plan.omit(i, Why::Frames);
        }
    }
}

/// One line of the condensed view and the original lines it stands for.
struct Row {
    text: String,
    first: usize,
    last: usize,
}

/// A run of omitted lines being collected.
struct Gap {
    first: usize,
    last: usize,
    counts: BTreeMap<Why, usize>,
}

impl Gap {
    /// The marker standing for the run, or the run's lines themselves when
    /// they are shorter than the marker.
    fn rows(self, lines: &[String]) -> Vec<Row> {
        let what: Vec<String> = self.counts.iter().map(|(w, n)| w.label(*n)).collect();
        let span = if self.first == self.last {
            format!("line {}", self.first + 1)
        } else {
            format!("lines {}-{}", self.first + 1, self.last + 1)
        };
        let marker = format!("[{span} omitted: {}]", what.join(", "));
        let omitted: Vec<usize> = (self.first..=self.last)
            .filter(|&i| !lines[i].trim().is_empty())
            .collect();
        let size: usize = omitted.iter().map(|&i| lines[i].len() + 1).sum();
        if size <= marker.len() + 1 {
            return omitted
                .into_iter()
                .map(|i| Row {
                    text: shown(&lines[i]),
                    first: i,
                    last: i,
                })
                .collect();
        }
        vec![Row {
            text: marker,
            first: self.first,
            last: self.last,
        }]
    }
}

fn shown(line: &str) -> String {
    let n = line.chars().count();
    if n <= LINE_CHARS {
        return line.to_owned();
    }
    let head: String = line.chars().take(LINE_CHARS).collect();
    format!("{head}… [+{} chars]", n - LINE_CHARS)
}

impl Plan {
    fn render(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = Vec::new();
        let mut gap: Option<Gap> = None;
        let mut pending_blank: Option<usize> = None;
        // Repeats of the previous shown line: three or more in a row become
        // the line and a count.
        let mut repeats = 0;
        let flush_repeats = |rows: &mut Vec<Row>, repeats: &mut usize| {
            match *repeats {
                0 => {}
                1 => {
                    let r = rows.last().expect("a repeated row");
                    let again = Row {
                        text: r.text.clone(),
                        first: r.last,
                        last: r.last,
                    };
                    rows.last_mut().expect("a repeated row").last -= 1;
                    rows.push(again);
                }
                n => {
                    if let Some(r) = rows.last_mut() {
                        r.text.push_str(&format!(" [×{}]", n + 1));
                    }
                }
            }
            *repeats = 0;
        };
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0
                && let Some(notes) = self.notes.get(&(i - 1))
            {
                flush_repeats(&mut rows, &mut repeats);
                if let Some(g) = gap.take() {
                    rows.extend(g.rows(&self.lines));
                }
                pending_blank = None;
                rows.extend(notes.iter().map(|n| Row {
                    text: n.clone(),
                    first: i - 1,
                    last: i - 1,
                }));
            }
            match self.marks[i] {
                Mark::Omit(why) => {
                    pending_blank = None;
                    let g = gap.get_or_insert(Gap {
                        first: i,
                        last: i,
                        counts: BTreeMap::new(),
                    });
                    g.last = i;
                    *g.counts.entry(why).or_insert(0) += 1;
                }
                // Blank lines separate what is shown; next to an omitted run
                // they are part of it.
                _ if self.blank(i) => match &mut gap {
                    Some(g) => g.last = i,
                    None => {
                        if !rows.is_empty() {
                            pending_blank.get_or_insert(i);
                        }
                    }
                },
                _ => {
                    if let Some(g) = gap.take() {
                        flush_repeats(&mut rows, &mut repeats);
                        rows.extend(g.rows(&self.lines));
                    }
                    if let Some(b) = pending_blank.take() {
                        flush_repeats(&mut rows, &mut repeats);
                        rows.push(Row {
                            text: String::new(),
                            first: b,
                            last: b,
                        });
                    }
                    let text = shown(line);
                    match rows.last_mut() {
                        Some(prev) if prev.text == text && !text.is_empty() => {
                            prev.last = i;
                            repeats += 1;
                        }
                        _ => {
                            flush_repeats(&mut rows, &mut repeats);
                            rows.push(Row {
                                text,
                                first: i,
                                last: i,
                            });
                        }
                    }
                }
            }
        }
        flush_repeats(&mut rows, &mut repeats);
        if let Some(g) = gap.take() {
            rows.extend(g.rows(&self.lines));
        }
        let last = self.lines.len().saturating_sub(1);
        if let Some(notes) = self.notes.get(&last) {
            rows.extend(notes.iter().map(|n| Row {
                text: n.clone(),
                first: last,
                last,
            }));
        }
        rows
    }
}

/// Keeps the condensed view within `max_chars`: the rows from the middle
/// are cut (the start, and more of the end, where the summaries are).
fn fit(rows: Vec<Row>, max_chars: usize) -> Vec<Row> {
    let size = |r: &Row| r.text.len() + 1;
    if rows.iter().map(size).sum::<usize>() <= max_chars {
        return rows;
    }
    let (head_budget, tail_budget) = (max_chars * 2 / 5, max_chars * 3 / 5);
    let mut head = 0;
    let mut used = 0;
    while head < rows.len() && used + size(&rows[head]) <= head_budget {
        used += size(&rows[head]);
        head += 1;
    }
    let mut tail = rows.len();
    used = 0;
    while tail > head && used + size(&rows[tail - 1]) <= tail_budget {
        used += size(&rows[tail - 1]);
        tail -= 1;
    }
    if tail <= head {
        return rows;
    }
    let (first, last) = (rows[head].first, rows[tail - 1].last);
    let cut = Row {
        text: format!(
            "[lines {}-{}: {} more lines of the condensed view not shown, to keep it short]",
            first + 1,
            last + 1,
            tail - head
        ),
        first,
        last,
    };
    let mut rows = rows;
    let rest = rows.split_off(tail);
    rows.truncate(head);
    rows.push(cut);
    rows.extend(rest);
    rows
}

/// A condensed view of command output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condensed {
    /// The view: kept lines in order, each omitted run as one
    /// `[lines a-b omitted: …]` marker (line numbers of the original, from 1).
    pub text: String,
    /// Lines of the original.
    pub lines: usize,
    /// The formats recognized, in the order they were found.
    pub formats: Vec<&'static str>,
}

/// Whether `text` is long enough to be worth condensing.
pub fn worth_trying(text: &str) -> bool {
    bulky::tokens(text) >= MIN_TOKENS
}

/// The condensed view of `text` (command or check output as `run_command`
/// renders it), at most about `max_tokens` long, or `None`: no format
/// recognized, or not much shorter than `text` (see [`MAX_PERCENT`]).
pub fn condense(text: &str, max_tokens: usize) -> Option<Condensed> {
    let condensed = recognize(text, max_tokens)?;
    let shown = bulky::tokens(&condensed.text) + NOTE_TOKENS;
    (shown * 100 <= bulky::tokens(text) * MAX_PERCENT).then_some(condensed)
}

/// The condensed view of `text` whenever a format is recognized, however
/// little it cuts ([`condense`] also weighs that).
pub fn recognize(text: &str, max_tokens: usize) -> Option<Condensed> {
    let mut plan = Plan::new(text);
    for i in 0..plan.len() {
        if STRUCTURE.is_match(plan.line(i)) {
            plan.keep(i);
        }
    }
    rust::apply(&mut plan);
    python::apply(&mut plan);
    js::apply(&mut plan);
    go::apply(&mut plan);
    jvm::apply(&mut plan);
    packages::apply(&mut plan);
    if !plan.any_found() {
        generic::apply(&mut plan);
    }
    if !plan.any_found() {
        return None;
    }
    let rows = fit(plan.render(), max_tokens.max(MIN_TOKENS) * 3);
    let mut text = String::new();
    for r in &rows {
        text.push_str(&r.text);
        text.push('\n');
    }
    Some(Condensed {
        text,
        lines: plan.len(),
        formats: plan.formats,
    })
}

/// The view the frontier is shown: the handle line, the condensed `body`
/// (sanitized) and the closing note naming the handle.
pub fn view(handle: &str, label: &str, condensed: &Condensed, body: &str) -> String {
    format!(
        "{handle} ({label}), condensed ({}):\n{body}condensed from {} lines; \
read_raw(handle=\"{handle}\", start_line=..., end_line=...) for the full output\n",
        condensed.formats.join(", "),
        condensed.lines
    )
}

/// Programs that show files or search them: their output is what the model
/// asked to see, whatever it contains.
const VIEWERS: &[&str] = &[
    "cat", "head", "tail", "less", "more", "sed", "awk", "gawk", "grep", "egrep", "fgrep", "rg",
    "ag", "find", "fd", "ls", "tree", "wc", "od", "xxd", "hexdump", "strings", "diff", "nl", "jq",
    "yq", "git", "bat", "file", "stat", "echo", "printf", "sort", "uniq", "cut", "column", "tr",
];
/// Filters that pick lines by content: output piped through one is already
/// reduced to what the model chose to see.
const CONTENT_FILTERS: &[&str] = &[
    "grep", "egrep", "fgrep", "rg", "ag", "awk", "gawk", "sed", "jq", "yq", "cut", "sort", "uniq",
    "wc", "tr", "column", "perl", "python", "python3", "node", "ruby",
];
/// Words before the program itself.
const PREFIXES: &[&str] = &["time", "env", "nice", "nohup", "command", "exec", "sudo"];

/// The program a pipeline stage runs: its first word after variable
/// assignments, wrappers (`time`, `env`, `timeout 60`) and a leading `cd`.
fn program(stage: &str) -> Option<String> {
    let words: Vec<&str> = stage.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].trim_start_matches(['(', '{']);
        if w.is_empty() || (w.contains('=') && !w.starts_with('-')) || PREFIXES.contains(&w) {
            i += 1;
        } else if w == "timeout" {
            i += 2;
        } else {
            let name = w.rsplit('/').next().unwrap_or(w);
            return Some(name.to_owned());
        }
    }
    None
}

/// Whether the output of `command` may be condensed: some command of it
/// (`a; b`, `a && b`, `a || b`) runs a program other than one that shows or
/// searches files (`cat`, `grep`, `git diff`), and does not pipe its output
/// through a filter that already picked the lines the model wanted
/// (`| grep FAIL`). `| head` and `| tail` cut by position, not content.
pub fn eligible(command: &str) -> bool {
    command
        .split(['\n', ';'])
        .flat_map(|s| s.split("&&"))
        .flat_map(|s| s.split("||"))
        .any(|sequence| {
            let mut stages = sequence.split('|').filter_map(program);
            let Some(first) = stages.next() else {
                return false;
            };
            !VIEWERS.contains(&first.as_str())
                && !stages.any(|p| CONTENT_FILTERS.contains(&p.as_str()))
        })
}

re!(
    /// Lines worth keeping inside a failure: assertions, expected and actual
    /// values, errors and exceptions, diffs.
    MESSAGE,
    r"(?i)assert|expected|actual|received|\bleft\b|\bright\b|\bgot\b|\bwant\b|panicked|error|exception|mismatch|not equal|differ|fail|^\s*[-+] |^E\s|^>\s"
);

/// Whether `line` carries a failure's message (see [`MESSAGE`]).
pub(crate) fn message(line: &str) -> bool {
    MESSAGE.is_match(line)
}

#[cfg(test)]
mod tests;
