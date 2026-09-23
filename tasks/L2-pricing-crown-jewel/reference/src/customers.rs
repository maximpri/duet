//! The customer book: each customer's contract tier, loaded from the
//! contracts system's CSV export (`data/customer_tiers.csv`).

use crate::pricing::Tier;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Customer {
    pub id: String,
    pub tier: Tier,
    /// Whether volume discounts pool quantities across the invoice (false for
    /// contracts that price every line on its own quantity).
    pub pooling: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CustomerBook {
    customers: BTreeMap<String, Customer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomerError {
    /// The header lacks a required column.
    MissingColumn(&'static str),
    /// A row (1-based line number) has fewer fields than the header.
    ShortRow(usize),
    /// A row (1-based line number) has a tier code this program does not know.
    UnknownTier { line: usize, code: String },
}

impl fmt::Display for CustomerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CustomerError::MissingColumn(c) => write!(f, "missing column {c}"),
            CustomerError::ShortRow(line) => write!(f, "line {line}: too few fields"),
            CustomerError::UnknownTier { line, code } => {
                write!(f, "line {line}: unknown tier code {code:?}")
            }
        }
    }
}

impl CustomerBook {
    /// Parses the CSV export: a header row, then one row per customer.
    /// Columns are found by name; other columns are ignored. Fields may be
    /// quoted (`"Acme, Inc."`). Blank lines and `#` comments are skipped.
    pub fn parse(text: &str) -> Result<Self, CustomerError> {
        let mut rows = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
        let header = rows.next().map(|(_, l)| split_csv(l)).unwrap_or_default();
        let column = |name: &'static str| {
            header
                .iter()
                .position(|h| h.trim() == name)
                .ok_or(CustomerError::MissingColumn(name))
        };
        let id_col = column("customer_id")?;
        let tier_col = column("tier_code")?;
        // Older exports have no pooling column: every customer pools.
        let pooling_col = column("pooling").ok();
        let mut customers = BTreeMap::new();
        for (i, line) in rows {
            let fields = split_csv(line);
            if fields.len() < header.len() {
                return Err(CustomerError::ShortRow(i + 1));
            }
            let code = fields[tier_col].trim();
            let tier = tier_from_code(code).ok_or_else(|| CustomerError::UnknownTier {
                line: i + 1,
                code: code.to_owned(),
            })?;
            let pooling = pooling_col.is_none_or(|c| fields[c].trim() != "per-line");
            let id = fields[id_col].trim().to_owned();
            customers.insert(id.clone(), Customer { id, tier, pooling });
        }
        Ok(Self { customers })
    }

    pub fn get(&self, id: &str) -> Option<&Customer> {
        self.customers.get(id)
    }

    pub fn len(&self) -> usize {
        self.customers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.customers.is_empty()
    }
}

fn tier_from_code(code: &str) -> Option<Tier> {
    match code {
        "STD" => Some(Tier::Standard),
        "SLV" => Some(Tier::Silver),
        "GLD" => Some(Tier::Gold),
        "PLAT" => Some(Tier::Platinum),
        _ => None,
    }
}

/// Splits one CSV line; double quotes group a field and `""` is a literal quote.
fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    fields
}
