// SPDX-License-Identifier: GPL-3.0-or-later
//! Hybrid mode with the local model off (`local.enabled = false`, the
//! `duet-hybrid-nolocal` evaluation lane) through the binary, against a
//! scripted frontier on loopback: no local server is probed or contacted,
//! sensitive files reach the frontier only as handles, `ask_local` is refused,
//! and none of the planted values appears in any frontier request. Local-only
//! mode and settings that need a local model to protect data are refused.

use duet_provider::mock_http::MockServer;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

const EMAIL: &str = "odile.vantersmark@example.org";
const PERSON: &str = "Odile Vantersmark";
const KEY: &str = "sk_live_9fQ2vX7mR4tL8wK3nZ6pB1cD5hJ0";

/// A loopback frontier that answers each request with the next scripted tool
/// call and keeps every request body.
struct Frontier {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Frontier {
    fn start(script: Vec<(&'static str, Value)>) -> Self {
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
    steps: &Mutex<VecDeque<(&'static str, Value)>>,
    bodies: &Mutex<Vec<String>>,
) -> Option<()> {
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
    let (name, args) = steps
        .lock()
        .unwrap()
        .pop_front()
        .expect("unscripted request");
    let call = json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": format!("c{n}"),
        "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]});
    let sse = format!(
        "data: {call}\n\n\
data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n\
data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":900,\"completion_tokens\":20}}}}\n\n\
data: [DONE]\n\n"
    );
    let mut out = stream;
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

fn env(owner_config: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, ws) = (root.join("owner"), root.join("ws"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn export() {}\n").unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,name,email,api_key\n1,{PERSON},{EMAIL},{KEY}\nError: row 2 has no email\n"),
    )
    .unwrap();
    std::fs::write(home.join("config.toml"), owner_config).unwrap();
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

/// `duet` with `DUET_LOCAL_PORTS` pointing at `local_trap`, where a bootstrap
/// probe would land.
fn duet(e: &Env, args: &[&str], local_trap: &MockServer) -> Output {
    Command::new(env!("CARGO_BIN_EXE_duet"))
        .args(args)
        .arg("--workspace")
        .arg(&e.ws)
        .env("DUET_CONFIG_HOME", &e.home)
        .env("ZAI_API_KEY", "loopback-test-key")
        .env("DUET_LOCAL_PORTS", local_trap.port.to_string())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// A local model server that would answer bootstrap's listing: any request
/// it records means Duet looked for a local model.
fn local_trap() -> MockServer {
    MockServer::start(&[(
        "GET /v1/models",
        200,
        r#"{"data":[{"id":"coder","max_model_len":65536}]}"#,
    )])
}

#[test]
fn hybrid_without_a_local_model_shows_handles_and_sends_nothing_sensitive() {
    let e = env("[local]\nenabled = false\n");
    let trap = local_trap();
    let frontier = Frontier::start(vec![
        ("read_file", json!({"path": "data/customers.csv"})),
        (
            "ask_local",
            json!({"handle": "h1", "questions": ["Quote row 1 in full."]}),
        ),
        ("finish", json!({"summary": "looked at the data"})),
    ]);
    let o = duet(
        &e,
        &[
            "run",
            "--mode",
            "hybrid",
            "--frontier-url",
            &frontier.url(),
            "--frontier-model",
            "glm-5.3-flash",
            "Find out why the customer export fails.",
        ],
        &trap,
    );
    let out = text(&o);
    assert_eq!(o.status.code(), Some(0), "{out}");
    assert!(
        out.contains("no local model (local.enabled = false)"),
        "{out}"
    );
    assert!(!out.contains("looking for a local server"), "{out}");
    assert!(
        trap.seen().is_empty(),
        "local server contacted: {:?}",
        trap.seen()
    );

    let bodies = frontier.bodies();
    assert_eq!(bodies.len(), 3);
    // The task says there is no local model; the file is a handle with its
    // structural warning, not a raw CSV line selected by an error keyword.
    // The question about it is refused.
    assert!(
        bodies[0].contains("This run has no local model"),
        "{}",
        bodies[0]
    );
    assert!(
        bodies[1].contains("h1 (data/customers.csv)"),
        "{}",
        bodies[1]
    );
    assert!(
        bodies[1].contains("1 row(s) with a number of fields other than 4"),
        "{}",
        bodies[1]
    );
    assert!(
        bodies[1].contains("no local model configured"),
        "{}",
        bodies[1]
    );
    assert!(
        bodies[2].contains("no local model is configured"),
        "{}",
        bodies[2]
    );
    for (i, body) in bodies.iter().enumerate() {
        for planted in [
            EMAIL,
            PERSON,
            "Vantersmark",
            KEY,
            "Error: row 2 has no email",
        ] {
            assert!(!body.contains(planted), "request {i} carries {planted}");
        }
    }
    let summary: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(summary["terminal"]["state"], "completed", "{summary}");
    assert_eq!(
        summary["stats"]["ledger"]["ask_local_calls"], 1,
        "{summary}"
    );
    assert!(summary["stats"]["ledger"]["local"].is_null(), "{summary}");
}

#[test]
fn what_needs_a_local_model_is_refused_when_it_is_off() {
    let trap = local_trap();
    // Local-only mode has nothing to drive it.
    let e = env("[local]\nenabled = false\n");
    let o = duet(&e, &["run", "--mode", "local-only", "Fix it."], &trap);
    assert_ne!(o.status.code(), Some(0));
    assert!(text(&o).contains("needs a local model"), "{}", text(&o));
    // The PII pass would be skipped silently: refused instead, before any request.
    let e = env("[local]\nenabled = false\n[sensitivity]\nlocal_pii_pass = true\n");
    let o = duet(
        &e,
        &[
            "run",
            "--mode",
            "hybrid",
            "--frontier-url",
            "http://127.0.0.1:9/v1",
            "Fix it.",
        ],
        &trap,
    );
    assert_ne!(o.status.code(), Some(0));
    assert!(
        text(&o).contains("sensitivity.local_pii_pass needs a local model"),
        "{}",
        text(&o)
    );
    assert!(!e.ws.join(".duet/runs").exists(), "a run was started");
    assert!(trap.seen().is_empty());
    // Doctor names the conflict, and says so when the local model is simply off.
    let doctor = |e: &Env| -> Value {
        let o = duet(e, &["doctor", "--json"], &trap);
        serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", text(&o)))
    };
    let check = |report: &Value| -> Value {
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "local model")
            .cloned()
            .unwrap_or_else(|| panic!("{report}"))
    };
    assert_eq!(check(&doctor(&e))["status"], "fail");
    let off = check(&doctor(&env("[local]\nenabled = false\n")));
    assert_eq!(off["status"], "pass", "{off}");
    assert!(off["detail"].as_str().unwrap().contains("handles only"));
}
