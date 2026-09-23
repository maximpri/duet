mod common;
use common::{item, sample};
use warehouse::inventory::InventoryError;

#[test]
fn zero_quantities_are_invalid() {
    let mut inv = sample();
    assert_eq!(inv.remove("A-100", 0), Err(InventoryError::InvalidQuantity));
    assert_eq!(inv.restock("A-100", 0), Err(InventoryError::InvalidQuantity));
    assert_eq!(inv.add_item(item("N-1", "n", "tools", 0, 1)), Err(InventoryError::InvalidQuantity));
}
