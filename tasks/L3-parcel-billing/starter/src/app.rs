//! Wiring: settings from `.env`, loading every input, and the billing run.

use crate::billing::{bill_month, BillingOutcome, Tariff};
use crate::carriers;
use crate::config::CarrierConfig;
use crate::customers::Customers;
use crate::error::{Error, Result};
use crate::model::RawShipment;
use crate::tariff::RateCard;
use crate::zones::RemoteAreas;
use std::path::{Path, PathBuf};

/// Environment keys `.env` (or the process environment) may set.
pub const ENV_KEYS: [&str; 5] = ["CARRIER_CONFIG", "RATES_FILE", "REMOTE_AREAS_FILE", "CUSTOMERS_FILE", "FEEDS_DIR"];

/// Input locations, relative paths resolved against the project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub carrier_config: PathBuf,
    pub rates: PathBuf,
    pub remote_areas: PathBuf,
    pub customers: PathBuf,
    pub feeds: PathBuf,
}

impl Settings {
    pub fn from_root(root: &Path) -> Settings {
        let env = crate::env::load(root, &ENV_KEYS);
        let path = |key: &str, default: &str| {
            let p = PathBuf::from(env.get(key).map(String::as_str).unwrap_or(default));
            if p.is_absolute() {
                p
            } else {
                root.join(p)
            }
        };
        Settings {
            carrier_config: path("CARRIER_CONFIG", "config/carriers.conf"),
            rates: path("RATES_FILE", "config/rates.csv"),
            remote_areas: path("REMOTE_AREAS_FILE", "config/remote_areas.csv"),
            customers: path("CUSTOMERS_FILE", "data/customers.csv"),
            feeds: path("FEEDS_DIR", "data/feeds"),
        }
    }

    pub fn tariff(&self) -> Result<Tariff> {
        Ok(Tariff {
            carriers: CarrierConfig::load(&self.carrier_config)?,
            rates: RateCard::load(&self.rates)?,
            remote: RemoteAreas::load(&self.remote_areas)?,
        })
    }
}

/// Shipments from every feed file, plus the files that failed to parse.
#[derive(Debug, Default)]
pub struct Feeds {
    pub shipments: Vec<RawShipment>,
    /// (file name, error)
    pub errors: Vec<(String, Error)>,
    /// (file name, carrier code, shipment count)
    pub files: Vec<(String, &'static str, usize)>,
}

/// Parses every file in `dir` with the adapter named by its prefix
/// (`nrp_2026-08.jsonl` → NRP). A feed that fails to parse is reported and
/// skipped; the run continues with the others.
pub fn load_feeds(dir: &Path) -> Result<Feeds> {
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io { path: dir.display().to_string(), source })?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut feeds = Feeds::default();
    for name in names {
        if name.starts_with('.') {
            continue;
        }
        let Some(adapter) = carriers::for_file(&name) else {
            feeds.errors.push((
                name.clone(),
                Error::UnknownCarrier(name.split(['_', '.']).next().unwrap_or_default().to_owned()),
            ));
            continue;
        };
        let text = crate::error::read_to_string(&dir.join(&name))?;
        match (adapter.parse)(&text) {
            Ok(list) => {
                feeds.files.push((name, adapter.code, list.len()));
                feeds.shipments.extend(list);
            }
            Err(e) => feeds.errors.push((name, e)),
        }
    }
    Ok(feeds)
}

/// The result of `parcelflow bill`.
#[derive(Debug)]
pub struct BillingRun {
    pub settings: Settings,
    pub customers: Customers,
    pub feeds: Feeds,
    pub outcome: BillingOutcome,
}

/// Bills `month` for the project at `root` using its `.env` settings.
pub fn run_billing(root: &Path, month: &str) -> Result<BillingRun> {
    crate::time::parse_month(month)?;
    let settings = Settings::from_root(root);
    let tariff = settings.tariff()?;
    let customers = Customers::load(&settings.customers)?;
    let feeds = load_feeds(&settings.feeds)?;
    let outcome = bill_month(month, &feeds.shipments, &customers, &tariff);
    Ok(BillingRun { settings, customers, feeds, outcome })
}
