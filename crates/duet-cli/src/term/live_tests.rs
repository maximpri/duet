// SPDX-License-Identifier: GPL-3.0-or-later
//! The live view against a real session in hybrid mode: the frontier's reply
//! streams in small pieces that cut through placeholders; the operator sees
//! the real values, the frontier never gets them, and watching changes no
//! request.

use super::feed::{Feed, Streamed};
use bytes::Bytes;
use duet_agent::session::{SessionLimits, TurnEnd};
use duet_agent::{RunConfig, Session};
use duet_boundary::OutboundGate;
use duet_boundary::audit::AuditLog;
use duet_boundary::engine::Engine;
use duet_boundary::live::{StreamEvent, StreamTap};
use duet_boundary::policy::Policy;
use duet_boundary::view::Presenter;
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SECRET: &str = "sk_live_9f8e7d6c5b4a39281706";
const EMAIL: &str = "marta.kowalczyk@corp-mail.net";

/// A frontier that answers with text and a `reply` quoting the
/// placeholders of the operator's message, streamed three bytes at a time.
#[derive(Clone, Default)]
struct Frontier {
    bodies: Arc<Mutex<Vec<String>>>,
}

fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('⟨') {
        let Some(len) = rest[start..].find('⟩') else {
            break;
        };
        let end = start + len + '⟩'.len_utf8();
        out.push(rest[start..end].to_owned());
        rest = &rest[end..];
    }
    out
}

impl Transport for Frontier {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        let text = String::from_utf8(body).unwrap();
        self.bodies.lock().unwrap().push(text.clone());
        let body: Value = serde_json::from_str(&text).unwrap();
        let last = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m["role"] == "user")
            .and_then(|m| m["content"].as_str())
            .unwrap_or_default()
            .to_owned();
        let tokens = placeholders(&last);
        assert!(tokens.len() >= 2, "{last}");
        let said = format!("Checking {}.", tokens[0]);
        let args = json!({"message": format!(
            "## Done\n- I will not contact {} directly.\n- The key {} is rotated.",
            tokens[0], tokens[1]
        )})
        .to_string();
        let pieces = |s: &str| -> Vec<String> {
            let chars: Vec<char> = s.chars().collect();
            chars.chunks(3).map(|c| c.iter().collect()).collect()
        };
        let mut chunks: Vec<String> = pieces(&said)
            .into_iter()
            .map(|p| {
                format!(
                    "data: {}\n\n",
                    json!({"choices": [{"delta": {"content": p}}]})
                )
            })
            .collect();
        for (i, p) in pieces(&args).into_iter().enumerate() {
            let call = if i == 0 {
                json!({"index": 0, "id": "c1", "type": "function",
                    "function": {"name": "reply", "arguments": p}})
            } else {
                json!({"index": 0, "function": {"arguments": p}})
            };
            chunks.push(format!(
                "data: {}\n\n",
                json!({"choices": [{"delta": {"tool_calls": [call]}}]})
            ));
        }
        chunks.push(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n".into(),
        );
        chunks.push(
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":20}}\n\n"
                .into(),
        );
        chunks.push("data: [DONE]\n\n".into());
        Box::pin(async move {
            Ok(HttpReply {
                status: 200,
                headers: vec![],
                body: futures_util::stream::iter(chunks.into_iter().map(|c| Ok(Bytes::from(c))))
                    .boxed(),
            })
        })
    }
}

#[derive(Default)]
struct Collect(Mutex<Vec<Streamed>>);

impl StreamTap for Collect {
    fn event(&self, e: StreamEvent<'_>) {
        self.0.lock().unwrap().push(e.into());
    }
}

/// One hybrid session turn; returns how it ended, the frontier's request
/// bodies and what the tap saw (when watched).
async fn turn(watched: bool) -> (TurnEnd, Vec<String>, Vec<Streamed>, Arc<Engine>) {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap().join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join(".env"), format!("STRIPE_KEY={SECRET}\n")).unwrap();
    std::fs::write(
        ws.join("data/customers.csv"),
        format!("id,email\n1,{EMAIL}\n"),
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
    let run_dir = ws.join(".duet/runs/s1");
    let frontier = Frontier::default();
    let mut pc = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    pc.backoff_scale = 0.0;
    let provider = ChatProvider::new(pc, Box::new(frontier.clone())).unwrap();
    let policy = Policy {
        sensitive_globs: vec![".env*".into(), "data/**".into()],
        detect_secrets: true,
        detect_pii: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        ..Policy::default()
    };
    let engine = Engine::open(&run_dir, policy, None).unwrap();
    let files = [".env".to_owned(), "data/customers.csv".to_owned()];
    engine.prime(&ws, &files, "Look after the customers.");
    let (filter, check) = engine.outbound();
    let gated = OutboundGate::new(AuditLog::open(&dir.path().join("audit.jsonl")).unwrap())
        .with_filter(filter)
        .with_check(check)
        .wrap(provider);
    let cfg = RunConfig {
        mode: "hybrid".into(),
        price: Box::new(|u| u.input as f64 / 1e6),
        ..RunConfig::new(ws.clone(), run_dir.clone(), "Look after the customers.")
    };
    let git = duet_git::Git::locate().unwrap();
    let mut session = Session::open(
        &cfg,
        &gated,
        engine.as_ref() as &dyn Presenter,
        &git,
        Arc::new(AtomicBool::new(false)),
        SessionLimits {
            frontier_usd: 100.0,
            working_time: Duration::from_secs(3600),
        },
        false,
    )
    .unwrap();
    let message = format!("Customer {EMAIL} complained; the key {SECRET} must be rotated.");
    let tap = Arc::new(Collect::default());
    let end = if watched {
        duet_boundary::live::observe(tap.clone(), session.turn(&message)).await
    } else {
        session.turn(&message).await
    };
    let bodies = frontier.bodies.lock().unwrap().clone();
    let seen = tap.0.lock().unwrap().clone();
    (end, bodies, seen, engine)
}

#[tokio::test(flavor = "multi_thread")]
async fn in_hybrid_the_operator_sees_values_as_they_stream_and_the_frontier_never_does() {
    let (end, bodies, seen, engine) = turn(true).await;
    let (plain_end, plain_bodies, unseen, _) = turn(false).await;
    // Watching changes nothing the frontier gets or the session ends with.
    assert_eq!(end, plain_end);
    assert_eq!(bodies, plain_bodies);
    assert!(unseen.is_empty());
    for b in &bodies {
        assert!(!b.contains(EMAIL) && !b.contains(SECRET), "leaked: {b}");
    }
    // The pieces cut through placeholders.
    let pieces: Vec<&str> = seen
        .iter()
        .filter_map(|e| match e {
            Streamed::Text(t) | Streamed::Arguments { delta: t, .. } => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        pieces.iter().any(|p| p.contains('⟨') && !p.contains('⟩')),
        "{pieces:?}"
    );
    // The operator's view, fed as it streamed: real values, never a piece
    // of a placeholder, and the reply is not repeated at the end.
    let restore = engine.clone();
    let mut feed = Feed::new(
        false,
        100,
        true,
        Arc::new(move |t: &str| restore.detokenize(t)),
    );
    feed.begin(std::time::Instant::now());
    let mut lines = Vec::new();
    for e in &seen {
        lines.extend(feed.stream(e));
        if let Some(p) = feed.partial() {
            assert!(!p.contains('⟨'), "{p}");
        }
    }
    lines.extend(feed.end(&end));
    assert_eq!(
        lines,
        [
            format!("duet: Checking {EMAIL}."),
            "duet: ## Done".to_owned(),
            format!("      • I will not contact {EMAIL} directly."),
            format!("      • The key {SECRET} is rotated."),
        ]
    );
    let TurnEnd::Replied { message } = &end else {
        panic!("{end:?}")
    };
    assert!(message.contains(EMAIL) && message.contains(SECRET));
}
