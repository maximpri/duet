// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet chat` end to end through the binary, against a scripted frontier on
//! loopback (no model server, no network beyond 127.0.0.1). The operator
//! drives a session over standard input: a first message, a clarifying
//! question and its answer, a second task, `/status`, `/diff`, `/undo`, an
//! interrupted turn and `/quit`; the session is then resumed, closed, and
//! its one audit log verifies. Approval on without a terminal refuses a
//! session, and `duet resume` refuses one.

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the scripted frontier does with one request.
enum Step {
    Call(&'static str, Value),
    /// Accept the request and never answer.
    Hang,
}

/// A loopback server speaking just enough HTTP/1.1 and SSE for one provider;
/// it keeps each request body.
struct Frontier {
    port: u16,
    steps: Arc<Mutex<VecDeque<Step>>>,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Frontier {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let steps: Arc<Mutex<VecDeque<Step>>> = Arc::default();
        let bodies: Arc<Mutex<Vec<String>>> = Arc::default();
        let (s, b) = (steps.clone(), bodies.clone());
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                if let Some(hung) = serve(stream, &s, &b) {
                    held.push(hung);
                }
            }
        });
        Self {
            port,
            steps,
            bodies,
        }
    }

    fn script(&self, steps: Vec<Step>) {
        self.steps.lock().unwrap().extend(steps);
    }

    fn requests(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }

    fn body(&self, i: usize) -> String {
        self.bodies.lock().unwrap()[i].clone()
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

fn serve(
    stream: TcpStream,
    steps: &Mutex<VecDeque<Step>>,
    bodies: &Mutex<Vec<String>>,
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
        let mut b = bodies.lock().unwrap();
        b.push(String::from_utf8_lossy(&body).into_owned());
        b.len()
    };
    let step = steps
        .lock()
        .unwrap()
        .pop_front()
        .expect("unscripted request");
    let (name, args) = match step {
        Step::Hang => return Some(stream),
        Step::Call(name, args) => (name, args),
    };
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
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(&ws)
                .status()
                .unwrap()
                .success()
        )
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.org",
        "commit",
        "-qm",
        "start",
    ]);
    Env {
        _dir: dir,
        home,
        ws,
    }
}

fn command(e: &Env, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_duet"));
    c.arg("--workspace")
        .arg(&e.ws)
        .args(args)
        .env("DUET_CONFIG_HOME", &e.home)
        .env("ZAI_API_KEY", "loopback-test-key")
        .env_remove("DUET_LOCAL_PORTS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// A running `duet chat`: what it printed so far, and its standard input.
struct Chat {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Arc<Mutex<String>>,
}

impl Chat {
    fn start(mut cmd: Command) -> Self {
        cmd.stdin(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let out: Arc<Mutex<String>> = Arc::default();
        for stream in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let sink = out.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let mut s = sink.lock().unwrap();
                    s.push_str(&line);
                    s.push('\n');
                }
            });
        }
        let stdin = child.stdin.take();
        Self { child, stdin, out }
    }

    fn output(&self) -> String {
        self.out.lock().unwrap().clone()
    }

    /// Waits until `needle` has been printed `count` times.
    fn wait_for(&self, needle: &str, count: usize) {
        let started = Instant::now();
        while self.output().matches(needle).count() < count {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "waited for {needle:?} ({count}x); printed:\n{}",
                self.output()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn finish(mut self) -> (i32, String) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        // Let the readers drain.
        std::thread::sleep(Duration::from_millis(200));
        (status.code().unwrap_or(-1), self.output())
    }
}

fn chat_command(e: &Env, f: &Frontier, extra: &[&str]) -> Command {
    let url = f.url();
    let mut args = vec![
        "chat",
        "--mode",
        "passthrough",
        "--no-privacy",
        "--frontier-url",
        &url,
        "--frontier-model",
        "glm-5.3-flash",
    ];
    args.extend_from_slice(extra);
    command(e, &args)
}

fn only_run_id(e: &Env) -> String {
    let ids: Vec<String> = std::fs::read_dir(e.ws.join(".duet/runs"))
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(ids.len(), 1, "{ids:?}");
    ids[0].clone()
}

#[test]
fn a_session_with_a_question_two_tasks_undo_interrupt_and_resume() {
    let e = env();
    let f = Frontier::start();
    f.script(vec![
        // Turn 1: look, then ask.
        Step::Call("read_file", json!({"path": "src/lib.rs"})),
        Step::Call(
            "ask_operator",
            json!({"question": "Should b return a value?", "options": ["unit", "u32"]}),
        ),
        // Turn 2 (the answer): write, then finish.
        Step::Call(
            "write_file",
            json!({"path": "src/b.rs", "content": "pub fn b() -> u32 { 2 }\n"}),
        ),
        Step::Call("finish", json!({"summary": "Added b returning u32."})),
        // Turn 3 (the second task): edit, then reply.
        Step::Call(
            "edit_file",
            json!({"path": "src/lib.rs", "edits": [{"old": "pub fn a() {}", "new": "pub fn a() {}\npub mod b;"}]}),
        ),
        Step::Call("reply", json!({"message": "Declared the module b in lib.rs."})),
    ]);
    let mut chat = Chat::start(chat_command(&e, &f, &["--", "Add a function b."]));
    chat.wait_for("duet asks: Should b return a value?", 1);
    chat.wait_for("options: unit / u32", 1);
    assert!(chat.output().contains("  · read_file src/lib.rs"));

    chat.send("u32 please");
    chat.wait_for("duet finished: Added b returning u32.", 1);
    chat.send("Also declare the module in lib.rs.");
    chat.wait_for("duet: Declared the module b in lib.rs.", 1);
    assert_eq!(f.requests(), 6);
    // The answer and the second task reached the frontier with the context.
    let last = f.body(5);
    for m in [
        "Add a function b.",
        "u32 please",
        "Also declare the module in lib.rs.",
    ] {
        assert!(last.contains(m), "{m} missing from {last}");
    }
    assert!(
        last.contains("\"reply\"") && last.contains("\"ask_operator\""),
        "{last}"
    );

    chat.send("/status");
    chat.wait_for("turns 3 · frontier requests 6", 1);
    chat.send("/diff");
    chat.wait_for("+pub mod b;", 1);
    chat.wait_for("new file src/b.rs (1 line)", 1);
    // Without a terminal the operator's messages are echoed into the output.
    assert!(chat.output().contains("you> u32 please"));
    chat.send("/undo");
    chat.wait_for("reverted the writes of turn 3 and later:", 1);
    assert_eq!(
        std::fs::read_to_string(e.ws.join("src/lib.rs")).unwrap(),
        "pub fn a() {}\n"
    );
    assert!(e.ws.join("src/b.rs").exists(), "turn 2's file stays");

    // An interrupted turn leaves the session open.
    f.script(vec![Step::Hang]);
    chat.send("Look at it again.");
    let started = Instant::now();
    while f.requests() < 7 {
        assert!(started.elapsed() < Duration::from_secs(60), "no request");
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = rustix::process::Pid::from_raw(chat.child.id() as i32).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::Int).unwrap();
    chat.wait_for("this turn was interrupted", 1);
    chat.send("/nope");
    chat.wait_for("unknown command /nope", 1);
    chat.send("/quit");
    let (code, out) = chat.finish();
    assert_eq!(code, 0, "{out}");
    let id = only_run_id(&e);
    assert!(
        out.contains(&format!("continue with: duet chat --resume {id}")),
        "{out}"
    );
    let summary: Value = serde_json::from_slice(
        &std::fs::read(e.ws.join(".duet/runs").join(&id).join("summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(summary["terminal"]["state"], "failed");

    // `duet resume` is for one-shot runs.
    let o = command(&e, &["resume", &id]).output().unwrap();
    assert!(!o.status.success());
    assert!(
        text(&o).contains("is a session; continue it with `duet chat --resume"),
        "{}",
        text(&o)
    );

    // Resume the most recent session: the recap, the notes, then close.
    f.script(vec![Step::Call(
        "reply",
        json!({"message": "lib.rs is back to one function."}),
    )]);
    let mut chat = Chat::start(chat_command(&e, &f, &["--resume"]));
    chat.wait_for(&format!("session {id} resumed: 4 turn(s)"), 1);
    chat.wait_for("you> Also declare the module in lib.rs.", 1);
    chat.send("What does lib.rs hold now?");
    chat.wait_for("duet: lib.rs is back to one function.", 1);
    let last = f.body(7);
    assert!(last.contains("interrupted your previous turn"), "{last}");
    assert!(
        last.contains("reverted the file changes you made since turn 3"),
        "{last}"
    );
    chat.send("/close");
    let (code, out) = chat.finish();
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&format!("session {id} closed")), "{out}");
    assert!(
        !command(&e, &["chat", "--resume", &id])
            .output()
            .unwrap()
            .status
            .success(),
        "a closed session does not resume"
    );

    // One audit log across both invocations, with an event per message.
    let o = command(&e, &["audit", "verify", &id]).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let o = command(&e, &["audit", "show", &id]).output().unwrap();
    let shown = text(&o);
    assert_eq!(shown.matches("operator_message").count(), 5, "{shown}");
    assert_eq!(shown.matches("run_end").count(), 2, "{shown}");
}

#[test]
fn approval_without_a_terminal_refuses_a_session() {
    let e = env();
    std::fs::write(
        e.home.join("config.toml"),
        "[oversight]\napprove = \"risky\"\n",
    )
    .unwrap();
    let f = Frontier::start();
    let o = chat_command(&e, &f, &["--", "anything"]).output().unwrap();
    assert!(!o.status.success());
    assert!(
        text(&o).contains("standard input is not a terminal; the session was not started"),
        "{}",
        text(&o)
    );
    assert!(!e.ws.join(".duet/runs").exists());
    assert_eq!(f.requests(), 0);
}
