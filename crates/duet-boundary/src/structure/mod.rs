// SPDX-License-Identifier: GPL-3.0-or-later
//! The structure of sensitive data, told without its values.
//!
//! Three deterministic views of sensitive content, computed on this machine
//! without any model, so the frontier learns how the data is laid out
//! without an `ask_local` round trip per question:
//!
//! - **Structure view** ([`profile`], [`Profile::render`]): the format, the
//!   number of records, the schema (JSON paths, columns, `.env` keys, XML
//!   element paths, log line templates), inferred types, each field's value
//!   formats as shape masks ([`shape`]: letters as `A`/`a`, digits as `9`,
//!   punctuation kept; date and time layouts as pictures such as
//!   `yyyy-MM-dd HH:mm:ss.SSSSSS`), presence, null and empty counts,
//!   distinct-count buckets, length ranges and anomalies (mixed formats,
//!   embedded delimiters, encoding problems, duplicates). Counts and shapes
//!   only, never a value.
//! - **Synthetic twin** ([`twin`]): the first records of a data file with
//!   every value replaced by a rule-generated fake of the same shape (valid
//!   dates, checksummed card numbers and IBANs, the same nulls, lengths,
//!   quoting and edge cases), deterministic per run seed. A fake depends on
//!   its value's shape and on where the value first occurs, never on the
//!   value itself; a check then proves no known value is in the twin.
//! - **Masked output** ([`masked`]): short output of a command that read
//!   sensitive data, with every value replaced by its shape; small numbers
//!   are shown only within a per-run budget.
//!
//! What counts as a value is decided by the caller's [`Knowledge`]: the
//! engine's vault and detectors, the words of the public files, and the
//! words that occur in values of sensitive data.

pub mod dates;
pub mod masked;
pub mod parse;
pub mod profile;
pub mod shape;
pub mod twin;

pub use profile::{Profile, profile};

use crate::detect::Kind;
use std::path::Path;

/// What the caller knows about sensitive values and public words.
pub trait Knowledge {
    /// Byte ranges of the sensitive values in `text` (known or detected),
    /// with their kind.
    fn values(&self, text: &str) -> Vec<(usize, usize, Kind)>;
    /// Whether the lower-cased word occurs in public content (the task,
    /// public files) or in schema the frontier was shown.
    fn public_word(&self, word: &str) -> bool;
    /// Whether the lower-cased word occurs in a value of sensitive data
    /// (a JSON string, a CSV cell): such a word is a value, not structure.
    fn data_word(&self, word: &str) -> bool;
    /// Whether `text` holds a value the caller knows as sensitive: what a
    /// synthetic twin must never hold. A detected format alone does not
    /// count (a fake card number is detected as a card).
    fn known(&self, _text: &str) -> bool {
        false
    }
}

/// Knowledge of nothing: every word is a value, nothing is detected.
pub struct Nothing;

impl Knowledge for Nothing {
    fn values(&self, _: &str) -> Vec<(usize, usize, Kind)> {
        Vec::new()
    }
    fn public_word(&self, _: &str) -> bool {
        false
    }
    fn data_word(&self, _: &str) -> bool {
        false
    }
}

/// The format of a piece of sensitive content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Json,
    JsonLines,
    Delimited {
        delimiter: char,
    },
    Env,
    FixedWidth,
    Xml,
    /// Lines, mostly starting with a timestamp or naming a log level.
    Log,
    /// Anything else: described by its line templates.
    Text,
}

impl Format {
    pub fn name(&self) -> String {
        match self {
            Format::Json => "JSON".into(),
            Format::JsonLines => "JSON lines".into(),
            Format::Delimited { delimiter: ',' } => "CSV".into(),
            Format::Delimited { delimiter: '\t' } => "TSV".into(),
            Format::Delimited { delimiter } => format!("text delimited by `{delimiter}`"),
            Format::Env => "KEY=value".into(),
            Format::FixedWidth => "fixed-width columns".into(),
            Format::Xml => "XML".into(),
            Format::Log => "log".into(),
            Format::Text => "text".into(),
        }
    }

    /// Whether the format has records a synthetic twin can be made of.
    pub fn has_records(&self) -> bool {
        !matches!(self, Format::Log | Format::Text)
    }
}

static LOG_START: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)^\s*(?:\[?\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}|\[?\d{2}:\d{2}:\d{2}|\[?[a-z]{3} +\d{1,2} \d{2}:\d{2}|\d{10,13}\b|\[?(?:trace|debug|info|warn|warning|error|fatal|critical)\b)",
    )
    .expect("static regex")
});

/// The format of `text`, read from `path`'s extension when given, else
/// from the content.
pub fn detect(text: &str, path: Option<&Path>) -> Format {
    let ext = path
        .and_then(|p| p.extension())
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let name = path
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && parse::json(text.trim_start_matches('\u{feff}')).is_some()
    {
        return Format::Json;
    }
    if parse::json_lines(text).is_some() {
        return Format::JsonLines;
    }
    // A file named as one is read as `KEY=value` when it has an
    // assignment; other text only when most of its lines are assignments.
    let env_name = name.starts_with(".env") || name.ends_with(".env") || ext == "env";
    if (env_name && !parse::assignments(text).is_empty())
        || (matches!(ext.as_str(), "" | "properties" | "cfg") && parse::looks_like_env(text))
    {
        return Format::Env;
    }
    if (ext == "xml" || trimmed.starts_with("<?xml") || trimmed.starts_with('<'))
        && parse::xml(text).is_some()
    {
        return Format::Xml;
    }
    let hint = match ext.as_str() {
        "csv" => Some(','),
        "tsv" | "tab" => Some('\t'),
        "psv" => Some('|'),
        _ => None,
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let loglike = lines.iter().filter(|l| LOG_START.is_match(l)).count();
    let is_log = ext == "log" || (!lines.is_empty() && loglike * 2 >= lines.len());
    if !is_log && let Some(d) = parse::sniff_delimiter(text, hint) {
        return Format::Delimited { delimiter: d };
    }
    if !is_log && parse::fixed_width(text).is_some() {
        return Format::FixedWidth;
    }
    if is_log { Format::Log } else { Format::Text }
}

/// The string values of structured content, trimmed: JSON strings,
/// delimited cells after a header, XML text and attributes, fixed-width
/// cells. The caller treats them as values wherever they appear again (in
/// masked output and line templates), even when every word of one is also
/// a word of the public files (`same-day`, a note of plain words).
pub fn values(text: &str, path: Option<&Path>) -> Vec<String> {
    fn json_values(v: &parse::Json, out: &mut Vec<String>) {
        match v {
            parse::Json::Object { entries, .. } => {
                entries.iter().for_each(|(_, x)| json_values(x, out))
            }
            parse::Json::Array { items, .. } => items.iter().for_each(|x| json_values(x, out)),
            parse::Json::String { value, .. } => out.push(value.trim().to_owned()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    let body = text.trim_start_matches('\u{feff}');
    match detect(body, path) {
        Format::Json => {
            if let Some(v) = parse::json(body) {
                json_values(&v, &mut out);
            }
        }
        Format::JsonLines => {
            for v in parse::json_lines(body).unwrap_or_default() {
                json_values(&v, &mut out);
            }
        }
        Format::Delimited { delimiter } => {
            if let Some(rows) = parse::delimited(body, delimiter) {
                let skip = usize::from(profile::has_header(&rows, &Nothing));
                for r in &rows[skip..] {
                    out.extend(r.cells.iter().map(|c| c.value.trim().to_owned()));
                }
            }
        }
        Format::Xml => {
            if let Some(x) = parse::xml(body) {
                out.extend(x.values.iter().map(|v| v.value.trim().to_owned()));
            }
        }
        Format::FixedWidth => {
            if let Some(cols) = parse::fixed_width(body) {
                for l in body.lines().skip(1) {
                    for c in &cols {
                        out.push(l.get(c.clone()).unwrap_or_default().trim().to_owned());
                    }
                }
            }
        }
        Format::Env | Format::Log | Format::Text => {}
    }
    out.retain(|v| !v.is_empty());
    out
}

/// Words that mean "no value" or a yes/no: never a value on their own.
pub fn marker_word(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "null"
            | "none"
            | "nil"
            | "n/a"
            | "na"
            | "undefined"
            | "nan"
            | "true"
            | "false"
            | "yes"
            | "no"
            | "y"
            | "n"
            | "-"
            | "--"
    )
}

/// Built-in member names of common languages' objects: data keys with
/// these names are worth knowing about (they collide with the language).
pub const SPECIAL_KEYS: &[&str] = &[
    "__proto__",
    "constructor",
    "prototype",
    "toString",
    "valueOf",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__class__",
    "__dict__",
    "__init__",
];

/// Whether a key (a JSON key, a column or element name) is schema that may
/// be shown as written: 1 to 64 characters of letters, digits and simple
/// punctuation, no run of four or more digits (dates, ids), and nothing
/// the caller knows or detects as a sensitive value. Other keys are data
/// (a map keyed by email addresses or customer ids) and shown as `*`.
pub fn schema_key(key: &str, k: &dyn Knowledge) -> bool {
    let n = key.chars().count();
    let mut digits = 0;
    let mut longest = 0;
    for c in key.chars() {
        if c.is_ascii_digit() {
            digits += 1;
            longest = longest.max(digits);
        } else {
            digits = 0;
        }
    }
    (1..=64).contains(&n)
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || " _-.$@#:/()[]".contains(c))
        && longest < 4
        && k.values(key).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_detected_by_content_and_extension() {
        let p = |s: &str| Some(Path::new(s).to_owned());
        assert_eq!(detect("{\"a\": [1, 2]}", None), Format::Json);
        assert_eq!(detect("{\"a\":1}\n{\"a\":2}\n", None), Format::JsonLines);
        assert_eq!(
            detect("id,name\n1,a\n2,b\n", p("data/x.csv").as_deref()),
            Format::Delimited { delimiter: ',' }
        );
        assert_eq!(
            detect("id\tname\n1\ta\n2\tb\n", None),
            Format::Delimited { delimiter: '\t' }
        );
        assert_eq!(detect("A=1\nB=two\n", p(".env").as_deref()), Format::Env);
        assert_eq!(
            detect("<r><a x=\"1\">t</a></r>", p("data/x.xml").as_deref()),
            Format::Xml
        );
        assert_eq!(
            detect(
                "2026-09-01T00:00:09Z INFO start, now\n2026-09-01T00:00:10Z ERROR x, y\n",
                p("logs/a.log").as_deref()
            ),
            Format::Log
        );
        assert_eq!(detect("plain words here\nand more\n", None), Format::Text);
    }

    #[test]
    fn keys_that_look_like_values_are_not_schema() {
        for ok in [
            "order_ref",
            "line1",
            "__proto__",
            "Order Date (UTC)",
            "psp_ts",
        ] {
            assert!(schema_key(ok, &Nothing), "{ok}");
        }
        for not in [
            "2026-09-01",
            "cust-004211",
            "",
            &"k".repeat(65),
            "a\nb",
            "x=y",
        ] {
            assert!(!schema_key(not, &Nothing), "{not}");
        }
    }
}
