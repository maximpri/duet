// SPDX-License-Identifier: GPL-3.0-or-later
//! Harness for the end-to-end privacy scenarios (`privacy_scenarios.rs`).
//!
//! Each scenario runs the real frontier loop (`duet_agent::run`, or a
//! session) in hybrid mode, composed the way `duet run` composes it: the
//! shipped default policy, the engine primed on the files git lists, the
//! engine's outbound filter and check in the gate, an anchored audit log under
//! `.duet/`. The workspace is a temporary git repository holding synthetic
//! values: a gitignored `.env` with a payment key and a database password, and
//! `data/customers.csv` with made-up names, emails and card numbers.
//!
//! The frontier is scripted: it answers each request with the next tool call
//! of its script and records the exact bytes of every request. The local model
//! is a stand-in that reads what the engine sends it (`<content>` and, for
//! questions, `<question>`) and answers either as it should ([`Local::Cooperative`]:
//! structure only) or carelessly ([`Local::Careless`]: it answers narrow
//! questions literally and repeats what it read in the given [`Echo`] forms).
//!
//! [`Fixture::assert_no_leak`] runs [`Canaries::find`] over every request
//! body, per message and over the raw bytes, and fails naming the request,
//! the channel (system prompt, user message, tool result of which tool, tool
//! call arguments), the canary and the form it appeared in.
//!
//! Nothing here contacts a model server or the network.

#![allow(dead_code)]

use bytes::Bytes;
use duet_agent::session::SessionLimits;
use duet_agent::{RunConfig, Session, Terminal};
use duet_boundary::OutboundGate;
use duet_boundary::audit::{AuditEvent, AuditLog, run_anchors};
use duet_boundary::engine::Engine;
use duet_boundary::policy::Policy;
use duet_boundary::testing::Received;
use duet_boundary::testing::canary::{Canaries, Form};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A payment key in `.env` (a format the secret detectors know).
pub const KEY: &str = "sk_live_Qm7Tz2Lp9Xv4Rb8Nw3Kd";
/// A database password in `.env` (no detector recognizes it on its own).
pub const DB_PASSWORD: &str = "quartz-otter-5519";

/// A made-up customer in `data/customers.csv`.
pub struct Customer {
    pub name: &'static str,
    pub surname: &'static str,
    pub email: &'static str,
    /// Luhn-valid, 16 digits.
    pub card: &'static str,
    /// Date of birth: personal, but no detector recognizes it, so no value
    /// filter claims it and [`planted`] leaves it out.
    pub born: &'static str,
}

pub const CUSTOMERS: [Customer; 3] = [
    Customer {
        name: "Vakdril Thorsko",
        surname: "Thorsko",
        email: "vakdril.thorsko@kestrelpost-mail.net",
        card: "4539148803436467",
        born: "1987-03-14",
    },
    Customer {
        name: "Orla Brennvik",
        surname: "Brennvik",
        email: "orla.brennvik@fjordmail-post.org",
        card: "5293761582049377",
        born: "1994-11-07",
    },
    Customer {
        name: "Ysolde Marrquin",
        surname: "Marrquin",
        email: "ysolde.marrquin@quillhaven-mail.io",
        card: "3762948510736285",
        born: "1979-06-21",
    },
];

/// The line of `data/customers.csv` (1-based, as the local model sees it)
/// holding `CUSTOMERS[i]`.
pub fn customer_line(i: usize) -> usize {
    i + 2
}

/// Every planted value a value filter claims to replace (SECURITY.md: secrets,
/// emails, card numbers, names and surnames in data files).
pub fn planted() -> Vec<String> {
    let mut v = vec![KEY.to_owned(), DB_PASSWORD.to_owned()];
    for c in &CUSTOMERS {
        v.extend([c.name, c.surname, c.email, c.card].map(str::to_owned));
    }
    v
}

/// How the local stand-in answers.
#[derive(Clone, Copy, Debug)]
pub enum Local {
    /// Describes structure only: line and field counts, how long the value
    /// asked about is and whether it passes the Luhn check. Its prose names
    /// an algorithm and a card network in title case ("the Luhn Algorithm",
    /// "Visa Network"), words that are not in what it read.
    Cooperative,
    /// Answers a narrow question about a value (its first digit, last four
    /// digits, number of digits, digit n) literally from the content, and
    /// repeats what it read in each given form, in summaries and answers.
    Careless(&'static [Echo]),
}

/// How a careless answer repeats the content it read. The line is the one a
/// question names (`line N`), else the line with the most digits; its values
/// are its comma-separated fields and their words, each of four or more
/// characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Echo {
    /// The whole line.
    Line,
    /// Runs of digits from it: the first four and last four digits of its
    /// longest digit run.
    Digits,
    /// Each value with a space between every two characters.
    Spaced,
    /// Each value in base64.
    Base64,
}

impl Echo {
    pub const ALL: &'static [Echo] = &[Echo::Line, Echo::Digits, Echo::Spaced, Echo::Base64];
    /// The forms the boundary is expected to stop in local output.
    pub const HANDLED: &'static [Echo] = &[Echo::Line, Echo::Digits];
}

/// Builds a tool call (name, arguments) from the request it answers.
pub type MakeCall = Box<dyn Fn(&Value) -> (&'static str, Value) + Send>;

/// One scripted frontier answer.
pub enum Step {
    /// A tool call.
    Call(&'static str, Value),
    /// A tool call made from the request it answers (to name a placeholder or
    /// a handle the frontier was shown).
    From(MakeCall),
}

impl Step {
    /// `ask_local` about the latest handle the frontier was shown.
    pub fn ask_latest(question: impl Into<String>) -> Step {
        let question = question.into();
        Step::From(Box::new(move |body| {
            let handle = latest_handle(body).expect("a handle in a tool result");
            ("ask_local", json!({"handle": handle, "question": question}))
        }))
    }
}

/// The scripted frontier: one tool call per request, every request body
/// recorded as sent.
#[derive(Clone, Default)]
pub struct Frontier {
    steps: Arc<Mutex<VecDeque<Step>>>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    unscripted: Arc<AtomicUsize>,
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let parsed: Value = serde_json::from_slice(&body).unwrap_or_default();
        let n = {
            let mut bodies = self.bodies.lock().unwrap();
            bodies.push(body);
            bodies.len()
        };
        let step = self.steps.lock().unwrap().pop_front();
        let (name, args) = match step {
            Some(Step::Call(name, args)) => (name, args),
            Some(Step::From(make)) => make(&parsed),
            None => {
                self.unscripted.fetch_add(1, Ordering::SeqCst);
                ("finish", json!({"summary": "(the script ran out)"}))
            }
        };
        let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0,
            "id": format!("c{n}"), "type": "function",
            "function": {"name": name, "arguments": args.to_string()}}]}}]});
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

/// Workspace options.
#[derive(Clone, Copy)]
pub struct Options {
    /// `.env` is listed in `.gitignore`, as in most repositories.
    pub env_ignored: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { env_ignored: true }
    }
}

/// A hybrid run (or session) in a fresh workspace.
pub struct Fixture {
    _dir: tempfile::TempDir,
    pub ws: PathBuf,
    pub run_id: String,
    pub run_dir: PathBuf,
    pub frontier: Frontier,
    pub local: Received,
    pub engine: Arc<Engine>,
    pub gated: duet_boundary::GatedFrontier,
    pub cfg: RunConfig,
    pub git: duet_git::Git,
    pub interrupted: Arc<AtomicBool>,
}

fn git(ws: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t.test"])
        .args(args)
        .current_dir(ws)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// The shipped default policy, read from the configuration registry as
/// `duet run` reads it (no owner or project file).
fn shipped_policy(owner: &Path) -> Policy {
    let cfg = duet_config::Config::load(owner, None).unwrap();
    let list = |k: &str| cfg.list(k).unwrap();
    let flag = |k: &str| cfg.bool(k).unwrap();
    Policy {
        sensitive_globs: list("sensitivity.globs"),
        protected_paths: list("sensitivity.protected_paths"),
        command_output_sensitive: flag("sensitivity.command_output_sensitive"),
        raw_ok_commands: list("sensitivity.raw_ok_commands"),
        secret_sinks: list("sensitivity.secret_sinks"),
        detect_secrets: flag("sensitivity.detect_secrets"),
        detect_pii: flag("sensitivity.detect_pii"),
        detect_entropy: flag("sensitivity.detect_entropy"),
        custom_patterns: list("sensitivity.custom_patterns"),
        bulky_tokens: cfg.int("sensitivity.bulky_tokens").unwrap() as usize,
        bulky_file_tokens: cfg.int("sensitivity.bulky_file_tokens").unwrap() as usize,
        local_brief: flag("sensitivity.local_brief"),
        interface_only: list("ip.interface_only"),
        sealed: list("ip.sealed"),
    }
}

impl Fixture {
    /// A workspace with the planted values, the engine primed as `duet run`
    /// primes it, and a run named `name` whose task (or first session
    /// message) is `objective`.
    pub fn new(name: &str, objective: &str, local: Local) -> Self {
        Self::with(name, objective, local, Options::default())
    }

    pub fn with(name: &str, objective: &str, local: Local, options: Options) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(
            ws.join("README.md"),
            "# ledger\n\nBilling helpers for the shop. Run the tests with cargo test.\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("src/lib.rs"),
            "/// Sum of the amounts, skipping refunds (negative amounts).\n\
             pub fn total(amounts: &[i64]) -> i64 {\n    amounts.iter().filter(|a| **a > 0).sum()\n}\n",
        )
        .unwrap();
        std::fs::write(
            ws.join(".env"),
            format!("STRIPE_SECRET_KEY={KEY}\nDB_PASSWORD={DB_PASSWORD}\n"),
        )
        .unwrap();
        let mut csv = String::from("id,name,born,email,card\n");
        for (i, c) in CUSTOMERS.iter().enumerate() {
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                i + 1,
                c.name,
                c.born,
                c.email,
                c.card
            ));
        }
        std::fs::write(ws.join("data/customers.csv"), csv).unwrap();
        if options.env_ignored {
            std::fs::write(ws.join(".gitignore"), ".env\n").unwrap();
        }
        git(&ws, &["init", "-q"]);
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-qm", "start"]);

        // As `duet run` prepares a run.
        let git = duet_git::Git::locate().unwrap();
        let _ = git.exclude_state_dir(&ws);
        let run_id = format!("20260925-000000-{name}");
        let run_dir = ws.join(".duet/runs").join(&run_id);
        duet_fs::private::ensure_private_dir(&run_dir).unwrap();
        duet_fs::private::ensure_private_dir(&ws.join(".duet/tmp")).unwrap();
        let (reader, received) =
            duet_boundary::testing::responsive_local(move |prompt| respond(local, prompt));
        let engine = Engine::open(
            &run_dir,
            shipped_policy(&root.join("owner/config.toml")),
            Some(reader),
        )
        .unwrap();
        engine.prime(&ws, &git.list_files(&ws).unwrap(), objective);

        let frontier = Frontier::default();
        let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
        pc.backoff_scale = 0.0;
        let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
        let audit = AuditLog::open_anchored(
            &ws.join(".duet/audit").join(format!("{run_id}.jsonl")),
            &run_anchors(&root.join("state"), &ws, &run_id),
        )
        .unwrap();
        let (filter, check) = engine.outbound();
        let gated = OutboundGate::new(audit)
            .with_filter(filter)
            .with_check(check)
            .wrap(provider);
        gated.audit().record(AuditEvent::RunStart {
            mode: "hybrid".into(),
            boundary: true,
        });
        let cfg = RunConfig {
            workspace: ws.clone(),
            run_dir: run_dir.clone(),
            objective: objective.into(),
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
            oversight: Default::default(),
            web: None,
            git_author: None,
            mcp: None,
            lsp: None,
        };
        Self {
            _dir: dir,
            ws,
            run_id,
            run_dir,
            frontier,
            local: received,
            engine,
            gated,
            cfg,
            git,
            interrupted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Appends to the frontier's script.
    pub fn script(&self, steps: Vec<Step>) {
        self.frontier.steps.lock().unwrap().extend(steps);
    }

    /// Runs the task to its terminal state (in the operator's form).
    pub async fn run(&self) -> Terminal {
        let (terminal, _) = duet_agent::run(
            &self.cfg,
            &self.gated,
            self.engine.as_ref(),
            &self.git,
            false,
            &self.interrupted,
        )
        .await;
        terminal
    }

    /// Opens a session whose first message is the fixture's objective.
    pub fn session(&self) -> Session<'_> {
        Session::open(
            &self.cfg,
            &self.gated,
            self.engine.as_ref(),
            &self.git,
            self.interrupted.clone(),
            SessionLimits {
                frontier_usd: 100.0,
                working_time: Duration::from_secs(3600),
            },
            false,
        )
        .unwrap()
    }

    /// Requests that found no script step left (answered with `finish`).
    pub fn unscripted(&self) -> usize {
        self.frontier.unscripted.load(Ordering::SeqCst)
    }

    /// The exact bytes of every request sent to the frontier, in order.
    pub fn bodies(&self) -> Vec<Vec<u8>> {
        self.frontier.bodies.lock().unwrap().clone()
    }

    /// The last request sent to the frontier.
    pub fn last_request(&self) -> Value {
        let bodies = self.bodies();
        serde_json::from_slice(bodies.last().expect("a request")).unwrap()
    }

    /// Every result of the tool `name` in the last request, in order.
    pub fn results_of(&self, name: &str) -> Vec<String> {
        let body = self.last_request();
        let names = call_names(&body);
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "tool")
            .filter(|m| {
                m["tool_call_id"]
                    .as_str()
                    .and_then(|id| names.get(id))
                    .is_some_and(|n| n == name)
            })
            .filter_map(|m| m["content"].as_str().map(str::to_owned))
            .collect()
    }

    /// The prompts the local model received, in order.
    pub fn local_prompts(&self) -> Vec<String> {
        (0..self.local.bodies().len())
            .map(|i| self.local.prompt(i))
            .collect()
    }

    /// Canaries for every planted value and `extra`.
    pub fn canaries<const N: usize>(&self, extra: [&str; N]) -> Canaries {
        Canaries::new(planted().into_iter().chain(extra.map(str::to_owned)))
    }

    /// Every appearance of a canary in what reached the frontier.
    pub fn leaks(&self, canaries: &Canaries) -> Vec<Leak> {
        let bodies = self.bodies();
        let total = bodies.len();
        let mut out = Vec::new();
        for (i, raw) in bodies.iter().enumerate() {
            let mut found: Vec<Leak> = Vec::new();
            let body: Value = serde_json::from_slice(raw).unwrap_or_default();
            for (channel, text) in channels(&body) {
                for f in canaries.find(&text) {
                    found.push(Leak {
                        request: i + 1,
                        requests: total,
                        channel: channel.clone(),
                        canary: canaries.values()[f.canary_index].clone(),
                        form: f.form,
                        context: around(text.as_bytes(), f.offset, f.len),
                    });
                }
            }
            // Spellings only the raw bytes hold (a JSON-level escape).
            for f in canaries.find(raw) {
                let canary = &canaries.values()[f.canary_index];
                if !found.iter().any(|l| &l.canary == canary) {
                    found.push(Leak {
                        request: i + 1,
                        requests: total,
                        channel: "raw request bytes".into(),
                        canary: canary.clone(),
                        form: f.form,
                        context: around(raw, f.offset, f.len),
                    });
                }
            }
            out.extend(found);
        }
        out
    }

    /// Fails with every appearance of a canary in what reached the frontier:
    /// the request, the channel, the canary, the form and where.
    pub fn assert_no_leak(&self, canaries: &Canaries) {
        let leaks = self.leaks(canaries);
        assert!(!self.bodies().is_empty(), "no request reached the frontier");
        if !leaks.is_empty() {
            let report: Vec<String> = leaks.iter().map(Leak::to_string).collect();
            panic!(
                "{} canary appearance(s) reached the frontier:\n{}",
                leaks.len(),
                report.join("\n")
            );
        }
    }
}

/// One appearance of a canary in a request to the frontier.
#[derive(Debug, Clone)]
pub struct Leak {
    /// 1-based request number (the frontier turn).
    pub request: usize,
    pub requests: usize,
    pub channel: String,
    pub canary: String,
    pub form: Form,
    pub context: String,
}

impl std::fmt::Display for Leak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "  request {} of {}, {}: {:?} of {:?} in …{}…",
            self.request, self.requests, self.channel, self.form, self.canary, self.context
        )
    }
}

fn around(hay: &[u8], offset: usize, len: usize) -> String {
    let start = offset.saturating_sub(40);
    let end = (offset + len + 40).min(hay.len());
    String::from_utf8_lossy(&hay[start..end]).replace('\n', "⏎")
}

/// Tool call id to tool name, over every assistant message in `body`.
fn call_names(body: &Value) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for m in body["messages"].as_array().into_iter().flatten() {
        for c in m["tool_calls"].as_array().into_iter().flatten() {
            if let (Some(id), Some(name)) = (c["id"].as_str(), c["function"]["name"].as_str()) {
                names.insert(id.to_owned(), name.to_owned());
            }
        }
    }
    names
}

/// Every string in a request body, labelled by where it goes: the system
/// prompt, a user message, assistant text, a tool call's arguments, a tool's
/// result, the tool definitions, other request fields.
fn channels(body: &Value) -> Vec<(String, String)> {
    let names = call_names(body);
    let mut out = Vec::new();
    let Some(obj) = body.as_object() else {
        return out;
    };
    for (key, value) in obj {
        if key != "messages" {
            let label = if key == "tools" {
                "tool definitions".to_owned()
            } else {
                format!("request field `{key}`")
            };
            strings(value, &label, &mut out);
            continue;
        }
        for (i, m) in value.as_array().into_iter().flatten().enumerate() {
            let role = m["role"].as_str().unwrap_or("?");
            let label = match role {
                "system" => format!("messages[{i}] system prompt"),
                "user" => format!("messages[{i}] user message"),
                "assistant" => format!("messages[{i}] assistant text"),
                "tool" => {
                    let name = m["tool_call_id"]
                        .as_str()
                        .and_then(|id| names.get(id))
                        .map_or("an unknown tool", String::as_str);
                    format!("messages[{i}] result of {name}")
                }
                other => format!("messages[{i}] {other} message"),
            };
            for (field, v) in m.as_object().into_iter().flatten() {
                if field == "tool_calls" {
                    for c in v.as_array().into_iter().flatten() {
                        let name = c["function"]["name"].as_str().unwrap_or("?");
                        strings(c, &format!("messages[{i}] {name} call arguments"), &mut out);
                    }
                } else if field != "role" && field != "tool_call_id" {
                    strings(v, &label, &mut out);
                }
            }
        }
    }
    out
}

fn strings(v: &Value, label: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) => out.push((label.to_owned(), s.clone())),
        Value::Array(a) => a.iter().for_each(|x| strings(x, label, out)),
        Value::Object(o) => o.values().for_each(|x| strings(x, label, out)),
        _ => {}
    }
}

/// The first placeholder (`⟨…⟩`) in the first user message of `body`.
pub fn first_placeholder(body: &Value) -> Option<String> {
    let text = body["messages"]
        .as_array()?
        .iter()
        .find(|m| m["role"] == "user")?["content"]
        .as_str()?;
    let start = text.find('⟨')?;
    let end = start + text[start..].find('⟩')? + '⟩'.len_utf8();
    Some(text[start..end].to_owned())
}

/// The handle (`h7`) a tool result offers for `ask_local`, if any.
fn offered_handle(result: &Value) -> Option<String> {
    let text = result["content"].as_str()?;
    let at = text.find("ask_local(handle=\"")? + "ask_local(handle=\"".len();
    let end = at + text[at..].find('"')?;
    Some(text[at..end].to_owned())
}

fn tool_results(body: &Value) -> impl DoubleEndedIterator<Item = &Value> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == "tool")
}

/// The handle the last tool result in `body` offers for `ask_local`.
pub fn latest_handle(body: &Value) -> Option<String> {
    offered_handle(tool_results(body).next_back()?)
}

/// The first handle any tool result in `body` offered for `ask_local`.
pub fn first_handle(body: &Value) -> Option<String> {
    tool_results(body).find_map(offered_handle)
}

// --- The local stand-ins ---------------------------------------------------

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find(close)?;
    Some(&text[start..end])
}

/// The content lines of a local prompt, without the engine's line numbers.
fn content_lines(prompt: &str) -> Vec<String> {
    between(prompt, "<content>\n", "</content>")
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            match t[digits..].strip_prefix("  ") {
                Some(rest) if digits > 0 => rest.to_owned(),
                _ => l.to_owned(),
            }
        })
        .collect()
}

fn digit_runs(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|r| !r.is_empty())
        .collect()
}

fn luhn(digits: &str) -> bool {
    let sum: u32 = digits
        .chars()
        .rev()
        .filter_map(|c| c.to_digit(10))
        .enumerate()
        .map(|(i, d)| match (i % 2, d * 2) {
            (1, x) if x > 9 => x - 9,
            (1, x) => x,
            _ => d,
        })
        .sum();
    sum.is_multiple_of(10)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for g in bytes.chunks(3) {
        let n = (u32::from(g[0]) << 16)
            | (u32::from(*g.get(1).unwrap_or(&0)) << 8)
            | u32::from(*g.get(2).unwrap_or(&0));
        for k in 0..4 {
            if k <= g.len() {
                s.push(TABLE[(n >> (18 - 6 * k)) as usize & 63] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// What a stand-in answers to one local prompt (JSON in the shared schema).
fn respond(local: Local, prompt: &str) -> String {
    let lines = content_lines(prompt);
    let question = between(prompt, "<question>", "</question>");
    // The line a question names, else the one with the most digits.
    let named = question.and_then(|q| {
        let at = q.to_lowercase().find("line ")? + "line ".len();
        let n: String = q[at..].chars().take_while(char::is_ascii_digit).collect();
        n.parse::<usize>().ok()
    });
    let (index, line) = named
        .and_then(|n| Some((n, lines.get(n.checked_sub(1)?)?.as_str())))
        .or_else(|| {
            lines
                .iter()
                .enumerate()
                .max_by_key(|(i, l)| {
                    (
                        l.chars().filter(char::is_ascii_digit).count(),
                        std::cmp::Reverse(*i),
                    )
                })
                .map(|(i, l)| (i + 1, l.as_str()))
        })
        .unwrap_or((1, ""));
    let digits = digit_runs(line)
        .into_iter()
        .max_by_key(|r| r.len())
        .unwrap_or_default();
    let fields = lines.first().map_or(0, |l| l.split(',').count());
    let structure = format!(
        "Structure only: {} lines; line 1 has {fields} comma-separated fields; values are withheld.",
        lines.len()
    );
    let text = match local {
        Local::Cooperative => match question {
            None => format!(
                "{structure} Numbers are checked with the Luhn Algorithm; most start in the Visa Network range."
            ),
            Some(_) => format!(
                "The value on line {index} is a {}-digit number; the Luhn Algorithm {} it. Values are withheld.",
                digits.len(),
                if luhn(digits) { "accepts" } else { "rejects" }
            ),
        },
        Local::Careless(echo) => {
            let mut parts = Vec::new();
            if let Some(q) = question.map(str::to_lowercase) {
                let nth = q
                    .find("digit ")
                    .map(|at| {
                        q[at + "digit ".len()..]
                            .chars()
                            .take_while(char::is_ascii_digit)
                            .collect::<String>()
                    })
                    .and_then(|n| n.parse::<usize>().ok());
                let d: Vec<char> = digits.chars().collect();
                if d.is_empty() {
                    parts.push("There is no number on that line.".to_owned());
                } else if q.contains("first digit") {
                    parts.push(format!("The first digit is {}.", d[0]));
                } else if q.contains("last four") {
                    parts.push(format!(
                        "The last four digits are {}.",
                        &digits[digits.len() - 4..]
                    ));
                } else if q.contains("how many digits") || q.contains("length") {
                    parts.push(format!("It has {} digits.", d.len()));
                } else if let Some(n) = nth.filter(|n| (1..=d.len()).contains(n)) {
                    parts.push(format!("Digit {n} is {}.", d[n - 1]));
                }
            }
            let mut values: Vec<&str> = Vec::new();
            for field in line.split(',').map(str::trim) {
                let words: Vec<&str> = field.split_whitespace().collect();
                if words.len() <= 3 {
                    values.push(field);
                }
                if words.len() > 1 {
                    values.extend(words);
                }
            }
            values.retain(|v| v.chars().count() >= 4);
            values.dedup();
            let each = |form: fn(&str) -> String| {
                values
                    .iter()
                    .map(|v| form(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            for e in echo {
                parts.push(match e {
                    Echo::Line => format!("Line {index} reads: {line}"),
                    Echo::Digits if digits.len() >= 8 => format!(
                        "Its number starts with {} and ends in {}.",
                        &digits[..4],
                        &digits[digits.len() - 4..]
                    ),
                    Echo::Digits => continue,
                    Echo::Spaced => format!(
                        "Spelled out: {}.",
                        each(|v| v.chars().map(String::from).collect::<Vec<_>>().join(" "))
                    ),
                    Echo::Base64 => format!("In base64: {}.", each(|v| base64(v.as_bytes()))),
                });
            }
            if parts.is_empty() {
                structure
            } else {
                parts.join(" ")
            }
        }
    };
    match question {
        None => json!({"summary": text, "facts": []}),
        Some(_) => json!({"answer": text, "evidence_lines": [index], "unanswerable": false}),
    }
    .to_string()
}
