// SPDX-License-Identifier: GPL-3.0-or-later
//! Values the operator types, through the frontier loop with a scripted
//! frontier and a scripted local model: a card number in the task never
//! reaches the frontier (whether or not its checksum holds), its placeholder
//! is a handle `ask_local` answers from in one call, no digits of it come back
//! through the local answer, and the operator is shown their own value in the
//! run's end state. Nothing here contacts a model server.

use bytes::Bytes;
use duet_agent::{ApproveMode, Oversight, RunConfig, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::AuditLog;
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
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
    bodies: Arc<Mutex<Vec<String>>>,
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

struct Outcome {
    terminal: Terminal,
    bodies: Vec<String>,
    local_prompts: Vec<String>,
    transcript: String,
}

/// A hybrid run in an empty repository on `objective`, with the frontier
/// making `script` and the local model answering `local`.
async fn run(
    root: &Path,
    objective: &str,
    script: Vec<(&str, Value)>,
    local: Vec<Value>,
) -> Outcome {
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("README.md"), "cardq\n").unwrap();
    let run_dir = ws.join(".duet/runs/r1");
    let frontier = Frontier {
        calls: Arc::new(Mutex::new(
            script.into_iter().map(|(n, a)| (n.to_owned(), a)).collect(),
        )),
        bodies: Arc::default(),
    };
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let (reader, received) =
        duet_boundary::testing::scripted_local(local.iter().map(Value::to_string).collect());
    // The shipped defaults: nothing in this repository is sensitive by policy.
    let policy = Policy {
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        ..Policy::default()
    };
    let engine = Engine::open(&run_dir, policy, Some(reader)).unwrap();
    engine.prime(&ws, &["README.md".to_owned()], objective);
    let (filter, check) = engine.outbound();
    let gated = OutboundGate::new(AuditLog::open(&root.join("audit.jsonl")).unwrap())
        .with_filter(filter)
        .with_check(check)
        .wrap(provider);
    let cfg = RunConfig {
        workspace: ws.clone(),
        run_dir: run_dir.clone(),
        objective: objective.into(),
        mode: "hybrid".into(),
        checks: vec![],
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
        price: Box::new(|u| u.input as f64 / 1e6),
        oversight: Oversight {
            mode: ApproveMode::Off,
            approver: None,
            ..Oversight::default()
        },
        web: None,
        git_author: None,
        mcp: None,
    };
    let git = duet_git::Git::locate().unwrap();
    let (terminal, _) = duet_agent::run(
        &cfg,
        &gated,
        engine.as_ref(),
        &git,
        false,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    Outcome {
        terminal,
        bodies: frontier.bodies.lock().unwrap().clone(),
        local_prompts: (0..received.bodies().len())
            .map(|i| received.prompt(i))
            .collect(),
        transcript: std::fs::read_to_string(run_dir.join("transcript.jsonl")).unwrap(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_typed_card_number_is_answered_through_its_placeholder_in_one_local_call() {
    // Seen in a live run: the frontier saw the placeholder, `ask_local` refused
    // it as an unknown handle, and fourteen turns later the frontier had dug
    // the value out of Duet's run state; a local summary gave its first digits.
    const CARD: &str = "4539578763621486";
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let out = run(
        &root,
        &format!("is this credit card number valid {CARD}?"),
        vec![
            (
                "ask_local",
                json!({"handle": "card:card#1", "questions": ["Does it pass the Luhn check? Its length and network prefix?"]}),
            ),
            ("finish", json!({"summary": "⟨card:card#1⟩ passes the Luhn check."})),
        ],
        vec![json!({
            "answer": "It passes the Luhn check; 16 digits; network prefix 4539 (Visa).",
            "evidence_lines": [1],
            "unanswerable": false
        })],
    )
    .await;
    let sent = out.bodies.concat();
    assert!(!sent.contains(CARD) && !sent.contains("4539"), "{sent}");
    // The frontier was told once how to use the placeholder, and the one call worked.
    assert!(
        out.bodies[0].contains("ask_local(handle=\\\"card:card#1\\\""),
        "{}",
        out.bodies[0]
    );
    assert!(
        out.bodies[1].contains("passes the Luhn check; 16 digits")
            && !out.bodies[1].contains("unknown handle"),
        "{}",
        out.bodies[1]
    );
    // The local model read the operator's message with the real value.
    assert_eq!(out.local_prompts.len(), 1);
    assert!(
        out.local_prompts[0].contains(CARD),
        "{}",
        out.local_prompts[0]
    );
    // The operator sees their own value; the transcript keeps the frontier's view.
    assert_eq!(
        out.terminal,
        Terminal::Completed {
            summary: format!("{CARD} passes the Luhn check.")
        }
    );
    let end = out.transcript.lines().last().unwrap();
    assert!(
        end.contains("\"kind\":\"end\"")
            && end.contains("⟨card:card#1⟩ passes")
            && !end.contains(CARD),
        "{end}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_labelled_number_that_fails_the_checksum_never_reaches_the_frontier() {
    // Seen in a live run: this number fails the Luhn check, so no detector
    // took it for a card, and it was sent as typed.
    const NUMBER: &str = "42977600076546677";
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let out = run(
        &root,
        &format!("is this credit card number valid {NUMBER}?"),
        vec![("finish", json!({"summary": "It fails the check."}))],
        vec![],
    )
    .await;
    assert!(
        matches!(out.terminal, Terminal::Completed { .. }),
        "{:?}",
        out.terminal
    );
    let sent = out.bodies.concat();
    assert!(!sent.contains(NUMBER) && !sent.contains("4297"), "{sent}");
    assert!(sent.contains("⟨card:card#1⟩"), "{sent}");
}
