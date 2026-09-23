//! Feed adapter for Jutland Pakke (JUT).
//!
//! Format: JSON lines, one shipment per line.
//! json lines, weight g, dims mm, time epoch.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::{
    self,
    json::{self, Value},
};
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "JUT";
pub const NAME: &str = "Jutland Pakke";
pub const FORMAT: &str = "json lines, weight g, dims mm, time epoch";

/// JUT status codes and their meaning.
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

/// JUT service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("24H", Service::Express), ("48H", Service::Standard), ("ECO", Service::Economy), ("PAL", Service::Freight)];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a JUT feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    text::content_lines(text)
        .map(|(line, l)| {
            let v = json::parse(l).map_err(|e| Error::feed(CODE, line, e.to_string()))?;
            let get = |k: &str| common::need(CODE, line, k, v.path(k).and_then(Value::as_text));
            RawFields {
                tracking: get("awb")?,
                customer: get("customer")?,
                service: get("product")?,
                weight: common::at_line(CODE, line, Weight::parse(get("weight_g")?, WeightUnit::Grams))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get("length_mm")?,
                    get("width_mm")?,
                    get("height_mm")?,
                    LengthUnit::Millimetres,
                )?,
                dest: Destination::new(get("country")?, get("postcode")?),
                status: get("event")?,
                at: common::at_line(CODE, line, time::parse_epoch(get("event_time")?))?,
                signed_by: v.path("receiver").and_then(Value::as_str),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"awb":"JUT00040012","customer":"C-9001","product":"24H","weight_g":2500,"length_mm":300,"width_mm":200,"height_mm":150,"country":"DE","postcode":"10115","event":"S6","event_time":"1786699805","receiver":"R. Receiver"}
{"awb":"JUT00040013","customer":"C-9002","product":"48H","weight_g":800,"length_mm":200,"width_mm":150,"height_mm":100,"country":"AT","postcode":"1010","event":"S2","event_time":"1786703405"}
{"awb":"JUT00040014","customer":"C-9001","product":"ECO","weight_g":12000,"length_mm":600,"width_mm":400,"height_mm":400,"country":"NO","postcode":"9990","event":"S4","event_time":"1786707005"}
"#;
    const BAD_STATUS: &str = r#"{"awb":"JUT00040012","customer":"C-9001","product":"24H","weight_g":2500,"length_mm":300,"width_mm":200,"height_mm":150,"country":"DE","postcode":"10115","event":"ZZZ","event_time":"1786699805","receiver":"R. Receiver"}
"#;
    const BAD_WEIGHT: &str = r#"{"awb":"JUT00040012","customer":"C-9001","product":"24H","weight_g":"heavy","length_mm":300,"width_mm":200,"height_mm":150,"country":"DE","postcode":"10115","event":"S6","event_time":"1786699805","receiver":"R. Receiver"}
"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "JUT00040012");
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
        assert!(err.to_string().starts_with("JUT feed line"), "{err}");
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
