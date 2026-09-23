use super::Inventory;
use std::collections::BTreeMap;

/// Tiered volume discount: 5% from 10 units, 10% from 50, 15% from 200. Rounds down.
pub fn price_with_discount(unit_price_cents: u64, quantity: u32) -> u64 {
    let gross = unit_price_cents * u64::from(quantity);
    let percent_off = match quantity {
        0..=9 => 0,
        10..=49 => 5,
        50..=199 => 10,
        _ => 15,
    };
    gross * (100 - percent_off) / 100
}

impl Inventory {
    /// Total stock value per category, in cents, with volume discounts applied.
    pub fn category_totals(&self) -> BTreeMap<String, u64> {
        let mut totals = BTreeMap::new();
        for item in self.items.values() {
            *totals.entry(item.category.clone()).or_insert(0) +=
                price_with_discount(item.unit_price_cents, item.quantity);
        }
        totals
    }
}
