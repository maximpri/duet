// SPDX-License-Identifier: GPL-3.0-or-later
//! Test support: a local model that replies from a script, so the engine and
//! the agent can be tested without a model server. Built only for tests and
//! with the `test-support` feature.

use crate::local::LocalReader;
use duet_provider::client::{HttpReply, Transport};
use duet_provider::{ChatProvider, ProviderConfig, ProviderError, Role};
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// What a scripted local model received.
#[derive(Clone, Default)]
pub struct Received(Arc<Mutex<Vec<Value>>>);

impl Received {
    /// Request bodies, in order.
    pub fn bodies(&self) -> Vec<Value> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The user prompt of request `i`.
    pub fn prompt(&self, i: usize) -> String {
        self.bodies()
            .get(i)
            .and_then(|b| b["messages"].as_array()?.last()?["content"].as_str())
            .unwrap_or_default()
            .to_owned()
    }
}

struct Script {
    replies: Arc<Mutex<VecDeque<String>>>,
    received: Received,
}

impl Transport for Script {
    fn post(
        &self,
        _url: String,
        _headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> BoxFuture<'static, Result<HttpReply, ProviderError>> {
        if let Ok(v) = serde_json::from_slice(&body) {
            self.received
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(v);
        }
        let reply = self
            .replies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| {
                "{\"answer\": \"no scripted reply\", \"unanswerable\": true}".into()
            });
        let chunks = vec![
            json!({"choices": [{"delta": {"content": reply}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 10}}),
        ];
        let mut sse: Vec<Result<bytes::Bytes, String>> = chunks
            .into_iter()
            .map(|c| Ok(bytes::Bytes::from(format!("data: {c}\n\n"))))
            .collect();
        sse.push(Ok(bytes::Bytes::from_static(b"data: [DONE]\n\n")));
        Box::pin(async move {
            Ok(HttpReply {
                status: 200,
                headers: vec![],
                body: futures_util::stream::iter(sse).boxed(),
            })
        })
    }
}

/// A local reader whose model answers with `replies` in order (each the model's
/// text, normally a JSON object).
pub fn scripted_local(replies: Vec<String>) -> (LocalReader, Received) {
    let received = Received::default();
    let script = Script {
        replies: Arc::new(Mutex::new(replies.into())),
        received: received.clone(),
    };
    let mut cfg = ProviderConfig::new(
        "http://127.0.0.1:9/v1",
        "scripted",
        Role::Local {
            allowlist: Vec::new(),
            allow_plaintext: false,
        },
    );
    cfg.backoff_scale = 0.0;
    let provider = ChatProvider::new(cfg, Box::new(script)).expect("loopback endpoint");
    (LocalReader::new(provider), received)
}
