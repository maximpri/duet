// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet run` and `duet resume` end to end through the binary, against a
//! scripted frontier on loopback (no model server, no network beyond
//! 127.0.0.1): Ctrl-C ends a run as a resumable `Failed{interrupted}` with its
//! summary; `duet resume` continues it to `Completed` with one intact, anchored
//! audit chain; a completed run or a malformed id is not resumed; and the
//! dollar budget ends a run as `BudgetStopped` (exit 3).

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the scripted frontier does with one request.
enum Step {
    /// Answer with one tool call, reporting this many prompt tokens.
    Call(&'static str, Value, u64),
    /// Accept the request and never answer.
    Hang,
}

/// A loopback server speaking just enough HTTP/1.1 and SSE for one provider.
struct Frontier {
    port: u16,
    steps: Arc<Mutex<VecDeque<Step>>>,
    requests: Arc<Mutex<u32>>,
}

impl Frontier {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let steps: Arc<Mutex<VecDeque<Step>>> = Arc::default();
        let requests: Arc<Mutex<u32>> = Arc::default();
        let (s, r) = (steps.clone(), requests.clone());
        std::thread::spawn(move || {
            // Hung connections are kept open here until the server goes away.
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                if let Some(hung) = serve(stream, &s, &r) {
                    held.push(hung);
                }
            }
        });
        Self {
            port,
            steps,
            requests,
        }
    }

    fn script(&self, steps: Vec<Step>) {
        self.steps.lock().unwrap().extend(steps);
    }

    fn requests(&self) -> u32 {
        *self.requests.lock().unwrap()
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

fn serve(
    stream: TcpStream,
    steps: &Mutex<VecDeque<Step>>,
    requests: &Mutex<u32>,
) -> Option<TcpStream> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    let n = {
        let mut r = requests.lock().unwrap();
        *r += 1;
        *r
    };
    let step = steps
        .lock()
        .unwrap()
        .pop_front()
        .expect("unscripted request");
    let (name, args, prompt) = match step {
        Step::Hang => return Some(stream),
        Step::Call(name, args, prompt) => (name, args, prompt),
    };
    let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": format!("c{n}"),
        "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]});
    let sse = format!(
        "data: {call}\n\n\
data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n\
data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":20}}}}\n\n\
data: [DONE]\n\n"
    );
    let mut out = stream;
    let _ = write!(
        out,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    );
    None
}

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    ws: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, ws) = (root.join("owner"), root.join("ws"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(git.success());
    Env {
        _dir: dir,
        home,
        ws,
    }
}

fn command(e: &Env, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_duet"));
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

fn start_run(e: &Env, f: &Frontier) -> Child {
    command(
        e,
        &[
            "run",
            "--mode",
            "passthrough",
            "--no-privacy",
            "--frontier-url",
            &f.url(),
            "--frontier-model",
            "glm-5.3-flash",
            "Add a function b to src/lib.rs.",
        ],
    )
    .spawn()
    .unwrap()
}

fn duet(e: &Env, args: &[&str]) -> Output {
    command(e, args).output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn only_run_id(e: &Env) -> String {
    let ids: Vec<String> = std::fs::read_dir(e.ws.join(".duet/runs"))
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(ids.len(), 1, "{ids:?}");
    ids[0].clone()
}

fn summary(e: &Env, id: &str) -> Value {
    serde_json::from_slice(
        &std::fs::read(e.ws.join(".duet/runs").join(id).join("summary.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn ctrl_c_ends_a_run_resumably_and_resume_completes_it() {
    let e = env();
    let f = Frontier::start();
    f.script(vec![
        Step::Call(
            "write_file",
            json!({"path": "src/b.rs", "content": "pub fn b() {}\n"}),
            1000,
        ),
        Step::Hang,
    ]);
    let child = start_run(&e, &f);
    // Interrupt while the frontier is "thinking" about the second request.
    let started = Instant::now();
    while f.requests() < 2 {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "no second request"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::Int).unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let printed: Value = serde_json::from_slice(&o.stdout).unwrap();
    let id = only_run_id(&e);
    assert_eq!(printed, summary(&e, &id));
    assert_eq!(printed["terminal"]["state"], "failed");
    assert_eq!(
        printed["terminal"]["reason"],
        "interrupted; resume with `duet resume`"
    );
    assert_eq!(
        std::fs::read_to_string(e.ws.join("src/b.rs")).unwrap(),
        "pub fn b() {}\n"
    );

    // Resume: the frontier finishes; the run completes.
    f.script(vec![Step::Call(
        "finish",
        json!({"summary": "added b"}),
        1000,
    )]);
    let o = duet(&e, &["resume", &id]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let s = summary(&e, &id);
    assert_eq!(s["terminal"]["state"], "completed");
    assert_eq!(s["terminal"]["summary"], "added b");
    assert_eq!(f.requests(), 3);

    // One audit log across both sessions: its chain and its anchor verify.
    let o = duet(&e, &["audit", "verify", &id]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("chain intact"), "{}", text(&o));
    let o = duet(&e, &["audit", "show", &id]);
    let shown = text(&o);
    assert_eq!(shown.matches("run_end").count(), 2, "{shown}");

    // A completed run is not resumed, and resume checks the id.
    let o = duet(&e, &["resume", &id]);
    assert!(!o.status.success());
    assert!(text(&o).contains("already completed"), "{}", text(&o));
    let o = duet(&e, &["resume", "../elsewhere"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("invalid run id"), "{}", text(&o));
    assert_eq!(f.requests(), 3);
}

#[test]
fn the_dollar_budget_ends_a_run_as_budget_stopped() {
    let e = env();
    // glm-5.3-flash input is $0.15 per million tokens: 100K tokens a turn
    // is $0.015, over this budget after the first turn.
    std::fs::write(
        e.home.join("config.toml"),
        "[limits]\nfrontier_usd = 0.01\n",
    )
    .unwrap();
    let f = Frontier::start();
    f.script(vec![
        Step::Call("read_file", json!({"path": "src/lib.rs"}), 100_000),
        Step::Call("finish", json!({"summary": "done"}), 100_000),
    ]);
    let o = start_run(&e, &f).wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));
    let s = summary(&e, &only_run_id(&e));
    assert_eq!(s["terminal"]["state"], "budget_stopped");
    assert_eq!(s["terminal"]["which"], "frontier_usd");
    assert_eq!(f.requests(), 1);
    let o = duet(&e, &["audit", "verify", &only_run_id(&e)]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
}

#[test]
fn progress_goes_to_standard_error_and_standard_output_keeps_the_summary() {
    for quiet in [false, true] {
        let e = env();
        let f = Frontier::start();
        f.script(vec![
            Step::Call(
                "write_file",
                json!({"path": "src/b.rs", "content": "pub fn b() {}\n"}),
                1000,
            ),
            Step::Call("finish", json!({"summary": "added b"}), 1000),
        ]);
        let url = f.url();
        let mut args = vec![
            "run",
            "--mode",
            "passthrough",
            "--no-privacy",
            "--frontier-url",
            &url,
            "--frontier-model",
            "glm-5.3-flash",
        ];
        if quiet {
            args.push("--quiet");
        }
        args.push("Add a function b to src/lib.rs.");
        let o = duet(&e, &args);
        assert_eq!(o.status.code(), Some(0), "{}", text(&o));
        // Standard output is the summary (as summary.json holds it) and
        // nothing else.
        let id = only_run_id(&e);
        let stored = std::fs::read(e.ws.join(".duet/runs").join(&id).join("summary.json")).unwrap();
        assert_eq!(o.stdout, [stored, b"\n".to_vec()].concat());
        // Standard error (not a terminal): one plain line per step.
        let stderr = String::from_utf8(o.stderr).unwrap();
        assert!(!stderr.contains('\x1b'), "{stderr}");
        for step in ["  · write_file src/b.rs", "  · finish (running the checks)"] {
            assert_eq!(stderr.contains(step), !quiet, "{step}: {stderr}");
        }
    }
}
