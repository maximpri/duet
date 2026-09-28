// SPDX-License-Identifier: GPL-3.0-or-later
//! Top clearance through the binary, against a scripted local model on
//! loopback and a frontier that must never be contacted: only the local model
//! works, no web tool is offered, commands get no network (no egress proxy),
//! MCP servers that could reach the network are not started, and the audit log
//! says so; a repository can require it; and `/mode top-clearance` continues a
//! session in it, which cannot be left again in that session.

use duet_provider::mock_http::MockServer;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the scripted model answers a chat request with.
enum Step {
    Call(&'static str, Value),
    Say(&'static str),
}

/// A loopback model that answers each chat request with the next scripted
/// step, answers `GET /v1/models` without using one, and keeps every request
/// body.
struct Model {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Model {
    fn start(script: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let steps = Arc::new(Mutex::new(VecDeque::from(script)));
        let bodies: Arc<Mutex<Vec<String>>> = Arc::default();
        let kept = bodies.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer(stream, &steps, &kept);
            }
        });
        Self { port, bodies }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }
}

fn answer(
    stream: TcpStream,
    steps: &Mutex<VecDeque<Step>>,
    bodies: &Mutex<Vec<String>>,
) -> Option<()> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request = String::new();
    reader.read_line(&mut request).ok()?;
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
    let mut out = stream;
    if request.starts_with("GET ") {
        let models = r#"{"data":[{"id":"coder","max_model_len":65536}]}"#;
        return write!(
            out,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{models}",
            models.len()
        )
        .ok();
    }
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
    let (delta, finish) = match step {
        Step::Call(name, args) => (
            json!({"tool_calls": [{"index": 0, "id": format!("c{n}"), "type": "function",
                "function": {"name": name, "arguments": args.to_string()}}]}),
            "tool_calls",
        ),
        Step::Say(text) => (json!({"content": text}), "stop"),
    };
    let chunk = json!({"choices": [{"delta": delta}]});
    let sse = format!(
        "data: {chunk}\n\n\
data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"{finish}\"}}]}}\n\n\
data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":900,\"completion_tokens\":20}}}}\n\n\
data: [DONE]\n\n"
    );
    write!(
        out,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    )
    .ok()
}

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    ws: PathBuf,
}

/// A repository and an owner configuration: the local model at `local`, the
/// frontier at `frontier` (a trap), and an MCP server reached over HTTP.
fn env(local: &Model, frontier: &MockServer, project_config: Option<&str>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, ws) = (root.join("owner"), root.join("ws"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn export() {}\n").unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[frontier]\nbase_url = \"http://127.0.0.1:{}/v1\"\n\
[local]\nbase_url = \"{}\"\nmodel = \"coder\"\n\
[sandbox]\nnetwork = \"all\"\n\
[mcp.servers.remote]\nurl = \"https://mcp.example.test/mcp\"\n",
            frontier.port,
            local.url()
        ),
    )
    .unwrap();
    if let Some(p) = project_config {
        std::fs::create_dir_all(ws.join(".duet")).unwrap();
        std::fs::write(ws.join(".duet/config.toml"), p).unwrap();
    }
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&ws)
            .status()
            .unwrap()
            .success()
    );
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

/// The names of the tools offered in a chat request.
fn tools(body: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(body).unwrap();
    v["tools"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Every run's recorded mode, and every audit line, of a workspace.
fn runs(ws: &Path) -> (Vec<String>, String) {
    let mut modes = Vec::new();
    for d in std::fs::read_dir(ws.join(".duet/runs")).unwrap() {
        let m: Value =
            serde_json::from_slice(&std::fs::read(d.unwrap().path().join("run.json")).unwrap())
                .unwrap();
        modes.push(m["mode"].as_str().unwrap().to_owned());
    }
    modes.sort();
    let mut audit = String::new();
    for f in std::fs::read_dir(ws.join(".duet/audit")).unwrap() {
        audit.push_str(&std::fs::read_to_string(f.unwrap().path()).unwrap());
    }
    (modes, audit)
}

#[test]
fn only_the_local_model_works_and_nothing_leaves() {
    // Something on this machine a command could reach with any network.
    let reachable = MockServer::start(&[("GET /", 200, "reached")]);
    let probe = format!(
        "env | grep -i proxy || echo no-proxy-in-env; \
         curl -s -m 3 http://127.0.0.1:{}/ || echo unreachable",
        reachable.port
    );
    let local = Model::start(vec![
        Step::Call("run_command", json!({"command": probe})),
        Step::Call("finish", json!({"summary": "Checked the environment."})),
    ]);
    let frontier = MockServer::start(&[]);
    let e = env(&local, &frontier, None);
    let o = command(&e, &["run", "--mode", "top-clearance", "Look around."])
        .output()
        .unwrap();
    let out = text(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert!(out.contains("TOP CLEARANCE"), "{out}");
    assert!(
        out.contains("MCP server `remote` is not started (it is reached over HTTP"),
        "{out}"
    );
    // The frontier was never contacted; the local model did all the work.
    assert!(frontier.seen().is_empty(), "{:?}", frontier.seen().len());
    let bodies = local.bodies();
    assert_eq!(bodies.len(), 2, "{out}");
    let offered = tools(&bodies[0]);
    assert!(offered.contains(&"run_command".to_owned()), "{offered:?}");
    for web in ["web_fetch", "web_search"] {
        assert!(!offered.contains(&web.to_owned()), "{offered:?}");
    }
    // The command ran with no network at all, although the owner allows all:
    // no egress proxy, and not even loopback.
    assert!(bodies[1].contains("no-proxy-in-env"), "{}", bodies[1]);
    assert!(bodies[1].contains("unreachable"), "{}", bodies[1]);
    assert!(reachable.seen().is_empty());
    let (modes, audit) = runs(&e.ws);
    assert_eq!(modes, ["top-clearance"]);
    assert!(audit.contains(r#""mode":"top-clearance""#), "{audit}");
    // `local-only` is the same mode, and a frontier override is refused.
    let o = command(
        &e,
        &[
            "run",
            "--mode",
            "top-clearance",
            "--frontier-model",
            "x",
            "Look around.",
        ],
    )
    .output()
    .unwrap();
    assert_ne!(o.status.code(), Some(0));
    assert!(
        text(&o).contains("top clearance uses no frontier"),
        "{}",
        text(&o)
    );
}

#[test]
fn a_repository_can_require_top_clearance() {
    let local = Model::start(vec![Step::Call("finish", json!({"summary": "Done."}))]);
    let frontier = MockServer::start(&[]);
    let e = env(&local, &frontier, Some("[clearance]\nrequired = \"top\"\n"));
    for mode in ["hybrid", "passthrough"] {
        let mut args = vec!["run", "--mode", mode];
        if mode == "passthrough" {
            args.push("--no-privacy");
        }
        args.push("Do it.");
        let o = command(&e, &args).output().unwrap();
        assert_ne!(o.status.code(), Some(0), "{mode}");
        assert!(
            text(&o).contains("clearance.required is top by this repository's .duet/config.toml"),
            "{}",
            text(&o)
        );
    }
    // Without --mode it runs in top clearance.
    let o = command(&e, &["run", "Do it."]).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(runs(&e.ws).0, ["top-clearance"]);
    assert!(frontier.seen().is_empty());
}

/// A running session: what it printed so far, and its standard input.
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

    fn wait_for(&self, needle: &str) {
        let started = Instant::now();
        while !self.output().contains(needle) {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "waited for {needle:?}; printed:\n{}",
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
        std::thread::sleep(Duration::from_millis(200));
        (status.code().unwrap_or(-1), self.output())
    }
}

#[test]
fn a_session_continues_in_top_clearance_and_cannot_leave_it() {
    let local = Model::start(vec![Step::Say("Done locally.")]);
    let frontier = Model::start(vec![Step::Say("Hello from the frontier.")]);
    let trap = MockServer::start(&[]);
    let e = env(&local, &trap, None);
    let url = frontier.url();
    let mut chat = Chat::start(command(
        &e,
        &[
            "--mode",
            "passthrough",
            "--no-privacy",
            "--frontier-url",
            &url,
            "Say hello.",
        ],
    ));
    chat.wait_for("Hello from the frontier.");
    chat.send("/mode");
    chat.wait_for("this session is passthrough");
    chat.send("/mode top-clearance");
    chat.wait_for("a new session starts with your next message");
    chat.send("Now work on the secret part.");
    chat.wait_for("Done locally.");
    chat.send("/mode hybrid");
    chat.wait_for("top clearance cannot be left in a session");
    chat.send("/close");
    let (code, out) = chat.finish();
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("left open"), "{out}");
    // Each model got only its own session's messages.
    let (f, l) = (frontier.bodies(), local.bodies());
    assert_eq!(f.len(), 1, "{out}");
    assert_eq!(l.len(), 1, "{out}");
    assert!(l[0].contains("secret part") && !f[0].contains("secret part"));
    assert!(!l[0].contains("Say hello."), "top clearance starts fresh");
    assert!(!tools(&l[0]).iter().any(|t| t.starts_with("web_")));
    assert_eq!(runs(&e.ws).0, ["passthrough", "top-clearance"]);
}
