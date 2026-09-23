//! `key=value` blocks separated by blank lines (or a terminator line).

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Block {
    /// Line number of the block's first key.
    pub line: usize,
    pub fields: BTreeMap<String, String>,
}

impl Block {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn require(&self, key: &str) -> Result<&str, String> {
        self.get(key).ok_or_else(|| format!("missing key {key}"))
    }
}

/// Parses blocks of `key<sep>value` lines. A blank line or a line equal to
/// `terminator` ends a block. Keys are lower-cased; `#` lines are comments.
pub fn parse_blocks(text: &str, sep: char, terminator: Option<&str>) -> Result<Vec<Block>, (usize, String)> {
    let mut blocks = Vec::new();
    let mut current = Block::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let end = line.is_empty() || terminator.is_some_and(|t| line == t);
        if end {
            if !current.fields.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once(sep).ok_or_else(|| (i + 1, format!("expected key{sep}value")))?;
        if current.fields.is_empty() {
            current.line = i + 1;
        }
        current.fields.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
    }
    if !current.fields.is_empty() {
        blocks.push(current);
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blank_separated_blocks() {
        let b = parse_blocks("A=1\nb = two\n\n\nc=3\n", '=', None).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].get("a"), Some("1"));
        assert_eq!(b[0].get("B"), Some("two"));
        assert_eq!(b[1].line, 5);
    }

    #[test]
    fn honours_terminators_and_separators() {
        let b = parse_blocks("id: 1\nEND\nid: 2\nEND\n", ':', Some("END")).unwrap();
        assert_eq!(b.iter().map(|x| x.get("id").unwrap()).collect::<Vec<_>>(), vec!["1", "2"]);
    }

    #[test]
    fn reports_malformed_lines() {
        assert_eq!(parse_blocks("a=1\nnonsense\n", '=', None).unwrap_err().0, 2);
        assert_eq!(Block::default().require("x").unwrap_err(), "missing key x");
    }
}
