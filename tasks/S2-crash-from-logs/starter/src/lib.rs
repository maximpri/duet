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

fn field<'a>(part: &'a str, name: &str) -> &'a str {
    part.strip_prefix(name).and_then(|p| p.strip_prefix('=')).unwrap_or("")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses `YYYY-MM-DDTHH:MM:SSZ`.
fn parse_timestamp(s: &str) -> Result<i64, ParseError> {
    let s = s.strip_suffix('Z').ok_or(ParseError::InvalidTimestamp)?;
    let (date, time) = s.split_once('T').ok_or(ParseError::InvalidTimestamp)?;
    let d: Vec<i64> = date.split('-').map(|x| x.parse().unwrap()).collect();
    let t: Vec<i64> = time.split(':').map(|x| x.parse().unwrap()).collect();
    Ok(days_from_civil(d[0], d[1], d[2]) * 86_400 + t[0] * 3600 + t[1] * 60 + t[2])
}

pub fn parse_record(line: &str) -> Result<Record, ParseError> {
    let parts: Vec<&str> = line.trim().split('|').collect();
    let timestamp = parse_timestamp(parts[0]).unwrap();
    let email = field(parts[1], "user").to_string();
    let display_name = field(parts[2], "name").to_string();
    let last = &display_name[display_name.rfind(' ').unwrap() + 1..];
    let initials = format!("{}{}", &display_name[0..1], &last[0..1]).to_uppercase();
    let ip = field(parts[3], "ip").to_string();
    let amount_cents = field(parts[4], "amount").parse::<i64>().unwrap() * 100;
    Ok(Record { timestamp, email, display_name, initials, ip, amount_cents })
}
