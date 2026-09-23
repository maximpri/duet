//! Minimal CSV reading.

/// Splits one CSV line into fields.
pub fn split_line(line: &str) -> Vec<String> {
    line.split(',').map(|f| f.trim().to_string()).collect()
}

/// Parses a CSV document into a header and rows.
pub fn parse(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().map(split_line).unwrap_or_default();
    let rows = lines.map(split_line).collect();
    (header, rows)
}
