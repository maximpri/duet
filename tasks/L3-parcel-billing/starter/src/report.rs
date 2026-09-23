//! Text reports for the command line.

use crate::app::BillingRun;
use crate::billing::run::Skip;
use crate::money::Cents;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Per-carrier feed summary, skipped shipments and per-customer totals.
pub fn summary(run: &BillingRun) -> String {
    let mut out = String::new();
    let o = &run.outcome;
    let _ = writeln!(out, "billing month {}", o.month);
    let _ = writeln!(out, "carrier config {}", run.settings.carrier_config.display());
    let _ = writeln!(out, "\nFEEDS");
    for (file, code, n) in &run.feeds.files {
        let _ = writeln!(out, "  {code:<4} {file:<28} {n:>5} shipments");
    }
    for (file, err) in &run.feeds.errors {
        let _ = writeln!(out, "  FAILED {file}: {err}");
    }
    let mut per_carrier: BTreeMap<&str, (usize, Cents)> = BTreeMap::new();
    for inv in o.invoices.values() {
        for l in &inv.lines {
            let e = per_carrier.entry(l.carrier.as_str()).or_default();
            e.0 += 1;
            e.1 += l.quote.total;
        }
    }
    let _ = writeln!(out, "\nBILLED PER CARRIER");
    for (code, (n, total)) in &per_carrier {
        let _ = writeln!(out, "  {code:<4} {n:>5} shipments {total:>12}");
    }
    let _ = writeln!(out, "\nSKIPPED ({})", o.skipped.len());
    for (carrier, tracking, why) in &o.skipped {
        let why = match why {
            Skip::UnknownCustomer(c) => format!("unknown customer {c}"),
            Skip::Rejected(r) => r.to_string(),
        };
        let _ = writeln!(out, "  {carrier:<4} {tracking:<18} {why}");
    }
    let _ = writeln!(out, "\nINVOICES");
    for inv in o.invoices.values() {
        let company = run.customers.get(&inv.customer).map(|c| c.company.as_str()).unwrap_or("?");
        let _ = writeln!(out, "  {:<8} {:<40} {:>4} lines {:>12}", inv.customer, company, inv.lines.len(), inv.total());
    }
    let _ = writeln!(
        out,
        "\n{} shipments billed, {} skipped, {} not billable this month",
        o.billed_count(),
        o.skipped.len(),
        o.not_billable
    );
    let _ = writeln!(out, "grand total {}", o.total());
    out
}

/// One line per priced shipment.
pub fn price_lines(run: &BillingRun) -> String {
    let mut out = String::new();
    for inv in run.outcome.invoices.values() {
        for l in &inv.lines {
            let q = &l.quote;
            let _ = writeln!(
                out,
                "PRICE {} {} {} {} {} z{} billable {} base {} fuel {} remote {} oversize {} disc {} total {}",
                l.carrier,
                l.tracking,
                inv.customer,
                l.service,
                l.dest,
                q.zone,
                q.billable.kg_string(),
                q.base,
                q.surcharges.fuel,
                q.surcharges.remote,
                q.surcharges.oversize,
                q.discount,
                q.total
            );
        }
    }
    out
}
