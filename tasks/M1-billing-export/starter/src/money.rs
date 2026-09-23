//! Money formatting.

/// Formats an amount held in minor units (cents) as a decimal string.
pub fn format_minor(amount_minor: i64) -> String {
    let sign = if amount_minor < 0 { "-" } else { "" };
    let abs = amount_minor.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}
