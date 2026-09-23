//! The price of one shipment (see `docs/BILLING.md`).

use crate::config::CarrierSettings;
use crate::customers::Customer;
use crate::model::RawShipment;
use crate::money::Cents;
use crate::tariff::surcharges::{surcharges, Surcharges};
use crate::tariff::{billable_weight, RateCard};
use crate::units::Weight;
use crate::zones::{self, RemoteAreas};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quote {
    pub billable: Weight,
    pub zone: u8,
    pub base: Cents,
    pub surcharges: Surcharges,
    pub discount: Cents,
    pub total: Cents,
}

/// Why a shipment cannot be billed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    Overweight { billable: Weight, limit: Weight },
    NoRate { zone: u8 },
}

impl fmt::Display for Reject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reject::Overweight { billable, limit } => write!(f, "billable {billable} exceeds service limit {limit}"),
            Reject::NoRate { zone } => write!(f, "no rate for zone {zone}"),
        }
    }
}

/// Prices `shipment` for `customer`.
pub fn quote(
    shipment: &RawShipment,
    customer: &Customer,
    settings: &CarrierSettings,
    rates: &RateCard,
    remote: &RemoteAreas,
) -> Result<Quote, Reject> {
    let billable = billable_weight(shipment.weight, shipment.dims, settings.dim_divisor);
    let limit = shipment.service.max_weight();
    if billable > limit {
        return Err(Reject::Overweight { billable, limit });
    }
    let zone = zones::zone(&shipment.dest.country);
    let base = rates.base_price(shipment.service, zone, billable).ok_or(Reject::NoRate { zone })?;
    let surcharges = surcharges(base, shipment.dims, &shipment.dest, settings, remote);
    let discount = base.percent(customer.discount_bp);
    let total = base - discount + surcharges.total();
    Ok(Quote { billable, zone, base, surcharges, discount, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Destination, Service, Status};
    use crate::time::Timestamp;
    use crate::units::Dims;

    fn shipment(grams: u64, dims: Dims, dest: &str) -> RawShipment {
        RawShipment {
            carrier: "TST",
            tracking: "T1".into(),
            customer: "C-1".into(),
            service: Service::Standard,
            weight: Weight::from_grams(grams),
            dims,
            dest: Destination::parse(dest).unwrap(),
            status: Status::Delivered,
            status_at: Timestamp(0),
            signed_by: None,
        }
    }

    fn customer(discount_bp: i64) -> Customer {
        Customer {
            id: "C-1".into(),
            company: "Test".into(),
            contact: "Tess".into(),
            email: "t@example.test".into(),
            discount_bp,
            account_manager: String::new(),
        }
    }

    fn setup() -> (CarrierSettings, RateCard, RemoteAreas) {
        let settings =
            CarrierSettings { fuel_bp: 1000, dim_divisor: 5000, remote_fee: Cents(500), oversize_fee: Cents(2000) };
        let rates =
            RateCard::parse("service,zone,first_500g,per_500g\nstandard,1,4.00,0.50\nstandard,4,12.00,1.50\n", "t")
                .unwrap();
        let remote = RemoteAreas::parse("country,from_inclusive,to_inclusive\nNO,9000,9990\n", "t").unwrap();
        (settings, rates, remote)
    }

    #[test]
    fn prices_a_domestic_parcel_without_discount() {
        let (s, r, a) = setup();
        let q = quote(&shipment(1200, Dims::new(300, 200, 100), "DE-10115"), &customer(0), &s, &r, &a).unwrap();
        assert_eq!(q.billable.grams, 1500);
        assert_eq!(q.base, Cents(500));
        assert_eq!(q.surcharges.fuel, Cents(50));
        assert_eq!(q.total, Cents(550));
    }

    #[test]
    fn remote_destinations_pay_the_remote_fee() {
        let (s, r, a) = setup();
        let q = quote(&shipment(400, Dims::new(100, 100, 100), "NO-9100"), &customer(0), &s, &r, &a).unwrap();
        assert_eq!(q.zone, 4);
        assert_eq!(q.surcharges.remote, Cents(500));
        assert_eq!(q.total, Cents(1200 + 120 + 500));
    }

    #[test]
    fn overweight_parcels_are_rejected() {
        let (s, r, a) = setup();
        let err = quote(&shipment(32_000, Dims::new(100, 100, 100), "DE-10115"), &customer(0), &s, &r, &a).unwrap_err();
        assert_eq!(err, Reject::Overweight { billable: Weight::from_grams(32_000), limit: Weight::from_grams(31_500) });
        assert!(err.to_string().contains("32.000 kg"));
    }

    #[test]
    fn missing_rates_are_rejected() {
        let (s, r, a) = setup();
        let err = quote(&shipment(400, Dims::new(100, 100, 100), "US-10001"), &customer(0), &s, &r, &a).unwrap_err();
        assert_eq!(err, Reject::NoRate { zone: 5 });
    }
}
