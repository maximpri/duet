// SPDX-License-Identifier: GPL-3.0-or-later
//! Property tests for the boundary's building blocks: the placeholder vault,
//! the copied-span filter and the detectors. The gate-level property ("no
//! canary survives the gate") is in `no_canary.rs`.
//!
//! Cases per property default to a small number so `tools/gate.sh` stays fast;
//! set `PROPTEST_CASES` to run more (for example `PROPTEST_CASES=20000`).

use declass_boundary::detect::{Detectors, Kind, scan, scan_each};
use declass_boundary::overlap::{MIN_CONSECUTIVE, OverlapIndex, REDACTED};
use declass_boundary::vault::{CLOSE, OPEN, Vault};
use proptest::prelude::*;

const DEFAULT_CASES: u32 = 256;

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

const KINDS: &[Kind] = &[
    Kind::Secret,
    Kind::Email,
    Kind::Phone,
    Kind::Name,
    Kind::Data,
    Kind::Card,
];

/// Vault values: arbitrary text without placeholder brackets, biased toward
/// words that also occur inside tokens (kind tags, labels), which is where a
/// naive replacement rewrites its own output.
fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        2 => "[^⟨⟩]{4,14}",
        2 => "[a-zA-Z0-9_\\\\\"./ -]{4,12}",
        1 => proptest::sample::select(vec![
            "secret", "email", "phone", "name", "data", "card", "code", "iban",
            "API_KEY", "DB_URL", "#1⟩x", "ecret:", "mail:em",
        ])
        .prop_map(str::to_owned),
    ]
}

fn label() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        Just(Some("API_KEY".to_owned())),
        Just(Some("DB_URL".to_owned())),
        "[A-Za-z_]{1,10}".prop_map(Some),
    ]
}

/// Filler around values: anything except text that spells a token.
fn filler() -> impl Strategy<Value = String> {
    "[^#]{0,10}"
}

struct Setup {
    vault: Vault,
    values: Vec<String>,
    aliases: Vec<String>,
}

fn setup(entries: Vec<(String, usize, Option<String>)>, aliases: Vec<(String, usize)>) -> Setup {
    let mut vault = Vault::in_memory();
    let mut values = Vec::new();
    for (v, k, l) in entries {
        if vault.contains(&v) {
            continue;
        }
        vault
            .token_for(&v, KINDS[k % KINDS.len()], l.as_deref(), "test")
            .unwrap();
        values.push(v);
    }
    let mut added = Vec::new();
    if !values.is_empty() {
        for (spelling, i) in aliases {
            if !vault.contains(&spelling) {
                vault.alias(&spelling, &values[i % values.len()]).unwrap();
                added.push(spelling);
            }
        }
    }
    Setup {
        vault,
        values,
        aliases: added,
    }
}

/// `text` with every known token replaced by a separator.
fn without_tokens(vault: &Vault, text: &str) -> String {
    let mut out = text.to_owned();
    for (_, e) in vault.values() {
        out = out.replace(&e.token, "\u{1}");
    }
    out
}

fn interleave(values: &[String], picks: &[(usize, String)]) -> String {
    let mut text = String::new();
    for (i, fill) in picks {
        text.push_str(fill);
        if !values.is_empty() {
            text.push_str(&values[i % values.len()]);
        }
    }
    text
}

proptest! {
    #![proptest_config(config())]

    /// detokenize ∘ tokenize is the identity on text holding vault values.
    #[test]
    fn vault_round_trips(
        entries in proptest::collection::vec((value(), any::<usize>(), label()), 1..8),
        picks in proptest::collection::vec((any::<usize>(), filler()), 0..10),
    ) {
        let s = setup(entries, Vec::new());
        let text = interleave(&s.values, &picks);
        let (tokenized, _) = s.vault.tokenize(&text);
        let (back, unknown) = s.vault.detokenize(&tokenized);
        prop_assert_eq!(&back, &text, "tokenized: {}", tokenized);
        prop_assert!(unknown.is_empty(), "{:?}", unknown);
    }

    /// No value (4+ bytes) or alias is left in tokenized text outside the tokens.
    #[test]
    fn tokenize_leaves_no_value(
        entries in proptest::collection::vec((value(), any::<usize>(), label()), 1..8),
        aliases in proptest::collection::vec((value(), any::<usize>()), 0..4),
        picks in proptest::collection::vec((any::<usize>(), filler()), 0..10),
    ) {
        let s = setup(entries, aliases);
        let all: Vec<String> = s.values.iter().chain(&s.aliases).cloned().collect();
        let text = interleave(&all, &picks);
        let (tokenized, _) = s.vault.tokenize(&text);
        let rest = without_tokens(&s.vault, &tokenized);
        for v in all.iter().filter(|v| v.len() >= 4) {
            prop_assert!(!rest.contains(v.as_str()), "{:?} left in {:?}", v, tokenized);
        }
    }

    /// Tokenizing twice changes nothing more (the outbound filter re-runs it on history).
    #[test]
    fn tokenize_is_idempotent(
        entries in proptest::collection::vec((value(), any::<usize>(), label()), 1..8),
        aliases in proptest::collection::vec((value(), any::<usize>()), 0..4),
        picks in proptest::collection::vec((any::<usize>(), filler()), 0..10),
    ) {
        let s = setup(entries, aliases);
        let all: Vec<String> = s.values.iter().chain(&s.aliases).cloned().collect();
        let once = s.vault.tokenize(&interleave(&all, &picks)).0;
        let (twice, n) = s.vault.tokenize(&once);
        prop_assert_eq!(&twice, &once);
        prop_assert_eq!(n, 0);
    }

    /// An alias shares its value's token but is never what the token detokenizes to.
    #[test]
    fn aliases_never_detokenize(
        entries in proptest::collection::vec((value(), any::<usize>(), label()), 1..6),
        aliases in proptest::collection::vec((value(), any::<usize>()), 1..4),
    ) {
        let s = setup(entries, aliases);
        for a in &s.aliases {
            let (_, entry) = s.vault.values().find(|(v, _)| v == a).unwrap();
            prop_assert!(entry.alias);
            let (value, primary) = s.vault.value_of(&entry.token).unwrap();
            prop_assert!(!primary.alias);
            prop_assert_ne!(value, a.as_str());
        }
    }

    /// Unknown or partial tokens and stray brackets never panic and come back unchanged.
    #[test]
    fn detokenize_leaves_unknown_text_alone(text in "[⟨⟩a-z:#0-9]{0,24}") {
        let mut v = Vault::in_memory();
        v.token_for("s3cretvalue", Kind::Secret, Some("K"), "x").unwrap();
        let (out, _) = v.detokenize(&text);
        if !text.contains("⟨secret:K#1⟩") {
            prop_assert_eq!(out, text);
        }
    }
}

/// Word soup from a small vocabulary, so windows repeat across texts. The
/// redaction marker's own words are left out (the marker is text too).
fn prose() -> impl Strategy<Value = String> {
    let word = proptest::sample::select(vec![
        "alpha",
        "beta",
        "gamma",
        "delta",
        "Northwind",
        "revenue",
        "Q3",
        "4812339",
        "é",
        "Ärger",
        "the",
        "of",
        "and",
    ]);
    let sep = proptest::sample::select(vec![" ", ", ", "\n", "::", "(", ")", "—", " ⟨", "⟩ "]);
    proptest::collection::vec((word, sep), 0..60)
        .prop_map(|ws| ws.into_iter().flat_map(|(w, s)| [w, s]).collect())
}

/// Spans a second pass would still redact: zero when no run of
/// `MIN_CONSECUTIVE` sensitive, non-public windows is left in `text`.
fn redacted_again(sensitive: &[String], public: &[String], text: &str) -> usize {
    let mut idx = OverlapIndex::default();
    for s in sensitive {
        idx.add_sensitive(s);
    }
    for p in public {
        idx.add_public(p);
    }
    idx.redact(text).1
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn overlap_never_panics(
        sensitive in proptest::collection::vec(".{0,300}", 0..3),
        public in proptest::collection::vec(".{0,300}", 0..3),
        text in ".{0,600}",
    ) {
        let mut idx = OverlapIndex::default();
        for s in &sensitive { idx.add_sensitive(s); }
        for p in &public { idx.add_public(p); }
        let _ = idx.redact(&text);
    }

    /// After redaction no run of `MIN_CONSECUTIVE` sensitive, non-public
    /// windows is left; copied text that was never public does not survive.
    #[test]
    fn overlap_redaction_leaves_no_copied_run(
        sensitive in proptest::collection::vec(prose(), 1..3),
        public in proptest::collection::vec(prose(), 0..2),
        noise in proptest::collection::vec(prose(), 0..4),
        cuts in proptest::collection::vec((any::<usize>(), any::<usize>()), 0..4),
    ) {
        // Outbound text: slices of sensitive text between noise.
        let mut text = String::new();
        for (i, (a, b)) in cuts.iter().enumerate() {
            let s = &sensitive[i % sensitive.len()];
            let bounds: Vec<usize> = s.char_indices().map(|(i, _)| i).chain([s.len()]).collect();
            let (x, y) = (bounds[a % bounds.len()], bounds[b % bounds.len()]);
            text.push_str(&s[x.min(y)..x.max(y)]);
            text.push(' ');
            if let Some(n) = noise.get(i) { text.push_str(n); text.push(' '); }
        }
        let mut idx = OverlapIndex::default();
        for s in &sensitive { idx.add_sensitive(s); }
        for p in &public { idx.add_public(p); }
        let (out, n) = idx.redact(&text);
        prop_assert!(n == 0 || out.contains(REDACTED));
        prop_assert_eq!(redacted_again(&sensitive, &public, &out), 0, "{}", out);
        // A whole sensitive text of 3+ windows, never public, never survives verbatim.
        for s in &sensitive {
            let words = s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).count();
            if words >= declass_boundary::overlap::WINDOW + MIN_CONSECUTIVE - 1
                && public.is_empty()
                && text.contains(s.as_str())
            {
                prop_assert!(!out.contains(s.as_str()), "{}", out);
            }
        }
    }
}

/// Text biased toward detector shapes.
fn detector_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("API_KEY=".to_owned()),
        Just("password: \"".to_owned()),
        Just("postgres://u:".to_owned()),
        Just("@".to_owned()),
        Just("-----BEGIN RSA PRIVATE KEY-----".to_owned()),
        Just("-----END RSA PRIVATE KEY-----".to_owned()),
        Just("sk_live_".to_owned()),
        Just("eyJ".to_owned()),
        Just("+1 (".to_owned()),
        Just("DE89".to_owned()),
        Just("4111 1111 1111 1111".to_owned()),
        Just("123-45-6789".to_owned()),
        Just("203.0.113.".to_owned()),
        Just("⟨é⟩ß\u{0301}".to_owned()),
        "[A-Za-z0-9]{1,30}",
        "[0-9 .-]{1,12}",
        ".{0,8}",
    ];
    proptest::collection::vec(piece, 0..30).prop_map(|p| p.concat())
}

proptest! {
    #![proptest_config(config())]

    /// Findings are in bounds, on char boundaries, sorted and non-overlapping.
    #[test]
    fn scan_spans_are_well_formed(text in prop_oneof![detector_text(), ".{0,200}"]) {
        let findings = scan(&text, Detectors::default());
        let mut last = 0;
        for f in &findings {
            prop_assert!(f.start < f.end && f.end <= text.len(), "{:?}", f);
            prop_assert!(text.is_char_boundary(f.start) && text.is_char_boundary(f.end));
            prop_assert!(f.start >= last, "overlap or unsorted: {:?}", findings);
            last = f.end;
        }
        // Unmerged findings: in bounds, on char boundaries, sorted by start,
        // and each inside one merged span.
        let each = scan_each(&text, Detectors::default());
        for w in each.windows(2) {
            prop_assert!(w[0].start <= w[1].start);
        }
        for f in &each {
            prop_assert!(f.start < f.end && f.end <= text.len());
            prop_assert!(text.is_char_boundary(f.start) && text.is_char_boundary(f.end));
            prop_assert!(findings.iter().any(|m| m.start <= f.start && f.end <= m.end), "{:?}", f);
        }
    }
}

#[test]
fn tokens_are_bracketed() {
    let mut v = Vault::in_memory();
    let t = v.token_for("abcdef", Kind::Secret, None, "x").unwrap();
    assert!(t.starts_with(OPEN) && t.ends_with(CLOSE));
}
