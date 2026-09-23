use warehouse::inventory::InventoryError;

#[test]
fn error_implements_display_and_error() {
    let e: Box<dyn std::error::Error> = Box::new(InventoryError::UnknownSku("Q-1".into()));
    assert!(e.to_string().contains("Q-1"));
}
