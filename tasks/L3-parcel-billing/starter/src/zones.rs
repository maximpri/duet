//! Tariff zones (all parcels leave from Germany) and remote delivery areas.

use crate::error::{Error, Result};
use crate::model::Destination;
use crate::text::csv::Table;
use std::path::Path;

/// Tariff zone of a destination country: 1 domestic, 2 neighbours,
/// 3 rest of the EU, 4 rest of Europe, 5 world.
pub fn zone(country: &str) -> u8 {
    match country.trim().to_ascii_uppercase().as_str() {
        "DE" => 1,
        "AT" | "BE" | "CH" | "CZ" | "DK" | "FR" | "LU" | "NL" | "PL" => 2,
        "BG" | "CY" | "EE" | "ES" | "FI" | "GR" | "HR" | "HU" | "IE" | "IT" | "LT" | "LV" | "MT" | "PT" | "RO"
        | "SE" | "SI" | "SK" => 3,
        "AD" | "AL" | "BA" | "GB" | "IS" | "LI" | "MC" | "MD" | "ME" | "MK" | "NO" | "RS" | "SM" | "TR" | "UA"
        | "VA" => 4,
        _ => 5,
    }
}

/// The digits of a postal code as a number (`981 31` → 98131); `None` if it
/// has no digits.
pub fn postal_number(postal: &str) -> Option<u64> {
    let digits: String = postal.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 12 {
        return None;
    }
    digits.parse().ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRange {
    pub country: String,
    pub from: u64,
    pub to: u64,
    pub note: String,
}

/// Remote delivery areas (`config/remote_areas.csv`).
#[derive(Debug, Clone, Default)]
pub struct RemoteAreas {
    pub ranges: Vec<RemoteRange>,
}

impl RemoteAreas {
    pub fn load(path: &Path) -> Result<RemoteAreas> {
        let text = crate::error::read_to_string(path)?;
        RemoteAreas::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, file: &str) -> Result<RemoteAreas> {
        let table = Table::parse(text, ',')?;
        let (Some(c), Some(f), Some(t)) =
            (table.column("country"), table.column("from_inclusive"), table.column("to_inclusive"))
        else {
            return Err(Error::data(file, 1, "expected columns country, from_inclusive, to_inclusive"));
        };
        let mut ranges = Vec::new();
        for r in table.records() {
            let num = |i: usize| {
                r.field(i).and_then(postal_number).ok_or_else(|| Error::data(file, r.line, "bad postal code"))
            };
            let range = RemoteRange {
                country: r.field(c).unwrap_or_default().to_ascii_uppercase(),
                from: num(f)?,
                to: num(t)?,
                note: r.get("note").unwrap_or_default().to_owned(),
            };
            if range.from > range.to {
                return Err(Error::data(file, r.line, "range ends before it starts"));
            }
            ranges.push(range);
        }
        Ok(RemoteAreas { ranges })
    }

    /// The remote range containing `dest`, if any.
    pub fn find(&self, dest: &Destination) -> Option<&RemoteRange> {
        let n = postal_number(&dest.postal)?;
        self.ranges.iter().find(|r| r.country == dest.country && (r.from..r.to).contains(&n))
    }

    pub fn is_remote(&self, dest: &Destination) -> bool {
        self.find(dest).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_by_country() {
        assert_eq!(zone("de"), 1);
        assert_eq!(zone("NL"), 2);
        assert_eq!(zone("SE"), 3);
        assert_eq!(zone("NO"), 4);
        assert_eq!(zone("US"), 5);
    }

    #[test]
    fn postal_numbers() {
        assert_eq!(postal_number("981 31"), Some(98131));
        assert_eq!(postal_number("NO-9990"), Some(9990));
        assert_eq!(postal_number("SW1A"), Some(1));
        assert_eq!(postal_number("ABC"), None);
    }

    #[test]
    fn finds_remote_areas() {
        let areas =
            RemoteAreas::parse("country,from_inclusive,to_inclusive,note\nNO,9000,9990,north\nSE,98000,98499,\n", "t")
                .unwrap();
        assert!(areas.is_remote(&Destination::new("NO", "9100")));
        assert!(areas.is_remote(&Destination::new("NO", "9000")));
        assert!(areas.is_remote(&Destination::new("SE", "981 31")));
        assert!(!areas.is_remote(&Destination::new("NO", "8999")));
        assert!(!areas.is_remote(&Destination::new("DK", "9100")));
        assert_eq!(areas.find(&Destination::new("NO", "9500")).unwrap().note, "north");
    }

    #[test]
    fn rejects_bad_ranges() {
        assert!(RemoteAreas::parse("country,from_inclusive,to_inclusive\nNO,9990,9000\n", "t").is_err());
        assert!(RemoteAreas::parse("country,from,to\nNO,1,2\n", "t").is_err());
    }
}
