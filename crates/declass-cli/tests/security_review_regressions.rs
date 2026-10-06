// SPDX-License-Identifier: GPL-3.0-or-later
//! Review enforcement, scanner coverage and session budgets through real loops.
//! Models are mocked and reviewed source is never executed.
mod privacy;
use privacy::{Fixture, Local, Options, Step};
use serde_json::{Value, json};

fn fixture(name: &str) -> Fixture {
    let mut f = Fixture::with(
        name,
        "Update client",
        Local::Cooperative,
        Options {
            no_local: true,
            ..Options::default()
        },
    );
    f.cfg.review.enabled = true;
    f.cfg.review.rules_only = true;
    f
}

fn scanner(script: &str) -> declass_agent::review::external::Scanner {
    declass_agent::review::external::Scanner {
        command: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        format: "bandit".into(),
        timeout_seconds: 5,
    }
}

fn report(f: &Fixture) -> Value {
    serde_json::from_slice(&std::fs::read(f.run_dir.join("security-review.json")).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn report_cap_does_not_disable_high_severity_blocking() {
    let mut f = fixture("review-report-cap-probe");
    f.cfg.review.block_high = true;
    let noise = format!("import random\n{}", "random.random()\n".repeat(64));
    f.script(vec![
        Step::Call("write_file", json!({"path":"a.py","content":noise})),
        Step::Call(
            "write_file",
            json!({"path":"z.py","content":"import requests\nrequests.get(url, verify=False)\n"}),
        ),
        Step::Call("finish", json!({"summary":"done"})),
    ]);
    let end = f.run().await;
    let r = report(&f);
    assert_eq!(
        r["blocked"],
        true,
        "{end:?}: findings={}, incomplete={}, blocked={}",
        r["findings"].as_array().unwrap().len(),
        r["incomplete"],
        r["blocked"]
    );
    assert!(matches!(end, declass_agent::Terminal::Failed { .. }));
    assert_eq!(r["findings"].as_array().unwrap().len(), 64);
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["confirmed"] == true && f["report"].as_str().unwrap().contains("z.py"))
    );
}

fn add_second(f: &mut Fixture) -> declass_boundary::testing::Received {
    use declass_boundary::{OutboundGate, audit::AuditLog, review::SecondReviewer};
    let reply = json!({"verdict":"likely","reason":"exposed_path","severity":"high","explanation":"TLS validation is disabled.","fix":"Enable verification."}).to_string();
    let (provider, received) =
        declass_boundary::testing::scripted_frontier(vec![reply], "glm-5.3-flash");
    let (filter, check) = f.engine.outbound();
    let gate = OutboundGate::new(AuditLog::open(&f.run_dir.join("second-audit.jsonl")).unwrap())
        .with_filter(filter)
        .with_check(check);
    let second = SecondReviewer::new(gate.wrap(provider), 1.0).unwrap();
    f.engine.set_review_second(&second).unwrap();
    f.cfg.review.second = Some(second);
    f.cfg.review.rules_only = false;
    received
}

#[tokio::test(flavor = "multi_thread")]
async fn later_turn_keeps_its_review_budget() {
    let mut f = fixture("review-later-turn-budget-probe");
    let received = add_second(&mut f);
    f.cfg.frontier_usd = 0.1;
    f.cfg.price = Box::new(|_| 0.06);
    f.script(vec![
        Step::Call("reply", json!({"message":"ready"})),
        Step::Call("finish", json!({"summary":"done"})),
    ]);
    let mut session = f.session();
    let first = session.turn("Review this workspace").await;
    std::fs::write(
        f.ws.join("client.py"),
        "import requests\nrequests.get(url, verify=False)\n",
    )
    .unwrap();
    let second = session.turn("Review the new client").await;
    assert_eq!(
        received.bodies().len(),
        1,
        "first={first:?}, second={second:?}, cost={}, report={}",
        session.stats().cost_usd,
        report(&f)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn review_obeys_the_remaining_session_budget() {
    let mut f = fixture("review-session-budget-probe");
    let received = add_second(&mut f);
    f.cfg.frontier_usd = 1.0;
    f.cfg.price = Box::new(|_| 0.03);
    f.script(vec![Step::Call("write_file", json!({"path":"client.py","content":"import requests\nrequests.get(url, verify=False)\n"})),
        Step::Call("finish", json!({"summary":"done"}))]);
    let limit = 0.060001;
    let mut session = declass_agent::Session::open(
        &f.cfg,
        &f.gated,
        f.engine.as_ref(),
        &f.git,
        f.interrupted.clone(),
        declass_agent::session::SessionLimits {
            frontier_usd: limit,
            working_time: std::time::Duration::from_secs(60),
        },
        false,
    )
    .unwrap();
    let end = session.turn("Update client").await;
    assert!(received.bodies().is_empty());
    assert!(
        session.stats().cost_usd <= limit,
        "{end:?}, cost={}, limit={limit}, review calls={}, report={}",
        session.stats().cost_usd,
        received.bodies().len(),
        report(&f)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn new_cross_file_finding_on_unchanged_sink_is_kept() {
    let mut f = fixture("review-cross-file-probe");
    std::fs::write(f.ws.join("source.py"), "value = 'safe'\n").unwrap();
    std::fs::write(f.ws.join("sink.py"), "def sink(value):\n    return value\n").unwrap();
    f.cfg.review.scanners = vec![scanner(
        r#"case "$(cat source.py)" in *unsafe*) printf '%s' '{"results":[{"filename":"sink.py","line_number":2,"test_id":"cross-file","issue_severity":"HIGH"}]}' ;; *) printf '%s' '{"results":[]}' ;; esac"#,
    )];
    f.script(vec![
        Step::Call(
            "write_file",
            json!({"path":"source.py","content":"value = 'unsafe'\n"}),
        ),
        Step::Call("finish", json!({"summary":"done"})),
    ]);
    let end = f.run().await;
    let r = report(&f);
    assert_eq!(r["external_unavailable"], 0, "{end:?}: {r}");
    assert_eq!(r["external_candidates"], 1, "{end:?}: {r}");
    assert_eq!(r["changed_files"], 1);
    assert!(r["reviewed_files"].get("sink.py").is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn scanner_failure_cannot_be_reported_as_complete() {
    for script in [
        r#"printf '%s' '{"results":[],"errors":[{"filename":"client.py","reason":"scanner analysis failed"}]}'; exit 2"#,
        r#"printf '%s' '{"results":[],"errors":[{"filename":"client.py","reason":"scanner analysis failed"}]}'; exit 0"#,
        r#"printf '%s' '{"results":[]}'; exit 2"#,
        r#"printf '%s' '{"results":[]}'; exit 1"#,
    ] {
        let mut f = fixture("review-scanner-error-probe");
        f.cfg.review.scanners = vec![scanner(script)];
        f.script(vec![
            Step::Call(
                "write_file",
                json!({"path":"client.py","content":"value = 1\n"}),
            ),
            Step::Call("finish", json!({"summary":"done"})),
        ]);
        let end = f.run().await;
        let r = report(&f);
        assert_eq!(r["external_unavailable"], 1, "{end:?}: {r}");
        assert_eq!(r["incomplete"], true, "{end:?}: {r}");
        assert!(f.run_dir.join("security-scanners/1-after.json").is_file());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn scanner_exit_one_with_validated_findings_is_accepted() {
    let mut f = fixture("review-scanner-findings-exit");
    f.cfg.review.scanners = vec![scanner(
        r#"if [ -f client.py ]; then printf '%s' '{"results":[{"filename":"client.py","line_number":1,"test_id":"synthetic","issue_severity":"HIGH"}]}'; exit 1; else printf '%s' '{"results":[]}'; fi"#,
    )];
    f.script(vec![
        Step::Call(
            "write_file",
            json!({"path":"client.py","content":"value = 1\n"}),
        ),
        Step::Call("finish", json!({"summary":"done"})),
    ]);
    let end = f.run().await;
    let r = report(&f);
    assert_eq!(r["external_unavailable"], 0, "{end:?}: {r}");
    assert_eq!(r["external_candidates"], 1, "{end:?}: {r}");
    assert_eq!(r["incomplete"], false, "{end:?}: {r}");
}
