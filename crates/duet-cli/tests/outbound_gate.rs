// SPDX-License-Identifier: GPL-3.0-or-later
//! The outbound gate through the frontier loop in hybrid mode, with a
//! scripted frontier: the final check never stops a run over a value the
//! outbound filter could have replaced, and a request it still refuses is
//! sent with the refused part withheld instead of ending the run.
//!
//! - A value the filter's detectors find only in a masking stub (a path in a
//!   command the stub quotes) is replaced in the model's own call
//!   too, and the run goes on (DUET-2026-019, found in an XL calibration run
//!   that ended `failed` after 65 minutes).
//! - Reading a file whose path the detectors take for a secret goes on too:
//!   `read_file` puts the path in its result.
//! - A part the check refuses after filtering (a stand-in for a path the
//!   filter does not know) is withheld; the run completes, the audit log
//!   holds a `send_withheld` event and the disclosure report counts it.
//! - A request that withholding cannot clear ends the run as `failed`, with
//!   the reason.
//!
//! Nothing here contacts a model server.

use bytes::Bytes;
use duet_agent::{RunConfig, RunStats, Terminal};
use duet_boundary::audit::{AuditEvent, AuditLog, Line, read};
use duet_boundary::engine::{Engine, PART_WITHHELD};
use duet_boundary::model::{Item, Request};
use duet_boundary::policy::Policy;
use duet_boundary::{OutboundCheck, OutboundFilter, OutboundGate};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RUN_ID: &str = "20260925-000000-gate01";
const KEY: &str = "sk_live_eQ7mZ2vK9pX4rT8wL3nB6cY1";
/// A test-suite file whose path the entropy detector takes for a secret, as
/// `test/test-suite/groups/function-fromMillis/case000.json` was in the run.
/// That path no longer is one (tokens are judged by their parts); a relative
/// path holding a random segment still is.
const SUITE: &str = "spec/suite/k7Qx2LmZ9wRt4VbN8sYc3HjD6gFa5UeP/case031.json";
const SUITE_STEM: &str = "spec/suite/k7Qx2LmZ9wRt4VbN8sYc3HjD6gFa5UeP/case031";

/// Replies with one tool call per request, in order, and records each body.
#[derive(Clone, Default)]
struct Frontier {
    calls: Arc<Mutex<VecDeque<(String, Value)>>>,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Frontier {
    fn new(calls: Vec<(&str, Value)>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(
                calls.into_iter().map(|(n, a)| (n.to_owned(), a)).collect(),
            )),
            bodies: Arc::default(),
        }
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        self.bodies
            .lock()
            .unwrap()
            .push(String::from_utf8(body).unwrap());
        let (name, args) = self.calls.lock().unwrap().pop_front().expect("unscripted");
        let n = self.bodies.lock().unwrap().len();
        let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": format!("c{n}"),
            "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]});
        let chunks = [
            format!("data: {call}\n\n"),
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

/// Lines of a public file, about `bytes` long.
fn filler(tag: &str, bytes: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    while out.len() < bytes {
        out.push_str(&format!(
            "{{\"expr\": \"$formatInteger({n}, '#,##0')\", \"result\": \"{tag} case {n}\"}}\n"
        ));
        n += 1;
    }
    out
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join(SUITE).parent().unwrap()).unwrap();
    std::fs::write(ws.join(".env"), format!("NOVAPAY_API_KEY={KEY}\n")).unwrap();
    std::fs::write(ws.join(SUITE), filler("suite", 3500)).unwrap();
    for k in 1..=6 {
        std::fs::write(ws.join(format!("src/part{k}.js")), filler("part", 3500)).unwrap();
    }
    (d, root)
}

struct Ended {
    terminal: Terminal,
    summary: Value,
    bodies: Vec<String>,
    events: Vec<AuditEvent>,
    transcript: String,
}

/// A hybrid run as `duet run` composes it: the engine primed on the
/// workspace, its filter and check on the gate, then `filter` and `check`.
async fn run(
    root: &Path,
    frontier: Frontier,
    filter: Option<Box<dyn OutboundFilter>>,
    check: Option<Box<dyn OutboundCheck>>,
    context_window: u64,
) -> Ended {
    let ws = root.join("ws");
    let run_dir = ws.join(".duet/runs").join(RUN_ID);
    std::fs::create_dir_all(&run_dir).unwrap();
    let audit_path = root.join("audit.jsonl");
    let policy = Policy {
        sensitive_globs: vec![".env*".into(), "data/**".into()],
        command_output_sensitive: true,
        secret_sinks: vec![".env*".into()],
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 4000,
        bulky_file_tokens: 12000,
        ..Policy::default()
    };
    let objective = "Fix $formatInteger for grouped patterns.";
    let engine = Engine::open(&run_dir, policy, None).unwrap();
    engine.prime(&ws, &[], objective);
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let (sanitize, known_values) = engine.outbound();
    let mut gate = OutboundGate::new(AuditLog::open(&audit_path).unwrap())
        .with_filter(sanitize)
        .with_check(known_values);
    if let Some(f) = filter {
        gate = gate.with_filter(f);
    }
    if let Some(c) = check {
        gate = gate.with_check(c);
    }
    let gated = gate.wrap(provider);
    gated.audit().record(AuditEvent::RunStart {
        mode: "hybrid".into(),
        boundary: true,
    });
    let cfg = RunConfig {
        mode: "hybrid".into(),
        wall_clock: Duration::from_secs(120),
        context_window,
        price: Box::new(|u| u.input as f64 / 1e6),
        ..RunConfig::new(ws.clone(), run_dir.clone(), objective)
    };
    let git = duet_git::Git::locate().unwrap();
    let (terminal, stats): (Terminal, RunStats) = duet_agent::run(
        &cfg,
        &gated,
        engine.as_ref(),
        &git,
        false,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    let summary = duet_agent::conclude(
        &run_dir,
        RUN_ID,
        Some(gated.audit()),
        &audit_path,
        &terminal,
        &stats,
    )
    .unwrap();
    let events = read(&audit_path)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) => Some(e.event),
            Line::Request(_) => None,
        })
        .collect();
    Ended {
        terminal,
        summary,
        bodies: frontier.bodies(),
        events,
        transcript: std::fs::read_to_string(run_dir.join("transcript.jsonl")).unwrap(),
    }
}

fn blocked(events: &[AuditEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AuditEvent::BlockedSend { .. }))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_found_in_a_masking_stub_is_replaced_in_the_call_it_quotes() {
    let (_d, root) = workspace();
    // The suite file is printed by a command (its result never names the
    // path); five more turns push the context over `mask_at`, and the stub
    // that replaces the first result quotes the command.
    let mut calls = vec![("run_command", json!({ "command": format!("cat {SUITE}") }))];
    for k in 1..=5 {
        calls.push(("read_file", json!({ "path": format!("src/part{k}.js") })));
    }
    calls.push(("finish", json!({"summary": "done"})));
    let frontier = Frontier::new(calls);
    let ended = run(&root, frontier, None, None, 7000).await;
    assert!(
        matches!(ended.terminal, Terminal::Completed { .. }),
        "{:?}",
        ended.terminal
    );
    assert!(
        ended.transcript.contains("\"kind\":\"masked\""),
        "the context was masked"
    );
    let masked = ended
        .bodies
        .iter()
        .position(|b| b.contains("[masked: run_command `cat "))
        .expect("a stub quoting the command was sent");
    // From that request on, the path is a placeholder in the stub and in the
    // model's own call alike.
    for b in &ended.bodies[masked..] {
        assert!(!b.contains(SUITE_STEM), "{b}");
    }
    assert!(
        ended.bodies[1].contains(SUITE_STEM),
        "the model's own call, before"
    );
    assert_eq!(blocked(&ended.events), 0);
    assert!(ended.bodies.iter().all(|b| !b.contains(KEY)));
}

#[tokio::test(flavor = "multi_thread")]
async fn reading_a_file_whose_path_a_detector_flags_does_not_stop_the_run() {
    let (_d, root) = workspace();
    let frontier = Frontier::new(vec![
        ("read_file", json!({ "path": SUITE })),
        ("finish", json!({"summary": "done"})),
    ]);
    let ended = run(&root, frontier, None, None, 200_000).await;
    assert!(
        matches!(ended.terminal, Terminal::Completed { .. }),
        "{:?}",
        ended.terminal
    );
    assert_eq!(ended.bodies.len(), 2);
    assert!(!ended.bodies[1].contains(SUITE_STEM), "{}", ended.bodies[1]);
    assert_eq!(blocked(&ended.events), 0);
}

/// Puts the `.env` key back into the last tool result after the engine's
/// filter ran: a stand-in for a path the filter does not cover.
struct Reinsert;

impl OutboundFilter for Reinsert {
    fn name(&self) -> &'static str {
        "reinsert"
    }
    fn apply(&self, request: &mut Request) -> Vec<String> {
        if let Some(Item::ToolResult { content, .. }) = request
            .items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, Item::ToolResult { .. }))
        {
            content.push_str(&format!("\nkey {KEY}"));
        }
        Vec::new()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_part_the_check_refuses_after_filtering_is_withheld_and_the_run_goes_on() {
    let (_d, root) = workspace();
    let frontier = Frontier::new(vec![
        ("read_file", json!({"path": "src/part1.js"})),
        ("finish", json!({"summary": "done"})),
    ]);
    let ended = run(&root, frontier, Some(Box::new(Reinsert)), None, 200_000).await;
    assert!(
        matches!(ended.terminal, Terminal::Completed { .. }),
        "{:?}",
        ended.terminal
    );
    assert!(ended.bodies.iter().all(|b| !b.contains(KEY)));
    assert!(
        ended.bodies[1].contains(PART_WITHHELD),
        "{}",
        ended.bodies[1]
    );
    assert_eq!(
        ended
            .events
            .iter()
            .filter(|e| matches!(e, AuditEvent::SendWithheld { check, parts: 1 } if check == "known-values"))
            .count(),
        1,
        "{:?}",
        ended.events
    );
    assert_eq!(blocked(&ended.events), 0);
    assert_eq!(
        ended.summary["disclosure"]["withheld_sends"]["known-values"],
        1
    );
    assert!(
        ended
            .transcript
            .contains("sanitize: withheld 1 part(s) the known-values check refused"),
        "the turn's interventions say so"
    );
}

/// Refuses every request.
struct Refuse;

impl OutboundCheck for Refuse {
    fn name(&self) -> &'static str {
        "refuse-all"
    }
    fn check(&self, _body: &Value) -> Result<(), String> {
        Err("nothing may be sent".into())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_withholding_cannot_clear_ends_the_run_with_the_reason() {
    let (_d, root) = workspace();
    let frontier = Frontier::new(vec![("finish", json!({"summary": "done"}))]);
    let ended = run(&root, frontier, None, Some(Box::new(Refuse)), 200_000).await;
    let Terminal::Failed { reason } = &ended.terminal else {
        panic!("{:?}", ended.terminal)
    };
    assert_eq!(
        reason,
        "frontier: request blocked by refuse-all: nothing may be sent; no part of the request \
could be withheld instead"
    );
    assert!(ended.bodies.is_empty(), "nothing was sent");
    assert_eq!(blocked(&ended.events), 1);
    assert_eq!(ended.summary["terminal"]["state"], "failed");
}
