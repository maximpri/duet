//! The canonical shipment model every carrier adapter produces.

use crate::time::Timestamp;
use crate::units::{Dims, Weight};
use std::fmt;

/// Canonical tracking status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    InfoReceived,
    PickedUp,
    InTransit,
    CustomsHold,
    OutForDelivery,
    DeliveryFailed,
    Delivered,
    Returned,
    Lost,
    Cancelled,
}

impl Status {
    pub const ALL: [Status; 10] = [
        Status::InfoReceived,
        Status::PickedUp,
        Status::InTransit,
        Status::CustomsHold,
        Status::OutForDelivery,
        Status::DeliveryFailed,
        Status::Delivered,
        Status::Returned,
        Status::Lost,
        Status::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::InfoReceived => "info-received",
            Status::PickedUp => "picked-up",
            Status::InTransit => "in-transit",
            Status::CustomsHold => "customs-hold",
            Status::OutForDelivery => "out-for-delivery",
            Status::DeliveryFailed => "delivery-failed",
            Status::Delivered => "delivered",
            Status::Returned => "returned",
            Status::Lost => "lost",
            Status::Cancelled => "cancelled",
        }
    }

    /// Final states never change again.
    pub fn is_final(self) -> bool {
        matches!(self, Status::Delivered | Status::Returned | Status::Lost | Status::Cancelled)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Our service levels. Carrier service codes map onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Service {
    Express,
    Standard,
    Economy,
    Freight,
}

impl Service {
    pub const ALL: [Service; 4] = [Service::Express, Service::Standard, Service::Economy, Service::Freight];

    pub fn as_str(self) -> &'static str {
        match self {
            Service::Express => "express",
            Service::Standard => "standard",
            Service::Economy => "economy",
            Service::Freight => "freight",
        }
    }

    pub fn parse(text: &str) -> Option<Service> {
        Service::ALL.into_iter().find(|s| s.as_str().eq_ignore_ascii_case(text.trim()))
    }

    /// Heaviest billable weight accepted for the service.
    pub fn max_weight(self) -> Weight {
        Weight::from_grams(match self {
            Service::Express => 30_000,
            Service::Standard => 31_500,
            Service::Economy => 40_000,
            Service::Freight => 1_000_000,
        })
    }
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Destination country (ISO 3166 alpha-2) and postal code as written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Destination {
    pub country: String,
    pub postal: String,
}

impl Destination {
    pub fn new(country: &str, postal: &str) -> Destination {
        Destination { country: country.trim().to_ascii_uppercase(), postal: postal.trim().to_owned() }
    }

    /// Parses `NO-9990` or `NO 9990`.
    pub fn parse(text: &str) -> Option<Destination> {
        let t = text.trim();
        let (c, p) = t.split_once(['-', ' '])?;
        (c.len() == 2 && !p.trim().is_empty()).then(|| Destination::new(c, p))
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.country, self.postal)
    }
}

/// One shipment as reported by a carrier feed: its latest status and the
/// parcel as measured by the carrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawShipment {
    pub carrier: &'static str,
    pub tracking: String,
    pub customer: String,
    pub service: Service,
    pub weight: Weight,
    pub dims: Dims,
    pub dest: Destination,
    pub status: Status,
    pub status_at: Timestamp,
    /// Name of the person who signed for a delivery, when the carrier reports it.
    pub signed_by: Option<String>,
}

impl RawShipment {
    pub fn is_delivered(&self) -> bool {
        self.status == Status::Delivered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_have_unique_names() {
        let mut names: Vec<_> = Status::ALL.iter().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Status::ALL.len());
        assert!(Status::Delivered.is_final());
        assert!(!Status::OutForDelivery.is_final());
    }

    #[test]
    fn services_parse_and_have_limits() {
        assert_eq!(Service::parse(" Express "), Some(Service::Express));
        assert_eq!(Service::parse("overnight"), None);
        assert_eq!(Service::Express.max_weight().grams, 30_000);
        assert!(Service::Freight.max_weight() > Service::Economy.max_weight());
    }

    #[test]
    fn destinations_parse() {
        assert_eq!(Destination::parse("no-9990"), Some(Destination::new("NO", "9990")));
        assert_eq!(Destination::parse("SE 981 31").unwrap().postal, "981 31");
        assert_eq!(Destination::parse("9990"), None);
        assert_eq!(Destination::new("de", " 10115 ").to_string(), "DE-10115");
    }
}
