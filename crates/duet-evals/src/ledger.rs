// SPDX-License-Identifier: GPL-3.0-or-later
//! Duet's own cost ledger, read from the run summary it writes into the
//! workspace (`.duet/runs/<id>/summary.json`, `stats.ledger`). The harness
//! does not link Duet; it reads the file as JSON and keeps what the report
//! shows. External lanes have no ledger.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

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
    /// Local model tokens (older records lack them).
    #[serde(default)]
    pub local_input_tokens: u64,
    #[serde(default)]
    pub local_output_tokens: u64,
    /// Frontier dollars at list price as Duet priced them.
    pub input_usd: f64,
    pub output_usd: f64,
}

/// The ledger of the Duet run in `workspace`, if it wrote one (the most
/// recent summary when there are several).
pub fn read(workspace: &Path) -> Option<DuetLedger> {
    let runs = workspace.join(".duet/runs");
    let newest = fs::read_dir(runs)
        .ok()?
        .flatten()
        .map(|e| e.path().join("summary.json"))
        .filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max()?
        .1;
    parse(&serde_json::from_slice(&fs::read(newest).ok()?).ok()?)
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
    if let Some(local) = l.get("local") {
        out.local_calls = u(local, "calls");
        out.local_busy_seconds = f(local, "seconds");
        out.local_input_tokens = u(local, "input_tokens");
        out.local_output_tokens = u(local, "output_tokens");
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
        assert_eq!((l.local_input_tokens, l.local_output_tokens), (1200, 80));
        assert!((l.local_busy_seconds - 42.5).abs() < 1e-9);
        // Older summaries without a ledger give none.
        assert!(parse(&json!({"stats": {"turns": 3}})).is_none());
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
