// SPDX-License-Identifier: GPL-3.0-or-later
//! The frontier loop: one continuous conversation until a terminal state.

use crate::context::{estimate, mask_if_needed};
use crate::journal::WriteJournal;
use crate::ledger::Ledger;
use crate::prompt::system_prompt;
use crate::tools::{self, Ctx, Outcome};
use crate::transcript::{Entry, Transcript};
use duet_boundary::GatedFrontier;
use duet_boundary::model::{Item, Request, StopReason, ToolCall, ToolSpec, Usage};
use duet_boundary::view::{Presenter, ViewClass};
use duet_git::Git;
use duet_sandbox::SandboxKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Terminal {
    Completed { summary: String },
    Failed { reason: String },
    BudgetStopped { which: String },
}

pub struct RunConfig {
    pub workspace: PathBuf,
    pub run_dir: PathBuf,
    pub objective: String,
    pub mode: String,
    pub checks: Vec<String>,
    pub sandbox: SandboxKind,
    pub network: bool,
    pub command_timeout: Duration,
    pub wall_clock: Duration,
    pub frontier_usd: f64,
    pub max_finish_attempts: u32,
    pub context_window: u64,
    pub mask_at: f64,
    pub max_output_tokens: u32,
    /// Sent as `reasoning_effort` unless `None`.
    pub reasoning_effort: Option<String>,
    /// Prices a response's usage in dollars.
    pub price: Box<dyn Fn(&Usage) -> f64 + Send + Sync>,
    /// Operator approval of risky actions (`oversight.approve`).
    pub oversight: crate::oversight::Oversight,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunStats {
    pub turns: u64,
    pub usage: Usage,
    pub cost_usd: f64,
    pub tool_calls: u64,
    pub masked_results: u64,
    pub wall_seconds: f64,
    /// Where the frontier input went, by how tool results were shown (see `ledger`).
    #[serde(default)]
    pub ledger: Ledger,
}

const MAX_TEXT_ONLY_TURNS: u32 = 3;
const MAX_LENGTH_STOPS: u32 = 3;

/// Provider-specific request fields for the configured reasoning effort.
fn reasoning_extra(effort: Option<&str>) -> serde_json::Map<String, serde_json::Value> {
    let mut extra = serde_json::Map::new();
    if let Some(e) = effort {
        extra.insert("reasoning_effort".into(), e.into());
    }
    extra
}

fn add(a: &mut Usage, b: &Usage) {
    a.input += b.input;
    a.cache_read += b.cache_read;
    a.cache_write += b.cache_write;
    a.output += b.output;
    a.reasoning += b.reasoning;
}

/// Runs to a terminal state. `resume` continues an existing transcript.
pub async fn run(
    cfg: &RunConfig,
    frontier: &GatedFrontier,
    presenter: &dyn Presenter,
    git: &Git,
    resume: bool,
    interrupted: &std::sync::atomic::AtomicBool,
) -> (Terminal, RunStats) {
    let started = Instant::now();
    let mut stats = RunStats::default();
    let terminal = match drive(
        cfg,
        frontier,
        presenter,
        git,
        resume,
        interrupted,
        &mut stats,
        started,
    )
    .await
    {
        Ok(t) => t,
        Err(reason) => Terminal::Failed { reason },
    };
    stats.wall_seconds = started.elapsed().as_secs_f64();
    stats.ledger.finish();
    if let Ok(t) = Transcript::open(&cfg.run_dir) {
        let _ = t.append(&Entry::End {
            terminal: terminal.clone(),
        });
    }
    (terminal, stats)
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    cfg: &RunConfig,
    frontier: &GatedFrontier,
    presenter: &dyn Presenter,
    git: &Git,
    resume: bool,
    interrupted: &std::sync::atomic::AtomicBool,
    stats: &mut RunStats,
    started: Instant,
) -> Result<Terminal, String> {
    let transcript = Transcript::open(&cfg.run_dir).map_err(|e| e.to_string())?;
    let name = cfg
        .workspace
        .file_name()
        .map_or("repository".into(), |n| n.to_string_lossy().into_owned());
    let system = system_prompt(&name, &cfg.checks);
    let mut items: Vec<Item> = Vec::new();
    // How each tool result was shown, by call id (for the ledger).
    let mut classes: HashMap<String, ViewClass> = HashMap::new();
    if resume {
        let restored =
            WriteJournal::recover(&cfg.run_dir, &cfg.workspace).map_err(|e| e.to_string())?;
        if !restored.is_empty() {
            eprintln!("rolled back {} interrupted write(s)", restored.len());
        }
        let entries = Transcript::read(&cfg.run_dir).map_err(|e| e.to_string())?;
        for e in &entries {
            if let Entry::Shown { call_id, class } = e {
                classes.insert(call_id.clone(), *class);
            }
        }
        // The ledger is rebuilt from the transcript; masking done before the
        // interruption is not replayed, so carried tokens are an upper bound.
        let mut calls: HashMap<String, ToolCall> = HashMap::new();
        for e in entries {
            match e {
                Entry::Item { item } => {
                    match &item {
                        Item::Assistant { tool_calls, .. } => {
                            for c in tool_calls {
                                calls.insert(c.id.clone(), c.clone());
                            }
                        }
                        Item::ToolResult { call_id, content } => {
                            if let Some(c) = calls.get(call_id) {
                                let class = classes.get(call_id).copied();
                                stats
                                    .ledger
                                    .on_result(c, content, class.unwrap_or(ViewClass::Raw));
                            }
                        }
                        Item::User { .. } => {}
                    }
                    items.push(item);
                }
                Entry::Usage {
                    usage, cost_usd, ..
                } => {
                    stats.ledger.on_request(&items, &system, &classes);
                    stats.ledger.on_usage(&usage, &*cfg.price);
                    add(&mut stats.usage, &usage);
                    stats.cost_usd += cost_usd;
                    stats.turns += 1;
                }
                _ => {}
            }
        }
        // A trailing assistant turn whose tool calls never got results cannot be
        // continued; drop it so the frontier re-decides.
        if let Some(Item::Assistant { tool_calls, .. }) = items.last()
            && !tool_calls.is_empty()
        {
            items.pop();
        }
        if items.is_empty() {
            return Err("nothing to resume: the transcript is empty".into());
        }
    } else {
        transcript
            .append(&Entry::Start {
                objective: cfg.objective.clone(),
                mode: cfg.mode.clone(),
                frontier_model: frontier.model().to_owned(),
            })
            .map_err(|e| e.to_string())?;
        let first = Item::User {
            text: presenter.sanitize_objective(&cfg.objective),
        };
        transcript
            .append(&Entry::Item {
                item: first.clone(),
            })
            .map_err(|e| e.to_string())?;
        items.push(first);
    }

    let specs: Vec<ToolSpec> = tools::specs_with(presenter.extra_tools());
    let mut journal = WriteJournal::open(&cfg.run_dir).map_err(|e| e.to_string())?;
    let (mut text_only, mut length_stops, mut finish_attempts) = (0u32, 0u32, 0u32);

    loop {
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(Terminal::Failed {
                reason: "interrupted; resume with `duet resume`".into(),
            });
        }
        if started.elapsed() >= cfg.wall_clock {
            return Ok(Terminal::BudgetStopped {
                which: "wall_clock".into(),
            });
        }
        if stats.cost_usd >= cfg.frontier_usd {
            return Ok(Terminal::BudgetStopped {
                which: "frontier_usd".into(),
            });
        }
        let before = estimate(&items, &system);
        let masked = mask_if_needed(&mut items, &system, cfg.context_window, cfg.mask_at);
        if masked > 0 {
            stats.masked_results += masked as u64;
            let _ = transcript.append(&Entry::Masked {
                items: masked,
                tokens_before: before,
                tokens_after: estimate(&items, &system),
            });
        }
        let request = Request {
            system: system.clone(),
            items: items.clone(),
            tools: specs.clone(),
            max_output_tokens: Some(cfg.max_output_tokens),
            extra: reasoning_extra(cfg.reasoning_effort.as_deref()),
            ..Request::default()
        };
        let (response, interventions) = frontier
            .create(&request)
            .await
            .map_err(|e| format!("frontier: {e}"))?;
        let cost = (cfg.price)(&response.usage);
        stats.turns += 1;
        stats.cost_usd += cost;
        add(&mut stats.usage, &response.usage);
        stats.ledger.on_request(&request.items, &system, &classes);
        stats.ledger.on_usage(&response.usage, &*cfg.price);
        let _ = transcript.append(&Entry::Usage {
            turn: stats.turns,
            usage: response.usage,
            cost_usd: cost,
            interventions,
        });

        let assistant = response.to_item();
        transcript
            .append(&Entry::Item {
                item: assistant.clone(),
            })
            .map_err(|e| e.to_string())?;
        items.push(assistant);

        match response.stop {
            StopReason::Length => {
                length_stops += 1;
                if length_stops > MAX_LENGTH_STOPS {
                    return Ok(Terminal::Failed {
                        reason: "responses repeatedly hit the output limit".into(),
                    });
                }
                // Tool calls in a truncated response are never executed.
                let nudge = Item::User { text: "Your last response was cut off at the output limit, so none of its tool calls ran. Continue with smaller steps.".into() };
                transcript
                    .append(&Entry::Item {
                        item: nudge.clone(),
                    })
                    .map_err(|e| e.to_string())?;
                items.push(nudge);
                continue;
            }
            StopReason::ContentFilter => {
                return Ok(Terminal::Failed {
                    reason: "the provider's content filter stopped the response".into(),
                });
            }
            _ => {}
        }

        if response.tool_calls.is_empty() {
            text_only += 1;
            if text_only > MAX_TEXT_ONLY_TURNS {
                return Ok(Terminal::Failed {
                    reason: "the model stopped calling tools without finishing".into(),
                });
            }
            let nudge = Item::User {
                text: "Continue working with the tools. When the task is complete, call `finish`."
                    .into(),
            };
            transcript
                .append(&Entry::Item {
                    item: nudge.clone(),
                })
                .map_err(|e| e.to_string())?;
            items.push(nudge);
            continue;
        }
        text_only = 0;

        let mut finished = None;
        for call in &response.tool_calls {
            stats.tool_calls += 1;
            let mut ctx = Ctx {
                workspace: &cfg.workspace,
                run_dir: &cfg.run_dir,
                sandbox: cfg.sandbox,
                git,
                presenter,
                journal: &mut journal,
                command_timeout: cfg.command_timeout,
                network: cfg.network,
                checks: &cfg.checks,
                audit: Some(frontier.audit()),
            };
            // Only what this call shows counts for it.
            let _ = presenter.take_view_class();
            let content = if finished.is_some() {
                "not run: the task was already finished".to_owned()
            } else if call.arguments.is_empty() && call.raw_arguments.trim() != "{}" {
                format!(
                    "error: arguments are not valid JSON: {}",
                    call.raw_arguments.chars().take(200).collect::<String>()
                )
            } else if let Err(e) = crate::oversight::review(
                &cfg.oversight,
                &call.name,
                &call.arguments,
                presenter,
                Some(frontier.audit()),
            ) {
                format!("error: {e}")
            } else {
                match tools::dispatch(&mut ctx, &call.name, &call.arguments).await {
                    Outcome::Result(text) => text,
                    Outcome::Error(e) => format!("error: {e}"),
                    Outcome::Finished { summary } => {
                        finished = Some(summary);
                        "finished; checks passed".to_owned()
                    }
                    Outcome::ChecksFailed(report) => {
                        finish_attempts += 1;
                        if finish_attempts >= cfg.max_finish_attempts {
                            return Ok(Terminal::Failed {
                                reason: format!(
                                    "checks still failing after {finish_attempts} finish attempts"
                                ),
                            });
                        }
                        format!("checks failed; the task is not complete:\n{report}")
                    }
                }
            };
            let class = presenter.take_view_class().unwrap_or(ViewClass::Raw);
            stats.ledger.on_result(call, &content, class);
            classes.insert(call.id.clone(), class);
            let result = Item::ToolResult {
                call_id: call.id.clone(),
                content,
            };
            transcript
                .append(&Entry::Item {
                    item: result.clone(),
                })
                .map_err(|e| e.to_string())?;
            let _ = transcript.append(&Entry::Shown {
                call_id: call.id.clone(),
                class,
            });
            items.push(result);
        }
        if let Some(summary) = finished {
            return Ok(Terminal::Completed { summary });
        }
    }
}
