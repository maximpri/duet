//! Feed adapter for Nordpost (NRP).
//!
//! Format: JSON lines, one shipment per line.
//! json lines, weight kg, dims cm, time iso-off.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::{
    self,
    json::{self, Value},
};
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "NRP";
pub const NAME: &str = "Nordpost";
pub const FORMAT: &str = "json lines, weight kg, dims cm, time iso-off";

/// NRP status codes and their meaning.
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

/// NRP service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("EXP", Service::Express), ("STD", Service::Standard), ("ECO", Service::Economy), ("FRT", Service::Freight)];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a NRP feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    text::content_lines(text)
        .map(|(line, l)| {
            let v = json::parse(l).map_err(|e| Error::feed(CODE, line, e.to_string()))?;
            let get = |k: &str| common::need(CODE, line, k, v.path(k).and_then(Value::as_text));
            RawFields {
                tracking: get("awb")?,
                customer: get("account")?,
                service: get("service_code")?,
                weight: common::at_line(CODE, line, Weight::parse(get("wgt_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get("length_cm")?,
                    get("width_cm")?,
                    get("height_cm")?,
                    LengthUnit::Centimetres,
                )?,
                dest: Destination::new(get("dest_country")?, get("postal")?),
                status: get("status_code")?,
                at: common::at_line(CODE, line, time::parse_iso(get("ts")?))?,
                signed_by: v.path("receiver").and_then(Value::as_str),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"awb":"NRP00040012","account":"C-9001","service_code":"EXP","wgt_kg":2.5,"length_cm":30,"width_cm":20,"height_cm":15,"dest_country":"DE","postal":"10115","status_code":"POD","ts":"2026-08-14T09:30:05+00:00","receiver":"R. Receiver"}
{"awb":"NRP00040013","account":"C-9002","service_code":"STD","wgt_kg":0.8,"length_cm":20,"width_cm":15,"height_cm":10,"dest_country":"AT","postal":"1010","status_code":"HUB","ts":"2026-08-14T10:30:05+00:00"}
{"awb":"NRP00040014","account":"C-9001","service_code":"ECO","wgt_kg":12,"length_cm":60,"width_cm":40,"height_cm":40,"dest_country":"NO","postal":"9990","status_code":"OFD","ts":"2026-08-14T11:30:05+00:00"}
"#;
    const BAD_STATUS: &str = r#"{"awb":"NRP00040012","account":"C-9001","service_code":"EXP","wgt_kg":2.5,"length_cm":30,"width_cm":20,"height_cm":15,"dest_country":"DE","postal":"10115","status_code":"ZZZ","ts":"2026-08-14T09:30:05+00:00","receiver":"R. Receiver"}
"#;
    const BAD_WEIGHT: &str = r#"{"awb":"NRP00040012","account":"C-9001","service_code":"EXP","wgt_kg":"heavy","length_cm":30,"width_cm":20,"height_cm":15,"dest_country":"DE","postal":"10115","status_code":"POD","ts":"2026-08-14T09:30:05+00:00","receiver":"R. Receiver"}
"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "NRP00040012");
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
        assert!(err.to_string().starts_with("NRP feed line"), "{err}");
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
