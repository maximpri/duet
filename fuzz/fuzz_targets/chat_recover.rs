// SPDX-License-Identifier: GPL-3.0-or-later
//! Streamed-chunk assembly and text tool-call recovery: never panic on model
//! output; a recovered call names a declared tool and its arguments round-trip.
#![no_main]

use declass_provider::chat::ChatAssembler;
use declass_provider::recover::recover_text_tool_call;
use declass_provider::types::ToolSpec;
use libfuzzer_sys::fuzz_target;
use serde_json::{Value, json};

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
            parameters: json!({"properties": 3, "required": [1, null, "x"]}),
        },
    ]
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let tools = tools();
    if let Some(call) = recover_text_tool_call(&text, &tools) {
        assert!(tools.iter().any(|t| t.name == call.name));
        let parsed: Value = serde_json::from_str(&call.raw_arguments).expect("valid JSON");
        assert_eq!(parsed, Value::Object(call.arguments));
    }
    // Each line is one `data:` payload.
    let mut assembler = ChatAssembler::default();
    for line in text.lines() {
        let _ = assembler.apply(line);
    }
    let _ = assembler.finish("m", Some(&tools));
});
