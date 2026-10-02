// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded syntax context; references are evidence for an opinion, not proof.
use super::*;
use std::collections::BTreeSet;

pub fn prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn callable(n: Node<'_>) -> bool {
    matches!(
        n.kind(),
        "function_definition"
            | "function_item"
            | "function_declaration"
            | "method_definition"
            | "arrow_function"
    )
}

/// The function surrounding a location returned by a language server. No I/O.
pub fn at(ext: &str, source: &str, line: usize) -> String {
    let Some(tree) = patterns::tree(ext, source) else {
        return String::new();
    };
    let found = nodes(tree.root_node())
        .into_iter()
        .filter(|n| {
            callable(*n) && n.start_position().row < line && n.end_position().row + 1 >= line
        })
        .min_by_key(|n| n.end_byte() - n.start_byte());
    let location = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>();
    let (start, end) = match found {
        Some(n) => {
            // n is already the requested function, not a call inside it.
            let n = n
                .parent()
                .filter(|p| p.kind() == "decorated_definition")
                .unwrap_or(n);
            (n.start_byte(), n.end_byte())
        }
        None => {
            let start = source
                .split_inclusive('\n')
                .take(line.saturating_sub(4))
                .map(str::len)
                .sum::<usize>();
            let len = source[start..]
                .split_inclusive('\n')
                .take(8)
                .map(str::len)
                .sum::<usize>();
            (start, start + len)
        }
    };
    let snippet = &source[start..end];
    let cap = MAX_CONTEXT / 3;
    // Large functions still include the requested line instead of always
    // returning their first bytes. Keep the window inside the chosen scope.
    let mut offset = location
        .saturating_sub(start)
        .saturating_sub(cap / 2)
        .min(snippet.len().saturating_sub(cap));
    while !snippet.is_char_boundary(offset) {
        offset += 1;
    }
    prefix(&snippet[offset..], cap).to_owned()
}

/// Import declarations and definitions of called helpers in the same file.
/// Includes at most four helpers and never exceeds the shared byte cap.
pub fn enrich(ext: &str, source: &str, context: &mut String) -> bool {
    let Some(tree) = patterns::tree(ext, source) else {
        return false;
    };
    Declarations::new(tree.root_node(), source).enrich(context)
}

struct Declaration<'a> {
    snippet: &'a str,
    /// Imports are always included; named helpers must occur in the context.
    helper: Option<&'a str>,
}

/// Related declarations in source order. A scan reuses its existing tree and
/// builds this once, rather than parsing and walking the entire source again
/// for every finding. Only source slices are retained, never another tree.
pub(super) struct Declarations<'a>(Vec<Declaration<'a>>);

impl<'a> Declarations<'a> {
    pub(super) fn new(root: Node<'_>, source: &'a str) -> Self {
        Self(
            nodes(root)
                .into_iter()
                .filter_map(|n| {
                    let import = matches!(
                        n.kind(),
                        "import_statement" | "import_from_statement" | "use_declaration"
                    );
                    let helper = callable(n)
                        .then(|| n.child_by_field_name("name"))
                        .flatten()
                        .map(|name| text(name, source));
                    (import || helper.is_some()).then(|| Declaration {
                        snippet: text(n, source),
                        helper,
                    })
                })
                .collect(),
        )
    }

    pub(super) fn enrich(&self, context: &mut String) -> bool {
        let names: BTreeSet<_> = words(context).map(str::to_owned).collect();
        let mut helpers = 0;
        let mut capped = false;
        for declaration in &self.0 {
            let helper = declaration.helper.is_some();
            if declaration.helper.is_some_and(|name| !names.contains(name)) {
                continue;
            }
            let snippet = declaration.snippet;
            if context.contains(snippet) {
                continue;
            }
            if helper && helpers >= 4 {
                capped = true;
                continue;
            }
            if context.len() + snippet.len() + 30 > MAX_CONTEXT {
                capped = true;
                continue;
            }
            context.push_str("\n\nRelated declaration:\n");
            context.push_str(snippet);
            helpers += usize::from(helper);
        }
        capped
    }
}

/// Candidate helper/call sites inside the same lexical function (1-based,
/// character columns). The host may ask an installed language server about them.
pub fn references(ext: &str, source: &str, line: usize) -> Vec<(usize, usize)> {
    let Some(tree) = patterns::tree(ext, source) else {
        return vec![];
    };
    let all = nodes(tree.root_node());
    let containing = all
        .iter()
        .copied()
        .filter(|n| {
            callable(*n) && n.start_position().row < line && n.end_position().row + 1 >= line
        })
        .min_by_key(|n| n.end_byte() - n.start_byte())
        .unwrap_or(tree.root_node());
    nodes(containing)
        .into_iter()
        .filter(|n| matches!(n.kind(), "call" | "call_expression"))
        .filter_map(|n| n.child_by_field_name("function"))
        .take(4)
        .map(|n| {
            let pos = n.start_position();
            let column = source[..n.start_byte()]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .count()
                + 1;
            (pos.row + 1, column)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_and_route_context_stay_bounded() {
        let source = "import os\ndef clean(x):\n    return x.replace(';', '')\n\n@app.route('/run')\ndef handler():\n    value = clean(request.args['x'])\n    os.system(value)\n";
        let scan = scan("py", source, &[]);
        let c = scan
            .candidates
            .iter()
            .find(|c| c.rule == "shell-input")
            .unwrap();
        assert!(c.context.contains("@app.route"));
        assert!(c.context.contains("def clean"));
        assert!(c.context.contains("import os"));
        assert!(c.context.len() <= MAX_CONTEXT);
        assert_eq!(prefix("ééé", 5), "éé");
    }

    #[test]
    fn related_declarations_keep_source_order_and_helper_limit() {
        let source = "import os\ndef first(): return 'é'\ndef second(): return 'ø'\ndef third(): return 'λ'\ndef fourth(): return 'δ'\ndef fifth(): return 'σ'\nimport sys\n";
        let mut context = "first() second() third() fourth() fifth()".to_owned();
        assert!(enrich("py", source, &mut context));
        assert!(context.contains("def fourth(): return 'δ'"));
        assert!(!context.contains("def fifth():"));
        assert!(context.ends_with("import sys"));
        assert!(context.find("import os").unwrap() < context.find("def first").unwrap());
    }

    #[test]
    fn finding_contexts_include_only_their_own_helpers() {
        let source = "import os\ndef alpha(x): return x.strip()\ndef beta(x): return x.upper()\ndef handler_a():\n    os.system(alpha(request.args['input']))\ndef handler_b():\n    os.system(beta(request.args['input']))\n";
        let scan = scan("py", source, &[]);
        assert!(!scan.incomplete);
        assert_eq!(scan.candidates.len(), 2);
        let first = &scan.candidates[0].context;
        let second = &scan.candidates[1].context;
        assert!(first.contains("import os"));
        assert!(first.contains("def alpha"));
        assert!(!first.contains("def beta"));
        assert!(second.contains("import os"));
        assert!(second.contains("def beta"));
        assert!(!second.contains("def alpha"));
    }
}
