// SPDX-License-Identifier: GPL-3.0-or-later
//! SSE decoder: never panics, and events do not depend on chunking.
#![no_main]

use declass_provider::sse::SseDecoder;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The first byte picks a chunk size for the split decode.
    let Some((&size, bytes)) = data.split_first() else {
        return;
    };
    let mut whole = SseDecoder::default();
    let mut expected = whole.push(bytes);
    expected.extend(whole.finish());

    let mut split = SseDecoder::default();
    let mut got = Vec::new();
    for chunk in bytes.chunks(usize::from(size).max(1)) {
        got.extend(split.push(chunk));
    }
    got.extend(split.finish());
    assert_eq!(expected, got);
});
