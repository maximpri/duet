// SPDX-License-Identifier: GPL-3.0-or-later
//! `go test` and `go build`/`go vet`.

use super::{Cap, Plan, Why, message, re};

re!(RUN, r"^\s*=== (?:RUN|PAUSE|CONT|NAME)\s");
re!(
    RESULT,
    r"^(\s*)--- (PASS|FAIL|SKIP): \S+ \(\d+(?:\.\d+)?s\)$"
);
re!(PACKAGE_OK, r"^ok\s+\S+\s+(?:\d+(?:\.\d+)?s|\(cached\))");
re!(PACKAGE_FAIL, r"^FAIL\s+\S+");
re!(NO_TESTS, r"^\?\s+\S+\s+\[no test files\]$");
re!(DOWNLOAD, r"^go: (?:downloading|finding|extracting) ");
re!(GOROUTINE, r"^goroutine \d+ \[.+\]:$");

pub(super) fn apply(plan: &mut Plan) {
    let n = plan.len();
    let detected = (0..n).any(|i| {
        let l = plan.line(i);
        RESULT.is_match(l) || PACKAGE_OK.is_match(l) || NO_TESTS.is_match(l)
    }) || (0..n).any(|i| PACKAGE_FAIL.is_match(plan.line(i)))
        && (0..n).any(|i| RUN.is_match(plan.line(i)) || plan.line(i) == "FAIL");
    if !detected {
        return;
    }
    plan.found("go test");
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if RUN.is_match(&line) || NO_TESTS.is_match(&line) || line == "PASS" {
            plan.omit(i, Why::Routine);
        } else if DOWNLOAD.is_match(&line) {
            plan.omit(i, Why::Build);
        } else if PACKAGE_OK.is_match(&line) {
            plan.omit(i, Why::Passing);
        } else if let Some(c) = RESULT.captures(&line) {
            // A test's own output is indented under its result line, up to
            // the result of a subtest.
            let depth = c[1].len();
            let end = plan.until(i, |l| {
                l.trim().is_empty() || indent(l) <= depth || RESULT.is_match(l)
            });
            match &c[2] {
                "PASS" => plan.omit_all(i..end, Why::Passing),
                "SKIP" => plan.omit_all(i..end, Why::Skipped),
                _ => {
                    plan.keep(i);
                    plan.block(i + 1..end, Cap { head: 12, tail: 4 }, &message);
                }
            }
            i = end;
            continue;
        } else if GOROUTINE.is_match(&line) {
            // The panicking goroutine's first frames (a function line and its
            // file:line each) say where it failed.
            plan.keep(i);
            let end = plan.until(i, |l| l.trim().is_empty());
            for k in i + 1..end {
                if k <= i + 4 {
                    plan.keep(k);
                } else {
                    plan.omit(k, Why::Frames);
                }
            }
            i = end;
            continue;
        }
        i += 1;
    }
}
