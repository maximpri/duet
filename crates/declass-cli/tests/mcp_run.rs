// SPDX-License-Identifier: GPL-3.0-or-later
//! MCP tools through the frontier loop in hybrid mode, with a scripted
//! frontier, a hostile public HTTP server, a sensitive stdio server in the
//! sandbox and an approver that denies: the tools are offered sorted with the
//! others, and no planted value reaches the frontier. Nothing here contacts a
//! model server.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use bytes::Bytes;
use declass_agent::mcp::{Hub, Setup};
use declass_agent::oversight::Action;
use declass_agent::{ApproveMode, Approver, Oversight, RunConfig, Terminal};
use declass_boundary::OutboundGate;
use declass_boundary::audit::{AuditEvent, AuditLog, Line, read};
use declass_boundary::engine::Engine;
use declass_boundary::policy::Policy;
use declass_boundary::view::{Presenter, Source};
use declass_mcp::mock::{Answer, HttpMock, Mock, SH_SERVER};
use declass_mcp::{Approve, Launch, ServerConfig, Trust};
use declass_provider::client::{HttpReply, Transport};
use declass_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SECRET: &str = "sk_live_9f8e7d6c5b4a39281706";
const EMAIL: &str = "marta.kowalczyk@corp-mail.net";

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

#[derive(Default)]
struct Deny(Mutex<Vec<Action>>);

impl Approver for Deny {
    fn approve(&self, action: &Action) -> bool {
        self.0.lock().unwrap().push(action.clone());
        false
    }
}

fn server(name: &str, launch: Launch, trust: Trust) -> ServerConfig {
    ServerConfig {
        name: name.into(),
        launch,
        env: vec![],
        headers_env: vec![],
        trust,
        network: false,
        approve: Approve::Writes,
        timeout: Duration::from_secs(5),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_tools_in_a_hybrid_run_leak_nothing_and_writes_need_approval() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = root.join("ws");
    let run_dir = root.join("run");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,email,key\n1,{EMAIL},{SECRET}\n"),
    )
    .unwrap();
    std::fs::write(ws.join("README.md"), "readme\n").unwrap();

    let policy = Policy {
        sensitive_globs: vec!["data/**".into()],
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        ..Policy::default()
    };
    let engine = Engine::open(&run_dir, policy, None).unwrap();
    engine.prime(&ws, &["data/customers.csv".to_owned()], "");
    // The placeholder the frontier knows the email by.
    let shown = engine.present(&Source::Other { label: "x".into() }, EMAIL.as_bytes());
    let token = shown.trim().to_owned();
    assert!(token.starts_with('⟨'), "{token}");

    let mock = Mock {
        echo_description: format!("Echo. Mail {EMAIL}; key {SECRET}."),
        canned: format!("row 1: {EMAIL} {SECRET}"),
        ..Mock::default()
    };
    let http = HttpMock::start(mock.clone(), Answer::EventStream).await;

    let script = vec![
        ("mcp__pub__canned", json!({})),
        ("mcp__pub__echo", json!({"text": format!("mail {token}")})),
        ("mcp__pub__write", json!({"text": "change it"})),
        ("mcp__local__note", json!({"text": token})),
        ("finish", json!({"summary": "done"})),
    ];
    let frontier = Frontier {
        calls: Arc::new(Mutex::new(
            script.into_iter().map(|(n, a)| (n.to_owned(), a)).collect(),
        )),
        bodies: Arc::default(),
    };
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let log = root.join("audit.jsonl");
    let (filter, check) = engine.outbound();
    let gated = OutboundGate::new(AuditLog::open(&log).unwrap())
        .with_filter(filter)
        .with_check(check)
        .wrap(provider);
    gated.audit().record(AuditEvent::RunStart {
        mode: "hybrid".into(),
        boundary: true,
    });
    let sandbox = declass_sandbox::detect().unwrap();
    // The owner trusts the local server's tools (no approval asked).
    let mut local = server(
        "local",
        Launch::Command {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), SH_SERVER.into()],
        },
        Trust::Sensitive,
    );
    local.approve = Approve::Auto;
    let servers = [
        server("pub", Launch::Url(http.url.clone()), Trust::Public),
        local,
    ];
    let hub = Hub::start(
        &servers,
        &Setup {
            workspace: &ws,
            run_dir: &run_dir,
            sandbox,
            presenter: engine.as_ref(),
            audit: Some(gated.audit()),
        },
    )
    .await;
    let hub = Arc::new(hub);
    let deny = Arc::new(Deny::default());
    let cfg = RunConfig {
        mode: "hybrid".into(),
        sandbox,
        price: Box::new(|u| u.input as f64 / 1e6),
        oversight: Oversight {
            mode: ApproveMode::Risky,
            approver: Some(deny.clone()),
            ..Oversight::default()
        },
        mcp: Some(hub.clone()),
        ..RunConfig::new(ws.clone(), run_dir, "Look up the customer.")
    };
    let git = declass_git::Git::locate().unwrap();
    let (terminal, _) = declass_agent::run(
        &cfg,
        &gated,
        engine.as_ref(),
        &git,
        false,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    hub.shutdown().await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );

    let bodies = frontier.bodies.lock().unwrap().clone();
    // The tools are offered with the built-in ones, sorted, in every request.
    let first: Value = serde_json::from_str(&bodies[0]).unwrap();
    let names: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    for n in [
        "mcp__local__cat",
        "mcp__local__note",
        "mcp__pub__echo",
        "read_file",
    ] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    assert!(bodies[1].contains("row 1:"), "{}", bodies[1]);
    assert!(bodies[2].contains("not sent"), "{}", bodies[2]);
    assert!(bodies[3].contains("did not approve"), "{}", bodies[3]);
    assert!(
        bodies[4].contains("result of MCP tool `note` on server `local`"),
        "{}",
        bodies[4]
    );
    for (i, b) in bodies.iter().enumerate() {
        assert!(
            !b.contains(EMAIL) && !b.contains(SECRET),
            "request {i} leaked: {b}"
        );
    }

    // The public server got only the argument-free call; the write was never
    // made; the local server got the real value.
    assert_eq!(mock.calls(), [json!({})]);
    let asked = deny.0.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, "mcp__pub__write");
    assert_eq!(
        std::fs::read_to_string(ws.join("received.txt"))
            .unwrap()
            .trim(),
        EMAIL
    );
    let kinds: Vec<String> = read(&log)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) => Some(e.event.kind().to_owned()),
            Line::Request(_) => None,
        })
        .collect();
    for k in ["mcp_server", "mcp_call", "outbound_refused", "approval"] {
        assert!(kinds.iter().any(|x| x == k), "{k} not audited: {kinds:?}");
    }
    let log_text = std::fs::read_to_string(&log).unwrap();
    assert!(!log_text.contains(EMAIL) && !log_text.contains(SECRET));
}
