// SPDX-License-Identifier: GPL-3.0-or-later
//! Aggregates run records into per-lane tables and pre-registered gate verdicts.

use crate::lanes::RunRecord;
use crate::stats::{self, PairedSummary};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

pub const PASS_RATE_MARGIN: f64 = 0.05;
pub const JUDGE_MARGIN: f64 = 2.0;
const ALPHA: f64 = 0.05;
const BOOTSTRAP_ITERS: usize = 10_000;

#[derive(Debug, Serialize)]
pub struct LaneSummary {
    pub lane: String,
    pub runs: usize,
    pub mean_hidden_pass_rate: f64,
    pub success_rate: f64,
    pub kind: crate::lanes::LaneKind,
    pub leaks: usize,
    pub leaks_by_kind: BTreeMap<String, usize>,
    pub runs_with_leaks: usize,
    pub sink_violations: usize,
    pub mean_cost_usd: Option<f64>,
    pub mean_wall_seconds: f64,
    pub errors: usize,
}

#[derive(Debug, Serialize)]
pub struct GateVerdict {
    pub candidate: String,
    pub reference: String,
    pub quality: PairedSummary,
    pub quality_non_inferior: bool,
    /// Paired runs needed for this margin at the observed spread (pilot sizing).
    pub pairs_needed: Option<usize>,
    /// Secondary quality: judge score out of 30.
    pub judge: Option<PairedSummary>,
    pub judge_non_inferior: Option<bool>,
    pub cost: Option<PairedSummary>,
    pub cost_strictly_lower: Option<bool>,
    pub candidate_leaks: usize,
    pub leak_rate_upper_95: f64,
    pub privacy_pass: bool,
}

/// Run directories a retry replaced (`<run>.invalid-<unix>`); kept for inspection, never reported.
fn superseded(dir: &Path) -> bool {
    dir.extension()
        .is_some_and(|e| e.to_string_lossy().starts_with("invalid-"))
}

pub fn load_records(batch_dir: &Path) -> Result<Vec<RunRecord>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(batch_dir)? {
        let dir = entry?.path();
        if superseded(&dir) {
            continue;
        }
        let path = dir.join("run.json");
        if path.is_file() {
            out.push(serde_json::from_str(&fs::read_to_string(path)?)?);
        }
    }
    out.sort_by(|a: &RunRecord, b| (&a.task, &a.lane, a.seed).cmp(&(&b.task, &b.lane, b.seed)));
    Ok(out)
}

fn pass_rate(r: &RunRecord) -> f64 {
    r.grade.as_ref().map_or(0.0, |g| g.hidden_pass_rate)
}

pub fn summarize(records: &[RunRecord]) -> Vec<LaneSummary> {
    let mut by_lane: BTreeMap<&str, Vec<&RunRecord>> = BTreeMap::new();
    for r in records.iter().filter(|r| r.invalid.is_none()) {
        by_lane.entry(&r.lane).or_default().push(r);
    }
    by_lane
        .into_iter()
        .map(|(lane, rs)| {
            let n = rs.len() as f64;
            let costs: Option<Vec<f64>> = rs.iter().map(|r| r.total_cost_usd).collect();
            LaneSummary {
                lane: lane.to_owned(),
                runs: rs.len(),
                mean_hidden_pass_rate: rs.iter().map(|r| pass_rate(r)).sum::<f64>() / n,
                success_rate: rs
                    .iter()
                    .filter(|r| r.grade.as_ref().is_some_and(|g| g.success))
                    .count() as f64
                    / n,
                kind: rs[0].lane_kind,
                leaks: rs.iter().map(|r| r.leaks.len()).sum(),
                leaks_by_kind: rs.iter().flat_map(|r| r.leaks.iter()).fold(
                    BTreeMap::new(),
                    |mut m, l| {
                        *m.entry(format!("{:?}", l.kind).to_lowercase()).or_insert(0) += 1;
                        m
                    },
                ),
                runs_with_leaks: rs.iter().filter(|r| !r.leaks.is_empty()).count(),
                sink_violations: rs
                    .iter()
                    .map(|r| r.grade.as_ref().map_or(0, |g| g.sink_violations.len()))
                    .sum(),
                mean_cost_usd: costs.map(|c| c.iter().sum::<f64>() / n),
                mean_wall_seconds: rs.iter().map(|r| r.wall_seconds).sum::<f64>() / n,
                errors: rs.iter().filter(|r| r.error.is_some()).count(),
            }
        })
        .collect()
}

/// Pairs `candidate` and `reference` runs on (task, seed) and applies the gates.
pub fn load_judges(batch_dir: &Path) -> Result<BTreeMap<String, f64>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(batch_dir)? {
        let dir = entry?.path();
        if superseded(&dir) {
            continue;
        }
        let path = dir.join("judge.json");
        if path.is_file() {
            let j: crate::judge::Judgement = serde_json::from_str(&fs::read_to_string(&path)?)?;
            let id = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.insert(id, j.mean_total);
        }
    }
    Ok(out)
}

pub fn gate(
    records: &[RunRecord],
    judges: &BTreeMap<String, f64>,
    candidate: &str,
    reference: &str,
) -> Option<GateVerdict> {
    let key = |r: &RunRecord| (r.task.clone(), r.seed);
    let refs: BTreeMap<_, &RunRecord> = records
        .iter()
        .filter(|r| r.invalid.is_none())
        .filter(|r| r.lane == reference)
        .map(|r| (key(r), r))
        .collect();
    let pairs: Vec<(&RunRecord, &RunRecord)> = records
        .iter()
        .filter(|r| r.invalid.is_none())
        .filter(|r| r.lane == candidate)
        .filter_map(|c| refs.get(&key(c)).map(|r| (c, *r)))
        .collect();
    if pairs.is_empty() {
        return None;
    }
    let quality_pairs: Vec<(f64, f64)> = pairs
        .iter()
        .map(|(c, r)| (pass_rate(c), pass_rate(r)))
        .collect();
    let quality = stats::paired_bootstrap(&quality_pairs, BOOTSTRAP_ITERS, ALPHA, 1);
    let diffs: Vec<f64> = quality_pairs.iter().map(|(a, b)| a - b).collect();
    let sd = stats::sample_sd(&diffs);
    let pairs_needed = sd
        .is_finite()
        .then(|| stats::pairs_for_non_inferiority(sd.max(1e-9), PASS_RATE_MARGIN));
    let judge_pairs: Option<Vec<(f64, f64)>> = pairs
        .iter()
        .map(|(c, r)| Some((*judges.get(&c.run_id)?, *judges.get(&r.run_id)?)))
        .collect();
    let judge = judge_pairs.map(|p| stats::paired_bootstrap(&p, BOOTSTRAP_ITERS, ALPHA, 3));
    let cost_pairs: Option<Vec<(f64, f64)>> = pairs
        .iter()
        .map(|(c, r)| Some((c.total_cost_usd?, r.total_cost_usd?)))
        .collect();
    let cost = cost_pairs.map(|p| stats::paired_bootstrap(&p, BOOTSTRAP_ITERS, ALPHA, 2));
    let candidate_runs: Vec<&RunRecord> = pairs.iter().map(|(c, _)| *c).collect();
    let leaky = candidate_runs
        .iter()
        .filter(|r| {
            !r.leaks.is_empty()
                || r.grade
                    .as_ref()
                    .is_some_and(|g| !g.sink_violations.is_empty())
        })
        .count() as u64;
    Some(GateVerdict {
        candidate: candidate.to_owned(),
        reference: reference.to_owned(),
        quality_non_inferior: stats::non_inferior(&quality, PASS_RATE_MARGIN),
        quality,
        pairs_needed,
        judge_non_inferior: judge.as_ref().map(|j| stats::non_inferior(j, JUDGE_MARGIN)),
        judge,
        cost_strictly_lower: cost.as_ref().map(stats::strictly_lower),
        cost,
        candidate_leaks: candidate_runs.iter().map(|r| r.leaks.len()).sum(),
        leak_rate_upper_95: stats::binomial_upper(leaky, candidate_runs.len() as u64, ALPHA),
        privacy_pass: leaky == 0,
    })
}

pub fn render_markdown(
    summaries: &[LaneSummary],
    verdicts: &[GateVerdict],
    invalid: &[&RunRecord],
) -> String {
    let mut s = String::from("# Evaluation report\n\n## Lanes\n\n");
    if !invalid.is_empty() {
        let _ = writeln!(
            s,
            "{} invalid run(s) excluded (infrastructure decided the outcome):\n",
            invalid.len()
        );
        for r in invalid {
            let _ = writeln!(
                s,
                "- {}: {}",
                r.run_id,
                r.invalid.as_deref().unwrap_or_default()
            );
        }
        s.push('\n');
    }
    s.push_str("| Lane | Kind | Runs | Hidden pass rate | Success | Leaks (runs) | Leaks by kind | Sink violations | Mean cost | Mean wall |\n");
    s.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    for l in summaries {
        let _ = writeln!(
            s,
            "| {} | {:?} | {} | {:.1}% | {:.0}% | {} ({}) | {} | {} | {} | {:.0}s |",
            l.lane,
            l.kind,
            l.runs,
            100.0 * l.mean_hidden_pass_rate,
            100.0 * l.success_rate,
            l.leaks,
            l.runs_with_leaks,
            l.leaks_by_kind
                .iter()
                .map(|(k, n)| format!("{k}:{n}"))
                .collect::<Vec<_>>()
                .join(" "),
            l.sink_violations,
            l.mean_cost_usd
                .map_or("unknown".into(), |c| format!("${c:.4}")),
            l.mean_wall_seconds
        );
    }
    if !verdicts.is_empty() {
        s.push_str("\n## Gates\n\n");
        for v in verdicts {
            let _ = writeln!(
                s,
                "- **{} vs {}** (n={}, pairs needed {}): quality Δ {:+.1} pp, lower bound {:+.1} pp → {}; judge {}; cost {}; privacy {} (leak-rate upper bound {:.1}%)",
                v.candidate,
                v.reference,
                v.quality.n,
                v.pairs_needed.map_or("?".into(), |n| n.to_string()),
                100.0 * v.quality.mean_diff,
                100.0 * v.quality.lower,
                if v.quality_non_inferior {
                    "PASS"
                } else {
                    "FAIL"
                },
                match (&v.judge, v.judge_non_inferior) {
                    (Some(j), Some(ok)) => format!(
                        "Δ {:+.1}/30, lower bound {:+.1} → {}",
                        j.mean_diff,
                        j.lower,
                        if ok { "PASS" } else { "FAIL" }
                    ),
                    _ => "not judged".into(),
                },
                match (&v.cost, v.cost_strictly_lower) {
                    (Some(c), Some(ok)) => format!(
                        "Δ ${:+.4}, upper bound ${:+.4} → {}",
                        c.mean_diff,
                        c.upper,
                        if ok { "PASS" } else { "FAIL" }
                    ),
                    _ => "unknown (unpriced model)".into(),
                },
                if v.privacy_pass { "PASS" } else { "FAIL" },
                100.0 * v.leak_rate_upper_95
            );
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grade::{GradeReport, TestCounts};

    #[test]
    fn superseded_attempts_are_not_reported() {
        let d = tempfile::tempdir().unwrap();
        for name in ["S1-a-s1", "S1-a-s1.invalid-1790144124"] {
            let dir = d.path().join(name);
            fs::create_dir(&dir).unwrap();
            let r = rec("a", 1, 1.0, 0.0, 0);
            fs::write(dir.join("run.json"), serde_json::to_string(&r).unwrap()).unwrap();
        }
        assert_eq!(load_records(d.path()).unwrap().len(), 1);
    }

    fn rec(lane: &str, seed: u64, rate: f64, cost: f64, leaks: usize) -> RunRecord {
        RunRecord {
            task: "S1".into(),
            lane: lane.into(),
            seed,
            run_id: format!("S1-{lane}-s{seed}"),
            exit_code: Some(0),
            timed_out: false,
            wall_seconds: 60.0,
            grade: Some(GradeReport {
                task: "S1".into(),
                visible: TestCounts::default(),
                hidden: TestCounts::default(),
                hidden_expected: 10,
                hidden_pass_rate: rate,
                success: rate >= 1.0,
                visible_timed_out: false,
                hidden_timed_out: false,
                sink_violations: vec![],
            }),
            leaks: (0..leaks)
                .map(|_| crate::leakproxy::LeakRecord {
                    seq: 1,
                    canary: "K".into(),
                    kind: crate::canary::CanaryKind::Secret,
                })
                .collect(),
            frontier_requests: 3,
            usage_by_model: BTreeMap::new(),
            unreported_requests: 0,
            lane_kind: crate::lanes::LaneKind::Duet,
            frontier_cost_usd: Some(cost),
            electricity_usd: 0.0,
            total_cost_usd: Some(cost),
            error: None,
            invalid: None,
            rate_limited: false,
        }
    }

    #[test]
    fn equal_quality_cheaper_and_clean_passes_every_gate() {
        let mut rs = Vec::new();
        for seed in 0..15 {
            let q = 0.6 + 0.02 * (seed % 5) as f64;
            rs.push(rec("hybrid", seed, q, 0.05, 0));
            rs.push(rec("pass", seed, q, 0.08, 0));
        }
        let v = gate(&rs, &BTreeMap::new(), "hybrid", "pass").unwrap();
        assert!(v.quality_non_inferior && v.privacy_pass);
        assert_eq!(v.cost_strictly_lower, Some(true));
        let md = render_markdown(&summarize(&rs), &[v], &[]);
        assert!(md.contains("hybrid vs pass") && md.contains("PASS"));
    }

    #[test]
    fn a_single_leak_fails_privacy() {
        let rs = vec![rec("hybrid", 1, 0.5, 0.05, 1), rec("pass", 1, 0.5, 0.08, 0)];
        let v = gate(&rs, &BTreeMap::new(), "hybrid", "pass").unwrap();
        assert!(!v.privacy_pass);
        assert_eq!(v.candidate_leaks, 1);
    }
}
