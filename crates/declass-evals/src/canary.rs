// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-run canaries.
//!
//! Task files contain placeholders of the form `{{canary:<kind>:<name>}}`.
//! Each run renders them with fresh, realistic-looking values derived from the
//! run seed. Values carry no marker Declass could key on, so the security engine
//! must detect them the way it would detect real sensitive content. The
//! manifest lists every value and the textual variants the leak proxy scans
//! for.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanaryKind {
    /// API-key-shaped token.
    Secret,
    /// Password-shaped string.
    Password,
    Email,
    Phone,
    /// Person name with a synthetic, rare surname.
    Person,
    /// A single-word display name (synthetic, rare).
    Mononym,
    /// A business number such as revenue; not secret-shaped.
    Number,
    /// A unique identifier-like literal planted in source code.
    Source,
    /// A unique phrase embedded in prompt-injection text.
    Injection,
}

impl CanaryKind {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "secret" => Self::Secret,
            "password" => Self::Password,
            "email" => Self::Email,
            "phone" => Self::Phone,
            "person" => Self::Person,
            "mononym" => Self::Mononym,
            "number" => Self::Number,
            "source" => Self::Source,
            "injection" => Self::Injection,
            other => bail!("unknown canary kind {other:?}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canary {
    pub kind: CanaryKind,
    pub name: String,
    pub value: String,
    /// Every textual form the proxy looks for in outbound traffic.
    pub variants: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub run_id: String,
    pub seed: u64,
    pub canaries: Vec<Canary>,
}

/// Deterministic generator: the same (task, seed) always yields the same values,
/// so lanes paired on a seed see identical canaries.
pub struct Generator {
    state: u64,
    by_key: BTreeMap<(CanaryKind, String), Canary>,
}

impl Generator {
    pub fn new(task_id: &str, seed: u64) -> Self {
        let digest = Sha256::digest(format!("{task_id}:{seed}").as_bytes());
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        Self {
            state: u64::from_le_bytes(bytes),
            by_key: BTreeMap::new(),
        }
    }

    fn next(&mut self) -> u64 {
        // splitmix64
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }

    fn chars(&mut self, alphabet: &[u8], n: usize) -> String {
        (0..n)
            .map(|_| alphabet[(self.next() % alphabet.len() as u64) as usize] as char)
            .collect()
    }

    fn digits(&mut self, n: usize) -> String {
        self.chars(b"0123456789", n)
    }

    fn rare_surname(&mut self) -> String {
        const A: &[&str] = &[
            "Vel", "Kor", "Mar", "Zeth", "Quen", "Brav", "Tolv", "Sab", "Yor",
        ];
        const B: &[&str] = &["an", "ek", "is", "ov", "ul", "ar", "en", "ith"];
        const C: &[&str] = &["dt", "rin", "sko", "mont", "wick", "lek", "baum", "vay"];
        format!("{}{}{}", self.pick(A), self.pick(B), self.pick(C))
    }

    fn generate(&mut self, kind: CanaryKind) -> (String, Vec<String>) {
        const B62: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        const FIRST: &[&str] = &[
            "Amelia", "Jonas", "Priya", "Mateo", "Sofia", "Kenji", "Leila", "Oscar", "Ines",
        ];
        match kind {
            CanaryKind::Secret => {
                let v = format!("sk_live_{}", self.chars(B62, 30));
                (v.clone(), vec![v])
            }
            CanaryKind::Password => {
                let v = format!(
                    "{}{}!{}",
                    self.chars(B62, 7),
                    self.digits(3),
                    self.chars(B62, 5)
                );
                (v.clone(), vec![v])
            }
            CanaryKind::Email => {
                let first = self.pick(FIRST).to_lowercase();
                let last = self.rare_surname().to_lowercase();
                let v = format!(
                    "{first}.{last}{}@mailbox-{}.net",
                    self.digits(2),
                    self.digits(3)
                );
                (v.clone(), vec![v])
            }
            CanaryKind::Phone => {
                let (a, b, c) = (self.digits(3), self.digits(3), self.digits(4));
                let v = format!("+1 ({a}) {b}-{c}");
                (v, vec![format!("({a}) {b}-{c}"), format!("{a}{b}{c}")])
            }
            CanaryKind::Person => {
                let v = format!("{} {}", self.pick(FIRST), self.rare_surname());
                let surname = v.split(' ').nth(1).unwrap_or_default().to_owned();
                (v.clone(), vec![v, surname])
            }
            CanaryKind::Mononym => {
                let v = self.rare_surname();
                (v.clone(), vec![v])
            }
            CanaryKind::Number => {
                let n = 1_000_000 + self.next() % 8_999_999;
                let plain = n.to_string();
                let grouped = format!("{},{},{}", n / 1_000_000, &plain[1..4], &plain[4..]);
                (grouped.clone(), vec![grouped, plain])
            }
            CanaryKind::Source => {
                let v = format!(
                    "zq{}",
                    self.chars(b"abcdefghijklmnopqrstuvwxyz0123456789", 14)
                );
                (v.clone(), vec![v])
            }
            CanaryKind::Injection => {
                let v = format!("verify-token {}", self.chars(B62, 16));
                let token = v.split(' ').nth(1).unwrap_or_default().to_owned();
                (v, vec![token])
            }
        }
    }

    /// The canary for `(kind, name)`, created on first use.
    pub fn get(&mut self, kind: CanaryKind, name: &str) -> Canary {
        if let Some(c) = self.by_key.get(&(kind, name.to_owned())) {
            return c.clone();
        }
        let (value, variants) = self.generate(kind);
        let canary = Canary {
            kind,
            name: name.to_owned(),
            value,
            variants,
        };
        self.by_key.insert((kind, name.to_owned()), canary.clone());
        canary
    }

    /// Replaces every `{{canary:kind:name}}` in `text`.
    pub fn render(&mut self, text: &str) -> Result<String> {
        const OPEN: &str = "{{canary:";
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find(OPEN) {
            out.push_str(&rest[..start]);
            let after = &rest[start + OPEN.len()..];
            let end = after
                .find("}}")
                .context("unterminated {{canary:...}} placeholder")?;
            let (kind, name) = after[..end]
                .split_once(':')
                .with_context(|| format!("placeholder {:?} needs kind:name", &after[..end]))?;
            let canary = self.get(CanaryKind::parse(kind)?, name);
            out.push_str(&canary.value);
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    pub fn manifest(&self, run_id: &str, seed: u64) -> Manifest {
        Manifest {
            run_id: run_id.to_owned(),
            seed,
            canaries: self.by_key.values().cloned().collect(),
        }
    }
}

impl Manifest {
    /// Canaries whose variants occur in `text`, scanning raw text and its
    /// JSON-escaped forms.
    pub fn find_in(&self, text: &str) -> Vec<&Canary> {
        self.canaries
            .iter()
            .filter(|c| {
                c.variants.iter().any(|v| {
                    text.contains(v.as_str()) || {
                        let escaped = serde_json::to_string(v).unwrap_or_default();
                        let escaped = escaped.trim_matches('"');
                        escaped != v && text.contains(escaped)
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_values_different_seed_different_values() {
        let text = "KEY={{canary:secret:KEY}}\nmail={{canary:email:cust1}}\n";
        let a = Generator::new("M1", 7).render(text).unwrap();
        let b = Generator::new("M1", 7).render(text).unwrap();
        let c = Generator::new("M1", 8).render(text).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(!a.contains("{{"));
    }

    #[test]
    fn repeated_placeholder_gets_one_value() {
        let mut g = Generator::new("M1", 1);
        let out = g
            .render("{{canary:person:alice}} / {{canary:person:alice}}")
            .unwrap();
        let (x, y) = out.split_once(" / ").unwrap();
        assert_eq!(x, y);
        assert_eq!(g.manifest("r", 1).canaries.len(), 1);
    }

    #[test]
    fn values_carry_no_marker_and_are_long_enough() {
        let mut g = Generator::new("X", 3);
        for kind in [
            CanaryKind::Secret,
            CanaryKind::Password,
            CanaryKind::Email,
            CanaryKind::Phone,
            CanaryKind::Person,
            CanaryKind::Number,
            CanaryKind::Source,
            CanaryKind::Injection,
        ] {
            let c = g.get(kind, "n");
            assert!(!c.value.to_lowercase().contains("canary"), "{c:?}");
            assert!(c.variants.iter().all(|v| v.len() >= 7), "{c:?}");
        }
    }

    #[test]
    fn finds_raw_json_escaped_and_variant_forms() {
        let mut g = Generator::new("S2", 11);
        let n = g.get(CanaryKind::Number, "rev");
        let p = g.get(CanaryKind::Phone, "ph");
        let m = g.manifest("r", 11);
        let plain = n.variants[1].clone();
        assert_eq!(m.find_in(&format!("total {plain} usd")).len(), 1);
        assert_eq!(m.find_in(&format!("call {}", p.variants[1])).len(), 1);
        assert!(m.find_in("nothing here").is_empty());
    }

    #[test]
    fn rejects_bad_placeholders() {
        let mut g = Generator::new("X", 1);
        assert!(g.render("{{canary:bogus:x}}").is_err());
        assert!(g.render("{{canary:secret}}").is_err());
        assert!(g.render("{{canary:secret:x").is_err());
    }
}
