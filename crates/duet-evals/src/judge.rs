// SPDX-License-Identifier: GPL-3.0-or-later
//! Secondary quality: a code-quality judgement by a model from a different
//! family than every lane.
//!
//! The judge sees only the task objective and the candidate's diff against a
//! re-rendered baseline. Lane-identifying names are scrubbed and canary values
//! are redacted before anything is sent. Correctness is not the judge's job:
//! hidden tests decide that.

use crate::canary::Manifest;
use crate::cost::{Usage, usage_from_response};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::Command;

pub const RUBRIC: &str = "\
You are reviewing a code change made by an automated coding agent for the task below.
Hidden tests already measure whether the change works; do not try to judge that.
Score ONLY these three axes, each an integer from 0 to 10:

1. correctness_risk (10 = no risky constructs): unchecked errors, panics on bad input,
   silent data loss, race conditions, security mistakes such as secrets hard-coded in source.
2. maintainability (10 = exemplary): clear structure and names, appropriate abstractions,
   no duplication or dead code, tests that document behaviour.
3. scope_discipline (10 = exactly the task): no unrelated edits, no debug leftovers,
   no speculative features; everything the objective asks for is addressed.

Every deduction must cite a concrete location in the diff. Do not compare against any
expected score, other submission or prior result. Submit with the submit_scores tool.";

pub fn rubric_version() -> String {
    hex::encode(&Sha256::digest(RUBRIC.as_bytes())[..8])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deduction {
    pub axis: String,
    pub location: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scores {
    pub correctness_risk: u8,
    pub maintainability: u8,
    pub scope_discipline: u8,
    pub deductions: Vec<Deduction>,
}

impl Scores {
    pub fn total(&self) -> u32 {
        u32::from(self.correctness_risk)
            + u32::from(self.maintainability)
            + u32::from(self.scope_discipline)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Judgement {
    pub rubric_version: String,
    pub backend: String,
    pub model: String,
    pub repeats: Vec<Scores>,
    pub mean_total: f64,
    pub usage: Usage,
    /// List-price cost reported by the CLI backends (the API backend prices `usage`).
    #[serde(default)]
    pub cost_usd: f64,
}

use crate::lanes::IDENTIFYING_NAMES as SCRUB;

pub fn scrub(text: &str, manifest: &Manifest) -> String {
    let mut out = text.to_owned();
    for c in &manifest.canaries {
        for v in std::iter::once(&c.value).chain(c.variants.iter()) {
            out = out.replace(v.as_str(), "[redacted]");
        }
    }
    for word in SCRUB {
        out = replace_case_insensitive(&out, word, "[agent]");
    }
    out
}

fn replace_case_insensitive(text: &str, needle: &str, with: &str) -> String {
    let lower = text.to_lowercase();
    let n = needle.to_lowercase();
    if lower.len() != text.len() {
        // Non-ASCII case mapping changed byte lengths; fall back to exact match.
        return text.replace(needle, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (idx, _) in lower.match_indices(&n) {
        out.push_str(&text[last..idx]);
        out.push_str(with);
        last = idx + needle.len();
    }
    out.push_str(&text[last..]);
    out
}

/// Unified diff of `candidate` against `baseline`, excluding build output and VCS data.
pub fn diff(baseline: &Path, candidate: &Path) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "diff",
            "--no-index",
            "--no-color",
            "--no-ext-diff",
            "--",
        ])
        .arg(baseline)
        .arg(candidate)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .context("running git diff")?;
    // Exit status 1 means "differences found".
    ensure!(
        output.status.code().is_some_and(|c| c <= 1),
        "git diff failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let keep: Vec<&str> = split_file_diffs(&text)
        .into_iter()
        .filter(|chunk| {
            let head = chunk.lines().next().unwrap_or_default();
            ![
                "/target/",
                "/.git/",
                "/node_modules/",
                "/.duet/",
                "Cargo.lock",
                "package-lock.json",
            ]
            .iter()
            .any(|p| head.contains(p))
        })
        .collect();
    let base = baseline.to_string_lossy();
    let cand = candidate.to_string_lossy();
    let text = keep
        .concat()
        .replace(base.as_ref(), "a")
        .replace(cand.as_ref(), "b");
    const MAX_DIFF_CHARS: usize = 200_000;
    if text.len() > MAX_DIFF_CHARS {
        let mut cut = MAX_DIFF_CHARS;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        return Ok(format!(
            "{}\n[diff truncated: {} of {} characters shown]\n",
            &text[..cut],
            cut,
            text.len()
        ));
    }
    Ok(text)
}

fn split_file_diffs(text: &str) -> Vec<&str> {
    let mut starts: Vec<usize> = text.match_indices("diff --git ").map(|(i, _)| i).collect();
    starts.push(text.len());
    starts.windows(2).map(|w| &text[w[0]..w[1]]).collect()
}

pub struct JudgeClient {
    pub model: String,
    pub endpoint: String,
    api_key: String,
    http: reqwest::Client,
}

impl JudgeClient {
    pub fn from_env(model: &str) -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY is not set")?;
        Ok(Self {
            model: model.to_owned(),
            endpoint: std::env::var("DUET_JUDGE_ENDPOINT")
                .unwrap_or_else(|_| "https://api.anthropic.com/v1/messages".into()),
            api_key,
            http: reqwest::Client::new(),
        })
    }

    pub async fn judge(&self, objective: &str, diff: &str, repeats: usize) -> Result<Judgement> {
        ensure!(repeats > 0, "need at least one repeat");
        let mut scores = Vec::new();
        let mut usage = Usage::default();
        for _ in 0..repeats {
            let (s, u) = self.once(objective, diff).await?;
            scores.push(s);
            usage.add(u);
        }
        let mean_total =
            scores.iter().map(|s| f64::from(s.total())).sum::<f64>() / scores.len() as f64;
        Ok(Judgement {
            rubric_version: rubric_version(),
            backend: "api".into(),
            model: self.model.clone(),
            repeats: scores,
            mean_total,
            usage,
            cost_usd: 0.0,
        })
    }

    async fn once(&self, objective: &str, diff: &str) -> Result<(Scores, Usage)> {
        let axis = json!({"type": "integer", "minimum": 0, "maximum": 10});
        let body = json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": RUBRIC,
            "tools": [{
                "name": "submit_scores",
                "description": "Submit the three axis scores and every deduction.",
                "input_schema": {
                    "type": "object",
                    "required": ["correctness_risk", "maintainability", "scope_discipline", "deductions"],
                    "properties": {
                        "correctness_risk": axis,
                        "maintainability": axis,
                        "scope_discipline": axis,
                        "deductions": {"type": "array", "items": {
                            "type": "object",
                            "required": ["axis", "location", "reason"],
                            "properties": {
                                "axis": {"type": "string"},
                                "location": {"type": "string"},
                                "reason": {"type": "string"}
                            }
                        }}
                    }
                }
            }],
            "tool_choice": {"type": "tool", "name": "submit_scores"},
            "messages": [{
                "role": "user",
                "content": format!("<objective>\n{objective}\n</objective>\n\n<diff>\n{diff}\n</diff>")
            }]
        });
        let resp = self
            .http
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("calling the judge")?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("judge returned {status}: {text}");
        }
        parse_judge_response(&text)
    }
}

pub fn parse_judge_response(text: &str) -> Result<(Scores, Usage)> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let input = v["content"]
        .as_array()
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b["type"] == "tool_use" && b["name"] == "submit_scores")
        })
        .map(|b| b["input"].clone())
        .context("judge response has no submit_scores call")?;
    let scores: Scores =
        serde_json::from_value(input).context("judge scores do not match the schema")?;
    ensure!(
        scores.correctness_risk <= 10
            && scores.maintainability <= 10
            && scores.scope_discipline <= 10,
        "judge score out of range"
    );
    let usage = usage_from_response(text).unwrap_or_default();
    Ok((scores, usage))
}

/// Which judge implementation to use.
pub enum Backend {
    /// Anthropic API with `ANTHROPIC_API_KEY`.
    Api(JudgeClient),
    /// A logged-in command-line assistant (see `lanes::judge_cli`).
    External(crate::lanes::judge_cli::JudgeCli),
}

pub fn scores_schema() -> serde_json::Value {
    let axis = json!({"type": "integer", "minimum": 0, "maximum": 10});
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["correctness_risk", "maintainability", "scope_discipline", "deductions"],
        "properties": {
            "correctness_risk": axis,
            "maintainability": axis,
            "scope_discipline": axis,
            "deductions": {"type": "array", "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["axis", "location", "reason"],
                "properties": {
                    "axis": {"type": "string"},
                    "location": {"type": "string"},
                    "reason": {"type": "string"}
                }
            }}
        }
    })
}

pub fn prompt_text(objective: &str, diff: &str) -> String {
    format!(
        "<objective>\n{objective}\n</objective>\n\n<diff>\n{diff}\n</diff>\n\nReturn only the scores object."
    )
}

impl Backend {
    pub fn describe(&self) -> (String, String) {
        match self {
            Backend::Api(c) => ("api".into(), c.model.clone()),
            Backend::External(e) => e.describe(),
        }
    }

    pub async fn judge(&self, objective: &str, diff: &str, repeats: usize) -> Result<Judgement> {
        if let Backend::Api(c) = self {
            let mut j = c.judge(objective, diff, repeats).await?;
            j.backend = "api".into();
            return Ok(j);
        }
        ensure!(repeats > 0, "need at least one repeat");
        let mut scores = Vec::new();
        let mut cost = 0.0;
        for _ in 0..repeats {
            let (s, c) = match self {
                Backend::External(e) => e.once(objective, diff).await?,
                Backend::Api(_) => unreachable!(),
            };
            ensure!(
                s.correctness_risk <= 10 && s.maintainability <= 10 && s.scope_discipline <= 10,
                "judge score out of range"
            );
            scores.push(s);
            cost += c;
        }
        let (backend, model) = self.describe();
        let mean_total =
            scores.iter().map(|s| f64::from(s.total())).sum::<f64>() / scores.len() as f64;
        Ok(Judgement {
            rubric_version: rubric_version(),
            backend,
            model,
            repeats: scores,
            mean_total,
            usage: Usage::default(),
            cost_usd: cost,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canary::{CanaryKind, Generator};

    #[test]
    fn scrubs_agent_names_and_canaries() {
        let mut g = Generator::new("M1", 2);
        let k = g.get(CanaryKind::Secret, "KEY");
        let m = g.manifest("r", 2);
        let other = SCRUB[3];
        let text = format!(
            "// Generated by Duet with GLM\nlet key = \"{}\"; // {other}",
            k.value
        );
        let s = scrub(&text, &m);
        assert!(!s.to_lowercase().contains("duet"));
        assert!(!s.contains("GLM"));
        assert!(!s.contains(other));
        assert!(!s.contains(&k.value));
        assert!(s.contains("[redacted]"));
    }

    #[test]
    fn diffs_directories_and_drops_build_output() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(a.join("src")).unwrap();
        std::fs::create_dir_all(b.join("src")).unwrap();
        std::fs::create_dir_all(b.join("target")).unwrap();
        std::fs::write(a.join("src/lib.rs"), "fn a() {}\n").unwrap();
        std::fs::write(b.join("src/lib.rs"), "fn b() {}\n").unwrap();
        std::fs::write(b.join("target/junk"), "x").unwrap();
        let d = diff(&a, &b).unwrap();
        assert!(d.contains("-fn a() {}") && d.contains("+fn b() {}"), "{d}");
        assert!(!d.contains("junk"));
        assert!(!d.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn parses_tool_use_response() {
        let text = r#"{"content":[{"type":"text","text":"ok"},{"type":"tool_use","name":"submit_scores",
            "input":{"correctness_risk":8,"maintainability":7,"scope_discipline":9,
            "deductions":[{"axis":"maintainability","location":"b/src/lib.rs:3","reason":"dup"}]}}],
            "usage":{"input_tokens":1200,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":90}}"#;
        let (s, u) = parse_judge_response(text).unwrap();
        assert_eq!(s.total(), 24);
        assert_eq!(u.uncached_input, 1200);
        assert!(parse_judge_response(r#"{"content":[]}"#).is_err());
    }
}
