//! Money in integer cents, and exact decimal helpers.

use crate::error::{Error, Result};
use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Cents(pub i64);

impl Cents {
    pub const ZERO: Cents = Cents(0);

    /// Parses `12`, `12.5`, `12.50`, `-3.10`. More than two decimals is an error.
    pub fn parse(text: &str) -> Result<Cents> {
        let d = Decimal::parse(text).map_err(|_| Error::value("amount", text))?;
        if d.scale > 2 {
            return Err(Error::value("amount", text));
        }
        Ok(Cents(d.mantissa * 10_i64.pow(2 - d.scale)))
    }

    /// `self * basis_points / 10_000`, rounded half up (away from zero).
    pub fn percent(self, basis_points: i64) -> Cents {
        Cents(div_round_half_up(self.0 * basis_points, 10_000))
    }
}

impl fmt::Display for Cents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let text = format!("{sign}{}.{:02}", abs / 100, abs % 100);
        f.pad(&text)
    }
}

impl Add for Cents {
    type Output = Cents;
    fn add(self, rhs: Cents) -> Cents {
        Cents(self.0 + rhs.0)
    }
}

impl Sub for Cents {
    type Output = Cents;
    fn sub(self, rhs: Cents) -> Cents {
        Cents(self.0 - rhs.0)
    }
}

impl Neg for Cents {
    type Output = Cents;
    fn neg(self) -> Cents {
        Cents(-self.0)
    }
}

impl AddAssign for Cents {
    fn add_assign(&mut self, rhs: Cents) {
        self.0 += rhs.0;
    }
}

impl Sum for Cents {
    fn sum<I: Iterator<Item = Cents>>(iter: I) -> Cents {
        iter.fold(Cents::ZERO, Add::add)
    }
}

/// Integer division rounding half away from zero.
pub fn div_round_half_up(n: i64, d: i64) -> i64 {
    assert!(d > 0, "divisor must be positive");
    if n >= 0 {
        (2 * n + d) / (2 * d)
    } else {
        -((2 * -n + d) / (2 * d))
    }
}

/// Integer division rounding up (for non-negative values).
pub fn div_ceil(n: u64, d: u64) -> u64 {
    n.div_ceil(d)
}

/// An exact decimal: `mantissa / 10^scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decimal {
    pub mantissa: i64,
    pub scale: u32,
}

impl Decimal {
    /// Parses `[-]digits[.digits]`. A decimal comma is accepted as well.
    pub fn parse(text: &str) -> Result<Decimal> {
        let t = text.trim();
        let (neg, t) = match t.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let t = t.replace(',', ".");
        let (int, frac) = match t.split_once('.') {
            Some((i, f)) => (i.to_owned(), f.to_owned()),
            None => (t.clone(), String::new()),
        };
        let valid = |s: &str| s.chars().all(|c| c.is_ascii_digit());
        if (int.is_empty() && frac.is_empty()) || !valid(&int) || !valid(&frac) || frac.len() > 9 {
            return Err(Error::value("decimal", text));
        }
        let digits = format!("{int}{frac}");
        let mantissa: i64 =
            if digits.is_empty() { 0 } else { digits.parse().map_err(|_| Error::value("decimal", text))? };
        Ok(Decimal { mantissa: if neg { -mantissa } else { mantissa }, scale: frac.len() as u32 })
    }

    /// `self * numerator / denominator`, rounded half up to an integer.
    pub fn scaled(self, numerator: i64, denominator: i64) -> i64 {
        let d = denominator * 10_i64.pow(self.scale);
        div_round_half_up(self.mantissa * numerator, d)
    }

    /// Basis points (1% = 100) of a percentage written like `12.5`.
    pub fn basis_points(self) -> i64 {
        self.scaled(100, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_amounts() {
        assert_eq!(Cents::parse("12").unwrap(), Cents(1200));
        assert_eq!(Cents::parse("12.5").unwrap(), Cents(1250));
        assert_eq!(Cents::parse("-3.10").unwrap(), Cents(-310));
        assert_eq!(Cents::parse("0,99").unwrap(), Cents(99));
        assert!(Cents::parse("1.234").is_err());
        assert!(Cents::parse("abc").is_err());
    }

    #[test]
    fn displays_amounts() {
        assert_eq!(Cents(123456).to_string(), "1234.56");
        assert_eq!(Cents(-5).to_string(), "-0.05");
        assert_eq!(format!("{:>8}", Cents(990)), "    9.90");
    }

    #[test]
    fn percentages_round_half_up() {
        assert_eq!(Cents(1000).percent(1250), Cents(125));
        assert_eq!(Cents(1).percent(5000), Cents(1));
        assert_eq!(Cents(-1).percent(5000), Cents(-1));
        assert_eq!(Cents(999).percent(1000), Cents(100));
    }

    #[test]
    fn rounding_helpers() {
        assert_eq!(div_round_half_up(5, 2), 3);
        assert_eq!(div_round_half_up(4, 3), 1);
        assert_eq!(div_round_half_up(-5, 2), -3);
        assert_eq!(div_ceil(10, 5), 2);
        assert_eq!(div_ceil(11, 5), 3);
    }

    #[test]
    fn decimals_are_exact() {
        let d = Decimal::parse("44.5").unwrap();
        assert_eq!(d, Decimal { mantissa: 445, scale: 1 });
        assert_eq!(d.scaled(1000, 1), 44_500);
        assert_eq!(Decimal::parse("12.5").unwrap().basis_points(), 1250);
        assert!(Decimal::parse(".").is_err());
        assert!(Decimal::parse("1.2.3").is_err());
    }

    #[test]
    fn sums_cents() {
        let total: Cents = [Cents(1), Cents(2), Cents(3)].into_iter().sum();
        assert_eq!(total, Cents(6));
    }
}
