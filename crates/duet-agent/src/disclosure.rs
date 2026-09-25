// SPDX-License-Identifier: GPL-3.0-or-later
//! The per-run disclosure report: what the boundary withheld from the frontier,
//! by class, built from the run's audit log and cost ledger.
//!
//! It holds counts, kinds and check names only, never a value, a path's
//! content or a placeholder's name. Counting rules:
//! - Placeholders: distinct placeholder tokens in the requests sent, by kind
//!   (`secret`, `email`, ...). The same value always has the same token, so
//!   this is the number of distinct withheld values the frontier saw stand-ins for.
//! - Copied spans and withheld protected lines: markers in the distinct
//!   messages sent (the conversation is re-sent with every request; a message
//!   is counted once).
//! - Protected handles: distinct `⟨body:…⟩` and `⟨value:…⟩` handles sent.
//! - Views (from the cost ledger): tool results shown as a handle and local
//!   summary, as placeholders, as local answers, as protected skeletons or
//!   notices, or as a bulky preview.
//! - Events (from the audit log): blocked sends by check, sandbox denials,
//!   `sensitive_data` commands and the files they marked, protected edits,
//!   approval decisions.

use crate::ledger::Ledger;
use duet_boundary::audit::{AuditEvent, Line};
use duet_boundary::detect::Kind;
use duet_boundary::ip::LINE_WITHHELD;
use duet_boundary::overlap::REDACTED;
use duet_boundary::view::ViewClass;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::LazyLock;

static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"⟨([a-z]+):[^⟩\s]*⟩").expect("static regex"));

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Views {
    /// Sensitive content (files, command output) shown as a handle and a local summary.
    pub handle_summary: u64,
    /// Secret-bearing files shown with every value as a placeholder.
    pub tokenized: u64,
    /// Answers the local model gave about withheld content.
    pub local_answers: u64,
    /// Protected source shown as a skeleton or an existence notice.
    pub protected: u64,
    /// Public but bulky content shown as a preview with a handle.
    pub bulky: u64,
    /// Everything else, shown after scanning.
    pub raw: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Approvals {
    pub asked: u64,
    pub approved: u64,
    pub denied: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Disclosure {
    /// False when any part of the run had the boundary off (passthrough).
    pub boundary: bool,
    /// Requests sent to the frontier.
    pub requests: u64,
    /// Requests the outbound filter changed before sending.
    pub requests_filtered: u64,
    /// Distinct placeholders sent, by kind.
    pub placeholders: BTreeMap<String, u64>,
    /// Copied spans of sensitive text removed.
    pub copied_spans_redacted: u64,
    /// Distinct handles standing for protected function bodies and constants.
    pub protected_handles: u64,
    /// Lines of protected code withheld from output shown to the frontier.
    pub protected_lines_withheld: u64,
    /// Requests refused by an outbound check (not sent), by check.
    pub blocked_sends: BTreeMap<String, u64>,
    pub sandbox_denials: u64,
    pub sensitive_data_runs: u64,
    /// Files marked sensitive because a `sensitive_data` command wrote them.
    pub derived_files: u64,
    pub protected_edits: u64,
    pub approvals: Approvals,
    /// `ask_local` calls (questions about withheld content).
    pub ask_local_calls: u64,
    /// How tool results were shown; `None` when the run's ledger is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub views: Option<Views>,
}

impl Disclosure {
    /// Builds the report from a run's audit log lines and, when available, its ledger.
    pub fn build(lines: &[Line], ledger: Option<&Ledger>) -> Self {
        let mut d = Disclosure::default();
        let mut starts = 0u64;
        let mut all_bounded = true;
        let mut seen_messages: HashSet<String> = HashSet::new();
        let mut tokens: BTreeSet<(String, String)> = BTreeSet::new();
        let kinds: Vec<&str> = Kind::ALL.iter().map(|k| k.tag()).collect();
        for line in lines {
            match line {
                Line::Request(r) => {
                    d.requests += 1;
                    if !r.interventions.is_empty() {
                        d.requests_filtered += 1;
                    }
                    // Chat Completions and Anthropic Messages bodies hold the
                    // conversation in `messages`, Responses bodies in `input`;
                    // Anthropic and Responses keep the system prompt outside it.
                    let messages = r
                        .request
                        .get("messages")
                        .or_else(|| r.request.get("input"))
                        .and_then(Value::as_array);
                    for m in messages.into_iter().flatten() {
                        if m.get("role").and_then(Value::as_str) == Some("system") {
                            continue;
                        }
                        let text = m.to_string();
                        for c in TOKEN.captures_iter(&text) {
                            tokens.insert((c[1].to_owned(), c[0].to_owned()));
                        }
                        if seen_messages.insert(text.clone()) {
                            d.copied_spans_redacted += text.matches(REDACTED).count() as u64;
                            d.protected_lines_withheld +=
                                text.matches(LINE_WITHHELD).count() as u64;
                        }
                    }
                }
                Line::Event(e) => match &e.event {
                    AuditEvent::RunStart { boundary, .. } => {
                        starts += 1;
                        all_bounded &= boundary;
                    }
                    AuditEvent::BlockedSend { check } => {
                        *d.blocked_sends.entry(check.clone()).or_default() += 1;
                    }
                    AuditEvent::SandboxDenial { .. } => d.sandbox_denials += 1,
                    AuditEvent::SensitiveCommand { derived_files, .. } => {
                        d.sensitive_data_runs += 1;
                        d.derived_files += derived_files.len() as u64;
                    }
                    AuditEvent::ProtectedEdit { .. } => d.protected_edits += 1,
                    AuditEvent::Approval { approved, .. } => {
                        d.approvals.asked += 1;
                        if *approved {
                            d.approvals.approved += 1;
                        } else {
                            d.approvals.denied += 1;
                        }
                    }
                    AuditEvent::RunEnd { .. }
                    | AuditEvent::EndpointTrust { .. }
                    | AuditEvent::ConfigChange { .. }
                    | AuditEvent::OperatorMessage { .. } => {}
                },
            }
        }
        d.boundary = starts > 0 && all_bounded;
        for (kind, _) in tokens {
            if kinds.contains(&kind.as_str()) {
                *d.placeholders.entry(kind).or_default() += 1;
            } else if kind == "body" || kind == "value" {
                d.protected_handles += 1;
            }
        }
        if let Some(l) = ledger {
            let results = |c: ViewClass| l.by_class.get(&c).map_or(0, |c| c.results);
            d.ask_local_calls = l.ask_local_calls;
            d.views = Some(Views {
                handle_summary: results(ViewClass::HandleSummary),
                tokenized: results(ViewClass::Tokenized),
                local_answers: results(ViewClass::LocalAnswer),
                protected: results(ViewClass::Protected),
                bulky: results(ViewClass::BulkyHandle),
                raw: results(ViewClass::Raw),
            });
        }
        d
    }

    /// The report as text, for `duet audit disclosure`.
    pub fn render(&self, run_id: &str) -> String {
        let mut out = format!("disclosure report for run {run_id}\n\n");
        if !self.boundary {
            out.push_str(
                "The privacy boundary was OFF (passthrough) for this run, or the log has no run start:\n\
everything the model read was sent to the frontier unfiltered. Nothing below was withheld.\n\n",
            );
        }
        let row = |out: &mut String, label: &str, n: u64| {
            out.push_str(&format!("  {label:<44} {n}\n"));
        };
        out.push_str(&format!(
            "requests sent: {} ({} changed by the outbound filter)\n\nwithheld from the frontier:\n",
            self.requests, self.requests_filtered
        ));
        if self.placeholders.is_empty() {
            row(&mut out, "values replaced by placeholders", 0);
        }
        for (kind, n) in &self.placeholders {
            row(&mut out, &format!("placeholders: {kind}"), *n);
        }
        row(
            &mut out,
            "copied spans of sensitive text removed",
            self.copied_spans_redacted,
        );
        row(
            &mut out,
            "protected bodies/constants (handles)",
            self.protected_handles,
        );
        row(
            &mut out,
            "protected code lines withheld from output",
            self.protected_lines_withheld,
        );
        match &self.views {
            Some(v) => {
                row(&mut out, "sensitive results held locally (handles)", v.handle_summary);
                row(&mut out, "secret-bearing files shown tokenized", v.tokenized);
                row(&mut out, "protected source views (skeleton/notice)", v.protected);
                row(&mut out, "bulky public results shown as previews", v.bulky);
                row(&mut out, "local answers about withheld content", v.local_answers);
            }
            None => out.push_str(
                "  (per-result views unavailable: the run has no summary.json, e.g. interrupted or purged)\n",
            ),
        }
        out.push_str("\nenforcement:\n");
        let blocked: u64 = self.blocked_sends.values().sum();
        row(&mut out, "requests blocked (not sent)", blocked);
        for (check, n) in &self.blocked_sends {
            row(&mut out, &format!("  by {check}"), *n);
        }
        row(&mut out, "sandbox denials", self.sandbox_denials);
        row(
            &mut out,
            "sensitive_data commands",
            self.sensitive_data_runs,
        );
        row(
            &mut out,
            "files marked sensitive by them",
            self.derived_files,
        );
        row(
            &mut out,
            "protected edits (local model)",
            self.protected_edits,
        );
        row(&mut out, "ask_local calls", self.ask_local_calls);
        if self.approvals.asked > 0 {
            out.push_str(&format!(
                "  {:<44} {} ({} approved, {} denied)\n",
                "operator approvals asked",
                self.approvals.asked,
                self.approvals.approved,
                self.approvals.denied
            ));
        }
        out.push_str("\nCounts and kinds only; values stay on this machine.\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::ClassCost;
    use duet_boundary::audit::{AuditLog, read};
    use serde_json::json;

    fn body(messages: &[(&str, &str)]) -> Value {
        let mut m = vec![json!({"role": "system", "content": "⟨secret:IGNORED#9⟩"})];
        m.extend(
            messages
                .iter()
                .map(|(role, content)| json!({"role": role, "content": content})),
        );
        json!({"model": "m", "messages": m})
    }

    #[test]
    fn counts_withheld_content_by_class_without_values() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("a.jsonl");
        let mut log = AuditLog::open(&path).unwrap();
        log.event(AuditEvent::RunStart {
            mode: "hybrid".into(),
            boundary: true,
        })
        .unwrap();
        let first = [
            (
                "user",
                "Fix it. DB is ⟨secret:DB_URL#1⟩, owner ⟨email:email#1⟩",
            ),
            (
                "tool",
                "row ⟨email:email#1⟩ ⟨email:email#2⟩ ⟨redacted:copied-sensitive-text⟩",
            ),
        ];
        log.append("u", "m", body(&first), vec!["sanitize: replaced".into()])
            .unwrap();
        // The conversation is re-sent: nothing already seen counts twice.
        let second = [
            first[0],
            first[1],
            (
                "tool",
                "fn f() { ⟨body:h3⟩ } ⟨protected code line withheld⟩ ⟨value:h4⟩ ⟨body:h3⟩ ⟨…⟩",
            ),
        ];
        log.append("u", "m", body(&second), vec![]).unwrap();
        for event in [
            AuditEvent::SandboxDenial {
                command: "cat data/a.csv".into(),
                access: "ordinary".into(),
            },
            AuditEvent::SensitiveCommand {
                command: "x".into(),
                exit_code: Some(0),
                derived_files: vec!["out.txt".into(), "b.txt".into()],
            },
            AuditEvent::BlockedSend {
                check: "no_known_values".into(),
            },
            AuditEvent::Approval {
                tool: "write_file".into(),
                risk: "write_outside_sources".into(),
                path: Some("Cargo.toml".into()),
                approved: false,
                decided_by: "operator".into(),
            },
        ] {
            log.event(event).unwrap();
        }
        let mut ledger = Ledger {
            ask_local_calls: 3,
            ..Ledger::default()
        };
        for (class, n) in [(ViewClass::HandleSummary, 2), (ViewClass::Protected, 1)] {
            ledger.by_class.insert(
                class,
                ClassCost {
                    results: n,
                    ..ClassCost::default()
                },
            );
        }
        let r = Disclosure::build(&read(&path).unwrap(), Some(&ledger));
        assert!(r.boundary);
        assert_eq!((r.requests, r.requests_filtered), (2, 1));
        assert_eq!(
            r.placeholders,
            BTreeMap::from([("email".into(), 2), ("secret".into(), 1)])
        );
        assert_eq!(r.copied_spans_redacted, 1);
        assert_eq!(r.protected_handles, 2);
        assert_eq!(r.protected_lines_withheld, 1);
        assert_eq!(r.blocked_sends["no_known_values"], 1);
        assert_eq!(
            (r.sandbox_denials, r.sensitive_data_runs, r.derived_files),
            (1, 1, 2)
        );
        assert_eq!(
            r.approvals,
            Approvals {
                asked: 1,
                approved: 0,
                denied: 1
            }
        );
        let views = r.views.clone().unwrap();
        assert_eq!((views.handle_summary, views.protected), (2, 1));

        // Neither form carries a token name, a command or a path.
        let text = format!("{}{}", r.render("r1"), serde_json::to_string(&r).unwrap());
        for absent in [
            "DB_URL",
            "h3",
            "cat data",
            "out.txt",
            "Cargo.toml",
            "IGNORED",
        ] {
            assert!(!text.contains(absent), "{absent}: {text}");
        }
        assert!(text.contains("placeholders: email"), "{text}");
    }

    #[test]
    fn anthropic_and_responses_bodies_are_read_like_chat_bodies() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("a.jsonl");
        let mut log = AuditLog::open(&path).unwrap();
        let anthropic = json!({"model": "m",
            "system": [{"type": "text", "text": "⟨secret:IGNORED#9⟩"}],
            "messages": [{"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "c", "content": "⟨email:email#1⟩"}]}]});
        let responses = json!({"model": "m", "instructions": "⟨secret:IGNORED#9⟩",
            "input": [{"role": "user", "content": "⟨secret:DB_URL#1⟩"},
                      {"type": "function_call_output", "call_id": "c", "output": "⟨email:email#2⟩"}]});
        log.append("u", "m", anthropic, vec![]).unwrap();
        log.append("u", "m", responses, vec![]).unwrap();
        let r = Disclosure::build(&read(&path).unwrap(), None);
        assert_eq!(r.requests, 2);
        assert_eq!(
            r.placeholders,
            BTreeMap::from([("email".into(), 2), ("secret".into(), 1)])
        );
    }

    #[test]
    fn passthrough_runs_are_reported_as_unprotected() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("a.jsonl");
        let mut log = AuditLog::open(&path).unwrap();
        log.event(AuditEvent::RunStart {
            mode: "passthrough".into(),
            boundary: false,
        })
        .unwrap();
        let r = Disclosure::build(&read(&path).unwrap(), None);
        assert!(!r.boundary && r.views.is_none());
        let text = r.render("r2");
        assert!(
            text.contains("boundary was OFF") && text.contains("unavailable"),
            "{text}"
        );
    }
}
