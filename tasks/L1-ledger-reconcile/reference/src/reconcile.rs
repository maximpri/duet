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

/// References are compared without surrounding whitespace and case.
fn normalize(reference: &str) -> String {
    reference.trim().to_uppercase()
}

/// Matches each ledger entry to one bank line with the same reference and amount, or else to
/// all unused bank lines with that reference when they sum to the entry (a split payment).
pub fn reconcile(statements: &[Statement], entries: &[Entry]) -> Reconciliation {
    let mut used: Vec<Vec<bool>> = statements.iter().map(|s| vec![false; s.lines.len()]).collect();
    let mut rec = Reconciliation::default();
    for entry in entries {
        let reference = normalize(&entry.reference);
        let candidates: Vec<(usize, usize)> = statements
            .iter()
            .enumerate()
            .flat_map(|(si, s)| s.lines.iter().enumerate().map(move |(li, l)| (si, li, l)))
            .filter(|(si, li, l)| !used[*si][*li] && normalize(&l.reference) == reference)
            .map(|(si, li, _)| (si, li))
            .collect();
        let exact = candidates
            .iter()
            .copied()
            .find(|&(si, li)| statements[si].lines[li].amount_cents == entry.amount_cents);
        let chosen = match exact {
            Some(one) => vec![one],
            None if candidates.len() > 1
                && candidates.iter().map(|&(si, li)| statements[si].lines[li].amount_cents).sum::<i64>()
                    == entry.amount_cents =>
            {
                candidates
            }
            None => Vec::new(),
        };
        if chosen.is_empty() {
            rec.unmatched_ledger.push(entry.entry_id.clone());
            continue;
        }
        for &(si, li) in &chosen {
            used[si][li] = true;
        }
        rec.matched.push(Match {
            entry_id: entry.entry_id.clone(),
            lines: chosen.iter().map(|&(si, li)| (statements[si].file_name.clone(), li)).collect(),
        });
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
