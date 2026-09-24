// SPDX-License-Identifier: GPL-3.0-or-later
//! The placeholder vault: sensitive value ↔ stable token, local only.
//!
//! The same value always gets the same token within a run, so the frontier can
//! reason about equality ("the email in row 3 is the one in the error") without
//! seeing the value. Tokens carry kind and origin, never the value, its length
//! or its format. The vault is persisted (mode 0600) so a resumed run keeps its
//! tokens.

use crate::detect::Kind;
use aho_corasick::{AhoCorasick, MatchKind};
use duet_fs::FsError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    /// Built on first use after a change; see [`Vault::tokenize`].
    #[serde(skip)]
    matcher: OnceLock<Matcher>,
}

/// One automaton over every value (4+ bytes) and every token. A token matches
/// as itself, so text inside a token is never rewritten.
#[derive(Debug)]
struct Matcher {
    automaton: Option<AhoCorasick>,
    tokens: std::collections::HashSet<String>,
    /// Per pattern: the replacement, or `None` for a token kept as is.
    replacement: Vec<Option<String>>,
}

/// Values shorter than this are never replaced (too many false positives).
pub const MIN_VALUE_BYTES: usize = 4;

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
        self.matcher = OnceLock::new();
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
        self.matcher = OnceLock::new();
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

    fn matcher(&self) -> &Matcher {
        self.matcher.get_or_init(|| {
            let mut patterns: Vec<&str> = Vec::new();
            let mut replacement = Vec::new();
            let mut tokens: Vec<&str> = self.by_value.values().map(|e| e.token.as_str()).collect();
            tokens.sort_unstable();
            tokens.dedup();
            for t in tokens {
                patterns.push(t);
                replacement.push(None);
            }
            for (v, e) in &self.by_value {
                if v.len() >= MIN_VALUE_BYTES {
                    patterns.push(v);
                    replacement.push(Some(e.token.clone()));
                }
            }
            let automaton = AhoCorasick::builder()
                .match_kind(MatchKind::LeftmostLongest)
                .build(&patterns)
                .ok();
            Matcher {
                automaton,
                tokens: self.by_value.values().map(|e| e.token.clone()).collect(),
                replacement,
            }
        })
    }

    /// Replaces every known value in `text` by its token, in one left-to-right
    /// pass preferring the longest value at each position (so a value
    /// containing another is replaced whole). Tokens already in the text,
    /// including ones this pass inserts, are never rewritten, so a value that
    /// also spells part of a token (`secret`, a key name) cannot corrupt it and
    /// tokenizing twice changes nothing.
    pub fn tokenize(&self, text: &str) -> (String, usize) {
        let m = self.matcher();
        let Some(ac) = &m.automaton else {
            return (text.to_owned(), 0);
        };
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        let mut count = 0;
        for hit in ac.find_iter(text) {
            if let Some(token) = &m.replacement[hit.pattern().as_usize()] {
                out.push_str(&text[last..hit.start()]);
                out.push_str(token);
                last = hit.end();
                count += 1;
            }
        }
        out.push_str(&text[last..]);
        (out, count)
    }

    /// The value behind `token`, if it exists.
    pub fn value_of(&self, token: &str) -> Option<(&str, &Entry)> {
        self.by_value
            .iter()
            .find(|(_, e)| e.token == token && !e.alias)
            .map(|(v, e)| (v.as_str(), e))
    }

    /// Byte ranges of the tokens in `text`, in order: an opening bracket up to
    /// the next closing one, with no other opening bracket between (tokens
    /// never contain one, so a stray `⟨` before a token does not swallow it).
    fn token_spans(text: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut open = None;
        for (i, c) in text.char_indices() {
            if c == OPEN {
                open = Some(i);
            } else if c == CLOSE
                && let Some(s) = open.take()
            {
                out.push((s, i + CLOSE.len_utf8()));
            }
        }
        out
    }

    /// `text` with every known token replaced by a space (unknown bracketed
    /// text is kept: it may be anything, including a value).
    pub fn strip_tokens(&self, text: &str) -> String {
        let known = &self.matcher().tokens;
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for (s, e) in Self::token_spans(text) {
            if known.contains(&text[s..e]) {
                out.push_str(&text[last..s]);
                out.push(' ');
                last = e;
            }
        }
        out.push_str(&text[last..]);
        out
    }

    /// Tokens present in `text`, in order of appearance.
    pub fn tokens_in(text: &str) -> Vec<String> {
        Self::token_spans(text)
            .into_iter()
            .map(|(s, e)| text[s..e].to_owned())
            .collect()
    }

    /// Replaces tokens with their values in one pass (a restored value is never
    /// read again as a token). Unknown tokens are left untouched and returned.
    pub fn detokenize(&self, text: &str) -> (String, Vec<String>) {
        let mut out = String::with_capacity(text.len());
        let mut unknown = Vec::new();
        let mut last = 0;
        for (s, e) in Self::token_spans(text) {
            let token = &text[s..e];
            if let Some((value, _)) = self.value_of(token) {
                out.push_str(&text[last..s]);
                out.push_str(value);
                last = e;
            } else {
                unknown.push(token.to_owned());
            }
        }
        out.push_str(&text[last..]);
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
    fn a_value_that_spells_part_of_a_token_leaves_tokens_intact() {
        // Found by the vault properties: replacing values one after another
        // rewrote text inside tokens inserted earlier (`secret` inside
        // `⟨secret:API_KEY#1⟩`), so tokens were corrupted and did not round-trip.
        let mut v = Vault::in_memory();
        v.token_for(
            "sk_live_abcdef123456",
            Kind::Secret,
            Some("API_KEY"),
            ".env",
        )
        .unwrap();
        v.token_for("secret", Kind::Secret, Some("MODE"), ".env")
            .unwrap();
        v.token_for("API_KEY", Kind::Secret, Some("NAME"), ".env")
            .unwrap();
        let text = "key sk_live_abcdef123456, mode secret, name API_KEY";
        let (t, n) = v.tokenize(text);
        assert_eq!(n, 3);
        assert_eq!(
            t,
            "key ⟨secret:API_KEY#1⟩, mode ⟨secret:MODE#1⟩, name ⟨secret:NAME#1⟩"
        );
        assert_eq!(v.tokenize(&t), (t.clone(), 0), "idempotent");
        assert_eq!(v.detokenize(&t).0, text);
        assert_eq!(v.detokenize(&format!("⟨{t}")).0, format!("⟨{text}"));
        assert_eq!(v.strip_tokens(&t), "key  , mode  , name  ");
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
