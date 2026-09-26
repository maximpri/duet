// SPDX-License-Identifier: GPL-3.0-or-later
//! Package installs and scripts: npm, pnpm and yarn (classic and berry).
//! Errors and the result lines are kept; progress, request logs, funding
//! notices and deprecation warnings are omitted; repeated warnings once.

use super::{Plan, Why, re};
use std::collections::HashSet;

re!(
    NPM,
    r"^npm (WARN|warn|ERR!|error|notice|http|timing|sill|silly|verb|info) "
);
re!(NPM_DEPRECATED, r"^npm (?:WARN|warn) deprecated ");
re!(
    NPM_RESULT,
    r"^(?:added|removed|changed|audited) \d+ packages?|^up to date|^found \d+ vulnerabilit|^\d+ (?:low|moderate|high|critical)"
);
re!(
    FUNDING,
    r"^\d+ packages? (?:is|are) looking for funding|^\s+run `npm fund` for details|^To address (?:all )?issues|^\s+npm audit fix|^Run `npm audit` for details|^npm notice"
);
re!(
    PNPM_PROGRESS,
    r"^Progress: resolved \d+|^[+-]+$|^Packages: [+-]\d+"
);
re!(
    PNPM_WARN_DEPRECATED,
    r"^\s*WARN\s+(?:deprecated|\d+ deprecated)"
);
re!(PNPM_ERROR, r"ERR_PNPM_|^\s*ERROR\s");
re!(
    PNPM_LIST,
    r"^(?:dependencies|devDependencies|optionalDependencies):$"
);
re!(PNPM_ENTRY, r"^[+-] \S+ \S+");
re!(
    PNPM_ROUTINE,
    r"^Lockfile is up to date|^Already up to date|^Scope: all \d+ workspace|^Progress: "
);
re!(YARN_STEP, r"^\[\d+/\d+\] ");
re!(YARN_CLASSIC, r"^(info|warning|error|success) ");
re!(YARN_BERRY, r"^➤ (YN\d{4}): (.*)$");
re!(DONE, r"^(?:Done in [\d.]+m?s|✨\s+Done in )");

pub(super) fn apply(plan: &mut Plan) {
    let n = plan.len();
    let detected = (0..n).any(|i| {
        let l = plan.line(i);
        NPM.is_match(l)
            || NPM_RESULT.is_match(l) && !l.starts_with(char::is_numeric)
            || PNPM_PROGRESS.is_match(l) && l.starts_with("Progress")
            || YARN_STEP.is_match(l)
            || YARN_BERRY.is_match(l)
            || l.contains("ERR_PNPM_")
    });
    if !detected {
        return;
    }
    plan.found("npm/pnpm/yarn");
    let mut seen = HashSet::new();
    // Inside pnpm's list of what it installed.
    let mut listing = false;
    for i in 0..n {
        let line = plan.line(i).to_owned();
        listing = PNPM_LIST.is_match(&line) || listing && PNPM_ENTRY.is_match(&line);
        let routine = listing
            || NPM_DEPRECATED.is_match(&line)
            || PNPM_WARN_DEPRECATED.is_match(&line)
            || FUNDING.is_match(&line)
            || PNPM_ROUTINE.is_match(&line);
        if routine {
            plan.omit(i, Why::Routine);
        } else if NPM_RESULT.is_match(&line) || DONE.is_match(&line) {
            plan.keep(i);
        } else if PNPM_PROGRESS.is_match(&line) || YARN_STEP.is_match(&line) {
            plan.omit(i, Why::Progress);
        } else if PNPM_ERROR.is_match(&line) {
            plan.keep(i);
        } else if let Some(c) = NPM.captures(&line) {
            match &c[1] {
                "http" | "timing" | "sill" | "silly" | "verb" | "info" | "notice" => {
                    plan.omit(i, Why::Progress)
                }
                _ if !seen.insert(line.clone()) => plan.omit(i, Why::Repeat),
                _ => plan.keep(i),
            }
        } else if let Some(c) = YARN_CLASSIC.captures(&line) {
            match &c[1] {
                "info" => plan.omit(i, Why::Routine),
                "warning" if line.contains("deprecated") => plan.omit(i, Why::Routine),
                _ if !seen.insert(line.clone()) => plan.omit(i, Why::Repeat),
                _ => plan.keep(i),
            }
        } else if let Some(c) = YARN_BERRY.captures(&line) {
            let (code, text) = (c[1].to_owned(), c[2].to_owned());
            match code.as_str() {
                "YN0000" if !text.contains("Failed") && !text.contains("Done") => {
                    plan.omit(i, Why::Progress)
                }
                // Fetching, building and cache notices.
                "YN0013" | "YN0007" | "YN0019" | "YN0085" => plan.omit(i, Why::Progress),
                // Peer dependency warnings: the first of each code.
                "YN0002" | "YN0060" if !seen.insert(code.clone()) => plan.omit(i, Why::Repeat),
                _ if !seen.insert(line.clone()) => plan.omit(i, Why::Repeat),
                _ => plan.keep(i),
            }
        }
    }
}
