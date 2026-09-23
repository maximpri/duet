//! Delimited text with optional quoting (`"a, b"`, `""` escapes).

use crate::error::{Error, Result};

/// Splits one line on `delim`, honouring double quotes.
pub fn split_line(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.trim().is_empty() {
            field.clear();
            quoted = true;
        } else if c == delim {
            out.push(std::mem::take(&mut field));
        } else {
            field.push(c);
        }
    }
    out.push(field);
    out
}

/// A delimited file with a header row.
#[derive(Debug, Clone)]
pub struct Table {
    pub header: Vec<String>,
    /// (line number, fields)
    pub rows: Vec<(usize, Vec<String>)>,
}

impl Table {
    /// Parses `text`; the first content line is the header. Header names are
    /// trimmed and compared case-insensitively.
    pub fn parse(text: &str, delim: char) -> Result<Table> {
        let mut lines = super::content_lines(text);
        let (_, head) = lines.next().ok_or_else(|| Error::data("table", 0, "empty file"))?;
        let header = split_line(head, delim).into_iter().map(|h| h.trim().to_ascii_lowercase()).collect();
        let rows = lines.map(|(n, l)| (n, split_line(l, delim))).collect();
        Ok(Table { header, rows })
    }

    /// Column index of `name` (case-insensitive).
    pub fn column(&self, name: &str) -> Option<usize> {
        let name = name.to_ascii_lowercase();
        self.header.iter().position(|h| *h == name)
    }

    pub fn has_column(&self, name: &str) -> bool {
        self.column(name).is_some()
    }

    /// Records keyed by header name.
    pub fn records(&self) -> impl Iterator<Item = Record<'_>> {
        self.rows.iter().map(move |(line, fields)| Record { table: self, line: *line, fields })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Record<'a> {
    table: &'a Table,
    pub line: usize,
    fields: &'a [String],
}

impl<'a> Record<'a> {
    /// The trimmed field under `name`, if the column exists.
    pub fn get(&self, name: &str) -> Option<&'a str> {
        let i = self.table.column(name)?;
        self.fields.get(i).map(|s| s.trim())
    }

    /// The trimmed field under `name`, or an error naming the column.
    pub fn require(&self, name: &str) -> std::result::Result<&'a str, String> {
        self.get(name).ok_or_else(|| format!("missing column {name}"))
    }

    pub fn field(&self, index: usize) -> Option<&'a str> {
        self.fields.get(index).map(|s| s.trim())
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_fields() {
        assert_eq!(split_line(r#"a,"b, c",d"#, ','), vec!["a", "b, c", "d"]);
        assert_eq!(split_line(r#""say ""hi""";x"#, ';'), vec![r#"say "hi""#, "x"]);
        assert_eq!(split_line("a||b", '|'), vec!["a", "", "b"]);
        assert_eq!(split_line("a\tb", '\t'), vec!["a", "b"]);
    }

    #[test]
    fn tables_look_up_columns_by_name() {
        let t = Table::parse("# feed\nTracking;Weight\nA1; 2.5 \nB2;3\n", ';').unwrap();
        assert_eq!(t.column("weight"), Some(1));
        let recs: Vec<_> = t.records().collect();
        assert_eq!(recs[0].get("WEIGHT"), Some("2.5"));
        assert_eq!(recs[1].line, 4);
        assert_eq!(recs[1].require("missing").unwrap_err(), "missing column missing");
    }

    #[test]
    fn empty_tables_are_errors() {
        assert!(Table::parse("\n# only comments\n", ',').is_err());
    }
}
