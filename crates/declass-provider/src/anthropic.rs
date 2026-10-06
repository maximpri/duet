// SPDX-License-Identifier: GPL-3.0-or-later
//! Anthropic Messages dialect (`POST /v1/messages`), from the public API
//! reference: request building with prompt-cache breakpoints, and assembly of
//! the streamed content blocks.

use crate::dialect::{sorted_tools, split_extra};
use crate::error::{ErrorKind, ProviderError};
use crate::image::anthropic_block;
use crate::live::Part as Live;
use crate::types::{
    Item, Part, Piece, Replay, Request, Response, StopReason, ToolCall, Usage, UsageStatus,
    assistant_pieces, attached_to_previous, estimate_tokens, images_after,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const DIALECT: &str = "anthropic";
/// The `anthropic-version` header value.
pub const API_VERSION: &str = "2023-06-01";
/// `max_tokens` is required by this API; used when the request sets none.
pub const DEFAULT_MAX_TOKENS: u32 = 32_000;

fn breakpoint() -> Value {
    json!({"type": "ephemeral"})
}

fn is_thinking(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("thinking" | "redacted_thinking")
    )
}

/// Builds the request body.
///
/// Caching: the stable prefix (tools, then system) ends in a breakpoint on the
/// system block (on the last tool when there is no system prompt), and a
/// rolling breakpoint sits on the last block of the conversation, so each
/// request reads everything the previous one wrote. Tools are sorted by name
/// and earlier turns are rebuilt identically, so the prefix is byte-stable.
pub fn build_body(model: &str, req: &Request, stream: bool) -> Value {
    let (effort, extra) = split_extra(&req.extra);
    let mut body = json!({
        "model": model,
        "max_tokens": req.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "stream": stream,
    });
    let mut tools: Vec<Value> = sorted_tools(&req.tools)
        .into_iter()
        .map(
            |t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}),
        )
        .collect();
    if !req.system.is_empty() {
        body["system"] =
            json!([{"type": "text", "text": req.system, "cache_control": breakpoint()}]);
    } else if let Some(last) = tools.last_mut() {
        last["cache_control"] = breakpoint();
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    let mut messages = messages(req);
    if let Some(block) = messages
        .last_mut()
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|blocks| blocks.iter_mut().rev().find(|b| !is_thinking(b)))
    {
        block["cache_control"] = breakpoint();
    }
    body["messages"] = Value::Array(messages);
    if let Some(effort) = effort {
        body["thinking"] = json!({"type": "adaptive"});
        body["output_config"]["effort"] = effort;
    }
    if let Some(schema) = &req.response_schema {
        body["output_config"]["format"] = json!({"type": "json_schema", "schema": schema});
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    for (k, v) in extra {
        body[k] = v;
    }
    body
}

/// Tool-call arguments as the object the model produced (from the raw text
/// the outbound filters edit), falling back to the parsed map.
fn input_of(call: &ToolCall) -> Value {
    match serde_json::from_str::<Value>(&call.raw_arguments) {
        Ok(v @ Value::Object(_)) => v,
        _ => Value::Object(call.arguments.clone()),
    }
}

/// The conversation as alternating user/assistant messages: consecutive items
/// of one role share a message, and tool results lead their user message.
fn messages(req: &Request) -> Vec<Value> {
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    let mut push = |role: &'static str, block: Value| {
        let is_result = block["type"] == "tool_result";
        match out.last_mut() {
            Some((r, blocks)) if *r == role => {
                let at = if is_result {
                    blocks
                        .iter()
                        .position(|b| b["type"] != "tool_result")
                        .unwrap_or(blocks.len())
                } else {
                    blocks.len()
                };
                blocks.insert(at, block);
            }
            _ => out.push((role, vec![block])),
        }
    };
    for (i, item) in req.items.iter().enumerate() {
        match item {
            Item::User { text } => {
                // Images before the text, as the API documentation advises.
                for img in images_after(&req.items, i) {
                    push("user", anthropic_block(img));
                }
                if !text.is_empty() {
                    push("user", json!({"type": "text", "text": text}));
                }
            }
            Item::ToolResult { call_id, content } => {
                let mut block = json!({"type": "tool_result", "tool_use_id": call_id});
                let images = images_after(&req.items, i);
                if images.is_empty() {
                    if !content.is_empty() {
                        block["content"] = Value::String(content.clone());
                    }
                } else {
                    let mut parts = Vec::new();
                    if !content.is_empty() {
                        parts.push(json!({"type": "text", "text": content}));
                    }
                    parts.extend(images.into_iter().map(anthropic_block));
                    block["content"] = Value::Array(parts);
                }
                push("user", block);
            }
            Item::Images { .. } if attached_to_previous(&req.items, i) => {}
            Item::Images { images, .. } => {
                for img in images.iter().filter(|img| img.is_loaded()) {
                    push("user", anthropic_block(img));
                }
            }
            Item::Assistant {
                text,
                tool_calls,
                replay,
                ..
            } => {
                for piece in assistant_pieces(DIALECT, text, tool_calls, replay.as_ref()) {
                    push(
                        "assistant",
                        match piece {
                            Piece::Opaque(block) => block.clone(),
                            Piece::Text(t) => json!({"type": "text", "text": t}),
                            Piece::Call(c) => json!({"type": "tool_use", "id": c.id,
                                "name": c.name, "input": input_of(c)}),
                        },
                    );
                }
            }
        }
    }
    out.into_iter()
        .map(|(role, content)| json!({"role": role, "content": content}))
        .collect()
}

enum Block {
    Text(String),
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        json: String,
        start: Value,
    },
    /// Redacted thinking and any other block, replayed as received.
    Other(Value),
}

/// Accumulates streamed events into one response.
#[derive(Default)]
pub struct AnthropicAssembler {
    id: Option<String>,
    model: Option<String>,
    blocks: BTreeMap<usize, Block>,
    stop: Option<String>,
    usage: Option<Usage>,
    output_bytes: usize,
}

fn count(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

impl AnthropicAssembler {
    pub fn output_bytes(&self) -> usize {
        self.output_bytes
    }

    /// What the stream holds so far (see [`crate::live`]).
    pub fn view(&self) -> Vec<(usize, Live<'_>)> {
        self.blocks
            .iter()
            .filter_map(|(i, b)| {
                let part = match b {
                    Block::Text(t) => Live::Text(t),
                    Block::Thinking { thinking, .. } => Live::Reasoning(thinking.len()),
                    Block::ToolUse { name, json, .. } => Live::Call {
                        name,
                        arguments: json,
                    },
                    Block::Other(_) => return None,
                };
                Some((*i, part))
            })
            .collect()
    }

    fn merge_usage(&mut self, u: &Value) {
        let usage = self.usage.get_or_insert_with(Usage::default);
        if let Some(n) = count(u, "input_tokens") {
            usage.input = n;
        }
        if let Some(n) = count(u, "cache_read_input_tokens") {
            usage.cache_read = n;
        }
        if let Some(n) = count(u, "cache_creation_input_tokens") {
            usage.cache_write = n;
        }
        if let Some(n) = count(u, "output_tokens") {
            usage.output = n;
        }
    }

    /// Applies one `data:` payload.
    pub fn apply(&mut self, data: &str) -> Result<(), ProviderError> {
        let event: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::new(ErrorKind::Malformed, format!("bad stream event: {e}"))
        })?;
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "error" => {
                // Overloaded or failed mid-stream: a fresh attempt can succeed.
                let detail = event.get("error").unwrap_or(&event);
                let mut e = ProviderError::new(ErrorKind::StreamError, detail.to_string());
                e.output_started = self.output_bytes > 0;
                e.partial_output_bytes = self.output_bytes;
                return Err(e);
            }
            "message_start" => {
                let m = &event["message"];
                if let Some(id) = m.get("id").and_then(Value::as_str) {
                    self.id = Some(id.to_owned());
                }
                if let Some(model) = m.get("model").and_then(Value::as_str) {
                    self.model = Some(model.to_owned());
                }
                if let Some(u) = m.get("usage").filter(|u| u.is_object()) {
                    self.merge_usage(u);
                }
            }
            "content_block_start" => {
                let index = count(&event, "index").unwrap_or(0) as usize;
                let b = &event["content_block"];
                let text = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                let block = match b.get("type").and_then(Value::as_str) {
                    Some("text") => Block::Text(text("text")),
                    Some("thinking") => Block::Thinking {
                        thinking: text("thinking"),
                        signature: text("signature"),
                    },
                    Some("tool_use") => Block::ToolUse {
                        id: text("id"),
                        name: text("name"),
                        json: String::new(),
                        start: b.get("input").cloned().unwrap_or(Value::Null),
                    },
                    _ => Block::Other(b.clone()),
                };
                self.blocks.insert(index, block);
            }
            "content_block_delta" => {
                let index = count(&event, "index").unwrap_or(0) as usize;
                let d = &event["delta"];
                let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("");
                let block = self
                    .blocks
                    .entry(index)
                    .or_insert_with(|| Block::Text(String::new()));
                match (d.get("type").and_then(Value::as_str), block) {
                    (Some("text_delta"), Block::Text(t)) => {
                        t.push_str(s("text"));
                        self.output_bytes += s("text").len();
                    }
                    (Some("thinking_delta"), Block::Thinking { thinking, .. }) => {
                        thinking.push_str(s("thinking"));
                        self.output_bytes += s("thinking").len();
                    }
                    (Some("signature_delta"), Block::Thinking { signature, .. }) => {
                        signature.push_str(s("signature"));
                    }
                    (Some("input_json_delta"), Block::ToolUse { json, .. }) => {
                        json.push_str(s("partial_json"));
                        self.output_bytes += s("partial_json").len();
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = event["delta"].get("stop_reason").and_then(Value::as_str) {
                    self.stop = Some(r.to_owned());
                }
                if let Some(u) = event.get("usage").filter(|u| u.is_object()) {
                    self.merge_usage(u);
                }
            }
            // ping, content_block_stop, message_stop and future event types.
            _ => {}
        }
        Ok(())
    }

    /// Completes the response. A stream without a stop reason was cut off.
    pub fn finish(self, requested_model: &str) -> Result<Response, ProviderError> {
        let Some(stop) = self.stop else {
            let mut e =
                ProviderError::new(ErrorKind::Truncated, "stream ended without a stop reason");
            e.output_started = self.output_bytes > 0;
            e.partial_output_bytes = self.output_bytes;
            return Err(e);
        };
        let (mut text, mut reasoning) = (String::new(), String::new());
        let (mut tool_calls, mut parts) = (Vec::new(), Vec::new());
        let mut opaque = false;
        for (_, block) in self.blocks {
            match block {
                Block::Text(t) => {
                    parts.push(Part::Text { bytes: t.len() });
                    text.push_str(&t);
                }
                Block::Thinking {
                    thinking,
                    signature,
                } => {
                    reasoning.push_str(&thinking);
                    opaque = true;
                    parts.push(Part::Opaque {
                        block: json!({"type": "thinking", "thinking": thinking, "signature": signature}),
                    });
                }
                Block::ToolUse {
                    id,
                    name,
                    json,
                    start,
                } => {
                    let raw = if !json.trim().is_empty() {
                        json
                    } else if start.is_object() {
                        start.to_string()
                    } else {
                        "{}".to_owned()
                    };
                    let arguments = match serde_json::from_str::<Value>(&raw) {
                        Ok(Value::Object(m)) => m,
                        _ => Map::new(),
                    };
                    parts.push(Part::Call { id: id.clone() });
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                        raw_arguments: raw,
                    });
                }
                Block::Other(block) => {
                    opaque = true;
                    parts.push(Part::Opaque { block });
                }
            }
        }
        let stop = match stop.as_str() {
            // The context window ended the output: as terminal as max_tokens.
            "model_context_window_exceeded" => StopReason::Length,
            other => StopReason::from_wire(other),
        };
        let usage = match self.usage {
            Some(u) => Usage {
                status: UsageStatus::Reported,
                ..u
            },
            None => Usage {
                output: estimate_tokens(self.output_bytes),
                status: UsageStatus::Estimated,
                ..Usage::default()
            },
        };
        Ok(Response {
            id: self.id,
            model: self.model.unwrap_or_else(|| requested_model.to_owned()),
            text,
            reasoning: (!reasoning.is_empty()).then_some(reasoning),
            tool_calls,
            stop,
            usage,
            attempts: Default::default(),
            replay: opaque.then(|| Replay {
                dialect: DIALECT.to_owned(),
                parts,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;

    fn tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: name.into(),
            parameters: json!({"type": "object"}),
        }
    }

    fn call(id: &str, raw: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: Map::new(),
            raw_arguments: raw.into(),
        }
    }

    fn req() -> Request {
        Request {
            system: "sys".into(),
            items: vec![
                Item::User { text: "hi".into() },
                Item::Assistant {
                    text: "reading".into(),
                    reasoning: None,
                    tool_calls: vec![call("t1", r#"{"path": "a"}"#), call("t2", "{}")],
                    replay: None,
                },
                Item::ToolResult {
                    call_id: "t1".into(),
                    content: "data".into(),
                },
                Item::ToolResult {
                    call_id: "t2".into(),
                    content: String::new(),
                },
            ],
            tools: vec![tool("zeta"), tool("alpha")],
            max_output_tokens: Some(4096),
            ..Request::default()
        }
    }

    /// The body without cache markers: what the cache key is computed over.
    fn unmarked(mut v: Value) -> Value {
        fn strip(v: &mut Value) {
            match v {
                Value::Object(m) => {
                    m.remove("cache_control");
                    m.values_mut().for_each(strip);
                }
                Value::Array(a) => a.iter_mut().for_each(strip),
                _ => {}
            }
        }
        strip(&mut v);
        v
    }

    #[test]
    fn body_has_sorted_tools_blocks_and_breakpoints() {
        let b = build_body("claude-opus-5-5", &req(), true);
        assert_eq!(b["max_tokens"], 4096);
        assert_eq!(b["tools"][0]["name"], "alpha");
        assert_eq!(b["tools"][1]["input_schema"]["type"], "object");
        assert_eq!(b["system"][0]["text"], "sys");
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(b["tools"][1].get("cache_control").is_none());
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[1]["role"], "assistant");
        assert_eq!(
            m[1]["content"][0],
            json!({"type": "text", "text": "reading"})
        );
        assert_eq!(m[1]["content"][1]["type"], "tool_use");
        assert_eq!(m[1]["content"][1]["input"], json!({"path": "a"}));
        // Both results share one user message; the empty one has no content.
        assert_eq!(m[2]["content"][0]["tool_use_id"], "t1");
        assert!(m[2]["content"][1].get("content").is_none());
        // The rolling breakpoint is on the last block only.
        assert_eq!(m[2]["content"][1]["cache_control"]["type"], "ephemeral");
        assert!(m[2]["content"][0].get("cache_control").is_none());
        assert!(b.get("thinking").is_none() && b.get("output_config").is_none());
    }

    #[test]
    fn without_a_system_prompt_the_last_tool_is_the_breakpoint() {
        let mut r = req();
        r.system.clear();
        let b = build_body("m", &r, true);
        assert!(b.get("system").is_none());
        assert_eq!(b["tools"][1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn the_prefix_is_stable_as_the_conversation_grows() {
        let short = unmarked(build_body("m", &req(), true));
        let mut longer = req();
        longer.items.push(Item::Assistant {
            text: "done".into(),
            reasoning: None,
            tool_calls: Vec::new(),
            replay: None,
        });
        longer.items.push(Item::User {
            text: "next".into(),
        });
        let long = unmarked(build_body("m", &longer, true));
        assert_eq!(short["system"], long["system"]);
        assert_eq!(short["tools"], long["tools"]);
        let (s, l) = (
            short["messages"].as_array().unwrap(),
            long["messages"].as_array().unwrap(),
        );
        assert_eq!(&l[..s.len()], &s[..]);
        // The breakpoint moved to the new last block.
        let marked = build_body("m", &longer, true);
        assert_eq!(
            marked["messages"][4]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn effort_maps_to_adaptive_thinking_and_output_config() {
        let mut r = req();
        r.extra.insert("reasoning_effort".into(), json!("high"));
        r.extra.insert("metadata".into(), json!({"user_id": "u"}));
        r.response_schema = Some(json!({"type": "object"}));
        let b = build_body("m", &r, true);
        assert_eq!(b["thinking"], json!({"type": "adaptive"}));
        assert_eq!(b["output_config"]["effort"], "high");
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
        assert!(b.get("reasoning_effort").is_none());
        assert_eq!(b["metadata"]["user_id"], "u");
    }

    const STREAM: &[&str] = &[
        r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-5-5","content":[],"stop_reason":null,"usage":{"input_tokens":12,"cache_creation_input_tokens":300,"cache_read_input_tokens":2000,"output_tokens":1}}}"#,
        r#"{"type":"ping"}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"c2ln"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading "}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"it."}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}"#,
        r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
        r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"th\": \"x\"}"}}"#,
        r#"{"type":"content_block_stop","index":2}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":40}}"#,
        r#"{"type":"message_stop"}"#,
    ];

    fn assembled(events: &[&str]) -> Response {
        let mut a = AnthropicAssembler::default();
        for e in events {
            a.apply(e).unwrap();
        }
        a.finish("requested").unwrap()
    }

    #[test]
    fn assembles_thinking_text_tool_use_and_cache_usage() {
        let r = assembled(STREAM);
        assert_eq!(r.id.as_deref(), Some("msg_1"));
        assert_eq!(r.model, "claude-opus-5-5");
        assert_eq!(r.text, "Reading it.");
        assert_eq!(r.reasoning.as_deref(), Some("plan"));
        assert_eq!(r.stop, StopReason::ToolCalls);
        assert_eq!(r.tool_calls[0].id, "toolu_1");
        assert_eq!(r.tool_calls[0].arguments["path"], "x");
        assert_eq!(r.tool_calls[0].raw_arguments, r#"{"path": "x"}"#);
        assert_eq!(
            (
                r.usage.input,
                r.usage.cache_read,
                r.usage.cache_write,
                r.usage.output
            ),
            (12, 2000, 300, 40)
        );
        assert_eq!(r.usage.status, UsageStatus::Reported);
        let replay = r.replay.clone().unwrap();
        assert_eq!(replay.dialect, "anthropic");
        assert_eq!(replay.parts.len(), 3);
    }

    #[test]
    fn a_signed_thinking_block_is_replayed_unchanged_in_place() {
        let r = assembled(STREAM);
        let mut next = req();
        next.items.push(r.to_item());
        next.items.push(Item::ToolResult {
            call_id: "toolu_1".into(),
            content: "file".into(),
        });
        let b = build_body("m", &next, true);
        let turn = &b["messages"][3]["content"];
        assert_eq!(
            turn[0],
            json!({"type": "thinking", "thinking": "plan", "signature": "c2ln"})
        );
        assert_eq!(turn[1], json!({"type": "text", "text": "Reading it."}));
        assert_eq!(turn[2]["id"], "toolu_1");
        assert_eq!(turn[2]["input"], json!({"path": "x"}));
        // Another dialect never sees the Anthropic-only blocks.
        let chat = crate::chat::build_body("m", &next, true);
        assert!(!chat.to_string().contains("c2ln"));
    }

    #[test]
    fn max_tokens_and_window_stops_are_terminal() {
        for reason in ["max_tokens", "model_context_window_exceeded"] {
            let delta = format!(
                r#"{{"type":"message_delta","delta":{{"stop_reason":"{reason}"}},"usage":{{"output_tokens":5}}}}"#
            );
            let r = assembled(&[&delta]);
            assert_eq!(r.stop, StopReason::Length, "{reason}");
            assert!(!r.stop.allows_tools());
        }
        let refusal = assembled(&[r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#]);
        assert_eq!(refusal.stop, StopReason::ContentFilter);
    }

    #[test]
    fn a_missing_stop_is_truncation_and_a_stream_error_retries() {
        let mut a = AnthropicAssembler::default();
        for e in &STREAM[..8] {
            a.apply(e).unwrap();
        }
        let e = a.finish("m").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Truncated);
        assert!(e.output_started && e.is_retryable());

        let mut b = AnthropicAssembler::default();
        for e in &STREAM[..8] {
            b.apply(e).unwrap();
        }
        let e = b
            .apply(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::StreamError);
        assert!(e.is_retryable() && e.output_started);
        assert!(e.message.contains("overloaded_error"));
    }

    #[test]
    fn missing_usage_is_estimated() {
        let r = assembled(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"hello"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ]);
        assert_eq!(r.text, "hello");
        assert_eq!(r.usage.status, UsageStatus::Estimated);
        assert!(r.replay.is_none(), "no opaque blocks, nothing to replay");
    }
}
