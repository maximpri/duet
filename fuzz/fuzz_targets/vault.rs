// SPDX-License-Identifier: GPL-3.0-or-later
//! Vault: tokenize never panics, is idempotent and leaves no value outside the
//! tokens; detokenize restores the text.
//!
//! Input: values separated by `\n`, then `\0`, then the text.
#![no_main]

use declass_boundary::detect::Kind;
use declass_boundary::vault::{CLOSE, MIN_VALUE_BYTES, OPEN, Vault};
use libfuzzer_sys::fuzz_target;

const KINDS: [Kind; 4] = [Kind::Secret, Kind::Email, Kind::Name, Kind::Data];

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let (values, text) = input.split_once('\0').unwrap_or((&input, ""));
    let mut vault = Vault::in_memory();
    let mut known = Vec::new();
    for (i, v) in values.split('\n').take(32).enumerate() {
        if v.is_empty() || v.contains([OPEN, CLOSE]) || vault.contains(v) {
            continue;
        }
        let label = (i % 2 == 0).then_some(v);
        vault
            .token_for(v, KINDS[i % KINDS.len()], label, "fuzz")
            .unwrap();
        known.push(v.to_owned());
    }
    let (once, _) = vault.tokenize(text);
    let (twice, n) = vault.tokenize(&once);
    assert_eq!(once, twice);
    assert_eq!(n, 0);
    let rest = vault.strip_tokens(&once);
    for v in known.iter().filter(|v| v.len() >= MIN_VALUE_BYTES) {
        assert!(!rest.contains(v.as_str()), "{v:?} left in {once:?}");
    }
    // Text that cannot spell a token round-trips.
    if !text.contains([OPEN, CLOSE]) {
        assert_eq!(vault.detokenize(&once).0, text);
    }
});
