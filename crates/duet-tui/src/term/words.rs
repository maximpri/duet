// SPDX-License-Identifier: GPL-3.0-or-later
//! How duet's work reads for the operator, as words: the progress line each
//! transcript entry adds (tool calls, tool errors, what the boundary withheld,
//! context masking, interrupts, sub-agents, steering) and how a turn ended.
//! Shared by the workspace, the line-by-line session and `duet run`'s progress.

use duet_agent::TurnEnd;
use duet_agent::transcript::Entry;
use duet_boundary::model::Item;
use duet_boundary::view::ViewClass;
use std::collections::HashMap;

/// How a turn's end reads in the conversation.
pub fn describe_end(end: &TurnEnd) -> String {
    let indent = |text: &str| text.trim().replace('\n', "\n      ");
    match end {
        TurnEnd::Replied { message } => format!("duet: {}", indent(message)),
        TurnEnd::Asked { question, options } => {
            let mut text = format!("duet asks: {}", indent(question));
            if let Some(options) = options {
                text.push_str(&format!("\n      question: {}", indent(&options.id)));
                if !options.choices.is_empty() {
                    text.push_str(&format!(
                        "\n      options: {}",
                        options
                            .choices
                            .iter()
                            .map(|c| indent(&c.label))
                            .collect::<Vec<_>>()
                            .join(" / ")
                    ));
                }
                for (i, choice) in options.choices.iter().enumerate() {
                    let recommended = if options.recommended.as_ref() == Some(&choice.id) {
                        " (recommended)"
                    } else {
                        ""
                    };
                    text.push_str(&format!(
                        "\n      {}. {}{}",
                        i + 1,
                        indent(&choice.label),
                        recommended
                    ));
                    if let Some(description) = &choice.description {
                        text.push_str(&format!("\n         {}", indent(description)));
                    }
                }
                if !options.choices.is_empty() {
                    text.push_str(&format!(
                        "\n      /answer {} N selects option N",
                        options.id
                    ));
                }
                if options.allow_freeform {
                    text.push_str(&format!(
                        "\n      /answer {} --text <answer> supplies your own answer",
                        options.id
                    ));
                }
            } else {
                text.push_str("\n      (your next message is the answer)");
            }
            text
        }
        TurnEnd::Completed { summary } => format!("duet finished: {}", indent(summary)),
        TurnEnd::Failed { reason } => format!(
            "this turn failed: {reason}\n      (the session is still open; your next message continues it)"
        ),
        TurnEnd::BudgetStopped { which } => format!(
            "this turn stopped: {which} reached (your next message continues within the limits)"
        ),
        TurnEnd::Interrupted => {
            "this turn was interrupted; your next message continues (duet is told)".into()
        }
        TurnEnd::Stopped => {
            "this turn stopped after a step, as you asked; your next message continues".into()
        }
    }
}

/// Characters of a progress line.
const LINE_CHARS: usize = 160;

pub fn clip(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > LINE_CHARS {
        format!("{}…", line.chars().take(LINE_CHARS).collect::<String>())
    } else {
        line.to_owned()
    }
}

fn held(class: ViewClass) -> &'static str {
    match class {
        ViewClass::Raw => "shown as is",
        ViewClass::Tokenized => "values replaced by placeholders",
        ViewClass::HandleSummary => "sensitive, held locally (a summary was sent)",
        ViewClass::LocalAnswer => "answered by the local model",
        ViewClass::BulkyHandle => "large, held locally (an outline was sent)",
        ViewClass::Protected => "protected source (only its interface was sent)",
    }
}

/// The progress lines one transcript entry adds: tool calls, tool errors,
/// what the boundary withheld, context masking and interrupts.
pub fn progress(
    entry: &Entry,
    names: &mut HashMap<String, String>,
    shown: &dyn Fn(&str) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    match entry {
        Entry::Item {
            item: Item::Assistant {
                text, tool_calls, ..
            },
        } => {
            let to_operator = |n: &str| n == "reply" || n == "ask_operator";
            if !text.trim().is_empty() && tool_calls.iter().any(|c| !to_operator(&c.name)) {
                out.push(format!("  ~ {}", clip(&shown(text))));
            }
            for c in tool_calls {
                names.insert(c.id.clone(), c.name.clone());
                if to_operator(&c.name) {
                    continue;
                }
                let arg = ["path", "command", "pattern", "dir", "question"]
                    .iter()
                    .find_map(|k| c.arguments.get(*k).and_then(|v| v.as_str()))
                    .map(|a| clip(&shown(a)))
                    .unwrap_or_default();
                let what = if c.name == "finish" {
                    "finish (running the checks)".to_owned()
                } else {
                    format!("{} {arg}", c.name)
                };
                out.push(format!("  · {}", what.trim_end()));
            }
        }
        Entry::Item {
            item: Item::ToolResult { call_id, content },
        } if content.starts_with("error:") || content.starts_with("checks failed") => {
            let tool = names.get(call_id).map_or("tool", String::as_str);
            out.push(format!("  ! {tool}: {}", clip(&shown(content))));
        }
        Entry::Shown { call_id, class } if *class != ViewClass::Raw => {
            let tool = names.get(call_id).map_or("tool", String::as_str);
            out.push(format!(
                "  ◦ withheld from the frontier: {tool} result {}",
                held(*class)
            ));
        }
        Entry::Usage { interventions, .. } => {
            for i in interventions {
                out.push(format!("  ◦ withheld from the frontier: {}", clip(i)));
            }
        }
        Entry::Masked { items, .. } => {
            out.push(format!("  ◦ context: {items} old tool result(s) shortened"))
        }
        Entry::Compacted {
            head,
            upto,
            tokens_before,
            tokens_after,
            ..
        } => out.push(format!(
            "  ◦ context: {} earlier item(s) condensed by the local model (~{tokens_before} → ~{tokens_after} tokens)",
            upto - head
        )),
        Entry::Interrupted { tool, .. } => out.push(format!("  ■ {tool} stopped")),
        Entry::SubagentStart {
            child, mode, task, ..
        } => out.push(format!(
            "  ⇢ sub-agent {child} ({mode}): {}",
            clip(&shown(task))
        )),
        // A sub-agent's own steps, under its id.
        Entry::Subagent { child, entry } => out.extend(
            progress(entry, names, shown)
                .into_iter()
                .map(|l| format!("  [{child}]{l}")),
        ),
        Entry::SubagentEnd {
            child,
            terminal,
            cost_usd,
            written,
            ..
        } => out.push(format!(
            "  ⇠ sub-agent {child} {} (${cost_usd:.4}, {} file(s) written)",
            terminal.state(),
            written.len()
        )),
        Entry::Steered {
            after_request,
            messages,
            ..
        } => {
            for m in messages {
                out.push(format!(
                    "  ▸ delivered to duet after request {after_request}: {}",
                    clip(&shown(m))
                ));
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_boundary::model::ToolCall;
    use serde_json::json;

    #[test]
    fn structured_question_text_has_explicit_id_choices_and_freeform_commands() {
        use duet_agent::questions::{QuestionOption, QuestionOptions};
        let options = QuestionOptions {
            id: "q7".into(),
            choices: vec![
                QuestionOption {
                    id: "unit".into(),
                    label: "unit".into(),
                    description: Some("No returned value".into()),
                },
                QuestionOption {
                    id: "u32".into(),
                    label: "u32".into(),
                    description: None,
                },
            ],
            allow_freeform: true,
            recommended: Some("unit".into()),
        };
        let text = describe_end(&TurnEnd::Asked {
            question: "Return type?".into(),
            options: Some(options.clone()),
        });
        for expected in [
            "question: q7",
            "options: unit / u32",
            "1. unit (recommended)",
            "No returned value",
            "2. u32",
            "/answer q7 N",
            "/answer q7 --text <answer>",
        ] {
            assert!(text.contains(expected), "missing {expected}");
        }
        let text = describe_end(&TurnEnd::Asked {
            question: "Explain?".into(),
            options: Some(QuestionOptions {
                choices: vec![],
                recommended: None,
                ..options.clone()
            }),
        });
        assert!(text.contains("/answer q7 --text"));
        assert!(!text.contains("selects option"));
        let text = describe_end(&TurnEnd::Asked {
            question: "Choose?".into(),
            options: Some(QuestionOptions {
                allow_freeform: false,
                ..options
            }),
        });
        assert!(!text.contains("--text"));
        assert!(
            describe_end(&TurnEnd::Asked {
                question: "Explain?".into(),
                options: None
            })
            .contains("next message is the answer")
        );
    }

    #[test]
    fn progress_shows_a_sub_agents_steps_under_its_id() {
        let mut names = HashMap::new();
        let shown = |t: &str| t.to_owned();
        let Value::Object(arguments) = json!({"path": "src/export/mod.rs"}) else {
            unreachable!()
        };
        let start = Entry::SubagentStart {
            child: "a1".into(),
            call_id: "c1".into(),
            mode: "read".into(),
            task: "Find the export.".into(),
            paths: vec![],
            journal_next: 1,
        };
        let step = Entry::Subagent {
            child: "a1".into(),
            entry: Box::new(Entry::Item {
                item: Item::Assistant {
                    text: String::new(),
                    reasoning: None,
                    tool_calls: vec![ToolCall {
                        id: "c2".into(),
                        name: "read_file".into(),
                        raw_arguments: String::new(),
                        arguments,
                    }],
                    replay: None,
                },
            }),
        };
        let end = Entry::SubagentEnd {
            child: "a1".into(),
            call_id: "c1".into(),
            terminal: duet_agent::Terminal::Completed {
                summary: "found".into(),
            },
            requests: 2,
            cost_usd: 0.002,
            seconds: 1.0,
            written: vec![],
            journal_end: 1,
        };
        let lines: Vec<String> = [start, step, end]
            .iter()
            .flat_map(|e| progress(e, &mut names, &shown))
            .collect();
        assert_eq!(
            lines,
            [
                "  ⇢ sub-agent a1 (read): Find the export.",
                "  [a1]  · read_file src/export/mod.rs",
                "  ⇠ sub-agent a1 completed ($0.0020, 0 file(s) written)",
            ]
        );
    }

    #[test]
    fn progress_shows_calls_errors_and_withheld_results_locally() {
        let mut names = HashMap::new();
        let shown = |t: &str| t.replace("⟨EMAIL#1⟩", "ana@example.org");
        let Value::Object(arguments) = json!({"path": "notes/⟨EMAIL#1⟩.txt"}) else {
            unreachable!()
        };
        let call = ToolCall {
            id: "c1".into(),
            name: "read_file".into(),
            raw_arguments: String::new(),
            arguments,
        };
        let lines = progress(
            &Entry::Item {
                item: Item::Assistant {
                    text: "Looking at the notes.".into(),
                    reasoning: None,
                    tool_calls: vec![call],
                    replay: None,
                },
            },
            &mut names,
            &shown,
        );
        assert_eq!(
            lines,
            vec![
                "  ~ Looking at the notes.".to_owned(),
                "  · read_file notes/ana@example.org.txt".to_owned()
            ]
        );
        let lines = progress(
            &Entry::Shown {
                call_id: "c1".into(),
                class: ViewClass::HandleSummary,
            },
            &mut names,
            &shown,
        );
        assert_eq!(
            lines,
            vec![
                "  ◦ withheld from the frontier: read_file result sensitive, held locally (a summary was sent)"
            ]
        );
        let lines = progress(
            &Entry::Item {
                item: Item::ToolResult {
                    call_id: "c1".into(),
                    content: "error: no such file".into(),
                },
            },
            &mut names,
            &shown,
        );
        assert_eq!(lines, vec!["  ! read_file: error: no such file"]);
    }

    use serde_json::Value;
}
