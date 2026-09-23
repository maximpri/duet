mod common;
use common::sample;
use warehouse::inventory::pricing::price_with_discount;

#[test]
fn stock_behaviour_is_unchanged() {
    let mut inv = sample();
    assert_eq!(inv.restock("B-220", 7).unwrap(), 10);
    assert_eq!(inv.remove("C-305", 50).unwrap(), 200);
    let low: Vec<&str> = inv.low_stock(12).iter().map(|i| i.sku.as_str()).collect();
    assert_eq!(low, vec!["B-220", "A-100"]);
    assert_eq!(inv.category_totals()["tools"], price_with_discount(1899, 12) + price_with_discount(12999, 10));
}
