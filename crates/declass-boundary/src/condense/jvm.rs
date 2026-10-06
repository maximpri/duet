// SPDX-License-Identifier: GPL-3.0-or-later
//! Maven and Gradle builds and their test summaries (Surefire, Gradle's
//! test task).

use super::{Cap, Plan, Why, js_frames, message, re};
use std::collections::HashSet;

pub(super) fn apply(plan: &mut Plan) {
    maven(plan);
    gradle(plan);
}

re!(LEVEL, r"^\[(INFO|WARNING|WARN|ERROR)\]");
re!(MAVEN_END, r"^\[INFO\] BUILD (?:SUCCESS|FAILURE)");
re!(
    MAVEN_CLASS,
    r"^\[(?:INFO|WARNING|ERROR)\] Tests run: \d+, Failures: (\d+), Errors: (\d+), Skipped: \d+.* --? in \S+"
);
re!(
    MAVEN_TOTAL,
    r"^\[(?:INFO|WARNING|ERROR)\] Tests run: \d+, Failures: \d+, Errors: \d+, Skipped: \d+"
);
re!(
    MAVEN_KEEP,
    r"^\[INFO\] (?:BUILD (?:SUCCESS|FAILURE)|Results:|Reactor Summary)|^\[INFO\] \S.* (?:SUCCESS|FAILURE|SKIPPED) \["
);
re!(
    MAVEN_DOWNLOAD,
    r"^(?:\[INFO\] )?Download(?:ing|ed) from |^Progress \(\d+\)"
);
re!(
    MAVEN_COMPILE,
    r"^\[INFO\] (?:Compiling \d+|Building |Changes detected|Nothing to compile|Copying \d+|Using ')"
);
re!(
    MAVEN_HELP,
    r"^\[ERROR\] (?:-> \[Help 1\]|To see the full stack trace|Re-run Maven using|For more information about the errors|\[Help 1\] http|\s*$)"
);

fn maven(plan: &mut Plan) {
    let n = plan.len();
    if !(0..n).any(|i| MAVEN_END.is_match(plan.line(i)) || MAVEN_TOTAL.is_match(plan.line(i))) {
        return;
    }
    plan.found("maven");
    let mut seen = HashSet::new();
    for i in 0..n {
        let line = plan.line(i).to_owned();
        let Some(c) = LEVEL.captures(&line) else {
            if MAVEN_DOWNLOAD.is_match(&line) {
                plan.omit(i, Why::Build);
            }
            continue;
        };
        let level = c[1].to_owned();
        if let Some(t) = MAVEN_CLASS.captures(&line) {
            if &t[1] == "0" && &t[2] == "0" {
                plan.omit(i, Why::Passing);
            } else {
                plan.keep(i);
            }
        } else if MAVEN_TOTAL.is_match(&line) || MAVEN_KEEP.is_match(&line) {
            plan.keep(i);
        } else if MAVEN_DOWNLOAD.is_match(&line) || MAVEN_COMPILE.is_match(&line) {
            plan.omit(i, Why::Build);
        } else if MAVEN_HELP.is_match(&line) || level == "INFO" {
            plan.omit(i, Why::Routine);
        } else if seen.insert(line.clone()) {
            plan.keep(i);
        } else {
            plan.omit(i, Why::Repeat);
        }
    }
    // Surefire prints stack traces under a failure, without a level.
    js_frames(plan, 0..n);
}

re!(GRADLE_END, r"^BUILD (?:SUCCESSFUL|FAILED) in ");
re!(
    GRADLE_TASK,
    r"^> Task :\S+(?: (UP-TO-DATE|NO-SOURCE|FROM-CACHE|SKIPPED|FAILED))?$"
);
re!(GRADLE_TEST, r"^(\S.*?) > (.+?) (PASSED|SKIPPED|FAILED)$");
re!(GRADLE_SUMMARY, r"^\d+ tests? completed, \d+ failed");
re!(
    GRADLE_ROUTINE,
    r"^(?:\d+ actionable tasks?:|Starting a Gradle Daemon|Deprecated Gradle features were used|You can use '--warning-mode all'|For more on this, please refer to|Use '--warning-mode all')"
);
re!(
    GRADLE_PROGRESS,
    r"^<[-=]+> \d+% (?:EXECUTING|CONFIGURING|INITIALIZING|WAITING)|^> (?:IDLE|Configuring project)"
);
re!(GRADLE_DOWNLOAD, r"^Download(?:ing)? https?://");
re!(GRADLE_TRY, r"^\* Try:$");
re!(
    GRADLE_MORE,
    r"^\* (?:Get more help|Exception is):|^BUILD (?:SUCCESSFUL|FAILED)"
);

fn gradle(plan: &mut Plan) {
    let n = plan.len();
    if !(0..n).any(|i| GRADLE_END.is_match(plan.line(i)) || GRADLE_TASK.is_match(plan.line(i))) {
        return;
    }
    plan.found("gradle");
    let mut i = 0;
    while i < n {
        let line = plan.line(i).to_owned();
        if let Some(c) = GRADLE_TASK.captures(&line) {
            if c.get(1).is_some_and(|m| m.as_str() == "FAILED") {
                plan.keep(i);
            } else {
                plan.omit(i, Why::Build);
            }
        } else if let Some(c) = GRADLE_TEST.captures(&line) {
            match &c[3] {
                "PASSED" => plan.omit(i, Why::Passing),
                "SKIPPED" => plan.omit(i, Why::Skipped),
                _ => {
                    plan.keep(i);
                    let end = plan.until(i, |l| {
                        l.trim().is_empty() || !l.starts_with(char::is_whitespace)
                    });
                    js_frames(plan, i + 1..end);
                    plan.block(i + 1..end, Cap { head: 4, tail: 2 }, &message);
                    i = end;
                    continue;
                }
            }
        } else if GRADLE_ROUTINE.is_match(&line) {
            plan.omit(i, Why::Routine);
        } else if GRADLE_PROGRESS.is_match(&line) {
            plan.omit(i, Why::Progress);
        } else if GRADLE_DOWNLOAD.is_match(&line) {
            plan.omit(i, Why::Build);
        } else if GRADLE_TRY.is_match(&line) {
            let end = plan.until(i, |l| GRADLE_MORE.is_match(l));
            plan.omit_all(i..end, Why::Routine);
            i = end;
            continue;
        } else if GRADLE_SUMMARY.is_match(&line) {
            plan.keep(i);
        }
        i += 1;
    }
}
