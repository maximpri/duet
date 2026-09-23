//! Command-line interface.
//!
//! ```text
//! parcelflow bill <YYYY-MM> [--root DIR] [--invoices] [--prices]
//! parcelflow parse <CARRIER> <FILE>
//! parcelflow quote <CARRIER> <SERVICE> <WEIGHT> <DIMS> <DEST> [--discount PCT] [--root DIR]
//! parcelflow carriers
//! parcelflow zones <COUNTRY>...
//! ```

use crate::app::{self, Settings};
use crate::billing::price::quote;
use crate::carriers;
use crate::customers::Customer;
use crate::error::{Error, Result};
use crate::model::{Destination, RawShipment, Service, Status};
use crate::money::Decimal;
use crate::report;
use crate::time::Timestamp;
use crate::units::{Dims, Weight};
use crate::zones;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Bill {
        month: String,
        root: PathBuf,
        invoices: bool,
        prices: bool,
    },
    Parse {
        carrier: String,
        file: PathBuf,
    },
    Quote {
        carrier: String,
        service: String,
        weight: String,
        dims: String,
        dest: String,
        discount: Option<String>,
        root: PathBuf,
    },
    Carriers,
    Zones {
        countries: Vec<String>,
    },
    Help,
}

pub const USAGE: &str = "usage:
  parcelflow bill <YYYY-MM> [--root DIR] [--invoices] [--prices]
  parcelflow parse <CARRIER> <FILE>
  parcelflow quote <CARRIER> <SERVICE> <WEIGHT> <DIMS> <DEST> [--discount PCT] [--root DIR]
  parcelflow carriers
  parcelflow zones <COUNTRY>...";

/// Parses the arguments after the program name.
pub fn parse_args(args: &[String]) -> Result<Command> {
    let mut flags = Vec::new();
    let mut positional = Vec::new();
    let mut root = PathBuf::from(".");
    let mut discount = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => root = PathBuf::from(it.next().ok_or_else(|| Error::Usage("--root needs a directory".into()))?),
            "--discount" => {
                discount = Some(it.next().ok_or_else(|| Error::Usage("--discount needs a percentage".into()))?.clone())
            }
            "-h" | "--help" => return Ok(Command::Help),
            f if f.starts_with("--") => flags.push(f.to_owned()),
            p => positional.push(p.to_owned()),
        }
    }
    let flag = |name: &str| flags.iter().any(|f| f == name);
    let unknown = |allowed: &[&str]| {
        flags.iter().find(|f| !allowed.contains(&f.as_str())).map(|f| Error::Usage(format!("unknown flag {f}")))
    };
    let Some((cmd, rest)) = positional.split_first() else { return Ok(Command::Help) };
    match (cmd.as_str(), rest) {
        ("bill", [month]) => {
            if let Some(e) = unknown(&["--invoices", "--prices"]) {
                return Err(e);
            }
            Ok(Command::Bill { month: month.clone(), root, invoices: flag("--invoices"), prices: flag("--prices") })
        }
        ("parse", [carrier, file]) => Ok(Command::Parse { carrier: carrier.clone(), file: PathBuf::from(file) }),
        ("quote", [carrier, service, weight, dims, dest]) => Ok(Command::Quote {
            carrier: carrier.clone(),
            service: service.clone(),
            weight: weight.clone(),
            dims: dims.clone(),
            dest: dest.clone(),
            discount,
            root,
        }),
        ("carriers", []) => Ok(Command::Carriers),
        ("zones", countries) if !countries.is_empty() => Ok(Command::Zones { countries: countries.to_vec() }),
        _ => Err(Error::Usage(format!("cannot understand {:?}", args.join(" ")))),
    }
}

/// Runs a command and returns what to print.
pub fn execute(cmd: Command) -> Result<String> {
    match cmd {
        Command::Help => Ok(USAGE.to_owned()),
        Command::Bill { month, root, invoices, prices } => {
            let run = app::run_billing(&root, &month)?;
            let mut out = String::new();
            if prices {
                out.push_str(&report::price_lines(&run));
            }
            out.push_str(&report::summary(&run));
            if invoices {
                for inv in run.outcome.invoices.values() {
                    out.push('\n');
                    out.push_str(&inv.render());
                }
            }
            Ok(out)
        }
        Command::Parse { carrier, file } => {
            let adapter = carriers::by_code(&carrier).ok_or_else(|| Error::UnknownCarrier(carrier.clone()))?;
            let text = crate::error::read_to_string(&file)?;
            let mut out = String::new();
            for s in (adapter.parse)(&text)? {
                let _ = writeln!(
                    out,
                    "{} {} {} {} {} {} {} {}",
                    s.tracking, s.customer, s.service, s.weight, s.dims, s.dest, s.status, s.status_at
                );
            }
            Ok(out)
        }
        Command::Quote { carrier, service, weight, dims, dest, discount, root } => {
            let adapter = carriers::by_code(&carrier).ok_or_else(|| Error::UnknownCarrier(carrier.clone()))?;
            let settings = Settings::from_root(&root);
            let tariff = settings.tariff()?;
            let shipment = RawShipment {
                carrier: adapter.code,
                tracking: "QUOTE".into(),
                customer: "QUOTE".into(),
                service: Service::parse(&service).ok_or_else(|| Error::value("service", service.clone()))?,
                weight: Weight::parse_with_unit(&weight)?,
                dims: Dims::parse_with_unit(&dims)?,
                dest: Destination::parse(&dest).ok_or_else(|| Error::value("destination", dest.clone()))?,
                status: Status::Delivered,
                status_at: Timestamp(0),
                signed_by: None,
            };
            let customer = Customer {
                id: "QUOTE".into(),
                company: String::new(),
                contact: String::new(),
                email: String::new(),
                discount_bp: match discount {
                    Some(d) => Decimal::parse(&d)?.basis_points(),
                    None => 0,
                },
                account_manager: String::new(),
            };
            let settings_for = tariff.carriers.get(adapter.code);
            match quote(&shipment, &customer, &settings_for, &tariff.rates, &tariff.remote) {
                Ok(q) => Ok(format!(
                    "zone {}\nbillable {}\nbase {}\nfuel {}\nremote {}\noversize {}\ndiscount {}\ntotal {}\n",
                    q.zone,
                    q.billable,
                    q.base,
                    q.surcharges.fuel,
                    q.surcharges.remote,
                    q.surcharges.oversize,
                    q.discount,
                    q.total
                )),
                Err(r) => Ok(format!("rejected: {r}\n")),
            }
        }
        Command::Carriers => {
            let mut out = String::new();
            for a in carriers::all() {
                let _ = writeln!(out, "{:<4} {:<24} {}", a.code, a.name, a.format);
            }
            Ok(out)
        }
        Command::Zones { countries } => {
            Ok(countries.iter().map(|c| format!("{} zone {}\n", c.to_ascii_uppercase(), zones::zone(c))).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn parses_bill() {
        assert_eq!(
            parse_args(&args("bill 2026-08 --root /tmp/x --prices")).unwrap(),
            Command::Bill { month: "2026-08".into(), root: "/tmp/x".into(), invoices: false, prices: true }
        );
    }

    #[test]
    fn parses_quote_with_discount() {
        let c = parse_args(&args("quote NRP express 2.5kg 30x20x10cm DE-10115 --discount 10")).unwrap();
        assert!(matches!(c, Command::Quote { discount: Some(ref d), .. } if d == "10"));
    }

    #[test]
    fn rejects_unknown_input() {
        assert!(parse_args(&args("bill")).is_err());
        assert!(parse_args(&args("bill 2026-08 --fast")).is_err());
        assert!(parse_args(&args("frobnicate")).is_err());
        assert_eq!(parse_args(&[]).unwrap(), Command::Help);
    }

    #[test]
    fn zones_command() {
        assert_eq!(execute(Command::Zones { countries: vec!["no".into()] }).unwrap(), "NO zone 4\n");
    }

    #[test]
    fn carriers_command_lists_every_adapter() {
        let out = execute(Command::Carriers).unwrap();
        assert_eq!(out.lines().count(), carriers::all().len());
    }
}
