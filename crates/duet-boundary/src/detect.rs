// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic detectors for secrets and personal data.
//!
//! Each detector reports byte spans with a kind. Detection is deliberately
//! broad: a false positive costs a placeholder, a false negative costs a leak.

use regex::Regex;
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
}

impl Kind {
    pub const ALL: [Kind; 10] = [
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
    ];

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
re!(SSN, r"\b\d{3}-\d{2}-\d{4}\b");
re!(IBAN, r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b");
re!(
    IPV4,
    r"\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}\b"
);
// Long random-looking tokens (entropy check applied afterwards).
re!(HIGH_ENTROPY, r"[A-Za-z0-9+/_-]{24,}={0,2}");

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

fn iban_valid(s: &str) -> bool {
    let rearranged: String = s[4..].chars().chain(s[..4].chars()).collect();
    let mut rem: u64 = 0;
    for c in rearranged.chars() {
        let v = if c.is_ascii_digit() {
            c as u64 - '0' as u64
        } else if c.is_ascii_uppercase() {
            c as u64 - 'A' as u64 + 10
        } else {
            return false;
        };
        rem = if v >= 10 {
            (rem * 100 + v) % 97
        } else {
            (rem * 10 + v) % 97
        };
    }
    rem == 1
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
}

/// Addresses at domains reserved for documentation and testing (RFC 2606 /
/// RFC 6761) cannot belong to anyone; code and tests use them as examples.
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
    let mut out = Vec::new();
    let mut push = |kind, m: regex::Match<'_>, label: Option<String>| {
        out.push(Finding {
            kind,
            start: m.start(),
            end: m.end(),
            label,
        })
    };
    if d.secrets {
        for m in TOKENS.find_iter(text) {
            push(Kind::Secret, m, None);
        }
        for m in PRIVATE_KEY.find_iter(text) {
            push(Kind::Secret, m, Some("PRIVATE_KEY".into()));
        }
        for c in URL_CREDENTIALS.captures_iter(text) {
            if let Some(m) = c.get(1) {
                push(Kind::Secret, m, Some("URL_PASSWORD".into()));
            }
        }
        for c in ASSIGNMENT
            .captures_iter(text)
            .chain(QUOTED_ASSIGNMENT.captures_iter(text))
        {
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
    if d.entropy {
        for m in HIGH_ENTROPY.find_iter(text) {
            if looks_random(m.as_str()) {
                push(Kind::Secret, m, None);
            }
        }
    }
    if d.pii {
        for m in EMAIL.find_iter(text) {
            if !reserved_example_domain(m.as_str()) {
                push(Kind::Email, m, None);
            }
        }
        for m in PHONE.find_iter(text) {
            push(Kind::Phone, m, None);
        }
        for m in CARD.find_iter(text) {
            if luhn(m.as_str()) {
                push(Kind::Card, m, None);
            }
        }
        for m in SSN.find_iter(text) {
            push(Kind::NationalId, m, None);
        }
        for m in IBAN.find_iter(text) {
            if iban_valid(m.as_str()) {
                push(Kind::Iban, m, None);
            }
        }
        for m in IPV4.find_iter(text) {
            let s = m.as_str();
            if !(s.starts_with("127.") || s == "0.0.0.0" || s.starts_with("255.")) {
                push(Kind::Ip, m, None);
            }
        }
    }
    out.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    out
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
    fn ordinary_code_is_not_flagged() {
        let code = "fn parse_timestamp(s: &str) -> Result<i64, ParseError> {\n    let token = next_token(&mut it);\n    let cfg = Config::from_sources(env_file, config_file)?;\n}";
        assert!(kinds(code).is_empty(), "{:?}", kinds(code));
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
