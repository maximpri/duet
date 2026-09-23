//! General-ledger entries.

use crate::date::Date;
use crate::money::parse_cents;
use crate::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub entry_id: String,
    pub date: Date,
    pub account: String,
    pub counterparty: String,
    pub reference: String,
    pub amount_cents: i64,
}

/// Parses the tab-separated ledger export (header row first).
pub fn parse_ledger(content: &str) -> Result<Vec<Entry>, ParseError> {
    let mut out = Vec::new();
    for (i, raw) in content.lines().enumerate().skip(1) {
        if raw.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = raw.split('\t').collect();
        let bad = |reason: String| ParseError::BadLine { line: i + 1, reason };
        if f.len() != 6 {
            return Err(bad(format!("expected 6 fields, found {}", f.len())));
        }
        out.push(Entry {
            entry_id: f[0].to_string(),
            date: Date::parse_iso(f[1]).ok_or_else(|| bad(format!("invalid date '{}'", f[1])))?,
            account: f[2].to_string(),
            counterparty: f[3].to_string(),
            reference: f[4].to_string(),
            amount_cents: parse_cents(f[5]).ok_or_else(|| bad(format!("invalid amount '{}'", f[5])))?,
        });
    }
    Ok(out)
}
