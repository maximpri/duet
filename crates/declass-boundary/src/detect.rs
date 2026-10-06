// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic detectors for secrets and personal data.
//!
//! Each detector reports byte spans with a kind. Detection is deliberately
//! broad: a false positive costs a placeholder, a false negative costs a leak.
//!
//! Declass's own detectors are here; international personal-data formats are in
//! [`crate::pii`], and the imported secret rules (the gitleaks rule set, used
//! as data) in [`crate::rules`]. Declass's own findings take precedence: an
//! imported match inside one of them is dropped.

use regex::{Regex, RegexSet};
use std::sync::LazyLock;

mod random;

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
re!(ISBN_LABEL, r"(?i)\bisbn(?:-1[03])?\s*:?\s*$");
re!(SSN, r"\b\d{3}-\d{2}-\d{4}\b");
re!(
    IPV4,
    r"\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}\b"
);
// Long random-looking tokens, judged afterwards by their parts ([`random`]).
re!(HIGH_ENTROPY, r"[A-Za-z0-9+/_-]{24,}={0,2}");

/// Declass's own expressions, in [`Own`] order. One pass of a set over all of
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

/// Whether `digits` start as a card network's numbers do, at a length that
/// network issues: Visa (4: 13, 16, 19), Mastercard (51–55, 2221–2720),
/// Mir (2200–2204), American Express (34, 37: 15), Diners (300–305, 3095,
/// 36, 38–39), JCB (3528–3589), Discover (6011, 644–649, 65), UnionPay (62),
/// Maestro and other debit ranges (50, 56–69), RuPay (60, 81, 82, 508) and
/// Troy (9792). Timestamps (`20170806110612`, `1510067557121`), snowflake
/// ids and the digits of a long decimal mostly start otherwise.
fn card_network(digits: &str) -> bool {
    let d = digits.as_bytes();
    let n = d.len();
    let prefix = |k: usize| -> u32 { digits.get(..k).and_then(|p| p.parse().ok()).unwrap_or(0) };
    match d.first() {
        Some(b'4') => matches!(n, 13 | 16 | 19),
        Some(b'3') => match prefix(2) {
            34 | 37 => n == 15,
            36 | 38 | 39 => (14..=19).contains(&n),
            35 => (3528..=3589).contains(&prefix(4)) && (16..=19).contains(&n),
            30 => ((300..=305).contains(&prefix(3)) || prefix(4) == 3095) && (14..=19).contains(&n),
            _ => false,
        },
        Some(b'2') => {
            ((2221..=2720).contains(&prefix(4)) || (2200..=2204).contains(&prefix(4)))
                && (16..=19).contains(&n)
        }
        Some(b'5') => matches!(prefix(2), 50..=58) && (12..=19).contains(&n),
        Some(b'6') => (12..=19).contains(&n),
        Some(b'8') => matches!(prefix(2), 81 | 82) && n == 16,
        Some(b'9') => prefix(4) == 9792 && n == 16,
        _ => false,
    }
}

/// Card-shaped numbers that pass Luhn, whatever their network: what sensitive
/// content is held to (a data file's card column is withheld as cards, with
/// every run of their digits, even when its numbers start as no network's
/// do). Public text is held to the network check as well ([`scan_each`]).
pub(crate) fn luhn_numbers(text: &str) -> Vec<std::ops::Range<usize>> {
    CARD.find_iter(text)
        .filter(|m| {
            luhn(m.as_str()) && !isbn(text, m.start(), m.as_str()) && !in_a_decimal(text, m.start())
        })
        .map(|m| m.range())
        .collect()
}

/// Whether `number` is printed in groups the way cards are: three groups or
/// more, the first of four digits (`4111 1111 1111 1111`, `3782 822463 10005`).
fn grouped_like_a_card(number: &str) -> bool {
    let groups: Vec<&str> = number.split([' ', '-']).filter(|g| !g.is_empty()).collect();
    groups.len() >= 3 && groups[0].len() == 4
}

/// Whether the digits at `start` are the fraction of a decimal
/// (`80.95999999999948`).
fn in_a_decimal(text: &str, start: usize) -> bool {
    let before = &text.as_bytes()[..start];
    before.len() >= 2
        && before[before.len() - 1] == b'.'
        && before[before.len() - 2].is_ascii_digit()
}

/// Whether the card-shaped number `number` at `start` is an ISBN: printed in
/// the ISBN-13 layout (five groups, a three-digit prefix first and the check
/// digit last: `978-0-596-51004-6`), or 978 or 979 and a valid ISBN-13 check
/// digit, or right after an `ISBN` label. Thirteen-digit card numbers start
/// with 4, and cards are printed in groups of four or so.
fn isbn(text: &str, start: usize, number: &str) -> bool {
    let digits: Vec<u32> = number.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 13 {
        return false;
    }
    let groups: Vec<&str> = number.split([' ', '-']).collect();
    let layout = groups.len() == 5 && groups[0].len() == 3 && groups[4].len() == 1;
    let weighted: u32 = digits
        .iter()
        .enumerate()
        .map(|(i, d)| if i % 2 == 0 { *d } else { 3 * d })
        .sum();
    let checked = matches!(digits[..3], [9, 7, 8 | 9]) && weighted.is_multiple_of(10);
    let mut from = start.saturating_sub(16);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    layout || checked || ISBN_LABEL.is_match(&text[from..start])
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
    let mut push = |kind, span: std::ops::Range<usize>, label: Option<String>| {
        out.push(Finding {
            kind,
            start: span.start,
            end: span.end,
            label,
        })
    };
    if d.secrets {
        if on(Own::Tokens) {
            for m in TOKENS.find_iter(text) {
                push(Kind::Secret, m.range(), None);
            }
        }
        if on(Own::PrivateKey) {
            for m in PRIVATE_KEY.find_iter(text) {
                push(Kind::Secret, m.range(), Some("PRIVATE_KEY".into()));
            }
        }
        if on(Own::UrlCredentials) {
            for c in URL_CREDENTIALS.captures_iter(text) {
                if let Some(m) = c.get(1) {
                    push(Kind::Secret, m.range(), Some("URL_PASSWORD".into()));
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
                        push(Kind::Secret, v.range(), Some(k.as_str().to_owned()));
                    }
                }
            }
        }
    }
    if d.entropy && on(Own::HighEntropy) {
        for m in HIGH_ENTROPY.find_iter(text) {
            for (start, end) in random::random_spans(text, m.start(), m.end()) {
                push(Kind::Secret, start..end, None);
            }
        }
    }
    if d.pii {
        if on(Own::Email) {
            for m in EMAIL.find_iter(text) {
                if !reserved_example_domain(m.as_str()) {
                    push(Kind::Email, m.range(), None);
                }
            }
        }
        if on(Own::Phone) {
            for m in PHONE.find_iter(text) {
                push(Kind::Phone, m.range(), None);
            }
        }
        if on(Own::Card) {
            for m in CARD.find_iter(text) {
                let number = m.as_str();
                let digits: String = number.chars().filter(char::is_ascii_digit).collect();
                // Luhn alone passes one number in ten: a card also starts as a
                // network's do, or is printed grouped like one (a card
                // labelled as such is found by the labelled-number rule).
                if luhn(number)
                    && !isbn(text, m.start(), number)
                    && !in_a_decimal(text, m.start())
                    && (card_network(&digits) || grouped_like_a_card(number))
                {
                    push(Kind::Card, m.range(), None);
                }
            }
        }
        if on(Own::LabelledNumber) {
            for m in LABELLED_NUMBER.find_iter(text) {
                if let Some(kind) = labelled_kind(text, m.start(), m.end()) {
                    push(kind, m.range(), None);
                }
            }
        }
        if on(Own::Ssn) {
            for m in SSN.find_iter(text) {
                push(Kind::NationalId, m.range(), None);
            }
        }
        if on(Own::Ipv4) {
            for m in IPV4.find_iter(text) {
                let s = m.as_str();
                if !(s.starts_with("127.") || s == "0.0.0.0" || s.starts_with("255.")) {
                    push(Kind::Ip, m.range(), None);
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
/// placeholder and the disclosure report). One inside a span declass's own
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
    fn luhn_valid_timestamps_ids_and_decimals_are_not_cards() {
        // Measured on public pages and the XL tasks (2026-09-27): about one
        // in ten of these passes Luhn and became a card placeholder.
        for text in [
            "https://web.archive.org/web/20170806110612/https://example.com/",
            "$fromMillis(1510067557121)",
            "pl.x 80.95999999999948",
            "tweet 1184523000453971961 was deleted",
        ] {
            assert!(luhn_passing_run(text), "pick a Luhn-valid example: {text}");
            assert!(
                !kinds(text).iter().any(|(k, _)| *k == Kind::Card),
                "{text}: {:?}",
                kinds(text)
            );
        }
        // Real test numbers of every network are still cards, bare or grouped.
        for card in [
            "4111111111111111",
            "4222222222222",
            "5555555555554444",
            "2223003122003222",
            "378282246310005",
            "30569309025904",
            "6011111111111117",
            "3530111333300000",
            "6200000000000005",
            "4111 1111 1111 1111",
            "3782 822463 10005",
        ] {
            assert_eq!(
                kinds(&format!("paid with {card}")),
                vec![(Kind::Card, card.to_owned())],
                "{card}"
            );
        }
    }

    /// Whether `text` holds a 13–19 digit run that passes Luhn (so the case
    /// tests the network check, not the checksum).
    fn luhn_passing_run(text: &str) -> bool {
        CARD.find_iter(text).any(|m| luhn(m.as_str()))
    }

    #[test]
    fn isbns_are_not_card_numbers() {
        // Seen in calibration runs (2026-09-25): a book citation in public
        // source, with a misprinted ISBN that passes the card checksum,
        // became a card placeholder.
        for text in [
            "// and in 'Collected Parsers', edited by the editors, Copyright 2007 Example Press, Inc. 798-0-596-51004-6",
            "978 5 9651 0044 6",
            "9785965100446",
            "ISBN: 9785965100453",
            "isbn-13 978-5965100545",
        ] {
            assert!(
                !kinds(text).iter().any(|(k, _)| *k == Kind::Card),
                "{text}: {:?}",
                kinds(text)
            );
        }
        // Grouped like a card, or labelled as one, it is one.
        for text in [
            "paid with 7980 5965 1004 6",
            "4111 1111 1111 1111",
            "card number 978-0-596-51004-6",
        ] {
            assert!(
                kinds(text).iter().any(|(k, _)| *k == Kind::Card),
                "{text}: {:?}",
                kinds(text)
            );
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

    // Seen in a live hybrid run (2026-09-26, a TypeScript app built from an
    // empty repository): six public values became secret placeholders, among
    // them the path of an EPERM error the frontier was debugging.

    #[test]
    fn hashed_bundle_names_in_build_output_are_not_secrets() {
        let text = "dist/index.html                   0.40 kB │ gzip:  0.27 kB\ndist/assets/index-PPSrQqgT.css    2.83 kB │ gzip:  0.99 kB\ndist/assets/index-DVuHW4gw.js   229.53 kB │ gzip: 71.48 kB\n";
        assert!(kinds(text).is_empty(), "{:?}", kinds(text));
    }

    #[test]
    fn absolute_paths_in_errors_are_not_secrets() {
        let text = "Error: EPERM: operation not permitted, mkdir '/Volumes/EXT_DISK/duet_v2/scratch/fullapp/app/data'\n  path: '/Volumes/EXT_DISK/duet_v2/scratch/fullapp/app/data'\n";
        assert!(kinds(text).is_empty(), "{:?}", kinds(text));
    }

    #[test]
    fn file_urls_in_stack_traces_are_not_secrets() {
        let text = "    at openDatabase (file:///Volumes/EXT_DISK/duet_v2/scratch/fullapp/app/dist/server/db.js:10:9)\n    at createAppWithDatabase (file:///Volumes/EXT_DISK/duet_v2/scratch/fullapp/app/dist/server/app.js:179:22)\n    at file:///Volumes/EXT_DISK/duet_v2/scratch/fullapp/app/dist/server/index.js:7:13\n";
        assert!(kinds(text).is_empty(), "{:?}", kinds(text));
    }

    #[test]
    fn test_suite_paths_are_not_secrets() {
        // Seen in calibration runs (2026-09-25): a test path (entropy 4.008)
        // quoted in the frontier's own command was taken for a secret, and
        // the outbound check refused the request.
        let text =
            "run_command: cat /test/test-suite/groups/function-fromMillis/case000/expected.json";
        assert!(kinds(text).is_empty(), "{:?}", kinds(text));
    }

    #[test]
    fn camel_case_identifiers_with_acronyms_are_not_secrets() {
        let text =
            "    at async asyncRunEntryPointWithESMLoader (node:internal/modules/run_main:101:5) {";
        assert!(kinds(text).is_empty(), "{:?}", kinds(text));
    }

    #[test]
    fn build_and_package_identifiers_are_not_secrets() {
        for text in [
            // Bundlers: vite and rollup, webpack, next, esbuild, parcel, source maps.
            "dist/assets/vendor-react-dom-Bx8f9aQz.js   142.10 kB │ map: 402.33 kB",
            "dist/assets/index-DVuHW4gw.js.map",
            "static/js/vendors-node_modules_react-dom_index_js.3f2a1b9c8d7e6f50a1b2.chunk.js",
            ".next/static/chunks/pages/_app-0a1b2c3d4e5f6789.js",
            ".next/static/chunks/app/dashboard/page-4f8e2a9c1b3d5e7f.js",
            "build/_assets/entry-client-ABCD2345.js",
            "dist/index.a1b2c3d4.js",
            "//# sourceMappingURL=index-DVuHW4gw.js.map",
            // Lockfiles: integrity digests and resolved URLs.
            "\"integrity\": \"sha1-2BcYtEaQmXvTzLpRwNsKdHjFgBc=\"",
            "resolved \"https://registry.yarnpkg.com/@babel/code-frame/-/code-frame-7.24.2.tgz#0a1b2c3d4e5f60718293a4b5c6d7e8f901234567\"",
            "golang.org/x/net v0.25.0 h1:d/OCCoBEUq33pjydKrGQhw7IlUPI2Oylr+8qLr5gYCQ=",
            // Git object ids, UUIDs and timestamps in paths.
            ".git/objects/3f/2a1b9c8d7e6f50a1b2c3d4e5f60718293a4b5c",
            "https://github.com/example-org/task-manager/blob/90b35a6239c3d8bdabc530a6a0816f7ff89a0aaf/src/server/app.ts#L42",
            "GET /api/v1/projects/286a9205-6c69-4f40-ba3b-039fbdd92f38/tasks HTTP/1.1",
            "backups/database-backup-2026-09-25T12-00-00Z.sqlite",
            "migrations/20240101120000_create_users_table/migration.sql",
            // Stack frames: Node, Python, Rust.
            "    at Module._compile (node:internal/modules/cjs/loader:1554:14)",
            "  File \"/home/runner/work/app/app/.venv/lib/python3.12/site-packages/sqlalchemy/engine/base.py\", line 1967, in _exec_single_context",
            "   at /rustc/90b35a6239c3d8bdabc530a6a0816f7ff89a0aaf/library/std/src/panicking.rs:665:5",
        ] {
            assert!(kinds(text).is_empty(), "{text}: {:?}", kinds(text));
        }
    }

    #[test]
    fn secrets_inside_paths_and_urls_are_still_found() {
        let key = "Q8f2LmZ0x9R4tWvB7nC1pK6sD3hJ5gYa";
        for text in [
            format!("GET https://api.internal.test/v1/keys/{key}/rotate HTTP/1.1"),
            format!("curl 'https://files.internal.test/download?token={key}&name=report'"),
            format!("fetch('/api/share/{key}')"),
            format!("saved to uploads/{key}/report.pdf"),
            format!("at file:///srv/app/cache/{key}/index.js:1:1"),
            format!("dist/assets/index-{key}.js"),
        ] {
            let k = kinds(&text);
            assert!(
                k.iter()
                    .any(|(kind, v)| *kind == Kind::Secret && v.contains(key)),
                "{text}: {k:?}"
            );
        }
        // Base64 with slashes is read whole, not by its parts.
        let b64 = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        assert_eq!(kinds(b64), vec![(Kind::Secret, b64.to_owned())]);
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
