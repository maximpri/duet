//! Feed adapter for Expedito (XPD).
//!
//! Format: blocks of key/value lines.
//! '=' pairs, blank-line separated, key/value, weight kg, dims LxWxH cm, time compact.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{RawShipment, Service, Status};
use crate::text::kv::parse_blocks;
use crate::time;
use crate::units::{Dims, Weight, WeightUnit};

pub const CODE: &str = "XPD";
pub const NAME: &str = "Expedito";
pub const FORMAT: &str = "'=' pairs, blank-line separated, key/value, weight kg, dims LxWxH cm, time compact";

/// XPD status codes and their meaning.
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

/// XPD service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("24H", Service::Express), ("48H", Service::Standard), ("ECO", Service::Economy), ("PAL", Service::Freight)];

const SEP: char = '=';
const TERMINATOR: Option<&str> = None;

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a XPD feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let blocks = parse_blocks(text, SEP, TERMINATOR).map_err(|(line, m)| Error::feed(CODE, line, m))?;
    blocks
        .iter()
        .map(|b| {
            let line = b.line;
            let get = |k: &str| common::need(CODE, line, k, b.get(k));
            RawFields {
                tracking: get("shipment_id")?,
                customer: get("customer")?,
                service: get("service")?,
                weight: common::at_line(CODE, line, Weight::parse(get("wgt_kg")?, WeightUnit::Kilograms))?,
                dims: common::at_line(CODE, line, Dims::parse_with_unit(get("dimensions")?))?,
                dest: common::dest_combined(CODE, line, get("destination")?)?,
                status: get("status_code")?,
                at: common::at_line(CODE, line, time::parse_compact(get("scanned_at")?))?,
                signed_by: None,
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"shipment_id=XPD00040012
customer=C-9001
service=24H
wgt_kg=2.5
dimensions=30x20x15cm
destination=DE-10115
status_code=ENTREGADO
scanned_at=20260814093005

shipment_id=XPD00040013
customer=C-9002
service=48H
wgt_kg=0.8
dimensions=20x15x10cm
destination=AT-1010
status_code=EN_TRANSITO
scanned_at=20260814103005

shipment_id=XPD00040014
customer=C-9001
service=ECO
wgt_kg=12
dimensions=60x40x40cm
destination=NO-9990
status_code=EN_REPARTO
scanned_at=20260814113005

"#;
    const BAD_STATUS: &str = r#"shipment_id=XPD00040012
customer=C-9001
service=24H
wgt_kg=2.5
dimensions=30x20x15cm
destination=DE-10115
status_code=ZZZ
scanned_at=20260814093005

"#;
    const BAD_WEIGHT: &str = r#"shipment_id=XPD00040012
customer=C-9001
service=24H
wgt_kg=heavy
dimensions=30x20x15cm
destination=DE-10115
status_code=ENTREGADO
scanned_at=20260814093005

"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "XPD00040012");
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
        assert!(err.to_string().starts_with("XPD feed line"), "{err}");
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
