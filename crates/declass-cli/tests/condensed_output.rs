// SPDX-License-Identifier: GPL-3.0-or-later
//! Command output through the frontier loop with a scripted frontier that
//! may answer with several tool calls at once: the calls of one response
//! run one after another in the order given, a test run's output reaches
//! the frontier condensed in hybrid mode (the whole output a `read_raw`
//! away, planted values replaced in both) and whole in pass-through mode.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use bytes::Bytes;
use declass_agent::{RunConfig, Terminal};
use declass_boundary::OutboundGate;
use declass_boundary::audit::AuditLog;
use declass_boundary::engine::Engine;
use declass_boundary::policy::Policy;
use declass_boundary::testing::canary::{Canaries, Options};
use declass_boundary::view::{PassThrough, Presenter};
use declass_provider::client::{HttpReply, Transport};
use declass_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// A customer in `data/customers.csv`, copied into a public test fixture.
const EMAIL: &str = "orla.brennvik@fjordmail-post.org";
const CARD: &str = "5293761582049377";

/// One scripted response: its tool calls (name, arguments), in order.
type Response = Vec<(&'static str, Value)>;

/// Replies to each request with the next response of its script (one or
/// more tool calls) and records every body.
#[derive(Clone, Default)]
struct Frontier {
    responses: Arc<Mutex<VecDeque<Response>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let n = {
            let mut bodies = self.bodies.lock().unwrap();
            bodies.push(serde_json::from_slice(&body).unwrap());
            bodies.len()
        };
        let calls = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![("finish", json!({"summary": "(script ran out)"}))]);
        let calls: Vec<Value> = calls
            .into_iter()
            .enumerate()
            .map(|(i, (name, args))| {
                json!({"index": i, "id": format!("c{n}-{i}"), "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}})
            })
            .collect();
        let delta = json!({"choices": [{"delta": {"tool_calls": calls}}]});
        let chunks = [
            format!("data: {delta}\n\n"),
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n".into(),
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":20}}\n\n"
                .into(),
            "data: [DONE]\n\n".into(),
        ];
        Box::pin(async move {
            Ok(HttpReply {
                status: 200,
                headers: vec![],
                body: futures_util::stream::iter(chunks.map(|c| Ok(Bytes::from(c)))).boxed(),
            })
        })
    }
}

/// The tool results a request carries, in order.
fn results(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// A workspace whose public test script prints a cargo-test-like run: many
/// passing tests and one failure whose assertion prints a customer's email
/// and card, copied into the test from the sensitive data.
fn workspace(dir: &Path) {
    let ws = dir.join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::create_dir_all(ws.join("tests")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,name,email,card\n7,Orla Brennvik,{EMAIL},{CARD}\n"),
    )
    .unwrap();
    let mut script = String::from("echo\necho 'running 41 tests'\n");
    for i in 0..40 {
        script.push_str(&format!("echo 'test refunds::case_{i:02} ... ok'\n"));
    }
    script.push_str(&format!(
        "echo 'test refunds::refund_to_card ... FAILED'\necho\necho 'failures:'\necho\n\
echo '---- refunds::refund_to_card stdout ----'\necho\n\
echo \"thread 'refunds::refund_to_card' panicked at tests/refunds.rs:31:5:\"\n\
echo 'assertion `left == right` failed: refund for {EMAIL}'\n\
echo '  left: \"{CARD}\"'\necho ' right: \"none\"'\necho\necho 'failures:'\n\
echo '    refunds::refund_to_card'\necho\n\
echo 'test result: FAILED. 40 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s'\n\
exit 101\n"
    ));
    std::fs::write(ws.join("tests/run.sh"), script).unwrap();
}

async fn run(
    dir: &Path,
    presenter: &dyn Presenter,
    engine: Option<&Arc<Engine>>,
    responses: Vec<Response>,
) -> (Terminal, Vec<Value>) {
    let frontier = Frontier::default();
    frontier.responses.lock().unwrap().extend(responses);
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let mut gate = OutboundGate::new(AuditLog::open(&dir.join("audit.jsonl")).unwrap());
    if let Some(e) = engine {
        let (filter, check) = e.outbound();
        gate = gate.with_filter(filter).with_check(check);
    }
    let gated = gate.wrap(provider);
    let cfg = RunConfig::new(dir.join("ws"), dir.join("run"), "Fix the refund test.");
    let git = declass_git::Git::locate().unwrap();
    let (terminal, _) = declass_agent::run(
        &cfg,
        &gated,
        presenter,
        &git,
        false,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    let bodies = frontier.bodies.lock().unwrap().clone();
    (terminal, bodies)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_calls_of_one_response_run_in_the_order_given() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    workspace(&dir);
    let presenter = PassThrough { max_bytes: 60_000 };
    let (terminal, bodies) = run(
        &dir,
        &presenter,
        None,
        vec![
            vec![
                (
                    "write_file",
                    json!({"path": "notes.txt", "content": "first\n"}),
                ),
                (
                    "run_command",
                    json!({"command": "cat notes.txt; echo second >> notes.txt"}),
                ),
                ("run_command", json!({"command": "cat notes.txt"})),
                ("read_file", json!({"path": "notes.txt"})),
            ],
            vec![("finish", json!({"summary": "done"}))],
        ],
    )
    .await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let got = results(&bodies[1]);
    assert_eq!(got.len(), 4, "{got:#?}");
    assert!(got[0].starts_with("created notes.txt"), "{}", got[0]);
    assert!(
        got[1].contains("first") && !got[1].contains("second"),
        "{}",
        got[1]
    );
    assert!(got[2].contains("first\nsecond"), "{}", got[2]);
    assert!(
        got[3].contains("1  first") && got[3].contains("2  second"),
        "{}",
        got[3]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_run_is_condensed_in_hybrid_mode_and_read_back_whole_without_planted_values() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    workspace(&dir);
    std::fs::create_dir_all(dir.join("run")).unwrap();
    // The shipped defaults that decide this: command output sensitive by
    // default, condensing on.
    let policy = Policy {
        sensitive_globs: vec!["data/**".into()],
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        condense_output: true,
        ..Policy::default()
    };
    let engine = Engine::open(&dir.join("run"), policy, None).unwrap();
    engine.prime(
        &dir.join("ws"),
        &["data/customers.csv".into(), "tests/run.sh".into()],
        "",
    );
    let (terminal, bodies) = run(
        &dir,
        engine.as_ref(),
        Some(&engine),
        vec![
            vec![("run_command", json!({"command": "sh tests/run.sh"}))],
            vec![(
                "read_raw",
                json!({"handle": "h1", "start_line": 1, "end_line": 20}),
            )],
            vec![("finish", json!({"summary": "done"}))],
        ],
    )
    .await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let shown = &results(&bodies[1])[0];
    assert!(
        shown.starts_with("h1 (output of `sh tests/run.sh`), condensed (cargo test):"),
        "{shown}"
    );
    assert!(
        shown.contains("test refunds::refund_to_card ... FAILED"),
        "{shown}"
    );
    assert!(shown.contains("40 passing tests"), "{shown}");
    assert!(!shown.contains("case_05 ... ok"), "{shown}");
    assert!(
        shown.contains("read_raw(handle=\"h1\", start_line=..., end_line=...) for the full output"),
        "{shown}"
    );
    let read = &results(&bodies[2])[1];
    assert!(read.contains("test refunds::case_05 ... ok"), "{read}");
    // No request carried a planted value in any form.
    let canaries = Canaries::with_options(
        [EMAIL, CARD],
        Options {
            digit_min: 8,
            ..Options::default()
        },
    );
    for (i, body) in bodies.iter().enumerate() {
        let text = body.to_string();
        assert!(canaries.find(&text).is_empty(), "request {i}: {text}");
    }
    assert!(shown.contains("refund for ⟨email:"), "{shown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn pass_through_mode_shows_a_test_run_whole() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    workspace(&dir);
    let presenter = PassThrough { max_bytes: 60_000 };
    let (_, bodies) = run(
        &dir,
        &presenter,
        None,
        vec![
            vec![("run_command", json!({"command": "sh tests/run.sh"}))],
            vec![("finish", json!({"summary": "done"}))],
        ],
    )
    .await;
    let shown = &results(&bodies[1])[0];
    assert!(shown.starts_with("exit code 101\n"), "{shown}");
    assert!(shown.contains("test refunds::case_05 ... ok"), "{shown}");
    assert!(!shown.contains("condensed"), "{shown}");
}
