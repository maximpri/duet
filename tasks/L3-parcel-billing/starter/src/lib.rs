//! parcelflow — multi-carrier parcel billing.
//!
//! Carrier feeds (`data/feeds/`) are parsed by one adapter per carrier into
//! [`model::RawShipment`]s. Delivered shipments are priced with our own tariff
//! (`config/rates.csv`), the carrier settings (`config/carriers*.conf`) and
//! the customer's contract (`data/customers.csv`), and invoiced per customer
//! and month. See `docs/BILLING.md` for the pricing rules.

pub mod app;
pub mod billing;
pub mod carriers;
pub mod cli;
pub mod config;
pub mod customers;
pub mod env;
pub mod error;
pub mod model;
pub mod money;
pub mod report;
pub mod tariff;
pub mod text;
pub mod time;
pub mod units;
pub mod zones;

pub use error::{Error, Result};
