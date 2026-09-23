//! Month-end reconciliation of bank statements against the general ledger.

pub mod date;
pub mod entries;
pub mod money;
pub mod reconcile;
pub mod report;
pub mod statement;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    UnknownFormat(String),
    BadLine { line: usize, reason: String },
}
