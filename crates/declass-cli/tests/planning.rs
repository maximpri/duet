// SPDX-License-Identifier: GPL-3.0-or-later
//! Plan mode is enforced by the host even when a provider calls hidden tools.
//! All provider responses are scripted in process; no model service is used.

use bytes::Bytes;
use declass_agent::session::{SessionLimits, TurnEnd};
use declass_agent::transcript::{Entry, Transcript};
use declass_agent::{RunConfig, Session};
use declass_boundary::OutboundGate;
use declass_boundary::audit::AuditLog;
use declass_boundary::model::ToolSpec;
use declass_boundary::view::{PassThrough, Presenter, Source};
use declass_provider::client::{HttpReply, Transport};
use declass_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct Frontier {
    calls: Arc<Mutex<VecDeque<(&'static str, Value)>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Frontier {
    fn script(&self, calls: impl IntoIterator<Item = (&'static str, Value)>) {
        self.calls.lock().unwrap().extend(calls);
    }

    fn last(&self) -> Value {
        self.requests.lock().unwrap().last().unwrap().clone()
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let mut requests = self.requests.lock().unwrap();
        requests.push(serde_json::from_slice(&body).unwrap());
        let id = format!("call{}", requests.len());
        let (name, args) = self
            .calls
            .lock()
            .unwrap()
            .pop_front()
            .expect("script exhausted");
        let delta = json!({"tool_calls": [{"index": 0, "id": id, "type": "function",
            "function": {"name": name, "arguments": args.to_string()}}]});
        let chunks = [
            format!("data: {}\n\n", json!({"choices": [{"delta": delta}]})),
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n".into(),
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":10}}\n\n"
                .into(),
            "data: [DONE]\n\n".into(),
        ];
        Box::pin(async move {
            Ok(HttpReply {
                status: 200,
                headers: Vec::new(),
                body: futures_util::stream::iter(chunks.into_iter().map(|s| Ok(Bytes::from(s))))
                    .boxed(),
            })
        })
    }
}

/// An installed extension with a visible side effect if dispatch reaches it.
/// Its deliberately unfamiliar name also tests that plan mode fails closed
/// for tools added after the built-in allowlist was written.
#[derive(Default)]
struct Extension {
    calls: AtomicUsize,
}

impl Presenter for Extension {
    fn present(&self, source: &Source, bytes: &[u8]) -> String {
        PassThrough { max_bytes: 60_000 }.present(source, bytes)
    }

    fn extra_tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "future_mutating_extension".into(),
            description: "A test extension with a side effect.".into(),
            parameters: json!({"type": "object", "properties": {}}),
        }]
    }

    fn call_tool(&self, name: &str, _args: &Map<String, Value>) -> Option<Result<String, String>> {
        (name == "future_mutating_extension").then(|| {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok("extension executed".into())
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    cfg: RunConfig,
    frontier: Frontier,
    gated: declass_boundary::GatedFrontier,
    presenter: Extension,
    git: declass_git::Git,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let ws = root.join("workspace");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(ws.join("original.txt"), "original planning evidence\n").unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&ws)
                .status()
                .unwrap()
                .success()
        );
        let frontier = Frontier::default();
        let mut provider =
            ProviderConfig::new("https://frontier.example/v1", "test", Role::Frontier);
        provider.backoff_scale = 0.0;
        let gated = OutboundGate::new(AuditLog::open(&root.join("audit.jsonl")).unwrap())
            .wrap(ChatProvider::new(provider, Box::new(frontier.clone())).unwrap());
        let mut cfg = RunConfig::new(ws.clone(), root.join("run"), "Plan the change.");
        cfg.checks = vec!["printf checked > checks-ran.txt && test -f implemented.txt".into()];
        let git = declass_git::Git::locate().unwrap();
        Self {
            _dir: dir,
            ws,
            cfg,
            frontier,
            gated,
            presenter: Extension::default(),
            git,
        }
    }

    fn open(&self, resume: bool) -> Result<Session<'_>, String> {
        Session::open(
            &self.cfg,
            &self.gated,
            &self.presenter,
            &self.git,
            Arc::new(AtomicBool::new(false)),
            SessionLimits {
                frontier_usd: 100.0,
                working_time: Duration::from_secs(300),
            },
            resume,
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn forced_mutations_and_finish_are_blocked_but_workspace_reads_work() {
    let f = Fixture::new();
    f.frontier.script([
        (
            "write_file",
            json!({"path":"written.txt", "content":"changed"}),
        ),
        (
            "edit_file",
            json!({"path":"original.txt", "edits":[{"old":"original", "new":"modified"}]}),
        ),
        (
            "run_command",
            json!({"command":"printf escaped > command-ran.txt"}),
        ),
        (
            "finish",
            json!({"summary":"Execute checks despite planning."}),
        ),
        ("future_mutating_extension", json!({})),
        ("delegate", json!({"task":"Write a file."})),
        ("explore", json!({"question":"Execute a command."})),
        (
            "code_nav",
            json!({"action":"symbols", "path":"original.txt"}),
        ),
        (
            "git_commit",
            json!({"message":"Bypass plan", "paths":["original.txt"]}),
        ),
        (
            "edit_protected",
            json!({"path":"original.txt", "request":"Replace the contents."}),
        ),
        (
            "rename",
            json!({"path":"original.txt", "new_name":"modified"}),
        ),
        ("web_fetch", json!({"url":"https://example.invalid/"})),
        ("web_search", json!({"query":"bypass plan"})),
        (
            "mcp__test__write",
            json!({"path":"written.txt", "content":"changed"}),
        ),
        ("read_file", json!({"path":"original.txt"})),
        ("list_files", json!({})),
        ("search", json!({"pattern":"planning evidence"})),
        ("diff", json!({})),
        (
            "reply",
            json!({"message":"Plan ready; no implementation performed."}),
        ),
    ]);
    let mut s = f.open(false).unwrap();
    s.set_planning(true).unwrap();
    assert!(s.is_planning());
    assert!(matches!(
        s.turn("Plan only.").await,
        TurnEnd::Replied { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(f.ws.join("original.txt")).unwrap(),
        "original planning evidence\n"
    );
    for file in ["written.txt", "command-ran.txt", "checks-ran.txt"] {
        assert!(!f.ws.join(file).exists(), "planning created {file}");
    }
    assert_eq!(f.presenter.calls.load(Ordering::SeqCst), 0);
    let last = f.frontier.last();
    let results: Vec<_> = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert_eq!(results.len(), 18);
    for blocked in &results[..14] {
        assert!(
            blocked.to_lowercase().contains("plan"),
            "unexpected refusal: {blocked}"
        );
    }
    assert!(results[14].contains("original planning evidence"));
    assert!(results[15].contains("original.txt"));
    assert!(results[16].contains("planning evidence"));
    for body in f.frontier.requests.lock().unwrap().iter() {
        let offered: Vec<_> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap())
            .collect();
        assert!(offered.contains(&"read_file") && offered.contains(&"reply"));
        for forbidden in [
            "write_file",
            "edit_file",
            "run_command",
            "finish",
            "future_mutating_extension",
            "delegate",
            "explore",
            "code_nav",
            "git_commit",
            "edit_protected",
            "rename",
            "web_fetch",
            "web_search",
            "mcp__test__write",
        ] {
            assert!(
                !offered.contains(&forbidden),
                "planning advertised {forbidden}"
            );
        }
    }
    assert!(
        !Transcript::read(&f.cfg.run_dir)
            .unwrap()
            .iter()
            .any(|e| matches!(
                e,
                Entry::TurnEnd {
                    end: TurnEnd::Completed { .. },
                    ..
                }
            ))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn planning_survives_resume_and_only_explicit_off_enables_writes_and_checks() {
    let f = Fixture::new();
    f.frontier.script([
        ("reply", json!({"message":"Plan ready."})),
        (
            "write_file",
            json!({"path":"implemented.txt", "content":"too early"}),
        ),
        ("reply", json!({"message":"Still planning."})),
        ("future_mutating_extension", json!({})),
        (
            "write_file",
            json!({"path":"implemented.txt", "content":"approved change"}),
        ),
        ("finish", json!({"summary":"Implemented and checked."})),
    ]);
    let mut s = f.open(false).unwrap();
    assert!(!s.is_planning());
    s.set_planning(true).unwrap();
    assert_eq!(s.turn("Plan the change.").await.state(), "replied");
    s.end(false);
    let mut s = f.open(true).unwrap();
    assert!(s.is_planning());
    assert_eq!(
        s.turn("Continue discussing the plan.").await.state(),
        "replied"
    );
    assert!(!f.ws.join("implemented.txt").exists());
    assert!(!f.ws.join("checks-ran.txt").exists());
    let before = f.frontier.requests.lock().unwrap().len();
    s.set_planning(false).unwrap();
    assert!(!s.is_planning());
    assert_eq!(
        f.frontier.requests.lock().unwrap().len(),
        before,
        "switching mode must not call the model"
    );
    assert!(
        !f.ws.join("implemented.txt").exists(),
        "switching mode must not implement the plan"
    );
    // Persist the explicit off transition before any execution, too.
    s.end(false);
    let mut s = f.open(true).unwrap();
    assert!(!s.is_planning());
    assert_eq!(
        s.turn("Implement the change now.").await,
        TurnEnd::Completed {
            summary: "Implemented and checked.".into()
        }
    );
    assert_eq!(
        std::fs::read_to_string(f.ws.join("implemented.txt")).unwrap(),
        "approved change"
    );
    assert_eq!(
        std::fs::read_to_string(f.ws.join("checks-ran.txt")).unwrap(),
        "checked"
    );
    assert_eq!(f.presenter.calls.load(Ordering::SeqCst), 1);
    let transitions: Vec<_> = Transcript::read(&f.cfg.run_dir)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            Entry::PlanMode { enabled } => Some(enabled),
            _ => None,
        })
        .collect();
    assert_eq!(transitions, [true, false]);
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_a_saved_plan_defers_pending_recovery_until_explicit_off() {
    let f = Fixture::new();
    let mut s = f.open(false).unwrap();
    s.set_planning(true).unwrap();
    s.end(false);
    // Reproduce a crash after a pending write, before its applied record.
    std::fs::create_dir_all(f.cfg.run_dir.join("writes")).unwrap();
    std::fs::write(
        f.cfg.run_dir.join("writes/1.before"),
        "before pending write",
    )
    .unwrap();
    std::fs::write(f.ws.join("original.txt"), "workspace at crash").unwrap();
    std::fs::write(
        f.cfg.run_dir.join("writes.jsonl"),
        "{\"state\":\"pending\",\"n\":1,\"path\":\"original.txt\",\"existed\":true}\n",
    )
    .unwrap();
    let journal_before = std::fs::read(f.cfg.run_dir.join("writes.jsonl")).unwrap();
    let mut s = f.open(true).unwrap();
    assert!(s.is_planning());
    assert!(matches!(
        s.turn("Inspect the plan.").await,
        TurnEnd::Failed { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(f.ws.join("original.txt")).unwrap(),
        "workspace at crash"
    );
    assert_eq!(
        std::fs::read(f.cfg.run_dir.join("writes.jsonl")).unwrap(),
        journal_before
    );
    assert!(f.frontier.requests.lock().unwrap().is_empty());
    s.set_planning(false).unwrap();
    assert!(
        s.set_planning(true).is_err(),
        "pending execution recovery must prevent re-entering planning"
    );
    assert!(!s.is_planning());
    f.frontier.script([
        ("read_file", json!({"path":"original.txt"})),
        ("reply", json!({"message":"Recovery complete."})),
    ]);
    assert_eq!(s.turn("Continue execution.").await.state(), "replied");
    assert_eq!(
        std::fs::read_to_string(f.ws.join("original.txt")).unwrap(),
        "before pending write"
    );
    assert!(
        f.frontier
            .last()
            .to_string()
            .contains("before pending write")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn planning_blocks_undo_of_an_earlier_execution_turn() {
    let f = Fixture::new();
    f.frontier.script([
        (
            "write_file",
            json!({"path":"implemented.txt", "content":"earlier change"}),
        ),
        ("reply", json!({"message":"Change prepared."})),
    ]);
    let mut s = f.open(false).unwrap();
    assert_eq!(s.turn("Make this change.").await.state(), "replied");
    s.set_planning(true).unwrap();
    let before = std::fs::read(f.cfg.run_dir.join("writes.jsonl")).unwrap();
    assert!(s.undo().unwrap_err().contains("plan"));
    assert_eq!(
        std::fs::read_to_string(f.ws.join("implemented.txt")).unwrap(),
        "earlier change"
    );
    assert_eq!(
        std::fs::read(f.cfg.run_dir.join("writes.jsonl")).unwrap(),
        before
    );
    s.set_planning(false).unwrap();
    let (_, restored) = s.undo().unwrap();
    assert_eq!(restored, [PathBuf::from("implemented.txt")]);
    assert!(!f.ws.join("implemented.txt").exists());
}
