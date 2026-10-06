// SPDX-License-Identifier: GPL-3.0-or-later
//! Personal data beyond US formats: international phone numbers (E.164, and
//! national numbers labelled as phone numbers), IBANs of every registry
//! country (length and mod-97 check), the main EU/UK national identifiers
//! with their check digits (UK NINO, DE Steuer-ID, FR NIR, ES DNI/NIE, IT
//! codice fiscale, NL BSN), IPv6 addresses, and postal addresses in labelled
//! fields.
//!
//! A format with a check is found only when the check holds. A format without
//! one (NINO) relies on its structure; one whose check lets one number in
//! eleven through (BSN, nine plain digits) counts only with its label nearby.

use crate::detect::{Finding, Kind};
use regex::{Regex, RegexSet};
use std::net::Ipv6Addr;
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

/// The detectors, in [`PATTERNS`] order.
#[derive(Clone, Copy)]
enum D {
    E164,
    PhoneLabelled,
    IbanCompact,
    IbanGrouped,
    Nino,
    SteuerId,
    Nir,
    Dni,
    Nie,
    CodiceFiscale,
    Bsn,
    Ipv6,
    Address,
}

/// One expression per detector. A set over all of them finds, in one pass,
/// which can match at all; only those run on their own (most text holds none).
const PATTERNS: [&str; 13] = [
    // `+` country code, then digit groups with single separators.
    r"\+[1-9][0-9]{0,2}(?:[ .\-]?\(?[0-9]{1,5}\)?){1,7}",
    // A national number after a phone label (`Tel.: 030 1234567`, `"mobile": "07700 900123"`).
    r#"(?i)\b(?:phone|tel|telephone|mobile|mob|cell|fax|telefon|telefono|teléfono|téléphone|cellulare|handy|mobil|portable|gsm)(?:[ _.-]?(?:no|nr|number|num|nummer|numero|número))?\.?["']?\s*[:=]?\s*["']?(\(?\+?[0-9][0-9 ()./-]{5,20}[0-9])"#,
    r"\b[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}\b",
    r"\b[A-Z]{2}[0-9]{2}(?: [A-Z0-9]{4}){2,7}(?: [A-Z0-9]{1,3})?\b",
    r"\b[A-CEGHJ-PR-TW-Z][A-CEGHJ-NPR-TW-Z] ?[0-9]{2} ?[0-9]{2} ?[0-9]{2} ?[A-D]\b",
    r"\b[1-9][0-9](?: ?[0-9]{3}){3}\b",
    r"\b[1-478] ?[0-9]{2} ?(?:0[1-9]|1[0-2]|[2-9][0-9]) ?(?:[0-9]{2}|2[AaBb]) ?[0-9]{3} ?[0-9]{3} ?[0-9]{2}\b",
    r"\b[0-9]{8}[ -]?[A-Z]\b",
    r"\b[XYZ][ -]?[0-9]{7}[ -]?[A-Z]\b",
    r"(?i)\b[A-Z]{6}[0-9LMNPQRSTUV]{2}[ABCDEHLMPRST][0-9LMNPQRSTUV]{2}[A-Z][0-9LMNPQRSTUV]{3}[A-Z]\b",
    r"\b(?:[0-9]{9}|[0-9]{4}\.[0-9]{2}\.[0-9]{3}|[0-9]{3} [0-9]{3} [0-9]{3})\b",
    // Eight groups, or fewer around `::`.
    r"(?i)(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}|[0-9a-f:]*::[0-9a-f:.]*",
    // A postal-address field and its value on the same line.
    r#"(?i)\b(?:(?:postal|mailing|home|billing|shipping|delivery|street|residential|correspondence|business|office|private|registered)[ _-]?address|address(?:[ _-]?line)?(?:[ _-]?[12])?|addr|street|anschrift|adresse|wohnadresse|straße|strasse|dirección|direccion|domicilio|indirizzo|adres|woonadres)["']?\s*[:=]\s*["']?([^\n"'`;|{}<>\\]{6,160})"#,
];

static SET: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new(PATTERNS).expect("static regex set"));
static EACH: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    PATTERNS
        .iter()
        .map(|p| Regex::new(p).expect("static regex"))
        .collect()
});

re!(
    BSN_LABEL,
    r"(?i)\b(?:bsn|burgerservicenummer|sofi[ -]?nummer|citizen\s+service\s+number)\b"
);
re!(WORD_RUN, r"\w+");
re!(LETTER_WORD, r"\p{L}{2,}");
// An address field named for something else (`ip address`, `email-address`).
re!(
    NOT_POSTAL,
    r"(?i)\b(?:ip|mac|e-?mail|bind|listen|server|remote|local|memory|mem|base|wallet|contract|web|url|host|return|load|virtual|physical|broadcast|network|net|gateway|proxy|target|source|start|end)[ ._-]?$"
);

/// Words a label may stand from the number it names (as for labelled numbers).
const LABEL_REACH: usize = 3;
const LABEL_WINDOW: usize = 64;

/// A check of a candidate's digits or letters.
type Check = fn(&str) -> bool;

/// Every international personal-data finding in `text` (a span two
/// detectors find is reported once).
pub(crate) fn scan(text: &str, out: &mut Vec<Finding>) {
    let mut found = Vec::new();
    scan_into(text, &mut found);
    found.sort_by_key(|f: &Finding| (f.start, std::cmp::Reverse(f.end)));
    found.dedup_by(|a, b| (a.kind, a.start, a.end) == (b.kind, b.start, b.end));
    out.extend(found);
}

fn scan_into(text: &str, out: &mut Vec<Finding>) {
    let hits = SET.matches(text);
    if !hits.matched_any() {
        return;
    }
    let on = |d: D| hits.matched(d as usize).then(|| &EACH[d as usize]);
    let mut push = |kind, start, end| {
        out.push(Finding {
            kind,
            start,
            end,
            label: None,
        })
    };
    for m in on(D::E164).into_iter().flat_map(|re| re.find_iter(text)) {
        if phone_ok(text, m.start(), m.as_str()) {
            push(Kind::Phone, m.start(), m.end());
        }
    }
    for c in on(D::PhoneLabelled)
        .into_iter()
        .flat_map(|re| re.captures_iter(text))
    {
        if let Some(m) = c.get(1) {
            let digits = m.as_str().bytes().filter(u8::is_ascii_digit).count();
            if (7..=15).contains(&digits) {
                push(Kind::Phone, m.start(), m.end());
            }
        }
    }
    for d in [D::IbanCompact, D::IbanGrouped] {
        for m in on(d).into_iter().flat_map(|re| re.find_iter(text)) {
            if let Some(end) = iban_end(m.as_str()) {
                push(Kind::Iban, m.start(), m.start() + end);
            }
        }
    }
    let ids: [(D, Check); 6] = [
        (D::Nino, nino_valid),
        (D::SteuerId, steuer_id_valid),
        (D::Nir, nir_valid),
        (D::Dni, dni_valid),
        (D::Nie, nie_valid),
        (D::CodiceFiscale, codice_fiscale_valid),
    ];
    for (d, valid) in ids {
        for m in on(d).into_iter().flat_map(|re| re.find_iter(text)) {
            if standalone(text, m.start(), m.end()) && valid(m.as_str()) {
                push(Kind::NationalId, m.start(), m.end());
            }
        }
    }
    for m in on(D::Bsn).into_iter().flat_map(|re| re.find_iter(text)) {
        if standalone(text, m.start(), m.end())
            && bsn_valid(m.as_str())
            && label_near(text, m.start(), m.end(), &BSN_LABEL)
        {
            push(Kind::NationalId, m.start(), m.end());
        }
    }
    for m in on(D::Ipv6).into_iter().flat_map(|re| re.find_iter(text)) {
        if let Some(end) = ipv6_end(text, m.start(), m.end()) {
            push(Kind::Ip, m.start(), end);
        }
    }
    for c in on(D::Address)
        .into_iter()
        .flat_map(|re| re.captures_iter(text))
    {
        let (Some(whole), Some(v)) = (c.get(0), c.get(1)) else {
            continue;
        };
        if NOT_POSTAL.is_match(tail(text, whole.start(), 24)) {
            continue;
        }
        let value = v.as_str().trim_end_matches([' ', '\t', '\r', ',', '.']);
        if postal_value(value) {
            push(Kind::Address, v.start(), v.start() + value.len());
        }
    }
}

/// Up to `bytes` of `text` before `at`.
fn tail(text: &str, at: usize, bytes: usize) -> &str {
    let mut from = at.saturating_sub(bytes);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..at]
}

/// A number standing alone: not part of a decimal, a range or a longer code.
fn standalone(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let mut after = text[end..].chars();
    let next = after.next();
    let next2 = after.next();
    !before.is_some_and(|c| matches!(c, '.' | ',' | '+' | '/' | '-' | '_'))
        && !(matches!(next, Some('.' | ',' | '-' | '/'))
            && next2.is_some_and(|c| c.is_ascii_digit()))
}

/// A label matching `label` within [`LABEL_REACH`] words before or after `start..end`.
fn label_near(text: &str, start: usize, end: usize, label: &Regex) -> bool {
    let mut from = start.saturating_sub(LABEL_WINDOW);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let before = &text[from..start];
    if label
        .find_iter(before)
        .any(|m| WORD_RUN.find_iter(&before[m.end()..]).count() <= LABEL_REACH)
    {
        return true;
    }
    let mut to = (end + LABEL_WINDOW).min(text.len());
    while !text.is_char_boundary(to) {
        to -= 1;
    }
    let after = &text[end..to];
    label
        .find_iter(after)
        .any(|m| WORD_RUN.find_iter(&after[..m.start()]).count() <= LABEL_REACH)
}

// ---- phone numbers

/// Assigned country calling codes with two digits.
const CC2: &[u16] = &[
    20, 27, 30, 31, 32, 33, 34, 36, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 51, 52, 53, 54, 55, 56,
    57, 58, 60, 61, 62, 63, 64, 65, 66, 81, 82, 84, 86, 90, 91, 92, 93, 94, 95, 98,
];

/// Assigned country calling codes with three digits.
const CC3: &[u16] = &[
    211, 212, 213, 216, 218, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233,
    234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252,
    253, 254, 255, 256, 257, 258, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 290, 291, 297,
    298, 299, 350, 351, 352, 353, 354, 355, 356, 357, 358, 359, 370, 371, 372, 373, 374, 375, 376,
    377, 378, 379, 380, 381, 382, 383, 385, 386, 387, 389, 420, 421, 423, 500, 501, 502, 503, 504,
    505, 506, 507, 508, 509, 590, 591, 592, 593, 594, 595, 596, 597, 598, 599, 670, 672, 673, 674,
    675, 676, 677, 678, 679, 680, 681, 682, 683, 685, 686, 687, 688, 689, 690, 691, 692, 800, 808,
    850, 852, 853, 855, 856, 870, 878, 880, 881, 882, 883, 886, 888, 960, 961, 962, 963, 964, 965,
    966, 967, 968, 970, 971, 972, 973, 974, 975, 976, 977, 979, 992, 993, 994, 995, 996, 998,
];

/// National number lengths (digits after the country code) of the larger
/// numbering plans; other codes take any total of 8 to 15 digits.
const NATIONAL_LENGTHS: &[(u16, u8, u8)] = &[
    (1, 10, 10),
    (7, 10, 10),
    (20, 8, 10),
    (27, 9, 9),
    (30, 10, 10),
    (31, 9, 9),
    (32, 8, 9),
    (33, 9, 9),
    (34, 9, 9),
    (36, 8, 9),
    (39, 6, 11),
    (40, 9, 9),
    (41, 9, 9),
    (43, 4, 13),
    (44, 9, 10),
    (45, 8, 8),
    (46, 7, 13),
    (47, 8, 8),
    (48, 9, 9),
    (49, 6, 13),
    (52, 10, 10),
    (54, 10, 11),
    (55, 10, 11),
    (61, 9, 9),
    (62, 8, 12),
    (63, 10, 10),
    (64, 8, 10),
    (65, 8, 8),
    (81, 9, 10),
    (82, 8, 10),
    (84, 9, 10),
    (86, 10, 11),
    (90, 10, 10),
    (91, 10, 10),
    (351, 9, 9),
    (353, 7, 9),
    (358, 5, 12),
    (380, 9, 9),
    (420, 9, 9),
    (421, 9, 9),
    (966, 9, 9),
    (971, 8, 9),
    (972, 8, 9),
];

/// The country code at the start of `digits` and its length.
fn country_code(digits: &str) -> Option<(u16, usize)> {
    let prefix = |n: usize| digits.get(..n).and_then(|p| p.parse::<u16>().ok());
    match prefix(1)? {
        c @ (1 | 7) => Some((c, 1)),
        _ => match prefix(2) {
            Some(c) if CC2.contains(&c) => Some((c, 2)),
            _ => prefix(3).filter(|c| CC3.contains(c)).map(|c| (c, 3)),
        },
    }
}

/// An E.164 number (`+44 20 7946 0958`) at `start`: a known country code,
/// the plan's national length, and not glued to other text (a version's build
/// metadata `1.0+20240101`, a time zone `12:00+0100`).
fn phone_ok(text: &str, start: usize, number: &str) -> bool {
    let before = text[..start].chars().next_back();
    if before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | '+' | '/' | '-' | '_' | ':'))
    {
        // `tel:+44…` is the one glued form that is a phone number.
        if !text[..start].to_ascii_lowercase().ends_with("tel:") {
            return false;
        }
    }
    let after = &text[start + number.len()..];
    if after.chars().next().is_some_and(|c| c.is_alphanumeric())
        || (after.starts_with(['.', ',']) && after[1..].starts_with(|c: char| c.is_ascii_digit()))
    {
        return false;
    }
    let digits: String = number.chars().filter(char::is_ascii_digit).collect();
    if !(8..=15).contains(&digits.len()) {
        return false;
    }
    // `+1 2 3 4 5 6 7 8` is a list, not a number.
    let single = number[1..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| g.len() == 1)
        .count();
    if single > 2 {
        return false;
    }
    let Some((cc, len)) = country_code(&digits) else {
        return false;
    };
    let national = (digits.len() - len) as u8;
    match NATIONAL_LENGTHS.iter().find(|(c, _, _)| *c == cc) {
        Some((_, min, max)) => (*min..=*max).contains(&national),
        None => national >= 6,
    }
}

// ---- IBAN

/// IBAN length by country (SWIFT IBAN registry).
const IBAN_LENGTHS: &[(&str, usize)] = &[
    ("AD", 24),
    ("AE", 23),
    ("AL", 28),
    ("AT", 20),
    ("AZ", 28),
    ("BA", 20),
    ("BE", 16),
    ("BG", 22),
    ("BH", 22),
    ("BI", 27),
    ("BR", 29),
    ("BY", 28),
    ("CH", 21),
    ("CR", 22),
    ("CY", 28),
    ("CZ", 24),
    ("DE", 22),
    ("DJ", 27),
    ("DK", 18),
    ("DO", 28),
    ("EE", 20),
    ("EG", 29),
    ("ES", 24),
    ("FI", 18),
    ("FK", 18),
    ("FO", 18),
    ("FR", 27),
    ("GB", 22),
    ("GE", 22),
    ("GI", 23),
    ("GL", 18),
    ("GR", 27),
    ("GT", 28),
    ("HN", 28),
    ("HR", 21),
    ("HU", 28),
    ("IE", 22),
    ("IL", 23),
    ("IQ", 23),
    ("IS", 26),
    ("IT", 27),
    ("JO", 30),
    ("KW", 30),
    ("KZ", 20),
    ("LB", 28),
    ("LC", 32),
    ("LI", 21),
    ("LT", 20),
    ("LU", 20),
    ("LV", 21),
    ("LY", 25),
    ("MC", 27),
    ("MD", 24),
    ("ME", 22),
    ("MK", 19),
    ("MN", 20),
    ("MR", 27),
    ("MT", 31),
    ("MU", 30),
    ("NI", 28),
    ("NL", 18),
    ("NO", 15),
    ("OM", 23),
    ("PK", 24),
    ("PL", 28),
    ("PS", 29),
    ("PT", 25),
    ("QA", 29),
    ("RO", 24),
    ("RS", 22),
    ("RU", 33),
    ("SA", 24),
    ("SC", 31),
    ("SD", 18),
    ("SE", 24),
    ("SI", 19),
    ("SK", 24),
    ("SM", 27),
    ("SO", 23),
    ("ST", 25),
    ("SV", 28),
    ("TL", 23),
    ("TN", 24),
    ("TR", 26),
    ("UA", 29),
    ("VA", 22),
    ("VG", 24),
    ("XK", 20),
    ("YE", 30),
];

/// The registry length of IBANs of `country`, if it is a registry country.
pub fn iban_length(country: &str) -> Option<usize> {
    IBAN_LENGTHS
        .iter()
        .find(|(c, _)| *c == country)
        .map(|(_, n)| *n)
}

/// Whether `iban` (compact, upper case) passes the ISO 13616 mod-97 check.
pub fn iban_checksum(iban: &str) -> bool {
    if iban.len() < 5 || !iban.is_ascii() {
        return false;
    }
    let mut rem: u64 = 0;
    for c in iban[4..].chars().chain(iban[..4].chars()) {
        let v = match c {
            '0'..='9' => u64::from(c as u8 - b'0'),
            'A'..='Z' => u64::from(c as u8 - b'A') + 10,
            _ => return false,
        };
        rem = if v >= 10 { rem * 100 + v } else { rem * 10 + v } % 97;
    }
    rem == 1
}

/// The end (a byte offset into `candidate`, printed with or without spaces) of
/// the IBAN it starts with: the registry length for its country (any length
/// for a country outside the registry) and a valid check.
fn iban_end(candidate: &str) -> Option<usize> {
    let compact: String = candidate.chars().filter(|c| *c != ' ').collect();
    let want = match iban_length(&compact[..2]) {
        Some(n) if compact.len() >= n => n,
        Some(_) => return None,
        None if (15..=34).contains(&compact.len()) => compact.len(),
        None => return None,
    };
    if !iban_checksum(&compact[..want]) {
        return None;
    }
    // Map the compact length back to the printed form.
    let mut seen = 0;
    for (i, c) in candidate.char_indices() {
        if c != ' ' {
            seen += 1;
            if seen == want {
                let end = i + c.len_utf8();
                // A cut inside a printed group is not an IBAN.
                return candidate[end..]
                    .chars()
                    .next()
                    .is_none_or(|n| n == ' ')
                    .then_some(end);
            }
        }
    }
    None
}

// ---- national identifiers

/// UK National Insurance number: structure only (no check digit); prefixes
/// never issued are refused.
pub fn nino_valid(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| *c != ' ').collect();
    !["BG", "GB", "KN", "NK", "NT", "TN", "ZZ"].contains(&&compact[..2])
}

fn digits_of(s: &str) -> Vec<u32> {
    s.chars().filter_map(|c| c.to_digit(10)).collect()
}

/// German tax identification number (Steuer-ID): 11 digits, first not 0;
/// among the first ten, one digit occurs twice or three times (never three in a
/// row) and the others at most once; ISO 7064 MOD 11,10 check digit.
pub fn steuer_id_valid(s: &str) -> bool {
    let d = digits_of(s);
    if d.len() != 11 || d[0] == 0 {
        return false;
    }
    let mut counts = [0u8; 10];
    for &x in &d[..10] {
        counts[x as usize] += 1;
    }
    let repeated: Vec<u8> = counts.iter().copied().filter(|&n| n > 1).collect();
    if repeated.len() != 1 || repeated[0] > 3 {
        return false;
    }
    if repeated[0] == 3 && d[..10].windows(3).any(|w| w[0] == w[1] && w[1] == w[2]) {
        return false;
    }
    let mut product = 10;
    for &x in &d[..10] {
        let mut sum = (x + product) % 10;
        if sum == 0 {
            sum = 10;
        }
        product = (sum * 2) % 11;
    }
    let check = (11 - product) % 10;
    check == d[10]
}

/// French social security number (NIR, 13 digits and a 2-digit key): the key
/// is 97 minus the number modulo 97; Corsican departments 2A and 2B count as 19
/// and 18.
pub fn nir_valid(s: &str) -> bool {
    let compact: String = s
        .chars()
        .filter(|c| *c != ' ')
        .collect::<String>()
        .to_ascii_uppercase();
    if compact.len() != 15 {
        return false;
    }
    let body = compact[..13].replace("2A", "19").replace("2B", "18");
    let (Ok(n), Ok(key)) = (body.parse::<u64>(), compact[13..].parse::<u64>()) else {
        return false;
    };
    (1..=97).contains(&key) && 97 - n % 97 == key
}

const DNI_LETTERS: &[u8; 23] = b"TRWAGMYFPDXBNJZSQVHLCKE";

fn dni_letter_ok(number: u32, letter: char) -> bool {
    char::from(DNI_LETTERS[(number % 23) as usize]) == letter
}

/// Spanish DNI: 8 digits and the check letter of the number modulo 23.
pub fn dni_valid(s: &str) -> bool {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    match (digits.parse::<u32>(), s.chars().last()) {
        (Ok(n), Some(letter)) if digits.len() == 8 => dni_letter_ok(n, letter),
        _ => false,
    }
}

/// Spanish NIE: X, Y or Z (read as 0, 1, 2), 7 digits and a DNI check letter.
pub fn nie_valid(s: &str) -> bool {
    let mut chars = s.chars();
    let lead = match chars.next() {
        Some('X') => 0,
        Some('Y') => 1,
        Some('Z') => 2,
        _ => return false,
    };
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    match (digits.parse::<u32>(), s.chars().last()) {
        (Ok(n), Some(letter)) if digits.len() == 7 => dni_letter_ok(lead * 10_000_000 + n, letter),
        _ => false,
    }
}

/// Italian codice fiscale: 15 characters and a check letter from the sum of
/// odd-position values (a fixed table) and even-position values, modulo 26.
pub fn codice_fiscale_valid(s: &str) -> bool {
    const ODD: [u32; 26] = [
        1, 0, 5, 7, 9, 13, 15, 17, 19, 21, 2, 4, 18, 20, 11, 3, 6, 8, 12, 14, 16, 10, 22, 25, 24,
        23,
    ];
    let upper = s.to_ascii_uppercase();
    let b = upper.as_bytes();
    if b.len() != 16 {
        return false;
    }
    let mut sum = 0;
    for (i, &c) in b[..15].iter().enumerate() {
        let index = match c {
            b'0'..=b'9' => u32::from(c - b'0'),
            b'A'..=b'Z' => u32::from(c - b'A'),
            _ => return false,
        };
        // Positions count from one: odd positions are even indices.
        sum += if i % 2 == 0 {
            ODD[index as usize]
        } else {
            index
        };
    }
    u32::from(b[15]) == u32::from(b'A') + sum % 26
}

/// Dutch citizen service number (BSN): 9 digits passing the eleven test
/// (weights 9 to 2, and -1 for the last digit).
pub fn bsn_valid(s: &str) -> bool {
    let d = digits_of(s);
    if d.len() != 9 || d.iter().all(|&x| x == 0) {
        return false;
    }
    let sum: i64 = d[..8]
        .iter()
        .zip((2..=9).rev())
        .map(|(&x, w)| i64::from(x) * w)
        .sum::<i64>()
        - i64::from(d[8]);
    sum % 11 == 0
}

// ---- IPv6

/// The end of the IPv6 address at `start..end`, if it is one worth
/// withholding: it parses, stands alone, is neither loopback nor unspecified,
/// and has three or more groups (so `dead::beef` in code is not one).
fn ipv6_end(text: &str, start: usize, end: usize) -> Option<usize> {
    let glued = |c: char| c.is_alphanumeric() || matches!(c, ':' | '.' | '_');
    if text[..start].chars().next_back().is_some_and(glued) {
        return None;
    }
    let candidate = text[start..end].trim_end_matches('.');
    let end = start + candidate.len();
    if text[end..]
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let addr: Ipv6Addr = candidate.parse().ok()?;
    // An embedded IPv4 address (`::ffff:192.0.2.1`) stands for two groups.
    let groups = candidate
        .split(':')
        .filter(|g| !g.is_empty())
        .map(|g| if g.contains('.') { 2 } else { 1 })
        .sum::<usize>();
    (groups >= 3 && !addr.is_loopback() && !addr.is_unspecified()).then_some(end)
}

// ---- postal addresses

/// The value of an address field reads like a postal address: a number (house
/// or postcode) and two or more words, not a host, a URL or a code expression.
fn postal_value(value: &str) -> bool {
    let v = value.trim();
    !v.starts_with(['⟨', '$', '%', '(', '[', '#', '@'])
        && !v.contains("://")
        && !v.contains("0x")
        && v.bytes().any(|b| b.is_ascii_digit())
        && LETTER_WORD.find_iter(v).count() >= 2
        && !v.contains('(')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<(Kind, String)> {
        let mut out = Vec::new();
        scan(text, &mut out);
        out.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
        out.into_iter()
            .map(|f| (f.kind, text[f.start..f.end].to_owned()))
            .collect()
    }

    fn only(text: &str, kind: Kind) -> Vec<String> {
        found(text)
            .into_iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, v)| v)
            .collect()
    }

    #[test]
    fn international_phone_numbers_of_major_regions() {
        for number in [
            "+44 20 7946 0958",
            "+49 30 12345678",
            "+33 1 23 45 67 89",
            "+34 912 345 678",
            "+39 06 1234 5678",
            "+31 20 123 4567",
            "+81 3-1234-5678",
            "+86 138 0013 8000",
            "+91 98765 43210",
            "+61 2 9876 5432",
            "+55 11 91234-5678",
            "+7 495 123-45-67",
            "+1 (415) 555-0199",
            "+353 1 234 5678",
            "+971 4 123 4567",
            "+442079460958",
        ] {
            let text = format!("call {number} today");
            assert_eq!(only(&text, Kind::Phone), vec![number.to_owned()], "{text}");
        }
        assert_eq!(
            only("<a href=\"tel:+442079460958\">", Kind::Phone),
            vec!["+442079460958".to_owned()]
        );
    }

    #[test]
    fn version_strings_timestamps_and_arithmetic_are_not_phone_numbers() {
        for text in [
            "version 1.0.0+20240101120000 built",
            "2026-09-25T10:15:32+05:30",
            "Date: Thu, 25 Sep 2026 10:15:32 +0100",
            "@@ -12,7 +12,8 @@ fn main()",
            "x = y +1234",
            "+1 2 3 4 5 6 7 8",
            "+44 20 79",
            "ratio +1.23456789",
            "+999 1234 5678",
            "+1 415 555 01",
        ] {
            assert!(
                only(text, Kind::Phone).is_empty(),
                "{text}: {:?}",
                found(text)
            );
        }
    }

    #[test]
    fn national_numbers_labelled_as_phones() {
        for (text, number) in [
            ("Tel.: 030 12345678", "030 12345678"),
            ("\"mobile\": \"07700 900123\"", "07700 900123"),
            ("Téléphone : 01 23 45 67 89", "01 23 45 67 89"),
            ("phone_number=(415) 555-0199", "(415) 555-0199"),
        ] {
            assert_eq!(only(text, Kind::Phone), vec![number.to_owned()], "{text}");
        }
        assert!(only("mobile app v2.14.3 released", Kind::Phone).is_empty());
        assert!(only("tel: 12", Kind::Phone).is_empty());
    }

    /// A synthetic IBAN of `country` (registry length) with a valid check.
    fn synthetic_iban(country: &str, len: usize) -> String {
        let bban: String = (0..len - 4)
            .map(|i| char::from(b"0123456789"[(i * 7 + 3) % 10]))
            .collect();
        let probe = format!("{country}00{bban}");
        let rearranged: String = probe[4..].chars().chain(probe[..4].chars()).collect();
        let mut rem: u64 = 0;
        for c in rearranged.chars() {
            let v = c.to_digit(36).unwrap() as u64;
            rem = if v >= 10 { rem * 100 + v } else { rem * 10 + v } % 97;
        }
        format!("{country}{:02}{bban}", 98 - rem)
    }

    #[test]
    fn ibans_of_every_registry_country_with_their_check() {
        for (country, len) in IBAN_LENGTHS {
            let iban = synthetic_iban(country, *len);
            assert_eq!(iban.len(), *len);
            assert_eq!(
                only(&format!("pay to {iban} now"), Kind::Iban),
                vec![iban.clone()]
            );
            // Printed in groups of four.
            let grouped = iban
                .as_bytes()
                .chunks(4)
                .map(|c| std::str::from_utf8(c).unwrap())
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(
                only(&format!("IBAN {grouped} ok"), Kind::Iban),
                vec![grouped.clone()],
                "{grouped}"
            );
            // A changed digit fails the check.
            let mut broken = iban.into_bytes();
            let last = broken.len() - 1;
            broken[last] = if broken[last] == b'1' { b'2' } else { b'1' };
            let broken = String::from_utf8(broken).unwrap();
            assert!(only(&broken, Kind::Iban).is_empty(), "{broken}");
        }
        // The wrong length for its country, even with a valid check.
        let de = synthetic_iban("DE", 22);
        let long = synthetic_iban("DE", 24);
        assert!(only(&long, Kind::Iban).is_empty(), "{long}");
        assert_eq!(only(&de, Kind::Iban), vec![de.clone()]);
        // A country outside the registry: the check alone.
        let other = synthetic_iban("DZ", 26);
        assert_eq!(only(&other, Kind::Iban), vec![other.clone()]);
        // The documentation example.
        assert_eq!(
            only("GB82 WEST 1234 5698 7654 32", Kind::Iban),
            vec!["GB82 WEST 1234 5698 7654 32".to_owned()]
        );
    }

    #[test]
    fn eu_and_uk_national_ids_with_their_checks() {
        for (valid, invalid) in [
            // UK NINO: structure; BG/GB/... and suffix E are never issued.
            ("AB 12 34 56 C", "BG 12 34 56 C"),
            ("CE123456D", "CE123456E"),
            // DE Steuer-ID.
            ("86095742719", "86095742718"),
            ("47 036 892 816", "47 036 892 815"),
            // FR NIR with key (and a Corsican department).
            ("1 84 12 76 451 089 46", "1 84 12 76 451 089 47"),
            ("2 90 06 2A 123 456 45", "2 90 06 2A 123 456 46"),
            // ES DNI and NIE.
            ("12345678Z", "12345678A"),
            ("X1234567L", "X1234567T"),
            // IT codice fiscale.
            ("RSSMRA85T10A562S", "RSSMRA85T10A562T"),
        ] {
            assert_eq!(
                only(&format!("id {valid}."), Kind::NationalId),
                vec![valid.to_owned()],
                "{valid}"
            );
            assert!(
                only(&format!("id {invalid}."), Kind::NationalId).is_empty(),
                "{invalid}: {:?}",
                found(invalid)
            );
        }
    }

    #[test]
    fn bsn_needs_its_label_and_the_eleven_test() {
        assert_eq!(
            only("BSN: 111222333", Kind::NationalId),
            vec!["111222333".to_owned()]
        );
        assert_eq!(
            only("burgerservicenummer 1234.56.782", Kind::NationalId),
            vec!["1234.56.782".to_owned()]
        );
        // Fails the eleven test; no label; a decimal.
        assert!(only("BSN: 111222334", Kind::NationalId).is_empty());
        assert!(only("order 111222333 shipped", Kind::NationalId).is_empty());
        assert!(only("BSN 0.111222333", Kind::NationalId).is_empty());
        assert!(bsn_valid("123456782") && !bsn_valid("123456781"));
    }

    #[test]
    fn ipv6_addresses_but_not_code_or_times() {
        for addr in [
            "2001:db8:85a3::8a2e:370:7334",
            "2001:0db8:85a3:0000:0000:8a2e:0370:7334",
            "fe80::1ff:fe23:4567:890a",
            "::ffff:192.0.2.128",
        ] {
            assert_eq!(
                only(&format!("from {addr}, then"), Kind::Ip),
                vec![addr.to_owned()],
                "{addr}"
            );
        }
        assert_eq!(
            only("listen [2001:db8::8:800:200c:417a]:8080", Kind::Ip),
            vec!["2001:db8::8:800:200c:417a".to_owned()]
        );
        for text in [
            "use std::io::Result;",
            "dead::beef",
            "bind ::1 and ::",
            "at 10:15:32 on",
            "mac 00:1a:2b:3c:4d:5e",
            "fe80::1",
            "a::b::c",
        ] {
            assert!(only(text, Kind::Ip).is_empty(), "{text}: {:?}", found(text));
        }
    }

    #[test]
    fn postal_addresses_in_labelled_fields() {
        for (text, value) in [
            (
                "address: 221B Baker Street, London NW1 6XE",
                "221B Baker Street, London NW1 6XE",
            ),
            (
                "\"shipping_address\": \"Hauptstraße 5, 10115 Berlin\",",
                "Hauptstraße 5, 10115 Berlin",
            ),
            (
                "Adresse : 12 rue de la Paix, 75002 Paris",
                "12 rue de la Paix, 75002 Paris",
            ),
            (
                "billing-address=Via Roma 10, 00184 Roma",
                "Via Roma 10, 00184 Roma",
            ),
            ("Address line 1: 4 Privet Drive", "4 Privet Drive"),
        ] {
            assert_eq!(only(text, Kind::Address), vec![value.to_owned()], "{text}");
        }
        for text in [
            "address: 0.0.0.0:8080",
            "listen_address = \"127.0.0.1:3000\"",
            "IP address: 203.0.113.9 and more words 42",
            "email address: kim at example dot com 12",
            "address: String,",
            "let address = format!(\"{}:{}\", host, port);",
            "address: see below",
            "address: https://example.com/3/a b",
            "address: 0x7ffd5a3c9e10 mapped here",
        ] {
            assert!(
                only(text, Kind::Address).is_empty(),
                "{text}: {:?}",
                found(text)
            );
        }
    }
}
