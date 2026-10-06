// SPDX-License-Identifier: GPL-3.0-or-later
use declass_review::{introduced, scan};

#[test]
fn broader_patterns_are_advisory_and_guard_removal_is_a_change() {
    for (rule, source) in [
        ("path-input", "open(request.args['path'])"),
        ("html-input", "render_template_string(request.args['html'])"),
        ("code-input", "eval(input())"),
        ("unsafe-deserialization", "pickle.loads(request.data)"),
        ("weak-crypto", "hashlib.md5(b'data')"),
        ("credential-literal", "api_key = 'a-credential-value'"),
        (
            "outbound-host",
            "requests.post('https://analytics.example.org/events')",
        ),
    ] {
        let s = scan("py", source, &[]);
        assert!(s.candidates.iter().any(|c| c.rule == rule), "{rule}: {s:?}");
        assert!(s.candidates.iter().all(|c| !c.confirmed));
    }
    assert_eq!(
        declass_review::hosts(
            "py",
            "requests.post('https://analytics.example.org/events')"
        ),
        ["analytics.example.org".into()].into()
    );
    let before = "@login_required\ndef handler():\n    return 'ok'\n";
    let removed = declass_review::removed_guards("py", before, "def handler():\n    return 'ok'\n");
    assert_eq!(removed.len(), 1);
    assert!(!removed[0].confirmed);
    assert!(declass_review::removed_guards("py", before, before).is_empty());
}

#[test]
fn tls_binding_literal_and_comment_controls() {
    for src in [
        "import requests\nrequests.get(url, verify=False)",
        "import requests as http\nhttp.get(url, verify = False)",
    ] {
        let s = scan("py", src, &[]);
        assert!(!s.incomplete);
        assert_eq!(s.candidates.len(), 1, "{src}");
        assert!(s.candidates[0].confirmed, "{src}");
    }
    for src in [
        "import requests\n# requests.get(url, verify=False)\nx = 'requests.get(url, verify=False)'",
        "import requests\nrequests.get(url, verify=True)",
        "import requests\nrequests.get(url, verify=verify)",
    ] {
        assert!(scan("py", src, &[]).candidates.is_empty(), "{src}");
    }
    for src in [
        "requests.get(url, verify=False)",
        "import requests\nrequests = client\nrequests.get(url, verify=False)",
        "import requests\ndef f(requests):\n    requests.get(url, verify=False)",
        "import requests\nfrom mocks import requests\nrequests.get(url, verify=False)",
        "import requests\nfor requests in clients:\n    requests.get(url, verify=False)",
        "import requests\nrequests.get(url, verify=False)\n!invalid",
    ] {
        assert!(
            scan("py", src, &[]).candidates.iter().all(|c| !c.confirmed),
            "{src}"
        );
    }
}

#[test]
fn shell_and_sql_trace_assignments_but_remain_advisory() {
    let src = "import os\ndef run():\n    cmd = input()\n    alias = cmd\n    os.system(alias)\n    sql = f'SELECT * FROM t WHERE id={cmd}'\n    cursor.execute(sql)\n";
    let s = scan("py", src, &[]);
    assert_eq!(
        s.candidates.iter().map(|c| c.rule).collect::<Vec<_>>(),
        ["shell-input", "sql-dynamic-text"]
    );
    assert!(s.candidates.iter().all(|c| !c.confirmed));
    let safe = "import subprocess\nsubprocess.run(['echo', input()], shell=False)\ncursor.execute('SELECT * FROM t WHERE id=?', (input(),))";
    assert!(scan("py", safe, &[]).candidates.is_empty());
}

#[test]
fn schema_fields_reach_sinks_in_supported_languages() {
    for (ext, src) in [
        (
            "py",
            "def export(row):\n    value = row['birth_date']\n    logger.info(value)",
        ),
        (
            "rs",
            "fn export(row: Row) { let value = row.birth_date; tracing::info!(value); }",
        ),
        (
            "ts",
            "function exportRow(row: Row) { const value = row.birth_date; console.log(value); }",
        ),
    ] {
        let s = scan(ext, src, &["birth_date".into()]);
        assert!(!s.incomplete, "{ext}");
        assert_eq!(s.candidates.len(), 1, "{ext}: {s:?}");
        assert_eq!(s.candidates[0].rule, "sensitive-sink");
        assert!(!s.candidates[0].confirmed);
    }
}

#[test]
fn before_after_counts_duplicates_and_changed_sources() {
    let original = "import requests\nrequests.get(url, verify=False)\n";
    let same = format!("# different line numbers\n{original}");
    assert!(
        introduced(scan("py", original, &[]), scan("py", &same, &[]))
            .candidates
            .is_empty()
    );
    let duplicate = format!("{same}requests.get(url, verify=False)\n");
    assert_eq!(
        introduced(scan("py", original, &[]), scan("py", &duplicate, &[]))
            .candidates
            .len(),
        1
    );
    let before = "def f():\n    cmd = 'echo ok'\n    os.system(cmd)";
    let after = before.replace("'echo ok'", "input()");
    assert_eq!(
        introduced(scan("py", before, &[]), scan("py", &after, &[]))
            .candidates
            .len(),
        1
    );
}

#[test]
fn unsupported_syntax_and_caps_never_claim_complete() {
    assert!(scan("java", "class A {}", &[]).incomplete);
    assert!(scan("py", "def invalid !!!", &[]).incomplete);
    let huge = format!(
        "import requests\n{}",
        "requests.get(url, verify=False)\n".repeat(80)
    );
    let s = scan("py", &huge, &[]);
    assert!(s.incomplete);
    assert_eq!(s.candidates.len(), declass_review::MAX_CANDIDATES);
}

#[test]
fn formatted_but_static_sql_and_explicit_shell_vectors() {
    let safe_sql =
        "sql = f'SELECT name FROM users WHERE password = ?'\ncur.execute(sql, (input(),))";
    assert!(scan("py", safe_sql, &[]).candidates.is_empty());
    let src = "import subprocess\ndef f(request):\n    value = request.form.get('action')\n    if not value:\n        value = ''\n    args = []\n    args.append('/bin/sh')\n    args.append('-c')\n    args.append(value)\n    subprocess.run(args)";
    let s = scan("py", src, &[]);
    assert_eq!(s.candidates.len(), 1);
    assert_eq!(s.candidates[0].rule, "shell-input");
    assert!(!s.candidates[0].confirmed);
}

#[test]
fn mutations_and_loop_bindings_trace_input_without_crossing_functions() {
    let src = "def handler(request):\n    for name in request.form.keys():\n        param = name\n    table = {}\n    table['value'] = param\n    command = 'echo '\n    command += table['value']\n    subprocess.run(command, shell=True)\n";
    let s = scan("py", src, &[]);
    assert!(!s.incomplete);
    assert_eq!(s.candidates.len(), 1);
    assert_eq!(s.candidates[0].rule, "shell-input");
    assert!(!s.candidates[0].confirmed);
    let safe = "def other():\n    value = input()\ndef handler():\n    value = 'echo ok'\n    os.system(value)\n";
    assert!(scan("py", safe, &[]).candidates.is_empty());
    // Writing a container does not taint the expression used as its key.
    let safe = "def handler():\n    key = 'echo ok'\n    table = {}\n    table[key] = input()\n    os.system(key)\n";
    assert!(scan("py", safe, &[]).candidates.is_empty());
}

#[test]
fn value_spans_match_literals_not_comments_and_validate_byte_ranges() {
    let src = "def handler():\n    value = 'neutral-é-canary'\n    logger.info(value)\n";
    let start = src.find("neutral").unwrap();
    assert!(scan("py", src, &[]).candidates.is_empty());
    let s = declass_review::scan_with_values(
        "py",
        src,
        &[],
        &[(start, start + "neutral-é-canary".len())],
    );
    assert_eq!(s.candidates.len(), 1);
    assert_eq!(s.candidates[0].rule, "sensitive-sink");
    assert!(s.candidates[0].message.contains("indexed private data"));
    let inside_utf8 = src.find('é').unwrap() + 1;
    assert!(
        declass_review::scan_with_values(
            "py",
            src,
            &[],
            &[
                (usize::MAX, 0),
                (0, usize::MAX),
                (inside_utf8, inside_utf8 + 1)
            ]
        )
        .candidates
        .is_empty()
    );
    let src = "logger.info('ok') # neutral-canary";
    let start = src.find("neutral").unwrap();
    assert!(
        declass_review::scan_with_values("py", src, &[], &[(start, src.len())])
            .candidates
            .is_empty()
    );
}
