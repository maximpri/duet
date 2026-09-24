// SPDX-License-Identifier: GPL-3.0-or-later
//! Property tests for the parsers that read untrusted model output: the SSE
//! decoder, the streamed-chunk assembler and text tool-call recovery. Model
//! output is attacker-influenced (prompt injection, a hostile local server), so
//! none of them may panic, whatever bytes arrive.
//!
//! Cases per property default to a small number so `tools/gate.sh` stays fast;
//! set `PROPTEST_CASES` to run more (for example `PROPTEST_CASES=20000`).

use duet_provider::chat::ChatAssembler;
use duet_provider::recover::recover_text_tool_call;
use duet_provider::sse::{SseDecoder, SseEvent};
use duet_provider::types::ToolSpec;
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
];

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
