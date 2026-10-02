// SPDX-License-Identifier: GPL-3.0-or-later
//! The outbound gate is the same for every wire dialect: the engine's filter
//! and check run on Duet's own request, and the body the audit log records is
//! byte for byte the body the transport sends, at the dialect's endpoint. A
//! provider refusing replayed reasoning gets one filtered, checked and audited
//! resend without it.

use bytes::Bytes;
use duet_boundary::audit::{AuditLog, Line, read};
use duet_boundary::engine::Engine;
use duet_boundary::model::{Item, Request, ToolCall};
use duet_boundary::policy::Policy;
use duet_boundary::{OutboundCheck, OutboundGate};
use duet_provider::client::{HttpReply, Transport};
use duet_provider::types::{Part, Replay};
use duet_provider::{ChatProvider, Dialect, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const SECRET: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";

#[derive(Clone, Default)]
struct Sent {
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Clone, Default)]
struct Script {
    replies: Arc<Mutex<VecDeque<(u16, String)>>>,
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl Script {
    fn new(replies: Vec<(u16, String)>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into())),
            sent: Arc::default(),
        }
    }

    fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }
}

impl Transport for Script {
    fn post(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        self.sent.lock().unwrap().push(Sent { url, headers, body });
        let (status, text) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unscripted request");
        Box::pin(async move {
            Ok(HttpReply {
                status,
                headers: vec![],
                body: futures_util::stream::iter([Ok(Bytes::from(text))]).boxed(),
            })
        })
    }
}

/// A successful streamed reply in each dialect.
fn ok_stream(dialect: Dialect) -> String {
    let events: Vec<Value> = match dialect {
        Dialect::Chat => vec![
            json!({"choices": [{"delta": {"content": "done"}, "finish_reason": "stop"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 1}}),
        ],
        Dialect::Anthropic => vec![
            json!({"type": "message_start", "message": {"id": "m", "usage": {"input_tokens": 10, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "done"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
            json!({"type": "message_stop"}),
        ],
        Dialect::Responses => vec![json!({"type": "response.completed", "response": {
            "status": "completed",
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "done"}]}],
            "usage": {"input_tokens": 10, "output_tokens": 1}}})],
    };
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

fn engine(dir: &std::path::Path) -> Arc<Engine> {
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join(".env"), format!("PAYMENTS_API_KEY={SECRET}\n")).unwrap();
    let policy = Policy {
        sensitive_globs: vec![".env*".into()],
        secret_sinks: vec![".env*".into()],
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        ..Policy::default()
    };
    let e = Engine::open(&dir.join("run"), policy, None).unwrap();
    assert_eq!(
        e.prime(&ws, &[".env".to_owned()], "fix the payments client"),
        1
    );
    e
}

fn provider(dialect: Dialect, script: &Script) -> ChatProvider {
    let mut cfg = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    cfg.dialect = dialect;
    cfg.backoff_scale = 0.0;
    cfg.max_attempts = Some(3);
    ChatProvider::new(cfg, Box::new(script.clone())).unwrap()
}

fn requests(audit: &std::path::Path) -> Vec<Value> {
    read(audit)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Line::Request(r) => Some(r.request),
            Line::Event(_) => None,
        })
        .collect()
}

fn leaky_request() -> Request {
    let raw =
        json!({"path": "client.rs", "content": format!("let key = \"{SECRET}\";")}).to_string();
    Request {
        system: "You are a coding agent.".into(),
        items: vec![
            Item::User {
                text: format!("the key is {SECRET}"),
            },
            Item::Assistant {
                text: String::new(),
                reasoning: None,
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "write_file".into(),
                    arguments: serde_json::from_str::<Map<String, Value>>(&raw).unwrap(),
                    raw_arguments: raw,
                }],
                replay: None,
            },
            Item::ToolResult {
                call_id: "c1".into(),
                content: format!("wrote {SECRET}"),
            },
        ],
        ..Request::default()
    }
}

#[tokio::test]
async fn every_dialect_sends_only_the_filtered_audited_body() {
    for dialect in [Dialect::Chat, Dialect::Anthropic, Dialect::Responses] {
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        let (filter, check) = e.outbound();
        let script = Script::new(vec![(200, ok_stream(dialect))]);
        let audit = d.path().join("audit.jsonl");
        let gated = OutboundGate::new(AuditLog::open(&audit).unwrap())
            .with_filter(filter)
            .with_check(check)
            .wrap(provider(dialect, &script));
        let (response, interventions) = gated.create(&leaky_request()).await.unwrap();
        assert_eq!(response.text, "done", "{dialect:?}");
        assert!(!interventions.is_empty(), "{dialect:?}");
        let sent = script.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].url,
            format!("https://frontier.example/v1{}", dialect.path())
        );
        let wire = String::from_utf8(sent[0].body.clone()).unwrap();
        assert!(
            !wire.contains(SECRET),
            "{dialect:?} sent the secret: {wire}"
        );
        assert!(
            wire.contains("⟨secret:PAYMENTS_API_KEY"),
            "{dialect:?}: {wire}"
        );
        let audited = requests(&audit);
        assert_eq!(audited.len(), 1);
        assert_eq!(
            audited[0],
            serde_json::from_slice::<Value>(&sent[0].body).unwrap(),
            "{dialect:?}: the audit log holds the body that was sent"
        );
    }
}

#[tokio::test]
async fn a_blocked_body_is_blocked_in_every_dialect() {
    // Without the filter, the final check alone stops the send.
    for dialect in [Dialect::Chat, Dialect::Anthropic, Dialect::Responses] {
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        let (_, check) = e.outbound();
        let script = Script::new(vec![]);
        let gated = OutboundGate::new(AuditLog::open(&d.path().join("a.jsonl")).unwrap())
            .with_check(check)
            .wrap(provider(dialect, &script));
        let err = gated.create(&leaky_request()).await.unwrap_err();
        assert!(
            matches!(err, duet_boundary::GateError::Blocked { .. }),
            "{dialect:?}: {err}"
        );
        assert!(script.sent().is_empty());
    }
}

struct RejectImage {
    digest: String,
    seen: Arc<Mutex<Vec<Vec<String>>>>,
}

impl OutboundCheck for RejectImage {
    fn name(&self) -> &'static str {
        "image-policy"
    }

    fn check(&self, body: &Value) -> Result<(), String> {
        assert!(!body.to_string().contains("data:image/"));
        Ok(())
    }

    fn check_images(&self, digests: &[String]) -> Result<(), String> {
        self.seen.lock().unwrap().push(digests.to_vec());
        if digests.contains(&self.digest) {
            Err("image is private".into())
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn a_rejected_image_never_reaches_any_dialect_or_the_audit_log() {
    let image =
        duet_provider::image::prepare(&duet_provider::image::solid_png(16, 16, [9, 17, 25]), 64)
            .unwrap();
    let request = Request {
        items: vec![
            Item::User {
                text: "Inspect this image".into(),
            },
            Item::Images {
                call_id: None,
                images: vec![image.clone()],
            },
        ],
        ..Request::default()
    };
    for dialect in [Dialect::Chat, Dialect::Anthropic, Dialect::Responses] {
        let d = tempfile::tempdir().unwrap();
        let audit = d.path().join("audit.jsonl");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let script = Script::new(vec![]);
        let gated = OutboundGate::new(AuditLog::open(&audit).unwrap())
            .with_check(Box::new(RejectImage {
                digest: image.sha256.clone(),
                seen: seen.clone(),
            }))
            .wrap(provider(dialect, &script));

        let err = gated.create(&request).await.unwrap_err();
        assert!(
            matches!(err, duet_boundary::GateError::Blocked { .. }),
            "{dialect:?}: {err}"
        );
        assert_eq!(*seen.lock().unwrap(), [vec![image.sha256.clone()]]);
        assert!(script.sent().is_empty(), "{dialect:?}");
        let lines = read(&audit).unwrap();
        assert!(matches!(lines.as_slice(), [Line::Event(_)]), "{lines:?}");
        let log = std::fs::read_to_string(&audit).unwrap();
        assert!(log.contains("image-policy"), "{dialect:?}: {log}");
        assert!(!log.contains(&image.sha256), "{dialect:?}: {log}");
        assert!(!log.contains(&image.base64()), "{dialect:?}: {log}");
    }
}

#[tokio::test]
async fn refused_replayed_reasoning_is_dropped_from_the_front_and_resent() {
    let d = tempfile::tempdir().unwrap();
    let thinking = json!({"type": "thinking", "thinking": "", "signature": "c2lnbmVk"});
    let mut req = Request {
        system: "s".into(),
        items: vec![
            Item::User { text: "hi".into() },
            Item::Assistant {
                text: "ok".into(),
                reasoning: None,
                tool_calls: Vec::new(),
                replay: Some(Replay {
                    dialect: "anthropic".into(),
                    parts: vec![
                        Part::Opaque {
                            block: thinking.clone(),
                        },
                        Part::Text { bytes: 2 },
                    ],
                }),
            },
            Item::User {
                text: "again".into(),
            },
        ],
        ..Request::default()
    };
    let refusal = json!({"type": "error", "error": {"type": "invalid_request_error",
        "message": "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation."}});
    let script = Script::new(vec![
        (400, refusal.to_string()),
        (200, ok_stream(Dialect::Anthropic)),
        (200, ok_stream(Dialect::Anthropic)),
    ]);
    let audit = d.path().join("audit.jsonl");
    let gated = OutboundGate::new(AuditLog::open(&audit).unwrap())
        .wrap(provider(Dialect::Anthropic, &script));
    gated.create(&req).await.unwrap();
    let sent = script.sent();
    assert_eq!(sent.len(), 2, "the refusal is not retried as is");
    let bodies: Vec<String> = sent
        .iter()
        .map(|s| String::from_utf8(s.body.clone()).unwrap())
        .collect();
    assert!(bodies[0].contains("c2lnbmVk") && !bodies[1].contains("c2lnbmVk"));
    assert_eq!(requests(&audit).len(), 2, "both sends are audited");
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01")
    );
    // Later turns keep the old turn without its reasoning: the prefix is stable.
    req.items.push(Item::Assistant {
        text: "fine".into(),
        reasoning: None,
        tool_calls: Vec::new(),
        replay: None,
    });
    req.items.push(Item::User {
        text: "more".into(),
    });
    gated.create(&req).await.unwrap();
    let third = String::from_utf8(script.sent()[2].body.clone()).unwrap();
    assert!(!third.contains("c2lnbmVk"));
}
