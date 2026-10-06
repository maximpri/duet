// SPDX-License-Identifier: GPL-3.0-or-later
//! Reviewer output is subject to the same privacy boundary as local answers.
use declass_boundary::engine::Engine;
use declass_boundary::policy::{Policy, StructureSettings};
use declass_boundary::testing::scripted_local;
use declass_boundary::view::{Explored, Presenter, Source};
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn frontier_opinions_are_fresh_gated_budgeted_and_never_see_private_sources() {
    use declass_boundary::{OutboundGate, audit::AuditLog, review::SecondReviewer};
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        "birth_date,card\n1994-11-07,5293761582049377\n",
    )
    .unwrap();
    let e = Engine::open(
        &d.path().join("run"),
        Policy {
            sealed: vec!["private.py".into()],
            sensitive_globs: vec!["data/**".into()],
            structure: StructureSettings {
                views: true,
                ..Default::default()
            },
            ..Policy::default()
        },
        None,
    )
    .unwrap();
    e.prime(
        &ws,
        &["data/customers.csv".into()],
        "WORKING_CONVERSATION_MUST_NOT_BE_SENT",
    );
    let reply = serde_json::json!({"verdict":"likely","reason":"exposed_path","severity":"high","explanation":"TLS validation is disabled.","fix":"Enable verification."}).to_string();
    let (provider, received) = declass_boundary::testing::scripted_frontier(
        vec![reply, "bad JSON".into()],
        "glm-5.3-flash",
    );
    let (filter, check) = e.outbound();
    let gate = OutboundGate::new(AuditLog::open(&d.path().join("audit.jsonl")).unwrap())
        .with_filter(filter)
        .with_check(check);
    let second = SecondReviewer::new(gate.wrap(provider), 0.1).unwrap();
    e.set_review_second(&second).unwrap();
    let source = "import requests\nrequests.get(url, verify=False)\n";
    let c = declass_review::scan("py", source, &[]).candidates.remove(0);
    let diff = serde_json::json!({"before":"","after":source}).to_string();
    for path in ["private.py", "data/script.py"] {
        assert!(e.review_second(&c, &[path.into()], &diff, 1.0).is_none());
    }
    for private in [
        "logger.info(row['birth_date'])",
        "logger.info('1994-11-07')",
    ] {
        let diff = serde_json::json!({"before":private,"after":source}).to_string();
        assert!(
            e.review_second(&c, &["open.py".into()], &diff, 1.0)
                .is_none()
        );
    }
    assert!(received.bodies().is_empty());
    assert!(
        e.review_second(&c, &["open.py".into()], &diff, 0.0)
            .unwrap()
            .is_err()
    );
    assert!(received.bodies().is_empty());
    assert!(
        e.review_second(&c, &["open.py".into()], &diff, 1.0)
            .unwrap()
            .is_ok()
    );
    let bodies = received.bodies();
    assert_eq!(bodies.len(), 1);
    let body = bodies[0].to_string();
    assert!(!body.contains("WORKING_CONVERSATION_MUST_NOT_BE_SENT"));
    assert!(!body.contains("1994-11-07"));
    assert!(
        bodies[0]
            .get("tools")
            .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(bodies[0]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(bodies[0]["reasoning_effort"], "low");
    assert_eq!(bodies[0]["response_format"]["type"], "json_object");
    assert_eq!(
        bodies[0]["max_tokens"],
        declass_boundary::review::MAX_OUTPUT_TOKENS
    );
    assert!(
        e.review_second(&c, &["open.py".into()], &diff, 1.0)
            .unwrap()
            .is_err()
    );
    let stats = e.take_review_usage();
    assert_eq!(stats.calls, 2);
    assert!(stats.cost_usd > 0.0);
    assert_eq!(stats.usage.input, 200);
    assert_eq!(second.spent_usd(), stats.cost_usd);
    assert!(e.take_review_usage().is_empty());
    second.restore_spent(0.1).await;
    second.restore_spent(0.01).await;
    assert_eq!(second.spent_usd(), 0.1);
    assert!(second.take_stats().is_empty());
    assert!(
        second
            .review(&c, &diff, 1.0)
            .await
            .unwrap_err()
            .contains("budget")
    );
    assert_eq!(received.bodies().len(), 2);
}

#[test]
fn contradictory_judgments_and_private_literal_dismissals_become_uncertain() {
    let mut c = declass_review::scan(
        "py",
        "import requests\nrequests.get(url, verify=False)",
        &[],
    )
    .candidates
    .remove(0);
    for (private, verdict, reason) in [
        (false, "likely", "blocked_path"),
        (true, "unlikely", "not_sensitive"),
    ] {
        c.known_private_value = private;
        let j = declass_boundary::review::judgment(serde_json::json!({"verdict":verdict,"reason":reason,"severity":"high","explanation":"test","fix":"test"}), &c).unwrap();
        assert_eq!(j.verdict, declass_review::Verdict::Uncertain);
    }
}

#[test]
fn protected_source_tokens_do_not_hide_known_private_values() {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(
        ws.join("data/customer.csv"),
        "birth_date,card\n1994-11-07,5293761582049377\n",
    )
    .unwrap();
    std::fs::write(ws.join(".env"), "SERVICE_KEY=K8h3m9q2s7v4r6t1\n").unwrap();
    let source = "def hidden_transform_7ca91f():\n    value = '1994-11-07'\n    other = 'prefix-K8h3m9q2s7v4r6t1-suffix'\n";
    std::fs::write(ws.join("private.py"), source).unwrap();
    let e = Engine::open(
        d.path(),
        Policy {
            sensitive_globs: vec!["data/**".into(), ".env".into()],
            sealed: vec!["private.py".into()],
            structure: StructureSettings {
                views: true,
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .unwrap();
    e.prime(
        &ws,
        &[
            "data/customer.csv".into(),
            ".env".into(),
            "private.py".into(),
        ],
        "review changes",
    );
    for value in ["1994-11-07", "K8h3m9q2s7v4r6t1"] {
        let start = source.find(value).unwrap();
        assert!(
            e.review_value_spans(source)
                .iter()
                .any(|(s, e)| *s <= start && *e >= start + value.len()),
            "known value hidden by a code token: {value}"
        );
    }
    assert!(e.review_value_spans("hidden_transform_7ca91f").is_empty());
}

#[test]
fn privacy_sources_include_schema_known_values_and_detectors_not_literal_labels() {
    let d = tempfile::tempdir().unwrap();
    let e = Engine::open(
        d.path(),
        Policy {
            sensitive_globs: vec!["data/**".into()],
            detect_pii: true,
            detect_secrets: true,
            structure: StructureSettings {
                views: true,
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let ws = d.path().join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(
        ws.join("data/customer.csv"),
        "birth_date,card\n1994-11-07,5293761582049377\n",
    )
    .unwrap();
    e.prime(&ws, &["data/customer.csv".into()], "review changes");
    let fields = e.review_fields();
    assert!(fields.contains(&"birth_date".into()));
    assert!(!e.review_value_spans("'1994-11-07'").is_empty());
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/review_privacy.json")).unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let src = case["source"].as_str().unwrap();
        let id = case["id"].as_str().unwrap();
        let spans = e.review_value_spans(src);
        let scan =
            declass_review::scan_with_values(case["ext"].as_str().unwrap(), src, &fields, &spans);
        let candidate = scan.candidates.iter().any(|c| c.rule == "sensitive-sink");
        // The bounded rules cannot prove these sanitizers/overwrites. They
        // nominate them for local review; local opinions never create blockers.
        let advisory_control = matches!(
            id,
            "py_redactor" | "ts_redactor" | "rs_redactor" | "py_overwrite"
        );
        if scan.incomplete || candidate != (case["expected"].as_bool().unwrap() || advisory_control)
        {
            failures.push(format!("{id}: {scan:?}, spans={spans:?}"));
        }
        assert!(scan.candidates.iter().all(|c| !c.confirmed));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_free_local_review_filters_private_prose_and_protected_source() {
    let d = tempfile::tempdir().unwrap();
    let code = "import requests\ndef secret_export(row):\n    requests.post(url, json=row['birth_date'], verify=False)\n";
    let reply = serde_json::json!({"verdict":"unlikely", "reason":"blocked_path", "severity":"high", "explanation":
        format!("Customer born 1994-11-07, card 5293761582049377. {code}"), "fix":"Keep verification enabled."});
    let (reader, received) = scripted_local(vec![reply.to_string()]);
    let policy = Policy {
        sealed: vec!["private.py".into()],
        sensitive_globs: vec!["data/**".into()],
        detect_pii: true,
        detect_secrets: true,
        structure: StructureSettings {
            views: true,
            ..Default::default()
        },
        ..Policy::default()
    };
    let e = Engine::open(d.path(), policy, Some(reader)).unwrap();
    let ws = d.path().join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(
        ws.join("data/customer.csv"),
        "birth_date,card\n1994-11-07,5293761582049377\n",
    )
    .unwrap();
    e.prime(&ws, &["data/customer.csv".into()], "review changes");
    assert!(e.review_fields().contains(&"birth_date".into()));
    let c = declass_review::scan("py", code, &e.review_fields())
        .candidates
        .remove(0);
    let j = e.review_candidate(&c).unwrap().unwrap();
    let raw = format!("{} {}", j.explanation, j.fix);
    let shown = e.present(
        &Source::Explore {
            question: "Review security and suggest a safe fix".into(),
            read: Explored(Arc::new(vec![(
                Some(PathBuf::from("private.py")),
                code.into(),
            )])),
        },
        raw.as_bytes(),
    );
    for secret in [
        "1994-11-07",
        "5293761582049377",
        "secret_export",
        "json=row",
    ] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(received.prompt(0).contains("secret_export"));
    assert_eq!(received.bodies().len(), 1);
    assert!(
        received.bodies()[0]
            .get("tools")
            .is_none_or(|t| t.as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(e.take_local_stats().unwrap().calls, 1);
}

#[tokio::test]
async fn malformed_or_schema_invalid_opinions_are_not_reports() {
    let candidate = declass_review::scan(
        "py",
        "import requests\nrequests.get(url, verify=False)",
        &[],
    )
    .candidates
    .remove(0);
    for replies in [
        vec!["PRIVATE_INVALID_REPLY".into(); 2],
        vec![r#"{"verdict":"safe","explanation":"PRIVATE_INVALID_REPLY","fix":"x"}"#.into()],
    ] {
        let (reader, _) = scripted_local(replies);
        let err = reader.review_candidate(&candidate).await.unwrap_err();
        assert!(!err.contains("PRIVATE_INVALID_REPLY"));
    }
}

#[tokio::test]
async fn a_frontier_role_is_refused_before_any_transport_call() {
    struct NeverSend;
    impl declass_provider::client::Transport for NeverSend {
        fn post(
            &self,
            _: String,
            _: Vec<(String, String)>,
            _: Vec<u8>,
        ) -> futures_util::future::BoxFuture<
            'static,
            Result<declass_provider::client::HttpReply, declass_provider::ProviderError>,
        > {
            panic!("a security candidate must never be sent through a frontier provider");
        }
    }
    let config = declass_provider::ProviderConfig::new(
        "https://frontier.example/v1",
        "test",
        declass_provider::Role::Frontier,
    );
    let reader = declass_boundary::local::LocalReader::new(
        declass_provider::ChatProvider::new(config, Box::new(NeverSend)).unwrap(),
    );
    let c = declass_review::scan(
        "py",
        "import requests\nrequests.get(url, verify=False)",
        &[],
    )
    .candidates
    .remove(0);
    assert!(
        reader
            .review_candidate(&c)
            .await
            .unwrap_err()
            .contains("local provider")
    );
}
