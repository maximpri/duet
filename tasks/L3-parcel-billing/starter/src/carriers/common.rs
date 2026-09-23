//! Helpers shared by the carrier adapters.

use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::time::Timestamp;
use crate::units::{parse_length, Dims, LengthUnit, Weight};

/// Case-insensitive lookup in a code table.
pub fn lookup<T: Copy>(table: &[(&str, T)], code: &str) -> Option<T> {
    let code = code.trim();
    table.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)).map(|(_, v)| *v)
}

/// Wraps any error as a feed error at `line`.
pub fn at_line<T, E: std::fmt::Display>(carrier: &str, line: usize, r: std::result::Result<T, E>) -> Result<T> {
    r.map_err(|e| Error::feed(carrier, line, e.to_string()))
}

/// Requires an optional field.
pub fn need<'a>(carrier: &str, line: usize, name: &str, value: Option<&'a str>) -> Result<&'a str> {
    match value.map(str::trim) {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(Error::feed(carrier, line, format!("missing {name}"))),
    }
}

/// Dimensions from three separate fields in `unit`.
pub fn dims3(carrier: &str, line: usize, l: &str, w: &str, h: &str, unit: LengthUnit) -> Result<Dims> {
    Ok(Dims::new(
        at_line(carrier, line, parse_length(l, unit))?,
        at_line(carrier, line, parse_length(w, unit))?,
        at_line(carrier, line, parse_length(h, unit))?,
    ))
}

/// A destination written as one field (`NO-9990`).
pub fn dest_combined(carrier: &str, line: usize, text: &str) -> Result<Destination> {
    Destination::parse(text).ok_or_else(|| Error::feed(carrier, line, format!("bad destination {text:?}")))
}

/// The fields every adapter extracts before mapping codes.
#[derive(Debug, Clone)]
pub struct RawFields<'a> {
    pub tracking: &'a str,
    pub customer: &'a str,
    pub service: &'a str,
    pub weight: Weight,
    pub dims: Dims,
    pub dest: Destination,
    pub status: &'a str,
    pub at: Timestamp,
    pub signed_by: Option<&'a str>,
}

impl RawFields<'_> {
    /// Maps the carrier's status and service codes and builds the shipment.
    pub fn finish(
        self,
        carrier: &'static str,
        line: usize,
        status: fn(&str) -> Option<Status>,
        service: fn(&str) -> Option<Service>,
    ) -> Result<RawShipment> {
        let st = status(self.status)
            .ok_or_else(|| Error::feed(carrier, line, format!("unknown status code {:?}", self.status)))?;
        let sv = service(self.service)
            .ok_or_else(|| Error::feed(carrier, line, format!("unknown service code {:?}", self.service)))?;
        if self.tracking.trim().is_empty() {
            return Err(Error::feed(carrier, line, "empty tracking number"));
        }
        Ok(RawShipment {
            carrier,
            tracking: self.tracking.trim().to_owned(),
            customer: self.customer.trim().to_owned(),
            service: sv,
            weight: self.weight,
            dims: self.dims,
            dest: self.dest,
            status: st,
            status_at: self.at,
            signed_by: self.signed_by.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &[(&str, Status)] = &[("DLV", Status::Delivered), ("ofd", Status::OutForDelivery)];

    #[test]
    fn lookups_ignore_case_and_whitespace() {
        assert_eq!(lookup(TABLE, " dlv "), Some(Status::Delivered));
        assert_eq!(lookup(TABLE, "OFD"), Some(Status::OutForDelivery));
        assert_eq!(lookup(TABLE, "XXX"), None);
    }

    #[test]
    fn need_rejects_blank_fields() {
        assert_eq!(need("T", 3, "weight", Some(" 2 ")).unwrap(), "2");
        assert_eq!(need("T", 3, "weight", Some("  ")).unwrap_err().to_string(), "T feed line 3: missing weight");
        assert!(need("T", 3, "weight", None).is_err());
    }

    #[test]
    fn finish_maps_codes() {
        let f = RawFields {
            tracking: " A1 ",
            customer: "C-1",
            service: "S",
            weight: Weight::from_grams(1),
            dims: Dims::default(),
            dest: Destination::new("DE", "10115"),
            status: "DLV",
            at: Timestamp(0),
            signed_by: Some(" "),
        };
        let s = f.clone().finish("T", 1, |c| lookup(TABLE, c), |_| Some(Service::Standard)).unwrap();
        assert_eq!(s.tracking, "A1");
        assert_eq!(s.signed_by, None);
        let bad = RawFields { status: "??", ..f };
        assert!(bad.finish("T", 1, |c| lookup(TABLE, c), |_| Some(Service::Standard)).is_err());
    }
}
