//! Feed adapter for Travesia Paqueteria (TRV).
//!
//! Format: delimited text with a header row; columns are looked up by name.
//! ';'-separated, header csv, weight kg, dims cm, time iso-z.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::csv::Table;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "TRV";
pub const NAME: &str = "Travesia Paqueteria";
pub const FORMAT: &str = "';'-separated, header csv, weight kg, dims cm, time iso-z";

/// TRV status codes and their meaning.
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

/// TRV service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("X", Service::Express), ("S", Service::Standard), ("E", Service::Economy), ("F", Service::Freight)];

const DELIM: char = ';';

/// Columns every TRV feed must have.
pub const REQUIRED: &[&str] = &[
    "shipment_id",
    "account",
    "service_code",
    "weight_kg",
    "l_cm",
    "w_cm",
    "h_cm",
    "country",
    "postal",
    "status",
    "event_time",
];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a TRV feed.
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
                tracking: get("shipment_id")?,
                customer: get("account")?,
                service: get("service_code")?,
                weight: common::at_line(CODE, line, Weight::parse(get("weight_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(CODE, line, get("l_cm")?, get("w_cm")?, get("h_cm")?, LengthUnit::Centimetres)?,
                dest: Destination::new(get("country")?, get("postal")?),
                status: get("status")?,
                at: common::at_line(CODE, line, time::parse_iso(get("event_time")?))?,
                signed_by: r.get("receiver"),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"shipment_id;account;service_code;weight_kg;l_cm;w_cm;h_cm;country;postal;status;event_time;receiver
TRV00040012;C-9001;X;2,5;30;20;15;DE;10115;ENTREGADO;2026-08-14T09:30:05Z;R. Receiver
TRV00040013;C-9002;S;0,8;20;15;10;AT;1010;EN_TRANSITO;2026-08-14T10:30:05Z;
TRV00040014;C-9001;E;12;60;40;40;NO;9990;EN_REPARTO;2026-08-14T11:30:05Z;
"#;
    const BAD_STATUS: &str = r#"shipment_id;account;service_code;weight_kg;l_cm;w_cm;h_cm;country;postal;status;event_time;receiver
TRV00040012;C-9001;X;2,5;30;20;15;DE;10115;ZZZ;2026-08-14T09:30:05Z;R. Receiver
"#;
    const BAD_WEIGHT: &str = r#"shipment_id;account;service_code;weight_kg;l_cm;w_cm;h_cm;country;postal;status;event_time;receiver
TRV00040012;C-9001;X;heavy;30;20;15;DE;10115;ENTREGADO;2026-08-14T09:30:05Z;R. Receiver
"#;
    const EMPTY: &str = r#"shipment_id;account;service_code;weight_kg;l_cm;w_cm;h_cm;country;postal;status;event_time;receiver
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "TRV00040012");
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
        assert!(err.to_string().starts_with("TRV feed line"), "{err}");
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
