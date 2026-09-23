//! Feed adapter for Alpina Kurier (ALP).
//!
//! Format: blocks of key/value lines.
//! '=' pairs, blank-line separated, key/value, weight g, dims cm, time epoch.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text::kv::parse_blocks;
use crate::time;
use crate::units::{LengthUnit, Weight, WeightUnit};

pub const CODE: &str = "ALP";
pub const NAME: &str = "Alpina Kurier";
pub const FORMAT: &str = "'=' pairs, blank-line separated, key/value, weight g, dims cm, time epoch";

/// ALP status codes and their meaning.
pub const STATUS_CODES: &[(&str, Status)] = &[
    ("DATEN_ERHALTEN", Status::InfoReceived),
    ("ABGEHOLT", Status::PickedUp),
    ("UNTERWEGS", Status::InTransit),
    ("IM_ZENTRUM", Status::InTransit),
    ("ZOLL", Status::CustomsHold),
    ("IN_ZUSTELLUNG", Status::OutForDelivery),
    ("NICHT_ANGETROFFEN", Status::DeliveryFailed),
    ("ZUGESTELLT", Status::Delivered),
    ("RUECKSENDUNG", Status::Returned),
    ("VERLUST", Status::Lost),
    ("STORNIERT", Status::Cancelled),
];

/// ALP service codes.
pub const SERVICE_CODES: &[(&str, Service)] = &[
    ("PRIO", Service::Express),
    ("NORM", Service::Standard),
    ("SPAR", Service::Economy),
    ("FRACHT", Service::Freight),
];

const SEP: char = '=';
const TERMINATOR: Option<&str> = None;

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a ALP feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let blocks = parse_blocks(text, SEP, TERMINATOR).map_err(|(line, m)| Error::feed(CODE, line, m))?;
    blocks
        .iter()
        .map(|b| {
            let line = b.line;
            let get = |k: &str| common::need(CODE, line, k, b.get(k));
            RawFields {
                tracking: get("tracking_no")?,
                customer: get("cust_no")?,
                service: get("svc")?,
                weight: common::at_line(CODE, line, Weight::parse(get("gross_g")?, WeightUnit::Grams))?,
                dims: common::dims3(
                    CODE,
                    line,
                    get("len_cm")?,
                    get("wid_cm")?,
                    get("hgt_cm")?,
                    LengthUnit::Centimetres,
                )?,
                dest: Destination::new(get("dest_country")?, get("postcode")?),
                status: get("event")?,
                at: common::at_line(CODE, line, time::parse_epoch(get("last_event")?))?,
                signed_by: b.get("pod_name"),
            }
            .finish(CODE, line, status, service)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"tracking_no=ALP00040012
cust_no=C-9001
svc=PRIO
gross_g=2500
len_cm=30
wid_cm=20
hgt_cm=15
dest_country=DE
postcode=10115
event=ZUGESTELLT
last_event=1786699805
pod_name=R. Receiver

tracking_no=ALP00040013
cust_no=C-9002
svc=NORM
gross_g=800
len_cm=20
wid_cm=15
hgt_cm=10
dest_country=AT
postcode=1010
event=IM_ZENTRUM
last_event=1786703405

tracking_no=ALP00040014
cust_no=C-9001
svc=SPAR
gross_g=12000
len_cm=60
wid_cm=40
hgt_cm=40
dest_country=NO
postcode=9990
event=IN_ZUSTELLUNG
last_event=1786707005

"#;
    const BAD_STATUS: &str = r#"tracking_no=ALP00040012
cust_no=C-9001
svc=PRIO
gross_g=2500
len_cm=30
wid_cm=20
hgt_cm=15
dest_country=DE
postcode=10115
event=ZZZ
last_event=1786699805
pod_name=R. Receiver

"#;
    const BAD_WEIGHT: &str = r#"tracking_no=ALP00040012
cust_no=C-9001
svc=PRIO
gross_g=heavy
len_cm=30
wid_cm=20
hgt_cm=15
dest_country=DE
postcode=10115
event=ZUGESTELLT
last_event=1786699805
pod_name=R. Receiver

"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "ALP00040012");
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
        assert!(err.to_string().starts_with("ALP feed line"), "{err}");
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
        status_daten_erhalten: "DATEN_ERHALTEN" => Status::InfoReceived,
        status_abgeholt: "ABGEHOLT" => Status::PickedUp,
        status_unterwegs: "UNTERWEGS" => Status::InTransit,
        status_im_zentrum: "IM_ZENTRUM" => Status::InTransit,
        status_zoll: "ZOLL" => Status::CustomsHold,
        status_in_zustellung: "IN_ZUSTELLUNG" => Status::OutForDelivery,
        status_nicht_angetroffen: "NICHT_ANGETROFFEN" => Status::DeliveryFailed,
        status_zugestellt: "ZUGESTELLT" => Status::Delivered,
        status_ruecksendung: "RUECKSENDUNG" => Status::Returned,
        status_verlust: "VERLUST" => Status::Lost,
        status_storniert: "STORNIERT" => Status::Cancelled,
    }
}
