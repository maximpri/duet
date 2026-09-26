// SPDX-License-Identifier: GPL-3.0-or-later
//! Cargo and rustc: `cargo test` (the libtest harness), `cargo build`,
//! `cargo check` and `cargo clippy` progress, and rustc's diagnostics.

use super::{Cap, Plan, Why, message, re};
use std::collections::{HashMap, HashSet};

re!(
    SECTION,
    r"^\s*(?:Running (?:unittests )?\S.*\(target/.*\)|Running (?:tests|benches|examples)/\S+.*|Doc-tests \S+)\s*$"
);
re!(HARNESS, r"^running \d+ tests?$");
re!(
    TEST,
    r"^test (.+?) \.\.\. (ok|FAILED|ignored(?:, .*)?|bench: .*)$"
);
re!(
    RESULT,
    r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out"
);
re!(FAILURE, r"^---- (.+?) (?:stdout|stderr) ----$");
re!(
    STEP,
    r"^\s+(?:Compiling|Checking|Downloaded|Downloading|Updating|Fresh|Blocking|Locking|Adding|Removing|Documenting|Packaging|Verifying|Archiving|Installing|Replacing|Unpacking|Generated|Scraping|Updated)\s"
);
re!(PROGRESS, r"^\s*Building \[[=> ]*\] \d+/\d+");
re!(BACKTRACE_NOTE, r"^note: run with `RUST_BACKTRACE=1`");
re!(
    DIAGNOSTIC,
    r"^(error|warning)(?:\[([A-Za-z]*\d+|[a-z_:]+)\])?: (.+)$"
);
re!(LOCATION, r"^\s*--> \S+:\d+:\d+");
re!(
    /// Lines that sum up a build rather than start a diagnostic.
    SUMMARY,
    r"^(?:error: could not compile|error: aborting due to|warning: build failed|warning: .* generated \d+ warnings?|error: test failed|error: \d+ targets? failed|error: failed to|error: no matching|Some errors have detailed explanations|For more information about)"
);
re!(
    /// A label under source in a diagnostic (`^^^ expected u32`).
    LABEL, r"^\s*\d*\s*\|.*\^");
re!(SOURCE, r"^\s*\d+\s*\|");
re!(BACKTICKS, r"`[^`]*`");

/// Warnings of one kind shown by location; beyond this they are counted.
const WARNINGS_PER_KIND: usize = 30;

pub(super) fn apply(plan: &mut Plan) {
    diagnostics(plan);
    harness(plan);
    steps(plan);
}

/// `cargo` progress: compile, download and lock lines.
fn steps(plan: &mut Plan) {
    let mut any = false;
    for i in 0..plan.len() {
        if STEP.is_match(plan.line(i)) {
            plan.omit(i, Why::Build);
            any = true;
        } else if PROGRESS.is_match(plan.line(i)) {
            plan.omit(i, Why::Progress);
            any = true;
        }
    }
    if any {
        plan.found("cargo");
    }
}

/// The libtest harness: passing tests and passing binaries omitted, failing
/// tests, their captured output (capped) and the failure lists kept.
fn harness(plan: &mut Plan) {
    let n = plan.len();
    if !(0..n).any(|i| HARNESS.is_match(plan.line(i)) || RESULT.is_match(plan.line(i))) {
        return;
    }
    plan.found("cargo test");
    // The `Running …` line of the current test binary, and whether it failed.
    let mut section: Option<usize> = None;
    let mut failed_sections = Vec::new();
    // The passing binaries' result lines: where each is, and their sums.
    let mut ok_results: Vec<usize> = Vec::new();
    let mut sums = [0u64; 5];
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if SECTION.is_match(&line) {
            plan.omit(i, Why::Routine);
            section = Some(i);
        } else if HARNESS.is_match(&line) || BACKTRACE_NOTE.is_match(&line) {
            plan.omit(i, Why::Routine);
        } else if let Some(c) = TEST.captures(&line) {
            match &c[2] {
                "ok" => plan.omit(i, Why::Passing),
                "FAILED" => {
                    plan.keep(i);
                    failed_sections.extend(section);
                }
                s if s.starts_with("ignored") => plan.omit(i, Why::Skipped),
                _ => plan.keep(i),
            }
        } else if let Some(c) = RESULT.captures(&line) {
            if &c[1] == "ok" {
                plan.omit(i, Why::Routine);
                ok_results.push(i);
                for (k, sum) in sums.iter_mut().enumerate() {
                    *sum += c[k + 2].parse::<u64>().unwrap_or(0);
                }
            } else {
                plan.keep(i);
                failed_sections.extend(section);
            }
        } else if FAILURE.is_match(&line) {
            plan.keep(i);
            failed_sections.extend(section);
            let end = plan.until(i, |l| FAILURE.is_match(l) || l == "failures:");
            plan.block(i + 1..end, Cap { head: 10, tail: 10 }, &message);
            i = end;
            continue;
        }
        i += 1;
    }
    for s in failed_sections {
        plan.force_keep(s);
    }
    // The passing binaries' counts: one line, or their sum after the last.
    match ok_results[..] {
        [] => {}
        [only] => plan.force_keep(only),
        [.., last] => plan.note(
            last,
            format!(
                "test result: ok, summed over {} test targets: {} passed; {} failed; {} ignored; \
{} measured; {} filtered out",
                ok_results.len(),
                sums[0],
                sums[1],
                sums[2],
                sums[3],
                sums[4]
            ),
        ),
    }
}

/// rustc diagnostics. Errors: the first of each kind (its code, or its
/// message with names blanked) whole up to a cap, later ones as their
/// header, location and labelled source lines. Warnings: the first of each
/// kind as header, location and labels, later ones as header and location
/// (up to [`WARNINGS_PER_KIND`]); a warning printed again at the same place
/// (the lib and the test build) is omitted.
fn diagnostics(plan: &mut Plan) {
    let n = plan.len();
    let mut kinds: HashMap<String, usize> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut any = false;
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        let Some(c) = DIAGNOSTIC.captures(&line) else {
            i += 1;
            continue;
        };
        if SUMMARY.is_match(&line) {
            plan.keep(i);
            i += 1;
            continue;
        }
        let end = plan.until(i, |l| l.trim().is_empty() || DIAGNOSTIC.is_match(l));
        let Some(location) = (i + 1..end).find(|&k| LOCATION.is_match(plan.line(k))) else {
            // A one-line diagnostic (`warning: unused manifest key`).
            i = end;
            continue;
        };
        any = true;
        let error = &c[1] == "error";
        let kind = match c.get(2) {
            Some(code) => format!("{} {}", &c[1], code.as_str()),
            None => format!("{} {}", &c[1], BACKTICKS.replace_all(&c[3], "`…`")),
        };
        let place = format!("{line}\n{}", plan.line(location).trim());
        let count = kinds.entry(kind).or_insert(0);
        *count += 1;
        if !error && (!seen.insert(place) || *count > WARNINGS_PER_KIND) {
            plan.omit_all(i..end, Why::Repeat);
        } else if error && *count == 1 {
            plan.keep(i);
            plan.block(i + 1..end, Cap { head: 14, tail: 3 }, &|l| {
                LOCATION.is_match(l)
            });
        } else {
            plan.keep(i);
            plan.keep(location);
            // The labelled source lines: an error's own expected/found, the
            // first warning's example of its kind.
            if error || *count == 1 {
                for k in location + 1..end {
                    if LABEL.is_match(plan.line(k)) {
                        if SOURCE.is_match(plan.line(k - 1)) {
                            plan.keep(k - 1);
                        }
                        plan.keep(k);
                    }
                }
            }
            plan.omit_all(i + 1..end, Why::Detail);
        }
        i = end;
    }
    if any {
        plan.found("rustc");
    }
}
