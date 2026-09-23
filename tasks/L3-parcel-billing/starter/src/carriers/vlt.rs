//! Feed adapter for Voltaire Colis (VLT).
//!
//! Format: JSON lines, one shipment per line.
//! json lines, weight kg, dims cm, time iso-z.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::{
    self,
    json::{self, Value},
};
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "VLT";
pub const NAME: &str = "Voltaire Colis";
pub const FORMAT: &str = "json lines, weight kg, dims cm, time iso-z";

/// VLT status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("ANNONCE", Status::InfoReceived),
    ("PRIS_EN_CHARGE", Status::PickedUp),
    ("ACHEMINEMENT", Status::InTransit),
    ("DOUANE", Status::CustomsHold),
    ("EN_LIVRAISON", Status::OutForDelivery),
    ("AVISE", Status::DeliveryFailed),
    ("LIVRE", Status::Delivered),
    ("RETOURNE", Status::Returned),
    ("PERDU", Status::Lost),
    ("ANNULE", Status::Cancelled),
];

/// VLT service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("X", Service::Express), ("S", Service::Standard), ("E", Service::Economy), ("F", Service::Freight)];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a VLT feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    text::content_lines(text)
        .map(|(line, l)| {
            let v = json::parse(l).map_err(|e| Error::feed(CODE, line, e.to_string()))?;
            let get = |k: &str| common::need(CODE, line, k, v.path(k).and_then(Value::as_text));
            RawFields {
                tracking: get("tracking")?,
                customer: get("account")?,
                service: get("service")?,
                weight: common::at_line(CODE, line, Weight::parse(get("gross_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get("len_cm")?,
                    get("wid_cm")?,
                    get("hgt_cm")?,
                    LengthUnit::Centimetres,
                )?,
                dest: Destination::new(get("dest_country")?, get("dest_zip")?),
                status: get("event")?,
                at: common::at_line(CODE, line, time::parse_iso(get("ts")?))?,
                signed_by: v.path("signed_by").and_then(Value::as_str),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"tracking":"VLT00040012","account":"C-9001","service":"X","gross_kg":2.5,"len_cm":30,"wid_cm":20,"hgt_cm":15,"dest_country":"DE","dest_zip":"10115","event":"LIVRE","ts":"2026-08-14T09:30:05Z","signed_by":"R. Receiver"}
{"tracking":"VLT00040013","account":"C-9002","service":"S","gross_kg":0.8,"len_cm":20,"wid_cm":15,"hgt_cm":10,"dest_country":"AT","dest_zip":"1010","event":"ACHEMINEMENT","ts":"2026-08-14T10:30:05Z"}
{"tracking":"VLT00040014","account":"C-9001","service":"E","gross_kg":12,"len_cm":60,"wid_cm":40,"hgt_cm":40,"dest_country":"NO","dest_zip":"9990","event":"EN_LIVRAISON","ts":"2026-08-14T11:30:05Z"}
"#;
    const BAD_STATUS: &str = r#"{"tracking":"VLT00040012","account":"C-9001","service":"X","gross_kg":2.5,"len_cm":30,"wid_cm":20,"hgt_cm":15,"dest_country":"DE","dest_zip":"10115","event":"ZZZ","ts":"2026-08-14T09:30:05Z","signed_by":"R. Receiver"}
"#;
    const BAD_WEIGHT: &str = r#"{"tracking":"VLT00040012","account":"C-9001","service":"X","gross_kg":"heavy","len_cm":30,"wid_cm":20,"hgt_cm":15,"dest_country":"DE","dest_zip":"10115","event":"LIVRE","ts":"2026-08-14T09:30:05Z","signed_by":"R. Receiver"}
"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "VLT00040012");
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
        assert!(err.to_string().starts_with("VLT feed line"), "{err}");
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
        status_annonce: "ANNONCE" => Status::InfoReceived,
        status_pris_en_charge: "PRIS_EN_CHARGE" => Status::PickedUp,
        status_acheminement: "ACHEMINEMENT" => Status::InTransit,
        status_douane: "DOUANE" => Status::CustomsHold,
        status_en_livraison: "EN_LIVRAISON" => Status::OutForDelivery,
        status_avise: "AVISE" => Status::DeliveryFailed,
        status_livre: "LIVRE" => Status::Delivered,
        status_retourne: "RETOURNE" => Status::Returned,
        status_perdu: "PERDU" => Status::Lost,
        status_annule: "ANNULE" => Status::Cancelled,
    }
}
