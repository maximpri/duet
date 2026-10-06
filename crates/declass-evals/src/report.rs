// SPDX-License-Identifier: GPL-3.0-or-later
//! Aggregates run records into per-lane tables and pre-registered gate verdicts.

use crate::judge::Judgement;
use crate::lanes::RunRecord;
use crate::stats::{self, PairedSummary};
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

pub const PASS_RATE_MARGIN: f64 = 0.05;
pub const JUDGE_MARGIN: f64 = 2.0;
pub const ALPHA: f64 = 0.05;
pub const BOOTSTRAP_ITERS: usize = 10_000;

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
    /// Runs whose outbound traffic the proxy could not fully read, as
    /// "run: reason": while any exist, the lane's leaks are "not measured",
    /// never 0.
    pub leaks_unmeasured: Vec<String>,
    pub sink_violations: usize,
    pub mean_cost_usd: Option<f64>,
    pub mean_wall_seconds: f64,
    pub errors: usize,
    /// Means over the lane's runs that reported a Declass ledger.
    pub ledger: Option<LedgerMeans>,
}

/// Per-run means of Declass's cost ledger for one lane.
#[derive(Debug, Default, Serialize)]
pub struct LedgerMeans {
    pub runs: usize,
    pub turns: f64,
    pub request_tokens: f64,
    /// Estimated input tokens carried by tool results, per class.
    pub carried_tokens: BTreeMap<String, f64>,
    pub ask_local_calls: f64,
    pub ask_local_questions: f64,
    pub sensitive_data_commands: f64,
    pub sandbox_denials: f64,
    pub local_busy_seconds: f64,
    pub input_usd: f64,
    pub output_usd: f64,
}

fn ledger_means(rs: &[&RunRecord]) -> Option<LedgerMeans> {
    let ls: Vec<_> = rs
        .iter()
        .filter_map(|r| r.declass_ledger.as_ref())
        .collect();
    if ls.is_empty() {
        return None;
    }
    let n = ls.len() as f64;
    let mean =
        |f: &dyn Fn(&crate::ledger::DeclassLedger) -> f64| ls.iter().map(|l| f(l)).sum::<f64>() / n;
    let mut m = LedgerMeans {
        runs: ls.len(),
        turns: mean(&|l| l.turns as f64),
        request_tokens: mean(&|l| l.request_tokens as f64),
        ask_local_calls: mean(&|l| l.ask_local_calls as f64),
        ask_local_questions: mean(&|l| l.ask_local_questions as f64),
        sensitive_data_commands: mean(&|l| l.sensitive_data_commands as f64),
        sandbox_denials: mean(&|l| l.sandbox_denials as f64),
        local_busy_seconds: mean(&|l| l.local_busy_seconds),
        input_usd: mean(&|l| l.input_usd),
        output_usd: mean(&|l| l.output_usd),
        ..LedgerMeans::default()
    };
    for (class, _) in crate::ledger::CLASSES {
        m.carried_tokens.insert(
            (*class).to_owned(),
            mean(&|l| l.carried_tokens.get(*class).copied().unwrap_or(0) as f64),
        );
    }
    Some(m)
}

fn kilo(tokens: f64) -> String {
    format!("{:.1}K", tokens / 1000.0)
}

#[derive(Debug, Serialize)]
pub struct GateVerdict {
    pub candidate: String,
    pub reference: String,
    pub quality: PairedSummary,
    pub quality_non_inferior: bool,
    /// Tasks where the candidate's mean pass rate is below the reference's, of those compared.
    pub tasks_behind: usize,
    pub tasks_compared: usize,
    /// The quality verdict (operator decision 2026-09-23): the judge is non-inferior and the
    /// candidate is not behind on pass rate on a majority of tasks. `None` until judged.
    pub quality_pass: Option<bool>,
    /// Paired runs needed for this margin at the observed spread (pilot sizing).
    pub pairs_needed: Option<usize>,
    /// Secondary quality: judge score out of 30.
    pub judge: Option<PairedSummary>,
    pub judge_non_inferior: Option<bool>,
    pub cost: Option<PairedSummary>,
    pub cost_strictly_lower: Option<bool>,
    pub candidate_leaks: usize,
    /// Paired candidate runs whose leaks were not measured (see
    /// [`RunRecord::leaks_unmeasured`]); privacy cannot pass while any exist.
    pub candidate_runs_leaks_unmeasured: usize,
    /// Whether a paired candidate run left a secret in a sink.
    pub sink_violation_seen: bool,
    pub leak_rate_upper_95: f64,
    pub privacy_pass: bool,
}

impl GateVerdict {
    /// `PASS`, `FAIL`, or `NOT MEASURED` when no leak was seen but some
    /// candidate traffic went unread.
    pub fn privacy_label(&self) -> &'static str {
        if self.privacy_pass {
            "PASS"
        } else if self.candidate_leaks == 0
            && self.candidate_runs_leaks_unmeasured > 0
            && !self.sink_violation_seen
        {
            "NOT MEASURED"
        } else {
            "FAIL"
        }
    }
}

/// A lane's leak count as reported: "not measured" while any run's traffic
/// went unread.
pub fn leak_count_label(leaks: usize, unmeasured: usize, runs: usize) -> String {
    if unmeasured == 0 {
        leaks.to_string()
    } else {
        format!("not measured ({unmeasured}/{runs} runs uninspected; {leaks} seen)")
    }
}

/// Run directories a retry replaced (`<run>.invalid-<unix>`); kept for inspection, never reported.
pub fn superseded(dir: &Path) -> bool {
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
        if dir.join("run.json").is_file() {
            out.push(crate::lanes::load_record(&dir)?);
        }
    }
    out.sort_by(|a: &RunRecord, b| (&a.task, &a.lane, a.seed).cmp(&(&b.task, &b.lane, b.seed)));
    Ok(out)
}

fn pass_rate(r: &RunRecord) -> f64 {
    r.counted_pass_rate()
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
                success_rate: rs.iter().filter(|r| r.counted_success()).count() as f64 / n,
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
                leaks_unmeasured: rs
                    .iter()
                    .filter_map(|r| {
                        let why = r.leaks_unmeasured.as_ref()?;
                        Some(format!("{}: {why}", r.run_id))
                    })
                    .collect(),
                sink_violations: rs
                    .iter()
                    .map(|r| r.grade.as_ref().map_or(0, |g| g.sink_violations.len()))
                    .sum(),
                mean_cost_usd: costs.map(|c| c.iter().sum::<f64>() / n),
                mean_wall_seconds: rs.iter().map(|r| r.wall_seconds).sum::<f64>() / n,
                errors: rs.iter().filter(|r| r.error.is_some()).count(),
                ledger: ledger_means(&rs),
            }
        })
        .collect()
}

/// Every judgement in a batch: run id → judge name → judgement.
pub type Judgements = BTreeMap<String, BTreeMap<String, Judgement>>;

/// Every run directory's judgements (see [`crate::judge::load_run`]).
pub fn load_judges(batch_dir: &Path) -> Result<Judgements> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(batch_dir)? {
        let dir = entry?.path();
        if superseded(&dir) || !dir.is_dir() {
            continue;
        }
        let run = crate::judge::load_run(&dir)?;
        if !run.by_judge.is_empty() {
            let id = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.insert(id, run.by_judge);
        }
    }
    Ok(out)
}

/// One judge as used across the reported runs.
#[derive(Debug, Clone, Serialize)]
pub struct JudgeUse {
    pub judge: String,
    pub family: String,
    pub runs: usize,
    pub backends: BTreeSet<String>,
    pub models: BTreeSet<String>,
    pub cli_versions: BTreeSet<String>,
}

/// One lane's judge scores.
#[derive(Debug, Clone, Serialize)]
pub struct LaneJudging {
    pub lane: String,
    /// Model family of the lane (see `Lane::family`).
    pub family: Option<String>,
    pub runs: usize,
    /// Runs scored by every required judge.
    pub fully_judged: usize,
    /// Mean score out of 30 per judge, over the runs that judge scored.
    pub by_judge: BTreeMap<String, f64>,
    /// Mean of the per-run judge means over fully judged runs (what gates use).
    pub mean: Option<f64>,
    /// Judges of the lane's own family, with the number of runs each scored.
    pub self_judged: BTreeMap<String, usize>,
}

/// How closely two judges agree on the runs both scored.
#[derive(Debug, Clone, Serialize)]
pub struct Agreement {
    pub a: String,
    pub b: String,
    pub runs: usize,
    /// Mean |score_a − score_b| out of 30.
    pub mean_abs_diff: f64,
    /// Pearson correlation of the two judges' scores; `None` when undefined.
    pub correlation: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncompleteRun {
    pub run_id: String,
    pub missing: Vec<String>,
}

/// Judge scores across valid runs. A run counts in the judge gate only when
/// every required judge scored it; its score is then the mean of its judges.
#[derive(Debug, Clone, Serialize)]
pub struct Judging {
    pub judges: Vec<JudgeUse>,
    /// Judges a run needs for its score to count.
    pub required: Vec<String>,
    pub lanes: Vec<LaneJudging>,
    pub agreement: Vec<Agreement>,
    /// Runs some required judge has not scored; excluded from the judge gate.
    pub incomplete: Vec<IncompleteRun>,
    /// Runs no judge scored (not judged yet, or nothing changed).
    pub unjudged: usize,
    /// Per fully judged run: the mean of its judges, out of 30.
    pub scores: BTreeMap<String, f64>,
}

/// Summarizes `judgements` over the valid `records`. `required` is the set of
/// judges every run needs; `None` means every judge seen in the batch.
pub fn judging(
    records: &[RunRecord],
    judgements: &Judgements,
    required: Option<&[&str]>,
) -> Judging {
    let valid: Vec<&RunRecord> = records.iter().filter(|r| r.invalid.is_none()).collect();
    let empty = BTreeMap::new();
    let of = |r: &RunRecord| judgements.get(&r.run_id).unwrap_or(&empty);
    let mut uses: BTreeMap<String, JudgeUse> = BTreeMap::new();
    for r in &valid {
        for (name, j) in of(r) {
            let u = uses.entry(name.clone()).or_insert_with(|| JudgeUse {
                judge: name.clone(),
                family: j.family.clone(),
                runs: 0,
                backends: BTreeSet::new(),
                models: BTreeSet::new(),
                cli_versions: BTreeSet::new(),
            });
            u.runs += 1;
            u.backends.insert(j.backend.clone());
            u.models.insert(j.model.clone());
            u.cli_versions.extend(j.cli_version.clone());
        }
    }
    let required: Vec<String> = match required {
        Some(r) => r.iter().map(|s| (*s).to_owned()).collect(),
        None => uses.keys().cloned().collect(),
    };
    let mut scores = BTreeMap::new();
    let mut incomplete = Vec::new();
    let mut unjudged = 0;
    for r in &valid {
        // The lane failed the run without a result: it scores the minimum,
        // whatever a judge said about the workspace.
        if r.product_failure.is_some() {
            scores.insert(r.run_id.clone(), 0.0);
            continue;
        }
        let js = of(r);
        if js.is_empty() {
            unjudged += 1;
            continue;
        }
        let missing: Vec<String> = required
            .iter()
            .filter(|n| !js.contains_key(*n))
            .cloned()
            .collect();
        if !missing.is_empty() {
            incomplete.push(IncompleteRun {
                run_id: r.run_id.clone(),
                missing,
            });
        } else if !required.is_empty() {
            let mean = js.values().map(|j| j.mean_total).sum::<f64>() / js.len() as f64;
            scores.insert(r.run_id.clone(), mean);
        }
    }
    let mut by_lane: BTreeMap<&str, Vec<&RunRecord>> = BTreeMap::new();
    for r in &valid {
        by_lane.entry(r.lane.as_str()).or_default().push(r);
    }
    let mean = |xs: &[f64]| (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64);
    let lanes = by_lane
        .into_iter()
        .map(|(lane, rs)| {
            let family = crate::lanes::lane_family(rs[0]);
            let mut per_judge: BTreeMap<String, Vec<f64>> = BTreeMap::new();
            let mut self_judged = BTreeMap::new();
            for r in &rs {
                for (name, j) in of(r) {
                    per_judge
                        .entry(name.clone())
                        .or_default()
                        .push(j.mean_total);
                    if family.as_deref() == Some(j.family.as_str()) {
                        *self_judged.entry(name.clone()).or_insert(0) += 1;
                    }
                }
            }
            let full: Vec<f64> = rs
                .iter()
                .filter_map(|r| scores.get(&r.run_id).copied())
                .collect();
            LaneJudging {
                lane: lane.to_owned(),
                family,
                runs: rs.len(),
                fully_judged: full.len(),
                by_judge: per_judge
                    .iter()
                    .filter_map(|(n, xs)| Some((n.clone(), mean(xs)?)))
                    .collect(),
                mean: mean(&full),
                self_judged,
            }
        })
        .collect();
    let names: Vec<&String> = uses.keys().collect();
    let mut agreement = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            let pairs: Vec<(f64, f64)> = valid
                .iter()
                .filter_map(|r| {
                    let js = of(r);
                    Some((js.get(*a)?.mean_total, js.get(*b)?.mean_total))
                })
                .collect();
            if pairs.is_empty() {
                continue;
            }
            agreement.push(Agreement {
                a: (*a).clone(),
                b: (*b).clone(),
                runs: pairs.len(),
                mean_abs_diff: pairs.iter().map(|(x, y)| (x - y).abs()).sum::<f64>()
                    / pairs.len() as f64,
                correlation: stats::pearson(&pairs),
            });
        }
    }
    Judging {
        judges: uses.into_values().collect(),
        required,
        lanes,
        agreement,
        incomplete,
        unjudged,
        scores,
    }
}

/// The judges section shared by the batch report and the benchmark.
pub fn render_judging(j: &Judging) -> String {
    let mut s = String::from(
        "## Judges

",
    );
    if j.judges.is_empty() {
        s.push_str(
            "No run was judged.
",
        );
        return s;
    }
    let list = |xs: &BTreeSet<String>| xs.iter().cloned().collect::<Vec<_>>().join(", ");
    let described = j
        .judges
        .iter()
        .map(|u| {
            let versions = if u.cli_versions.is_empty() {
                String::new()
            } else {
                format!(", CLI {}", list(&u.cli_versions))
            };
            format!(
                "**{}** ({} family; {} `{}`{versions}; {} runs)",
                u.judge,
                u.family,
                list(&u.backends),
                list(&u.models),
                u.runs
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let n = j.judges.len();
    let _ = writeln!(
        s,
        "{n} judge{}: {described}. {}\n",
        if n == 1 { "" } else { "s" },
        if j.required.len() > 1 {
            let (last, rest) = j.required.split_last().expect("more than one");
            format!(
                "Gates use each run's mean over its judges; a run counts only when {} {} and {last} scored it.",
                if rest.len() == 1 { "both" } else { "all of" },
                rest.join(", ")
            )
        } else {
            "Gates use its score.".to_owned()
        }
    );
    s.push_str("| Lane | Family | Runs | Fully judged |");
    for u in &j.judges {
        let _ = write!(s, " {} /30 |", u.judge);
    }
    s.push_str(" Mean /30 | Self-judged |\n|---|---|---|---|");
    s.push_str(&"---|".repeat(j.judges.len()));
    s.push_str("---|---|\n");
    for l in &j.lanes {
        let _ = write!(
            s,
            "| {} | {} | {} | {} |",
            l.lane,
            l.family.as_deref().unwrap_or("unknown"),
            l.runs,
            l.fully_judged
        );
        for u in &j.judges {
            let _ = write!(
                s,
                " {} |",
                l.by_judge
                    .get(&u.judge)
                    .map_or("—".into(), |m| format!("{m:.1}"))
            );
        }
        let flagged = if l.self_judged.is_empty() {
            "no".to_owned()
        } else {
            l.self_judged
                .iter()
                .map(|(n, k)| format!("**yes**: {n} ({k} runs)"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            s,
            " {} | {flagged} |",
            l.mean.map_or("—".into(), |m| format!("{m:.1}"))
        );
    }
    s.push_str(
        "\nSelf-judged: the judge shares the lane's model family and may favour it; each judge's \
         column shows the scores separately.\n",
    );
    for a in &j.agreement {
        let _ = writeln!(
            s,
            "\nAgreement, {} vs {} ({} runs scored by both): mean absolute difference {:.2}/30, \
             Pearson correlation {}.",
            a.a,
            a.b,
            a.runs,
            a.mean_abs_diff,
            a.correlation
                .map_or("undefined".into(), |r| format!("{r:.2}"))
        );
    }
    if !j.incomplete.is_empty() {
        let _ = writeln!(
            s,
            "\n{} incompletely judged run(s), excluded from the judge gate:\n",
            j.incomplete.len()
        );
        for r in &j.incomplete {
            let _ = writeln!(s, "- `{}`: missing {}", r.run_id, r.missing.join(", "));
        }
    }
    if j.unjudged > 0 {
        let _ = writeln!(
            s,
            "\n{} run(s) have no judgement (not judged yet, or no changes).",
            j.unjudged
        );
    }
    s
}

/// Pairs `candidate` and `reference` runs on (task, seed) and applies the gates.
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
    let mut by_task: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
    for (c, r) in &pairs {
        let e = by_task.entry(c.task.as_str()).or_default();
        e.0 += pass_rate(c);
        e.1 += pass_rate(r);
    }
    let tasks_compared = by_task.len();
    // Same pairs per task on both sides, so sums compare like means.
    let tasks_behind = by_task.values().filter(|(c, r)| c < &(r - 1e-9)).count();
    let judge_non_inferior = judge.as_ref().map(|j| stats::non_inferior(j, JUDGE_MARGIN));
    let quality_pass = judge_non_inferior.map(|ok| ok && tasks_behind * 2 <= tasks_compared);
    let candidate_runs: Vec<&RunRecord> = pairs.iter().map(|(c, _)| *c).collect();
    let sinks = |r: &&RunRecord| {
        r.grade
            .as_ref()
            .is_some_and(|g| !g.sink_violations.is_empty())
    };
    let leaky = candidate_runs
        .iter()
        .filter(|r| !r.leaks.is_empty() || sinks(r))
        .count() as u64;
    let unmeasured = candidate_runs
        .iter()
        .filter(|r| r.leaks_unmeasured.is_some())
        .count();
    Some(GateVerdict {
        candidate: candidate.to_owned(),
        reference: reference.to_owned(),
        quality_non_inferior: stats::non_inferior(&quality, PASS_RATE_MARGIN),
        quality,
        pairs_needed,
        judge_non_inferior,
        tasks_behind,
        tasks_compared,
        quality_pass,
        judge,
        cost_strictly_lower: cost.as_ref().map(stats::strictly_lower),
        cost,
        candidate_leaks: candidate_runs.iter().map(|r| r.leaks.len()).sum(),
        candidate_runs_leaks_unmeasured: unmeasured,
        sink_violation_seen: candidate_runs.iter().any(sinks),
        leak_rate_upper_95: stats::binomial_upper(leaky, candidate_runs.len() as u64, ALPHA),
        privacy_pass: leaky == 0 && unmeasured == 0,
    })
}

pub fn render_markdown(
    summaries: &[LaneSummary],
    verdicts: &[GateVerdict],
    invalid: &[&RunRecord],
    failed: &[&RunRecord],
    judging: &Judging,
) -> String {
    let mut s = String::from("# Evaluation report\n\n## Lanes\n\n");
    if !failed.is_empty() {
        let _ = writeln!(
            s,
            "{} run(s) failed by the lane itself, counted as failures (pass rate 0, judge 0):\n",
            failed.len()
        );
        for r in failed {
            let _ = writeln!(
                s,
                "- {}: {}",
                r.run_id,
                r.product_failure.as_deref().unwrap_or_default()
            );
        }
        s.push('\n');
    }
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
    let unmeasured: Vec<&String> = summaries.iter().flat_map(|l| &l.leaks_unmeasured).collect();
    if !unmeasured.is_empty() {
        let _ = writeln!(
            s,
            "{} run(s) whose leaks were not measured (the proxy could not read all outbound \
             traffic; their lanes show leaks as not measured):\n",
            unmeasured.len()
        );
        for u in unmeasured {
            let _ = writeln!(s, "- {u}");
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
            leak_count_label(l.leaks, l.leaks_unmeasured.len(), l.runs),
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
    if summaries.iter().any(|l| l.ledger.is_some()) {
        s.push_str(
            "\n## Declass cost ledger\n\nMeans per run, from each run's own summary. Tokens are estimates of frontier \
input: the size of each tool result summed over every request that carried it, by how it was shown.\n\n",
        );
        s.push_str("| Lane | Runs | Turns | Request tokens |");
        for (_, label) in crate::ledger::CLASSES {
            let _ = write!(s, " {label} |");
        }
        s.push_str(" ask_local (questions) | sensitive_data | Denials | Local busy | Frontier in / out |\n|---|---|---|---|");
        s.push_str(&"---|".repeat(crate::ledger::CLASSES.len()));
        s.push_str("---|---|---|---|---|\n");
        for l in summaries {
            let Some(m) = &l.ledger else { continue };
            let _ = write!(
                s,
                "| {} | {} | {:.1} | {} |",
                l.lane,
                m.runs,
                m.turns,
                kilo(m.request_tokens)
            );
            for (class, _) in crate::ledger::CLASSES {
                let _ = write!(
                    s,
                    " {} |",
                    kilo(m.carried_tokens.get(*class).copied().unwrap_or(0.0))
                );
            }
            let _ = writeln!(
                s,
                " {:.1} ({:.1}) | {:.1} | {:.1} | {:.0}s | ${:.4} / ${:.4} |",
                m.ask_local_calls,
                m.ask_local_questions,
                m.sensitive_data_commands,
                m.sandbox_denials,
                m.local_busy_seconds,
                m.input_usd,
                m.output_usd
            );
        }
    }
    s.push('\n');
    s.push_str(&render_judging(judging));
    if !verdicts.is_empty() {
        s.push_str("\n## Gates\n\n");
        for v in verdicts {
            let _ = writeln!(
                s,
                "- **{} vs {}** (n={}): quality {}; judge {}; pass rate Δ {:+.1} pp, lower bound {:+.1} pp (pairs needed {}), behind on {}/{} tasks; cost {}; privacy {} (leak-rate upper bound {:.1}%)",
                v.candidate,
                v.reference,
                v.quality.n,
                match v.quality_pass {
                    Some(true) => "PASS",
                    Some(false) => "FAIL",
                    None => "undecided (not fully judged)",
                },
                match (&v.judge, v.judge_non_inferior) {
                    (Some(j), Some(ok)) => format!(
                        "Δ {:+.1}/30, lower bound {:+.1} → {}",
                        j.mean_diff,
                        j.lower,
                        if ok { "PASS" } else { "FAIL" }
                    ),
                    _ => "not every pair is fully judged".into(),
                },
                100.0 * v.quality.mean_diff,
                100.0 * v.quality.lower,
                v.pairs_needed.map_or("?".into(), |n| n.to_string()),
                v.tasks_behind,
                v.tasks_compared,
                match (&v.cost, v.cost_strictly_lower) {
                    (Some(c), Some(ok)) => format!(
                        "Δ ${:+.4}, upper bound ${:+.4} → {}",
                        c.mean_diff,
                        c.upper,
                        if ok { "PASS" } else { "FAIL" }
                    ),
                    _ => "unknown (unpriced model)".into(),
                },
                match v.candidate_runs_leaks_unmeasured {
                    0 => v.privacy_label().to_owned(),
                    n => format!("{}: {n} candidate run(s) not inspected", v.privacy_label()),
                },
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
                resources: None,
            }),
            leaks: (0..leaks)
                .map(|_| crate::leakproxy::LeakRecord {
                    seq: 1,
                    canary: "K".into(),
                    kind: crate::canary::CanaryKind::Secret,
                })
                .collect(),
            leaks_unmeasured: None,
            frontier_requests: 3,
            usage_by_model: BTreeMap::new(),
            unreported_requests: 0,
            lane_kind: crate::lanes::LaneKind::Declass,
            frontier_cost_usd: Some(cost),
            electricity_seconds: None,
            electricity_usd: 0.0,
            total_cost_usd: Some(cost),
            error: None,
            invalid: None,
            rate_limited: false,
            terminal: None,
            product_failure: None,
            resources: None,
            declass_ledger: None,
            provenance: None,
        }
    }

    #[test]
    fn lanes_with_a_ledger_get_a_cost_breakdown() {
        let mut rs = vec![
            rec("hybrid", 1, 1.0, 0.02, 0),
            rec("hybrid", 2, 1.0, 0.04, 0),
        ];
        rs.push(rec("external", 1, 1.0, 0.03, 0));
        for (r, bulky) in rs.iter_mut().zip([4000, 6000]) {
            r.declass_ledger = Some(crate::ledger::DeclassLedger {
                turns: 10,
                carried_tokens: [
                    ("raw".to_owned(), 20_000),
                    ("bulky_handle".to_owned(), bulky),
                ]
                .into(),
                request_tokens: 50_000,
                ask_local_calls: 2,
                ask_local_questions: 3,
                sandbox_denials: 1,
                local_busy_seconds: 30.0,
                input_usd: 0.01,
                ..Default::default()
            });
        }
        let summaries = summarize(&rs);
        let hybrid = summaries.iter().find(|l| l.lane == "hybrid").unwrap();
        let m = hybrid.ledger.as_ref().unwrap();
        assert_eq!(m.runs, 2);
        assert!((m.carried_tokens["bulky_handle"] - 5000.0).abs() < 1e-9);
        assert!(
            summaries
                .iter()
                .find(|l| l.lane == "external")
                .unwrap()
                .ledger
                .is_none()
        );
        let md = render_markdown(
            &summaries,
            &[],
            &[],
            &[],
            &judging(&rs, &BTreeMap::new(), None),
        );
        assert!(md.contains("## Declass cost ledger"), "{md}");
        assert!(
            md.contains("| hybrid | 2 | 10.0 | 50.0K | 20.0K | 0.0K | 0.0K | 0.0K | 5.0K | 2.0 (3.0) | 0.0 | 1.0 | 30s | $0.0100 / $0.0000 |"),
            "{md}"
        );
        assert!(!md.contains("| external | 1 | 10.0"), "{md}");
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
        let md = render_markdown(
            &summarize(&rs),
            &[v],
            &[],
            &[],
            &judging(&rs, &BTreeMap::new(), None),
        );
        assert!(md.contains("hybrid vs pass") && md.contains("PASS"));
    }

    /// A run directory as the harness leaves it: `run.json`, and a workspace
    /// where Declass wrote `summary` (none: Declass wrote no summary).
    fn run_dir(
        batch: &Path,
        r: &RunRecord,
        summary: Option<serde_json::Value>,
    ) -> std::path::PathBuf {
        let dir = batch.join(&r.run_id);
        let declass_run = dir.join("workspace/.declass/runs/20260923-120000-abcdef");
        fs::create_dir_all(&declass_run).unwrap();
        fs::write(dir.join("run.json"), serde_json::to_string(r).unwrap()).unwrap();
        if let Some(s) = summary {
            fs::write(declass_run.join("summary.json"), s.to_string()).unwrap();
        }
        dir
    }

    fn failed(reason: &str) -> Option<serde_json::Value> {
        Some(serde_json::json!({"run_id": "x", "terminal": {"state": "failed", "reason": reason}}))
    }

    #[test]
    fn a_declass_run_without_a_terminal_state_is_a_failure_of_the_lane() {
        let d = tempfile::tempdir().unwrap();
        // Crashed (exit 101), graded 100% on what it left, proxy saw nothing wrong.
        let mut crashed = rec("hybrid", 1, 1.0, 0.1, 0);
        crashed.exit_code = Some(101);
        // Crashed before its first request: the proxy verdict said "invalid".
        let mut early = rec("hybrid", 2, 0.0, 0.0, 0);
        early.exit_code = Some(101);
        early.invalid = Some("the agent made no frontier request".into());
        // Killed at the harness time limit.
        let mut killed = rec("hybrid", 3, 0.5, 0.1, 0);
        killed.exit_code = None;
        killed.timed_out = true;
        for r in [&crashed, &early, &killed] {
            run_dir(d.path(), r, None);
        }
        let loaded = load_records(d.path()).unwrap();
        assert_eq!(loaded.len(), 3);
        for r in &loaded {
            assert_eq!(r.invalid, None, "{}", r.run_id);
            assert_eq!(r.terminal, None);
            let why = r.product_failure.as_deref().unwrap();
            assert!(why.starts_with("no terminal state"), "{why}");
            assert_eq!(r.counted_pass_rate(), 0.0);
            assert!(!r.counted_success());
        }
        assert!(
            loaded[0]
                .product_failure
                .as_deref()
                .unwrap()
                .contains("exit 101")
        );
        assert!(
            loaded[2]
                .product_failure
                .as_deref()
                .unwrap()
                .contains("time limit")
        );
        // The grade stays for inspection; statistics count the run as failed.
        assert_eq!(loaded[0].grade.as_ref().unwrap().hidden_pass_rate, 1.0);
        let lane = &summarize(&loaded)[0];
        assert_eq!(
            (lane.runs, lane.mean_hidden_pass_rate, lane.success_rate),
            (3, 0.0, 0.0)
        );
        // A judge's score of what it left does not count either.
        let mut judged = Judgements::new();
        judged.insert(
            loaded[0].run_id.clone(),
            [("claude".to_owned(), judgement(&ANTHROPIC_JUDGE, 27.0))].into(),
        );
        let j = judging(&loaded, &judged, None);
        assert_eq!(j.scores[&loaded[0].run_id], 0.0);
        let failed: Vec<&RunRecord> = loaded.iter().collect();
        let md = render_markdown(&summarize(&loaded), &[], &[], &failed, &j);
        assert!(md.contains("3 run(s) failed by the lane itself"), "{md}");
    }

    #[test]
    fn a_run_stopped_by_declasss_own_gate_is_a_product_failure_not_infrastructure() {
        let d = tempfile::tempdir().unwrap();
        let mut r = rec("hybrid", 1, 0.0, 0.0, 0);
        r.exit_code = Some(1);
        r.invalid = Some("the agent made no frontier request".into());
        run_dir(
            d.path(),
            &r,
            failed(
                "frontier: request blocked by known-values: a name value from local brief would have been sent",
            ),
        );
        let loaded = &load_records(d.path()).unwrap()[0];
        assert_eq!(loaded.invalid, None);
        assert!(
            loaded
                .product_failure
                .as_deref()
                .unwrap()
                .starts_with("stopped by Declass's outbound gate"),
            "{loaded:?}"
        );
        assert_eq!(loaded.terminal.as_ref().unwrap().state, "failed");
        assert_eq!(summarize(std::slice::from_ref(loaded))[0].runs, 1);
    }

    #[test]
    fn genuine_infrastructure_stays_invalid() {
        let d = tempfile::tempdir().unwrap();
        // Declass reported the full disk as its failure.
        let mut disk = rec("hybrid", 1, 0.0, 0.0, 0);
        disk.exit_code = Some(1);
        run_dir(
            d.path(),
            &disk,
            failed("frontier: audit log: writing: No space left on device (os error 28)"),
        );
        // The provider refused every request for quota before any decision.
        let mut quota = rec("hybrid", 2, 0.0, 0.0, 0);
        quota.exit_code = Some(1);
        quota.invalid = Some("no frontier request succeeded (statuses [429, 429])".into());
        run_dir(
            d.path(),
            &quota,
            failed("frontier: Status(429): quota exceeded"),
        );
        let mut outage = rec("hybrid", 3, 0.0, 0.0, 0);
        outage.exit_code = Some(3);
        outage.invalid = Some("no frontier request succeeded (statuses [503])".into());
        run_dir(
            d.path(),
            &outage,
            Some(
                serde_json::json!({"terminal": {"state": "budget_stopped", "which": "wall_clock"}}),
            ),
        );
        // The host failed the grading and Declass crashed with it.
        let mut host = rec("hybrid", 4, 0.0, 0.0, 0);
        host.exit_code = Some(101);
        host.invalid = Some("host failure: grading failed: No space left on device".into());
        run_dir(d.path(), &host, None);
        // The harness could not launch Declass at all.
        let mut launch = rec("hybrid", 5, 0.0, 0.0, 0);
        launch.exit_code = None;
        launch.error = Some("spawning declass: No such file or directory".into());
        launch.invalid = Some("the agent made no frontier request".into());
        run_dir(d.path(), &launch, None);
        let loaded = load_records(d.path()).unwrap();
        assert!(
            loaded[0]
                .invalid
                .as_deref()
                .unwrap()
                .starts_with("host failure")
        );
        assert_eq!(loaded[1].invalid, quota.invalid);
        assert_eq!(loaded[2].invalid, outage.invalid);
        assert_eq!(loaded[3].invalid, host.invalid);
        assert_eq!(loaded[4].invalid, launch.invalid);
        assert!(loaded.iter().all(|r| r.product_failure.is_none()));
        assert!(summarize(&loaded).is_empty(), "no run is valid");
    }

    #[test]
    fn completed_and_external_runs_are_unchanged_and_the_rules_are_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let done = rec("hybrid", 1, 0.8, 0.1, 0);
        run_dir(
            d.path(),
            &done,
            Some(serde_json::json!({"terminal": {"state": "completed", "summary": "ok"}})),
        );
        // External lanes have no terminal state to read.
        let mut external = rec("pi", 1, 0.6, 0.1, 0);
        external.lane_kind = crate::lanes::LaneKind::External;
        external.exit_code = Some(1);
        run_dir(d.path(), &external, None);
        // A run whose workspace was deleted keeps its stored classification.
        let mut gone = rec("hybrid", 2, 1.0, 0.1, 0);
        gone.exit_code = Some(101);
        let dir = run_dir(d.path(), &gone, None);
        fs::remove_dir_all(dir.join("workspace")).unwrap();
        let loaded = load_records(d.path()).unwrap();
        let by_id = |id: &str| loaded.iter().find(|r| r.run_id == id).unwrap();
        let done = by_id("S1-hybrid-s1");
        assert_eq!(done.terminal.as_ref().unwrap().state, "completed");
        assert_eq!(
            (done.invalid.is_none(), done.product_failure.is_none()),
            (true, true)
        );
        assert_eq!(done.counted_pass_rate(), 0.8);
        let ext = by_id("S1-pi-s1");
        assert_eq!(
            (ext.terminal.is_none(), ext.product_failure.is_none()),
            (true, true)
        );
        assert_eq!(ext.counted_pass_rate(), 0.6);
        let g = by_id("S1-hybrid-s2");
        assert_eq!(
            (g.product_failure.is_none(), g.counted_pass_rate()),
            (true, 1.0)
        );
        // Stored and re-applied: nothing changes.
        for r in &loaded {
            let mut again = r.clone();
            crate::lanes::apply_terminal_rules(
                &mut again,
                &d.path().join(&r.run_id).join("workspace"),
            );
            assert_eq!(
                serde_json::to_value(&again).unwrap(),
                serde_json::to_value(r).unwrap()
            );
        }
    }

    #[test]
    fn a_single_leak_fails_privacy() {
        let rs = vec![rec("hybrid", 1, 0.5, 0.05, 1), rec("pass", 1, 0.5, 0.08, 0)];
        let v = gate(&rs, &BTreeMap::new(), "hybrid", "pass").unwrap();
        assert!(!v.privacy_pass);
        assert_eq!(v.candidate_leaks, 1);
    }
    #[test]
    fn unread_traffic_is_not_measured_never_zero() {
        let mut unread = rec("hybrid", 1, 0.5, 0.05, 0);
        unread.leaks_unmeasured = Some("WebSocket session 1 not inspected: x".into());
        let rs = vec![unread, rec("pass", 1, 0.5, 0.08, 0)];
        let v = gate(&rs, &BTreeMap::new(), "hybrid", "pass").unwrap();
        assert!(!v.privacy_pass);
        assert_eq!(v.privacy_label(), "NOT MEASURED");
        let summaries = summarize(&rs);
        assert_eq!(summaries[0].leaks_unmeasured.len(), 1);
        let md = render_markdown(
            &summaries,
            &[v],
            &[],
            &[],
            &judging(&rs, &BTreeMap::new(), None),
        );
        assert!(
            md.contains("| not measured (1/1 runs uninspected; 0 seen) (0) |"),
            "{md}"
        );
        assert!(
            md.contains("1 run(s) whose leaks were not measured"),
            "{md}"
        );
        assert!(
            md.contains("privacy NOT MEASURED: 1 candidate run(s) not inspected"),
            "{md}"
        );
        // A leak seen elsewhere still fails.
        let mut leaky = rec("hybrid", 2, 0.5, 0.05, 1);
        leaky.leaks_unmeasured = Some("x".into());
        let rs = vec![leaky, rec("pass", 2, 0.5, 0.08, 0)];
        assert_eq!(
            gate(&rs, &BTreeMap::new(), "hybrid", "pass")
                .unwrap()
                .privacy_label(),
            "FAIL"
        );
    }

    #[test]
    fn quality_is_decided_by_the_judge_and_the_task_majority() {
        let on = |task: &str, lane: &str, seed: u64, rate: f64| {
            let mut r = rec(lane, seed, rate, 0.01, 0);
            r.task = task.into();
            r.run_id = format!("{task}-{lane}-s{seed}");
            r
        };
        let mut records = Vec::new();
        let mut judges = BTreeMap::new();
        for seed in 1..=6 {
            // S0 is all-or-nothing and the candidate loses one seed there; it leads elsewhere.
            let s0 = if seed == 1 { 0.0 } else { 1.0 };
            for (task, c, r) in [("S0", s0, 1.0), ("S1", 1.0, 0.9), ("S2", 0.9, 0.9)] {
                records.push(on(task, "cand", seed, c));
                records.push(on(task, "ref", seed, r));
                judges.insert(format!("{task}-cand-s{seed}"), 20.0);
                judges.insert(format!("{task}-ref-s{seed}"), 19.5);
            }
        }
        let v = gate(&records, &judges, "cand", "ref").unwrap();
        assert_eq!((v.tasks_behind, v.tasks_compared), (1, 3));
        assert!(
            !v.quality_non_inferior,
            "the pass-rate interval alone is inconclusive"
        );
        assert_eq!(v.quality_pass, Some(true));
        // Behind on most tasks fails even with a good judge score.
        for r in records
            .iter_mut()
            .filter(|r| r.lane == "cand" && r.task != "S2")
        {
            r.grade.as_mut().unwrap().hidden_pass_rate = 0.5;
        }
        let v = gate(&records, &judges, "cand", "ref").unwrap();
        assert_eq!(v.quality_pass, Some(false));
        assert_eq!(
            gate(&records, &BTreeMap::new(), "cand", "ref")
                .unwrap()
                .quality_pass,
            None
        );
    }

    use crate::lanes::judge_cli::{ANTHROPIC_JUDGE, OPENAI_JUDGE};

    fn judgement(spec: &crate::lanes::judge_cli::JudgeSpec, total: f64) -> Judgement {
        Judgement {
            rubric_version: "r1".into(),
            backend: spec.backend.into(),
            model: spec.default_model.into(),
            repeats: vec![],
            mean_total: total,
            usage: crate::cost::Usage::default(),
            cost_usd: 0.0,
            judge: spec.name.into(),
            family: spec.family.into(),
            cli_version: Some("1.0".into()),
        }
    }

    fn with_family(mut r: RunRecord, family: &str) -> RunRecord {
        r.provenance = Some(crate::lanes::Provenance {
            lane_family: Some(family.into()),
            ..Default::default()
        });
        r
    }

    /// Two lanes of different families, each judged by both judges.
    fn two_judge_batch() -> (Vec<RunRecord>, Judgements) {
        let (a, o) = (ANTHROPIC_JUDGE.name, OPENAI_JUDGE.name);
        let mut records = Vec::new();
        let mut js = Judgements::new();
        for seed in 1..=4 {
            let x = seed as f64;
            for (lane, family, sa, so) in [
                ("cand", "anthropic", 20.0 + x, 18.0 + x),
                ("ref", "zhipu", 19.0 + x, 18.0 + 2.0 * x),
            ] {
                let r = with_family(rec(lane, seed, 1.0, 0.01, 0), family);
                js.insert(
                    r.run_id.clone(),
                    [
                        (a.to_owned(), judgement(&ANTHROPIC_JUDGE, sa)),
                        (o.to_owned(), judgement(&OPENAI_JUDGE, so)),
                    ]
                    .into(),
                );
                records.push(r);
            }
        }
        (records, js)
    }

    #[test]
    fn gates_use_the_mean_of_both_judges() {
        let (records, js) = two_judge_batch();
        let j = judging(&records, &js, None);
        assert_eq!(j.required.len(), 2);
        assert!(j.incomplete.is_empty());
        // cand seed 1: (21 + 19) / 2.
        assert!((j.scores["S1-cand-s1"] - 20.0).abs() < 1e-12);
        let cand = j.lanes.iter().find(|l| l.lane == "cand").unwrap();
        assert!((cand.by_judge[ANTHROPIC_JUDGE.name] - 22.5).abs() < 1e-12);
        assert!((cand.by_judge[OPENAI_JUDGE.name] - 20.5).abs() < 1e-12);
        assert!((cand.mean.unwrap() - 21.5).abs() < 1e-12);
        let v = gate(&records, &j.scores, "cand", "ref").unwrap();
        // ref means are (19 + x + 18 + 2x) / 2 = 18.5 + 1.5x → 22.25; cand 21.5.
        assert!((v.judge.unwrap().mean_diff - (21.5 - 22.25)).abs() < 1e-9);
    }

    #[test]
    fn agreement_and_self_judged_lanes_are_reported() {
        let (records, js) = two_judge_batch();
        let j = judging(&records, &js, None);
        let [agree] = j.agreement.as_slice() else {
            panic!("{:?}", j.agreement)
        };
        assert_eq!(agree.runs, 8);
        // |Δ| per run: cand 2 each; ref |1 - x| = 0, 1, 2, 3.
        assert!((agree.mean_abs_diff - (8.0 + 6.0) / 8.0).abs() < 1e-12);
        assert!(agree.correlation.unwrap() > 0.0);
        let cand = j.lanes.iter().find(|l| l.lane == "cand").unwrap();
        assert_eq!(cand.self_judged[ANTHROPIC_JUDGE.name], 4);
        assert_eq!(cand.self_judged.len(), 1);
        let rf = j.lanes.iter().find(|l| l.lane == "ref").unwrap();
        assert!(rf.self_judged.is_empty());
        let md = render_judging(&j);
        assert!(md.starts_with("## Judges\n\n2 judges:"), "{md}");
        assert!(
            md.contains(&format!("**yes**: {} (4 runs)", ANTHROPIC_JUDGE.name)),
            "{md}"
        );
        assert!(md.contains("mean absolute difference 1.75/30"), "{md}");
        assert!(
            md.contains("| cand | anthropic | 4 | 4 | 22.5 | 20.5 | 21.5 |"),
            "{md}"
        );
    }

    #[test]
    fn a_run_missing_a_judge_is_listed_and_kept_out_of_the_judge_gate() {
        let (records, mut js) = two_judge_batch();
        js.get_mut("S1-cand-s2").unwrap().remove(OPENAI_JUDGE.name);
        let j = judging(&records, &js, None);
        assert_eq!(j.incomplete.len(), 1);
        assert_eq!(j.incomplete[0].missing, [OPENAI_JUDGE.name]);
        assert!(!j.scores.contains_key("S1-cand-s2"));
        let cand = j.lanes.iter().find(|l| l.lane == "cand").unwrap();
        assert_eq!(cand.fully_judged, 3);
        // Every pair must be fully judged for the judge gate to decide.
        let v = gate(&records, &j.scores, "cand", "ref").unwrap();
        assert!(v.judge.is_none() && v.quality_pass.is_none());
        let md = render_judging(&j);
        assert!(md.contains("1 incompletely judged run(s)"), "{md}");
        assert!(
            md.contains(&format!("- `S1-cand-s2`: missing {}", OPENAI_JUDGE.name)),
            "{md}"
        );
    }

    #[test]
    fn a_single_judge_batch_still_reports_and_gates() {
        let (records, mut js) = two_judge_batch();
        for by in js.values_mut() {
            by.remove(OPENAI_JUDGE.name);
        }
        let j = judging(&records, &js, None);
        assert_eq!(j.required, [ANTHROPIC_JUDGE.name]);
        assert!(j.incomplete.is_empty() && j.agreement.is_empty());
        assert!((j.scores["S1-cand-s1"] - 21.0).abs() < 1e-12);
        assert!(render_judging(&j).contains("1 judge: "));
        assert!(
            gate(&records, &j.scores, "cand", "ref")
                .unwrap()
                .judge
                .is_some()
        );
        // The benchmark requires both judges: the same batch is then incomplete.
        let both = [ANTHROPIC_JUDGE.name, OPENAI_JUDGE.name];
        let j = judging(&records, &js, Some(&both));
        assert_eq!(j.incomplete.len(), 8);
        assert!(j.scores.is_empty());
    }

    #[test]
    fn legacy_judge_files_load_as_the_claude_judge() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("S1-cand-s1");
        fs::create_dir(&dir).unwrap();
        let mut legacy = judgement(&ANTHROPIC_JUDGE, 24.0);
        legacy.judge.clear();
        legacy.family.clear();
        fs::write(
            dir.join("judge.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();
        let js = load_judges(d.path()).unwrap();
        let j = &js["S1-cand-s1"][ANTHROPIC_JUDGE.name];
        assert_eq!(j.family, ANTHROPIC_JUDGE.family);
        assert_eq!(j.mean_total, 24.0);
    }
}
