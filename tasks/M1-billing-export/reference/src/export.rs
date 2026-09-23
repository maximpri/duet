//! Builds the export consumed by finance.

use crate::csv;
use crate::money::{div_round, format_minor, minor_digits, parse_rate_micro};
use std::collections::HashMap;

pub use crate::money::Rounding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    MissingColumn(String),
    InvalidBalance { customer_id: String, value: String },
    UnknownCurrency(String),
    InvalidRate(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportOptions {
    pub rounding: Rounding,
}

fn column(header: &[String], name: &str) -> Result<usize, ExportError> {
    header
        .iter()
        .position(|h| h == name)
        .ok_or_else(|| ExportError::MissingColumn(name.to_string()))
}

fn rates(fx_csv: &str) -> Result<HashMap<String, i128>, ExportError> {
    let (header, rows) = csv::parse(fx_csv);
    let (c, r) = (column(&header, "currency")?, column(&header, "eur_per_unit")?);
    rows.iter()
        .map(|row| {
            let code = row.get(c).cloned().unwrap_or_default();
            let rate = row.get(r).and_then(|v| parse_rate_micro(v)).ok_or_else(|| ExportError::InvalidRate(code.clone()))?;
            Ok((code, rate))
        })
        .collect()
}

pub fn export_customers(customers_csv: &str, fx_csv: &str, options: &ExportOptions) -> Result<String, ExportError> {
    let (header, rows) = csv::parse(customers_csv);
    let id = column(&header, "customer_id")?;
    let email = column(&header, "billing_email")?;
    let currency = column(&header, "currency")?;
    let balance = column(&header, "balance_minor")?;
    let rates = rates(fx_csv)?;
    let mut out = String::from("customer_id,billing_email,currency,balance,balance_eur\n");
    for row in rows {
        let get = |i: usize| row.get(i).cloned().unwrap_or_default();
        let (cid, cur, raw) = (get(id), get(currency), get(balance));
        let amount: i64 = raw.parse().map_err(|_| ExportError::InvalidBalance { customer_id: cid.clone(), value: raw.clone() })?;
        let rate = *rates.get(&cur).ok_or_else(|| ExportError::UnknownCurrency(cur.clone()))?;
        let digits = minor_digits(&cur);
        let den = 10i128.pow(digits) * 1_000_000;
        let eur_cents = div_round(i128::from(amount) * rate * 100, den, options.rounding);
        out.push_str(&format!(
            "{},{},{},{},{}\n",
            csv::quote(&cid),
            csv::quote(&get(email)),
            csv::quote(&cur),
            format_minor(amount, digits),
            format_minor(eur_cents as i64, 2)
        ));
    }
    Ok(out)
}
