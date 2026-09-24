// SPDX-License-Identifier: GPL-3.0-or-later
//! Secondary quality: code-quality judgements by two judges of different model
//! families (PLAN §3, 2026-09-24).
//!
//! The judge sees only the task objective and the candidate's diff against a
//! re-rendered baseline. Lane-identifying names are scrubbed and canary values
//! are redacted before anything is sent. Correctness is not the judge's job:
//! hidden tests decide that.

use crate::canary::Manifest;
use crate::cost::{Usage, usage_from_response};
use crate::lanes::RunRecord;
use crate::lanes::judge_cli::{self, JudgeCli, JudgeSpec};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
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
    pub fn check(&self) -> Result<()> {
        ensure!(
            self.correctness_risk <= 10
                && self.maintainability <= 10
                && self.scope_discipline <= 10,
            "judge score out of range"
        );
        Ok(())
    }

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
    /// Which judge this is (its file is `judge-<judge>.json`); empty in files
    /// written before there were two judges.
    #[serde(default)]
    pub judge: String,
    /// The judge's model family.
    #[serde(default)]
    pub family: String,
    /// First line of the judge CLI's `--version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
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
            judge: judge_cli::API_JUDGE.name.into(),
            family: judge_cli::API_JUDGE.family.into(),
            cli_version: None,
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
    scores.check()?;
    let usage = usage_from_response(text).unwrap_or_default();
    Ok((scores, usage))
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

/// Which judge implementation to use.
pub enum Backend {
    /// Anthropic API with `ANTHROPIC_API_KEY`.
    Api(JudgeClient),
    /// A logged-in command-line assistant (see `lanes::judge_cli`).
    External(JudgeCli),
    /// Canned answers, for tests.
    #[cfg(test)]
    Stub(tests::Stub),
}

impl Backend {
    pub fn new(spec: &JudgeSpec, model: &str) -> Result<Self> {
        Ok(if spec.backend == judge_cli::API_JUDGE.backend {
            Backend::Api(JudgeClient::from_env(model)?)
        } else {
            Backend::External(JudgeCli::new(spec, model)?)
        })
    }

    pub fn spec(&self) -> JudgeSpec {
        match self {
            Backend::Api(_) => judge_cli::API_JUDGE,
            Backend::External(e) => e.spec(),
            #[cfg(test)]
            Backend::Stub(s) => s.spec,
        }
    }

    fn model(&self) -> String {
        match self {
            Backend::Api(c) => c.model.clone(),
            Backend::External(e) => e.model().to_owned(),
            #[cfg(test)]
            Backend::Stub(_) => "stub-model".into(),
        }
    }

    fn version(&self) -> Option<String> {
        match self {
            Backend::Api(_) => None,
            Backend::External(e) => e.version(),
            #[cfg(test)]
            Backend::Stub(_) => Some("stub 1.0".into()),
        }
    }

    async fn once(&self, objective: &str, diff: &str) -> Result<(Scores, f64)> {
        let (scores, cost) = match self {
            Backend::Api(c) => c.once(objective, diff).await.map(|(s, _)| (s, 0.0))?,
            Backend::External(e) => e.once(objective, diff).await?,
            #[cfg(test)]
            Backend::Stub(s) => s.once()?,
        };
        scores.check()?;
        Ok((scores, cost))
    }

    pub async fn judge(&self, objective: &str, diff: &str, repeats: usize) -> Result<Judgement> {
        if let Backend::Api(c) = self {
            return c.judge(objective, diff, repeats).await;
        }
        ensure!(repeats > 0, "need at least one repeat");
        let spec = self.spec();
        let mut scores = Vec::new();
        let mut cost = 0.0;
        for _ in 0..repeats {
            // An answer that is missing, malformed or out of range is asked for once more.
            let (s, c) = match self.once(objective, diff).await {
                Ok(answer) => answer,
                Err(first) => self.once(objective, diff).await.with_context(|| {
                    format!("{} judge failed twice; first attempt: {first:#}", spec.name)
                })?,
            };
            scores.push(s);
            cost += c;
        }
        let mean_total =
            scores.iter().map(|s| f64::from(s.total())).sum::<f64>() / scores.len() as f64;
        Ok(Judgement {
            rubric_version: rubric_version(),
            backend: spec.backend.into(),
            model: self.model(),
            repeats: scores,
            mean_total,
            usage: Usage::default(),
            cost_usd: cost,
            judge: spec.name.into(),
            family: spec.family.into(),
            cli_version: self.version(),
        })
    }
}

/// Every judgement stored for one run, by judge name.
#[derive(Debug, Default)]
pub struct RunJudgements {
    pub by_judge: BTreeMap<String, Judgement>,
    /// Raw judge files in file-name order, for digests.
    pub files: Vec<(String, Vec<u8>)>,
}

/// Reads `judge-<name>.json` for each judge, and the single-judge `judge.json`
/// of older batches as the judge it names (a per-judge file wins).
pub fn load_run(dir: &Path) -> Result<RunJudgements> {
    let mut names: Vec<String> = judge_cli::SPECS
        .iter()
        .map(|s| judge_cli::file_name(s.name))
        .collect();
    names.push(judge_cli::LEGACY_FILE.to_owned());
    names.sort();
    names.dedup();
    let mut out = RunJudgements::default();
    for name in names {
        let bytes = match fs::read(dir.join(&name)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("{}/{name}", dir.display())),
        };
        let mut j: Judgement = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}/{name}", dir.display()))?;
        let spec = judge_cli::spec_for_judgement(&j.backend);
        if j.judge.is_empty() {
            j.judge = spec.name.into();
        }
        if j.family.is_empty() {
            j.family = spec.family.into();
        }
        let legacy = name == judge_cli::LEGACY_FILE;
        if !(legacy && out.by_judge.contains_key(&j.judge)) {
            out.by_judge.insert(j.judge.clone(), j);
        }
        out.files.push((name, bytes));
    }
    Ok(out)
}

/// Judges every valid run of `batch` with each of `judges`, skipping a run a
/// judge has already scored unless `rejudge`. `input` yields the objective and
/// the scrubbed diff for a run, or `None` when the run changed nothing.
/// Returns the number of judgements written.
pub async fn judge_runs(
    batch: &Path,
    judges: &[Backend],
    repeats: usize,
    rejudge: bool,
    mut input: impl FnMut(&RunRecord, &Path) -> Result<Option<(String, String)>>,
) -> Result<usize> {
    let mut names: Vec<&str> = judges.iter().map(|j| j.spec().name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    ensure!(names.len() == before, "each judge may be given only once");
    let mut written = 0;
    for rec in crate::report::load_records(batch)? {
        if rec.invalid.is_some() {
            continue;
        }
        let run_dir = batch.join(&rec.run_id);
        let existing = load_run(&run_dir)?;
        let pending: Vec<&Backend> = judges
            .iter()
            .filter(|j| rejudge || !existing.by_judge.contains_key(j.spec().name))
            .collect();
        if pending.is_empty() {
            continue;
        }
        let Some((objective, diff)) = input(&rec, &run_dir)? else {
            println!("{}: no changes; skipped", rec.run_id);
            continue;
        };
        for j in pending {
            let name = j.spec().name;
            let judgement = j
                .judge(&objective, &diff, repeats)
                .await
                .with_context(|| format!("{}: {name} judge", rec.run_id))?;
            println!("{}: {name} {:.1}/30", rec.run_id, judgement.mean_total);
            fs::write(
                run_dir.join(judge_cli::file_name(name)),
                serde_json::to_string_pretty(&judgement)?,
            )?;
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::canary::{CanaryKind, Generator};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// A judge that returns canned answers in order; `None` is a malformed answer.
    pub struct Stub {
        pub spec: JudgeSpec,
        pub answers: Mutex<VecDeque<Option<u8>>>,
        pub calls: Mutex<usize>,
    }

    impl Stub {
        /// Every axis scores `score` (the total is 3 × `score`).
        pub fn backend(spec: JudgeSpec, answers: &[Option<u8>]) -> Backend {
            Backend::Stub(Stub {
                spec,
                answers: Mutex::new(answers.iter().copied().collect()),
                calls: Mutex::new(0),
            })
        }

        pub fn once(&self) -> Result<(Scores, f64)> {
            *self.calls.lock().unwrap() += 1;
            match self.answers.lock().unwrap().pop_front() {
                Some(Some(x)) => Ok((
                    Scores {
                        correctness_risk: x,
                        maintainability: x,
                        scope_discipline: x,
                        deductions: vec![],
                    },
                    0.0,
                )),
                Some(None) => bail!("judge returned no structured scores"),
                None => bail!("stub has no answer left"),
            }
        }
    }

    fn calls(b: &Backend) -> usize {
        match b {
            Backend::Stub(s) => *s.calls.lock().unwrap(),
            _ => unreachable!(),
        }
    }

    fn batch_with_runs(root: &Path, runs: &[(&str, Option<&str>)]) {
        for (id, invalid) in runs {
            let dir = root.join(id);
            fs::create_dir_all(&dir).unwrap();
            let rec = serde_json::json!({
                "task": "S1", "lane": id.split('-').nth(1).unwrap(), "lane_kind": "external",
                "seed": 1, "run_id": id, "exit_code": 0, "timed_out": false, "wall_seconds": 1.0,
                "grade": null, "leaks": [], "frontier_requests": 0, "usage_by_model": {},
                "unreported_requests": 0, "frontier_cost_usd": null, "electricity_usd": 0.0,
                "total_cost_usd": null, "error": null, "invalid": invalid
            });
            fs::write(dir.join("run.json"), rec.to_string()).unwrap();
        }
    }

    const ANTHROPIC: &str = judge_cli::ANTHROPIC_JUDGE.name;
    const OPENAI: &str = judge_cli::OPENAI_JUDGE.name;

    fn input(_: &RunRecord, _: &Path) -> Result<Option<(String, String)>> {
        Ok(Some(("objective".into(), "diff".into())))
    }

    #[tokio::test]
    async fn each_judge_is_stored_separately_and_judged_runs_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        batch_with_runs(
            d.path(),
            &[("S1-a-s1", None), ("S1-b-s1", Some("provider down"))],
        );
        let judges = [
            Stub::backend(judge_cli::ANTHROPIC_JUDGE, &[Some(7)]),
            Stub::backend(judge_cli::OPENAI_JUDGE, &[Some(5)]),
        ];
        assert_eq!(
            judge_runs(d.path(), &judges, 1, false, input)
                .await
                .unwrap(),
            2
        );
        let run = load_run(&d.path().join("S1-a-s1")).unwrap();
        assert_eq!(run.by_judge[ANTHROPIC].mean_total, 21.0);
        assert_eq!(run.by_judge[OPENAI].mean_total, 15.0);
        assert_eq!(run.by_judge[OPENAI].family, "openai");
        assert_eq!(run.by_judge[OPENAI].model, "stub-model");
        assert_eq!(
            run.by_judge[OPENAI].cli_version.as_deref(),
            Some("stub 1.0")
        );
        for name in [ANTHROPIC, OPENAI] {
            assert!(
                d.path()
                    .join("S1-a-s1")
                    .join(judge_cli::file_name(name))
                    .is_file()
            );
        }
        assert!(
            load_run(&d.path().join("S1-b-s1"))
                .unwrap()
                .by_judge
                .is_empty()
        );
        // A second pass has nothing to do; --rejudge asks again.
        assert_eq!(
            judge_runs(d.path(), &judges, 1, false, input)
                .await
                .unwrap(),
            0
        );
        assert_eq!(calls(&judges[0]), 1);
        let again = [Stub::backend(judge_cli::OPENAI_JUDGE, &[Some(6)])];
        assert_eq!(
            judge_runs(d.path(), &again, 1, true, input).await.unwrap(),
            1
        );
        let run = load_run(&d.path().join("S1-a-s1")).unwrap();
        assert_eq!(run.by_judge[OPENAI].mean_total, 18.0);
        assert_eq!(run.by_judge[ANTHROPIC].mean_total, 21.0);
    }

    #[tokio::test]
    async fn a_legacy_judge_file_counts_as_the_claude_judge() {
        let d = tempfile::tempdir().unwrap();
        batch_with_runs(d.path(), &[("S1-a-s1", None)]);
        let mut legacy: serde_json::Value = serde_json::json!({
            "rubric_version": "r0", "backend": "claude-cli", "model": "m", "repeats": [],
            "mean_total": 20.0,
            "usage": {"uncached_input": 0, "cache_read": 0, "cache_write": 0, "output": 0}
        });
        let dir = d.path().join("S1-a-s1");
        fs::write(dir.join("judge.json"), legacy.to_string()).unwrap();
        let run = load_run(&dir).unwrap();
        assert_eq!(run.by_judge[ANTHROPIC].family, "anthropic");
        assert_eq!(run.files.len(), 1);
        // Only the missing judge runs.
        let judges = [
            Stub::backend(judge_cli::ANTHROPIC_JUDGE, &[]),
            Stub::backend(judge_cli::OPENAI_JUDGE, &[Some(6)]),
        ];
        assert_eq!(
            judge_runs(d.path(), &judges, 1, false, input)
                .await
                .unwrap(),
            1
        );
        assert_eq!(calls(&judges[0]), 0);
        // A per-judge file supersedes the legacy one.
        legacy["mean_total"] = 12.0.into();
        fs::write(
            dir.join(judge_cli::file_name(ANTHROPIC)),
            legacy.to_string(),
        )
        .unwrap();
        assert_eq!(load_run(&dir).unwrap().by_judge["claude"].mean_total, 12.0);
    }

    #[tokio::test]
    async fn a_malformed_answer_is_retried_once() {
        let good = Stub::backend(judge_cli::OPENAI_JUDGE, &[None, Some(8), Some(6)]);
        let j = good.judge("o", "d", 2).await.unwrap();
        assert_eq!(j.mean_total, 21.0);
        assert_eq!(calls(&good), 3);
        let bad = Stub::backend(judge_cli::OPENAI_JUDGE, &[None, None, Some(9)]);
        let err = bad.judge("o", "d", 1).await.unwrap_err();
        assert!(format!("{err:#}").contains("failed twice"), "{err:#}");
        let range = Stub::backend(judge_cli::ANTHROPIC_JUDGE, &[Some(11), Some(11)]);
        assert!(range.judge("o", "d", 1).await.is_err());
    }

    #[tokio::test]
    async fn a_judge_may_not_be_given_twice() {
        let d = tempfile::tempdir().unwrap();
        let judges = [
            Stub::backend(judge_cli::ANTHROPIC_JUDGE, &[]),
            Stub::backend(judge_cli::API_JUDGE, &[]),
        ];
        assert!(
            judge_runs(d.path(), &judges, 1, false, input)
                .await
                .is_err()
        );
    }

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
