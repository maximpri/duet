// SPDX-License-Identifier: GPL-3.0-or-later
//! The embedding API (ARCHITECTURE.md, "Embedding Duet") through the frontier
//! loop with a scripted frontier (no model server): audit subscribers are told
//! of every record of a run's log in chain order, with its position and hash
//! and never content; the end hook is called exactly once per run and per
//! session invocation, whatever the terminal state (completed, failed, a
//! panic, budget-stopped, interrupted and resumed, before the audit log
//! opened); and failing or panicking hooks never change the run, only leave
//! `hook_failed` events. Runs are composed the way `duet run` composes them:
//! an anchored audit log, `duet_agent::run`, then `duet_agent::conclude_with`.

use bytes::Bytes;
use duet_agent::embed::{
    Appended, AuditSubscriber, EndHook, EndReport, Ending, Hooks, Opened, PolicyMeta, Recorded,
    RunKind,
};
use duet_agent::session::{SESSION_LEFT, SessionLimits};
use duet_agent::{RunConfig, Session, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{
    Anchor, AuditEvent, AuditLog, Line, RunAnchors, read, run_anchors, verify_report,
};
use duet_boundary::engine::Engine;
use duet_boundary::view::{PassThrough, Presenter, Source};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RUN_ID: &str = "20260926-000000-e1b2c3";
/// In the workspace's source file: it reaches the frontier (pass-through), so
/// it is in the audit log's request bodies, and must reach no hook.
const MARKER: &str = "MARKER_7731";

#[derive(Clone)]
enum Step {
    Call(&'static str, Value),
    /// This status for this and every later request.
    DownWith(u16),
    /// Raise the interrupt flag, then never answer.
    InterruptAndHang,
}

#[derive(Clone)]
struct Frontier {
    steps: Arc<Mutex<VecDeque<Step>>>,
    flag: Arc<AtomicBool>,
}

impl Frontier {
    fn new(steps: Vec<Step>, flag: &Arc<AtomicBool>) -> Self {
        Self {
            steps: Arc::new(Mutex::new(steps.into())),
            flag: flag.clone(),
        }
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        _body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let step = {
            let mut steps = self.steps.lock().unwrap();
            match steps.front().cloned().expect("unscripted request") {
                s @ Step::DownWith(_) => s,
                _ => steps.pop_front().unwrap(),
            }
        };
        let reply = |status: u16, chunks: Vec<String>| {
            Box::pin(async move {
                Ok(HttpReply {
                    status,
                    headers: vec![],
                    body: futures_util::stream::iter(
                        chunks.into_iter().map(|c| Ok(Bytes::from(c))),
                    )
                    .boxed(),
                })
            }) as BoxFuture<'static, Result<HttpReply, ProviderError>>
        };
        match step {
            Step::Call(name, args) => {
                let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0,
                    "id": format!("c-{name}"), "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}}]}}]});
                reply(
                    200,
                    vec![
                        format!("data: {call}\n\n"),
                        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n"
                            .into(),
                        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":20}}\n\n"
                            .into(),
                        "data: [DONE]\n\n".into(),
                    ],
                )
            }
            Step::DownWith(status) => reply(status, vec!["{\"error\":\"no\"}".into()]),
            Step::InterruptAndHang => {
                self.flag.store(true, Ordering::SeqCst);
                Box::pin(futures_util::future::pending())
            }
        }
    }
}

/// Keeps what it is told; `fails` makes it return an error (`Some(false)`)
/// or panic (`Some(true)`) on every record.
#[derive(Default)]
struct Recorder {
    name: &'static str,
    opened: Mutex<Vec<Opened>>,
    seen: Mutex<Vec<Appended>>,
    fails: Option<bool>,
}

impl Recorder {
    fn named(name: &'static str, fails: Option<bool>) -> Arc<Self> {
        Arc::new(Self {
            name,
            fails,
            ..Self::default()
        })
    }
    fn seen(&self) -> Vec<Appended> {
        self.seen.lock().unwrap().clone()
    }
}

impl AuditSubscriber for Recorder {
    fn name(&self) -> &str {
        self.name
    }
    fn opened(&self, log: &Opened) -> Result<(), String> {
        self.opened.lock().unwrap().push(log.clone());
        Ok(())
    }
    fn appended(&self, record: &Appended) -> Result<(), String> {
        self.seen.lock().unwrap().push(record.clone());
        match self.fails {
            None => Ok(()),
            Some(false) => Err("exporter unavailable".into()),
            Some(true) => panic!("subscriber defect"),
        }
    }
}

/// Keeps the reports it is given.
#[derive(Default)]
struct Ends {
    name: &'static str,
    reports: Mutex<Vec<EndReport>>,
    fails: Option<bool>,
}

impl Ends {
    fn named(name: &'static str, fails: Option<bool>) -> Arc<Self> {
        Arc::new(Self {
            name,
            fails,
            ..Self::default()
        })
    }
    fn reports(&self) -> Vec<EndReport> {
        self.reports.lock().unwrap().clone()
    }
}

impl EndHook for Ends {
    fn name(&self) -> &str {
        self.name
    }
    fn ended(&self, report: &EndReport) -> Result<(), String> {
        self.reports.lock().unwrap().push(report.clone());
        match self.fails {
            None => Ok(()),
            Some(false) => Err("receipt key unavailable".into()),
            Some(true) => panic!("end hook defect"),
        }
    }
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("ws/src")).unwrap();
    std::fs::write(
        root.join("ws/src/lib.rs"),
        format!("pub fn a() {{}} // {MARKER}\n"),
    )
    .unwrap();
    (d, root)
}

fn run_dir(root: &Path) -> PathBuf {
    root.join("ws/.duet/runs").join(RUN_ID)
}
fn audit_path(root: &Path) -> PathBuf {
    root.join("ws/.duet/audit").join(format!("{RUN_ID}.jsonl"))
}
fn anchors(root: &Path) -> RunAnchors {
    run_anchors(&root.join("state"), &root.join("ws"), RUN_ID)
}

fn policy_meta() -> PolicyMeta {
    let mut m = PolicyMeta::new("test", "not verified");
    m.name = "acme".into();
    m
}

/// One invocation of a run, composed as `duet run` composes it.
struct Invocation<'a> {
    root: &'a Path,
    hooks: &'a Hooks,
    steps: Vec<Step>,
    presenter: &'a dyn Presenter,
    /// Hybrid mode: the security engine presents and gates.
    engine: Option<Arc<Engine>>,
    resume: bool,
    wall_clock: Duration,
}

impl<'a> Invocation<'a> {
    fn new(root: &'a Path, hooks: &'a Hooks, steps: Vec<Step>) -> Self {
        Self {
            root,
            hooks,
            steps,
            presenter: &PASS,
            engine: None,
            resume: false,
            wall_clock: Duration::from_secs(60),
        }
    }

    async fn run(self) -> (Terminal, Value) {
        let root = self.root;
        let flag = Arc::new(AtomicBool::new(false));
        let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
        pc.backoff_scale = 0.001;
        pc.cancel = Some(flag.clone());
        let provider = ChatProvider::new(pc, Box::new(Frontier::new(self.steps, &flag))).unwrap();
        let mut log = AuditLog::open_anchored(&audit_path(root), &anchors(root)).unwrap();
        self.hooks.attach(&mut log);
        let mut gate = OutboundGate::new(log);
        if let Some(e) = &self.engine {
            let (filter, check) = e.outbound();
            gate = gate.with_filter(filter).with_check(check);
        }
        let gated = gate.wrap(provider);
        let mode = if self.engine.is_some() {
            "hybrid"
        } else {
            "passthrough"
        };
        gated.audit().record(AuditEvent::RunStart {
            mode: mode.into(),
            boundary: self.engine.is_some(),
        });
        let presenter: &dyn Presenter = match &self.engine {
            Some(e) => e.as_ref(),
            None => self.presenter,
        };
        let cfg = RunConfig {
            wall_clock: self.wall_clock,
            frontier_usd: 10.0,
            ..RunConfig::new(root.join("ws"), run_dir(root), "Tidy src/lib.rs.")
        };
        let git = duet_git::Git::locate().unwrap();
        let (terminal, stats) =
            duet_agent::run(&cfg, &gated, presenter, &git, self.resume, &flag).await;
        let meta = policy_meta();
        let summary = duet_agent::conclude_with(
            &cfg.run_dir,
            RUN_ID,
            Some(gated.audit()),
            &audit_path(root),
            &terminal,
            &stats,
            &Ending {
                kind: RunKind::Run,
                mode,
                resumed: self.resume,
                policy: Some(&meta),
                hooks: self.hooks,
            },
        )
        .unwrap();
        (terminal, summary)
    }
}

const PASS: PassThrough = PassThrough { max_bytes: 60_000 };

fn read_then_finish() -> Vec<Step> {
    vec![
        Step::Call("read_file", json!({"path": "src/lib.rs"})),
        Step::Call("finish", json!({"summary": "done"})),
    ]
}

fn digest(line: &str) -> String {
    hex::encode(Sha256::digest(line.as_bytes()))
}

fn log_lines(root: &Path) -> Vec<String> {
    std::fs::read_to_string(audit_path(root))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn events(root: &Path) -> Vec<AuditEvent> {
    read(&audit_path(root))
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) => Some(e.event),
            Line::Request(_) => None,
        })
        .collect()
}

fn chain_is_intact(root: &Path) {
    let (code, lines) = verify_report(&audit_path(root), &anchors(root)).unwrap();
    assert_eq!(code, 0, "{lines:?}");
}

/// The report's head is the log's head after `run_end`, and its anchor file
/// holds it.
fn head_is_the_logs(report: &EndReport, lines: &[String]) {
    let chain = report.chain.as_ref().expect("the log was open");
    assert_eq!(chain.records, lines.len() as u64);
    assert_eq!(chain.head, digest(lines.last().unwrap()));
    let anchor: Anchor =
        serde_json::from_slice(&std::fs::read(chain.anchor.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!((anchor.records, &anchor.head), (chain.records, &chain.head));
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribers_are_told_every_record_of_a_run_in_order_and_no_content() {
    let (_d, root) = workspace();
    let (a, b) = (Recorder::named("a", None), Recorder::named("b", None));
    let hooks = Hooks::default().subscribe(a.clone()).subscribe(b.clone());
    let (terminal, _) = Invocation::new(&root, &hooks, read_then_finish())
        .run()
        .await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );

    let opened = a.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(
        (opened[0].records, opened[0].run_id.as_deref()),
        (0, Some(RUN_ID))
    );
    let seen = a.seen();
    assert_eq!(seen, b.seen(), "every subscriber is told the same");
    // Every line of the log, in order, with its position and hash.
    let lines = log_lines(&root);
    assert_eq!(seen.len(), lines.len());
    for (n, (record, line)) in seen.iter().zip(&lines).enumerate() {
        assert_eq!(record.seq, n as u64 + 1);
        assert_eq!(record.hash, digest(line));
        if n > 0 {
            assert_eq!(record.prev, seen[n - 1].hash);
        }
    }
    assert!(matches!(
        &seen[0].record,
        Recorded::Event {
            event: AuditEvent::RunStart { .. }
        }
    ));
    assert!(matches!(
        &seen.last().unwrap().record,
        Recorded::Event {
            event: AuditEvent::RunEnd { terminal }
        } if terminal == "completed"
    ));
    let requests: Vec<u64> = seen
        .iter()
        .filter_map(|r| match &r.record {
            Recorded::Request { bytes, .. } => Some(*bytes),
            // The record kinds may grow (`Recorded` is non-exhaustive).
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|b| *b > 0));
    // The file's text went to the frontier and is in the log's request
    // bodies, but not in anything a subscriber was told.
    assert!(lines.iter().any(|l| l.contains(MARKER)));
    let told = serde_json::to_string(&seen).unwrap();
    assert!(!told.contains(MARKER), "{told}");
    chain_is_intact(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_end_hook_is_called_once_for_a_completed_run() {
    let (_d, root) = workspace();
    let ends = Ends::named("receipts", None);
    let hooks = Hooks::default().on_end(ends.clone());
    let (_, summary) = Invocation::new(&root, &hooks, read_then_finish())
        .run()
        .await;
    let reports = ends.reports();
    assert_eq!(reports.len(), 1);
    let r = &reports[0];
    assert_eq!(r.run_id, RUN_ID);
    assert_eq!((r.kind, r.mode.as_str()), (RunKind::Run, "passthrough"));
    assert_eq!(r.state, "completed");
    assert!(!r.resumed && !r.resumable && !r.interrupted);
    assert_eq!(r.budget, None);
    assert_eq!(r.audit_log, audit_path(&root));
    assert_eq!(r.policy.as_ref().map(|p| p.name.as_str()), Some("acme"));
    assert_eq!(r.stats.turns, 2);
    assert_eq!(r.stats.tool_calls, 2);
    assert_eq!(r.stats.input_tokens, 2000);
    let disclosure = r.disclosure.as_ref().unwrap();
    assert_eq!(disclosure.requests, 2);
    assert_eq!(
        serde_json::to_value(disclosure).unwrap(),
        summary["disclosure"]
    );
    head_is_the_logs(r, &log_lines(&root));
    // Counts, digests and paths: no summary, reason or file content.
    let text = serde_json::to_string(r).unwrap();
    assert!(!text.contains(MARKER) && !text.contains("done"), "{text}");
}

/// Shows content as it is, except that it panics on the first file it shows.
struct PanicsOnRead;

impl Presenter for PanicsOnRead {
    fn present(&self, source: &Source, bytes: &[u8]) -> String {
        if matches!(source, Source::File { .. }) {
            panic!("presenter defect while showing a file");
        }
        PASS.present(source, bytes)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_end_hook_is_called_once_for_a_failed_a_panicked_and_a_budget_stopped_run() {
    // An error a retry cannot fix.
    let (_d, root) = workspace();
    let ends = Ends::named("receipts", None);
    let hooks = Hooks::default().on_end(ends.clone());
    let (terminal, _) = Invocation::new(&root, &hooks, vec![Step::DownWith(401)])
        .run()
        .await;
    assert!(matches!(terminal, Terminal::Failed { .. }), "{terminal:?}");
    let reports = ends.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].state, "failed");
    assert!(!reports[0].interrupted);
    // As `duet resume` would say.
    assert_eq!(
        reports[0].resumable,
        duet_agent::resumable(&run_dir(&root)).is_ok()
    );
    head_is_the_logs(&reports[0], &log_lines(&root));

    // A panic in the run.
    let (_d, root) = workspace();
    let ends = Ends::named("receipts", None);
    let hooks = Hooks::default().on_end(ends.clone());
    let mut run = Invocation::new(&root, &hooks, read_then_finish());
    run.presenter = &PanicsOnRead;
    let (terminal, _) = run.run().await;
    assert!(
        matches!(&terminal, Terminal::Failed { reason } if reason.starts_with("internal error")),
        "{terminal:?}"
    );
    let reports = ends.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].state, "failed");
    // The panic's message is not in the report.
    assert!(
        !serde_json::to_string(&reports[0])
            .unwrap()
            .contains("defect")
    );

    // The wall clock.
    let (_d, root) = workspace();
    let ends = Ends::named("receipts", None);
    let hooks = Hooks::default().on_end(ends.clone());
    let mut run = Invocation::new(&root, &hooks, read_then_finish());
    run.wall_clock = Duration::ZERO;
    let (terminal, _) = run.run().await;
    assert!(matches!(terminal, Terminal::BudgetStopped { .. }));
    let reports = ends.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].state, "budget_stopped");
    assert_eq!(reports[0].budget.as_deref(), Some("wall_clock"));
    head_is_the_logs(&reports[0], &log_lines(&root));

    // A run that failed before its audit log could be opened (the CLI
    // concludes it all the same): no chain to report.
    let (_d, root) = workspace();
    let ends = Ends::named("receipts", None);
    let hooks = Hooks::default().on_end(ends.clone());
    std::fs::create_dir_all(run_dir(&root)).unwrap();
    let terminal = Terminal::Failed {
        reason: "no sandbox".into(),
    };
    duet_agent::conclude_with(
        &run_dir(&root),
        RUN_ID,
        None,
        &audit_path(&root),
        &terminal,
        &duet_agent::RunStats::default(),
        &Ending {
            kind: RunKind::Run,
            mode: "hybrid",
            resumed: false,
            policy: None,
            hooks: &hooks,
        },
    )
    .unwrap();
    let reports = ends.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].state, "failed");
    assert!(reports[0].chain.is_none() && reports[0].disclosure.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_run_and_its_resume_are_one_call_each() {
    let (_d, root) = workspace();
    let ends = Ends::named("receipts", None);
    let recorder = Recorder::named("records", None);
    let hooks = Hooks::default()
        .on_end(ends.clone())
        .subscribe(recorder.clone());
    let (terminal, _) = Invocation::new(
        &root,
        &hooks,
        vec![
            Step::Call("read_file", json!({"path": "src/lib.rs"})),
            Step::InterruptAndHang,
        ],
    )
    .run()
    .await;
    assert_eq!(
        terminal,
        Terminal::Failed {
            reason: duet_agent::run::INTERRUPTED.into()
        }
    );
    let first = ends.reports();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].state, "failed");
    assert!(first[0].interrupted && first[0].resumable && !first[0].resumed);
    head_is_the_logs(&first[0], &log_lines(&root));

    let mut resumed = Invocation::new(
        &root,
        &hooks,
        vec![Step::Call("finish", json!({"summary": "done"}))],
    );
    resumed.resume = true;
    let (terminal, _) = resumed.run().await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let reports = ends.reports();
    assert_eq!(reports.len(), 2, "one call per invocation");
    let second = &reports[1];
    assert_eq!(second.state, "completed");
    assert!(second.resumed && !second.resumable && !second.interrupted);
    let lines = log_lines(&root);
    head_is_the_logs(second, &lines);
    assert!(second.chain.as_ref().unwrap().records > first[0].chain.as_ref().unwrap().records);

    // The subscriber was attached twice (once per invocation) and told of
    // each record once; the second time it was told where the chain stood.
    let opened = recorder.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[1].records, first[0].chain.as_ref().unwrap().records);
    assert_eq!(opened[1].head, first[0].chain.as_ref().unwrap().head);
    let seqs: Vec<u64> = recorder.seen().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, (1..=lines.len() as u64).collect::<Vec<_>>());
    chain_is_intact(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn failing_hooks_never_change_the_run() {
    // The same run without hooks, for comparison.
    let (_d, plain_root) = workspace();
    let none = Hooks::default();
    let (plain, plain_summary) = Invocation::new(&plain_root, &none, read_then_finish())
        .run()
        .await;

    let (_d, root) = workspace();
    let good = Recorder::named("good", None);
    let receipts = Ends::named("receipts", None);
    let hooks = Hooks::default()
        .subscribe(Recorder::named("export", Some(false)))
        .subscribe(Recorder::named("broken", Some(true)))
        .subscribe(good.clone())
        .on_end(Ends::named("signer", Some(false)))
        .on_end(Ends::named("crashy", Some(true)))
        .on_end(receipts.clone());
    let (terminal, summary) = Invocation::new(&root, &hooks, read_then_finish())
        .run()
        .await;
    assert_eq!(terminal, plain);
    assert_eq!(summary["terminal"], plain_summary["terminal"]);
    assert_eq!(summary["stats"]["turns"], plain_summary["stats"]["turns"]);
    // The other end hooks were still called, once.
    assert_eq!(receipts.reports().len(), 1);

    let events = events(&root);
    let failed: Vec<(String, String, Option<u64>, bool)> = events
        .iter()
        .filter_map(|e| match e {
            AuditEvent::HookFailed {
                hook,
                stage,
                seq,
                panicked,
            } => Some((hook.clone(), stage.clone(), *seq, *panicked)),
            _ => None,
        })
        .collect();
    // Both failing subscribers on every record of the run and on the two
    // end hooks' failure records, each failure recorded once (not again for
    // the records of their own failures).
    let run_records = log_lines(&plain_root).len();
    let audit_failures = failed.iter().filter(|f| f.1 == "audit").count();
    assert_eq!(audit_failures, 2 * (run_records + 2), "{failed:?}");
    assert!(failed.contains(&("broken".into(), "audit".into(), Some(1), true)));
    assert!(failed.contains(&("export".into(), "audit".into(), Some(1), false)));
    // The failing end hooks, after the run's end.
    let end_failures: Vec<_> = failed.iter().filter(|f| f.1 == "end").collect();
    assert_eq!(
        end_failures,
        [
            &("signer".to_owned(), "end".to_owned(), None, false),
            &("crashy".to_owned(), "end".to_owned(), None, true)
        ]
    );
    let run_end = events
        .iter()
        .position(|e| matches!(e, AuditEvent::RunEnd { .. }))
        .unwrap();
    assert!(
        events[run_end + 1..]
            .iter()
            .all(|e| matches!(e, AuditEvent::HookFailed { .. }))
    );
    // The good subscriber was told of everything, the failure records too.
    assert_eq!(good.seen().len(), log_lines(&root).len());
    // The reported head is the head at `run_end` (with the audit failures
    // before it); the end hooks' failures follow it.
    let report = &receipts.reports()[0];
    let lines = log_lines(&root);
    let at_end = report.chain.as_ref().unwrap().records as usize;
    assert_eq!(
        report.chain.as_ref().unwrap().head,
        digest(&lines[at_end - 1])
    );
    assert!(lines.len() > at_end);
    chain_is_intact(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_calls_the_end_hook_once_per_invocation() {
    let (_d, root) = workspace();
    let ends = Ends::named("receipts", None);
    let recorder = Recorder::named("records", None);
    let hooks = Hooks::default()
        .on_end(ends.clone())
        .subscribe(recorder.clone());
    let limits = SessionLimits {
        frontier_usd: 100.0,
        working_time: Duration::from_secs(3600),
    };
    for (resume, closed) in [(false, false), (true, true)] {
        let flag = Arc::new(AtomicBool::new(false));
        let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
        pc.backoff_scale = 0.0;
        let frontier = Frontier::new(
            vec![Step::Call("reply", json!({"message": "Here."}))],
            &flag,
        );
        let provider = ChatProvider::new(pc, Box::new(frontier)).unwrap();
        let mut log = AuditLog::open_anchored(&audit_path(&root), &anchors(&root)).unwrap();
        hooks.attach(&mut log);
        let gated = OutboundGate::new(log).wrap(provider);
        gated.audit().record(AuditEvent::RunStart {
            mode: "passthrough".into(),
            boundary: false,
        });
        let cfg = RunConfig::new(root.join("ws"), run_dir(&root), "Hello.");
        let git = duet_git::Git::locate().unwrap();
        let mut session =
            Session::open(&cfg, &gated, &PASS, &git, flag.clone(), limits, resume).unwrap();
        let message = if resume { "Still there?" } else { "Hello." };
        assert_eq!(session.turn(message).await.state(), "replied");
        let (terminal, stats) = session.end(closed);
        duet_agent::conclude_with(
            &cfg.run_dir,
            RUN_ID,
            Some(gated.audit()),
            &audit_path(&root),
            &terminal,
            &stats,
            &Ending {
                kind: RunKind::Session,
                mode: "passthrough",
                resumed: resume,
                policy: None,
                hooks: &hooks,
            },
        )
        .unwrap();
        let reports = ends.reports();
        assert_eq!(reports.len(), if resume { 2 } else { 1 });
        let r = reports.last().unwrap();
        assert_eq!(r.kind, RunKind::Session);
        assert_eq!(r.resumed, resume);
        if closed {
            assert_eq!(r.state, "completed");
            assert!(!r.resumable);
        } else {
            assert_eq!(
                terminal,
                Terminal::Failed {
                    reason: SESSION_LEFT.into()
                }
            );
            assert_eq!(r.state, "failed");
            assert!(r.resumable && !r.interrupted);
        }
        head_is_the_logs(r, &log_lines(&root));
    }
    // The operator's messages were audited as counts, and a subscriber was
    // told of them without their text.
    let seen = recorder.seen();
    assert_eq!(
        seen.iter()
            .filter(|r| matches!(
                &r.record,
                Recorded::Event {
                    event: AuditEvent::OperatorMessage { .. }
                }
            ))
            .count(),
        2
    );
    let told = serde_json::to_string(&seen).unwrap();
    assert!(!told.contains("Still there?"), "{told}");
    chain_is_intact(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn in_hybrid_mode_no_hook_is_told_a_sensitive_value() {
    const SECRET: &str = "sk_live_51Hq8ZtLm4Vb2Xc9Rw7Ty3Ui";
    const EMAIL: &str = "ingrid.solberg@fjordpost.example";
    let (_d, root) = workspace();
    let ws = root.join("ws");
    std::fs::write(ws.join(".env"), format!("STRIPE_KEY={SECRET}\n")).unwrap();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,email\n1,{EMAIL}\n"),
    )
    .unwrap();
    let engine = Engine::open(
        &run_dir(&root),
        duet_boundary::policy::Policy {
            sensitive_globs: vec![".env*".into(), "data/**".into()],
            command_output_sensitive: true,
            detect_secrets: true,
            detect_pii: true,
            bulky_tokens: 2000,
            bulky_file_tokens: 12000,
            ..duet_boundary::policy::Policy::default()
        },
        None,
    )
    .unwrap();
    engine.prime(
        &ws,
        &[".env".to_owned(), "data/customers.csv".to_owned()],
        "Tidy src/lib.rs.",
    );
    let recorder = Recorder::named("records", None);
    let ends = Ends::named("receipts", None);
    let hooks = Hooks::default()
        .subscribe(recorder.clone())
        .on_end(ends.clone());
    let mut run = Invocation::new(
        &root,
        &hooks,
        vec![
            Step::Call("read_file", json!({"path": ".env"})),
            Step::Call("read_file", json!({"path": "data/customers.csv"})),
            Step::Call(
                "run_command",
                json!({"command": "cat .env data/customers.csv"}),
            ),
            Step::Call("finish", json!({"summary": "done"})),
        ],
    );
    run.engine = Some(engine);
    let (terminal, _) = run.run().await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    let told = format!(
        "{}{}",
        serde_json::to_string(&recorder.seen()).unwrap(),
        serde_json::to_string(&ends.reports()).unwrap()
    );
    for planted in [SECRET, EMAIL, "STRIPE_KEY=", "fjordpost"] {
        assert!(!told.contains(planted), "{planted} reached a hook: {told}");
    }
    // The hooks were told of the run: its requests and its end.
    assert_eq!(ends.reports().len(), 1);
    assert!(recorder.seen().len() >= 6);
    chain_is_intact(&root);
}
