// SPDX-License-Identifier: GPL-3.0-or-later
//! Dates and times in values: recognized by their layout, described as a
//! picture (`yyyy-MM-dd HH:mm:ss.SSSSSS`, `yyyyMMddHHmmss`) and generated
//! afresh in exactly that layout for a synthetic twin (valid calendar
//! dates, same digit counts, separators, fraction length and zone form).

use super::twin::Rng;

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// How a word is capitalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Lower,
    Upper,
    Title,
}

fn case_of(word: &str) -> Case {
    if word.chars().all(|c| !c.is_lowercase()) {
        Case::Upper
    } else if word.chars().next().is_some_and(char::is_uppercase) {
        Case::Title
    } else {
        Case::Lower
    }
}

fn cased(word: &str, case: Case) -> String {
    match case {
        Case::Lower => word.to_lowercase(),
        Case::Upper => word.to_uppercase(),
        Case::Title => {
            let mut c = word.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Lit(String),
    Year4,
    Year2,
    /// Digits as written (1 or 2).
    Month(usize),
    /// A month's name, abbreviated to three letters or in full.
    MonthName {
        full: bool,
        case: Case,
    },
    Day(usize),
    /// Day or month: the value is valid either way round.
    DayOrMonth(usize),
    Weekday {
        full: bool,
        case: Case,
    },
    Hour(usize),
    Minute,
    Second,
    /// Fraction of a second, with this many digits.
    Frac(usize),
    /// A zone offset with its sign: `+02:00` (with a colon) or `-0200`.
    Offset {
        colon: bool,
    },
    /// `AM` / `pm`.
    Meridiem(Case),
}

/// A date or time layout, recognized in one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tk {
    Digits,
    Letters,
    Other,
}

fn tokens(value: &str) -> Vec<(Tk, &str)> {
    let mut out: Vec<(Tk, &str)> = Vec::new();
    let mut start = 0;
    let mut kind: Option<Tk> = None;
    for (i, c) in value.char_indices() {
        let k = if c.is_ascii_digit() {
            Tk::Digits
        } else if c.is_alphabetic() {
            Tk::Letters
        } else {
            Tk::Other
        };
        match kind {
            Some(prev) if prev == k && k != Tk::Other => {}
            Some(prev) => {
                out.push((prev, &value[start..i]));
                start = i;
                kind = Some(k);
            }
            None => kind = Some(k),
        }
    }
    if let Some(k) = kind {
        out.push((k, &value[start..]));
    }
    out
}

fn days_in(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

fn valid_date(y: u32, m: u32, d: u32) -> bool {
    (1..=12).contains(&m) && d >= 1 && d <= days_in(y, m)
}

fn num(s: &str) -> u32 {
    s.parse().unwrap_or(u32::MAX)
}

/// Whether `word` is a month's name in full (`Some(true)`) or abbreviated
/// to three letters (`Some(false)`; `May` reads as the abbreviation).
fn month_name(word: &str) -> Option<bool> {
    let w = word.to_lowercase();
    MONTHS.iter().find_map(|m| {
        if w.len() == 3 && m.starts_with(&w) {
            Some(false)
        } else if *m == w {
            Some(true)
        } else {
            None
        }
    })
}

fn weekday_name(word: &str) -> Option<bool> {
    let w = word.to_lowercase();
    WEEKDAYS.iter().find_map(|d| {
        if *d == w {
            Some(true)
        } else if w.len() == 3 && d.starts_with(&w) {
            Some(false)
        } else {
            None
        }
    })
}

/// A cursor over a value's tokens.
struct Cur<'a> {
    t: Vec<(Tk, &'a str)>,
    i: usize,
    parts: Vec<Part>,
}

impl<'a> Cur<'a> {
    fn peek(&self) -> Option<(Tk, &'a str)> {
        self.t.get(self.i).copied()
    }
    fn lit(&mut self, s: &str) -> bool {
        match self.peek() {
            Some((_, x)) if x == s => {
                self.parts.push(Part::Lit(s.to_owned()));
                self.i += 1;
                true
            }
            _ => false,
        }
    }
    fn digits(&mut self, lens: &[usize]) -> Option<&'a str> {
        match self.peek() {
            Some((Tk::Digits, d)) if lens.contains(&d.len()) => {
                self.i += 1;
                Some(d)
            }
            _ => None,
        }
    }
    fn done(&self) -> bool {
        self.i == self.t.len()
    }

    /// `HH:mm[:ss[.SSS]]`, then a zone or meridiem, at the cursor.
    fn time(&mut self) -> bool {
        let save = (self.i, self.parts.len());
        let ok = (|| {
            let h = self.digits(&[1, 2])?;
            if num(h) > 23 {
                return None;
            }
            self.parts.push(Part::Hour(h.len()));
            if !self.lit(":") {
                return None;
            }
            if num(self.digits(&[2])?) > 59 {
                return None;
            }
            self.parts.push(Part::Minute);
            if self.lit(":") {
                if num(self.digits(&[2])?) > 60 {
                    return None;
                }
                self.parts.push(Part::Second);
                if matches!(self.peek(), Some((Tk::Other, "." | ","))) {
                    let sep = self.peek()?.1;
                    self.i += 1;
                    let f = self.digits(&(1..=9).collect::<Vec<_>>())?;
                    self.parts.push(Part::Lit(sep.to_owned()));
                    self.parts.push(Part::Frac(f.len()));
                }
            }
            self.zone();
            Some(())
        })();
        if ok.is_none() {
            self.i = save.0;
            self.parts.truncate(save.1);
            return false;
        }
        true
    }

    /// An optional zone (`Z`, `+02:00`, `+0200`, ` UTC`) or meridiem.
    fn zone(&mut self) {
        let at = |c: &Self, k: usize| c.t.get(c.i + k).copied();
        match self.peek() {
            Some((Tk::Other, " ")) => {
                if let Some((Tk::Letters, w)) = at(self, 1) {
                    let lower = w.to_lowercase();
                    let part = match lower.as_str() {
                        "am" | "pm" => Part::Meridiem(case_of(w)),
                        "utc" | "gmt" => Part::Lit(w.to_owned()),
                        _ => return,
                    };
                    self.i += 2;
                    self.parts.push(Part::Lit(" ".into()));
                    self.parts.push(part);
                }
            }
            Some((Tk::Letters, "Z")) => {
                self.i += 1;
                self.parts.push(Part::Lit("Z".into()));
            }
            Some((Tk::Other, sign @ ("+" | "-"))) => {
                let colon = match (at(self, 1), at(self, 2), at(self, 3)) {
                    (Some((Tk::Digits, h)), Some((Tk::Other, ":")), Some((Tk::Digits, m)))
                        if h.len() == 2 && m.len() == 2 && num(h) <= 14 && num(m) <= 59 =>
                    {
                        true
                    }
                    (Some((Tk::Digits, hm)), _, _)
                        if hm.len() == 4 && num(&hm[..2]) <= 14 && num(&hm[2..]) <= 59 =>
                    {
                        false
                    }
                    _ => return,
                };
                let _ = sign;
                self.i += if colon { 4 } else { 2 };
                self.parts.push(Part::Offset { colon });
            }
            _ => {}
        }
    }
}

/// The date or time layout `value` is written in, if it is one.
pub fn recognize(value: &str) -> Option<Layout> {
    let v = value.trim();
    if v.len() != value.len() || v.is_empty() || v.len() > 40 {
        return None;
    }
    let t = tokens(v);
    let mut c = Cur {
        t,
        i: 0,
        parts: Vec::new(),
    };
    // Compact forms: `20260901`, `202609011628`, `20260901162824`,
    // `20260901T162824Z`.
    if let Some((Tk::Digits, d)) = c.peek()
        && matches!(d.len(), 8 | 12 | 14)
    {
        let (y, m, day) = (num(&d[..4]), num(&d[4..6]), num(&d[6..8]));
        let time_ok = d.len() == 8
            || (num(&d[8..10]) <= 23
                && num(&d[10..12]) <= 59
                && (d.len() == 12 || num(&d[12..14]) <= 60));
        if (1900..=2100).contains(&y) && valid_date(y, m, day) && time_ok {
            c.i += 1;
            c.parts.extend([Part::Year4, Part::Month(2), Part::Day(2)]);
            if d.len() >= 12 {
                c.parts.extend([Part::Hour(2), Part::Minute]);
            }
            if d.len() == 14 {
                c.parts.push(Part::Second);
                if matches!(c.peek(), Some((Tk::Other, "."))) {
                    c.i += 1;
                    let f = c.digits(&(1..=9).collect::<Vec<_>>())?;
                    c.parts.push(Part::Lit(".".into()));
                    c.parts.push(Part::Frac(f.len()));
                }
            }
            if d.len() == 8 && matches!(c.peek(), Some((Tk::Letters, "T"))) {
                c.i += 1;
                let hms = c.digits(&[6])?;
                if num(&hms[..2]) > 23 || num(&hms[2..4]) > 59 || num(&hms[4..]) > 60 {
                    return None;
                }
                c.parts.extend([
                    Part::Lit("T".into()),
                    Part::Hour(2),
                    Part::Minute,
                    Part::Second,
                ]);
                c.zone();
            }
            return c.done().then_some(Layout { parts: c.parts });
        }
        return None;
    }
    // A leading weekday: `Tue, 01 Sep 2026`, `Tuesday 1 September 2026`.
    if let Some((Tk::Letters, w)) = c.peek()
        && let Some(full) = weekday_name(w)
    {
        c.parts.push(Part::Weekday {
            full,
            case: case_of(w),
        });
        c.i += 1;
        let _ = c.lit(",");
        if !c.lit(" ") {
            return None;
        }
    }
    let date = date(&mut c);
    if date {
        if c.done() {
            return Some(Layout { parts: c.parts });
        }
        if !(c.lit("T") || c.lit(" ")) {
            return None;
        }
        if !c.time() {
            return None;
        }
    } else if !c.time() {
        return None;
    }
    c.done().then_some(Layout { parts: c.parts })
}

/// A date at the cursor (numeric, or with a month's name).
fn date(c: &mut Cur<'_>) -> bool {
    let save = (c.i, c.parts.len());
    let restore = |c: &mut Cur<'_>| {
        c.i = save.0;
        c.parts.truncate(save.1);
    };
    // yyyy-MM-dd, yyyy/MM/dd, yyyy.MM.dd, and yyyy-MM.
    if let Some(y) = c.digits(&[4]) {
        let y = num(y);
        if let Some((Tk::Other, sep @ ("-" | "/" | "."))) = c.peek() {
            c.i += 1;
            if let Some(m) = c.digits(&[1, 2]) {
                let mlen = m.len();
                let m = num(m);
                if c.peek() == Some((Tk::Other, sep)) {
                    c.i += 1;
                    if let Some(d) = c.digits(&[1, 2])
                        && valid_date(y, m, num(d))
                    {
                        c.parts.extend([
                            Part::Year4,
                            Part::Lit(sep.into()),
                            Part::Month(mlen),
                            Part::Lit(sep.into()),
                            Part::Day(d.len()),
                        ]);
                        return true;
                    }
                } else if sep != "." && (1..=12).contains(&m) && mlen == 2 && c.done() {
                    c.parts
                        .extend([Part::Year4, Part::Lit(sep.into()), Part::Month(2)]);
                    return true;
                }
            }
        }
        restore(c);
        return false;
    }
    // dd/MM/yyyy or MM/dd/yyyy (also with `-` and `.`), and dd Mon yyyy.
    if let Some(a) = c.digits(&[1, 2]) {
        if let Some((Tk::Other, sep @ ("-" | "/" | "."))) = c.peek() {
            c.i += 1;
            if let Some(b) = c.digits(&[1, 2])
                && c.peek() == Some((Tk::Other, sep))
            {
                c.i += 1;
                if let Some(y) = c.digits(&[4, 2]) {
                    let (na, nb) = (num(a), num(b));
                    let yy = if y.len() == 4 { num(y) } else { 2000 + num(y) };
                    let (pa, pb) = if valid_date(yy, nb, na) && valid_date(yy, na, nb) {
                        (Part::DayOrMonth(a.len()), Part::DayOrMonth(b.len()))
                    } else if valid_date(yy, nb, na) {
                        (Part::Day(a.len()), Part::Month(b.len()))
                    } else if valid_date(yy, na, nb) {
                        (Part::Month(a.len()), Part::Day(b.len()))
                    } else {
                        restore(c);
                        return false;
                    };
                    let year = if y.len() == 4 {
                        Part::Year4
                    } else {
                        Part::Year2
                    };
                    c.parts
                        .extend([pa, Part::Lit(sep.into()), pb, Part::Lit(sep.into()), year]);
                    return true;
                }
            }
            // dd-Mon-yyyy
            restore(c);
            let _ = c.digits(&[1, 2]);
        }
        let sep = match c.peek() {
            Some((Tk::Other, s @ (" " | "-"))) => s,
            _ => {
                restore(c);
                return false;
            }
        };
        c.i += 1;
        if let Some((Tk::Letters, m)) = c.peek()
            && let Some(full) = month_name(m)
            && (1..=31).contains(&num(a))
        {
            c.i += 1;
            if c.peek() == Some((Tk::Other, sep)) {
                c.i += 1;
                if c.digits(&[4]).is_some() {
                    c.parts.extend([
                        Part::Day(a.len()),
                        Part::Lit(sep.into()),
                        Part::MonthName {
                            full,
                            case: case_of(m),
                        },
                        Part::Lit(sep.into()),
                        Part::Year4,
                    ]);
                    return true;
                }
            }
        }
        restore(c);
        return false;
    }
    // Mon d, yyyy
    if let Some((Tk::Letters, m)) = c.peek()
        && let Some(full) = month_name(m)
    {
        c.i += 1;
        if c.peek() == Some((Tk::Other, " ")) {
            c.i += 1;
            if let Some(d) = c.digits(&[1, 2])
                && (1..=31).contains(&num(d))
            {
                let comma = c.peek() == Some((Tk::Other, ","));
                if comma {
                    c.i += 1;
                }
                if c.peek() == Some((Tk::Other, " ")) {
                    c.i += 1;
                    if c.digits(&[4]).is_some() {
                        c.parts.push(Part::MonthName {
                            full,
                            case: case_of(m),
                        });
                        c.parts.push(Part::Lit(" ".into()));
                        c.parts.push(Part::Day(d.len()));
                        if comma {
                            c.parts.push(Part::Lit(",".into()));
                        }
                        c.parts.push(Part::Lit(" ".into()));
                        c.parts.push(Part::Year4);
                        return true;
                    }
                }
            }
        }
    }
    restore(c);
    false
}

impl Layout {
    /// The layout as a picture: `yyyy-MM-dd'T'HH:mm:ss.SSSX`.
    pub fn picture(&self) -> String {
        let mut out = String::new();
        for p in &self.parts {
            match p {
                Part::Lit(s) if s.chars().any(char::is_alphabetic) => {
                    out.push('\'');
                    out.push_str(s);
                    out.push('\'');
                }
                Part::Lit(s) => out.push_str(s),
                Part::Year4 => out.push_str("yyyy"),
                Part::Year2 => out.push_str("yy"),
                Part::Month(1) => out.push('M'),
                Part::Month(_) => out.push_str("MM"),
                Part::MonthName { full: false, .. } => out.push_str("MMM"),
                Part::MonthName { full: true, .. } => out.push_str("MMMM"),
                Part::Day(1) => out.push('d'),
                Part::Day(_) => out.push_str("dd"),
                Part::DayOrMonth(1) => out.push('?'),
                Part::DayOrMonth(_) => out.push_str("??"),
                Part::Weekday { full: false, .. } => out.push_str("EEE"),
                Part::Weekday { full: true, .. } => out.push_str("EEEE"),
                Part::Hour(1) => out.push('H'),
                Part::Hour(_) => out.push_str("HH"),
                Part::Minute => out.push_str("mm"),
                Part::Second => out.push_str("ss"),
                Part::Frac(n) => out.push_str(&"S".repeat(*n)),
                Part::Offset { colon: true } => out.push_str("XXX"),
                Part::Offset { colon: false } => out.push_str("XX"),
                Part::Meridiem(_) => out.push('a'),
            }
        }
        out
    }

    /// What the layout holds: `date`, `date-time`, `time` or `year-month`.
    pub fn kind(&self) -> &'static str {
        let has = |f: fn(&Part) -> bool| self.parts.iter().any(f);
        let date = has(|p| matches!(p, Part::Day(_) | Part::DayOrMonth(_)));
        let time = has(|p| matches!(p, Part::Hour(_)));
        match (date, time) {
            (true, true) => "date-time",
            (true, false) => "date",
            (false, true) => "time",
            (false, false) => "year-month",
        }
    }

    /// A fresh value in this layout: a valid date between 2000 and 2030 and
    /// a time of day drawn at random, written with the same digit counts,
    /// separators, fraction length and zone form.
    pub fn generate(&self, rng: &mut Rng) -> String {
        let year = rng.range(2000, 2030) as u32;
        let full_name = self
            .parts
            .iter()
            .any(|p| matches!(p, Part::MonthName { full: true, .. }));
        // `May` in full reads back as an abbreviation: not drawn for a full name.
        let month = match rng.range(1, 11) as u32 {
            m if m >= 5 && full_name => m + 1,
            m if full_name => m,
            _ => rng.range(1, 12) as u32,
        };
        let mut out = String::new();
        let two = |n: u64| format!("{n:02}");
        // A 1-digit field keeps one digit; a 2-digit one is zero-padded.
        let fit = |rng: &mut Rng, width: usize, lo: u64, hi: u64| {
            if width == 1 {
                rng.range(lo, hi.min(9)).to_string()
            } else {
                two(rng.range(lo, hi))
            }
        };
        for p in &self.parts {
            match p {
                Part::Lit(s) => out.push_str(s),
                Part::Year4 => out.push_str(&year.to_string()),
                Part::Year2 => out.push_str(&two(u64::from(year % 100))),
                Part::Month(w) => {
                    let m = if *w == 1 {
                        u64::from(month.min(9))
                    } else {
                        u64::from(month)
                    };
                    out.push_str(&if *w == 1 { m.to_string() } else { two(m) });
                }
                Part::MonthName { full, case } => {
                    let name = MONTHS[(month - 1) as usize];
                    let name = if *full { name } else { &name[..3] };
                    out.push_str(&cased(name, *case));
                }
                Part::Day(w) => out.push_str(&fit(rng, *w, 1, 28)),
                Part::DayOrMonth(w) => out.push_str(&fit(rng, *w, 1, 12)),
                Part::Weekday { full, case } => {
                    let name = *rng.pick(&WEEKDAYS);
                    let name = if *full { name } else { &name[..3] };
                    out.push_str(&cased(name, *case));
                }
                Part::Hour(w) => {
                    let meridiem = self.parts.iter().any(|p| matches!(p, Part::Meridiem(_)));
                    let (lo, hi) = if meridiem { (1, 12) } else { (0, 23) };
                    out.push_str(&fit(rng, *w, lo, hi));
                }
                Part::Minute | Part::Second => out.push_str(&two(rng.range(0, 59))),
                Part::Frac(n) => {
                    for _ in 0..*n {
                        out.push(char::from(b'0' + rng.below(10) as u8));
                    }
                }
                Part::Offset { colon } => {
                    out.push(if rng.below(2) == 0 { '+' } else { '-' });
                    let h = two(rng.range(0, 12));
                    let m = if rng.below(4) == 0 { "30" } else { "00" };
                    out.push_str(&h);
                    if *colon {
                        out.push(':');
                    }
                    out.push_str(m);
                }
                Part::Meridiem(case) => {
                    let m = if rng.below(2) == 0 { "am" } else { "pm" };
                    out.push_str(&cased(m, *case));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(v: &str) -> Option<String> {
        recognize(v).map(|l| l.picture())
    }

    #[test]
    fn common_layouts_are_recognized_as_pictures() {
        for (value, want) in [
            ("2026-09-01", "yyyy-MM-dd"),
            ("2026-09-01T16:28:24Z", "yyyy-MM-dd'T'HH:mm:ss'Z'"),
            ("2026-09-01 16:28:27.382673", "yyyy-MM-dd HH:mm:ss.SSSSSS"),
            ("2026-09-01T16:28:24.5+02:00", "yyyy-MM-dd'T'HH:mm:ss.SXXX"),
            ("2026-09-01T16:28:24+0200", "yyyy-MM-dd'T'HH:mm:ssXX"),
            ("20260902060000", "yyyyMMddHHmmss"),
            ("20260902", "yyyyMMdd"),
            ("20260901T162824Z", "yyyyMMdd'T'HHmmss'Z'"),
            ("25/03/2026", "dd/MM/yyyy"),
            ("03/25/2026", "MM/dd/yyyy"),
            ("05/03/2026", "??/??/yyyy"),
            ("1.9.2026", "?.?.yyyy"),
            ("01 Sep 2026", "dd MMM yyyy"),
            ("Sep 1, 2026", "MMM d, yyyy"),
            (
                "Tue, 01 Sep 2026 10:00:00 GMT",
                "EEE, dd MMM yyyy HH:mm:ss 'GMT'",
            ),
            ("16:28", "HH:mm"),
            ("4:05 PM", "H:mm a"),
            ("2026-09", "yyyy-MM"),
        ] {
            assert_eq!(picture(value).as_deref(), Some(want), "{value}");
        }
        for not in [
            "2026",
            "7306743338619943",
            "12345678",
            "2026-13-01",
            "2026-02-30",
            "abc",
            "1.5",
            "10:75",
            "99999999999999",
            "FH-0907",
        ] {
            assert_eq!(picture(not), None, "{not}");
        }
    }

    #[test]
    fn generated_values_have_the_same_layout() {
        let mut rng = Rng::new(7);
        for value in [
            "2026-09-01T16:28:24Z",
            "2026-09-01 16:28:27.382673",
            "20260902060000",
            "05/03/2026",
            "Sep 1, 2026",
            "Tue, 01 Sep 2026 10:00:00 GMT",
            "4:05 PM",
            "2026-09-01T16:28:24+0200",
        ] {
            let layout = recognize(value).unwrap();
            for _ in 0..50 {
                let fresh = layout.generate(&mut rng);
                assert_eq!(
                    recognize(&fresh).map(|l| l.picture()),
                    Some(layout.picture()),
                    "{value} -> {fresh}"
                );
                // Same shape, except an offset's sign, which is drawn too.
                let signless = |v: &str| {
                    super::super::shape::shape(v, super::super::shape::Detail::Runs)
                        .replace('-', "+")
                };
                assert_eq!(signless(&fresh), signless(value), "{value} -> {fresh}");
            }
        }
    }
}
