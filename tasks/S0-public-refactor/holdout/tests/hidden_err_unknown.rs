mod common;
use common::sample;
use warehouse::inventory::InventoryError;

#[test]
fn unknown_sku_is_typed() {
    let mut inv = sample();
    assert_eq!(inv.remove("Z-9", 1), Err(InventoryError::UnknownSku("Z-9".into())));
    assert_eq!(inv.restock("Z-9", 1), Err(InventoryError::UnknownSku("Z-9".into())));
}
