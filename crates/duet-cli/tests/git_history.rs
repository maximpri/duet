// SPDX-License-Identifier: GPL-3.0-or-later
//! The git tools through the frontier loop with a scripted frontier: in a
//! repository whose history holds a removed key, a sensitive file, sealed
//! source and an author email, nothing of them reaches the frontier; the tool
//! set is fixed and sorted; a commit waits for the operator's approval and is
//! audited. Nothing here contacts a model server.

use bytes::Bytes;
use duet_agent::git_tools::CommitPolicy;
use duet_agent::oversight::Action;
use duet_agent::session::SessionLimits;
use duet_agent::{ApproveMode, Approver, Oversight, RunConfig, Session, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{AuditEvent, AuditLog, Line, read};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_git::{Git, Identity};
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

const KEY: &str = "sk_live_4hT9wQ2mV7xL1pR8zK3nB6cY";
const EMAIL: &str = "joran.feldspar77@inbox-904.net";
const OLD_TOTAL: &str = "7340961";
const TOTAL: &str = "6219488";
const SEALED: &str = "5150515";

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

/// Approves everything and remembers what it was asked.
#[derive(Default)]
struct Approve(Mutex<Vec<Action>>);

impl Approver for Approve {
    fn approve(&self, action: &Action) -> bool {
        self.0.lock().unwrap().push(action.clone());
        true
    }
}

fn put(ws: &Path, path: &str, text: &str) {
    let p = ws.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn repo(ws: &Path, git: &Git) {
    let run = |args: &[&str]| {
        git.run(
            ws,
            args,
            &[
                ("GIT_AUTHOR_NAME", "Joran Feldspar"),
                ("GIT_AUTHOR_EMAIL", EMAIL),
            ],
            None,
        )
        .unwrap();
    };
    run(&["init", "-q", "-b", "main"]);
    git.exclude_state_dir(ws).unwrap();
    put(
        ws,
        "src/lib.rs",
        &format!("pub const KEY: &str = \"{KEY}\";\n"),
    );
    put(
        ws,
        "data/ledger.csv",
        &format!("account,total\nA-1,{OLD_TOTAL}\n"),
    );
    put(
        ws,
        "src/core.rs",
        &format!("pub fn k() -> u32 {{ {SEALED} }}\n"),
    );
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "Import ledger tooling"]);
    put(
        ws,
        "src/lib.rs",
        "pub fn key() -> Option<String> { std::env::var(\"K\").ok() }\n",
    );
    put(
        ws,
        "data/ledger.csv",
        &format!("account,total\nA-1,{TOTAL}\n"),
    );
    run(&["add", "-A"]);
    run(&["commit", "-q", "-m", "Move the key to the environment"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_reaches_the_frontier_only_through_the_boundary() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let git = Git::locate().unwrap();
    repo(&ws, &git);
    let run_dir = ws.join(".duet/runs/r1");
    std::fs::create_dir_all(&run_dir).unwrap();
    let script = vec![
        ("git_log", json!({})),
        ("git_show", json!({"rev": "HEAD~1"})),
        ("git_show", json!({"rev": "HEAD"})),
        (
            "git_show",
            json!({"rev": "HEAD~1", "path": "data/ledger.csv"}),
        ),
        ("git_blame", json!({"path": "data/ledger.csv"})),
        ("git_log", json!({"path": "src/core.rs"})),
        ("run_command", json!({"command": "git show HEAD~1"})),
        (
            "write_file",
            json!({"path": "src/report.rs", "content": "pub fn report() {}\n"}),
        ),
        (
            "git_commit",
            json!({"message": "Add the report module", "paths": ["src/report.rs"]}),
        ),
        ("git_status", json!({})),
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
        detect_entropy: true,
        bulky_tokens: 4000,
        bulky_file_tokens: 12000,
        sealed: vec!["src/core.rs".into()],
        ..Policy::default()
    };
    let engine = Engine::open(&run_dir, policy, None).unwrap();
    engine.prime(&ws, &git.list_files(&ws).unwrap(), "Add a report module.");
    let log = root.join("audit.jsonl");
    let (filter, check) = engine.outbound();
    let gated = OutboundGate::new(AuditLog::open(&log).unwrap())
        .with_filter(filter)
        .with_check(check)
        .wrap(provider);
    let approver = Arc::new(Approve::default());
    let cfg = RunConfig {
        mode: "hybrid".into(),
        wall_clock: Duration::from_secs(120),
        price: Box::new(|u| u.input as f64 / 1e6),
        oversight: Oversight {
            mode: ApproveMode::Risky,
            approver: Some(approver.clone()),
            git_commit: CommitPolicy::Ask,
        },
        git_author: Identity::parse("Olive Operator <olive@example.test>"),
        ..RunConfig::new(ws.clone(), run_dir, "Add a report module.")
    };
    let (terminal, _stats) = duet_agent::run(
        &cfg,
        &gated,
        engine.as_ref(),
        &git,
        false,
        &Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let bodies = frontier.bodies.lock().unwrap().clone();
    // Nothing sensitive reached the frontier, in any request.
    let all = bodies.concat();
    for secret in [KEY, EMAIL, OLD_TOTAL, TOTAL, SEALED] {
        assert!(!all.contains(secret), "{secret} reached the frontier");
    }
    // The tool set is the same in every request, sorted, with the git tools.
    let tools = |b: &str| -> Vec<String> {
        let v: Value = serde_json::from_str(b).unwrap();
        v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let first = tools(&bodies[0]);
    let mut sorted = first.clone();
    sorted.sort();
    assert_eq!(first, sorted);
    for name in duet_agent::git_tools::NAMES {
        assert!(first.iter().any(|t| t == name), "{name} offered");
    }
    assert!(bodies.iter().all(|b| tools(b) == first));
    // What the frontier saw: the removed key's diff line, a handle for the
    // data file's history, nothing of sealed source, and the hint for `git`.
    assert!(bodies[3].contains("-pub const KEY"), "{}", bodies[3]);
    assert!(bodies[4].contains("ask_local"), "{}", bodies[4]);
    assert!(bodies[6].contains("not available"), "{}", bodies[6]);
    assert!(
        bodies[7].contains("use the git_status, git_log"),
        "{}",
        bodies[7]
    );
    assert!(!all.contains("core.rs\\n") && !all.contains("M  src/core.rs"));

    // The commit was approved by the operator (who saw the message) and audited.
    let asked = approver.0.lock().unwrap().clone();
    let commit: Vec<&Action> = asked.iter().filter(|a| a.tool == "git_commit").collect();
    assert_eq!(commit.len(), 1, "{asked:?}");
    assert_eq!(commit[0].command.as_deref(), Some("Add the report module"));
    let head = git.head(&ws).unwrap().unwrap();
    let out = git
        .run(
            &ws,
            &["log", "-1", "--format=%an|%s", "--name-only"],
            &[],
            None,
        )
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out).trim(),
        "Olive Operator|Add the report module\n\nsrc/report.rs"
    );
    let events: Vec<AuditEvent> = read(&log)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) => Some(e.event),
            Line::Request(_) => None,
        })
        .collect();
    assert!(events.contains(&AuditEvent::GitCommit {
        hash: head,
        paths: vec!["src/report.rs".into()],
    }));
    let log_text = std::fs::read_to_string(&log).unwrap();
    for secret in [KEY, EMAIL, OLD_TOTAL, TOTAL, SEALED] {
        assert!(!log_text.contains(secret), "{secret} in the audit log");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_commits_its_own_files_and_undo_never_touches_history() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let git = Git::locate().unwrap();
    repo(&ws, &git);
    let base = git.head(&ws).unwrap().unwrap();
    let run_dir = ws.join(".duet/runs/s1");
    let script = vec![
        (
            "write_file",
            json!({"path": "src/report.rs", "content": "pub fn report() {}\n"}),
        ),
        ("git_commit", json!({"message": "Add the report module"})),
        ("reply", json!({"message": "Committed."})),
        ("reply", json!({"message": "Understood."})),
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
    let gated =
        OutboundGate::new(AuditLog::open(&root.join("audit.jsonl")).unwrap()).wrap(provider);
    let cfg = RunConfig {
        wall_clock: Duration::from_secs(120),
        price: Box::new(|u| u.input as f64 / 1e6),
        oversight: Oversight {
            git_commit: CommitPolicy::Allow,
            ..Oversight::default()
        },
        git_author: Identity::parse("Olive Operator <olive@example.test>"),
        ..RunConfig::new(ws.clone(), run_dir, "Add a report module and commit it.")
    };
    let presenter = duet_boundary::view::PassThrough { max_bytes: 60_000 };
    let limits = SessionLimits {
        frontier_usd: 10.0,
        working_time: Duration::from_secs(600),
    };
    let mut s = Session::open(
        &cfg,
        &gated,
        &presenter,
        &git,
        Arc::new(AtomicBool::new(false)),
        limits,
        false,
    )
    .unwrap();
    s.turn("Add a report module and commit it.").await;
    let head = git.head(&ws).unwrap().unwrap();
    assert_ne!(head, base, "the session committed");
    // Undo reverts the turn's files; the commit stays in history.
    let (_, paths) = s.undo().unwrap();
    assert_eq!(paths, [std::path::PathBuf::from("src/report.rs")]);
    assert!(!ws.join("src/report.rs").exists());
    assert_eq!(git.head(&ws).unwrap().unwrap(), head);
    s.turn("Thanks.").await;
    let bodies = frontier.bodies.lock().unwrap().clone();
    let last = bodies.last().unwrap();
    assert!(last.contains("Commits are never undone"), "{last}");
    let _ = s.end(true);
}
