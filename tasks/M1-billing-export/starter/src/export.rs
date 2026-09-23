//! Builds the export consumed by finance.

use crate::csv;
use crate::money::format_minor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    MissingColumn(String),
    InvalidBalance { customer_id: String, value: String },
}

fn column(header: &[String], name: &str) -> Result<usize, ExportError> {
    header
        .iter()
        .position(|h| h == name)
        .ok_or_else(|| ExportError::MissingColumn(name.to_string()))
}

pub fn export_customers(customers_csv: &str) -> Result<String, ExportError> {
    let (header, rows) = csv::parse(customers_csv);
    let id = column(&header, "customer_id")?;
    let email = column(&header, "billing_email")?;
    let currency = column(&header, "currency")?;
    let balance = column(&header, "balance_minor")?;
    let mut out = String::from("customer_id,billing_email,currency,balance\n");
    for row in rows {
        let amount: i64 = row[balance].parse().map_err(|_| ExportError::InvalidBalance {
            customer_id: row[id].clone(),
            value: row[balance].clone(),
        })?;
        out.push_str(&format!("{},{},{},{}\n", row[id], row[email], row[currency], format_minor(amount)));
    }
    Ok(out)
}
