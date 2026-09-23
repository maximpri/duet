//! Customer contracts (`data/customers.csv`).

use crate::error::{Error, Result};
use crate::money::Decimal;
use crate::text::csv::Table;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Customer {
    pub id: String,
    pub company: String,
    pub contact: String,
    pub email: String,
    /// Contract discount in basis points.
    pub discount_bp: i64,
    pub account_manager: String,
}

#[derive(Debug, Clone, Default)]
pub struct Customers {
    by_id: BTreeMap<String, Customer>,
}

impl Customers {
    pub fn load(path: &Path) -> Result<Customers> {
        let text = crate::error::read_to_string(path)?;
        Customers::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, file: &str) -> Result<Customers> {
        let table = Table::parse(text, ',')?;
        let mut by_id = BTreeMap::new();
        for r in table.records() {
            let get = |k: &str| r.require(k).map_err(|m| Error::data(file, r.line, m));
            let discount = get("discount_pct")?;
            let discount_bp = if discount.is_empty() {
                0
            } else {
                Decimal::parse(discount)
                    .map_err(|_| Error::data(file, r.line, format!("bad discount {discount}")))?
                    .basis_points()
            };
            let c = Customer {
                id: get("customer_id")?.to_owned(),
                company: get("company")?.to_owned(),
                contact: get("contact_name")?.to_owned(),
                email: get("email")?.to_owned(),
                discount_bp,
                account_manager: r.get("account_manager").unwrap_or_default().to_owned(),
            };
            if by_id.insert(c.id.clone(), c).is_some() {
                return Err(Error::data(file, r.line, "duplicate customer id"));
            }
        }
        Ok(Customers { by_id })
    }

    pub fn get(&self, id: &str) -> Option<&Customer> {
        self.by_id.get(id.trim())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Customer> {
        self.by_id.values()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "customer_id,company,contact_name,email,phone,discount_pct,account_manager\n\
C-1,Acme,Ann Example,ann@example.test,+49 1,12.5,Bo\n\
C-2,\"Beta, Inc\",Ben Example,ben@example.test,,,\n";

    #[test]
    fn loads_contracts() {
        let c = Customers::parse(SAMPLE, "t").unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.get("C-1").unwrap().discount_bp, 1250);
        assert_eq!(c.get(" C-2 ").unwrap().company, "Beta, Inc");
        assert_eq!(c.get("C-2").unwrap().discount_bp, 0);
        assert!(c.get("C-3").is_none());
    }

    #[test]
    fn rejects_duplicates_and_missing_columns() {
        let dup = format!("{SAMPLE}C-1,X,Y,z@example.test,,0,\n");
        assert!(Customers::parse(&dup, "t").is_err());
        assert!(Customers::parse("customer_id,company\nC-1,X\n", "t").is_err());
    }
}
