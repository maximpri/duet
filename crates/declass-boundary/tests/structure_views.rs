// SPDX-License-Identifier: GPL-3.0-or-later
//! Property: no value of a sensitive data file reaches the frontier through
//! its structure view, its synthetic sample or the masked output of a
//! command that printed it, in any spelling the canary matcher knows; and
//! the sample is a valid file of the same kind (it parses, its card numbers
//! pass Luhn, its IBANs mod-97, its dates are dates in the same layout).
//!
//! Each case writes random customers (invented names, emails, phone numbers,
//! card numbers, IBANs, dates of birth, amounts, codes, categorical
//! statuses and notes holding the delimiter and quotes) as CSV and as JSON,
//! primes an engine on them with structure views on, and collects every
//! text the frontier would be shown. `PROPTEST_CASES` sets the number of
//! cases (default 16).

use declass_boundary::engine::Engine;
use declass_boundary::policy::{Policy, StructureSettings};
use declass_boundary::testing::canary::{Canaries, Options};
use declass_boundary::view::{Presenter, Source};
use proptest::prelude::*;
use serde_json::json;
use std::sync::Arc;

const DEFAULT_CASES: u32 = 16;

fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CASES);
    ProptestConfig {
        cases,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

#[derive(Debug, Clone)]
struct Customer {
    first: String,
    last: String,
    email: String,
    phone: String,
    card: String,
    iban: String,
    born: String,
    amount: String,
    code: String,
    status: String,
    note: String,
}

/// Invented words: syllables that form no public word.
fn word(min: usize, max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(
        proptest::sample::select(vec![
            "zor", "vak", "quen", "dril", "thar", "sko", "vel", "wick", "tol", "ven", "rin", "ksa",
            "brix", "mo", "dza", "ulf", "ør", "æn", "ji",
        ]),
        min..=max,
    )
    .prop_map(|s| s.concat())
}

fn title(w: &str) -> String {
    let mut c = w.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn luhn_check(digits: &[u32]) -> u32 {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 0 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    (10 - sum % 10) % 10
}

fn card() -> impl Strategy<Value = String> {
    proptest::collection::vec(0u32..10, 14).prop_map(|body| {
        let mut d = vec![4];
        d.extend(body);
        d.push(luhn_check(&d));
        d.iter().map(ToString::to_string).collect()
    })
}

fn iban() -> impl Strategy<Value = String> {
    proptest::collection::vec(0u32..10, 18).prop_map(|body| {
        let bban: String = body.iter().map(ToString::to_string).collect();
        // DE: 22 characters; check digits from mod 97 of BBAN + "DE00".
        let digits = format!("{bban}131400");
        let rem = digits.chars().fold(0u64, |r, c| {
            (r * 10 + u64::from(c.to_digit(10).unwrap_or(0))) % 97
        });
        format!("DE{:02}{bban}", 98 - rem)
    })
}

fn customer() -> impl Strategy<Value = Customer> {
    (
        (word(2, 3), word(2, 3)),
        "[a-z]{3,8}\\.[a-z]{2,6}[0-9]{0,2}@[a-z]{4,8}-[0-9]{2,3}\\.(net|org|io)",
        "\\+1 \\([2-9][0-9]{2}\\) [2-9][0-9]{2}-[0-9]{4}",
        card(),
        iban(),
        "19[5-9][0-9]-(0[1-9]|1[0-2])-(0[1-9]|1[0-9]|2[0-8])",
        "[1-9][0-9]{3,4}\\.[0-9]{2}",
        "[A-Z]{3}-[0-9]{5}",
        proptest::sample::select(vec!["vakzorrin", "tolvenksa", "drilskoth"]),
        (word(2, 3), word(2, 3), 0usize..4),
    )
        .prop_map(
            |((first, last), email, phone, card, iban, born, amount, code, status, note)| {
                let (a, b, style) = note;
                let note = match style {
                    0 => format!("{a} {b}"),
                    1 => format!("{a}, {b}"),
                    2 => format!("say \"{a}\" to {b}"),
                    _ => String::new(),
                };
                Customer {
                    first: title(&first),
                    last: title(&last),
                    email,
                    phone,
                    card,
                    iban,
                    born,
                    amount,
                    code,
                    status: status.to_owned(),
                    note,
                }
            },
        )
}

fn csv_field(v: &str) -> String {
    if v.contains([',', '"', '\n']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_owned()
    }
}

fn files(customers: &[Customer]) -> (String, String) {
    let mut csv = String::from("id,name,email,phone,card,iban,born,amount,code,status,note\n");
    let mut records = Vec::new();
    for (i, c) in customers.iter().enumerate() {
        let name = format!("{} {}", c.first, c.last);
        let fields = [
            (i + 1).to_string(),
            name.clone(),
            c.email.clone(),
            c.phone.clone(),
            c.card.clone(),
            c.iban.clone(),
            c.born.clone(),
            c.amount.clone(),
            c.code.clone(),
            c.status.clone(),
            c.note.clone(),
        ];
        csv.push_str(
            &fields
                .iter()
                .map(|f| csv_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        csv.push('\n');
        records.push(json!({
            "id": i + 1,
            "customer": {"name": name, "email": c.email, "phone": c.phone},
            "payment": {"card": c.card, "iban": c.iban, "amount": c.amount.parse::<f64>().unwrap_or(0.0)},
            "born": c.born,
            "code": c.code,
            "status": c.status,
            "note": if c.note.is_empty() { serde_json::Value::Null } else { json!(c.note) },
        }));
    }
    let json = serde_json::to_string_pretty(&json!({"month": "2026-09", "customers": records}))
        .unwrap_or_default();
    (csv, json)
}

/// Every value planted, 6 characters or longer (shorter ones, such as an
/// id, occur in any text by chance).
fn planted(customers: &[Customer]) -> Vec<String> {
    let mut v = Vec::new();
    for c in customers {
        v.extend([
            format!("{} {}", c.first, c.last),
            c.first.clone(),
            c.last.clone(),
            c.email.clone(),
            c.phone.clone(),
            c.card.clone(),
            c.iban.clone(),
            c.born.clone(),
            c.amount.clone(),
            c.code.clone(),
            c.status.clone(),
        ]);
        v.extend(
            c.note
                .split(|x: char| !x.is_alphanumeric())
                .map(str::to_owned),
        );
    }
    v.retain(|x| x.chars().count() >= 6);
    v.sort();
    v.dedup();
    v
}

fn numbered(text: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(i, l)| format!("{:>4}  {l}\n", i + 1))
        .collect()
}

/// Every text the frontier would be shown of the two files.
fn shown(csv: &str, json: &str) -> (tempfile::TempDir, Vec<(String, String)>) {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    let run = d.path().join("run");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(ws.join("data/customers.csv"), csv).unwrap();
    std::fs::write(ws.join("data/customers.json"), json).unwrap();
    std::fs::write(
        ws.join("src/lib.rs"),
        "// Imports the customer export: rows, names, cards and notes.\nfn main() {}\n",
    )
    .unwrap();
    let policy = Policy {
        sensitive_globs: vec!["data/**".into()],
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        structure: StructureSettings {
            views: true,
            synthetic_rows: 20,
            masked_numbers: 24,
            output_probes: Some(12),
        },
        ..Policy::default()
    };
    let e: Arc<Engine> = Engine::open(&run, policy, None).unwrap();
    e.prime(
        &ws,
        &[
            "data/customers.csv".into(),
            "data/customers.json".into(),
            "src/lib.rs".into(),
        ],
        "Import the customer export.",
    );
    let mut out = vec![(
        "task".to_owned(),
        e.sanitize_objective("Import the customer export."),
    )];
    for (path, text) in [("data/customers.csv", csv), ("data/customers.json", json)] {
        let view = e.present(
            &Source::File {
                path: path.into(),
                ranged: false,
            },
            numbered(text).as_bytes(),
        );
        let handle = view
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        out.push((format!("view of {path}"), view));
        let sample = e
            .call_tool(
                "synthetic_sample",
                json!({"handle": handle, "rows": 20}).as_object().unwrap(),
            )
            .unwrap();
        out.push((
            format!("sample of {path}"),
            sample.unwrap_or_else(|withheld| withheld),
        ));
    }
    let masked = e.present(
        &Source::SensitiveCommand {
            command: "cat data/customers.csv".into(),
            exit_code: Some(0),
        },
        format!("exit code 0\n--- stdout ---\n{csv}").as_bytes(),
    );
    out.push(("masked output".into(), masked));
    (d, out)
}

fn luhn_ok(digits: &str) -> bool {
    let d: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    d.split_last()
        .is_some_and(|(&check, body)| luhn_check(body) == check)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn no_value_leaks_through_views_samples_or_masked_output(
        customers in proptest::collection::vec(customer(), 2..8),
    ) {
        let (csv, json) = files(&customers);
        let (_d, texts) = shown(&csv, &json);
        // Fakes are drawn at random, so a run of 4 or 5 digits of one can
        // equal a run of a real number by chance (a fake depends on shapes
        // only, so nothing flows); 6 digits by chance is one in a million.
        let canaries = Canaries::with_options(
            planted(&customers),
            Options { digit_min: 6, ..Options::default() },
        );
        for (what, text) in &texts {
            let found = canaries.find(text);
            prop_assert!(
                found.is_empty(),
                "{what} shows {:?}:\n{text}",
                found
                    .iter()
                    .map(|f| (&canaries.values()[f.canary_index], f.form))
                    .collect::<Vec<_>>()
            );
        }
        // The samples are files of the same kind, with valid fakes.
        let sample = |i: usize| -> String {
            texts[i].1.split_once('\n').map(|(_, body)| body.to_owned()).unwrap_or_default()
        };
        let csv_sample = sample(2);
        let lines: Vec<&str> = csv_sample.lines().collect();
        prop_assert!(lines[0].starts_with("id,name,email"), "{csv_sample}");
        prop_assert_eq!(lines.len(), customers.len() + 1);
        let v: serde_json::Value = serde_json::from_str(&sample(4)).map_err(|e| {
            TestCaseError::fail(format!("the JSON sample does not parse: {e}\n{}", sample(4)))
        })?;
        let records = v["customers"].as_array().cloned().unwrap_or_default();
        prop_assert_eq!(records.len(), customers.len());
        for r in &records {
            let card = r["payment"]["card"].as_str().unwrap_or_default();
            prop_assert!(card.len() == 16 && luhn_ok(card), "{card}");
            let iban = r["payment"]["iban"].as_str().unwrap_or_default();
            prop_assert!(declass_boundary::pii::iban_checksum(iban), "{iban}");
            let born = r["born"].as_str().unwrap_or_default();
            prop_assert_eq!(
                declass_boundary::structure::dates::recognize(born).map(|l| l.picture()),
                Some("yyyy-MM-dd".to_owned())
            );
        }
    }
}

#[test]
fn a_twin_holds_no_value_the_knowledge_knows_whatever_the_seed() {
    // The generator alone, with knowledge of every value: over many seeds
    // no fake holds a known value (it is redrawn) and the check the engine
    // runs on the whole sample would pass.
    struct Knows(Vec<String>);
    impl declass_boundary::structure::Knowledge for Knows {
        fn values(&self, text: &str) -> Vec<(usize, usize, declass_boundary::detect::Kind)> {
            self.0
                .iter()
                .filter_map(|v| text.find(v.as_str()).map(|at| (at, at + v.len())))
                .map(|(s, e)| (s, e, declass_boundary::detect::Kind::Data))
                .collect()
        }
        fn public_word(&self, _: &str) -> bool {
            false
        }
        fn data_word(&self, _: &str) -> bool {
            true
        }
        fn known(&self, text: &str) -> bool {
            self.0.iter().any(|v| text.contains(v.as_str()))
        }
    }
    let mut csv = String::from("code,pin,note\n");
    let mut values = Vec::new();
    for i in 0..40 {
        let code = format!("{:04}", (i * 37) % 10_000);
        let pin = format!("{}", 1000 + i * 7);
        csv.push_str(&format!("{code},{pin},note {i}\n"));
        values.extend([code, pin]);
    }
    let k = Knows(values.clone());
    for seed in 0..50 {
        let t = declass_boundary::structure::twin::twin(
            &csv,
            Some(std::path::Path::new("data/c.csv")),
            &k,
            seed,
            40,
        )
        .unwrap();
        assert_eq!(t.stuck, 0);
        let body = t.text.split_once('\n').unwrap().1;
        for v in &values {
            assert!(
                !body.contains(v.as_str()),
                "{v} in the twin (seed {seed}):\n{body}"
            );
        }
    }
}
