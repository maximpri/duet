//! Feed adapter for Kestrel Express (KSX).
//!
//! Format: delimited text; the first line is a header and fields are read by position.
//! ';'-separated, positional, weight kg, dims cm, time iso-z.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::{self, csv::split_line};
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "KSX";
pub const NAME: &str = "Kestrel Express";
pub const FORMAT: &str = "';'-separated, positional, weight kg, dims cm, time iso-z";

/// KSX status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("MAN", Status::InfoReceived),
    ("PUP", Status::PickedUp),
    ("ITR", Status::InTransit),
    ("HUB", Status::InTransit),
    ("CUS", Status::CustomsHold),
    ("OFD", Status::OutForDelivery),
    ("NDL", Status::DeliveryFailed),
    ("DLV", Status::Delivered),
    ("POD", Status::Delivered),
    ("RTS", Status::Returned),
    ("LST", Status::Lost),
    ("CXL", Status::Cancelled),
];

/// KSX service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("EXP", Service::Express), ("STD", Service::Standard), ("ECO", Service::Economy), ("FRT", Service::Freight)];

const DELIM: char = ';';

/// Fields per record: tracking, customer, service, weight_kg, l_cm, w_cm, h_cm, country, postal, status, updated.
pub const COLUMNS: usize = 11;

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a KSX feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let mut lines = text::content_lines(text);
    if lines.next().is_none() {
        return Ok(Vec::new());
    }
    lines
        .map(|(line, l)| {
            let fields = split_line(l, DELIM);
            if fields.len() < COLUMNS {
                return Err(Error::feed(CODE, line, format!("expected {COLUMNS} fields, found {}", fields.len())));
            }
            let get = |i: usize, name: &str| common::need(CODE, line, name, fields.get(i).map(String::as_str));
            RawFields {
                tracking: get(0, "tracking")?,
                customer: get(1, "customer")?,
                service: get(2, "service")?,
                weight: common::at_line(CODE, line, Weight::parse(get(3, "weight_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get(4, "l_cm")?,
                    get(5, "w_cm")?,
                    get(6, "h_cm")?,
                    LengthUnit::Centimetres,
                )?,
                dest: Destination::new(get(7, "country")?, get(8, "postal")?),
                status: get(9, "status")?,
                at: common::at_line(CODE, line, time::parse_iso(get(10, "updated")?))?,
                signed_by: None,
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"tracking;customer;service;weight_kg;l_cm;w_cm;h_cm;country;postal;status;updated
KSX00040012;C-9001;EXP;2.5;30;20;15;DE;10115;POD;2026-08-14T09:30:05Z
KSX00040013;C-9002;STD;0.8;20;15;10;AT;1010;HUB;2026-08-14T10:30:05Z
KSX00040014;C-9001;ECO;12;60;40;40;NO;9990;OFD;2026-08-14T11:30:05Z
"#;
    const BAD_STATUS: &str = r#"tracking;customer;service;weight_kg;l_cm;w_cm;h_cm;country;postal;status;updated
KSX00040012;C-9001;EXP;2.5;30;20;15;DE;10115;ZZZ;2026-08-14T09:30:05Z
"#;
    const BAD_WEIGHT: &str = r#"tracking;customer;service;weight_kg;l_cm;w_cm;h_cm;country;postal;status;updated
KSX00040012;C-9001;EXP;heavy;30;20;15;DE;10115;POD;2026-08-14T09:30:05Z
"#;
    const EMPTY: &str = r#"tracking;customer;service;weight_kg;l_cm;w_cm;h_cm;country;postal;status;updated
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "KSX00040012");
        assert_eq!(s.customer, "C-9001");
        assert_eq!(s.service, Service::Express);
        assert_eq!(s.weight.grams, 2500);
        assert_eq!(s.dims, crate::units::Dims::new(300, 200, 150));
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
        assert!(err.to_string().starts_with("KSX feed line"), "{err}");
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
        status_man: "MAN" => Status::InfoReceived,
        status_pup: "PUP" => Status::PickedUp,
        status_itr: "ITR" => Status::InTransit,
        status_hub: "HUB" => Status::InTransit,
        status_cus: "CUS" => Status::CustomsHold,
        status_ofd: "OFD" => Status::OutForDelivery,
        status_ndl: "NDL" => Status::DeliveryFailed,
        status_dlv: "DLV" => Status::Delivered,
        status_pod: "POD" => Status::Delivered,
        status_rts: "RTS" => Status::Returned,
        status_lst: "LST" => Status::Lost,
        status_cxl: "CXL" => Status::Cancelled,
    }
}
