// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding planted marker values ("canaries") in outbound bytes.
//!
//! Privacy tests plant synthetic values (a fake card number, a fake API key)
//! in a workspace and assert that none reaches the frontier. Looking only for
//! the exact value misses a marker that left in another spelling, so
//! [`Canaries::find`] also reports these forms (see [`Form`]):
//!
//! - the value in another letter case;
//! - escaped for JSON (once, twice, with `\uXXXX` or `\/`), URL-encoded
//!   (either hex case, `+` for space, every byte) and HTML-escaped (named or
//!   numeric entities);
//! - base64 (standard or URL-safe, padded or not) and hex (either case) of the
//!   value or of any substring of 8+ bytes, at any alignment inside a longer
//!   encoded blob: every encoded run is decoded at each starting offset and the
//!   decoded bytes are searched;
//! - the value reversed, and its characters split by spaces, dashes, dots,
//!   underscores or newlines;
//! - for values that are mostly digits: any run of 4+ of its digits, as digits
//!   or spelled as number words ("four five three nine").
//!
//! Deterministic and linear in the input for a fixed set of canaries: two
//! Aho-Corasick passes, one scan per split pattern, and window lookups over
//! decoded runs and digit runs.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

/// How a canary appeared. Declared from most to least direct; a finding
/// inside a more direct finding of the same canary is not reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Form {
    /// Byte for byte.
    Exact,
    /// Same letters in another case.
    CaseVariant,
    /// JSON string escaping (once or twice, `\uXXXX` for non-ASCII, `\/`).
    JsonEscaped,
    /// Percent-encoding (upper or lower hex, `+` for space, or every byte).
    UrlEncoded,
    /// HTML escaping (named entities, or every character as `&#N;`/`&#xH;`).
    HtmlEscaped,
    /// The value's characters with separators between each.
    SeparatorSplit,
    /// The characters in reverse order.
    Reversed,
    /// Base64 (standard or URL-safe) of the value or a substring of it.
    Base64,
    /// Hex (either case) of the value or a substring of it.
    Hex,
    /// Digits of a mostly-digit value spelled as English number words.
    SpelledDigits,
    /// A run of consecutive digits of a mostly-digit value.
    DigitFragment,
}

/// One appearance of a canary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Index of the canary in the values given to [`Canaries::new`].
    pub canary_index: usize,
    pub form: Form,
    /// Byte offset in the searched input.
    pub offset: usize,
    /// Byte length of the appearance in the searched input.
    pub len: usize,
}

/// Tuning for [`Canaries::with_options`].
#[derive(Debug, Clone)]
pub struct Options {
    /// Shortest run of a canary's digits reported as a [`Form::DigitFragment`]
    /// or [`Form::SpelledDigits`] (default 4).
    pub digit_min: usize,
    /// Digit strings that are common anyway (years, protocol constants such
    /// as a `max_tokens` value): a digit fragment found inside one of them is
    /// not reported.
    pub ignore_fragments: Vec<String>,
    /// Shortest substring of a canary reported inside decoded base64 or hex
    /// (default 8; a shorter canary must appear whole).
    pub encoded_min: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            digit_min: 4,
            ignore_fragments: Vec::new(),
            encoded_min: 8,
        }
    }
}

/// Characters that may separate the characters of a split value.
const SPLIT_SEPARATORS: &[u8] = b" -._\n\r";
/// Most separator characters between two characters of a split value.
const SPLIT_GAP_MAX: usize = 3;
const DIGIT_WORDS: [&str; 10] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

/// A set of canaries, compiled for searching.
pub struct Canaries {
    values: Vec<String>,
    ignore: Vec<String>,
    /// Exact, escaped, encoded and reversed spellings.
    literal: Option<AhoCorasick>,
    literal_ids: Vec<(usize, Form)>,
    /// Case-insensitive spellings; a match that differs from the value is a
    /// case variant.
    folded: Option<AhoCorasick>,
    folded_ids: Vec<usize>,
    /// Per canary: its characters without separators, when 4+ remain.
    split_cores: Vec<Option<Vec<Vec<u8>>>>,
    /// Canary bytes, for decoded base64 and hex.
    bytes: FragmentIndex,
    /// Digits of mostly-digit canaries.
    digits: FragmentIndex,
}

impl Canaries {
    /// Canaries with default [`Options`].
    pub fn new<I, S>(values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::with_options(values, Options::default())
    }

    pub fn with_options<I, S>(values: I, options: Options) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let values: Vec<String> = values.into_iter().map(Into::into).collect();
        let mut literal_pats: Vec<Vec<u8>> = Vec::new();
        let mut literal_ids = Vec::new();
        let mut folded_pats: Vec<Vec<u8>> = Vec::new();
        let mut folded_ids = Vec::new();
        for (i, v) in values.iter().enumerate() {
            if v.is_empty() {
                continue;
            }
            let mut seen: HashSet<String> = HashSet::new();
            let mut add = |s: String, form: Form| {
                if !s.is_empty() && seen.insert(s.clone()) {
                    literal_pats.push(s.into_bytes());
                    literal_ids.push((i, form));
                }
            };
            add(v.clone(), Form::Exact);
            json_forms(v)
                .into_iter()
                .for_each(|s| add(s, Form::JsonEscaped));
            url_forms(v)
                .into_iter()
                .for_each(|s| add(s, Form::UrlEncoded));
            html_forms(v)
                .into_iter()
                .for_each(|s| add(s, Form::HtmlEscaped));
            if v.chars().count() >= 4 {
                add(v.chars().rev().collect(), Form::Reversed);
            }
            let (lower, upper) = (v.to_lowercase(), v.to_uppercase());
            if lower != upper {
                let mut forms = vec![v.clone(), lower, upper];
                forms.dedup();
                for f in forms {
                    folded_pats.push(f.into_bytes());
                    folded_ids.push(i);
                }
            }
        }
        let literal = (!literal_pats.is_empty())
            .then(|| AhoCorasick::new(&literal_pats).expect("canary patterns"));
        let folded = (!folded_pats.is_empty()).then(|| {
            AhoCorasickBuilder::new()
                .ascii_case_insensitive(true)
                .build(&folded_pats)
                .expect("canary patterns")
        });
        let split_cores = values
            .iter()
            .map(|v| {
                let core: Vec<Vec<u8>> = v
                    .chars()
                    .filter(|c| !(c.is_ascii() && SPLIT_SEPARATORS.contains(&(*c as u8))))
                    .map(|c| c.to_string().into_bytes())
                    .collect();
                (core.len() >= 4).then_some(core)
            })
            .collect();
        let bytes = FragmentIndex::new(
            values.iter().map(|v| v.as_bytes().to_vec()).collect(),
            options.encoded_min.max(1),
        );
        let digit_min = options.digit_min.max(1);
        let digits = FragmentIndex::new(
            values
                .iter()
                .map(|v| {
                    let d: Vec<u8> = v.bytes().filter(u8::is_ascii_digit).collect();
                    let visible = v.chars().filter(|c| !c.is_whitespace()).count();
                    if d.len() >= digit_min && d.len() * 2 >= visible {
                        d
                    } else {
                        Vec::new()
                    }
                })
                .collect(),
            digit_min,
        );
        Self {
            values,
            ignore: options.ignore_fragments,
            literal,
            literal_ids,
            folded,
            folded_ids,
            split_cores,
            bytes,
            digits,
        }
    }

    /// The canary values, in the order given.
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// Every appearance of a canary in `input`, ordered by offset. An
    /// appearance inside a more direct appearance of the same canary (a digit
    /// run inside the exact value, say) is not reported separately.
    pub fn find(&self, input: impl AsRef<[u8]>) -> Vec<Finding> {
        let hay = input.as_ref();
        let mut out = Vec::new();
        let mut push = |canary_index, form, offset, len| {
            out.push(Finding {
                canary_index,
                form,
                offset,
                len,
            })
        };
        if let Some(ac) = &self.literal {
            for m in ac.find_overlapping_iter(hay) {
                let (c, form) = self.literal_ids[m.pattern().as_usize()];
                push(c, form, m.start(), m.len());
            }
        }
        if let Some(ac) = &self.folded {
            for m in ac.find_overlapping_iter(hay) {
                let c = self.folded_ids[m.pattern().as_usize()];
                if &hay[m.range()] != self.values[c].as_bytes() {
                    push(c, Form::CaseVariant, m.start(), m.len());
                }
            }
        }
        for (c, core) in self.split_cores.iter().enumerate() {
            if let Some(core) = core {
                for (offset, len) in find_split(hay, core) {
                    push(c, Form::SeparatorSplit, offset, len);
                }
            }
        }
        for (start, run) in runs(hay, |b| base64_value(b).is_some(), 6) {
            for shift in 0..4.min(run.len()) {
                let decoded = decode_base64(&run[shift..]);
                let base = start + shift;
                for (c, j, m) in self.bytes.matches(&decoded) {
                    // Byte j is in characters 4(j/3)+j%3 and the one after.
                    let first = base + (j / 3) * 4 + j % 3;
                    let last = j + m - 1;
                    let end = (base + (last / 3) * 4 + last % 3 + 2).min(start + run.len());
                    push(c, Form::Base64, first, end - first);
                }
            }
        }
        for (start, run) in runs(hay, |b| b.is_ascii_hexdigit(), 8) {
            for shift in 0..2 {
                let decoded: Vec<u8> = run[shift..]
                    .chunks_exact(2)
                    .map(|p| hex_value(p[0]) << 4 | hex_value(p[1]))
                    .collect();
                for (c, j, m) in self.bytes.matches(&decoded) {
                    push(c, Form::Hex, start + shift + 2 * j, 2 * m);
                }
            }
        }
        if !self.digits.is_empty() {
            for (start, run) in runs(hay, |b| b.is_ascii_digit(), 1) {
                for (c, j, m) in self.digits.matches(run) {
                    if !self.ignored(&run[j..j + m]) {
                        push(c, Form::DigitFragment, start + j, m);
                    }
                }
            }
            for (digits, spans) in spelled_digit_runs(hay) {
                for (c, j, m) in self.digits.matches(&digits) {
                    if !self.ignored(&digits[j..j + m]) {
                        let offset = spans[j].0;
                        push(c, Form::SpelledDigits, offset, spans[j + m - 1].1 - offset);
                    }
                }
            }
        }
        dedup(out)
    }

    fn ignored(&self, digits: &[u8]) -> bool {
        std::str::from_utf8(digits).is_ok_and(|d| self.ignore.iter().any(|i| i.contains(d)))
    }
}

/// Drops findings inside a more direct finding of the same canary, then
/// orders by offset.
fn dedup(mut all: Vec<Finding>) -> Vec<Finding> {
    all.sort_by_key(|f| (f.form, Reverse(f.len), f.offset, f.canary_index));
    let mut kept: Vec<Finding> = Vec::new();
    for f in all {
        let covered = kept.iter().any(|k| {
            k.canary_index == f.canary_index
                && k.offset <= f.offset
                && f.offset + f.len <= k.offset + k.len
        });
        if !covered {
            kept.push(f);
        }
    }
    kept.sort_by_key(|f| (f.offset, f.canary_index, f.form, f.len));
    kept
}

/// Windows of each canary's sequence, to find shared runs in other bytes.
struct FragmentIndex {
    seqs: Vec<Vec<u8>>,
    windows: HashMap<Vec<u8>, Vec<(usize, usize)>>,
    /// Distinct window sizes in use (a sequence shorter than the minimum is
    /// indexed whole).
    sizes: Vec<usize>,
}

impl FragmentIndex {
    fn new(seqs: Vec<Vec<u8>>, min: usize) -> Self {
        let mut windows: HashMap<Vec<u8>, Vec<(usize, usize)>> = HashMap::new();
        let mut sizes = Vec::new();
        for (c, s) in seqs.iter().enumerate() {
            let w = min.min(s.len());
            if w == 0 {
                continue;
            }
            if !sizes.contains(&w) {
                sizes.push(w);
            }
            for p in 0..=s.len() - w {
                windows
                    .entry(s[p..p + w].to_vec())
                    .or_default()
                    .push((c, p));
            }
        }
        sizes.sort_unstable();
        Self {
            seqs,
            windows,
            sizes,
        }
    }

    fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Maximal runs shared by `text` and an indexed sequence, each at least a
    /// window long: (canary, start in `text`, length).
    fn matches(&self, text: &[u8]) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::new();
        for &w in &self.sizes {
            if text.len() < w {
                continue;
            }
            for i in 0..=text.len() - w {
                let Some(hits) = self.windows.get(&text[i..i + w]) else {
                    continue;
                };
                for &(c, p) in hits {
                    let s = &self.seqs[c];
                    // Reported once, from where the shared run starts.
                    if i > 0 && p > 0 && text[i - 1] == s[p - 1] {
                        continue;
                    }
                    let mut m = w;
                    while i + m < text.len() && p + m < s.len() && text[i + m] == s[p + m] {
                        m += 1;
                    }
                    out.push((c, i, m));
                }
            }
        }
        out
    }
}

/// Maximal runs of bytes accepted by `accept`, at least `min` long.
fn runs(hay: &[u8], accept: impl Fn(u8) -> bool, min: usize) -> Vec<(usize, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < hay.len() {
        if !accept(hay[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < hay.len() && accept(hay[i]) {
            i += 1;
        }
        if i - start >= min {
            out.push((start, &hay[start..i]));
        }
    }
    out
}

/// Where `core` appears with 1..=3 separators between every two characters:
/// (offset, length).
fn find_split(hay: &[u8], core: &[Vec<u8>]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let first = &core[0];
    let mut i = 0;
    while i < hay.len() {
        if !hay[i..].starts_with(first) {
            i += 1;
            continue;
        }
        let mut pos = i + first.len();
        let mut ok = true;
        for ch in &core[1..] {
            let gap = hay[pos..]
                .iter()
                .take_while(|b| SPLIT_SEPARATORS.contains(b))
                .count();
            if !(1..=SPLIT_GAP_MAX).contains(&gap) || !hay[pos + gap..].starts_with(ch) {
                ok = false;
                break;
            }
            pos += gap + ch.len();
        }
        if ok {
            out.push((i, pos - i));
            i = pos;
        } else {
            i += 1;
        }
    }
    out
}

/// A sequence of spelled digits: the digits, and the byte span of each word.
type SpelledRun = (Vec<u8>, Vec<(usize, usize)>);

/// Sequences of spelled digits ("four five", "four-five"; any case).
fn spelled_digit_runs(hay: &[u8]) -> Vec<SpelledRun> {
    let word_at = |i: usize| -> Option<(u8, usize)> {
        if i > 0 && hay[i - 1].is_ascii_alphabetic() {
            return None;
        }
        DIGIT_WORDS.iter().enumerate().find_map(|(d, w)| {
            let end = i + w.len();
            let fits = hay.get(i..end)?.eq_ignore_ascii_case(w.as_bytes());
            let bounded = hay.get(end).is_none_or(|b| !b.is_ascii_alphabetic());
            (fits && bounded).then_some((b'0' + d as u8, end))
        })
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < hay.len() {
        let Some((d, end)) = word_at(i) else {
            i += 1;
            continue;
        };
        let mut digits = vec![d];
        let mut spans = vec![(i, end)];
        let mut pos = end;
        loop {
            let gap = hay[pos..]
                .iter()
                .take_while(|b| **b == b' ' || **b == b'-')
                .count();
            if gap == 0 {
                break;
            }
            match word_at(pos + gap) {
                Some((d, end)) => {
                    digits.push(d);
                    spans.push((pos + gap, end));
                    pos = end;
                }
                None => break,
            }
        }
        out.push((digits, spans));
        i = pos;
    }
    out
}

fn base64_value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// Decodes base64 characters (either alphabet, no padding); a trailing
/// partial group yields the bytes it completes.
fn decode_base64(chars: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chars.len() * 3 / 4);
    for group in chars.chunks(4) {
        let v: Vec<u32> = group
            .iter()
            .map(|b| u32::from(base64_value(*b).unwrap_or(0)))
            .collect();
        let n = v.iter().fold(0u32, |acc, x| acc << 6 | x) << (6 * (4 - v.len()));
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..v.len().saturating_sub(1)]);
    }
    out
}

fn hex_value(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => b - b'A' + 10,
    }
}

/// JSON-escaped spellings of `v`: once, with non-ASCII as `\uXXXX`, with `/`
/// as `\/`, and twice (JSON inside a JSON string).
fn json_forms(v: &str) -> Vec<String> {
    let quoted = |s: &str| {
        let q = serde_json::to_string(s).unwrap_or_default();
        q[1..q.len() - 1].to_owned()
    };
    let once = quoted(v);
    let mut ascii = String::new();
    for c in once.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                ascii.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    let slash = once.replace('/', "\\/");
    let twice = quoted(&once);
    vec![once, ascii, slash, twice]
}

/// Percent-encoded spellings of `v`: upper and lower hex, `+` for space, and
/// every byte encoded.
fn url_forms(v: &str) -> Vec<String> {
    let encode = |lower: bool, plus: bool, all: bool| {
        let mut s = String::new();
        for b in v.bytes() {
            if !all && (b.is_ascii_alphanumeric() || b"-._~".contains(&b)) {
                s.push(b as char);
            } else if plus && b == b' ' {
                s.push('+');
            } else if lower {
                s.push_str(&format!("%{b:02x}"));
            } else {
                s.push_str(&format!("%{b:02X}"));
            }
        }
        s
    };
    vec![
        encode(false, false, false),
        encode(true, false, false),
        encode(false, true, false),
        encode(false, false, true),
        encode(true, false, true),
    ]
}

/// HTML-escaped spellings of `v`: named entities for the special characters
/// (with the usual spellings of `'`), and every character as a decimal or hex
/// character reference.
fn html_forms(v: &str) -> Vec<String> {
    let named = |apos: &str| {
        let mut s = String::new();
        for c in v.chars() {
            match c {
                '&' => s.push_str("&amp;"),
                '<' => s.push_str("&lt;"),
                '>' => s.push_str("&gt;"),
                '"' => s.push_str("&quot;"),
                '\'' => s.push_str(apos),
                c => s.push(c),
            }
        }
        s
    };
    vec![
        named("&#39;"),
        named("&#x27;"),
        named("&apos;"),
        v.chars().map(|c| format!("&#{};", c as u32)).collect(),
        v.chars().map(|c| format!("&#x{:x};", c as u32)).collect(),
        v.chars().map(|c| format!("&#x{:X};", c as u32)).collect(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = "4539 1488 0343 6467";
    const KEY: &str = "sk_live_Zq81vWx03LmNpRt5";

    fn forms(c: &Canaries, input: &str) -> Vec<Form> {
        c.find(input).into_iter().map(|f| f.form).collect()
    }

    fn b64(bytes: &[u8], url_safe: bool) -> String {
        let table: &[u8; 64] = if url_safe {
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
        } else {
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        };
        let mut s = String::new();
        for g in bytes.chunks(3) {
            let n = (u32::from(g[0]) << 16)
                | (u32::from(*g.get(1).unwrap_or(&0)) << 8)
                | u32::from(*g.get(2).unwrap_or(&0));
            for k in 0..=g.len() {
                s.push(table[(n >> (18 - 6 * k)) as usize & 63] as char);
            }
        }
        while !s.len().is_multiple_of(4) {
            s.push('=');
        }
        s
    }

    #[test]
    fn exact_with_offset_and_index() {
        let c = Canaries::new([CARD, KEY]);
        let f = c.find(format!("key={KEY} card {CARD}"));
        assert_eq!(
            f,
            vec![
                Finding {
                    canary_index: 1,
                    form: Form::Exact,
                    offset: 4,
                    len: KEY.len()
                },
                Finding {
                    canary_index: 0,
                    form: Form::Exact,
                    offset: 10 + KEY.len(),
                    len: CARD.len()
                },
            ]
        );
    }

    #[test]
    fn unrelated_text_and_other_numbers_find_nothing() {
        let c = Canaries::new([CARD, KEY, "Vakdril Thorsko"]);
        for s in [
            "",
            "Fix the balance rounding in the monthly report for each customer.",
            "card 4716 2290 5512 8830, key sk_live_AAAAAAAAAAAAAAAAAAAAAAAA",
            "{\"max_tokens\": 32000, \"model\": \"m\", \"stream\": true}",
            "one two three",
            "someone gave nine to eighty",
            "SGVsbG8gd29ybGQsIHRoaXMgaXMgbm90IGEgc2VjcmV0",
            "deadbeefcafebabe0123456789abcdef",
            "v a k d r o l",
        ] {
            assert!(c.find(s).is_empty(), "{s:?}: {:?}", c.find(s));
        }
    }

    #[test]
    fn case_variants() {
        let c = Canaries::new([KEY, "Ørvak Dzajin"]);
        assert_eq!(forms(&c, &KEY.to_uppercase()), [Form::CaseVariant]);
        assert_eq!(
            forms(&c, &format!("x{}x", KEY.to_lowercase())),
            [Form::CaseVariant]
        );
        assert_eq!(forms(&c, "sk_LIVE_zq81vwx03lmnprt5"), [Form::CaseVariant]);
        assert_eq!(forms(&c, "ørvak dzajin"), [Form::CaseVariant]);
        assert_eq!(forms(&c, "ØRVAK DZAJIN"), [Form::CaseVariant]);
        assert_eq!(forms(&c, KEY), [Form::Exact]);
        // Digits have no case.
        assert!(
            Canaries::new(["123456"])
                .find("123456")
                .iter()
                .all(|f| f.form == Form::Exact)
        );
    }

    #[test]
    fn json_escaped_once_twice_and_unicode() {
        let v = "pa\"ss\\wörd/1";
        let c = Canaries::new([v]);
        assert_eq!(forms(&c, r#"{"a":"pa\"ss\\wörd/1"}"#), [Form::JsonEscaped]);
        assert_eq!(forms(&c, r#"pa\"ss\\wörd/1"#), [Form::JsonEscaped]);
        assert_eq!(forms(&c, r#"pa\"ss\\wörd\/1"#), [Form::JsonEscaped]);
        assert_eq!(
            forms(&c, r#"{"args":"{\"v\":\"pa\\\"ss\\\\wörd/1\"}"}"#),
            [Form::JsonEscaped]
        );
        // A value with nothing to escape is only ever exact.
        assert_eq!(
            forms(&Canaries::new([KEY]), &format!("\"{KEY}\"")),
            [Form::Exact]
        );
    }

    #[test]
    fn url_encoded() {
        let c = Canaries::new(["a b/c@d.io"]);
        for s in [
            "?q=a%20b%2Fc%40d.io",
            "?q=a%20b%2fc%40d.io",
            "?q=a+b%2Fc%40d.io",
            "%61%20%62%2F%63%40%64%2E%69%6F",
        ] {
            assert_eq!(forms(&c, s), [Form::UrlEncoded], "{s}");
        }
        assert!(c.find("a%20b%2Fc%40e.io").is_empty());
    }

    #[test]
    fn html_escaped() {
        let c = Canaries::new(["x<y&'z'>"]);
        for s in [
            "<p>x&lt;y&amp;&#39;z&#39;&gt;</p>",
            "x&lt;y&amp;&apos;z&apos;&gt;",
            "&#120;&#60;&#121;&#38;&#39;&#122;&#39;&#62;",
            "&#x78;&#x3c;&#x79;&#x26;&#x27;&#x7a;&#x27;&#x3e;",
        ] {
            assert_eq!(forms(&c, s), [Form::HtmlEscaped], "{s}");
        }
    }

    #[test]
    fn base64_whole_and_substrings_at_every_alignment() {
        let c = Canaries::new([KEY]);
        for url_safe in [false, true] {
            assert_eq!(forms(&c, &b64(KEY.as_bytes(), url_safe)), [Form::Base64]);
            // Inside a longer blob, at each of the three byte alignments.
            for pad in ["", "a", "ab", "abc"] {
                let blob = b64(format!("{pad}token={KEY};").as_bytes(), url_safe);
                let f = c.find(format!("data: {blob}"));
                assert_eq!(f.len(), 1, "{pad:?} {blob}: {f:?}");
                assert_eq!(f[0].form, Form::Base64);
                assert!(f[0].offset >= 6 && f[0].offset + f[0].len <= 6 + blob.len());
            }
            // Only a substring of 8+ bytes leaked.
            let part = &KEY[5..15];
            let blob = b64(format!("xy{part}").as_bytes(), url_safe);
            assert_eq!(forms(&c, &blob), [Form::Base64], "{blob}");
        }
        // Seven bytes of the canary are below the minimum.
        assert!(c.find(b64(b"live_Zq", false)).is_empty());
        // A different value.
        assert!(c.find(b64(b"pk_test_Hh27yUe99QaBcDe0", false)).is_empty());
        assert!(
            c.find(b64(b"unrelated secret-ish text here", false))
                .is_empty()
        );
    }

    #[test]
    fn short_canary_in_base64_must_be_whole() {
        let c = Canaries::new(["Kx7q2"]);
        assert_eq!(forms(&c, &b64(b"id=Kx7q2", false)), [Form::Base64]);
        assert!(c.find(b64(b"id=Kx7q9", false)).is_empty());
    }

    #[test]
    fn hex_lower_upper_and_substrings() {
        let c = Canaries::new([KEY]);
        let hex: String = KEY.bytes().map(|b| format!("{b:02x}")).collect();
        assert_eq!(forms(&c, &hex), [Form::Hex]);
        assert_eq!(forms(&c, &hex.to_uppercase()), [Form::Hex]);
        // Odd alignment inside a longer hex run.
        assert_eq!(forms(&c, &format!("f{hex}0")), [Form::Hex]);
        let part: String = KEY[3..13].bytes().map(|b| format!("{b:02x}")).collect();
        assert_eq!(forms(&c, &format!("hash {part}")), [Form::Hex]);
        let short: String = KEY[3..9].bytes().map(|b| format!("{b:02x}")).collect();
        assert!(c.find(&short).is_empty());
    }

    #[test]
    fn reversed() {
        let c = Canaries::new([KEY]);
        let r: String = KEY.chars().rev().collect();
        assert_eq!(forms(&c, &format!("x {r} y")), [Form::Reversed]);
        // Too short to reverse meaningfully.
        assert!(Canaries::new(["abc"]).find("cba").is_empty());
    }

    #[test]
    fn separator_split() {
        let c = Canaries::new(["Vakdril", "4539-1488"]);
        for s in [
            "V a k d r i l",
            "V-a-k-d-r-i-l",
            "V.a.k.d.r.i.l",
            "V_a_k_d_r_i_l",
            "V\na\nk\nd\nr\ni\nl",
            "V - a - k - d - r - i - l",
        ] {
            assert_eq!(forms(&c, s), [Form::SeparatorSplit], "{s:?}");
        }
        // The canary's own separators are not required.
        assert!(forms(&c, "4 5 3 9 1 4 8 8").contains(&Form::SeparatorSplit));
        // Not every gap separated, or another word.
        assert!(c.find("V a kd r i l").is_empty());
        assert!(c.find("V a k d r o l").is_empty());
        assert!(c.find("V,a,k,d,r,i,l").is_empty());
    }

    #[test]
    fn digit_fragments() {
        let c = Canaries::new([CARD]);
        assert_eq!(forms(&c, "ends in 6467"), [Form::DigitFragment]);
        // Across the canary's own spaces, and longer runs as one finding.
        let f = c.find("ref 14880343 ok");
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].form, f[0].offset, f[0].len),
            (Form::DigitFragment, 4, 8)
        );
        assert_eq!(forms(&c, "4539148803436467"), [Form::DigitFragment]);
        // Below the minimum, digits not in the canary, and the exact value.
        assert!(c.find("453 148 034").is_empty());
        assert!(c.find("7777 2222").is_empty());
        assert_eq!(forms(&c, CARD), [Form::Exact]);
        // Configurable minimum and ignored fragments.
        let strict = Canaries::with_options(
            [CARD],
            Options {
                digit_min: 6,
                ..Options::default()
            },
        );
        assert!(strict.find("ends in 6467").is_empty());
        assert_eq!(forms(&strict, "x 880343 y"), [Form::DigitFragment]);
        let years = Canaries::with_options(
            ["2024 5566 7788"],
            Options {
                ignore_fragments: vec!["2024".into()],
                ..Options::default()
            },
        );
        assert!(years.find("copyright 2024").is_empty());
        // Also a fragment inside an ignored string.
        let constants = Canaries::with_options(
            ["+1 (200) 200-0000"],
            Options {
                ignore_fragments: vec!["32000".into()],
                ..Options::default()
            },
        );
        assert!(constants.find("\"max_tokens\":32000").is_empty());
        assert_eq!(forms(&constants, "x 2000000 y"), [Form::DigitFragment]);
        assert_eq!(forms(&years, "id 20245566"), [Form::DigitFragment]);
        // Values that are not mostly digits have no digit fragments.
        assert!(Canaries::new(["user1234@mail.io"]).find("1234").is_empty());
    }

    #[test]
    fn spelled_digits() {
        let c = Canaries::new([CARD]);
        for s in [
            "four five three nine",
            "Four-Five-Three-Nine",
            "last digits: six four six seven.",
            "one four  eight eight zero",
        ] {
            assert_eq!(forms(&c, s), [Form::SpelledDigits], "{s:?}");
        }
        let f = c.find("x six four six seven");
        assert_eq!((f[0].offset, f[0].len), (2, 18));
        // Too few, other digits, and words inside other words.
        assert!(c.find("four five three").is_empty());
        assert!(c.find("seven seven seven seven").is_empty());
        assert!(c.find("fourfive threenine").is_empty());
        assert!(c.find("someone fourteen fiver").is_empty());
    }

    #[test]
    fn a_thousand_requests_are_fast() {
        let values: Vec<String> = (0..40)
            .map(|i| format!("sk_live_{i:02}Zq81vWx03LmNpRt5{i}"))
            .chain((0..20).map(|i| format!("4539 14{i:02} 0343 6467")))
            .collect();
        let c = Canaries::new(values);
        let body = "{\"role\":\"user\",\"content\":\"please refactor the reconcile function\"} "
            .repeat(30);
        let t = std::time::Instant::now();
        for _ in 0..1000 {
            assert!(c.find(&body).is_empty());
        }
        // Generous bound for debug builds on shared machines.
        assert!(t.elapsed().as_secs() < 60, "{:?}", t.elapsed());
    }
}
