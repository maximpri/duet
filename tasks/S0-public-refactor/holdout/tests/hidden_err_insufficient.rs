mod common;
use common::sample;
use warehouse::inventory::InventoryError;

#[test]
fn insufficient_stock_is_typed() {
    let mut inv = sample();
    assert_eq!(
        inv.remove("B-220", 5),
        Err(InventoryError::InsufficientStock { sku: "B-220".into(), requested: 5, available: 3 })
    );
}
