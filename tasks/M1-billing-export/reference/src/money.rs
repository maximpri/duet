//! Money formatting and exact conversion.

/// ISO 4217 minor-unit digits.
pub fn minor_digits(currency: &str) -> u32 {
    match currency {
        "JPY" | "KRW" | "ISK" | "CLP" | "VND" => 0,
        "BHD" | "KWD" | "OMR" | "JOD" | "TND" | "LYD" | "IQD" => 3,
        _ => 2,
    }
}

/// Formats an amount held in minor units with `digits` decimals.
pub fn format_minor(amount_minor: i64, digits: u32) -> String {
    let sign = if amount_minor < 0 { "-" } else { "" };
    let abs = amount_minor.unsigned_abs();
    if digits == 0 {
        return format!("{sign}{abs}");
    }
    let scale = 10u64.pow(digits);
    format!("{sign}{}.{:0width$}", abs / scale, abs % scale, width = digits as usize)
}

/// Parses a decimal rate into millionths.
pub fn parse_rate_micro(s: &str) -> Option<i128> {
    let (whole, frac) = s.trim().split_once('.').unwrap_or((s.trim(), ""));
    if frac.len() > 6 || whole.is_empty() || !whole.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: i128 = whole.parse().ok()?;
    let frac: i128 = format!("{frac:0<6}").parse().ok()?;
    Some(whole * 1_000_000 + frac)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rounding {
    #[default]
    HalfUp,
    HalfEven,
}

/// Divides `num` by positive `den`, rounding to an integer.
pub fn div_round(num: i128, den: i128, rounding: Rounding) -> i128 {
    let (q, r) = (num / den, num % den);
    let twice = 2 * r.abs();
    let away = if num < 0 { -1 } else { 1 };
    match twice.cmp(&den) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + away,
        std::cmp::Ordering::Equal => match rounding {
            Rounding::HalfUp => q + away,
            Rounding::HalfEven => if q % 2 == 0 { q } else { q + away },
        },
    }
}
