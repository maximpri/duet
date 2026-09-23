//! Customer invoices.

use super::price::Quote;
use crate::model::{RawShipment, Service};
use crate::money::Cents;
use crate::time::Timestamp;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceLine {
    pub carrier: String,
    pub tracking: String,
    pub service: Service,
    pub delivered: Timestamp,
    pub dest: String,
    pub quote: Quote,
}

impl InvoiceLine {
    pub fn new(s: &RawShipment, quote: Quote) -> InvoiceLine {
        InvoiceLine {
            carrier: s.carrier.to_owned(),
            tracking: s.tracking.clone(),
            service: s.service,
            delivered: s.status_at,
            dest: s.dest.to_string(),
            quote,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    pub customer: String,
    pub month: String,
    pub lines: Vec<InvoiceLine>,
}

impl Invoice {
    pub fn new(customer: &str, month: &str) -> Invoice {
        Invoice { customer: customer.to_owned(), month: month.to_owned(), lines: Vec::new() }
    }

    pub fn total(&self) -> Cents {
        self.lines.iter().map(|l| l.quote.total).sum()
    }

    /// Plain-text invoice: one line per shipment and a total.
    pub fn render(&self) -> String {
        let mut out = format!("INVOICE {} {}\n", self.customer, self.month);
        let _ = writeln!(
            out,
            "{:<10} {:<5} {:<16} {:<9} {:<12} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9}",
            "date", "carr", "tracking", "service", "dest", "kg", "base", "surch", "disc", "", "total"
        );
        for l in &self.lines {
            let q = &l.quote;
            let _ = writeln!(
                out,
                "{:<10} {:<5} {:<16} {:<9} {:<12} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9}",
                l.delivered.date_string(),
                l.carrier,
                l.tracking,
                l.service.as_str(),
                l.dest,
                q.billable.kg_string(),
                q.base,
                q.surcharges.total(),
                -q.discount,
                "",
                q.total
            );
        }
        let _ = writeln!(out, "{:>112}", format!("total {}", self.total()));
        out
    }
}
