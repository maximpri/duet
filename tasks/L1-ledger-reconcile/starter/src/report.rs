//! Month-end variance report.

use crate::entries::Entry;
use crate::statement::Statement;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountVariance {
    pub account: String,
    pub ledger_cents: i64,
    pub bank_cents: i64,
    /// bank - ledger
    pub variance_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    UnmappedStatement(String),
    BadAccountsFile(String),
}

/// Per-account totals for `month` (`YYYY-MM`). `accounts_csv` maps statement file
/// prefixes to ledger accounts (`statement_prefix,account`, header row first).
pub fn variance_report(
    _statements: &[Statement],
    _entries: &[Entry],
    _accounts_csv: &str,
    _month: &str,
) -> Result<Vec<AccountVariance>, ReportError> {
    todo!("month-end variance report")
}
