//! Warehouse inventory: stock, pricing and reporting.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub sku: String,
    pub name: String,
    pub category: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
}

#[derive(Debug, Default)]
pub struct Inventory {
    items: BTreeMap<String, Item>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, sku: &str) -> Option<&Item> {
        self.items.get(sku)
    }

    pub fn add_item(&mut self, item: Item) -> Result<(), String> {
        if item.quantity == 0 {
            return Err("quantity must be positive".to_string());
        }
        if self.items.contains_key(&item.sku) {
            return Err(format!("duplicate sku {}", item.sku));
        }
        self.items.insert(item.sku.clone(), item);
        Ok(())
    }

    pub fn remove(&mut self, sku: &str, quantity: u32) -> Result<u32, String> {
        if quantity == 0 {
            return Err("quantity must be positive".to_string());
        }
        let item = self.items.get_mut(sku).ok_or(format!("unknown sku {sku}"))?;
        if item.quantity < quantity {
            return Err(format!(
                "insufficient stock for {sku}: requested {quantity}, available {}",
                item.quantity
            ));
        }
        item.quantity -= quantity;
        Ok(item.quantity)
    }

    pub fn restock(&mut self, sku: &str, quantity: u32) -> Result<u32, String> {
        if quantity == 0 {
            return Err("quantity must be positive".to_string());
        }
        let item = self.items.get_mut(sku).ok_or(format!("unknown sku {sku}"))?;
        item.quantity += quantity;
        Ok(item.quantity)
    }

    /// SKUs at or below `threshold`, sorted by quantity then SKU.
    pub fn low_stock(&self, threshold: u32) -> Vec<&Item> {
        let mut low: Vec<&Item> = self.items.values().filter(|i| i.quantity <= threshold).collect();
        low.sort_by(|a, b| a.quantity.cmp(&b.quantity).then(a.sku.cmp(&b.sku)));
        low
    }

    /// Total stock value per category, in cents, with volume discounts applied.
    pub fn category_totals(&self) -> BTreeMap<String, u64> {
        let mut totals = BTreeMap::new();
        for item in self.items.values() {
            *totals.entry(item.category.clone()).or_insert(0) +=
                price_with_discount(item.unit_price_cents, item.quantity);
        }
        totals
    }

    pub fn render_report(&self) -> String {
        let mut out = String::from("SKU        NAME                 QTY      VALUE\n");
        let mut grand = 0;
        for item in self.items.values() {
            let value = price_with_discount(item.unit_price_cents, item.quantity);
            grand += value;
            out.push_str(&format!(
                "{:<10} {:<20} {:>4} {:>10}\n",
                item.sku,
                truncate(&item.name, 20),
                item.quantity,
                format_cents(value)
            ));
        }
        for (category, total) in self.category_totals() {
            out.push_str(&format!("{:<36}{:>10}\n", format!("total {category}"), format_cents(total)));
        }
        out.push_str(&format!("{:<36}{:>10}\n", "grand total", format_cents(grand)));
        out
    }
}

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

fn format_cents(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}
