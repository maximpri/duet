//! The August 2026 run over the real inputs of the workspace: every expectation
//! follows from docs/BILLING.md applied to config/, data/ and .env.

use parcelflow::app::{run_billing, BillingRun};
use parcelflow::billing::run::Skip;
use parcelflow::money::Cents;
use std::path::Path;

fn run() -> BillingRun {
    run_billing(Path::new("."), "2026-08").unwrap()
}

fn billed(run: &BillingRun, tracking: &str) -> Option<Cents> {
    run.outcome.invoices.values().flat_map(|i| &i.lines).find(|l| l.tracking == tracking).map(|l| l.quote.total)
}

#[test]
fn real_data_every_feed_parses() {
    let r = run();
    assert!(r.feeds.errors.is_empty(), "{:?}", r.feeds.errors);
    assert_eq!(r.feeds.files.len(), 30);
}

#[test]
fn real_data_no_kestrel_parcel_is_rejected() {
    let r = run();
    let ksx: Vec<_> =
        r.outcome.skipped.iter().filter(|(c, _, why)| c == "KSX" && matches!(why, Skip::Rejected(_))).collect();
    assert!(ksx.is_empty(), "{ksx:?}");
    assert_eq!(billed(&r, "KSX32442111"), Some(Cents(5027)));
}

#[test]
fn real_data_nordpost_deliveries_in_the_right_month() {
    let r = run();
    assert_eq!(billed(&r, "NRP84725145"), Some(Cents(648)));
    assert_eq!(billed(&r, "NRP12445869"), Some(Cents(1297)));
    assert_eq!(billed(&r, "NRP64544162"), None);
    assert_eq!(billed(&r, "NRP37013618"), None);
}

#[test]
fn real_data_counts() {
    let o = run().outcome;
    assert_eq!((o.billed_count(), o.skipped.len(), o.not_billable), (1308, 24, 364));
}

#[test]
fn real_data_customer_invoices() {
    let o = run().outcome;
    let total = |c: &str| o.invoices[c].total();
    assert_eq!(total("C-1002"), Cents(62086));
    assert_eq!(total("C-1009"), Cents(66232));
    assert_eq!(total("C-1016"), Cents(75112));
    assert_eq!(total("C-1039"), Cents(139859));
}

#[test]
fn real_data_grand_total() {
    assert_eq!(run().outcome.total(), Cents(4_576_122));
}
