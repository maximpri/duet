//! Feed adapter for Granite Couriers (GRN).
//!
//! Format: blocks of key/value lines.
//! ':' pairs, blocks end with END, key/value, weight lb, dims in, time iso-z.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{RawShipment, Service, Status};
use crate::text::kv::parse_blocks;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "GRN";
pub const NAME: &str = "Granite Couriers";
pub const FORMAT: &str = "':' pairs, blocks end with END, key/value, weight lb, dims in, time iso-z";

/// GRN status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("EV-INF", Status::InfoReceived),
    ("EV-PKP", Status::PickedUp),
    ("EV-TRN", Status::InTransit),
    ("EV-HUB", Status::InTransit),
    ("EV-CST", Status::CustomsHold),
    ("EV-OFD", Status::OutForDelivery),
    ("EV-NHA", Status::DeliveryFailed),
    ("EV-DLV", Status::Delivered),
    ("EV-RET", Status::Returned),
    ("EV-LOS", Status::Lost),
    ("EV-CAN", Status::Cancelled),
];

/// GRN service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("EXP", Service::Express), ("STD", Service::Standard), ("ECO", Service::Economy), ("FRT", Service::Freight)];

const SEP: char = ':';
const TERMINATOR: Option<&str> = Some("END");

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a GRN feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let blocks = parse_blocks(text, SEP, TERMINATOR).map_err(|(line, m)| Error::feed(CODE, line, m))?;
    blocks
        .iter()
        .map(|b| {
            let line = b.line;
            let get = |k: &str| common::need(CODE, line, k, b.get(k));
            RawFields {
                tracking: get("tracking")?,
                customer: get("customer")?,
                service: get("product")?,
                weight: common::at_line(CODE, line, Weight::parse(get("gross_lb")?, WeightUnit::Pounds))?,
                dims: common::dims3(CODE, line, get("len_in")?, get("wid_in")?, get("hgt_in")?, LengthUnit::Inches)?,
                dest: common::dest_combined(CODE, line, get("dest")?)?,
                status: get("event")?,
                at: common::at_line(CODE, line, time::parse_iso(get("event_time")?))?,
                signed_by: b.get("receiver"),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"tracking: GRN00040012
customer: C-9001
product: EXP
gross_lb: 5.5
len_in: 11.8
wid_in: 7.9
hgt_in: 5.9
dest: DE-10115
event: EV-DLV
event_time: 2026-08-14T09:30:05Z
receiver: R. Receiver
END

tracking: GRN00040013
customer: C-9002
product: STD
gross_lb: 1.8
len_in: 7.9
wid_in: 5.9
hgt_in: 3.9
dest: AT-1010
event: EV-HUB
event_time: 2026-08-14T10:30:05Z
END

tracking: GRN00040014
customer: C-9001
product: ECO
gross_lb: 26.5
len_in: 23.6
wid_in: 15.7
hgt_in: 15.7
dest: NO-9990
event: EV-OFD
event_time: 2026-08-14T11:30:05Z
END

"#;
    const BAD_STATUS: &str = r#"tracking: GRN00040012
customer: C-9001
product: EXP
gross_lb: 5.5
len_in: 11.8
wid_in: 7.9
hgt_in: 5.9
dest: DE-10115
event: ZZZ
event_time: 2026-08-14T09:30:05Z
receiver: R. Receiver
END

"#;
    const BAD_WEIGHT: &str = r#"tracking: GRN00040012
customer: C-9001
product: EXP
gross_lb: heavy
len_in: 11.8
wid_in: 7.9
hgt_in: 5.9
dest: DE-10115
event: EV-DLV
event_time: 2026-08-14T09:30:05Z
receiver: R. Receiver
END

"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "GRN00040012");
        assert_eq!(s.customer, "C-9001");
        assert_eq!(s.service, Service::Express);
        assert_eq!(s.weight.grams, 2495);
        assert_eq!(s.dims, crate::units::Dims::new(300, 201, 150));
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
        assert!(err.to_string().starts_with("GRN feed line"), "{err}");
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
        status_ev_inf: "EV-INF" => Status::InfoReceived,
        status_ev_pkp: "EV-PKP" => Status::PickedUp,
        status_ev_trn: "EV-TRN" => Status::InTransit,
        status_ev_hub: "EV-HUB" => Status::InTransit,
        status_ev_cst: "EV-CST" => Status::CustomsHold,
        status_ev_ofd: "EV-OFD" => Status::OutForDelivery,
        status_ev_nha: "EV-NHA" => Status::DeliveryFailed,
        status_ev_dlv: "EV-DLV" => Status::Delivered,
        status_ev_ret: "EV-RET" => Status::Returned,
        status_ev_los: "EV-LOS" => Status::Lost,
        status_ev_can: "EV-CAN" => Status::Cancelled,
    }
}
