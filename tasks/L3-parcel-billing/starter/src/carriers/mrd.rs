//! Feed adapter for Meridian Freight (MRD).
//!
//! Format: delimited text with a header row; columns are looked up by name.
//! ';'-separated, header csv, weight kg, dims cm, time compact.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{RawShipment, Service, Status};
use crate::text::csv::Table;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "MRD";
pub const NAME: &str = "Meridian Freight";
pub const FORMAT: &str = "';'-separated, header csv, weight kg, dims cm, time compact";

/// MRD status codes and their meaning.
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

/// MRD service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("24H", Service::Express), ("48H", Service::Standard), ("ECO", Service::Economy), ("PAL", Service::Freight)];

const DELIM: char = ';';

/// Columns every MRD feed must have.
pub const REQUIRED: &[&str] =
    &["awb", "client_id", "product", "weight_kg", "l_cm", "w_cm", "h_cm", "dest", "scan", "updated"];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a MRD feed.
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
                tracking: get("awb")?,
                customer: get("client_id")?,
                service: get("product")?,
                weight: common::at_line(CODE, line, Weight::parse(get("weight_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(CODE, line, get("l_cm")?, get("w_cm")?, get("h_cm")?, LengthUnit::Centimetres)?,
                dest: common::dest_combined(CODE, line, get("dest")?)?,
                status: get("scan")?,
                at: common::at_line(CODE, line, time::parse_compact(get("updated")?))?,
                signed_by: None,
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"awb;client_id;product;weight_kg;l_cm;w_cm;h_cm;dest;scan;updated
MRD00040012;C-9001;24H;2.5;30;20;15;DE-10115;510;20260814093005
MRD00040013;C-9002;48H;0.8;20;15;10;AT-1010;210;20260814103005
MRD00040014;C-9001;ECO;12;60;40;40;NO-9990;400;20260814113005
"#;
    const BAD_STATUS: &str = r#"awb;client_id;product;weight_kg;l_cm;w_cm;h_cm;dest;scan;updated
MRD00040012;C-9001;24H;2.5;30;20;15;DE-10115;ZZZ;20260814093005
"#;
    const BAD_WEIGHT: &str = r#"awb;client_id;product;weight_kg;l_cm;w_cm;h_cm;dest;scan;updated
MRD00040012;C-9001;24H;heavy;30;20;15;DE-10115;510;20260814093005
"#;
    const EMPTY: &str = r#"awb;client_id;product;weight_kg;l_cm;w_cm;h_cm;dest;scan;updated
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "MRD00040012");
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
        assert!(err.to_string().starts_with("MRD feed line"), "{err}");
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
