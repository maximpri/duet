// SPDX-License-Identifier: GPL-3.0-or-later
//! Copied-span filter: never panics, and its output has no copied run left.
//!
//! Input: sensitive text, `\0`, public text, `\0`, outbound text.
#![no_main]

use declass_boundary::overlap::OverlapIndex;
use libfuzzer_sys::fuzz_target;

/// Words of the redaction marker: sensitive text holding them can form a new
/// run across the marker, so the fixpoint is only asserted without them.
const MARKER_WORDS: [&str; 4] = ["redacted", "copied", "sensitive", "text"];

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let mut parts = input.splitn(3, '\0');
    let (sensitive, public, text) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    let mut idx = OverlapIndex::default();
    idx.add_sensitive(sensitive);
    idx.add_public(public);
    let (out, _) = idx.redact(text);
    let lower = sensitive.to_lowercase();
    if !MARKER_WORDS.iter().any(|w| lower.contains(w)) {
        assert_eq!(idx.redact(&out).1, 0, "copied run left in {out:?}");
    }
});
