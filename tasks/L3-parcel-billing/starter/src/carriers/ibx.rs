//! Feed adapter for Iberix (IBX).
//!
//! Format: fixed-width records.
//! fixed width, weight g, dims cm, time dotted.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::{self, fixed};
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "IBX";
pub const NAME: &str = "Iberix";
pub const FORMAT: &str = "fixed width, weight g, dims cm, time dotted";

/// IBX status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("PREAVISO", Status::InfoReceived),
    ("RECOGIDO", Status::PickedUp),
    ("EN_TRANSITO", Status::InTransit),
    ("EN_ADUANA", Status::CustomsHold),
    ("EN_REPARTO", Status::OutForDelivery),
    ("AUSENTE", Status::DeliveryFailed),
    ("ENTREGADO", Status::Delivered),
    ("DEVUELTO", Status::Returned),
    ("EXTRAVIADO", Status::Lost),
    ("ANULADO", Status::Cancelled),
];

/// IBX service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("P1", Service::Express), ("P2", Service::Standard), ("P3", Service::Economy), ("P9", Service::Freight)];

/// Record layout: (field, start column, width).
pub const LAYOUT: fixed::Layout = &[
    ("tracking", 0, 14),
    ("customer", 14, 8),
    ("service", 22, 4),
    ("weight", 26, 8),
    ("l", 34, 5),
    ("w", 39, 5),
    ("h", 44, 5),
    ("country", 49, 2),
    ("postal", 51, 10),
    ("status", 61, 12),
    ("time", 73, 16),
];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a IBX feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    text::content_lines(text)
        .map(|(line, l)| {
            let min = fixed::record_len(LAYOUT) - 2;
            if l.len() < min {
                return Err(Error::feed(CODE, line, format!("record shorter than {min} characters")));
            }
            let get = |k: &str| common::need(CODE, line, k, fixed::field(l, LAYOUT, k));
            RawFields {
                tracking: get("tracking")?,
                customer: get("customer")?,
                service: get("service")?,
                weight: common::at_line(CODE, line, Weight::parse(get("weight")?, WeightUnit::Grams))?,
                dims: common::dims3(CODE, line, get("l")?, get("w")?, get("h")?, LengthUnit::Centimetres)?,
                dest: Destination::new(get("country")?, get("postal")?),
                status: get("status")?,
                at: common::at_line(CODE, line, time::parse_dotted(get("time")?))?,
                signed_by: None,
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"IBX00040012   C-9001  P1      250030   20   15   DE10115     ENTREGADO   14.08.2026 09:30
IBX00040013   C-9002  P2       80020   15   10   AT1010      EN_TRANSITO 14.08.2026 10:30
IBX00040014   C-9001  P3     1200060   40   40   NO9990      EN_REPARTO  14.08.2026 11:30
"#;
    const BAD_STATUS: &str = r#"IBX00040012   C-9001  P1      250030   20   15   DE10115     ZZZ         14.08.2026 09:30
"#;
    const BAD_WEIGHT: &str = r#"IBX00040012   C-9001  P1     heavy30   20   15   DE10115     ENTREGADO   14.08.2026 09:30
"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "IBX00040012");
        assert_eq!(s.customer, "C-9001");
        assert_eq!(s.service, Service::Express);
        assert_eq!(s.weight.grams, 2500);
        assert_eq!(s.dims, crate::units::Dims::new(300, 200, 150));
        assert_eq!(s.dest, crate::model::Destination::new("DE", "10115"));
        assert_eq!(s.status, Status::Delivered);
        assert_eq!(s.status_at.to_string(), "2026-08-14T09:30:00Z");
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
        assert!(err.to_string().starts_with("IBX feed line"), "{err}");
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
        status_preaviso: "PREAVISO" => Status::InfoReceived,
        status_recogido: "RECOGIDO" => Status::PickedUp,
        status_en_transito: "EN_TRANSITO" => Status::InTransit,
        status_en_aduana: "EN_ADUANA" => Status::CustomsHold,
        status_en_reparto: "EN_REPARTO" => Status::OutForDelivery,
        status_ausente: "AUSENTE" => Status::DeliveryFailed,
        status_entregado: "ENTREGADO" => Status::Delivered,
        status_devuelto: "DEVUELTO" => Status::Returned,
        status_extraviado: "EXTRAVIADO" => Status::Lost,
        status_anulado: "ANULADO" => Status::Cancelled,
    }
}
