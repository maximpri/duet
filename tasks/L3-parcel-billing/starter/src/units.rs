//! Weights (grams) and lengths (millimetres), with conversions from the
//! units carriers use in their feeds.

use crate::error::{Error, Result};
use crate::money::Decimal;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Weight {
    pub grams: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeightUnit {
    Grams,
    Kilograms,
    Pounds,
    Ounces,
}

impl WeightUnit {
    pub fn parse(text: &str) -> Result<WeightUnit> {
        match text.trim().to_ascii_lowercase().as_str() {
            "g" | "gr" | "gram" | "grams" => Ok(WeightUnit::Grams),
            "kg" | "kgs" | "kilo" | "kilogram" | "kilograms" => Ok(WeightUnit::Kilograms),
            "lb" | "lbs" | "pound" | "pounds" => Ok(WeightUnit::Pounds),
            "oz" | "ounce" | "ounces" => Ok(WeightUnit::Ounces),
            _ => Err(Error::value("weight unit", text)),
        }
    }

    /// (numerator, denominator) converting one unit to grams.
    fn to_grams(self) -> (i64, i64) {
        match self {
            WeightUnit::Grams => (1, 1),
            WeightUnit::Kilograms => (1000, 1),
            WeightUnit::Pounds => (45_359_237, 100_000),
            WeightUnit::Ounces => (28_349_523_125, 1_000_000_000),
        }
    }
}

impl Weight {
    pub fn from_grams(grams: u64) -> Weight {
        Weight { grams }
    }

    /// A decimal amount in `unit`, rounded half up to the gram.
    pub fn parse(value: &str, unit: WeightUnit) -> Result<Weight> {
        let d = Decimal::parse(value).map_err(|_| Error::value("weight", value))?;
        if d.mantissa < 0 {
            return Err(Error::value("weight", value));
        }
        let (num, den) = unit.to_grams();
        Ok(Weight { grams: d.scaled(num, den) as u64 })
    }

    /// Parses a value with a unit suffix: `1.25kg`, `3 lb`, `750g`.
    pub fn parse_with_unit(text: &str) -> Result<Weight> {
        let t = text.trim();
        let split = t.find(|c: char| c.is_ascii_alphabetic()).ok_or_else(|| Error::value("weight", text))?;
        Weight::parse(&t[..split], WeightUnit::parse(&t[split..])?)
    }

    pub fn kg_string(self) -> String {
        format!("{}.{:03}", self.grams / 1000, self.grams % 1000)
    }
}

impl fmt::Display for Weight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} kg", self.kg_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthUnit {
    Millimetres,
    Centimetres,
    Inches,
}

impl LengthUnit {
    pub fn parse(text: &str) -> Result<LengthUnit> {
        match text.trim().to_ascii_lowercase().as_str() {
            "mm" => Ok(LengthUnit::Millimetres),
            "cm" => Ok(LengthUnit::Centimetres),
            "in" | "inch" | "inches" | "\"" => Ok(LengthUnit::Inches),
            _ => Err(Error::value("length unit", text)),
        }
    }

    fn to_mm(self) -> (i64, i64) {
        match self {
            LengthUnit::Millimetres => (1, 1),
            LengthUnit::Centimetres => (10, 1),
            LengthUnit::Inches => (254, 10),
        }
    }
}

/// A length in millimetres, rounded half up from the feed's unit.
pub fn parse_length(value: &str, unit: LengthUnit) -> Result<u64> {
    let d = Decimal::parse(value).map_err(|_| Error::value("length", value))?;
    if d.mantissa < 0 {
        return Err(Error::value("length", value));
    }
    let (num, den) = unit.to_mm();
    Ok(d.scaled(num, den) as u64)
}

/// Parcel dimensions in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dims {
    pub l: u64,
    pub w: u64,
    pub h: u64,
}

impl Dims {
    pub fn new(l: u64, w: u64, h: u64) -> Dims {
        Dims { l, w, h }
    }

    /// Parses `LxWxH` (`x`, `X` or `*` separated) in `unit`.
    pub fn parse(text: &str, unit: LengthUnit) -> Result<Dims> {
        let parts: Vec<&str> = text.split(['x', 'X', '*']).map(str::trim).collect();
        if parts.len() != 3 {
            return Err(Error::value("dimensions", text));
        }
        Ok(Dims {
            l: parse_length(parts[0], unit)?,
            w: parse_length(parts[1], unit)?,
            h: parse_length(parts[2], unit)?,
        })
    }

    /// Parses `LxWxH` followed by a unit: `60x40x40cm`, `24x16x16 in`.
    pub fn parse_with_unit(text: &str) -> Result<Dims> {
        let t = text.trim();
        let split = t
            .rfind(|c: char| c.is_ascii_digit() || c == '.')
            .map(|i| i + 1)
            .ok_or_else(|| Error::value("dimensions", text))?;
        Dims::parse(&t[..split], LengthUnit::parse(&t[split..])?)
    }

    pub fn volume_mm3(self) -> u64 {
        self.l * self.w * self.h
    }

    pub fn longest(self) -> u64 {
        self.l.max(self.w).max(self.h)
    }

    /// Length plus girth (2 × the two shorter sides), in millimetres.
    pub fn length_plus_girth(self) -> u64 {
        let mut s = [self.l, self.w, self.h];
        s.sort_unstable();
        s[2] + 2 * (s[0] + s[1])
    }
}

impl fmt::Display for Dims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}x{} mm", self.l, self.w, self.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_weights_to_grams() {
        assert_eq!(Weight::parse("1.25", WeightUnit::Kilograms).unwrap().grams, 1250);
        assert_eq!(Weight::parse("750", WeightUnit::Grams).unwrap().grams, 750);
        assert_eq!(Weight::parse("1", WeightUnit::Pounds).unwrap().grams, 454);
        assert_eq!(Weight::parse("10", WeightUnit::Pounds).unwrap().grams, 4536);
        assert_eq!(Weight::parse("16", WeightUnit::Ounces).unwrap().grams, 454);
        assert!(Weight::parse("-1", WeightUnit::Grams).is_err());
    }

    #[test]
    fn parses_weights_with_units() {
        assert_eq!(Weight::parse_with_unit("2.5kg").unwrap().grams, 2500);
        assert_eq!(Weight::parse_with_unit("3 lb").unwrap().grams, 1361);
        assert!(Weight::parse_with_unit("12").is_err());
        assert!(Weight::parse_with_unit("12 stone").is_err());
    }

    #[test]
    fn formats_weights() {
        assert_eq!(Weight::from_grams(1205).to_string(), "1.205 kg");
    }

    #[test]
    fn converts_lengths_to_mm() {
        assert_eq!(parse_length("60", LengthUnit::Centimetres).unwrap(), 600);
        assert_eq!(parse_length("12.5", LengthUnit::Inches).unwrap(), 318);
        assert_eq!(parse_length("410", LengthUnit::Millimetres).unwrap(), 410);
    }

    #[test]
    fn parses_dimensions() {
        assert_eq!(Dims::parse("60x40x40", LengthUnit::Centimetres).unwrap(), Dims::new(600, 400, 400));
        assert_eq!(Dims::parse_with_unit("24X16X16 in").unwrap(), Dims::new(610, 406, 406));
        assert_eq!(Dims::parse_with_unit("300*200*100mm").unwrap(), Dims::new(300, 200, 100));
        assert!(Dims::parse("60x40", LengthUnit::Centimetres).is_err());
    }

    #[test]
    fn dimension_measures() {
        let d = Dims::new(600, 400, 300);
        assert_eq!(d.volume_mm3(), 72_000_000);
        assert_eq!(d.longest(), 600);
        assert_eq!(d.length_plus_girth(), 600 + 2 * 700);
    }
}
