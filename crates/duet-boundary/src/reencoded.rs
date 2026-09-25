// SPDX-License-Identifier: GPL-3.0-or-later
//! Other spellings of a value in text the local model writes: its characters
//! spaced out (`V a k d r i l`, `4-5-3-9`), or base64 or hex of it.
//!
//! The value filters match values as written, so the engine normalizes local
//! output before matching: spaced-out runs are reduced to their characters
//! and matched against the vault's skeletons ([`crate::vault::skeleton`]), and
//! encoded runs are decoded at every alignment and the decoded bytes matched
//! against the vault and the copied-span index.
//!
//! Encoded runs are decoded rather than refused outright: ordinary words are
//! valid base64 (and many are valid hex), so a rule that refuses what looks
//! encoded either misses a short value's encoding (8 characters, no digits,
//! no padding) or removes ordinary words from every answer. Decoding and
//! matching has neither problem; what it cannot see is an encoding of
//! something the engine does not know (see SECURITY.md, Known limits).

/// Shown in place of an encoded run that decodes to sensitive content.
pub const ENCODED: &str = "⟨redacted:encoded-sensitive-text⟩";

/// Characters that end a spaced-out run (punctuation between list items).
/// Any other character that is not a letter or digit may stand between the
/// characters of a spaced-out value (` `, `-`, `.`, `_`, `@`, ...).
const BREAKS: &[char] = &[
    ',', ';', ':', '!', '?', '(', ')', '[', ']', '{', '}', '"', '\'', '`', '<', '>', '|', '⟨', '⟩',
    '“', '”', '‘', '’',
];
/// Most characters between two characters of a spaced-out run.
pub const GAP_MAX: usize = 3;
/// Fewest characters in a spaced-out run.
pub const SPACED_MIN: usize = 4;
/// Shortest base64 run decoded (4 bytes, unpadded).
pub const BASE64_MIN: usize = 6;
/// Shortest hex run decoded (4 bytes).
pub const HEX_MIN: usize = 8;

/// A run of single characters with separators between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spaced {
    /// Byte range of the run, from its first character to its last.
    pub start: usize,
    pub end: usize,
    /// Byte range of each letter or digit of the run.
    pub chars: Vec<(usize, usize)>,
}

/// Every spaced-out run in `text`: [`SPACED_MIN`] or more letters or digits,
/// each standing alone (no letter or digit next to it), with 1 to
/// [`GAP_MAX`] other characters between each two, none of them a list break
/// (`,`, `;`, `:`, brackets, quotes, ...).
pub fn spaced_runs(text: &str) -> Vec<Spaced> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    // Letters and digits standing alone, in order.
    let mut units: Vec<(usize, usize)> = Vec::new();
    for (i, &(at, c)) in chars.iter().enumerate() {
        let alone = |j: Option<usize>| {
            j.and_then(|j| chars.get(j))
                .is_none_or(|&(_, n)| !n.is_alphanumeric())
        };
        if c.is_alphanumeric() && alone(i.checked_sub(1)) && alone(Some(i + 1)) {
            units.push((at, at + c.len_utf8()));
        }
    }
    let mut out = Vec::new();
    let mut run: Vec<(usize, usize)> = Vec::new();
    let flush = |run: &mut Vec<(usize, usize)>, out: &mut Vec<Spaced>| {
        if run.len() >= SPACED_MIN {
            out.push(Spaced {
                start: run[0].0,
                end: run[run.len() - 1].1,
                chars: std::mem::take(run),
            });
        }
        run.clear();
    };
    for u in units {
        if let Some(&(_, prev_end)) = run.last() {
            let gap = &text[prev_end..u.0];
            let n = gap.chars().count();
            if !(1..=GAP_MAX).contains(&n) || gap.chars().any(|c| BREAKS.contains(&c)) {
                flush(&mut run, &mut out);
            }
        }
        run.push(u);
    }
    flush(&mut run, &mut out);
    out
}

/// A run of text that may be base64 or hex, and what it decodes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    /// Byte range of the run, padding included.
    pub start: usize,
    pub end: usize,
    /// The bytes it decodes to at each alignment, as base64 (standard or
    /// URL-safe) and, when it is also hex, as hex.
    pub decoded: Vec<Vec<u8>>,
}

fn base64_value(b: u8) -> Option<u32> {
    Some(match b {
        b'A'..=b'Z' => b - b'A',
        b'a'..=b'z' => b - b'a' + 26,
        b'0'..=b'9' => b - b'0' + 52,
        b'+' | b'-' => 62,
        b'/' | b'_' => 63,
        _ => return None,
    } as u32)
}

/// `digits` (base64 characters, no padding) decoded; a trailing partial byte
/// is dropped.
fn decode_base64(digits: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &d in digits {
        let Some(v) = base64_value(d) else { break };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

fn decode_hex(digits: &[u8]) -> Vec<u8> {
    digits
        .chunks_exact(2)
        .filter_map(|p| {
            let h = (p[0] as char).to_digit(16)?;
            let l = (p[1] as char).to_digit(16)?;
            Some((h * 16 + l) as u8)
        })
        .collect()
}

/// Every run of base64 characters (standard or URL-safe, [`BASE64_MIN`] or
/// more, then optional `=` padding) in `text`, decoded at each of its four
/// alignments, and each run of [`HEX_MIN`]+ hex digits inside it decoded at
/// both of its alignments: an encoding embedded in a longer run, or preceded
/// by a few other characters (`0x`), still decodes to the value at one of them.
pub fn encoded_runs(text: &str) -> Vec<Encoded> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if base64_value(bytes[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && base64_value(bytes[i]).is_some() {
            i += 1;
        }
        let digits = &bytes[start..i];
        let mut end = i;
        while end < bytes.len() && end - i < 2 && bytes[end] == b'=' {
            end += 1;
        }
        if digits.len() < BASE64_MIN {
            i = end.max(i);
            continue;
        }
        let mut decoded: Vec<Vec<u8>> = (0..4.min(digits.len()))
            .map(|k| decode_base64(&digits[k..]))
            .filter(|d| !d.is_empty())
            .collect();
        // Hex inside the run too (`0x54686f…`, `id54686f…`).
        for hex in digits
            .split(|b| !b.is_ascii_hexdigit())
            .filter(|h| h.len() >= HEX_MIN)
        {
            decoded.extend((0..2).map(|k| decode_hex(&hex[k..])));
        }
        out.push(Encoded {
            start,
            end,
            decoded,
        });
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for g in bytes.chunks(3) {
            let n = (u32::from(g[0]) << 16)
                | (u32::from(*g.get(1).unwrap_or(&0)) << 8)
                | u32::from(*g.get(2).unwrap_or(&0));
            for k in 0..4 {
                if k <= g.len() {
                    s.push(T[(n >> (18 - 6 * k)) as usize & 63] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    fn spelled(text: &str) -> Vec<String> {
        spaced_runs(text)
            .iter()
            .map(|r| r.chars.iter().map(|&(s, e)| &text[s..e]).collect())
            .collect()
    }

    #[test]
    fn spaced_out_values_are_found_whatever_the_separators() {
        let text = "Spelled out: 2     1, V a k d r i l   T h o r s k o, v a k . t h o @ k e s . n e t, \
                    4-5-3-9 1_4_8_8, x y z, a-b@c+d.";
        assert_eq!(
            spelled(text),
            vec!["VakdrilThorsko", "vakthokesnet", "45391488", "abcd"]
        );
        // Words, short runs and list items are not spelled-out values.
        for plain in [
            "The value is withheld; line 3 has 5 fields.",
            "a b c",
            "Options: a, b, c, d.",
            "Fields 1, 2, 3 and 4 are set.",
        ] {
            assert!(
                spaced_runs(plain).is_empty(),
                "{plain}: {:?}",
                spelled(plain)
            );
        }
    }

    #[test]
    fn encoded_runs_decode_at_every_alignment() {
        let text = format!(
            "In base64: {}, url-safe {}, embedded x{}, hex {} and {}.",
            b64(b"Thorsko"),
            b64(b"a?>b~Brennvik").replace('+', "-").replace('/', "_"),
            b64(b"..Marrquin"),
            "54686f72736b6f",
            "0x054686f72736b6f"
        );
        let runs = encoded_runs(&text);
        let found = |needle: &[u8]| {
            runs.iter().any(|r| {
                r.decoded
                    .iter()
                    .any(|d| d.windows(needle.len()).any(|w| w == needle))
            })
        };
        for needle in [&b"Thorsko"[..], b"Brennvik", b"Marrquin"] {
            assert!(found(needle), "{}", String::from_utf8_lossy(needle));
        }
        let hex = runs.iter().filter(|r| {
            r.decoded
                .iter()
                .any(|d| d.as_slice() == b"Thorsko".as_slice())
        });
        assert!(hex.count() >= 2, "base64 and both hex alignments: {runs:?}");
        // Runs cover their padding.
        let first = &runs
            .iter()
            .find(|r| text[r.start..r.end].ends_with("=="))
            .unwrap();
        assert_eq!(&text[first.start..first.end], b64(b"Thorsko"));
    }
}
