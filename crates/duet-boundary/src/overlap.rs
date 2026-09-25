// SPDX-License-Identifier: GPL-3.0-or-later
//! Copied-span filter.
//!
//! Detectors only find content that has a recognizable shape. Confidential
//! prose ("Q3 revenue for Northwind was 4,812,339") has none, so the gate also
//! refuses to send long runs of text copied from sensitive content. Every
//! sensitive handle is indexed as hashes of 8-token windows; outbound text with
//! 3 or more consecutive matching windows (~24 tokens) has that span redacted.
//! Text the local model writes about sensitive content is checked much more
//! strictly ([`LOCAL_WINDOW`]-token windows): it should describe, never quote.
//! Windows that also occur in public files are exempt, so ordinary shared code
//! is never redacted.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

pub const WINDOW: usize = 8;
pub const MIN_CONSECUTIVE: usize = 3;
pub const REDACTED: &str = "⟨redacted:copied-sensitive-text⟩";

/// Word tokens with their byte ranges (letters/digits runs; everything else separates).
fn tokens(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, i));
        }
    }
    if let Some(s) = start {
        out.push((s, text.len()));
    }
    out
}

fn window_hashes(text: &str, window: usize) -> Vec<(u64, usize, usize)> {
    let toks = tokens(text);
    if toks.len() < window {
        return Vec::new();
    }
    (0..=toks.len() - window)
        .map(|i| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for &(s, e) in &toks[i..i + window] {
                text[s..e].to_lowercase().hash(&mut h);
            }
            (h.finish(), toks[i].0, toks[i + window - 1].1)
        })
        .collect()
}

/// Local-model output is a description of sensitive content, never a
/// quotation: any run of this many consecutive tokens copied from sensitive
/// content is removed (the general outbound filter needs ~24).
pub const LOCAL_WINDOW: usize = 4;

/// One index: windows of `window` tokens, a span counts once `min_run`
/// consecutive windows match sensitive text and not public text.
struct Level {
    window: usize,
    min_run: usize,
    sensitive: HashSet<u64>,
    public: HashSet<u64>,
}

impl Level {
    fn new(window: usize, min_run: usize) -> Self {
        Self {
            window,
            min_run,
            sensitive: HashSet::new(),
            public: HashSet::new(),
        }
    }

    fn redact(&self, text: &str) -> (String, usize) {
        if self.sensitive.is_empty() {
            return (text.to_owned(), 0);
        }
        let windows = window_hashes(text, self.window);
        let hit: Vec<bool> = windows
            .iter()
            .map(|(h, _, _)| self.sensitive.contains(h) && !self.public.contains(h))
            .collect();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < hit.len() {
            if !hit[i] {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < hit.len() && hit[j] {
                j += 1;
            }
            if j - i >= self.min_run {
                let (s, e) = (windows[i].1, windows[j - 1].2);
                // Windows overlap by up to `window - 1` tokens, so a run separated
                // from the previous one by a single miss can start inside it.
                match spans.last_mut() {
                    Some(prev) if s <= prev.1 => prev.1 = prev.1.max(e),
                    _ => spans.push((s, e)),
                }
            }
            i = j;
        }
        if spans.is_empty() {
            return (text.to_owned(), 0);
        }
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for &(s, e) in &spans {
            out.push_str(&text[last..s]);
            out.push_str(REDACTED);
            last = e;
        }
        out.push_str(&text[last..]);
        (out, spans.len())
    }
}

pub struct OverlapIndex {
    normal: Level,
    strict: Level,
}

impl Default for OverlapIndex {
    fn default() -> Self {
        Self {
            normal: Level::new(WINDOW, MIN_CONSECUTIVE),
            strict: Level::new(LOCAL_WINDOW, 1),
        }
    }
}

impl OverlapIndex {
    pub fn add_sensitive(&mut self, text: &str) {
        for level in [&mut self.normal, &mut self.strict] {
            let w = level.window;
            level
                .sensitive
                .extend(window_hashes(text, w).into_iter().map(|w| w.0));
        }
    }

    pub fn add_public(&mut self, text: &str) {
        for level in [&mut self.normal, &mut self.strict] {
            let w = level.window;
            level
                .public
                .extend(window_hashes(text, w).into_iter().map(|w| w.0));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.normal.sensitive.is_empty()
    }

    /// Redacts copied sensitive spans (~24+ tokens). Returns the text and how
    /// many spans were removed.
    pub fn redact(&self, text: &str) -> (String, usize) {
        self.normal.redact(text)
    }

    /// Redacts any run of [`LOCAL_WINDOW`] or more tokens copied from sensitive
    /// content: for text the local model wrote about that content.
    pub fn redact_strict(&self, text: &str) -> (String, usize) {
        self.strict.redact(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET_NOTE: &str = "Board memo: the acquisition of Northwind closes on the fourteenth; \
        do not disclose that Q3 revenue for Northwind was four point eight million or that \
        the offer price is thirty two dollars per share until the announcement.";

    #[test]
    fn redacts_long_copied_spans_only() {
        let mut idx = OverlapIndex::default();
        idx.add_sensitive(SECRET_NOTE);
        let leak = format!("The log says: {}", &SECRET_NOTE[12..160]);
        let (out, n) = idx.redact(&leak);
        assert_eq!(n, 1);
        assert!(
            out.contains(REDACTED) && !out.contains("acquisition of Northwind closes"),
            "{out}"
        );
        // A short mention is allowed (it is below the ~24-token threshold).
        let (short, n) = idx.redact("It mentions Northwind and the announcement.");
        assert_eq!(n, 0);
        assert_eq!(short, "It mentions Northwind and the announcement.");
    }

    #[test]
    fn copied_runs_split_by_a_public_window_are_merged() {
        // One window in the middle of a copied passage also occurs in public text,
        // so it is exempt: the passage splits into two runs whose byte spans overlap
        // (neighbouring windows share 7 tokens). Redaction must merge them.
        let words: Vec<&str> = SECRET_NOTE.split_whitespace().collect();
        for mid in WINDOW..words.len() - 2 * WINDOW {
            let mut idx = OverlapIndex::default();
            idx.add_sensitive(SECRET_NOTE);
            idx.add_public(&words[mid..mid + WINDOW].join(" "));
            let (out, n) = idx.redact(SECRET_NOTE);
            assert!(n >= 1, "{out}");
            assert!(!out.contains("acquisition of Northwind closes"), "{out}");
        }
    }

    #[test]
    fn public_text_is_exempt() {
        let code = "fn reconcile(statements: &[Statement], entries: &[Entry]) -> Reconciliation { \
                    let mut used = vec![false; statements.len()]; for entry in entries { match_entry(entry, &mut used); } }";
        let mut idx = OverlapIndex::default();
        idx.add_sensitive(code); // e.g. a log that echoes source code
        idx.add_public(code);
        assert_eq!(idx.redact(code).1, 0);
    }

    #[test]
    fn case_and_punctuation_do_not_hide_copies() {
        let mut idx = OverlapIndex::default();
        idx.add_sensitive(SECRET_NOTE);
        let shouted = SECRET_NOTE.to_uppercase().replace(',', " ;");
        assert_eq!(idx.redact(&shouted).1, 1);
    }
}
