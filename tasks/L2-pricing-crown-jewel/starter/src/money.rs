//! Money formatting. Amounts are integer cents throughout.

/// `123456` → `"1,234.56"`.
pub fn format_cents(cents: u64) -> String {
    let units = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in units.chars().enumerate() {
        if i > 0 && (units.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped}.{:02}", cents % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        assert_eq!(format_cents(0), "0.00");
        assert_eq!(format_cents(123_456), "1,234.56");
        assert_eq!(format_cents(100_000_005), "1,000,000.05");
    }
}
