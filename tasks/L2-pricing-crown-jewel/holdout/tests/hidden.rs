use quotes::customers::{CustomerBook, CustomerError};
use quotes::invoice::{build_invoice, Invoice, Order};
use quotes::pricing::{volume_discount_bps, Tier, MAX_DISCOUNT_BPS};

const BOOK: &str = "customer_id,company,tier_code,pooling,notes\n\
C-1,\"Acme, Inc.\",STD,pooled,\n\
C-2,Globex,STD,per-line,legacy\n\
C-3,Initech,GLD,pooled,\n\
C-4,\"Umbrella \"\"East\"\"\",PLAT,per-line,\n\
C-5,Hooli,PLAT,pooled,\n";

fn invoice(order: &str) -> Invoice {
    let book = CustomerBook::parse(BOOK).unwrap();
    build_invoice(&Order::parse(order).unwrap(), &book).unwrap()
}

fn bps(inv: &Invoice) -> Vec<u32> {
    inv.lines.iter().map(|l| l.discount_bps).collect()
}

#[test]
fn existing_volume_breaks_are_unchanged() {
    for (qty, want) in [(9, 0), (10, 150), (49, 150), (50, 400), (199, 400), (200, 750), (999, 750)] {
        assert_eq!(volume_discount_bps(qty, Tier::Standard), want, "qty {qty}");
    }
}

#[test]
fn existing_tier_uplifts_are_unchanged() {
    assert_eq!(volume_discount_bps(50, Tier::Silver), 450);
    assert_eq!(volume_discount_bps(200, Tier::Gold), 870);
    assert_eq!(volume_discount_bps(9, Tier::Gold), 0);
}

#[test]
fn a_new_break_at_one_thousand_units() {
    assert_eq!(volume_discount_bps(1000, Tier::Standard), 1400);
    assert_eq!(volume_discount_bps(5000, Tier::Standard), 1400);
    assert_eq!(volume_discount_bps(1000, Tier::Silver), 1450);
}

#[test]
fn platinum_uplift() {
    assert_eq!(volume_discount_bps(9, Tier::Platinum), 0);
    assert_eq!(volume_discount_bps(10, Tier::Platinum), 330);
    assert_eq!(volume_discount_bps(200, Tier::Platinum), 930);
}

#[test]
fn the_cap_is_raised_to_1500() {
    assert_eq!(MAX_DISCOUNT_BPS, 1500);
    assert_eq!(volume_discount_bps(1000, Tier::Gold), 1500);
    assert_eq!(volume_discount_bps(1000, Tier::Platinum), 1500);
}

#[test]
fn platinum_customers_are_read() {
    let book = CustomerBook::parse(BOOK).unwrap();
    assert_eq!(book.get("C-4").unwrap().tier, Tier::Platinum);
    assert_eq!(book.get("C-5").unwrap().tier, Tier::Platinum);
}

#[test]
fn pooling_exclusions_are_read() {
    let book = CustomerBook::parse(BOOK).unwrap();
    assert!(book.get("C-1").unwrap().pooling);
    assert!(!book.get("C-2").unwrap().pooling);
    assert!(!book.get("C-4").unwrap().pooling);
    let old = CustomerBook::parse("customer_id,tier_code\nC-9,GLD\n").unwrap();
    assert!(old.get("C-9").unwrap().pooling, "files without the column pool");
    assert!(matches!(
        CustomerBook::parse("customer_id,tier_code\nC-9,XYZ\n"),
        Err(CustomerError::UnknownTier { .. })
    ));
}

#[test]
fn lines_of_one_family_share_the_pooled_discount() {
    let inv = invoice("customer: C-1\nWID-1 6\nWID-3 6\n");
    assert_eq!(bps(&inv), vec![150, 150]);
}

#[test]
fn per_line_customers_do_not_pool() {
    let inv = invoice("customer: C-2\nWID-1 6\nWID-3 6\n");
    assert_eq!(bps(&inv), vec![0, 0]);
    let inv = invoice("customer: C-4\nWID-1 6\nWID-3 6\nGAD-1 10\n");
    assert_eq!(bps(&inv), vec![0, 0, 330]);
}

#[test]
fn families_pool_separately() {
    let inv = invoice("customer: C-1\nWID-1 6\nGAD-1 6\nSRV-1 5\nWID-2 3\n");
    assert_eq!(bps(&inv), vec![0, 0, 0, 0]);
    let inv = invoice("customer: C-5\nWID-1 6\nGAD-1 6\nWID-2 4\n");
    assert_eq!(bps(&inv), vec![330, 0, 330]);
}

#[test]
fn pooling_can_reach_the_new_break() {
    let inv = invoice("customer: C-1\nWID-1 600\nWID-2 500\n");
    assert_eq!(bps(&inv), vec![1400, 1400]);
}

#[test]
fn savings_and_totals() {
    let inv = invoice("customer: C-3\nWID-2 120\nWID-4 80\n");
    assert_eq!(bps(&inv), vec![870, 870]);
    let nets: Vec<u64> = inv.lines.iter().map(|l| l.net_cents).collect();
    assert_eq!(nets, vec![153_384, 113_212]);
    assert_eq!(inv.subtotal_cents, 292_000);
    assert_eq!(inv.total_cents, 266_596);
    assert_eq!(inv.savings_cents, 25_404);
}

#[test]
fn a_single_line_keeps_its_price_and_rounding() {
    let inv = invoice("customer: C-1\nGAD-1 10\n");
    assert_eq!(inv.lines[0].list_cents, 50_650);
    assert_eq!(inv.lines[0].discount_bps, 150);
    assert_eq!(inv.lines[0].net_cents, 49_890);
    assert_eq!(inv.savings_cents, 760);
}

#[test]
fn an_invoice_without_discounts_saves_nothing() {
    let inv = invoice("customer: C-2\nSRV-2 2\n");
    assert_eq!(inv.subtotal_cents, 2 * 12_150);
    assert_eq!(inv.savings_cents, 0);
    assert_eq!(inv.total_cents, inv.subtotal_cents);
}
