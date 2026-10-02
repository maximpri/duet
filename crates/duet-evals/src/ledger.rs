// SPDX-License-Identifier: GPL-3.0-or-later
//! Duet's own cost ledger, read from the run summary it writes into the
//! workspace (`.duet/runs/<id>/summary.json`, `stats.ledger`). The harness
//! does not link Duet; it reads the file as JSON and keeps what the report
//! shows. External lanes have no ledger.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Classes in the order the report shows them (Duet's view class names).
pub const CLASSES: &[(&str, &str)] = &[
    ("raw", "raw"),
    ("tokenized", "tokenized"),
    ("handle_summary", "summary"),
    ("local_answer", "answer"),
    ("bulky_handle", "bulky"),
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DuetLedger {
    pub turns: u64,
    /// Estimated frontier input tokens carried by tool results, per class.
    pub carried_tokens: BTreeMap<String, u64>,
    /// Tool results per class.
    pub results: BTreeMap<String, u64>,
    /// Estimated size of all frontier requests.
    pub request_tokens: u64,
    pub ask_local_calls: u64,
    pub ask_local_questions: u64,
    pub sensitive_data_commands: u64,
    pub sandbox_denials: u64,
    pub local_calls: u64,
    pub local_busy_seconds: f64,
    /// True only when every local role reports failed and canceled request
    /// time. Older summaries remain on the wall-clock electricity upper bound.
    #[serde(default)]
    pub local_request_time_complete: bool,
    /// Absent in older runs: zero would incorrectly imply no retries occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_answer_retries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_answer_retry_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_malformed_retries: Option<u64>,
    /// Local model tokens (older records lack them).
    #[serde(default)]
    pub local_input_tokens: u64,
    #[serde(default)]
    pub local_output_tokens: u64,
    /// `explore` calls, and how many ended with a report; their local
    /// model's work is included in the `local_*` fields above.
    #[serde(default)]
    pub explore_calls: u64,
    #[serde(default)]
    pub explore_reported: u64,
    /// Frontier dollars at list price as Duet priced them.
    pub input_usd: f64,
    pub output_usd: f64,
}

/// The directory of the newest Duet run in `workspace` that wrote a summary.
pub fn newest_run_dir(workspace: &Path) -> Option<PathBuf> {
    let runs = workspace.join(".duet/runs");
    fs::read_dir(runs)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter_map(|d| {
            Some((
                fs::metadata(d.join("summary.json")).ok()?.modified().ok()?,
                d,
            ))
        })
        .max()
        .map(|(_, d)| d)
}

/// The newest run summary Duet wrote in `workspace`, if any.
fn newest_summary(workspace: &Path) -> Option<Value> {
    let newest = newest_run_dir(workspace)?.join("summary.json");
    serde_json::from_slice(&fs::read(newest).ok()?).ok()
}

/// The ledger of the Duet run in `workspace`, if it wrote one (the most
/// recent summary when there are several).
pub fn read(workspace: &Path) -> Option<DuetLedger> {
    parse(&newest_summary(workspace)?)
}

/// How a Duet run ended, as its summary records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuetTerminal {
    /// `completed`, `failed` or `budget_stopped`.
    pub state: String,
    /// The failure reason, the budget that stopped the run, or the completion summary.
    #[serde(default)]
    pub detail: String,
}

/// The terminal state of the Duet run in `workspace` (the most recent
/// summary); `None` when Duet wrote no summary with a terminal state.
pub fn read_terminal(workspace: &Path) -> Option<DuetTerminal> {
    terminal_of(&newest_summary(workspace)?)
}

pub fn terminal_of(summary: &Value) -> Option<DuetTerminal> {
    let t = summary.get("terminal")?;
    let state = t.get("state")?.as_str()?.to_owned();
    let detail = ["reason", "which", "summary"]
        .iter()
        .find_map(|k| t.get(*k).and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    Some(DuetTerminal { state, detail })
}

pub fn parse(summary: &Value) -> Option<DuetLedger> {
    let stats = summary.get("stats")?;
    let l = stats.get("ledger")?;
    let u = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    let f = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let mut out = DuetLedger {
        turns: u(stats, "turns"),
        request_tokens: u(l, "request_tokens"),
        ask_local_calls: u(l, "ask_local_calls"),
        ask_local_questions: u(l, "ask_local_questions"),
        sensitive_data_commands: u(l, "sensitive_data_commands"),
        sandbox_denials: u(l, "sandbox_denials"),
        input_usd: f(l, "input_usd"),
        output_usd: f(l, "output_usd"),
        ..DuetLedger::default()
    };
    // The engine's local roles, then the explorer's local model.
    let explore = l.get("explore");
    let engine_timed = l
        .get("local")
        .and_then(|v| v.get("request_time_complete"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let explorer_timed = explore.is_none_or(|e| {
        u(e, "calls") == 0
            || e.get("local")
                .and_then(|v| v.get("request_time_complete"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
    });
    out.local_request_time_complete = engine_timed && explorer_timed;
    if let Some(local) = l.get("local") {
        out.local_answer_retries = local.get("answer_retries").and_then(Value::as_u64);
        out.local_answer_retry_seconds = local.get("answer_retry_seconds").and_then(Value::as_f64);
        out.local_malformed_retries = local.get("malformed_retries").and_then(Value::as_u64);
    }
    for local in [l.get("local"), explore.and_then(|e| e.get("local"))]
        .into_iter()
        .flatten()
    {
        out.local_calls += u(local, "calls");
        out.local_busy_seconds += f(local, "seconds");
        out.local_input_tokens += u(local, "input_tokens");
        out.local_output_tokens += u(local, "output_tokens");
    }
    if let Some(e) = explore {
        out.explore_calls = u(e, "calls");
        out.explore_reported = u(e, "reported");
    }
    for (class, c) in l
        .get("by_class")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        out.carried_tokens
            .insert(class.clone(), u(c, "carried_tokens"));
        out.results.insert(class.clone(), u(c, "results"));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary() -> Value {
        json!({"run_id": "r", "terminal": {"state": "completed", "summary": "ok"},
               "stats": {"turns": 9, "ledger": {
                   "by_class": {"raw": {"results": 5, "added_tokens": 900, "carried_tokens": 7000, "input_usd": 0.001},
                                "bulky_handle": {"results": 1, "added_tokens": 600, "carried_tokens": 4000, "input_usd": 0.0005}},
                   "request_tokens": 30000, "ask_local_calls": 2, "ask_local_questions": 5,
                   "sensitive_data_commands": 1, "sandbox_denials": 3,
                   "input_usd": 0.004, "output_usd": 0.001,
                   "local": {"calls": 4, "input_tokens": 1200, "cached_tokens": 0, "output_tokens": 80, "seconds": 42.5}}}})
    }

    #[test]
    fn parses_the_ledger_from_a_summary() {
        let l = parse(&summary()).unwrap();
        assert_eq!(l.turns, 9);
        assert_eq!(l.carried_tokens["bulky_handle"], 4000);
        assert_eq!(l.results["raw"], 5);
        assert_eq!((l.ask_local_calls, l.ask_local_questions), (2, 5));
        assert_eq!((l.sensitive_data_commands, l.sandbox_denials), (1, 3));
        assert_eq!(l.local_calls, 4);
        assert!(!l.local_request_time_complete);
        assert_eq!((l.local_input_tokens, l.local_output_tokens), (1200, 80));
        assert!((l.local_busy_seconds - 42.5).abs() < 1e-9);
        assert_eq!(l.local_answer_retries, None);
        assert_eq!(l.local_answer_retry_seconds, None);
        assert_eq!(l.local_malformed_retries, None);
        // Older summaries without a ledger give none.
        assert!(parse(&json!({"stats": {"turns": 3}})).is_none());
    }

    #[test]
    fn retry_counts_are_kept_when_instrumented_and_unknown_in_old_records() {
        let mut s = summary();
        let local = &mut s["stats"]["ledger"]["local"];
        local["answer_retries"] = json!(2);
        local["answer_retry_seconds"] = json!(12.5);
        local["malformed_retries"] = json!(1);
        let l = parse(&s).unwrap();
        assert_eq!(l.local_answer_retries, Some(2));
        assert_eq!(l.local_answer_retry_seconds, Some(12.5));
        assert_eq!(l.local_malformed_retries, Some(1));
        let encoded = serde_json::to_value(parse(&summary()).unwrap()).unwrap();
        assert!(encoded.get("local_answer_retries").is_none());
        assert!(encoded.get("local_answer_retry_seconds").is_none());
        assert!(encoded.get("local_malformed_retries").is_none());
        let old: DuetLedger = serde_json::from_value(encoded).unwrap();
        assert_eq!(old.local_answer_retries, None);
    }

    #[test]
    fn the_explorers_local_work_counts_as_local_work() {
        let mut s = summary();
        s["stats"]["ledger"]["explore"] = json!({"calls": 2, "reported": 1, "steps": 9,
            "bytes_read": 40000, "local": {"calls": 9, "input_tokens": 30000, "cached_tokens": 0,
            "output_tokens": 900, "seconds": 57.5}});
        let l = parse(&s).unwrap();
        assert_eq!((l.explore_calls, l.explore_reported), (2, 1));
        assert_eq!(l.local_calls, 13);
        assert_eq!((l.local_input_tokens, l.local_output_tokens), (31200, 980));
        assert!((l.local_busy_seconds - 100.0).abs() < 1e-9);
        assert!(!l.local_request_time_complete);
    }

    #[test]
    fn request_time_is_complete_only_when_every_used_local_role_marks_it() {
        let mut s = summary();
        s["stats"]["ledger"]["local"]["request_time_complete"] = json!(true);
        assert!(parse(&s).unwrap().local_request_time_complete);
        s["stats"]["ledger"]["explore"] = json!({"calls": 1,
            "local": {"seconds": 2.0}});
        assert!(!parse(&s).unwrap().local_request_time_complete);
        s["stats"]["ledger"]["explore"]["local"]["request_time_complete"] = json!(true);
        assert!(parse(&s).unwrap().local_request_time_complete);
    }

    #[test]
    fn reads_the_terminal_state() {
        let t = terminal_of(&summary()).unwrap();
        assert_eq!((t.state.as_str(), t.detail.as_str()), ("completed", "ok"));
        let failed = json!({"terminal": {"state": "failed", "reason": "frontier: Auth: bad key"}});
        assert_eq!(
            terminal_of(&failed).unwrap().detail,
            "frontier: Auth: bad key"
        );
        let stopped = json!({"terminal": {"state": "budget_stopped", "which": "wall_clock"}});
        assert_eq!(terminal_of(&stopped).unwrap().detail, "wall_clock");
        assert!(terminal_of(&json!({"stats": {}})).is_none());
    }

    #[test]
    fn reads_the_newest_summary_in_a_workspace() {
        let d = tempfile::tempdir().unwrap();
        assert!(read(d.path()).is_none());
        let run = d.path().join(".duet/runs/20260923-1");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("summary.json"), summary().to_string()).unwrap();
        fs::create_dir_all(d.path().join(".duet/runs/unfinished")).unwrap();
        assert_eq!(read(d.path()).unwrap().turns, 9);
    }
}
