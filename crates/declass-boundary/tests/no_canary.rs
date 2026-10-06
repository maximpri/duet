// SPDX-License-Identifier: GPL-3.0-or-later
//! Property: no canary survives the gate.
//!
//! Sensitive files (`.env`, a CSV, a log) are generated with canaries of every
//! shape the engine indexes, the engine is primed on them as at run start, and
//! outbound requests are generated that carry those canaries in every place a
//! request can hold text: user text, tool results, assistant text, reasoning
//! and tool-call arguments (JSON inside JSON). The outbound filter and the
//! final check run as the gate runs them.
//!
//! Asserted, for the body of every wire dialect (Chat Completions, Anthropic
//! Messages, Responses): after the filter, the check passes and no covered
//! spelling of any canary is in the request body, raw or JSON-decoded; and a
//! body that still holds one (6+ bytes) is refused by the check.
//!
//! Covered spellings (what the value filters claim, and all this test asserts):
//! - secrets from `.env` (any characters, including `\` and `"`), provider-shaped
//!   tokens, emails, phone numbers, card numbers, IBANs and IPv4 addresses:
//!   exactly as they appear in the sensitive file;
//! - person names (two or more title-case words, or the value of a person field
//!   such as `name=`): as written, and the surname alone when it has 4+ letters
//!   and is not a word of public content;
//! - long numbers (6+ digits, plain or comma-grouped): as written, plain
//!   digits, comma-grouped, and read as minor units (`51861.26`, `51,861.26`).
//!
//! Also asserted: no transformed spelling of a canary value is left in the
//! filtered body (`testing::canary`: case, escaping, base64/hex of 8+ bytes,
//! reversed, separator-split, 4+ digit runs of mostly-digit values, spelled
//! digits). Only covered spellings are placed, so a finding here means the
//! filter left part of a value behind (a digit run, say), not that a
//! re-encoding got through.
//!
//! Not covered by value filters, by design: values under 4 bytes (replaced
//! only where a detector sees them; the final check needs 6+), a first name
//! alone, other letter cases, other number formats, and any re-encoding
//! (base64, hex, reversed, split, character codes). A program that could
//! re-encode sensitive content never gets to read it: sensitive paths are
//! unreadable to commands (OS sandbox), which is access control, not filtering.

use declass_boundary::engine::Engine;
use declass_boundary::model::{Item, Request, ToolCall};
use declass_boundary::policy::Policy;
use declass_boundary::testing::canary::{Canaries as Matcher, Options};
use declass_provider::Dialect;

const DIALECTS: [Dialect; 3] = [Dialect::Chat, Dialect::Anthropic, Dialect::Responses];
use proptest::prelude::*;
use serde_json::{Map, Value, json};
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

/// Words of the public file and the task (single words found here are not identifying).
const PUBLIC_SRC: &str = "// Reconciles customer balances and sends the monthly report.\n\
pub fn reconcile(balance: i64, customer: &str) -> String { format!(\"{customer} {balance}\") }\n";
const OBJECTIVE: &str = "Fix the balance rounding in the monthly report for each customer.";

#[derive(Debug, Clone)]
struct Canaries {
    env_secrets: Vec<String>,
    tokens: Vec<String>,
    emails: Vec<String>,
    phones: Vec<String>,
    ips: Vec<String>,
    cards: Vec<String>,
    ibans: Vec<String>,
    names: Vec<(String, String)>,
    single_names: Vec<String>,
    numbers: Vec<u64>,
    grouped: Vec<u64>,
}

/// Invented words: syllables that do not form public words.
fn word(min: usize, max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(
        proptest::sample::select(vec![
            "zor", "vak", "quen", "dril", "thar", "sko", "vel", "wick", "tol", "ven", "rin", "ksa",
            "yl", "brix", "mo", "dza", "ulf", "ør", "æn", "ji",
        ]),
        min..=max,
    )
    .prop_map(|s| {
        let w: String = s.concat();
        let mut c = w.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or_default()
    })
}

fn canaries() -> impl Strategy<Value = Canaries> {
    let env_secret = "[A-Za-z0-9][A-Za-z0-9!@%+/.^*\\\\\"-]{8,20}[A-Za-z0-9]";
    let token = prop_oneof![
        "sk_live_[A-Za-z0-9]{24}",
        "ghp_[A-Za-z0-9]{36}",
        "AKIA[0-9A-Z]{16}",
        "xoxb-[0-9]{6}-[A-Za-z0-9]{12}",
    ];
    let email = "[a-z]{3,8}\\.[a-z]{2,6}[0-9]{0,2}@[a-z]{4,8}-[0-9]{2,3}\\.(net|org|io|de)";
    let phone = prop_oneof![
        "\\+1 \\([2-9][0-9]{2}\\) [2-9][0-9]{2}-[0-9]{4}",
        "[2-9][0-9]{2}-[2-9][0-9]{2}-[0-9]{4}",
        "\\+44 [2-9][0-9]{2} [2-9][0-9]{2} [0-9]{4}",
    ];
    let ip = "(1[0-9]|[2-9][0-9])\\.(1[0-9]{2})\\.([1-9][0-9])\\.(1[0-9]{2})";
    (
        proptest::collection::vec(env_secret, 1..4),
        proptest::collection::vec(token, 1..3),
        proptest::collection::vec(email, 1..4),
        proptest::collection::vec(phone, 1..3),
        proptest::collection::vec(ip, 0..2),
        proptest::collection::vec(card(), 0..2),
        proptest::collection::vec(iban(), 0..2),
        proptest::collection::vec((word(1, 3), word(2, 3)), 1..4),
        proptest::collection::vec(word(2, 3), 1..3),
        proptest::collection::vec(100_000u64..10_000_000_000, 1..4),
        proptest::collection::vec(1_000_000u64..10_000_000_000, 1..3),
    )
        .prop_map(
            |(
                env_secrets,
                tokens,
                emails,
                phones,
                ips,
                cards,
                ibans,
                names,
                single_names,
                numbers,
                grouped,
            )| {
                Canaries {
                    env_secrets,
                    tokens,
                    emails,
                    phones,
                    ips,
                    cards,
                    ibans,
                    names,
                    single_names,
                    numbers,
                    grouped,
                }
            },
        )
}

/// A Luhn-valid 16-digit card number, plain or in groups of four.
fn card() -> impl Strategy<Value = String> {
    (proptest::collection::vec(0u32..10, 15), any::<bool>()).prop_map(|(mut d, spaced)| {
        d[0] = 4;
        let sum: u32 = d
            .iter()
            .rev()
            .enumerate()
            .map(|(i, &x)| {
                if i % 2 == 0 {
                    (x * 2) % 9 + u32::from(x == 9) * 9
                } else {
                    x
                }
            })
            .sum();
        d.push((10 - sum % 10) % 10);
        let digits: String = d.iter().map(|x| char::from(b'0' + *x as u8)).collect();
        if spaced {
            digits
                .as_bytes()
                .chunks(4)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            digits
        }
    })
}

/// A German IBAN with valid check digits.
fn iban() -> impl Strategy<Value = String> {
    "[0-9]{18}".prop_map(|bban| {
        // DE = 13 14; check digits = 98 - (bban + "131400") mod 97.
        let rem = format!("{bban}131400")
            .bytes()
            .fold(0u32, |r, b| (r * 10 + u32::from(b - b'0')) % 97);
        format!("DE{:02}{bban}", 98 - rem)
    })
}

fn group(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The sensitive files, with canaries placed next to each other using `seps`.
fn files(c: &Canaries, seps: &[&str]) -> Vec<(String, String)> {
    let sep = |i: usize| seps[i % seps.len()];
    let mut env = String::from("# service credentials\n");
    for (i, s) in c.env_secrets.iter().enumerate() {
        let export = if i % 2 == 0 { "" } else { "export " };
        env.push_str(&format!("{export}SERVICE_{i}_TOKEN={s}\n"));
    }
    let mut csv = String::from("id,name,email,phone,balance\n");
    for (i, (first, last)) in c.names.iter().enumerate() {
        csv.push_str(&format!(
            "C-{i},{first} {last},{},{},{}\n",
            c.emails[i % c.emails.len()],
            c.phones[i % c.phones.len()],
            c.numbers[i % c.numbers.len()],
        ));
    }
    let mut log = String::new();
    for (i, t) in c.tokens.iter().enumerate() {
        log.push_str(&format!("INFO charge ok{}token {t}\n", sep(i)));
    }
    for (i, n) in c.single_names.iter().enumerate() {
        log.push_str(&format!("WARN retry name={n}; attempt {i}\n"));
        log.push_str(&format!("{{\"display_name\": \"{n}\", \"ok\": false}}\n"));
    }
    for (i, g) in c.grouped.iter().enumerate() {
        log.push_str(&format!(
            "INFO settled{}{}{}EUR\n",
            sep(i),
            group(*g),
            sep(i + 1)
        ));
    }
    for (i, m) in c.emails.iter().enumerate() {
        let phone = &c.phones[i % c.phones.len()];
        // Left of an email only a character an address cannot contain: after
        // `-` or `.` the address itself would be longer.
        log.push_str(&format!("INFO contact: {m}{}{phone}\n", sep(i + 1)));
    }
    for (i, p) in c.phones.iter().enumerate() {
        log.push_str(&format!("INFO callback{}{p}\n", sep(i)));
    }
    for (i, n) in c.numbers.iter().enumerate() {
        log.push_str(&format!("INFO balance{}{n}{}ok\n", sep(i), sep(i + 1)));
        // Digits next to digits: detectors may read the pair as one card number.
        log.push_str(&format!("INFO {} {n}\n", c.phones[i % c.phones.len()]));
    }
    for (i, card) in c.cards.iter().enumerate() {
        log.push_str(&format!("WARN declined card{}{card}\n", sep(i)));
    }
    for (i, iban) in c.ibans.iter().enumerate() {
        log.push_str(&format!(
            "INFO payout{}{iban}{}queued\n",
            sep(i),
            sep(i + 1)
        ));
    }
    for (i, ip) in c.ips.iter().enumerate() {
        log.push_str(&format!("DEBUG from {ip}{}ok\n", sep(i)));
    }
    for (i, (first, last)) in c.names.iter().enumerate() {
        log.push_str(&format!(
            "ERROR refund failed for {first} {last}{}, {}{}{}\n",
            sep(i),
            c.emails[i % c.emails.len()],
            sep(i + 1),
            c.numbers[i % c.numbers.len()],
        ));
    }
    vec![
        (".env".into(), env),
        ("data/customers.csv".into(), csv),
        ("logs/app.log".into(), log),
        ("src/lib.rs".into(), PUBLIC_SRC.into()),
    ]
}

fn is_public_word(w: &str) -> bool {
    let lower = w.to_lowercase();
    [PUBLIC_SRC, OBJECTIVE].iter().any(|t| {
        t.split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|x| x.to_lowercase() == lower)
    })
}

/// Every spelling the engine claims to replace.
fn covered_spellings(c: &Canaries) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.extend(c.env_secrets.iter().cloned());
    out.extend(c.tokens.iter().cloned());
    out.extend(c.emails.iter().cloned());
    out.extend(c.phones.iter().cloned());
    out.extend(c.ips.iter().cloned());
    out.extend(c.cards.iter().cloned());
    out.extend(c.ibans.iter().cloned());
    for (first, last) in &c.names {
        out.push(format!("{first} {last}"));
        if last.chars().count() >= 4 && !is_public_word(last) {
            out.push(last.clone());
        }
    }
    out.extend(c.single_names.iter().cloned());
    for n in c.numbers.iter().chain(&c.grouped) {
        let d = n.to_string();
        let (units, cents) = d.split_at(d.len() - 2);
        let units: u64 = units.parse().unwrap_or(0);
        out.push(d.clone());
        out.push(group(*n));
        out.push(format!("{units}.{cents}"));
        out.push(format!("{}.{cents}", group(units)));
    }
    out.retain(|s| s.len() >= 4);
    out.sort();
    out.dedup();
    out
}

/// The canary values themselves (the covered ones: a first name alone and a
/// public or short surname are not replaced by design), for the transformed
/// spelling check. Digit runs the dialects put in every body (`max_tokens`)
/// are ignored.
fn transformed_matcher(c: &Canaries, filter: &dyn declass_boundary::OutboundFilter) -> Matcher {
    let mut values: Vec<String> = Vec::new();
    values.extend(c.env_secrets.iter().cloned());
    values.extend(c.tokens.iter().cloned());
    values.extend(c.emails.iter().cloned());
    values.extend(c.phones.iter().cloned());
    values.extend(c.ips.iter().cloned());
    values.extend(c.cards.iter().cloned());
    values.extend(c.ibans.iter().cloned());
    for (first, last) in &c.names {
        values.push(format!("{first} {last}"));
        if last.chars().count() >= 4 && !is_public_word(last) {
            values.push(last.clone());
        }
    }
    values.extend(c.single_names.iter().cloned());
    values.extend(c.numbers.iter().chain(&c.grouped).map(u64::to_string));
    let mut empty = request(&[]);
    filter.apply(&mut empty);
    let mut ignore_fragments = Vec::new();
    for dialect in DIALECTS {
        let body = dialect.build_body("m", &empty, true).to_string();
        ignore_fragments.extend(
            body.split(|ch: char| !ch.is_ascii_digit())
                .filter(|d| d.len() >= 4)
                .map(str::to_owned),
        );
    }
    Matcher::with_options(
        values,
        Options {
            ignore_fragments,
            ..Options::default()
        },
    )
}

fn prime(c: &Canaries, seps: &[&str]) -> (tempfile::TempDir, Arc<Engine>) {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().canonicalize().unwrap().join("ws");
    let fs = files(c, seps);
    for (path, text) in &fs {
        let p = ws.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let policy = Policy {
        sensitive_globs: vec![
            ".env*".into(),
            "data/**".into(),
            "logs/**".into(),
            "*.log".into(),
        ],
        command_output_sensitive: true,
        secret_sinks: vec![".env*".into()],
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 2000,
        ..Policy::default()
    };
    let e = Engine::open(&d.path().join("run"), policy, None).unwrap();
    let names: Vec<String> = fs.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(e.prime(&ws, &names, OBJECTIVE), 3);
    (d, e)
}

/// Where a canary is placed in the request.
#[derive(Debug, Clone, Copy)]
enum Channel {
    User,
    ToolResult,
    AssistantText,
    Reasoning,
    ToolArguments,
}

const CHANNELS: [Channel; 5] = [
    Channel::User,
    Channel::ToolResult,
    Channel::AssistantText,
    Channel::Reasoning,
    Channel::ToolArguments,
];

fn request(placed: &[(Channel, String)]) -> Request {
    let mut user = String::from("Please look at this.");
    let mut tool = String::from("ok");
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut content = String::new();
    for (ch, s) in placed {
        let target = match ch {
            Channel::User => &mut user,
            Channel::ToolResult => &mut tool,
            Channel::AssistantText => &mut text,
            Channel::Reasoning => &mut reasoning,
            Channel::ToolArguments => &mut content,
        };
        target.push_str(s);
    }
    let raw = serde_json::to_string(&json!({"path": "notes.md", "content": content})).unwrap();
    let arguments: Map<String, Value> = serde_json::from_str(&raw).unwrap();
    Request {
        system: "You are a coding agent.".into(),
        items: vec![
            Item::User { text: user },
            Item::Assistant {
                text,
                reasoning: Some(reasoning),
                replay: None,
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "write_file".into(),
                    arguments,
                    raw_arguments: raw,
                }],
            },
            Item::ToolResult {
                call_id: "call_1".into(),
                content: tool,
            },
        ],
        ..Request::default()
    }
}

/// Every string in `v`, and the strings of any string that is itself JSON
/// (tool-call arguments), so an escaped spelling is seen decoded.
fn decoded_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            out.push(s.clone());
            if let Ok(inner @ (Value::Object(_) | Value::Array(_))) =
                serde_json::from_str::<Value>(s)
            {
                decoded_strings(&inner, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| decoded_strings(x, out)),
        Value::Object(m) => m.iter().for_each(|(k, x)| {
            out.push(k.clone());
            decoded_strings(x, out);
        }),
        _ => {}
    }
}

fn contains_anywhere(body: &Value, needle: &str) -> bool {
    if body.to_string().contains(needle) {
        return true;
    }
    let mut strings = Vec::new();
    decoded_strings(body, &mut strings);
    strings.iter().any(|s| s.contains(needle))
}

fn filler() -> impl Strategy<Value = String> {
    proptest::sample::select(vec![
        " ", "\n", ", ", " see ", "\"", "\\", "(", ") ", "=", ": ", " – ", "\t", "é ",
    ])
    .prop_map(str::to_owned)
}

const SEPS: &[&str] = &[" ", ",", ";", " | ", "\t", " - ", ": ", "-", "/"];

proptest! {
    #![proptest_config(config())]

    #[test]
    fn no_canary_survives_the_gate(
        c in canaries(),
        seps in proptest::collection::vec(proptest::sample::select(SEPS), 1..4),
        placement in proptest::collection::vec((any::<usize>(), filler(), filler()), 1..64),
    ) {
        let (_d, engine) = prime(&c, &seps);
        let (filter, check) = engine.outbound();
        let spellings = covered_spellings(&c);
        let matcher = transformed_matcher(&c, filter.as_ref());
        // Every spelling at least once, in a channel chosen by the placement.
        let placed: Vec<(Channel, String)> = spellings
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (ch, before, after) = &placement[i % placement.len()];
                (CHANNELS[(ch + i) % CHANNELS.len()], format!("{before}{s}{after}"))
            })
            .collect();
        let mut req = request(&placed);
        // The matcher sees the canaries before the filter runs.
        let unfiltered = Dialect::Chat.build_body("m", &req, true).to_string();
        prop_assert!(!matcher.find(&unfiltered).is_empty());
        filter.apply(&mut req);
        for dialect in DIALECTS {
            let body = dialect.build_body("m", &req, true);
            prop_assert!(check.check(&body).is_ok(), "{:?}: filtered request blocked: {:?}", dialect, check.check(&body));
            for s in &spellings {
                prop_assert!(!contains_anywhere(&body, s), "{:?}: {:?} survived the filter: {}", dialect, s, body);
            }
            let mut texts = vec![body.to_string()];
            decoded_strings(&body, &mut texts);
            for t in &texts {
                let found = matcher.find(t);
                prop_assert!(
                    found.is_empty(),
                    "{:?}: transformed canary survived the filter: {:?} in {:?}",
                    dialect,
                    found
                        .iter()
                        .map(|f| (f.form, &matcher.values()[f.canary_index], &t[f.offset..f.offset + f.len]))
                        .collect::<Vec<_>>(),
                    t
                );
            }
        }
        // The check alone refuses a body that still holds a spelling, raw or as
        // escaped tool-call arguments (in Anthropic bodies, a decoded object).
        for s in spellings.iter().filter(|s| s.len() >= 6) {
            for dialect in DIALECTS {
                let raw = request(&[(Channel::User, s.clone())]);
                prop_assert!(check.check(&dialect.build_body("m", &raw, true)).is_err(), "{:?}: check missed raw {:?}", dialect, s);
                let args = request(&[(Channel::ToolArguments, s.clone())]);
                prop_assert!(check.check(&dialect.build_body("m", &args, true)).is_err(), "{:?}: check missed {:?} in arguments", dialect, s);
            }
        }
    }
}
