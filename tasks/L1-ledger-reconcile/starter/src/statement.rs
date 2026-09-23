//! Bank statement files.

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

/// Parses a statement exported as `date,description,amount,reference`.
pub fn parse_statement(file_name: &str, content: &str) -> Result<Statement, ParseError> {
    let mut lines = Vec::new();
    for (i, raw) in content.lines().enumerate().skip(1) {
        if raw.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = raw.split(',').collect();
        if f.len() != 4 {
            return Err(ParseError::BadLine { line: i + 1, reason: format!("expected 4 fields, found {}", f.len()) });
        }
        let date = Date::parse_iso(f[0]).ok_or_else(|| ParseError::BadLine { line: i + 1, reason: format!("invalid date '{}'", f[0]) })?;
        let amount_cents = parse_cents(f[2]).ok_or_else(|| ParseError::BadLine { line: i + 1, reason: format!("invalid amount '{}'", f[2]) })?;
        lines.push(StatementLine { date, description: f[1].to_string(), amount_cents, reference: f[3].to_string() });
    }
    Ok(Statement { file_name: file_name.to_string(), lines })
}
