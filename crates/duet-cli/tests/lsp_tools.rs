// SPDX-License-Identifier: GPL-3.0-or-later
//! The language-server tools through the frontier loop, with a scripted
//! frontier and the scripted language server running in-process. The server
//! is deliberately not sandboxed here, so it reads the sensitive and protected
//! files and answers with them: whatever it says, the frontier must see
//! locations only for sensitive and sealed files, declarations only for
//! interface-only files, and `rename` must refuse to touch any of them.
//! Nothing here contacts a model server.

use bytes::Bytes;
use duet_agent::{RunConfig, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{AuditLog, Line, read};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::view::{PassThrough, Presenter};
use duet_lsp::mock::{MockLauncher, detected};
use duet_lsp::{Lsp, Settings};
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

const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";
const BALANCE: &str = "8977066";
const SEALED_LITERAL: &str = "zq4k2m9x8v7c6b5n";
const BODY_TOKEN: &str = "uplift_zq71";

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

const MAIN: &str = "fn main() {\n    let t = customer_total();\n    let q = quote(t);\n    let k = master_key();\n    helper();\n}\n";

fn workspace(root: &Path) -> PathBuf {
    let ws = root.join("ws");
    for dir in ["data", "src/pricing", "src/vault"] {
        std::fs::create_dir_all(ws.join(dir)).unwrap();
    }
    std::fs::write(
        ws.join("data/customers.rs"),
        format!("/// Owner: {EMAIL}\npub fn customer_total() -> u64 {{ {BALANCE} }}\n"),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/pricing/engine.rs"),
        format!(
            "/// Quotes a price.\npub fn quote(base: u64) -> u64 {{\n    let {BODY_TOKEN} = base * 8731 + 42;\n    {BODY_TOKEN}\n}}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/vault/keys.rs"),
        format!("pub fn master_key() -> &'static str {{\n    \"{SEALED_LITERAL}\"\n}}\n"),
    )
    .unwrap();
    std::fs::write(ws.join("src/main.rs"), MAIN).unwrap();
    std::fs::write(ws.join("src/util.rs"), "pub fn helper() {}\n").unwrap();
    ws
}

const FILES: [&str; 5] = [
    "data/customers.rs",
    "src/main.rs",
    "src/pricing/engine.rs",
    "src/util.rs",
    "src/vault/keys.rs",
];

fn config(ws: &Path, run_dir: &Path, lsp: Arc<Lsp>) -> RunConfig {
    RunConfig {
        workspace: ws.to_path_buf(),
        run_dir: run_dir.to_path_buf(),
        objective: "Tidy the entry point.".into(),
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
        oversight: duet_agent::Oversight::default(),
        web: None,
        git_author: None,
        mcp: None,
        lsp: Some(lsp),
        subagents: None,
    }
}

fn servers(ws: &Path, launcher: &MockLauncher) -> Arc<Lsp> {
    Arc::new(Lsp::new(
        ws.to_path_buf(),
        vec![detected("rust", &["rs"])],
        Box::new(launcher.clone()),
        Settings {
            enabled: true,
            request_timeout: Duration::from_secs(5),
            diagnostics_wait: Duration::from_secs(2),
            ..Settings::default()
        },
    ))
}

async fn run(
    root: &Path,
    ws: &Path,
    presenter: &dyn Presenter,
    engine: Option<&Arc<Engine>>,
    lsp: Arc<Lsp>,
    script: Vec<(&str, Value)>,
) -> (Terminal, Vec<String>, PathBuf) {
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
    let mut gate = OutboundGate::new(AuditLog::open(&log).unwrap());
    if let Some(e) = engine {
        let (filter, check) = e.outbound();
        gate = gate.with_filter(filter).with_check(check);
    }
    let gated = gate.wrap(provider);
    let cfg = config(ws, &root.join("run"), lsp);
    let git = duet_git::Git::locate().unwrap();
    let (terminal, _) = duet_agent::run(
        &cfg,
        &gated,
        presenter,
        &git,
        false,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    let bodies = frontier.bodies.lock().unwrap().clone();
    (terminal, bodies, log)
}

/// The tool result of the `n`th call (1-based), as the frontier received it.
fn result(bodies: &[String], n: usize) -> String {
    let v: Value = serde_json::from_str(&bodies[n]).unwrap();
    v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

fn nav(op: &str, path: &str, line: u32, column: u32) -> Value {
    json!({"op": op, "path": path, "line": line, "column": column})
}

#[tokio::test(flavor = "multi_thread")]
async fn hybrid_language_server_answers_never_show_sensitive_or_protected_content() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = workspace(&root);
    let policy = Policy {
        sensitive_globs: vec!["data/**".into()],
        interface_only: vec!["src/pricing/**".into()],
        sealed: vec!["src/vault/**".into()],
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        ..Policy::default()
    };
    let engine = Engine::open(&root.join("run"), policy, None).unwrap();
    let files: Vec<String> = FILES.iter().map(|f| (*f).to_owned()).collect();
    engine.prime(&ws, &files, "");
    let launcher = MockLauncher::default();
    let script = vec![
        ("code_nav", nav("references", "src/main.rs", 2, 13)),
        ("code_nav", nav("hover", "src/main.rs", 2, 13)),
        (
            "code_nav",
            json!({"op": "workspace_symbols", "query": "customer"}),
        ),
        ("code_nav", nav("hover", "src/main.rs", 3, 13)),
        ("code_nav", nav("references", "src/main.rs", 3, 13)),
        ("code_nav", nav("definition", "src/main.rs", 4, 13)),
        (
            "code_nav",
            json!({"op": "symbols", "path": "src/vault/keys.rs"}),
        ),
        ("code_nav", nav("definition", "data/customers.rs", 2, 8)),
        ("code_nav", nav("hover", "src/pricing/engine.rs", 3, 9)),
        (
            "rename",
            json!({"path": "src/main.rs", "line": 2, "column": 13, "new_name": "grand_total"}),
        ),
        (
            "rename",
            json!({"path": "src/main.rs", "line": 3, "column": 13, "new_name": "price"}),
        ),
        (
            "rename",
            json!({"path": "src/main.rs", "line": 5, "column": 5, "new_name": "assist"}),
        ),
        ("finish", json!({"summary": "renamed"})),
    ];
    let (terminal, bodies, log) = run(
        &root,
        &ws,
        engine.as_ref(),
        Some(&engine),
        servers(&ws, &launcher),
        script,
    )
    .await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let names = {
        let v: Value = serde_json::from_str(&bodies[0]).unwrap();
        v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert!(names.contains(&"code_nav".to_owned()) && names.contains(&"rename".to_owned()));
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "the tool set is sorted");

    // Nothing of the sensitive file or the withheld code reached the frontier.
    for (i, body) in bodies.iter().enumerate() {
        for leak in [EMAIL, BALANCE, SEALED_LITERAL, BODY_TOKEN, "8731"] {
            assert!(!body.contains(leak), "request {i} carries {leak}");
        }
    }
    let refs = result(&bodies, 1);
    assert!(
        refs.contains("src/main.rs:2:13  let t = customer_total();"),
        "{refs}"
    );
    assert!(
        refs.contains("data/customers.rs:2:8  ⟨withheld: sensitive file⟩"),
        "{refs}"
    );
    assert!(
        result(&bodies, 2).contains("⟨withheld: sensitive file⟩"),
        "{}",
        result(&bodies, 2)
    );
    assert!(
        result(&bodies, 3).contains("data/customers.rs:2:8  ⟨withheld: sensitive file⟩"),
        "{}",
        result(&bodies, 3)
    );
    // Interface-only: the declaration and its documentation, never a body line.
    let hover = result(&bodies, 4);
    assert!(
        hover.contains("pub fn quote(base: u64) -> u64") && hover.contains("Quotes a price."),
        "{hover}"
    );
    assert!(
        result(&bodies, 5).contains("src/pricing/engine.rs  ⟨withheld: protected code⟩"),
        "{}",
        result(&bodies, 5)
    );
    // Sealed: not even the line.
    assert!(
        result(&bodies, 6).contains("src/vault/keys.rs  ⟨withheld: sealed source⟩"),
        "{}",
        result(&bodies, 6)
    );
    assert!(
        result(&bodies, 7).contains("⟨withheld: sealed source⟩"),
        "{}",
        result(&bodies, 7)
    );
    assert!(result(&bodies, 8).contains("is sensitive"));
    assert!(result(&bodies, 9).contains("is protected source"));
    // Renames that would touch a sensitive or protected file change nothing.
    assert!(
        result(&bodies, 10).contains("data/customers.rs (sensitive); nothing was changed"),
        "{}",
        result(&bodies, 10)
    );
    assert!(
        result(&bodies, 11).contains("protected source"),
        "{}",
        result(&bodies, 11)
    );
    assert!(
        std::fs::read_to_string(ws.join("data/customers.rs"))
            .unwrap()
            .contains("customer_total")
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("src/main.rs"))
            .unwrap()
            .matches("customer_total")
            .count(),
        1
    );
    // A rename within open source files is applied to all of them.
    let renamed = result(&bodies, 12);
    assert!(renamed.contains("2 edit(s) in 2 file(s)"), "{renamed}");
    assert!(
        std::fs::read_to_string(ws.join("src/main.rs"))
            .unwrap()
            .contains("    assist();")
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("src/util.rs")).unwrap(),
        "pub fn assist() {}\n"
    );

    // The server was never told it may read the sensitive data or git history.
    let denied = launcher.denied.lock().unwrap().clone();
    assert_eq!(denied.len(), 1, "one server for the run: {denied:?}");
    assert!(denied[0].contains(&ws.join("data")), "{denied:?}");
    assert!(denied[0].contains(&ws.join(".git")) && denied[0].contains(&ws.join(".duet")));
    assert!(
        !denied[0].iter().any(|p| p.starts_with(ws.join("src"))),
        "protected source stays readable to the server: {denied:?}"
    );

    // Every call is audited, without content.
    let text = std::fs::read_to_string(&log).unwrap();
    for leak in [EMAIL, BALANCE] {
        assert!(!text.contains(leak), "audit carries {leak}");
    }
    let events: Vec<Value> = read(&log)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) if e.event.kind() == "language_server" => {
                Some(serde_json::to_value(&e.event).unwrap())
            }
            _ => None,
        })
        .collect();
    let recorded = serde_json::to_string(&events).unwrap();
    for leak in ["grand_total", "assist", "customer"] {
        assert!(
            !recorded.contains(leak),
            "a language_server event carries {leak}"
        );
    }
    let ops: Vec<String> = events
        .iter()
        .map(|e| {
            format!(
                "{} {}",
                e["op"].as_str().unwrap(),
                e["outcome"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(ops[0], "server started", "{ops:?}");
    assert_eq!(ops[1], "code_nav:references ok", "{ops:?}");
    assert!(
        ops.contains(&"code_nav:definition refused".to_owned()),
        "{ops:?}"
    );
    assert_eq!(
        ops.iter().filter(|o| o.starts_with("rename")).count(),
        3,
        "{ops:?}"
    );
    let refs_event = &events[1];
    assert_eq!(
        (
            refs_event["shown"].as_u64(),
            refs_event["withheld"].as_u64()
        ),
        (Some(1), Some(1))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pass_through_shows_answers_and_a_crashing_server_never_ends_the_run() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = workspace(&root);
    std::fs::write(ws.join("src/c.rs"), "fn crash() {}\n").unwrap();
    let launcher = MockLauncher::default();
    let script = vec![
        ("code_nav", nav("hover", "src/main.rs", 2, 13)),
        ("code_nav", nav("hover", "src/c.rs", 1, 4)),
        ("code_nav", nav("definition", "src/main.rs", 2, 13)),
        ("finish", json!({"summary": "looked"})),
    ];
    let passthrough = PassThrough { max_bytes: 60_000 };
    let (terminal, bodies, _) = run(
        &root,
        &ws,
        &passthrough,
        None,
        servers(&ws, &launcher),
        script,
    )
    .await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    assert!(
        result(&bodies, 1).contains(EMAIL),
        "pass-through shows content as it is"
    );
    let crashed = result(&bodies, 2);
    assert!(
        crashed.contains("unavailable for the rest of this run"),
        "{crashed}"
    );
    assert!(result(&bodies, 3).contains("unavailable"));
}
