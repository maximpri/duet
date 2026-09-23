//! Bank statement files.
//!
//! Two export formats exist:
//! - `date,description,amount,reference` with ISO dates and `1234.56` amounts;
//! - `Buchungsdatum;Verwendungszweck;Betrag;Referenz` with `DD.MM.YYYY` dates and
//!   `1.234,56` amounts.

use crate::date::Date;
use crate::money::parse_cents;
use crate::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementLine {
    pub date: Date,
    pub description: String,
    pub amount_cents: i64,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub file_name: String,
    pub lines: Vec<StatementLine>,
}

impl Statement {
    /// File-name prefix before the first `_2`, e.g. `bank_a` for `bank_a_2026-08.csv`.
    pub fn prefix(&self) -> &str {
        self.file_name.split("_2").next().unwrap_or(&self.file_name)
    }
}

enum Format {
    Us,
    European,
}

fn parse_eu_date(s: &str) -> Option<Date> {
    let mut it = s.trim().split('.');
    let (day, month, year) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    Date::parse_iso(&format!("{year}-{month}-{day}"))
}

fn parse_eu_amount(s: &str) -> Option<i64> {
    parse_cents(&s.trim().replace('.', "").replace(',', "."))
}

pub fn parse_statement(file_name: &str, content: &str) -> Result<Statement, ParseError> {
    let mut rows = content.lines().enumerate();
    let header = rows.next().map(|(_, h)| h.trim()).unwrap_or_default();
    let format = match header {
        "date,description,amount,reference" => Format::Us,
        "Buchungsdatum;Verwendungszweck;Betrag;Referenz" => Format::European,
        other => return Err(ParseError::UnknownFormat(other.to_string())),
    };
    let sep = match format {
        Format::Us => ',',
        Format::European => ';',
    };
    let mut lines = Vec::new();
    for (i, raw) in rows {
        if raw.trim().is_empty() {
            continue;
        }
        let bad = |reason: String| ParseError::BadLine { line: i + 1, reason };
        let f: Vec<&str> = raw.split(sep).collect();
        if f.len() != 4 {
            return Err(bad(format!("expected 4 fields, found {}", f.len())));
        }
        let (date, amount) = match format {
            Format::Us => (Date::parse_iso(f[0]), parse_cents(f[2])),
            Format::European => (parse_eu_date(f[0]), parse_eu_amount(f[2])),
        };
        lines.push(StatementLine {
            date: date.ok_or_else(|| bad(format!("invalid date '{}'", f[0])))?,
            description: f[1].to_string(),
            amount_cents: amount.ok_or_else(|| bad(format!("invalid amount '{}'", f[2])))?,
            reference: f[3].to_string(),
        });
    }
    Ok(Statement { file_name: file_name.to_string(), lines })
}
