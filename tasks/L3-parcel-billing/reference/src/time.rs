//! Timestamps (seconds since the Unix epoch, UTC) and the timestamp formats
//! used by carrier feeds. Billing months are UTC calendar months.

use crate::error::{Error, Result};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month, day) of a day number from [`days_from_civil`].
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

impl Timestamp {
    pub fn from_civil(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Result<Timestamp> {
        if !(1..=12).contains(&mo) || d == 0 || d > days_in_month(y, mo) || h > 23 || mi > 59 || s > 60 {
            return Err(Error::value("date", format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")));
        }
        Ok(Timestamp(days_from_civil(y, mo, d) * 86_400 + (h * 3600 + mi * 60 + s.min(59)) as i64))
    }

    /// (year, month, day, hour, minute, second) in UTC.
    pub fn civil(self) -> (i64, u32, u32, u32, u32, u32) {
        let days = self.0.div_euclid(86_400);
        let secs = self.0.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        (y, m, d, (secs / 3600) as u32, ((secs % 3600) / 60) as u32, (secs % 60) as u32)
    }

    /// The UTC billing month, `YYYY-MM`.
    pub fn month_key(self) -> String {
        let (y, m, ..) = self.civil();
        format!("{y:04}-{m:02}")
    }

    pub fn date_string(self) -> String {
        let (y, m, d, ..) = self.civil();
        format!("{y:04}-{m:02}-{d:02}")
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, mo, d, h, mi, s) = self.civil();
        write!(f, "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    }
}

fn num(text: &str, whole: &str) -> Result<u32> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return Err(Error::value("timestamp", whole));
    }
    text.parse().map_err(|_| Error::value("timestamp", whole))
}

/// ISO-8601: `YYYY-MM-DDTHH:MM[:SS[.fff]]` followed by `Z`, `+HH:MM`, `-HH:MM`
/// or nothing (UTC). A space may replace the `T`.
///
/// An offset is applied: `2026-09-01T00:40:00+02:00` is `2026-08-31T22:40:00Z`.
pub fn parse_iso(text: &str) -> Result<Timestamp> {
    let t = text.trim();
    if t.len() < 16 {
        return Err(Error::value("timestamp", text));
    }
    let (date, rest) = t.split_at(10);
    let dp: Vec<&str> = date.split('-').collect();
    if dp.len() != 3 || !(rest.starts_with('T') || rest.starts_with(' ')) {
        return Err(Error::value("timestamp", text));
    }
    let rest = &rest[1..];
    let zone_at = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_at);
    let clock = clock.split('.').next().unwrap_or_default();
    let cp: Vec<&str> = clock.split(':').collect();
    if cp.len() < 2 || cp.len() > 3 {
        return Err(Error::value("timestamp", text));
    }
    let sec = if cp.len() == 3 { num(cp[2], text)? } else { 0 };
    let offset = if zone.is_empty() || zone == "Z" {
        0
    } else {
        parse_offset(zone).ok_or_else(|| Error::value("timestamp", text))?
    };
    let local = Timestamp::from_civil(
        num(dp[0], text)? as i64,
        num(dp[1], text)?,
        num(dp[2], text)?,
        num(cp[0], text)?,
        num(cp[1], text)?,
        sec,
    )?;
    Ok(Timestamp(local.0 - offset))
}

/// `+HH:MM`, `-HH:MM`, `+HHMM` or `+HH` as seconds east of UTC.
pub fn parse_offset(zone: &str) -> Option<i64> {
    let sign = match zone.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let body = zone[1..].replace(':', "");
    if !body.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m) = match body.len() {
        2 => (body.parse::<i64>().ok()?, 0),
        4 => (body[..2].parse::<i64>().ok()?, body[2..].parse::<i64>().ok()?),
        _ => return None,
    };
    if h > 14 || m > 59 {
        return None;
    }
    Some(sign * (h * 3600 + m * 60))
}

/// Seconds since the epoch, as text.
pub fn parse_epoch(text: &str) -> Result<Timestamp> {
    text.trim().parse::<i64>().map(Timestamp).map_err(|_| Error::value("timestamp", text))
}

/// `YYYYMMDDHHMMSS`, UTC.
pub fn parse_compact(text: &str) -> Result<Timestamp> {
    let t = text.trim();
    if t.len() != 14 {
        return Err(Error::value("timestamp", text));
    }
    Timestamp::from_civil(
        num(&t[0..4], text)? as i64,
        num(&t[4..6], text)?,
        num(&t[6..8], text)?,
        num(&t[8..10], text)?,
        num(&t[10..12], text)?,
        num(&t[12..14], text)?,
    )
}

/// `DD.MM.YYYY HH:MM`, UTC.
pub fn parse_dotted(text: &str) -> Result<Timestamp> {
    let t = text.trim();
    let (date, clock) = t.split_once(' ').ok_or_else(|| Error::value("timestamp", text))?;
    let dp: Vec<&str> = date.split('.').collect();
    let cp: Vec<&str> = clock.trim().split(':').collect();
    if dp.len() != 3 || cp.len() != 2 {
        return Err(Error::value("timestamp", text));
    }
    Timestamp::from_civil(
        num(dp[2], text)? as i64,
        num(dp[1], text)?,
        num(dp[0], text)?,
        num(cp[0], text)?,
        num(cp[1], text)?,
        0,
    )
}

/// Validates a billing month `YYYY-MM`.
pub fn parse_month(text: &str) -> Result<(i64, u32)> {
    let (y, m) = text.trim().split_once('-').ok_or_else(|| Error::value("month", text))?;
    let y: i64 = y.parse().map_err(|_| Error::value("month", text))?;
    let m: u32 = m.parse().map_err(|_| Error::value("month", text))?;
    if !(1..=12).contains(&m) || !(1970..=9999).contains(&y) {
        return Err(Error::value("month", text));
    }
    Ok((y, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-1_000_000, -1, 0, 1, 19_000, 20_697, 2_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 8, 31), 20_696);
    }

    #[test]
    fn parses_iso_utc() {
        let ts = parse_iso("2026-08-31T22:15:00Z").unwrap();
        assert_eq!(ts.to_string(), "2026-08-31T22:15:00Z");
        assert_eq!(parse_iso("2026-08-31 22:15").unwrap(), ts);
        assert_eq!(parse_iso("2026-08-31T22:15:00.250Z").unwrap(), ts);
    }

    #[test]
    fn rejects_bad_iso() {
        for bad in ["2026-02-30T10:00:00Z", "2026-08-31", "2026-08-31T25:00:00Z", "2026-08-31T10:00:00+2", "yesterday"]
        {
            assert!(parse_iso(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parses_offsets() {
        assert_eq!(parse_offset("+02:00"), Some(7200));
        assert_eq!(parse_offset("-0530"), Some(-19_800));
        assert_eq!(parse_offset("+01"), Some(3600));
        assert_eq!(parse_offset("02:00"), None);
        assert_eq!(parse_offset("+25:00"), None);
    }

    #[test]
    fn parses_other_formats() {
        let ts = Timestamp::from_civil(2026, 8, 14, 9, 30, 5).unwrap();
        assert_eq!(parse_compact("20260814093005").unwrap(), ts);
        assert_eq!(parse_epoch(&ts.0.to_string()).unwrap(), ts);
        assert_eq!(parse_dotted("14.08.2026 09:30").unwrap().0, ts.0 - 5);
        assert!(parse_compact("2026081409300").is_err());
    }

    #[test]
    fn month_keys() {
        assert_eq!(parse_iso("2026-08-01T00:00:00Z").unwrap().month_key(), "2026-08");
        assert_eq!(parse_iso("2026-07-31T23:59:59Z").unwrap().month_key(), "2026-07");
        assert_eq!(parse_month("2026-08").unwrap(), (2026, 8));
        assert!(parse_month("2026-13").is_err());
    }

    #[test]
    fn leap_years() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
    }
}
