//! Amount parsing.

/// Parses `-1234.56` style amounts into cents.
pub fn parse_cents(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (whole, frac) = s.split_once('.').unwrap_or((s, "0"));
    if frac.len() > 2 || whole.is_empty() {
        return None;
    }
    let cents = whole.parse::<i64>().ok()? * 100 + format!("{frac:0<2}").parse::<i64>().ok()?;
    Some(if neg { -cents } else { cents })
}
