// SPDX-License-Identifier: GPL-3.0-or-later
//! The public benchmark report (`duet-eval report --final`).
//!
//! Reads one or more batch directories, pairs runs across them by
//! (task, seed, lane), and writes a Markdown report and a machine-readable
//! JSON file with the same numbers. Everything is computed from the raw run
//! records, judge files, manifests and Duet run summaries with fixed bootstrap
//! seeds and sorted inputs, so the same inputs always regenerate the same bytes.

use crate::canary::Manifest;
use crate::cost::PriceTable;
use crate::judge::Judgement;
use crate::lanes::{LaneKind, RunRecord};
use crate::ledger::DuetLedger;
use crate::report::{self, ALPHA, BOOTSTRAP_ITERS, GateVerdict};
use crate::stats::{self, Interval};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const SCHEMA: u32 = 2;

/// A batch directory with its run and judged-run counts.
pub type BatchCounts = (PathBuf, usize, usize);

/// One run and the raw files around it.
#[derive(Debug, Clone)]
pub struct LoadedRun {
    pub record: RunRecord,
    /// The run directory as reached through the batch argument.
    pub dir: PathBuf,
    /// Judgements by judge name.
    pub judges: BTreeMap<String, Judgement>,
    /// Canaries planted in the run, by kind (values are never read into the report).
    pub canaries_by_kind: BTreeMap<String, usize>,
    /// Duet's ledger, from the run's own summary when present, else the record.
    pub ledger: Option<DuetLedger>,
    /// SHA-256 over the run's raw record and judge files.
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchInfo {
    pub path: String,
    /// Link to the batch relative to the report file.
    pub link: String,
    pub runs: usize,
    pub judged: usize,
}

/// Loads every run under `batches`, rejecting a (task, seed, lane) seen twice.
pub fn load(batches: &[PathBuf]) -> Result<(Vec<LoadedRun>, Vec<BatchCounts>)> {
    let mut runs: BTreeMap<(String, u64, String), LoadedRun> = BTreeMap::new();
    let mut info = Vec::new();
    for batch in batches {
        ensure!(
            batch.is_dir(),
            "{} is not a batch directory",
            batch.display()
        );
        let mut dirs: Vec<PathBuf> = fs::read_dir(batch)?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<_>>()?;
        dirs.sort();
        let (mut count, mut judged) = (0, 0);
        for dir in dirs {
            if report::superseded(&dir) || !dir.join("run.json").is_file() {
                continue;
            }
            let run = load_run(&dir)?;
            let key = (
                run.record.task.clone(),
                run.record.seed,
                run.record.lane.clone(),
            );
            if let Some(prev) = runs.get(&key) {
                bail!(
                    "run {} appears twice: {} and {} (pass each run once; symlink batches replace their sources)",
                    run.record.run_id,
                    prev.dir.display(),
                    dir.display()
                );
            }
            count += 1;
            judged += usize::from(!run.judges.is_empty());
            runs.insert(key, run);
        }
        info.push((batch.clone(), count, judged));
    }
    Ok((runs.into_values().collect(), info))
}

fn load_run(dir: &Path) -> Result<LoadedRun> {
    let raw = fs::read(dir.join("run.json")).with_context(|| format!("{}", dir.display()))?;
    let mut record: RunRecord = serde_json::from_slice(&raw)
        .with_context(|| format!("parsing {}/run.json", dir.display()))?;
    crate::lanes::apply_terminal_rules(&mut record, &dir.join("workspace"));
    let mut hasher = Sha256::new();
    hasher.update(&raw);
    let judges = crate::judge::load_run(dir)?;
    for (_, bytes) in &judges.files {
        hasher.update(bytes);
    }
    let mut canaries_by_kind = BTreeMap::new();
    if let Ok(text) = fs::read_to_string(dir.join("manifest.json"))
        && let Ok(m) = serde_json::from_str::<Manifest>(&text)
    {
        for c in &m.canaries {
            *canaries_by_kind.entry(kind_name(c.kind)).or_insert(0) += 1;
        }
    }
    let ledger = match record.lane_kind {
        LaneKind::Duet => {
            crate::ledger::read(&dir.join("workspace")).or_else(|| record.duet_ledger.clone())
        }
        LaneKind::External => None,
    };
    Ok(LoadedRun {
        record,
        dir: dir.to_path_buf(),
        judges: judges.by_judge,
        canaries_by_kind,
        ledger,
        digest: hex::encode(hasher.finalize()),
    })
}

fn kind_name(kind: crate::canary::CanaryKind) -> String {
    format!("{kind:?}").to_lowercase()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LeakStats {
    pub runs_with_leaks: usize,
    /// Leaked canary occurrences (one per canary per request).
    pub canaries: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub sink_violations: usize,
    /// Runs with a leak or a secret-sink violation.
    pub runs_failing_privacy: usize,
    /// Exact (Clopper–Pearson) one-sided 95% upper bound on the per-run leak rate.
    pub leak_rate_upper_95: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    pub runs: usize,
    pub hidden_pass_rate: Option<Interval>,
    pub successes: usize,
    pub success_rate: f64,
    /// Mean of the judges, out of 30, over fully judged runs.
    pub judge: Option<Interval>,
    pub leaks: LeakStats,
    /// Mean total cost (frontier at list price plus electricity); `None` if any run is unpriced.
    pub cost_usd: Option<Interval>,
    /// Paired Σcost / Σcost of the reference lane on the same (task, seed).
    pub cost_ratio_vs_reference: Option<Interval>,
    pub wall_seconds: Option<Interval>,
    /// Mean per run, from the provider's usage as captured by the proxy.
    pub frontier_input_tokens: f64,
    pub frontier_output_tokens: f64,
    /// Mean per run over runs with a Duet ledger.
    pub local_input_tokens: Option<f64>,
    pub local_output_tokens: Option<f64>,
    pub ledger_runs: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaneBlock {
    pub lane: String,
    pub kind: LaneKind,
    pub overall: Metrics,
    pub by_task: BTreeMap<String, Metrics>,
}

#[derive(Debug, Serialize)]
pub struct GateBlock {
    #[serde(flatten)]
    pub verdict: GateVerdict,
    /// Paired cost ratio candidate / reference (the privacy premium).
    pub cost_ratio: Option<Interval>,
    /// Gate 3 is reported, not gated (PLAN §3, 2026-09-24).
    pub cost_rule: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JudgeSetup {
    pub judge: String,
    pub family: String,
    pub backend: String,
    pub model: String,
    pub rubric_version: String,
    pub repeats: usize,
    pub cli_version: Option<String>,
    pub artifacts: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PriceRow {
    pub model: String,
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
    pub source: String,
    pub observed: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Method {
    /// Canaries planted per kind, summed over the included runs.
    pub canaries_planted: BTreeMap<String, usize>,
    pub judges: Vec<JudgeSetup>,
    pub current_rubric_version: String,
    pub prices: Vec<PriceRow>,
    pub electricity_usd_per_kwh: f64,
    /// Frontier models seen in usage without a verified price.
    pub unpriced_models: Vec<String>,
    pub bootstrap_iterations: usize,
    pub alpha: f64,
    pub pass_rate_margin: f64,
    pub judge_margin: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LaneVersions {
    pub lane_models: BTreeSet<String>,
    /// Models named in the requests the proxy captured.
    pub frontier_models: BTreeSet<String>,
    pub git_commits: BTreeSet<String>,
    pub harness_versions: BTreeSet<String>,
    pub agent_versions: BTreeSet<String>,
    /// Runs recorded before provenance was kept.
    pub runs_without_provenance: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunRow {
    pub run_id: String,
    pub task: String,
    pub lane: String,
    pub seed: u64,
    pub path: String,
    pub sha256: String,
    pub excluded: Option<String>,
    /// Why the lane failed the run by itself (counted as a failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FinalReport {
    pub schema: u32,
    pub reference: String,
    pub batches: Vec<BatchInfo>,
    /// SHA-256 over every run's digest in (task, seed, lane) order.
    pub inputs_sha256: String,
    pub lanes: Vec<LaneBlock>,
    pub gates: Vec<GateBlock>,
    /// Both judges' scores, their agreement and self-judged lanes.
    pub judging: report::Judging,
    pub method: Method,
    pub versions: BTreeMap<String, LaneVersions>,
    pub runs: Vec<RunRow>,
}

fn pass_rate(r: &RunRecord) -> f64 {
    r.counted_pass_rate()
}

/// Metrics over `runs` (valid runs of one lane, optionally one task).
fn metrics(
    runs: &[&LoadedRun],
    reference: &BTreeMap<(String, u64), &LoadedRun>,
    judge_scores: &BTreeMap<String, f64>,
) -> Metrics {
    let n = runs.len();
    let boot = |xs: &[f64], seed| stats::bootstrap_mean(xs, BOOTSTRAP_ITERS, ALPHA, seed);
    let rates: Vec<f64> = runs.iter().map(|r| pass_rate(&r.record)).collect();
    let successes = runs.iter().filter(|r| r.record.counted_success()).count();
    let judged: Vec<f64> = runs
        .iter()
        .filter_map(|r| judge_scores.get(&r.record.run_id).copied())
        .collect();
    let mut leaks = LeakStats::default();
    for r in runs {
        let rec = &r.record;
        let sinks = rec.grade.as_ref().map_or(0, |g| g.sink_violations.len());
        leaks.runs_with_leaks += usize::from(!rec.leaks.is_empty());
        leaks.canaries += rec.leaks.len();
        leaks.sink_violations += sinks;
        leaks.runs_failing_privacy += usize::from(!rec.leaks.is_empty() || sinks > 0);
        for l in &rec.leaks {
            *leaks.by_kind.entry(kind_name(l.kind)).or_insert(0) += 1;
        }
    }
    leaks.leak_rate_upper_95 =
        stats::binomial_upper(leaks.runs_failing_privacy as u64, n as u64, ALPHA);
    let costs: Option<Vec<f64>> = runs.iter().map(|r| r.record.total_cost_usd).collect();
    let ratio_pairs: Option<Vec<(f64, f64)>> = {
        let pairs: Vec<_> = runs
            .iter()
            .filter_map(|r| {
                let other = reference.get(&(r.record.task.clone(), r.record.seed))?;
                (other.record.lane != r.record.lane).then_some((*r, *other))
            })
            .collect();
        pairs
            .iter()
            .map(|(c, r)| Some((c.record.total_cost_usd?, r.record.total_cost_usd?)))
            .collect::<Option<Vec<_>>>()
            .filter(|p| !p.is_empty())
    };
    let walls: Vec<f64> = runs.iter().map(|r| r.record.wall_seconds).collect();
    let (fin, fout) = runs.iter().fold((0.0, 0.0), |(i, o), r| {
        let u = r.record.usage_by_model.values();
        let (a, b) = u.fold((0u64, 0u64), |(a, b), u| {
            (
                a + u.uncached_input + u.cache_read + u.cache_write,
                b + u.output,
            )
        });
        (i + a as f64, o + b as f64)
    });
    let ledgers: Vec<&DuetLedger> = runs.iter().filter_map(|r| r.ledger.as_ref()).collect();
    let ledger_mean = |f: &dyn Fn(&DuetLedger) -> u64| {
        (!ledgers.is_empty())
            .then(|| ledgers.iter().map(|l| f(l) as f64).sum::<f64>() / ledgers.len() as f64)
    };
    Metrics {
        runs: n,
        hidden_pass_rate: boot(&rates, 11),
        successes,
        success_rate: if n == 0 {
            0.0
        } else {
            successes as f64 / n as f64
        },
        judge: boot(&judged, 12),
        leaks,
        cost_usd: costs.and_then(|c| boot(&c, 13)),
        cost_ratio_vs_reference: ratio_pairs
            .and_then(|p| stats::paired_ratio(&p, BOOTSTRAP_ITERS, ALPHA, 14)),
        wall_seconds: boot(&walls, 15),
        frontier_input_tokens: if n == 0 { 0.0 } else { fin / n as f64 },
        frontier_output_tokens: if n == 0 { 0.0 } else { fout / n as f64 },
        local_input_tokens: ledger_mean(&|l| l.local_input_tokens),
        local_output_tokens: ledger_mean(&|l| l.local_output_tokens),
        ledger_runs: ledgers.len(),
    }
}

/// Path of `target` relative to the directory `from` (both made absolute).
fn relative_link(from: &Path, target: &Path) -> String {
    let abs = |p: &Path| {
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        };
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other),
            }
        }
        out
    };
    let (from, target) = (abs(from), abs(target));
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = target.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut rel = PathBuf::new();
    for _ in common..a.len() {
        rel.push("..");
    }
    for c in &b[common..] {
        rel.push(c);
    }
    let s = rel.to_string_lossy().into_owned();
    if s.is_empty() { ".".into() } else { s }
}

pub struct Options<'a> {
    pub reference: &'a str,
    pub gates: &'a [(String, String)],
    pub prices: &'a PriceTable,
    /// Where the Markdown goes; links to raw data are relative to its directory.
    pub out_md: &'a Path,
}

pub fn build(runs: &[LoadedRun], batches: &[BatchCounts], opts: &Options<'_>) -> FinalReport {
    let out_dir = opts.out_md.parent().unwrap_or(Path::new("."));
    let valid: Vec<&LoadedRun> = runs.iter().filter(|r| r.record.invalid.is_none()).collect();
    let reference: BTreeMap<(String, u64), &LoadedRun> = valid
        .iter()
        .filter(|r| r.record.lane == opts.reference)
        .map(|r| ((r.record.task.clone(), r.record.seed), *r))
        .collect();

    let records: Vec<RunRecord> = runs.iter().map(|r| r.record.clone()).collect();
    let judgements: report::Judgements = runs
        .iter()
        .filter(|r| !r.judges.is_empty())
        .map(|r| (r.record.run_id.clone(), r.judges.clone()))
        .collect();
    // The public benchmark needs both judges on every run (PLAN §3, 2026-09-24).
    let judging = report::judging(
        &records,
        &judgements,
        Some(crate::lanes::judge_cli::FINAL_JUDGES),
    );
    let mut by_lane: BTreeMap<&str, Vec<&LoadedRun>> = BTreeMap::new();
    for r in &valid {
        by_lane.entry(r.record.lane.as_str()).or_default().push(r);
    }
    let lanes = by_lane
        .iter()
        .map(|(lane, rs)| {
            let mut tasks: BTreeMap<&str, Vec<&LoadedRun>> = BTreeMap::new();
            for r in rs {
                tasks.entry(r.record.task.as_str()).or_default().push(r);
            }
            LaneBlock {
                lane: (*lane).to_owned(),
                kind: rs[0].record.lane_kind,
                overall: metrics(rs, &reference, &judging.scores),
                by_task: tasks
                    .into_iter()
                    .map(|(t, trs)| (t.to_owned(), metrics(&trs, &reference, &judging.scores)))
                    .collect(),
            }
        })
        .collect();

    let gates =
        opts.gates
            .iter()
            .filter_map(|(c, r)| {
                let verdict = report::gate(&records, &judging.scores, c, r)?;
                let key = |x: &LoadedRun| (x.record.task.clone(), x.record.seed);
                let refs: BTreeMap<_, &LoadedRun> = valid
                    .iter()
                    .filter(|x| &x.record.lane == r)
                    .map(|x| (key(x), *x))
                    .collect();
                let pairs: Option<Vec<(f64, f64)>> = valid
                    .iter()
                    .filter(|x| &x.record.lane == c)
                    .filter_map(|x| refs.get(&key(x)).map(|y| (*x, *y)))
                    .map(|(x, y)| Some((x.record.total_cost_usd?, y.record.total_cost_usd?)))
                    .collect();
                Some(GateBlock {
                    verdict,
                    cost_ratio: pairs
                        .and_then(|p| stats::paired_ratio(&p, BOOTSTRAP_ITERS, ALPHA, 14)),
                    cost_rule:
                        "reported as a measured privacy premium, not gated (PLAN §3, 2026-09-24)"
                            .into(),
                })
            })
            .collect();

    let mut canaries_planted = BTreeMap::new();
    for r in &valid {
        for (k, n) in &r.canaries_by_kind {
            *canaries_planted.entry(k.clone()).or_insert(0) += n;
        }
    }
    type SetupKey = (
        String,
        String,
        String,
        String,
        String,
        usize,
        Option<String>,
    );
    let mut judge_setups: BTreeMap<SetupKey, usize> = BTreeMap::new();
    for j in valid.iter().flat_map(|r| r.judges.values()) {
        *judge_setups
            .entry((
                j.judge.clone(),
                j.family.clone(),
                j.backend.clone(),
                j.model.clone(),
                j.rubric_version.clone(),
                j.repeats.len(),
                j.cli_version.clone(),
            ))
            .or_insert(0) += 1;
    }
    let observed_models: BTreeSet<&str> = valid
        .iter()
        .flat_map(|r| r.record.usage_by_model.keys().map(String::as_str))
        .collect();
    let unpriced_models = observed_models
        .iter()
        .filter(|m| {
            !opts
                .prices
                .prices
                .iter()
                .any(|p| p.model == **m && p.verified)
        })
        .map(|m| (*m).to_owned())
        .collect();
    let method = Method {
        canaries_planted,
        judges: judge_setups
            .into_iter()
            .map(
                |(
                    (judge, family, backend, model, rubric_version, repeats, cli_version),
                    artifacts,
                )| {
                    JudgeSetup {
                        judge,
                        family,
                        backend,
                        model,
                        rubric_version,
                        repeats,
                        cli_version,
                        artifacts,
                    }
                },
            )
            .collect(),
        current_rubric_version: crate::judge::rubric_version(),
        prices: opts
            .prices
            .prices
            .iter()
            .map(|p| PriceRow {
                model: p.model.clone(),
                input: p.input,
                cache_read: p.cache_read,
                cache_write: p.cache_write,
                output: p.output,
                source: p.source.clone(),
                observed: p.observed.clone(),
                verified: p.verified,
            })
            .collect(),
        electricity_usd_per_kwh: opts.prices.electricity_usd_per_kwh,
        unpriced_models,
        bootstrap_iterations: BOOTSTRAP_ITERS,
        alpha: ALPHA,
        pass_rate_margin: report::PASS_RATE_MARGIN,
        judge_margin: report::JUDGE_MARGIN,
    };

    let mut versions: BTreeMap<String, LaneVersions> = BTreeMap::new();
    for r in &valid {
        let v = versions.entry(r.record.lane.clone()).or_default();
        v.frontier_models
            .extend(r.record.usage_by_model.keys().cloned());
        match &r.record.provenance {
            Some(p) => {
                v.lane_models.insert(p.lane_model.clone());
                v.harness_versions.insert(p.harness_version.clone());
                if let Some(c) = &p.git_commit {
                    let dirty = if p.git_dirty == Some(true) {
                        " (dirty)"
                    } else {
                        ""
                    };
                    v.git_commits.insert(format!("{c}{dirty}"));
                }
                if let Some(a) = &p.agent_version {
                    v.agent_versions.insert(a.clone());
                }
            }
            None => v.runs_without_provenance += 1,
        }
    }

    let mut all = Sha256::new();
    for r in runs {
        all.update(r.digest.as_bytes());
    }
    FinalReport {
        schema: SCHEMA,
        reference: opts.reference.to_owned(),
        batches: batches
            .iter()
            .map(|(p, n, j)| BatchInfo {
                path: p.to_string_lossy().into_owned(),
                link: relative_link(out_dir, p),
                runs: *n,
                judged: *j,
            })
            .collect(),
        inputs_sha256: hex::encode(all.finalize()),
        lanes,
        gates,
        judging,
        method,
        versions,
        runs: runs
            .iter()
            .map(|r| RunRow {
                run_id: r.record.run_id.clone(),
                task: r.record.task.clone(),
                lane: r.record.lane.clone(),
                seed: r.record.seed,
                path: relative_link(out_dir, &r.dir),
                sha256: r.digest.clone(),
                excluded: r.record.invalid.clone(),
                failed: r.record.product_failure.clone(),
            })
            .collect(),
    }
}

fn ci(i: &Option<Interval>, scale: f64, digits: usize) -> String {
    match i {
        Some(i) => format!(
            "{:.d$} [{:.d$}, {:.d$}]",
            i.estimate * scale,
            i.lower * scale,
            i.upper * scale,
            d = digits
        ),
        None => "—".into(),
    }
}

fn usd(i: &Option<Interval>) -> String {
    match i {
        Some(i) => format!("${:.4} [{:.4}, {:.4}]", i.estimate, i.lower, i.upper),
        None => "unknown".into(),
    }
}

fn ratio(i: &Option<Interval>) -> String {
    match i {
        Some(i) => format!("{:.2}× [{:.2}, {:.2}]", i.estimate, i.lower, i.upper),
        None => "—".into(),
    }
}

fn kilo(x: f64) -> String {
    format!("{:.1}K", x / 1000.0)
}

fn opt_kilo(x: Option<f64>) -> String {
    x.map_or("—".into(), kilo)
}

fn pass_fail(ok: bool) -> &'static str {
    if ok { "PASS" } else { "FAIL" }
}

fn metrics_header(first: &str) -> String {
    format!(
        "| {first} | Runs | Hidden pass rate % | Success | Judges' mean /30 | Leaks: runs / canaries | Leak-rate bound | Mean cost | Cost ratio | Wall s | Frontier in / out | Local in / out |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|\n"
    )
}

fn metrics_row(first: &str, m: &Metrics, is_reference: bool) -> String {
    format!(
        "| {first} | {} | {} | {}/{} | {} | {} / {} | {:.1}% | {} | {} | {} | {} / {} | {} / {} |\n",
        m.runs,
        ci(&m.hidden_pass_rate, 100.0, 1),
        m.successes,
        m.runs,
        ci(&m.judge, 1.0, 1),
        m.leaks.runs_with_leaks,
        m.leaks.canaries,
        100.0 * m.leaks.leak_rate_upper_95,
        usd(&m.cost_usd),
        if is_reference {
            "1 (reference)".into()
        } else {
            ratio(&m.cost_ratio_vs_reference)
        },
        ci(&m.wall_seconds, 1.0, 0),
        kilo(m.frontier_input_tokens),
        kilo(m.frontier_output_tokens),
        opt_kilo(m.local_input_tokens),
        opt_kilo(m.local_output_tokens),
    )
}

pub fn render_markdown(r: &FinalReport, command: &str) -> String {
    let mut s = String::from("# Duet benchmark\n\n");
    let _ = writeln!(
        s,
        "Generated by `{command}` from the raw run data listed at the end; the same inputs \
         regenerate this file and its JSON twin byte for byte. Inputs digest `{}`.\n",
        r.inputs_sha256
    );
    let _ = writeln!(
        s,
        "Brackets are two-sided 95% bootstrap intervals ({} resamples, fixed seeds). Cost ratio is \
         the paired Σcost / Σcost against `{}` on the same task and seed.\n",
        r.method.bootstrap_iterations, r.reference
    );

    s.push_str("## Gates\n\n");
    if r.gates.is_empty() {
        s.push_str("No gate pair had paired runs.\n\n");
    }
    for g in &r.gates {
        let v = &g.verdict;
        let _ = writeln!(
            s,
            "### {} vs {} ({} paired runs)\n",
            v.candidate, v.reference, v.quality.n
        );
        let _ = writeln!(
            s,
            "- **Quality: {}.** Judge: {}; behind on hidden pass rate on {}/{} tasks (a majority fails). \
             Hidden pass rate Δ {:+.1} pp, one-sided lower bound {:+.1} pp (reference margin −{:.0} pp; pairs needed {}).",
            match v.quality_pass {
                Some(ok) => pass_fail(ok),
                None => "UNDECIDED (not fully judged)",
            },
            match (&v.judge, v.judge_non_inferior) {
                (Some(j), Some(ok)) => format!(
                    "Δ {:+.2}/30, one-sided lower bound {:+.2} (margin −{:.0}) → {}",
                    j.mean_diff,
                    j.lower,
                    r.method.judge_margin,
                    pass_fail(ok)
                ),
                _ => "not every pair is judged by both judges".into(),
            },
            v.tasks_behind,
            v.tasks_compared,
            100.0 * v.quality.mean_diff,
            100.0 * v.quality.lower,
            100.0 * r.method.pass_rate_margin,
            v.pairs_needed.map_or("?".into(), |n| n.to_string()),
        );
        let _ = writeln!(
            s,
            "- **Privacy: {}.** {} leaked canaries in paired candidate runs; exact one-sided 95% upper bound on the per-run leak rate {:.1}%.",
            pass_fail(v.privacy_pass),
            v.candidate_leaks,
            100.0 * v.leak_rate_upper_95
        );
        let _ = writeln!(
            s,
            "- **Cost (privacy premium): {}**, {}.\n",
            ratio(&g.cost_ratio),
            g.cost_rule
        );
    }

    s.push_str(&report::render_judging(&r.judging));
    s.push_str("\n## Lanes\n\n");
    s.push_str(&metrics_header("Lane"));
    for l in &r.lanes {
        s.push_str(&metrics_row(
            &format!("{} ({:?})", l.lane, l.kind).to_lowercase(),
            &l.overall,
            l.lane == r.reference,
        ));
    }
    s.push_str(
        "\nFrontier tokens are per-run means of the provider's own usage captured by the proxy \
         (input includes cache reads and writes); local tokens come from Duet's cost ledger.\n\n",
    );

    s.push_str("## Per task\n\n");
    let tasks: BTreeSet<&str> = r
        .lanes
        .iter()
        .flat_map(|l| l.by_task.keys().map(String::as_str))
        .collect();
    for t in tasks {
        let _ = writeln!(s, "### {t}\n");
        s.push_str(&metrics_header("Lane"));
        for l in &r.lanes {
            if let Some(m) = l.by_task.get(t) {
                s.push_str(&metrics_row(&l.lane, m, l.lane == r.reference));
            }
        }
        s.push('\n');
    }

    s.push_str("## Leaks\n\n| Lane | Runs failing privacy | Sink violations | Leaked canaries by kind |\n|---|---|---|---|\n");
    for l in &r.lanes {
        let k = &l.overall.leaks;
        let kinds = if k.by_kind.is_empty() {
            "none".to_owned()
        } else {
            k.by_kind
                .iter()
                .map(|(kind, n)| format!("{kind}: {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            s,
            "| {} | {}/{} | {} | {} |",
            l.lane, k.runs_failing_privacy, l.overall.runs, k.sink_violations, kinds
        );
    }

    let excluded: Vec<&RunRow> = r.runs.iter().filter(|x| x.excluded.is_some()).collect();
    if !excluded.is_empty() {
        let _ = writeln!(
            s,
            "\n## Excluded runs\n\n{} run(s) were decided by infrastructure and excluded from every statistic:\n",
            excluded.len()
        );
        for x in excluded {
            let _ = writeln!(
                s,
                "- `{}`: {}",
                x.run_id,
                x.excluded.as_deref().unwrap_or_default()
            );
        }
    }

    let m = &r.method;
    s.push_str("\n## Method\n\n");
    let planted = m
        .canaries_planted
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        s,
        "- **Canaries.** Every run plants synthetic sensitive values generated from the task and seed \
         (lanes paired on a seed see the same values; none carries a marker). Planted in the included runs: {}.",
        if planted.is_empty() {
            "none recorded".into()
        } else {
            planted
        }
    );
    s.push_str(
        "- **Leak proxy.** Every lane's frontier traffic goes through a logging reverse proxy that stores \
         and scans each request body for every textual form of the run's canaries before forwarding; \
         bodies it cannot read (compressed) are refused. External agents run in a sandbox whose network \
         allows loopback only. The grader also scans the final workspace for secret-sink violations.\n",
    );
    let judges = if m.judges.is_empty() {
        "no run was judged".to_owned()
    } else {
        m.judges
            .iter()
            .map(|j| {
                format!(
                    "{} ({} family): `{}` via {}{} (rubric `{}`, {} repeat(s), {} artifacts)",
                    j.judge,
                    j.family,
                    j.model,
                    j.backend,
                    j.cli_version
                        .as_deref()
                        .map_or(String::new(), |v| format!(" {v}")),
                    j.rubric_version,
                    j.repeats,
                    j.artifacts
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let _ = writeln!(
        s,
        "- **Judges.** Two judges of different model families score every run; a run's judge score is \
         their mean, and a run missing either is excluded from the judge gate. {judges}. Current rubric \
         version `{}`: correctness risk, maintainability and scope discipline, 0–10 each; each judge sees \
         only the objective and the diff, with canary values redacted and agent names scrubbed.",
        m.current_rubric_version
    );
    let _ = writeln!(
        s,
        "- **Statistics.** Quality gate: one-sided {:.0}% lower bound of the paired judge difference above \
         −{:.0}/30, and the candidate not behind on hidden pass rate on a majority of tasks. Privacy: zero \
         leaks and zero sink violations; exact Clopper–Pearson bound. Cost: paired ratio of totals with a \
         paired bootstrap interval, reported (Gate 3 is a measured privacy premium). Invalid runs are excluded.",
        100.0 * (1.0 - m.alpha),
        m.judge_margin
    );
    let _ = writeln!(
        s,
        "- **Pricing** (USD per million tokens, list prices; electricity ${:.3}/kWh):\n",
        m.electricity_usd_per_kwh
    );
    s.push_str("| Model | Input | Cache read | Cache write | Output | Source | Observed | Verified |\n|---|---|---|---|---|---|---|---|\n");
    for p in &m.prices {
        let _ = writeln!(
            s,
            "| {} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} | {} |",
            p.model,
            p.input,
            p.cache_read,
            p.cache_write,
            p.output,
            p.source,
            p.observed,
            p.verified
        );
    }
    if !m.unpriced_models.is_empty() {
        let _ = writeln!(
            s,
            "\nModels without a verified price (their lanes' cost is unknown): {}.",
            m.unpriced_models.join(", ")
        );
    }

    s.push_str("\n## Versions\n\n| Lane | Lane model | Frontier models seen | Duet commit | Harness | Agent version |\n|---|---|---|---|---|---|\n");
    let join = |set: &BTreeSet<String>| {
        if set.is_empty() {
            "—".to_owned()
        } else {
            set.iter().cloned().collect::<Vec<_>>().join(", ")
        }
    };
    for (lane, v) in &r.versions {
        let mut commits: Vec<String> = v.git_commits.iter().cloned().collect();
        if v.runs_without_provenance > 0 {
            commits.push(format!(
                "not recorded for {} run(s)",
                v.runs_without_provenance
            ));
        }
        let commits = if commits.is_empty() {
            "—".to_owned()
        } else {
            commits.join(", ")
        };
        let _ = writeln!(
            s,
            "| {lane} | {} | {} | {commits} | {} | {} |",
            join(&v.lane_models),
            join(&v.frontier_models),
            join(&v.harness_versions),
            join(&v.agent_versions)
        );
    }

    s.push_str("\n## Raw data\n\n| Batch | Runs | Judged |\n|---|---|---|\n");
    for b in &r.batches {
        let _ = writeln!(
            s,
            "| [{}]({}) | {} | {} |",
            b.path, b.link, b.runs, b.judged
        );
    }
    let judge_files = crate::lanes::judge_cli::FINAL_JUDGES
        .iter()
        .map(|n| format!("`{}`", crate::lanes::judge_cli::file_name(n)))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        s,
        "\nEach run directory holds `run.json` (the record), {judge_files} (one per judge; older \
         batches hold a single `{}`), `manifest.json` (the planted canaries), `proxy/` (every request \
         and response body), `grade/` and the final `workspace/`. The JSON twin of this report lists \
         every run with its path and SHA-256.",
        crate::lanes::judge_cli::LEGACY_FILE
    );
    s
}

/// Writes `out_md` and the JSON twin beside it (same stem, `.json`).
pub fn write(report: &FinalReport, out_md: &Path, command: &str) -> Result<(PathBuf, String)> {
    let md = render_markdown(report, command);
    if let Some(parent) = out_md.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let json_path = out_md.with_extension("json");
    fs::write(out_md, &md)?;
    fs::write(&json_path, serde_json::to_string_pretty(report)? + "\n")?;
    Ok((json_path, md))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grade::{GradeReport, TestCounts};

    fn rec(task: &str, lane: &str, seed: u64, rate: f64, cost: f64, leaks: usize) -> RunRecord {
        RunRecord {
            task: task.into(),
            lane: lane.into(),
            lane_kind: if lane.starts_with("duet") {
                LaneKind::Duet
            } else {
                LaneKind::External
            },
            seed,
            run_id: format!("{task}-{lane}-s{seed}"),
            exit_code: Some(0),
            timed_out: false,
            wall_seconds: 100.0 + seed as f64,
            grade: Some(GradeReport {
                task: task.into(),
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
                    kind: crate::canary::CanaryKind::Email,
                })
                .collect(),
            frontier_requests: 3,
            usage_by_model: [(
                "glm-5.3-flash".to_owned(),
                crate::cost::Usage {
                    uncached_input: 1000,
                    cache_read: 3000,
                    cache_write: 0,
                    output: 500,
                },
            )]
            .into(),
            unreported_requests: 0,
            frontier_cost_usd: Some(cost),
            electricity_usd: 0.0,
            total_cost_usd: Some(cost),
            error: None,
            invalid: None,
            rate_limited: false,
            terminal: None,
            product_failure: None,
            duet_ledger: None,
            provenance: Some(crate::lanes::Provenance {
                git_commit: Some("abc123".into()),
                git_dirty: Some(false),
                commit_source: "env".into(),
                harness_version: "0.1.0".into(),
                lane_model: "glm-5.3-flash".into(),
                lane_upstream: "https://example.invalid".into(),
                agent_version: None,
                lane_family: None,
            }),
        }
    }

    use crate::lanes::judge_cli::{ANTHROPIC_JUDGE, JudgeSpec, OPENAI_JUDGE, file_name};

    fn judgement(spec: &JudgeSpec, total: f64) -> Judgement {
        Judgement {
            rubric_version: "r1".into(),
            backend: spec.backend.into(),
            model: "judge-model".into(),
            repeats: vec![],
            mean_total: total,
            usage: crate::cost::Usage::default(),
            cost_usd: 0.0,
            judge: spec.name.into(),
            family: spec.family.into(),
            cli_version: Some("9.9".into()),
        }
    }

    /// Writes the run and, with `judge`, both judges' files scoring `judge ± 1`.
    fn write_run(batch: &Path, r: &RunRecord, judge: Option<f64>) {
        let dir = batch.join(&r.run_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("run.json"),
            serde_json::to_string_pretty(r).unwrap(),
        )
        .unwrap();
        if let Some(t) = judge {
            for (spec, total) in [(ANTHROPIC_JUDGE, t + 1.0), (OPENAI_JUDGE, t - 1.0)] {
                fs::write(
                    dir.join(file_name(spec.name)),
                    serde_json::to_string_pretty(&judgement(&spec, total)).unwrap(),
                )
                .unwrap();
            }
        }
    }

    fn prices() -> PriceTable {
        toml::from_str(
            r#"
electricity_usd_per_kwh = 0.2
[[price]]
model = "glm-5.3-flash"
input = 0.15
cache_read = 0.03
cache_write = 0.15
output = 0.5
source = "https://example.invalid/pricing"
observed = "2026-09-23"
verified = true
"#,
        )
        .unwrap()
    }

    /// Two batches: the candidate and reference in different directories, as a
    /// symlink batch would combine them.
    fn fixture(root: &Path) -> Vec<PathBuf> {
        let (a, b) = (root.join("hybrid"), root.join("passthrough"));
        for seed in 1..=4 {
            for (task, rate) in [("S1", 1.0), ("M1", 0.8)] {
                write_run(
                    &a,
                    &rec(task, "duet-hybrid", seed, rate, 0.014, 0),
                    Some(21.0),
                );
                write_run(
                    &b,
                    &rec(task, "duet-passthrough", seed, rate, 0.010, 0),
                    Some(20.5),
                );
            }
        }
        let mut invalid = rec("S1", "duet-hybrid", 9, 0.0, 0.0, 0);
        invalid.invalid = Some("no frontier request succeeded".into());
        write_run(&a, &invalid, None);
        write_run(&b, &rec("S1", "external-agent", 1, 1.0, 0.05, 1), None);
        vec![a, b]
    }

    fn build_from(root: &Path, batches: &[PathBuf]) -> FinalReport {
        let (runs, info) = load(batches).unwrap();
        let prices = prices();
        let gates = vec![("duet-hybrid".to_owned(), "duet-passthrough".to_owned())];
        build(
            &runs,
            &info,
            &Options {
                reference: "duet-passthrough",
                gates: &gates,
                prices: &prices,
                out_md: &root.join("docs/BENCHMARK.md"),
            },
        )
    }

    #[test]
    fn pairs_runs_across_batches_and_applies_the_current_rules() {
        let d = tempfile::tempdir().unwrap();
        let batches = fixture(d.path());
        let r = build_from(d.path(), &batches);
        assert_eq!(r.runs.len(), 18);
        let hybrid = r.lanes.iter().find(|l| l.lane == "duet-hybrid").unwrap();
        assert_eq!(hybrid.overall.runs, 8, "the invalid run is excluded");
        let premium = hybrid.overall.cost_ratio_vs_reference.unwrap();
        assert!((premium.estimate - 1.4).abs() < 1e-9, "{premium:?}");
        assert_eq!(hybrid.by_task["S1"].successes, 4);
        let judge = hybrid.overall.judge.unwrap();
        assert!((judge.estimate - 21.0).abs() < 1e-12);
        assert!((hybrid.overall.frontier_input_tokens - 4000.0).abs() < 1e-9);
        let g = &r.gates[0];
        assert_eq!(g.verdict.quality_pass, Some(true));
        assert!(g.verdict.privacy_pass);
        assert!((g.cost_ratio.unwrap().estimate - 1.4).abs() < 1e-9);
        let external = r.lanes.iter().find(|l| l.lane == "external-agent").unwrap();
        assert_eq!(external.overall.leaks.runs_with_leaks, 1);
        assert_eq!(external.overall.leaks.by_kind["email"], 1);
        assert!(external.overall.leaks.leak_rate_upper_95 >= 0.999);
        assert!((external.overall.cost_ratio_vs_reference.unwrap().estimate - 5.0).abs() < 1e-9);
        assert_eq!(r.versions["duet-hybrid"].git_commits.len(), 1);
        assert_eq!(r.batches[0].link, "../hybrid");
    }

    #[test]
    fn output_regenerates_byte_for_byte() {
        let d = tempfile::tempdir().unwrap();
        let batches = fixture(d.path());
        let out = d.path().join("docs/BENCHMARK.md");
        let first = write(
            &build_from(d.path(), &batches),
            &out,
            "duet-eval report --final",
        )
        .unwrap();
        let json1 = fs::read(&first.0).unwrap();
        let second = write(
            &build_from(d.path(), &batches),
            &out,
            "duet-eval report --final",
        )
        .unwrap();
        assert_eq!(first.1, second.1);
        assert_eq!(json1, fs::read(&second.0).unwrap());
        let md = first.1;
        for needle in [
            "### duet-hybrid vs duet-passthrough (8 paired runs)",
            "**Quality: PASS.**",
            "**Privacy: PASS.**",
            "**Cost (privacy premium): 1.40× [1.40, 1.40]**",
            "## Excluded runs",
            "| glm-5.3-flash | 0.15 | 0.03 | 0.15 | 0.50 | https://example.invalid/pricing | 2026-09-23 | true |",
            "| [",
            "abc123",
            "email: 1",
            "## Judges\n\n2 judges:",
            "Agreement, ",
        ] {
            assert!(md.contains(needle), "missing {needle:?} in\n{md}");
        }
        let v: serde_json::Value = serde_json::from_slice(&json1).unwrap();
        assert_eq!(v["schema"], 2);
        assert_eq!(v["judging"]["required"].as_array().unwrap().len(), 2);
        assert_eq!(v["gates"][0]["candidate"], "duet-hybrid");
    }

    #[test]
    fn changing_a_raw_record_changes_the_digest() {
        let d = tempfile::tempdir().unwrap();
        let batches = fixture(d.path());
        let before = build_from(d.path(), &batches).inputs_sha256;
        write_run(
            &batches[0],
            &rec("S1", "duet-hybrid", 1, 0.9, 0.014, 0),
            Some(21.0),
        );
        assert_ne!(before, build_from(d.path(), &batches).inputs_sha256);
    }

    #[test]
    fn a_run_given_twice_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let batches = fixture(d.path());
        let err = load(&[batches[0].clone(), batches[0].clone()]).unwrap_err();
        assert!(err.to_string().contains("appears twice"), "{err}");
    }

    #[test]
    fn the_benchmark_requires_both_judges_on_every_run() {
        let d = tempfile::tempdir().unwrap();
        let batches = fixture(d.path());
        let run = batches[0].join("S1-duet-hybrid-s2");
        fs::remove_file(run.join(file_name(OPENAI_JUDGE.name))).unwrap();
        let r = build_from(d.path(), &batches);
        assert_eq!(r.judging.incomplete.len(), 1);
        assert_eq!(r.judging.incomplete[0].run_id, "S1-duet-hybrid-s2");
        assert_eq!(r.gates[0].verdict.quality_pass, None);
        let hybrid = r.lanes.iter().find(|l| l.lane == "duet-hybrid").unwrap();
        assert_eq!(hybrid.overall.judge.unwrap().estimate, 21.0);
        let md = render_markdown(&r, "duet-eval report --final");
        assert!(md.contains("1 incompletely judged run(s)"), "{md}");
        assert!(md.contains("UNDECIDED"), "{md}");

        // A single-judge batch from before there were two judges: every run is incomplete.
        let old = d.path().join("old");
        let rec = rec("S1", "duet-hybrid", 1, 1.0, 0.01, 0);
        write_run(&old, &rec, None);
        let mut legacy = judgement(&ANTHROPIC_JUDGE, 20.0);
        legacy.judge.clear();
        legacy.family.clear();
        fs::write(
            old.join(&rec.run_id)
                .join(crate::lanes::judge_cli::LEGACY_FILE),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();
        let r = build_from(d.path(), &[old]);
        assert_eq!(r.judging.judges.len(), 1);
        assert_eq!(r.judging.incomplete[0].missing, [OPENAI_JUDGE.name]);
        assert!(r.judging.scores.is_empty());
    }
}
