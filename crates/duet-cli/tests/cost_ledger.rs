// SPDX-License-Identifier: GPL-3.0-or-later
//! The frontier loop against a scripted frontier: bulky public results reach
//! the frontier as a handle and a preview, and the run's ledger charges each
//! result to the class it was shown as.

use bytes::Bytes;
use duet_agent::{RunConfig, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::AuditLog;
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::view::{PassThrough, Presenter, ViewClass};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Replies with one tool call per request, in order, and records each body.
#[derive(Clone)]
struct Frontier {
    calls: Arc<Mutex<VecDeque<(String, Value)>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
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
            .push(serde_json::from_slice(&body).unwrap());
        let (name, args) = self.calls.lock().unwrap().pop_front().expect("unscripted");
        let n = self.bodies.lock().unwrap().len();
        let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": format!("c{n}"),
            "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]});
        let chunks = [
            format!("data: {call}\n\n"),
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n".into(),
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4000,\"completion_tokens\":50}}\n\n"
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

fn source(lines: usize) -> String {
    (1..=lines)
        .map(|i| match i {
            200 => "pub fn settle(batch: &Batch) -> Total {\n".to_owned(),
            _ => format!("    let step_{i} = apply(batch, {i});\n"),
        })
        .collect()
}

struct Outcome {
    terminal: Terminal,
    stats: duet_agent::RunStats,
    bodies: Vec<Value>,
}

async fn run(dir: &Path, presenter: &dyn Presenter, script: Vec<(&str, Value)>) -> Outcome {
    let ws = dir.join("ws");
    let run_dir = dir.join("run");
    let frontier = Frontier {
        calls: Arc::new(Mutex::new(
            script.into_iter().map(|(n, a)| (n.to_owned(), a)).collect(),
        )),
        bodies: Arc::default(),
    };
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let gated = OutboundGate::new(AuditLog::open(&dir.join("audit.jsonl")).unwrap()).wrap(provider);
    let cfg = RunConfig {
        workspace: ws,
        run_dir,
        objective: "Change settle().".into(),
        mode: "test".into(),
        checks: vec![],
        // No command runs in these scripts.
        sandbox: duet_sandbox::detect().unwrap_or(duet_sandbox::SandboxKind::Seatbelt),
        network: false,
        command_timeout: Duration::from_secs(30),
        wall_clock: Duration::from_secs(60),
        frontier_usd: 10.0,
        max_finish_attempts: 2,
        context_window: 200_000,
        mask_at: 0.7,
        max_output_tokens: 1000,
        reasoning_effort: None,
        price: Box::new(|u| (u.input as f64 + 4.0 * u.output as f64) / 1e6),
        oversight: duet_agent::Oversight::default(),
        web: None,
        git_author: None,
        mcp: None,
    };
    let git = duet_git::Git::locate().unwrap();
    let (terminal, stats) = duet_agent::run(
        &cfg,
        &gated,
        presenter,
        &git,
        false,
        &std::sync::Arc::new(AtomicBool::new(false)),
    )
    .await;
    let bodies = frontier.bodies.lock().unwrap().clone();
    Outcome {
        terminal,
        stats,
        bodies,
    }
}

fn workspace() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("ws/src")).unwrap();
    std::fs::write(d.path().join("ws/src/settle.rs"), source(400)).unwrap();
    d
}

fn script() -> Vec<(&'static str, Value)> {
    vec![
        ("read_file", json!({"path": "src/settle.rs"})),
        (
            "read_raw",
            json!({"handle": "h1", "start_line": 195, "end_line": 205}),
        ),
        ("finish", json!({"summary": "done"})),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn bulky_files_reach_the_frontier_as_a_preview_and_are_charged_as_such() {
    let d = workspace();
    let policy = Policy {
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 2000,
        ..Policy::default()
    };
    let engine = Engine::open(&d.path().join("run"), policy, None).unwrap();
    let out = run(d.path(), engine.as_ref(), script()).await;
    assert!(
        matches!(out.terminal, Terminal::Completed { .. }),
        "{:?}",
        out.terminal
    );
    // The second request carries the preview, not the file.
    let second = out.bodies[1].to_string();
    assert!(
        second.contains("h1 (src/settle.rs): lines 1-400"),
        "{second}"
    );
    assert!(second.contains("pub fn settle(batch: &Batch)"), "outline");
    assert!(!second.contains("step_300"), "body past the head was sent");
    // The third carries the requested range.
    let third = out.bodies[2].to_string();
    assert!(third.contains("step_199") && !third.contains("step_300"));

    let l = &out.stats.ledger;
    let bulky = &l.by_class[&ViewClass::BulkyHandle];
    let raw = &l.by_class[&ViewClass::Raw];
    assert_eq!((bulky.results, raw.results), (1, 2));
    // The preview rode along in requests 2 and 3; the range only in request 3.
    assert!(bulky.carried_tokens >= 2 * bulky.added_tokens - 2);
    assert!(bulky.added_tokens < 1500, "{}", bulky.added_tokens);
    assert!(l.request_tokens > bulky.carried_tokens + raw.carried_tokens);
    assert!((l.input_usd - 3.0 * 4000.0 / 1e6).abs() < 1e-9);
    assert!((l.output_usd - 3.0 * 200.0 / 1e6).abs() < 1e-9);
    assert!(bulky.input_usd > 0.0 && bulky.input_usd < l.input_usd);

    // The summary written by the CLI carries the ledger by class name.
    let v = serde_json::to_value(&out.stats).unwrap();
    assert!(v["ledger"]["by_class"]["bulky_handle"]["carried_tokens"].is_u64());
}

#[tokio::test(flavor = "multi_thread")]
async fn passthrough_shows_the_whole_file_and_charges_it_as_raw() {
    let d = workspace();
    let passthrough = PassThrough { max_bytes: 60_000 };
    let mut s = script();
    s.remove(1);
    let out = run(d.path(), &passthrough, s).await;
    assert!(matches!(out.terminal, Terminal::Completed { .. }));
    assert!(out.bodies[1].to_string().contains("step_300"));
    let l = &out.stats.ledger;
    assert_eq!(
        l.by_class.keys().copied().collect::<Vec<_>>(),
        [ViewClass::Raw]
    );
    assert_eq!(l.by_class[&ViewClass::Raw].results, 2);
    assert!(l.by_class[&ViewClass::Raw].added_tokens > 3000);
}
