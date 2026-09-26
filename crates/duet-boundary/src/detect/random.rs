// SPDX-License-Identifier: GPL-3.0-or-later
//! Whether a long token the entropy detector found is random.
//!
//! Entropy alone does not tell a key from a name: once it is long enough, a
//! path, a hashed bundle name or a long identifier has as many bits per
//! character as a key (`/Volumes/EXT_DISK/duet_v2/app/data` scores 4.4, above
//! the threshold). So a token is read by its parts, split at `/`, `-` and
//! `_`:
//!
//! - a **name** reads as words: a lower-case, ALL-CAPS or Capitalized word of
//!   three letters or more, perhaps numbered (`python3`), or camel case with
//!   acronyms (`asyncRunEntryPointWithESMLoader`, `OAuth2Authorization`);
//! - an **id** is a number or hex of one letter case: UUIDs, git object ids,
//!   content hashes, dates and timestamps;
//! - a **hash** is a bundler's content hash in a file name: at the end of the
//!   token, 6–16 characters, followed by a build-artifact extension
//!   (`index-DVuHW4gw.js`, `chunk-ABCD2345.css`, `_app-0a1b2c3d.js.map`); a
//!   Next.js build id (`.next/static/<21 characters>/`) is one too;
//! - a part of one letter case, or Capitalized, with two numbers at most
//!   (`v2`, `db`, `25T12`, `cp39`) says too little either way;
//! - a **long** part (24 characters or more) is judged on its own.
//!
//! A token whose every part has one of these shapes and that holds at least
//! two names is structured; any other token is judged whole, as before, and
//! so is anything with `+` or `=` padding (base64). A structured token with
//! no random long part is not a secret. One with a random long part is
//! withheld whole, unless it is in an absolute path or a URL: there the part
//! is a path segment of its own and is withheld alone (a key in a URL path
//! is found, the path around it is not). A random token's parts are
//! mixed-case fragments that fit none of the shapes; a test measures how
//! rarely one passes.

use super::entropy;
use regex::Regex;
use std::ops::RangeInclusive;
use std::sync::LazyLock;

/// Fewest characters in a part judged on its own (the candidate's minimum).
const LONG: usize = 24;
/// Bytes looked back for the start of the word a token is in.
const WORD_REACH: usize = 2048;
/// Names a token needs to be read by its parts.
const ANCHORS: usize = 2;
/// Length of a bundler's content hash (vite and rollup 8, next 16; webpack's
/// longer ones are hex, which is an id anyway).
const HASH: RangeInclusive<usize> = 6..=16;

/// A Next.js build id: the directory under `.next/static/` (and
/// `/_next/static/` in URLs) named by 21 random characters.
static NEXT_BUILD_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|/)_?next/static/([A-Za-z0-9_-]{21})(?:/|$)").expect("static regex")
});

/// A build-artifact extension right after a token, inner suffixes allowed
/// (`.js`, `.chunk.js`, `.js.map`, `.module.css`).
static ARTIFACT_EXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:\.[A-Za-z0-9-]{1,16}){0,3}?\.(?:m?js|cjs|jsx|tsx?|css|map|json|html?|svg|png|jpe?g|gif|webp|avif|ico|woff2?|ttf|otf|eot|wasm)\b",
    )
    .expect("static regex")
});

/// Subresource-integrity digests: the hash of public content
/// (`sha512-<base64>` in lockfiles and `<script integrity>`). A `go.sum`
/// digest (`h1:<base64>`) is known by what precedes it.
const DIGEST_PREFIXES: &[&str] = &["sha1-", "sha256-", "sha384-", "sha512-"];

/// The spans of the candidate `text[start..end]` that are random: the whole
/// candidate, some of its long parts, or none.
pub(super) fn random_spans(text: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let token = &text[start..end];
    if text[..start].ends_with("h1:") {
        return Vec::new();
    }
    // An extension is short: the regex never needs the rest of the text.
    let mut to = (end + 40).min(text.len());
    while !text.is_char_boundary(to) {
        to -= 1;
    }
    let Some(parts) = structured(token, &text[end..to]) else {
        return if looks_random(token) {
            vec![(start, end)]
        } else {
            Vec::new()
        };
    };
    let random: Vec<(usize, usize)> = parts
        .into_iter()
        .filter(|&(s, e)| e - s >= LONG && looks_random(&token[s..e]))
        .map(|(s, e)| (start + s, start + e))
        .collect();
    // In a rooted path (an absolute path, a URL) a random part is a segment
    // of its own; elsewhere the token as a whole is withheld.
    if random.is_empty() || rooted(text, start) {
        random
    } else {
        vec![(start, end)]
    }
}

/// Whether the token at `start` is in an absolute path or a URL: the word
/// it is in (up to a space, quote or bracket) starts with `/`, `~/` or
/// `file:`, or holds `://`. A word that starts more than [`WORD_REACH`]
/// bytes back is not taken for one.
fn rooted(text: &str, start: usize) -> bool {
    let mut low = start.saturating_sub(WORD_REACH);
    while !text.is_char_boundary(low) {
        low += 1;
    }
    let from = match text[low..start]
        .char_indices()
        .rev()
        .find(|&(_, c)| c.is_whitespace() || "'\"`()<>[]{},;=".contains(c))
    {
        Some((i, c)) => low + i + c.len_utf8(),
        None if low > 0 => return false,
        None => 0,
    };
    let (prefix, word) = (&text[from..start], &text[from..]);
    ["/", "~/", "file:"].iter().any(|p| word.starts_with(p)) || prefix.contains("://")
}

/// The parts of `token` when it reads as structured (see the module notes);
/// `after` is the text that follows it.
fn structured(token: &str, after: &str) -> Option<Vec<(usize, usize)>> {
    // `+` and padding mark base64, whatever its parts look like.
    if token.contains('+') || token.ends_with('=') {
        return None;
    }
    let parts: Vec<(usize, usize)> = split(token);
    if parts.len() < 2 {
        return None;
    }
    let hash_from = hash_start(token, &parts, after);
    let build_id = NEXT_BUILD_ID
        .captures(token)
        .and_then(|c| c.get(1))
        .map(|m| m.range());
    let mut names = 0;
    for (i, &(s, e)) in parts.iter().enumerate() {
        let part = &token[s..e];
        if name(part) {
            names += 1;
        } else if !(e - s >= LONG
            || one_case(part)
            || id(part)
            || hash_from.is_some_and(|h| i >= h)
            || build_id
                .as_ref()
                .is_some_and(|r| r.start <= s && e <= r.end))
        {
            return None;
        }
    }
    (names >= ANCHORS).then_some(parts)
}

/// The non-empty runs of `token` between `/`, `-` and `_`.
fn split(token: &str) -> Vec<(usize, usize)> {
    let mut parts = Vec::new();
    let mut from = 0;
    for (i, b) in token.bytes().enumerate() {
        if matches!(b, b'/' | b'-' | b'_') {
            if i > from {
                parts.push((from, i));
            }
            from = i + 1;
        }
    }
    if token.len() > from {
        parts.push((from, token.len()));
    }
    parts
}

/// The index of the first part of a content hash ending the token, when a
/// build-artifact extension follows: the last parts back to a name or a
/// `/`, joined by `-` or `_` (base64url hashes hold them), [`HASH`]
/// characters in all.
fn hash_start(token: &str, parts: &[(usize, usize)], after: &str) -> Option<usize> {
    if !ARTIFACT_EXT.is_match(after) {
        return None;
    }
    let end = parts.last()?.1;
    let mut first = None;
    for i in (0..parts.len()).rev() {
        let (s, e) = parts[i];
        if end - s > *HASH.end() || token[e..end].contains('/') || name(&token[s..e]) {
            break;
        }
        first = Some(i);
    }
    first.filter(|&i| HASH.contains(&(end - parts[i].0)))
}

/// A number, or hex of one letter case: an id, digest, date or version.
fn id(part: &str) -> bool {
    part.bytes().all(|b| b.is_ascii_digit())
        || part.bytes().all(|b| b.is_ascii_hexdigit())
            && !(part.bytes().any(|b| b.is_ascii_lowercase())
                && part.bytes().any(|b| b.is_ascii_uppercase()))
}

/// Letters of one case, or Capitalized, with at most two numbers among them
/// (`v2`, `25T12`, `es2020`, `v1beta1`).
fn one_case(part: &str) -> bool {
    let b = part.as_bytes();
    let lower = b.iter().any(u8::is_ascii_lowercase);
    let upper = b.iter().filter(|c| c.is_ascii_uppercase()).count();
    let numbers = b
        .iter()
        .enumerate()
        .filter(|&(i, c)| c.is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_digit()))
        .count();
    (!lower || upper == 0 || upper == 1 && b[0].is_ascii_uppercase()) && numbers <= 2
}

/// A part that reads as a word or words: three letters or more in lower
/// case, ALL CAPS or Capitalized, perhaps numbered (`python3`,
/// `universal2`, `sha256`), or camel case ([`word_shaped`]).
fn name(part: &str) -> bool {
    let b = part.as_bytes();
    let letters = b.iter().take_while(|c| c.is_ascii_alphabetic()).count();
    let (word, number) = b.split_at(letters);
    let upper = word.iter().filter(|c| c.is_ascii_uppercase()).count();
    (letters >= 3
        && number.len() <= 4
        && number.iter().all(u8::is_ascii_digit)
        && (upper == 0 || upper == letters || upper == 1 && word[0].is_ascii_uppercase()))
        || word_shaped(part)
}

/// Words of one or two letters that identifiers use (`isOnline`, `toString`,
/// `ThisIsAVeryLongName`, `OAuth2`, `getXOffset`, `IOError`).
const SHORT_WORDS: &[&str] = &[
    "a", "i", "o", "x", "y", "z", "an", "as", "at", "be", "by", "db", "do", "ex", "fs", "go", "id",
    "if", "in", "io", "ip", "is", "it", "js", "me", "my", "no", "of", "ok", "on", "or", "os", "re",
    "to", "ts", "ui", "up", "we",
];

/// Whether a mixed-case alphanumeric run reads as camel-case words. Split at
/// case changes (`asyncRunEntryPointWithESMLoader` is async, Run, Entry,
/// Point, With, ESM, Loader) and numbers, it needs two words or more of
/// three letters on average, a word of one or two letters only if
/// identifiers use it, at most one single letter, two acronyms of five
/// letters, two numbers of four digits, and two capitals in five letters at
/// most. Random letters change case every other character or so, which
/// makes words of one or two letters that are none of those.
fn word_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    if !b.iter().all(u8::is_ascii_alphanumeric)
        || !b.iter().any(u8::is_ascii_lowercase)
        || !b.iter().any(u8::is_ascii_uppercase)
    {
        return false;
    }
    let (mut words, mut letters, mut upper, mut singles, mut acronyms, mut numbers) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut i = 0;
    while i < b.len() {
        let start = i;
        if b[i].is_ascii_digit() {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            numbers += 1;
            if i - start > 4 {
                return false;
            }
            continue;
        }
        let mut acronym = false;
        if b[i].is_ascii_uppercase() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_uppercase() {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_lowercase() && j - i > 1 {
                // An acronym, less the capital that starts the next word.
                j -= 1;
            }
            if j - i > 1 {
                acronym = true;
            } else {
                while j < b.len() && b[j].is_ascii_lowercase() {
                    j += 1;
                }
            }
            i = j;
        } else {
            while i < b.len() && b[i].is_ascii_lowercase() {
                i += 1;
            }
        }
        let word = &s[start..i];
        let n = word.len();
        words += 1;
        letters += n;
        upper += word.bytes().filter(u8::is_ascii_uppercase).count();
        singles += usize::from(n == 1);
        acronyms += usize::from(acronym);
        if n <= 2 && !SHORT_WORDS.contains(&word.to_ascii_lowercase().as_str()) || n > 5 && acronym
        {
            return false;
        }
    }
    words >= 2
        && letters >= 3 * words
        && singles <= 1
        && acronyms <= 2
        && numbers <= 2
        && 5 * upper <= 2 * letters
}

/// Whether a token (or a long part) looks random: high entropy, two or more
/// character classes, and none of the shapes of public values (a name, a
/// git object id, an integrity digest).
fn looks_random(s: &str) -> bool {
    let classes = [
        s.bytes().any(|b| b.is_ascii_lowercase()),
        s.bytes().any(|b| b.is_ascii_uppercase()),
        s.bytes().any(|b| b.is_ascii_digit()),
    ];
    entropy(s) >= 4.0
        && classes.iter().filter(|c| **c).count() >= 2
        && !(s.len() == 40
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
        && !DIGEST_PREFIXES.iter().any(|p| s.starts_with(p))
        && !word_shaped(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A seeded xorshift generator: the measurement is the same on every run.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }

        fn token(&mut self, alphabet: &[u8], len: usize) -> String {
            (0..len)
                .map(|_| alphabet[self.below(alphabet.len())] as char)
                .collect()
        }
    }

    const BASE62: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    /// Random tokens a key could be, 24 to 64 characters of each alphabet:
    /// of those with the entropy and character classes the rule looks at,
    /// how many are not withheld whole (read as names or as structured).
    /// Measured at 200,000 per alphabet (2026-09-26): base62 2, base64 1,
    /// base64url 7, base36 0, letters 45. The rule before this one missed
    /// 12, 2, 2,041, 0 and 1,122 (camel-case letters, and any token with
    /// `__`).
    #[test]
    fn random_tokens_are_withheld_whole() {
        const SAMPLES: usize = 200_000;
        const BOUND: usize = 60;
        let alphabets = [
            ("base62", BASE62.to_owned()),
            ("base64", format!("{BASE62}+/")),
            ("base64url", format!("{BASE62}-_")),
            ("base36", "abcdefghijklmnopqrstuvwxyz0123456789".to_owned()),
            ("letters", BASE62[..52].to_owned()),
        ];
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for (name, alphabet) in &alphabets {
            let (mut judged, mut missed) = (0, Vec::new());
            for _ in 0..SAMPLES {
                let len = 24 + rng.below(41);
                let t = rng.token(alphabet.as_bytes(), len);
                let classes = [
                    t.bytes().any(|b| b.is_ascii_lowercase()),
                    t.bytes().any(|b| b.is_ascii_uppercase()),
                    t.bytes().any(|b| b.is_ascii_digit()),
                ];
                // What the rule looks at at all: high entropy, two classes.
                if entropy(&t) < 4.0 || classes.iter().filter(|c| **c).count() < 2 {
                    continue;
                }
                judged += 1;
                let covered: usize = random_spans(&t, 0, t.len())
                    .iter()
                    .map(|(s, e)| e - s)
                    .sum();
                if covered < t.len() {
                    missed.push(t);
                }
            }
            println!(
                "{name}: {judged} random tokens judged, {} not withheld whole {:?}",
                missed.len(),
                &missed[..missed.len().min(3)]
            );
            assert!(judged > SAMPLES * 3 / 4, "{name}: {judged}");
            assert!(missed.len() <= BOUND, "{name}: {missed:?}");
        }
    }

    fn spans(text: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(i) =
            text[at..].find(|c: char| c.is_ascii_alphanumeric() || "+/_-".contains(c))
        {
            let start = at + i;
            let end = text[start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || "+/_-=".contains(c)))
                .map_or(text.len(), |e| start + e);
            if end - start >= LONG {
                out.extend(
                    random_spans(text, start, end)
                        .into_iter()
                        .map(|(s, e)| &text[s..e]),
                );
            }
            at = end;
        }
        out
    }

    #[test]
    fn identifiers_read_as_words() {
        for s in [
            "asyncRunEntryPointWithESMLoader",
            "OAuth2AuthorizationCodeGrantHandler",
            "ThisIsAVeryLongTypeNameUsedInTests",
            "AbstractSingletonProxyFactoryBean",
            "DirectMethodHandleAccessor",
            "handleHTTPResponse",
            "XMLHttpRequestEventTarget",
            "processTicksAndRejections",
            "Utf8DecoderForBase64Streams",
        ] {
            assert!(word_shaped(s), "{s}");
        }
        for s in [
            "QmXvTzLpRwNsKdHjFgBcYtEa",
            "Q8f2LmZ0x9R4tWvB7nC1pK6sD3hJ5gYa",
            "DVuHW4gw",
            "wJalrXUtnFEMI",
            "bPxRfiCYEXAMPLEKEY",
            "lowercaseonly",
        ] {
            assert!(!word_shaped(s), "{s}");
        }
    }

    #[test]
    fn paths_are_judged_by_their_parts() {
        // Nothing random in any part.
        for text in [
            "mkdir '/Volumes/EXT_DISK/duet_v2/scratch/fullapp/app/data'",
            "file:///Users/dev/projects/task-manager/dist/server/app.js:179:22",
            "/home/runner/work/task-manager/task-manager/node_modules/vitest/dist/index.js",
            "GET /api/v1/orders/286a9205-6c69-4f40-ba3b-039fbdd92f38/items HTTP/1.1",
            "/rustc/90b35a6239c3d8bdabc530a6a0816f7ff89a0aaf/library/std/src/panicking.rs:665:5",
            "projects/sample-project/locations/europe-west1/services/api-backend",
            ".duet/runs/20260926-002521-957296/transcript.jsonl",
        ] {
            assert_eq!(spans(text), Vec::<&str>::new(), "{text}");
        }
        // A random segment of a rooted path is withheld alone; a relative
        // path or a bare token holding one is withheld whole.
        let key = "Q8f2LmZ0x9R4tWvB7nC1pK6sD3hJ5gYa";
        assert_eq!(
            spans(&format!("https://api.internal.test/v1/keys/{key}/rotate")),
            vec![key]
        );
        assert_eq!(spans(&format!("/srv/share/{key}/report")), vec![key]);
        let rel = format!("uploads/{key}/report");
        assert_eq!(spans(&rel), vec![rel.as_str()]);
        // Base64 is never read by parts, whatever its parts look like.
        let b64 = "SbwgEzi/l1oev15spEbJzpHxKPbbXeB5zBDDPAt/qfh+";
        assert_eq!(spans(b64), vec![b64]);
    }

    #[test]
    fn content_hashes_in_file_names_are_not_random() {
        for text in [
            "dist/assets/index-DVuHW4gw.js",
            "dist/assets/vendor-react-dom-Bx8f9aQz.js.map",
            "build/static/js/main-app-Wq2iJEZ9fY.chunk.js",
            ".output/public/_nuxt/entry-DVuHW4gw.css",
            "dist/assets/index-B_x3Q-4QaZ9k.js",
        ] {
            assert_eq!(spans(text), Vec::<&str>::new(), "{text}");
        }
        // The same shape without a build-artifact extension is judged as before.
        let text = "reports/quarterly-summary/export-Xk29fLq8Zm3R";
        assert_eq!(spans(text), vec![text]);
    }

    #[test]
    fn module_digests_are_not_random() {
        for text in [
            "golang.org/x/net v0.25.0 h1:d/OCCoBEUq33pjydKrGQhw7IlUPI2Oylr+8qLr5gYCQ=",
            "\"integrity\": \"sha1-2BcYtEaQmXvTzLpRwNsKdHjFgBc=\"",
        ] {
            assert_eq!(spans(text), Vec::<&str>::new(), "{text}");
        }
    }
}
