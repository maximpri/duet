//! Feed adapter for Zenit Post (ZEN).
//!
//! Format: pipe-separated records: H (header), D (detail), T (trailer with the detail count).
//! H/D/T records, weight g, dims LxWxH mm, time epoch.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text;
use crate::time;
use crate::units::{Dims, Weight, WeightUnit};

pub const CODE: &str = "ZEN";
pub const NAME: &str = "Zenit Post";
pub const FORMAT: &str = "H/D/T records, weight g, dims LxWxH mm, time epoch";

/// ZEN status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("S0", Status::InfoReceived),
    ("S1", Status::PickedUp),
    ("S2", Status::InTransit),
    ("S3", Status::CustomsHold),
    ("S4", Status::OutForDelivery),
    ("S5", Status::DeliveryFailed),
    ("S6", Status::Delivered),
    ("S7", Status::Returned),
    ("S8", Status::Lost),
    ("S9", Status::Cancelled),
];

/// ZEN service codes.
pub const SERVICE_CODES: &[(&str, Service)] = &[
    ("PRIO", Service::Express),
    ("NORM", Service::Standard),
    ("SPAR", Service::Economy),
    ("FRACHT", Service::Freight),
];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a ZEN feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let mut out = Vec::new();
    let mut header = false;
    let mut trailer: Option<(usize, usize)> = None;
    for (line, l) in text::content_lines(text) {
        let fields: Vec<&str> = l.split('|').collect();
        match fields[0].trim() {
            "H" => {
                if fields.get(1).map(|c| c.trim()) != Some(CODE) {
                    return Err(Error::feed(CODE, line, "header names another carrier"));
                }
                header = true;
            }
            "D" => {
                if !header || trailer.is_some() {
                    return Err(Error::feed(CODE, line, "detail record outside header and trailer"));
                }
                let get = |i: usize, name: &str| common::need(CODE, line, name, fields.get(i).copied());
                out.push(
                    RawFields {
                        tracking: get(1, "tracking")?,
                        customer: get(2, "customer")?,
                        service: get(3, "service")?,
                        weight: common::at_line(CODE, line, Weight::parse(get(4, "weight")?, WeightUnit::Grams))?,
                        dims: common::at_line(CODE, line, Dims::parse_with_unit(get(5, "dims")?))?,
                        dest: Destination::new(get(6, "country")?, get(7, "postal")?),
                        status: get(8, "status")?,
                        at: common::at_line(CODE, line, time::parse_epoch(get(9, "time")?))?,
                        signed_by: None,
                    }
                    .finish(CODE, line, status, service)?,
                );
            }
            "T" => {
                let count = common::need(CODE, line, "count", fields.get(1).copied())?;
                trailer = Some((line, common::at_line(CODE, line, count.parse::<usize>())?));
            }
            other => return Err(Error::feed(CODE, line, format!("unknown record type {other:?}"))),
        }
    }
    match trailer {
        None if !header => Ok(out),
        None => Err(Error::feed(CODE, 0, "missing trailer")),
        Some((line, n)) if n != out.len() => {
            Err(Error::feed(CODE, line, format!("trailer count {n} but {} detail records", out.len())))
        }
        Some(_) => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"H|ZEN|20260814
D|ZEN00040012|C-9001|PRIO|2500|300x200x150mm|DE|10115|S6|1786699805
D|ZEN00040013|C-9002|NORM|800|200x150x100mm|AT|1010|S2|1786703405
D|ZEN00040014|C-9001|SPAR|12000|600x400x400mm|NO|9990|S4|1786707005
T|3
"#;
    const BAD_STATUS: &str = r#"H|ZEN|20260814
D|ZEN00040012|C-9001|PRIO|2500|300x200x150mm|DE|10115|ZZZ|1786699805
T|1
"#;
    const BAD_WEIGHT: &str = r#"H|ZEN|20260814
D|ZEN00040012|C-9001|PRIO|heavy|300x200x150mm|DE|10115|S6|1786699805
T|1
"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "ZEN00040012");
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
        assert!(err.to_string().starts_with("ZEN feed line"), "{err}");
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
        status_s0: "S0" => Status::InfoReceived,
        status_s1: "S1" => Status::PickedUp,
        status_s2: "S2" => Status::InTransit,
        status_s3: "S3" => Status::CustomsHold,
        status_s4: "S4" => Status::OutForDelivery,
        status_s5: "S5" => Status::DeliveryFailed,
        status_s6: "S6" => Status::Delivered,
        status_s7: "S7" => Status::Returned,
        status_s8: "S8" => Status::Lost,
        status_s9: "S9" => Status::Cancelled,
    }
}
