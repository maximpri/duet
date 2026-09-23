//! Feed adapter for Drava Post (DRV).
//!
//! Format: delimited text with a header row; columns are looked up by name.
//! ','-separated, header csv, weight kg, dims cm, time dotted.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::csv::Table;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "DRV";
pub const NAME: &str = "Drava Post";
pub const FORMAT: &str = "','-separated, header csv, weight kg, dims cm, time dotted";

/// DRV status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("05", Status::InfoReceived),
    ("10", Status::PickedUp),
    ("20", Status::InTransit),
    ("21", Status::InTransit),
    ("30", Status::CustomsHold),
    ("40", Status::OutForDelivery),
    ("41", Status::DeliveryFailed),
    ("50", Status::Delivered),
    ("60", Status::Returned),
    ("70", Status::Lost),
    ("99", Status::Cancelled),
];

/// DRV service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("X", Service::Express), ("S", Service::Standard), ("E", Service::Economy), ("F", Service::Freight)];

const DELIM: char = ',';

/// Columns every DRV feed must have.
pub const REQUIRED: &[&str] = &[
    "parcel_id",
    "shipper_account",
    "svc",
    "wgt_kg",
    "l_cm",
    "w_cm",
    "h_cm",
    "dest_country",
    "postal",
    "status_code",
    "updated",
];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a DRV feed.
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
                tracking: get("parcel_id")?,
                customer: get("shipper_account")?,
                service: get("svc")?,
                weight: common::at_line(CODE, line, Weight::parse(get("wgt_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(CODE, line, get("l_cm")?, get("w_cm")?, get("h_cm")?, LengthUnit::Centimetres)?,
                dest: Destination::new(get("dest_country")?, get("postal")?),
                status: get("status_code")?,
                at: common::at_line(CODE, line, time::parse_dotted(get("updated")?))?,
                signed_by: r.get("pod_name"),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"parcel_id,shipper_account,svc,wgt_kg,l_cm,w_cm,h_cm,dest_country,postal,status_code,updated,pod_name
DRV00040012,C-9001,X,2.5,30,20,15,DE,10115,50,14.08.2026 09:30,R. Receiver
DRV00040013,C-9002,S,0.8,20,15,10,AT,1010,21,14.08.2026 10:30,
DRV00040014,C-9001,E,12,60,40,40,NO,9990,40,14.08.2026 11:30,
"#;
    const BAD_STATUS: &str = r#"parcel_id,shipper_account,svc,wgt_kg,l_cm,w_cm,h_cm,dest_country,postal,status_code,updated,pod_name
DRV00040012,C-9001,X,2.5,30,20,15,DE,10115,ZZZ,14.08.2026 09:30,R. Receiver
"#;
    const BAD_WEIGHT: &str = r#"parcel_id,shipper_account,svc,wgt_kg,l_cm,w_cm,h_cm,dest_country,postal,status_code,updated,pod_name
DRV00040012,C-9001,X,heavy,30,20,15,DE,10115,50,14.08.2026 09:30,R. Receiver
"#;
    const EMPTY: &str = r#"parcel_id,shipper_account,svc,wgt_kg,l_cm,w_cm,h_cm,dest_country,postal,status_code,updated,pod_name
"#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "DRV00040012");
        assert_eq!(s.customer, "C-9001");
        assert_eq!(s.service, Service::Express);
        assert_eq!(s.weight.grams, 2500);
        assert_eq!(s.dims, crate::units::Dims::new(300, 200, 150));
        assert_eq!(s.dest, crate::model::Destination::new("DE", "10115"));
        assert_eq!(s.status, Status::Delivered);
        assert_eq!(s.status_at.to_string(), "2026-08-14T09:30:00Z");
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
        assert!(err.to_string().starts_with("DRV feed line"), "{err}");
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
        status_05: "05" => Status::InfoReceived,
        status_10: "10" => Status::PickedUp,
        status_20: "20" => Status::InTransit,
        status_21: "21" => Status::InTransit,
        status_30: "30" => Status::CustomsHold,
        status_40: "40" => Status::OutForDelivery,
        status_41: "41" => Status::DeliveryFailed,
        status_50: "50" => Status::Delivered,
        status_60: "60" => Status::Returned,
        status_70: "70" => Status::Lost,
        status_99: "99" => Status::Cancelled,
    }
}
