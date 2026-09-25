// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic detectors for secrets and personal data.
//!
//! Each detector reports byte spans with a kind. Detection is deliberately
//! broad: a false positive costs a placeholder, a false negative costs a leak.
//!
//! Duet's own detectors are here; international personal-data formats are in
//! [`crate::pii`], and the imported secret rules (the gitleaks rule set, used
//! as data) in [`crate::rules`]. Duet's own findings take precedence: an
//! imported match inside one of them is dropped.

use regex::{Regex, RegexSet};
use std::sync::LazyLock;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Secret,
    Email,
    Phone,
    Card,
    NationalId,
    Iban,
    Ip,
    Name,
    Data,
    /// A fragment of protected source code (a literal or distinctive token).
    Code,
    /// A bank account or routing number (recognized by its label).
    Account,
    /// A postal address.
    Address,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Secret,
        Kind::Email,
        Kind::Phone,
        Kind::Card,
        Kind::NationalId,
        Kind::Iban,
        Kind::Ip,
        Kind::Name,
        Kind::Data,
        Kind::Code,
        Kind::Account,
        Kind::Address,
    ];

    /// Kinds whose values are identifying numbers: a run of their digits is
    /// part of the value even when the rest is withheld.
    pub fn is_numeric_identifier(self) -> bool {
        matches!(
            self,
            Kind::Card | Kind::NationalId | Kind::Iban | Kind::Phone | Kind::Account
        )
    }

    pub fn tag(self) -> &'static str {
        match self {
            Kind::Secret => "secret",
            Kind::Email => "email",
            Kind::Phone => "phone",
            Kind::Card => "card",
            Kind::NationalId => "id",
            Kind::Iban => "iban",
            Kind::Ip => "ip",
            Kind::Name => "name",
            Kind::Data => "data",
            Kind::Code => "code",
            Kind::Account => "account",
            Kind::Address => "address",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    /// For assignments, the key name (e.g. `DB_PASSWORD`).
    pub label: Option<String>,
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

// Provider-shaped tokens.
re!(
    TOKENS,
    r"(?x)
    \b(?:
        sk_(?:live|test)_[A-Za-z0-9]{16,}       # payment-style keys
      | (?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,} # forge tokens
      | github_pat_[A-Za-z0-9_]{40,}
      | xox[abprs]-[A-Za-z0-9-]{10,}             # chat bot tokens
      | AKIA[0-9A-Z]{16}                          # cloud access key ids
      | AIza[0-9A-Za-z_-]{35}                     # cloud api keys
      | sk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}      # model provider keys
      | glpat-[A-Za-z0-9_-]{20}
      | eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}  # JWT
    )\b"
);
re!(
    PRIVATE_KEY,
    r"-----BEGIN (?:[A-Z]+ )?PRIVATE KEY-----[\s\S]*?-----END (?:[A-Z]+ )?PRIVATE KEY-----"
);
// scheme://user:password@host
re!(
    URL_CREDENTIALS,
    r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:([^\s@/]+)@"
);
// KEY=value / key: value where the key looks sensitive.
// Environment-style assignments (UPPER_CASE keys), quoted or not.
re!(
    ASSIGNMENT,
    r#"\b([A-Z0-9_]*(?:PASSWORD|PASSWD|PWD|SECRET|TOKEN|API_?KEY|ACCESS_?KEY|PRIVATE_?KEY|CREDENTIAL|AUTH|SIGNING)[A-Z0-9_]*)\s*[:=]\s*["']?([^\s"'#,;]{6,})"#
);
// Any sensitive-looking key with a quoted literal value (e.g. a hard-coded secret in code or YAML).
re!(
    QUOTED_ASSIGNMENT,
    r#"(?i)\b([A-Za-z0-9_.-]*(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential|signing)[A-Za-z0-9_.-]*)["']?\s*[:=]\s*["']([^"'\s]{6,})["']"#
);
re!(EMAIL, r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b");
re!(
    PHONE,
    r"(?:\+\d{1,3}[\s.-]?)?\(?\b\d{3}\)?[\s.-]\d{3}[\s.-]\d{4}\b"
);
re!(CARD, r"\b(?:\d[ -]?){13,19}\b");
// A 12-19 digit number (spaces or dashes between digits allowed), checked for
// a label nearby: what someone calls a card or account number is one, whether
// or not its checksum holds.
re!(LABELLED_NUMBER, r"\b\d(?:[ -]?\d){11,18}\b");
re!(
    NUMBER_LABEL,
    r"(?i)\b(?:(credit|debit|card|visa|mastercard|amex|maestro)|(iban)|(account|acct|routing|bank|sort\s+code|bsb)|(ssn|social\s+security|passport|licen[cs]e|tax\s*id|national\s+id|id\s+(?:number|no)|personal\s+id))"
);
re!(WORD_RUN, r"\w+");
re!(SSN, r"\b\d{3}-\d{2}-\d{4}\b");
re!(
    IPV4,
    r"\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}\b"
);
// Long random-looking tokens (entropy check applied afterwards).
re!(HIGH_ENTROPY, r"[A-Za-z0-9+/_-]{24,}={0,2}");

/// Duet's own expressions, in [`Own`] order. One pass of a set over all of
/// them finds which can match at all; only those run on their own.
static OWN: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new([
        TOKENS.as_str(),
        PRIVATE_KEY.as_str(),
        URL_CREDENTIALS.as_str(),
        ASSIGNMENT.as_str(),
        QUOTED_ASSIGNMENT.as_str(),
        HIGH_ENTROPY.as_str(),
        EMAIL.as_str(),
        PHONE.as_str(),
        CARD.as_str(),
        LABELLED_NUMBER.as_str(),
        SSN.as_str(),
        IPV4.as_str(),
    ])
    .expect("static regex set")
});

#[derive(Clone, Copy)]
enum Own {
    Tokens,
    PrivateKey,
    UrlCredentials,
    Assignment,
    QuotedAssignment,
    HighEntropy,
    Email,
    Phone,
    Card,
    LabelledNumber,
    Ssn,
    Ipv4,
}

#[derive(Debug, Clone, Copy)]
pub struct Detectors {
    pub secrets: bool,
    pub pii: bool,
    pub entropy: bool,
}

impl Default for Detectors {
    fn default() -> Self {
        Self {
            secrets: true,
            pii: true,
            entropy: true,
        }
    }
}

fn luhn(digits: &str) -> bool {
    let ds: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&ds.len()) {
        return false;
    }
    let sum: u32 = ds
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Words a label may stand from the number it names.
const LABEL_REACH: usize = 3;
/// Bytes searched for a label on each side of a number.
const LABEL_WINDOW: usize = 64;

/// The kind a label nearby gives the number at `start..end`: the closest
/// label within [`LABEL_REACH`] words before it, else after it.
fn labelled_kind(text: &str, start: usize, end: usize) -> Option<Kind> {
    let kind_of = |c: &regex::Captures<'_>| {
        [Kind::Card, Kind::Iban, Kind::Account, Kind::NationalId]
            .into_iter()
            .zip(1..)
            .find_map(|(k, i)| c.get(i).map(|_| k))
    };
    let mut from = start.saturating_sub(LABEL_WINDOW);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let before = &text[from..start];
    let near_before = NUMBER_LABEL
        .captures_iter(before)
        .filter(|c| {
            WORD_RUN
                .find_iter(&before[c.get(0).map_or(0, |m| m.end())..])
                .count()
                <= LABEL_REACH
        })
        .last();
    if let Some(c) = near_before {
        return kind_of(&c);
    }
    let mut to = (end + LABEL_WINDOW).min(text.len());
    while !text.is_char_boundary(to) {
        to -= 1;
    }
    let after = &text[end..to];
    NUMBER_LABEL
        .captures_iter(after)
        .find(|c| {
            WORD_RUN
                .find_iter(&after[..c.get(0).map_or(0, |m| m.start())])
                .count()
                <= LABEL_REACH
        })
        .and_then(|c| kind_of(&c))
}

/// Shannon entropy in bits per character.
pub fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let n = s.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = f64::from(c) / n;
            -p * p.log2()
        })
        .sum()
}

fn looks_random(s: &str) -> bool {
    let classes = [
        s.bytes().any(|b| b.is_ascii_lowercase()),
        s.bytes().any(|b| b.is_ascii_uppercase()),
        s.bytes().any(|b| b.is_ascii_digit()),
    ];
    entropy(s) >= 4.0
        && classes.iter().filter(|c| **c).count() >= 2
        && !s.contains("__")
        && !s
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase() && s.len() == 40)
        && !is_integrity_digest(s)
        && !is_camel_case_words(s)
}

/// A subresource-integrity value (`sha512-<base64>`, as in lockfiles and
/// `<script integrity=…>`): the digest of public content.
fn is_integrity_digest(s: &str) -> bool {
    ["sha256-", "sha384-", "sha512-"]
        .iter()
        .any(|p| s.starts_with(p))
}

/// An identifier made of words (`HttpRequestRetryPolicy`): letters only,
/// each capital starting a word, words of three letters on average. Random
/// letters change case every other character or so.
fn is_camel_case_words(s: &str) -> bool {
    if !s.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    let words = s
        .bytes()
        .enumerate()
        .filter(|(i, b)| *i == 0 || b.is_ascii_uppercase())
        .count();
    let single = s
        .as_bytes()
        .windows(2)
        .filter(|w| w[0].is_ascii_uppercase() && w[1].is_ascii_uppercase())
        .count();
    words >= 3 && single <= 2 && s.len() >= 3 * words
}

/// Addresses at domains reserved for documentation and testing (RFC 2606 /
/// RFC 6761) cannot belong to anyone; code and tests use them as examples.
/// The owner's and project's own detectors (`sensitivity.custom_patterns`):
/// every match is a `data` value, in sensitive and public text alike.
#[derive(Debug, Clone, Default)]
pub struct CustomPatterns(Vec<Regex>);

impl CustomPatterns {
    /// Compiles `patterns`; an empty or invalid pattern is an error (a
    /// pattern silently dropped would be a value silently sent).
    pub fn compile(patterns: &[String]) -> Result<Self, String> {
        patterns
            .iter()
            .map(|p| {
                if p.is_empty() {
                    return Err("an empty custom pattern would match everywhere".to_owned());
                }
                Regex::new(p).map_err(|e| format!("custom pattern {p:?}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every non-empty match, sorted by start (longer first).
    pub fn find(&self, text: &str) -> Vec<Finding> {
        let mut out: Vec<Finding> = self
            .0
            .iter()
            .flat_map(|re| re.find_iter(text))
            .filter(|m| !m.is_empty())
            .map(|m| Finding {
                kind: Kind::Data,
                start: m.start(),
                end: m.end(),
                label: None,
            })
            .collect();
        out.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
        out.dedup();
        out
    }
}

/// [`scan`] plus the custom patterns, overlaps merged the same way.
pub fn scan_with(text: &str, d: Detectors, custom: &CustomPatterns) -> Vec<Finding> {
    let mut all = scan_each(text, d);
    all.extend(custom.find(text));
    all.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    let mut merged: Vec<Finding> = Vec::new();
    for f in all {
        match merged.last_mut() {
            Some(last) if f.start < last.end => last.end = last.end.max(f.end),
            _ => merged.push(f),
        }
    }
    merged
}

fn reserved_example_domain(email: &str) -> bool {
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    let d = domain.to_ascii_lowercase();
    ["example.com", "example.org", "example.net"].contains(&d.as_str())
        || [".test", ".example", ".invalid", ".localhost"]
            .iter()
            .any(|tld| d.ends_with(tld) || d == tld[1..])
}

/// All findings in `text`, sorted by start, with overlapping findings merged
/// into one span (the first finding's kind and label).
pub fn scan(text: &str, d: Detectors) -> Vec<Finding> {
    let mut merged: Vec<Finding> = Vec::new();
    for f in scan_each(text, d) {
        match merged.last_mut() {
            Some(last) if f.start < last.end => {
                if f.end > last.end {
                    last.end = f.end;
                }
            }
            _ => merged.push(f),
        }
    }
    merged
}

/// Every finding in `text`, sorted by start (longer first), overlaps kept: a
/// value inside a longer match is still a value on its own.
pub fn scan_each(text: &str, d: Detectors) -> Vec<Finding> {
    scan_each_in(text, d, None)
}

/// [`scan_each`] for text that came from the workspace file `path`: imported
/// rules with a path condition (`*.tf`, `*.ya?ml`) run on it, and their path
/// allowlists (lockfiles, vendored dependencies) apply.
pub fn scan_each_in(text: &str, d: Detectors, path: Option<&str>) -> Vec<Finding> {
    let mut out = Vec::new();
    let hits = OWN.matches(text);
    let on = |o: Own| hits.matched(o as usize);
    let mut push = |kind, m: regex::Match<'_>, label: Option<String>| {
        out.push(Finding {
            kind,
            start: m.start(),
            end: m.end(),
            label,
        })
    };
    if d.secrets {
        if on(Own::Tokens) {
            for m in TOKENS.find_iter(text) {
                push(Kind::Secret, m, None);
            }
        }
        if on(Own::PrivateKey) {
            for m in PRIVATE_KEY.find_iter(text) {
                push(Kind::Secret, m, Some("PRIVATE_KEY".into()));
            }
        }
        if on(Own::UrlCredentials) {
            for c in URL_CREDENTIALS.captures_iter(text) {
                if let Some(m) = c.get(1) {
                    push(Kind::Secret, m, Some("URL_PASSWORD".into()));
                }
            }
        }
        let assignments = [
            (Own::Assignment, &*ASSIGNMENT),
            (Own::QuotedAssignment, &*QUOTED_ASSIGNMENT),
        ];
        for (_, re) in assignments.into_iter().filter(|(o, _)| on(*o)) {
            for c in re.captures_iter(text) {
                if let (Some(k), Some(v)) = (c.get(1), c.get(2)) {
                    let value = v.as_str();
                    let placeholder_like = value.starts_with('$')
                        || value.starts_with('<')
                        || value.starts_with('⟨')
                        || value.eq_ignore_ascii_case("changeme")
                        || value.eq_ignore_ascii_case("change_me");
                    if !placeholder_like {
                        push(Kind::Secret, v, Some(k.as_str().to_owned()));
                    }
                }
            }
        }
    }
    if d.entropy && on(Own::HighEntropy) {
        for m in HIGH_ENTROPY.find_iter(text) {
            if looks_random(m.as_str()) {
                push(Kind::Secret, m, None);
            }
        }
    }
    if d.pii {
        if on(Own::Email) {
            for m in EMAIL.find_iter(text) {
                if !reserved_example_domain(m.as_str()) {
                    push(Kind::Email, m, None);
                }
            }
        }
        if on(Own::Phone) {
            for m in PHONE.find_iter(text) {
                push(Kind::Phone, m, None);
            }
        }
        if on(Own::Card) {
            for m in CARD.find_iter(text) {
                if luhn(m.as_str()) {
                    push(Kind::Card, m, None);
                }
            }
        }
        if on(Own::LabelledNumber) {
            for m in LABELLED_NUMBER.find_iter(text) {
                if let Some(kind) = labelled_kind(text, m.start(), m.end()) {
                    push(kind, m, None);
                }
            }
        }
        if on(Own::Ssn) {
            for m in SSN.find_iter(text) {
                push(Kind::NationalId, m, None);
            }
        }
        if on(Own::Ipv4) {
            for m in IPV4.find_iter(text) {
                let s = m.as_str();
                if !(s.starts_with("127.") || s == "0.0.0.0" || s.starts_with("255.")) {
                    push(Kind::Ip, m, None);
                }
            }
        }
        crate::pii::scan(text, &mut out);
    }
    if d.secrets {
        add_imported(text, path, &mut out);
    }
    out.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    out
}

/// Secrets the imported rules find, each labelled with its rule id (for the
/// placeholder and the disclosure report). One inside a span duet's own
/// detectors found is left to them.
fn add_imported(text: &str, path: Option<&str>, out: &mut Vec<Finding>) {
    let found = crate::rules::imported().find(text, path);
    if found.is_empty() {
        return;
    }
    out.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    // The furthest end among own findings starting at or before each one.
    let reach: Vec<usize> = out
        .iter()
        .scan(0, |max, f| {
            *max = f.end.max(*max);
            Some(*max)
        })
        .collect();
    let own = out.len();
    for m in found {
        let i = out[..own].partition_point(|f| f.start <= m.start);
        if i > 0 && reach[i - 1] >= m.end {
            continue;
        }
        out.push(Finding {
            kind: Kind::Secret,
            start: m.start,
            end: m.end,
            label: Some(m.rule.label().to_owned()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        scan(text, Detectors::default())
            .into_iter()
            .map(|f| (f.kind, text[f.start..f.end].to_owned()))
            .collect()
    }

    #[test]
    fn finds_provider_tokens_and_assignments() {
        let k = kinds(
            "PAYMENTS_API_KEY=sk_live_A1b2C3d4E5f6G7h8I9j0KlMnOp\nexport LEDGER_DB_PASSWORD=\"Xy7!pQr9sT2\"\n",
        );
        assert!(
            k.iter()
                .any(|(kind, v)| *kind == Kind::Secret && v.starts_with("sk_live_"))
        );
        assert!(
            k.iter()
                .any(|(kind, v)| *kind == Kind::Secret && v == "Xy7!pQr9sT2")
        );
    }

    #[test]
    fn finds_url_credentials_private_keys_and_jwts() {
        let text = "DATABASE_URL=postgres://app:S3cr3tPw@db.internal:5432/prod\n-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----\nauth eyJhbGciOiJIUzI1.eyJzdWIiOiIxMjM0.SflKxwRJSMeKKF2QT4";
        let k = kinds(text);
        assert!(k.iter().any(|(_, v)| v.contains("S3cr3tPw")));
        assert!(k.iter().any(|(_, v)| v.starts_with("-----BEGIN")));
        assert!(k.iter().any(|(_, v)| v.starts_with("eyJ")));
    }

    #[test]
    fn placeholders_and_references_are_not_secrets() {
        assert!(kinds("API_KEY=${API_KEY}\nTOKEN=<your token>\npassword = CHANGE_ME").is_empty());
    }

    #[test]
    fn finds_personal_data() {
        let k = kinds(
            "mail amelia.velanwick42@mailbox-311.net call +1 (415) 555-0199, card 4111 1111 1111 1111, ssn 123-45-6789, iban DE89370400440532013000 from 203.0.113.41",
        );
        let found: Vec<Kind> = k.iter().map(|(kind, _)| *kind).collect();
        for kind in [
            Kind::Email,
            Kind::Phone,
            Kind::Card,
            Kind::NationalId,
            Kind::Iban,
            Kind::Ip,
        ] {
            assert!(found.contains(&kind), "{kind:?} not found in {k:?}");
        }
    }

    #[test]
    fn luhn_and_iban_filter_false_positives() {
        assert!(
            !kinds("order 1234 5678 9012 3456")
                .iter()
                .any(|(k, _)| *k == Kind::Card)
        );
        assert!(
            !kinds("ref GB00ABCD12345678901234")
                .iter()
                .any(|(k, _)| *k == Kind::Iban)
        );
    }

    #[test]
    fn labelled_numbers_are_found_whatever_their_checksum() {
        // Seen in a live run: a card number that fails the checksum, typed by
        // the operator, reached the frontier as it was.
        let k = kinds("is this credit card number valid 42977600076546677?");
        assert_eq!(k, vec![(Kind::Card, "42977600076546677".to_owned())]);
        for (text, kind, value) in [
            (
                "acct 0012-3456-7890-12 closed",
                Kind::Account,
                "0012-3456-7890-12",
            ),
            (
                "1234 5678 9012 3456 is my debit card",
                Kind::Card,
                "1234 5678 9012 3456",
            ),
            (
                "passport no. 120382716455",
                Kind::NationalId,
                "120382716455",
            ),
            (
                "routing and account: 021000021000",
                Kind::Account,
                "021000021000",
            ),
        ] {
            assert_eq!(kinds(text), vec![(kind, value.to_owned())], "{text}");
        }
        // Unlabelled, or labelled too far away, or too short: not this detector.
        for text in [
            "order 1234 5678 9012 3456",
            "card declined; see the ticket about the retry queue timing 1790367049547",
            "account 12345678901",
            "created_ms 1790367049547",
        ] {
            assert!(kinds(text).is_empty(), "{text}: {:?}", kinds(text));
        }
    }

    #[test]
    fn ordinary_code_is_not_flagged() {
        let code = "fn parse_timestamp(s: &str) -> Result<i64, ParseError> {\n    let token = next_token(&mut it);\n    let cfg = Config::from_sources(env_file, config_file)?;\n}";
        assert!(kinds(code).is_empty(), "{:?}", kinds(code));
    }

    #[test]
    fn custom_patterns_find_data_values_and_refuse_bad_patterns() {
        let c =
            CustomPatterns::compile(&["CUST-[0-9]{6}".into(), "[a-z0-9]+\\.corp\\.example".into()])
                .unwrap();
        let text = "order CUST-004211 from db1.corp.example, CUST-12 is too short";
        let found: Vec<(Kind, &str)> = c
            .find(text)
            .iter()
            .map(|f| (f.kind, &text[f.start..f.end]))
            .collect();
        assert_eq!(
            found,
            vec![
                (Kind::Data, "CUST-004211"),
                (Kind::Data, "db1.corp.example")
            ]
        );
        // Empty matches never become spans.
        assert!(
            CustomPatterns::compile(&["x*".into()])
                .unwrap()
                .find("abc")
                .is_empty()
        );
        assert!(CustomPatterns::compile(&["(".into()]).is_err());
        assert!(CustomPatterns::compile(&[String::new()]).is_err());
        // Merged with the built-in detectors.
        let d = Detectors::default();
        let merged = scan_with("mail kim@corp.net about CUST-004211", d, &c);
        let kinds: Vec<Kind> = merged.iter().map(|f| f.kind).collect();
        assert_eq!(kinds, vec![Kind::Email, Kind::Data]);
    }

    #[test]
    fn identifiers_and_integrity_digests_are_not_random() {
        for text in [
            "AbstractSingletonProxyFactoryBean",
            "ThisIsAVeryLongTypeNameUsedInTests",
            "ReadOnlyAccessPolicyAttachmentForAuditRole",
            "\"integrity\": \"sha512-y4jvZt97iR2PkwcRTtsYIBlBnD8RXl4pfUxIhtF+ub9xV+1Ew3ku03BdBZz1I3CmAXRS0C/1UtRz62gykIaJ2g==\"",
        ] {
            assert!(kinds(text).is_empty(), "{text}: {:?}", kinds(text));
        }
        // Random letters and mixed tokens stay secrets.
        for text in [
            "QmXvTzLpRwNsKdHjFgBcYtEa",
            "Q8f2LmZ0x9R4tWvB7nC1pK6sD3hJ5gYa",
        ] {
            assert!(
                kinds(text).iter().any(|(k, _)| *k == Kind::Secret),
                "{text}"
            );
        }
    }

    #[test]
    fn high_entropy_strings_are_flagged() {
        assert!(
            kinds("key: Q8f2LmZ0x9R4tWvB7nC1pK6sD3hJ5gYa")
                .iter()
                .any(|(k, _)| *k == Kind::Secret)
        );
    }
}
