// SPDX-License-Identifier: GPL-3.0-or-later
//! The run's cost ledger: where frontier input went, by how tool results were
//! shown, and what the local side did.
//!
//! Method (estimates, not provider counts): a tool result costs its size once
//! when it is added and again with every later request, because each request
//! re-sends the conversation (cached or not). For each request the ledger adds
//! the current size of every result in it (masked results count as their
//! stubs) to that result's class: `carried_tokens`. Sizes are serialized
//! bytes at 3.5 bytes per token, the context manager's estimate. Input dollars
//! (input, cache read and cache write at list price) are split between classes
//! in proportion to `carried_tokens` over `request_tokens`, the estimated size
//! of all requests including the system prompt, the task and the model's own
//! messages; the rest of the input is not attributed to any class.

use crate::context::estimate;
use duet_boundary::local::CallStats;
use duet_boundary::model::{Item, ToolCall, Usage};
use duet_boundary::view::ViewClass;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Tool output that reports a command was denied by the sandbox.
const DENIAL: &str = "Operation not permitted";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClassCost {
    /// Tool results shown this way.
    pub results: u64,
    /// Their estimated size when added.
    pub added_tokens: u64,
    /// Their estimated size summed over every request that carried them.
    pub carried_tokens: u64,
    /// Share of the run's input dollars.
    pub input_usd: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    /// Per `ViewClass` (snake_case name): what tool results of that class cost.
    pub by_class: BTreeMap<ViewClass, ClassCost>,
    /// Estimated size of all frontier requests.
    pub request_tokens: u64,
    pub ask_local_calls: u64,
    pub ask_local_questions: u64,
    pub sensitive_data_commands: u64,
    /// Tool results reporting that the sandbox denied access.
    pub sandbox_denials: u64,
    /// Frontier dollars at list price, split into input (including cache) and output.
    pub input_usd: f64,
    pub output_usd: f64,
    /// Of `input_usd` and `output_usd`: the estimated cost of attempts that
    /// failed after output started (retried infrastructure failures).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed_attempts_usd: f64,
    /// What the local model did (hybrid runs), including its busy seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<CallStats>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn tokens_of(text: &str) -> u64 {
    let bytes = serde_json::to_string(text).map_or(text.len(), |s| s.len());
    (bytes as u64).div_ceil(7) * 2
}

impl Ledger {
    /// A request about to be sent: every result in it is carried once more.
    pub fn on_request(
        &mut self,
        items: &[Item],
        system: &str,
        classes: &HashMap<String, ViewClass>,
    ) {
        self.request_tokens += estimate(items, system);
        for item in items {
            if let Item::ToolResult { call_id, content } = item {
                let class = classes.get(call_id).copied().unwrap_or(ViewClass::Raw);
                self.by_class.entry(class).or_default().carried_tokens += tokens_of(content);
            }
        }
    }

    /// A response's usage, priced by `price`.
    pub fn on_usage(&mut self, usage: &Usage, price: &dyn Fn(&Usage) -> f64) {
        let total = price(usage);
        let input = price(&Usage {
            output: 0,
            reasoning: 0,
            ..*usage
        });
        self.input_usd += input;
        self.output_usd += (total - input).max(0.0);
    }

    /// Estimated usage of attempts that failed mid-stream, priced by `price`.
    /// Disjoint from the billed usage given to [`Ledger::on_usage`].
    pub fn on_failed_usage(&mut self, usage: &Usage, price: &dyn Fn(&Usage) -> f64) {
        let before = self.input_usd + self.output_usd;
        self.on_usage(usage, price);
        self.failed_attempts_usd += self.input_usd + self.output_usd - before;
    }

    /// A tool result added to the conversation.
    pub fn on_result(&mut self, call: &ToolCall, content: &str, class: ViewClass) {
        let c = self.by_class.entry(class).or_default();
        c.results += 1;
        c.added_tokens += tokens_of(content);
        match call.name.as_str() {
            "ask_local" => {
                self.ask_local_calls += 1;
                self.ask_local_questions +=
                    match call.arguments.get("questions").and_then(Value::as_array) {
                        Some(qs) => qs.len() as u64,
                        None => u64::from(call.arguments.contains_key("question")),
                    };
            }
            "run_command"
                if call
                    .arguments
                    .get("sensitive_data")
                    .and_then(Value::as_bool)
                    .unwrap_or(false) =>
            {
                self.sensitive_data_commands += 1;
            }
            _ => {}
        }
        if content.contains(DENIAL) {
            self.sandbox_denials += 1;
        }
    }

    /// Splits input dollars between classes; call once the run has ended.
    pub fn finish(&mut self) {
        let total = self.request_tokens.max(1) as f64;
        for c in self.by_class.values_mut() {
            c.input_usd = self.input_usd * c.carried_tokens as f64 / total;
        }
    }
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
            raw_arguments: String::new(),
            arguments,
        }
    }

    #[test]
    fn results_are_charged_to_their_class_for_every_request_that_carries_them() {
        let mut l = Ledger::default();
        let mut classes = HashMap::new();
        let mut items = vec![Item::User {
            text: "task".into(),
        }];
        l.on_request(&items, "s", &classes);
        assert!(l.by_class.is_empty());

        let read = call("a", "read_file", json!({"path": "src/lib.rs"}));
        let body = "x".repeat(700);
        items.push(Item::Assistant {
            text: String::new(),
            reasoning: None,
            replay: None,
            tool_calls: vec![read.clone()],
        });
        items.push(Item::ToolResult {
            call_id: "a".into(),
            content: body.clone(),
        });
        classes.insert("a".to_owned(), ViewClass::BulkyHandle);
        l.on_result(&read, &body, ViewClass::BulkyHandle);
        // Carried by the next three requests.
        for _ in 0..3 {
            l.on_request(&items, "s", &classes);
        }
        let bulky = &l.by_class[&ViewClass::BulkyHandle];
        assert_eq!(bulky.results, 1);
        assert_eq!(bulky.added_tokens, tokens_of(&body));
        assert_eq!(bulky.carried_tokens, 3 * tokens_of(&body));
        // A result without a recorded class counts as raw.
        items.push(Item::ToolResult {
            call_id: "b".into(),
            content: "y".repeat(70),
        });
        l.on_request(&items, "s", &classes);
        assert_eq!(
            l.by_class[&ViewClass::Raw].carried_tokens,
            tokens_of(&"y".repeat(70))
        );

        l.on_usage(
            &Usage {
                input: 1000,
                output: 100,
                ..Usage::default()
            },
            &|u: &Usage| (u.input + 10 * u.output) as f64 / 1e6,
        );
        l.finish();
        assert!((l.input_usd - 0.001).abs() < 1e-12 && (l.output_usd - 0.001).abs() < 1e-12);
        let share: f64 = l.by_class.values().map(|c| c.input_usd).sum();
        assert!(share > 0.0 && share <= l.input_usd + 1e-12, "{share}");
    }

    #[test]
    fn counts_local_questions_sensitive_commands_and_denials() {
        let mut l = Ledger::default();
        let ask = call(
            "a",
            "ask_local",
            json!({"handle": "h1", "questions": ["a", "b", "c"]}),
        );
        l.on_result(&ask, "1. ...", ViewClass::LocalAnswer);
        let ask1 = call("b", "ask_local", json!({"handle": "h1", "question": "a"}));
        l.on_result(&ask1, "...", ViewClass::LocalAnswer);
        let held = call(
            "c",
            "run_command",
            json!({"command": "cargo run", "sensitive_data": true}),
        );
        l.on_result(&held, "h2 (output ...)", ViewClass::HandleSummary);
        let denied = call("d", "run_command", json!({"command": "cat data/a.csv"}));
        l.on_result(
            &denied,
            "exit code 1\ncat: data/a.csv: Operation not permitted\n",
            ViewClass::Raw,
        );
        let plain = call("e", "finish", Value::Object(Map::new()));
        l.on_result(&plain, "finished", ViewClass::Raw);
        assert_eq!((l.ask_local_calls, l.ask_local_questions), (2, 4));
        assert_eq!(l.sensitive_data_commands, 1);
        assert_eq!(l.sandbox_denials, 1);
        assert_eq!(l.by_class[&ViewClass::LocalAnswer].results, 2);
        assert_eq!(l.by_class[&ViewClass::Raw].results, 2);
    }

    #[test]
    fn serializes_classes_by_name() {
        let mut l = Ledger::default();
        l.by_class
            .insert(ViewClass::BulkyHandle, ClassCost::default());
        let v = serde_json::to_value(&l).unwrap();
        assert!(v["by_class"]["bulky_handle"].is_object(), "{v}");
        let back: Ledger = serde_json::from_value(v).unwrap();
        assert_eq!(back, l);
    }
}
