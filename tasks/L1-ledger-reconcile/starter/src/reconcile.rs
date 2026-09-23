//! Matching statement lines to ledger entries.

use crate::entries::Entry;
use crate::statement::{Statement, StatementLine};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub entry_id: String,
    /// (statement file name, line index within that statement)
    pub lines: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciliation {
    pub matched: Vec<Match>,
    pub unmatched_bank: Vec<StatementLine>,
    /// Entry ids with no matching bank line(s), in ledger order.
    pub unmatched_ledger: Vec<String>,
}

/// Matches each ledger entry to the bank line with the same reference and amount.
pub fn reconcile(statements: &[Statement], entries: &[Entry]) -> Reconciliation {
    let mut used: Vec<Vec<bool>> = statements.iter().map(|s| vec![false; s.lines.len()]).collect();
    let mut rec = Reconciliation::default();
    for entry in entries {
        let mut found = None;
        'search: for (si, s) in statements.iter().enumerate() {
            for (li, line) in s.lines.iter().enumerate() {
                if !used[si][li] && line.reference == entry.reference && line.amount_cents == entry.amount_cents {
                    found = Some((si, li));
                    break 'search;
                }
            }
        }
        match found {
            Some((si, li)) => {
                used[si][li] = true;
                rec.matched.push(Match { entry_id: entry.entry_id.clone(), lines: vec![(statements[si].file_name.clone(), li)] });
            }
            None => rec.unmatched_ledger.push(entry.entry_id.clone()),
        }
    }
    for (si, s) in statements.iter().enumerate() {
        for (li, line) in s.lines.iter().enumerate() {
            if !used[si][li] {
                rec.unmatched_bank.push(line.clone());
            }
        }
    }
    rec
}
