// SPDX-License-Identifier: GPL-3.0-or-later
//! Duet evaluation harness: dogfood tasks, canaries, leak proxy, grading,
//! judging and statistics.

mod benchmark;
mod canary;
mod cost;
mod grade;
mod judge;
mod lanes;
mod leakproxy;
mod ledger;
mod profile;
mod report;
mod stats;
mod task;
mod workspace;
mod wsframe;

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
    /// Judge the code quality of every run in a batch, once per judge; each
    /// judge's result is stored separately in the run directory.
    Judge {
        batch: PathBuf,
        /// Judges: logged-in assistant CLIs of two model families (see
        /// lanes/judge_cli.rs), or `api` (ANTHROPIC_API_KEY) in place of the Claude CLI.
        #[arg(long, value_delimiter = ',', default_value = lanes::judge_cli::DEFAULT_JUDGES)]
        judges: Vec<String>,
        /// A judge's model as backend=model, e.g. claude-cli=claude-opus-5-5 (repeatable);
        /// every judge has a default (lanes/judge_cli.rs).
        #[arg(long = "model")]
        models: Vec<String>,
        #[arg(long, default_value_t = 2)]
        repeats: usize,
        /// Judge again runs a judge has already scored (its file is replaced).
        #[arg(long)]
        rejudge: bool,
    },
    /// Summarize a batch and apply the gates. With --final, write the public
    /// benchmark (Markdown and JSON) from one or more batches, paired by task, seed and lane.
    Report {
        #[arg(required = true, num_args = 1..)]
        batches: Vec<PathBuf>,
        /// Gate pairs as candidate:reference, e.g. duet-hybrid:duet-passthrough
        /// (with --final the default is duet-hybrid:duet-passthrough).
        #[arg(long, value_delimiter = ',')]
        gate: Vec<String>,
        /// Write the public benchmark report instead of a batch summary.
        #[arg(long = "final")]
        final_report: bool,
        /// Benchmark Markdown path; the JSON twin is written beside it.
        #[arg(long, default_value = "docs/BENCHMARK.md")]
        out: PathBuf,
        /// Reference lane for cost ratios (the privacy premium).
        #[arg(long, default_value = "duet-passthrough")]
        reference: String,
        #[arg(long, default_value = "crates/duet-evals/pricing.toml")]
        prices: PathBuf,
        /// Also price every run's recorded tokens at these list prices (batch
        /// report): `flagships` (glm-5.3, claude-opus-5-5, gpt-5.5), model names
        /// in --prices, or the path of another price table.
        #[arg(long = "project-prices", value_delimiter = ',')]
        project_prices: Vec<String>,
    },
    /// Check lanes' prerequisites without calling any model: program and version,
    /// required environment (values never printed), login files, proxy routing,
    /// and DNS for the proxy's upstream.
    Preflight {
        #[arg(long, value_delimiter = ',', required = true)]
        lanes: Vec<String>,
        #[arg(long)]
        lanes_file: Option<PathBuf>,
        /// Skip the DNS lookup of each lane's upstream.
        #[arg(long)]
        no_dns: bool,
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
    task::load_one(tasks, id)
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
            // Every requested task spec is loaded before the first run, and other
            // packages are never parsed, so adding a task mid-batch cannot stop it.
            let packages = task_ids
                .iter()
                .map(|id| load_task(&cli.tasks, id))
                .collect::<Result<Vec<_>>>()?;
            for t in &packages {
                for &seed in &seeds {
                    for name in &lane_names {
                        let lane = lanes::find_lane(&all_lanes, name)?;
                        let run_dir = out.join(format!("{}-{}-s{seed}", t.spec.id, lane.name));
                        if run_dir.join("run.json").is_file() {
                            let prev = lanes::load_record(&run_dir)?;
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
                                package: t,
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
                            100.0 * rec.counted_pass_rate(),
                            match rec.leaks_unmeasured {
                                None => rec.leaks.len().to_string(),
                                Some(_) => format!("not measured ({} seen)", rec.leaks.len()),
                            },
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
            judges,
            models,
            repeats,
            rejudge,
        } => {
            let backends = judges
                .iter()
                .map(|name| {
                    let spec = lanes::judge_cli::spec_for_backend(name)?;
                    judge::Backend::new(&spec, &lanes::judge_cli::model_for(&spec, &models)?)
                })
                .collect::<Result<Vec<_>>>()?;
            judge_batch(&cli.tasks, &batch, &backends, repeats, rejudge).await?
        }
        Cmd::Report {
            batches,
            gate,
            final_report,
            out,
            reference,
            prices,
            project_prices,
        } => {
            let pairs = gate
                .iter()
                .map(|g| {
                    g.split_once(':')
                        .map(|(c, r)| (c.to_owned(), r.to_owned()))
                        .context("gate must be candidate:reference")
                })
                .collect::<Result<Vec<_>>>()?;
            if final_report {
                ensure!(
                    project_prices.is_empty(),
                    "--project-prices applies to a batch report (run it per batch)"
                );
                let pairs = if pairs.is_empty() {
                    vec![("duet-hybrid".to_owned(), "duet-passthrough".to_owned())]
                } else {
                    pairs
                };
                final_benchmark(&batches, &pairs, &reference, &prices, &out)?;
            } else {
                let [batch] = batches.as_slice() else {
                    bail!("several batches need --final");
                };
                batch_report(batch, &pairs, &reference, &prices, &project_prices)?;
            }
        }
        Cmd::Preflight {
            lanes: names,
            lanes_file,
            no_dns,
        } => {
            let all = lanes::load_lanes(lanes_file.as_deref())?;
            let home = lanes::operator_home()?;
            let env = |k: &str| std::env::var(k).ok();
            let cx = lanes::preflight::Context {
                operator_home: &home,
                env: &env,
                resolve: !no_dns,
            };
            let mut failed = Vec::new();
            for name in &names {
                let lane = lanes::find_lane(&all, name)?;
                let checks = lanes::preflight::check(lane, &cx).await;
                print!("{}", lanes::preflight::render(lane, &checks));
                if checks
                    .iter()
                    .any(|c| c.status == lanes::preflight::Status::Fail)
                {
                    failed.push(name.as_str());
                }
            }
            ensure!(
                failed.is_empty(),
                "preflight failed for {}",
                failed.join(", ")
            );
            println!("preflight ok: {}", names.join(", "));
        }
        Cmd::Selftest => selftest().await?,
    }
    Ok(())
}

fn batch_report(
    batch: &Path,
    gate: &[(String, String)],
    reference: &str,
    prices: &Path,
    project_prices: &[String],
) -> Result<()> {
    let prices = cost::PriceTable::load(prices)?;
    let tables = profile::resolve_tables(project_prices, &prices)?;
    let records = report::load_records(batch)?;
    ensure!(!records.is_empty(), "no runs in {}", batch.display());
    let summaries = report::summarize(&records);
    // A batch report requires every judge seen in the batch; `report --final` requires both.
    let judging = report::judging(&records, &report::load_judges(batch)?, None);
    let mut verdicts = Vec::new();
    for (c, r) in gate {
        match report::gate(&records, &judging.scores, c, r) {
            Some(v) => verdicts.push(v),
            None => eprintln!("no paired runs for {c}:{r}"),
        }
    }
    let invalid: Vec<&lanes::RunRecord> = records.iter().filter(|r| r.invalid.is_some()).collect();
    let failed: Vec<&lanes::RunRecord> = records
        .iter()
        .filter(|r| r.invalid.is_none() && r.product_failure.is_some())
        .collect();
    let mut md = report::render_markdown(&summaries, &verdicts, &invalid, &failed, &judging);
    // The cost profile, and prices projected when asked (M5.2 instrumentation).
    let profiles = profile::load_batch(batch, &records, &prices)?;
    let lanes = profile::summarize_all(&profiles);
    md.push_str(&profile::render_profile(&lanes));
    let projections = (!tables.is_empty()).then(|| profile::project(&profiles, &tables, reference));
    if let Some(p) = &projections {
        md.push_str(&profile::render_projection(p));
    }
    fs::write(
        batch.join("cost-profile.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "summaries": lanes,
            "projections": projections,
            "runs": profiles,
        }))?,
    )?;
    fs::write(batch.join("report.md"), &md)?;
    fs::write(
        batch.join("verdicts.json"),
        serde_json::to_string_pretty(&verdicts)?,
    )?;
    fs::write(
        batch.join("judging.json"),
        serde_json::to_string_pretty(&judging)?,
    )?;
    print!("{md}");
    Ok(())
}

fn final_benchmark(
    batches: &[PathBuf],
    gates: &[(String, String)],
    reference: &str,
    prices: &Path,
    out: &Path,
) -> Result<()> {
    let prices = cost::PriceTable::load(prices)?;
    let (runs, info) = benchmark::load(batches)?;
    ensure!(!runs.is_empty(), "no runs in the given batches");
    let report = benchmark::build(
        &runs,
        &info,
        &benchmark::Options {
            reference,
            gates,
            prices: &prices,
            out_md: out,
        },
    );
    let mut command = String::from("duet-eval report --final");
    for b in batches {
        command.push(' ');
        command.push_str(&b.to_string_lossy());
    }
    let (json, _) = benchmark::write(&report, out, &command)?;
    println!(
        "wrote {} and {} ({} runs, inputs {})",
        out.display(),
        json.display(),
        report.runs.len(),
        &report.inputs_sha256[..16]
    );
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
    judges: &[judge::Backend],
    repeats: usize,
    rejudge: bool,
) -> Result<()> {
    let written = judge::judge_runs(batch, judges, repeats, rejudge, |rec, run_dir| {
        let t = load_task(tasks, &rec.task)?;
        let manifest = lanes::manifest_of(run_dir)?;
        let baseline = std::env::temp_dir().join(format!("duet-eval-base-{}", rec.run_id));
        if baseline.exists() {
            fs::remove_dir_all(&baseline)?;
        }
        workspace::prepare(&t, rec.seed, &rec.run_id, &baseline)?;
        let d = judge::diff(&baseline, &run_dir.join("workspace"))?;
        fs::remove_dir_all(&baseline)?;
        Ok((!d.trim().is_empty()).then(|| (t.objective.clone(), judge::scrub(&d, &manifest))))
    })
    .await?;
    println!("{written} judgement(s) written");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_defaults_to_both_judge_families() {
        let cli = Cli::try_parse_from(["duet-eval", "judge", "b"]).unwrap();
        let Cmd::Judge { judges, models, .. } = cli.command else {
            panic!("not judge");
        };
        assert_eq!(judges.len(), 2);
        assert_eq!(judges.join(","), lanes::judge_cli::DEFAULT_JUDGES);
        assert!(models.is_empty());
    }
}
