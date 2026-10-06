// SPDX-License-Identifier: GPL-3.0-or-later
//! Every run ends in a real terminal state: `Completed`, `Failed{reason}` or
//! `BudgetStopped{which}`. Through the frontier loop with a scripted frontier
//! (no model server): a panic in the presenter or the gate ends the run as
//! `Failed` with its summary and audit end event; the dollar and wall-clock
//! budgets stop it; frontier and local-model outages are retried in place until
//! the wall clock stops the run; errors a retry cannot fix fail it; an
//! interrupted run resumes to `Completed` with one intact audit chain across
//! both sessions; an interrupt kills a running command's process tree; a full
//! disk pauses the run instead of failing it; and the estimated usage of
//! attempts that failed mid-stream is charged to the dollar budget. Each session is composed the way `declass run` composes it: an
//! anchored audit log, `declass_agent::run`, then `declass_agent::conclude`.

use bytes::Bytes;
use declass_agent::{RunConfig, Terminal};
use declass_boundary::audit::{
    AuditEvent, AuditLog, Line, RunAnchors, read, run_anchors, verify_report,
};
use declass_boundary::engine::Engine;
use declass_boundary::local::LocalReader;
use declass_boundary::model::Request;
use declass_boundary::policy::Policy;
use declass_boundary::view::{PassThrough, Presenter, Source};
use declass_boundary::{OutboundFilter, OutboundGate};
use declass_provider::client::{HttpReply, Transport};
use declass_provider::{ChatProvider, ErrorKind, ProviderConfig, ProviderError, Role};
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
    /// Start answering, then drop the connection (an attempt that failed
    /// after output started).
    CutAfterOutput,
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
            Step::CutAfterOutput => Box::pin(async move {
                let chunks: [Result<Bytes, String>; 2] = [
                    Ok(Bytes::from_static(
                        b"data: {\"choices\":[{\"delta\":{\"content\":\"Let me look at the file first\"}}]}\n\n",
                    )),
                    Err("connection reset".into()),
                ];
                Ok(HttpReply {
                    status: 200,
                    headers: vec![],
                    body: futures_util::stream::iter(chunks).boxed(),
                })
            }),
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
    /// The providers' retry deadline (`declass run` gives the frontier and the
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
    root.join("ws/.declass/runs").join(RUN_ID)
}
fn audit_path(root: &Path) -> PathBuf {
    root.join("ws/.declass/audit")
        .join(format!("{RUN_ID}.jsonl"))
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
    // As `declass run` does: the audit log waits out a full disk from the start.
    gated
        .audit()
        .set_wait(Some(Arc::new(declass_agent::host::HostPolicy::until(
            (tokio::time::Instant::now() + s.wall_clock).into_std(),
            Some(s.flag.clone()),
        ))));
    gated.audit().record(AuditEvent::RunStart {
        mode: "test".into(),
        boundary: false,
    });
    let cfg = RunConfig {
        wall_clock: s.wall_clock,
        frontier_usd: s.frontier_usd,
        price: Box::new(|u| (u.input as f64 + 4.0 * u.output as f64) / 1e6),
        ..RunConfig::new(ws, run_dir(root), "Tidy src/lib.rs.")
    };
    let git = declass_git::Git::locate().unwrap();
    let (terminal, stats) =
        declass_agent::run(&cfg, &gated, s.presenter, &git, s.resume, &s.flag).await;
    let summary = declass_agent::conclude(
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
    assert!(declass_agent::resumable(&run_dir(&root)).is_ok());
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
    // retry deadline (how `declass run` sets it up).
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
    // Retried in place until the wall clock ended the run (above). How many
    // attempts fit in 800 ms depends on the machine's load, so the count only
    // has to show retrying.
    assert!(
        *unreachable.0.lock().unwrap() > 2,
        "local calls were not retried"
    );
    assert_eq!(frontier.requests(), 1);
    // The read that could not be summarized was not recorded as a result:
    // the frontier re-decides that turn on resume.
    let transcript = std::fs::read_to_string(run_dir(&root).join("transcript.jsonl")).unwrap();
    assert!(!transcript.contains("unavailable"), "{transcript}");
    assert!(!transcript.contains("tool_result"), "{transcript}");
    assert!(declass_agent::resumable(&run_dir(&root)).is_ok());
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
            reason: declass_agent::run::INTERRUPTED.into()
        }
    );
    assert_eq!(first.requests(), 2);
    assert_eq!(stored_summary(&root)["terminal"]["state"], "failed");
    assert_eq!(run_ends(&root), ["failed"]);
    chain_is_intact(&root);
    assert!(declass_agent::resumable(&run_dir(&root)).is_ok());

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
        declass_agent::resumable(&run_dir(&root)),
        Err("the run already completed".into())
    );
}

/// A command that sleeps, with a detached descendant that would write
/// `marker` after 3 s if it survived.
fn sleeper(marker: &Path) -> String {
    format!(
        "/usr/bin/nohup /bin/sh -c 'sleep 3; echo alive > {}' >/dev/null 2>&1 & sleep 60",
        marker.display()
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupt_kills_a_running_command_and_ends_the_run_resumably() {
    let (_d, root) = workspace();
    let marker = root.join("ws/survivor");
    let flag = Arc::new(AtomicBool::new(false));
    let frontier = Frontier::new(
        vec![Step::Call(
            "run_command",
            json!({"command": sleeper(&marker)}),
        )],
        &flag,
    );
    let passthrough = PassThrough { max_bytes: 60_000 };
    let mut s = Session::new(&root, frontier.clone(), &passthrough);
    s.flag = flag.clone();
    let raiser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        flag.store(true, Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let ended = session(s).await;
    raiser.await.unwrap();
    // Stopped promptly, not after the command's 60 s.
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        ended.terminal,
        Terminal::Failed {
            reason: declass_agent::run::INTERRUPTED.into()
        }
    );
    assert_eq!(stored_summary(&root)["terminal"]["state"], "failed");
    assert_eq!(run_ends(&root), ["failed"]);
    chain_is_intact(&root);
    // The interruption is recorded against the call; its result is not.
    let entries = declass_agent::transcript::Transcript::read(&run_dir(&root)).unwrap();
    assert!(entries.iter().any(|e| matches!(e,
        declass_agent::transcript::Entry::Interrupted { tool, .. } if tool == "run_command")));
    assert!(!entries.iter().any(|e| matches!(
        e,
        declass_agent::transcript::Entry::Item {
            item: declass_boundary::model::Item::ToolResult { .. }
        }
    )));
    assert!(declass_agent::resumable(&run_dir(&root)).is_ok());
    // The whole process tree is gone: the detached descendant never wrote.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(!marker.exists(), "a descendant of the command survived");
}

/// Makes the disk "full" for the listed writes: each `(op, file name, times)`
/// fails that many times, after writing a few bytes, as a disk filling up
/// mid-write does.
fn fill_disk(
    root: &Path,
    budget: Vec<(&'static str, &'static str, u32)>,
) -> (declass_fs::fault::Injected, Arc<Mutex<u32>>) {
    let failures = Arc::new(Mutex::new(0));
    let seen = failures.clone();
    let budget = Mutex::new(budget);
    let guard = declass_fs::fault::inject(root, move |op, path| {
        let name = path.file_name()?.to_string_lossy().into_owned();
        let mut budget = budget.lock().unwrap();
        let slot = budget
            .iter_mut()
            .find(|(o, n, left)| *o == op && name.ends_with(n) && *left > 0)?;
        slot.2 -= 1;
        *seen.lock().unwrap() += 1;
        Some(declass_fs::fault::Fault {
            after_bytes: 7,
            errno: rustix_nospc(),
        })
    });
    (guard, failures)
}

fn rustix_nospc() -> declass_fs::fault::Errno {
    declass_fs::fault::Errno::NOSPC
}

#[test]
fn the_audit_log_of_the_test_run_is_the_one_filled() {
    assert!(audit_path(Path::new("/r")).ends_with(format!("{RUN_ID}.jsonl")));
    assert!(RUN_ID.ends_with("abcdef"));
}

/// Every line of a JSON-lines file parses: nothing torn, nothing half-written.
fn whole_lines(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.ends_with('\n'), "{}: torn tail", path.display());
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{}: {e}: {l}", path.display())))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_disk_pauses_the_run_and_it_completes_once_space_returns() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let frontier = Frontier::new(
        vec![
            Step::Call(
                "write_file",
                json!({"path": "src/new.rs", "content": "pub fn b() {}\n"}),
            ),
            Step::Call("read_file", json!({"path": "src/new.rs"})),
            Step::Call("finish", json!({"summary": "done"})),
        ],
        &flag,
    );
    let (_guard, failures) = fill_disk(
        &root,
        vec![
            // The workspace write, the transcript, the write journal, the
            // audit log (a request) and the summary each hit a full disk.
            ("atomic write", "new.rs", 2),
            ("append", "transcript.jsonl", 2),
            ("append", "writes.jsonl", 1),
            ("append", "abcdef.jsonl", 2),
            ("write", "summary.json", 1),
        ],
    );
    let passthrough = PassThrough { max_bytes: 60_000 };
    let mut s = Session::new(&root, frontier.clone(), &passthrough);
    s.flag = flag;
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::Completed {
            summary: "done".into()
        }
    );
    assert_eq!(
        *failures.lock().unwrap(),
        8,
        "every injected failure was hit"
    );
    // Nothing was lost, repeated or half-written.
    assert_eq!(
        std::fs::read_to_string(root.join("ws/src/new.rs")).unwrap(),
        "pub fn b() {}\n"
    );
    assert_eq!(frontier.requests(), 3);
    let transcript = whole_lines(&run_dir(&root).join("transcript.jsonl"));
    let results = transcript
        .iter()
        .filter(|e| e["kind"] == "item" && e["item"]["type"] == "tool_result")
        .count();
    assert_eq!(results, 3);
    let journal = whole_lines(&run_dir(&root).join("writes.jsonl"));
    assert_eq!(journal.len(), 2, "{journal:?}");
    assert_eq!(journal[0]["state"], "pending");
    assert_eq!(journal[1]["state"], "applied");
    whole_lines(&audit_path(&root));
    let summary = stored_summary(&root);
    assert_eq!(summary["terminal"]["state"], "completed");
    assert_eq!(summary["stats"]["turns"], 3);
    assert_eq!(run_ends(&root), ["completed"]);
    chain_is_intact(&root);
    // No temporary file was left next to the written file.
    let names: Vec<_> = std::fs::read_dir(root.join("ws/src"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disk_that_stays_full_ends_the_run_at_the_wall_clock() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let frontier = read_then_finish(&flag);
    // Full for every transcript write during the run's 1 s wall clock; space
    // returns shortly after, while the run's end is being recorded.
    let full_until = std::time::Instant::now() + Duration::from_millis(1500);
    let _guard = declass_fs::fault::inject(&root, move |op, path| {
        (op == "append"
            && path.ends_with("transcript.jsonl")
            && std::time::Instant::now() < full_until)
            .then_some(declass_fs::fault::Fault {
                after_bytes: 7,
                errno: rustix_nospc(),
            })
    });
    let passthrough = PassThrough { max_bytes: 60_000 };
    let mut s = Session::new(&root, frontier.clone(), &passthrough);
    s.wall_clock = Duration::from_secs(1);
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::BudgetStopped {
            which: "wall_clock".into()
        }
    );
    assert_eq!(
        frontier.requests(),
        0,
        "the run went on without its transcript"
    );
    assert_eq!(stored_summary(&root)["terminal"]["which"], "wall_clock");
    assert_eq!(run_ends(&root), ["budget_stopped"]);
    chain_is_intact(&root);
    // Only the end entry made it, whole.
    let transcript = whole_lines(&run_dir(&root).join("transcript.jsonl"));
    assert_eq!(transcript.len(), 1, "{transcript:?}");
    assert_eq!(transcript[0]["kind"], "end");
}

/// The test price: $1 per million input tokens, $4 per million output tokens.
fn test_price(u: &Value) -> f64 {
    let n = |k: &str| u[k].as_f64().unwrap_or(0.0);
    (n("input") + 4.0 * n("output")) / 1e6
}

#[tokio::test(flavor = "multi_thread")]
async fn attempts_that_failed_mid_stream_are_charged_once() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let frontier = Frontier::new(
        vec![
            Step::CutAfterOutput,
            Step::Call("read_file", json!({"path": "src/lib.rs"})),
            Step::CutAfterOutput,
            Step::Call("finish", json!({"summary": "done"})),
        ],
        &flag,
    );
    let passthrough = PassThrough { max_bytes: 60_000 };
    let s = Session::new(&root, frontier.clone(), &passthrough);
    let ended = session(s).await;
    assert!(matches!(ended.terminal, Terminal::Completed { .. }));
    assert_eq!(frontier.requests(), 4);
    let stats = &stored_summary(&root)["stats"];
    let billed = test_price(&stats["usage"]);
    let failed = test_price(&stats["failed_attempt_usage"]);
    assert!(failed > 0.0, "{stats}");
    // Billed usage is only the two answered requests.
    assert_eq!(stats["usage"]["input"], 2000);
    assert_eq!(stats["usage"]["output"], 40);
    // The run's cost and its ledger count both, each once.
    let cost = stats["cost_usd"].as_f64().unwrap();
    assert!((cost - (billed + failed)).abs() < 1e-9, "{stats}");
    let ledger = &stats["ledger"];
    let ledger_total =
        ledger["input_usd"].as_f64().unwrap() + ledger["output_usd"].as_f64().unwrap();
    assert!((ledger_total - cost).abs() < 1e-9, "{ledger}");
    assert!((ledger["failed_attempts_usd"].as_f64().unwrap() - failed).abs() < 1e-9);
    let transcript = whole_lines(&run_dir(&root).join("transcript.jsonl"));
    let charged: f64 = transcript
        .iter()
        .filter(|e| e["kind"] == "failed_attempts")
        .map(|e| e["cost_usd"].as_f64().unwrap())
        .sum();
    assert!((charged - failed).abs() < 1e-9);

    // Charged to the dollar budget: a budget the answered request alone
    // would not reach stops the run once the failed attempt is counted.
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let frontier = Frontier::new(
        vec![
            Step::CutAfterOutput,
            Step::Call("read_file", json!({"path": "src/lib.rs"})),
            Step::Call("finish", json!({"summary": "done"})),
        ],
        &flag,
    );
    let mut s = Session::new(&root, frontier.clone(), &passthrough);
    let answered = (1000.0 + 4.0 * 20.0) / 1e6;
    s.frontier_usd = answered + failed / 4.0;
    let ended = session(s).await;
    assert_eq!(
        ended.terminal,
        Terminal::BudgetStopped {
            which: "frontier_usd".into()
        }
    );
    assert_eq!(frontier.requests(), 2);
}
