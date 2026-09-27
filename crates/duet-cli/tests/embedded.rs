// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet_cli::main_with` the way a program that embeds Duet uses it
//! (ARCHITECTURE.md §13): this test binary (no test harness) is such a
//! program. Started with `EMBEDDED_CHILD` set it is the embedded command line
//! — product "Acme Duet 9.9.9", the unsigned policy file named by
//! `ACME_POLICY`, an audit subscriber and an end hook that append what they
//! are told to files, and one doctor check — and otherwise it runs the checks
//! below against itself and against the `duet` binary, against a scripted
//! frontier on loopback: `--version` names the product; the policy bounds
//! `config` (and a missing one stops everything) while `duet` itself ignores
//! it; a run and a session invocation each reach the end hook exactly once,
//! with a chain head the subscriber was told of; and `doctor` adds the check
//! and names the product and the Duet it is built on.

use duet_agent::embed::{
    Appended, AuditSubscriber, EndHook, EndReport, Hooks, PolicyFile, Recorded,
};
use duet_cli::{Check, DoctorCheck, DoctorContext, Embedding, Product, Status};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};
use std::sync::{Arc, Mutex};

const CHILD: &str = "EMBEDDED_CHILD";

// --- The embedding program -------------------------------------------------

fn append(path: &Path, line: &str) -> Result<(), String> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// Appends `seq hash kind` for every record it is told of.
struct Records(PathBuf);

impl AuditSubscriber for Records {
    fn name(&self) -> &str {
        "acme-records"
    }

    fn appended(&self, r: &Appended) -> Result<(), String> {
        let kind = match &r.record {
            Recorded::Event { event } => event.kind().to_owned(),
            Recorded::Request { .. } => "request".to_owned(),
            _ => "other".to_owned(),
        };
        append(&self.0, &format!("{} {} {kind}", r.seq, r.hash))
    }
}

/// Appends each end report as one JSON line.
struct Ends(PathBuf);

impl EndHook for Ends {
    fn name(&self) -> &str {
        "acme-ends"
    }

    fn ended(&self, report: &EndReport) -> Result<(), String> {
        append(
            &self.0,
            &serde_json::to_string(report).map_err(|e| e.to_string())?,
        )
    }
}

struct AcmeCheck;

impl DoctorCheck for AcmeCheck {
    fn run(&self, ctx: &DoctorContext<'_>) -> Vec<Check> {
        vec![
            Check::new(
                "acme",
                Status::Warn,
                format!("configuration loaded: {}", ctx.config.is_some()),
            )
            .fix("nothing to fix"),
        ]
    }
}

fn embedded() -> ExitCode {
    let var = |k: &str| PathBuf::from(std::env::var_os(k).expect(k));
    let hooks = Hooks::default()
        .subscribe(Arc::new(Records(var("ACME_RECORDS"))))
        .on_end(Arc::new(Ends(var("ACME_ENDS"))));
    duet_cli::main_with(
        Embedding::new(Product::new("Acme Duet", "9.9.9"))
            .with_policy(Arc::new(PolicyFile::new(var("ACME_POLICY"))))
            .with_hooks(hooks)
            .with_doctor_check(Arc::new(AcmeCheck)),
    )
}

fn main() -> ExitCode {
    if std::env::var_os(CHILD).is_some() {
        return embedded();
    }
    versions();
    policy_bounds_configuration();
    runs_and_sessions_reach_the_hooks_once();
    doctor_adds_the_check_and_names_the_product();
    println!("embedded: all checks passed");
    ExitCode::SUCCESS
}

// --- A scripted frontier ---------------------------------------------------

/// A loopback server answering each request with the next scripted tool call.
struct Frontier {
    port: u16,
}

impl Frontier {
    fn start(calls: Vec<(&'static str, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(Mutex::new(VecDeque::from(calls)));
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().flatten().enumerate() {
                let call = calls
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unscripted request");
                serve(stream, n, call);
            }
        });
        Self { port }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

fn serve(stream: TcpStream, n: usize, (name, args): (&str, Value)) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": format!("c{n}"),
        "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]});
    let sse = format!(
        "data: {call}\n\n\
data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n\
data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":1000,\"completion_tokens\":20}}}}\n\n\
data: [DONE]\n\n"
    );
    let mut out = stream;
    let _ = write!(
        out,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    );
}

// --- Checks ------------------------------------------------------------------

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    ws: PathBuf,
}

const POLICY: &str = "[policy]\nname = \"acme\"\nversion = \"1\"\n[limits]\nfrontier_usd = 2.0\n";

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, ws) = (root.join("owner"), root.join("ws"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(root.join("policy.toml"), POLICY).unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(git.success());
    Env {
        _dir: dir,
        root,
        home,
        ws,
    }
}

/// The embedded command line (this binary as the child) or `duet` itself.
fn command(e: &Env, embedded: bool, args: &[&str]) -> Command {
    let mut c = if embedded {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.env(CHILD, "1")
            .env("ACME_POLICY", e.root.join("policy.toml"))
            .env("ACME_RECORDS", e.root.join("records.txt"))
            .env("ACME_ENDS", e.root.join("ends.jsonl"));
        c
    } else {
        Command::new(env!("CARGO_BIN_EXE_duet"))
    };
    c.args(args)
        .arg("--workspace")
        .arg(&e.ws)
        .env("DUET_CONFIG_HOME", &e.home)
        .env("ZAI_API_KEY", "loopback-test-key")
        .env_remove("DUET_LOCAL_PORTS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

fn run(e: &Env, embedded: bool, args: &[&str]) -> Output {
    command(e, embedded, args).output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn versions() {
    let e = env();
    let o = run(&e, false, &["--version"]);
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        format!("duet {}\n", env!("CARGO_PKG_VERSION"))
    );
    let o = run(&e, true, &["--version"]);
    assert_eq!(String::from_utf8_lossy(&o.stdout), "Acme Duet 9.9.9\n");
}

fn policy_bounds_configuration() {
    let e = env();
    // `duet` itself has no policy layer: the owner may set any value.
    let o = run(&e, false, &["config", "set", "limits.frontier_usd", "5.0"]);
    assert!(o.status.success(), "{}", text(&o));
    // The embedded command line applies its policy: the looser owner value
    // gives way, and a change past the bound is refused.
    let o = run(&e, true, &["config", "list"]);
    assert!(o.status.success(), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.starts_with("# policy layer: acme 1 from "), "{out}");
    assert!(
        out.contains("limits.frontier_usd = 2.0  (policy; the policy bounds it at 2.0;"),
        "{out}"
    );
    let o = run(&e, true, &["config", "set", "limits.frontier_usd", "3.0"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("Error: limits.frontier_usd = 3.0 is not allowed by the policy acme 1"),
        "{err}"
    );
    // A policy that cannot be loaded stops every command that reads settings.
    std::fs::remove_file(e.root.join("policy.toml")).unwrap();
    let o = run(&e, true, &["config", "list"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("does not run without its policy"),
        "{}",
        text(&o)
    );
    assert!(run(&e, false, &["config", "list"]).status.success());
}

fn ends(e: &Env) -> Vec<Value> {
    std::fs::read_to_string(e.root.join("ends.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The subscriber was told of every record in order, and the end report's
/// head is the log's last line and the last record the subscriber saw.
fn head_matches(e: &Env, report: &Value) {
    let id = report["run_id"].as_str().unwrap();
    let log =
        std::fs::read_to_string(e.ws.join(".duet/audit").join(format!("{id}.jsonl"))).unwrap();
    let last = log.lines().last().unwrap();
    let head = hex::encode(Sha256::digest(last.as_bytes()));
    assert_eq!(report["chain"]["head"], head.as_str(), "{report}");
    let seen = std::fs::read_to_string(e.root.join("records.txt")).unwrap();
    let seen: Vec<&str> = seen.lines().collect();
    for (i, line) in seen.iter().enumerate() {
        assert!(line.starts_with(&format!("{} ", i + 1)), "{seen:?}");
    }
    assert!(seen[0].ends_with(" run_start"), "{seen:?}");
    assert_eq!(seen.len(), log.lines().count(), "{seen:?}");
    assert!(
        seen.last().unwrap().contains(&format!(" {head} run_end")),
        "{seen:?}"
    );
}

fn runs_and_sessions_reach_the_hooks_once() {
    // A run.
    let e = env();
    let f = Frontier::start(vec![
        ("read_file", json!({"path": "src/lib.rs"})),
        ("finish", json!({"summary": "read it"})),
    ]);
    let url = f.url();
    let o = run(
        &e,
        true,
        &[
            "run",
            "--mode",
            "passthrough",
            "--no-privacy",
            "--quiet",
            "--frontier-url",
            &url,
            "--frontier-model",
            "glm-5.3-flash",
            "Read src/lib.rs.",
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let reports = ends(&e);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let r = &reports[0];
    assert_eq!(
        (r["kind"].as_str(), r["state"].as_str(), r["mode"].as_str()),
        (Some("run"), Some("completed"), Some("passthrough")),
        "{r}"
    );
    assert_eq!(r["policy"]["name"], "acme", "{r}");
    head_matches(&e, r);
    // The summary on standard output is `duet run`'s own.
    let printed: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(printed["terminal"]["state"], "completed");

    // A session invocation, left open at the end of input.
    let e = env();
    let f = Frontier::start(vec![("reply", json!({"message": "Hello."}))]);
    let url = f.url();
    let mut c = command(
        &e,
        true,
        &[
            "--mode",
            "passthrough",
            "--no-privacy",
            "--frontier-url",
            &url,
            "--frontier-model",
            "glm-5.3-flash",
        ],
    );
    c.stdin(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Say hello.\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let reports = ends(&e);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let r = &reports[0];
    assert_eq!(
        (
            r["kind"].as_str(),
            r["state"].as_str(),
            r["resumable"].as_bool()
        ),
        (Some("session"), Some("failed"), Some(true)),
        "{r}"
    );
    head_matches(&e, r);
}

fn doctor_adds_the_check_and_names_the_product() {
    let e = env();
    let report = |embedded: bool| -> Value {
        let o = run(&e, embedded, &["doctor", "--json"]);
        serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", text(&o)))
    };
    let check = |r: &Value, name: &str| -> Option<Value> {
        r["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .cloned()
    };
    let ours = report(true);
    assert_eq!(ours["product"], "Acme Duet", "{ours}");
    assert_eq!(ours["version"], "9.9.9");
    assert_eq!(ours["core_version"], env!("CARGO_PKG_VERSION"));
    let version = check(&ours, "version").unwrap();
    let detail = version["detail"].as_str().unwrap();
    assert!(detail.starts_with("Acme Duet 9.9.9 ("), "{detail}");
    assert!(
        detail.contains(&format!("; Duet Core {}", env!("CARGO_PKG_VERSION"))),
        "{detail}"
    );
    let acme = check(&ours, "acme").unwrap();
    assert_eq!(
        (acme["status"].as_str(), acme["detail"].as_str()),
        (Some("warn"), Some("configuration loaded: true"))
    );
    assert!(check(&ours, "policy").is_some(), "{ours}");

    let plain = report(false);
    assert!(plain.get("product").is_none(), "{plain}");
    assert!(check(&plain, "acme").is_none() && check(&plain, "policy").is_none());
    let detail = check(&plain, "version").unwrap()["detail"].clone();
    assert!(
        detail
            .as_str()
            .unwrap()
            .starts_with(&format!("duet {} (", env!("CARGO_PKG_VERSION"))),
        "{detail}"
    );
}
