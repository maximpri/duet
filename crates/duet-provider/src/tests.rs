// SPDX-License-Identifier: GPL-3.0-or-later
//! Client behaviour against a scripted transport, plus opt-in live smoke tests.

use crate::client::{ChatProvider, HttpReply, ProviderConfig, Role, Transport};
use crate::error::{ErrorKind, ProviderError};
use crate::types::{Item, Request, StopReason, ToolSpec, UsageStatus};
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

enum Scripted {
    Reply {
        status: u16,
        headers: Vec<(String, String)>,
        chunks: Vec<Result<&'static str, &'static str>>,
    },
    ConnectError,
}

#[derive(Clone)]
struct Script {
    replies: Arc<Mutex<VecDeque<Scripted>>>,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Script {
    fn new(replies: Vec<Scripted>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into())),
            bodies: Arc::default(),
        }
    }
}

impl Transport for Script {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        self.bodies
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        let next = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unscripted request");
        Box::pin(async move {
            match next {
                Scripted::ConnectError => Err(ProviderError::new(
                    ErrorKind::Transport,
                    "connection refused",
                )),
                Scripted::Reply {
                    status,
                    headers,
                    chunks,
                } => {
                    let body = futures_util::stream::iter(chunks.into_iter().map(|c| {
                        c.map(|s| Bytes::from_static(s.as_bytes()))
                            .map_err(str::to_owned)
                    }))
                    .boxed();
                    Ok(HttpReply {
                        status,
                        headers,
                        body,
                    })
                }
            }
        })
    }
}

const OK_STREAM: &[&str] = &[
    "data: {\"id\":\"r\",\"model\":\"glm-5.3\",\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":50,\"completion_tokens\":3,\"prompt_tokens_details\":{\"cached_tokens\":40}}}\n\n",
    "data: [DONE]\n\n",
];

fn ok() -> Scripted {
    Scripted::Reply {
        status: 200,
        headers: vec![],
        chunks: OK_STREAM.iter().map(|s| Ok(*s)).collect(),
    }
}

fn provider(script: &Script) -> ChatProvider {
    let mut cfg = ProviderConfig::new("https://frontier.example/v1", "glm-5.3", Role::Frontier);
    cfg.backoff_scale = 0.0;
    ChatProvider::new(cfg, Box::new(script.clone())).unwrap()
}

fn request() -> Request {
    Request {
        system: "s".into(),
        items: vec![Item::User { text: "hi".into() }],
        ..Request::default()
    }
}

#[tokio::test]
async fn streams_a_response_with_reported_usage() {
    let s = Script::new(vec![ok()]);
    let r = provider(&s).create(&request()).await.unwrap();
    assert_eq!(r.text, "hello");
    assert_eq!(r.stop, StopReason::Stop);
    assert_eq!(
        (r.usage.input, r.usage.cache_read, r.usage.output),
        (10, 40, 3)
    );
    assert_eq!(r.attempts.attempts, 1);
}

#[tokio::test]
async fn retried_failures_do_not_poison_usage() {
    // A 429, a connection error, and a mid-stream provider error, then success.
    let s = Script::new(vec![
        Scripted::Reply {
            status: 429,
            headers: vec![("retry-after".into(), "0".into())],
            chunks: vec![Ok("slow down")],
        },
        Scripted::ConnectError,
        Scripted::Reply {
            status: 200,
            headers: vec![],
            chunks: vec![
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"partial output\"}}]}\n\n"),
                Ok("data: {\"error\":{\"type\":\"overloaded_error\"}}\n\n"),
            ],
        },
        ok(),
    ]);
    let r = provider(&s).create(&request()).await.unwrap();
    assert_eq!(r.attempts.attempts, 4);
    // Billed usage is exactly the successful attempt's reported usage.
    assert_eq!(r.attempts.billed, r.usage);
    assert_eq!(r.usage.status, UsageStatus::Reported);
    // Only the mid-stream failure (which produced output) is estimated, separately.
    assert!(r.attempts.estimated_failed.output > 0);
    assert_eq!(r.attempts.estimated_failed.status, UsageStatus::Estimated);
}

#[tokio::test]
async fn transport_error_mid_stream_retries() {
    let s = Script::new(vec![
        Scripted::Reply {
            status: 200,
            headers: vec![],
            chunks: vec![
                Ok("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n"),
                Err("connection reset"),
            ],
        },
        ok(),
    ]);
    assert_eq!(
        provider(&s)
            .create(&request())
            .await
            .unwrap()
            .attempts
            .attempts,
        2
    );
}

#[tokio::test]
async fn auth_errors_are_not_retried() {
    let s = Script::new(vec![Scripted::Reply {
        status: 401,
        headers: vec![],
        chunks: vec![Ok("bad key")],
    }]);
    let e = provider(&s).create(&request()).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Auth);
}

#[tokio::test]
async fn context_overflow_is_classified_and_not_retried() {
    let s = Script::new(vec![Scripted::Reply {
        status: 400,
        headers: vec![],
        chunks: vec![Ok(
            "{\"error\":\"This model's maximum context length is 65536 tokens\"}",
        )],
    }]);
    assert_eq!(
        provider(&s).create(&request()).await.unwrap_err().kind,
        ErrorKind::ContextOverflow
    );
}

#[tokio::test]
async fn gives_up_after_max_attempts() {
    let s = Script::new(
        (0..6)
            .map(|_| Scripted::Reply {
                status: 503,
                headers: vec![],
                chunks: vec![Ok("down")],
            })
            .collect(),
    );
    let e = provider(&s).create(&request()).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Status(503));
}

#[tokio::test]
async fn length_stop_is_terminal_not_retried() {
    let s = Script::new(vec![Scripted::Reply {
        status: 200,
        headers: vec![],
        chunks: vec![Ok(
            "data: {\"choices\":[{\"delta\":{\"content\":\"cut\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
        )],
    }]);
    let r = provider(&s).create(&request()).await.unwrap();
    assert_eq!(r.stop, StopReason::Length);
    assert_eq!(r.attempts.attempts, 1);
}

#[tokio::test(start_paused = true)]
async fn stalled_stream_times_out_and_retries() {
    struct Stall(Script);
    impl Transport for Stall {
        fn post(
            &self,
            url: String,
            h: Vec<(String, String)>,
            b: Vec<u8>,
        ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
            if self.0.bodies.lock().unwrap().is_empty() {
                self.0.bodies.lock().unwrap().push(json!({}));
                return Box::pin(async {
                    Ok(HttpReply {
                        status: 200,
                        headers: vec![],
                        body: futures_util::stream::pending().boxed(),
                    })
                });
            }
            self.0.post(url, h, b)
        }
    }
    let mut cfg = ProviderConfig::new("https://frontier.example/v1", "m", Role::Frontier);
    cfg.backoff_scale = 0.0;
    cfg.first_byte_timeout = Duration::from_secs(5);
    let p = ChatProvider::new(cfg, Box::new(Stall(Script::new(vec![ok()])))).unwrap();
    assert_eq!(p.create(&request()).await.unwrap().attempts.attempts, 2);
}

#[test]
fn local_role_refuses_non_loopback_endpoints() {
    let cfg = ProviderConfig::new(
        "https://api.z.ai/api/coding/paas/v4",
        "m",
        Role::Local { allowlist: vec![] },
    );
    let e = ChatProvider::new(cfg, Box::new(Script::new(vec![])))
        .err()
        .unwrap();
    assert_eq!(e.kind, ErrorKind::Forbidden);
}

#[tokio::test]
async fn local_role_recovers_text_tool_calls() {
    let s = Script::new(vec![Scripted::Reply {
        status: 200,
        headers: vec![],
        chunks: vec![Ok(
            "data: {\"choices\":[{\"delta\":{\"content\":\"<tool_call><function=read_file><parameter=path>a.rs</parameter></function></tool_call>\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )],
    }]);
    let mut cfg = ProviderConfig::new(
        "http://127.0.0.1:8080/v1",
        "m",
        Role::Local { allowlist: vec![] },
    );
    cfg.backoff_scale = 0.0;
    let p = ChatProvider::new(cfg, Box::new(s)).unwrap();
    let mut req = request();
    req.tools = vec![ToolSpec {
        name: "read_file".into(),
        description: String::new(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
    }];
    let r = p.create(&req).await.unwrap();
    assert_eq!(r.tool_calls[0].arguments["path"], "a.rs");
    assert_eq!(r.stop, StopReason::ToolCalls);
}

// ---- live smoke tests: `DUET_LIVE_GLM=1 DUET_LIVE_LOCAL=1 cargo test -p duet-provider -- --ignored live_`

fn live_tool() -> ToolSpec {
    ToolSpec {
        name: "record_answer".into(),
        description: "Record the final answer.".into(),
        parameters: json!({"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"]}),
    }
}

#[tokio::test]
#[ignore]
async fn live_glm_tool_round_trip_and_cache() {
    if std::env::var("DUET_LIVE_GLM").is_err() {
        return;
    }
    let mut cfg = ProviderConfig::new(
        "https://api.z.ai/api/coding/paas/v4",
        "glm-5.3",
        Role::Frontier,
    );
    cfg.api_key_env = Some("ZAI_API_KEY".into());
    let p = ChatProvider::with_reqwest(cfg).unwrap();
    let filler = "Reference line about ledgers and reconciliation.\n".repeat(600);
    let mut req = Request {
        system: format!("You are a precise assistant.\n{filler}"),
        items: vec![Item::User {
            text: "What is 17 + 25? Call record_answer with the result.".into(),
        }],
        tools: vec![live_tool()],
        ..Request::default()
    };
    let first = p.create(&req).await.unwrap();
    assert_eq!(
        first.tool_calls.first().map(|c| c.name.as_str()),
        Some("record_answer"),
        "{first:?}"
    );
    assert_eq!(first.tool_calls[0].arguments["answer"], 42);
    req.items.push(first.to_item());
    req.items.push(Item::ToolResult {
        call_id: first.tool_calls[0].id.clone(),
        content: "recorded".into(),
    });
    let second = p.create(&req).await.unwrap();
    assert!(
        second.usage.cache_read > 0,
        "expected prefix cache hit: {:?}",
        second.usage
    );
}

#[tokio::test]
#[ignore]
async fn live_local_tool_round_trip_and_prefix_reuse() {
    let Ok(url) = std::env::var("DUET_LIVE_LOCAL") else {
        return;
    };
    let url = if url == "1" {
        "http://192.168.50.132:8080/v1".to_owned()
    } else {
        url
    };
    let host = crate::endpoint::host_port(&url)
        .map(|(h, p)| format!("{h}:{p}"))
        .unwrap();
    let mut cfg = ProviderConfig::new(
        &url,
        "omlx-coding",
        Role::Local {
            allowlist: vec![host],
        },
    );
    cfg.api_key_env = Some("OMLX_API_KEY".into());
    let p = ChatProvider::with_reqwest(cfg).unwrap();
    let filler = "Reference line about ledgers and reconciliation.\n".repeat(400);
    let req = Request {
        system: format!("You are a precise assistant. Answer with the tool.\n{filler}"),
        items: vec![Item::User {
            text: "What is 17 + 25? Call record_answer with the result.".into(),
        }],
        tools: vec![live_tool()],
        max_output_tokens: Some(2048),
        ..Request::default()
    };
    let t0 = std::time::Instant::now();
    let first = p.create(&req).await.unwrap();
    let cold = t0.elapsed();
    assert_eq!(
        first.tool_calls.first().map(|c| c.name.as_str()),
        Some("record_answer"),
        "{first:?}"
    );
    let t1 = std::time::Instant::now();
    p.create(&req).await.unwrap();
    let warm = t1.elapsed();
    eprintln!(
        "local cold {cold:?}, warm {warm:?}, usage {:?}",
        first.usage
    );
    assert!(
        warm < cold,
        "expected prefix reuse to make the repeat faster"
    );
}
