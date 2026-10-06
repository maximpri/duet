// SPDX-License-Identifier: GPL-3.0-or-later
//! Binding and location regressions; synthetic source is parsed, never executed.
#[test]
fn comprehension_receiver_is_not_the_imported_requests_module() {
    for source in [
        "import requests\nvalues = [requests.get('cached', verify=False) for requests in local_stores]\n",
        "import requests as http\nvalues = (http.get('cached', verify=False) for _, http in local_stores)\n",
        "import requests\nvalues = {requests.get('cached', verify=False) for requests in local_stores}\n",
    ] {
        let scan = declass_review::scan("py", source, &[]);
        assert!(!scan.incomplete, "{scan:?}");
        assert!(
            scan.candidates
                .iter()
                .any(|c| c.rule == "tls-verification-disabled"),
            "{scan:?}"
        );
        assert!(!scan.candidates.iter().any(|c| c.confirmed), "{scan:?}");
    }
}

#[test]
fn matched_receiver_is_not_the_imported_requests_module() {
    for source in [
        "import requests\nmatch store:\n    case requests:\n        requests.get('cached', verify=False)\n",
        "import requests as http\nmatch store:\n    case {'client': http}:\n        http.get('cached', verify=False)\n",
        "import requests\nmatch store:\n    case Store() as requests:\n        requests.get('cached', verify=False)\n",
    ] {
        let scan = declass_review::scan("py", source, &[]);
        assert!(!scan.incomplete, "{scan:?}");
        assert!(
            scan.candidates
                .iter()
                .any(|c| c.rule == "tls-verification-disabled"),
            "{scan:?}"
        );
        assert!(!scan.candidates.iter().any(|c| c.confirmed), "{scan:?}");
    }
}

#[test]
fn requested_function_survives_context_truncation() {
    let mut source = "# harmless preamble that is unrelated to the requested helper\n".repeat(85);
    source.push_str("def target(value):\n    return value.strip()\n");
    let context = declass_review::context::at("py", &source, 87);
    assert!(
        context.contains("def target"),
        "target missing from {} bytes of context",
        context.len()
    );
}

#[test]
fn function_context_preserves_nested_scope_decorators_and_large_locations() {
    let nested = "def outer():\n    unrelated()\n    @decorate\n    def inner():\n        target()\n    other()\n";
    let context = declass_review::context::at("py", nested, 5);
    assert!(context.contains("@decorate\n    def inner"));
    assert!(context.contains("target()"));
    assert!(!context.contains("unrelated()"));
    assert!(!context.contains("other()"));
    let mut large = format!(
        "def large():\n{}",
        "    # éééééééééééééééééééé\n".repeat(150)
    );
    large.push_str("    requested_location()\n");
    large.push_str(&"    # øøøøøøøøøøøøøøøøøøøø\n".repeat(150));
    let context = declass_review::context::at("py", &large, 152);
    assert!(context.len() <= declass_review::MAX_CONTEXT / 3);
    assert!(context.contains("requested_location()"));
    let js = "function outer() {\n const helper = () => {\n  target();\n };\n other();\n}";
    let context = declass_review::context::at("js", js, 3);
    assert!(context.contains("target()"));
    assert!(!context.contains("other()"));
}
