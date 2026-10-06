// SPDX-License-Identifier: GPL-3.0-or-later
//! A `tools/call` result as text. Text blocks are kept; anything else (images,
//! audio, binary resources) becomes a one-line description, since only text
//! can be scanned before it is shown.

use serde_json::Value;

/// Characters of text kept from one result (the caller's presenter applies its
/// own, usually smaller, limits).
pub const MAX_TEXT: usize = 1 << 20;

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// Decoded size of base64 `data`.
fn decoded_len(data: &str) -> usize {
    let data = data.trim_end_matches('=');
    data.len() * 3 / 4
}

fn describe(block: &Value) -> String {
    let short = |s: &str| s.chars().take(200).collect::<String>();
    match field(block, "type") {
        "text" => field(block, "text").to_owned(),
        kind @ ("image" | "audio") => format!(
            "[{kind}: {}, {} bytes; not shown]",
            short(field(block, "mimeType")),
            decoded_len(field(block, "data"))
        ),
        "resource_link" => format!(
            "[resource link: {} <{}>{}]",
            short(field(block, "name")),
            short(field(block, "uri")),
            match field(block, "mimeType") {
                "" => String::new(),
                m => format!(" ({})", short(m)),
            }
        ),
        "resource" => {
            let r = block.get("resource").unwrap_or(&Value::Null);
            match r.get("text").and_then(Value::as_str) {
                Some(text) => format!("[resource <{}>]\n{text}", short(field(r, "uri"))),
                None => format!(
                    "[resource <{}>: {}, {} bytes of binary data; not shown]",
                    short(field(r, "uri")),
                    short(field(r, "mimeType")),
                    decoded_len(field(r, "blob"))
                ),
            }
        }
        other => format!("[content of type {}; not shown]", short(other)),
    }
}

/// The result's content blocks joined by newlines; the structured result
/// (`structuredContent`) when there are none.
pub fn render(result: &Value) -> String {
    let blocks = result
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut out = blocks.iter().map(describe).collect::<Vec<_>>().join("\n");
    if blocks.is_empty()
        && let Some(s) = result.get("structuredContent")
    {
        out = s.to_string();
    }
    if out.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        let total = out.len();
        out.truncate(cut);
        out.push_str(&format!("\n[result truncated: {cut} of {total} bytes]"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn non_text_content_is_described_not_shown() {
        let r = json!({"content": [
            {"type": "text", "text": "hello"},
            {"type": "image", "data": "aGVsbG8h", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "body"}},
            {"type": "resource", "resource": {"uri": "file:///b.bin", "mimeType": "application/zip", "blob": "AAAA"}},
            {"type": "resource_link", "uri": "file:///c", "name": "c"},
            {"type": "audio", "data": "", "mimeType": "audio/wav"}
        ]});
        let text = render(&r);
        assert!(text.starts_with("hello\n[image: image/png, 6 bytes; not shown]"));
        assert!(text.contains("[resource <file:///a.txt>]\nbody"));
        assert!(text.contains("application/zip, 3 bytes of binary data"));
        assert!(text.contains("[resource link: c <file:///c>]"));
        assert!(!text.contains("aGVsbG8h"));
    }

    #[test]
    fn structured_content_is_used_when_there_are_no_blocks() {
        let r = json!({"content": [], "structuredContent": {"n": 3}});
        assert_eq!(render(&r), r#"{"n":3}"#);
    }
}
