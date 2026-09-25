// SPDX-License-Identifier: GPL-3.0-or-later
//! Every run ends in a real terminal state: `Completed`, `Failed{reason}` or
//! `BudgetStopped{which}`. Through the frontier loop with a scripted frontier
//! (no model server): a panic in the presenter or the gate ends the run as
//! `Failed` with its summary and audit end event; the dollar and wall-clock
//! budgets stop it; frontier and local-model outages are retried in place until
//! the wall clock stops the run; errors a retry cannot fix fail it; and an
//! interrupted run resumes to `Completed` with one intact audit chain across
//! both sessions. Each session is composed the way `duet run` composes it: an
//! anchored audit log, `duet_agent::run`, then `duet_agent::conclude`.

use bytes::Bytes;
use duet_agent::{RunConfig, Terminal};
use duet_boundary::audit::{
    AuditEvent, AuditLog, Line, RunAnchors, read, run_anchors, verify_report,
};
use duet_boundary::engine::Engine;
use duet_boundary::local::LocalReader;
use duet_boundary::model::Request;
use duet_boundary::policy::Policy;
use duet_boundary::view::{PassThrough, Presenter, Source};
use duet_boundary::{OutboundFilter, OutboundGate};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ErrorKind, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RUN_ID: &str = "20260924-000000-abcdef";

/// What the scripted frontier does with one request.
#[derive(Clone)]
enum Step {
    /// Answer with one tool call.
    Call(&'static str, Value),
    /// Answer with this HTTP status, for this and every later request.
    DownWith(u16),
    /// Set the interrupt flag, then never answer.
    InterruptAndHang,
}

#[derive(Clone)]
struct Frontier {
    steps: Arc<Mutex<VecDeque<Step>>>,
    requests: Arc<Mutex<u32>>,
    flag: Arc<AtomicBool>,
    /// Usage reported per answer (prompt, completion tokens).
    usage: (u64, u64),
}

impl Frontier {
    fn new(steps: Vec<Step>, flag: &Arc<AtomicBool>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into())),
            requests: Arc::default(),
            flag: flag.clone(),
            usage: (1000, 20),
        }
    }
    fn requests(&self) -> u32 {
        *self.requests.lock().unwrap()
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        _body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let n = {
            let mut r = self.requests.lock().unwrap();
            *r += 1;
            *r
        };
        let step = {
            let mut steps = self.steps.lock().unwrap();
            match steps.front().cloned().expect("unscripted request") {
                s @ Step::DownWith(_) => s,
                _ => steps.pop_front().unwrap(),
            }
        };
        let (prompt, completion) = self.usage;
        match step {
            Step::Call(name, args) => {
                let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0,
                    "id": format!("c{n}"), "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}}]}}]});
                let chunks = [
                    format!("data: {call}\n\n"),
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n"
                        .into(),
                    format!(
                        "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":{completion}}}}}\n\n"
                    ),
                    "data: [DONE]\n\n".into(),
                ];
                Box::pin(async move {
                    Ok(HttpReply {
                        status: 200,
                        headers: vec![],
                        body: futures_util::stream::iter(chunks.map(|c| Ok(Bytes::from(c))))
                            .boxed(),
                    })
                })
            }
            Step::DownWith(status) => Box::pin(async move {
                Ok(HttpReply {
                    status,
                    headers: vec![],
                    body: futures_util::stream::iter([Ok(Bytes::from_static(b"unavailable"))])
                        .boxed(),
                })
            }),
            Step::InterruptAndHang => {
                self.flag.store(true, Ordering::SeqCst);
                Box::pin(futures_util::future::pending())
            }
        }
    }
}

/// A model server that refuses every connection.
#[derive(Clone, Default)]
struct Unreachable(Arc<Mutex<u32>>);

impl Transport for Unreachable {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        _body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        *self.0.lock().unwrap() += 1;
        Box::pin(async {
            Err(ProviderError::new(
                ErrorKind::Transport,
                "connection refused",
            ))
        })
    }
}

struct Session<'a> {
    root: &'a Path,
    frontier: Box<dyn Transport>,
    presenter: &'a dyn Presenter,
    filter: Option<Box<dyn OutboundFilter>>,
    flag: Arc<AtomicBool>,
    resume: bool,
    wall_clock: Duration,
    /// The providers' retry deadline (`duet run` gives the frontier and the
    /// local model the same one).
    deadline: Option<tokio::time::Instant>,
    frontier_usd: f64,
}

impl<'a> Session<'a> {
    fn new(
        root: &'a Path,
        frontier: impl Transport + 'static,
        presenter: &'a dyn Presenter,
    ) -> Self {
        Self {
            root,
            frontier: Box::new(frontier),
            presenter,
            filter: None,
            flag: Arc::new(AtomicBool::new(false)),
            resume: false,
            wall_clock: Duration::from_secs(60),
            deadline: None,
            frontier_usd: 10.0,
        }
    }
}

struct Ended {
    terminal: Terminal,
    summary: Value,
}

fn run_dir(root: &Path) -> PathBuf {
    root.join("ws/.duet/runs").join(RUN_ID)
}
fn audit_path(root: &Path) -> PathBuf {
    root.join("ws/.duet/audit").join(format!("{RUN_ID}.jsonl"))
}
fn anchor(root: &Path) -> RunAnchors {
    run_anchors(&root.join("state"), &root.join("ws"), RUN_ID)
}

async fn session(s: Session<'_>) -> Ended {
    let (root, ws) = (s.root, s.root.join("ws"));
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.001;
    pc.deadline = s.deadline;
    pc.cancel = Some(s.flag.clone());
    let provider = ChatProvider::new(pc, s.frontier).unwrap();
    let audit = AuditLog::open_anchored(&audit_path(root), &anchor(root)).unwrap();
    let mut gate = OutboundGate::new(audit);
    if let Some(f) = s.filter {
        gate = gate.with_filter(f);
    }
    let gated = gate.wrap(provider);
    gated.audit().record(AuditEvent::RunStart {
        mode: "test".into(),
        boundary: false,
    });
    let cfg = RunConfig {
        workspace: ws,
        run_dir: run_dir(root),
        objective: "Tidy src/lib.rs.".into(),
        mode: "test".into(),
        checks: vec![],
        sandbox: duet_sandbox::detect().unwrap_or(duet_sandbox::SandboxKind::Seatbelt),
        network: false,
        command_timeout: Duration::from_secs(30),
        wall_clock: s.wall_clock,
        frontier_usd: s.frontier_usd,
        max_finish_attempts: 2,
        context_window: 200_000,
        mask_at: 0.7,
        max_output_tokens: 1000,
        reasoning_effort: None,
        price: Box::new(|u| (u.input as f64 + 4.0 * u.output as f64) / 1e6),
        oversight: duet_agent::Oversight::default(),
    };
    let git = duet_git::Git::locate().unwrap();
    let (terminal, stats) =
        duet_agent::run(&cfg, &gated, s.presenter, &git, s.resume, &s.flag).await;
    let summary = duet_agent::conclude(
        &cfg.run_dir,
        RUN_ID,
        Some(gated.audit()),
        &audit_path(root),
        &terminal,
        &stats,
    )
    .unwrap();
    Ended { terminal, summary }
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("ws/src")).unwrap();
    std::fs::create_dir_all(root.join("ws/data")).unwrap();
    std::fs::write(root.join("ws/src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(
        root.join("ws/data/orders.csv"),
        "id,customer,amount\n1,Orla Brennvik,4812339\n",
    )
    .unwrap();
    (d, root)
}

fn stored_summary(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(run_dir(root).join("summary.json")).unwrap()).unwrap()
}

/// The audit log's end events, in order.
fn run_ends(root: &Path) -> Vec<String> {
    read(&audit_path(root))
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) => match e.event {
                AuditEvent::RunEnd { terminal } => Some(terminal),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn chain_is_intact(root: &Path) {
    let (code, lines) = verify_report(&audit_path(root), &anchor(root)).unwrap();
    assert_eq!(code, 0, "{lines:?}");
    assert!(lines[0].starts_with("chain intact"), "{lines:?}");
}

fn read_then_finish(flag: &Arc<AtomicBool>) -> Frontier {
    Frontier::new(
        vec![
            Step::Call("read_file", json!({"path": "src/lib.rs"})),
            Step::Call("finish", json!({"summary": "done"})),
        ],
        flag,
    )
}

/// Shows content as it is, except that it panics on the first file it shows.
struct PanicsOnRead(PassThrough);

impl Presenter for PanicsOnRead {
    fn present(&self, source: &Source, bytes: &[u8]) -> String {
        if matches!(source, Source::File { .. }) {
            panic!("presenter defect while showing a file");
        }
        self.0.present(source, bytes)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panic_in_the_presenter_ends_the_run_as_failed_with_its_records() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let presenter = PanicsOnRead(PassThrough { max_bytes: 60_000 });
    let ended = session(Session::new(&root, read_then_finish(&flag), &presenter)).await;
    let Terminal::Failed { reason } = &ended.terminal else {
        panic!("{:?}", ended.terminal)
    };
    assert_eq!(
        reason,
        "internal error: presenter defect while showing a file"
    );
    // The summary is written, the audit log ends with the run's end event and
    // its chain and anchor verify, and the transcript records the end.
    let summary = stored_summary(&root);
    assert_eq!(summary, ended.summary);
    assert_eq!(summary["terminal"]["state"], "failed");
    assert_eq!(summary["terminal"]["reason"], reason.as_str());
    assert_eq!(summary["stats"]["turns"], 1);
    assert_eq!(run_ends(&root), ["failed"]);
    chain_is_intact(&root);
    let transcript = std::fs::read_to_string(run_dir(&root).join("transcript.jsonl")).unwrap();
    assert!(
        transcript
            .lines()
            .last()
            .unwrap()
            .contains("internal error"),
        "{transcript}"
    );
    // The half-done turn is re-decided on resume, not replayed.
    assert!(duet_agent::resumable(&run_dir(&root)).is_ok());
}

struct PanickingFilter;

impl OutboundFilter for PanickingFilter {
    fn name(&self) -> &'static str {
        "defective"
    }
    fn apply(&self, _request: &mut Request) -> Vec<String> {
        panic!("{}", String::from("gate filter defect"))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panic_in_the_gate_ends_the_run_as_failed_before_anything_is_sent() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let frontier = read_then_finish(&flag);
    let passthrough = PassThrough { max_bytes: 60_000 };
    let mut s = Session::new(&root, frontier.clone(), &passthrough);
    s.filter = Some(Box::new(PanickingFilter));
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::Failed {
            reason: "internal error: gate filter defect".into()
        }
    );
    assert_eq!(frontier.requests(), 0, "a request left the gate");
    assert_eq!(stored_summary(&root)["terminal"]["state"], "failed");
    assert_eq!(run_ends(&root), ["failed"]);
    chain_is_intact(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dollar_budget_stops_the_run() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let mut frontier = Frontier::new(
        vec![
            Step::Call("read_file", json!({"path": "src/lib.rs"})),
            Step::Call("read_file", json!({"path": "src/lib.rs"})),
            Step::Call("read_file", json!({"path": "src/lib.rs"})),
            Step::Call("finish", json!({"summary": "done"})),
        ],
        &flag,
    );
    // $0.10 per turn at the test price; the budget is $0.25.
    frontier.usage = (100_000, 0);
    let passthrough = PassThrough { max_bytes: 60_000 };
    let mut s = Session::new(&root, frontier.clone(), &passthrough);
    s.frontier_usd = 0.25;
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::BudgetStopped {
            which: "frontier_usd".into()
        }
    );
    assert_eq!(frontier.requests(), 3);
    let summary = stored_summary(&root);
    assert_eq!(summary["terminal"]["state"], "budget_stopped");
    assert!(summary["stats"]["cost_usd"].as_f64().unwrap() >= 0.25);
    assert_eq!(run_ends(&root), ["budget_stopped"]);
    chain_is_intact(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_wall_clock_stops_the_run() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let frontier = read_then_finish(&flag);
    let passthrough = PassThrough { max_bytes: 60_000 };
    let mut s = Session::new(&root, frontier.clone(), &passthrough);
    s.wall_clock = Duration::ZERO;
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::BudgetStopped {
            which: "wall_clock".into()
        }
    );
    assert_eq!(frontier.requests(), 0);
    assert_eq!(stored_summary(&root)["terminal"]["which"], "wall_clock");
    assert_eq!(run_ends(&root), ["budget_stopped"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frontier_outage_is_retried_in_place_until_the_wall_clock_ends_the_run() {
    // Once through the run's own wall clock, once through the provider's
    // retry deadline (how `duet run` sets it up).
    for provider_deadline in [None, Some(Duration::from_millis(800))] {
        for status in [503, 429, 500] {
            let (_d, root) = workspace();
            let flag = Arc::new(AtomicBool::new(false));
            let frontier = Frontier::new(
                vec![
                    Step::Call("read_file", json!({"path": "src/lib.rs"})),
                    Step::DownWith(status),
                ],
                &flag,
            );
            let passthrough = PassThrough { max_bytes: 60_000 };
            let mut s = Session::new(&root, frontier.clone(), &passthrough);
            s.wall_clock = Duration::from_millis(800);
            s.deadline = provider_deadline.map(|d| tokio::time::Instant::now() + d);
            let ended = session(s).await;
            assert_eq!(
                ended.terminal,
                Terminal::BudgetStopped {
                    which: "wall_clock".into()
                },
                "{status} {provider_deadline:?}"
            );
            // Retried well past the old fixed cap of six attempts.
            assert!(frontier.requests() > 7, "{}", frontier.requests());
            assert_eq!(run_ends(&root), ["budget_stopped"]);
            chain_is_intact(&root);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_a_retry_cannot_fix_fail_the_run() {
    for (status, kind) in [(401, "Auth"), (400, "Status(400)")] {
        let (_d, root) = workspace();
        let flag = Arc::new(AtomicBool::new(false));
        let frontier = Frontier::new(vec![Step::DownWith(status)], &flag);
        let passthrough = PassThrough { max_bytes: 60_000 };
        let ended = session(Session::new(&root, frontier.clone(), &passthrough)).await;
        let Terminal::Failed { reason } = &ended.terminal else {
            panic!("{status}: {:?}", ended.terminal)
        };
        assert!(reason.starts_with(&format!("frontier: {kind}")), "{reason}");
        assert_eq!(frontier.requests(), 1, "{status} was retried");
        assert_eq!(run_ends(&root), ["failed"]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_model_outage_ends_at_the_wall_clock_without_a_degraded_result() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let unreachable = Unreachable::default();
    let mut lc = ProviderConfig::new(
        "http://127.0.0.1:9/v1",
        "local",
        Role::Local {
            allowlist: Vec::new(),
            allow_plaintext: false,
        },
    );
    lc.backoff_scale = 0.001;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    lc.deadline = Some(deadline);
    lc.cancel = Some(flag.clone());
    let local = ChatProvider::new(lc, Box::new(unreachable.clone())).unwrap();
    let policy = Policy {
        sensitive_globs: vec!["data/**".into()],
        detect_pii: true,
        ..Policy::default()
    };
    let engine = Engine::open(&run_dir(&root), policy, Some(LocalReader::new(local))).unwrap();
    let frontier = Frontier::new(
        vec![
            Step::Call("read_file", json!({"path": "data/orders.csv"})),
            Step::Call("finish", json!({"summary": "done"})),
        ],
        &flag,
    );
    let mut s = Session::new(&root, frontier.clone(), engine.as_ref());
    s.wall_clock = Duration::from_secs(60);
    s.deadline = Some(deadline);
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::BudgetStopped {
            which: "wall_clock".into()
        }
    );
    assert!(
        *unreachable.0.lock().unwrap() > 7,
        "local calls were not retried"
    );
    assert_eq!(frontier.requests(), 1);
    // The read that could not be summarized was not recorded as a result:
    // the frontier re-decides that turn on resume.
    let transcript = std::fs::read_to_string(run_dir(&root).join("transcript.jsonl")).unwrap();
    assert!(!transcript.contains("unavailable"), "{transcript}");
    assert!(!transcript.contains("tool_result"), "{transcript}");
    assert!(duet_agent::resumable(&run_dir(&root)).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_run_resumes_to_completion_with_one_intact_audit_chain() {
    let (_d, root) = workspace();
    let passthrough = PassThrough { max_bytes: 60_000 };

    // Session 1: one tool turn, then the interrupt arrives while the frontier
    // is answering the second request.
    let flag = Arc::new(AtomicBool::new(false));
    let first = Frontier::new(
        vec![
            Step::Call(
                "write_file",
                json!({"path": "src/new.rs", "content": "pub fn b() {}\n"}),
            ),
            Step::InterruptAndHang,
        ],
        &flag,
    );
    let mut s = Session::new(&root, first.clone(), &passthrough);
    s.flag = flag;
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::Failed {
            reason: duet_agent::run::INTERRUPTED.into()
        }
    );
    assert_eq!(first.requests(), 2);
    assert_eq!(stored_summary(&root)["terminal"]["state"], "failed");
    assert_eq!(run_ends(&root), ["failed"]);
    chain_is_intact(&root);
    assert!(duet_agent::resumable(&run_dir(&root)).is_ok());

    // Session 2: resume. The conversation continues where it stopped.
    let flag = Arc::new(AtomicBool::new(false));
    let second = Frontier::new(
        vec![Step::Call("finish", json!({"summary": "done"}))],
        &flag,
    );
    let mut s = Session::new(&root, second.clone(), &passthrough);
    s.flag = flag;
    s.resume = true;
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::Completed {
            summary: "done".into()
        }
    );
    assert_eq!(second.requests(), 1);
    assert_eq!(
        std::fs::read_to_string(root.join("ws/src/new.rs")).unwrap(),
        "pub fn b() {}\n"
    );
    let summary = stored_summary(&root);
    assert_eq!(summary["terminal"]["state"], "completed");
    // Usage of both sessions: two answered requests before, one after.
    assert_eq!(summary["stats"]["turns"], 2);
    // One log, both sessions, each with its end; chain and anchor intact.
    assert_eq!(run_ends(&root), ["failed", "completed"]);
    chain_is_intact(&root);
    let starts = read(&audit_path(&root))
        .unwrap()
        .into_iter()
        .filter(|l| matches!(l, Line::Event(e) if matches!(e.event, AuditEvent::RunStart { .. })))
        .count();
    assert_eq!(starts, 2);
    // A completed run is not resumed again.
    assert_eq!(
        duet_agent::resumable(&run_dir(&root)),
        Err("the run already completed".into())
    );
}
