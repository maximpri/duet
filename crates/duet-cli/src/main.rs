// SPDX-License-Identifier: GPL-3.0-or-later
//! Duet command-line interface. This is the composition root: the only place
//! providers are constructed, and it hands the agent a gated frontier.

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use duet_agent::{RunConfig, Terminal};
use duet_boundary::audit::{
    AnchorCheck, AuditEvent, AuditLog, Line, Verification, anchor_path, check_anchor, verify,
};
use duet_boundary::engine::Engine;
use duet_boundary::local::LocalReader;
use duet_boundary::policy::Policy;
use duet_boundary::view::PassThrough;
use duet_boundary::{GatedFrontier, OutboundGate};
use duet_config::Config;
use duet_provider::{ChatProvider, ProviderConfig, Role};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

mod doctor;
mod setup;

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
        /// Acknowledge that `--mode passthrough` turns the privacy boundary off
        /// (required for that mode).
        #[arg(long)]
        no_privacy: bool,
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
    /// Measure the configured local model in its reading roles (accuracy, schema, leaks, prefill).
    LocalEval {
        /// Fixture sizes in log lines (800 lines is about 16K tokens).
        #[arg(long, value_delimiter = ',', default_value = "40,300,800")]
        sizes: Vec<usize>,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Write the full report (per-fixture outcomes) here as JSON.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Check configuration, endpoints, sandbox, git, disk and audit logs; print
    /// pass/warn/fail with a fix for each. Exit code: 0 pass, 1 warn, 2 fail.
    Doctor {
        /// Also contact the configured servers (model listings and context
        /// windows only; never a model call). Without it doctor uses no network.
        #[arg(long)]
        online: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// List a run's outbound requests and security events.
    Show {
        run_id: String,
        /// Print the log's JSON lines as stored.
        #[arg(long)]
        raw: bool,
    },
    /// Verify the audit log's hash chain and its anchor in the owner state directory.
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
        /// Apply a change that loosens privacy (it is recorded in the owner's
        /// config audit log). Without it such a change is shown and refused.
        #[arg(long)]
        confirm: bool,
    },
    /// Point the local role at a known backend (Ollama, LM Studio, llama.cpp,
    /// vLLM, oMLX, MLX) on loopback. Without a name, lists the presets.
    Preset {
        name: Option<String>,
        /// Also set local.model.
        #[arg(long)]
        model: Option<String>,
        /// A port other than the backend's default.
        #[arg(long)]
        port: Option<u16>,
        /// Apply a change that loosens privacy (changing an endpoint does).
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct RunManifest {
    run_id: String,
    mode: Mode,
    objective: String,
    frontier_url: String,
    frontier_model: String,
    /// A local endpoint found by bootstrap for this run (no local model configured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local: Option<LocalOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalOverride {
    base_url: String,
    model: String,
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

/// The local model provider and the audit event recording why its endpoint is
/// trusted. A remote host over plain HTTP is refused unless the owner opted in.
fn local_provider(
    cfg: &Config,
    over: Option<&LocalOverride>,
) -> Result<(ChatProvider, AuditEvent)> {
    let url = match over {
        Some(o) => o.base_url.clone(),
        None => cfg.str("local.base_url")?,
    };
    let model = match over {
        Some(o) => o.model.clone(),
        None => cfg.str("local.model")?,
    };
    let allowlist = cfg.list("local.allowlist")?;
    let allow_plaintext = cfg.bool("local.allow_plaintext")?;
    let trust = duet_provider::endpoint::check_local_endpoint(&url, &allowlist, allow_plaintext)
        .map_err(|e| anyhow::anyhow!(e.message))?;
    let mut pc = ProviderConfig::new(
        &url,
        &model,
        Role::Local {
            allowlist,
            allow_plaintext,
        },
    );
    let key_env = cfg.str("local.api_key_env")?;
    if !key_env.is_empty() {
        pc.api_key_env = Some(key_env);
    }
    if duet_provider::endpoint::is_plaintext_remote(&url) {
        eprintln!(
            "warning: the local model endpoint {url} is a remote host over plain HTTP (allowed by local.allow_plaintext); sensitive content crosses the network unencrypted"
        );
    }
    let host = duet_provider::endpoint::host_port(&url)
        .map(|(h, p)| format!("{h}:{p}"))
        .unwrap_or_default();
    let event = AuditEvent::EndpointTrust {
        role: "local".into(),
        host,
        trust: trust.as_str().into(),
    };
    Ok((ChatProvider::with_reqwest(pc)?, event))
}

/// Where the anchor of a run's audit log lives (owner state directory).
pub(crate) fn run_anchor(ws: &Path, run_id: &str) -> PathBuf {
    anchor_path(&duet_config::owner_state_dir(), ws, run_id)
}

fn checked_run_id(run_id: &str) -> Result<&str> {
    ensure!(
        !run_id.is_empty() && !run_id.contains('/') && !run_id.contains(".."),
        "invalid run id"
    );
    Ok(run_id)
}

const PASSTHROUGH_BANNER: &str = "\
=====================================================================
 PRIVACY BOUNDARY OFF (passthrough mode)
 File contents, command output, secrets and personal data are sent to
 the frontier provider unfiltered. Use it only on repositories with
 nothing sensitive, as a reference lane.
=====================================================================";

fn gated(
    ws: &Path,
    run_id: &str,
    provider: ChatProvider,
    engine: Option<&Arc<Engine>>,
) -> Result<GatedFrontier> {
    let audit = AuditLog::open_anchored(
        &ws.join(".duet/audit").join(format!("{run_id}.jsonl")),
        &run_anchor(ws, run_id),
        run_id,
    )?;
    let mut gate = OutboundGate::new(audit);
    if let Some(e) = engine {
        let (filter, check) = e.outbound();
        gate = gate.with_filter(filter).with_check(check);
    }
    Ok(gate.wrap(provider))
}

fn policy(cfg: &Config) -> Result<Policy> {
    Ok(Policy {
        sensitive_globs: cfg.list("sensitivity.globs")?,
        protected_paths: cfg.list("sensitivity.protected_paths")?,
        command_output_sensitive: cfg.bool("sensitivity.command_output_sensitive")?,
        raw_ok_commands: cfg.list("sensitivity.raw_ok_commands")?,
        secret_sinks: cfg.list("sensitivity.secret_sinks")?,
        detect_secrets: cfg.bool("sensitivity.detect_secrets")?,
        detect_pii: cfg.bool("sensitivity.detect_pii")?,
        detect_entropy: cfg.bool("sensitivity.detect_entropy")?,
        bulky_tokens: cfg.int("sensitivity.bulky_tokens")? as usize,
        bulky_file_tokens: cfg.int("sensitivity.bulky_file_tokens")? as usize,
        local_brief: cfg.bool("sensitivity.local_brief")?,
        interface_only: cfg.list("ip.interface_only")?,
        sealed: cfg.list("ip.sealed")?,
    })
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

    if manifest.mode == Mode::Passthrough {
        eprintln!("{PASSTHROUGH_BANNER}");
    }
    let mut trust = None;
    let engine = match manifest.mode {
        Mode::Hybrid => {
            let (local, event) = local_provider(&cfg, manifest.local.as_ref())?;
            trust = Some(event);
            Some(Engine::open(
                &run_dir,
                policy(&cfg)?,
                Some(LocalReader::new(local)),
            )?)
        }
        _ => None,
    };
    let (driver, price_model) = match manifest.mode {
        Mode::Passthrough | Mode::Hybrid => (
            frontier_provider(&cfg, &manifest.frontier_url, &manifest.frontier_model)?,
            manifest.frontier_model.clone(),
        ),
        Mode::LocalOnly => {
            let (local, event) = local_provider(&cfg, manifest.local.as_ref())?;
            trust = Some(event);
            (local, String::new())
        }
    };
    if let Some(e) = &engine {
        let files = git.list_files(&ws).unwrap_or_default();
        let primed = e.prime(&ws, &files, &manifest.objective);
        eprintln!("security engine: indexed {primed} sensitive file(s)");
    }
    let frontier = gated(&ws, &manifest.run_id, driver, engine.as_ref())?;
    frontier.audit().record(AuditEvent::RunStart {
        mode: format!("{:?}", manifest.mode).to_lowercase(),
        boundary: manifest.mode != Mode::Passthrough,
    });
    if let Some(event) = trust {
        frontier.audit().record(event);
    }
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
        reasoning_effort: Some(cfg.str("frontier.reasoning_effort")?).filter(|e| e != "default"),
        price: Box::new(move |u| price.as_ref().map_or(0.0, |p| p.cost(u))),
    };
    let passthrough = PassThrough { max_bytes: 60_000 };
    let presenter: &dyn duet_boundary::view::Presenter = match &engine {
        Some(e) => e.as_ref(),
        None => &passthrough,
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let flag = interrupted.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\ninterrupt received; finishing the current step");
            flag.store(true, Ordering::SeqCst);
        }
    });
    let (terminal, mut stats) =
        duet_agent::run(&run_cfg, &frontier, presenter, &git, resume, &interrupted).await;
    // Local model work of this invocation (a resumed run reports only its own).
    stats.ledger.local = engine.as_ref().and_then(|e| e.take_local_stats());
    // The final event also anchors the log's final head.
    frontier.audit().record(AuditEvent::RunEnd {
        terminal: match &terminal {
            Terminal::Completed { .. } => "completed",
            Terminal::Failed { .. } => "failed",
            Terminal::BudgetStopped { .. } => "budget_stopped",
        }
        .into(),
    });
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

/// The owner's log of settings changes.
pub(crate) fn config_audit_path() -> PathBuf {
    duet_config::owner_state_dir().join("config-audit.jsonl")
}

/// One line of `duet audit show`.
fn show_line(line: &str) -> String {
    let time = |ms: u128| {
        time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
            .map(|t| {
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    t.year(),
                    u8::from(t.month()),
                    t.day(),
                    t.hour(),
                    t.minute(),
                    t.second()
                )
            })
            .unwrap_or_default()
    };
    match duet_boundary::audit::parse_line(line) {
        Some(Line::Request(r)) => format!(
            "#{:<4} {}  request         {} {} ({} bytes){}",
            r.seq,
            time(r.unix_ms),
            r.endpoint,
            r.model,
            serde_json::to_vec(&r.request).map_or(0, |b| b.len()),
            if r.interventions.is_empty() {
                String::new()
            } else {
                format!("; {} intervention(s)", r.interventions.len())
            }
        ),
        Some(Line::Event(e)) => {
            let mut fields = serde_json::to_value(&e.event).unwrap_or_default();
            if let Some(m) = fields.as_object_mut() {
                m.remove("kind");
            }
            format!(
                "#{:<4} {}  {:<15} {fields}",
                e.seq,
                time(e.unix_ms),
                e.event.kind()
            )
        }
        None => format!("unparseable: {}", line.chars().take(80).collect::<String>()),
    }
}

/// `duet audit verify`: the chain, then the anchor. Returns the exit code.
fn verify_run(path: &Path, anchor: &Path) -> Result<i32> {
    match verify(path).with_context(|| format!("no audit log at {}", path.display()))? {
        Verification::Intact { records } => println!("chain intact: {records} records"),
        Verification::Broken { at_seq, reason } => {
            println!("BROKEN at record {at_seq}: {reason}");
            return Ok(1);
        }
    }
    Ok(match check_anchor(path, anchor)? {
        AnchorCheck::Matches => {
            println!("anchor matches ({})", anchor.display());
            0
        }
        AnchorCheck::Extends { unanchored } => {
            println!(
                "anchor matches; {unanchored} later record(s) were never anchored (an interrupted append)"
            );
            0
        }
        AnchorCheck::Mismatch(reason) => {
            println!("REWRITTEN OR TRUNCATED since it was anchored: {reason}");
            1
        }
        AnchorCheck::Missing => {
            println!(
                "no anchor at {}: a rewritten log cannot be detected (run predates anchoring, or another state directory)",
                anchor.display()
            );
            2
        }
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
            no_privacy,
        } => {
            match (mode, no_privacy) {
                (Mode::Passthrough, false) => bail!(
                    "--mode passthrough turns the privacy boundary off: everything the model reads, \
including secrets and personal data, is sent to the frontier provider unfiltered. \
Add --no-privacy to confirm, or use --mode hybrid."
                ),
                (Mode::Hybrid | Mode::LocalOnly, true) => {
                    bail!("--no-privacy only applies to --mode passthrough")
                }
                _ => {}
            }
            let objective = match (objective, objective_file) {
                (Some(o), None) => o,
                (None, Some(f)) => std::fs::read_to_string(&f)
                    .with_context(|| format!("reading {}", f.display()))?,
                _ => bail!("give the task as text or with --objective-file (not both)"),
            };
            let cfg = load_config(&ws)?;
            let local = match mode {
                Mode::Hybrid | Mode::LocalOnly => match setup::bootstrap(&cfg).await {
                    Ok(found) => found.map(|b| LocalOverride {
                        base_url: b.base_url,
                        model: b.model,
                    }),
                    Err(code) => std::process::exit(code),
                },
                Mode::Passthrough => None,
            };
            let manifest = RunManifest {
                run_id: new_run_id(),
                mode,
                objective,
                frontier_url: frontier_url.unwrap_or(cfg.str("frontier.base_url")?),
                frontier_model: frontier_model.unwrap_or(cfg.str("frontier.model")?),
                local,
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
            AuditCmd::Show { run_id, raw } => {
                let path = ws
                    .join(".duet/audit")
                    .join(format!("{}.jsonl", checked_run_id(&run_id)?));
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("no audit log for {run_id}"))?;
                if raw {
                    print!("{text}");
                } else {
                    for line in text.lines().filter(|l| !l.is_empty()) {
                        println!("{}", show_line(line));
                    }
                }
            }
            AuditCmd::Verify { run_id } => {
                let path = ws
                    .join(".duet/audit")
                    .join(format!("{}.jsonl", checked_run_id(&run_id)?));
                std::process::exit(verify_run(&path, &run_anchor(&ws, &run_id))?);
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
                    confirm,
                } => {
                    let v: toml::Value = toml::from_str::<toml::Table>(&format!("v = {value}"))
                        .with_context(|| format!("{value} is not a TOML value"))?["v"]
                        .clone();
                    if !project {
                        std::process::exit(setup::apply_owner(
                            &mut cfg,
                            &[(key.as_str(), v)],
                            confirm,
                        )?);
                    }
                    // A project may only tighten, so nothing here needs confirming.
                    let old = cfg.value(&key)?.clone();
                    cfg.set_project(&key, v.clone())?;
                    AuditLog::open(&config_audit_path())?.event(AuditEvent::ConfigChange {
                        key: key.clone(),
                        file: "project".into(),
                        old: old.to_string(),
                        new: v.to_string(),
                        weakens: None,
                        confirmed: confirm,
                    })?;
                    println!("{key} = {}", cfg.value(&key)?);
                }
                ConfigCmd::Preset {
                    name,
                    model,
                    port,
                    confirm,
                } => std::process::exit(setup::preset(
                    &mut cfg,
                    name.as_deref(),
                    model.as_deref(),
                    port,
                    confirm,
                )?),
            }
        }
        Cmd::Purge { run_id, all } => {
            let cfg = load_config(&ws)?;
            purge(&ws, run_id.as_deref(), all, cfg.int("data.retention_days")?)?;
        }
        Cmd::LocalEval { sizes, seed, out } => {
            let cfg = load_config(&ws)?;
            let reader = LocalReader::new(local_provider(&cfg, None)?.0);
            let fixtures = duet_boundary::local_eval::fixtures(seed, &sizes);
            let report = duet_boundary::local_eval::run(&reader, &fixtures, |o| {
                eprintln!(
                    "{:<22} {} {} {:>6.1}s  answer: {} call(s) {} in / {} cached / {} out  digest: {:.0}s {} call(s) {} cached / {} out{}",
                    o.name,
                    if o.correct { "correct" } else { "WRONG  " },
                    if o.digest_mentions {
                        "digest-ok"
                    } else {
                        "digest-miss"
                    },
                    o.answer_seconds,
                    o.answer_calls.calls,
                    o.answer_calls.input_tokens,
                    o.answer_calls.cached_tokens,
                    o.answer_calls.output_tokens,
                    o.digest_calls.seconds,
                    o.digest_calls.calls,
                    o.digest_calls.cached_tokens,
                    o.digest_calls.output_tokens,
                    if o.leaked.is_empty() {
                        String::new()
                    } else {
                        format!("  LEAKED {}", o.leaked.len())
                    }
                );
            })
            .await;
            println!(
                "accuracy {:.2}  evidence {:.2}  digest recall {:.2}  schema {:.2}  leaks {}  prefill {:.0} tok/s  digest cache reuse {:.0}%  => {}",
                report.accuracy,
                report.evidence_recall,
                report.digest_recall,
                report.schema_valid,
                report.leaks,
                report.prefill_tok_s,
                100.0 * report.digest_cache_reuse,
                if report.pass { "PASS" } else { "FAIL" }
            );
            if let Some(p) = out {
                std::fs::write(p, serde_json::to_vec_pretty(&report)?)?;
            }
        }
        Cmd::Doctor { online, json } => {
            let checks = doctor::run(&ws, online).await;
            if json {
                println!("{}", doctor::render_json(&checks, online));
            } else {
                print!("{}", doctor::render_text(&checks, online));
            }
            std::process::exit(doctor::exit_code(&checks));
        }
    }
    Ok(())
}
