// SPDX-License-Identifier: GPL-3.0-or-later
//! The detection corpus (`tests/corpus/`): recall on synthetic positives and
//! false positives on hard negatives, measured on every gate run.
//!
//! - Imported rules: every rule gets a positive generated from its own
//!   expression ([`generate`]); a rule no generated text satisfies needs a
//!   hand-written one in `positives.toml`. Each must be found by its rule and
//!   withheld by the full detector.
//! - Hand-written positives (`positives.toml`): duet's own formats and the
//!   international personal-data formats; each value must be withheld as its kind.
//! - Hard negatives (`negatives/`): hashes, UUIDs, lockfiles, base64 of public
//!   data, identifiers, fixtures, minified code, logs. Lines with a finding are
//!   false positives; their count per file may not rise above
//!   `baseline.toml` (lower the baseline when it falls).
//! - Rules that do not compile are listed; their number may not rise either.
//!
//! `DUET_CORPUS_DUMP=<file>` writes the generated positives for review.
//! Nothing in the corpus is a real credential or a real person's data.

#[path = "corpus/generate.rs"]
mod generate;

use duet_boundary::detect::{Detectors, Finding, Kind, scan_each_in};
use duet_boundary::rules::{Rule, imported};
use generate::{Sampler, seed};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Attempts at generating a positive for one rule.
const ATTEMPTS: u32 = 200;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

#[derive(Deserialize)]
struct Positives {
    case: Vec<Case>,
}

/// A hand-written positive: `rule` (an imported rule that must find a secret
/// in `text`) or `kind` and `value` (a value the detector must withhold as that kind).
#[derive(Deserialize)]
struct Case {
    rule: Option<String>,
    /// A kind tag (`secret`, `id`, `iban`, ...).
    kind: Option<String>,
    value: Option<String>,
    path: Option<String>,
    text: String,
}

#[derive(Deserialize)]
struct Baseline {
    rules: RuleBaseline,
    negatives: BTreeMap<String, FileBaseline>,
}

#[derive(Deserialize)]
struct RuleBaseline {
    failed: usize,
    partial: usize,
}

/// Lines with a finding in one negative file, scanned as that file (its
/// name as the path) and as text of unknown origin.
#[derive(Deserialize, Clone, Copy, Default, PartialEq, Debug)]
struct FileBaseline {
    as_file: usize,
    as_text: usize,
}

fn load<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = corpus_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Whether the findings together cover `start..end`.
fn covered(findings: &[Finding], start: usize, end: usize) -> bool {
    let mut spans: Vec<(usize, usize)> = findings.iter().map(|f| (f.start, f.end)).collect();
    spans.sort_unstable();
    let mut at = start;
    for (s, e) in spans {
        if s <= at && e > at {
            at = e;
        }
        if at >= end {
            return true;
        }
    }
    false
}

/// A generated positive for `rule`: the document, its path, and the span the
/// rule found in it.
fn generated(rule: &Rule) -> Option<(String, Option<String>, usize, usize)> {
    let path = rule
        .path_pattern()
        .and_then(|p| Sampler::new(seed(rule.id()), 0).sample(p))
        .map(|p| format!("corpus/sample{p}"));
    for attempt in 0..ATTEMPTS {
        let sample = Sampler::new(seed(rule.id()), attempt).sample(rule.pattern())?;
        // The rule runs only on text holding one of its keywords.
        let lower = sample.to_ascii_lowercase();
        let doc = match rule.keywords().first() {
            Some(k) if !rule.keywords().iter().any(|k| lower.contains(k.as_str())) => {
                format!("# {k}\n{sample}\n")
            }
            _ => format!("{sample}\n"),
        };
        let found = imported().find(&doc, path.as_deref());
        if let Some(m) = found.iter().find(|m| m.rule.id() == rule.id()) {
            return Some((doc.clone(), path.clone(), m.start, m.end));
        }
    }
    None
}

#[test]
fn imported_rules_compile_and_find_their_positives() {
    let set = imported();
    let baseline: Baseline = load("baseline.toml");
    let positives: Positives = load("positives.toml");
    for f in set.failed() {
        println!("rule not compiled: {} ({})", f.rule, f.reason);
    }
    for f in set.partial() {
        println!("rule partly supported: {} ({})", f.rule, f.reason);
    }
    let d = Detectors::default();
    let mut dump = String::new();
    let mut by_rule = 0;
    let mut withheld = 0;
    let mut missing = Vec::new();
    let mut not_withheld = Vec::new();
    let mut hand_written: BTreeMap<&str, Vec<&Case>> = BTreeMap::new();
    for c in &positives.case {
        if let Some(r) = &c.rule {
            hand_written.entry(r.as_str()).or_default().push(c);
        }
    }
    for rule in set.rules() {
        let mut docs = Vec::new();
        match generated(rule) {
            Some((doc, path, start, end)) => docs.push((doc, path, Some((start, end)))),
            None if hand_written.contains_key(rule.id()) => {}
            None => missing.push(rule.id().to_owned()),
        }
        for c in hand_written.get(rule.id()).into_iter().flatten() {
            docs.push((c.text.clone(), c.path.clone(), None));
        }
        for (doc, path, span) in docs {
            let found = set.find(&doc, path.as_deref());
            let Some(m) = found.iter().find(|m| m.rule.id() == rule.id()) else {
                missing.push(format!("{} (hand-written)", rule.id()));
                continue;
            };
            let (start, end) = span.unwrap_or((m.start, m.end));
            by_rule += 1;
            if covered(&scan_each_in(&doc, d, path.as_deref()), start, end) {
                withheld += 1;
            } else {
                not_withheld.push(rule.id().to_owned());
            }
            dump.push_str(&format!(
                "== {} {}\n{doc}\n",
                rule.id(),
                path.unwrap_or_default()
            ));
        }
    }
    if let Ok(out) = std::env::var("DUET_CORPUS_DUMP") {
        std::fs::write(&out, dump).expect("write the corpus dump");
    }
    let total = set.rules().len();
    println!(
        "imported rules: gitleaks v{}: {total} compiled, {} not compiled, {} partly supported, {} path-only ({})",
        duet_boundary::rules::notice_field("version")
            .unwrap_or("?")
            .trim_start_matches('v'),
        set.failed().len(),
        set.partial().len(),
        set.path_only().len(),
        set.path_only().join(", ")
    );
    println!(
        "imported-rule positives: {by_rule} found by their rule, {withheld} withheld by the full detector; rules without a positive: {}",
        missing.len()
    );
    assert!(total >= 200, "the vendored rule file lost rules: {total}");
    assert!(
        set.failed().len() <= baseline.rules.failed,
        "more rules fail to compile than the baseline ({}): {:?}",
        baseline.rules.failed,
        set.failed()
    );
    assert!(
        set.partial().len() <= baseline.rules.partial,
        "more rules are only partly supported than the baseline ({}): {:?}",
        baseline.rules.partial,
        set.partial()
    );
    assert!(
        missing.is_empty(),
        "rules without a detected positive: {missing:?}"
    );
    assert!(
        not_withheld.is_empty(),
        "found by their rule but not withheld: {not_withheld:?}"
    );
}

#[test]
fn hand_written_positives_are_withheld_as_their_kind() {
    let positives: Positives = load("positives.toml");
    let d = Detectors::default();
    let mut checked = 0;
    let mut failures = Vec::new();
    for c in &positives.case {
        let (Some(tag), Some(value)) = (&c.kind, &c.value) else {
            continue;
        };
        let kind = Kind::ALL
            .into_iter()
            .find(|k| k.tag() == tag)
            .unwrap_or_else(|| panic!("unknown kind {tag}"));
        let start = c
            .text
            .find(value.as_str())
            .unwrap_or_else(|| panic!("{value:?} is not in its text"));
        let end = start + value.len();
        let found: Vec<Finding> = scan_each_in(&c.text, d, c.path.as_deref())
            .into_iter()
            .filter(|f| f.kind == kind)
            .collect();
        checked += 1;
        if !covered(&found, start, end) {
            failures.push(format!("{tag} {value:?} in {:?}", c.text));
        }
    }
    println!(
        "hand-written positives: {} of {checked} withheld as their kind",
        checked - failures.len()
    );
    assert!(failures.is_empty(), "not withheld: {failures:#?}");
}

/// Lines of `text` holding part of a finding.
fn flagged_lines(text: &str, path: Option<&str>) -> Vec<usize> {
    let findings = scan_each_in(text, Detectors::default(), path);
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let mut lines: Vec<usize> = findings
        .iter()
        .flat_map(|f| {
            let first = starts.partition_point(|&s| s <= f.start) - 1;
            let last = starts.partition_point(|&s| s < f.end.max(f.start + 1)) - 1;
            first..=last
        })
        .collect();
    lines.dedup();
    lines.sort_unstable();
    lines.dedup();
    lines
}

#[test]
fn false_positives_stay_within_the_baseline() {
    let baseline: Baseline = load("baseline.toml");
    let dir = corpus_dir().join("negatives");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("negatives")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let (mut lines_total, mut file_total, mut text_total) = (0, 0, 0);
    let mut regressions = Vec::new();
    let mut improved = Vec::new();
    println!("hard negatives: lines with a finding (as the file / as text of unknown origin)");
    for name in &names {
        let text = std::fs::read_to_string(dir.join(name)).expect("negative file");
        let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
        let as_file = flagged_lines(&text, Some(name));
        let as_text = flagged_lines(&text, None);
        let now = FileBaseline {
            as_file: as_file.len(),
            as_text: as_text.len(),
        };
        let allowed = baseline.negatives.get(name).copied().unwrap_or_default();
        println!(
            "  {name:<20} {lines:>4} lines  {:>3} / {:>3}   (baseline {} / {})",
            now.as_file, now.as_text, allowed.as_file, allowed.as_text
        );
        if std::env::var_os("DUET_CORPUS_VERBOSE").is_some() {
            let all: Vec<&str> = text.lines().collect();
            for i in &as_text {
                println!(
                    "      {}: {}",
                    i + 1,
                    all[*i].chars().take(160).collect::<String>()
                );
            }
        }
        if now.as_file > allowed.as_file || now.as_text > allowed.as_text {
            regressions.push(format!("{name}: {now:?} > {allowed:?}"));
        } else if now != allowed {
            improved.push(format!("{name}: {now:?} < {allowed:?}"));
        }
        lines_total += lines;
        file_total += now.as_file;
        text_total += now.as_text;
    }
    let rate = |n: usize| 100.0 * n as f64 / lines_total.max(1) as f64;
    println!(
        "false positives: {file_total} of {lines_total} lines as files ({:.1}%), {text_total} as text ({:.1}%)",
        rate(file_total),
        rate(text_total)
    );
    if !improved.is_empty() {
        println!(
            "fewer false positives than the baseline; lower it in baseline.toml: {improved:?}"
        );
    }
    for name in baseline.negatives.keys() {
        assert!(
            names.contains(name),
            "baseline names a missing file: {name}"
        );
    }
    assert!(
        regressions.is_empty(),
        "false positives rose: {regressions:#?}"
    );
}

/// End to end in hybrid mode: every positive, in a public file the model
/// reads and in text the frontier writes itself, reaches the frontier only as
/// a placeholder. (Passthrough mode has no boundary: it shows content as it is.)
#[test]
fn positives_reach_the_frontier_only_as_placeholders() {
    use duet_boundary::engine::Engine;
    use duet_boundary::model::{Item, Request};
    use duet_boundary::policy::Policy;
    use duet_boundary::view::{Presenter, Source};

    let positives: Positives = load("positives.toml");
    let mut cases: Vec<(String, String, Option<String>)> = positives
        .case
        .iter()
        .filter_map(|c| Some((c.text.clone(), c.value.clone()?, c.path.clone())))
        .collect();
    for rule in imported().rules() {
        if let Some((doc, path, start, end)) = generated(rule) {
            let value = doc[start..end].to_owned();
            cases.push((doc, value, path));
        }
    }
    let d = tempfile::tempdir().unwrap();
    let policy = Policy {
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 1_000_000,
        bulky_file_tokens: 1_000_000,
        ..Policy::default()
    };
    let e = Engine::open(d.path(), policy, None).unwrap();
    let mut leaked = Vec::new();
    for (doc, value, path) in &cases {
        let path = path.clone().unwrap_or_else(|| "notes.txt".into());
        let source = Source::File {
            path: PathBuf::from(&path),
            ranged: false,
        };
        if e.present(&source, doc.as_bytes()).contains(value.as_str()) {
            leaked.push(format!("read_file {path}: {value}"));
        }
    }
    let (filter, check) = e.outbound();
    let mut req = Request {
        items: cases
            .iter()
            .map(|(_, v, _)| Item::User {
                text: format!("see {v} there"),
            })
            .collect(),
        ..Request::default()
    };
    filter.apply(&mut req);
    let body = serde_json::to_value(&req.items).unwrap();
    for (_, v, _) in &cases {
        if body.to_string().contains(v.as_str()) {
            leaked.push(format!("outbound: {v}"));
        }
    }
    assert!(check.check(&body).is_ok());
    println!(
        "end to end: {} positives, {} reached the frontier",
        cases.len(),
        leaked.len()
    );
    assert!(leaked.is_empty(), "{leaked:#?}");
}
