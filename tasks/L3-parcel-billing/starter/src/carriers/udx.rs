//! Feed adapter for Udinex (UDX).
//!
//! Format: delimited text; the first line is a header and fields are read by position.
//! ','-separated, positional, weight g, dims mm, time epoch.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::{self, csv::split_line};
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "UDX";
pub const NAME: &str = "Udinex";
pub const FORMAT: &str = "','-separated, positional, weight g, dims mm, time epoch";

/// UDX status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("100", Status::InfoReceived),
    ("110", Status::PickedUp),
    ("200", Status::InTransit),
    ("210", Status::InTransit),
    ("300", Status::CustomsHold),
    ("400", Status::OutForDelivery),
    ("450", Status::DeliveryFailed),
    ("500", Status::Delivered),
    ("510", Status::Delivered),
    ("600", Status::Returned),
    ("700", Status::Lost),
    ("900", Status::Cancelled),
];

/// UDX service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("P1", Service::Express), ("P2", Service::Standard), ("P3", Service::Economy), ("P9", Service::Freight)];

const DELIM: char = ',';

/// Fields per record: awb, acct, prod, grams, l_mm, w_mm, h_mm, cc, zip, code, epoch.
pub const COLUMNS: usize = 11;

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a UDX feed.
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
                tracking: get(0, "awb")?,
                customer: get(1, "acct")?,
                service: get(2, "prod")?,
                weight: common::at_line(CODE, line, Weight::parse(get(3, "grams")?, WeightUnit::Grams))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get(4, "l_mm")?,
                    get(5, "w_mm")?,
                    get(6, "h_mm")?,
                    LengthUnit::Millimetres,
                )?,
                dest: Destination::new(get(7, "cc")?, get(8, "zip")?),
                status: get(9, "code")?,
                at: common::at_line(CODE, line, time::parse_epoch(get(10, "epoch")?))?,
                signed_by: None,
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"awb,acct,prod,grams,l_mm,w_mm,h_mm,cc,zip,code,epoch
UDX00040012,C-9001,P1,2500,300,200,150,DE,10115,510,1786699805
UDX00040013,C-9002,P2,800,200,150,100,AT,1010,210,1786703405
UDX00040014,C-9001,P3,12000,600,400,400,NO,9990,400,1786707005
"#;
    const BAD_STATUS: &str = r#"awb,acct,prod,grams,l_mm,w_mm,h_mm,cc,zip,code,epoch
UDX00040012,C-9001,P1,2500,300,200,150,DE,10115,ZZZ,1786699805
"#;
    const BAD_WEIGHT: &str = r#"awb,acct,prod,grams,l_mm,w_mm,h_mm,cc,zip,code,epoch
UDX00040012,C-9001,P1,heavy,300,200,150,DE,10115,510,1786699805
"#;
    const EMPTY: &str = r#"awb,acct,prod,grams,l_mm,w_mm,h_mm,cc,zip,code,epoch
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "UDX00040012");
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
        assert!(err.to_string().starts_with("UDX feed line"), "{err}");
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
        status_100: "100" => Status::InfoReceived,
        status_110: "110" => Status::PickedUp,
        status_200: "200" => Status::InTransit,
        status_210: "210" => Status::InTransit,
        status_300: "300" => Status::CustomsHold,
        status_400: "400" => Status::OutForDelivery,
        status_450: "450" => Status::DeliveryFailed,
        status_500: "500" => Status::Delivered,
        status_510: "510" => Status::Delivered,
        status_600: "600" => Status::Returned,
        status_700: "700" => Status::Lost,
        status_900: "900" => Status::Cancelled,
    }
}
