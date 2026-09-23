//! Parser for payment event lines:
//! `<timestamp>|user=<email>|name=<display name>|ip=<address>|amount=<amount>`

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Seconds since the Unix epoch, UTC.
    pub timestamp: i64,
    pub email: String,
    pub display_name: String,
    /// Upper-case initials of the first and last word of the display name.
    pub initials: String,
    pub ip: String,
    /// Amount in cents.
    pub amount_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    MissingField(&'static str),
    InvalidTimestamp,
    InvalidAmount,
}

fn field<'a>(parts: &[&'a str], index: usize, name: &'static str) -> Result<&'a str, ParseError> {
    parts
        .get(index)
        .and_then(|p| p.strip_prefix(name))
        .and_then(|p| p.strip_prefix('='))
        .ok_or(ParseError::MissingField(name))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn numbers(s: &str, sep: char, n: usize) -> Result<Vec<i64>, ParseError> {
    let v: Vec<i64> = s
        .split(sep)
        .map(|x| x.parse().map_err(|_| ParseError::InvalidTimestamp))
        .collect::<Result<_, _>>()?;
    if v.len() == n { Ok(v) } else { Err(ParseError::InvalidTimestamp) }
}

/// Parses `YYYY-MM-DDTHH:MM:SS` followed by `Z` or `±HH:MM`.
fn parse_timestamp(s: &str) -> Result<i64, ParseError> {
    let (local, offset) = if let Some(rest) = s.strip_suffix('Z') {
        (rest, 0)
    } else {
        let idx = s.len().checked_sub(6).ok_or(ParseError::InvalidTimestamp)?;
        let (local, off) = (s.get(..idx), s.get(idx..));
        let (local, off) = local.zip(off).ok_or(ParseError::InvalidTimestamp)?;
        let sign = match off.as_bytes().first() {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => return Err(ParseError::InvalidTimestamp),
        };
        let hm = numbers(&off[1..], ':', 2)?;
        (local, sign * (hm[0] * 3600 + hm[1] * 60))
    };
    let (date, time) = local.split_once('T').ok_or(ParseError::InvalidTimestamp)?;
    let d = numbers(date, '-', 3)?;
    let t = numbers(time, ':', 3)?;
    if !(1..=12).contains(&d[1]) || !(1..=31).contains(&d[2]) || t[0] > 23 || t[1] > 59 || t[2] > 60 {
        return Err(ParseError::InvalidTimestamp);
    }
    Ok(days_from_civil(d[0], d[1], d[2]) * 86_400 + t[0] * 3600 + t[1] * 60 + t[2] - offset)
}

fn parse_cents(s: &str) -> Result<i64, ParseError> {
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() || frac.len() > 2 || !whole.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::InvalidAmount);
    }
    let whole: i64 = whole.parse().map_err(|_| ParseError::InvalidAmount)?;
    let frac: i64 = format!("{frac:0<2}").parse().map_err(|_| ParseError::InvalidAmount)?;
    Ok(whole * 100 + frac)
}

fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().and_then(|w| w.chars().next());
    let last = words.last().and_then(|w| w.chars().next());
    first.into_iter().chain(last).flat_map(char::to_uppercase).collect()
}

pub fn parse_record(line: &str) -> Result<Record, ParseError> {
    let parts: Vec<&str> = line.trim().split('|').collect();
    let timestamp = parse_timestamp(parts.first().copied().unwrap_or(""))?;
    let email = field(&parts, 1, "user")?.to_string();
    let display_name = field(&parts, 2, "name")?.to_string();
    let ip = field(&parts, 3, "ip")?.to_string();
    let amount_cents = parse_cents(field(&parts, 4, "amount")?)?;
    Ok(Record { timestamp, initials: initials(&display_name), email, display_name, ip, amount_cents })
}
