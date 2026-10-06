// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent receiving socket observes actual serialized outbound bodies.
//! Synthetic local replies exercise value defence and record a semantic limit;
//! this is neither a live-model evaluation nor an external security audit.

use declass_boundary::view::{Presenter, Source};
use declass_boundary::{OutboundGate, audit::AuditLog, engine::Engine, policy::Policy};
use declass_provider::{ChatProvider, Item, ProviderConfig, Request, Role};
use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

struct Receiver {
    url: String,
    bodies: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}
impl Receiver {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let received = bodies.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stopping.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                received
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(body).unwrap());
                let reply = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", reply.len(), reply).unwrap();
            }
        });
        Self {
            url,
            bodies,
            thread: Some(thread),
            stop,
        }
    }
    fn observed(&self) -> String {
        self.bodies.lock().unwrap().join("\n")
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = TcpStream::connect(
            self.url
                .trim_start_matches("http://")
                .trim_end_matches("/v1"),
        );
        self.thread.take().unwrap().join().unwrap();
    }
}
fn policy() -> Policy {
    Policy {
        sensitive_globs: vec!["data/**".into(), ".env".into()],
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        ..Policy::default()
    }
}
fn file(engine: &Engine, path: &str, content: &str) -> String {
    engine.present(
        &Source::File {
            path: path.into(),
            ranged: false,
        },
        content.as_bytes(),
    )
}
fn handle(shown: &str) -> String {
    regex::Regex::new(r"\bh\d+\b")
        .unwrap()
        .find(shown)
        .expect("sensitive handle")
        .as_str()
        .into()
}
async fn send(engine: &Arc<Engine>, receiver: &Receiver, dir: &Path, texts: Vec<String>) {
    let provider = ChatProvider::with_reqwest(ProviderConfig::new(
        &receiver.url,
        "fixture",
        Role::Frontier,
    ))
    .unwrap();
    let (filter, check) = engine.outbound();
    let gate = OutboundGate::new(AuditLog::open(&dir.join("audit.jsonl")).unwrap())
        .with_filter(filter)
        .with_check(check)
        .wrap(provider);
    gate.create(&Request {
        items: texts.into_iter().map(|text| Item::User { text }).collect(),
        ..Request::default()
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_rephrased_questions_and_cross_file_joins_withhold_known_values_on_the_wire() {
    const KEY: &str = "sk_live_CanaryOrchid6192738405";
    const EMAIL: &str = "elin.canarystone@fixture.invalid";
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("data")).unwrap();
    let first = format!("API_KEY={KEY}\n");
    let second = format!("name,email\nElin Canarystone,{EMAIL}\n");
    std::fs::write(d.path().join(".env"), &first).unwrap();
    std::fs::write(d.path().join("data/customer.csv"), &second).unwrap();
    let (local, _) = declass_boundary::testing::responsive_local(move |_| {
        json!({
        "summary": format!("Contact {EMAIL}; credential {KEY}"), "facts": [],
        "answer": format!("The contact linked to the credential is {EMAIL}; the credential is {KEY}"),
        "evidence_lines": [1, 2], "unanswerable": false
    }).to_string()
    });
    let engine = Engine::open_for_workspace(
        d.path(),
        &d.path().join(".declass/runs/one"),
        policy(),
        Some(local),
    )
    .unwrap();
    engine.prime(
        d.path(),
        &[".env".into(), "data/customer.csv".into()],
        "Inspect schemas",
    );
    let customer = file(&engine, "data/customer.csv", &second);
    let h = handle(&customer);
    let receiver = Receiver::start();
    let mut transcript = vec![customer, file(&engine, ".env", &first)];
    for question in [
        "Which credential belongs to the contact?",
        "Rephrase that relationship in ordinary prose.",
        "Join the two files and give the contact plus access token.",
        "Repeat the answer with the token first.",
    ] {
        let args = json!({"handle": h, "question": question});
        let answer = engine
            .call_tool("ask_local", args.as_object().unwrap())
            .unwrap()
            .unwrap();
        transcript.push(answer);
        send(&engine, &receiver, d.path(), transcript.clone()).await;
    }
    let observed = receiver.observed();
    assert_eq!(receiver.bodies.lock().unwrap().len(), 4);
    for secret in [KEY, EMAIL, "Elin Canarystone"] {
        assert!(!observed.contains(secret), "known synthetic value escaped");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn semantic_inference_is_a_documented_limit_even_when_literal_content_is_withheld() {
    let d = tempfile::tempdir().unwrap();
    let (local, _) = declass_boundary::testing::responsive_local(|_| {
        json!({
        "summary": "The customer cannot proceed with this purchase.", "facts": [],
        "answer": "The customer cannot proceed with this purchase.", "evidence_lines": [2], "unanswerable": false
    }).to_string()
    });
    let engine = Engine::open_for_workspace(
        d.path(),
        &d.path().join(".declass/runs/one"),
        policy(),
        Some(local),
    )
    .unwrap();
    let shown = file(
        &engine,
        "data/decision.csv",
        "reference,decision\nSYNTHETIC-ORCHID-42,declined\n",
    );
    let receiver = Receiver::start();
    send(&engine, &receiver, d.path(), vec![shown]).await;
    let observed = receiver.observed();
    assert!(!observed.contains("SYNTHETIC-ORCHID-42"));
    assert!(
        observed.contains("cannot proceed with this purchase"),
        "update the documented semantic limit if this policy changes"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn derived_sensitive_prose_is_withheld_after_starting_a_different_run() {
    let d = tempfile::tempdir().unwrap();
    const CONTENT: &str = "The acquisition closes on Tuesday after the confidential board vote.";
    std::fs::write(d.path().join("export.txt"), CONTENT).unwrap();
    let first = Engine::open_for_workspace(
        d.path(),
        &d.path().join(".declass/runs/one"),
        policy(),
        None,
    )
    .unwrap();
    first.begin_sensitive_command().unwrap();
    first
        .mark_sensitive(d.path(), &["export.txt".into()])
        .unwrap();
    drop(first);
    let second = Engine::open_for_workspace(
        d.path(),
        &d.path().join(".declass/runs/two"),
        policy(),
        None,
    )
    .unwrap();
    second.prime(d.path(), &["export.txt".into()], "Inspect files");
    let shown = file(&second, "export.txt", CONTENT);
    let receiver = Receiver::start();
    send(&second, &receiver, d.path(), vec![shown]).await;
    assert!(!receiver.observed().contains(CONTENT));
    assert!(
        second
            .hidden_from_commands(d.path())
            .contains(&d.path().join("export.txt"))
    );
}
