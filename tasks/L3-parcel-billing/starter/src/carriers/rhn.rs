//! Feed adapter for Rhein Paket (RHN).
//!
//! Format: pipe-separated records: H (header), D (detail), T (trailer with the detail count).
//! H/D/T records, weight kg, dims LxWxH mm, time iso-z.

use super::common::{self, lookup, RawFields};
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::text;
use crate::time;
use crate::units::{Dims, Weight, WeightUnit};

pub const CODE: &str = "RHN";
pub const NAME: &str = "Rhein Paket";
pub const FORMAT: &str = "H/D/T records, weight kg, dims LxWxH mm, time iso-z";

/// RHN status codes and their meaning.
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

/// RHN service codes.
pub const SERVICE_CODES: &[(&str, Service)] = &[
    ("PRIO", Service::Express),
    ("NORM", Service::Standard),
    ("SPAR", Service::Economy),
    ("FRACHT", Service::Freight),
];

pub fn status(code: &str) -> Option<Status> {
    lookup(STATUS_CODES, code)
}

pub fn service(code: &str) -> Option<Service> {
    lookup(SERVICE_CODES, code)
}

/// Parses a RHN feed.
pub fn parse(text: &str) -> Result<Vec<RawShipment>> {
    let mut out = Vec::new();
    let mut header = false;
    let mut trailer: Option<(usize, usize)> = None;
    for (line, l) in text::content_lines(text) {
        let fields: Vec<&str> = l.split('|').collect();
        match fields[0].trim() {
            "H" => {
                if fields.get(1).map(|c| c.trim()) != Some(CODE) {
                    return Err(Error::feed(CODE, line, "header names another carrier"));
                }
                header = true;
            }
            "D" => {
                if !header || trailer.is_some() {
                    return Err(Error::feed(CODE, line, "detail record outside header and trailer"));
                }
                let get = |i: usize, name: &str| common::need(CODE, line, name, fields.get(i).copied());
                out.push(
                    RawFields {
                        tracking: get(1, "tracking")?,
                        customer: get(2, "customer")?,
                        service: get(3, "service")?,
                        weight: common::at_line(CODE, line, Weight::parse(get(4, "weight")?, WeightUnit::Kilograms))?,
                        dims: common::at_line(CODE, line, Dims::parse_with_unit(get(5, "dims")?))?,
                        dest: Destination::new(get(6, "country")?, get(7, "postal")?),
                        status: get(8, "status")?,
                        at: common::at_line(CODE, line, time::parse_iso(get(9, "time")?))?,
                        signed_by: None,
                    }
                    .finish(CODE, line, status, service)?,
                );
            }
            "T" => {
                let count = common::need(CODE, line, "count", fields.get(1).copied())?;
                trailer = Some((line, common::at_line(CODE, line, count.parse::<usize>())?));
            }
            other => return Err(Error::feed(CODE, line, format!("unknown record type {other:?}"))),
        }
    }
    match trailer {
        None if !header => Ok(out),
        None => Err(Error::feed(CODE, 0, "missing trailer")),
        Some((line, n)) if n != out.len() => {
            Err(Error::feed(CODE, line, format!("trailer count {n} but {} detail records", out.len())))
        }
        Some(_) => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"H|RHN|20260814
D|RHN00040012|C-9001|PRIO|2.5|300x200x150mm|DE|10115|ZUGESTELLT|2026-08-14T09:30:05Z
D|RHN00040013|C-9002|NORM|0.8|200x150x100mm|AT|1010|IM_ZENTRUM|2026-08-14T10:30:05Z
D|RHN00040014|C-9001|SPAR|12|600x400x400mm|NO|9990|IN_ZUSTELLUNG|2026-08-14T11:30:05Z
T|3
"#;
    const BAD_STATUS: &str = r#"H|RHN|20260814
D|RHN00040012|C-9001|PRIO|2.5|300x200x150mm|DE|10115|ZZZ|2026-08-14T09:30:05Z
T|1
"#;
    const BAD_WEIGHT: &str = r#"H|RHN|20260814
D|RHN00040012|C-9001|PRIO|heavy|300x200x150mm|DE|10115|ZUGESTELLT|2026-08-14T09:30:05Z
T|1
"#;
    const EMPTY: &str = r#""#;

    #[test]
    fn parses_sample_feed() {
        let list = parse(SAMPLE).unwrap();
        assert_eq!(list.len(), 3);
        let s = &list[0];
        assert_eq!(s.carrier, CODE);
        assert_eq!(s.tracking, "RHN00040012");
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
        assert!(err.to_string().starts_with("RHN feed line"), "{err}");
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
