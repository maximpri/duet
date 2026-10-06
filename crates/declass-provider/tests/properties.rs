// SPDX-License-Identifier: GPL-3.0-or-later
//! Property tests for the parsers that read untrusted model output: the SSE
//! decoder, the streamed-chunk assemblers of every dialect and text tool-call
//! recovery. Model
//! output is attacker-influenced (prompt injection, a hostile local server), so
//! none of them may panic, whatever bytes arrive.
//!
//! Cases per property default to a small number so `tools/gate.sh` stays fast;
//! set `PROPTEST_CASES` to run more (for example `PROPTEST_CASES=20000`).

use declass_provider::anthropic::AnthropicAssembler;
use declass_provider::chat::ChatAssembler;
use declass_provider::recover::recover_text_tool_call;
use declass_provider::responses::ResponsesAssembler;
use declass_provider::sse::{SseDecoder, SseEvent};
use declass_provider::types::{Part, Replay, ToolCall, ToolSpec, assistant_pieces};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const DEFAULT_CASES: u32 = 256;

fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CASES);
    ProptestConfig {
        cases,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// Bytes biased toward SSE syntax: field names, separators, CR/LF, invalid UTF-8.
fn sse_bytes() -> impl Strategy<Value = Vec<u8>> {
    let piece = prop_oneof![
        Just(b"data:".to_vec()),
        Just(b"data: ".to_vec()),
        Just(b"event:".to_vec()),
        Just(b"id: 1".to_vec()),
        Just(b": comment".to_vec()),
        Just(b"\n".to_vec()),
        Just(b"\r\n".to_vec()),
        Just(b"\r".to_vec()),
        Just(b"[DONE]".to_vec()),
        Just(vec![0xff, 0xfe]),
        Just("⟨é⟩".as_bytes().to_vec()),
        proptest::collection::vec(any::<u8>(), 0..8),
    ];
    proptest::collection::vec(piece, 0..40).prop_map(|p| p.concat())
}

fn decode_in_chunks(bytes: &[u8], cuts: &[usize]) -> Vec<SseEvent> {
    let mut d = SseDecoder::default();
    let mut out = Vec::new();
    let mut last = 0;
    let mut cuts: Vec<usize> = cuts.iter().map(|c| c % (bytes.len() + 1)).collect();
    cuts.sort_unstable();
    for c in cuts {
        out.extend(d.push(&bytes[last..c]));
        last = c;
    }
    out.extend(d.push(&bytes[last..]));
    out.extend(d.finish());
    out
}

/// Keys the assembler looks at, so generated chunks reach its branches.
const CHUNK_KEYS: &[&str] = &[
    "choices",
    "delta",
    "message",
    "content",
    "reasoning_content",
    "reasoning",
    "tool_calls",
    "index",
    "id",
    "function",
    "name",
    "arguments",
    "finish_reason",
    "usage",
    "prompt_tokens",
    "prompt_tokens_details",
    "cached_tokens",
    "completion_tokens",
    "completion_tokens_details",
    "reasoning_tokens",
    "model",
    "error",
    // Anthropic Messages and Responses events.
    "type",
    "message",
    "content_block",
    "text",
    "thinking",
    "signature",
    "partial_json",
    "input",
    "stop_reason",
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
    "response",
    "item",
    "output_index",
    "call_id",
    "output",
    "summary",
    "encrypted_content",
    "status",
    "incomplete_details",
    "reason",
    "input_tokens_details",
];

/// Event types of the Anthropic Messages and Responses streams.
const EVENT_TYPES: &[&str] = &[
    "message_start",
    "content_block_start",
    "content_block_delta",
    "message_delta",
    "message_stop",
    "error",
    "text",
    "thinking",
    "tool_use",
    "text_delta",
    "thinking_delta",
    "signature_delta",
    "input_json_delta",
    "response.created",
    "response.output_item.added",
    "response.output_item.done",
    "response.output_text.delta",
    "response.function_call_arguments.delta",
    "response.reasoning_summary_text.delta",
    "response.completed",
    "response.incomplete",
    "response.failed",
    "message",
    "function_call",
    "reasoning",
    "output_text",
];

/// A JSON object with an event `type`, so generated events reach the branches.
fn event() -> impl Strategy<Value = Value> {
    (proptest::sample::select(EVENT_TYPES), json_value()).prop_map(|(t, v)| {
        let mut m = match v {
            Value::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("delta".into(), other);
                m
            }
        };
        m.insert("type".into(), json!(t));
        Value::Object(m)
    })
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<u64>().prop_map(|n| json!(n)),
        any::<i64>().prop_map(|n| json!(n)),
        any::<f64>().prop_map(|n| json!(n)),
        prop_oneof![
            Just("stop".to_owned()),
            Just("tool_calls".to_owned()),
            Just("length".to_owned()),
            Just("<tool_call><function=read_file><parameter=path>a</parameter></tool_call>".into()),
            ".{0,12}",
        ]
        .prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::vec((proptest::sample::select(CHUNK_KEYS), inner), 0..5)
                .prop_map(|kv| Value::Object(
                    kv.into_iter()
                        .map(|(k, v)| (k.to_owned(), v))
                        .collect::<Map<_, _>>()
                )),
        ]
    })
}

fn tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read_file".into(),
            description: String::new(),
            parameters: json!({"type":"object","properties":{"path":{},"start":{}},"required":["path"]}),
        },
        ToolSpec {
            name: "odd".into(),
            description: String::new(),
            // Malformed schemas must not panic either.
            parameters: json!({"properties": 3, "required": [1, null, "x"]}),
        },
    ]
}

/// Text biased toward the recovered call syntax.
fn call_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("<tool_call>".to_owned()),
        Just("</tool_call>".to_owned()),
        Just("<function=read_file>".to_owned()),
        Just("<function=odd>".to_owned()),
        Just("</function>".to_owned()),
        Just("<parameter=path>".to_owned()),
        Just("<parameter=start>".to_owned()),
        Just("<parameter=x>".to_owned()),
        Just("</parameter>".to_owned()),
        Just("{\"a\": [1, 2]}".to_owned()),
        Just(" \n".to_owned()),
        Just("é⟩".to_owned()),
        ".{0,6}",
    ];
    proptest::collection::vec(piece, 0..24).prop_map(|p| p.concat())
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn sse_decoder_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut d = SseDecoder::default();
        let _ = d.push(&bytes);
        let _ = d.finish();
    }

    /// Events do not depend on how the stream was split into network chunks.
    #[test]
    fn sse_events_do_not_depend_on_chunking(bytes in sse_bytes(), cuts in proptest::collection::vec(any::<usize>(), 0..12)) {
        let whole = decode_in_chunks(&bytes, &[]);
        let split = decode_in_chunks(&bytes, &cuts);
        prop_assert_eq!(whole, split);
    }

    #[test]
    fn chat_assembler_never_panics_on_arbitrary_text(chunks in proptest::collection::vec(".{0,64}", 0..8)) {
        let mut a = ChatAssembler::default();
        for c in &chunks {
            let _ = a.apply(c);
        }
        let _ = a.finish("m", Some(&tools()));
    }

    #[test]
    fn chat_assembler_never_panics_on_arbitrary_json(chunks in proptest::collection::vec(json_value(), 0..8), recover in any::<bool>()) {
        let mut a = ChatAssembler::default();
        for c in &chunks {
            let _ = a.apply(&c.to_string());
        }
        let t = tools();
        let _ = a.finish("m", recover.then_some(t.as_slice()));
    }

    #[test]
    fn anthropic_and_responses_assemblers_never_panic(events in proptest::collection::vec(event(), 0..10), texts in proptest::collection::vec(".{0,32}", 0..3)) {
        let mut a = AnthropicAssembler::default();
        let mut r = ResponsesAssembler::default();
        for e in &events {
            let _ = a.apply(&e.to_string());
            let _ = r.apply(&e.to_string());
        }
        for t in &texts {
            let _ = a.apply(t);
            let _ = r.apply(t);
        }
        let _ = a.finish("m");
        let _ = r.finish("m");
    }

    /// Replaying a turn never panics, whatever layout a (possibly edited)
    /// transcript holds, and never loses or duplicates the text or a call.
    #[test]
    fn replayed_turns_keep_their_text_and_calls(text in ".{0,24}", lengths in proptest::collection::vec(0usize..12, 0..4), ids in proptest::collection::vec("[ab]", 0..4)) {
        let calls: Vec<ToolCall> = ["a", "b"].iter().map(|id| ToolCall {
            id: (*id).into(), name: "x".into(), arguments: Map::new(), raw_arguments: "{}".into(),
        }).collect();
        let mut parts: Vec<Part> = lengths.iter().map(|&bytes| Part::Text { bytes }).collect();
        parts.extend(ids.iter().map(|id| Part::Call { id: id.clone() }));
        parts.push(Part::Opaque { block: json!({"type": "thinking"}) });
        let replay = Replay { dialect: "anthropic".into(), parts };
        let pieces = assistant_pieces("anthropic", &text, &calls, Some(&replay));
        let mut joined = String::new();
        let mut seen = Vec::new();
        for p in &pieces {
            match p {
                declass_provider::types::Piece::Text(t) => joined.push_str(t),
                declass_provider::types::Piece::Call(c) => seen.push(c.id.clone()),
                declass_provider::types::Piece::Opaque(_) => {}
            }
        }
        prop_assert_eq!(joined, text);
        seen.sort();
        prop_assert_eq!(seen, vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn tool_call_recovery_never_panics(text in call_text(), noise in ".{0,64}") {
        let t = tools();
        for input in [text.as_str(), noise.as_str(), &format!("{noise}{text}")] {
            if let Some(call) = recover_text_tool_call(input, &t) {
                // A recovered call is well formed: declared tool, arguments that round-trip.
                prop_assert!(t.iter().any(|s| s.name == call.name));
                let parsed: Value = serde_json::from_str(&call.raw_arguments).unwrap();
                prop_assert_eq!(parsed, Value::Object(call.arguments.clone()));
            }
        }
    }
}
