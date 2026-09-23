// SPDX-License-Identifier: GPL-3.0-or-later
//! Duet command-line interface. This is the composition root: the only place
//! providers are constructed, and it hands the agent a gated frontier.

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use duet_agent::{RunConfig, Terminal};
use duet_boundary::audit::{AuditLog, Verification, verify};
use duet_boundary::view::PassThrough;
use duet_boundary::{GatedFrontier, OutboundGate};
use duet_config::Config;
use duet_provider::{ChatProvider, ProviderConfig, Role};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "duet",
    version,
    about = "Frontier-level coding with sensitive information kept on your machine"
)]
struct Cli {
    /// Repository to work in (default: current directory).
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    /// Frontier only; the security engine is off (reference lane).
    Passthrough,
    /// Frontier decides; sensitive content is processed only by the local model.
    Hybrid,
    /// The local model does everything (reference lane).
    LocalOnly,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a task.
    Run {
        /// The task, as text.
        objective: Option<String>,
        /// Read the task from a file.
        #[arg(long)]
        objective_file: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "hybrid")]
        mode: Mode,
        /// Override the frontier endpoint for this run.
        #[arg(long)]
        frontier_url: Option<String>,
        /// Override the frontier model for this run.
        #[arg(long)]
        frontier_model: Option<String>,
    },
    /// Continue an interrupted run.
    Resume { run_id: String },
    /// Inspect what was sent to the frontier.
    Audit {
        #[command(subcommand)]
        action: AuditCmd,
    },
    /// Read or change settings.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Delete raw run data (handles, transcripts, vault) older than the retention period, or one run.
    Purge {
        run_id: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Print every outbound request of a run.
    Show { run_id: String },
    /// Verify the audit log's hash chain.
    Verify { run_id: String },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show every setting with its value and origin.
    List,
    Get {
        key: String,
    },
    /// Set a value (TOML literal, e.g. 5.0, true, "text", ["a", "b"]).
    Set {
        key: String,
        value: String,
        /// Write to the project's .duet/config.toml instead of the owner config.
        #[arg(long)]
        project: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct RunManifest {
    run_id: String,
    mode: Mode,
    objective: String,
    frontier_url: String,
    frontier_model: String,
}

fn workspace(cli: &Cli) -> Result<PathBuf> {
    let ws = cli.workspace.clone().unwrap_or(std::env::current_dir()?);
    ws.canonicalize()
        .with_context(|| format!("workspace {}", ws.display()))
}

fn load_config(ws: &Path) -> Result<Config> {
    Ok(Config::load(
        &duet_config::owner_config_path(),
        Some(&ws.join(".duet/config.toml")),
    )?)
}

fn new_run_id() -> String {
    let now = time::OffsetDateTime::now_utc();
    let short = uuid::Uuid::new_v4().simple().to_string();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}-{}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        &short[..6]
    )
}

fn frontier_provider(cfg: &Config, url: &str, model: &str) -> Result<ChatProvider> {
    let mut pc = ProviderConfig::new(url, model, Role::Frontier);
    let key_env = cfg.str("frontier.api_key_env")?;
    if !key_env.is_empty() {
        pc.api_key_env = Some(key_env);
    }
    Ok(ChatProvider::with_reqwest(pc)?)
}

fn local_provider(cfg: &Config) -> Result<ChatProvider> {
    let url = cfg.str("local.base_url")?;
    let mut pc = ProviderConfig::new(
        &url,
        &cfg.str("local.model")?,
        Role::Local {
            allowlist: cfg.list("local.allowlist")?,
        },
    );
    let key_env = cfg.str("local.api_key_env")?;
    if !key_env.is_empty() {
        pc.api_key_env = Some(key_env);
    }
    if duet_provider::endpoint::is_plaintext_remote(&url) {
        eprintln!(
            "warning: the local model endpoint {url} is a remote host over plain HTTP; sensitive content would cross the network unencrypted"
        );
    }
    Ok(ChatProvider::with_reqwest(pc)?)
}

fn gated(ws: &Path, run_id: &str, provider: ChatProvider) -> Result<GatedFrontier> {
    let audit = AuditLog::open(&ws.join(".duet/audit").join(format!("{run_id}.jsonl")))?;
    Ok(OutboundGate::new(audit).wrap(provider))
}

async fn execute(ws: PathBuf, manifest: RunManifest, resume: bool) -> Result<i32> {
    let _lock = duet_fs::lock::WorkspaceLock::acquire(&ws)?;
    let cfg = load_config(&ws)?;
    let run_dir = ws.join(".duet/runs").join(&manifest.run_id);
    duet_fs::private::ensure_private_dir(&run_dir)?;
    duet_fs::private::ensure_private_dir(&ws.join(".duet/tmp"))?;
    if !resume {
        duet_fs::private::write_private(
            &run_dir.join("run.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
    }
    let git = duet_git::Git::locate()?;
    let _ = git.exclude_state_dir(&ws);
    let sandbox = duet_sandbox::detect()?;

    let (driver, price_model) = match manifest.mode {
        Mode::Passthrough => (
            frontier_provider(&cfg, &manifest.frontier_url, &manifest.frontier_model)?,
            manifest.frontier_model.clone(),
        ),
        Mode::LocalOnly => (local_provider(&cfg)?, String::new()),
        Mode::Hybrid => bail!(
            "hybrid mode arrives with the security engine (milestone M3); use --mode passthrough"
        ),
    };
    let frontier = gated(&ws, &manifest.run_id, driver)?;
    let price = duet_provider::price::builtin(&price_model);
    if price.is_none() && !price_model.is_empty() {
        eprintln!(
            "warning: no price known for {price_model}; the dollar budget cannot be enforced"
        );
    }
    let wall_minutes = cfg.int("limits.wall_clock_minutes")? as u64;
    let run_cfg = RunConfig {
        workspace: ws.clone(),
        run_dir: run_dir.clone(),
        objective: manifest.objective.clone(),
        mode: format!("{:?}", manifest.mode).to_lowercase(),
        checks: cfg.list("checks.commands")?,
        sandbox,
        network: cfg.bool("sandbox.network")?,
        command_timeout: Duration::from_secs(cfg.int("limits.command_timeout_seconds")? as u64),
        wall_clock: Duration::from_secs(wall_minutes * 60),
        frontier_usd: cfg.float("limits.frontier_usd")?,
        max_finish_attempts: cfg.int("limits.max_finish_attempts")? as u32,
        context_window: cfg.int("context.window_tokens")? as u64,
        mask_at: cfg.float("context.mask_at")?,
        max_output_tokens: 32_768,
        price: Box::new(move |u| price.as_ref().map_or(0.0, |p| p.cost(u))),
    };
    let presenter = PassThrough { max_bytes: 60_000 };
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = interrupted.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\ninterrupt received; finishing the current step");
            flag.store(true, Ordering::SeqCst);
        }
    });
    let (terminal, stats) =
        duet_agent::run(&run_cfg, &frontier, &presenter, &git, resume, &interrupted).await;
    let summary = serde_json::json!({
        "run_id": manifest.run_id,
        "terminal": terminal,
        "stats": stats,
    });
    duet_fs::private::write_private(
        &run_dir.join("summary.json"),
        &serde_json::to_vec_pretty(&summary)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(match terminal {
        Terminal::Completed { .. } => 0,
        Terminal::Failed { .. } => 1,
        Terminal::BudgetStopped { .. } => 3,
    })
}

fn purge(ws: &Path, run_id: Option<&str>, all: bool, retention_days: i64) -> Result<()> {
    let runs = ws.join(".duet/runs");
    if let Some(id) = run_id {
        ensure!(!id.contains('/') && !id.contains(".."), "invalid run id");
        std::fs::remove_dir_all(runs.join(id)).with_context(|| format!("run {id}"))?;
        println!("purged {id}");
        return Ok(());
    }
    let cutoff =
        std::time::SystemTime::now() - Duration::from_secs(retention_days.max(0) as u64 * 86_400);
    for entry in std::fs::read_dir(&runs).into_iter().flatten().flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if all || old {
            std::fs::remove_dir_all(entry.path())?;
            println!("purged {}", entry.file_name().to_string_lossy());
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let ws = workspace(&cli)?;
    match cli.command {
        Cmd::Run {
            objective,
            objective_file,
            mode,
            frontier_url,
            frontier_model,
        } => {
            let objective = match (objective, objective_file) {
                (Some(o), None) => o,
                (None, Some(f)) => std::fs::read_to_string(&f)
                    .with_context(|| format!("reading {}", f.display()))?,
                _ => bail!("give the task as text or with --objective-file (not both)"),
            };
            let cfg = load_config(&ws)?;
            let manifest = RunManifest {
                run_id: new_run_id(),
                mode,
                objective,
                frontier_url: frontier_url.unwrap_or(cfg.str("frontier.base_url")?),
                frontier_model: frontier_model.unwrap_or(cfg.str("frontier.model")?),
            };
            eprintln!("run {} ({:?})", manifest.run_id, manifest.mode);
            std::process::exit(execute(ws, manifest, false).await?);
        }
        Cmd::Resume { run_id } => {
            let path = ws.join(".duet/runs").join(&run_id).join("run.json");
            let manifest: RunManifest = serde_json::from_slice(
                &std::fs::read(&path).with_context(|| format!("no run {run_id}"))?,
            )?;
            std::process::exit(execute(ws, manifest, true).await?);
        }
        Cmd::Audit { action } => match action {
            AuditCmd::Show { run_id } => {
                let path = ws.join(".duet/audit").join(format!("{run_id}.jsonl"));
                print!(
                    "{}",
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("no audit log for {run_id}"))?
                );
            }
            AuditCmd::Verify { run_id } => {
                let path = ws.join(".duet/audit").join(format!("{run_id}.jsonl"));
                match verify(&path)? {
                    Verification::Intact { records } => {
                        println!("intact: {records} outbound requests")
                    }
                    Verification::Broken { at_seq, reason } => {
                        println!("BROKEN at record {at_seq}: {reason}");
                        std::process::exit(1);
                    }
                }
            }
        },
        Cmd::Config { action } => {
            let mut cfg = load_config(&ws)?;
            match action {
                ConfigCmd::List => {
                    for s in duet_config::REGISTRY {
                        let origin = cfg
                            .origin(s.key)
                            .map_or("?".to_owned(), |o| format!("{o:?}").to_lowercase());
                        println!("{} = {}  ({origin}; {})", s.key, cfg.value(s.key)?, s.help);
                    }
                }
                ConfigCmd::Get { key } => println!("{}", cfg.value(&key)?),
                ConfigCmd::Set {
                    key,
                    value,
                    project,
                } => {
                    let v: toml::Value = toml::from_str::<toml::Table>(&format!("v = {value}"))
                        .with_context(|| format!("{value} is not a TOML value"))?["v"]
                        .clone();
                    if project {
                        cfg.set_project(&key, v)?
                    } else {
                        cfg.set_owner(&key, v)?
                    }
                    println!("{key} = {}", cfg.value(&key)?);
                }
            }
        }
        Cmd::Purge { run_id, all } => {
            let cfg = load_config(&ws)?;
            purge(&ws, run_id.as_deref(), all, cfg.int("data.retention_days")?)?;
        }
    }
    Ok(())
}
