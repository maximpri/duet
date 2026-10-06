// SPDX-License-Identifier: GPL-3.0-or-later
//! Recovery of one tool call that a local model wrote as text instead of a
//! structured call, in the `<tool_call><function=NAME><parameter=P>V</parameter>…`
//! form. Every ambiguity fails closed.

use crate::types::{ToolCall, ToolSpec};
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<tool_call>\s*<function=([A-Za-z_][A-Za-z0-9_]*)>\s*(.*?)\s*</tool_call>")
        .expect("static regex")
});
static PARAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<parameter=([A-Za-z_][A-Za-z0-9_]*)>\s*(.*?)\s*</parameter>")
        .expect("static regex")
});

pub fn recover_text_tool_call(text: &str, tools: &[ToolSpec]) -> Option<ToolCall> {
    if text.matches("<tool_call>").count() != 1 || text.matches("</tool_call>").count() != 1 {
        return None;
    }
    let caps = CALL.captures(text)?;
    let name = caps.get(1)?.as_str();
    let spec = tools.iter().find(|t| t.name == name)?;
    let mut body = caps.get(2)?.as_str().trim();
    if let Some(b) = body.strip_suffix("</function>") {
        body = b.trim_end();
    }
    let mut arguments = Map::new();
    let mut cursor = 0;
    for p in PARAM.captures_iter(body) {
        let whole = p.get(0)?;
        if !body[cursor..whole.start()].trim().is_empty() {
            return None;
        }
        let key = p.get(1)?.as_str();
        if arguments.contains_key(key) {
            return None;
        }
        let raw = p.get(2)?.as_str().trim();
        arguments.insert(
            key.to_owned(),
            serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned())),
        );
        cursor = whole.end();
    }
    if !body[cursor..].trim().is_empty() {
        return None;
    }
    let properties = spec.parameters.get("properties").and_then(Value::as_object);
    if let Some(props) = properties
        && arguments.keys().any(|k| !props.contains_key(k))
    {
        return None;
    }
    let required = spec.parameters.get("required").and_then(Value::as_array);
    if required
        .into_iter()
        .flatten()
        .any(|r| r.as_str().is_none_or(|r| !arguments.contains_key(r)))
    {
        return None;
    }
    let digest = Sha256::digest(caps.get(0)?.as_str().as_bytes());
    Some(ToolCall {
        id: format!("call_text_{}", hex::encode(&digest[..8])),
        name: name.to_owned(),
        raw_arguments: serde_json::to_string(&arguments).ok()?,
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "read_file".into(),
            description: String::new(),
            parameters: json!({"type":"object","properties":{"path":{},"start":{}},"required":["path"]}),
        }]
    }

    #[test]
    fn recovers_one_well_formed_call() {
        let t = "thinking...\n<tool_call>\n<function=read_file>\n<parameter=path>\nsrc/lib.rs\n</parameter>\n<parameter=start>\n10\n</parameter>\n</function>\n</tool_call>";
        let c = recover_text_tool_call(t, &tools()).unwrap();
        assert_eq!(c.name, "read_file");
        assert_eq!(c.arguments["path"], "src/lib.rs");
        assert_eq!(c.arguments["start"], 10);
    }

    #[test]
    fn fails_closed_on_ambiguity() {
        let ok =
            "<tool_call><function=read_file><parameter=path>a</parameter></function></tool_call>";
        assert!(recover_text_tool_call(ok, &tools()).is_some());
        assert!(
            recover_text_tool_call(&format!("{ok}{ok}"), &tools()).is_none(),
            "two calls"
        );
        assert!(
            recover_text_tool_call(
                "<tool_call><function=nope><parameter=path>a</parameter></tool_call>",
                &tools()
            )
            .is_none()
        );
        assert!(
            recover_text_tool_call(
                "<tool_call><function=read_file><parameter=start>1</parameter></tool_call>",
                &tools()
            )
            .is_none(),
            "missing required"
        );
        assert!(recover_text_tool_call("<tool_call><function=read_file><parameter=path>a</parameter><parameter=x>1</parameter></tool_call>", &tools()).is_none(), "undeclared");
        assert!(
            recover_text_tool_call(
                "<tool_call><function=read_file><parameter=path>a</parameter>junk</tool_call>",
                &tools()
            )
            .is_none()
        );
    }
}
