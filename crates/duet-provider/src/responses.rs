// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenAI Responses dialect (`POST /v1/responses`), from the public API
//! reference. Requests are stateless (`store: false`): the whole conversation
//! is sent each turn, and reasoning is carried between turns as the encrypted
//! reasoning items the provider returned.

use crate::dialect::{sorted_tools, split_extra};
use crate::error::{ErrorKind, ProviderError};
use crate::image::responses_part;
use crate::types::{
    Item, Part, Piece, Replay, Request, Response, StopReason, ToolCall, Usage, UsageStatus,
    assistant_pieces, attached_to_previous, estimate_tokens, images_after,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const DIALECT: &str = "responses";

/// Builds the request body. Tools are sorted by name and earlier turns are
/// rebuilt identically, so the prefix is byte-stable; `prompt_cache_key` is
/// derived from the stable prefix (model, instructions, tools) so requests of
/// one run are routed to the same cache.
pub fn build_body(model: &str, req: &Request, stream: bool) -> Value {
    let (effort, extra) = split_extra(&req.extra);
    let tools: Vec<Value> = sorted_tools(&req.tools)
        .into_iter()
        .map(|t| {
            json!({"type": "function", "name": t.name, "description": t.description,
                   "parameters": t.parameters, "strict": false})
        })
        .collect();
    let mut body = json!({
        "model": model,
        "input": input(req),
        "stream": stream,
        "store": false,
    });
    if !req.system.is_empty() {
        body["instructions"] = Value::String(req.system.clone());
    }
    let mut key = Sha256::new();
    key.update(model.as_bytes());
    key.update([0]);
    key.update(req.system.as_bytes());
    key.update([0]);
    key.update(Value::Array(tools.clone()).to_string().as_bytes());
    body["prompt_cache_key"] =
        Value::String(format!("duet-{}", &hex::encode(key.finalize())[..32]));
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(max) = req.max_output_tokens {
        body["max_output_tokens"] = json!(max);
    }
    if let Some(effort) = effort {
        body["reasoning"] = json!({"effort": effort});
        // Stateless requests carry reasoning forward only in encrypted form.
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(schema) = &req.response_schema {
        body["text"] = json!({"format": {"type": "json_schema", "name": "response",
                                         "strict": true, "schema": schema}});
    }
    for (k, v) in extra {
        body[k] = v;
    }
    body
}

fn input(req: &Request) -> Vec<Value> {
    let mut out = Vec::with_capacity(req.items.len());
    for (i, item) in req.items.iter().enumerate() {
        match item {
            Item::User { text } => {
                let images = images_after(&req.items, i);
                if images.is_empty() {
                    out.push(json!({"role": "user", "content": text}));
                } else {
                    let mut parts: Vec<Value> = images.into_iter().map(responses_part).collect();
                    if !text.is_empty() {
                        parts.push(json!({"type": "input_text", "text": text}));
                    }
                    out.push(json!({"role": "user", "content": parts}));
                }
            }
            Item::ToolResult { call_id, content } => {
                let images = images_after(&req.items, i);
                let output = if images.is_empty() {
                    Value::String(content.clone())
                } else {
                    let mut parts = Vec::new();
                    if !content.is_empty() {
                        parts.push(json!({"type": "input_text", "text": content}));
                    }
                    parts.extend(images.into_iter().map(responses_part));
                    Value::Array(parts)
                };
                out.push(
                    json!({"type": "function_call_output", "call_id": call_id, "output": output}),
                );
            }
            Item::Images { .. } if attached_to_previous(&req.items, i) => {}
            Item::Images { images, .. } => {
                let parts: Vec<Value> = images
                    .iter()
                    .filter(|img| img.is_loaded())
                    .map(responses_part)
                    .collect();
                if !parts.is_empty() {
                    out.push(json!({"role": "user", "content": parts}));
                }
            }
            Item::Assistant {
                text,
                tool_calls,
                replay,
                ..
            } => {
                for piece in assistant_pieces(DIALECT, text, tool_calls, replay.as_ref()) {
                    out.push(match piece {
                        Piece::Opaque(item) => item.clone(),
                        Piece::Text(t) => json!({"role": "assistant", "content": t}),
                        Piece::Call(c) => json!({"type": "function_call", "call_id": c.id,
                            "name": c.name, "arguments": c.raw_arguments}),
                    });
                }
            }
        }
    }
    out
}

#[derive(Default)]
struct OutputItem {
    /// The item as announced or, once done, as completed.
    item: Value,
    done: bool,
    text: String,
    arguments: String,
    reasoning: String,
}

/// Accumulates streamed events into one response.
#[derive(Default)]
pub struct ResponsesAssembler {
    id: Option<String>,
    model: Option<String>,
    items: BTreeMap<u64, OutputItem>,
    /// The final `response` object (`response.completed` or `.incomplete`).
    terminal: Option<Value>,
    output_bytes: usize,
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

impl ResponsesAssembler {
    pub fn output_bytes(&self) -> usize {
        self.output_bytes
    }

    fn stream_error(&self, detail: &Value) -> ProviderError {
        let mut e = ProviderError::new(ErrorKind::StreamError, detail.to_string());
        e.output_started = self.output_bytes > 0;
        e.partial_output_bytes = self.output_bytes;
        e
    }

    /// Applies one `data:` payload.
    pub fn apply(&mut self, data: &str) -> Result<(), ProviderError> {
        let event: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::new(ErrorKind::Malformed, format!("bad stream event: {e}"))
        })?;
        let index = event.get("output_index").and_then(Value::as_u64);
        let delta = text_of(&event, "delta");
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "error" => return Err(self.stream_error(&event)),
            "response.failed" => {
                let r = &event["response"];
                return Err(self.stream_error(r.get("error").unwrap_or(r)));
            }
            "response.created" | "response.in_progress" => {
                let r = &event["response"];
                if let Some(id) = r.get("id").and_then(Value::as_str) {
                    self.id = Some(id.to_owned());
                }
                if let Some(m) = r.get("model").and_then(Value::as_str) {
                    self.model = Some(m.to_owned());
                }
            }
            "response.output_item.added" => {
                let slot = self.items.entry(index.unwrap_or(0)).or_default();
                if !slot.done {
                    slot.item = event["item"].clone();
                }
            }
            "response.output_item.done" => {
                let slot = self.items.entry(index.unwrap_or(0)).or_default();
                slot.item = event["item"].clone();
                slot.done = true;
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                self.output_bytes += delta.len();
                self.items
                    .entry(index.unwrap_or(0))
                    .or_default()
                    .text
                    .push_str(&delta);
            }
            "response.function_call_arguments.delta" => {
                self.output_bytes += delta.len();
                self.items
                    .entry(index.unwrap_or(0))
                    .or_default()
                    .arguments
                    .push_str(&delta);
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.output_bytes += delta.len();
                self.items
                    .entry(index.unwrap_or(0))
                    .or_default()
                    .reasoning
                    .push_str(&delta);
            }
            "response.completed" | "response.incomplete" => {
                self.terminal = Some(event["response"].clone());
            }
            _ => {}
        }
        Ok(())
    }

    /// Completes the response. A stream without a terminal event was cut off.
    pub fn finish(mut self, requested_model: &str) -> Result<Response, ProviderError> {
        let Some(terminal) = self.terminal.take() else {
            let mut e = ProviderError::new(
                ErrorKind::Truncated,
                "stream ended without response.completed",
            );
            e.output_started = self.output_bytes > 0;
            e.partial_output_bytes = self.output_bytes;
            return Err(e);
        };
        // Items missing from the stream (a server that sends only the final
        // object) come from the terminal response's output.
        if let Some(output) = terminal.get("output").and_then(Value::as_array) {
            for (i, item) in output.iter().enumerate() {
                let slot = self.items.entry(i as u64).or_default();
                if !slot.done {
                    slot.item = item.clone();
                    slot.done = true;
                }
            }
        }
        let (mut text, mut reasoning, mut refused) = (String::new(), String::new(), false);
        let (mut tool_calls, mut parts) = (Vec::new(), Vec::new());
        let mut opaque = false;
        for (_, slot) in self.items {
            let item = &slot.item;
            match item.get("type").and_then(Value::as_str).unwrap_or("") {
                "message" => {
                    let mut t = String::new();
                    for c in item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        match c.get("type").and_then(Value::as_str) {
                            Some("output_text") => t.push_str(&text_of(c, "text")),
                            Some("refusal") => refused = true,
                            _ => {}
                        }
                    }
                    if t.is_empty() && !slot.done {
                        t = slot.text;
                    }
                    parts.push(Part::Text { bytes: t.len() });
                    text.push_str(&t);
                }
                "function_call" => {
                    let mut raw = text_of(item, "arguments");
                    if raw.is_empty() {
                        raw = slot.arguments;
                    }
                    if raw.trim().is_empty() {
                        raw = "{}".to_owned();
                    }
                    let arguments = match serde_json::from_str::<Value>(&raw) {
                        Ok(Value::Object(m)) => m,
                        _ => Map::new(),
                    };
                    let id = text_of(item, "call_id");
                    parts.push(Part::Call { id: id.clone() });
                    tool_calls.push(ToolCall {
                        id,
                        name: text_of(item, "name"),
                        arguments,
                        raw_arguments: raw,
                    });
                }
                "reasoning" => {
                    let summary: String = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|s| text_of(s, "text"))
                        .collect();
                    reasoning.push_str(if summary.is_empty() {
                        &slot.reasoning
                    } else {
                        &summary
                    });
                    // Only an encrypted item can be sent back to a stateless
                    // endpoint; a bare id refers to nothing stored.
                    if item.get("encrypted_content").is_some_and(Value::is_string) {
                        opaque = true;
                        parts.push(Part::Opaque {
                            block: item.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        let status = text_of(&terminal, "status");
        let reason = terminal
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let stop = match status.as_str() {
            "incomplete" => StopReason::from_wire(if reason.is_empty() {
                "incomplete"
            } else {
                reason
            }),
            _ if refused && text.is_empty() && tool_calls.is_empty() => StopReason::ContentFilter,
            _ if !tool_calls.is_empty() => StopReason::ToolCalls,
            "completed" | "" => StopReason::Stop,
            other => StopReason::Other(other.to_owned()),
        };
        let usage = match terminal.get("usage").filter(|u| u.is_object()) {
            Some(u) => usage_from_responses(u),
            None => Usage {
                output: estimate_tokens(self.output_bytes),
                status: UsageStatus::Estimated,
                ..Usage::default()
            },
        };
        Ok(Response {
            id: self.id.or_else(|| {
                terminal
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }),
            model: self
                .model
                .or_else(|| {
                    terminal
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| requested_model.to_owned()),
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

/// Normalizes a Responses `usage` object.
pub fn usage_from_responses(u: &Value) -> Usage {
    let get = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
    let input = get(u, "/input_tokens");
    let cached = get(u, "/input_tokens_details/cached_tokens").min(input);
    Usage {
        input: input - cached,
        cache_read: cached,
        cache_write: 0,
        output: get(u, "/output_tokens"),
        reasoning: get(u, "/output_tokens_details/reasoning_tokens"),
        status: UsageStatus::Reported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;

    fn req() -> Request {
        Request {
            system: "sys".into(),
            items: vec![
                Item::User { text: "hi".into() },
                Item::Assistant {
                    text: String::new(),
                    reasoning: None,
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        arguments: Map::new(),
                        raw_arguments: r#"{"path": "a"}"#.into(),
                    }],
                    replay: None,
                },
                Item::ToolResult {
                    call_id: "call_1".into(),
                    content: "data".into(),
                },
            ],
            tools: ["zeta", "alpha"]
                .map(|n| ToolSpec {
                    name: n.into(),
                    description: n.into(),
                    parameters: json!({"type": "object"}),
                })
                .to_vec(),
            max_output_tokens: Some(2048),
            ..Request::default()
        }
    }

    #[test]
    fn body_is_stateless_sorted_and_prefix_stable() {
        let b = build_body("gpt-5.5", &req(), true);
        assert_eq!(b["store"], false);
        assert_eq!(b["instructions"], "sys");
        assert_eq!(b["tools"][0]["name"], "alpha");
        assert_eq!(b["tools"][0]["type"], "function");
        assert_eq!(b["max_output_tokens"], 2048);
        assert_eq!(b["input"][0], json!({"role": "user", "content": "hi"}));
        assert_eq!(b["input"][1]["type"], "function_call");
        assert_eq!(b["input"][1]["arguments"], r#"{"path": "a"}"#);
        assert_eq!(b["input"][2]["type"], "function_call_output");
        assert!(b.get("reasoning").is_none() && b.get("include").is_none());
        let mut longer = req();
        longer.items.push(Item::User {
            text: "more".into(),
        });
        let l = build_body("gpt-5.5", &longer, true);
        assert_eq!(b["prompt_cache_key"], l["prompt_cache_key"]);
        let (s, l) = (
            b["input"].as_array().unwrap(),
            l["input"].as_array().unwrap(),
        );
        assert_eq!(&l[..s.len()], &s[..]);
        let other = build_body(
            "gpt-5.5",
            &Request {
                system: "x".into(),
                ..req()
            },
            true,
        );
        assert_ne!(b["prompt_cache_key"], other["prompt_cache_key"]);
    }

    #[test]
    fn effort_and_schema_map_to_their_fields() {
        let mut r = req();
        r.extra.insert("reasoning_effort".into(), json!("low"));
        r.response_schema = Some(json!({"type": "object"}));
        let b = build_body("m", &r, true);
        assert_eq!(b["reasoning"]["effort"], "low");
        assert_eq!(b["include"][0], "reasoning.encrypted_content");
        assert_eq!(b["text"]["format"]["type"], "json_schema");
        assert!(b.get("reasoning_effort").is_none());
    }

    const STREAM: &[&str] = &[
        r#"{"type":"response.created","response":{"id":"resp_1","model":"gpt-5.5","status":"in_progress"}}"#,
        r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}"#,
        r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"ZW5j"}}"#,
        r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}"#,
        r#"{"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"Reading"}"#,
        r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Reading"}]}}"#,
        r#"{"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"fc_1","call_id":"call_9","name":"read_file","arguments":""}}"#,
        r#"{"type":"response.function_call_arguments.delta","output_index":2,"delta":"{\"path\":"}"#,
        r#"{"type":"response.function_call_arguments.delta","output_index":2,"delta":"\"x\"}"}"#,
        r#"{"type":"response.output_item.done","output_index":2,"item":{"type":"function_call","id":"fc_1","call_id":"call_9","name":"read_file","arguments":"{\"path\":\"x\"}"}}"#,
        r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":1000,"input_tokens_details":{"cached_tokens":800},"output_tokens":50,"output_tokens_details":{"reasoning_tokens":30}}}}"#,
    ];

    fn assembled(events: &[&str]) -> Response {
        let mut a = ResponsesAssembler::default();
        for e in events {
            a.apply(e).unwrap();
        }
        a.finish("requested").unwrap()
    }

    #[test]
    fn assembles_text_function_calls_reasoning_and_cached_usage() {
        let r = assembled(STREAM);
        assert_eq!(r.id.as_deref(), Some("resp_1"));
        assert_eq!(r.model, "gpt-5.5");
        assert_eq!(r.text, "Reading");
        assert_eq!(r.stop, StopReason::ToolCalls);
        assert_eq!(r.tool_calls[0].id, "call_9");
        assert_eq!(r.tool_calls[0].arguments["path"], "x");
        assert_eq!(
            (
                r.usage.input,
                r.usage.cache_read,
                r.usage.output,
                r.usage.reasoning
            ),
            (200, 800, 50, 30)
        );
        // The encrypted reasoning item is replayed first, unchanged.
        let mut next = req();
        next.items.push(r.to_item());
        let b = build_body("gpt-5.5", &next, true);
        let input = b["input"].as_array().unwrap();
        assert_eq!(input[3]["encrypted_content"], "ZW5j");
        assert_eq!(input[4], json!({"role": "assistant", "content": "Reading"}));
        assert_eq!(input[5]["call_id"], "call_9");
    }

    #[test]
    fn incomplete_responses_are_terminal() {
        let r = assembled(&[
            r#"{"type":"response.output_text.delta","output_index":0,"delta":"par"}"#,
            r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        ]);
        assert_eq!(r.stop, StopReason::Length);
        assert_eq!(r.usage.status, UsageStatus::Estimated);
        let f = assembled(&[
            r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"}}}"#,
        ]);
        assert_eq!(f.stop, StopReason::ContentFilter);
    }

    #[test]
    fn failures_and_cut_off_streams_retry() {
        let mut a = ResponsesAssembler::default();
        a.apply(STREAM[4]).unwrap();
        let e = a
            .apply(r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"boom"}}}"#)
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::StreamError);
        assert!(e.is_retryable() && e.output_started);

        let mut b = ResponsesAssembler::default();
        b.apply(STREAM[4]).unwrap();
        let e = b.finish("m").unwrap_err();
        assert_eq!(e.kind, ErrorKind::Truncated);
        assert!(e.is_retryable());
    }

    #[test]
    fn a_final_object_alone_is_enough() {
        let r = assembled(&[
            r#"{"type":"response.completed","response":{"id":"r","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"hi"}]}],"usage":{"input_tokens":5,"output_tokens":1}}}"#,
        ]);
        assert_eq!(r.text, "hi");
        assert_eq!(r.stop, StopReason::Stop);
        assert!(r.replay.is_none());
    }
}
