//! Fixed-width records.

/// A field layout: (name, start column (0-based), width).
pub type Layout = &'static [(&'static str, usize, usize)];

/// The trimmed field `name` of `line` under `layout`; `None` if the layout has
/// no such field or the line is too short.
pub fn field<'a>(line: &'a str, layout: Layout, name: &str) -> Option<&'a str> {
    let &(_, start, width) = layout.iter().find(|(n, _, _)| *n == name)?;
    let end = (start + width).min(line.len());
    if start >= line.len() || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
        return None;
    }
    Some(line[start..end].trim())
}

/// The minimum line length a layout needs.
pub fn record_len(layout: Layout) -> usize {
    layout.iter().map(|(_, s, w)| s + w).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: Layout = &[("id", 0, 4), ("kg", 4, 6), ("svc", 10, 3)];

    #[test]
    fn slices_fields() {
        let line = "A12 002.50EXP";
        assert_eq!(field(line, LAYOUT, "id"), Some("A12"));
        assert_eq!(field(line, LAYOUT, "kg"), Some("002.50"));
        assert_eq!(field(line, LAYOUT, "svc"), Some("EXP"));
        assert_eq!(field(line, LAYOUT, "nope"), None);
        assert_eq!(field("A1", LAYOUT, "svc"), None);
        assert_eq!(record_len(LAYOUT), 13);
    }
}
