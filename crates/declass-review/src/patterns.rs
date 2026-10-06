// SPDX-License-Identifier: GPL-3.0-or-later
//! Original advisory patterns. Names and syntax nominate a review; no runtime
//! type, reachable-path, sanitizer or authorization proof is claimed.
use super::*;
use std::collections::BTreeSet;

pub(super) fn untrusted(n: Node<'_>, src: &str) -> bool {
    let s = text(n, src);
    (matches!(
        n.kind(),
        "attribute" | "member_expression" | "field_expression"
    ) && ["request.", "req.", "sys.argv", "os.environ", "location."]
        .iter()
        .any(|p| s.starts_with(p)))
        || (matches!(n.kind(), "call" | "call_expression")
            && n.child_by_field_name("function").is_some_and(|f| {
                let f = text(f, src);
                matches!(
                    f,
                    "input"
                        | "getenv"
                        | "std::env::args"
                        | "std::env::var"
                        | "env::args"
                        | "env::var"
                ) || [
                    ".read",
                    ".read_text",
                    ".read_to_string",
                    ".read_line",
                    ".getenv",
                ]
                .iter()
                .any(|x| f.ends_with(x))
            }))
}

type Add<'a> = dyn FnMut(&'static str, Severity, bool, &'static str, &str) + 'a;

pub(super) fn calls(
    ext: &str,
    call: Node<'_>,
    src: &str,
    callee: &str,
    proof: &Evidence,
    add: &mut Add<'_>,
) {
    let short = callee.rsplit(['.', ':']).next().unwrap_or(callee);
    let raw = text(call, src);
    if proof.untrusted {
        if ext != "py" && matches!(short, "exec" | "execSync") {
            add(
                "shell-input",
                Severity::High,
                false,
                "Potential external input reaches a command interpreter. Use a fixed executable and an argument vector.",
                &proof.text,
            );
        }
        if matches!(
            short,
            "open"
                | "readFile"
                | "readFileSync"
                | "writeFile"
                | "writeFileSync"
                | "send_file"
                | "sendFile"
        ) {
            add(
                "path-input",
                Severity::High,
                false,
                "External input may choose a filesystem path. Resolve it under an allowed root and check the resolved path, including symlinks.",
                &proof.text,
            );
        }
        if matches!(
            short,
            "render_template_string" | "write" | "insertAdjacentHTML" | "html"
        ) {
            add(
                "html-input",
                Severity::High,
                false,
                "External input may reach an HTML or template sink. Verify contextual escaping and avoid interpreting input as template source.",
                &proof.text,
            );
        }
        if matches!(short, "eval" | "exec" | "compile" | "Function") {
            add(
                "code-input",
                Severity::High,
                false,
                "External input may be interpreted as code. Replace evaluation with a data parser or a fixed operation allowlist.",
                &proof.text,
            );
        }
        if [
            "pickle.load",
            "pickle.loads",
            "dill.load",
            "dill.loads",
            "yaml.load",
            "yaml.unsafe_load",
        ]
        .contains(&callee)
            && !raw.contains("SafeLoader")
        {
            add(
                "unsafe-deserialization",
                Severity::High,
                false,
                "Untrusted input reaches a deserializer capable of constructing executable objects. Use a data-only format and safe loader.",
                &proof.text,
            );
        }
        if matches!(short, "xpath" | "evaluate" | "search_s") && proof.dynamic {
            add(
                "query-input",
                Severity::Medium,
                false,
                "External input is interpolated into a structured query. Use variable binding or the query language's contextual escaping.",
                &proof.text,
            );
        }
        if matches!(short, "redirect" | "sendRedirect") {
            add(
                "redirect-input",
                Severity::Medium,
                false,
                "External input chooses a redirect destination. Validate the parsed scheme and destination against an allowlist.",
                &proof.text,
            );
        }
    }
    if matches!(short, "md5" | "sha1" | "MD5" | "SHA1")
        || ["Md5::digest", "Sha1::digest", "DES.new"].contains(&callee)
        || (short == "createHash"
            && ["'md5'", "\"md5\"", "'sha1'", "\"sha1\""]
                .iter()
                .any(|s| raw.contains(s)))
    {
        add(
            "weak-crypto",
            Severity::Medium,
            false,
            "A legacy cryptographic primitive is used. Determine whether this protects security-sensitive data; use a modern primitive and a password KDF where appropriate.",
            raw,
        );
    }
    if matches!(short, "random" | "randint" | "randrange" | "choice") {
        add(
            "predictable-random",
            Severity::Medium,
            false,
            "A non-cryptographic random generator is used. If this produces credentials, tokens or security decisions, use an operating-system cryptographic generator.",
            raw,
        );
    }
    if matches!(short, "parse" | "fromstring")
        && proof.untrusted
        && (callee.contains("etree") || callee.contains("xml"))
    {
        add(
            "xml-input",
            Severity::Medium,
            false,
            "Untrusted XML reaches a parser. Verify that external entities, network access and resource expansion are disabled.",
            &proof.text,
        );
    }
    if outbound(short) {
        for host in call_hosts(call, src) {
            add(
                "outbound-host",
                Severity::Medium,
                false,
                "An outbound destination is present. For a change review, this host was not observed in the baseline's recognized outbound calls; verify purpose and data minimization.",
                &host,
            );
        }
    }
}

fn outbound(name: &str) -> bool {
    matches!(
        name,
        "get" | "post" | "put" | "patch" | "send" | "fetch" | "request" | "track"
    )
}
fn call_hosts(call: Node<'_>, src: &str) -> BTreeSet<String> {
    nodes(call)
        .into_iter()
        .filter(|n| matches!(n.kind(), "string" | "string_literal"))
        .filter_map(|n| {
            let s = text(n, src).trim_matches(['\'', '"']);
            let rest = s
                .strip_prefix("https://")
                .or_else(|| s.strip_prefix("http://"))?;
            let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
            (!host.is_empty() && !host.contains(['{', '$', '\\', ' ']))
                .then(|| host.to_ascii_lowercase())
        })
        .collect()
}

pub(super) fn tree(ext: &str, src: &str) -> Option<tree_sitter::Tree> {
    let grammar = match ext {
        "py" => tree_sitter_python::LANGUAGE.into(),
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "tsx" | "jsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "ts" | "js" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        _ => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    parser.parse(src, None)
}

/// Recognized static outbound hosts, for repository-wide baseline comparison.
pub fn hosts(ext: &str, src: &str) -> BTreeSet<String> {
    let Some(tree) = tree(ext, src) else {
        return BTreeSet::new();
    };
    nodes(tree.root_node())
        .into_iter()
        .filter(|n| matches!(n.kind(), "call" | "call_expression"))
        .filter(|n| {
            n.child_by_field_name("function")
                .is_some_and(|f| outbound(text(f, src).rsplit(['.', ':']).next().unwrap_or("")))
        })
        .flat_map(|n| call_hosts(n, src))
        .collect()
}

pub(super) fn assignments(
    root: Node<'_>,
    src: &str,
    fields: &[String],
    spans: &[(usize, usize)],
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for n in nodes(root) {
        if !matches!(
            n.kind(),
            "assignment" | "assignment_expression" | "let_declaration" | "variable_declarator"
        ) {
            continue;
        }
        let (Some(left), Some(right)) = (binding(n), value(n)) else {
            continue;
        };
        let name = text(left, src);
        let v = text(right, src).trim_matches(['\'', '"']);
        let literal = matches!(
            right.kind(),
            "string" | "string_literal" | "raw_string_literal"
        );
        let credential = sensitive_name(name, &[])
            && literal
            && v.len() >= 8
            && !["redacted", "withheld", "changeme", "example", "placeholder"]
                .iter()
                .any(|s| v.to_ascii_lowercase().contains(s));
        let p = evidence(right, src, n, fields, spans);
        let html = name.ends_with(".innerHTML") || name.ends_with(".outerHTML");
        if credential || (html && p.untrusted) {
            let (rule, message) = if credential {
                (
                    "credential-literal",
                    "A credential-named binding contains a literal. Verify that it is a fixture; otherwise remove it, rotate the exposed credential and load it from an approved secret store.",
                )
            } else {
                (
                    "html-input",
                    "External input may reach an HTML property. Use textContent or a context-appropriate sanitizer.",
                )
            };
            out.push(Candidate {
                rule,
                line: n.start_position().row + 1,
                severity: if html {
                    Severity::High
                } else {
                    Severity::Medium
                },
                confirmed: false,
                known_private_value: p.value_sensitive,
                message,
                identity: format!("{rule}\0{}", text(n, src)),
                context: context(n, src),
            });
            if out.len() == MAX_CANDIDATES {
                break;
            }
        }
    }
    out
}

fn guards(ext: &str, src: &str) -> BTreeMap<String, usize> {
    let Some(tree) = tree(ext, src) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for n in nodes(tree.root_node()) {
        let f = if matches!(n.kind(), "call" | "call_expression") {
            n.child_by_field_name("function")
        } else if n.kind() == "decorator" {
            n.named_child(0).filter(|n| n.kind() == "identifier")
        } else {
            None
        };
        let Some(f) = f else { continue };
        if matches!(
            text(f, src).rsplit(['.', ':']).next(),
            Some(
                "authenticate"
                    | "authorize"
                    | "require_auth"
                    | "requires_auth"
                    | "login_required"
                    | "check_permission"
            )
        ) {
            *out.entry(compact(text(n, src))).or_default() += 1;
        }
    }
    out
}

/// Removal of recognized authorization calls/decorators is an advisory change
/// finding, including file deletion. It cannot prove an auth bypass.
pub fn removed_guards(ext: &str, before: &str, after: &str) -> Vec<Candidate> {
    let mut now = guards(ext, after);
    guards(ext,before).into_iter().filter_map(|(guard,n)| {
        (n > now.remove(&guard).unwrap_or(0)).then(|| Candidate {
            rule:"auth-guard-removed", line:1, severity:Severity::High, confirmed:false, known_private_value:false,
            message:"A recognized authorization check was removed or changed. Verify that every affected entry point still enforces authorization.",
            identity:format!("auth-guard-removed\0{guard}"),
            context: format!("Before:\n{}\nAfter:\n{}", context::prefix(before, MAX_CONTEXT/2-16), context::prefix(after, MAX_CONTEXT/2-16)),
        })
    }).take(MAX_CANDIDATES).collect()
}
