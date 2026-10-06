// SPDX-License-Identifier: GPL-3.0-or-later
//! Detectors: never panic; spans are in bounds, on char boundaries, sorted and
//! (merged) disjoint, and every unmerged finding lies inside a merged one.
#![no_main]

use declass_boundary::detect::{Detectors, scan, scan_each};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let merged = scan(&text, Detectors::default());
    let mut last = 0;
    for f in &merged {
        assert!(f.start < f.end && f.end <= text.len());
        assert!(text.is_char_boundary(f.start) && text.is_char_boundary(f.end));
        assert!(f.start >= last);
        last = f.end;
    }
    for f in scan_each(&text, Detectors::default()) {
        assert!(text.is_char_boundary(f.start) && text.is_char_boundary(f.end));
        assert!(merged.iter().any(|m| m.start <= f.start && f.end <= m.end));
    }
});
