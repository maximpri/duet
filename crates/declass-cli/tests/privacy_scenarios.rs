// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end privacy scenarios for Declass's own boundary: whole hybrid runs and
//! sessions through the real frontier loop, with synthetic values planted in a
//! temporary git workspace, a scripted frontier that records every request
//! byte for byte, and local stand-ins that answer as they should or
//! carelessly. Each test asserts that no planted value, in any spelling the
//! canary matcher knows, reached the frontier. See `privacy/mod.rs`.
//!
//! A gap found here is closed at the class level, then its scenario stays as
//! a regression test. A scenario for a gap with no defence yet is marked
//! `#[ignore]` with a reason naming the gap (none is, at present).

mod privacy;

use declass_agent::Terminal;
use declass_agent::session::TurnEnd;
use declass_boundary::audit::AuditEvent;
use declass_boundary::testing::canary::Canaries;
use privacy::{
    CUSTOMERS, DB_PASSWORD, Echo, Fixture, KEY, Local, Options, Step, customer_line, first_handle,
    first_placeholder, latest_handle,
};
use serde_json::json;

/// A 17-digit number that fails the Luhn check.
const CARD_A: &str = "48613927054718395";
/// A Luhn-valid 16-digit number.
const CARD_B: &str = "4917352680941621";

/// Both stand-ins: the boundary must hold whether the local model describes
/// structure or repeats what it read (in the forms the engine is meant to stop).
const LOCALS: [Local; 2] = [Local::Cooperative, Local::Careless(Echo::HANDLED)];

fn completed(t: &Terminal) -> bool {
    matches!(t, Terminal::Completed { .. })
}

#[tokio::test(flavor = "multi_thread")]
async fn error_words_in_customer_data_do_not_publish_rows_as_diagnostics() {
    // Found in a real launch dogfood run: `.invalid` in an email selected
    // a CSV row as an error line. Pattern detection missed its arbitrary
    // customer ID, reserved-domain email and ordinary monetary amount.
    let f = Fixture::new(
        "data-diagnostics",
        "Inspect the customer export layout.",
        Local::Cooperative,
    );
    std::fs::write(
        f.ws.join("data/customers.csv"),
        "customer_id,name,email,status,amount\nCUST-CANARY-ORCHID,Mira Canarystone,mira.canary@privacy-fixture.invalid,active,125.50\n",
    )
    .unwrap();
    f.script(vec![
        Step::Call("read_file", json!({"path": "data/customers.csv"})),
        Step::Call("finish", json!({"summary": "Inspected the column layout."})),
    ]);
    assert!(completed(&f.run().await));
    f.assert_no_leak(&f.canaries([
        "CUST-CANARY-ORCHID",
        "Mira Canarystone",
        "mira.canary@privacy-fixture.invalid",
        "125.50",
    ]));
}

#[tokio::test(flavor = "multi_thread")]
async fn card_a_a_labelled_number_failing_the_checksum_never_reaches_the_frontier() {
    // Seen in a live run (DECLASS-2026-011): no detector took it for a card.
    let objective =
        format!("My credit card number is {CARD_A} and the payment form rejects it. Find out why.");
    let f = Fixture::new("carda", &objective, Local::Cooperative);
    f.script(vec![
        Step::Call("read_file", json!({"path": "src/lib.rs"})),
        Step::Call(
            "finish",
            json!({"summary": "The form checks the Luhn digit; that number fails it."}),
        ),
    ]);
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    f.assert_no_leak(&f.canaries([CARD_A]));
}

#[tokio::test(flavor = "multi_thread")]
async fn card_a_an_unlabelled_number_failing_the_checksum_never_reaches_the_frontier() {
    let objective = format!(
        "The payment form rejects {CARD_A} although the customer says it is their Visa. Find out why."
    );
    let f = Fixture::new("carda-unlabelled", &objective, Local::Cooperative);
    f.script(vec![Step::Call(
        "finish",
        json!({"summary": "The form checks the Luhn digit."}),
    )]);
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    f.assert_no_leak(&f.canaries([CARD_A]));
}

/// Card B as seen in a live run: the operator asks about a valid card number;
/// the frontier asks the local model about its placeholder, reads Declass's run
/// state with a `sensitive_data` command, and finishes naming the placeholder.
fn card_b(name: &str, local: Local) -> Fixture {
    let f = Fixture::new(
        name,
        &format!("Is this credit card number valid? {CARD_B}"),
        local,
    );
    f.script(vec![
        Step::From(Box::new(|body| {
            let token = first_placeholder(body).expect("the number as a placeholder");
            (
                "ask_local",
                json!({"handle": token, "questions": ["Does it pass the Luhn check? How many digits?"]}),
            )
        })),
        Step::Call(
            "run_command",
            json!({"command": "ls -R .declass; cat .declass/runs/*/vault.json; tail -n 20 .declass/runs/*/transcript.jsonl",
                "sensitive_data": true}),
        ),
        Step::From(Box::new(|body| {
            let token = first_placeholder(body).expect("the number as a placeholder");
            (
                "finish",
                json!({"summary": format!("{token} passes the Luhn check.")}),
            )
        })),
    ]);
    f
}

#[tokio::test(flavor = "multi_thread")]
async fn card_b_a_valid_card_number_stays_out_through_ask_local_and_a_run_state_read() {
    for (i, local) in LOCALS.into_iter().enumerate() {
        let f = card_b(&format!("cardb-{i}"), local);
        let end = f.run().await;
        assert!(completed(&end), "{local:?}: {end:?}");
        assert_eq!(f.unscripted(), 0);
        let planted = f.canaries([CARD_B]);
        f.assert_no_leak(&planted);
        // The sensitive command could not read Declass's run state: what the
        // local model was given of its output holds no vaulted value.
        for p in f.local_prompts() {
            if p.contains("(ran with sensitive data)") {
                assert!(
                    planted.find(&p).is_empty(),
                    "{local:?}: run state read: {p}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn card_b_the_placeholder_works_for_the_frontier_and_the_operator_sees_the_value() {
    let f = card_b("cardb-use", Local::Cooperative);
    let end = f.run().await;
    // The operator is shown their own value in the end state.
    assert_eq!(
        end,
        Terminal::Completed {
            summary: format!("{CARD_B} passes the Luhn check.")
        }
    );
    // The placeholder was a handle: the local model read the operator's
    // message, and its answer reached the frontier with its own words intact
    // (the algorithm and the network it named are not people).
    let answers = f.results_of("ask_local");
    assert_eq!(answers.len(), 1);
    assert!(
        answers[0].contains("16-digit number; the Luhn Algorithm accepts it"),
        "{}",
        answers[0]
    );
    let summaries = f.results_of("run_command");
    assert!(
        summaries[0].contains("the Luhn Algorithm; most start in the Visa Network range"),
        "{}",
        summaries[0]
    );
    let prompts = f.local_prompts();
    assert!(
        prompts.iter().any(|p| p.contains(CARD_B)),
        "the local model never read the operator's value"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sensitive_data_command_cannot_read_the_vault() {
    for (i, local) in LOCALS.into_iter().enumerate() {
        let f = Fixture::new(
            &format!("vault-{i}"),
            "Check that the customer export matches the billing records.",
            local,
        );
        let vault = format!(".declass/runs/{}/vault.json", f.run_id);
        f.script(vec![
            Step::Call("read_file", json!({"path": "data/customers.csv"})),
            Step::Call(
                "run_command",
                json!({"command": format!("cat {vault}; od -c {vault} | head -40"), "sensitive_data": true}),
            ),
            Step::ask_latest("What does the output contain? Quote line 1."),
            Step::Call("run_command", json!({"command": format!("cat {vault}")})),
            Step::Call("finish", json!({"summary": "Checked."})),
        ]);
        let end = f.run().await;
        assert!(completed(&end), "{local:?}: {end:?}");
        assert_eq!(f.unscripted(), 0);
        // The vault exists and holds the primed values.
        let stored = std::fs::read_to_string(f.ws.join(&vault)).unwrap();
        assert!(stored.contains(CUSTOMERS[0].card), "vault not primed");
        let planted = f.canaries([]);
        f.assert_no_leak(&planted);
        for p in f.local_prompts() {
            if p.contains("(ran with sensitive data)") {
                assert!(planted.find(&p).is_empty(), "{local:?}: vault read: {p}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_derived_by_a_sensitive_data_command_stays_sensitive_when_read() {
    // A careless whole-line echo of a transformed copy (reversed names) is the
    // copy-window gap below, not a derived-file one: only digit runs here.
    for (i, local) in [Local::Cooperative, Local::Careless(&[Echo::Digits])]
        .into_iter()
        .enumerate()
    {
        let f = Fixture::new(
            &format!("derived-{i}"),
            "Prepare an export of the customer list for the billing team.",
            local,
        );
        f.script(vec![
            Step::Call(
                "run_command",
                json!({"command": "tr a-z A-Z < data/customers.csv > export.txt && rev data/customers.csv > export.rev \
                    && base64 < data/customers.csv > export.b64",
                    "sensitive_data": true}),
            ),
            Step::Call("read_file", json!({"path": "export.txt"})),
            Step::Call("read_file", json!({"path": "export.rev"})),
            Step::Call("read_file", json!({"path": "export.b64"})),
            Step::Call("read_file", json!({"path": "export.rev", "start_line": 2, "end_line": 3})),
            Step::Call("run_command", json!({"command": "cat export.txt export.rev export.b64"})),
            Step::Call("search", json!({"pattern": "[A-Z]{6,}|@"})),
            Step::Call("finish", json!({"summary": "Export written."})),
        ]);
        let end = f.run().await;
        assert!(completed(&end), "{local:?}: {end:?}");
        assert_eq!(f.unscripted(), 0);
        assert!(f.ws.join("export.b64").exists());
        f.assert_no_leak(&f.canaries([]));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_derived_into_target_stays_sensitive_when_read() {
    let f = Fixture::new(
        "derived-target",
        "Prepare an export of the customer list for the billing team.",
        Local::Cooperative,
    );
    f.script(vec![
        Step::Call(
            "run_command",
            json!({"command": "mkdir -p target && tr a-z A-Z < data/customers.csv > target/export.txt \
                && rev data/customers.csv > target/export.rev",
                "sensitive_data": true}),
        ),
        Step::Call("read_file", json!({"path": "target/export.txt"})),
        Step::Call("read_file", json!({"path": "target/export.rev"})),
        Step::Call("finish", json!({"summary": "Export written."})),
    ]);
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_public_test_printing_customer_data_is_condensed_without_it() {
    // A public test copied a customer's row into its fixture. Its run's
    // output (command output under the shipped policy) is shown condensed,
    // the failure kept, and the whole output read back with read_raw: the
    // values stay replaced in both.
    for (i, local) in LOCALS.into_iter().enumerate() {
        let f = Fixture::new(
            &format!("condensed-{i}"),
            "Fix the failing refund test.",
            local,
        );
        let c = &CUSTOMERS[1];
        let mut script = String::from("echo; echo 'running 41 tests'\n");
        for k in 0..40 {
            script.push_str(&format!("echo 'test refunds::case_{k:02} ... ok'\n"));
        }
        script.push_str(&format!(
            "echo 'test refunds::refund_to_card ... FAILED'; echo; echo 'failures:'; echo\n\
echo '---- refunds::refund_to_card stdout ----'\n\
echo \"thread 'refunds::refund_to_card' panicked at tests/refunds.rs:31:5:\"\n\
echo 'assertion `left == right` failed: refund for {} <{}>'\n\
echo '  left: \"{}\"'; echo ' right: \"none\"'; echo; echo 'failures:'\n\
echo '    refunds::refund_to_card'; echo\n\
echo 'test result: FAILED. 40 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s'\n\
exit 101\n",
            c.name, c.email, c.card
        ));
        std::fs::create_dir_all(f.ws.join("tests")).unwrap();
        std::fs::write(f.ws.join("tests/refunds.sh"), script).unwrap();
        f.script(vec![
            Step::Call("run_command", json!({"command": "sh tests/refunds.sh"})),
            Step::From(Box::new(|body| {
                let shown = body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .rev()
                    .find(|m| m["role"] == "tool")
                    .and_then(|m| m["content"].as_str())
                    .unwrap_or_default();
                let handle = shown.split_whitespace().next().unwrap_or_default();
                (
                    "read_raw",
                    json!({"handle": handle, "start_line": 1, "end_line": 60}),
                )
            })),
            Step::Call("finish", json!({"summary": "Found the failing refund."})),
        ]);
        let end = f.run().await;
        assert!(completed(&end), "{local:?}: {end:?}");
        assert_eq!(f.unscripted(), 0);
        let shown = &f.results_of("run_command")[0];
        assert!(shown.contains("condensed (cargo test)"), "{shown}");
        assert!(
            shown.contains("refunds::refund_to_card ... FAILED"),
            "{shown}"
        );
        let read = &f.results_of("read_raw")[0];
        assert!(read.contains("refunds::case_05 ... ok"), "{read}");
        f.assert_no_leak(&f.canaries([]));
    }
}

/// Ten `ask_local` calls about one card number, each a narrow question: the
/// first digit, the last four, the length, then digits 2 to 8 one by one.
fn probing() -> Fixture {
    let f = Fixture::new(
        "probing",
        "Find out why the second customer's card is declined.",
        Local::Careless(&[]),
    );
    let line = customer_line(1);
    let mut steps = vec![Step::Call(
        "read_file",
        json!({"path": "data/customers.csv"}),
    )];
    let mut questions = vec![
        format!("What is the first digit of the card number on line {line}?"),
        format!("What are the last four digits of the card number on line {line}?"),
        format!("How many digits does the card number on line {line} have?"),
    ];
    questions
        .extend((2..=8).map(|n| format!("What is digit {n} of the card number on line {line}?")));
    // Every call names the handle of the file read first.
    for q in questions {
        steps.push(Step::From(Box::new(move |body| {
            let handle = first_handle(body).expect("the file's handle");
            ("ask_local", json!({"handle": handle, "question": q}))
        })));
    }
    steps.push(Step::Call(
        "finish",
        json!({"summary": "Declined by the issuer."}),
    ));
    f.script(steps);
    f
}

#[tokio::test(flavor = "multi_thread")]
async fn narrow_questions_about_one_value_repeat_no_run_of_its_digits() {
    let f = probing();
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    assert_eq!(f.unscripted(), 0);
    assert_eq!(f.results_of("ask_local").len(), 10);
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn narrow_questions_about_one_value_do_not_add_up_to_it() {
    let f = probing();
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    // What the frontier can assemble: the digits it was given for positions
    // 1 to 8 (answers 1 and 4 to 10), in order.
    let answers = f.results_of("ask_local");
    let assembled: String = [0, 3, 4, 5, 6, 7, 8, 9]
        .iter()
        .filter_map(|&i| {
            // "The first digit is 5.", "Digit 5 is 7.": the digit follows "is".
            let (_, after) = answers[i].split_once(" is ")?;
            after.chars().next().filter(char::is_ascii_digit)
        })
        .collect();
    let found = Canaries::new([CUSTOMERS[1].card]).find(&assembled);
    assert!(
        found.is_empty(),
        "the frontier assembled {assembled:?} of {} from {} answers: {answers:?}",
        CUSTOMERS[1].card,
        answers.len()
    );
}

/// The frontier reads the customer file and asks the local model about the
/// first customer's record; the local model answers carelessly in `echo`.
async fn careless(name: &str, echo: &'static [Echo]) -> Fixture {
    let f = Fixture::new(
        name,
        "Find out why the second customer's card is declined.",
        Local::Careless(echo),
    );
    f.script(vec![
        Step::Call("read_file", json!({"path": "data/customers.csv"})),
        Step::ask_latest(format!("Describe the record on line {}.", customer_line(0))),
        Step::Call("finish", json!({"summary": "Declined by the issuer."})),
    ]);
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    assert_eq!(f.unscripted(), 0);
    f
}

#[tokio::test(flavor = "multi_thread")]
async fn a_careless_local_answer_repeating_a_whole_line_is_cleaned() {
    let f = careless("careless-line", &[Echo::Line]).await;
    let answer = &f.results_of("ask_local")[0];
    assert!(answer.contains("Line 2 reads:"), "{answer}");
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_careless_local_answer_repeating_a_whole_line_leaves_no_field_of_it() {
    let f = careless("careless-line-field", &[Echo::Line]).await;
    let born = CUSTOMERS.map(|c| c.born);
    f.assert_no_leak(&f.canaries(born));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_careless_local_answer_repeating_digit_runs_is_cleaned() {
    let f = careless("careless-digits", &[Echo::Digits]).await;
    let answer = &f.results_of("ask_local")[0];
    assert!(answer.contains("starts with"), "{answer}");
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_careless_local_answer_spelling_a_value_out_is_cleaned() {
    let f = careless("careless-spaced", &[Echo::Spaced]).await;
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_careless_local_answer_encoding_a_value_is_cleaned() {
    let f = careless("careless-base64", &[Echo::Base64]).await;
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_in_a_steering_message_is_replaced() {
    let f = Fixture::new(
        "steer",
        "Look at src/lib.rs; total should skip refunds.",
        Local::Cooperative,
    );
    f.script(vec![
        Step::Call(
            "reply",
            json!({"message": "Noted; total already skips refunds."}),
        ),
        Step::Call("reply", json!({"message": "Understood."})),
    ]);
    let mut s = f.session();
    // Queued as the turn starts: delivered at its first safe point, as a
    // message typed while it runs is.
    s.steering().steer(format!(
        "Also: the payment key {KEY} was pasted into a log; rotate it."
    ));
    let first = s.turn(&f.cfg.objective).await;
    assert_eq!(first.state(), "replied", "{first:?}");
    assert_eq!(s.history()[0].steered.len(), 1, "steering not delivered");
    let second = s
        .turn(&format!("Use {KEY} only through the environment."))
        .await;
    assert!(matches!(second, TurnEnd::Replied { .. }), "{second:?}");
    let _ = s.end(true);
    f.assert_no_leak(&f.canaries([]));
}

/// A session in which the operator types the database password from `.env`.
async fn password_in_a_message(name: &str, options: Options) -> Fixture {
    let f = Fixture::with(
        name,
        "Look at src/lib.rs; total should skip refunds.",
        Local::Cooperative,
        options,
    );
    f.script(vec![
        Step::Call("reply", json!({"message": "Noted."})),
        Step::Call("reply", json!({"message": "Understood."})),
    ]);
    let mut s = f.session();
    assert_eq!(s.turn(&f.cfg.objective).await.state(), "replied");
    s.steering().steer(format!(
        "The database password is {DB_PASSWORD}; keep it out of the code."
    ));
    assert_eq!(s.turn("Carry on.").await.state(), "replied");
    let _ = s.end(true);
    f
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_from_a_tracked_env_file_typed_by_the_operator_is_replaced() {
    let f = password_in_a_message(
        "password-tracked",
        Options {
            env_ignored: false,
            ..Options::default()
        },
    )
    .await;
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_from_an_ignored_env_file_typed_by_the_operator_is_replaced() {
    let f = password_in_a_message("password-ignored", Options::default()).await;
    f.assert_no_leak(&f.canaries([]));
}

// --- Images -----------------------------------------------------------------
// Detectors cannot read an image. The workspace holds screenshots of the
// customer table (on a public path and on a sensitive one) and the local
// model reads images; every other image setting is shipped
// (`images.to_frontier = "never"`, `frontier.vision` off).

fn images() -> Options {
    Options {
        images: true,
        ..Options::default()
    }
}

/// No image data in any request.
fn no_image_sent(f: &Fixture) {
    for body in f.bodies() {
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("data:image/"),
            "an image reached the frontier"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn image_a_screenshot_of_customer_data_reaches_the_frontier_only_as_a_cleaned_description() {
    for (i, local) in LOCALS.into_iter().enumerate() {
        let f = Fixture::with(
            &format!("image-{i}"),
            "The export screen looks wrong; compare it with the scanned form.",
            local,
            images(),
        );
        f.script(vec![
            Step::Call("read_file", json!({"path": "docs/export-screen.png"})),
            Step::ask_latest("Which name, email and card number does the first row show?"),
            Step::Call("read_file", json!({"path": "data/scan.png"})),
            Step::ask_latest("Transcribe the scan exactly."),
            Step::Call(
                "finish",
                json!({"summary": "Compared the screen with the scan."}),
            ),
        ]);
        let end = f.run().await;
        assert!(completed(&end), "{local:?}: {end:?}");
        assert_eq!(f.unscripted(), 0);
        f.assert_no_leak(&f.canaries([]));
        no_image_sent(&f);
        // The local model saw each image for its description and the question.
        let shown = f
            .local
            .bodies()
            .iter()
            .filter(|b| b.to_string().contains("data:image/png;base64,"))
            .count();
        assert_eq!(shown, 4, "{local:?}");
        let results = f.results_of("read_file");
        assert!(
            results.iter().all(|r| r.contains("ask_local(handle=")),
            "{results:?}"
        );
        // The careless stand-in transcribed the rows; what arrived are placeholders.
        if matches!(local, Local::Careless(_)) {
            assert!(results.iter().all(|r| r.contains('⟨')), "{results:?}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn image_the_operator_attaches_from_a_sensitive_path_is_described_never_sent() {
    let f = Fixture::with(
        "image-attached",
        "What does this scan show? Make the import accept it.",
        Local::Careless(Echo::HANDLED),
        images(),
    );
    f.script(vec![Step::Call(
        "finish",
        json!({"summary": "The import now accepts the scanned layout."}),
    )]);
    let mut s = f.session();
    // Marked public, a sensitive-path image is still refused.
    let refused = s.attach(f.ws.join("data/scan.png"), true).unwrap_err();
    assert!(refused.contains("sensitive path"), "{refused}");
    s.attach(f.ws.join("data/scan.png"), false).unwrap();
    assert_eq!(s.turn(&f.cfg.objective).await.state(), "completed");
    let _ = s.end(true);
    f.assert_no_leak(&f.canaries([]));
    no_image_sent(&f);
    let first = f.bodies()[0].clone();
    assert!(
        String::from_utf8_lossy(&first).contains("[image attached by the operator]"),
        "the description is in the message"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn image_a_screenshot_of_an_unindexed_password_is_described_without_it() {
    let f = Fixture::with(
        "image-password",
        "The deploy script prints something odd; see the screenshot.",
        Local::Careless(Echo::HANDLED),
        images(),
    );
    f.script(vec![
        Step::Call("read_file", json!({"path": "docs/terminal.png"})),
        Step::Call("finish", json!({"summary": "done"})),
    ]);
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    no_image_sent(&f);
    f.assert_no_leak(&f.canaries([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn structure_views_samples_and_masked_output_carry_no_planted_value() {
    // The frontier is told the structure of the sensitive files up front,
    // reads one (structure view and a sample record), asks for a sample,
    // reads `.env` (value formats) and runs programs on the data (masked
    // output). No planted value reaches it in any spelling, dates of birth
    // included: no detector knows them, and the views never show a value.
    for (i, local) in LOCALS.into_iter().enumerate() {
        let f = Fixture::new(
            &format!("structure-{i}"),
            "Add a CSV import for the customer export.",
            local,
        );
        f.script(vec![
            Step::Call("read_file", json!({"path": "data/customers.csv"})),
            Step::From(Box::new(|body| {
                let handle = latest_handle(body).expect("a handle");
                ("synthetic_sample", json!({"handle": handle, "rows": 3}))
            })),
            Step::Call("read_file", json!({"path": ".env"})),
            Step::Call(
                "run_command",
                json!({"command": "for i in 1 2 3 4 5 6; do cat data/customers.csv; done", "sensitive_data": true}),
            ),
            Step::Call(
                "run_command",
                json!({"command": "head -2 data/customers.csv | tail -1", "sensitive_data": true}),
            ),
            Step::Call("finish", json!({"summary": "Checked the export's layout."})),
        ]);
        let end = f.run().await;
        assert!(completed(&end), "{local:?}: {end:?}");
        assert_eq!(f.unscripted(), 0);
        f.assert_no_leak(&f.canaries(CUSTOMERS.map(|c| c.born)));
        // What the frontier learned instead.
        let task = f.task_message();
        assert!(
            task.contains("- data/customers.csv: CSV, 3 rows of 5 fields"),
            "{task}"
        );
        let read = f.results_of("read_file");
        assert!(
            read[0].contains("born — string; 3; all distinct; len 10; date yyyy-MM-dd ×3"),
            "{}",
            read[0]
        );
        assert!(
            read[0].contains("Synthetic sample (record 1 of 3"),
            "{}",
            read[0]
        );
        assert!(read[1].contains("DB_PASSWORD — "), "{}", read[1]);
        let sample = &f.results_of("synthetic_sample")[0];
        assert!(sample.contains("records 1, 2, 3 of 3"), "{sample}");
        assert!(sample.contains("id,name,born,email,card"), "{sample}");
        let runs = f.results_of("run_command");
        assert!(
            runs[1].contains("Output with every value masked"),
            "{}",
            runs[1]
        );
        assert!(runs[1].contains("Aa Aa,9999-99-99,"), "{}", runs[1]);
        let events = f.audit_events();
        assert!(events.iter().any(|e| matches!(e,
            AuditEvent::SyntheticSample { outcome, records: 3, .. } if outcome == "shown")));
        assert!(events.iter().any(|e| matches!(
            e,
            AuditEvent::OutputProbe {
                count: 1,
                shown: true,
                ..
            }
        )));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn grep_probes_of_the_env_file_stop_at_the_budget() {
    // Seen in a live run without a local model (S1, `results/nolocal-smoke`):
    // unable to ask about `.env`, the frontier ran 34 `sensitive_data`
    // commands such as `grep -q KEY .env && echo M` and read one bit from
    // each (a line, or none, and its byte count). Short outputs are probes:
    // past the run's budget (shipped: 12) the view of each is the same,
    // whatever the command printed, and every probe is audited.
    let f = Fixture::with(
        "grep-probes",
        "Load the settings: every key in .env is required.",
        Local::Cooperative,
        Options {
            no_local: true,
            ..Options::default()
        },
    );
    let keys = ["STRIPE_SECRET_KEY", "PORT", "DB_PASSWORD", "LOG_LEVEL"];
    let mut steps: Vec<Step> = (0..16)
        .map(|i| {
            let key = keys[i % keys.len()];
            Step::Call(
                "run_command",
                json!({"command": format!("grep -q '^{key}=' .env && echo M"), "sensitive_data": true}),
            )
        })
        .collect();
    steps.push(Step::Call("finish", json!({"summary": "Probed."})));
    f.script(steps);
    let end = f.run().await;
    assert!(completed(&end), "{end:?}");
    assert_eq!(f.unscripted(), 0);
    let results = f.results_of("run_command");
    assert_eq!(results.len(), 16);
    // Within the budget a probe still answers (the key is there or not)...
    assert!(results[0].contains("\n  M\n"), "{}", results[0]);
    assert!(!results[1].contains("\n  M\n"), "{}", results[1]);
    // ...past it, the view no longer depends on the output: it names the
    // command (the frontier's own text) and nothing else.
    let without_command = |i: usize| {
        let key = keys[i % keys.len()];
        results[i].replace(&format!("grep -q '^{key}=' .env && echo M"), "…")
    };
    for (i, r) in results.iter().enumerate().skip(12) {
        assert_eq!(without_command(i), without_command(12));
        assert!(r.contains("withheld"), "{r}");
    }
    assert!(without_command(12).contains("sensitivity.output_probes"));
    let probes: Vec<(u32, bool)> = f
        .audit_events()
        .into_iter()
        .filter_map(|e| match e {
            AuditEvent::OutputProbe { count, shown, .. } => Some((count, shown)),
            _ => None,
        })
        .collect();
    assert_eq!(probes.len(), 16);
    assert!(probes.iter().take(12).all(|p| p.1) && probes.iter().skip(12).all(|p| !p.1));
    // The structure of `.env` was in the task all along.
    let task = f.task_message();
    assert!(
        task.contains("- .env: KEY=value, 2 assignments: STRIPE_SECRET_KEY ("),
        "{task}"
    );
    f.assert_no_leak(&f.canaries([]));
}
