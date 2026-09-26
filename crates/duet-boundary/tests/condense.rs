// SPDX-License-Identifier: GPL-3.0-or-later
//! The condensers (`duet_boundary::condense`) on real and synthetic command
//! output (`tests/condense/`), measured on every gate run:
//!
//! - `recorded.jsonl`: command output recorded in the Gate 2 and X1/X2
//!   calibration runs (`results/gate2b`, `gate2e`, `xcal`: public benchmark
//!   repositories), with every canary-bearing output left out and paths
//!   anonymized.
//! - `synthetic/`: hand-written output of the runners those runs did not use
//!   (pytest, unittest, jest, vitest, mocha, go, tsc, eslint, npm, pnpm,
//!   yarn, maven, gradle, clippy) in each tool's own format.
//!
//! Each output goes through what the engine does: eligibility by command,
//! the size floor, the condenser. Every failing test's name and every
//! assertion line the original holds (found by the independent rules below,
//! not the condensers') must be in the condensed view, and the token
//! reduction per format and overall is printed (`--nocapture`) and held
//! above a floor.
//!
//! The ignored `recorded_runs` reads the runs themselves
//! (`DUET_CONDENSE_RESULTS=<results dir>`), measures every command output
//! they recorded, and with `DUET_CONDENSE_WRITE=1` rewrites `recorded.jsonl`.
//! `DUET_CONDENSE_SHOW=1` prints every condensed view.

use duet_boundary::bulky::tokens;
use duet_boundary::condense::{self, Condensed};
use duet_boundary::detect::{Detectors, scan_each_in};
use duet_boundary::testing::canary::{Canaries, Options};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The public bulky threshold the engine passes as the condensed view's size cap.
const MAX_TOKENS: usize = 2000;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/condense")
}

/// One command's output.
struct Record {
    source: String,
    command: String,
    output: String,
    /// Shown as a handle (sensitive, or offloaded): never condensed.
    held: bool,
}

fn recorded() -> Vec<Record> {
    std::fs::read_to_string(dir().join("recorded.jsonl"))
        .unwrap()
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            let s = |k: &str| v[k].as_str().unwrap().to_owned();
            Record {
                source: s("source"),
                command: s("command"),
                output: s("output"),
                held: false,
            }
        })
        .collect()
}

/// `synthetic/<name>.txt`: the first line is `$ <command>`, the rest the
/// output as `run_command` renders it.
fn synthetic() -> Vec<Record> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir().join("synthetic"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            let (first, output) = text.split_once('\n').unwrap();
            Record {
                source: format!("synthetic/{}", p.file_name().unwrap().to_string_lossy()),
                command: first.strip_prefix("$ ").unwrap().to_owned(),
                output: output.to_owned(),
                held: false,
            }
        })
        .collect()
}

/// What the frontier would be shown of `r`, as the engine builds it; `None`:
/// the output as it is.
fn engine_view(r: &Record) -> Option<(Condensed, String)> {
    if r.held || !condense::eligible(&r.command) || !condense::worth_trying(&r.output) {
        return None;
    }
    let c = condense::condense(&r.output, MAX_TOKENS)?;
    let label = format!("output of `{}`", r.command);
    let view = condense::view("h12", &label, &c, &c.text);
    Some((c, view))
}

static FAILING: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^test (\S+) \.\.\. FAILED$",
        r"^---- (\S+) (?:stdout|stderr) ----$",
        r"^(?:FAILED|ERROR) (\S+\.py::\S+)",
        r"^_{3,} (\S.*?) _{3,}$",
        r"^(?:FAIL|ERROR): (\S+ \(\S+\))",
        r"^\s+● (\S.*)$",
        r"^\s+(?:✕|×) (\S.*?)(?: \(\d+ ?ms\)| \d+ms)?$",
        r"^\s?FAIL\s+(\S.*)$",
        r"^\s*✖ (\S.*?) \(\d",
        r"^\s*not ok \d+ - (\S.*)$",
        r"^\s*--- FAIL: (\S+)",
        r"^(\S.*? > .+) FAILED$",
        r"Tests run:.*<<< (?:FAILURE|ERROR)!.* --? in (\S+)",
        r"^\s+\d+\) (\S.*)$",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static ASSERTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)assertion|\bexpected\b.*\b(?:but|got|found|received)\b|^\s*(?:left|right)\s*:|^\s*(?:Expected|Received)\s*:|panicked at|^E\s+\S|^\s*[\w.]*AssertionError\b",
    )
    .unwrap()
});

/// What of `original` must be in the condensed view: failing tests' names and
/// assertion lines (trimmed, first 120 characters).
fn must_survive(original: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in original.lines() {
        for re in FAILING.iter() {
            if let Some(c) = re.captures(line) {
                out.push(c[1].trim().to_owned());
            }
        }
        if ASSERTION.is_match(line) && !line.trim_start().starts_with("at ") {
            out.push(line.trim().chars().take(120).collect());
        }
    }
    out.retain(|s| s.chars().count() >= 3);
    out.dedup();
    out
}

#[derive(Default)]
struct Tally {
    outputs: usize,
    condensed: usize,
    before: usize,
    after: usize,
}

impl Tally {
    fn add(&mut self, before: usize, after: Option<usize>) {
        self.outputs += 1;
        self.before += before;
        self.after += after.unwrap_or(before);
        self.condensed += usize::from(after.is_some());
    }

    fn row(&self, name: &str) -> String {
        format!(
            "  {name:<20} {:>5} {:>9} {:>9} {:>9} {:>6.1}%",
            self.outputs,
            self.condensed,
            self.before,
            self.after,
            100.0 * (self.before - self.after.min(self.before)) as f64 / self.before.max(1) as f64
        )
    }
}

/// Condenses every record: survival failures, and the tallies per format
/// (by the first format recognized; `-` for output left as it is).
fn measure(records: &[Record]) -> (Vec<String>, BTreeMap<String, Tally>, Tally) {
    let mut lost = Vec::new();
    let mut per: BTreeMap<String, Tally> = BTreeMap::new();
    let mut all = Tally::default();
    for r in records {
        let before = tokens(&r.output);
        match engine_view(r) {
            Some((c, view)) => {
                if std::env::var_os("DUET_CONDENSE_SHOW").is_some() {
                    println!("===== {} `{}`\n{view}", r.source, r.command);
                }
                for s in must_survive(&r.output) {
                    if !view.contains(&s) {
                        lost.push(format!("{}: `{}`: {s:?}", r.source, r.command));
                    }
                }
                let after = tokens(&view);
                per.entry(c.formats[0].to_owned())
                    .or_default()
                    .add(before, Some(after));
                all.add(before, Some(after));
            }
            None => {
                per.entry("-".into()).or_default().add(before, None);
                all.add(before, None);
            }
        }
    }
    (lost, per, all)
}

fn print(title: &str, per: &BTreeMap<String, Tally>, all: &Tally) {
    println!("{title}");
    println!(
        "  {:<20} {:>5} {:>9} {:>9} {:>9} {:>7}",
        "format", "out", "condensed", "tokens", "after", "saved"
    );
    for (name, t) in per {
        println!("{}", t.row(name));
    }
    println!("{}", all.row("all"));
}

#[test]
fn recorded_output_keeps_every_failure_and_gets_shorter() {
    let records = recorded();
    assert!(records.len() >= 50, "{}", records.len());
    let (lost, per, all) = measure(&records);
    print(
        "recorded command output (tests/condense/recorded.jsonl)",
        &per,
        &all,
    );
    assert!(lost.is_empty(), "{lost:#?}");
    let condensed: Vec<&Tally> = per
        .iter()
        .filter(|(k, _)| *k != "-")
        .map(|(_, t)| t)
        .collect();
    let (before, after) = condensed
        .iter()
        .fold((0, 0), |(b, a), t| (b + t.before, a + t.after));
    // Condensed outputs shrink by more than a third (41% when this was
    // written), and the output as a whole by some.
    assert!(after * 100 <= before * 65, "{after} of {before}");
    assert!(all.after < all.before, "{} of {}", all.after, all.before);
}

/// Every runner's format is recognized, and its view keeps every failure,
/// whether or not it cuts enough to be used: a `not-condensed-` fixture is
/// output where nearly every line is the failure itself (all tests failing,
/// a list of compile errors), which is shown as it is; `unknown-` output is
/// in no format at all.
#[test]
fn synthetic_output_of_every_runner_is_recognized_and_keeps_every_failure() {
    let records = synthetic();
    assert!(records.len() >= 20, "{}", records.len());
    let mut lost = Vec::new();
    for r in &records {
        let recognized = condense::recognize(&r.output, MAX_TOKENS);
        let unknown = r.source.contains("/unknown-");
        assert_eq!(recognized.is_none(), unknown, "{}", r.source);
        if let Some(c) = recognized {
            for s in must_survive(&r.output) {
                if !c.text.contains(&s) {
                    lost.push(format!("{}: {s:?}", r.source));
                }
            }
        }
        let shown = engine_view(r);
        let declined = unknown || r.source.contains("/not-condensed-");
        assert_eq!(
            shown.is_none(),
            declined,
            "{}:\n{}",
            r.source,
            shown.map(|v| v.1).unwrap_or_default()
        );
    }
    assert!(lost.is_empty(), "{lost:#?}");
    let (_, per, all) = measure(&records);
    print(
        "synthetic runner output (tests/condense/synthetic)",
        &per,
        &all,
    );
    // One fixture of each format family is condensed.
    for format in [
        "cargo test",
        "rustc",
        "pytest",
        "unittest",
        "jest",
        "vitest",
        "mocha",
        "node --test",
        "go test",
        "tsc",
        "eslint",
        "npm/pnpm/yarn",
        "maven",
        "gradle",
        "test counts",
    ] {
        assert!(
            per.get(format).is_some_and(|t| t.condensed > 0),
            "{format} never condensed"
        );
    }
}

static MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[lines? (\d+)(?:-(\d+))?(?: omitted: |: \d+ more lines of the condensed view)")
        .unwrap()
});
static REPEATS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" \[×(\d+)\]$").unwrap());

/// Whether `view` accounts for every line of `original`, in order: each
/// line is shown (as the terminal would leave it; cut when long; repeats
/// counted) or lies in the range of a marker; nothing is added but markers
/// and bracketed notes.
fn accounted(original: &str, view: &str) -> Result<(), String> {
    let terminal = |l: &str| -> String {
        let l = l.rsplit('\r').find(|s| !s.is_empty()).unwrap_or("");
        Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]")
            .unwrap()
            .replace_all(l, "")
            .into_owned()
    };
    let lines: Vec<String> = original.lines().map(terminal).collect();
    let mut next = 0;
    let skip_blank = |next: &mut usize| {
        while *next < lines.len() && lines[*next].trim().is_empty() {
            *next += 1;
        }
    };
    for row in view.lines() {
        if row.is_empty() {
            continue;
        }
        if let Some(c) = MARKER.captures(row) {
            let first: usize = c[1].parse().unwrap();
            let last: usize = c.get(2).map_or(first, |m| m.as_str().parse().unwrap());
            skip_blank(&mut next);
            if first - 1 < next {
                return Err(format!(
                    "marker `{row}` goes back to line {first} (at {})",
                    next + 1
                ));
            }
            if lines[next..first - 1].iter().any(|l| !l.trim().is_empty()) {
                return Err(format!(
                    "lines {}-{} dropped before `{row}`",
                    next + 1,
                    first - 1
                ));
            }
            next = last;
            continue;
        }
        if row.starts_with('[') && row.ends_with(']') && !lines.iter().any(|l| l == row) {
            continue;
        }
        let (text, times) = match REPEATS.captures(row) {
            Some(c) => (
                &row[..c.get(0).unwrap().start()],
                c[1].parse::<usize>().unwrap(),
            ),
            None => (row, 1),
        };
        for _ in 0..times {
            skip_blank(&mut next);
            let Some(line) = lines.get(next) else {
                return Err(format!("`{row}` is past the end"));
            };
            let same = match text.split_once("… [+") {
                Some((head, _)) if text.ends_with(" chars]") => line.starts_with(head),
                _ => line == text,
            };
            if !same {
                return Err(format!("`{row}` shown where line {} is `{line}`", next + 1));
            }
            next += 1;
        }
    }
    skip_blank(&mut next);
    if next < lines.len() {
        return Err(format!(
            "lines {}-{} dropped at the end",
            next + 1,
            lines.len()
        ));
    }
    Ok(())
}

#[test]
fn every_line_is_shown_or_named_by_a_marker_in_order() {
    let mut records = recorded();
    records.extend(synthetic());
    let mut wrong = Vec::new();
    for r in &records {
        if let Some(c) = condense::recognize(&r.output, MAX_TOKENS)
            && let Err(e) = accounted(&r.output, &c.text)
        {
            wrong.push(format!("{}: {e}", r.source));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    // The check itself notices a dropped, invented or reordered line.
    assert!(accounted("a\nb\nc\n", "a\n[line 2 omitted: 1 routine line]\nc\n").is_ok());
    assert!(accounted("a\nb\nc\n", "a\nc\n").is_err());
    assert!(accounted("a\nb\n", "b\na\n").is_err());
    assert!(accounted("a\n", "a\nx\n").is_err());
    assert!(accounted("a\nb\nb\nb\n", "a\nb [×3]\n").is_ok());
}

/// Anonymizes what a recorded output says about where it ran.
fn anonymize(text: &str) -> String {
    static WORKSPACE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"/Volumes/[^/\s]+/duet_v2/results/[^/\s]+/[^/\s]+/(workspace|tmp)").unwrap()
    });
    static HOME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/Users/[a-z0-9_]+").unwrap());
    static VOLUME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/Volumes/[^/\s]+").unwrap());
    let t = WORKSPACE.replace_all(text, "/work/$1");
    let t = HOME.replace_all(&t, "/Users/dev");
    VOLUME.replace_all(&t, "/Volumes/disk").into_owned()
}

/// Every command output (`run_command`, failed checks) a run's transcripts
/// recorded, with its lane and the canaries planted in its workspace.
fn run_outputs(run: &Path) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let runs = run.join("workspace/.duet/runs");
    let Ok(entries) = std::fs::read_dir(&runs) else {
        return out;
    };
    for e in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(e.path().join("transcript.jsonl")) else {
            continue;
        };
        let mut calls: BTreeMap<String, (String, String)> = BTreeMap::new();
        let mut classes: BTreeMap<String, String> = BTreeMap::new();
        let mut results: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            match v["kind"].as_str() {
                Some("item") => {
                    let item = &v["item"];
                    match item["type"].as_str() {
                        Some("assistant") => {
                            for c in item["tool_calls"].as_array().into_iter().flatten() {
                                let name = c["name"].as_str().unwrap_or_default().to_owned();
                                let command = c["arguments"]["command"]
                                    .as_str()
                                    .unwrap_or("(checks)")
                                    .to_owned();
                                calls.insert(
                                    c["id"].as_str().unwrap_or_default().to_owned(),
                                    (name, command),
                                );
                            }
                        }
                        Some("tool_result") => results.push((
                            item["call_id"].as_str().unwrap_or_default().to_owned(),
                            item["content"].as_str().unwrap_or_default().to_owned(),
                        )),
                        _ => {}
                    }
                }
                Some("shown") => {
                    classes.insert(
                        v["call_id"].as_str().unwrap_or_default().to_owned(),
                        v["class"].as_str().unwrap_or_default().to_owned(),
                    );
                }
                _ => {}
            }
        }
        for (id, content) in results {
            let Some((name, command)) = calls.get(&id) else {
                continue;
            };
            let checks = name == "finish" && content.starts_with("checks failed");
            if name != "run_command" && !checks {
                continue;
            }
            let class = classes.get(&id).cloned().unwrap_or_default();
            let content = match content.strip_prefix("checks failed; the task is not complete:\n") {
                Some(rest) => rest.to_owned(),
                None => content,
            };
            out.push((command.clone(), content, class));
        }
    }
    out
}

fn canaries(run: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(run.join("manifest.json")) else {
        return Vec::new();
    };
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    let mut out = Vec::new();
    for c in v["canaries"].as_array().into_iter().flatten() {
        out.extend(c["value"].as_str().map(str::to_lowercase));
        for variant in c["variants"].as_array().into_iter().flatten() {
            out.extend(variant.as_str().map(str::to_lowercase));
        }
    }
    out.retain(|c| c.len() >= 4);
    out
}

/// Commands that name the benchmark's sensitive files: their output may
/// carry planted data in a form no canary check knows.
static SENSITIVE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"data/|logs?/|\.env|\.csv|\.log\b|\.db\b|secrets?/|\.duet").unwrap()
});
/// Long numbers: planted card and account numbers survive in rounded or
/// reformatted spellings.
static LONG_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{12,}").unwrap());

/// The recorded runs themselves: every command output of `gate2b`, `gate2e`
/// and `xcal` measured as the frontier saw it (hybrid output is already
/// sanitized, and held output is left as it is), and with
/// `DUET_CONDENSE_WRITE=1` the outputs in a recognized format written to
/// `recorded.jsonl`: only output shown raw, from a command naming no
/// sensitive path, holding no placeholder, no canary of any run in any form
/// the canary finder knows (digit runs of 6+), no long number and nothing
/// the secret and personal-data detectors find.
#[test]
#[ignore = "needs the recorded runs: DUET_CONDENSE_RESULTS=<results dir>"]
fn recorded_runs() {
    let root = PathBuf::from(
        std::env::var("DUET_CONDENSE_RESULTS").expect("DUET_CONDENSE_RESULTS=<results dir>"),
    );
    let all_canaries: Vec<String> = ["gate2b", "gate2e", "xcal"]
        .iter()
        .flat_map(|b| std::fs::read_dir(root.join(b)).unwrap().flatten())
        .flat_map(|e| canaries(&e.path()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let finder = Canaries::with_options(
        all_canaries,
        Options {
            digit_min: 6,
            ..Options::default()
        },
    );
    let detectors = Detectors {
        secrets: true,
        pii: true,
        entropy: false,
    };
    let mut records = Vec::new();
    let mut fixture: Vec<Value> = Vec::new();
    let mut seen = HashSet::new();
    let mut dropped = BTreeMap::<&str, usize>::new();
    for batch in ["gate2b", "gate2e", "xcal"] {
        let mut runs: Vec<PathBuf> = std::fs::read_dir(root.join(batch))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join("manifest.json").is_file())
            .collect();
        runs.sort();
        for run in runs {
            let name = run.file_name().unwrap().to_string_lossy().into_owned();
            let planted = canaries(&run);
            for (command, output, class) in run_outputs(&run) {
                let source = format!("{batch}/{name}");
                // Output held as sensitive is never condensed: it stays as shown.
                records.push(Record {
                    source: source.clone(),
                    command: command.clone(),
                    output: output.clone(),
                    held: class != "raw",
                });
                if std::env::var_os("DUET_CONDENSE_WRITE").is_none() {
                    continue;
                }
                let lower = format!("{command}\n{output}").to_lowercase();
                let reason = if class != "raw" {
                    Some("held or offloaded")
                } else if output.contains('⟨') {
                    Some("placeholders")
                } else if planted.iter().any(|c| lower.contains(c.as_str()))
                    || !finder.find(format!("{command}\n{output}")).is_empty()
                {
                    Some("canary")
                } else if SENSITIVE_PATH.is_match(&command) {
                    Some("names a sensitive path")
                } else if LONG_NUMBER.is_match(&output)
                    || !scan_each_in(&output, detectors, None).is_empty()
                {
                    Some("long number or detector finding")
                } else if !condense::worth_trying(&output) || !condense::eligible(&command) {
                    Some("not eligible or short")
                } else if condense::recognize(&output, MAX_TOKENS).is_none() {
                    Some("no format recognized")
                } else if !seen.insert(output.clone()) {
                    Some("duplicate")
                } else {
                    None
                };
                if let Some(r) = reason {
                    *dropped.entry(r).or_default() += 1;
                    continue;
                }
                fixture.push(json!({
                    "source": source,
                    "command": anonymize(&command),
                    "output": anonymize(&output),
                }));
            }
        }
    }
    let (lost, per, all) = measure(&records);
    print(
        "recorded runs (gate2b, gate2e, xcal), every command output",
        &per,
        &all,
    );
    println!("survival failures: {}", lost.len());
    for l in &lost {
        println!("  {l}");
    }
    if std::env::var_os("DUET_CONDENSE_WRITE").is_some() {
        println!("fixture: {} outputs; left out: {dropped:?}", fixture.len());
        let text: String = fixture.iter().map(|v| format!("{v}\n")).collect();
        std::fs::write(dir().join("recorded.jsonl"), text).unwrap();
    }
}
