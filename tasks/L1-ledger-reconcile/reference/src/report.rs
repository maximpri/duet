//! Month-end variance report.

use crate::entries::Entry;
use crate::statement::Statement;
use std::collections::BTreeMap;

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

fn parse_accounts(accounts_csv: &str) -> Result<BTreeMap<String, String>, ReportError> {
    let mut map = BTreeMap::new();
    for line in accounts_csv.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let (prefix, account) = line
            .split_once(',')
            .ok_or_else(|| ReportError::BadAccountsFile(line.to_string()))?;
        map.insert(prefix.trim().to_string(), account.trim().to_string());
    }
    Ok(map)
}

/// Per-account totals for `month` (`YYYY-MM`). `accounts_csv` maps statement file
/// prefixes to ledger accounts (`statement_prefix,account`, header row first).
pub fn variance_report(
    statements: &[Statement],
    entries: &[Entry],
    accounts_csv: &str,
    month: &str,
) -> Result<Vec<AccountVariance>, ReportError> {
    let accounts = parse_accounts(accounts_csv)?;
    let mut totals: BTreeMap<String, (i64, i64)> =
        accounts.values().map(|a| (a.clone(), (0, 0))).collect();
    for s in statements {
        let account = accounts
            .get(s.prefix())
            .ok_or_else(|| ReportError::UnmappedStatement(s.file_name.clone()))?;
        let bank: i64 = s.lines.iter().filter(|l| l.date.month_key() == month).map(|l| l.amount_cents).sum();
        totals.entry(account.clone()).or_default().1 += bank;
    }
    for e in entries.iter().filter(|e| e.date.month_key() == month) {
        if let Some(t) = totals.get_mut(&e.account) {
            t.0 += e.amount_cents;
        }
    }
    Ok(totals
        .into_iter()
        .map(|(account, (ledger_cents, bank_cents))| AccountVariance {
            account,
            ledger_cents,
            bank_cents,
            variance_cents: bank_cents - ledger_cents,
        })
        .collect())
}
