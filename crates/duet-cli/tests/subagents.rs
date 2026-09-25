// SPDX-License-Identifier: GPL-3.0-or-later
//! Sub-agents (`delegate`) through the frontier loop with a scripted frontier
//! that answers each conversation (the parent's, each sub-agent's) from its
//! own script: fresh context and a fixed tool set per mode, reports in the
//! parent's result, no planted value reaching the frontier in hybrid mode,
//! writes confined to the given paths, read-only commands, depth 1, parallel
//! read sub-agents, budgets, interrupt, resume after a crash mid-sub-agent,
//! session undo, and a separate sub-agent model on the same audit chain.
//! Nothing here contacts a model server.
#![cfg(any(target_os = "macos", target_os = "linux"))]

use bytes::Bytes;
use duet_agent::driver::Driver;
use duet_agent::session::SessionLimits;
use duet_agent::subagents::Subagents;
use duet_agent::transcript::{Entry, Transcript};
use duet_agent::{RunConfig, RunStats, Session, Terminal, TurnEnd};
use duet_boundary::audit::{AuditEvent, AuditLog, Line, Verification, read, verify};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::testing::canary::Canaries;
use duet_boundary::view::{PassThrough, Presenter};
use duet_boundary::{GatedFrontier, OutboundGate};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SECRET: &str = "sk_live_9f8e7d6c5b4a39281706";
const EMAIL: &str = "marta.kowalczyk@corp-mail.net";
const CARD: &str = "4539578763621486";
const OBJECTIVE: &str = "PARENT: fix the CSV export.";

/// What the scripted frontier answers one request with.
enum Step {
    /// Tool calls, in order, in one response.
    Calls(Vec<(&'static str, Value)>),
    /// `ask_local` about the first handle named in the last tool result.
    AskFirstHandle,
    /// Never answers.
    Hang,
    /// Answers with the step after this many milliseconds.
    Slow(u64, Box<Step>),
}

fn call(name: &'static str, args: Value) -> Step {
    Step::Calls(vec![(name, args)])
}

/// Each conversation's script, by the marker its first user message holds.
type Scripts = Arc<Mutex<Vec<(String, VecDeque<Step>)>>>;

/// Answers each conversation from the script whose marker its first user
/// message contains, and records every request body. Counts the requests
/// in flight (sub-agents running at the same time).
#[derive(Clone, Default)]
struct Frontier {
    scripts: Scripts,
    bodies: Arc<Mutex<Vec<String>>>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

fn first_user(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

fn last_content(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .last()
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

impl Frontier {
    fn script(&self, marker: &str, steps: Vec<Step>) {
        self.scripts
            .lock()
            .unwrap()
            .push((marker.to_owned(), steps.into()));
    }

    fn bodies(&self) -> Vec<Value> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .map(|b| serde_json::from_str(b).unwrap())
            .collect()
    }

    fn raw(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }

    /// The requests of the conversation whose first message holds `marker`.
    fn of(&self, marker: &str) -> Vec<Value> {
        self.bodies()
            .into_iter()
            .filter(|b| first_user(b).contains(marker))
            .collect()
    }
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let text = String::from_utf8(body).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        let first = first_user(&value);
        let n = {
            let mut bodies = self.bodies.lock().unwrap();
            bodies.push(text);
            bodies.len()
        };
        let mut step = {
            let mut scripts = self.scripts.lock().unwrap();
            let (_, queue) = scripts
                .iter_mut()
                .find(|(m, _)| first.contains(m.as_str()))
                .unwrap_or_else(|| panic!("no script for {first}"));
            queue
                .pop_front()
                .unwrap_or_else(|| panic!("the script for {first} ran out"))
        };
        let mut delay = 0;
        while let Step::Slow(ms, inner) = step {
            delay += ms;
            step = *inner;
        }
        let calls = |calls: Vec<(&str, Value)>| {
            let calls: Vec<Value> = calls
                .into_iter()
                .enumerate()
                .map(|(i, (name, args))| {
                    json!({"index": i, "id": format!("c{n}_{i}"), "type": "function",
                        "function": {"name": name, "arguments": args.to_string()}})
                })
                .collect();
            json!({ "tool_calls": calls })
        };
        let delta = match step {
            Step::Hang => return Box::pin(futures_util::future::pending()),
            Step::Slow(..) => unreachable!(),
            Step::Calls(list) => calls(list),
            Step::AskFirstHandle => {
                let last = last_content(&value);
                let handle = last
                    .split("handle=\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or("h1")
                    .to_owned();
                calls(vec![(
                    "ask_local",
                    json!({"handle": handle, "question": "Which email and card does row 1 hold?"}),
                )])
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
        let (active, peak) = (self.active.clone(), self.peak.clone());
        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(HttpReply {
                status: 200,
                headers: vec![],
                body: futures_util::stream::iter(chunks.map(|c| Ok(Bytes::from(c)))).boxed(),
            })
        })
    }
}

/// 1000 prompt tokens per request: $0.001 each.
fn price(u: &duet_boundary::model::Usage) -> f64 {
    u.input as f64 / 1e6
}

fn subagents(max_parallel: usize, max_usd: f64) -> Subagents {
    Subagents {
        max_parallel,
        max_usd,
        max_time: Duration::from_secs(60),
        model: None,
        price: Arc::new(price),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: PathBuf,
    run_dir: PathBuf,
    log: PathBuf,
    frontier: Frontier,
    engine: Option<Arc<Engine>>,
    git: duet_git::Git,
    interrupted: Arc<AtomicBool>,
}

const PASS: PassThrough = PassThrough { max_bytes: 60_000 };

/// A local model that repeats the sensitive values it read in every answer
/// and summary: the boundary must clean what it says.
fn careless_local() -> duet_boundary::local::LocalReader {
    let reply = json!({
        "summary": format!("Row 1 is {EMAIL} with card {CARD}; key {SECRET}."),
        "facts": [format!("email {EMAIL}"), format!("card {CARD}")],
        "answer": format!("The email is {EMAIL} and the card is {CARD}."),
        "evidence_lines": [2],
        "unanswerable": false
    })
    .to_string();
    duet_boundary::testing::scripted_local(vec![reply; 40]).0
}

/// A workspace with public source, a secret file and customer data, as a
/// git repository; `hybrid` puts the security engine in front.
fn fixture(hybrid: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join("src/export")).unwrap();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub mod export;\n").unwrap();
    std::fs::write(
        ws.join("src/export/mod.rs"),
        "pub fn export_rows(rows: &[String]) -> String {\n    rows[..rows.len() - 1].join(\"\\n\")\n}\n",
    )
    .unwrap();
    std::fs::write(ws.join(".env"), format!("STRIPE_KEY={SECRET}\n")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,email,card\n1,{EMAIL},{CARD}\n"),
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
    let run_dir = ws.join(".duet/runs/r1");
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
        let engine = Engine::open(&run_dir, policy, Some(careless_local())).unwrap();
        let files = [
            ".env".to_owned(),
            "data/customers.csv".to_owned(),
            "src/lib.rs".to_owned(),
            "src/export/mod.rs".to_owned(),
        ];
        engine.prime(&ws, &files, OBJECTIVE);
        engine
    });
    Fixture {
        _dir: dir,
        log: root.join("audit.jsonl"),
        ws,
        run_dir,
        frontier: Frontier::default(),
        engine,
        git: duet_git::Git::locate().unwrap(),
        interrupted: Arc::new(AtomicBool::new(false)),
    }
}

impl Fixture {
    fn presenter(&self) -> &dyn Presenter {
        match &self.engine {
            Some(e) => e.as_ref(),
            None => &PASS,
        }
    }

    /// A gate on the run's audit log (reopened, as a resumed run does).
    fn gated(&self, frontier: &Frontier, model: &str) -> GatedFrontier {
        let mut pc = ProviderConfig::new("https://frontier.example/v1", model, Role::Frontier);
        pc.backoff_scale = 0.0;
        let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
        let mut gate = OutboundGate::new(AuditLog::open(&self.log).unwrap());
        if let Some(e) = &self.engine {
            let (filter, check) = e.outbound();
            gate = gate.with_filter(filter).with_check(check);
        }
        gate.wrap(provider)
    }

    fn config(&self, sub: Option<Subagents>, frontier_usd: f64) -> RunConfig {
        RunConfig {
            mode: if self.engine.is_some() {
                "hybrid"
            } else {
                "passthrough"
            }
            .into(),
            sandbox: duet_sandbox::detect().unwrap(),
            wall_clock: Duration::from_secs(120),
            frontier_usd,
            price: Box::new(price),
            subagents: sub,
            ..RunConfig::new(self.ws.clone(), self.run_dir.clone(), OBJECTIVE)
        }
    }

    async fn run(
        &self,
        cfg: &RunConfig,
        gated: &GatedFrontier,
        resume: bool,
    ) -> (Terminal, RunStats) {
        duet_agent::run(
            cfg,
            gated,
            self.presenter(),
            &self.git,
            resume,
            &self.interrupted,
        )
        .await
    }

    fn entries(&self) -> Vec<Entry> {
        Transcript::read(&self.run_dir).unwrap()
    }

    fn events(&self) -> Vec<AuditEvent> {
        read(&self.log)
            .unwrap()
            .into_iter()
            .filter_map(|l| match l {
                Line::Event(e) => Some(e.event),
                Line::Request(_) => None,
            })
            .collect()
    }
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}

fn system(body: &Value) -> String {
    body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The tool results in a request, in order.
fn results(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn subagent_ends(entries: &[Entry]) -> Vec<(String, Terminal)> {
    entries
        .iter()
        .filter_map(|e| match e {
            Entry::SubagentEnd {
                child, terminal, ..
            } => Some((child.clone(), terminal.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_sub_agent_starts_fresh_with_its_own_tools_and_reports_back() {
    let f = fixture(false);
    let task = "[child A] Find the function that exports rows and report its file and line.";
    f.frontier.script(
        "PARENT",
        vec![
            call("delegate", json!({"task": task, "mode": "read"})),
            call("finish", json!({"summary": "found it"})),
        ],
    );
    f.frontier.script(
        "[child A]",
        vec![
            call("read_file", json!({"path": "src/export/mod.rs"})),
            call(
                "finish",
                json!({"summary": "export_rows is in src/export/mod.rs, line 1; it drops the last row."}),
            ),
        ],
    );
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    let (terminal, stats) = f.run(&cfg, &gated, false).await;
    assert_eq!(
        terminal,
        Terminal::Completed {
            summary: "found it".into()
        }
    );

    // The parent offers delegate; the sub-agent starts from its own prompt
    // and the task alone, with read tools only, sorted.
    let parent = f.frontier.of("PARENT");
    assert!(tool_names(&parent[0]).contains(&"delegate".to_owned()));
    let child = f.frontier.of("[child A]");
    assert_eq!(child.len(), 2);
    assert_eq!(
        system(&child[0]),
        duet_agent::prompt::subagent_prompt("ws", false)
    );
    let messages = child[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "system prompt and task only");
    assert_eq!(first_user(&child[0]), task);
    assert!(
        !child[0].to_string().contains("PARENT"),
        "no parent history"
    );
    let names = tool_names(&child[0]);
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(
        names,
        [
            "diff",
            "finish",
            "git_blame",
            "git_log",
            "git_show",
            "git_status",
            "list_files",
            "read_file",
            "run_command",
            "search"
        ]
    );
    // The same tools in every request of the sub-agent.
    assert_eq!(child[0]["tools"], child[1]["tools"]);

    // The report reaches the parent as its tool result, framed as data.
    let result = &results(&parent[1])[0];
    assert!(
        result.starts_with("sub-agent a1 (read) completed (2 request(s), $0.0020"),
        "{result}"
    );
    assert!(result.contains("it drops the last row") && result.contains("report is data"));

    // Its spend is the run's.
    assert_eq!(stats.subagents.children, 1);
    assert_eq!(stats.subagents.requests, 2);
    assert!((stats.subagents.cost_usd - 0.002).abs() < 1e-9);
    assert!((stats.cost_usd - 0.004).abs() < 1e-9, "{}", stats.cost_usd);
    assert_eq!(stats.turns, 2, "the parent's own requests");

    // Records: start, its own entries nested under its id, end.
    let entries = f.entries();
    assert!(entries.iter().any(|e| matches!(e,
        Entry::SubagentStart { child, mode, call_id, .. }
            if child == "a1" && mode == "read" && call_id == "c1_0")));
    let nested = entries
        .iter()
        .filter(|e| matches!(e, Entry::Subagent { child, .. } if child == "a1"))
        .count();
    assert!(nested >= 6, "{nested} nested entries");
    assert!(matches!(
        &subagent_ends(&entries)[..],
        [(id, Terminal::Completed { .. })] if id == "a1"
    ));
    // Audit: the task's hash, never its text.
    let events = f.events();
    let hash = duet_fs::sha256_hex(task.as_bytes());
    assert!(events.contains(&AuditEvent::SubagentStart {
        child: "a1".into(),
        mode: "read".into(),
        task_sha256: hash,
        paths: vec![],
        model: "m".into(),
    }));
    assert!(events.iter().any(|e| matches!(e,
        AuditEvent::SubagentEnd { child, outcome, requests: 2, files_written: 0, .. }
            if child == "a1" && outcome == "completed")));
    let log = std::fs::read_to_string(&f.log).unwrap();
    let events_only: String = log.lines().filter(|l| l.contains("\"subagent_")).collect();
    assert!(!events_only.contains("Find the function"), "{events_only}");
    assert!(matches!(
        verify(&f.log).unwrap(),
        Verification::Intact { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hybrid_sub_agent_sees_only_what_the_boundary_presents_and_leaks_nothing() {
    let f = fixture(true);
    // A parent that (somehow) writes a real value into the task, and a
    // sub-agent that tries every way to the data and copies values into its
    // report: nothing planted may reach the frontier.
    let task = format!("[child A] Check what the export does for the customer {EMAIL}.");
    f.frontier.script(
        "PARENT",
        vec![
            call("delegate", json!({"task": task, "mode": "read"})),
            call("finish", json!({"summary": "checked"})),
        ],
    );
    f.frontier.script(
        "[child A]",
        vec![
            call("read_file", json!({"path": "data/customers.csv"})),
            Step::AskFirstHandle,
            call("read_file", json!({"path": ".env"})),
            call(
                "run_command",
                json!({"command": "cat data/customers.csv .env; od -c data/customers.csv | head"}),
            ),
            call(
                "run_command",
                json!({"command": "cat data/customers.csv", "sensitive_data": true}),
            ),
            call("search", json!({"pattern": "@"})),
            call(
                "write_file",
                json!({"path": "leak.txt", "content": "anything"}),
            ),
            call(
                "finish",
                json!({"summary": format!("Row 1 is {EMAIL}, card {CARD}, key {SECRET}.")}),
            ),
        ],
    );
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    let (terminal, _) = f.run(&cfg, &gated, false).await;
    assert_eq!(
        terminal,
        Terminal::Completed {
            summary: "checked".into()
        }
    );

    let child = f.frontier.of("[child A]");
    assert_eq!(child.len(), 8);
    // It is told what its parent was told about the boundary.
    assert!(
        first_user(&child[0]).contains("Sensitive in this repository"),
        "{}",
        first_user(&child[0])
    );
    // In hybrid the sub-agent has ask_local and read_raw, and its command
    // cannot ask for sensitive data.
    let names = tool_names(&child[0]);
    assert!(names.contains(&"ask_local".into()) && names.contains(&"read_raw".into()));
    let run_command = child[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["function"]["name"] == "run_command")
        .unwrap();
    assert!(
        run_command["function"]["parameters"]["properties"]
            .get("sensitive_data")
            .is_none()
    );
    let last = results(&child[7]);
    assert!(last[0].contains("ask_local(handle="), "{}", last[0]);
    assert!(last[3].contains("Operation not permitted"), "{}", last[3]);
    assert!(
        last[4].contains("cannot read sensitive data"),
        "{}",
        last[4]
    );
    assert!(
        last[6].contains("not available to a read sub-agent"),
        "{}",
        last[6]
    );
    assert!(!f.ws.join("leak.txt").exists());

    // No planted value, in any spelling, in any request the frontier got.
    let canaries = Canaries::new([SECRET, EMAIL, CARD]);
    for (i, body) in f.frontier.raw().iter().enumerate() {
        let found = canaries.find(body);
        assert!(found.is_empty(), "request {i}: {found:?}\n{body}");
    }
    // Nor in the audit log (its request records hold what was sent).
    let log = std::fs::read_to_string(&f.log).unwrap();
    let found = canaries.find(&log);
    assert!(
        found.is_empty(),
        "audit log: {found:?}\n{}",
        found
            .iter()
            .map(|x| &log[x.offset.saturating_sub(120)..(x.offset + x.len + 40).min(log.len())])
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    // The values were there to leak: the local transcript holds the
    // sub-agent's report as it wrote it.
    let transcript = std::fs::read_to_string(f.run_dir.join("transcript.jsonl")).unwrap();
    assert!(!canaries.find(&transcript).is_empty());
    // The report reached the parent with the values replaced.
    let parent = f.frontier.of("PARENT");
    let report = &results(&parent[1])[0];
    assert!(report.contains("Row 1 is ⟨"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_sub_agent_writes_only_its_paths_and_its_commands_write_nothing() {
    let f = fixture(false);
    f.frontier.script(
        "PARENT",
        vec![
            call(
                "delegate",
                json!({"task": "[child W] Fix export_rows so it keeps the last row.",
                    "mode": "write", "paths": ["src/export/**"]}),
            ),
            call("finish", json!({"summary": "fixed"})),
        ],
    );
    f.frontier.script(
        "[child W]",
        vec![
            call(
                "edit_file",
                json!({"path": "src/export/mod.rs", "edits": [
                    {"old": "rows[..rows.len() - 1]", "new": "rows"}]}),
            ),
            call(
                "write_file",
                json!({"path": "src/export/tests.rs", "content": "// tests\n#[test]\nfn t() {}\n"}),
            ),
            call(
                "write_file",
                json!({"path": "src/lib.rs", "content": "pub mod other;\n"}),
            ),
            call(
                "run_command",
                json!({"command": "echo x > src/export/cmd.txt; echo y > outside.txt; cat src/lib.rs"}),
            ),
            call(
                "delegate",
                json!({"task": "[child X] deeper", "mode": "read"}),
            ),
            call("finish", json!({"summary": "export_rows keeps every row now."})),
        ],
    );
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    let (terminal, stats) = f.run(&cfg, &gated, false).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );

    // Its own paths were written; the rest was refused.
    assert!(
        std::fs::read_to_string(f.ws.join("src/export/mod.rs"))
            .unwrap()
            .contains("rows.join")
    );
    assert!(f.ws.join("src/export/tests.rs").exists());
    assert_eq!(
        std::fs::read_to_string(f.ws.join("src/lib.rs")).unwrap(),
        "pub mod export;\n"
    );
    // Commands cannot write at all, not even inside its paths.
    assert!(!f.ws.join("src/export/cmd.txt").exists());
    assert!(!f.ws.join("outside.txt").exists());
    let child = f.frontier.of("[child W]");
    let seen = results(child.last().unwrap());
    assert!(
        seen[2].contains("outside the paths this sub-agent may write (src/export/**)"),
        "{}",
        seen[2]
    );
    assert!(
        seen[3].contains("pub mod export"),
        "reads work: {}",
        seen[3]
    );
    // Depth 1: no delegate among its tools, and a call to it is refused.
    assert!(!tool_names(&child[0]).contains(&"delegate".to_owned()));
    assert!(tool_names(&child[0]).contains(&"write_file".to_owned()));
    assert!(seen[4].contains("cannot delegate"), "{}", seen[4]);
    assert!(f.frontier.of("[child X]").is_empty());
    assert_eq!(stats.subagents.children, 1);

    // The parent learns what changed.
    let parent = f.frontier.of("PARENT");
    let result = &results(&parent[1])[0];
    assert!(
        result.contains("src/export/mod.rs (modified, +1 -1)")
            && result.contains("src/export/tests.rs (created, 3 lines)"),
        "{result}"
    );
    // The writes are journaled like the parent's.
    let written: Vec<PathBuf> = duet_agent::journal::written(&f.run_dir)
        .into_iter()
        .map(|w| w.path)
        .collect();
    assert_eq!(
        written,
        [
            PathBuf::from("src/export/mod.rs"),
            PathBuf::from("src/export/tests.rs")
        ]
    );
    assert!(f.events().iter().any(|e| matches!(e,
        AuditEvent::SubagentStart { mode, paths, .. } if mode == "write" && paths == &["src/export/**"])));
}

#[tokio::test(flavor = "multi_thread")]
async fn read_sub_agents_asked_for_together_run_in_parallel_up_to_the_limit() {
    let f = fixture(false);
    let task =
        |id: &str| json!({"task": format!("[child {id}] Look at part {id}."), "mode": "read"});
    f.frontier.script(
        "PARENT",
        vec![
            Step::Calls(vec![
                ("delegate", task("A")),
                ("delegate", task("B")),
                ("delegate", task("C")),
            ]),
            call("finish", json!({"summary": "all three reported"})),
        ],
    );
    for id in ["A", "B", "C"] {
        f.frontier.script(
            &format!("[child {id}]"),
            vec![Step::Slow(
                400,
                Box::new(call(
                    "finish",
                    json!({"summary": format!("part {id} is fine")}),
                )),
            )],
        );
    }
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(2, 1.0)), 10.0);
    let (terminal, stats) = f.run(&cfg, &gated, false).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    assert_eq!(f.frontier.peak.load(Ordering::SeqCst), 2, "max_parallel");
    assert_eq!(stats.subagents.children, 3);

    // Results in the order the calls were made.
    let parent = f.frontier.of("PARENT");
    let got = results(&parent[1]);
    assert_eq!(got.len(), 3);
    for (i, id) in ["A", "B", "C"].iter().enumerate() {
        assert!(
            got[i].contains(&format!("sub-agent a{}", i + 1))
                && got[i].contains(&format!("part {id} is fine")),
            "{}",
            got[i]
        );
    }
    // Every read sub-agent has the same prompt and tools: one cached prefix.
    let a = &f.frontier.of("[child A]")[0];
    let c = &f.frontier.of("[child C]")[0];
    assert_eq!((system(a), &a["tools"]), (system(c), &c["tools"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sub_agent_that_spends_its_budget_stops_and_the_parent_pays_for_it() {
    let f = fixture(false);
    let read = || call("read_file", json!({"path": "src/lib.rs"}));
    f.frontier.script(
        "PARENT",
        vec![
            call(
                "delegate",
                json!({"task": "[child A] Read forever.", "mode": "read"}),
            ),
            call(
                "delegate",
                json!({"task": "[child B] Read forever too.", "mode": "read",
                    "budget": {"usd": 0.0004}}),
            ),
        ],
    );
    f.frontier.script("[child A]", vec![read(), read(), read()]);
    f.frontier.script("[child B]", vec![read(), read()]);
    let gated = f.gated(&f.frontier, "m");
    // Each sub-agent: at most $0.0015; the run: $0.0045.
    let cfg = f.config(Some(subagents(3, 0.0015)), 0.0045);
    let (terminal, stats) = f.run(&cfg, &gated, false).await;

    // A: two requests ($0.002), then stopped by subagents.max_usd; B asked
    // for less and stopped after one. The run then has spent 0.002 (parent)
    // + 0.003 (sub-agents) and stops on its own budget.
    assert_eq!(
        terminal,
        Terminal::BudgetStopped {
            which: "frontier_usd".into()
        }
    );
    assert_eq!(f.frontier.of("[child A]").len(), 2);
    assert_eq!(f.frontier.of("[child B]").len(), 1);
    let ends = subagent_ends(&f.entries());
    assert_eq!(
        ends,
        [
            (
                "a1".to_owned(),
                Terminal::BudgetStopped {
                    which: "subagents.max_usd".into()
                }
            ),
            (
                "a2".to_owned(),
                Terminal::BudgetStopped {
                    which: "budget.usd".into()
                }
            )
        ]
    );
    let parent = f.frontier.of("PARENT");
    let told = &results(&parent[1])[0];
    assert!(
        told.contains("sub-agent a1 (read) stopped: its budget (subagents.max_usd) is spent"),
        "{told}"
    );
    assert!((stats.cost_usd - 0.005).abs() < 1e-9, "{}", stats.cost_usd);
    assert!((stats.subagents.cost_usd - 0.003).abs() < 1e-9);
    assert!(f.events().iter().any(|e| matches!(e,
        AuditEvent::SubagentEnd { outcome, .. } if outcome == "budget_stopped")));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupt_stops_the_sub_agent_and_its_command_at_once() {
    let f = fixture(false);
    f.frontier.script(
        "PARENT",
        vec![call(
            "delegate",
            json!({"task": "[child A] Run the slow thing.", "mode": "read"}),
        )],
    );
    f.frontier.script(
        "[child A]",
        vec![call("run_command", json!({"command": "sleep 60"}))],
    );
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    let flag = f.interrupted.clone();
    let raiser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        flag.store(true, Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let (terminal, _) = f.run(&cfg, &gated, false).await;
    raiser.await.unwrap();
    assert_eq!(
        terminal,
        Terminal::Failed {
            reason: duet_agent::run::INTERRUPTED.into()
        }
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the command was killed"
    );
    let entries = f.entries();
    assert!(matches!(
        &subagent_ends(&entries)[..],
        [(_, Terminal::Failed { reason })] if reason == duet_agent::run::INTERRUPTED
    ));
    assert!(entries.iter().any(|e| matches!(e,
        Entry::Interrupted { tool, .. } if tool == "delegate")));
    assert!(duet_agent::resumable(&f.run_dir).is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_mid_sub_agent_rolls_its_writes_back_on_resume() {
    let f = fixture(false);
    f.frontier.script(
        "PARENT",
        vec![call(
            "delegate",
            json!({"task": "[child W] Add a helper.", "mode": "write", "paths": ["src/export/**"]}),
        )],
    );
    f.frontier.script(
        "[child W]",
        vec![
            call(
                "write_file",
                json!({"path": "src/export/helper.rs", "content": "pub fn helper() {}\n"}),
            ),
            call(
                "edit_file",
                json!({"path": "src/export/mod.rs", "edits": [
                    {"old": "rows[..rows.len() - 1]", "new": "rows"}]}),
            ),
            Step::Hang,
        ],
    );
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    // The process dies while the sub-agent waits for the frontier: nothing
    // after that point is recorded.
    let crashed = tokio::time::timeout(Duration::from_secs(3), f.run(&cfg, &gated, false)).await;
    assert!(crashed.is_err(), "the run should still be waiting");
    drop(gated);
    assert!(f.ws.join("src/export/helper.rs").exists());
    assert!(subagent_ends(&f.entries()).is_empty());

    // Resume: the sub-agent is ended, its writes rolled back, and the parent
    // re-decides that step.
    let second = Frontier::default();
    second.script(
        "PARENT",
        vec![call("finish", json!({"summary": "done without it"}))],
    );
    let gated = f.gated(&second, "m");
    let (terminal, stats) = f.run(&cfg, &gated, true).await;
    assert_eq!(
        terminal,
        Terminal::Completed {
            summary: "done without it".into()
        }
    );
    assert!(!f.ws.join("src/export/helper.rs").exists());
    assert!(
        std::fs::read_to_string(f.ws.join("src/export/mod.rs"))
            .unwrap()
            .contains("rows[..rows.len() - 1]")
    );
    // The step is re-decided: the request holds the task alone.
    let resumed = &second.bodies()[0];
    assert_eq!(
        resumed["messages"].as_array().unwrap().len(),
        2,
        "{resumed}"
    );
    let entries = f.entries();
    assert!(matches!(
        &subagent_ends(&entries)[..],
        [(id, Terminal::Failed { reason })]
            if id == "a1" && reason.contains("before its result was recorded")
    ));
    let reverted: Vec<&Vec<PathBuf>> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::SubagentReverted { paths, .. } => Some(paths),
            _ => None,
        })
        .collect();
    assert_eq!(reverted.len(), 1);
    let mut paths = reverted[0].clone();
    paths.sort();
    assert_eq!(
        paths,
        [
            PathBuf::from("src/export/helper.rs"),
            PathBuf::from("src/export/mod.rs")
        ]
    );
    // What it spent before the crash still counts.
    assert_eq!(stats.subagents.children, 1);
    assert_eq!(stats.subagents.requests, 2);
    assert!((stats.cost_usd - 0.004).abs() < 1e-9, "{}", stats.cost_usd);
    assert!(f.events().iter().any(|e| matches!(e,
        AuditEvent::SubagentEnd { outcome, requests: 2, .. } if outcome == "failed")));
    assert!(matches!(
        verify(&f.log).unwrap(),
        Verification::Intact { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_reverts_what_a_write_sub_agent_wrote_in_the_turn() {
    let f = fixture(false);
    f.frontier.script(
        "PARENT",
        vec![
            call(
                "delegate",
                json!({"task": "[child W] Add a helper.", "mode": "write", "paths": ["src/export/*.rs"]}),
            ),
            call("reply", json!({"message": "The helper is in place."})),
        ],
    );
    f.frontier.script(
        "[child W]",
        vec![
            call(
                "write_file",
                json!({"path": "src/export/helper.rs", "content": "pub fn helper() {}\n"}),
            ),
            call("finish", json!({"summary": "added src/export/helper.rs"})),
        ],
    );
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    let limits = SessionLimits {
        frontier_usd: 100.0,
        working_time: Duration::from_secs(3600),
    };
    let mut s = Session::open(
        &cfg,
        &gated,
        f.presenter(),
        &f.git,
        f.interrupted.clone(),
        limits,
        false,
    )
    .unwrap();
    let end = s.turn("PARENT: add a helper to the export module.").await;
    assert_eq!(
        end,
        TurnEnd::Replied {
            message: "The helper is in place.".into()
        }
    );
    assert!(f.ws.join("src/export/helper.rs").exists());
    assert_eq!(s.stats().subagents.children, 1);
    let (turn, paths) = s.undo().unwrap();
    assert_eq!(
        (turn, paths),
        (1, vec![PathBuf::from("src/export/helper.rs")])
    );
    assert!(!f.ws.join("src/export/helper.rs").exists());
    let (terminal, _) = s.end(true);
    assert!(matches!(terminal, Terminal::Completed { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_turn_rolls_back_its_write_sub_agent_before_the_next_one() {
    let f = fixture(false);
    f.frontier.script(
        "PARENT",
        vec![
            call(
                "delegate",
                json!({"task": "[child W] Add a helper.", "mode": "write", "paths": ["src/export/**"]}),
            ),
            // Turn 2 starts from the files as they were.
            call("reply", json!({"message": "Nothing was added."})),
        ],
    );
    f.frontier.script(
        "[child W]",
        vec![
            call(
                "write_file",
                json!({"path": "src/export/helper.rs", "content": "pub fn helper() {}\n"}),
            ),
            Step::Hang,
        ],
    );
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    let limits = SessionLimits {
        frontier_usd: 100.0,
        working_time: Duration::from_secs(3600),
    };
    let mut s = Session::open(
        &cfg,
        &gated,
        f.presenter(),
        &f.git,
        f.interrupted.clone(),
        limits,
        false,
    )
    .unwrap();
    let flag = f.interrupted.clone();
    let raiser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        flag.store(true, Ordering::SeqCst);
    });
    assert_eq!(s.turn("PARENT: add a helper.").await, TurnEnd::Interrupted);
    raiser.await.unwrap();
    assert!(f.ws.join("src/export/helper.rs").exists());
    assert_eq!(
        s.turn("PARENT: what now?").await,
        TurnEnd::Replied {
            message: "Nothing was added.".into()
        }
    );
    assert!(!f.ws.join("src/export/helper.rs").exists());
    assert!(f.entries().iter().any(|e| matches!(e,
        Entry::SubagentReverted { paths, .. } if paths == &[PathBuf::from("src/export/helper.rs")])));
    let _ = s.end(true);
}

#[tokio::test(flavor = "multi_thread")]
async fn sub_agents_can_be_driven_by_another_model_on_the_same_audit_chain() {
    let f = fixture(true);
    f.frontier.script(
        "PARENT",
        vec![
            call(
                "delegate",
                json!({"task": format!("[child A] Summarize the export for {EMAIL}."), "mode": "read"}),
            ),
            call("finish", json!({"summary": "done"})),
        ],
    );
    let cheap = Frontier::default();
    cheap.script(
        "[child A]",
        vec![call("finish", json!({"summary": "It joins rows."}))],
    );
    let gated = f.gated(&f.frontier, "big");
    // The sub-agents' own gate: the engine's filters, the run's audit log.
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "cheap", Role::Frontier);
    pc.backoff_scale = 0.0;
    let engine = f.engine.as_ref().unwrap();
    let (filter, check) = engine.outbound();
    let child_gate = OutboundGate::with_audit(gated.audit().clone())
        .with_filter(filter)
        .with_check(check)
        .wrap(ChatProvider::new(pc, Box::new(cheap.clone())).unwrap());
    let mut sub = subagents(3, 1.0);
    sub.model = Some(Arc::new(child_gate) as Arc<dyn Driver>);
    let cfg = f.config(Some(sub), 10.0);
    let (terminal, _) = f.run(&cfg, &gated, false).await;
    assert!(
        matches!(terminal, Terminal::Completed { .. }),
        "{terminal:?}"
    );
    assert_eq!(f.frontier.of("[child A]").len(), 0);
    assert_eq!(cheap.of("[child A]").len(), 1);
    // Filtered like every request: the value in the task became a placeholder.
    assert!(!cheap.raw()[0].contains(EMAIL), "{}", cheap.raw()[0]);
    let models: Vec<String> = read(&f.log)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Request(r) => Some(r.model),
            Line::Event(_) => None,
        })
        .collect();
    assert_eq!(models, ["big", "cheap", "big"]);
    assert!(f.events().iter().any(|e| matches!(e,
        AuditEvent::SubagentStart { model, .. } if model == "cheap")));
    assert!(matches!(
        verify(&f.log).unwrap(),
        Verification::Intact { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sub_agent_stops_with_its_parents_turn() {
    let f = fixture(false);
    f.frontier.script(
        "PARENT",
        vec![call(
            "delegate",
            json!({"task": "[child A] Read a lot.", "mode": "read"}),
        )],
    );
    let read = || {
        Step::Slow(
            300,
            Box::new(call("read_file", json!({"path": "src/lib.rs"}))),
        )
    };
    f.frontier
        .script("[child A]", vec![read(), read(), read(), read(), read()]);
    let gated = f.gated(&f.frontier, "m");
    let cfg = f.config(Some(subagents(3, 1.0)), 10.0);
    let limits = SessionLimits {
        frontier_usd: 100.0,
        working_time: Duration::from_secs(3600),
    };
    let mut s = Session::open(
        &cfg,
        &gated,
        f.presenter(),
        &f.git,
        f.interrupted.clone(),
        limits,
        false,
    )
    .unwrap();
    let steering = s.steering();
    let stopper = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        steering.stop();
    });
    let end = s.turn("PARENT: read everything.").await;
    stopper.await.unwrap();
    assert_eq!(end, TurnEnd::Stopped);
    assert!(f.frontier.of("[child A]").len() < 5);
    assert!(matches!(
        &subagent_ends(&f.entries())[..],
        [(_, Terminal::Failed { reason })] if reason.contains("operator stopped")
    ));
}

/// A child's nested entries replay as its own conversation (for viewers).
#[test]
fn nested_entries_round_trip_through_the_transcript() {
    let d = tempfile::tempdir().unwrap();
    let t = Transcript::open(d.path()).unwrap();
    t.append(&Entry::Subagent {
        child: "a1".into(),
        entry: Box::new(Entry::Item {
            item: duet_boundary::model::Item::User {
                text: "task".into(),
            },
        }),
    })
    .unwrap();
    let back = Transcript::read(d.path()).unwrap();
    assert!(matches!(&back[..], [Entry::Subagent { child, entry }]
        if child == "a1" && matches!(entry.as_ref(), Entry::Item { .. })));
}
