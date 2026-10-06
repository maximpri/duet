// SPDX-License-Identifier: GPL-3.0-or-later
//! Test output of no known runner that still reports counts ("12 passed,
//! 1 failed"): lines that mark a passing check and progress bars are
//! omitted, everything else kept. Tried only when no specific format is
//! recognized.

use super::{Plan, Why, re};

re!(
    COUNTS,
    r"(?i)\b\d+\s+(?:tests?\s+)?(?:passed|passing|failed|failing|failures?|succeeded|errored)\b"
);
re!(
    PASS,
    r"^\s*(?:✓|✔|√|\[\s*(?:OK|PASS|PASSED)\s*\]|PASS(?:ED)?:?\s)|\s(?:\.\.\.|:|-)\s*(?:ok|OK|PASS(?:ED)?|passed)\s*$"
);
re!(
    BAR,
    r"^\s*[\[(|]?[#=\-*>.█▏▎▍▌▋▊▉ ]{10,}[\])|]?\s*\d{1,3}(?:\.\d+)?\s*%|\b\d{1,3}%\s*\|[█▏▎▍▌▋▊▉ #=\-]+\|"
);

pub(super) fn apply(plan: &mut Plan) {
    let n = plan.len();
    if !(0..n).any(|i| COUNTS.is_match(plan.line(i))) {
        return;
    }
    let passing: Vec<usize> = (0..n).filter(|&i| PASS.is_match(plan.line(i))).collect();
    let bars: Vec<usize> = (0..n).filter(|&i| BAR.is_match(plan.line(i))).collect();
    // A single matching line is too little to tell a format by.
    if passing.len() + bars.len() < 3 {
        return;
    }
    plan.found("test counts");
    for i in passing {
        if !COUNTS.is_match(plan.line(i)) {
            plan.omit(i, Why::Passing);
        }
    }
    for i in bars {
        plan.omit(i, Why::Progress);
    }
}
