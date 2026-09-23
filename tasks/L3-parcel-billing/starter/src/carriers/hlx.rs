//! Feed adapter for Helix Express (HLX).
//!
//! Format: delimited text with a header row; columns are looked up by name.
//! '|'-separated, header csv, weight oz, dims in, time epoch.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::csv::Table;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "HLX";
pub const NAME: &str = "Helix Express";
pub const FORMAT: &str = "'|'-separated, header csv, weight oz, dims in, time epoch";

/// HLX status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("EV-INF", Status::InfoReceived),
    ("EV-PKP", Status::PickedUp),
    ("EV-TRN", Status::InTransit),
    ("EV-HUB", Status::InTransit),
    ("EV-CST", Status::CustomsHold),
    ("EV-OFD", Status::OutForDelivery),
    ("EV-NHA", Status::DeliveryFailed),
    ("EV-DLV", Status::Delivered),
    ("EV-RET", Status::Returned),
    ("EV-LOS", Status::Lost),
    ("EV-CAN", Status::Cancelled),
];

/// HLX service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("X", Service::Express), ("S", Service::Standard), ("E", Service::Economy), ("F", Service::Freight)];

const DELIM: char = '|';

/// Columns every HLX feed must have.
pub const REQUIRED: &[&str] =
    &["shipment_id", "account", "service", "weight_oz", "l_in", "w_in", "h_in", "cc", "dest_zip", "scan", "ts"];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a HLX feed.
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
                tracking: get("shipment_id")?,
                customer: get("account")?,
                service: get("service")?,
                weight: common::at_line(CODE, line, Weight::parse(get("weight_oz")?, WeightUnit::Ounces))?,
                dims: common::dims3(CODE, line, get("l_in")?, get("w_in")?, get("h_in")?, LengthUnit::Inches)?,
                dest: Destination::new(get("cc")?, get("dest_zip")?),
                status: get("scan")?,
                at: common::at_line(CODE, line, time::parse_epoch(get("ts")?))?,
                signed_by: None,
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"shipment_id|account|service|weight_oz|l_in|w_in|h_in|cc|dest_zip|scan|ts
HLX00040012|C-9001|X|88.2|11.8|7.9|5.9|DE|10115|EV-DLV|1786699805
HLX00040013|C-9002|S|28.2|7.9|5.9|3.9|AT|1010|EV-HUB|1786703405
HLX00040014|C-9001|E|423.3|23.6|15.7|15.7|NO|9990|EV-OFD|1786707005
"#;
    const BAD_STATUS: &str = r#"shipment_id|account|service|weight_oz|l_in|w_in|h_in|cc|dest_zip|scan|ts
HLX00040012|C-9001|X|88.2|11.8|7.9|5.9|DE|10115|ZZZ|1786699805
"#;
    const BAD_WEIGHT: &str = r#"shipment_id|account|service|weight_oz|l_in|w_in|h_in|cc|dest_zip|scan|ts
HLX00040012|C-9001|X|heavy|11.8|7.9|5.9|DE|10115|EV-DLV|1786699805
"#;
    const EMPTY: &str = r#"shipment_id|account|service|weight_oz|l_in|w_in|h_in|cc|dest_zip|scan|ts
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "HLX00040012");
        assert_eq!(s.customer, "C-9001");
        assert_eq!(s.service, Service::Express);
        assert_eq!(s.weight.grams, 2500);
        assert_eq!(s.dims, crate::units::Dims::new(300, 201, 150));
        assert_eq!(s.dest, crate::model::Destination::new("DE", "10115"));
        assert_eq!(s.status, Status::Delivered);
        assert_eq!(s.status_at.to_string(), "2026-08-14T09:30:05Z");
        assert_eq!(s.signed_by.as_deref(), None);
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
        assert!(err.to_string().starts_with("HLX feed line"), "{err}");
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
        status_ev_inf: "EV-INF" => Status::InfoReceived,
        status_ev_pkp: "EV-PKP" => Status::PickedUp,
        status_ev_trn: "EV-TRN" => Status::InTransit,
        status_ev_hub: "EV-HUB" => Status::InTransit,
        status_ev_cst: "EV-CST" => Status::CustomsHold,
        status_ev_ofd: "EV-OFD" => Status::OutForDelivery,
        status_ev_nha: "EV-NHA" => Status::DeliveryFailed,
        status_ev_dlv: "EV-DLV" => Status::Delivered,
        status_ev_ret: "EV-RET" => Status::Returned,
        status_ev_los: "EV-LOS" => Status::Lost,
        status_ev_can: "EV-CAN" => Status::Cancelled,
    }
}
