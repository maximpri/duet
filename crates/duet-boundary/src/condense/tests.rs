// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

/// `n` passing cargo tests, one failing with `detail` as its captured output.
fn cargo_run(n: usize, detail: &str) -> String {
    let mut out = String::from("exit code 101\n--- stdout ---\n\nrunning 40 tests\n");
    for i in 0..n {
        out.push_str(&format!("test ledger::case_{i:02} ... ok\n"));
        if i == 7 {
            out.push_str("test ledger::refund_matches ... FAILED\n");
        }
    }
    out.push_str("\nfailures:\n\n---- ledger::refund_matches stdout ----\n\n");
    out.push_str(detail);
    out.push_str(
        "\n\nfailures:\n    ledger::refund_matches\n\n\
test result: FAILED. 39 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\n",
    );
    out
}

const PANIC: &str = "thread 'ledger::refund_matches' panicked at src/ledger.rs:88:9:\n\
assertion `left == right` failed\n  left: 1250\n right: 1200";

#[test]
fn passing_tests_go_and_the_failure_stays_with_line_numbers_to_read_back() {
    let text = cargo_run(39, PANIC);
    let c = condense(&text, 2000).expect("condensed");
    assert_eq!(c.formats, ["cargo test"]);
    assert_eq!(c.lines, text.lines().count());
    for kept in [
        "exit code 101",
        "test ledger::refund_matches ... FAILED",
        "panicked at src/ledger.rs:88:9:",
        "  left: 1250",
        " right: 1200",
        "test result: FAILED. 39 passed; 1 failed",
    ] {
        assert!(c.text.contains(kept), "{kept}:\n{}", c.text);
    }
    assert!(!c.text.contains("case_03 ... ok"), "{}", c.text);
    // Omitted runs are named by the lines they cover, counted by kind.
    assert!(
        c.text
            .contains("[lines 4-12 omitted: 8 passing tests, 1 routine line]"),
        "{}",
        c.text
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[12], "test ledger::refund_matches ... FAILED");
    assert!(
        lines[3..12]
            .iter()
            .all(|l| l.ends_with(" ok") || l.starts_with("running"))
    );
    // The same input condenses the same way.
    assert_eq!(condense(&text, 2000), Some(c));
}

#[test]
fn passing_result_lines_are_summed_and_a_single_one_is_kept() {
    let mut text = String::from("exit code 0\n--- stdout ---\n");
    for (binary, n) in [("lib", 12), ("api", 9), ("regression", 5)] {
        text.push_str(&format!("\nrunning {n} tests\n"));
        for i in 0..n {
            text.push_str(&format!("test {binary}_{i} ... ok\n"));
        }
        text.push_str(&format!(
            "\ntest result: ok. {n} passed; 0 failed; 1 ignored; 0 measured; 2 filtered out; finished in 0.00s\n"
        ));
    }
    let c = condense(&text, 2000).expect("condensed");
    assert!(
        c.text.contains(
            "[test result: ok, summed over 3 test targets: 26 passed; 0 failed; 3 ignored; 0 measured; 6 filtered out]"
        ),
        "{}",
        c.text
    );
    let one = "exit code 0\n--- stdout ---\n\nrunning 30 tests\n".to_owned()
        + &(0..30)
            .map(|i| format!("test t_{i} ... ok\n"))
            .collect::<String>()
        + "\ntest result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
    let c = condense(&one, 2000).expect("condensed");
    assert!(c.text.contains("test result: ok. 30 passed"), "{}", c.text);
    assert!(!c.text.contains("summed"), "{}", c.text);
}

#[test]
fn unknown_or_short_or_failure_dense_output_is_not_condensed() {
    let log: String = (0..80)
        .map(|i| format!("2026-09-26T10:{i:02}:00Z worker picked job {i}\n"))
        .collect();
    assert_eq!(recognize(&log, 2000), None);
    assert!(!worth_trying(
        "exit code 0\n--- stdout ---\ntest a ... ok\n"
    ));
    // Every line is the failure itself: recognized, but not worth a handle.
    let dense =
        "exit code 101\n--- stdout ---\n\nrunning 1 test\ntest only ... FAILED\n\nfailures:\n\n\
---- only stdout ----\n"
            .to_owned()
            + &(0..20)
                .map(|i| {
                    format!(
                        "assertion `left == right` failed: row {i} differs from the expected row\n"
                    )
                })
                .collect::<String>()
            + "\nfailures:\n    only\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
    assert!(recognize(&dense, 2000).is_some());
    assert_eq!(condense(&dense, 2000), None);
}

#[test]
fn a_long_failure_keeps_its_ends_and_message_lines() {
    let mut detail = String::from("thread 'x' panicked at src/a.rs:1:1:\n");
    for i in 0..120 {
        detail.push_str(&format!("debug trace line {i}\n"));
        if i == 60 {
            detail.push_str("assertion failed: totals differ at row 61\n");
        }
    }
    detail.push_str("left: 3\nright: 4");
    let c = condense(&cargo_run(39, &detail), 2000).expect("condensed");
    assert!(c.text.contains("panicked at src/a.rs:1:1"), "{}", c.text);
    assert!(c.text.contains("assertion failed: totals differ at row 61"));
    assert!(c.text.contains("right: 4") && c.text.contains("debug trace line 119"));
    assert!(!c.text.contains("debug trace line 40\n"), "{}", c.text);
    assert!(c.text.contains("detail lines]"), "{}", c.text);
}

#[test]
fn escape_sequences_and_rewritten_lines_are_shown_as_a_terminal_would_leave_them() {
    let mut text = String::from("exit code 0\n--- stdout ---\n");
    for i in 0..40 {
        text.push_str(&format!("\x1b[32m✓\x1b[0m item {i} (3 ms)\n"));
    }
    text.push_str("progress 10%\rprogress 50%\rprogress 100%\n");
    text.push_str("\x1b[1m40 passed\x1b[0m, 0 failed\n");
    let c = condense(&text, 2000).expect("condensed");
    assert_eq!(c.formats, ["test counts"]);
    assert!(c.text.contains("40 passed, 0 failed"), "{}", c.text);
    assert!(c.text.contains("progress 100%"), "{}", c.text);
    assert!(
        !c.text.contains('\x1b') && !c.text.contains('\r'),
        "{:?}",
        c.text
    );
}

#[test]
fn runs_shorter_than_their_marker_are_shown_and_repeats_are_counted_from_three() {
    let mut plan = Plan::new("a\nb\nx\nsame\nsame\nsame\nsame\ntwice\ntwice\nend\n");
    plan.found("test");
    plan.omit(1, Why::Routine);
    let rows: Vec<String> = plan.render().into_iter().map(|r| r.text).collect();
    assert_eq!(
        rows,
        ["a", "b", "x", "same [×4]", "twice", "twice", "end"],
        "a one-letter omitted line is cheaper than its marker"
    );
}

#[test]
fn an_overlong_view_is_cut_in_the_middle_keeping_start_and_end() {
    let mut text = String::from("exit code 101\n--- stdout ---\n\nrunning 400 tests\n");
    for i in 0..400 {
        text.push_str(&format!("test case_{i:03} ... FAILED\n"));
    }
    text.push_str(
        "\ntest result: FAILED. 0 passed; 400 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n",
    );
    let c = recognize(&text, 500).expect("recognized");
    assert!(c.text.len() <= 500 * 3 + 200, "{}", c.text.len());
    assert!(c.text.contains("test case_000 ... FAILED"));
    assert!(c.text.contains("test result: FAILED. 0 passed; 400 failed"));
    assert!(
        c.text
            .contains("more lines of the condensed view not shown"),
        "{}",
        c.text
    );
}

#[test]
fn rustc_errors_keep_every_location_and_the_first_of_each_kind_whole() {
    let mut text = String::from("exit code 101\n--- stderr ---\n");
    for i in 0..60 {
        text.push_str(&format!("   Compiling dep_{i} v1.0.0\n"));
    }
    for (line, found) in [(10, "u32"), (20, "u64"), (30, "i8")] {
        text.push_str(&format!(
            "error[E0308]: mismatched types\n  --> src/lib.rs:{line}:5\n   |\n{line} |     total\n   \
|     ^^^^^ expected `i64`, found `{found}`\n   |\nhelp: you can convert\n   |\n{line} |     \
total.into()\n   |          +++++++\n\n"
        ));
    }
    let warning = "warning: unused variable: `zone`\n  --> src/lib.rs:4:9\n   |\n4  |     let zone = 1;\n   \
|         ^^^^ help: prefix it with an underscore: `_zone`\n   |\n   = note: `#[warn(unused_variables)]` on by default\n\n";
    text.push_str(warning);
    text.push_str(warning);
    text.push_str("error: could not compile `ledger` (lib) due to 3 previous errors\n");
    let c = condense(&text, 2000).expect("condensed");
    assert_eq!(c.formats, ["rustc", "cargo"]);
    for kept in [
        "--> src/lib.rs:10:5",
        "--> src/lib.rs:20:5",
        "--> src/lib.rs:30:5",
        "expected `i64`, found `u64`",
        "expected `i64`, found `i8`",
        "help: you can convert",
        "could not compile `ledger`",
    ] {
        assert!(c.text.contains(kept), "{kept}:\n{}", c.text);
    }
    // The fix suggestion once, for the first error of the kind.
    assert_eq!(c.text.matches("help: you can convert").count(), 1);
    // The same warning at the same place (lib and test build): once.
    assert_eq!(c.text.matches("unused variable: `zone`").count(), 1);
    assert!(c.text.contains("60 build and download lines"), "{}", c.text);
}

#[test]
fn the_sandboxs_notes_are_kept_whole() {
    // A registry the egress proxy refused, noted after an install's output.
    let mut text = String::from("exit code 1\n--- stderr ---\n");
    for i in 0..40 {
        text.push_str(&format!(
            "npm http fetch GET 200 https://registry.npmjs.org/pkg-{i} 12ms (cache hit)\n"
        ));
    }
    text.push_str(
        "npm error code E403\nnpm error 403 Forbidden - GET https://npm.corp.example/@corp%2fui\n",
    );
    let note = "[sandbox] the egress proxy refused: npm.corp.example:443. Commands reach only the \
package registries in sandbox.registries (an owner setting).";
    text.push_str(&format!("\n{note}\n"));
    let c = condense(&text, 2000).expect("condensed");
    assert!(c.text.contains(note), "{}", c.text);
    assert!(c.text.contains("npm error 403 Forbidden"), "{}", c.text);
    assert!(c.text.contains("40 progress lines"), "{}", c.text);
}

#[test]
fn eligibility_follows_what_the_command_asked_to_see() {
    for yes in [
        "cargo test",
        "cargo test 2>&1 | tail -40",
        "cd web && npm test",
        "npm test; echo exit=$?",
        "python -m pytest -q | head -100",
        "RUST_BACKTRACE=1 timeout 60 cargo test --lib",
        "grep -n foo src/a.rs; cargo test",
        "cargo test || true",
    ] {
        assert!(eligible(yes), "{yes}");
    }
    for no in [
        "cat target/test-output.txt",
        "cargo test 2>&1 | grep -E 'FAILED|panicked'",
        "sed -n '1,80p' tests/regression.rs",
        "git diff",
        "node --test 2>&1 | awk '/fail/'",
        "rg 'test result' -n logs.txt",
    ] {
        assert!(!eligible(no), "{no}");
    }
}

/// Condensing in the engine: only output the frontier may see, sanitized
/// whole before any line is left out, kept under a public handle.
mod engine {
    use crate::engine::Engine;
    use crate::policy::Policy;
    use crate::view::{Presenter, Source, ViewClass};
    use serde_json::{Map, Value, json};
    use std::sync::Arc;

    const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";
    const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";
    /// In `data/customers.csv` only: no detector recognizes it alone.
    const CUSTOMER: &str = "Orsolya Quennefeldt-Varga";
    /// Fails the card checksum: a card only where a label names it.
    const NUMBER: &str = "4539578763621487";

    fn policy(command_output_sensitive: bool) -> Policy {
        Policy {
            sensitive_globs: vec!["data/**".into()],
            command_output_sensitive,
            raw_ok_commands: vec!["cargo build".into()],
            detect_secrets: true,
            detect_pii: true,
            detect_entropy: true,
            bulky_tokens: 2000,
            bulky_file_tokens: 2000,
            condense_output: true,
            ..Policy::default()
        }
    }

    /// An engine primed with a customer file, as `duet run` primes it.
    fn engine(policy: Policy) -> (tempfile::TempDir, Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(
            ws.join("data/customers.csv"),
            format!("id,name,email\n1,{CUSTOMER},{EMAIL}\n"),
        )
        .unwrap();
        std::fs::create_dir_all(d.path().join("run")).unwrap();
        let e = Engine::open(&d.path().join("run"), policy, None).unwrap();
        e.prime(&ws, &["data/customers.csv".to_owned()], "");
        (d, e)
    }

    fn command(c: &str) -> Source {
        Source::Command {
            command: c.into(),
            exit_code: Some(101),
        }
    }

    fn read_raw(e: &Engine, handle: &str) -> String {
        let Value::Object(args) = json!({"handle": handle, "start_line": 1, "end_line": 500})
        else {
            unreachable!()
        };
        let args: Map<String, Value> = args;
        e.call_tool("read_raw", &args).unwrap().unwrap()
    }

    /// A public test's failing assertion that prints planted values.
    fn planted_output() -> String {
        super::cargo_run(
            39,
            &format!(
                "thread 'ledger::refund_matches' panicked at tests/refunds.rs:31:5:\n\
assertion `left == right` failed: refund for {CUSTOMER} <{EMAIL}> signed with {KEY}\n  \
left: \"{EMAIL}\"\n right: \"billing@example.com\""
            ),
        )
    }

    #[test]
    fn planted_values_in_a_failing_assertion_are_tokenized_in_the_view_and_the_handle() {
        for sensitive_by_default in [true, false] {
            let (_d, e) = engine(policy(sensitive_by_default));
            let out = planted_output();
            assert!(out.len() < crate::engine::INLINE_OUTPUT_CHARS);
            let shown = e.present(&command("cargo test"), out.as_bytes());
            assert_eq!(e.take_view_class(), Some(ViewClass::BulkyHandle));
            assert!(
                shown.starts_with("h1 (output of `cargo test`), condensed (cargo test):"),
                "{shown}"
            );
            assert!(
                shown.ends_with(&format!(
                    "condensed from {} lines; read_raw(handle=\"h1\", start_line=..., end_line=...) \
for the full output\n",
                    out.lines().count()
                )),
                "{shown}"
            );
            assert!(shown.contains("assertion `left == right` failed: refund for"));
            let raw = read_raw(&e, "h1");
            assert!(raw.contains("test ledger::case_03 ... ok"), "{raw}");
            for text in [&shown, &raw] {
                for value in [KEY, EMAIL, CUSTOMER, "Quennefeldt"] {
                    assert!(!text.contains(value), "{value} in:\n{text}");
                }
                assert!(text.contains("⟨email:"), "{text}");
            }
        }
    }

    #[test]
    fn a_value_is_judged_with_its_whole_context_before_lines_are_left_out() {
        // The label on a passing line the view omits still makes the number
        // on a kept line a card number.
        let (_d, e) = engine(policy(false));
        let mut out = String::from("exit code 1\n--- stdout ---\n");
        for i in 0..60 {
            out.push_str(&format!("[ OK ] golden case {i}\n"));
        }
        out.push_str(&format!(
            "[ OK ] masks the card\n[FAIL] refund\n    expected: {NUMBER}\n    actual:   none\n\n60 passed, 1 failed\n"
        ));
        let shown = e.present(&command("./golden.sh"), out.as_bytes());
        assert!(shown.contains("condensed (test counts)"), "{shown}");
        assert!(!shown.contains("masks the card"), "{shown}");
        assert!(!shown.contains(NUMBER), "{shown}");
        assert!(shown.contains("expected: ⟨card:"), "{shown}");
    }

    #[test]
    fn sensitive_output_is_held_as_before_never_condensed() {
        let (_d, e) = engine(policy(true));
        // Output of a command that read sensitive data.
        let held = e.present(
            &Source::SensitiveCommand {
                command: "cargo test".into(),
                exit_code: Some(101),
            },
            planted_output().as_bytes(),
        );
        assert_eq!(e.take_view_class(), Some(ViewClass::HandleSummary));
        assert!(
            held.contains("ask_local") && !held.contains("condensed"),
            "{held}"
        );
        // Output too long to show whole when command output is sensitive.
        let mut long = planted_output();
        while long.len() <= crate::engine::INLINE_OUTPUT_CHARS {
            long.push_str("test ledger::more ... ok\n");
        }
        let held = e.present(&command("cargo test"), long.as_bytes());
        assert_eq!(e.take_view_class(), Some(ViewClass::HandleSummary));
        assert!(!held.contains("condensed"), "{held}");
        let h = held.split_whitespace().next().unwrap();
        let Value::Object(args) = json!({"handle": h}) else {
            unreachable!()
        };
        assert!(e.call_tool("read_raw", &args).unwrap().is_err());
        for value in [KEY, EMAIL, CUSTOMER] {
            assert!(!held.contains(value), "{held}");
        }
    }

    #[test]
    fn checks_and_allowlisted_commands_are_condensed_and_off_means_as_before() {
        let (_d, e) = engine(policy(true));
        let checks = format!("$ cargo test\n{}", planted_output());
        let shown = e.present(&Source::Checks, checks.as_bytes());
        assert!(
            shown.starts_with("h1 (check output), condensed (cargo test):"),
            "{shown}"
        );
        assert!(!shown.contains(EMAIL), "{shown}");
        // Output the model filtered itself is shown as it asked.
        let shown = e.present(
            &command("cargo test 2>&1 | grep -v panicked"),
            planted_output().as_bytes(),
        );
        assert!(!shown.contains("condensed"), "{shown}");
        assert!(shown.contains("case_03 ... ok") && !shown.contains(EMAIL));

        let (_d, off) = engine(Policy {
            condense_output: false,
            ..policy(false)
        });
        let shown = off.present(&command("cargo test"), planted_output().as_bytes());
        assert!(!shown.contains("condensed") && shown.contains("case_03 ... ok"));
        assert!(!shown.contains(EMAIL), "{shown}");
    }

    #[test]
    fn unknown_long_output_falls_back_to_the_outline_without_a_local_model() {
        let (_d, e) = engine(policy(false));
        let log: String = (0..900)
            .map(|i| format!("worker picked job {i} from the queue\n"))
            .collect();
        let shown = e.present(&command("./bin/worker --once"), log.as_bytes());
        assert_eq!(e.take_view_class(), Some(ViewClass::BulkyHandle));
        assert!(!shown.contains("condensed"), "{shown}");
        assert!(shown.contains("too long to show whole") && shown.contains("Last lines"));
        assert!(!shown.contains("Summary by the local model"), "{shown}");
    }
}
