//! Readers for the text formats carriers send: delimited files, key=value
//! blocks, fixed-width records and JSON lines.

pub mod csv;
pub mod fixed;
pub mod json;
pub mod kv;

/// Lines of `text` with their 1-based numbers, skipping blank lines and lines
/// starting with `#`.
pub fn content_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim_end_matches('\r')))
        .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
}

#[cfg(test)]
mod tests {
    #[test]
    fn content_lines_skip_blanks_and_comments() {
        let lines: Vec<_> = super::content_lines("# head\n\na\r\n  # x\nb\n").collect();
        assert_eq!(lines, vec![(3, "a"), (5, "b")]);
    }
}
