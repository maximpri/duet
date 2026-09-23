// SPDX-License-Identifier: GPL-3.0-or-later
//! Interface-only views of source files.
//!
//! A skeleton keeps what a caller needs — signatures, type definitions, doc
//! comments and public constants — and withholds what an implementation is:
//! function and method bodies, the values of private constants, top-level
//! statements and ordinary comments. The file is parsed with tree-sitter and
//! the withheld parts are reported as byte ranges into the original text, so
//! the view is deterministic and exact for any UTF-8 content.
//!
//! A file that does not parse cleanly has no skeleton: error recovery could
//! place body text outside a body, so the caller withholds the whole file.

use std::path::Path;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    Python,
}

impl Lang {
    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "rs" => Some(Self::Rust),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "py" | "pyi" => Some(Self::Python),
            _ => None,
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// A function body or top-level statement: replaced by a body handle.
    Body,
    /// The value of a non-public constant or variable: replaced by a value handle.
    Value,
    /// An ordinary (non-doc) comment: removed.
    Comment,
}

/// One withheld byte range of the original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: SpanKind,
    /// What the range belongs to, e.g. `fn Engine::quote` or `const RATE`.
    pub item: String,
    /// Whether the range is a braced block (rendered as `{ marker }`).
    pub braced: bool,
}

/// The withheld ranges of `src`, in order and non-overlapping.
pub fn withheld(lang: Lang, src: &str) -> Result<Vec<Span>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&lang.grammar())
        .map_err(|e| format!("grammar: {e}"))?;
    let tree = parser.parse(src, None).ok_or("the parser gave up")?;
    let root = tree.root_node();
    if root.has_error() {
        return Err("the file does not parse cleanly".into());
    }
    let mut w = Walk {
        src,
        spans: Vec::new(),
    };
    match lang {
        Lang::Rust => w.rust(root, ""),
        Lang::TypeScript | Lang::Tsx => w.ts(root, ""),
        Lang::Python => w.python(root, ""),
    }
    w.spans.sort_by_key(|s| s.start);
    Ok(w.spans)
}

/// `src` with every span replaced: bodies and values by `marker(span)`,
/// comments removed (with their line when they stood alone).
pub fn render(src: &str, spans: &[Span], mut marker: impl FnMut(&Span) -> String) -> String {
    let mut out = String::with_capacity(src.len() / 2);
    let mut last = 0;
    for s in spans {
        if s.start < last {
            continue;
        }
        out.push_str(&src[last..s.start]);
        match s.kind {
            SpanKind::Comment => {
                let line_start = out.rfind('\n').map_or(0, |i| i + 1);
                let rest_end = src[s.end..].find('\n').map_or(src.len(), |i| s.end + i);
                if out[line_start..].trim().is_empty() && src[s.end..rest_end].trim().is_empty() {
                    out.truncate(line_start);
                    last = (rest_end + 1).min(src.len());
                } else {
                    let kept = out.trim_end_matches([' ', '\t']).len();
                    out.truncate(kept);
                    last = s.end;
                }
            }
            SpanKind::Body | SpanKind::Value => {
                let m = marker(s);
                if s.braced {
                    out.push_str(&format!("{{ {m} }}"));
                } else {
                    out.push_str(&m);
                }
                last = s.end;
            }
        }
    }
    out.push_str(&src[last..]);
    out
}

struct Walk<'s> {
    src: &'s str,
    spans: Vec<Span>,
}

impl<'s> Walk<'s> {
    fn text(&self, n: Node<'_>) -> &'s str {
        &self.src[n.start_byte()..n.end_byte()]
    }

    fn field_text(&self, n: Node<'_>, field: &str) -> &'s str {
        n.child_by_field_name(field).map_or("", |c| self.text(c))
    }

    fn push(&mut self, n: Node<'_>, kind: SpanKind, item: String, braced: bool) {
        let mut end = n.end_byte();
        if kind == SpanKind::Comment {
            // Some grammars include the line break in a line comment.
            end = n.start_byte() + self.text(n).trim_end_matches(['\n', '\r']).len();
        }
        self.spans.push(Span {
            start: n.start_byte(),
            end,
            kind,
            item,
            braced,
        });
    }

    fn children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
        let mut c = n.walk();
        n.children(&mut c).collect()
    }

    // ---- Rust ----

    fn rust(&mut self, n: Node<'_>, owner: &str) {
        match n.kind() {
            "function_item" => {
                if let Some(body) = n.child_by_field_name("body") {
                    let item = format!("fn {owner}{}", self.field_text(n, "name"));
                    self.push(body, SpanKind::Body, item, true);
                }
            }
            "const_item" | "static_item" => {
                let public = Self::children(n)
                    .iter()
                    .any(|c| c.kind() == "visibility_modifier" && self.text(*c) == "pub");
                if !public && let Some(v) = n.child_by_field_name("value") {
                    let item = format!("const {}", self.field_text(n, "name"));
                    self.push(v, SpanKind::Value, item, false);
                }
            }
            "macro_definition" => {
                let name = self.field_text(n, "name");
                for rule in Self::children(n) {
                    if let Some(right) = rule.child_by_field_name("right") {
                        self.push(right, SpanKind::Body, format!("macro {name}!"), true);
                    }
                }
            }
            "line_comment" | "block_comment" => {
                let doc = n.child_by_field_name("outer").is_some()
                    || n.child_by_field_name("inner").is_some();
                if !doc {
                    self.push(n, SpanKind::Comment, String::new(), false);
                }
            }
            "impl_item" | "trait_item" => {
                let ty = match n.kind() {
                    "impl_item" => self.field_text(n, "type"),
                    _ => self.field_text(n, "name"),
                };
                let owner = format!("{ty}::");
                for c in Self::children(n) {
                    self.rust(c, &owner);
                }
            }
            _ => {
                for c in Self::children(n) {
                    self.rust(c, owner);
                }
            }
        }
    }

    // ---- TypeScript ----

    fn is_function(n: Node<'_>) -> bool {
        matches!(
            n.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "function_expression"
                | "function"
                | "generator_function"
                | "arrow_function"
                | "method_definition"
        )
    }

    fn ts(&mut self, n: Node<'_>, owner: &str) {
        let top = n.parent().is_some_and(|p| p.kind() == "program");
        match n.kind() {
            k if Self::is_function(n) => {
                if let Some(body) = n.child_by_field_name("body") {
                    let name = self.field_text(n, "name");
                    let name = if name.is_empty() { owner } else { name };
                    let braced = body.kind() == "statement_block";
                    let kind = if k == "method_definition" {
                        "method"
                    } else {
                        "function"
                    };
                    self.push(body, SpanKind::Body, format!("{kind} {name}"), braced);
                }
            }
            "lexical_declaration" | "variable_declaration" if top => {
                // Not exported: values withheld, function values reduced to their signature.
                for d in Self::children(n) {
                    if d.kind() != "variable_declarator" {
                        continue;
                    }
                    let name = self.field_text(d, "name");
                    if let Some(v) = d.child_by_field_name("value") {
                        if Self::is_function(v) {
                            self.ts(v, name);
                        } else {
                            self.push(v, SpanKind::Value, format!("const {name}"), false);
                        }
                    }
                }
            }
            "variable_declarator" => {
                let name = self.field_text(n, "name");
                for c in Self::children(n) {
                    self.ts(c, name);
                }
            }
            "public_field_definition" => {
                let private = Self::children(n).iter().any(|c| {
                    (c.kind() == "accessibility_modifier" && self.text(*c) != "public")
                        || c.kind() == "private_property_identifier"
                });
                let name = self.field_text(n, "name");
                match n.child_by_field_name("value") {
                    Some(v) if Self::is_function(v) => self.ts(v, name),
                    Some(v) if private => {
                        self.push(v, SpanKind::Value, format!("field {name}"), false)
                    }
                    _ => {}
                }
            }
            "expression_statement" if top => {
                self.push(n, SpanKind::Body, "top-level statement".into(), false);
            }
            "comment" => {
                if !self.text(n).starts_with("/**") {
                    self.push(n, SpanKind::Comment, String::new(), false);
                }
            }
            _ => {
                for c in Self::children(n) {
                    self.ts(c, owner);
                }
            }
        }
    }

    // ---- Python ----

    fn is_docstring(&self, n: Node<'_>) -> bool {
        n.kind() == "expression_statement"
            && n.named_child_count() == 1
            && n.named_child(0).is_some_and(|c| c.kind() == "string")
    }

    fn python(&mut self, n: Node<'_>, owner: &str) {
        match n.kind() {
            "function_definition" => {
                let Some(body) = n.child_by_field_name("body") else {
                    return;
                };
                let item = format!("def {owner}{}", self.field_text(n, "name"));
                let stmts = Self::children(body);
                let skip = usize::from(stmts.first().is_some_and(|s| self.is_docstring(*s)));
                if let (Some(first), Some(last)) = (stmts.get(skip), stmts.last()) {
                    self.spans.push(Span {
                        start: first.start_byte(),
                        end: last.end_byte().max(first.end_byte()),
                        kind: SpanKind::Body,
                        item,
                        braced: false,
                    });
                }
            }
            "class_definition" => {
                let owner = format!("{}.", self.field_text(n, "name"));
                if let Some(body) = n.child_by_field_name("body") {
                    for c in Self::children(body) {
                        self.python(c, &owner);
                    }
                }
            }
            "decorated_definition" => {
                for c in Self::children(n) {
                    self.python(c, owner);
                }
            }
            "comment" => self.push(n, SpanKind::Comment, String::new(), false),
            "import_statement"
            | "import_from_statement"
            | "future_import_statement"
            | "decorator" => {}
            "expression_statement" if self.is_docstring(n) => {}
            "expression_statement" => {
                let assign = n
                    .named_child(0)
                    .filter(|c| matches!(c.kind(), "assignment" | "augmented_assignment"));
                match assign {
                    Some(a) => {
                        let name = self.field_text(a, "left");
                        let constant = !name.starts_with('_')
                            && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                            && name
                                .chars()
                                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                        if !constant && let Some(v) = a.child_by_field_name("right") {
                            self.push(v, SpanKind::Value, format!("{owner}{name}"), false);
                        }
                    }
                    None => self.push(n, SpanKind::Body, "top-level statement".into(), false),
                }
            }
            "module" | "block" => {
                for c in Self::children(n) {
                    self.python(c, owner);
                }
            }
            // Other statements at module or class level (if/for/try/with/…) run code.
            _ if n.is_named() => {
                self.push(n, SpanKind::Body, "top-level statement".into(), false);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skeleton(lang: Lang, src: &str) -> (String, Vec<Span>) {
        let spans = withheld(lang, src).unwrap();
        let mut n = 0;
        let out = render(src, &spans, |s| {
            n += 1;
            match s.kind {
                SpanKind::Value => format!("⟨value:h{n}⟩"),
                _ => format!("⟨body:h{n}⟩"),
            }
        });
        (out, spans)
    }

    const RUST: &str = r#"//! Pricing engine.
use std::collections::BTreeMap;

/// Largest discount, in basis points.
pub const MAX_DISCOUNT_BPS: u32 = 1200;
const UPLIFT_GOLD: u32 = 120; // committee 2026
pub(crate) static TABLE_KEY: &str = "zqk3v9x2m1p8r7t6";

/// A customer's tier.
#[derive(Debug, Clone, Copy)]
pub enum Tier {
    Standard,
    /// Pays early.
    Gold,
}

// Internal note: calibrated by the pricing team.
pub struct Engine {
    pub region: String,
}

impl Engine {
    /// Discount for `qty` units — naïve form ✓.
    pub fn discount_bps(&self, qty: u32, tier: Tier) -> u32 {
        let base = if qty >= 200 { 750 } else { 150 };
        base + UPLIFT_GOLD * (tier as u32) // tuned
    }
}

pub trait Priced {
    fn price(&self) -> u64;
    fn label(&self) -> String {
        format!("prix réduit: {} €", self.price())
    }
}

macro_rules! bps {
    ($x:expr) => {
        $x * 100
    };
}

fn helper_ünïcode(s: &str) -> usize {
    s.chars().filter(|c| *c == 'ß').count()
}
"#;

    #[test]
    fn rust_skeleton_keeps_the_interface_and_withholds_bodies() {
        let (out, spans) = skeleton(Lang::Rust, RUST);
        for kept in [
            "//! Pricing engine.",
            "/// Largest discount, in basis points.",
            "pub const MAX_DISCOUNT_BPS: u32 = 1200;",
            "const UPLIFT_GOLD: u32 = ⟨value:",
            "pub(crate) static TABLE_KEY: &str = ⟨value:",
            "/// Pays early.\n    Gold,",
            "pub struct Engine {\n    pub region: String,\n}",
            "/// Discount for `qty` units — naïve form ✓.",
            "pub fn discount_bps(&self, qty: u32, tier: Tier) -> u32 { ⟨body:",
            "fn price(&self) -> u64;",
            "fn label(&self) -> String { ⟨body:",
            "($x:expr) => { ⟨body:",
            "fn helper_ünïcode(s: &str) -> usize { ⟨body:",
        ] {
            assert!(out.contains(kept), "missing {kept:?} in:\n{out}");
        }
        for withheld in [
            "750",
            "committee",
            "zqk3v9x2m1p8r7t6",
            "Internal note",
            "tuned",
            "prix réduit",
            "* 100",
            "'ß'",
            "= 120;",
        ] {
            assert!(!out.contains(withheld), "{withheld:?} leaked:\n{out}");
        }
        let items: Vec<&str> = spans
            .iter()
            .filter(|s| s.kind == SpanKind::Body)
            .map(|s| s.item.as_str())
            .collect();
        assert_eq!(
            items,
            [
                "fn Engine::discount_bps",
                "fn Priced::label",
                "macro bps!",
                "fn helper_ünïcode"
            ]
        );
        // Byte ranges are exact: every body range is a whole braced block.
        for s in spans.iter().filter(|s| s.kind == SpanKind::Body) {
            let t = &RUST[s.start..s.end];
            assert!(t.starts_with('{') && t.ends_with('}'), "{t:?}");
        }
    }

    #[test]
    fn skeletons_are_deterministic_and_reject_broken_files() {
        assert_eq!(skeleton(Lang::Rust, RUST).0, skeleton(Lang::Rust, RUST).0);
        assert!(withheld(Lang::Rust, "fn broken( { let x = ; }").is_err());
        assert!(withheld(Lang::Python, "def f(:\n  pass\n").is_err());
    }

    const TS: &str = r#"/** Tier codes. */
export enum Tier { Standard = "S", Gold = "G" }

/** Largest discount. */
export const MAX_BPS = 1200;
const SECRET_UPLIFT = 120;
// calibrated in 2026
export interface Line { sku: string; qty: number }

/** Discount in basis points — “naïve” ✓. */
export function discountBps(qty: number, tier: Tier): number {
  return qty >= 200 ? 750 : 150;
}

export const double = (x: number): number => x * 2;
const hidden = function (a: string) { return a + "zqsecretliteral01"; };

export class Engine {
  private table = { gold: 777 };
  region = "eu";
  constructor(private readonly r: string) { this.r = r + "é"; }
  quote(line: Line): number { return line.qty * 3; }
}

register(Engine);
"#;

    #[test]
    fn typescript_skeleton_keeps_the_interface_and_withholds_bodies() {
        let (out, _) = skeleton(Lang::TypeScript, TS);
        for kept in [
            "/** Tier codes. */",
            "export enum Tier { Standard = \"S\", Gold = \"G\" }",
            "export const MAX_BPS = 1200;",
            "const SECRET_UPLIFT = ⟨value:",
            "export interface Line { sku: string; qty: number }",
            "/** Discount in basis points — “naïve” ✓. */",
            "export function discountBps(qty: number, tier: Tier): number { ⟨body:",
            "export const double = (x: number): number => ⟨body:",
            "const hidden = function (a: string) { ⟨body:",
            "private table = ⟨value:",
            "region = \"eu\";",
            "constructor(private readonly r: string) { ⟨body:",
            "quote(line: Line): number { ⟨body:",
        ] {
            assert!(out.contains(kept), "missing {kept:?} in:\n{out}");
        }
        for withheld in [
            "750",
            "= 120;",
            "calibrated",
            "x * 2",
            "zqsecretliteral01",
            "777",
            "\"é\"",
            "* 3",
            "register(Engine)",
        ] {
            assert!(!out.contains(withheld), "{withheld:?} leaked:\n{out}");
        }
        let tsx = "export function View(p: { n: number }) { return <div>{p.n * 7}</div>; }\n";
        let (out, _) = skeleton(Lang::Tsx, tsx);
        assert!(
            out.contains("export function View(p: { n: number }) { ⟨body:") && !out.contains("* 7")
        );
    }

    const PY: &str = r#""""Pricing engine."""
import math
from dataclasses import dataclass

MAX_DISCOUNT_BPS = 1200
_UPLIFT = {"gold": 120}
table_key = "zqk3v9x2m1p8r7t6"
# calibrated in 2026


@dataclass
class Quote:
    """A priced line — naïve ✓."""
    sku: str
    qty: int = 1
    RATE = 3

    def total(self) -> int:
        """Net total in cents."""
        rate = 750 if self.qty >= 200 else 150
        return self.qty * rate

    async def fetch(self): return "réponse"


def helper(x):
    return x * 42


if __name__ == "__main__":
    print(helper(2))
"#;

    #[test]
    fn python_skeleton_keeps_docstrings_and_withholds_bodies() {
        let (out, spans) = skeleton(Lang::Python, PY);
        for kept in [
            "\"\"\"Pricing engine.\"\"\"",
            "import math",
            "MAX_DISCOUNT_BPS = 1200",
            "_UPLIFT = ⟨value:",
            "table_key = ⟨value:",
            "@dataclass\nclass Quote:",
            "\"\"\"A priced line — naïve ✓.\"\"\"",
            "sku: str",
            "qty: int = ⟨value:",
            "RATE = 3",
            "def total(self) -> int:\n        \"\"\"Net total in cents.\"\"\"\n        ⟨body:",
            "async def fetch(self): ⟨body:",
            "def helper(x):\n    ⟨body:",
        ] {
            assert!(out.contains(kept), "missing {kept:?} in:\n{out}");
        }
        for withheld in [
            "\"gold\"",
            "zqk3v9x2m1p8r7t6",
            "calibrated",
            "750",
            "réponse",
            "* 42",
            "__main__",
        ] {
            assert!(!out.contains(withheld), "{withheld:?} leaked:\n{out}");
        }
        assert!(
            spans.iter().any(|s| s.item == "def Quote.total"),
            "{spans:?}"
        );
    }

    #[test]
    fn languages_come_from_extensions() {
        assert_eq!(Lang::for_path(Path::new("a/b.rs")), Some(Lang::Rust));
        assert_eq!(Lang::for_path(Path::new("x.tsx")), Some(Lang::Tsx));
        assert_eq!(Lang::for_path(Path::new("x.py")), Some(Lang::Python));
        assert_eq!(Lang::for_path(Path::new("x.go")), None);
    }
}
