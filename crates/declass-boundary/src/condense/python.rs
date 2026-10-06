// SPDX-License-Identifier: GPL-3.0-or-later
//! Python test runners: pytest and unittest (also what Django's and
//! `python -m unittest` print).

use super::{Cap, Plan, Why, message, python_frames, re};

re!(SESSION, r"^=+ test session starts =+$");
re!(
    FINAL,
    r"^=+ .*\b\d+ (?:passed|failed|errors?|skipped|xfailed|xpassed|deselected|warnings?|no tests ran)\b.* in [\d.]+s\b.*=+$"
);
re!(SUMMARY_LINE, r"^(?:FAILED|ERROR) \S+\.py(?:::\S+)?");
re!(
    ROUTINE,
    r"^(?:platform \S+ -- Python|cachedir: |rootdir: |configfile: |plugins: |testpaths: |cov: |django: |hypothesis profile|benchmark: |asyncio: |Using --randomly-seed|-- Docs: https://docs\.pytest\.org)"
);
re!(COLLECTING, r"^collecting \.\.\.");
re!(PROGRESS, r"^(\S+\.py) ([.sxXFE]+)\s*(?:\[\s*\d+%\])?$");
re!(
    VERBOSE,
    r"^\S+\.py::\S.*? (PASSED|FAILED|ERROR|SKIPPED|XFAIL|XPASS)\b.*?(?:\[\s*\d+%\])?$"
);
re!(HEADING, r"^=+ (.+?) =+$");
re!(FAILURE, r"^_{3,} (.+?) _{3,}$");
re!(
    /// Lines of a pytest failure that carry its message: `E ` lines, the
    /// failing source line (`>`), `file.py:12: AssertionError` locations, and the
    /// captured-output headings.
    PYTEST_MESSAGE,
    r"^E\s|^>\s|^\S+\.py:\d+: |^-+ Captured .* -+$"
);

re!(RAN, r"^Ran \d+ tests? in [\d.]+s$");
re!(DOTS, r"^[.sxEFu]+$");
re!(UNITTEST_OK, r" \.\.\. (?:ok|expected failure)$");
re!(UNITTEST_SKIP, r" \.\.\. skipped\b");
re!(RULE, r"^={40,}$");
re!(DASHES, r"^-{40,}$");
re!(UNITTEST_FAILURE, r"^(?:FAIL|ERROR|UNEXPECTED SUCCESS): \S");
re!(
    EXCEPTION,
    r"^[A-Za-z_][\w.]*(?:Error|Exception|Failure|Exit|Interrupt|Warning)\b|^Traceback \(most recent call last\)"
);

pub(super) fn apply(plan: &mut Plan) {
    pytest(plan);
    unittest(plan);
}

fn pytest(plan: &mut Plan) {
    let n = plan.len();
    let detected = (0..n).any(|i| {
        let l = plan.line(i);
        SESSION.is_match(l) || FINAL.is_match(l) || SUMMARY_LINE.is_match(l)
    });
    if !detected {
        return;
    }
    plan.found("pytest");
    let mut section = String::new();
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if SESSION.is_match(&line) || ROUTINE.is_match(&line) {
            plan.omit(i, Why::Routine);
        } else if COLLECTING.is_match(&line) {
            plan.omit(i, Why::Progress);
        } else if let Some(c) = PROGRESS.captures(&line) {
            if c[2].contains(['F', 'E']) {
                plan.keep(i);
            } else {
                plan.omit(i, Why::Passing);
            }
        } else if let Some(c) = VERBOSE.captures(&line) {
            match &c[1] {
                "PASSED" | "XPASS" => plan.omit(i, Why::Passing),
                "SKIPPED" | "XFAIL" => plan.omit(i, Why::Skipped),
                _ => plan.keep(i),
            }
        } else if FINAL.is_match(&line) {
            plan.keep(i);
        } else if let Some(c) = HEADING.captures(&line) {
            section = c[1].to_ascii_lowercase();
            plan.keep(i);
            let end = plan.until(i, |l| HEADING.is_match(l) || FINAL.is_match(l));
            match section.as_str() {
                "warnings summary" => {
                    plan.block(i + 1..end, Cap { head: 6, tail: 0 }, &|_| false);
                }
                "passes" => plan.omit_all(i + 1..end, Why::Passing),
                _ => {}
            }
        } else if FAILURE.is_match(&line) && (section == "failures" || section == "errors") {
            plan.keep(i);
            let end = plan.until(i, |l| FAILURE.is_match(l) || HEADING.is_match(l));
            python_frames(plan, i + 1..end);
            plan.block(i + 1..end, Cap { head: 4, tail: 10 }, &|l| {
                PYTEST_MESSAGE.is_match(l)
            });
            i = end;
            continue;
        }
        i += 1;
    }
}

fn unittest(plan: &mut Plan) {
    let n = plan.len();
    if !(0..n).any(|i| RAN.is_match(plan.line(i))) {
        return;
    }
    plan.found("unittest");
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if DOTS.is_match(&line) {
            if line.contains(['E', 'F']) {
                plan.keep(i);
            } else {
                plan.omit(i, Why::Passing);
            }
        } else if UNITTEST_OK.is_match(&line) {
            plan.omit(i, Why::Passing);
        } else if UNITTEST_SKIP.is_match(&line) {
            plan.omit(i, Why::Skipped);
        } else if RULE.is_match(&line) && i + 1 < n && UNITTEST_FAILURE.is_match(plan.line(i + 1)) {
            plan.omit(i, Why::Routine);
            plan.keep(i + 1);
            // The block runs to the next `====` rule, or to the `----` rule before `Ran`.
            let end = plan.until(i + 1, |l| RULE.is_match(l));
            let end = (i + 2..end)
                .find(|&k| {
                    DASHES.is_match(plan.line(k)) && k + 1 < n && RAN.is_match(plan.line(k + 1))
                })
                .unwrap_or(end);
            for k in plan.matching(i + 2..end, &DASHES) {
                plan.omit(k, Why::Routine);
            }
            python_frames(plan, i + 2..end);
            plan.block(i + 2..end, Cap { head: 2, tail: 8 }, &|l| {
                EXCEPTION.is_match(l) || message(l)
            });
            i = end;
            continue;
        } else if DASHES.is_match(&line) {
            plan.omit(i, Why::Routine);
        }
        i += 1;
    }
}
