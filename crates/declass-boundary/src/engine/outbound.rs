// SPDX-License-Identifier: GPL-3.0-or-later
//! The outbound filter and the final check (see [`crate::gate`]).
//!
//! Both read a request the same way, so the filter never leaves what the
//! check refuses. The texts of a request are every string of it (the system
//! prompt, tool descriptions, every field of every item, replayed reasoning
//! blocks) and, in a string that is itself JSON (tool-call arguments, a JSON
//! tool result), every string and key inside it. A value is disclosed when it
//! occurs in one of those texts outside Declass's tokens and markers
//! ([`Vault::disclosed_in`]).
//!
//! The filter works in two passes. The first re-runs the detectors, the
//! copied-span filter and the protected-code filter over the text the model
//! is shown (the operator's messages, tool results); what the detectors find
//! joins the vault. The second replaces every value the vault holds by then in
//! every text of the request. Doing both item by item missed values the first
//! pass found in a later item (a masking stub quoting a command) wherever they
//! also occurred earlier (the command in the model's own tool call), and the
//! check then blocked the request and ended the run (DECLASS-2026-019).
//!
//! The check reads the body as sent, string by string, and skips strings that
//! are only its framing ([`Framing`]): a vault value that spells a key, a role
//! or the model's name must not block every request (DECLASS-2026-020).
//!
//! When the check still refuses a request, [`OutboundFilter::withhold`]
//! replaces each part that holds a value by [`PART_WITHHELD`].

use super::{Engine, State};
use crate::gate::{Framing, OutboundCheck, OutboundFilter};
use crate::model::{Item, Request};
use crate::vault::Vault;
use declass_provider::types::Part;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;

/// Stands in for a part of a request that still held a sensitive value after
/// filtering ([`OutboundFilter::withhold`]).
pub const PART_WITHHELD: &str = "⟨withheld:held-a-sensitive-value⟩";

/// Levels of JSON inside JSON that are read (tool-call arguments are JSON
/// inside the body's JSON; a JSON document in them is one level more).
const NESTED_DEPTH: usize = 4;

/// `s` parsed, if it is a JSON object, array or string.
fn nested(s: &str) -> Option<Value> {
    if !s.trim_start().starts_with(['{', '[', '"']) {
        return None;
    }
    serde_json::from_str::<Value>(s)
        .ok()
        .filter(|v| matches!(v, Value::Object(_) | Value::Array(_) | Value::String(_)))
}

/// Calls `visit` on each text of the string `s` (`s`, and if it is JSON every
/// string and key inside it) until it returns `true`.
fn texts_of(s: &str, depth: usize, visit: &mut dyn FnMut(&str) -> bool) -> bool {
    visit(s) || (depth < NESTED_DEPTH && nested(s).is_some_and(|v| texts_in(&v, depth + 1, visit)))
}

/// [`texts_of`] every string and key of `v`.
fn texts_in(v: &Value, depth: usize, visit: &mut dyn FnMut(&str) -> bool) -> bool {
    match v {
        Value::String(s) => texts_of(s, depth, visit),
        Value::Array(a) => a.iter().any(|x| texts_in(x, depth, visit)),
        Value::Object(m) => m.iter().any(|(k, x)| visit(k) || texts_in(x, depth, visit)),
        _ => false,
    }
}

/// Whether a text of `s` holds a value the check refuses.
fn holds_value(vault: &Vault, s: &str) -> bool {
    texts_of(s, 0, &mut |t| vault.disclosed_in(t).is_some())
}

/// Whether a text of `v` holds a value the check refuses.
fn value_holds_value(vault: &Vault, v: &Value) -> bool {
    texts_in(v, 0, &mut |t| vault.disclosed_in(t).is_some())
}

/// Replaces every value the vault holds in each text of `s`, until none holds
/// one; JSON in which something was replaced is written back compactly.
/// Returns how many were replaced.
fn scrub(vault: &Vault, s: &mut String, depth: usize) -> usize {
    let mut total = 0;
    // Writing JSON back can join text that was apart, so it is read again; a
    // round that replaces nothing ends it.
    for _ in 0..NESTED_DEPTH {
        let (tokenized, mut n) = vault.tokenize(s);
        if n > 0 {
            *s = tokenized;
        }
        if depth < NESTED_DEPTH
            && let Some(mut v) = nested(s)
        {
            let inner = scrub_value(vault, &mut v, depth + 1);
            if inner > 0 {
                *s = v.to_string();
                n += inner;
            }
        }
        total += n;
        if n == 0 {
            break;
        }
    }
    total
}

/// [`scrub`] every string of `v`, and replace values in its keys.
fn scrub_value(vault: &Vault, v: &mut Value, depth: usize) -> usize {
    match v {
        Value::String(s) => scrub(vault, s, depth),
        Value::Array(a) => a.iter_mut().map(|x| scrub_value(vault, x, depth)).sum(),
        Value::Object(m) => {
            let mut n = 0;
            for (k, mut x) in std::mem::take(m) {
                let (k, replaced) = vault.tokenize(&k);
                n += replaced + scrub_value(vault, &mut x, depth);
                m.insert(k, x);
            }
            n
        }
        _ => 0,
    }
}

/// Whether a replayed reasoning block holds a value (it cannot be edited).
fn replay_holds_value(vault: &Vault, parts: &[Part]) -> bool {
    parts.iter().any(|p| match p {
        Part::Opaque { block } => value_holds_value(vault, block),
        _ => false,
    })
}

/// Describes only locations and replacement tokens, never the matched values.
/// Field-local line numbers let the operator find the edit without logging
/// source snippets or detokenizing any protected data.
fn change_note(vault: &Vault, location: &str, before: &str, after: &str) -> String {
    let mut prior = std::collections::HashMap::<String, usize>::new();
    for token in Vault::tokens_in(before) {
        *prior.entry(token).or_default() += 1;
    }
    let mut tokens = Vec::new();
    for token in Vault::tokens_in(after) {
        let count = prior.entry(token.clone()).or_default();
        if *count > 0 {
            *count -= 1;
            continue;
        }
        if tokens.contains(&token) {
            continue;
        }
        tokens.push(token);
    }
    let replacements = tokens
        .iter()
        .take(8)
        .map(|token| match vault.value_of(token) {
            Some((_, entry)) => {
                let mut origin = entry.origin.clone();
                scrub(vault, &mut origin, 0);
                format!(
                    "{token} ({}; first seen {})",
                    entry.kind.tag(),
                    origin.chars().take(120).collect::<String>()
                )
            }
            None => token.clone(),
        })
        .collect::<Vec<_>>();
    let original: Vec<_> = before.lines().collect();
    let changed = after
        .lines()
        .enumerate()
        .filter(|(i, line)| original.get(*i).copied() != Some(*line))
        .map(|(i, _)| (i + 1).to_string())
        .collect::<Vec<_>>();
    let mut location = location.to_owned();
    scrub(vault, &mut location, 0);
    let location = location.chars().take(512).collect::<String>();
    let mut note = format!(
        "{location}: filtered line(s) {}",
        changed
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    if changed.len() > 8 {
        note.push_str(&format!(" (+{} more lines)", changed.len() - 8));
    }
    if !replacements.is_empty() {
        note.push_str(&format!("; replaced with {}", replacements.join(", ")));
    }
    if tokens.len() > 8 {
        note.push_str(&format!("; {} more replacement types", tokens.len() - 8));
    }
    scrub(vault, &mut note, 0);
    note
}

fn changed_fields(vault: &Vault, before: &Value, after: &Value, path: &str, out: &mut Vec<String>) {
    if before == after || out.len() >= 32 {
        return;
    }
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in b {
                changed_fields(
                    vault,
                    a.get(key).unwrap_or(&Value::Null),
                    value,
                    &format!("{path}.{key}"),
                    out,
                );
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, value) in b.iter().enumerate() {
                changed_fields(
                    vault,
                    a.get(i).unwrap_or(&Value::Null),
                    value,
                    &format!("{path}[{i}]"),
                    out,
                );
            }
        }
        (Value::String(a), Value::String(b)) => out.push(change_note(vault, path, a, b)),
        _ => out.push(change_note(
            vault,
            path,
            &before.to_string(),
            &after.to_string(),
        )),
    }
}

/// The second pass: every value the vault holds is replaced in every text of
/// the request. Tool names and parameter schemas are the request's framing
/// (fixed for a run) and a call to a declared tool keeps its name.
fn replace_known(vault: &Vault, request: &mut Request) -> Vec<String> {
    let mut notes = Vec::new();
    let before = request.system.clone();
    let n = scrub(vault, &mut request.system, 0);
    if n > 0 {
        notes.push(change_note(
            vault,
            "system prompt",
            &before,
            &request.system,
        ));
    }
    for tool in &mut request.tools {
        let before = tool.description.clone();
        if scrub(vault, &mut tool.description, 0) > 0 {
            notes.push(change_note(
                vault,
                &format!("tool {} description", tool.name),
                &before,
                &tool.description,
            ));
        }
    }
    let declared: HashSet<String> = request.tools.iter().map(|t| t.name.clone()).collect();
    for (index, item) in request.items.iter_mut().enumerate() {
        let locator = match &*item {
            Item::ToolResult { call_id, .. } => {
                format!("history item {} · result for call {call_id}", index + 1)
            }
            Item::User { .. } => format!("history item {} · user message", index + 1),
            _ => format!("history item {} · assistant", index + 1),
        };
        match item {
            Item::User { text } | Item::ToolResult { content: text, .. } => {
                let before = text.clone();
                if scrub(vault, text, 0) > 0 {
                    notes.push(change_note(vault, &locator, &before, text));
                }
            }
            Item::Images { .. } => {}
            Item::Assistant {
                text,
                reasoning,
                tool_calls,
                replay,
            } => {
                let before = text.clone();
                let mut replaced = scrub(vault, text, 0);
                if replaced > 0 {
                    notes.push(change_note(
                        vault,
                        &format!("{locator}.text"),
                        &before,
                        text,
                    ));
                }
                if let Some(r) = reasoning {
                    let before = r.clone();
                    let n = scrub(vault, r, 0);
                    replaced += n;
                    if n > 0 {
                        notes.push(change_note(
                            vault,
                            &format!("{locator}.reasoning"),
                            &before,
                            r,
                        ));
                    }
                }
                for call in tool_calls.iter_mut() {
                    let before = Value::Object(call.arguments.clone());
                    let before_raw = call.raw_arguments.clone();
                    if !declared.contains(&call.name) {
                        let before_name = call.name.clone();
                        let n = scrub(vault, &mut call.name, 0);
                        replaced += n;
                        if n > 0 {
                            notes.push(change_note(
                                vault,
                                &format!("{locator} · call {} · name", call.id),
                                &before_name,
                                &call.name,
                            ));
                        }
                    }
                    let previous = replaced;
                    // Arguments are JSON: a value holding `"` or `\` is
                    // escaped in the raw text and found once it is parsed.
                    replaced += scrub(vault, &mut call.raw_arguments, 0);
                    // The parsed arguments follow the raw ones; a dialect
                    // sends them when the raw text is not a JSON object.
                    match serde_json::from_str::<Value>(&call.raw_arguments) {
                        Ok(Value::Object(args)) => call.arguments = args,
                        _ => {
                            let mut args = Value::Object(std::mem::take(&mut call.arguments));
                            replaced += scrub_value(vault, &mut args, 0);
                            if let Value::Object(args) = args {
                                call.arguments = args;
                            }
                        }
                    }
                    if replaced > previous {
                        let source = call
                            .arguments
                            .get("path")
                            .and_then(Value::as_str)
                            .map(|p| format!(" · {p}"))
                            .unwrap_or_default();
                        let location = format!(
                            "{locator} · call {} · {}{source} · arguments",
                            call.id, call.name
                        );
                        let mut fields = Vec::new();
                        changed_fields(
                            vault,
                            &before,
                            &Value::Object(call.arguments.clone()),
                            &location,
                            &mut fields,
                        );
                        if fields.is_empty() {
                            fields.push(change_note(
                                vault,
                                &location,
                                &before_raw,
                                &call.raw_arguments,
                            ));
                        }
                        if fields.len() == 32 {
                            fields.push(format!("{locator}: field preview limited to 32; inspect recorded outbound text"));
                        }
                        notes.extend(fields);
                    }
                }
                if replaced > 0 {
                    // Signed or encrypted reasoning cannot be edited; it is
                    // dropped with the edited turn rather than replayed.
                    if replay.take().is_some() {
                        notes.push(format!(
                            "{locator}: dropped signed replay because its message changed"
                        ));
                    }
                } else if replay
                    .as_ref()
                    .is_some_and(|r| replay_holds_value(vault, &r.parts))
                {
                    *replay = None;
                    notes.push(format!("{locator}.replay: dropped replayed reasoning containing a known sensitive value"));
                }
            }
        }
    }
    notes
}

/// Outbound filter: sanitizes every item again (idempotent). The model's own
/// messages get known values replaced too: it can reconstruct a value it never
/// saw verbatim (from character codes, a reformatted number), and history must
/// not carry that value back out.
pub(super) struct Sanitize(pub(super) Arc<Engine>);

impl OutboundFilter for Sanitize {
    fn name(&self) -> &'static str {
        "sanitize"
    }

    fn apply(&self, request: &mut Request) -> Vec<String> {
        let mut notes = Vec::new();
        let mut st = self.0.lock();
        for (index, item) in request.items.iter_mut().enumerate() {
            let locator = match &*item {
                Item::ToolResult { call_id, .. } => {
                    format!("history item {} · result for call {call_id}", index + 1)
                }
                _ => format!("history item {} · user content", index + 1),
            };
            match item {
                Item::User { text } | Item::ToolResult { content: text, .. } => {
                    let cleaned = self.0.sanitize(&mut st, text, "outbound", false);
                    let (cleaned, spans) = st.overlap.redact(&cleaned);
                    let (cleaned, _) = Engine::ip_redact(&mut st, &cleaned);
                    if cleaned != *text {
                        notes.push(format!(
                            "{}; detector/copy/protected-content check ({spans} copied span(s))",
                            change_note(&st.vault, &locator, text, &cleaned)
                        ));
                        *text = cleaned;
                    }
                }
                // An image is routed before it joins the conversation; one
                // that was not routed to the frontier is dropped here (and
                // the check refuses any that remains).
                Item::Images { images, .. } => {
                    let before = images.len();
                    images.retain(|img| st.frontier_images.contains(&img.sha256));
                    if images.len() < before {
                        notes.push(format!(
                            "withheld {} image(s) not approved for the frontier",
                            before - images.len()
                        ));
                    }
                }
                // The model's own text is not scanned by the detectors (its
                // test data is its own); the second pass replaces known values.
                Item::Assistant { .. } => {}
            }
        }
        notes.extend(replace_known(&st.vault, request));
        for note in &mut notes {
            scrub(&st.vault, note, 0);
        }
        notes
    }

    fn withhold(&self, request: &mut Request) -> usize {
        let st = self.0.lock();
        withhold_parts(&st, request)
    }
}

/// Replaces each part of `request` that holds a value the check refuses (or
/// an image not routed to the frontier) by [`PART_WITHHELD`]; reasoning and
/// replayed reasoning are dropped, a call's arguments become `{}`. Returns how
/// many parts were withheld.
fn withhold_parts(st: &State, request: &mut Request) -> usize {
    let vault = &st.vault;
    let withhold = |s: &mut String| -> usize {
        match holds_value(vault, s) {
            true => {
                *s = PART_WITHHELD.to_owned();
                1
            }
            false => 0,
        }
    };
    let mut n = withhold(&mut request.system);
    for t in &mut request.tools {
        n += withhold(&mut t.description);
    }
    let declared: HashSet<String> = request.tools.iter().map(|t| t.name.clone()).collect();
    for item in &mut request.items {
        match item {
            Item::User { text } | Item::ToolResult { content: text, .. } => n += withhold(text),
            Item::Images { images, .. } => {
                let before = images.len();
                images.retain(|img| st.frontier_images.contains(&img.sha256));
                n += before - images.len();
            }
            Item::Assistant {
                text,
                reasoning,
                tool_calls,
                replay,
            } => {
                let before = n;
                n += withhold(text);
                if reasoning.as_deref().is_some_and(|r| holds_value(vault, r)) {
                    *reasoning = None;
                    n += 1;
                }
                for call in tool_calls.iter_mut() {
                    if !declared.contains(&call.name) {
                        n += withhold(&mut call.name);
                    }
                    if holds_value(vault, &call.raw_arguments)
                        || value_holds_value(vault, &Value::Object(call.arguments.clone()))
                    {
                        call.raw_arguments = "{}".into();
                        call.arguments = Map::new();
                        n += 1;
                    }
                }
                if replay
                    .as_ref()
                    .is_some_and(|r| replay_holds_value(vault, &r.parts))
                {
                    *replay = None;
                    n += 1;
                } else if n > before {
                    *replay = None;
                }
            }
        }
    }
    n
}

/// Final check: no value in the vault may appear in any text of the body
/// that is not its framing.
pub(super) struct NoKnownValues(pub(super) Arc<Engine>);

/// [`texts_in`] `body`, except its strings and keys that are framing.
fn content_texts(v: &Value, framing: &Framing, visit: &mut dyn FnMut(&str) -> bool) -> bool {
    match v {
        Value::String(s) => !framing.contains(s) && texts_of(s, 0, visit),
        Value::Array(a) => a.iter().any(|x| content_texts(x, framing, visit)),
        Value::Object(m) => m
            .iter()
            .any(|(k, x)| (!framing.contains(k) && visit(k)) || content_texts(x, framing, visit)),
        _ => false,
    }
}

impl OutboundCheck for NoKnownValues {
    fn name(&self) -> &'static str {
        "known-values"
    }

    /// Without the framing every string of `body` is content.
    fn check(&self, body: &Value) -> Result<(), String> {
        self.check_framed(body, &Framing::default())
    }

    fn check_framed(&self, body: &Value, framing: &Framing) -> Result<(), String> {
        let st = self.0.lock();
        let mut found = None;
        content_texts(body, framing, &mut |t| {
            found = st
                .vault
                .disclosed_in(t)
                .map(|(_, e)| format!("a {} value from {}", e.kind.tag(), e.origin));
            found.is_some()
        });
        match found {
            Some(what) => Err(format!("{what} would have been sent")),
            None => Ok(()),
        }
    }

    /// Only images routed to the frontier may be sent.
    fn check_images(&self, digests: &[String]) -> Result<(), String> {
        let st = self.0.lock();
        match digests.iter().find(|d| !st.frontier_images.contains(*d)) {
            Some(d) => Err(format!(
                "an image not approved for the frontier (sha256 {}) would have been sent",
                &d[..d.len().min(12)]
            )),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{Detectors, scan};
    use crate::gate::framing_request;
    use crate::model::{ToolCall, ToolSpec};
    use crate::view::{Presenter, Source};
    use declass_provider::Dialect;
    use declass_provider::image::redact;
    use declass_provider::types::Replay;
    use serde_json::json;

    const KEY: &str = "sk_live_Qm8vT2xW9pL4nR7kZ3cY6bH1";
    const EMAIL: &str = "amelia.velanwick42@mailbox-311.net";
    const MODEL: &str = "glm-test-4.6";
    /// A path the entropy detector takes for a secret where it is quoted
    /// relative (`.{PATH}.json`). In the run that found DECLASS-2026-019 it was a
    /// plain test-suite path (`/test/test-suite/groups/…/case000`), which is
    /// no longer one (tokens are judged by their parts); a path holding a
    /// random segment still is.
    const PATH: &str = "/spec/suite/k7Qx2LmZ9wRt4VbN8sYc3HjD6gFa5UeP/case031";
    const DIALECTS: [Dialect; 3] = [Dialect::Chat, Dialect::Anthropic, Dialect::Responses];

    fn engine() -> (tempfile::TempDir, Arc<Engine>) {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::open(d.path(), super::super::tests::policy(), None).unwrap();
        (d, e)
    }

    fn env(e: &Engine, text: &str) {
        e.present(
            &Source::File {
                path: ".env".into(),
                ranged: false,
            },
            text.as_bytes(),
        );
    }

    fn tools() -> Vec<ToolSpec> {
        ["read_file", "run_command"]
            .into_iter()
            .map(|name| ToolSpec {
                name: name.into(),
                description: format!("{name}: see the arguments."),
                parameters: json!({"type": "object",
                    "properties": {"arg": {"type": "string", "description": "the argument"}},
                    "required": ["arg"]}),
            })
            .collect()
    }

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        let Value::Object(arguments) = args.clone() else {
            unreachable!()
        };
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            raw_arguments: args.to_string(),
        }
    }

    fn assistant(text: &str, reasoning: Option<&str>, calls: Vec<ToolCall>) -> Item {
        Item::Assistant {
            text: text.into(),
            reasoning: reasoning.map(str::to_owned),
            tool_calls: calls,
            replay: None,
        }
    }

    fn result(id: &str, content: &str) -> Item {
        Item::ToolResult {
            call_id: id.into(),
            content: content.into(),
        }
    }

    /// Signed reasoning replayed with an Anthropic turn, holding `thinking`.
    fn signed(thinking: &str, text: &str) -> Replay {
        Replay {
            dialect: "anthropic".into(),
            parts: vec![
                Part::Opaque {
                    block: json!({"type": "thinking", "thinking": thinking, "signature": "c2ln"}),
                },
                Part::Text { bytes: text.len() },
            ],
        }
    }

    /// The gate's check of `req` in every dialect, with the body's framing.
    fn gate_check(check: &dyn OutboundCheck, req: &Request) -> Result<(), String> {
        for dialect in DIALECTS {
            let body = redact(&dialect.build_body(MODEL, req, true)).0;
            let framing =
                Framing::of(&redact(&dialect.build_body(MODEL, &framing_request(req), true)).0);
            check
                .check_framed(&body, &framing)
                .map_err(|e| format!("{dialect:?}: {e}"))?;
        }
        Ok(())
    }

    fn bodies(req: &Request) -> String {
        DIALECTS
            .iter()
            .map(|d| d.build_body(MODEL, req, true).to_string())
            .collect()
    }

    #[test]
    fn a_value_the_filter_finds_in_a_later_item_is_replaced_in_earlier_ones() {
        // DECLASS-2026-019, from an XL calibration run: context masking replaced
        // an old result by a stub quoting the call's command; the filter's
        // detectors took a test-suite path in it for a secret, but the same
        // path in the model's own tool calls before it had already been
        // passed, and the final check blocked the request and ended the run.
        let (_d, e) = engine();
        let command = format!("node -e \"const c=require('.{PATH}.json'); console.log(c.length)\"");
        assert!(
            scan(&command, Detectors::default())
                .iter()
                .any(|f| &command[f.start..f.end] == PATH),
            "precondition: a detector takes the path for a secret"
        );
        let first = call("c1", "run_command", json!({ "command": command }));
        let stub = format!(
            "[masked: run_command `{command}`, ~900 tokens removed to save context; repeat the call if you need it]"
        );
        let mut req = Request {
            system: "You are the engineer.".into(),
            tools: tools(),
            items: vec![
                Item::User {
                    text: "Fix the date formatting.".into(),
                },
                Item::Assistant {
                    text: "Checking the cases.".into(),
                    reasoning: None,
                    tool_calls: vec![first.clone()],
                    replay: Some(signed("look at the cases", "Checking the cases.")),
                },
                result("c1", &stub),
                assistant(
                    &format!("The cases in .{PATH}.json fail."),
                    Some(&format!("{PATH} has 19 cases")),
                    vec![call("c2", "run_command", json!({ "command": command }))],
                ),
                result("c2", "exit code 1\n--- stdout ---\n19\n"),
            ],
            ..Request::default()
        };
        assert!(!e.lock().vault.contains(PATH));
        let (filter, check) = e.outbound();
        let notes = filter.apply(&mut req);
        assert!(e.lock().vault.contains(PATH), "found in the stub");
        gate_check(check.as_ref(), &req).unwrap();
        assert!(!bodies(&req).contains(PATH), "{}", bodies(&req));
        let Item::Assistant {
            tool_calls, replay, ..
        } = &req.items[1]
        else {
            unreachable!()
        };
        assert!(
            tool_calls[0].arguments["command"]
                .as_str()
                .unwrap()
                .contains("⟨secret:secret#1⟩"),
            "{tool_calls:?}"
        );
        assert!(replay.is_none(), "an edited turn is not replayed signed");
        assert!(
            notes
                .iter()
                .any(|n| n.contains("call c1") && n.contains("arguments.command")),
            "{notes:?}"
        );
    }

    #[test]
    fn filtering_details_locate_the_field_line_and_origin_without_copying_values() {
        let (_d, e) = engine();
        env(&e, &format!("PAYMENTS_API_KEY={KEY}\n"));
        let (filter, check) = e.outbound();
        let mut req = Request {
            tools: tools(),
            items: vec![assistant(
                "",
                None,
                vec![call(
                    "edit42",
                    "edit_file",
                    json!({
                        "path": "src/client.rs",
                        "edits": [{"old": "old", "new": format!("first line\nkey={KEY}\nlast line")}]
                    }),
                )],
            )],
            ..Request::default()
        };
        let original = req.clone();
        let notes = filter.apply(&mut req);
        let detail = notes.join("\n");
        for expected in [
            "history item 1",
            "call edit42",
            "edit_file",
            "src/client.rs",
            "arguments.edits[0].new",
            "line(s) 2",
            "⟨secret:PAYMENTS_API_KEY#1⟩",
            "first seen .env",
        ] {
            assert!(detail.contains(expected), "missing {expected}: {detail}");
        }
        assert!(!detail.contains(KEY));
        assert!(
            !detail.contains("first line"),
            "no source snippets are logged"
        );
        gate_check(check.as_ref(), &req).unwrap();
        assert!(!bodies(&req).contains(KEY));
        assert!(filter.apply(&mut req).is_empty());
        let mut next_request = original;
        assert_eq!(
            filter.apply(&mut next_request),
            notes,
            "history rechecks have stable descriptions"
        );
    }

    #[test]
    fn a_vault_value_that_spells_the_wire_format_blocks_nothing() {
        // DECLASS-2026-020: `.env` values such as `required` or the model's name
        // occur in every request's framing (tool schemas, the model field),
        // where no filter can replace them.
        let (_d, e) = engine();
        env(
            &e,
            &format!("SSL_MODE=required\nDEFAULT_ROLE=assistant\nLLM_MODEL={MODEL}\n"),
        );
        let (filter, check) = e.outbound();
        let mut req = Request {
            system: "You are the engineer.".into(),
            tools: tools(),
            items: vec![
                Item::User {
                    text: format!("The client uses {MODEL}; TLS is required."),
                },
                assistant(
                    "Reading the config as the assistant.",
                    None,
                    vec![call("c1", "read_file", json!({"arg": "config.toml"}))],
                ),
                result("c1", "sslmode = \"required\""),
            ],
            ..Request::default()
        };
        let unfiltered = req.clone();
        filter.apply(&mut req);
        gate_check(check.as_ref(), &req).unwrap();
        // Content is still filtered: the values are placeholders there.
        let Item::User { text } = &req.items[0] else {
            unreachable!()
        };
        assert!(
            !text.contains(MODEL) && !text.contains("required"),
            "{text}"
        );
        // The framing is untouched.
        assert_eq!(req.tools[0].parameters, unfiltered.tools[0].parameters);
        // Without the framing, every body would be refused.
        let body = Dialect::Chat.build_body(MODEL, &req, true);
        assert!(check.check(&body).is_err());
        // Content that holds one is still refused.
        let leaked = Request {
            items: vec![Item::User {
                text: format!("model {MODEL}"),
            }],
            ..req.clone()
        };
        assert!(gate_check(check.as_ref(), &leaked).is_err());
    }

    #[test]
    fn the_system_prompt_tool_descriptions_and_replayed_reasoning_are_filtered() {
        let (_d, e) = engine();
        env(&e, &format!("PAYMENTS_API_KEY={KEY}\nOWNER={EMAIL}\n"));
        let (filter, check) = e.outbound();
        let mut described = tools();
        described[1].description = format!("Runs commands; the owner is {EMAIL}.");
        let mut req = Request {
            system: format!("Checks run as `API_KEY={KEY} npm test`."),
            tools: described,
            items: vec![Item::Assistant {
                text: "Done.".into(),
                reasoning: Some("nothing to add".into()),
                tool_calls: Vec::new(),
                replay: Some(signed(&format!("the key is {KEY}"), "Done.")),
            }],
            ..Request::default()
        };
        assert!(gate_check(check.as_ref(), &req).is_err());
        let notes = filter.apply(&mut req);
        gate_check(check.as_ref(), &req).unwrap();
        assert!(req.system.contains("⟨secret:PAYMENTS_API_KEY#1⟩"));
        assert!(!req.tools[1].description.contains(EMAIL));
        assert!(matches!(
            &req.items[0],
            Item::Assistant { replay: None, .. }
        ));
        assert_eq!(notes.len(), 3, "{notes:?}");
    }

    #[test]
    fn json_is_read_by_the_filter_as_the_check_reads_it() {
        // A value spelled with JSON escapes is only visible once decoded; the
        // check decodes JSON inside strings, so the filter does too.
        let (_d, e) = engine();
        env(&e, &format!("PAYMENTS_API_KEY={KEY}\n"));
        let (filter, check) = e.outbound();
        let escaped: String = KEY
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect();
        let doc = format!("{{\"config\": {{\"key\": \"{escaped}\", \"retries\": 3}}}}");
        let nested = serde_json::to_string(&json!({"arg": doc})).unwrap();
        let mut req = Request {
            tools: tools(),
            items: vec![
                assistant(
                    "",
                    None,
                    vec![{
                        let mut c = call("c1", "run_command", json!({}));
                        c.raw_arguments = nested;
                        c
                    }],
                ),
                result("c1", &doc),
            ],
            ..Request::default()
        };
        assert!(!doc.contains(KEY));
        assert!(
            gate_check(check.as_ref(), &req).is_err(),
            "the check decodes"
        );
        filter.apply(&mut req);
        gate_check(check.as_ref(), &req).unwrap();
        let Item::ToolResult { content, .. } = &req.items[1] else {
            unreachable!()
        };
        let v: Value = serde_json::from_str(content).unwrap();
        assert_eq!(v["config"]["key"], "⟨secret:PAYMENTS_API_KEY#1⟩");
        assert_eq!(v["config"]["retries"], 3);
    }

    #[test]
    fn withholding_replaces_each_part_that_still_holds_a_value() {
        let (_d, e) = engine();
        env(&e, &format!("PAYMENTS_API_KEY={KEY}\nOWNER={EMAIL}\n"));
        let (filter, check) = e.outbound();
        // As if the filter had missed them: values in every part.
        let mut req = Request {
            system: format!("key {KEY}"),
            tools: tools(),
            items: vec![
                Item::User {
                    text: format!("mail {EMAIL}"),
                },
                Item::Assistant {
                    text: format!("using {KEY}"),
                    reasoning: Some(format!("{EMAIL} owns it")),
                    tool_calls: vec![
                        call(
                            "c1",
                            "run_command",
                            json!({"arg": format!("curl -u {KEY}")}),
                        ),
                        call("c2", "read_file", json!({"arg": "README.md"})),
                    ],
                    replay: Some(signed("fine", "")),
                },
                result("c1", &format!("401 for {KEY}")),
                result("c2", "# Readme"),
            ],
            ..Request::default()
        };
        assert!(gate_check(check.as_ref(), &req).is_err());
        assert_eq!(filter.withhold(&mut req), 6);
        gate_check(check.as_ref(), &req).unwrap();
        assert_eq!(req.system, PART_WITHHELD);
        let Item::Assistant {
            text,
            reasoning,
            tool_calls,
            replay,
        } = &req.items[1]
        else {
            unreachable!()
        };
        assert_eq!(text, PART_WITHHELD);
        assert!(reasoning.is_none() && replay.is_none());
        assert_eq!(tool_calls[0].raw_arguments, "{}");
        assert_eq!(
            tool_calls[0].name, "run_command",
            "a declared tool keeps its name"
        );
        assert_eq!(tool_calls[1].raw_arguments, r#"{"arg":"README.md"}"#);
        assert!(matches!(&req.items[3], Item::ToolResult { content, .. } if content == "# Readme"));
        // Nothing left: a second pass withholds nothing.
        assert_eq!(filter.withhold(&mut req), 0);
    }
}
