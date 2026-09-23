//! Carrier feed adapters, one module per carrier.
//!
//! Every adapter exposes `CODE`, `NAME`, `FORMAT`, its status and service
//! code tables and `parse(text) -> Result<Vec<RawShipment>>`. Feed files are
//! named `<code>_<period>.<ext>`; see `docs/CARRIERS.md`.

use crate::error::Result;
use crate::model::RawShipment;

/// One test per status code: `name: "CODE" => Status::X,`.
#[cfg(test)]
macro_rules! status_cases {
    ($($name:ident: $code:expr => $status:expr,)*) => {
        $(
            #[test]
            fn $name() {
                assert_eq!(status($code), Some($status));
            }
        )*
    };
}

pub mod alp;
pub mod arc;
pub mod blt;
pub mod cdx;
pub mod common;
pub mod drv;
pub mod elb;
pub mod fjd;
pub mod grn;
pub mod hlx;
pub mod ibx;
pub mod jut;
pub mod krp;
pub mod ksx;
pub mod lum;
pub mod mrd;
pub mod nrp;
pub mod nva;
pub mod okt;
pub mod prl;
pub mod qnt;
pub mod rhn;
pub mod skn;
pub mod tau;
pub mod trv;
pub mod udx;
pub mod vlt;
pub mod wsx;
pub mod xpd;
pub mod yrd;
pub mod zen;

#[derive(Debug, Clone, Copy)]
pub struct Adapter {
    pub code: &'static str,
    pub name: &'static str,
    pub format: &'static str,
    pub parse: fn(&str) -> Result<Vec<RawShipment>>,
}

macro_rules! adapter {
    ($m:ident) => {
        Adapter { code: $m::CODE, name: $m::NAME, format: $m::FORMAT, parse: $m::parse }
    };
}

static ADAPTERS: &[Adapter] = &[
    adapter!(alp),
    adapter!(blt),
    adapter!(cdx),
    adapter!(drv),
    adapter!(elb),
    adapter!(fjd),
    adapter!(grn),
    adapter!(hlx),
    adapter!(ibx),
    adapter!(jut),
    adapter!(krp),
    adapter!(ksx),
    adapter!(lum),
    adapter!(mrd),
    adapter!(nrp),
    adapter!(nva),
    adapter!(okt),
    adapter!(prl),
    adapter!(qnt),
    adapter!(rhn),
    adapter!(skn),
    adapter!(tau),
    adapter!(trv),
    adapter!(udx),
    adapter!(vlt),
    adapter!(wsx),
    adapter!(xpd),
    adapter!(yrd),
    adapter!(zen),
    adapter!(arc),
];

pub fn all() -> &'static [Adapter] {
    ADAPTERS
}

pub fn by_code(code: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|a| a.code.eq_ignore_ascii_case(code.trim()))
}

/// The adapter for a feed file named `<code>_...`.
pub fn for_file(file_name: &str) -> Option<&'static Adapter> {
    let prefix = file_name.split(['_', '.']).next()?;
    by_code(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_upper_case() {
        let mut codes: Vec<_> = all().iter().map(|a| a.code).collect();
        assert!(codes.iter().all(|c| c.len() == 3 && c.chars().all(|x| x.is_ascii_uppercase())));
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all().len());
    }

    #[test]
    fn finds_adapters_by_file_name() {
        assert_eq!(for_file("nrp_2026-08.jsonl").map(|a| a.code), Some("NRP"));
        assert_eq!(for_file("KSX_2026-08.csv").map(|a| a.code), Some("KSX"));
        assert!(for_file("zzz_2026-08.csv").is_none());
    }

    #[test]
    fn every_adapter_accepts_an_empty_feed_or_reports_why() {
        for a in all() {
            let _ = (a.parse)("");
        }
    }
}
