// SPDX-License-Identifier: GPL-3.0-or-later
//! The structure view: a deterministic profile of sensitive content.
//!
//! Every value is read, none is shown: a field is described by its types,
//! how often it is present, null or empty, a bucket of its distinct values,
//! its length range, its shapes ([`super::shape`]) or date pictures
//! ([`super::dates`]), and what it looks like (an email, integer text, a
//! UUID). Identifying values and free text are shaped by their runs
//! (`Aa Aa`), so letter counts of a person's name are not shown; values a
//! detector takes for secrets only by length and character kinds.

use super::dates;
use super::masked::{self, Template};
use super::parse::{self, Json};
use super::shape::{self, Detail};
use super::{Format, Knowledge, SPECIAL_KEYS, schema_key};
use crate::detect::Kind;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Shapes (or pictures) listed per field.
const SHAPES_SHOWN: usize = 4;
/// Fields listed; the rest are counted.
const FIELDS_SHOWN: usize = 80;
/// A string's letters are shown position by position only when it has at
/// most this many letters in at most [`EXACT_RUNS`] runs: a code or a short
/// word. Longer text is shaped by its runs.
const EXACT_LETTERS: usize = 12;
const EXACT_RUNS: usize = 3;
/// Exact shapes of a field beyond which its run shapes are shown instead.
const EXACT_SHAPES_MAX: usize = 6;
/// A value this long (characters) is counted as long.
const LONG_VALUE: usize = 1000;
/// 2^53: integers above it are not exact as IEEE doubles (JavaScript numbers).
const EXACT_DOUBLE: u128 = 9_007_199_254_740_992;

/// Kinds whose values identify someone or something: shaped by runs.
fn identifying(kind: Kind) -> bool {
    !matches!(kind, Kind::Data | Kind::Code)
}

fn hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// What one value is.
enum Leaf<'a> {
    Str(&'a str),
    Num(&'a str),
    Bool(bool),
    Null,
    Object(usize),
    Array(usize),
}

/// What is known about one field (a JSON path, a column, a key).
#[derive(Default, Debug)]
pub struct Field {
    pub path: String,
    /// Values seen (nulls included).
    pub count: usize,
    /// Records the field occurs in.
    pub in_records: usize,
    last_record: Option<usize>,
    /// A value of the document outside its records.
    pub outside: bool,
    pub nulls: usize,
    pub empties: usize,
    pub types: BTreeMap<&'static str, usize>,
    /// Lengths of string values, in characters.
    pub lengths: Option<(usize, usize)>,
    exact: BTreeMap<String, usize>,
    runs: BTreeMap<String, usize>,
    by_runs: bool,
    secret: BTreeMap<String, usize>,
    distinct: HashSet<u64>,
    /// What values look like: a date picture, a detected kind, `integer text`.
    pub looks: BTreeMap<String, usize>,
    /// Date pictures, counted apart (several in one field is an anomaly).
    pictures: BTreeMap<String, usize>,
    /// Array lengths.
    pub items: Option<(usize, usize)>,
    /// Digits of integers (JSON numbers and integer text).
    pub digits: Option<(usize, usize)>,
    /// Decimal places of non-integer numbers.
    pub decimals: BTreeMap<usize, usize>,
    pub negatives: usize,
    pub above_2_53: usize,
    pub leading_zeros: usize,
    pub padded: usize,
    pub line_breaks: usize,
    /// Values holding the delimiter or a quote (delimited text).
    pub embedded: usize,
    pub non_ascii: usize,
    pub control: usize,
    pub long: usize,
    /// Values holding detected values inside longer text, by kind.
    pub holding: BTreeMap<&'static str, usize>,
    /// How the field is written (`.env`: `export`, quoting).
    pub notes: BTreeSet<&'static str>,
}

fn widen(r: &mut Option<(usize, usize)>, n: usize) {
    *r = Some(match *r {
        Some((a, b)) => (a.min(n), b.max(n)),
        None => (n, n),
    });
}

static UUID: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
    )
    .expect("static regex")
});
static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]{1,15}://\S+$").expect("static regex")
});

/// Whether `s` is an integer written plainly (`-12`, `007`).
fn integer_text(s: &str) -> bool {
    let d = s.strip_prefix('-').unwrap_or(s);
    !d.is_empty() && d.len() <= 40 && d.bytes().all(|b| b.is_ascii_digit())
}

fn decimal_text(s: &str) -> bool {
    let d = s.strip_prefix('-').unwrap_or(s);
    match d.split_once(['.', ',']) {
        Some((a, b)) => {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|x| x.is_ascii_digit())
                && b.bytes().all(|x| x.is_ascii_digit())
        }
        None => false,
    }
}

fn above_exact_double(int_digits: &str) -> bool {
    let d = int_digits.trim_start_matches('0');
    d.len() > 38 || d.parse::<u128>().is_ok_and(|n| n > EXACT_DOUBLE)
}

/// A URL's shape with its password withheld: `a://a:•••@a.a:9999/a`.
pub fn url_shape(s: &str) -> Option<String> {
    if !URL.is_match(s) {
        return None;
    }
    let (scheme, rest) = s.split_once("://")?;
    let (auth, host) = match rest.split_once('@') {
        Some((a, h)) if !a.contains('/') => (Some(a), h),
        _ => (None, rest),
    };
    let mut out = format!("{}://", shape::shape(scheme, Detail::Runs));
    if let Some(a) = auth {
        match a.split_once(':') {
            Some((user, _)) => {
                out.push_str(&shape::shape(user, Detail::Runs));
                out.push_str(":•••@");
            }
            None => {
                out.push_str(&shape::shape(a, Detail::Runs));
                out.push('@');
            }
        }
    }
    out.push_str(&shape::shape(host, Detail::Runs));
    Some(out)
}

impl Field {
    fn new(path: String) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }

    fn observe(
        &mut self,
        leaf: Leaf<'_>,
        record: Option<usize>,
        k: &dyn Knowledge,
        delim: Option<char>,
    ) {
        self.count += 1;
        match record {
            Some(r) if self.last_record != Some(r) => {
                self.last_record = Some(r);
                self.in_records += 1;
            }
            None => self.outside = true,
            _ => {}
        }
        match leaf {
            Leaf::Null => self.nulls += 1,
            Leaf::Bool(b) => {
                *self.types.entry("boolean").or_default() += 1;
                self.distinct.insert(u64::from(b));
            }
            Leaf::Object(n) => {
                *self.types.entry("object").or_default() += 1;
                if n == 0 {
                    self.empties += 1;
                }
            }
            Leaf::Array(n) => {
                *self.types.entry("array").or_default() += 1;
                widen(&mut self.items, n);
                if n == 0 {
                    self.empties += 1;
                }
            }
            Leaf::Num(raw) => self.number(raw),
            Leaf::Str(s) => self.string(s, k, delim),
        }
    }

    fn number(&mut self, raw: &str) {
        self.distinct.insert(hash(raw));
        let body = raw.strip_prefix('-').unwrap_or(raw);
        if raw.starts_with('-') {
            self.negatives += 1;
        }
        let (mantissa, exponent) = match body.split_once(['e', 'E']) {
            Some((m, e)) => (m, Some(e)),
            None => (body, None),
        };
        let (int, frac) = match mantissa.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (mantissa, None),
        };
        if frac.is_none() && exponent.is_none() {
            *self.types.entry("integer").or_default() += 1;
            widen(&mut self.digits, int.len());
            if above_exact_double(int) {
                self.above_2_53 += 1;
            }
        } else {
            *self.types.entry("number").or_default() += 1;
            *self.decimals.entry(frac.map_or(0, str::len)).or_default() += 1;
        }
        let s = shape::shape(raw, Detail::Exact);
        *self.exact.entry(s.clone()).or_default() += 1;
        *self.runs.entry(s).or_default() += 1;
    }

    fn string(&mut self, s: &str, k: &dyn Knowledge, delim: Option<char>) {
        *self.types.entry("string").or_default() += 1;
        let len = s.chars().count();
        widen(&mut self.lengths, len);
        self.distinct.insert(hash(s));
        if s.is_empty() {
            self.empties += 1;
            return;
        }
        if s.trim() != s {
            self.padded += 1;
        }
        if s.contains(['\n', '\r']) {
            self.line_breaks += 1;
        }
        if delim.is_some_and(|d| s.contains(d)) || (delim.is_some() && s.contains('"')) {
            self.embedded += 1;
        }
        if s.chars().any(|c| c.is_alphabetic() && !c.is_ascii()) {
            self.non_ascii += 1;
        }
        if s.chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            self.control += 1;
        }
        if len > LONG_VALUE {
            self.long += 1;
        }
        let trimmed = s.trim();
        let spans = k.values(s);
        let whole = spans
            .iter()
            .find(|(a, b, _)| s[..*a].trim().is_empty() && s[*b..].trim().is_empty())
            .map(|(_, _, kind)| *kind);
        let secret =
            spans.iter().any(|(_, _, kind)| *kind == Kind::Secret) && url_shape(trimmed).is_none();
        let ident = spans.iter().any(|(_, _, kind)| identifying(*kind));
        // A date is described by its layout's picture, not by shapes.
        if let Some(layout) = dates::recognize(s) {
            let p = format!("{} {}", layout.kind(), layout.picture());
            *self.pictures.entry(p).or_default() += 1;
            return;
        }
        let mut look = |l: String| *self.looks.entry(l).or_default() += 1;
        if URL.is_match(trimmed) {
            look("url".into());
        } else if let Some(kind) =
            whole.filter(|k| *k != Kind::Data && !(*k == Kind::Secret && integer_text(trimmed)))
        {
            look(kind.tag().to_owned());
        } else if integer_text(trimmed) {
            look("integer text".into());
            let digits = trimmed.trim_start_matches('-');
            widen(&mut self.digits, digits.len());
            if digits.len() > 1 && digits.starts_with('0') {
                self.leading_zeros += 1;
            }
            if above_exact_double(digits) {
                self.above_2_53 += 1;
            }
        } else if decimal_text(trimmed) {
            look("decimal text".into());
        } else if matches!(
            trimmed.to_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "y" | "n"
        ) {
            look("boolean text".into());
        } else if UUID.is_match(trimmed) {
            look("uuid".into());
        } else {
            for (_, _, kind) in &spans {
                if *kind != Kind::Data {
                    *self.holding.entry(kind.tag()).or_default() += 1;
                }
            }
        }
        if secret {
            *self.secret.entry(shape::secret_summary(s)).or_default() += 1;
            return;
        }
        if let Some(u) = url_shape(trimmed) {
            *self.runs.entry(u.clone()).or_default() += 1;
            *self.exact.entry(u).or_default() += 1;
            return;
        }
        let runs = shape::shape(s, Detail::Runs);
        let letters = s.chars().filter(|c| c.is_alphabetic()).count();
        let letter_runs = runs.matches(['a', 'A']).count();
        *self.runs.entry(runs).or_default() += 1;
        if ident || letters > EXACT_LETTERS || letter_runs > EXACT_RUNS {
            self.by_runs = true;
        } else {
            *self
                .exact
                .entry(shape::shape(s, Detail::Exact))
                .or_default() += 1;
        }
    }

    /// The shapes to show: exact ones when few and none needed collapsing.
    fn shapes(&self) -> &BTreeMap<String, usize> {
        if self.by_runs || self.exact.len() > EXACT_SHAPES_MAX {
            &self.runs
        } else {
            &self.exact
        }
    }

    fn distinct_text(&self) -> Option<String> {
        let values = self.count - self.nulls;
        let n = self.distinct.len();
        if values < 2 || self.types.keys().all(|t| *t == "object" || *t == "array") {
            return None;
        }
        Some(if n == values {
            "all distinct".into()
        } else if n == 1 {
            "one value".into()
        } else {
            let bucket = match n {
                2..=5 => "2-5",
                6..=20 => "6-20",
                21..=100 => "21-100",
                101..=1000 => "101-1000",
                _ => ">1000",
            };
            format!("{bucket} distinct")
        })
    }

    /// A `KEY=value` entry: how its value is written.
    fn render_env(&self) -> String {
        let mut parts: Vec<String> = self.notes.iter().map(|n| (*n).to_owned()).collect();
        let (a, b) = self.lengths.unwrap_or((0, 0));
        let looks = |l: &str| self.looks.contains_key(l);
        // A number's digit count says no more than its length and kinds.
        let what = if b == 0 {
            "empty".to_owned()
        } else if looks("integer text") {
            match self.digits {
                Some((1, 1)) => "integer, 1 digit".to_owned(),
                Some((d, e)) if d == e => format!("integer, {d} digits"),
                Some((d, e)) => format!("integer, {d}-{e} digits"),
                None => "integer".to_owned(),
            }
        } else if looks("url") {
            format!("URL {}", keys(self.shapes()))
        } else if !self.secret.is_empty() {
            format!("secret-like, {}", keys(&self.secret))
        } else if !self.pictures.is_empty() {
            keys(&self.pictures)
        } else if looks("boolean text") {
            "boolean text".to_owned()
        } else {
            let len = if a == b {
                format!("{a}")
            } else {
                format!("{a}-{b}")
            };
            format!("{len} chars, shape {}", keys(self.shapes()))
        };
        parts.push(what);
        if self.count > 1 {
            parts.push(format!("assigned {} times", self.count));
        }
        format!("  {} — {}", self.path, parts.join("; "))
    }

    fn render(&self, records: Option<usize>) -> String {
        let mut parts: Vec<String> = Vec::new();
        let types: Vec<(&&str, &usize)> = self.types.iter().collect();
        parts.push(match types.as_slice() {
            [] => "null".into(),
            [(t, _)] => (**t).to_owned(),
            many => {
                let mut v: Vec<_> = many.to_vec();
                v.sort_by(|a, b| b.1.cmp(a.1));
                v.iter()
                    .map(|(t, n)| format!("{t} ×{n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        });
        let mut presence = format!("{}", self.count);
        if let Some(r) = records
            && !self.outside
            && self.in_records < r
        {
            presence.push_str(&format!(" (missing in {} record(s))", r - self.in_records));
        }
        if self.nulls > 0 {
            presence.push_str(&format!(", null ×{}", self.nulls));
        }
        if self.empties > 0 {
            presence.push_str(&format!(", empty ×{}", self.empties));
        }
        parts.push(presence);
        if let Some(d) = self.distinct_text() {
            parts.push(d);
        }
        if let Some((a, b)) = self.lengths
            && self.types.contains_key("string")
            && self.secret.is_empty()
        {
            parts.push(if a == b {
                format!("len {a}")
            } else {
                format!("len {a}-{b}")
            });
        }
        if let Some((a, b)) = self.items {
            parts.push(if a == b {
                format!("{a} items")
            } else {
                format!("{a}-{b} items")
            });
        }
        if let Some((a, b)) = self.digits
            && self.pictures.is_empty()
        {
            parts.push(match (a, b) {
                (1, 1) => "1 digit".to_owned(),
                _ if a == b => format!("{a} digits"),
                _ => format!("{a}-{b} digits"),
            });
        }
        if !self.decimals.is_empty() {
            let d: Vec<String> = self
                .decimals
                .iter()
                .map(|(places, n)| format!("{places} ×{n}"))
                .collect();
            parts.push(format!("decimal places {}", d.join(", ")));
        }
        if self.negatives > 0 {
            parts.push(format!("negative ×{}", self.negatives));
        }
        if !self.secret.is_empty() {
            parts.push(format!("secret-like, {}", top(&self.secret)));
        } else if !self.pictures.is_empty() {
            parts.push(top(&self.pictures));
            if !self.shapes().is_empty() {
                parts.push(format!("other shapes {}", top(self.shapes())));
            }
        } else if !self.shapes().is_empty() {
            parts.push(format!("shapes {}", top(self.shapes())));
        }
        if !self.looks.is_empty() {
            parts.push(format!("looks like {}", top(&self.looks)));
        }
        if !self.holding.is_empty() {
            let h: Vec<String> = self
                .holding
                .iter()
                .map(|(k, n)| format!("{k} ×{n}"))
                .collect();
            parts.push(format!("text holding {}", h.join(", ")));
        }
        format!("  {} — {}", self.path, parts.join("; "))
    }

    /// Notable things about this field, for the anomalies list.
    fn anomalies(&self, out: &mut Vec<String>) {
        let p = &self.path;
        let kinds: Vec<&&str> = self.types.keys().collect();
        if kinds.len() > 1 {
            let t: Vec<String> = self
                .types
                .iter()
                .map(|(t, n)| format!("{t} ×{n}"))
                .collect();
            out.push(format!("{p}: mixed types ({})", t.join(", ")));
        }
        if self.pictures.len() > 1 {
            out.push(format!(
                "{p}: {} date/time layouts in one field",
                self.pictures.len()
            ));
        } else if !self.pictures.is_empty() && !self.shapes().is_empty() {
            out.push(format!("{p}: dates mixed with values that are not dates"));
        }
        if self.above_2_53 > 0 {
            out.push(format!(
                "{p}: {} integer(s) above 2^53 (not exact as doubles, e.g. JavaScript numbers)",
                self.above_2_53
            ));
        }
        for (n, what) in [
            (self.leading_zeros, "integer text with leading zeros"),
            (self.padded, "value(s) with leading or trailing whitespace"),
            (self.line_breaks, "value(s) holding a line break"),
            (self.embedded, "value(s) holding the delimiter or a quote"),
            (self.control, "value(s) holding control characters"),
            (self.long, "value(s) longer than 1000 characters"),
        ] {
            if n > 0 {
                out.push(format!("{p}: {n} {what}"));
            }
        }
    }
}

/// The entries without counts, most frequent first.
fn keys(m: &BTreeMap<String, usize>) -> String {
    let mut v: Vec<(&String, &usize)> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    v.iter()
        .take(SHAPES_SHOWN)
        .map(|(s, _)| shape::printable(s))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The most frequent entries, `shape ×n`, then how many more.
fn top(m: &BTreeMap<String, usize>) -> String {
    let mut v: Vec<(&String, &usize)> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let mut out: Vec<String> = v
        .iter()
        .take(SHAPES_SHOWN)
        .map(|(s, n)| {
            let s = if s.is_empty() {
                "(empty)".to_owned()
            } else {
                shape::printable(s)
            };
            format!("{s} ×{n}")
        })
        .collect();
    if v.len() > SHAPES_SHOWN {
        out.push(format!("+{} more", v.len() - SHAPES_SHOWN));
    }
    out.join(", ")
}

/// The fields of one piece of content, in the order first seen.
struct Fields<'k> {
    k: &'k dyn Knowledge,
    list: Vec<Field>,
    index: HashMap<String, usize>,
    delim: Option<char>,
}

impl<'k> Fields<'k> {
    fn new(k: &'k dyn Knowledge, delim: Option<char>) -> Self {
        Self {
            k,
            list: Vec::new(),
            index: HashMap::new(),
            delim,
        }
    }

    fn observe(&mut self, path: &str, leaf: Leaf<'_>, record: Option<usize>) {
        let i = match self.index.get(path) {
            Some(&i) => i,
            None => {
                self.list.push(Field::new(path.to_owned()));
                self.index.insert(path.to_owned(), self.list.len() - 1);
                self.list.len() - 1
            }
        };
        self.list[i].observe(leaf, record, self.k, self.delim);
    }
}

/// The profile of a piece of sensitive content.
#[derive(Debug)]
pub struct Profile {
    pub format: Format,
    /// Records (objects of the record array, rows, lines), if it has any.
    pub records: Option<usize>,
    /// Where the records are, in words (`the array \`orders\``).
    pub layout: String,
    pub fields: Vec<Field>,
    pub anomalies: Vec<String>,
    pub templates: Vec<Template>,
    /// Lines, for templates: how many there are.
    pub lines: usize,
    /// Schema names shown (keys, columns), for the caller's vocabulary.
    pub schema: BTreeSet<String>,
}

struct JsonWalk<'a, 'k> {
    fields: Fields<'k>,
    text: &'a str,
    duplicate_keys: BTreeMap<String, usize>,
    special: BTreeMap<String, BTreeSet<String>>,
    withheld_keys: usize,
    schema: BTreeSet<String>,
    depth: usize,
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

impl JsonWalk<'_, '_> {
    /// A record: its fields, not the record itself (its count is the
    /// profile's).
    fn record(&mut self, node: &Json, path: &str, record: usize, depth: usize) {
        match node {
            Json::Object { .. } => self.members(node, path, Some(record), depth),
            other => self.walk(other, path, Some(record), depth),
        }
    }

    fn walk(&mut self, node: &Json, path: &str, record: Option<usize>, depth: usize) {
        if let Json::Object { entries, .. } = node
            && !path.is_empty()
        {
            self.fields
                .observe(path, Leaf::Object(entries.len()), record);
        }
        self.members(node, path, record, depth);
    }

    fn members(&mut self, node: &Json, path: &str, record: Option<usize>, depth: usize) {
        self.depth = self.depth.max(depth);
        match node {
            Json::Object { entries, .. } => {
                let mut seen: HashSet<&str> = HashSet::new();
                for (key, value) in entries {
                    let name = key.name.as_str();
                    let shown = if schema_key(name, self.fields.k) {
                        self.schema.insert(name.to_owned());
                        name.to_owned()
                    } else {
                        self.withheld_keys += 1;
                        "*".to_owned()
                    };
                    let child = join(path, &shown);
                    if !seen.insert(name) {
                        *self.duplicate_keys.entry(child.clone()).or_default() += 1;
                    }
                    if SPECIAL_KEYS.contains(&name) {
                        self.special.entry(name.to_owned()).or_default().insert(
                            if path.is_empty() {
                                "(top level)".into()
                            } else {
                                path.to_owned()
                            },
                        );
                    }
                    self.walk(value, &child, record, depth + 1);
                }
            }
            Json::Array { items, .. } => {
                if !path.is_empty() {
                    self.fields.observe(path, Leaf::Array(items.len()), record);
                }
                let child = format!("{path}[]");
                for item in items {
                    self.walk(item, &child, record, depth + 1);
                }
            }
            Json::String { value, .. } => self.fields.observe(path, Leaf::Str(value), record),
            Json::Number { span } => {
                self.fields
                    .observe(path, Leaf::Num(&self.text[span.clone()]), record)
            }
            Json::Bool { value, .. } => self.fields.observe(path, Leaf::Bool(*value), record),
            Json::Null { .. } => self.fields.observe(path, Leaf::Null, record),
        }
    }
}

/// The records of a JSON document: the array of objects it is (`""`), or
/// the longest array of objects among its top-level keys (that key), if
/// there is one with two or more elements.
pub fn json_records(root: &Json) -> Option<(Option<usize>, &Vec<Json>)> {
    let objects = |items: &Vec<Json>| {
        items.len() >= 2
            && items
                .iter()
                .filter(|x| matches!(x, Json::Object { .. } | Json::Array { .. }))
                .count()
                * 2
                >= items.len()
    };
    match root {
        Json::Array { items, .. } if objects(items) => Some((None, items)),
        Json::Object { entries, .. } => entries
            .iter()
            .enumerate()
            .filter_map(|(i, (_, v))| match v {
                Json::Array { items, .. } if objects(items) => Some((i, items)),
                _ => None,
            })
            .max_by_key(|(i, items)| (items.len(), std::cmp::Reverse(*i)))
            .map(|(i, items)| (Some(i), items)),
        _ => None,
    }
}

fn profile_json(text: &str, root: &Json, k: &dyn Knowledge, lines: bool) -> Profile {
    let mut w = JsonWalk {
        fields: Fields::new(k, None),
        text,
        duplicate_keys: BTreeMap::new(),
        special: BTreeMap::new(),
        withheld_keys: 0,
        schema: BTreeSet::new(),
        depth: 0,
    };
    let mut anomalies = Vec::new();
    let (records, layout, raw_records): (usize, String, Vec<&str>) = if lines {
        let Json::Array { items, .. } = root else {
            unreachable!("JSON lines are read as an array of documents")
        };
        for (i, item) in items.iter().enumerate() {
            w.record(item, "", i, 0);
        }
        (
            items.len(),
            format!("{} documents, one per line", items.len()),
            items.iter().map(|x| &text[x.span()]).collect(),
        )
    } else {
        match json_records(root) {
            Some((None, items)) => {
                for (i, item) in items.iter().enumerate() {
                    w.record(item, "[]", i, 1);
                }
                (
                    items.len(),
                    format!("an array of {} records", items.len()),
                    items.iter().map(|x| &text[x.span()]).collect(),
                )
            }
            Some((Some(at), items)) => {
                let Json::Object { entries, .. } = root else {
                    unreachable!("records under a key are in an object")
                };
                let key = &entries[at].0.name;
                let shown = if schema_key(key, k) {
                    w.schema.insert(key.clone());
                    key.clone()
                } else {
                    "*".into()
                };
                let others: Vec<String> = entries
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != at)
                    .map(|(_, (key, _))| key.name.clone())
                    .collect();
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i != at {
                        let name = if schema_key(&key.name, k) {
                            w.schema.insert(key.name.clone());
                            key.name.clone()
                        } else {
                            "*".into()
                        };
                        w.walk(value, &name, None, 1);
                    }
                }
                let path = format!("{shown}[]");
                for (i, item) in items.iter().enumerate() {
                    w.record(item, &path, i, 2);
                }
                let mut layout = format!(
                    "an object; its records are the {} elements of `{shown}`",
                    items.len()
                );
                if !others.is_empty() {
                    layout.push_str(&format!(
                        "; {} other top-level key(s) hold single values",
                        others.len()
                    ));
                }
                (
                    items.len(),
                    layout,
                    items.iter().map(|x| &text[x.span()]).collect(),
                )
            }
            None => {
                w.record(root, "", 0, 0);
                (1, "a single document".into(), Vec::new())
            }
        }
    };
    let duplicates = duplicate_records(&raw_records);
    if duplicates > 0 {
        anomalies.push(format!(
            "{duplicates} record(s) identical to an earlier one"
        ));
    }
    for (path, n) in &w.duplicate_keys {
        anomalies.push(format!("{path}: key repeated in the same object ×{n}"));
    }
    if !w.special.is_empty() {
        let s: Vec<String> = w
            .special
            .iter()
            .map(|(key, at)| {
                format!(
                    "{key} (in {})",
                    at.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            })
            .collect();
        anomalies.push(format!(
            "keys named like built-in object members: {}",
            s.join("; ")
        ));
    }
    if w.withheld_keys > 0 {
        anomalies.push(format!(
            "{} key(s) look like data (ids, dates, detected values) and are shown as `*`",
            w.withheld_keys
        ));
    }
    if w.depth > 8 {
        anomalies.push(format!("nesting up to {} levels deep", w.depth));
    }
    Profile {
        format: if lines {
            Format::JsonLines
        } else {
            Format::Json
        },
        records: Some(records),
        layout,
        fields: w.fields.list,
        anomalies,
        templates: Vec::new(),
        lines: text.lines().count(),
        schema: w.schema,
    }
}

fn duplicate_records(raw: &[&str]) -> usize {
    let mut seen = HashSet::new();
    raw.iter().filter(|r| !seen.insert(hash(r.trim()))).count()
}

/// Whether the first row of delimited text is a header: every cell a
/// distinct name (not a number, date or detected value).
pub fn has_header(rows: &[parse::Row], k: &dyn Knowledge) -> bool {
    let Some(first) = rows.first() else {
        return false;
    };
    if rows.len() < 2 {
        return false;
    }
    let mut names = HashSet::new();
    first.cells.iter().all(|c| {
        let v = c.value.trim();
        !v.is_empty()
            && !integer_text(v)
            && !decimal_text(v)
            && dates::recognize(v).is_none()
            && k.values(v).is_empty()
            && names.insert(v.to_owned())
    })
}

/// The name a column is shown by.
pub fn column_name(header: Option<&str>, i: usize, k: &dyn Knowledge) -> String {
    match header {
        Some(h) if schema_key(h.trim(), k) => h.trim().to_owned(),
        Some(_) => format!("column {} (name withheld)", i + 1),
        None => format!("column {}", i + 1),
    }
}

fn profile_delimited(text: &str, delimiter: char, k: &dyn Knowledge) -> Option<Profile> {
    let rows = parse::delimited(text, delimiter)?;
    let header = has_header(&rows, k);
    let names: Vec<String> = match (header, rows.first()) {
        (true, Some(h)) => h
            .cells
            .iter()
            .enumerate()
            .map(|(i, c)| column_name(Some(&c.value), i, k))
            .collect(),
        _ => Vec::new(),
    };
    let body = &rows[usize::from(header)..];
    let mut fields = Fields::new(k, Some(delimiter));
    let width = names.len().max(body.first().map_or(0, |r| r.cells.len()));
    let mut ragged = 0;
    let mut quoted = 0;
    let mut cells = 0;
    for (r, row) in body.iter().enumerate() {
        if row.cells.len() != width {
            ragged += 1;
        }
        for (i, cell) in row.cells.iter().enumerate() {
            let name = names
                .get(i)
                .cloned()
                .unwrap_or_else(|| column_name(None, i, k));
            fields.observe(&name, Leaf::Str(&cell.value), Some(r));
            cells += 1;
            if cell.quoted {
                quoted += 1;
            }
        }
    }
    let mut anomalies = Vec::new();
    if ragged > 0 {
        anomalies.push(format!(
            "{ragged} row(s) with a number of fields other than {width}"
        ));
    }
    let raw: Vec<&str> = body.iter().map(|r| &text[r.span.clone()]).collect();
    let duplicates = duplicate_records(&raw);
    if duplicates > 0 {
        anomalies.push(format!("{duplicates} row(s) identical to an earlier one"));
    }
    if header {
        let mut seen = HashSet::new();
        if rows[0]
            .cells
            .iter()
            .any(|c| !seen.insert(c.value.trim().to_lowercase()))
        {
            anomalies.push("repeated column names in the header".into());
        }
    }
    let quoting = match quoted {
        0 => "no field quoted".to_owned(),
        n if n == cells => "every field quoted".to_owned(),
        n => format!("{n} of {cells} fields quoted"),
    };
    let schema = if header {
        names
            .iter()
            .filter(|n| !n.starts_with("column "))
            .cloned()
            .collect()
    } else {
        BTreeSet::new()
    };
    Some(Profile {
        format: Format::Delimited { delimiter },
        records: Some(body.len()),
        layout: format!(
            "{} rows of {width} fields{}; {quoting}",
            body.len(),
            if header {
                " after a header row"
            } else {
                ", no header row"
            }
        ),
        fields: fields.list,
        anomalies,
        templates: Vec::new(),
        lines: text.lines().count(),
        schema,
    })
}

fn profile_env(text: &str, k: &dyn Knowledge) -> Profile {
    let mut fields = Fields::new(k, None);
    let mut schema = BTreeSet::new();
    let all = parse::assignments(text);
    for a in &all {
        schema.insert(a.key.clone());
        fields.observe(&a.key, Leaf::Str(&a.value), None);
        let f = fields.index[&a.key];
        let notes = &mut fields.list[f].notes;
        if a.export {
            notes.insert("exported");
        }
        match a.quote {
            Some('"') => notes.insert("double-quoted"),
            Some(_) => notes.insert("single-quoted"),
            None => false,
        };
    }
    let mut seen = HashSet::new();
    let repeated = all.iter().filter(|a| !seen.insert(a.key.as_str())).count();
    let mut anomalies = Vec::new();
    if repeated > 0 {
        anomalies.push(format!("{repeated} key(s) assigned more than once"));
    }
    // Lines that are neither assignments, comments nor blank: shown by the
    // runs of their shape (a malformed line, a line in another syntax).
    let mut other: BTreeMap<String, usize> = BTreeMap::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || !parse::assignments(line).is_empty() {
            continue;
        }
        let masked = if k.values(t).is_empty() {
            shape::shape(t, Detail::Runs)
        } else {
            shape::secret_summary(t)
        };
        *other.entry(masked).or_default() += 1;
    }
    if !other.is_empty() {
        anomalies.push(format!(
            "{} line(s) that are neither assignments nor comments: {}",
            other.values().sum::<usize>(),
            top(&other)
        ));
    }
    Profile {
        format: Format::Env,
        records: None,
        layout: format!("{} assignments", all.len()),
        fields: fields.list,
        anomalies,
        templates: Vec::new(),
        lines: text.lines().count(),
        schema,
    }
}

fn profile_fixed(text: &str, k: &dyn Knowledge) -> Option<Profile> {
    let cols = parse::fixed_width(text)?;
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let cell = |l: &str, c: &std::ops::Range<usize>| -> String {
        l.get(c.start..c.end.min(l.len()))
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let first: Vec<String> = cols.iter().map(|c| cell(lines[0], c)).collect();
    let header = first.iter().all(|v| {
        !v.is_empty()
            && v.chars().all(|c| c.is_alphabetic() || " _-".contains(c))
            && k.values(v).is_empty()
    }) && lines[1..].iter().any(|l| {
        cols.iter()
            .any(|c| cell(l, c).chars().any(|x| x.is_ascii_digit()))
    });
    let names: Vec<String> = (0..cols.len())
        .map(|i| column_name(header.then(|| first[i].as_str()), i, k))
        .collect();
    let body = &lines[usize::from(header)..];
    let mut fields = Fields::new(k, None);
    for (r, l) in body.iter().enumerate() {
        for (i, c) in cols.iter().enumerate() {
            fields.observe(&names[i], Leaf::Str(&cell(l, c)), Some(r));
        }
    }
    let widths: Vec<String> = cols
        .iter()
        .map(|c| format!("{}-{}", c.start + 1, c.end))
        .collect();
    Some(Profile {
        format: Format::FixedWidth,
        records: Some(body.len()),
        layout: format!(
            "{} lines of {} columns at characters {}{}",
            body.len(),
            cols.len(),
            widths.join(", "),
            if header { " after a header line" } else { "" }
        ),
        fields: fields.list,
        anomalies: Vec::new(),
        templates: Vec::new(),
        lines: text.lines().count(),
        schema: if header {
            names
                .into_iter()
                .filter(|n| !n.starts_with("column "))
                .collect()
        } else {
            BTreeSet::new()
        },
    })
}

fn profile_xml(text: &str, k: &dyn Knowledge) -> Option<Profile> {
    let x = parse::xml(text)?;
    let mut fields = Fields::new(k, None);
    let mut schema = BTreeSet::new();
    let shown = |path: &str, schema: &mut BTreeSet<String>| -> String {
        path.split('/')
            .map(|seg| {
                let name = seg.trim_start_matches('@');
                if schema_key(name, k) {
                    schema.insert(name.to_owned());
                    seg.to_owned()
                } else {
                    "*".to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("/")
    };
    for v in &x.values {
        let p = shown(&v.path, &mut schema);
        fields.observe(&p, Leaf::Str(&v.value), v.record);
    }
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    for n in &x.record_names {
        *names.entry(n.as_str()).or_default() += 1;
    }
    let kinds: Vec<String> = names
        .iter()
        .map(|(n, c)| {
            let n = if schema_key(n, k) {
                (*n).to_owned()
            } else {
                "*".into()
            };
            format!("{n} ×{c}")
        })
        .collect();
    let root = if schema_key(&x.root, k) {
        x.root.clone()
    } else {
        "*".into()
    };
    Some(Profile {
        format: Format::Xml,
        records: Some(x.records.len()),
        layout: format!(
            "root element `{root}` with {} child element(s) ({})",
            x.records.len(),
            kinds.join(", ")
        ),
        fields: fields.list,
        anomalies: Vec::new(),
        templates: Vec::new(),
        lines: text.lines().count(),
        schema,
    })
}

fn profile_lines(text: &str, format: Format, k: &dyn Knowledge) -> Profile {
    let lines: Vec<&str> = text.lines().collect();
    let templates = masked::templates(&lines, k);
    let mut fields = Fields::new(k, None);
    let mut levels: BTreeMap<String, usize> = BTreeMap::new();
    if format == Format::Log {
        for l in &lines {
            let mut words = l.split_whitespace();
            let first = words.next().unwrap_or_default();
            let stamp = dates::recognize(first)
                .map(|_| first.to_owned())
                .or_else(|| {
                    let two = l.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                    dates::recognize(&two).map(|_| two)
                });
            if let Some(s) = stamp {
                fields.observe("timestamp", Leaf::Str(&s), None);
            }
            for w in l.split_whitespace().take(6) {
                let w = w.trim_matches(|c: char| !c.is_alphabetic());
                let u = w.to_uppercase();
                if matches!(
                    u.as_str(),
                    "TRACE"
                        | "DEBUG"
                        | "INFO"
                        | "NOTICE"
                        | "WARN"
                        | "WARNING"
                        | "ERROR"
                        | "FATAL"
                        | "CRITICAL"
                ) {
                    *levels.entry(u).or_default() += 1;
                    break;
                }
            }
        }
    }
    let mut layout = format!("{} lines", lines.len());
    if !levels.is_empty() {
        let l: Vec<String> = levels.iter().map(|(k, n)| format!("{k} ×{n}")).collect();
        layout.push_str(&format!("; levels {}", l.join(", ")));
    }
    if !templates.is_empty() {
        let covered: usize = templates.iter().map(|t| t.count).sum();
        layout.push_str(&format!(
            "; {} line templates cover {covered} lines",
            templates.len()
        ));
    }
    Profile {
        format,
        records: None,
        layout,
        fields: fields.list,
        anomalies: Vec::new(),
        templates,
        lines: lines.len(),
        schema: BTreeSet::new(),
    }
}

/// Anomalies of the text as a whole: encoding and line endings.
fn text_anomalies(text: &str, out: &mut Vec<String>) {
    let replaced = text.matches('\u{fffd}').count();
    if replaced > 0 {
        out.push(format!(
            "{replaced} byte sequence(s) that are not valid UTF-8 (read as U+FFFD)"
        ));
    }
    if text.starts_with('\u{feff}') {
        out.push("starts with a byte-order mark".into());
    }
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > 0 && lf > 0 {
        out.push(format!("mixed line endings: {crlf} CRLF, {lf} LF"));
    }
    if text.contains('\0') {
        out.push("holds NUL bytes".into());
    }
}

/// The profile of `text` (read from `path`, when it is a file), or `None`
/// when there is nothing to say about it beyond its size.
pub fn profile(text: &str, path: Option<&Path>, k: &dyn Knowledge) -> Option<Profile> {
    let format = super::detect(text, path);
    let body = text.trim_start_matches('\u{feff}');
    let mut p = match &format {
        Format::Json => profile_json(body, &parse::json(body)?, k, false),
        Format::JsonLines => {
            let docs = parse::json_lines(body)?;
            let root = Json::Array {
                span: 0..body.len(),
                items: docs,
            };
            profile_json(body, &root, k, true)
        }
        Format::Delimited { delimiter } => profile_delimited(body, *delimiter, k)?,
        Format::Env => profile_env(body, k),
        Format::FixedWidth => profile_fixed(body, k)?,
        Format::Xml => profile_xml(body, k)?,
        Format::Log | Format::Text => {
            let p = profile_lines(body, format.clone(), k);
            if p.templates.is_empty() && p.fields.is_empty() {
                return None;
            }
            p
        }
    };
    let mut anomalies = Vec::new();
    for f in &p.fields {
        f.anomalies(&mut anomalies);
    }
    text_anomalies(text, &mut anomalies);
    p.anomalies.extend(anomalies);
    Some(p)
}

impl Profile {
    /// A short outline for the task note: format, layout and the fields
    /// with their types (or, for `KEY=value`, how each value is written),
    /// at most `max` characters.
    pub fn outline(&self, max: usize) -> String {
        let mut out = format!("{}, {}", self.format.name(), self.layout);
        let fields: Vec<String> = self
            .fields
            .iter()
            .map(|f| match self.format {
                Format::Env => f.render_env().trim_start().replacen(" — ", " (", 1) + ")",
                _ => {
                    let types: Vec<&str> = f.types.keys().copied().collect();
                    let t = if types.is_empty() {
                        "null".to_owned()
                    } else {
                        types.join("|")
                    };
                    let mut pictures: Vec<(&String, &usize)> = f.pictures.iter().collect();
                    pictures.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                    match pictures.as_slice() {
                        [] => format!("{} ({t})", f.path),
                        [(p, _)] => format!("{} ({t}, {p})", f.path),
                        [(p, _), rest @ ..] => {
                            format!("{} ({t}, {p} and {} other layouts)", f.path, rest.len())
                        }
                    }
                }
            })
            .collect();
        let fields: Vec<String> = fields
            .into_iter()
            .zip(&self.fields)
            .filter(|(_, f)| {
                f.types.is_empty() || f.types.keys().any(|t| *t != "object" && *t != "array")
            })
            .map(|(s, _)| s)
            .collect();
        if !fields.is_empty() {
            out.push_str(": ");
            let mut used = out.len();
            let mut shown = 0;
            for f in &fields {
                if used + f.len() + 2 > max {
                    break;
                }
                if shown > 0 {
                    out.push_str(", ");
                }
                out.push_str(f);
                used = out.len();
                shown += 1;
            }
            if shown < fields.len() {
                out.push_str(&format!(" … and {} more", fields.len() - shown));
            }
        }
        out
    }

    /// The structure view: what this profile may show, as lines of text.
    pub fn render(&self) -> String {
        let mut out = format!(
            "Structure ({}, read here without a model; counts and shapes only, no values; \
letters as A/a, digits as 9):\n  {}: {}\n",
            self.format.name(),
            self.format.name(),
            self.layout
        );
        if !self.fields.is_empty() {
            let header = match self.format {
                Format::Env => "  key — format",
                Format::Log => "  field — values",
                _ => "  field — type; present; distinct; length; shapes",
            };
            out.push_str(header);
            out.push('\n');
            for f in self.fields.iter().take(FIELDS_SHOWN) {
                out.push_str(&match self.format {
                    Format::Env => f.render_env(),
                    _ => f.render(self.records),
                });
                out.push('\n');
            }
            if self.fields.len() > FIELDS_SHOWN {
                out.push_str(&format!(
                    "  … {} more fields\n",
                    self.fields.len() - FIELDS_SHOWN
                ));
            }
        }
        if !self.templates.is_empty() {
            out.push_str(&masked::render_templates(&self.templates, self.lines));
        }
        if !self.anomalies.is_empty() {
            out.push_str("  Notable:\n");
            for a in &self.anomalies {
                out.push_str(&format!("  - {a}\n"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::Nothing;
    use super::*;

    /// Knowledge that detects `@` words as emails and title-case pairs as names.
    struct Toy;
    impl Knowledge for Toy {
        fn values(&self, text: &str) -> Vec<(usize, usize, Kind)> {
            let re = regex::Regex::new(r"\S+@\S+|\b[A-Z][a-z]+ [A-Z][a-z]+\b").unwrap();
            re.find_iter(text)
                .map(|m| {
                    let kind = if m.as_str().contains('@') {
                        Kind::Email
                    } else {
                        Kind::Name
                    };
                    (m.start(), m.end(), kind)
                })
                .collect()
        }
        fn public_word(&self, _: &str) -> bool {
            true
        }
        fn data_word(&self, _: &str) -> bool {
            false
        }
    }

    const ORDERS: &str = r#"{
  "export_month": "2026-09",
  "orders": [
    {"order_ref": 9306743338619943, "placed_at": "2026-09-01T16:28:24Z",
     "customer": {"name": "Amelia Quenanlek", "email": "oscar.y01@mailbox-989.net"},
     "payment": {"psp_ts": "2026-09-01 16:28:27.382673", "settlement": "20260902060000"},
     "service": "same-day", "coupon": "", "lines": [{"qty": 1, "unit_price": 3.2}]},
    {"order_ref": 1864320965969843, "placed_at": "2026-09-01T21:42:37Z",
     "customer": {"name": "Kenji Tolvisbaum", "email": "amelia.m91@mailbox-505.net", "__proto__": {"x": 1}},
     "payment": {"psp_ts": "2026-09-02 08:00:01.5", "settlement": "20260903060000"},
     "service": "express", "coupon": "", "lines": []}
  ]
}"#;

    #[test]
    fn a_json_export_is_described_by_paths_shapes_and_pictures() {
        let p = profile(ORDERS, Some(Path::new("data/orders.json")), &Toy).unwrap();
        let view = p.render();
        assert_eq!(p.records, Some(2));
        for want in [
            "records are the 2 elements of `orders`",
            "orders[].order_ref — integer; 2; all distinct; 16 digits",
            "orders[].payment.psp_ts — string; 2; all distinct; len 21-26; date-time yyyy-MM-dd HH:mm:ss.S ×1, date-time yyyy-MM-dd HH:mm:ss.SSSSSS ×1",
            "date-time yyyyMMddHHmmss ×2",
            "orders[].customer.name — string; 2; all distinct; len 16; shapes Aa Aa ×2; looks like name ×2",
            "shapes a.a99@a-999.a ×2",
            "orders[].service — string; 2; all distinct; len 7-8; shapes aaaa-aaa ×1, aaaaaaa ×1",
            "orders[].coupon — string; 2, empty ×2; one value",
            "orders[].lines — array; 2, empty ×1; 0-1 items",
            "orders[].lines[].unit_price — number; 1 (missing in 1 record(s)); decimal places 1 ×1",
            "orders[].customer.__proto__ — object; 1 (missing in 1 record(s))",
            "orders[].payment.psp_ts: 2 date/time layouts in one field",
            "orders[].order_ref: 1 integer(s) above 2^53",
            "keys named like built-in object members: __proto__ (in orders[].customer)",
        ] {
            assert!(view.contains(want), "missing {want:?} in:\n{view}");
        }
        for value in [
            "Amelia",
            "Quenanlek",
            "oscar",
            "9306743338619943",
            "382673",
            "same-day",
            "express",
            "mailbox",
        ] {
            assert!(!view.contains(value), "{value} shown in:\n{view}");
        }
    }

    #[test]
    fn csv_columns_embedded_delimiters_and_ragged_rows_are_counted() {
        let csv = "id,name,note,amount\n1,\"Stone, Ann\",\"said \"\"hi\"\"\",12.50\n2,Bo Li,plain,7\n3,Cy,x\n";
        let p = profile(csv, Some(Path::new("data/c.csv")), &Nothing).unwrap();
        let view = p.render();
        assert_eq!(p.records, Some(3));
        for want in [
            "CSV: 3 rows of 4 fields after a header row; 2 of 11 fields quoted",
            "  id — string; 3; all distinct; len 1; 1 digit; shapes 9 ×3; looks like integer text ×3",
            "name: 1 value(s) holding the delimiter or a quote",
            "note: 1 value(s) holding the delimiter or a quote",
            "1 row(s) with a number of fields other than 4",
            "amount — string; 2 (missing in 1 record(s))",
        ] {
            assert!(view.contains(want), "missing {want:?} in:\n{view}");
        }
        assert!(!view.contains("Stone") && !view.contains("hi"), "{view}");
    }

    #[test]
    fn env_keys_are_listed_with_value_formats_and_secrets_by_length() {
        struct Secrets;
        impl Knowledge for Secrets {
            fn values(&self, t: &str) -> Vec<(usize, usize, Kind)> {
                if t.starts_with("sk_") {
                    vec![(0, t.len(), Kind::Secret)]
                } else {
                    Vec::new()
                }
            }
            fn public_word(&self, _: &str) -> bool {
                false
            }
            fn data_word(&self, _: &str) -> bool {
                false
            }
        }
        let env = "API_KEY=sk_live_Qm8vT2xW9pL4nR7k\nLIMIT=500\nDB_URL=postgres://app:hunter2@db.internal:5432/orders\n";
        let view = profile(env, Some(Path::new(".env")), &Secrets)
            .unwrap()
            .render();
        for want in [
            "API_KEY — secret-like, 24 chars: A-Z a-z 0-9 _",
            "LIMIT — integer, 3 digits",
            "DB_URL — URL a://a:•••@a.a:9999/a",
        ] {
            assert!(view.contains(want), "missing {want:?} in:\n{view}");
        }
        assert!(!view.contains("hunter2") && !view.contains("500") && !view.contains("Qm8v"));
    }

    #[test]
    fn maps_keyed_by_values_show_their_keys_as_stars() {
        let json = r#"{"by_customer": {"ann@x.net": {"n": 1}, "bo@y.org": {"n": 2}}}"#;
        let view = profile(json, None, &Toy).unwrap().render();
        assert!(view.contains("by_customer.*.n — integer; 2"), "{view}");
        assert!(view.contains("2 key(s) look like data"), "{view}");
        assert!(!view.contains("ann@x.net"), "{view}");
    }

    #[test]
    fn xml_and_fixed_width_have_fields_too() {
        let xml = "<orders><order id=\"17\"><name>Ann Stone</name></order><order id=\"18\"><name>Bo Li</name></order></orders>";
        let view = profile(xml, Some(Path::new("data/o.xml")), &Toy)
            .unwrap()
            .render();
        assert!(
            view.contains("root element `orders` with 2 child element(s) (order ×2)"),
            "{view}"
        );
        assert!(
            view.contains("orders/order/@id — string; 2; all distinct; len 2"),
            "{view}"
        );
        assert!(!view.contains("Stone"), "{view}");
        let fixed = "ID   NAME     AMT\n001  Stone    12.5\n002  Brenn    03.0\n003  Kade     99.9\n004  Olm      10.0\n";
        let view = profile(fixed, Some(Path::new("data/f.txt")), &Nothing)
            .unwrap()
            .render();
        assert!(
            view.contains(
                "4 lines of 3 columns at characters 1-3, 6-10, 15-18 after a header line"
            ),
            "{view}"
        );
        assert!(
            view.contains("AMT — string; 4; all distinct; len 4; shapes 99.9 ×4"),
            "{view}"
        );
    }
}
