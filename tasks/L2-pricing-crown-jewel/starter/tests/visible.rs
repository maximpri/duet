use quotes::catalog::parse_sku;
use quotes::customers::CustomerBook;
use quotes::invoice::{build_invoice, Order};
use quotes::pricing::{volume_discount_bps, Tier};

const BOOK: &str = "customer_id,company,tier_code\nC-1,\"Acme, Inc.\",STD\nC-2,Globex,GLD\n";

#[test]
fn small_orders_get_no_volume_discount() {
    assert_eq!(volume_discount_bps(1, Tier::Standard), 0);
    assert_eq!(volume_discount_bps(9, Tier::Gold), 0);
}

#[test]
fn discounts_grow_with_quantity() {
    let a = volume_discount_bps(10, Tier::Standard);
    let b = volume_discount_bps(60, Tier::Standard);
    let c = volume_discount_bps(250, Tier::Standard);
    assert!(0 < a && a < b && b < c, "{a} {b} {c}");
    assert!(volume_discount_bps(250, Tier::Gold) > c);
}

#[test]
fn customers_parse_by_column_name() {
    let book = CustomerBook::parse(BOOK).unwrap();
    assert_eq!(book.len(), 2);
    assert_eq!(book.get("C-2").unwrap().tier, Tier::Gold);
}

#[test]
fn a_single_line_invoice_is_the_quoted_line() {
    let book = CustomerBook::parse(BOOK).unwrap();
    let order = Order::parse("customer: C-1\nWID-2 3\n").unwrap();
    let inv = build_invoice(&order, &book).unwrap();
    assert_eq!(inv.lines.len(), 1);
    assert_eq!(inv.lines[0].sku, "WID-2");
    assert_eq!(inv.lines[0].discount_bps, 0);
    assert_eq!(inv.total_cents, inv.subtotal_cents);
    assert_eq!(parse_sku("SRV-1").unwrap().to_string(), "SRV-1");
}
