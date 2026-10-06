// SPDX-License-Identifier: GPL-3.0-or-later
//! JavaScript and TypeScript: vitest, jest, mocha, Node's own test runner
//! (`node --test`, spec and TAP reporters), tsc and eslint.

use super::{Cap, Plan, Why, js_frames, message, re};
use std::collections::{HashMap, HashSet};

pub(super) fn apply(plan: &mut Plan) {
    vitest(plan);
    jest(plan);
    node_test(plan);
    mocha(plan);
    tsc(plan);
    eslint(plan);
    if plan.any_found() {
        node_warnings(plan);
    }
}

re!(NODE_WARNING, r"^\(node:\d+\) (?:\[\w+\] )?(.*)$");
re!(TRACE_HINT, r"^\(Use `node --trace-\w+ \.\.\.` to show");

/// Node's process warnings, printed once per worker: the first of each kept.
fn node_warnings(plan: &mut Plan) {
    let mut seen = HashSet::new();
    for i in 0..plan.len() {
        if let Some(c) = NODE_WARNING.captures(plan.line(i)) {
            if seen.insert(c[1].to_owned()) {
                plan.keep(i);
            } else {
                plan.omit(i, Why::Repeat);
            }
        } else if TRACE_HINT.is_match(plan.line(i)) {
            plan.omit(i, Why::Routine);
        }
    }
}

re!(VITEST_FILES, r"^\s*Test Files\s+.*\(\d+\)\s*$");
re!(VITEST_RUN, r"^\s*RUN\s+v\d");
re!(
    VITEST_FILE_OK,
    r"^\s*✓ \S.* \(\d+ tests?(?: \| \d+ skipped)?\)(?: \d+(?:\.\d+)?m?s)?$"
);
re!(
    VITEST_FILE_SKIPPED,
    r"^\s*↓ \S.* \(\d+ tests? \| \d+ skipped\)"
);
re!(VITEST_TEST_OK, r"^\s+✓ ");
re!(VITEST_TEST_SKIPPED, r"^\s+↓ ");
re!(VITEST_FAIL, r"^\s?FAIL\s+\S");
re!(VITEST_RULE, r"^⎯{3,}");
re!(VITEST_COUNTER, r"^⎯+\[\d+/\d+\]⎯*$");
re!(VITEST_TIMING, r"^\s*(?:Start at|Duration)\s");
re!(
    VITEST_MESSAGE,
    r"Error|Expected|Received|^\s*[-+] |❯ \S+:\d+:\d+|^\s*\d+\|.*|^\s+\|\s+\^"
);

fn vitest(plan: &mut Plan) {
    let n = plan.len();
    if !(0..n).any(|i| VITEST_FILES.is_match(plan.line(i))) {
        return;
    }
    plan.found("vitest");
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if VITEST_RUN.is_match(&line)
            || VITEST_COUNTER.is_match(&line)
            || VITEST_TIMING.is_match(&line)
        {
            plan.omit(i, Why::Routine);
        } else if VITEST_FILE_OK.is_match(&line) || VITEST_TEST_OK.is_match(&line) {
            plan.omit(i, Why::Passing);
        } else if VITEST_FILE_SKIPPED.is_match(&line) || VITEST_TEST_SKIPPED.is_match(&line) {
            plan.omit(i, Why::Skipped);
        } else if VITEST_FAIL.is_match(&line) {
            plan.keep(i);
            let end = plan.until(i, |l| VITEST_FAIL.is_match(l) || VITEST_RULE.is_match(l));
            js_frames(plan, i + 1..end);
            plan.block(i + 1..end, Cap { head: 12, tail: 6 }, &|l| {
                VITEST_MESSAGE.is_match(l)
            });
            i = end;
            continue;
        }
        i += 1;
    }
}

re!(JEST_TOTAL, r"^Tests:\s+.*\d+ total$");
re!(JEST_SUITE, r"^\s*(PASS|FAIL)\s+\S");
re!(JEST_OK, r"^\s+(?:✓|√) ");
re!(JEST_FAILED, r"^\s+(?:✕|×) ");
re!(JEST_SKIPPED, r"^\s+○ ");
re!(JEST_FAILURE, r"^\s+● (.+)$");
re!(JEST_REPEAT, r"^Summary of all failing tests$");
re!(JEST_SUMMARY, r"^(?:Test Suites|Tests|Snapshots|Time):\s");
re!(
    JEST_MESSAGE,
    r"Expected|Received|expect\(|Error|^\s*>\s*\d+ \||^\s+\|\s+\^|^\s*[-+] "
);

fn jest(plan: &mut Plan) {
    let n = plan.len();
    if !(0..n).any(|i| JEST_TOTAL.is_match(plan.line(i))) {
        return;
    }
    plan.found("jest");
    let ends = |l: &str| {
        JEST_FAILURE.is_match(l)
            || JEST_SUITE.is_match(l)
            || JEST_SUMMARY.is_match(l)
            || JEST_REPEAT.is_match(l)
    };
    let mut repeating = false;
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if let Some(c) = JEST_SUITE.captures(&line) {
            if &c[1] == "PASS" {
                plan.omit(i, Why::Passing);
            } else {
                plan.keep(i);
            }
        } else if JEST_OK.is_match(&line) {
            plan.omit(i, Why::Passing);
        } else if JEST_SKIPPED.is_match(&line) {
            plan.omit(i, Why::Skipped);
        } else if JEST_FAILED.is_match(&line) {
            plan.keep(i);
        } else if JEST_REPEAT.is_match(&line) {
            // Jest repeats every failure here: kept once above.
            plan.omit(i, Why::Repeat);
            repeating = true;
        } else if JEST_SUMMARY.is_match(&line) {
            repeating = false;
        } else if let Some(c) = JEST_FAILURE.captures(&line) {
            let end = plan.until(i, ends);
            if repeating {
                plan.omit_all(i..end, Why::Repeat);
            } else if c[1].starts_with("Console") {
                plan.keep(i);
                plan.block(i + 1..end, Cap { head: 4, tail: 0 }, &|_| false);
            } else {
                plan.keep(i);
                js_frames(plan, i + 1..end);
                plan.block(i + 1..end, Cap { head: 12, tail: 2 }, &|l| {
                    JEST_MESSAGE.is_match(l)
                });
            }
            i = end;
            continue;
        }
        i += 1;
    }
}

re!(NODE_SUMMARY, r"^ℹ (?:tests|pass|fail) \d+$");
re!(NODE_OK, r"^\s*✔ .+\(\d+(?:\.\d+)?m?s\)(?: # .*)?$");
re!(NODE_FAILED, r"^\s*✖ .+\(\d+(?:\.\d+)?m?s\)(?: # .*)?$");
re!(NODE_SKIPPED, r"^\s*﹣ .+# (?:SKIP|TODO)");
re!(NODE_SECTION, r"^✖ failing tests:$");
re!(NODE_AT, r"^test at \S+:\d+:\d+$");
re!(NODE_ENTRY, r"^\s*(?:✔|✖|▶|ℹ|﹣) ");
re!(TAP_COUNT, r"^# (?:tests|pass|fail) \d+$");
re!(TAP_OK, r"^\s*ok \d+ - ");
re!(TAP_NOT_OK, r"^\s*not ok \d+ - ");
re!(
    TAP_ROUTINE,
    r"^\s*(?:TAP version \d+|1\.\.\d+|# Subtest: .*)$"
);
re!(TAP_YAML_START, r"^\s+---$");
re!(TAP_YAML_END, r"^\s+\.\.\.$");

/// Node's built-in test runner. Its spec reporter prints a failure's error
/// under the test and again under `✖ failing tests:`; the second copy is kept.
fn node_test(plan: &mut Plan) {
    let n = plan.len();
    let spec = (0..n).any(|i| NODE_SUMMARY.is_match(plan.line(i)));
    let tap = (0..n).any(|i| TAP_COUNT.is_match(plan.line(i)))
        && (0..n).any(|i| TAP_OK.is_match(plan.line(i)) || TAP_NOT_OK.is_match(plan.line(i)));
    if !spec && !tap {
        return;
    }
    plan.found("node --test");
    let section = (0..n).find(|&i| NODE_SECTION.is_match(plan.line(i)));
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if NODE_OK.is_match(&line) || TAP_OK.is_match(&line) {
            plan.omit(i, Why::Passing);
            if TAP_OK.is_match(&line) && i + 1 < n && TAP_YAML_START.is_match(plan.line(i + 1)) {
                let end = plan.until(i + 1, |l| TAP_YAML_END.is_match(l));
                plan.omit_all(i + 1..(end + 1).min(n), Why::Passing);
                i = end + 1;
                continue;
            }
        } else if NODE_SKIPPED.is_match(&line) {
            plan.omit(i, Why::Skipped);
        } else if TAP_ROUTINE.is_match(&line) {
            plan.omit(i, Why::Routine);
        } else if NODE_FAILED.is_match(&line)
            || TAP_NOT_OK.is_match(&line)
            || NODE_AT.is_match(&line)
        {
            plan.keep(i);
            // The error printed under a failing test, up to the next entry.
            let end = plan.until(i, |l| {
                NODE_ENTRY.is_match(l)
                    || NODE_AT.is_match(l)
                    || NODE_SECTION.is_match(l)
                    || TAP_OK.is_match(l)
                    || TAP_NOT_OK.is_match(l)
                    || TAP_COUNT.is_match(l)
            });
            if NODE_AT.is_match(&line) {
                i += 1;
                continue;
            }
            if section.is_some_and(|s| i < s) {
                plan.omit_all(i + 1..end, Why::Repeat);
            } else {
                js_frames(plan, i + 1..end);
                plan.block(i + 1..end, Cap { head: 10, tail: 3 }, &message);
            }
            i = end;
            continue;
        }
        i += 1;
    }
}

re!(MOCHA_PASSING, r"^\s+\d+ passing \(\d+(?:\.\d+)?m?s\)$");
re!(MOCHA_FAILING, r"^\s+\d+ failing$");
re!(MOCHA_OK, r"^\s+(?:✓|✔|√) ");
re!(MOCHA_PENDING, r"^\s+- \S");
re!(MOCHA_FAILURE, r"^\s+\d+\) \S");

fn mocha(plan: &mut Plan) {
    let n = plan.len();
    let Some(passing) = (0..n).find(|&i| MOCHA_PASSING.is_match(plan.line(i))) else {
        return;
    };
    plan.found("mocha");
    for i in 0..passing {
        let line = plan.line(i);
        if MOCHA_OK.is_match(line) {
            plan.omit(i, Why::Passing);
        } else if MOCHA_PENDING.is_match(line) {
            plan.omit(i, Why::Skipped);
        }
    }
    let Some(failing) = (passing..n).find(|&i| MOCHA_FAILING.is_match(plan.line(i))) else {
        return;
    };
    let mut i = failing + 1;
    while i < n {
        if MOCHA_FAILURE.is_match(plan.line(i)) {
            plan.keep(i);
            let end = plan.until(i, |l| MOCHA_FAILURE.is_match(l));
            js_frames(plan, i + 1..end);
            plan.block(i + 1..end, Cap { head: 12, tail: 2 }, &message);
            i = end;
        } else {
            i += 1;
        }
    }
}

re!(
    TSC,
    r"^(\S.*?)(?:\((\d+),(\d+)\):|:(\d+):(\d+) -) (error|warning) (TS\d+): (.*)$"
);
re!(TSC_FOUND, r"^Found \d+ errors?\b");
re!(TSC_TABLE, r"^Errors\s+Files$|^\s+\d+\s+\S+:\d+$");
re!(
    TSC_WATCH,
    r"^\[?\d+:\d+:\d+(?: [AP]M)?\]? (?:Starting compilation|File change detected)"
);

/// tsc: the first error of each code whole (its elaboration or code frame),
/// later ones as their header line, which holds the location.
fn tsc(plan: &mut Plan) {
    let n = plan.len();
    let headers: Vec<usize> = (0..n).filter(|&i| TSC.is_match(plan.line(i))).collect();
    if headers.is_empty() {
        return;
    }
    plan.found("tsc");
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for (k, &h) in headers.iter().enumerate() {
        let code = TSC
            .captures(plan.line(h))
            .map(|c| c[7].to_owned())
            .unwrap_or_default();
        let next = headers.get(k + 1).copied().unwrap_or(n);
        let end = (h + 1..next)
            .find(|&i| TSC_FOUND.is_match(plan.line(i)) || TSC_TABLE.is_match(plan.line(i)))
            .unwrap_or(next);
        plan.keep(h);
        let seen = kinds.entry(code).or_insert(0);
        *seen += 1;
        if *seen == 1 {
            plan.block(h + 1..end, Cap { head: 8, tail: 0 }, &|_| false);
        } else {
            plan.omit_all(h + 1..end, Why::Detail);
        }
    }
    for i in 0..n {
        if TSC_TABLE.is_match(plan.line(i)) || TSC_WATCH.is_match(plan.line(i)) {
            plan.omit(i, Why::Routine);
        }
    }
}

re!(
    ESLINT_PROBLEM,
    r"^\s+\d+:\d+\s+(error|warning)\s+.+?\s{2,}(\S+)$"
);
re!(
    ESLINT_TOTAL,
    r"^✖ \d+ problems? \(\d+ errors?, \d+ warnings?\)"
);
re!(
    ESLINT_FILE,
    r"^(?:/|[A-Za-z]:\\|\./)?\S+\.(?:m?[jt]sx?|c[jt]s|vue|svelte|astro)$"
);

/// eslint: every error; warnings up to three per rule; a file's name kept
/// when a problem under it is.
fn eslint(plan: &mut Plan) {
    let n = plan.len();
    let problems = (0..n)
        .filter(|&i| ESLINT_PROBLEM.is_match(plan.line(i)))
        .count();
    if problems == 0 || !(problems >= 2 || (0..n).any(|i| ESLINT_TOTAL.is_match(plan.line(i)))) {
        return;
    }
    plan.found("eslint");
    let mut per_rule: HashMap<String, usize> = HashMap::new();
    let mut file: Option<usize> = None;
    for i in 0..n {
        let line = plan.line(i).to_owned();
        if ESLINT_FILE.is_match(&line) {
            plan.omit(i, Why::Routine);
            file = Some(i);
        } else if let Some(c) = ESLINT_PROBLEM.captures(&line) {
            let seen = per_rule.entry(c[2].to_owned()).or_insert(0);
            *seen += 1;
            if &c[1] == "error" || *seen <= 3 {
                plan.keep(i);
                if let Some(f) = file {
                    plan.force_keep(f);
                }
            } else {
                plan.omit(i, Why::Repeat);
            }
        }
    }
}
