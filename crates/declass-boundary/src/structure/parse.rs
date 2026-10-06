// SPDX-License-Identifier: GPL-3.0-or-later
//! Parsers that keep where every value is: JSON (and JSON lines),
//! delimited text (CSV, TSV and the like, quoting as RFC 4180), `KEY=value`
//! files, fixed-width columns and simple XML. Every value carries the byte
//! range it was read from, so a synthetic twin can replace values in the
//! original text and keep everything else (layout, key order, quoting,
//! whitespace, duplicate keys) byte for byte.

use std::ops::Range;

/// Deepest nesting read; deeper documents are not parsed (a structure view
/// is not worth a stack overflow on hostile input).
pub const MAX_DEPTH: usize = 128;

// ---- JSON ------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Json {
    Object {
        span: Range<usize>,
        entries: Vec<(Key, Json)>,
    },
    Array {
        span: Range<usize>,
        items: Vec<Json>,
    },
    String {
        span: Range<usize>,
        value: String,
    },
    /// The literal as written (`7306743338619943`, `19.90`, `1e-3`).
    Number {
        span: Range<usize>,
    },
    Bool {
        span: Range<usize>,
        value: bool,
    },
    Null {
        span: Range<usize>,
    },
}

#[derive(Debug, Clone)]
pub struct Key {
    /// The key's string literal, quotes included.
    pub span: Range<usize>,
    pub name: String,
}

impl Json {
    pub fn span(&self) -> Range<usize> {
        match self {
            Json::Object { span, .. }
            | Json::Array { span, .. }
            | Json::String { span, .. }
            | Json::Number { span }
            | Json::Bool { span, .. }
            | Json::Null { span } => span.clone(),
        }
    }
}

struct JsonParser<'a> {
    b: &'a [u8],
    text: &'a str,
    at: usize,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while self.at < self.b.len() && matches!(self.b[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        let start = self.at;
        match *self.b.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut entries = Vec::new();
                self.ws();
                if self.b.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Some(Json::Object {
                        span: start..self.at,
                        entries,
                    });
                }
                loop {
                    self.ws();
                    let key_start = self.at;
                    let name = self.string()?;
                    let key = Key {
                        span: key_start..self.at,
                        name,
                    };
                    self.ws();
                    if self.b.get(self.at) != Some(&b':') {
                        return None;
                    }
                    self.at += 1;
                    let v = self.value(depth + 1)?;
                    entries.push((key, v));
                    self.ws();
                    match self.b.get(self.at)? {
                        b',' => self.at += 1,
                        b'}' => {
                            self.at += 1;
                            return Some(Json::Object {
                                span: start..self.at,
                                entries,
                            });
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.ws();
                if self.b.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Some(Json::Array {
                        span: start..self.at,
                        items,
                    });
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.b.get(self.at)? {
                        b',' => self.at += 1,
                        b']' => {
                            self.at += 1;
                            return Some(Json::Array {
                                span: start..self.at,
                                items,
                            });
                        }
                        _ => return None,
                    }
                }
            }
            b'"' => {
                let value = self.string()?;
                Some(Json::String {
                    span: start..self.at,
                    value,
                })
            }
            b't' => self.word(b"true", |span| Json::Bool { span, value: true }),
            b'f' => self.word(b"false", |span| Json::Bool { span, value: false }),
            b'n' => self.word(b"null", |span| Json::Null { span }),
            b'-' | b'0'..=b'9' => {
                if self.b[self.at] == b'-' {
                    self.at += 1;
                }
                let digits = |p: &mut Self| {
                    let s = p.at;
                    while p.at < p.b.len() && p.b[p.at].is_ascii_digit() {
                        p.at += 1;
                    }
                    p.at > s
                };
                if !digits(self) {
                    return None;
                }
                if self.b.get(self.at) == Some(&b'.') {
                    self.at += 1;
                    if !digits(self) {
                        return None;
                    }
                }
                if matches!(self.b.get(self.at), Some(b'e' | b'E')) {
                    self.at += 1;
                    if matches!(self.b.get(self.at), Some(b'+' | b'-')) {
                        self.at += 1;
                    }
                    if !digits(self) {
                        return None;
                    }
                }
                Some(Json::Number {
                    span: start..self.at,
                })
            }
            _ => None,
        }
    }

    fn word(&mut self, w: &[u8], make: impl Fn(Range<usize>) -> Json) -> Option<Json> {
        if self.b[self.at..].starts_with(w) {
            let start = self.at;
            self.at += w.len();
            Some(make(start..self.at))
        } else {
            None
        }
    }

    /// A string literal at `at` (which holds `"`), decoded.
    fn string(&mut self) -> Option<String> {
        if self.b.get(self.at) != Some(&b'"') {
            return None;
        }
        let start = self.at;
        self.at += 1;
        let mut escaped = false;
        loop {
            let c = *self.b.get(self.at)?;
            self.at += 1;
            match c {
                b'\\' => {
                    escaped = true;
                    self.at += 1;
                }
                b'"' => break,
                b'\n' => return None,
                _ => {}
            }
        }
        let raw = &self.text[start..self.at];
        if !escaped {
            return Some(raw[1..raw.len() - 1].to_owned());
        }
        serde_json::from_str::<String>(raw).ok()
    }
}

/// `text` as one JSON document, if it is one (surrounding whitespace allowed).
pub fn json(text: &str) -> Option<Json> {
    let mut p = JsonParser {
        b: text.as_bytes(),
        text,
        at: 0,
    };
    let v = p.value(0)?;
    p.ws();
    (p.at == text.len()).then_some(v)
}

/// `text` as JSON lines: every non-empty line one JSON object or array,
/// two or more of them. Each value's spans are in `text`.
pub fn json_lines(text: &str) -> Option<Vec<Json>> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if !body.trim().is_empty() {
            if !body.trim_start().starts_with(['{', '[']) {
                return None;
            }
            let mut p = JsonParser {
                b: text.as_bytes(),
                text,
                at: offset,
            };
            let v = p.value(0)?;
            let end = offset + body.len();
            if v.span().end > end || !text[v.span().end..end].trim().is_empty() {
                return None;
            }
            out.push(v);
        }
        offset += line.len();
    }
    (out.len() >= 2).then_some(out)
}

// ---- Delimited text --------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Cell {
    /// The field as written, quotes included.
    pub span: Range<usize>,
    /// The field's value (quotes removed, `""` read as `"`).
    pub value: String,
    pub quoted: bool,
}

#[derive(Debug, Clone)]
pub struct Row {
    /// The record without its line break.
    pub span: Range<usize>,
    pub cells: Vec<Cell>,
}

/// Rows of `text` split at `delimiter`, fields quoted with `"` as RFC 4180
/// (a quoted field may hold the delimiter, `""` and line breaks). Empty
/// lines are skipped. `None` when a quote is never closed.
pub fn delimited(text: &str, delimiter: char) -> Option<Vec<Row>> {
    let b = text.as_bytes();
    let d = delimiter as u8;
    let mut rows = Vec::new();
    let mut at = 0;
    while at < b.len() {
        // Skip blank lines.
        if b[at] == b'\n' || b[at] == b'\r' {
            at += 1;
            continue;
        }
        let row_start = at;
        let mut cells = Vec::new();
        loop {
            let start = at;
            if b.get(at) == Some(&b'"') {
                at += 1;
                let mut value = String::new();
                let mut piece = at;
                loop {
                    match b.get(at) {
                        None => return None,
                        Some(b'"') if b.get(at + 1) == Some(&b'"') => {
                            value.push_str(&text[piece..=at]);
                            at += 2;
                            piece = at;
                        }
                        Some(b'"') => {
                            value.push_str(&text[piece..at]);
                            at += 1;
                            break;
                        }
                        Some(_) => at += 1,
                    }
                }
                // Text after the closing quote belongs to the field.
                let tail = at;
                while at < b.len() && b[at] != d && b[at] != b'\n' && b[at] != b'\r' {
                    at += 1;
                }
                value.push_str(&text[tail..at]);
                cells.push(Cell {
                    span: start..at,
                    value,
                    quoted: true,
                });
            } else {
                while at < b.len() && b[at] != d && b[at] != b'\n' && b[at] != b'\r' {
                    at += 1;
                }
                cells.push(Cell {
                    span: start..at,
                    value: text[start..at].to_owned(),
                    quoted: false,
                });
            }
            if b.get(at) == Some(&d) {
                at += 1;
                continue;
            }
            break;
        }
        let row_end = at;
        if b.get(at) == Some(&b'\r') {
            at += 1;
        }
        if b.get(at) == Some(&b'\n') {
            at += 1;
        }
        rows.push(Row {
            span: row_start..row_end,
            cells,
        });
    }
    Some(rows)
}

/// The delimiter `text` is written with, if it is delimited text: the
/// candidate (`hint` first, then `,`, tab, `;`, `|`) that splits the first
/// lines into the same number (2 or more) of fields on nearly every line.
pub fn sniff_delimiter(text: &str, hint: Option<char>) -> Option<char> {
    let candidates: Vec<char> = hint.into_iter().chain([',', '\t', ';', '|']).collect();
    let sample: String = text.lines().take(40).collect::<Vec<_>>().join("\n");
    for d in candidates {
        let Some(rows) = delimited(&sample, d) else {
            continue;
        };
        if rows.len() < 2 {
            continue;
        }
        let widths: Vec<usize> = rows.iter().map(|r| r.cells.len()).collect();
        let first = widths[0];
        if first < 2 {
            continue;
        }
        // The file's extension says it is delimited: most rows agreeing is
        // enough (ragged rows are an anomaly to report, not a reason to
        // read it as prose).
        let share = if Some(d) == hint { 6 } else { 9 };
        let same = widths.iter().filter(|&&w| w == first).count();
        if same * 10 >= widths.len() * share {
            return Some(d);
        }
    }
    None
}

// ---- KEY=value -------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Assignment {
    pub key: String,
    /// The value as written (quotes included).
    pub span: Range<usize>,
    /// The value without surrounding quotes.
    pub value: String,
    /// The value was quoted, and with which quote.
    pub quote: Option<char>,
    /// The line starts with `export `.
    pub export: bool,
}

/// The assignments of a `KEY=value` file (`export` allowed, `#` comments
/// skipped).
pub fn assignments(text: &str) -> Vec<Assignment> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let lead = body.len() - trimmed.len();
        let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let skip = lead + (trimmed.len() - rest.len());
        if !rest.starts_with('#')
            && let Some((key, value)) = rest.split_once('=')
            && is_env_key(key.trim())
        {
            let vstart = offset + skip + key.len() + 1;
            let v = value.trim_end();
            let lead_ws = v.len() - v.trim_start().len();
            let v = v.trim_start();
            let start = vstart + lead_ws;
            let quote = v
                .chars()
                .next()
                .filter(|q| (*q == '"' || *q == '\'') && v.len() >= 2 && v.ends_with(*q));
            let inner = match quote {
                Some(_) => &v[1..v.len() - 1],
                None => v,
            };
            out.push(Assignment {
                key: key.trim().to_owned(),
                span: start..start + v.len(),
                value: inner.to_owned(),
                quote,
                export: rest.len() < trimmed.len(),
            });
        }
        offset += line.len();
    }
    out
}

fn is_env_key(k: &str) -> bool {
    let mut chars = k.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// Whether `text` reads as a `KEY=value` file: most of its lines that are
/// neither empty nor comments are assignments, and there are some.
pub fn looks_like_env(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let n = assignments(text).len();
    n >= 1 && n * 10 >= lines.len() * 8
}

// ---- Fixed-width columns ---------------------------------------------------

/// Column ranges (in characters) of fixed-width text: lines of equal length
/// (5 or more, ASCII) with at least two columns separated by positions
/// that are blank on every line.
pub fn fixed_width(text: &str) -> Option<Vec<Range<usize>>> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.len() < 5 || !lines.iter().all(|l| l.is_ascii()) {
        return None;
    }
    let width = lines.iter().map(|l| l.len()).max()?;
    let same = lines.iter().filter(|l| l.len() == width).count();
    if same * 10 < lines.len() * 8 || width < 3 {
        return None;
    }
    let blank: Vec<bool> = (0..width)
        .map(|i| {
            lines
                .iter()
                .all(|l| l.as_bytes().get(i).is_none_or(|b| *b == b' '))
        })
        .collect();
    let mut cols = Vec::new();
    let mut start = None;
    for (i, &b) in blank.iter().enumerate() {
        match (b, start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                cols.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        cols.push(s..width);
    }
    // Delimited text or prose is not fixed-width: every column must be
    // filled on most lines.
    let filled = cols.iter().all(|c| {
        lines
            .iter()
            .filter(|l| l.get(c.clone()).is_some_and(|s| !s.trim().is_empty()))
            .count()
            * 10
            >= lines.len() * 8
    });
    (cols.len() >= 2 && filled).then_some(cols)
}

// ---- Simple XML ------------------------------------------------------------

/// A piece of simple XML: an element's text or an attribute's value, with
/// the element path it belongs to.
#[derive(Debug, Clone)]
pub struct XmlValue {
    /// `root/record/name`, or `root/record/@id` for an attribute.
    pub path: String,
    pub span: Range<usize>,
    pub value: String,
    /// Index of the record (child of the root) it is in, if any.
    pub record: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Xml {
    pub root: String,
    /// Byte ranges of the root's child elements.
    pub records: Vec<Range<usize>>,
    pub record_names: Vec<String>,
    pub values: Vec<XmlValue>,
    /// Where the root's content starts and ends (between its tags).
    pub inner: Range<usize>,
}

/// `text` as simple XML: elements, attributes, text, comments, a
/// declaration; no DTD, no CDATA, no mixed content across records. `None`
/// for anything else.
pub fn xml(text: &str) -> Option<Xml> {
    let t = text.trim_start();
    if !t.starts_with('<') {
        return None;
    }
    let b = text.as_bytes();
    let mut at = text.len() - t.len();
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut root: Option<String> = None;
    let mut inner = 0..0;
    let mut records = Vec::new();
    let mut record_names = Vec::new();
    let mut record_start = 0;
    let mut values = Vec::new();
    let path = |stack: &[(String, usize)]| {
        stack
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join("/")
    };
    while at < b.len() {
        if b[at] == b'<' {
            let rest = &text[at..];
            if rest.starts_with("<?") {
                at += rest.find("?>")? + 2;
                continue;
            }
            if rest.starts_with("<!--") {
                at += rest.find("-->")? + 3;
                continue;
            }
            if rest.starts_with("<!") {
                return None;
            }
            let end = at + rest.find('>')?;
            let tag = &text[at + 1..end];
            if let Some(name) = tag.strip_prefix('/') {
                let (open, _) = stack.pop()?;
                if open != name.trim() {
                    return None;
                }
                if stack.len() == 1 {
                    records.push(record_start..end + 1);
                    record_names.push(open);
                } else if stack.is_empty() {
                    inner.end = at;
                }
                at = end + 1;
                continue;
            }
            let selfclosing = tag.ends_with('/');
            let tag = tag.trim_end_matches('/');
            let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
            let name = &tag[..name_end];
            if name.is_empty() || stack.len() > MAX_DEPTH {
                return None;
            }
            if stack.is_empty() {
                if root.is_some() {
                    return None;
                }
                root = Some(name.to_owned());
                inner.start = end + 1;
            } else if stack.len() == 1 {
                record_start = at;
            }
            stack.push((name.to_owned(), at));
            let record = (stack.len() >= 2).then_some(records.len());
            // Attributes: name="value" or name='value'.
            let mut a = at + 1 + name_end;
            let attrs_end = at + 1 + tag.len();
            while a < attrs_end {
                let rest = &text[a..attrs_end];
                let Some(eq) = rest.find('=') else { break };
                let aname = rest[..eq].trim();
                let after = &rest[eq + 1..];
                let lead = after.len() - after.trim_start().len();
                let q = after.trim_start().chars().next()?;
                if q != '"' && q != '\'' {
                    return None;
                }
                let vstart = a + eq + 1 + lead + 1;
                let vlen = text[vstart..attrs_end].find(q)?;
                values.push(XmlValue {
                    path: format!("{}/@{aname}", path(&stack)),
                    span: vstart..vstart + vlen,
                    value: text[vstart..vstart + vlen].to_owned(),
                    record,
                });
                a = vstart + vlen + 1;
            }
            if selfclosing {
                stack.pop();
                if stack.len() == 1 {
                    records.push(record_start..end + 1);
                    record_names.push(name.to_owned());
                }
            }
            at = end + 1;
        } else {
            let end = text[at..].find('<').map_or(text.len(), |i| at + i);
            let body = &text[at..end];
            if !body.trim().is_empty() {
                if stack.is_empty() {
                    return None;
                }
                let lead = body.len() - body.trim_start().len();
                let v = body.trim();
                values.push(XmlValue {
                    path: path(&stack),
                    span: at + lead..at + lead + v.len(),
                    value: v.to_owned(),
                    record: (stack.len() >= 2).then_some(records.len()),
                });
            }
            at = end;
        }
    }
    if !stack.is_empty() {
        return None;
    }
    Some(Xml {
        root: root?,
        records,
        record_names,
        values,
        inner,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_keeps_spans_duplicates_and_big_numbers() {
        let text = r#"{"a": 7306743338619943, "a": "x\"y", "b": [true, null, 1.50]}"#;
        let Some(Json::Object { entries, .. }) = json(text) else {
            panic!("not parsed")
        };
        assert_eq!(entries.len(), 3);
        assert_eq!(&text[entries[0].1.span()], "7306743338619943");
        assert!(matches!(&entries[1].1, Json::String { value, .. } if value == "x\"y"));
        assert_eq!(&text[entries[0].0.span.clone()], "\"a\"");
        let Json::Array { items, .. } = &entries[2].1 else {
            panic!()
        };
        assert_eq!(&text[items[2].span()], "1.50");
        assert!(json("{\"a\": }").is_none());
        assert!(json(&"[".repeat(MAX_DEPTH + 5)).is_none());
    }

    #[test]
    fn json_lines_need_two_documents() {
        let text = "{\"a\":1}\n\n{\"a\":2}\n";
        let docs = json_lines(text).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(&text[docs[1].span()], "{\"a\":2}");
        assert!(json_lines("{\"a\":1}\n").is_none());
        assert!(json_lines("{\"a\":1}\nplain\n").is_none());
    }

    #[test]
    fn delimited_fields_follow_rfc_4180_quoting() {
        let text = "id,name,note\r\n1,\"Stone, A\",\"say \"\"hi\"\"\"\n2,B,\"two\nlines\"\n";
        let rows = delimited(text, ',').unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].cells[1].value, "Stone, A");
        assert_eq!(rows[1].cells[2].value, "say \"hi\"");
        assert!(rows[1].cells[2].quoted);
        assert_eq!(rows[2].cells[2].value, "two\nlines");
        assert_eq!(&text[rows[1].cells[1].span.clone()], "\"Stone, A\"");
        assert_eq!(sniff_delimiter(text, None), Some(','));
        assert_eq!(sniff_delimiter("a\tb\n1\t2\n3\t4\n", None), Some('\t'));
        assert_eq!(sniff_delimiter("just some prose\nmore prose\n", None), None);
    }

    #[test]
    fn env_assignments_keep_value_spans() {
        let text = "# c\nexport A_KEY=\"v 1\"\nB=2\nnot an assignment\n";
        let a = assignments(text);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].key, "A_KEY");
        assert_eq!(a[0].value, "v 1");
        assert_eq!(&text[a[0].span.clone()], "\"v 1\"");
        assert_eq!(&text[a[1].span.clone()], "2");
        assert!(!looks_like_env(text));
        assert!(looks_like_env("A=1\nB=2\n# x\n"));
    }

    #[test]
    fn fixed_width_columns_are_the_blank_gaps() {
        let text = "ID   NAME     AMT\n001  Stone    12.5\n002  Brenn    03.0\n003  Kade     99.9\n004  Olm      10.0\n";
        let cols = fixed_width(text).unwrap();
        assert_eq!(cols, vec![0..3, 5..10, 14..18]);
        assert!(fixed_width("a,b\n1,2\n").is_none());
    }

    #[test]
    fn simple_xml_values_have_paths_and_records() {
        let text = "<?xml version=\"1.0\"?>\n<orders>\n  <order id=\"7\"><name>Ann Stone</name></order>\n  <order id=\"8\"><name>Bo</name></order>\n</orders>\n";
        let x = xml(text).unwrap();
        assert_eq!(x.root, "orders");
        assert_eq!(x.records.len(), 2);
        let paths: Vec<&str> = x.values.iter().map(|v| v.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "orders/order/@id",
                "orders/order/name",
                "orders/order/@id",
                "orders/order/name"
            ]
        );
        assert_eq!(&text[x.values[1].span.clone()], "Ann Stone");
        assert_eq!(x.values[3].record, Some(1));
        assert!(xml("<a><b></a></b>").is_none());
    }
}
