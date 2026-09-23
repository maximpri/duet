use warehouse::inventory::pricing::price_with_discount;
use warehouse::inventory::{Inventory, InventoryError, Item};

fn item(sku: &str, name: &str, cat: &str, q: u32, p: u64) -> Item {
    Item { sku: sku.into(), name: name.into(), category: cat.into(), quantity: q, unit_price_cents: p }
}

fn sample() -> Inventory {
    let mut inv = Inventory::new();
    inv.add_item(item("A-100", "Adjustable wrench", "tools", 12, 1899)).unwrap();
    inv.add_item(item("B-220", "Cordless drill with long battery life", "tools", 3, 12999)).unwrap();
    inv.add_item(item("C-305", "Safety goggles", "safety", 250, 499)).unwrap();
    inv.add_item(item("D-410", "Hi-vis vest", "safety", 60, 1250)).unwrap();
    inv
}

const REPORT: &str = "\
SKU        NAME                 QTY      VALUE\n\
A-100      Adjustable wrench      12     216.48\n\
B-220      Cordless drill with…    3     389.97\n\
C-305      Safety goggles        250    1060.37\n\
D-410      Hi-vis vest            60     675.00\n\
total safety                           1735.37\n\
total tools                             606.45\n\
grand total                            2341.82\n\
";

#[test]
fn report_text_is_unchanged() {
    assert_eq!(sample().render_report(), REPORT);
}

#[test]
fn pricing_lives_in_its_own_module() {
    assert_eq!(price_with_discount(100, 9), 900);
    assert_eq!(price_with_discount(100, 10), 950);
    assert_eq!(price_with_discount(100, 50), 4500);
    assert_eq!(price_with_discount(100, 200), 17000);
}

#[test]
fn duplicate_sku_is_typed() {
    let mut inv = sample();
    assert_eq!(inv.add_item(item("A-100", "x", "tools", 1, 1)), Err(InventoryError::DuplicateSku("A-100".into())));
}

#[test]
fn unknown_sku_is_typed() {
    let mut inv = sample();
    assert_eq!(inv.remove("Z-9", 1), Err(InventoryError::UnknownSku("Z-9".into())));
    assert_eq!(inv.restock("Z-9", 1), Err(InventoryError::UnknownSku("Z-9".into())));
}

#[test]
fn insufficient_stock_is_typed() {
    let mut inv = sample();
    assert_eq!(
        inv.remove("B-220", 5),
        Err(InventoryError::InsufficientStock { sku: "B-220".into(), requested: 5, available: 3 })
    );
}

#[test]
fn zero_quantities_are_invalid() {
    let mut inv = sample();
    assert_eq!(inv.remove("A-100", 0), Err(InventoryError::InvalidQuantity));
    assert_eq!(inv.restock("A-100", 0), Err(InventoryError::InvalidQuantity));
    assert_eq!(inv.add_item(item("N-1", "n", "tools", 0, 1)), Err(InventoryError::InvalidQuantity));
}

#[test]
fn error_implements_display_and_error() {
    let e: Box<dyn std::error::Error> = Box::new(InventoryError::UnknownSku("Q-1".into()));
    assert!(e.to_string().contains("Q-1"));
}

#[test]
fn stock_behaviour_is_unchanged() {
    let mut inv = sample();
    assert_eq!(inv.restock("B-220", 7).unwrap(), 10);
    assert_eq!(inv.remove("C-305", 50).unwrap(), 200);
    let low: Vec<&str> = inv.low_stock(12).iter().map(|i| i.sku.as_str()).collect();
    assert_eq!(low, vec!["B-220", "A-100"]);
    assert_eq!(inv.category_totals()["tools"], price_with_discount(1899, 12) + price_with_discount(12999, 10));
}
