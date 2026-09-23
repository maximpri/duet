#![allow(dead_code)]
use warehouse::inventory::{Inventory, Item};

pub fn item(sku: &str, name: &str, cat: &str, q: u32, p: u64) -> Item {
    Item { sku: sku.into(), name: name.into(), category: cat.into(), quantity: q, unit_price_cents: p }
}

pub fn sample() -> Inventory {
    let mut inv = Inventory::new();
    inv.add_item(item("A-100", "Adjustable wrench", "tools", 12, 1899)).unwrap();
    inv.add_item(item("B-220", "Cordless drill with long battery life", "tools", 3, 12999)).unwrap();
    inv.add_item(item("C-305", "Safety goggles", "safety", 250, 499)).unwrap();
    inv.add_item(item("D-410", "Hi-vis vest", "safety", 60, 1250)).unwrap();
    inv
}
