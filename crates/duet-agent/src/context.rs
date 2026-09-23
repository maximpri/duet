// SPDX-License-Identifier: GPL-3.0-or-later
//! Keeps the frontier's context under its window without summarizing.
//!
//! The conversation only grows at the end. When it passes `mask_at` of the
//! window, old tool results are replaced — all at once — by short stubs, and
//! masking continues until the estimate is under half the window. Doing it in
//! one batch means the provider's prefix cache is invalidated rarely.
//!
//! Masking works on whole turns: every result of one assistant turn is masked
//! together, oldest turns first, and the latest turns stay. Items are never
//! removed, so each tool call keeps its result. A stub names the call (tool
//! and main argument) and any handle the result referred to, so the model can
//! re-read exactly what it needs instead of repeating the work.

use duet_boundary::model::{Item, ToolCall};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Rough token estimate (about 3.5 bytes per token).
pub fn estimate(items: &[Item], system: &str) -> u64 {
    let bytes: usize = system.len()
        + items
            .iter()
            .map(|i| serde_json::to_string(i).map_or(0, |s| s.len()))
            .sum::<usize>();
    tokens_of_bytes(bytes)
}

fn tokens_of_bytes(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(7) * 2
}

/// Tokens a piece of text adds to a request (as serialized).
fn tokens_of(text: &str) -> u64 {
    tokens_of_bytes(serde_json::to_string(text).map_or(text.len(), |s| s.len()))
}

/// Assistant turns whose results always stay.
const KEEP_RECENT_TURNS: usize = 4;
/// Results this short cost less than a round trip to get them back; they stay.
const MIN_MASKED_CHARS: usize = 400;
const STUB_PREFIX: &str = "[masked: ";
/// Characters of a call's argument quoted in its stub.
const ARG_CHARS: usize = 120;

/// A result that begins with a handle (`h12 (source): ...`) refers to content
/// kept on this machine.
static HANDLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(h\d+) \(").expect("static regex"));

pub fn is_masked(content: &str) -> bool {
    content.starts_with(STUB_PREFIX)
}

fn quoted(s: &str) -> String {
    let mut out: String = s.chars().take(ARG_CHARS).collect();
    if s.chars().count() > ARG_CHARS {
        out.push('…');
    }
    out
}

/// `read_file src/lib.rs lines 10-80`, `run_command `cargo test``, ...
fn describe(call: &ToolCall) -> String {
    let arg = |k: &str| call.arguments.get(k).and_then(Value::as_str).map(quoted);
    let num = |k: &str| call.arguments.get(k).and_then(Value::as_u64);
    let main = match call.name.as_str() {
        "read_file" => arg("path").map(|p| match (num("start_line"), num("end_line")) {
            (None, None) => p,
            (s, e) => format!(
                "{p} lines {}-{}",
                s.unwrap_or(1),
                e.map_or("end".to_owned(), |e| e.to_string())
            ),
        }),
        "run_command" => arg("command").map(|c| format!("`{c}`")),
        "search" => arg("pattern").map(|p| format!("`{p}`")),
        "list_files" => arg("dir"),
        "ask_local" => arg("handle"),
        "read_raw" => arg("handle").map(|h| match (num("start_line"), num("end_line")) {
            (None, None) => h,
            (s, e) => format!(
                "{h} lines {}-{}",
                s.map_or("start".to_owned(), |s| s.to_string()),
                e.map_or("…".to_owned(), |e| e.to_string())
            ),
        }),
        _ => None,
    };
    match main {
        Some(m) => format!("{} {m}", call.name),
        None => call.name.clone(),
    }
}

/// The stub that replaces `content`, the result of `call`.
pub fn stub(call: Option<&ToolCall>, content: &str) -> String {
    let what = call.map_or("tool result".to_owned(), describe);
    let tokens = tokens_of(content);
    match HANDLE.captures(content).and_then(|c| c.get(1)) {
        Some(h) => {
            let h = h.as_str();
            format!(
                "{STUB_PREFIX}{what} → {h}, ~{tokens} tokens removed to save context; \
{h} is still available (read_raw for public content, ask_local for sensitive)]"
            )
        }
        None => format!(
            "{STUB_PREFIX}{what}, ~{tokens} tokens removed to save context; repeat the call if you need it]"
        ),
    }
}

/// Masks old tool results if needed. Returns how many were masked.
pub fn mask_if_needed(items: &mut [Item], system: &str, window: u64, mask_at: f64) -> usize {
    let mut current = estimate(items, system);
    if (current as f64) < mask_at * window as f64 {
        return 0;
    }
    let target = window / 2;
    // Result positions grouped by the assistant turn that called them.
    let mut calls: HashMap<String, ToolCall> = HashMap::new();
    let mut turns: Vec<Vec<usize>> = Vec::new();
    for (n, item) in items.iter().enumerate() {
        match item {
            Item::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                for c in tool_calls {
                    calls.insert(c.id.clone(), c.clone());
                }
                turns.push(Vec::new());
            }
            Item::ToolResult { .. } => match turns.last_mut() {
                Some(t) => t.push(n),
                None => turns.push(vec![n]),
            },
            _ => {}
        }
    }
    turns.retain(|t| !t.is_empty());
    let maskable = turns.len().saturating_sub(KEEP_RECENT_TURNS);
    let mut masked = 0;
    for turn in turns.iter().take(maskable) {
        if current <= target {
            break;
        }
        for &pos in turn {
            if let Item::ToolResult { call_id, content } = &mut items[pos]
                && !is_masked(content)
                && content.len() >= MIN_MASKED_CHARS
            {
                let replacement = stub(calls.get(call_id.as_str()), content);
                current = current.saturating_sub(tokens_of(content)) + tokens_of(&replacement);
                *content = replacement;
                masked += 1;
            }
        }
    }
    masked
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        let Value::Object(arguments) = args else {
            unreachable!()
        };
        ToolCall {
            id: id.into(),
            name: name.into(),
            raw_arguments: serde_json::to_string(&arguments).unwrap(),
            arguments,
        }
    }

    /// `n` turns, each reading `per_turn` files with results of `size` bytes.
    fn items(n: usize, per_turn: usize, size: usize) -> Vec<Item> {
        let mut v = vec![Item::User {
            text: "task".into(),
        }];
        for t in 0..n {
            let calls: Vec<ToolCall> = (0..per_turn)
                .map(|k| {
                    call(
                        &format!("c{t}-{k}"),
                        "read_file",
                        json!({"path": format!("src/f{t}_{k}.rs")}),
                    )
                })
                .collect();
            v.push(Item::Assistant {
                text: String::new(),
                reasoning: None,
                tool_calls: calls.clone(),
            });
            for c in calls {
                v.push(Item::ToolResult {
                    call_id: c.id,
                    content: "x".repeat(size),
                });
            }
        }
        v
    }

    fn masked_at(v: &[Item], pos: usize) -> bool {
        matches!(&v[pos], Item::ToolResult { content, .. } if is_masked(content))
    }

    #[test]
    fn nothing_masked_below_threshold() {
        let mut v = items(3, 1, 100);
        assert_eq!(mask_if_needed(&mut v, "s", 100_000, 0.7), 0);
    }

    #[test]
    fn masks_oldest_turns_first_keeps_recent_and_drops_below_half() {
        let mut v = items(20, 1, 7000);
        let before = estimate(&v, "s");
        let window = before + 1000;
        let masked = mask_if_needed(&mut v, "s", window, 0.7);
        assert!(masked > 0);
        assert!(estimate(&v, "s") <= window / 2 + 3000);
        let last: Vec<&Item> = v.iter().rev().take(2 * KEEP_RECENT_TURNS).collect();
        assert!(
            last.iter()
                .all(|i| !matches!(i, Item::ToolResult { content, .. } if is_masked(content)))
        );
        assert!(masked_at(&v, 2));
        // A second call right after does nothing (batching).
        assert_eq!(mask_if_needed(&mut v, "s", window, 0.7), 0);
    }

    #[test]
    fn a_turn_is_masked_whole_and_every_call_keeps_its_result() {
        let mut v = items(12, 3, 4000);
        let ids = |v: &[Item]| -> Vec<String> {
            v.iter()
                .filter_map(|i| match i {
                    Item::ToolResult { call_id, .. } => Some(call_id.clone()),
                    _ => None,
                })
                .collect()
        };
        let before_ids = ids(&v);
        let window = estimate(&v, "s") + 1000;
        assert!(mask_if_needed(&mut v, "s", window, 0.7) > 0);
        assert_eq!(ids(&v), before_ids, "no result removed or reordered");
        // Each turn is [assistant, r, r, r]: its three results share one fate.
        for t in 0..12 {
            let base = 1 + t * 4;
            let states: Vec<bool> = (1..=3).map(|k| masked_at(&v, base + k)).collect();
            assert!(
                states.iter().all(|s| *s == states[0]),
                "turn {t} split: {states:?}"
            );
        }
    }

    #[test]
    fn short_results_stay() {
        let mut v = items(20, 1, 7000);
        if let Item::ToolResult { content, .. } = &mut v[2] {
            *content = "edited src/f0_0.rs (1 edit)".into();
        }
        let window = estimate(&v, "s") + 1000;
        assert!(mask_if_needed(&mut v, "s", window, 0.7) > 0);
        assert!(!masked_at(&v, 2));
        assert!(masked_at(&v, 4));
    }

    #[test]
    fn stubs_name_the_call_and_the_handle() {
        let read = call(
            "a",
            "read_file",
            json!({"path": "src/ledger.rs", "start_line": 10, "end_line": 80}),
        );
        let s = stub(Some(&read), &"x".repeat(6998));
        assert!(
            s.starts_with("[masked: read_file src/ledger.rs lines 10-80, ~2000 tokens"),
            "{s}"
        );
        assert!(s.contains("repeat the call"), "{s}");
        let cmd = call("b", "run_command", json!({"command": "cargo test -p fx"}));
        let held =
            "h7 (output of `cargo test -p fx`): 900 lines, ...".to_owned() + &"y".repeat(900);
        let s = stub(Some(&cmd), &held);
        assert!(
            s.starts_with("[masked: run_command `cargo test -p fx` → h7,"),
            "{s}"
        );
        assert!(s.contains("h7 is still available"), "{s}");
        let ask = call(
            "c",
            "ask_local",
            json!({"handle": "h3", "questions": ["q"]}),
        );
        assert!(stub(Some(&ask), "answer").starts_with("[masked: ask_local h3,"));
        assert!(stub(None, "z").starts_with("[masked: tool result,"));
        let odd = call("d", "finish", Value::Object(Map::new()));
        assert!(stub(Some(&odd), "z").starts_with("[masked: finish,"));
    }
}
