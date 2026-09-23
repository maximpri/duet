//! A monthly billing run over every carrier's shipments.

use super::invoice::{Invoice, InvoiceLine};
use super::price::{quote, Reject};
use crate::config::CarrierConfig;
use crate::customers::Customers;
use crate::model::RawShipment;
use crate::tariff::RateCard;
use crate::zones::RemoteAreas;
use std::collections::BTreeMap;

/// Everything a billing run prices with.
#[derive(Debug, Clone, Default)]
pub struct Tariff {
    pub carriers: CarrierConfig,
    pub rates: RateCard,
    pub remote: RemoteAreas,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    UnknownCustomer(String),
    Rejected(Reject),
}

#[derive(Debug, Clone, Default)]
pub struct BillingOutcome {
    pub month: String,
    /// One invoice per customer with at least one billed shipment, by customer id.
    pub invoices: BTreeMap<String, Invoice>,
    /// Delivered shipments of the month that could not be billed: (carrier, tracking, why).
    pub skipped: Vec<(String, String, Skip)>,
    /// Shipments not delivered in the month.
    pub not_billable: usize,
}

impl BillingOutcome {
    pub fn total(&self) -> crate::money::Cents {
        self.invoices.values().map(Invoice::total).sum()
    }

    pub fn billed_count(&self) -> usize {
        self.invoices.values().map(|i| i.lines.len()).sum()
    }
}

/// Bills every shipment delivered in `month` (`YYYY-MM`, UTC).
pub fn bill_month(month: &str, shipments: &[RawShipment], customers: &Customers, tariff: &Tariff) -> BillingOutcome {
    let mut out = BillingOutcome { month: month.to_owned(), ..BillingOutcome::default() };
    for s in shipments {
        if !s.is_delivered() || s.status_at.month_key() != month {
            out.not_billable += 1;
            continue;
        }
        let Some(customer) = customers.get(&s.customer) else {
            out.skipped.push((s.carrier.to_owned(), s.tracking.clone(), Skip::UnknownCustomer(s.customer.clone())));
            continue;
        };
        let settings = tariff.carriers.get(s.carrier);
        match quote(s, customer, &settings, &tariff.rates, &tariff.remote) {
            Ok(q) => out
                .invoices
                .entry(customer.id.clone())
                .or_insert_with(|| Invoice::new(&customer.id, month))
                .lines
                .push(InvoiceLine::new(s, q)),
            Err(r) => out.skipped.push((s.carrier.to_owned(), s.tracking.clone(), Skip::Rejected(r))),
        }
    }
    for inv in out.invoices.values_mut() {
        inv.lines.sort_by(|a, b| (a.delivered, &a.carrier, &a.tracking).cmp(&(b.delivered, &b.carrier, &b.tracking)));
    }
    out
}
