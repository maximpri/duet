// SPDX-License-Identifier: GPL-3.0-or-later
//! Duet evaluation harness: dogfood tasks, canaries, leak proxy, grading,
//! judging and statistics.

mod canary;
mod cost;
mod grade;
mod judge;
mod lanes;
mod leakproxy;
mod ledger;
mod report;
mod stats;
mod task;
mod workspace;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "duet-eval", about = "Duet evaluation harness")]
struct Cli {
    /// Directory holding the dogfood task packages.
    #[arg(long, global = true, default_value = "tasks")]
    tasks: PathBuf,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Validate every task package and its seal.
    Validate,
    /// Recompute a task's seal after an intentional change.
    Seal { task: String },
    /// Check that a task is sound: the starter fails the hidden tests and the
    /// reference solution passes all of them.
    Check { task: String },
    /// Render a task workspace for inspection.
    Prepare {
        task: String,
        seed: u64,
        dest: PathBuf,
    },
    /// Grade an existing workspace prepared with `prepare <task> <seed>`.
    Grade {
        task: String,
        seed: u64,
        workspace: PathBuf,
    },
    /// Run lanes over tasks and seeds.
    Run {
        #[arg(long, value_delimiter = ',')]
        lanes: Vec<String>,
        #[arg(long = "task", value_delimiter = ',')]
        task_ids: Vec<String>,
        /// Seeds, e.g. 1-5 or 1,2,7.
        #[arg(long, default_value = "1")]
        seeds: String,
        #[arg(long)]
        out: PathBuf,
        /// Alternative lane definitions.
        #[arg(long)]
        lanes_file: Option<PathBuf>,
        #[arg(long, default_value = "crates/duet-evals/pricing.toml")]
        prices: PathBuf,
        /// Run agents without the network/filesystem sandbox (not for gates).
        #[arg(long)]
        no_sandbox: bool,
        /// Average watts drawn by the local model host while generating.
        #[arg(long, default_value_t = 120.0)]
        local_watts: f64,
    },
    /// Judge the code quality of every run in a batch.
    Judge {
        batch: PathBuf,
        /// Judge backend: a logged-in assistant CLI (see lanes/judge_cli.rs) or `api` (ANTHROPIC_API_KEY).
        #[arg(long, default_value = "claude-cli")]
        backend: String,
        #[arg(long, default_value = "claude-opus-5-5")]
        model: String,
        #[arg(long, default_value_t = 2)]
        repeats: usize,
    },
    /// Summarize a batch and apply the gates.
    Report {
        batch: PathBuf,
        /// Gate pairs as candidate:reference, e.g. duet-hybrid:duet-passthrough.
        #[arg(long, value_delimiter = ',')]
        gate: Vec<String>,
    },
    /// Harness self-test: proxy, canaries, statistics and pricing.
    Selftest,
}

fn parse_seeds(spec: &str) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u64, u64) = (a.trim().parse()?, b.trim().parse()?);
            ensure!(a <= b, "bad seed range {part}");
            out.extend(a..=b);
        } else {
            out.push(part.trim().parse()?);
        }
    }
    Ok(out)
}

fn load_task(tasks: &Path, id: &str) -> Result<task::TaskPackage> {
    task::load_all(tasks)?
        .into_iter()
        .find(|t| t.spec.id == id)
        .with_context(|| format!("no task {id} under {}", tasks.display()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Validate => {
            let all = task::load_all(&cli.tasks)?;
            for t in &all {
                let kinds: Vec<String> = t
                    .spec
                    .sensitive
                    .iter()
                    .map(|s| format!("{}={:?}", s.path, s.kind))
                    .collect();
                println!("      {} sensitive: {}", t.spec.id, kinds.join(", "));
            }
            ensure!(!all.is_empty(), "no tasks under {}", cli.tasks.display());
            let mut failed = false;
            for t in &all {
                match t.verify_seal() {
                    Ok(()) => println!("ok    {} {} ({:?})", t.spec.id, t.spec.name, t.spec.tier),
                    Err(e) => {
                        failed = true;
                        println!("FAIL  {}: {e:#}", t.spec.id);
                    }
                }
            }
            ensure!(!failed, "some task packages are invalid");
        }
        Cmd::Seal { task } => {
            let t = load_task(&cli.tasks, &task)?;
            t.write_seal()?;
            println!("sealed {}", t.spec.id);
        }
        Cmd::Check { task } => check_task(&load_task(&cli.tasks, &task)?)?,
        Cmd::Prepare { task, seed, dest } => {
            let t = load_task(&cli.tasks, &task)?;
            let p = workspace::prepare(&t, seed, &format!("{task}-inspect-s{seed}"), &dest)?;
            println!(
                "prepared {} with {} canaries",
                dest.display(),
                p.manifest.canaries.len()
            );
        }
        Cmd::Grade {
            task,
            seed,
            workspace,
        } => {
            let t = load_task(&cli.tasks, &task)?;
            let mut g = canary::Generator::new(&t.spec.id, seed);
            // Re-render the task files to regenerate the same canaries as `prepare`.
            for layer in ["starter", "assets"] {
                let base = t.root.join(layer);
                if base.is_dir() {
                    for f in task::walk_files(&base)? {
                        if let Ok(text) = fs::read_to_string(&f) {
                            g.render(&text)?;
                        }
                    }
                }
            }
            let manifest = g.manifest("grade", seed);
            let scratch =
                std::env::temp_dir().join(format!("duet-eval-grade-{}", std::process::id()));
            let report = grade::grade(
                &t,
                &workspace,
                &manifest,
                &scratch,
                Duration::from_secs(900),
            )?;
            let _ = fs::remove_dir_all(&scratch);
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::Run {
            lanes: lane_names,
            task_ids,
            seeds,
            out,
            lanes_file,
            prices,
            no_sandbox,
            local_watts,
        } => {
            let all_lanes = lanes::load_lanes(lanes_file.as_deref())?;
            let prices = cost::PriceTable::load(&prices)?;
            let seeds = parse_seeds(&seeds)?;
            fs::create_dir_all(&out)?;
            for id in &task_ids {
                let t = load_task(&cli.tasks, id)?;
                for &seed in &seeds {
                    for name in &lane_names {
                        let lane = lanes::find_lane(&all_lanes, name)?;
                        let run_dir = out.join(format!("{}-{}-s{seed}", t.spec.id, lane.name));
                        if let Ok(text) = fs::read_to_string(run_dir.join("run.json")) {
                            let prev: lanes::RunRecord = serde_json::from_str(&text)?;
                            if prev.invalid.is_none() {
                                println!("{:<28} done earlier; skipped", prev.run_id);
                                continue;
                            }
                        }
                        let mut attempt = 0;
                        let rec = loop {
                            if run_dir.exists() {
                                let stamp = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)?
                                    .as_secs();
                                fs::rename(
                                    &run_dir,
                                    run_dir.with_extension(format!("invalid-{stamp}")),
                                )?;
                            }
                            let rec = lanes::run_one(lanes::RunConfig {
                                package: &t,
                                lane,
                                seed,
                                out_dir: &out,
                                prices: &prices,
                                sandbox: !no_sandbox,
                                local_watts,
                            })
                            .await?;
                            attempt += 1;
                            if rec.invalid.is_some() && rec.rate_limited && attempt < 3 {
                                eprintln!(
                                    "{}: invalid ({}); waiting for the provider",
                                    rec.run_id,
                                    rec.invalid.as_deref().unwrap_or_default()
                                );
                                if lanes::wait_until_available(lane, Duration::from_secs(6 * 3600))
                                    .await
                                {
                                    continue;
                                }
                            }
                            break rec;
                        };
                        println!(
                            "{:<28} pass {:>5.1}%  leaks {}  cost {}  {:.0}s{}",
                            rec.run_id,
                            100.0 * rec.grade.as_ref().map_or(0.0, |g| g.hidden_pass_rate),
                            rec.leaks.len(),
                            rec.total_cost_usd
                                .map_or("unknown".into(), |c| format!("${c:.4}")),
                            rec.wall_seconds,
                            rec.error
                                .as_deref()
                                .map_or(String::new(), |e| format!("  error: {e}"))
                        );
                    }
                }
            }
        }
        Cmd::Judge {
            batch,
            backend,
            model,
            repeats,
        } => {
            let backend = match backend.as_str() {
                "api" => judge::Backend::Api(judge::JudgeClient::from_env(&model)?),
                cli => judge::Backend::External(lanes::judge_cli::JudgeCli::parse(cli, &model)?),
            };
            judge_batch(&cli.tasks, &batch, &backend, repeats).await?
        }
        Cmd::Report { batch, gate } => {
            let records = report::load_records(&batch)?;
            ensure!(!records.is_empty(), "no runs in {}", batch.display());
            let summaries = report::summarize(&records);
            let judges = report::load_judges(&batch)?;
            let mut verdicts = Vec::new();
            for g in &gate {
                let (c, r) = g
                    .split_once(':')
                    .context("gate must be candidate:reference")?;
                match report::gate(&records, &judges, c, r) {
                    Some(v) => verdicts.push(v),
                    None => eprintln!("no paired runs for {g}"),
                }
            }
            let invalid: Vec<&lanes::RunRecord> =
                records.iter().filter(|r| r.invalid.is_some()).collect();
            let md = report::render_markdown(&summaries, &verdicts, &invalid);
            fs::write(batch.join("report.md"), &md)?;
            fs::write(
                batch.join("verdicts.json"),
                serde_json::to_string_pretty(&verdicts)?,
            )?;
            print!("{md}");
        }
        Cmd::Selftest => selftest().await?,
    }
    Ok(())
}

fn check_task(t: &task::TaskPackage) -> Result<()> {
    t.verify_seal()?;
    let tmp = std::env::temp_dir().join(format!(
        "duet-eval-check-{}-{}",
        t.spec.id,
        std::process::id()
    ));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    let timeout = Duration::from_secs(15 * 60);
    let starter = tmp.join("starter");
    let p = workspace::prepare(t, 1, "check", &starter)?;
    let base = grade::grade(t, &starter, &p.manifest, &tmp.join("g1"), timeout)?;
    println!(
        "starter:   hidden {}/{} (must be below 100%)",
        base.hidden.passed, base.hidden_expected
    );
    let reference = t.root.join("reference");
    ensure!(
        reference.is_dir(),
        "{}: missing reference/ solution",
        t.spec.id
    );
    let solved = tmp.join("solved");
    grade::stage(&starter, &[reference], &solved)?;
    // A reference may delete starter files it replaces (one relative path per line).
    let deletions = solved.join(".reference-deletes");
    if deletions.is_file() {
        for rel in fs::read_to_string(&deletions)?
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            let target = solved.join(rel);
            ensure!(
                target.starts_with(&solved) && !rel.contains(".."),
                "bad deletion path {rel}"
            );
            if target.is_file() {
                fs::remove_file(&target)?;
            }
        }
        fs::remove_file(&deletions)?;
    }
    let sol = grade::grade(t, &solved, &p.manifest, &tmp.join("g2"), timeout)?;
    println!(
        "reference: hidden {}/{} (must be 100%), visible {} passed / {} failed, sink violations {}",
        sol.hidden.passed,
        sol.hidden_expected,
        sol.visible.passed,
        sol.visible.failed,
        sol.sink_violations.len()
    );
    fs::remove_dir_all(&tmp)?;
    ensure!(
        !base.success,
        "{}: the starter already passes the hidden tests",
        t.spec.id
    );
    ensure!(
        sol.success && sol.hidden.passed == t.spec.hidden_test_count,
        "{}: the reference does not pass exactly {} hidden tests",
        t.spec.id,
        t.spec.hidden_test_count
    );
    ensure!(
        sol.sink_violations.is_empty(),
        "{}: reference violates secret sinks",
        t.spec.id
    );
    println!("check ok: {}", t.spec.id);
    Ok(())
}

async fn judge_batch(
    tasks: &Path,
    batch: &Path,
    client: &judge::Backend,
    repeats: usize,
) -> Result<()> {
    for rec in report::load_records(batch)? {
        let run_dir = batch.join(&rec.run_id);
        let out = run_dir.join("judge.json");
        if out.exists() {
            continue;
        }
        let t = load_task(tasks, &rec.task)?;
        let manifest = lanes::manifest_of(&run_dir)?;
        let baseline = std::env::temp_dir().join(format!("duet-eval-base-{}", rec.run_id));
        if baseline.exists() {
            fs::remove_dir_all(&baseline)?;
        }
        workspace::prepare(&t, rec.seed, &rec.run_id, &baseline)?;
        let d = judge::diff(&baseline, &run_dir.join("workspace"))?;
        fs::remove_dir_all(&baseline)?;
        if d.trim().is_empty() {
            println!("{}: no changes; skipped", rec.run_id);
            continue;
        }
        let j = client
            .judge(&t.objective, &judge::scrub(&d, &manifest), repeats)
            .await?;
        println!("{}: {:.1}/30", rec.run_id, j.mean_total);
        fs::write(out, serde_json::to_string_pretty(&j)?)?;
    }
    Ok(())
}

async fn selftest() -> Result<()> {
    use canary::{CanaryKind, Generator};
    // 1. Canaries are deterministic and detectable.
    let mut g = Generator::new("selftest", 1);
    let key = g.get(CanaryKind::Secret, "KEY");
    let manifest = g.manifest("selftest", 1);
    ensure!(
        manifest.find_in(&format!("x {} y", key.value)).len() == 1,
        "canary detection"
    );
    // 2. The proxy catches a planted canary against a dead upstream.
    let dir = std::env::temp_dir().join(format!("duet-eval-selftest-{}", std::process::id()));
    let proxy =
        leakproxy::start("127.0.0.1:0".parse()?, "http://127.0.0.1:9", manifest, &dir).await?;
    let _ = reqwest::Client::new()
        .post(format!("{}/v1/chat", proxy.base_url()))
        .body(format!("{{\"content\":\"{}\"}}", key.value))
        .send()
        .await?;
    proxy.stop();
    let leaks = leakproxy::read_leaks(&dir)?;
    fs::remove_dir_all(&dir)?;
    ensure!(leaks.len() == 1, "proxy did not record the planted canary");
    // 3. Statistics reproduce a known interval.
    let upper = stats::binomial_upper(0, 60, 0.05);
    ensure!(
        (upper - (1.0 - 0.05_f64.powf(1.0 / 60.0))).abs() < 1e-6,
        "binomial bound"
    );
    // 4. Prices load and are verified.
    let prices = cost::PriceTable::load(Path::new("crates/duet-evals/pricing.toml"))?;
    if prices.prices.iter().any(|p| !p.verified) {
        bail!("pricing.toml contains unverified prices");
    }
    for p in &prices.prices {
        println!("price {} observed {} ({})", p.model, p.observed, p.source);
    }
    println!("selftest ok: canaries, proxy, statistics, pricing");
    Ok(())
}
