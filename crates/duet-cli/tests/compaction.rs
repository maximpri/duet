// SPDX-License-Identifier: GPL-3.0-or-later
//! Context compaction (`duet_agent::compaction`) through the frontier loop,
//! with a scripted frontier that records every request byte for byte and a
//! scripted local model:
//!
//! - a long hybrid run is compacted: the first message and the recent turns
//!   stay, every tool call keeps its result, and the requests after a
//!   compaction only append to it (the new prefix is stable);
//! - a run interrupted after a compaction resumes with the same request,
//!   byte for byte;
//! - without a summary (a local model that answers nonsense, or none at all:
//!   pass-through, or hybrid with `local.enabled` off) the run masks as
//!   before and completes;
//! - planted values never reach the frontier through a summary, even when
//!   the local stand-in writes them into it, and the local model reads only
//!   what the frontier was sent;
//! - in a session, the operator's latest message survives a compaction
//!   verbatim.
//!
//! Nothing here contacts a model server.

mod privacy;

use bytes::Bytes;
use duet_agent::compaction::SUMMARY_PREFIX;
use duet_agent::transcript::{Entry, Transcript};
use duet_agent::{Compaction, RunConfig, RunStats, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{AuditEvent, AuditLog, Line, read};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::testing::Received;
use duet_boundary::view::{PassThrough, Presenter};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use privacy::{Echo, Fixture, Local, Step, is_condense};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RUN_ID: &str = "20260926-000000-compact";
const KEY: &str = "sk_live_Hq3Nw8Zt5Lm2Xr9Vb4Kd";
/// Public files the frontier reads, about 4 KB each.
const PARTS: usize = 8;
/// A summary the cooperative local model writes.
const NOTES: &str = "Task: make the parser accept grouped patterns.\nDone: read src/part1.rs to src/part8.rs.\nOpen: change format_integer in src/part3.rs.";

/// Replies with one tool call per request, in order, and records each body.
/// When `interrupt` is set, it raises the flag once this many requests
/// carrying a summary have been answered.
#[derive(Clone, Default)]
struct Frontier {
    calls: Arc<Mutex<VecDeque<(String, Value)>>>,
    bodies: Arc<Mutex<Vec<String>>>,
    interrupt: Option<(usize, Arc<AtomicBool>)>,
}

impl Frontier {
    fn new(calls: Vec<(&str, Value)>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(
                calls.into_iter().map(|(n, a)| (n.to_owned(), a)).collect(),
            )),
            ..Self::default()
        }
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let n = {
            let mut bodies = self.bodies.lock().unwrap();
            bodies.push(String::from_utf8(body).unwrap());
            if let Some((after, flag)) = &self.interrupt
                && bodies.iter().filter(|b| b.contains(SUMMARY_PREFIX)).count() == *after
            {
                flag.store(true, Ordering::SeqCst);
            }
            bodies.len()
        };
        let (name, args) = self
            .calls
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(("finish".into(), json!({"summary": "done"})));
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

/// Plausible public source of about `bytes` bytes.
fn source(tag: usize, bytes: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    while out.len() < bytes {
        out.push_str(&format!(
            "/// Formats group {n} of part {tag}.\npub fn format_group_{tag}_{n}(value: i64) -> String {{\n    format!(\"{{value:>{n}}}\")\n}}\n"
        ));
        n += 1;
    }
    out
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join(".env"), format!("NOVAPAY_API_KEY={KEY}\n")).unwrap();
    for k in 1..=PARTS {
        std::fs::write(ws.join(format!("src/part{k}.rs")), source(k, 4000)).unwrap();
    }
    (d, root)
}

/// `reads` reads of the public files, then `finish`.
fn reads(reads: usize) -> Vec<(&'static str, Value)> {
    let mut calls: Vec<(&str, Value)> = (0..reads)
        .map(|i| {
            (
                "read_file",
                json!({ "path": format!("src/part{}.rs", 1 + i % PARTS) }),
            )
        })
        .collect();
    calls.push(("finish", json!({"summary": "done"})));
    calls
}

/// What the local stand-in writes: the notes for a summary request
/// (numbered, so each summary differs), a digest otherwise; `broken`
/// answers every summary request with prose.
fn local_model(broken: bool) -> (duet_boundary::local::LocalReader, Received) {
    let written = std::sync::atomic::AtomicUsize::new(0);
    duet_boundary::testing::responsive_local(move |prompt| {
        if is_condense(prompt) {
            if broken {
                "I would rather not.".into()
            } else {
                let n = written.fetch_add(1, Ordering::SeqCst) + 1;
                json!({ "summary": format!("{NOTES}\nSummary {n}.") }).to_string()
            }
        } else {
            json!({"summary": "A file.", "facts": []}).to_string()
        }
    })
}

fn policy() -> Policy {
    Policy {
        sensitive_globs: vec![".env*".into()],
        command_output_sensitive: true,
        secret_sinks: vec![".env*".into()],
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 4000,
        bulky_file_tokens: 12000,
        ..Policy::default()
    }
}

/// How a run is composed.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Setup {
    /// Hybrid with a local model that writes summaries.
    Hybrid,
    /// Hybrid with a local model that answers summary requests with prose.
    BrokenLocal,
    /// Hybrid without a local model (`local.enabled = false`).
    NoLocal,
    /// Pass-through: no engine, no local model.
    PassThrough,
}

struct Ended {
    terminal: Terminal,
    stats: RunStats,
    bodies: Vec<String>,
    local: Received,
    entries: Vec<Entry>,
    events: Vec<AuditEvent>,
}

/// One run (or resume) in `root/ws`, as `duet run` composes it: hybrid (the
/// engine opened on the run directory and primed, its filter and check on
/// the gate) or pass-through (no engine, an empty gate).
async fn run(
    root: &Path,
    frontier: Frontier,
    setup: Setup,
    context_window: u64,
    resume: bool,
    interrupted: Arc<AtomicBool>,
) -> Ended {
    let ws = root.join("ws");
    let run_dir = ws.join(".duet/runs").join(RUN_ID);
    std::fs::create_dir_all(&run_dir).unwrap();
    let audit_path = root.join("audit.jsonl");
    let objective = "Make the parser accept grouped patterns.";
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let (reader, local) = local_model(setup == Setup::BrokenLocal);
    let hybrid = setup != Setup::PassThrough;
    let reader = (setup != Setup::NoLocal).then_some(reader);
    let engine = hybrid.then(|| {
        let e = Engine::open(&run_dir, policy(), reader).unwrap();
        e.prime(&ws, &[], objective);
        e
    });
    let mut gate = OutboundGate::new(AuditLog::open(&audit_path).unwrap());
    if let Some(e) = &engine {
        let (filter, check) = e.outbound();
        gate = gate.with_filter(filter).with_check(check);
    }
    let gated = gate.wrap(provider);
    let passthrough = PassThrough { max_bytes: 60_000 };
    let presenter: &dyn Presenter = match &engine {
        Some(e) => e.as_ref(),
        None => &passthrough,
    };
    let cfg = RunConfig {
        mode: if hybrid { "hybrid" } else { "passthrough" }.into(),
        wall_clock: Duration::from_secs(120),
        context_window,
        compaction: Some(Compaction { at: 9000, to: 0.4 }),
        price: Box::new(|u| u.input as f64 / 1e6),
        ..RunConfig::new(ws.clone(), run_dir.clone(), objective)
    };
    let git = duet_git::Git::locate().unwrap();
    let (terminal, stats) =
        duet_agent::run(&cfg, &gated, presenter, &git, resume, &interrupted).await;
    let events = read(&audit_path)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Event(e) => Some(e.event),
            Line::Request(_) => None,
        })
        .collect();
    Ended {
        terminal,
        stats,
        bodies: frontier.bodies(),
        local,
        entries: Transcript::read(&run_dir).unwrap(),
        events,
    }
}

fn completed(t: &Terminal) -> bool {
    matches!(t, Terminal::Completed { .. })
}

fn messages(body: &str) -> Vec<Value> {
    let v: Value = serde_json::from_str(body).unwrap();
    v["messages"].as_array().unwrap().clone()
}

/// The summary message of a request, if it carries one.
fn summary_of(body: &str) -> Option<String> {
    messages(body)
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str())
        .find(|t| t.starts_with(SUMMARY_PREFIX))
        .map(str::to_owned)
}

/// Every tool message answers a call of the assistant message before it,
/// and every call is answered.
fn assert_paired(body: &str) {
    let mut open: HashSet<String> = HashSet::new();
    for m in messages(body) {
        match m["role"].as_str() {
            Some("assistant") => {
                assert!(open.is_empty(), "unanswered calls {open:?}");
                open = m["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| c["id"].as_str().unwrap().to_owned())
                    .collect();
            }
            Some("tool") => {
                let id = m["tool_call_id"].as_str().unwrap();
                assert!(open.remove(id), "a result without its call: {id}");
            }
            _ => {}
        }
    }
}

fn count(entries: &[Entry], f: fn(&Entry) -> bool) -> usize {
    entries.iter().filter(|e| f(e)).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_run_is_compacted_and_the_new_prefix_stays_stable() {
    let (_d, root) = workspace();
    let frontier = Frontier::new(reads(30));
    let ended = run(
        &root,
        frontier,
        Setup::Hybrid,
        200_000,
        false,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(completed(&ended.terminal), "{:?}", ended.terminal);
    let compacted = count(&ended.entries, |e| matches!(e, Entry::Compacted { .. }));
    assert!(compacted >= 2, "{compacted} compaction(s)");
    assert_eq!(ended.stats.compactions as usize, compacted);
    assert_eq!(
        ended
            .events
            .iter()
            .filter(
                |e| matches!(e, AuditEvent::Compaction { outcome, .. } if outcome == "compacted")
            )
            .count(),
        compacted
    );
    let first = messages(&ended.bodies[0]);
    let mut stable_pairs = 0;
    for (i, body) in ended.bodies.iter().enumerate() {
        assert_paired(body);
        let m = messages(body);
        // The system prompt and the task never change.
        assert_eq!(m[..2], first[..2], "request {i}");
        let Some(summary) = summary_of(body) else {
            continue;
        };
        assert_eq!(
            m[2]["content"],
            summary.as_str(),
            "the summary follows the task"
        );
        assert!(summary.contains(NOTES));
        assert_eq!(
            m[3]["role"], "assistant",
            "the recent turns start with a turn"
        );
        // Until the next compaction, the next request only appends.
        if let Some(next) = ended.bodies.get(i + 1)
            && summary_of(next).as_deref() == Some(summary.as_str())
        {
            let n = messages(next);
            assert!(n.len() > m.len());
            assert_eq!(n[..m.len()], m[..], "request {} extends request {i}", i + 1);
            stable_pairs += 1;
        }
    }
    assert!(stable_pairs >= 3, "{stable_pairs}");
    // The local model read the conversation as it was sent.
    let prompts: Vec<String> = (0..ended.local.bodies().len())
        .map(|i| ended.local.prompt(i))
        .filter(|p| is_condense(p))
        .collect();
    assert_eq!(prompts.len(), compacted);
    assert!(prompts[0].contains("call read_file {\"path\":\"src/part1.rs\"}"));
    assert!(prompts.iter().all(|p| !p.contains(KEY)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_resumed_after_a_compaction_sends_the_same_request() {
    let (_d, root) = workspace();
    let flag = Arc::new(AtomicBool::new(false));
    let script = reads(30);
    let frontier = Frontier {
        interrupt: Some((3, flag.clone())),
        ..Frontier::new(script.clone())
    };
    let first = run(&root, frontier, Setup::Hybrid, 200_000, false, flag).await;
    assert!(
        matches!(&first.terminal, Terminal::Failed { reason } if reason.contains("interrupted")),
        "{:?}",
        first.terminal
    );
    let sent = first.bodies.len();
    assert!(count(&first.entries, |e| matches!(e, Entry::Compacted { .. })) >= 1);
    // The interrupted turn is asked again: the rest of the script from there.
    let rest: Vec<(&str, Value)> = script[sent - 1..].to_vec();
    let again = run(
        &root,
        Frontier::new(rest),
        Setup::Hybrid,
        200_000,
        true,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(completed(&again.terminal), "{:?}", again.terminal);
    assert_eq!(
        again.bodies[0],
        first.bodies[sent - 1],
        "the resumed request is the interrupted one, byte for byte"
    );
    assert!(summary_of(&again.bodies[0]).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_summary_the_run_masks_as_before_and_completes() {
    // A local model that answers a summary request with prose: each attempt
    // fails, the run waits for growth before the next and masks at the
    // window as it always did.
    let (_d, root) = workspace();
    let ended = run(
        &root,
        Frontier::new(reads(30)),
        Setup::BrokenLocal,
        24_000,
        false,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(completed(&ended.terminal), "{:?}", ended.terminal);
    let failed = count(&ended.entries, |e| {
        matches!(e, Entry::CompactionFailed { .. })
    });
    assert!(failed >= 1);
    assert!(failed < 10, "retries wait for growth: {failed}");
    assert_eq!(
        count(&ended.entries, |e| matches!(e, Entry::Compacted { .. })),
        0
    );
    assert!(count(&ended.entries, |e| matches!(e, Entry::Masked { .. })) >= 1);
    assert!(
        ended
            .events
            .iter()
            .any(|e| matches!(e, AuditEvent::Compaction { outcome, .. } if outcome == "failed"))
    );
    assert!(ended.bodies.iter().all(|b| summary_of(b).is_none()));

    // Without a local model (pass-through, or hybrid with `local.enabled`
    // off) compaction never starts: no attempt, no failure, masking as before.
    for setup in [Setup::PassThrough, Setup::NoLocal] {
        let (_d, root) = workspace();
        let ended = run(
            &root,
            Frontier::new(reads(30)),
            setup,
            24_000,
            false,
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert!(
            completed(&ended.terminal),
            "{setup:?}: {:?}",
            ended.terminal
        );
        assert!(
            ended
                .entries
                .iter()
                .all(|e| !matches!(e, Entry::Compacted { .. } | Entry::CompactionFailed { .. })),
            "{setup:?}"
        );
        assert!(
            ended
                .events
                .iter()
                .all(|e| !matches!(e, AuditEvent::Compaction { .. }))
        );
        assert!(count(&ended.entries, |e| matches!(e, Entry::Masked { .. })) >= 1);
        assert!(ended.local.bodies().is_empty(), "{setup:?}");
    }
}

// --- Privacy: the planted values of the privacy scenarios ------------------

/// Public files large enough to grow the conversation past the threshold.
fn grow(f: &Fixture) {
    for k in 1..=PARTS {
        std::fs::write(f.ws.join(format!("src/part{k}.rs")), source(k, 4000)).unwrap();
    }
}

/// Reads of the growing files.
fn growing(n: usize) -> Vec<Step> {
    (0..n)
        .map(|i| {
            Step::Call(
                "read_file",
                json!({ "path": format!("src/part{}.rs", 1 + i % PARTS) }),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn planted_values_never_reach_the_frontier_through_a_summary() {
    for local in [Local::Cooperative, Local::Careless(Echo::ALL)] {
        let mut f = Fixture::new(
            "compact-leak",
            "Make total skip refunds; the customer data is in data/customers.csv.",
            local,
        );
        f.cfg.compaction = Some(Compaction { at: 9000, to: 0.4 });
        grow(&f);
        let mut steps = vec![
            Step::Call("read_file", json!({"path": ".env"})),
            Step::Call("read_file", json!({"path": "data/customers.csv"})),
            Step::ask_latest("What is the card number on line 2?"),
            Step::Call("run_command", json!({"command": "git log --oneline"})),
        ];
        steps.extend(growing(24));
        steps.push(Step::Call("finish", json!({"summary": "done"})));
        f.script(steps);
        let end = f.run().await;
        assert!(
            matches!(end, Terminal::Completed { .. }),
            "{local:?}: {end:?}"
        );
        let bodies = f.bodies();
        assert!(
            bodies
                .iter()
                .any(|b| String::from_utf8_lossy(b).contains(SUMMARY_PREFIX)),
            "{local:?}: a summary reached the frontier"
        );
        if let Local::Careless(_) = local {
            // The values were written, and replaced in every form.
            let summary = bodies
                .iter()
                .find_map(|b| summary_of(&String::from_utf8_lossy(b)))
                .unwrap();
            for marker in [
                "Values to keep: ⟨secret:",
                "⟨redacted:digits-of-a-withheld-number⟩",
                "Spelled out: ⟨secret:",
                "In base64: ⟨redacted:encoded-sensitive-text⟩",
            ] {
                assert!(summary.contains(marker), "{marker}: {summary}");
            }
        }
        let canaries = f.canaries([]);
        f.assert_no_leak(&canaries);
        // The local model read what the frontier was sent: no planted value.
        let condensed: Vec<String> = f
            .local_prompts()
            .into_iter()
            .filter(|p| is_condense(p))
            .collect();
        assert!(!condensed.is_empty());
        for p in &condensed {
            assert!(
                canaries.find(p).is_empty(),
                "{local:?}: a planted value in the compaction input: {:?}",
                canaries.find(p)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sessions_latest_operator_message_survives_a_compaction_verbatim() {
    let mut f = Fixture::new(
        "compact-session",
        "Look at src/lib.rs; total should skip refunds.",
        Local::Cooperative,
    );
    f.cfg.compaction = Some(Compaction { at: 9000, to: 0.4 });
    grow(&f);
    let ask = "Now also make every format_group function pad with zeros, please.";
    let mut steps = vec![Step::Call("reply", json!({"message": "Noted."}))];
    steps.extend(growing(24));
    steps.push(Step::Call("reply", json!({"message": "Done."})));
    f.script(steps);
    let mut s = f.session();
    assert_eq!(s.turn(&f.cfg.objective).await.state(), "replied");
    assert_eq!(s.turn(ask).await.state(), "replied");
    let _ = s.end(true);
    let summaries: Vec<String> = f
        .bodies()
        .iter()
        .filter_map(|b| summary_of(&String::from_utf8_lossy(b)))
        .collect();
    assert!(!summaries.is_empty(), "the session was compacted");
    for text in &summaries {
        assert!(text.ends_with(ask), "{text}");
    }
    f.assert_no_leak(&f.canaries([]));
}
