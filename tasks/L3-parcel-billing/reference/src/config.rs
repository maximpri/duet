//! Per-carrier billing settings (`config/carriers*.conf`).
//!
//! INI-style sections: `[default]` first, then one `[CODE]` section per
//! carrier overriding any default. `volumetric_divisor` (2026 files) and
//! `dim_divisor` (older files) are the same setting. Unknown keys are ignored so that files
//! written for newer releases still load.

use crate::error::{Error, Result};
use crate::money::{Cents, Decimal};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarrierSettings {
    /// Fuel surcharge in basis points of the base rate.
    pub fuel_bp: i64,
    /// Volumetric divisor: cm³ per kg of dimensional weight.
    pub dim_divisor: u64,
    pub remote_fee: Cents,
    pub oversize_fee: Cents,
}

impl Default for CarrierSettings {
    fn default() -> Self {
        CarrierSettings { fuel_bp: 0, dim_divisor: 5000, remote_fee: Cents::ZERO, oversize_fee: Cents::ZERO }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CarrierConfig {
    pub default: CarrierSettings,
    pub carriers: BTreeMap<String, CarrierSettings>,
}

impl CarrierConfig {
    /// Settings for `carrier` (upper-case code), falling back to `[default]`.
    pub fn get(&self, carrier: &str) -> CarrierSettings {
        self.carriers.get(&carrier.to_ascii_uppercase()).copied().unwrap_or(self.default)
    }

    pub fn load(path: &Path) -> Result<CarrierConfig> {
        let text = crate::error::read_to_string(path)?;
        CarrierConfig::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, file: &str) -> Result<CarrierConfig> {
        let err = |line: usize, message: String| Error::Config { file: file.to_owned(), line, message };
        let mut config = CarrierConfig::default();
        let mut section: Option<String> = None;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim().to_ascii_uppercase();
                if name != "DEFAULT" {
                    config.carriers.insert(name.clone(), config.default);
                }
                section = Some(name);
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| err(i + 1, "expected key = value".into()))?;
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            let target = match section.as_deref() {
                None => return Err(err(i + 1, "setting outside a section".into())),
                Some("DEFAULT") => &mut config.default,
                Some(name) => config.carriers.get_mut(name).expect("section inserted above"),
            };
            let bad = |what: &str| err(i + 1, format!("invalid {what}: {value}"));
            match key.as_str() {
                "fuel_pct" => target.fuel_bp = Decimal::parse(value).map_err(|_| bad("percentage"))?.basis_points(),
                "dim_divisor" | "volumetric_divisor" => {
                    target.dim_divisor = value.parse().ok().filter(|d| *d > 0).ok_or_else(|| bad("divisor"))?
                }
                "remote_fee" => target.remote_fee = Cents::parse(value).map_err(|_| bad("amount"))?,
                "oversize_fee" => target.oversize_fee = Cents::parse(value).map_err(|_| bad("amount"))?,
                _ => {}
            }
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# test
[default]
fuel_pct = 10
dim_divisor = 5000
remote_fee = 4.90

[nrp]
fuel_pct = 12.5   # summer surcharge
oversize_fee = 18

[KSX]
dim_divisor = 4000
";

    #[test]
    fn carriers_inherit_defaults() {
        let c = CarrierConfig::parse(SAMPLE, "t").unwrap();
        let nrp = c.get("NRP");
        assert_eq!(nrp.fuel_bp, 1250);
        assert_eq!(nrp.dim_divisor, 5000);
        assert_eq!(nrp.remote_fee, Cents(490));
        assert_eq!(nrp.oversize_fee, Cents(1800));
        assert_eq!(c.get("ksx").dim_divisor, 4000);
        assert_eq!(c.get("ZZZ").fuel_bp, 1000);
    }

    #[test]
    fn rejects_malformed_settings() {
        assert!(CarrierConfig::parse("[default]\nfuel_pct = lots\n", "t").is_err());
        assert!(CarrierConfig::parse("fuel_pct = 1\n", "t").is_err());
        assert!(CarrierConfig::parse("[default]\ndim_divisor = 0\n", "t").is_err());
        assert!(CarrierConfig::parse("[default]\njust words\n", "t").is_err());
    }

    #[test]
    fn ignores_unknown_keys() {
        let c = CarrierConfig::parse("[default]\nsomething_new = 1\n", "t").unwrap();
        assert_eq!(c.default, CarrierSettings::default());
    }
}
