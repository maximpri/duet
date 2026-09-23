//! Rate card (`config/rates.csv`): price of the first 500 g and of every
//! further 500 g, per service and zone.

use crate::error::{Error, Result};
use crate::model::Service;
use crate::money::Cents;
use crate::text::csv::Table;
use crate::units::Weight;
use std::collections::BTreeMap;
use std::path::Path;

pub const STEP_GRAMS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub first_step: Cents,
    pub per_step: Cents,
}

#[derive(Debug, Clone, Default)]
pub struct RateCard {
    rates: BTreeMap<(Service, u8), Rate>,
}

impl RateCard {
    pub fn load(path: &Path) -> Result<RateCard> {
        let text = crate::error::read_to_string(path)?;
        RateCard::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, file: &str) -> Result<RateCard> {
        let table = Table::parse(text, ',')?;
        let mut rates = BTreeMap::new();
        for r in table.records() {
            let get = |k: &str| r.require(k).map_err(|m| Error::data(file, r.line, m));
            let service =
                Service::parse(get("service")?).ok_or_else(|| Error::data(file, r.line, "unknown service"))?;
            let zone: u8 = get("zone")?.parse().map_err(|_| Error::data(file, r.line, "bad zone"))?;
            let money = |k: &str| {
                get(k).and_then(|v| Cents::parse(v).map_err(|_| Error::data(file, r.line, format!("bad {k}"))))
            };
            let rate = Rate { first_step: money("first_500g")?, per_step: money("per_500g")? };
            if rates.insert((service, zone), rate).is_some() {
                return Err(Error::data(file, r.line, "duplicate rate"));
            }
        }
        Ok(RateCard { rates })
    }

    pub fn rate(&self, service: Service, zone: u8) -> Option<Rate> {
        self.rates.get(&(service, zone)).copied()
    }

    /// Base price of `billable` (a multiple of 500 g) for the service and zone.
    pub fn base_price(&self, service: Service, zone: u8, billable: Weight) -> Option<Cents> {
        let rate = self.rate(service, zone)?;
        let steps = billable.grams.div_ceil(STEP_GRAMS).max(1) as i64;
        Some(Cents(rate.first_step.0 + (steps - 1) * rate.per_step.0))
    }

    pub fn len(&self) -> usize {
        self.rates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = "service,zone,first_500g,per_500g\nexpress,1,9.90,1.10\nstandard,1,4.50,0.40\n";

    #[test]
    fn prices_by_steps() {
        let card = RateCard::parse(CARD, "t").unwrap();
        assert_eq!(card.base_price(Service::Express, 1, Weight::from_grams(500)), Some(Cents(990)));
        assert_eq!(card.base_price(Service::Express, 1, Weight::from_grams(2000)), Some(Cents(990 + 3 * 110)));
        assert_eq!(card.base_price(Service::Standard, 1, Weight::from_grams(0)), Some(Cents(450)));
        assert_eq!(card.base_price(Service::Standard, 2, Weight::from_grams(500)), None);
    }

    #[test]
    fn rejects_bad_rows() {
        assert!(RateCard::parse("service,zone,first_500g,per_500g\nwarp,1,1,1\n", "t").is_err());
        assert!(RateCard::parse(&format!("{CARD}express,1,1,1\n"), "t").is_err());
    }
}
