// SPDX-License-Identifier: GPL-3.0-or-later
//! Sessions through the frontier loop with a scripted frontier (no model
//! server): turns end on replies, questions and finished tasks with the
//! context kept; operator messages are sanitized and audited like task text,
//! the sensitive-path note is given once; per-turn and session budgets;
//! an interrupted turn leaves the session open; undo reverts a turn's writes;
//! and a session resumes from its transcript.

use bytes::Bytes;
use duet_agent::session::{SESSION_LEFT, SessionLimits, TurnEnd};
use duet_agent::transcript::{Entry, Transcript};
use duet_agent::{RunConfig, Session, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{AuditEvent, AuditLog, Line, read};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::view::{PassThrough, Presenter};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SECRET: &str = "sk_live_9f8e7d6c5b4a39281706";
const EMAIL: &str = "marta.kowalczyk@corp-mail.net";

/// What the scripted frontier answers one request with.
enum Step {
    Call(&'static str, Value),
    /// A message without tool calls.
    Text(&'static str),
    /// `reply` quoting the first placeholder in the request.
    EchoPlaceholder,
    /// Never answers.
    Hang,
}

#[derive(Clone, Default)]
struct Frontier {
    steps: Arc<Mutex<VecDeque<Step>>>,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Frontier {
    fn script(&self, steps: Vec<Step>) {
        self.steps.lock().unwrap().extend(steps);
    }

    fn body(&self, i: usize) -> Value {
        serde_json::from_str(&self.bodies.lock().unwrap()[i]).unwrap()
    }

    fn last(&self) -> Value {
        let n = self.bodies.lock().unwrap().len();
        self.body(n - 1)
    }

    fn count(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }
}

fn first_placeholder(text: &str) -> Option<String> {
    let start = text.find('⟨')?;
    let end = start + text[start..].find('⟩')? + '⟩'.len_utf8();
    Some(text[start..end].to_owned())
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let text = String::from_utf8(body).unwrap();
        self.bodies.lock().unwrap().push(text.clone());
        let n = self.count();
        let step = self.steps.lock().unwrap().pop_front().expect("unscripted");
        let delta = match step {
            Step::Hang => return Box::pin(futures_util::future::pending()),
            Step::Text(t) => json!({"content": t}),
            Step::Call(name, args) => json!({"tool_calls": [{"index": 0, "id": format!("c{n}"),
                "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}),
            Step::EchoPlaceholder => {
                let body: Value = serde_json::from_str(&text).unwrap();
                let last = user_texts(&body).pop().unwrap_or_default();
                let token = first_placeholder(&last).expect("a placeholder in the message");
                let args = json!({"message": format!("I will not contact {token} directly.")});
                json!({"tool_calls": [{"index": 0, "id": format!("c{n}"),
                    "type": "function", "function": {"name": "reply", "arguments": args.to_string()}}]})
            }
        };
        let finish = if delta.get("content").is_some() {
            "stop"
        } else {
            "tool_calls"
        };
        let chunks = [
            format!("data: {}\n\n", json!({"choices": [{"delta": delta}]})),
            format!(
                "data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"{finish}\"}}]}}\n\n"
            ),
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

struct Fixture {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    run_dir: PathBuf,
    log: PathBuf,
    frontier: Frontier,
    gated: duet_boundary::GatedFrontier,
    engine: Option<Arc<Engine>>,
    cfg: RunConfig,
    git: duet_git::Git,
    interrupted: Arc<AtomicBool>,
}

/// A workspace with a public source file and sensitive data; `hybrid` puts
/// the security engine in front of the frontier.
fn fixture(hybrid: bool, turn_usd: f64) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(ws.join(".env"), format!("STRIPE_KEY={SECRET}\n")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,email\n1,{EMAIL}\n"),
    )
    .unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&ws)
            .status()
            .unwrap()
            .success()
    );
    let run_dir = ws.join(".duet/runs/s1");
    let frontier = Frontier::default();
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let log = root.join("audit.jsonl");
    let mut gate = OutboundGate::new(AuditLog::open(&log).unwrap());
    let engine = hybrid.then(|| {
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
        engine.prime(&ws, &files, "Add functions to the library.");
        engine
    });
    if let Some(e) = &engine {
        let (filter, check) = e.outbound();
        gate = gate.with_filter(filter).with_check(check);
    }
    let gated = gate.wrap(provider);
    let cfg = RunConfig {
        workspace: ws.clone(),
        run_dir: run_dir.clone(),
        objective: "Add functions to the library.".into(),
        mode: if hybrid { "hybrid" } else { "passthrough" }.into(),
        checks: vec![],
        sandbox: duet_sandbox::detect().unwrap_or(duet_sandbox::SandboxKind::Seatbelt),
        network: false,
        command_timeout: Duration::from_secs(30),
        wall_clock: Duration::from_secs(60),
        frontier_usd: turn_usd,
        max_finish_attempts: 2,
        context_window: 200_000,
        mask_at: 0.7,
        max_output_tokens: 1000,
        reasoning_effort: None,
        // 1000 prompt tokens per request: $0.001 each.
        price: Box::new(|u| u.input as f64 / 1e6),
        oversight: Default::default(),
    };
    Fixture {
        _dir: dir,
        ws,
        run_dir,
        log,
        frontier,
        gated,
        engine,
        cfg,
        git: duet_git::Git::locate().unwrap(),
        interrupted: Arc::new(AtomicBool::new(false)),
    }
}

const PASS: PassThrough = PassThrough { max_bytes: 60_000 };

impl Fixture {
    fn presenter(&self) -> &dyn Presenter {
        match &self.engine {
            Some(e) => e.as_ref(),
            None => &PASS,
        }
    }

    fn open(&self, limits: SessionLimits, resume: bool) -> Session<'_> {
        Session::open(
            &self.cfg,
            &self.gated,
            self.presenter(),
            &self.git,
            self.interrupted.clone(),
            limits,
            resume,
        )
        .unwrap()
    }
}

const ROOMY: SessionLimits = SessionLimits {
    frontier_usd: 100.0,
    working_time: Duration::from_secs(3600),
};

/// The texts of the user messages in a request body.
fn user_texts(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str().map(str::to_owned))
        .collect()
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn turns_end_on_replies_questions_and_finish_and_keep_the_context() {
    let f = fixture(false, 10.0);
    f.frontier.script(vec![
        Step::Call(
            "write_file",
            json!({"path": "src/b.rs", "content": "pub fn b() {}\n"}),
        ),
        Step::Call("reply", json!({"message": "Added b in src/b.rs."})),
        // Turn 2: a question ends the turn.
        Step::Call(
            "ask_operator",
            json!({"question": "Should c be public?", "options": ["public", "private"]}),
        ),
        // Turn 3: the answer continues the work; finish ends the turn.
        Step::Call(
            "write_file",
            json!({"path": "src/c.rs", "content": "pub fn c() {}\n"}),
        ),
        Step::Call("finish", json!({"summary": "Added a public c."})),
        // Turn 4: a message without tool calls is the reply.
        Step::Text("Both functions are in place."),
    ]);
    let mut s = f.open(ROOMY, false);
    let end = s.turn("Add a function b.").await;
    assert_eq!(
        end,
        TurnEnd::Replied {
            message: "Added b in src/b.rs.".into()
        }
    );
    let end = s.turn("Now add a function c.").await;
    assert_eq!(
        end,
        TurnEnd::Asked {
            question: "Should c be public?\noptions: public / private".into()
        }
    );
    assert_eq!(
        s.turn("public").await,
        TurnEnd::Completed {
            summary: "Added a public c.".into()
        }
    );
    assert_eq!(
        s.turn("Anything left?").await,
        TurnEnd::Replied {
            message: "Both functions are in place.".into()
        }
    );
    assert_eq!(s.turns(), 4);
    assert!(f.ws.join("src/b.rs").exists() && f.ws.join("src/c.rs").exists());

    // Every request carries the whole conversation so far.
    let last = f.frontier.last();
    let users = user_texts(&last);
    for m in [
        "Add a function b.",
        "Now add a function c.",
        "public",
        "Anything left?",
    ] {
        assert!(users.iter().any(|u| u == m), "{m} missing from {users:?}");
    }
    let tools = tool_names(&last);
    assert!(tools.contains(&"reply".into()) && tools.contains(&"ask_operator".into()));
    let system = last["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("conversation with"), "{system}");

    // The reply's tool result tells the frontier the turn ended.
    let second = serde_json::to_string(&f.frontier.body(2)).unwrap();
    assert!(second.contains("delivered to the operator"), "{second}");

    let (terminal, stats) = s.end(true);
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    assert_eq!(stats.turns, 6);
    let entries = Transcript::read(&f.run_dir).unwrap();
    let ends: Vec<&str> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::TurnEnd { end, .. } => Some(end.state()),
            _ => None,
        })
        .collect();
    assert_eq!(ends, ["replied", "asked", "completed", "replied"]);
    assert!(
        duet_agent::resumable(&f.run_dir).is_err(),
        "a closed session is over"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_one_shot_run_keeps_its_prompt_and_tools() {
    let f = fixture(false, 10.0);
    f.frontier.script(vec![Step::Call(
        "finish",
        json!({"summary": "nothing to do"}),
    )]);
    let (terminal, _) = duet_agent::run(
        &f.cfg,
        &f.gated,
        f.presenter(),
        &f.git,
        false,
        &f.interrupted,
    )
    .await;
    assert!(matches!(terminal, Terminal::Completed { .. }));
    let body = f.frontier.body(0);
    let tools = tool_names(&body);
    assert!(!tools.contains(&"reply".into()) && !tools.contains(&"ask_operator".into()));
    assert_eq!(
        body["messages"][0]["content"].as_str().unwrap(),
        duet_agent::prompt::system_prompt("ws", &[])
    );
    assert!(!duet_agent::session::is_session(&f.run_dir));
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_messages_cross_the_boundary_like_task_text() {
    let f = fixture(true, 10.0);
    f.frontier.script(vec![
        Step::Call("reply", json!({"message": "Ready."})),
        Step::EchoPlaceholder,
        Step::Call("reply", json!({"message": "Understood."})),
    ]);
    let mut s = f.open(ROOMY, false);
    assert_eq!(
        s.turn("Look at the library first.").await.state(),
        "replied"
    );
    let end = s
        .turn(&format!(
            "Customer {EMAIL} complained; the key {SECRET} must be rotated."
        ))
        .await;
    // The operator sees the real value in duet's reply; the frontier never did.
    assert_eq!(
        end,
        TurnEnd::Replied {
            message: format!("I will not contact {EMAIL} directly.")
        }
    );
    s.turn("Thanks.").await;
    let bodies = f.frontier.bodies.lock().unwrap().clone();
    for b in &bodies {
        assert!(!b.contains(EMAIL) && !b.contains(SECRET), "leaked: {b}");
    }
    // The sensitive-path note goes with the first message only.
    let last = f.frontier.last();
    let notes = user_texts(&last)
        .iter()
        .filter(|u| u.contains("Sensitive in this repository"))
        .count();
    assert_eq!(notes, 1, "{last}");
    let second_message = user_texts(&last)
        .into_iter()
        .find(|u| u.starts_with("Customer"))
        .unwrap();
    assert!(second_message.contains('⟨'), "{second_message}");

    // Each operator message is audited before the request that carries it.
    let lines = read(&f.log).unwrap();
    let events: Vec<(u64, usize)> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Event(e) => match &e.event {
                AuditEvent::OperatorMessage {
                    exchange,
                    placeholders,
                } => Some((*exchange, *placeholders)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[0], (1, 0));
    assert!(events[1].1 >= 2, "{events:?}");
    // The transcript keeps the operator's words locally, as typed.
    let entries = Transcript::read(&f.run_dir).unwrap();
    assert!(entries.iter().any(|e| matches!(e,
        Entry::TurnStart { message, .. } if message.contains(EMAIL))));
}

#[tokio::test(flavor = "multi_thread")]
async fn budgets_stop_turns_and_the_session_spends_its_own() {
    // Each request costs $0.001: the turn cap stops the second request of a
    // turn, the session cap the fifth request.
    let f = fixture(false, 0.0015);
    f.frontier.script(vec![
        Step::Call("list_files", json!({})),
        Step::Call("list_files", json!({})),
        Step::Call("list_files", json!({})),
        Step::Call("list_files", json!({})),
        Step::Call("list_files", json!({})),
    ]);
    let limits = SessionLimits {
        frontier_usd: 0.0045,
        working_time: Duration::from_secs(3600),
    };
    let mut s = f.open(limits, false);
    assert_eq!(
        s.turn("Look around.").await,
        TurnEnd::BudgetStopped {
            which: "limits.frontier_usd".into()
        }
    );
    // The session stays open: the next message gets a turn of its own.
    assert_eq!(
        s.turn("Keep going.").await,
        TurnEnd::BudgetStopped {
            which: "limits.frontier_usd".into()
        }
    );
    assert_eq!(
        s.turn("Once more.").await,
        TurnEnd::BudgetStopped {
            which: "session.frontier_usd".into()
        }
    );
    assert_eq!(s.spent(), Some("session.frontier_usd"));
    let requests = f.frontier.count();
    assert_eq!(
        s.turn("And again.").await,
        TurnEnd::BudgetStopped {
            which: "session.frontier_usd".into()
        }
    );
    assert_eq!(f.frontier.count(), requests, "no request once it is spent");
    let (terminal, _) = s.end(false);
    assert_eq!(
        terminal,
        Terminal::BudgetStopped {
            which: "session.frontier_usd".into()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_turn_leaves_the_session_open_and_it_resumes() {
    let f = fixture(false, 10.0);
    f.frontier.script(vec![
        Step::Call(
            "write_file",
            json!({"path": "src/b.rs", "content": "pub fn b() {}\n"}),
        ),
        Step::Hang,
        Step::Call("reply", json!({"message": "Checked; b is there."})),
    ]);
    let mut s = f.open(ROOMY, false);
    let flag = f.interrupted.clone();
    let stopper = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        flag.store(true, Ordering::SeqCst);
    });
    assert_eq!(s.turn("Add b.").await, TurnEnd::Interrupted);
    stopper.await.unwrap();
    // The next turn starts with the flag cleared, and the frontier is told.
    assert_eq!(s.turn("Is b there?").await.state(), "replied");
    let users = user_texts(&f.frontier.last());
    assert!(
        users
            .iter()
            .any(|u| u.contains("interrupted your previous turn") && u.ends_with("Is b there?")),
        "{users:?}"
    );
    let (terminal, stats) = s.end(false);
    assert_eq!(
        terminal,
        Terminal::Failed {
            reason: SESSION_LEFT.into()
        }
    );
    assert!(duet_agent::resumable(&f.run_dir).is_ok());
    assert!(duet_agent::session::is_session(&f.run_dir));

    // Resumed: the history, the counts and the context come back.
    f.frontier
        .script(vec![Step::Call("reply", json!({"message": "Still here."}))]);
    let mut s = f.open(ROOMY, true);
    assert_eq!(s.turns(), 2);
    assert_eq!(s.stats().turns, stats.turns);
    let history = s.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].end, Some(TurnEnd::Interrupted));
    assert_eq!(history[1].message, "Is b there?");
    assert_eq!(s.turn("Are you there?").await.state(), "replied");
    let users = user_texts(&f.frontier.last());
    assert_eq!(users.len(), 3, "{users:?}");
    assert!(users[0].starts_with("Add b."), "{users:?}");
    // The sensitive-path note is not repeated on resume (none here) and the
    // interrupted note was already delivered.
    assert_eq!(users[2], "Are you there?");
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_reverts_the_last_turns_writes_and_tells_the_frontier() {
    let f = fixture(false, 10.0);
    f.frontier.script(vec![
        Step::Call(
            "write_file",
            json!({"path": "src/b.rs", "content": "pub fn b() {}\n"}),
        ),
        Step::Call("reply", json!({"message": "b added."})),
        Step::Call(
            "edit_file",
            json!({"path": "src/lib.rs", "edits": [{"old": "pub fn a() {}", "new": "pub fn a() -> u8 { 1 }"}]}),
        ),
        Step::Call(
            "write_file",
            json!({"path": "src/c.rs", "content": "pub fn c() {}\n"}),
        ),
        Step::Call("reply", json!({"message": "a changed, c added."})),
        Step::Call("reply", json!({"message": "Understood."})),
    ]);
    let mut s = f.open(ROOMY, false);
    s.turn("Add b.").await;
    s.turn("Change a, add c.").await;
    let (turn, mut paths) = s.undo().unwrap();
    paths.sort();
    assert_eq!(turn, 2);
    assert_eq!(
        paths,
        [PathBuf::from("src/c.rs"), PathBuf::from("src/lib.rs")]
    );
    assert_eq!(
        std::fs::read_to_string(f.ws.join("src/lib.rs")).unwrap(),
        "pub fn a() {}\n"
    );
    assert!(!f.ws.join("src/c.rs").exists());
    assert!(f.ws.join("src/b.rs").exists(), "turn 1 is kept");
    s.turn("Keep a as it was.").await;
    let users = user_texts(&f.frontier.last());
    let last = users.last().unwrap();
    assert!(
        last.contains("reverted the file changes you made since turn 2")
            && last.contains("src/lib.rs"),
        "{last}"
    );
    // Undo walks back: turn 3 wrote nothing, then turn 1 goes.
    assert_eq!(s.undo().unwrap(), (3, vec![]));
    assert_eq!(s.undo().unwrap(), (1, vec![PathBuf::from("src/b.rs")]));
    assert!(!f.ws.join("src/b.rs").exists());
    assert!(s.undo().is_err(), "nothing left to undo");
    let _ = s.end(false);
}
