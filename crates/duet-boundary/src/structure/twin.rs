// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic twins: records of a data file with every value replaced by a
//! rule-generated fake.
//!
//! The twin keeps the file's layout byte for byte outside its values (key
//! order, indentation, quoting, delimiters, line endings, nulls, missing
//! keys, array lengths) and each value's shape (lengths, digit counts,
//! letter case, punctuation, leading zeros), and makes fakes that are valid
//! where the format checks: calendar dates in the same layout, card numbers
//! that pass Luhn, IBANs that pass mod-97 for a registry country of the
//! same length, national ids and phone numbers the detectors still
//! recognize, integers on the same side of 2^53. Emails get a reserved
//! `.test` domain. Booleans are drawn at random.
//!
//! **What a fake depends on.** Its value's shape and kind, and the position
//! of the value's first occurrence in the file (so equal values get equal
//! fakes, across fields too, and a larger sample repeats a smaller one),
//! mixed with the run's seed. Never the value's characters: a fake cannot
//! be inverted to its value, however many are shown. The caller then checks
//! that no value it knows appears in the twin.
//!
//! **Which records.** The first, then (greedily) records whose fields show
//! shapes, types, nulls, missing keys or anomalies the ones chosen do not,
//! then the next in order; shown in file order with their positions.

use super::dates;
use super::parse::{self, Json};
use super::profile::{has_header, json_records};
use super::shape::{self, Class, Detail};
use super::{Format, Knowledge, schema_key};
use crate::detect::{Detectors, Kind, scan_each};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::Path;

/// A small deterministic generator (SplitMix64).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (`n` > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    /// A number in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi.saturating_sub(lo) + 1)
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// `a` and `b` mixed into one seed.
pub fn mix(a: u64, b: u64) -> u64 {
    Rng::new(a ^ b.wrapping_mul(0xD6E8_FEB8_6659_FD93)).next_u64()
}

const CONSONANTS: &[u8] = b"bcdfghjklmnprstvz";
const VOWELS: &[u8] = b"aeiou";
const LOWER2: &[char] = &['é', 'ö', 'ñ', 'ø', 'å', 'ü', 'ç', 'ł', 'š', 'ž'];
const UPPER2: &[char] = &['É', 'Ö', 'Ñ', 'Ø', 'Å', 'Ü', 'Ç', 'Ł', 'Š', 'Ž'];
/// Words that mean "no value": kept as written.
const NULLISH: &[&str] = &[
    "null",
    "none",
    "nil",
    "n/a",
    "na",
    "undefined",
    "nan",
    "-",
    "--",
];
const BOOLEANS: &[(&str, &str)] = &[("true", "false"), ("yes", "no"), ("y", "n")];
/// URL schemes kept as written: they name a protocol, not a value.
const SCHEMES: &[&str] = &[
    "http",
    "https",
    "ftp",
    "ftps",
    "sftp",
    "ssh",
    "git",
    "file",
    "ws",
    "wss",
    "postgres",
    "postgresql",
    "mysql",
    "mariadb",
    "redis",
    "rediss",
    "mongodb",
    "mongodb+srv",
    "amqp",
    "amqps",
    "s3",
    "gs",
    "jdbc",
    "ldap",
    "ldaps",
    "smtp",
    "imap",
    "mqtt",
    "kafka",
    "nats",
];

/// A letter of the same kind as `orig` (case, and UTF-8 length for
/// letters outside ASCII); `vowel` picks among vowels for ASCII.
fn letter(orig: char, vowel: bool, rng: &mut Rng) -> char {
    if orig.is_ascii() {
        let c = char::from(*rng.pick(if vowel { VOWELS } else { CONSONANTS }));
        return if orig.is_ascii_uppercase() {
            c.to_ascii_uppercase()
        } else {
            c
        };
    }
    let pick = |rng: &mut Rng, base: u32, n: u64| char::from_u32(base + rng.below(n) as u32);
    match (orig.len_utf8(), shape::class(orig)) {
        (2, Class::Upper) => *rng.pick(UPPER2),
        (2, Class::Lower) => *rng.pick(LOWER2),
        (2, _) => pick(rng, 0x05D0, 27).unwrap_or('a'),
        (3, _) => pick(rng, 0x4E00, 0x51A6).unwrap_or('a'),
        _ => pick(rng, 0x20000, 0xA6D7).unwrap_or('a'),
    }
}

/// `value` with each letter run replaced by pronounceable letters of the
/// same case and each digit run by random digits (a run of two or more
/// keeps a leading zero, and keeps a leading non-zero digit non-zero);
/// everything else kept.
pub fn generic(value: &str, rng: &mut Rng) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < chars.len() {
        match shape::class(chars[i]) {
            Class::Upper | Class::Lower | Class::Caseless => {
                let mut vowel = rng.below(2) == 0;
                while i < chars.len()
                    && matches!(
                        shape::class(chars[i]),
                        Class::Upper | Class::Lower | Class::Caseless
                    )
                {
                    out.push(letter(chars[i], vowel, rng));
                    // Now and then two consonants, as in real words.
                    vowel = if vowel { false } else { rng.below(5) != 0 };
                    i += 1;
                }
            }
            Class::Digit => {
                let start = i;
                while i < chars.len() && shape::class(chars[i]) == Class::Digit {
                    i += 1;
                }
                let run = i - start;
                for j in 0..run {
                    let d = match (j, run > 1, chars[start]) {
                        (0, true, '0') => 0,
                        (0, true, _) => rng.range(1, 9),
                        _ => rng.below(10),
                    };
                    out.push(char::from(b'0' + d as u8));
                }
            }
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

const EXACT_DOUBLE: u128 = 9_007_199_254_740_992;

/// (above 2^53, exact as a double) for integer digits.
fn double_class(digits: &str) -> Option<(bool, bool)> {
    let v: u128 = digits.parse().ok()?;
    Some((v > EXACT_DOUBLE, (v as f64) as u128 == v))
}

/// Digits for an integer part of `int.len()` digits: the same leading zero
/// or non-zero lead, epoch seconds and milliseconds kept plausible, and a
/// long integer on the same side of 2^53 and as exact (or not) as a double.
fn integer(int: &str, rng: &mut Rng) -> String {
    let n = int.len();
    if n <= 1 {
        return if n == 0 {
            String::new()
        } else {
            rng.below(10).to_string()
        };
    }
    if let Ok(v) = int.parse::<u64>() {
        if n == 10 && (946_684_800..=2_147_483_647).contains(&v) {
            return rng.range(946_684_800, 2_147_483_647).to_string();
        }
        if n == 13 && (946_684_800_000..=2_147_483_647_000).contains(&v) {
            return rng.range(946_684_800_000, 2_147_483_647_000).to_string();
        }
    }
    let want = double_class(int);
    let mut out = String::new();
    for _ in 0..64 {
        out = generic(int, rng);
        if want.is_none() || double_class(&out) == want {
            break;
        }
        // Push the lead up when it must be above 2^53 (16 digits start with 9).
        if want.is_some_and(|w| w.0) && n == 16 {
            out.replace_range(0..1, "9");
            if double_class(&out) == want {
                break;
            }
        }
    }
    out
}

/// A number like `raw` (`-12.50`, `7306743338619943`, `1e-3`, `3,5`): same
/// sign, digit counts and separators, a fraction ending in zero only if the
/// original's does.
pub fn number(raw: &str, rng: &mut Rng) -> String {
    let (sign, body) = match raw.chars().next() {
        Some(s @ ('-' | '+')) => (s.to_string(), &raw[1..]),
        _ => (String::new(), raw),
    };
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i..])),
        None => (body, None),
    };
    let (int, frac) = match mantissa.find(['.', ',']) {
        Some(i) => (
            &mantissa[..i],
            Some((&mantissa[i..i + 1], &mantissa[i + 1..])),
        ),
        None => (mantissa, None),
    };
    let mut out = sign;
    out.push_str(&integer(int, rng));
    if let Some((sep, f)) = frac {
        out.push_str(sep);
        let zero_end = f.ends_with('0');
        for j in 0..f.len() {
            let d = if j + 1 == f.len() {
                if zero_end { 0 } else { rng.range(1, 9) }
            } else {
                rng.below(10)
            };
            out.push(char::from(b'0' + d as u8));
        }
    }
    if let Some(e) = exponent {
        out.push_str(&e[..1]);
        let rest = &e[1..];
        let (s, digits) = match rest.chars().next() {
            Some(c @ ('+' | '-')) => (c.to_string(), &rest[1..]),
            _ => (String::new(), rest),
        };
        out.push_str(&s);
        out.push_str(&generic(digits, rng));
    }
    out
}

fn luhn_check(digits: &[u8]) -> u8 {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let d = u32::from(d);
            if i % 2 == 0 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    ((10 - sum % 10) % 10) as u8
}

/// Puts `digits` (or letters) into `template`'s alphanumeric positions,
/// keeping its separators.
fn refill(template: &str, fill: &[char]) -> String {
    let mut it = fill.iter();
    template
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                *it.next().unwrap_or(&c)
            } else {
                c
            }
        })
        .collect()
}

/// A card number of the same length and grouping that passes Luhn, with a
/// network prefix for its length (drawn, never the original's).
pub fn card(value: &str, rng: &mut Rng) -> String {
    let n = value.chars().filter(char::is_ascii_digit).count();
    let mut d: Vec<u8> = match n {
        15 => vec![3, if rng.below(2) == 0 { 4 } else { 7 }],
        14 => vec![3, 6],
        16 if rng.below(2) == 0 => vec![5, rng.range(1, 5) as u8],
        _ => vec![4],
    };
    while d.len() + 1 < n {
        d.push(rng.below(10) as u8);
    }
    d.push(luhn_check(&d));
    let fill: Vec<char> = d.iter().map(|x| char::from(b'0' + x)).collect();
    refill(value, &fill)
}

fn mod97(s: &str) -> u32 {
    let mut rem: u32 = 0;
    for c in s.chars() {
        let v = match c {
            '0'..='9' => c as u32 - '0' as u32,
            'A'..='Z' => c as u32 - 'A' as u32 + 10,
            _ => continue,
        };
        rem = if v >= 10 { rem * 100 + v } else { rem * 10 + v } % 97;
    }
    rem
}

/// An IBAN of the same length and grouping for a registry country with that
/// length, passing mod-97; the account part keeps the original's letter and
/// digit positions.
pub fn iban(value: &str, rng: &mut Rng) -> String {
    let compact: Vec<char> = value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let n = compact.len();
    let mut countries: Vec<String> = Vec::new();
    for a in 'A'..='Z' {
        for b in 'A'..='Z' {
            let c = format!("{a}{b}");
            if crate::pii::iban_length(&c) == Some(n) {
                countries.push(c);
            }
        }
    }
    if countries.is_empty() || n < 5 {
        return generic(value, rng);
    }
    let country = rng.pick(&countries).clone();
    let bban: String = compact[4..]
        .iter()
        .map(|c| {
            if c.is_ascii_digit() {
                char::from(b'0' + rng.below(10) as u8)
            } else {
                char::from(b'A' + rng.below(26) as u8)
            }
        })
        .collect();
    let check = 98 - mod97(&format!("{bban}{country}00"));
    let fake: Vec<char> = format!("{country}{check:02}{bban}").chars().collect();
    refill(value, &fake)
}

/// An email address with the local part and domain labels generated and
/// a reserved `.test` top-level domain.
pub fn email(value: &str, rng: &mut Rng) -> String {
    let Some((local, domain)) = value.rsplit_once('@') else {
        return generic(value, rng);
    };
    let labels: Vec<&str> = domain.split('.').collect();
    let mut out = generic(local, rng);
    out.push('@');
    let kept = labels.len().saturating_sub(1).max(1);
    let fake: Vec<String> = labels[..kept].iter().map(|l| generic(l, rng)).collect();
    out.push_str(&fake.join("."));
    out.push_str(".test");
    out
}

/// A random string of the same length from the same character kinds.
pub fn secret(value: &str, rng: &mut Rng) -> String {
    let mut pool: Vec<char> = Vec::new();
    let mut others: Vec<char> = Vec::new();
    for c in value.chars() {
        match shape::class(c) {
            Class::Upper => pool.extend('A'..='Z'),
            Class::Lower | Class::Caseless => pool.extend('a'..='z'),
            Class::Digit => pool.extend('0'..='9'),
            _ => {
                if !others.contains(&c) {
                    others.push(c);
                }
            }
        }
    }
    pool.sort_unstable();
    pool.dedup();
    pool.extend(others);
    if pool.is_empty() {
        return value.to_owned();
    }
    value.chars().map(|_| *rng.pick(&pool)).collect()
}

/// Whether the detectors find `s` whole as a value of `kind`.
fn detected_as(s: &str, kind: Kind) -> bool {
    let t = s.trim();
    let lead = s.len() - s.trim_start().len();
    scan_each(s, Detectors::default())
        .iter()
        .any(|f| f.kind == kind && f.start <= lead && f.end >= lead + t.len())
}

/// A value of `kind` (a phone number, a national id) the detectors still
/// recognize: generated, then its last letter or digit searched for one
/// that passes the format's check.
fn checked(value: &str, kind: Kind, rng: &mut Rng) -> String {
    let mut last = String::new();
    for _ in 0..8 {
        last = generic(value, rng);
        if detected_as(&last, kind) {
            return last;
        }
        let chars: Vec<char> = last.chars().collect();
        if let Some(pos) = chars.iter().rposition(char::is_ascii_alphanumeric) {
            let candidates: Vec<char> = if chars[pos].is_ascii_digit() {
                ('0'..='9').collect()
            } else if chars[pos].is_ascii_uppercase() {
                ('A'..='Z').collect()
            } else {
                ('a'..='z').collect()
            };
            for c in candidates {
                let mut t = chars.clone();
                t[pos] = c;
                let s: String = t.iter().collect();
                if detected_as(&s, kind) {
                    return s;
                }
            }
        }
    }
    last
}

fn luhn_ok(digits: &str) -> bool {
    let d: Vec<u8> = digits.bytes().map(|b| b - b'0').collect();
    match d.split_last() {
        Some((&check, body)) => luhn_check(body) == check,
        None => false,
    }
}

/// A fake for the text value `value`.
pub fn text(value: &str, k: &dyn Knowledge, rng: &mut Rng) -> String {
    if value.is_empty() {
        return String::new();
    }
    let trimmed = value.trim();
    let lower = trimmed.to_lowercase();
    if NULLISH.contains(&lower.as_str()) {
        return value.to_owned();
    }
    for (a, b) in BOOLEANS {
        if lower == *a || lower == *b {
            let pick = if rng.below(2) == 0 { a } else { b };
            let cased: String = if trimmed.chars().all(|c| !c.is_lowercase()) {
                pick.to_uppercase()
            } else if trimmed.chars().next().is_some_and(char::is_uppercase) {
                let mut c = pick.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default()
            } else {
                (*pick).to_owned()
            };
            return value.replace(trimmed, &cased);
        }
    }
    if let Some(layout) = dates::recognize(value) {
        return layout.generate(rng);
    }
    let lead = value.len() - value.trim_start().len();
    let whole = k
        .values(value)
        .into_iter()
        .chain(
            scan_each(value, Detectors::default())
                .into_iter()
                .map(|f| (f.start, f.end, f.kind)),
        )
        .filter(|(s, e, _)| *s <= lead && *e >= lead + trimmed.len())
        .map(|(_, _, kind)| kind)
        .min_by_key(|kind| match kind {
            Kind::Card | Kind::Iban => 0,
            Kind::Email => 1,
            Kind::Secret => 3,
            _ => 2,
        });
    let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
    let card_like = (13..=19).contains(&digits.len())
        && trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || c == ' ' || c == '-')
        && luhn_ok(&digits);
    let compact: String = trimmed.chars().filter(|c| *c != ' ').collect();
    let iban_like = compact.len() >= 15
        && compact.chars().take(2).all(|c| c.is_ascii_uppercase())
        && crate::pii::iban_length(&compact[..2]) == Some(compact.len())
        && crate::pii::iban_checksum(&compact);
    let fake = match whole {
        Some(Kind::Card) => card(trimmed, rng),
        Some(Kind::Iban) => iban(trimmed, rng),
        _ if card_like => card(trimmed, rng),
        _ if iban_like => iban(trimmed, rng),
        Some(Kind::Email) => email(trimmed, rng),
        Some(Kind::Secret) if !trimmed.contains("://") => secret(trimmed, rng),
        Some(kind @ (Kind::Phone | Kind::NationalId | Kind::Account | Kind::Ip)) => {
            checked(trimmed, kind, rng)
        }
        _ => {
            let numeric = {
                let d = trimmed.strip_prefix(['-', '+']).unwrap_or(trimmed);
                !d.is_empty()
                    && d.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && d.chars()
                        .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
                    && d.matches(['.', ',']).count() <= 1
            };
            if numeric {
                number(trimmed, rng)
            } else if let Some((scheme, rest)) = trimmed.split_once("://")
                && SCHEMES.contains(&scheme.to_lowercase().as_str())
            {
                format!("{scheme}://{}", generic(rest, rng))
            } else {
                generic(trimmed, rng)
            }
        }
    };
    format!("{}{fake}{}", &value[..lead], &value[lead + trimmed.len()..])
}

/// How a value is faked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Role {
    Text,
    Number,
    Key,
}

/// Makes the twin's fakes: one per distinct (role, value), seeded by the
/// value's first position among `order`.
struct Faker<'k> {
    seed: u64,
    k: &'k dyn Knowledge,
    order: HashMap<(Role, String), u64>,
    made: HashMap<(Role, String), String>,
    /// Fakes that a check found holding a known value after every try.
    pub stuck: usize,
}

/// Tries per value to draw a fake that holds no known value.
const TRIES: u64 = 16;

impl<'k> Faker<'k> {
    fn new(seed: u64, k: &'k dyn Knowledge, all: Vec<(Role, String)>) -> Self {
        let mut order = HashMap::new();
        for v in all {
            let n = order.len() as u64;
            order.entry(v).or_insert(n);
        }
        Self {
            seed,
            k,
            order,
            made: HashMap::new(),
            stuck: 0,
        }
    }

    fn fake(&mut self, role: Role, value: &str) -> String {
        let key = (role, value.to_owned());
        if let Some(f) = self.made.get(&key) {
            return f.clone();
        }
        let at = self
            .order
            .get(&key)
            .copied()
            .unwrap_or(u64::MAX - self.made.len() as u64);
        let mut fake = String::new();
        for attempt in 0..TRIES {
            let mut rng = Rng::new(mix(mix(self.seed, at), attempt));
            fake = match role {
                Role::Number => number(value, &mut rng),
                Role::Text => text(value, self.k, &mut rng),
                Role::Key => generic(value, &mut rng),
            };
            if !self.k.known(&fake) {
                break;
            }
            if attempt + 1 == TRIES {
                self.stuck += 1;
            }
        }
        self.made.insert(key, fake.clone());
        fake
    }

    /// A boolean for the literal at byte `at`: drawn, whatever it was.
    fn boolean(&self, at: usize) -> &'static str {
        if Rng::new(mix(self.seed ^ 0xB001, at as u64)).below(2) == 0 {
            "true"
        } else {
            "false"
        }
    }
}

/// A synthetic twin.
#[derive(Debug, Clone)]
pub struct Twin {
    pub text: String,
    pub format: Format,
    /// 1-based positions of the records it holds.
    pub records: Vec<usize>,
    /// Records in the original.
    pub total: usize,
    /// Text copied as written from the original besides layout (a header
    /// row), which the caller may treat as shown schema.
    pub verbatim: Vec<String>,
    /// Values for which no fake free of known values was found.
    pub stuck: usize,
}

fn hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Records to show: the first, then those adding features the chosen ones
/// lack, then the next in order; sorted.
fn select(features: &[Vec<u64>], n: usize) -> Vec<usize> {
    const CONSIDERED: usize = 5000;
    let n = n.min(features.len());
    if n == 0 {
        return Vec::new();
    }
    let limit = features.len().min(CONSIDERED);
    let mut covered: HashSet<u64> = features[0].iter().copied().collect();
    let mut taken = vec![false; features.len()];
    taken[0] = true;
    let mut chosen = vec![0];
    while chosen.len() < n {
        let mut best: Option<(usize, usize)> = None;
        for (i, f) in features.iter().enumerate().take(limit) {
            if taken[i] {
                continue;
            }
            let gain = f.iter().filter(|x| !covered.contains(x)).count();
            if gain > 0 && best.is_none_or(|(_, g)| gain > g) {
                best = Some((i, gain));
            }
        }
        let pick = match best {
            Some((i, _)) => i,
            None => match (0..features.len()).find(|&i| !taken[i]) {
                Some(i) => i,
                None => break,
            },
        };
        taken[pick] = true;
        covered.extend(features[pick].iter().copied());
        chosen.push(pick);
    }
    chosen.sort_unstable();
    chosen
}

/// What makes a value worth a record in the sample: its shape class.
fn feature(path: &str, value: &str) -> String {
    let class = if value.is_empty() {
        "empty".to_owned()
    } else if let Some(l) = dates::recognize(value) {
        l.picture()
    } else {
        let mut s: String = shape::shape(value, Detail::Loose)
            .chars()
            .take(40)
            .collect();
        if value.contains('\n') {
            s.push_str("|nl");
        }
        if value.trim() != value {
            s.push_str("|pad");
        }
        s
    };
    format!("{path}\u{0}{class}")
}

fn number_feature(path: &str, raw: &str) -> String {
    let neg = raw.starts_with('-');
    let int: String = raw
        .trim_start_matches('-')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let places = raw.split_once('.').map_or(0, |(_, f)| f.len());
    let big = double_class(&int).is_some_and(|c| c.0);
    format!("{path}\u{0}n{neg}{places}{big}")
}

fn json_features(node: &Json, path: &str, text: &str, out: &mut Vec<String>) {
    match node {
        Json::Object { entries, .. } => {
            out.push(format!("{path}\u{0}object{}", entries.is_empty()));
            for (key, v) in entries {
                json_features(v, &format!("{path}.{}", key.name), text, out);
            }
        }
        Json::Array { items, .. } => {
            out.push(format!("{path}\u{0}array{}", items.is_empty()));
            for v in items {
                json_features(v, &format!("{path}[]"), text, out);
            }
        }
        Json::String { value, .. } => out.push(feature(path, value)),
        Json::Number { span } => out.push(number_feature(path, &text[span.clone()])),
        Json::Bool { .. } => out.push(format!("{path}\u{0}bool")),
        Json::Null { .. } => out.push(format!("{path}\u{0}null")),
    }
}

/// Feature sets per record, with a feature for every path a record lacks
/// that another has.
fn with_missing(per_record: Vec<Vec<String>>) -> Vec<Vec<u64>> {
    let paths = |f: &Vec<String>| -> BTreeSet<String> {
        f.iter()
            .filter_map(|x| x.split('\u{0}').next().map(str::to_owned))
            .collect()
    };
    let all: BTreeSet<String> = per_record.iter().flat_map(paths).collect();
    per_record
        .iter()
        .map(|f| {
            let have = paths(f);
            let mut out: Vec<u64> = f.iter().map(|x| hash(x)).collect();
            out.extend(
                all.difference(&have)
                    .map(|p| hash(&format!("missing\u{0}{p}"))),
            );
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect()
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Every value of `node` in document order, for the fakes' seeds.
fn json_values(node: &Json, text: &str, k: &dyn Knowledge, out: &mut Vec<(Role, String)>) {
    match node {
        Json::Object { entries, .. } => {
            for (key, v) in entries {
                if !schema_key(&key.name, k) {
                    out.push((Role::Key, key.name.clone()));
                }
                json_values(v, text, k, out);
            }
        }
        Json::Array { items, .. } => {
            for v in items {
                json_values(v, text, k, out);
            }
        }
        Json::String { value, .. } => out.push((Role::Text, value.clone())),
        Json::Number { span } => out.push((Role::Number, text[span.clone()].to_owned())),
        _ => {}
    }
}

/// The replacements that make `node` a twin.
fn json_replacements(
    node: &Json,
    text: &str,
    f: &mut Faker<'_>,
    out: &mut Vec<(Range<usize>, String)>,
) {
    match node {
        Json::Object { entries, .. } => {
            for (key, v) in entries {
                if !schema_key(&key.name, f.k) {
                    out.push((key.span.clone(), json_string(&f.fake(Role::Key, &key.name))));
                }
                json_replacements(v, text, f, out);
            }
        }
        Json::Array { items, .. } => {
            for v in items {
                json_replacements(v, text, f, out);
            }
        }
        Json::String { span, value } => {
            out.push((span.clone(), json_string(&f.fake(Role::Text, value))));
        }
        Json::Number { span } => {
            out.push((span.clone(), f.fake(Role::Number, &text[span.clone()])));
        }
        Json::Bool { span, .. } => out.push((span.clone(), f.boolean(span.start).to_owned())),
        Json::Null { .. } => {}
    }
}

/// `text[range]` with the replacements inside it applied.
fn splice(text: &str, range: Range<usize>, repl: &[(Range<usize>, String)]) -> String {
    let mut out = String::new();
    let mut at = range.start;
    for (r, with) in repl {
        if r.start >= range.start && r.end <= range.end && r.start >= at {
            out.push_str(&text[at..r.start]);
            out.push_str(with);
            at = r.end;
        }
    }
    out.push_str(&text[at..range.end]);
    out
}

fn twin_json(
    text: &str,
    root: &Json,
    f_seed: u64,
    k: &dyn Knowledge,
    rows: usize,
    lines: bool,
) -> Twin {
    let mut all = Vec::new();
    json_values(root, text, k, &mut all);
    let mut f = Faker::new(f_seed, k, all);
    let mut repl = Vec::new();
    let (items, at): (&[Json], Option<usize>) = if lines {
        match root {
            Json::Array { items, .. } => (items, None),
            _ => (&[], None),
        }
    } else {
        match json_records(root) {
            Some((at, items)) => (items, at),
            None => (&[], None),
        }
    };
    if items.len() < 2 {
        json_replacements(root, text, &mut f, &mut repl);
        let stuck = f.stuck;
        return Twin {
            text: splice(text, 0..text.len(), &repl),
            format: Format::Json,
            records: vec![1],
            total: 1,
            verbatim: Vec::new(),
            stuck,
        };
    }
    let features: Vec<Vec<String>> = items
        .iter()
        .map(|r| {
            let mut out = Vec::new();
            json_features(r, "", text, &mut out);
            out
        })
        .collect();
    let chosen = select(&with_missing(features), rows);
    // The document around the records (other keys) and the chosen records.
    if let (Some(at), Json::Object { entries, .. }) = (at, root) {
        for (i, (key, v)) in entries.iter().enumerate() {
            if i != at {
                if !schema_key(&key.name, k) {
                    repl.push((key.span.clone(), json_string(&f.fake(Role::Key, &key.name))));
                }
                json_replacements(v, text, &mut f, &mut repl);
            } else if !schema_key(&key.name, k) {
                repl.push((key.span.clone(), json_string(&f.fake(Role::Key, &key.name))));
            }
        }
    }
    for &i in &chosen {
        json_replacements(&items[i], text, &mut f, &mut repl);
    }
    repl.sort_by_key(|r| r.0.start);
    let first = items[0].span();
    let second = items[1].span();
    let last = items[items.len() - 1].span();
    let sep = &text[first.end..second.start];
    let body: Vec<String> = chosen
        .iter()
        .map(|&i| splice(text, items[i].span(), &repl))
        .collect();
    let out = if lines {
        let mut s = body.join("\n");
        s.push('\n');
        s
    } else {
        format!(
            "{}{}{}",
            splice(text, 0..first.start, &repl),
            body.join(sep),
            splice(text, last.end..text.len(), &repl)
        )
    };
    let stuck = f.stuck;
    Twin {
        text: out,
        format: if lines {
            Format::JsonLines
        } else {
            Format::Json
        },
        records: chosen.iter().map(|i| i + 1).collect(),
        total: items.len(),
        verbatim: Vec::new(),
        stuck,
    }
}

fn cell_text(value: &str, quoted: bool, delimiter: char) -> String {
    if quoted || value.contains([delimiter, '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn twin_delimited(
    text: &str,
    delimiter: char,
    seed: u64,
    k: &dyn Knowledge,
    rows: usize,
) -> Option<Twin> {
    let all_rows = parse::delimited(text, delimiter)?;
    let header = has_header(&all_rows, k);
    let body = &all_rows[usize::from(header)..];
    let mut all = Vec::new();
    for row in body {
        for c in &row.cells {
            all.push((Role::Text, c.value.clone()));
        }
    }
    let mut f = Faker::new(seed, k, all);
    let names: Vec<String> = if header {
        all_rows[0].cells.iter().map(|c| c.value.clone()).collect()
    } else {
        Vec::new()
    };
    let features: Vec<Vec<String>> = body
        .iter()
        .map(|r| {
            let mut out: Vec<String> = r
                .cells
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut x = feature(&i.to_string(), &c.value);
                    if c.value.contains([delimiter, '"']) {
                        x.push_str("|embedded");
                    }
                    x
                })
                .collect();
            out.push(format!("width\u{0}{}", r.cells.len()));
            out
        })
        .collect();
    let chosen = select(&with_missing(features), rows);
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let d = delimiter.to_string();
    let mut lines = Vec::new();
    let mut verbatim = Vec::new();
    if header {
        let cells: Vec<String> = all_rows[0]
            .cells
            .iter()
            .map(|c| {
                if schema_key(c.value.trim(), k) {
                    text[c.span.clone()].to_owned()
                } else {
                    cell_text(&f.fake(Role::Key, &c.value), c.quoted, delimiter)
                }
            })
            .collect();
        let line = cells.join(&d);
        if names.iter().all(|n| schema_key(n.trim(), k)) {
            verbatim.push(line.clone());
        }
        lines.push(line);
    }
    for &i in &chosen {
        let cells: Vec<String> = body[i]
            .cells
            .iter()
            .map(|c| cell_text(&f.fake(Role::Text, &c.value), c.quoted, delimiter))
            .collect();
        lines.push(cells.join(&d));
    }
    let mut out = lines.join(eol);
    out.push_str(eol);
    let stuck = f.stuck;
    Some(Twin {
        text: out,
        format: Format::Delimited { delimiter },
        records: chosen.iter().map(|i| i + 1).collect(),
        total: body.len(),
        verbatim,
        stuck,
    })
}

fn twin_env(text: &str, seed: u64, k: &dyn Knowledge) -> Twin {
    let assignments = parse::assignments(text);
    let all = assignments
        .iter()
        .map(|a| (Role::Text, a.value.clone()))
        .collect();
    let mut f = Faker::new(seed, k, all);
    let mut out = String::new();
    let mut offset = 0;
    let mut next = assignments.iter().peekable();
    for line in text.split_inclusive('\n') {
        let end = offset + line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        let eol = &line[body.len()..];
        if let Some(a) = next.peek()
            && a.span.start >= offset
            && a.span.end <= end
        {
            let a = next.next().expect("peeked");
            let fake = f.fake(Role::Text, &a.value);
            let written = match a.quote {
                Some(q) => format!("{q}{fake}{q}"),
                None => fake,
            };
            out.push_str(&text[offset..a.span.start]);
            out.push_str(&written);
            out.push_str(&text[a.span.end..end]);
        } else if body.trim().is_empty() {
            out.push_str(eol);
        }
        // Comments are prose: not in the twin.
        offset = end;
    }
    let n = assignments.len();
    let stuck = f.stuck;
    Twin {
        text: out,
        format: Format::Env,
        records: (1..=n).collect(),
        total: n,
        verbatim: Vec::new(),
        stuck,
    }
}

fn twin_fixed(text: &str, seed: u64, k: &dyn Knowledge, rows: usize) -> Option<Twin> {
    let cols = parse::fixed_width(text)?;
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let cell = |l: &str, c: &Range<usize>| -> String {
        l.get(c.start..c.end.min(l.len()))
            .unwrap_or_default()
            .to_owned()
    };
    let first: Vec<String> = cols
        .iter()
        .map(|c| cell(lines[0], c).trim().to_owned())
        .collect();
    let header = first
        .iter()
        .all(|v| !v.is_empty() && v.chars().all(|c| c.is_alphabetic() || " _-".contains(c)))
        && first.iter().all(|v| schema_key(v, k));
    let body = &lines[usize::from(header)..];
    let all = body
        .iter()
        .flat_map(|l| {
            cols.iter()
                .map(|c| (Role::Text, cell(l, c).trim().to_owned()))
        })
        .collect();
    let mut f = Faker::new(seed, k, all);
    let features: Vec<Vec<String>> = body
        .iter()
        .map(|l| {
            cols.iter()
                .enumerate()
                .map(|(i, c)| feature(&i.to_string(), cell(l, c).trim()))
                .collect()
        })
        .collect();
    let chosen = select(&with_missing(features), rows);
    let mut out = String::new();
    let mut verbatim = Vec::new();
    if header {
        out.push_str(lines[0]);
        out.push('\n');
        verbatim.push(lines[0].to_owned());
    }
    for &i in &chosen {
        let l = body[i];
        let mut line = String::new();
        let mut at = 0;
        for c in &cols {
            line.push_str(l.get(at..c.start.min(l.len())).unwrap_or_default());
            let raw = cell(l, c);
            let trimmed = raw.trim();
            let lead = raw.len() - raw.trim_start().len();
            let fake = f.fake(Role::Text, trimmed);
            line.push_str(&raw[..lead]);
            line.push_str(&fake);
            line.push_str(&raw[lead + trimmed.len()..]);
            at = c.end.min(l.len());
        }
        line.push_str(l.get(at..).unwrap_or_default());
        out.push_str(&line);
        out.push('\n');
    }
    let stuck = f.stuck;
    Some(Twin {
        text: out,
        format: Format::FixedWidth,
        records: chosen.iter().map(|i| i + 1).collect(),
        total: body.len(),
        verbatim,
        stuck,
    })
}

fn xml_decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn xml_encode(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

static XML_COMMENT: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)<!--.*?-->").expect("static regex"));
static XML_NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"</?([^\s/>!?]+)|\s([^\s=/>]+)\s*="#).expect("static regex")
});

fn twin_xml(text: &str, seed: u64, k: &dyn Knowledge, rows: usize) -> Result<Twin, String> {
    let x = parse::xml(text).ok_or("the XML could not be read")?;
    // Element and attribute names are copied as written: each must be schema.
    let stripped = XML_COMMENT.replace_all(text, "");
    for c in XML_NAME.captures_iter(&stripped) {
        let name = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
        if !name.is_empty() && !name.starts_with("xml") && !schema_key(name, k) {
            return Err("element or attribute names in it look like data".into());
        }
    }
    let all = x
        .values
        .iter()
        .map(|v| (Role::Text, xml_decode(&v.value)))
        .collect();
    let mut f = Faker::new(seed, k, all);
    let mut features: Vec<Vec<String>> = vec![Vec::new(); x.records.len()];
    for v in &x.values {
        if let Some(r) = v.record {
            features[r].push(feature(&v.path, &xml_decode(&v.value)));
        }
    }
    let chosen = select(&with_missing(features), rows.max(1));
    let mut repl: Vec<(Range<usize>, String)> = Vec::new();
    for v in &x.values {
        if v.record.is_none_or(|r| chosen.contains(&r)) {
            repl.push((
                v.span.clone(),
                xml_encode(&f.fake(Role::Text, &xml_decode(&v.value))),
            ));
        }
    }
    repl.sort_by_key(|r| r.0.start);
    let out = if x.records.len() >= 2 {
        let first = x.records[0].clone();
        let sep = &text[first.end..x.records[1].start];
        let last = x.records[x.records.len() - 1].clone();
        let body: Vec<String> = chosen
            .iter()
            .map(|&i| splice(text, x.records[i].clone(), &repl))
            .collect();
        format!(
            "{}{}{}",
            splice(text, 0..first.start, &repl),
            body.join(sep),
            splice(text, last.end..text.len(), &repl)
        )
    } else {
        splice(text, 0..text.len(), &repl)
    };
    let stuck = f.stuck;
    Ok(Twin {
        text: XML_COMMENT.replace_all(&out, "").into_owned(),
        format: Format::Xml,
        records: chosen.iter().map(|i| i + 1).collect(),
        total: x.records.len(),
        verbatim: Vec::new(),
        stuck,
    })
}

/// The synthetic twin of `text` (read from `path`, when a file) with up to
/// `rows` records, or why there is none.
pub fn twin(
    text: &str,
    path: Option<&Path>,
    k: &dyn Knowledge,
    seed: u64,
    rows: usize,
) -> Result<Twin, String> {
    let body = text.trim_start_matches('\u{feff}');
    let format = super::detect(body, path);
    let rows = rows.max(1);
    match format {
        Format::Json => {
            let root = parse::json(body).ok_or("the JSON could not be read")?;
            Ok(twin_json(body, &root, seed, k, rows, false))
        }
        Format::JsonLines => {
            let docs = parse::json_lines(body).ok_or("the JSON lines could not be read")?;
            let root = Json::Array {
                span: 0..body.len(),
                items: docs,
            };
            Ok(twin_json(body, &root, seed, k, rows, true))
        }
        Format::Delimited { delimiter } => twin_delimited(body, delimiter, seed, k, rows)
            .ok_or_else(|| "the delimited text could not be read".into()),
        Format::Env => Ok(twin_env(body, seed, k)),
        Format::FixedWidth => {
            twin_fixed(body, seed, k, rows).ok_or_else(|| "the columns could not be read".into())
        }
        Format::Xml => twin_xml(body, seed, k, rows),
        Format::Log | Format::Text => Err(format!(
            "{} has no records to imitate; its line templates are in the structure view",
            format.name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::super::Nothing;
    use super::*;

    #[test]
    fn fakes_keep_shape_and_validity() {
        let mut rng = Rng::new(3);
        for _ in 0..200 {
            let c = card("4539 1488 0343 6467", &mut rng);
            assert_eq!(shape::shape(&c, Detail::Exact), "9999 9999 9999 9999");
            let digits: String = c.chars().filter(char::is_ascii_digit).collect();
            assert!(luhn_ok(&digits), "{c}");
            assert!(detected_as(&c, Kind::Card), "{c}");
            let i = iban("NL91 ABNA 0417 1643 00", &mut rng);
            let compact: String = i.chars().filter(|c| *c != ' ').collect();
            assert_eq!(compact.len(), 18);
            assert!(crate::pii::iban_checksum(&compact), "{i}");
            assert_eq!(
                shape::shape(&i, Detail::Exact).len(),
                "AA99 AAAA 9999 9999 99".len()
            );
            let n = number("7306743338619943", &mut rng);
            assert_eq!(n.len(), 16);
            assert_eq!(double_class(&n), double_class("7306743338619943"));
            let big = number("9306743338619943", &mut rng);
            assert_eq!(
                double_class(&big),
                double_class("9306743338619943"),
                "{big}"
            );
            assert_eq!(
                shape::shape(&number("-12.50", &mut rng), Detail::Exact),
                "-99.99"
            );
            assert!(number("-12.50", &mut rng).ends_with('0'));
            let e = email("oscar.yorovvay01@mailbox-989.net", &mut rng);
            assert!(e.ends_with(".test"), "{e}");
            assert_eq!(
                shape::shape(e.split('@').next().unwrap(), Detail::Exact),
                "aaaaa.aaaaaaaa99"
            );
            let g = generic("FH-0907 Ødegaard", &mut rng);
            assert_eq!(shape::shape(&g, Detail::Exact), "AA-9999 Aaaaaaaa");
            assert!(g.starts_with(|c: char| c.is_ascii_uppercase()));
            assert_eq!(&g[3..4], "0");
        }
        let phone = checked("+1 (607) 914-6209", Kind::Phone, &mut rng);
        assert!(detected_as(&phone, Kind::Phone), "{phone}");
        let dni = checked("12345678Z", Kind::NationalId, &mut rng);
        assert!(crate::pii::dni_valid(&dni), "{dni}");
    }

    #[test]
    fn a_json_twin_keeps_layout_and_replaces_every_value() {
        let text = "{\n  \"month\": \"2026-09\",\n  \"orders\": [\n    {\"ref\": 7306743338619943, \"name\": \"Amelia Quenanlek\", \"ok\": true, \"note\": null, \"ts\": \"2026-09-01 16:28:27.382673\"},\n    {\"ref\": 5864320965969843, \"name\": \"Kenji Tolvisbaum\", \"ok\": false, \"note\": \"call first\", \"ts\": \"2026-09-02 08:00:01.5\"},\n    {\"ref\": 7306743338619943, \"name\": \"Amelia Quenanlek\", \"ok\": true, \"note\": null, \"ts\": \"2026-09-01 16:28:27.382673\"}\n  ]\n}\n";
        let t = twin(text, Some(Path::new("data/o.json")), &Nothing, 42, 2).unwrap();
        assert_eq!(t.total, 3);
        // The first record, then the one with a note and another layout.
        assert_eq!(t.records, vec![1, 2]);
        let v: serde_json::Value = serde_json::from_str(&t.text).unwrap();
        let orders = v["orders"].as_array().unwrap();
        assert_eq!(orders.len(), 2);
        assert!(orders[0]["note"].is_null());
        for value in [
            "Amelia",
            "Quenanlek",
            "Kenji",
            "7306743338619943",
            "382673",
            "call first",
        ] {
            assert!(!t.text.contains(value), "{value} in {}", t.text);
        }
        assert!(t.text.starts_with("{\n  \"month\": \""), "{}", t.text);
        let ts = orders[0]["ts"].as_str().unwrap();
        assert_eq!(
            dates::recognize(ts).unwrap().picture(),
            "yyyy-MM-dd HH:mm:ss.SSSSSS"
        );
        // Deterministic per seed, and a larger sample starts the same way.
        let again = twin(text, None, &Nothing, 42, 2).unwrap();
        assert_eq!(again.text, t.text);
        let other = twin(text, None, &Nothing, 43, 2).unwrap();
        assert_ne!(other.text, t.text);
        let three = twin(text, None, &Nothing, 42, 3).unwrap();
        let v3: serde_json::Value = serde_json::from_str(&three.text).unwrap();
        assert_eq!(v3["orders"][0], orders[0]);
        // Equal values get equal fakes.
        assert_eq!(v3["orders"][0]["name"], v3["orders"][2]["name"]);
    }

    #[test]
    fn csv_twins_keep_the_header_quoting_and_embedded_delimiters() {
        let text = "id,name,note\r\n1,\"Stone, Ann\",\"said \"\"hi\"\"\"\r\n2,Bo Li,plain\r\n";
        let t = twin(text, Some(Path::new("data/c.csv")), &Nothing, 1, 5).unwrap();
        let rows = parse::delimited(&t.text, ',').unwrap();
        assert_eq!(rows.len(), 3);
        assert!(t.text.starts_with("id,name,note\r\n"));
        assert_eq!(t.verbatim, vec!["id,name,note".to_owned()]);
        assert!(rows[1].cells[1].quoted && rows[1].cells[1].value.contains(", "));
        assert!(rows[1].cells[2].value.contains('"'));
        assert!(!t.text.contains("Stone") && !t.text.contains("hi\"\""));
    }

    #[test]
    fn env_twins_drop_comments_and_keep_keys() {
        let text = "# prod creds for alice\nexport API_KEY=\"sk_live_Qm8vT2xW9pL4nR7kZ3cY\"\nLIMIT=500\n\nURL=postgres://app:pw@db:5432/x\n";
        let t = twin(text, Some(Path::new(".env")), &Nothing, 9, 5).unwrap();
        assert!(
            !t.text.contains("alice") && !t.text.contains("Qm8v"),
            "{}",
            t.text
        );
        assert!(t.text.contains("export API_KEY=\""), "{}", t.text);
        assert!(t.text.contains("URL=postgres://"), "{}", t.text);
        let limit = t.text.lines().find(|l| l.starts_with("LIMIT=")).unwrap();
        assert_eq!(limit.len(), "LIMIT=500".len());
    }

    #[test]
    fn logs_have_no_twin() {
        let log = "2026-09-01T00:00:09Z INFO a\n2026-09-01T00:00:10Z INFO b\n";
        assert!(twin(log, Some(Path::new("logs/a.log")), &Nothing, 1, 5).is_err());
    }
}
