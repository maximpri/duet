//! Product catalogue. SKU codes look like `WID-2`: a family code and a
//! variant number.

use std::fmt;

/// A product family. Volume discounts are agreed per family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Widget,
    Gadget,
    Service,
}

/// A stock-keeping unit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Sku {
    pub family: Family,
    pub variant: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkuError {
    Malformed(String),
    UnknownFamily(String),
}

impl fmt::Display for SkuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkuError::Malformed(code) => write!(f, "malformed SKU {code:?}"),
            SkuError::UnknownFamily(code) => write!(f, "unknown product family {code:?}"),
        }
    }
}

impl fmt::Display for Sku {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let family = match self.family {
            Family::Widget => "WID",
            Family::Gadget => "GAD",
            Family::Service => "SRV",
        };
        write!(f, "{family}-{}", self.variant)
    }
}

/// Parses a SKU code such as `WID-2`, `GAD-1` or `SRV-3`.
pub fn parse_sku(code: &str) -> Result<Sku, SkuError> {
    let code = code.trim();
    let (family, variant) = code
        .split_once('-')
        .ok_or_else(|| SkuError::Malformed(code.to_owned()))?;
    let family = match family {
        "WID" => Family::Widget,
        "GAD" => Family::Gadget,
        "SRV" => Family::Service,
        other => return Err(SkuError::UnknownFamily(other.to_owned())),
    };
    let variant = variant
        .parse()
        .map_err(|_| SkuError::Malformed(code.to_owned()))?;
    Ok(Sku { family, variant })
}
