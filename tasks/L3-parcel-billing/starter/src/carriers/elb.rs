//! Feed adapter for Elbe Cargo (ELB).
//!
//! Format: JSON lines, one shipment per line.
//! json lines, weight with unit, dims LxWxH cm, time iso-z.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::{
    self,
    json::{self, Value},
};
use crate::time;
use crate::units::{Dims, Weight};

pub const CODE: &str = "ELB";
pub const NAME: &str = "Elbe Cargo";
pub const FORMAT: &str = "json lines, weight with unit, dims LxWxH cm, time iso-z";

/// ELB status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("manifested", Status::InfoReceived),
    ("collected", Status::PickedUp),
    ("in_transit", Status::InTransit),
    ("arrived_at_hub", Status::InTransit),
    ("customs_hold", Status::CustomsHold),
    ("out_for_delivery", Status::OutForDelivery),
    ("delivery_attempted", Status::DeliveryFailed),
    ("delivered", Status::Delivered),
    ("delivered_to_locker", Status::Delivered),
    ("returned_to_sender", Status::Returned),
    ("lost", Status::Lost),
    ("cancelled", Status::Cancelled),
];

/// ELB service codes.
pub const SERVICE_CODES: &[(&str, Service)] = &[
    ("express", Service::Express),
    ("parcel", Service::Standard),
    ("saver", Service::Economy),
    ("freight", Service::Freight),
];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a ELB feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    text::content_lines(text)
        .map(|(line, l)| {
            let v = json::parse(l).map_err(|e| Error::feed(CODE, line, e.to_string()))?;
            let get = |k: &str| common::need(CODE, line, k, v.path(k).and_then(Value::as_text));
            RawFields {
                tracking: get("tracking_no")?,
                customer: get("account")?,
                service: get("service_code")?,
                weight: common::at_line(CODE, line, Weight::parse_with_unit(get("parcel.weight")?))?,
                dims: common::at_line(CODE, line, Dims::parse_with_unit(get("parcel.dims")?))?,
                dest: Destination::new(get("to.country")?, get("to.postal")?),
                status: get("last_event.code")?,
                at: common::at_line(CODE, line, time::parse_iso(get("last_event.at")?))?,
                signed_by: v.path("last_event.signed_by").and_then(Value::as_str),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"tracking_no":"ELB00040012","account":"C-9001","service_code":"express","parcel":{"weight":"2.5 kg","dims":"30x20x15cm"},"to":{"country":"DE","postal":"10115"},"last_event":{"code":"delivered_to_locker","at":"2026-08-14T09:30:05Z","signed_by":"R. Receiver"}}
{"tracking_no":"ELB00040013","account":"C-9002","service_code":"parcel","parcel":{"weight":"0.8 kg","dims":"20x15x10cm"},"to":{"country":"AT","postal":"1010"},"last_event":{"code":"arrived_at_hub","at":"2026-08-14T10:30:05Z"}}
{"tracking_no":"ELB00040014","account":"C-9001","service_code":"saver","parcel":{"weight":"12 kg","dims":"60x40x40cm"},"to":{"country":"NO","postal":"9990"},"last_event":{"code":"out_for_delivery","at":"2026-08-14T11:30:05Z"}}
"#;
    const BAD_STATUS: &str = r#"{"tracking_no":"ELB00040012","account":"C-9001","service_code":"express","parcel":{"weight":"2.5 kg","dims":"30x20x15cm"},"to":{"country":"DE","postal":"10115"},"last_event":{"code":"ZZZ","at":"2026-08-14T09:30:05Z","signed_by":"R. Receiver"}}
"#;
    const BAD_WEIGHT: &str = r#"{"tracking_no":"ELB00040012","account":"C-9001","service_code":"express","parcel":{"weight":"heavy","dims":"30x20x15cm"},"to":{"country":"DE","postal":"10115"},"last_event":{"code":"delivered_to_locker","at":"2026-08-14T09:30:05Z","signed_by":"R. Receiver"}}
"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "ELB00040012");
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
        assert!(err.to_string().starts_with("ELB feed line"), "{err}");
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
        status_manifested: "manifested" => Status::InfoReceived,
        status_collected: "collected" => Status::PickedUp,
        status_in_transit: "in_transit" => Status::InTransit,
        status_arrived_at_hub: "arrived_at_hub" => Status::InTransit,
        status_customs_hold: "customs_hold" => Status::CustomsHold,
        status_out_for_delivery: "out_for_delivery" => Status::OutForDelivery,
        status_delivery_attempted: "delivery_attempted" => Status::DeliveryFailed,
        status_delivered: "delivered" => Status::Delivered,
        status_delivered_to_locker: "delivered_to_locker" => Status::Delivered,
        status_returned_to_sender: "returned_to_sender" => Status::Returned,
        status_lost: "lost" => Status::Lost,
        status_cancelled: "cancelled" => Status::Cancelled,
    }
}
