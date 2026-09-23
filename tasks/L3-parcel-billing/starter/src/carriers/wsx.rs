//! Feed adapter for Westex (WSX).
//!
//! Format: delimited text with a header row; columns are looked up by name.
//! ','-separated, header csv, weight lb, dims in, time iso-z.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::csv::Table;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "WSX";
pub const NAME: &str = "Westex";
pub const FORMAT: &str = "','-separated, header csv, weight lb, dims in, time iso-z";

/// WSX status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("manifested", Status::InfoReceived),
    ("collected", Status::PickedUp),
    ("in_transit", Status::InTransit),
    ("arrived_at_hub", Status::InTransit),
    ("customs_hold", Status::CustomsHold),
    ("out_for_delivery", Status::OutForDelivery),
    ("delivery_attempted", Status::DeliveryFailed),
    ("delivered", Status::Delivered),
    ("delivered_to_locker", Status::Delivered),
    ("returned_to_sender", Status::Returned),
    ("lost", Status::Lost),
    ("cancelled", Status::Cancelled),
];

/// WSX service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("EXP", Service::Express), ("STD", Service::Standard), ("ECO", Service::Economy), ("FRT", Service::Freight)];

const DELIM: char = ',';

/// Columns every WSX feed must have.
pub const REQUIRED: &[&str] = &[
    "tracking_no",
    "shipper_account",
    "product",
    "gross_lb",
    "len_in",
    "wid_in",
    "hgt_in",
    "cc",
    "postcode",
    "event",
    "last_event",
];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a WSX feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let table = common::at_line(CODE, 1, Table::parse(text, DELIM))?;
    if let Some(col) = REQUIRED.iter().find(|c| !table.has_column(c)) {
        return Err(Error::feed(CODE, 1, format!("missing column {col}")));
    }
    table
        .records()
        .map(|r| {
            let line = r.line;
            let get = |k: &str| common::need(CODE, line, k, r.get(k));
            RawFields {
                tracking: get("tracking_no")?,
                customer: get("shipper_account")?,
                service: get("product")?,
                weight: common::at_line(CODE, line, Weight::parse(get("gross_lb")?, WeightUnit::Pounds))?,
                dims: common::dims3(CODE, line, get("len_in")?, get("wid_in")?, get("hgt_in")?, LengthUnit::Inches)?,
                dest: Destination::new(get("cc")?, get("postcode")?),
                status: get("event")?,
                at: common::at_line(CODE, line, time::parse_iso(get("last_event")?))?,
                signed_by: r.get("pod_name"),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"tracking_no,shipper_account,product,gross_lb,len_in,wid_in,hgt_in,cc,postcode,event,last_event,pod_name
WSX00040012,C-9001,EXP,5.5,11.8,7.9,5.9,DE,10115,delivered_to_locker,2026-08-14T09:30:05Z,R. Receiver
WSX00040013,C-9002,STD,1.8,7.9,5.9,3.9,AT,1010,arrived_at_hub,2026-08-14T10:30:05Z,
WSX00040014,C-9001,ECO,26.5,23.6,15.7,15.7,NO,9990,out_for_delivery,2026-08-14T11:30:05Z,
"#;
    const BAD_STATUS: &str = r#"tracking_no,shipper_account,product,gross_lb,len_in,wid_in,hgt_in,cc,postcode,event,last_event,pod_name
WSX00040012,C-9001,EXP,5.5,11.8,7.9,5.9,DE,10115,ZZZ,2026-08-14T09:30:05Z,R. Receiver
"#;
    const BAD_WEIGHT: &str = r#"tracking_no,shipper_account,product,gross_lb,len_in,wid_in,hgt_in,cc,postcode,event,last_event,pod_name
WSX00040012,C-9001,EXP,heavy,11.8,7.9,5.9,DE,10115,delivered_to_locker,2026-08-14T09:30:05Z,R. Receiver
"#;
    const EMPTY: &str = r#"tracking_no,shipper_account,product,gross_lb,len_in,wid_in,hgt_in,cc,postcode,event,last_event,pod_name
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "WSX00040012");
        assert_eq!(s.customer, "C-9001");
        assert_eq!(s.service, Service::Express);
        assert_eq!(s.weight.grams, 2495);
        assert_eq!(s.dims, crate::units::Dims::new(300, 201, 150));
        assert_eq!(s.dest, crate::model::Destination::new("DE", "10115"));
        assert_eq!(s.status, Status::Delivered);
        assert_eq!(s.status_at.to_string(), "2026-08-14T09:30:05Z");
        assert_eq!(s.signed_by.as_deref(), Some("R. Receiver"));
        assert_eq!(list[1].status, Status::InTransit);
        assert_eq!(list[1].dest.country, "AT");
        assert_eq!(list[2].service, Service::Economy);
        assert_eq!(list[2].status, Status::OutForDelivery);
    }

    #[test]
    fn rejects_unknown_status_codes() {
        let err = parse(BAD_STATUS).unwrap_err();
        assert!(err.to_string().contains("unknown status code"), "{err}");
    }

    #[test]
    fn rejects_malformed_weights() {
        let err = parse(BAD_WEIGHT).unwrap_err();
        assert!(err.to_string().starts_with("WSX feed line"), "{err}");
    }

    #[test]
    fn empty_feed_has_no_shipments() {
        assert!(parse(EMPTY).unwrap().is_empty());
    }

    #[test]
    fn every_service_code_maps() {
        for (code, s) in SERVICE_CODES {
            assert_eq!(service(code), Some(*s));
            assert_eq!(service(&code.to_ascii_lowercase()), Some(*s));
        }
        assert_eq!(service("??"), None);
    }

    status_cases! {
        status_manifested: "manifested" => Status::InfoReceived,
        status_collected: "collected" => Status::PickedUp,
        status_in_transit: "in_transit" => Status::InTransit,
        status_arrived_at_hub: "arrived_at_hub" => Status::InTransit,
        status_customs_hold: "customs_hold" => Status::CustomsHold,
        status_out_for_delivery: "out_for_delivery" => Status::OutForDelivery,
        status_delivery_attempted: "delivery_attempted" => Status::DeliveryFailed,
        status_delivered: "delivered" => Status::Delivered,
        status_delivered_to_locker: "delivered_to_locker" => Status::Delivered,
        status_returned_to_sender: "returned_to_sender" => Status::Returned,
        status_lost: "lost" => Status::Lost,
        status_cancelled: "cancelled" => Status::Cancelled,
    }
}
