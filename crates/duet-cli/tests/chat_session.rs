// SPDX-License-Identifier: GPL-3.0-or-later
//! A `duet` session end to end through the binary (no terminal: line by line), against a scripted frontier on
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
    /// Answer with the step after this many milliseconds.
    Slow(u64, Box<Step>),
    /// The first step when the request contains the text, else the second.
    IfSent(&'static str, Box<Step>, Box<Step>),
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
    let body = String::from_utf8_lossy(&body).into_owned();
    let n = {
        let mut b = bodies.lock().unwrap();
        b.push(body.clone());
        b.len()
    };
    let mut step = steps
        .lock()
        .unwrap()
        .pop_front()
        .expect("unscripted request");
    let (name, args) = loop {
        step = match step {
            Step::Hang => return Some(stream),
            Step::Call(name, args) => break (name, args),
            Step::Slow(ms, inner) => {
                std::thread::sleep(Duration::from_millis(ms));
                *inner
            }
            Step::IfSent(needle, then, otherwise) => {
                if body.contains(needle) {
                    *then
                } else {
                    *otherwise
                }
            }
        };
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

/// A running `duet` session: what it printed so far, and its standard input.
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
        out.contains(&format!("continue with: duet --resume {id}")),
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
        text(&o).contains("is a session; continue it with `duet --resume"),
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
        !command(&e, &["--resume", &id])
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

#[test]
fn a_message_typed_while_duet_works_steers_the_turn_and_stop_ends_it() {
    let e = env();
    let f = Frontier::start();
    f.script(vec![
        Step::Slow(1500, Box::new(Step::Call("list_files", json!({})))),
        Step::IfSent(
            "tests first",
            Box::new(Step::Call(
                "reply",
                json!({"message": "Understood: tests first."}),
            )),
            Box::new(Step::Call("reply", json!({"message": "Went ahead."}))),
        ),
        // Turn 2: stopped after its first step.
        Step::Slow(1500, Box::new(Step::Call("list_files", json!({})))),
    ]);
    let mut chat = Chat::start(chat_command(&e, &f, &["--", "Plan the change."]));
    let started = Instant::now();
    while f.requests() < 1 {
        assert!(started.elapsed() < Duration::from_secs(60), "no request");
        std::thread::sleep(Duration::from_millis(20));
    }
    chat.send("Please write the tests first.");
    chat.wait_for(
        "▸ for duet after the current step: Please write the tests first.",
        1,
    );
    chat.wait_for(
        "▸ delivered to duet after request 1: Please write the tests first.",
        1,
    );
    chat.wait_for("duet: Understood: tests first.", 1);

    chat.send("Now list everything again.");
    while f.requests() < 3 {
        assert!(started.elapsed() < Duration::from_secs(60), "no request");
        std::thread::sleep(Duration::from_millis(20));
    }
    chat.send("/stop");
    chat.wait_for("■ stopping after the current step", 1);
    chat.wait_for("this turn stopped after a step, as you asked", 1);
    assert_eq!(f.requests(), 3, "no request after the stop");
    chat.send("/quit");
    let (code, out) = chat.finish();
    assert_eq!(code, 0, "{out}");
    let id = only_run_id(&e);
    let o = command(&e, &["audit", "show", &id]).output().unwrap();
    // Two turns and one steering delivery, each audited.
    assert_eq!(
        text(&o).matches("operator_message").count(),
        3,
        "{}",
        text(&o)
    );
}

#[test]
fn piped_output_is_plain_lines_as_before() {
    let e = env();
    let f = Frontier::start();
    f.script(vec![
        Step::Call("read_file", json!({"path": "src/lib.rs"})),
        Step::Call("reply", json!({"message": "Done.\nTwo lines."})),
    ]);
    let mut c = chat_command(&e, &f, &[]);
    c.stdin(Stdio::piped());
    let mut child = c.spawn().unwrap();
    // The message, then the end of input.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Add a function b.\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let id = only_run_id(&e);
    let summary: Value = serde_json::from_slice(
        &std::fs::read(e.ws.join(".duet/runs").join(&id).join("summary.json")).unwrap(),
    )
    .unwrap();
    let cost = summary["stats"]["cost_usd"].as_f64().unwrap();
    // Without a terminal: no prompt, no escape sequences, exactly these lines.
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        format!(
            "  · read_file src/lib.rs\n\
duet: Done.\n      Two lines.\n\
session {id} left open (${cost:.4} so far); continue with: duet --resume {id}\n"
        )
    );
    assert!(
        !o.stderr.contains(&0x1b),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// `duet` on a real terminal (a pseudo-terminal from `script`, 30 rows by 100
/// columns), its screen read back through a terminal emulator: the workspace
/// takes the screen, shows the status bar and the input, takes a typed
/// message, shows the reply and what other code printed, and gives the
/// terminal back when the operator leaves.
#[test]
fn the_workspace_on_a_terminal() {
    let e = env();
    let f = Frontier::start();
    f.script(vec![
        Step::Call("read_file", json!({"path": "src/lib.rs"})),
        Step::Call("reply", json!({"message": "Hello from the workspace."})),
    ]);
    let url = f.url();
    let script: Vec<String> = if cfg!(target_os = "macos") {
        ["script", "-q", "/dev/null"].map(str::to_owned).to_vec()
    } else {
        // util-linux: the command is one string.
        vec![]
    };
    let duet = env!("CARGO_BIN_EXE_duet");
    let inner = format!(
        "stty rows 30 cols 100; exec '{duet}' --workspace '{}' --mode passthrough --no-privacy \
--frontier-url '{url}' --frontier-model glm-5.3-flash",
        e.ws.display()
    );
    let mut c = if script.is_empty() {
        let mut c = Command::new("script");
        c.args(["-q", "-e", "-c", &inner, "/dev/null"]);
        c
    } else {
        let mut c = Command::new(&script[0]);
        c.args(&script[1..]).args(["sh", "-c", &inner]);
        c
    };
    c.env("DUET_CONFIG_HOME", &e.home)
        .env("ZAI_API_KEY", "loopback-test-key")
        .env("TERM", "xterm-256color")
        .env_remove("NO_COLOR")
        .env_remove("DUET_LOCAL_PORTS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let screen = Arc::new(Mutex::new(vt100::Parser::new(30, 100, 0)));
    let mut out = child.stdout.take().unwrap();
    let feed = screen.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = out.read(&mut buf) {
            if n == 0 {
                break;
            }
            feed.lock().unwrap().process(&buf[..n]);
        }
    });
    let shown = || screen.lock().unwrap().screen().contents();
    let wait_for = |what: &str| {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(30) {
            if shown().contains(what) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the screen never showed {what:?}:\n{}", shown());
    };
    let mut keys = child.stdin.take().unwrap();
    wait_for(" message ");
    wait_for("duet · passthrough · glm-5.3-flash");
    keys.write_all(b"Say hello.\r").unwrap();
    wait_for("Hello from the workspace.");
    wait_for("you> Say hello.");
    wait_for("read_file src/lib.rs");
    // `session <id> (Passthrough)` is printed on standard error by the
    // session's setup: it lands in the conversation, not over the screen.
    wait_for("│ session ");
    wait_for("turn 1 · $");
    // Shown with `--nocapture`, for looking at the layout.
    eprintln!("{}", shown());
    keys.write_all(b"/quit\r").unwrap();
    let status = child.wait().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(status.success(), "{status:?}\n{}", shown());
    // The terminal is given back: the main screen holds the closing line.
    let main = shown();
    assert!(main.contains("left open"), "{main}");
    assert!(!main.contains("conversation"), "{main}");
    assert_eq!(f.requests(), 2);
}
