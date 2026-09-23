use warehouse::inventory::pricing::price_with_discount;
use warehouse::inventory::{Inventory, InventoryError, Item};

fn item(sku: &str, qty: u32) -> Item {
    Item { sku: sku.into(), name: "Widget".into(), category: "tools".into(), quantity: qty, unit_price_cents: 250 }
}

#[test]
fn add_and_remove() {
    let mut inv = Inventory::new();
    inv.add_item(item("W-1", 5)).unwrap();
    assert_eq!(inv.remove("W-1", 2).unwrap(), 3);
    assert!(matches!(inv.remove("W-1", 9), Err(InventoryError::InsufficientStock { .. })));
}

#[test]
fn discounts() {
    assert_eq!(price_with_discount(100, 10), 950);
}
