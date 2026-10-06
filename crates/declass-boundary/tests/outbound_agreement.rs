// SPDX-License-Identifier: GPL-3.0-or-later
//! Property: the outbound filter leaves nothing the final check refuses.
//!
//! For any request, the check passes what the filter returns, in the body of
//! every wire dialect and with that body's framing, as the gate runs them
//! (filter ∘ check never blocks). The requests carry values in every part a
//! request has: the system prompt, tool descriptions, the operator's text,
//! tool results (plain, and JSON with the value spelled in `\u` escapes),
//! the model's text and reasoning, tool-call arguments (as a string, as a key,
//! as a JSON document inside a string), the name of a call to an undeclared
//! tool, replayed reasoning blocks, and masking stubs that quote a call's
//! command. The values are known to the vault from a `.env` (any characters,
//! and words of the wire format itself: roles, block types, schema keywords,
//! the model's name), or found by the detectors only in the filter's own pass
//! (provider-shaped keys, emails, test-suite paths the entropy detector takes
//! for secrets), anywhere before or after the item they are found in.
//!
//! Also asserted: the check still refuses a value the vault knows in content
//! (it stays fail-closed), and the stricter pass the gate falls back on
//! (`withhold`) alone clears any request of the values the vault knows.
//!
//! Found DECLASS-2026-019 (a value found in a later item stayed in earlier ones)
//! and DECLASS-2026-020 (a value spelling the wire format blocked every request).

use declass_boundary::engine::Engine;
use declass_boundary::gate::{Framing, OutboundCheck, framing_request};
use declass_boundary::model::{Item, Request, ToolCall, ToolSpec};
use declass_boundary::policy::Policy;
use declass_boundary::view::{Presenter, Source};
use declass_provider::Dialect;
use declass_provider::image::redact;
use declass_provider::types::{Part, Replay};
use proptest::prelude::*;
use serde_json::{Map, Value, json};
use std::sync::Arc;

const DEFAULT_CASES: u32 = 32;
const MODEL: &str = "glm-test-4.6";
const DIALECTS: [Dialect; 3] = [Dialect::Chat, Dialect::Anthropic, Dialect::Responses];
/// Assistant turns in each request.
const TURNS: usize = 4;

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

/// `.env` values: any characters a value may hold, or a word the wire format
/// itself uses.
fn env_value() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[A-Za-z0-9][A-Za-z0-9!@%+/.^*\\\\\"-]{6,14}[A-Za-z0-9]",
        1 => proptest::sample::select(vec![
            "required", "assistant", "function", "properties", "description",
            "parameters", "content", "tool_calls", "arguments", "function_call",
            "tool_use", "input_text", "reasoning_content", "thinking", "signature",
            "tool_result", "instructions", MODEL,
        ])
        .prop_map(str::to_owned),
    ]
}

/// Values the detectors find in outbound text, not in the vault before.
fn found_value() -> impl Strategy<Value = String> {
    prop_oneof![
        "sk_live_[A-Za-z0-9]{24}",
        "ghp_[A-Za-z0-9]{36}",
        "[a-z]{4,8}\\.[a-z]{3,6}[0-9]{2}@[a-z]{4,8}-[0-9]{3}\\.net",
        "/spec/[a-z]{4,6}/groups/function-[a-z]{4,6}[A-Z][a-z]{4,7}/case0[0-9]{2}",
    ]
}

/// Where a value goes in the request.
#[derive(Debug, Clone, Copy)]
enum Slot {
    System,
    ToolDescription,
    User,
    ResultText,
    /// A JSON tool result, the value spelled in `\u` escapes.
    ResultJson,
    AssistantText,
    Reasoning,
    Argument,
    ArgumentKey,
    /// A JSON document inside an argument string.
    ArgumentJson,
    UndeclaredCall,
    Replay,
    /// The call's command, and its result a masking stub quoting it.
    Stub,
}

const SLOTS: [Slot; 13] = [
    Slot::System,
    Slot::ToolDescription,
    Slot::User,
    Slot::ResultText,
    Slot::ResultJson,
    Slot::AssistantText,
    Slot::Reasoning,
    Slot::Argument,
    Slot::ArgumentKey,
    Slot::ArgumentJson,
    Slot::UndeclaredCall,
    Slot::Replay,
    Slot::Stub,
];

/// Every character as a `\u` escape (JSON text that decodes to `s`).
fn escaped(s: &str) -> String {
    s.encode_utf16().map(|u| format!("\\u{u:04x}")).collect()
}

#[derive(Default, Clone)]
struct Turn {
    text: String,
    reasoning: String,
    command: String,
    argument_keys: Vec<String>,
    argument_json: String,
    undeclared: Vec<String>,
    replay: Option<String>,
    result: String,
    result_json: Vec<String>,
    stub: bool,
}

fn tools(description: &str) -> Vec<ToolSpec> {
    ["read_file", "run_command"]
        .into_iter()
        .map(|name| ToolSpec {
            name: name.into(),
            description: format!("{name}: see the arguments.{description}"),
            parameters: json!({"type": "object",
                "properties": {"command": {"type": "string", "description": "what to run"}},
                "required": ["command"]}),
        })
        .collect()
}

fn build(placed: &[(Slot, usize, String)]) -> Request {
    let mut system = String::from("You are the engineer.");
    let mut description = String::new();
    let mut user = String::from("Fix the export.");
    let mut turns = vec![Turn::default(); TURNS];
    for (slot, at, v) in placed {
        let t = &mut turns[at % TURNS];
        match slot {
            Slot::System => system.push_str(&format!(" Note {v}.")),
            Slot::ToolDescription => description.push_str(&format!(" Uses {v}.")),
            Slot::User => user.push_str(&format!(" See {v}.")),
            Slot::ResultText => t.result.push_str(&format!("line {v}\n")),
            Slot::ResultJson => t.result_json.push(escaped(v)),
            Slot::AssistantText => t.text.push_str(&format!("Found {v}. ")),
            Slot::Reasoning => t.reasoning.push_str(&format!("{v} matters. ")),
            Slot::Argument => t.command.push_str(&format!(" {v}")),
            Slot::ArgumentKey => t.argument_keys.push(v.clone()),
            Slot::ArgumentJson => t.argument_json.push_str(&format!("{v} ")),
            Slot::UndeclaredCall => t.undeclared.push(format!("call_{v}")),
            Slot::Replay => t.replay = Some(format!("the value is {v}")),
            Slot::Stub => {
                t.command.push_str(&format!(" .{v}.json"));
                t.stub = true;
            }
        }
    }
    let mut items = vec![Item::User { text: user }];
    for (n, t) in turns.into_iter().enumerate() {
        let id = format!("c{n}");
        let command = format!("node -e \"require('./x')\"{}", t.command);
        let mut args = Map::new();
        args.insert("command".into(), Value::String(command.clone()));
        for k in &t.argument_keys {
            args.insert(k.clone(), json!(1));
        }
        if !t.argument_json.is_empty() {
            args.insert(
                "input".into(),
                Value::String(json!({"doc": t.argument_json}).to_string()),
            );
        }
        let mut calls = vec![ToolCall {
            id: id.clone(),
            name: "run_command".into(),
            raw_arguments: Value::Object(args.clone()).to_string(),
            arguments: args,
        }];
        for (k, name) in t.undeclared.iter().enumerate() {
            calls.push(ToolCall {
                id: format!("{id}-{k}"),
                name: name.clone(),
                arguments: Map::new(),
                raw_arguments: "{}".into(),
            });
        }
        let text = t.text;
        items.push(Item::Assistant {
            replay: t.replay.map(|thinking| Replay {
                dialect: "anthropic".into(),
                parts: vec![
                    Part::Opaque {
                        block: json!({"type": "thinking", "thinking": thinking, "signature": "c2ln"}),
                    },
                    Part::Text { bytes: text.len() },
                ],
            }),
            text,
            reasoning: Some(t.reasoning),
            tool_calls: calls.clone(),
        });
        let mut content = if t.stub {
            format!(
                "[masked: run_command `{command}`, ~900 tokens removed to save context; repeat the call if you need it]"
            )
        } else {
            format!("exit code 0\n--- stdout ---\n{}", t.result)
        };
        if !t.result_json.is_empty() {
            let values: Vec<String> = t.result_json.iter().map(|e| format!("\"{e}\"")).collect();
            content = format!(
                "{{\"rows\": [{}], \"text\": {}}}",
                values.join(", "),
                Value::String(content)
            );
        }
        for c in &calls {
            items.push(Item::ToolResult {
                call_id: c.id.clone(),
                content: if c.id == id {
                    content.clone()
                } else {
                    "unknown tool".into()
                },
            });
        }
    }
    Request {
        system,
        items,
        tools: tools(&description),
        max_output_tokens: Some(32_768),
        ..Request::default()
    }
}

fn engine(values: &[String]) -> (tempfile::TempDir, Arc<Engine>) {
    let d = tempfile::tempdir().unwrap();
    let policy = Policy {
        sensitive_globs: vec![".env*".into()],
        command_output_sensitive: true,
        secret_sinks: vec![".env*".into()],
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 2000,
        ..Policy::default()
    };
    let e = Engine::open(d.path(), policy, None).unwrap();
    let env: String = values
        .iter()
        .enumerate()
        .map(|(i, v)| format!("V{i}={v}\n"))
        .collect();
    e.present(
        &Source::File {
            path: ".env".into(),
            ranged: false,
        },
        env.as_bytes(),
    );
    (d, e)
}

/// The gate's check of `req` in `dialect`, with the body's framing.
fn gate_check(check: &dyn OutboundCheck, dialect: Dialect, req: &Request) -> Result<(), String> {
    let body = redact(&dialect.build_body(MODEL, req, true)).0;
    let framing = Framing::of(&redact(&dialect.build_body(MODEL, &framing_request(req), true)).0);
    check.check_framed(&body, &framing)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn the_check_passes_what_the_filter_returns(
        env in proptest::collection::vec(env_value(), 1..5),
        found in proptest::collection::vec(found_value(), 0..3),
        env_places in proptest::collection::vec((0..SLOTS.len(), 0..TURNS), 1..10),
        // A found value is placed once where the detectors see it (a tool
        // result, the operator's text, a stub) and elsewhere at random.
        found_places in proptest::collection::vec(((0..3usize, 0..TURNS), (0..SLOTS.len(), 0..TURNS)), 0..3),
    ) {
        let (_d, e) = engine(&env);
        let (filter, check) = e.outbound();
        let mut placed: Vec<(Slot, usize, String)> = env_places
            .iter()
            .enumerate()
            .map(|(i, &(s, at))| (SLOTS[s], at, env[i % env.len()].clone()))
            .collect();
        if !found.is_empty() {
            for (i, &((seen, seen_at), (s, at))) in found_places.iter().enumerate() {
                let v = &found[i % found.len()];
                let seen = [Slot::ResultText, Slot::User, Slot::Stub][seen];
                placed.push((seen, seen_at, v.clone()));
                placed.push((SLOTS[s], at, v.clone()));
            }
        }
        let unfiltered = build(&placed);
        let mut req = unfiltered.clone();
        filter.apply(&mut req);
        for dialect in DIALECTS {
            let verdict = gate_check(check.as_ref(), dialect, &req);
            prop_assert!(
                verdict.is_ok(),
                "{:?}: the check refused the filtered request: {:?}\n{}",
                dialect,
                verdict,
                dialect.build_body(MODEL, &req, true)
            );
        }
        // The framing was not rewritten: tools keep their names and schemas.
        prop_assert_eq!(&req.tools[0].parameters, &unfiltered.tools[0].parameters);
        // Fail-closed: a value the vault knows is still refused in content.
        for v in env.iter().filter(|v| v.len() >= 6) {
            let leaked = Request {
                items: vec![Item::User { text: format!("see {v} here") }],
                ..req.clone()
            };
            for dialect in DIALECTS {
                prop_assert!(
                    gate_check(check.as_ref(), dialect, &leaked).is_err(),
                    "{:?}: {:?} not refused in content", dialect, v
                );
            }
        }
        // The stricter pass alone clears the unfiltered request.
        let mut strict = unfiltered;
        filter.withhold(&mut strict);
        for dialect in DIALECTS {
            let verdict = gate_check(check.as_ref(), dialect, &strict);
            prop_assert!(verdict.is_ok(), "{:?}: withheld request refused: {:?}", dialect, verdict);
        }
    }
}
