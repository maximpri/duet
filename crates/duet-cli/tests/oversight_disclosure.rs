// SPDX-License-Identifier: GPL-3.0-or-later
//! Operator approval and the disclosure report, through the frontier loop with
//! a scripted frontier and an injected approver, and through the `duet` binary
//! (a run with approval on and no terminal is refused; `audit disclosure`).
//! Nothing here contacts a model server.

use bytes::Bytes;
use duet_agent::disclosure::Disclosure;
use duet_agent::oversight::Action;
use duet_agent::{ApproveMode, Approver, Oversight, RunConfig, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{AuditEvent, AuditLog, Line, read};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::process::{Command, Output, Stdio};
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

/// Denies everything and remembers what it was asked.
#[derive(Default)]
struct Deny(Mutex<Vec<Action>>);

impl Approver for Deny {
    fn approve(&self, action: &Action) -> bool {
        self.0.lock().unwrap().push(action.clone());
        false
    }
}

fn workspace(root: &Path) {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join(".env"), format!("STRIPE_KEY={SECRET}\n")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,email\n1,{EMAIL}\n"),
    )
    .unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_write_is_a_tool_error_and_the_report_counts_what_was_withheld() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    workspace(&root);
    let ws = root.join("ws");
    let run_dir = root.join("run");
    let script = vec![
        ("read_file", json!({"path": ".env"})),
        ("read_file", json!({"path": "data/customers.csv"})),
        (
            "write_file",
            json!({"path": "Cargo.toml", "content": "[package]\nname = \"x\"\n"}),
        ),
        (
            "write_file",
            json!({"path": "src/b.rs", "content": "pub fn b() {}\n"}),
        ),
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
    let policy = Policy {
        sensitive_globs: vec![".env*".into(), "data/**".into()],
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        ..Policy::default()
    };
    let engine = Engine::open(&run_dir, policy, None).unwrap();
    let files = [".env".to_owned(), "data/customers.csv".to_owned()];
    engine.prime(&ws, &files, "");
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
    let deny = Arc::new(Deny::default());
    let cfg = RunConfig {
        workspace: ws.clone(),
        run_dir,
        objective: "Add a package manifest.".into(),
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
            mode: ApproveMode::Risky,
            approver: Some(deny.clone()),
            ..Oversight::default()
        },
        web: None,
        git_author: None,
        mcp: None,
        lsp: None,
        subagents: None,
    };
    let git = duet_git::Git::locate().unwrap();
    let (terminal, stats) = duet_agent::run(
        &cfg,
        &gated,
        engine.as_ref(),
        &git,
        false,
        &std::sync::Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );

    // Only the manifest write needed approval; it was refused and not made,
    // and the frontier was told so; the source write went ahead.
    let asked = deny.0.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].path.as_deref(), Some("Cargo.toml"));
    assert!(!ws.join("Cargo.toml").exists());
    assert!(ws.join("src/b.rs").exists());
    let bodies = frontier.bodies.lock().unwrap().clone();
    assert!(bodies[3].contains("did not approve"), "{}", bodies[3]);

    let lines = read(&log).unwrap();
    let approvals: Vec<&AuditEvent> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Event(e) if e.event.kind() == "approval" => Some(&e.event),
            _ => None,
        })
        .collect();
    assert_eq!(
        approvals,
        [&AuditEvent::Approval {
            tool: "write_file".into(),
            risk: "write_outside_sources".into(),
            path: Some("Cargo.toml".into()),
            approved: false,
            decided_by: "operator".into(),
        }]
    );

    let report = Disclosure::build(&lines, Some(&stats.ledger));
    assert!(report.boundary);
    assert_eq!(report.requests, 5);
    assert_eq!(report.placeholders.get("secret"), Some(&1), "{report:?}");
    let views = report.views.clone().unwrap();
    assert_eq!((views.tokenized, views.handle_summary), (1, 1), "{views:?}");
    assert_eq!((report.approvals.asked, report.approvals.denied), (1, 1));
    let shown = format!(
        "{}{}",
        report.render("r"),
        serde_json::to_string(&report).unwrap()
    );
    let log_text = std::fs::read_to_string(&log).unwrap();
    for text in [&shown, &log_text, &bodies.concat()] {
        assert!(!text.contains(SECRET) && !text.contains(EMAIL));
    }
}

struct Env {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    ws: std::path::PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, ws) = (root.join("owner"), root.join("ws"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    Env {
        _dir: dir,
        home,
        ws,
    }
}

fn duet(e: &Env, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_duet"))
        .args(args)
        .arg("--workspace")
        .arg(&e.ws)
        .env("DUET_CONFIG_HOME", &e.home)
        .env_remove("ZAI_API_KEY")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn with_approval_on_a_run_without_a_terminal_is_refused_before_it_starts() {
    let e = env();
    for mode in ["risky", "all"] {
        std::fs::write(
            e.home.join("config.toml"),
            format!("[oversight]\napprove = \"{mode}\"\n"),
        )
        .unwrap();
        for args in [
            vec!["run", "do it"],
            vec!["run", "--mode", "passthrough", "--no-privacy", "do it"],
        ] {
            let o = duet(&e, &args);
            assert!(!o.status.success(), "{}", text(&o));
            let out = text(&o);
            assert!(
                out.contains("not a terminal") && out.contains("was not started"),
                "{out}"
            );
            assert!(!e.ws.join(".duet/runs").exists(), "a run was created");
        }
    }
    // A project cannot turn approval on (or off) for the owner.
    std::fs::remove_file(e.home.join("config.toml")).unwrap();
    std::fs::create_dir_all(e.ws.join(".duet")).unwrap();
    std::fs::write(
        e.ws.join(".duet/config.toml"),
        "[oversight]\napprove = \"off\"\n",
    )
    .unwrap();
    let o = duet(&e, &["config", "get", "oversight.approve"]);
    assert!(
        !o.status.success() && text(&o).contains("owner"),
        "{}",
        text(&o)
    );
}

#[test]
fn audit_disclosure_reports_counts_from_the_log_and_the_summary() {
    let e = env();
    let log = e.ws.join(".duet/audit/r1.jsonl");
    let mut a = AuditLog::open(&log).unwrap();
    a.event(AuditEvent::RunStart {
        mode: "hybrid".into(),
        boundary: true,
    })
    .unwrap();
    let body = json!({"messages": [
        {"role": "user", "content": "fix ⟨secret:DB_URL#1⟩"},
        {"role": "tool", "tool_call_id": "c1", "content": "⟨email:email#1⟩ ⟨redacted:copied-sensitive-text⟩"}
    ]});
    a.append("https://f/v1", "m", body, vec!["sanitize: x".into()])
        .unwrap();
    a.event(AuditEvent::SandboxDenial {
        command: "cat data/x.csv".into(),
        access: "ordinary".into(),
    })
    .unwrap();
    let run = e.ws.join(".duet/runs/r1");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(
        run.join("summary.json"),
        json!({"stats": {"ledger": {"by_class": {"handle_summary": {"results": 2,
            "added_tokens": 1, "carried_tokens": 1, "input_usd": 0.0}},
            "request_tokens": 1, "ask_local_calls": 4, "ask_local_questions": 4,
            "sensitive_data_commands": 0, "sandbox_denials": 1,
            "input_usd": 0.0, "output_usd": 0.0}}})
        .to_string(),
    )
    .unwrap();

    let o = duet(&e, &["audit", "disclosure", "r1"]);
    assert!(o.status.success(), "{}", text(&o));
    let out = text(&o);
    for shown in [
        "placeholders: email",
        "placeholders: secret",
        "copied spans of sensitive text removed",
        "sensitive results held locally (handles)",
        "sandbox denials",
    ] {
        assert!(out.contains(shown), "{shown}: {out}");
    }
    assert!(
        !out.contains("DB_URL") && !out.contains("data/x.csv"),
        "{out}"
    );

    let o = duet(&e, &["audit", "disclosure", "r1", "--json"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["placeholders"]["secret"], 1);
    assert_eq!(v["copied_spans_redacted"], 1);
    assert_eq!(v["views"]["handle_summary"], 2);
    assert_eq!(v["ask_local_calls"], 4);
    assert_eq!(v["sandbox_denials"], 1);

    assert!(!duet(&e, &["audit", "disclosure", "../r1"]).status.success());
    assert!(!duet(&e, &["audit", "disclosure", "nope"]).status.success());
}
