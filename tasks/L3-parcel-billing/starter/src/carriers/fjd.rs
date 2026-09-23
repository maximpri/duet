//! Feed adapter for Fjord Freight (FJD).
//!
//! Format: delimited text with a header row; columns are looked up by name.
//! tab-separated, header csv, weight kg, dims cm, time iso-00.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::csv::Table;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "FJD";
pub const NAME: &str = "Fjord Freight";
pub const FORMAT: &str = "tab-separated, header csv, weight kg, dims cm, time iso-00";

/// FJD status codes and their meaning.
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

/// FJD service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("24H", Service::Express), ("48H", Service::Standard), ("ECO", Service::Economy), ("PAL", Service::Freight)];

const DELIM: char = '\t';

/// Columns every FJD feed must have.
pub const REQUIRED: &[&str] =
    &["awb", "account", "product", "wgt_kg", "length_cm", "width_cm", "height_cm", "country", "zip", "scan", "ts"];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a FJD feed.
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
                customer: get("account")?,
                service: get("product")?,
                weight: common::at_line(CODE, line, Weight::parse(get("wgt_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get("length_cm")?,
                    get("width_cm")?,
                    get("height_cm")?,
                    LengthUnit::Centimetres,
                )?,
                dest: Destination::new(get("country")?, get("zip")?),
                status: get("scan")?,
                at: common::at_line(CODE, line, time::parse_iso(get("ts")?))?,
                signed_by: r.get("receiver"),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"awb	account	product	wgt_kg	length_cm	width_cm	height_cm	country	zip	scan	ts	receiver
FJD00040012	C-9001	24H	2.5	30	20	15	DE	10115	S6	2026-08-14T09:30:05+00:00	R. Receiver
FJD00040013	C-9002	48H	0.8	20	15	10	AT	1010	S2	2026-08-14T10:30:05+00:00	
FJD00040014	C-9001	ECO	12	60	40	40	NO	9990	S4	2026-08-14T11:30:05+00:00	
"#;
    const BAD_STATUS: &str = r#"awb	account	product	wgt_kg	length_cm	width_cm	height_cm	country	zip	scan	ts	receiver
FJD00040012	C-9001	24H	2.5	30	20	15	DE	10115	ZZZ	2026-08-14T09:30:05+00:00	R. Receiver
"#;
    const BAD_WEIGHT: &str = r#"awb	account	product	wgt_kg	length_cm	width_cm	height_cm	country	zip	scan	ts	receiver
FJD00040012	C-9001	24H	heavy	30	20	15	DE	10115	S6	2026-08-14T09:30:05+00:00	R. Receiver
"#;
    const EMPTY: &str = r#"awb	account	product	wgt_kg	length_cm	width_cm	height_cm	country	zip	scan	ts	receiver
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "FJD00040012");
        assert_eq!(s.customer, "C-9001");
        assert_eq!(s.service, Service::Express);
        assert_eq!(s.weight.grams, 2500);
        assert_eq!(s.dims, crate::units::Dims::new(300, 200, 150));
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
        assert!(err.to_string().starts_with("FJD feed line"), "{err}");
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
