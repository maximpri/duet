// SPDX-License-Identifier: GPL-3.0-or-later
//! The placeholder vault: sensitive value ↔ stable token, local only.
//!
//! The same value always gets the same token within a run, so the frontier can
//! reason about equality ("the email in row 3 is the one in the error") without
//! seeing the value. Tokens carry kind and origin, never the value, its length
//! or its format. The vault is persisted (mode 0600) so a resumed run keeps its
//! tokens.

use crate::detect::Kind;
use duet_fs::FsError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const OPEN: char = '⟨';
pub const CLOSE: char = '⟩';

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub token: String,
    pub kind: Kind,
    /// Where the value was first seen (a workspace path or a source label).
    pub origin: String,
    /// Another spelling of a value already in the vault (a surname, digits without
    /// separators): replaced by that value's token, never produced by detokenizing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alias: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Vault {
    by_value: BTreeMap<String, Entry>,
    counters: BTreeMap<String, u32>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Vault {
    /// Opens the run's vault, creating it if absent.
    pub fn open(path: &Path) -> Result<Self, FsError> {
        let mut v: Vault = match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| FsError::io("parse vault", path, e))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vault::default(),
            Err(e) => return Err(FsError::io("read vault", path, e)),
        };
        v.path = Some(path.to_path_buf());
        Ok(v)
    }

    pub fn in_memory() -> Self {
        Self::default()
    }

    fn persist(&self) -> Result<(), FsError> {
        if let Some(p) = &self.path {
            duet_fs::private::write_private(p, &serde_json::to_vec(self).unwrap_or_default())?;
        }
        Ok(())
    }

    /// The token for `value`, creating one. `label` names it (e.g. an env key);
    /// without a label the kind is used.
    pub fn token_for(
        &mut self,
        value: &str,
        kind: Kind,
        label: Option<&str>,
        origin: &str,
    ) -> Result<String, FsError> {
        if let Some(e) = self.by_value.get(value) {
            return Ok(e.token.clone());
        }
        let base: String = label
            .map(|l| {
                l.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .take(40)
                    .collect()
            })
            .filter(|l: &String| !l.is_empty())
            .unwrap_or_else(|| kind.tag().to_owned());
        let n = self
            .counters
            .entry(format!("{}:{base}", kind.tag()))
            .or_insert(0);
        *n += 1;
        let token = format!("{OPEN}{}:{base}#{n}{CLOSE}", kind.tag());
        self.by_value.insert(
            value.to_owned(),
            Entry {
                token: token.clone(),
                kind,
                origin: origin.to_owned(),
                alias: false,
            },
        );
        self.persist()?;
        Ok(token)
    }

    /// Makes `spelling` another form of `value` (which must already have a token).
    pub fn alias(&mut self, spelling: &str, value: &str) -> Result<(), FsError> {
        if spelling == value || self.by_value.contains_key(spelling) {
            return Ok(());
        }
        let Some(entry) = self.by_value.get(value).cloned() else {
            return Ok(());
        };
        self.by_value.insert(
            spelling.to_owned(),
            Entry {
                alias: true,
                ..entry
            },
        );
        self.persist()
    }

    /// Whether `value` (or a spelling of it) is a known sensitive value.
    pub fn contains(&self, value: &str) -> bool {
        self.by_value.contains_key(value)
    }

    pub fn len(&self) -> usize {
        self.by_value.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_value.is_empty()
    }

    /// Replaces every known value in `text` by its token (longest values first,
    /// so a value containing another is replaced whole).
    pub fn tokenize(&self, text: &str) -> (String, usize) {
        let mut values: Vec<&String> = self.by_value.keys().collect();
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        let mut out = text.to_owned();
        let mut count = 0;
        for v in values {
            if v.len() < 4 {
                continue;
            }
            let hits = out.matches(v.as_str()).count();
            if hits > 0 {
                out = out.replace(v.as_str(), &self.by_value[v].token);
                count += hits;
            }
        }
        (out, count)
    }

    /// The value behind `token`, if it exists.
    pub fn value_of(&self, token: &str) -> Option<(&str, &Entry)> {
        self.by_value
            .iter()
            .find(|(_, e)| e.token == token && !e.alias)
            .map(|(v, e)| (v.as_str(), e))
    }

    /// Tokens present in `text`, in order of appearance.
    pub fn tokens_in(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(i) = rest.find(OPEN) {
            let after = &rest[i..];
            match after.find(CLOSE) {
                Some(j) => {
                    out.push(after[..j + CLOSE.len_utf8()].to_owned());
                    rest = &after[j + CLOSE.len_utf8()..];
                }
                None => break,
            }
        }
        out
    }

    /// Replaces tokens with their values. Unknown tokens are left untouched and returned.
    pub fn detokenize(&self, text: &str) -> (String, Vec<String>) {
        let mut out = text.to_owned();
        let mut unknown = Vec::new();
        for token in Self::tokens_in(text) {
            match self.value_of(&token) {
                Some((value, _)) => out = out.replace(&token, value),
                None => unknown.push(token),
            }
        }
        (out, unknown)
    }

    /// Values of the given kinds (for scanning outbound text).
    pub fn values(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.by_value.iter().map(|(v, e)| (v.as_str(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_tokens_round_trip() {
        let mut v = Vault::in_memory();
        let a = v
            .token_for(
                "sk_live_abcdef123456",
                Kind::Secret,
                Some("PAYMENTS_API_KEY"),
                ".env",
            )
            .unwrap();
        let again = v
            .token_for("sk_live_abcdef123456", Kind::Secret, None, "logs/x")
            .unwrap();
        assert_eq!(a, again);
        assert_eq!(a, "⟨secret:PAYMENTS_API_KEY#1⟩");
        let e = v
            .token_for("amelia@example.test", Kind::Email, None, "data/u.csv")
            .unwrap();
        assert_eq!(e, "⟨email:email#1⟩");
        let text = "key sk_live_abcdef123456 for amelia@example.test";
        let (t, n) = v.tokenize(text);
        assert_eq!(n, 2);
        assert!(!t.contains("sk_live") && !t.contains("amelia"));
        let (back, unknown) = v.detokenize(&t);
        assert_eq!(back, text);
        assert!(unknown.is_empty());
    }

    #[test]
    fn longer_values_replace_first_and_unknown_tokens_are_reported() {
        let mut v = Vault::in_memory();
        v.token_for("abcd1234", Kind::Secret, Some("A"), "x")
            .unwrap();
        v.token_for("abcd1234-extended", Kind::Secret, Some("B"), "x")
            .unwrap();
        let (t, _) = v.tokenize("abcd1234-extended");
        assert_eq!(t, "⟨secret:B#1⟩");
        let (_, unknown) = v.detokenize("use ⟨secret:NOPE#9⟩");
        assert_eq!(unknown, vec!["⟨secret:NOPE#9⟩"]);
    }

    #[test]
    fn persists_privately() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.json");
        let mut v = Vault::open(&p).unwrap();
        v.token_for("s3cretvalue", Kind::Secret, Some("K"), ".env")
            .unwrap();
        let reopened = Vault::open(&p).unwrap();
        assert_eq!(reopened.tokenize("s3cretvalue").0, "⟨secret:K#1⟩");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
