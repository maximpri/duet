// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenAI-compatible Chat Completions dialect (z.ai, oMLX, LM Studio, llama.cpp, vLLM, Ollama).

use crate::error::{ErrorKind, ProviderError};
use crate::image::chat_part;
use crate::live::Part as Live;
use crate::recover::recover_text_tool_call;
use crate::types::{
    Item, Request, Response, StopReason, ToolCall, Usage, UsageStatus, attached_to_previous,
    estimate_tokens, images_after,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Builds the request body. Tools are sorted by name and every earlier item is
/// serialized exactly as it was first sent, so the prefix is byte-stable.
pub fn build_body(model: &str, req: &Request, stream: bool) -> Value {
    let mut messages = Vec::with_capacity(req.items.len() + 1);
    if !req.system.is_empty() {
        messages.push(json!({"role": "system", "content": req.system}));
    }
    // Tool messages carry text only: images of tool results follow the run
    // of tool messages as one user message.
    let mut pending: Vec<Value> = Vec::new();
    let flush = |pending: &mut Vec<Value>, messages: &mut Vec<Value>| {
        if !pending.is_empty() {
            messages.push(json!({"role": "user", "content": std::mem::take(pending)}));
        }
    };
    for (i, item) in req.items.iter().enumerate() {
        if !matches!(item, Item::ToolResult { .. } | Item::Images { .. }) {
            flush(&mut pending, &mut messages);
        }
        messages.push(match item {
            Item::User { text } => {
                let images = images_after(&req.items, i);
                if images.is_empty() {
                    json!({"role": "user", "content": text})
                } else {
                    let mut parts: Vec<Value> = images.into_iter().map(chat_part).collect();
                    if !text.is_empty() {
                        parts.push(json!({"type": "text", "text": text}));
                    }
                    json!({"role": "user", "content": parts})
                }
            }
            Item::Images { .. } if attached_to_previous(&req.items, i) => continue,
            Item::Images { images, .. } => {
                let parts: Vec<Value> = images
                    .iter()
                    .filter(|img| img.is_loaded())
                    .map(chat_part)
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                json!({"role": "user", "content": parts})
            }
            Item::Assistant {
                text,
                reasoning,
                tool_calls,
                ..
            } => {
                let mut m = json!({"role": "assistant", "content": text});
                if let Some(r) = reasoning {
                    m["reasoning_content"] = Value::String(r.clone());
                }
                if !tool_calls.is_empty() {
                    m["tool_calls"] = tool_calls
                        .iter()
                        .map(|c| {
                            json!({"id": c.id, "type": "function",
                                   "function": {"name": c.name, "arguments": c.raw_arguments}})
                        })
                        .collect();
                }
                m
            }
            Item::ToolResult { call_id, content } => {
                let images = images_after(&req.items, i);
                if !images.is_empty() {
                    pending.push(json!({"type": "text",
                        "text": format!("[image from the result of tool call {call_id}]")}));
                    pending.extend(images.into_iter().map(chat_part));
                }
                json!({"role": "tool", "tool_call_id": call_id, "content": content})
            }
        });
    }
    flush(&mut pending, &mut messages);
    let mut body = json!({"model": model, "messages": messages, "stream": stream});
    if stream {
        body["stream_options"] = json!({"include_usage": true});
    }
    if !req.tools.is_empty() {
        let mut tools = req.tools.clone();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        body["tools"] = tools
            .iter()
            .map(|t| {
                json!({"type": "function",
                       "function": {"name": t.name, "description": t.description, "parameters": t.parameters}})
            })
            .collect();
    }
    if let Some(max) = req.max_output_tokens {
        body["max_tokens"] = json!(max);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(schema) = &req.response_schema {
        body["response_format"] = json!({"type": "json_schema",
            "json_schema": {"name": "response", "strict": true, "schema": schema}});
    }
    for (k, v) in &req.extra {
        body[k] = v.clone();
    }
    body
}

#[derive(Default)]
struct CallParts {
    id: String,
    name: String,
    arguments: String,
}

/// Accumulates streamed chunks into one response.
#[derive(Default)]
pub struct ChatAssembler {
    id: Option<String>,
    model: Option<String>,
    text: String,
    reasoning: String,
    calls: BTreeMap<usize, CallParts>,
    usage: Option<Value>,
    finish: Option<String>,
    output_bytes: usize,
    saw_done: bool,
}

impl ChatAssembler {
    pub fn output_bytes(&self) -> usize {
        self.output_bytes
    }

    /// What the stream holds so far (see [`crate::live`]).
    pub fn view(&self) -> Vec<(usize, Live<'_>)> {
        let mut parts = vec![
            (0, Live::Reasoning(self.reasoning.len())),
            (1, Live::Text(&self.text)),
        ];
        parts.extend(self.calls.iter().map(|(i, c)| {
            (
                2 + i,
                Live::Call {
                    name: &c.name,
                    arguments: &c.arguments,
                },
            )
        }));
        parts
    }

    /// Applies one `data:` payload.
    pub fn apply(&mut self, data: &str) -> Result<(), ProviderError> {
        if data.trim() == "[DONE]" {
            self.saw_done = true;
            return Ok(());
        }
        let event: Value = serde_json::from_str(data).map_err(|e| {
            ProviderError::new(ErrorKind::Malformed, format!("bad stream chunk: {e}"))
        })?;
        if let Some(err) = event.get("error") {
            let mut e = ProviderError::new(ErrorKind::StreamError, err.to_string());
            e.output_started = self.output_bytes > 0;
            e.partial_output_bytes = self.output_bytes;
            return Err(e);
        }
        if let Some(id) = event.get("id").and_then(Value::as_str) {
            self.id.get_or_insert_with(|| id.to_owned());
        }
        if let Some(m) = event.get("model").and_then(Value::as_str) {
            self.model.get_or_insert_with(|| m.to_owned());
        }
        if let Some(u) = event.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(u.clone());
        }
        for choice in event
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(f) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish = Some(f.to_owned());
            }
            let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) else {
                continue;
            };
            if let Some(t) = delta.get("content").and_then(Value::as_str) {
                self.text.push_str(t);
                self.output_bytes += t.len();
            }
            if let Some(r) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Value::as_str)
            {
                self.reasoning.push_str(r);
                self.output_bytes += r.len();
            }
            for (fallback, call) in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let index = call
                    .get("index")
                    .and_then(Value::as_u64)
                    .map_or(fallback, |i| i as usize);
                let parts = self.calls.entry(index).or_default();
                if let Some(id) = call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    parts.id = id.to_owned();
                }
                let f = call.get("function");
                if let Some(n) = f.and_then(|f| f.get("name")).and_then(Value::as_str) {
                    parts.name.push_str(n);
                }
                match f.and_then(|f| f.get("arguments")) {
                    Some(Value::String(a)) => {
                        parts.arguments.push_str(a);
                        self.output_bytes += a.len();
                    }
                    Some(obj @ Value::Object(_)) if parts.arguments.is_empty() => {
                        parts.arguments = obj.to_string();
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Completes the response. `tools` enables text tool-call recovery.
    pub fn finish(
        self,
        requested_model: &str,
        recover_from: Option<&[crate::types::ToolSpec]>,
    ) -> Result<Response, ProviderError> {
        let Some(finish) = self.finish.clone() else {
            let mut e =
                ProviderError::new(ErrorKind::Truncated, "stream ended without a finish reason");
            e.output_started = self.output_bytes > 0;
            e.partial_output_bytes = self.output_bytes;
            return Err(e);
        };
        let mut tool_calls = Vec::new();
        for (index, parts) in self.calls {
            let raw = if parts.arguments.trim().is_empty() {
                "{}".to_owned()
            } else {
                parts.arguments
            };
            let arguments = match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(m)) => m,
                // Unparseable arguments are kept raw; the agent returns a tool error.
                _ => Map::new(),
            };
            tool_calls.push(ToolCall {
                id: if parts.id.is_empty() {
                    format!("call_{index}")
                } else {
                    parts.id
                },
                name: parts.name,
                arguments,
                raw_arguments: raw,
            });
        }
        let mut text = self.text;
        let mut stop = StopReason::from_wire(&finish);
        if tool_calls.is_empty()
            && stop.allows_tools()
            && let Some(recovered) =
                recover_from.and_then(|tools| recover_text_tool_call(&text, tools))
        {
            text.clear();
            tool_calls.push(recovered);
            stop = StopReason::ToolCalls;
        }
        let usage = match self.usage.as_ref() {
            Some(u) => usage_from_chat(u),
            None => Usage {
                output: estimate_tokens(self.output_bytes),
                status: UsageStatus::Estimated,
                ..Usage::default()
            },
        };
        Ok(Response {
            id: self.id,
            model: self.model.unwrap_or_else(|| requested_model.to_owned()),
            reasoning: (!self.reasoning.is_empty()).then_some(self.reasoning),
            text,
            tool_calls,
            stop,
            usage,
            attempts: Default::default(),
            replay: None,
        })
    }
}

/// Normalizes a Chat Completions `usage` object.
pub fn usage_from_chat(u: &Value) -> Usage {
    let get = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let prompt = get(u, "prompt_tokens");
    let cached = u
        .get("prompt_tokens_details")
        .map(|d| get(d, "cached_tokens"))
        .unwrap_or(0)
        .min(prompt);
    let written = u
        .get("prompt_tokens_details")
        .map(|d| get(d, "cache_write_tokens"))
        .unwrap_or(0)
        .min(prompt - cached);
    Usage {
        input: prompt - cached - written,
        cache_read: cached,
        cache_write: written,
        output: get(u, "completion_tokens"),
        reasoning: u
            .get("completion_tokens_details")
            .map(|d| get(d, "reasoning_tokens"))
            .unwrap_or(0),
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
                    reasoning: Some("think".into()),
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        arguments: Map::new(),
                        raw_arguments: "{\"path\": \"a\"}".into(),
                    }],
                    replay: None,
                },
                Item::ToolResult {
                    call_id: "c1".into(),
                    content: "data".into(),
                },
            ],
            tools: vec![
                ToolSpec {
                    name: "zeta".into(),
                    description: "z".into(),
                    parameters: json!({}),
                },
                ToolSpec {
                    name: "alpha".into(),
                    description: "a".into(),
                    parameters: json!({}),
                },
            ],
            ..Request::default()
        }
    }

    #[test]
    fn body_is_stable_sorted_and_replays_raw_arguments() {
        let b = build_body("m", &req(), true);
        assert_eq!(b["tools"][0]["function"]["name"], "alpha");
        assert_eq!(
            b["messages"][2]["tool_calls"][0]["function"]["arguments"],
            "{\"path\": \"a\"}"
        );
        assert_eq!(b["messages"][2]["reasoning_content"], "think");
        assert_eq!(b["messages"][3]["role"], "tool");
        assert_eq!(b["stream_options"]["include_usage"], true);
        // Appending an item must leave the serialized prefix unchanged.
        let mut longer = req();
        longer.items.push(Item::User {
            text: "more".into(),
        });
        let short = serde_json::to_string(&build_body("m", &req(), true)["messages"]).unwrap();
        let long = serde_json::to_string(&build_body("m", &longer, true)["messages"]).unwrap();
        assert!(long.starts_with(&short[..short.len() - 1]));
    }

    #[test]
    fn assembles_streamed_tool_calls_and_usage() {
        let mut a = ChatAssembler::default();
        for chunk in [
            r#"{"id":"r1","model":"glm-5.3","choices":[{"delta":{"reasoning_content":"plan"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"read_","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"file","arguments":"th\":\"x\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_tokens_details":{"cached_tokens":60},"completion_tokens_details":{"reasoning_tokens":5}}}"#,
            "[DONE]",
        ] {
            a.apply(chunk).unwrap();
        }
        let r = a.finish("glm-5.3", None).unwrap();
        assert_eq!(r.tool_calls[0].name, "read_file");
        assert_eq!(r.tool_calls[0].arguments["path"], "x");
        assert_eq!(r.stop, StopReason::ToolCalls);
        assert_eq!(r.reasoning.as_deref(), Some("plan"));
        assert_eq!(
            (
                r.usage.input,
                r.usage.cache_read,
                r.usage.output,
                r.usage.reasoning
            ),
            (40, 60, 20, 5)
        );
    }

    #[test]
    fn missing_finish_is_truncation_and_missing_usage_is_estimated() {
        let mut a = ChatAssembler::default();
        a.apply(r#"{"choices":[{"delta":{"content":"partial"}}]}"#)
            .unwrap();
        let e = a.finish("m", None).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Truncated);
        assert!(e.output_started);

        let mut b = ChatAssembler::default();
        b.apply(r#"{"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]}"#)
            .unwrap();
        let r = b.finish("m", None).unwrap();
        assert_eq!(r.usage.status, UsageStatus::Estimated);
    }

    #[test]
    fn length_stop_is_reported_and_blocks_tools() {
        let mut a = ChatAssembler::default();
        a.apply(r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"x","arguments":"{"}}]},"finish_reason":"length"}]}"#).unwrap();
        let r = a.finish("m", None).unwrap();
        assert_eq!(r.stop, StopReason::Length);
        assert!(!r.stop.allows_tools());
    }

    #[test]
    fn stream_error_mid_way_is_retryable_stream_error() {
        let mut a = ChatAssembler::default();
        a.apply(r#"{"choices":[{"delta":{"content":"abc"}}]}"#)
            .unwrap();
        let e = a
            .apply(r#"{"error":{"message":"overloaded"}}"#)
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::StreamError);
        assert!(e.is_retryable() && e.output_started);
    }
}
