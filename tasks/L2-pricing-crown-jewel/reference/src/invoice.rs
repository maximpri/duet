//! Invoices: an order priced for one customer.

use crate::catalog::{parse_sku, Sku, SkuError};
use crate::customers::CustomerBook;
use crate::money::format_cents;
use crate::catalog::Family;
use crate::pricing::{pricing_revision, quote_line_pooled};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLine {
    pub sku: Sku,
    pub qty: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub customer_id: String,
    pub lines: Vec<OrderLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoiceError {
    MissingCustomer,
    UnknownCustomer(String),
    BadLine { line: usize, text: String },
    Sku(SkuError),
}

impl fmt::Display for InvoiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvoiceError::MissingCustomer => write!(f, "the order names no customer"),
            InvoiceError::UnknownCustomer(id) => write!(f, "unknown customer {id}"),
            InvoiceError::BadLine { line, text } => write!(f, "line {line}: cannot read {text:?}"),
            InvoiceError::Sku(e) => write!(f, "{e}"),
        }
    }
}

impl Order {
    /// Parses an order: a `customer: <id>` line, then one `<SKU> <quantity>`
    /// per line. Blank lines and `#` comments are skipped.
    pub fn parse(text: &str) -> Result<Order, InvoiceError> {
        let mut customer_id = None;
        let mut lines = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(id) = line.strip_prefix("customer:") {
                customer_id = Some(id.trim().to_owned());
                continue;
            }
            let bad = || InvoiceError::BadLine {
                line: i + 1,
                text: line.to_owned(),
            };
            let mut parts = line.split_whitespace();
            let sku = parse_sku(parts.next().ok_or_else(bad)?).map_err(InvoiceError::Sku)?;
            let qty = parts
                .next()
                .and_then(|q| q.parse().ok())
                .filter(|q| *q > 0)
                .ok_or_else(bad)?;
            if parts.next().is_some() {
                return Err(bad());
            }
            lines.push(OrderLine { sku, qty });
        }
        Ok(Order {
            customer_id: customer_id.ok_or(InvoiceError::MissingCustomer)?,
            lines,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceLine {
    pub sku: String,
    pub qty: u32,
    /// List price of the line, in cents.
    pub list_cents: u64,
    /// Volume discount applied to the line, in basis points.
    pub discount_bps: u32,
    /// Line price after the discount, in cents.
    pub net_cents: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    pub customer_id: String,
    pub lines: Vec<InvoiceLine>,
    /// Sum of the lines' list prices.
    pub subtotal_cents: u64,
    /// Sum of the lines' discounts (`subtotal_cents - total_cents`).
    pub savings_cents: u64,
    /// Sum of the lines' net prices.
    pub total_cents: u64,
}

/// Prices every line of `order` for its customer. For customers who pool,
/// the volume discount of a line is decided by the invoice's total quantity
/// of that line's product family; otherwise by the line's own quantity.
pub fn build_invoice(order: &Order, book: &CustomerBook) -> Result<Invoice, InvoiceError> {
    let customer = book
        .get(&order.customer_id)
        .ok_or_else(|| InvoiceError::UnknownCustomer(order.customer_id.clone()))?;
    let mut per_family: BTreeMap<Family, u32> = BTreeMap::new();
    for l in &order.lines {
        *per_family.entry(l.sku.family).or_default() += l.qty;
    }
    let mut lines = Vec::new();
    for l in &order.lines {
        let pooled = if customer.pooling {
            per_family[&l.sku.family]
        } else {
            l.qty
        };
        let q = quote_line_pooled(&l.sku, l.qty, pooled, customer.tier);
        lines.push(InvoiceLine {
            sku: l.sku.to_string(),
            qty: l.qty,
            list_cents: q.list_cents,
            discount_bps: q.discount_bps,
            net_cents: q.net_cents,
        });
    }
    let subtotal_cents: u64 = lines.iter().map(|l| l.list_cents).sum();
    let total_cents: u64 = lines.iter().map(|l| l.net_cents).sum();
    Ok(Invoice {
        customer_id: order.customer_id.clone(),
        subtotal_cents,
        savings_cents: subtotal_cents - total_cents,
        total_cents,
        lines,
    })
}

/// The invoice as printed for the customer.
pub fn render(invoice: &Invoice) -> String {
    let mut out = format!("Invoice for {}\n", invoice.customer_id);
    for l in &invoice.lines {
        out.push_str(&format!(
            "{:<8} {:>6}  {:>12}  -{:>5.2}%  {:>12}\n",
            l.sku,
            l.qty,
            format_cents(l.list_cents),
            f64::from(l.discount_bps) / 100.0,
            format_cents(l.net_cents)
        ));
    }
    out.push_str(&format!(
        "Subtotal {:>12}\nSavings  {:>12}\nTotal    {:>12}\nPricing revision {}\n",
        format_cents(invoice.subtotal_cents),
        format_cents(invoice.savings_cents),
        format_cents(invoice.total_cents),
        pricing_revision()
    ));
    out
}
