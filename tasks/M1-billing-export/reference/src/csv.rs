//! Minimal RFC 4180-style CSV reading: quoted fields may contain commas and doubled quotes.

/// Splits one CSV line into fields.
pub fn split_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if field.trim().is_empty() => {
                field.clear();
                quoted = true;
            }
            (',', false) => fields.push(std::mem::take(&mut field).trim().to_string()),
            _ => field.push(c),
        }
    }
    fields.push(field.trim().to_string());
    fields
}

/// Parses a CSV document into a header and rows, skipping blank and `#` comment lines.
pub fn parse(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let header = lines.next().map(split_line).unwrap_or_default();
    let rows = lines.map(split_line).collect();
    (header, rows)
}

/// Quotes a field when it contains a comma or quote.
pub fn quote(field: &str) -> String {
    if field.contains(',') || field.contains('"') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}
