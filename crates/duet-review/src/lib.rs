// SPDX-License-Identifier: GPL-3.0-or-later
//! First, deliberately narrow review rules. Syntax is evidence for a candidate,
//! not a proof of exploitability. Only imported Python requests calls with a
//! literal `verify=False` and no binding ambiguity are rule-confirmed. No I/O,
//! execution, external rules or model access belongs in this crate.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser};

pub const REVISION: u32 = 3;
pub const MAX_CONTEXT: usize = 12_000;
pub const MAX_CANDIDATES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    High,
    Medium,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub rule: &'static str,
    pub line: usize,
    pub severity: Severity,
    pub confirmed: bool,
    /// A source classified by the host's privacy index, not by the reviewer.
    pub known_private_value: bool,
    pub message: &'static str,
    /// Stable across line shifts; includes the expressions traced into a sink.
    /// Private source material: hash before persisting or displaying.
    pub identity: String,
    /// Raw source, for a local reviewer only; never a frontier tool result.
    pub context: String,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub candidates: Vec<Candidate>,
    pub incomplete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Likely,
    Unlikely,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Judgment {
    pub verdict: Verdict,
    pub severity: OpinionSeverity,
    pub reason: Reason,
    pub explanation: String,
    pub fix: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpinionSeverity {
    High,
    Medium,
    Low,
    Informational,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    ExposedPath,
    BlockedPath,
    NotSensitive,
    NotApplicable,
    MissingContext,
}

impl Judgment {
    /// A structured contradiction is uncertainty, never evidence of safety.
    /// Keep the prose for the local output filter, and expose the normalized
    /// verdict to every consumer, including measurement harnesses.
    pub fn reconcile(&mut self, known_private_value: bool) {
        let expected = match self.reason {
            Reason::ExposedPath => Verdict::Likely,
            Reason::BlockedPath | Reason::NotSensitive | Reason::NotApplicable => Verdict::Unlikely,
            Reason::MissingContext => Verdict::Uncertain,
        };
        if self.verdict != expected || (known_private_value && self.reason == Reason::NotSensitive)
        {
            self.verdict = Verdict::Uncertain;
            self.reason = Reason::MissingContext;
            self.explanation = format!(
                "The opinion contradicted its structured evidence or the host's private-source classification; treat it as uncertain. {}",
                self.explanation
            );
        }
    }
}

pub mod context;
mod patterns;
pub use patterns::{hosts, removed_guards};

pub fn supported(extension: &str) -> bool {
    matches!(extension, "py" | "rs" | "ts" | "tsx" | "js" | "jsx")
}

fn text<'a>(n: Node<'_>, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}
fn nodes(n: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut todo = vec![n];
    while let Some(n) = todo.pop() {
        out.push(n);
        let mut c = n.walk();
        let children: Vec<_> = n.named_children(&mut c).collect();
        todo.extend(children.into_iter().rev());
    }
    out
}
fn compact(s: &str) -> String {
    s.split_whitespace().collect()
}
fn scope(mut n: Node<'_>) -> Node<'_> {
    while let Some(parent) = n.parent() {
        n = parent;
        if matches!(
            n.kind(),
            "function_definition"
                | "function_item"
                | "function_declaration"
                | "arrow_function"
                | "method_definition"
        ) {
            break;
        }
    }
    n
}
fn context(n: Node<'_>, src: &str) -> String {
    let enclosing = scope(n);
    let enclosing = enclosing
        .parent()
        .filter(|p| p.kind() == "decorated_definition")
        .unwrap_or(enclosing);
    let s = text(enclosing, src);
    if s.len() <= MAX_CONTEXT {
        return s.to_owned();
    }
    // A bounded window centred on the call, keeping UTF-8 boundaries.
    let mut start = n.start_byte().saturating_sub(MAX_CONTEXT / 2);
    while !src.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + MAX_CONTEXT).min(src.len());
    while !src.is_char_boundary(end) {
        end -= 1;
    }
    src[start..end].to_owned()
}

/// Conservative expression expansion, inside one lexical function only. This
/// discovers candidates, including assignments in branches; it never confirms
/// a path. Includes simple container writes and loop bindings, but does not
/// prove aliasing, control flow, sanitization or interprocedural behavior.
struct Evidence {
    text: String,
    dynamic: bool,
    shell: bool,
    sensitive: bool,
    value_sensitive: bool,
    untrusted: bool,
    capped: bool,
}

fn binding(n: Node<'_>) -> Option<Node<'_>> {
    n.child_by_field_name("left")
        .or_else(|| n.child_by_field_name("pattern"))
        .or_else(|| n.child_by_field_name("name"))
}

fn value(n: Node<'_>) -> Option<Node<'_>> {
    n.child_by_field_name("right")
        .or_else(|| n.child_by_field_name("value"))
}

// Follow the root of a member/index write, not the index expression: a write
// to table[index] updates table; it does not taint index. Container keys are
// deliberately merged, so a sibling-key read can remain a false candidate.
fn root_binding(mut n: Node<'_>) -> Node<'_> {
    while matches!(
        n.kind(),
        "subscript"
            | "subscript_expression"
            | "attribute"
            | "member_expression"
            | "field_expression"
            | "index_expression"
    ) {
        let Some(next) = n
            .child_by_field_name("object")
            .or_else(|| n.child_by_field_name("argument"))
            .or_else(|| n.child_by_field_name("value"))
            .or_else(|| n.named_child(0))
        else {
            break;
        };
        n = next;
    }
    n
}

fn sensitive_name(name: &str, fields: &[String]) -> bool {
    [
        "password",
        "secret",
        "token",
        "ssn",
        "card_number",
        "api_key",
    ]
    .contains(&name.to_ascii_lowercase().as_str())
        || fields.iter().any(|f| f.eq_ignore_ascii_case(name))
}

fn private_literal(n: Node<'_>, spans: &[(usize, usize)]) -> bool {
    matches!(
        n.kind(),
        "string"
            | "string_literal"
            | "raw_string_literal"
            | "template_string"
            | "integer"
            | "integer_literal"
            | "float"
            | "float_literal"
            | "number"
    ) && spans
        .iter()
        .any(|(s, e)| *s < n.end_byte() && *e > n.start_byte())
}

fn sensitive_node(n: Node<'_>, src: &str, fields: &[String], spans: &[(usize, usize)]) -> bool {
    if private_literal(n, spans) {
        return true;
    }
    // Literal labels and comments are not value flows. A known value must
    // overlap an actual literal, not just the enclosing argument list.
    if matches!(n.kind(), "string" | "string_literal" | "template_string") {
        if private_literal(n, spans) {
            return true;
        }
        return n.parent().is_some_and(|p| {
            (matches!(
                p.kind(),
                "subscript" | "subscript_expression" | "index_expression"
            ) || (matches!(p.kind(), "argument_list" | "arguments")
                && p.parent()
                    .and_then(|c| c.child_by_field_name("function"))
                    .is_some_and(|f| text(f, src).ends_with(".get"))))
                && sensitive_name(text(n, src).trim_matches(['\'', '"']), fields)
        });
    }
    if !matches!(
        n.kind(),
        "identifier" | "field_identifier" | "property_identifier"
    ) {
        return false;
    }
    // Object keys and keyword argument names describe slots, not their values.
    if n.parent().is_some_and(|p| {
        ["pair", "keyword_argument", "field_initializer"].contains(&p.kind())
            && p.child_by_field_name("key")
                .or_else(|| p.child_by_field_name("name"))
                .or_else(|| p.child_by_field_name("field"))
                .is_some_and(|key| key.id() == n.id())
    }) {
        return false;
    }
    sensitive_name(text(n, src), fields)
}

fn evidence(
    n: Node<'_>,
    src: &str,
    call: Node<'_>,
    fields: &[String],
    spans: &[(usize, usize)],
) -> Evidence {
    let enclosing = scope(call);
    let assignments: Vec<_> = nodes(enclosing)
        .into_iter()
        .filter(|a| {
            matches!(
                a.kind(),
                "assignment"
                    | "augmented_assignment"
                    | "assignment_expression"
                    | "augmented_assignment_expression"
                    | "compound_assignment_expr"
                    | "let_declaration"
                    | "variable_declarator"
                    | "call"
                    | "call_expression"
                    | "for_statement"
                    | "for_expression"
            ) && (if matches!(a.kind(), "for_statement" | "for_expression") {
                value(*a).is_some_and(|v| v.end_byte() < call.start_byte())
            } else {
                a.end_byte() < call.start_byte()
            }) && scope(*a).id() == enclosing.id()
        })
        .collect();
    let mut out = Evidence {
        text: String::new(),
        dynamic: false,
        shell: false,
        sensitive: false,
        value_sensitive: false,
        untrusted: false,
        capped: false,
    };
    let mut todo = vec![(n, 0)];
    let mut seen = std::collections::HashSet::new();
    while let Some((expr, depth)) = todo.pop() {
        if depth >= 12 || out.text.len() + text(expr, src).len() + 1 > MAX_CONTEXT {
            out.capped = true;
            break;
        }
        out.text.push_str(text(expr, src));
        out.text.push('\n');
        let parts = nodes(expr);
        out.sensitive |= parts.iter().any(|n| sensitive_node(*n, src, fields, spans));
        out.value_sensitive |= parts.iter().any(|n| private_literal(*n, spans));
        out.untrusted |= parts.iter().any(|n| patterns::untrusted(*n, src));
        out.dynamic |= parts.iter().any(|n| {
            matches!(
                n.kind(),
                "binary_operator" | "binary_expression" | "interpolation" | "template_substitution"
            ) || (n.kind() == "call"
                && n.child_by_field_name("function")
                    .is_some_and(|f| text(f, src).ends_with(".format")))
        });
        out.shell |= parts.iter().any(|n| {
            n.kind() == "string"
                && ["sh", "bash", "/bin/sh", "/bin/bash", "cmd", "cmd.exe"]
                    .contains(&text(*n, src).trim_matches(['\'', '"']))
        });
        for id in parts.into_iter().filter(|i| i.kind() == "identifier") {
            let name = text(id, src);
            if !seen.insert(name) {
                continue;
            }
            for value in assignments.iter().filter_map(|a| {
                if matches!(a.kind(), "call" | "call_expression") {
                    let function = a.child_by_field_name("function")?;
                    let object = function
                        .child_by_field_name("object")
                        .or_else(|| function.child_by_field_name("value"))?;
                    let method = function
                        .child_by_field_name("attribute")
                        .or_else(|| function.child_by_field_name("property"))
                        .or_else(|| function.child_by_field_name("field"))?;
                    return (text(object, src) == name
                        && matches!(
                            text(method, src),
                            "append" | "extend" | "push" | "push_str" | "arg" | "args"
                        ))
                    .then(|| a.child_by_field_name("arguments"))
                    .flatten();
                }
                let left = root_binding(binding(*a)?);
                (left.kind() == "identifier" && text(left, src) == name)
                    .then(|| value(*a))
                    .flatten()
            }) {
                todo.push((value, depth + 1));
            }
        }
    }
    out
}

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
}

/// Resolve only `import requests [as name]` with no other binding of that name
/// anywhere in the file. A doubtful binding makes the candidate advisory.
fn requests_bindings(root: Node<'_>, src: &str) -> BTreeMap<String, bool> {
    let all = nodes(root);
    let mut found = BTreeMap::new();
    for n in &all {
        if n.kind() != "import_statement" || n.parent().is_none_or(|p| p.kind() != "module") {
            continue;
        }
        let mut c = n.walk();
        for item in n.named_children(&mut c) {
            let name = item.child_by_field_name("name").unwrap_or(item);
            if text(name, src) == "requests" {
                let alias = item
                    .child_by_field_name("alias")
                    .map_or("requests", |a| text(a, src));
                found.insert(alias.to_owned(), true);
            }
        }
    }
    for (alias, valid) in &mut found {
        for n in &all {
            if matches!(n.kind(), "import_statement") {
                continue;
            }
            let binding = match n.kind() {
                "assignment"
                | "augmented_assignment"
                | "named_expression"
                | "for_statement"
                | "for_in_clause" => n
                    .child_by_field_name("left")
                    .or_else(|| n.child_by_field_name("name")),
                "function_definition" | "class_definition" => n.child_by_field_name("name"),
                "parameters"
                | "lambda_parameters"
                | "import_from_statement"
                | "with_item"
                | "except_clause"
                | "case_pattern"
                | "delete_statement"
                | "global_statement"
                | "nonlocal_statement" => Some(*n),
                _ => None,
            };
            if binding.is_some_and(|b| words(text(b, src)).any(|w| w == alias)) {
                *valid = false;
            }
        }
        let imports: Vec<_> = all
            .iter()
            .filter(|n| n.kind() == "import_statement" && words(text(**n, src)).any(|w| w == alias))
            .collect();
        if imports.len() != 1 {
            *valid = false;
        }
    }
    found
}

/// Rules on one UTF-8 source file. `fields` are schema/secret *names*, never
/// their values. Every matching privacy flow remains advisory.
pub fn scan(extension: &str, src: &str, fields: &[String]) -> Scan {
    scan_with_values(extension, src, fields, &[])
}

/// As `scan`, with byte ranges recognized by the caller's local privacy
/// boundary. Values stay with that boundary; ranges are only matched against
/// syntax literals. Invalid ranges are ignored, never used to slice source.
pub fn scan_with_values(
    extension: &str,
    src: &str,
    fields: &[String],
    spans: &[(usize, usize)],
) -> Scan {
    let spans: Vec<_> = spans
        .iter()
        .copied()
        .filter(|(s, e)| {
            s < e && *e <= src.len() && src.is_char_boundary(*s) && src.is_char_boundary(*e)
        })
        .collect();
    let Some(tree) = patterns::tree(extension, src) else {
        return Scan {
            incomplete: true,
            ..Scan::default()
        };
    };
    let root = tree.root_node();
    let mut out = Scan {
        incomplete: root.has_error(),
        ..Scan::default()
    };
    let bindings = if extension == "py" {
        requests_bindings(root, src)
    } else {
        BTreeMap::new()
    };
    for call in nodes(root)
        .into_iter()
        .filter(|n| matches!(n.kind(), "call" | "call_expression" | "macro_invocation"))
    {
        let Some(func) = call
            .child_by_field_name("function")
            .or_else(|| call.child_by_field_name("macro"))
        else {
            continue;
        };
        let Some(args) = call
            .child_by_field_name("arguments")
            .or_else(|| call.named_child(1))
        else {
            continue;
        };
        let callee = compact(text(func, src));
        let arg_text = text(args, src);
        let proof = evidence(args, src, call, fields, &spans);
        out.incomplete |= proof.capped;
        let short = callee.rsplit(['.', ':']).next().unwrap_or(&callee);
        let mut add = |rule, severity, confirmed, message, evidence: &str| {
            if out.candidates.len() == MAX_CANDIDATES {
                out.incomplete = true;
                return;
            }
            out.candidates.push(Candidate {
                rule,
                line: call.start_position().row + 1,
                severity,
                confirmed: confirmed && !root.has_error(),
                known_private_value: proof.value_sensitive,
                message,
                identity: format!("{rule}\0{}\0{}", compact(text(func, src)), evidence),
                context: context(call, src),
            });
        };
        if extension == "py"
            && matches!(
                short,
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "request"
            )
        {
            let mut c = args.walk();
            let disabled = args.named_children(&mut c).any(|a| {
                a.kind() == "keyword_argument"
                    && a.child_by_field_name("name")
                        .is_some_and(|n| text(n, src) == "verify")
                    && a.child_by_field_name("value")
                        .is_some_and(|n| n.kind() == "false")
            });
            if disabled {
                let confirmed = callee.split_once('.').is_some_and(|(module, method)| {
                    !method.contains('.') && bindings.get(module) == Some(&true)
                });
                add(
                    "tls-verification-disabled",
                    Severity::High,
                    confirmed,
                    "TLS certificate verification is explicitly disabled; restore verification and configure the trusted CA.",
                    text(call, src),
                );
            }
        }
        if extension == "rs"
            && matches!(
                short,
                "danger_accept_invalid_certs" | "danger_accept_invalid_hostnames"
            )
            && compact(arg_text) == "(true)"
        {
            add(
                "tls-verification-disabled",
                Severity::High,
                false,
                "A TLS verification bypass is enabled; resolve the receiver type and restore verification.",
                text(call, src),
            );
        }
        let shell = extension == "py"
            && (matches!(callee.as_str(), "os.system" | "os.popen")
                || (callee.starts_with("subprocess.")
                    && matches!(
                        short,
                        "run" | "call" | "Popen" | "check_output" | "check_call"
                    )
                    && (proof.shell
                        || nodes(args).iter().any(|a| {
                            a.kind() == "keyword_argument" && compact(text(*a, src)) == "shell=True"
                        }))));
        if shell && proof.untrusted {
            add(
                "shell-input",
                Severity::High,
                false,
                "Potential external input reaches a shell. Use an argument vector without a shell and validate the input.",
                &proof.text,
            );
        }
        patterns::calls(extension, call, src, &callee, &proof, &mut add);
        if matches!(short, "execute" | "executemany" | "query") {
            // Parameterized calls are not flagged merely for having input in
            // the separate parameter argument.
            if let Some(first) = args.named_child(0) {
                let sql = evidence(first, src, call, fields, &spans);
                if sql.dynamic {
                    add(
                        "sql-dynamic-text",
                        Severity::Medium,
                        false,
                        "SQL text appears dynamically constructed; verify parameter binding and allowlist any identifiers.",
                        &sql.text,
                    );
                }
            }
        }
        let log = matches!(
            short,
            "print"
                | "println"
                | "debug"
                | "info"
                | "warn"
                | "warning"
                | "error"
                | "exception"
                | "log"
        );
        let outbound = matches!(short, "post" | "put" | "patch" | "send" | "fetch" | "track");
        if proof.sensitive && (log || outbound) {
            add(
                "sensitive-sink",
                Severity::Medium,
                false,
                if proof.value_sensitive {
                    "A literal recognized as private by the local privacy boundary reaches logging or an outbound call. Its sensitivity can come from indexed private data outside this source context; a hard-coded literal is not necessarily public. Assess redaction, purpose and destination."
                } else {
                    "A potentially sensitive field reaches logging or an outbound call. Verify redaction, purpose and destination locally."
                },
                &proof.text,
            );
        }
    }
    out.candidates
        .extend(patterns::assignments(root, src, fields, &spans));
    if out.candidates.len() > MAX_CANDIDATES {
        out.candidates.truncate(MAX_CANDIDATES);
        out.incomplete = true;
    }
    if !out.candidates.is_empty() {
        let declarations = context::Declarations::new(root, src);
        for candidate in &mut out.candidates {
            out.incomplete |= declarations.enrich(&mut candidate.context);
        }
    }
    out
}

/// Multiset subtraction: moving a call or adding an unrelated line does not
/// re-report it; duplicating it or changing its traced inputs does.
pub fn introduced(before: Scan, after: Scan) -> Scan {
    let mut counts = BTreeMap::<String, usize>::new();
    for c in before.candidates {
        *counts
            .entry(format!("{}\0{}", c.confirmed, c.identity))
            .or_default() += 1;
    }
    let incomplete = before.incomplete || after.incomplete;
    let candidates = after
        .candidates
        .into_iter()
        .filter(|c| {
            let n = counts
                .entry(format!("{}\0{}", c.confirmed, c.identity))
                .or_default();
            if *n > 0 {
                *n -= 1;
                false
            } else {
                true
            }
        })
        .collect();
    Scan {
        candidates,
        incomplete,
    }
}
