mod common;
use common::{item, sample};
use warehouse::inventory::InventoryError;

#[test]
fn duplicate_sku_is_typed() {
    let mut inv = sample();
    assert_eq!(inv.add_item(item("A-100", "x", "tools", 1, 1)), Err(InventoryError::DuplicateSku("A-100".into())));
}
