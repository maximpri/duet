use super::{price_with_discount, Inventory};

impl Inventory {
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
