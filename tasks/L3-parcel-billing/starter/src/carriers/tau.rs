//! Feed adapter for Tauern Transport (TAU).
//!
//! Format: blocks of key/value lines.
//! ':' pairs, blocks end with --, key/value, weight kg, dims cm, time compact.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::kv::parse_blocks;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "TAU";
pub const NAME: &str = "Tauern Transport";
pub const FORMAT: &str = "':' pairs, blocks end with --, key/value, weight kg, dims cm, time compact";

/// TAU status codes and their meaning.
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

/// TAU service codes.
pub const SERVICE_CODES: &[(&str, Service)] =
    &[("24H", Service::Express), ("48H", Service::Standard), ("ECO", Service::Economy), ("PAL", Service::Freight)];

const SEP: char = ':';
const TERMINATOR: Option<&str> = Some("--");

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a TAU feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let blocks = parse_blocks(text, SEP, TERMINATOR).map_err(|(line, m)| Error::feed(CODE, line, m))?;
    blocks
        .iter()
        .map(|b| {
            let line = b.line;
            let get = |k: &str| common::need(CODE, line, k, b.get(k));
            RawFields {
                tracking: get("tracking_no")?,
                customer: get("account")?,
                service: get("service")?,
                weight: common::at_line(CODE, line, Weight::parse(get("weight_kg")?, WeightUnit::Kilograms))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get("len_cm")?,
                    get("wid_cm")?,
                    get("hgt_cm")?,
                    LengthUnit::Centimetres,
                )?,
                dest: Destination::new(get("dest_country")?, get("postal")?),
                status: get("status_code")?,
                at: common::at_line(CODE, line, time::parse_compact(get("last_event")?))?,
                signed_by: None,
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"tracking_no: TAU00040012
account: C-9001
service: 24H
weight_kg: 2.5
len_cm: 30
wid_cm: 20
hgt_cm: 15
dest_country: DE
postal: 10115
status_code: EV-DLV
last_event: 20260814093005
--

tracking_no: TAU00040013
account: C-9002
service: 48H
weight_kg: 0.8
len_cm: 20
wid_cm: 15
hgt_cm: 10
dest_country: AT
postal: 1010
status_code: EV-HUB
last_event: 20260814103005
--

tracking_no: TAU00040014
account: C-9001
service: ECO
weight_kg: 12
len_cm: 60
wid_cm: 40
hgt_cm: 40
dest_country: NO
postal: 9990
status_code: EV-OFD
last_event: 20260814113005
--

"#;
    const BAD_STATUS: &str = r#"tracking_no: TAU00040012
account: C-9001
service: 24H
weight_kg: 2.5
len_cm: 30
wid_cm: 20
hgt_cm: 15
dest_country: DE
postal: 10115
status_code: ZZZ
last_event: 20260814093005
--

"#;
    const BAD_WEIGHT: &str = r#"tracking_no: TAU00040012
account: C-9001
service: 24H
weight_kg: heavy
len_cm: 30
wid_cm: 20
hgt_cm: 15
dest_country: DE
postal: 10115
status_code: EV-DLV
last_event: 20260814093005
--

"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "TAU00040012");
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
        assert!(err.to_string().starts_with("TAU feed line"), "{err}");
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
