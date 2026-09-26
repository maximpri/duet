// SPDX-License-Identifier: GPL-3.0-or-later
//! Duet command-line interface. This is the composition root: the only place
//! providers are constructed, and it hands the agent a gated frontier.

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use duet_agent::disclosure::Disclosure;
use duet_agent::{RunConfig, Terminal};
use duet_boundary::audit::{
    AuditEvent, AuditHandle, AuditLog, RunAnchors, describe_line, run_anchors, verify_report,
};
use duet_boundary::engine::Engine;
use duet_boundary::local::LocalReader;
use duet_boundary::policy::Policy;
use duet_boundary::view::PassThrough;
use duet_boundary::{GatedFrontier, OutboundGate};
use duet_config::{Config, Target};
use duet_provider::{ChatProvider, Dialect, ProviderConfig, Role};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

mod approve;
mod chat;
mod doctor;
mod egress;
mod images;
mod lsp;
mod mcp;
mod setup;
mod subagents;
mod term;
mod web;

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
        /// Attach an image (PNG, JPEG, GIF, WebP) to the task; repeatable. In
        /// hybrid mode the local model describes it and the frontier gets the
        /// description (needs local.vision).
        #[arg(long = "image", value_name = "PATH")]
        image: Vec<PathBuf>,
        /// Attach an image the frontier may see itself (it is not scanned:
        /// only for images holding nothing sensitive; never from a sensitive
        /// path; needs frontier.vision); recorded as your decision. Repeatable.
        #[arg(long = "image-public", value_name = "PATH")]
        image_public: Vec<PathBuf>,
        /// No progress on standard error (tool steps, the frontier's text as
        /// it streams, a status line on a terminal). Standard output (the
        /// summary) is the same either way.
        #[arg(long)]
        quiet: bool,
    },
    /// Continue an interrupted run.
    Resume {
        run_id: String,
        /// No progress on standard error (as for `duet run`).
        #[arg(long)]
        quiet: bool,
    },
    /// Code in a conversation: each message you type is a turn duet works
    /// on, with the same boundary, sandbox, budgets and audit as a run; duet
    /// replies, asks when it needs a decision, and keeps the context for your
    /// next message. /help inside lists the commands.
    Chat {
        /// The first message (otherwise typed at the prompt).
        message: Option<String>,
        #[arg(long, value_enum, default_value = "hybrid")]
        mode: Mode,
        /// Override the frontier endpoint for this session.
        #[arg(long)]
        frontier_url: Option<String>,
        /// Override the frontier model for this session.
        #[arg(long)]
        frontier_model: Option<String>,
        /// Acknowledge that `--mode passthrough` turns the privacy boundary off.
        #[arg(long)]
        no_privacy: bool,
        /// Continue a session: the one named, or the most recent open one.
        #[arg(long, num_args = 0..=1, value_name = "SESSION_ID")]
        resume: Option<Option<String>>,
    },
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
    /// Terminal UI: settings screens generated from the registry (edits use the
    /// same checks, confirmation and audit as `duet config set`), IP levels,
    /// the audit viewer, and a run view with changed files and diffs that can
    /// also start runs.
    Tui,
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
    /// What the boundary withheld from the frontier in a run, by class (counts
    /// and kinds only, never values).
    Disclosure {
        run_id: String,
        #[arg(long)]
        json: bool,
    },
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
    /// vLLM, oMLX, MLX) on loopback, the frontier at a known provider (zai,
    /// anthropic, openai: endpoint, model, key variable and dialect), or web
    /// search at a private SearXNG in Docker (searxng: prints the commands,
    /// runs nothing). Without a name, lists the presets.
    Preset {
        name: Option<String>,
        /// Also set local.model (for a frontier preset: frontier.model instead
        /// of the preset's default).
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
    /// The frontier's wire dialect when the run started (runs from before
    /// dialects existed have none and use Chat Completions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    frontier_dialect: Option<String>,
    /// A local endpoint found by bootstrap for this run (no local model configured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local: Option<LocalOverride>,
    /// A session (`duet chat`) rather than a one-shot run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    session: bool,
    /// Images attached to the task (`--image`, `--image-public`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    images: Vec<images::AttachedImage>,
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
    let cfg = Config::load(
        &duet_config::owner_config_path(),
        Some(&ws.join(".duet/config.toml")),
    )?;
    for note in &cfg.notes {
        eprintln!("warning: {note}");
    }
    Ok(cfg)
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

/// What stops a provider's in-place retries: the run's wall-clock deadline and
/// its interrupt flag. Without a run (`duet local-eval`) retries are bounded.
#[derive(Clone)]
struct RunLimits {
    deadline: tokio::time::Instant,
    interrupted: Arc<AtomicBool>,
}

fn limit_retries(pc: &mut ProviderConfig, limits: Option<&RunLimits>) {
    match limits {
        Some(l) => {
            pc.deadline = Some(l.deadline);
            pc.cancel = Some(l.interrupted.clone());
        }
        None => pc.max_attempts = Some(6),
    }
}

/// The configured frontier dialect.
fn frontier_dialect(cfg: &Config) -> Result<Dialect> {
    let name = cfg.str("frontier.dialect")?;
    Dialect::parse(&name).with_context(|| format!("unknown frontier.dialect {name}"))
}

fn frontier_provider(
    cfg: &Config,
    url: &str,
    model: &str,
    dialect: Dialect,
    limits: &RunLimits,
) -> Result<ChatProvider> {
    let mut pc = ProviderConfig::new(url, model, Role::Frontier);
    pc.dialect = dialect;
    limit_retries(&mut pc, Some(limits));
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
    limits: Option<&RunLimits>,
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
    limit_retries(&mut pc, limits);
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

/// Whether a run in `mode` uses a local model (`local.enabled`). With it off,
/// hybrid runs without one (handles only; `ask_local` and `edit_protected`
/// refused), and anything that would need one to protect data or to run at
/// all is refused rather than silently skipped.
pub(crate) fn local_enabled(cfg: &Config, mode: Mode) -> Result<bool> {
    if cfg.bool("local.enabled")? {
        return Ok(true);
    }
    match mode {
        Mode::LocalOnly => {
            bail!("--mode local-only needs a local model, and local.enabled is false")
        }
        Mode::Hybrid => ensure!(
            !cfg.bool("sensitivity.local_pii_pass")?,
            "sensitivity.local_pii_pass needs a local model, and local.enabled is false: turn one of them off"
        ),
        Mode::Passthrough => {}
    }
    Ok(false)
}

/// The anchors of a run's audit log (owner state directory).
pub(crate) fn run_anchor(ws: &Path, run_id: &str) -> RunAnchors {
    run_anchors(&duet_config::owner_state_dir(), ws, run_id)
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

fn audit_log_path(ws: &Path, run_id: &str) -> PathBuf {
    ws.join(".duet/audit").join(format!("{run_id}.jsonl"))
}

fn open_audit(ws: &Path, run_id: &str) -> Result<AuditLog> {
    Ok(AuditLog::open_anchored(
        &audit_log_path(ws, run_id),
        &run_anchor(ws, run_id),
    )?)
}

fn gated(
    ws: &Path,
    run_id: &str,
    provider: ChatProvider,
    engine: Option<&Arc<Engine>>,
) -> Result<GatedFrontier> {
    let audit = open_audit(ws, run_id)?;
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
        custom_patterns: cfg.list("sensitivity.custom_patterns")?,
        bulky_tokens: cfg.int("sensitivity.bulky_tokens")? as usize,
        bulky_file_tokens: cfg.int("sensitivity.bulky_file_tokens")? as usize,
        local_brief: cfg.bool("sensitivity.local_brief")?,
        local_pii_pass: cfg.bool("sensitivity.local_pii_pass")?,
        interface_only: cfg.list("ip.interface_only")?,
        sealed: cfg.list("ip.sealed")?,
        local_vision: cfg.bool("local.vision")?,
        images_to_frontier: duet_boundary::images::ToFrontier::parse(
            &cfg.str("images.to_frontier")?,
        )
        .unwrap_or_default(),
    })
}

async fn execute(ws: PathBuf, manifest: RunManifest, resume: bool, quiet: bool) -> Result<i32> {
    let _lock = duet_fs::lock::WorkspaceLock::acquire(&ws)?;
    let cfg = load_config(&ws)?;
    // Before anything starts: with approval on and no terminal, the run is refused.
    let mut oversight = approve::oversight(&cfg)?;
    // Progress on standard error: live on a terminal, else compact lines.
    let live = std::io::stderr().is_terminal() && term::capable();
    let watch = (!quiet).then(|| term::watch::Watch::start(live));
    if let (Some(w), Some(a)) = (&watch, oversight.approver.take()) {
        // A question on the terminal pauses the status line.
        oversight.approver = Some(if live {
            term::watch::holding(a, w.clone())
        } else {
            a
        });
    }
    let run_dir = ws.join(".duet/runs").join(&manifest.run_id);
    duet_fs::private::ensure_private_dir(&run_dir)?;
    duet_fs::private::ensure_private_dir(&ws.join(".duet/tmp"))?;
    if !resume {
        duet_fs::private::write_private(
            &run_dir.join("run.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
    }
    // From here on the run exists, and it ends in a terminal state with a
    // summary whatever happens: an error or a panic ends it as `Failed`.
    let wall_minutes = cfg.int("limits.wall_clock_minutes")? as u64;
    let limits = RunLimits {
        deadline: tokio::time::Instant::now() + Duration::from_secs(wall_minutes * 60),
        interrupted: Arc::new(AtomicBool::new(false)),
    };
    let flag = limits.interrupted.clone();
    let notices = watch.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            match notices.filter(|_| live) {
                Some(w) => w.note("interrupt received; stopping the current step"),
                None => eprintln!("\ninterrupt received; stopping the current step"),
            }
            flag.store(true, Ordering::SeqCst);
        }
    });
    // The run's steps from its transcript, as they are written.
    let transcript = run_dir.join("transcript.jsonl");
    let from = if resume {
        std::fs::metadata(&transcript).map_or(0, |m| m.len())
    } else {
        0
    };
    let done = Arc::new(AtomicBool::new(false));
    let follower = watch.as_ref().map(|w| {
        let w = w.clone();
        tokio::spawn(chat::follow(
            transcript,
            from,
            done.clone(),
            Box::new(move |e| w.entry(e)),
        ))
    });
    let mut audit = None;
    let work = start(
        &ws,
        &manifest,
        &cfg,
        oversight,
        &run_dir,
        resume,
        &limits,
        &mut audit,
        watch.as_ref(),
    );
    let work = match &watch {
        Some(w) => futures_util::future::Either::Left(duet_boundary::live::observe(w.tap(), work)),
        None => futures_util::future::Either::Right(work),
    };
    let started = std::panic::AssertUnwindSafe(work).catch_unwind().await;
    done.store(true, Ordering::SeqCst);
    if let Some(f) = follower {
        let _ = f.await;
    }
    if let Some(w) = &watch {
        w.stop();
    }
    let (terminal, stats) = match started {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => (
            Terminal::Failed {
                reason: format!("{e:#}"),
            },
            duet_agent::RunStats::default(),
        ),
        Err(panic) => (
            Terminal::internal_error(panic.as_ref()),
            duet_agent::RunStats::default(),
        ),
    };
    // A run that failed before its audit log was open still records its end.
    let audit = match audit {
        Some(a) => Some(a),
        None => open_audit(&ws, &manifest.run_id)
            .map(AuditHandle::new)
            .map_err(|e| eprintln!("warning: audit log not opened: {e:#}"))
            .ok(),
    };
    let summary = duet_agent::conclude(
        &run_dir,
        &manifest.run_id,
        audit.as_ref(),
        &audit_log_path(&ws, &manifest.run_id),
        &terminal,
        &stats,
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(match terminal {
        Terminal::Completed { .. } => 0,
        Terminal::Failed { .. } => 1,
        Terminal::BudgetStopped { .. } => 3,
    })
}

/// Sets the run up and drives it to a terminal state. `audit` receives the
/// run's audit log as soon as it is open.
#[allow(clippy::too_many_arguments)]
async fn start(
    ws: &Path,
    manifest: &RunManifest,
    cfg: &Config,
    oversight: duet_agent::Oversight,
    run_dir: &Path,
    resume: bool,
    limits: &RunLimits,
    audit: &mut Option<AuditHandle>,
    watch: Option<&Arc<term::watch::Watch>>,
) -> Result<(Terminal, duet_agent::RunStats)> {
    let p = prepare(ws, manifest, cfg, oversight, run_dir, limits, audit).await?;
    if let Some(w) = watch {
        if let Some(e) = &p.engine {
            let e = e.clone();
            w.restore(Arc::new(move |t: &str| {
                duet_boundary::view::Presenter::detokenize(e.as_ref(), t)
            }));
        }
        // Set up: the status line starts.
        w.release();
    }
    let passthrough = PassThrough { max_bytes: 60_000 };
    let presenter: &dyn duet_boundary::view::Presenter = match &p.engine {
        Some(e) => e.as_ref(),
        None => &passthrough,
    };
    let (terminal, mut stats) = duet_agent::run(
        &p.run_cfg,
        &p.frontier,
        presenter,
        &p.git,
        resume,
        &limits.interrupted,
    )
    .await;
    mcp::stop(&p.run_cfg).await;
    // Local model work of this invocation (a resumed run reports only its own).
    stats.ledger.local = p.engine.as_ref().and_then(|e| e.take_local_stats());
    Ok((terminal, stats))
}

/// What a run or a session works with.
struct Prepared {
    git: duet_git::Git,
    /// The security engine (hybrid mode).
    engine: Option<Arc<Engine>>,
    frontier: GatedFrontier,
    run_cfg: RunConfig,
}

/// Opens the engine, the providers and the audit log, records the start
/// events, starts the MCP servers and builds the run configuration. `audit`
/// receives the audit log as soon as it is open.
async fn prepare(
    ws: &Path,
    manifest: &RunManifest,
    cfg: &Config,
    oversight: duet_agent::Oversight,
    run_dir: &Path,
    limits: &RunLimits,
    audit: &mut Option<AuditHandle>,
) -> Result<Prepared> {
    let git = duet_git::Git::locate()?;
    let _ = git.exclude_state_dir(ws);
    let sandbox = duet_sandbox::detect()?;

    if manifest.mode == Mode::Passthrough {
        eprintln!("{PASSTHROUGH_BANNER}");
    }
    let mut trust = None;
    let engine = match manifest.mode {
        Mode::Hybrid if !local_enabled(cfg, Mode::Hybrid)? => {
            eprintln!(
                "no local model (local.enabled = false): sensitive content reaches the frontier only as handles; ask_local and edit_protected are refused"
            );
            Some(Engine::open(run_dir, policy(cfg)?, None)?)
        }
        Mode::Hybrid => {
            let (local, event) = local_provider(cfg, manifest.local.as_ref(), Some(limits))?;
            trust = Some(event);
            Some(Engine::open(
                run_dir,
                policy(cfg)?,
                Some(LocalReader::new(local)),
            )?)
        }
        _ => None,
    };
    let (driver, price_model) = match manifest.mode {
        Mode::Passthrough | Mode::Hybrid => (
            frontier_provider(
                cfg,
                &manifest.frontier_url,
                &manifest.frontier_model,
                match &manifest.frontier_dialect {
                    Some(name) => Dialect::parse(name)
                        .with_context(|| format!("unknown frontier dialect {name}"))?,
                    None => Dialect::Chat,
                },
                limits,
            )?,
            manifest.frontier_model.clone(),
        ),
        Mode::LocalOnly => {
            local_enabled(cfg, Mode::LocalOnly)?;
            let (local, event) = local_provider(cfg, manifest.local.as_ref(), Some(limits))?;
            trust = Some(event);
            (local, String::new())
        }
    };
    if let Some(e) = &engine {
        let files = git.list_files(ws).unwrap_or_default();
        let primed = e.prime(ws, &files, &manifest.objective);
        eprintln!("security engine: indexed {primed} sensitive file(s)");
    }
    let frontier = gated(ws, &manifest.run_id, driver, engine.as_ref())?;
    // A full disk is waited out from the first event (see `duet_agent::host`).
    frontier
        .audit()
        .set_wait(Some(Arc::new(duet_agent::host::HostPolicy::until(
            limits.deadline.into_std(),
            Some(limits.interrupted.clone()),
        ))));
    *audit = Some(frontier.audit().clone());
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
    let frontier_key_env = cfg.str("frontier.api_key_env")?;
    // Every field from the configuration, not `..RunConfig::new`: a new field
    // is a compile error here until the CLI sets it.
    let run_cfg = RunConfig {
        workspace: ws.to_path_buf(),
        run_dir: run_dir.to_path_buf(),
        objective: manifest.objective.clone(),
        mode: format!("{:?}", manifest.mode).to_lowercase(),
        checks: cfg.list("checks.commands")?,
        sandbox,
        network: egress::network(cfg)?,
        command_timeout: Duration::from_secs(cfg.int("limits.command_timeout_seconds")? as u64),
        wall_clock: Duration::from_secs(wall_minutes * 60),
        frontier_usd: cfg.float("limits.frontier_usd")?,
        max_finish_attempts: cfg.int("limits.max_finish_attempts")? as u32,
        context_window: cfg.int("context.window_tokens")? as u64,
        mask_at: cfg.float("context.mask_at")?,
        max_output_tokens: 32_768,
        reasoning_effort: Some(cfg.str("frontier.reasoning_effort")?).filter(|e| e != "default"),
        price: Box::new(move |u| price.as_ref().map_or(0.0, |p| p.cost(u))),
        oversight,
        web: web::access(
            cfg,
            // Local-only runs have no frontier.
            (manifest.mode != Mode::LocalOnly).then(|| web::Frontier {
                base_url: &manifest.frontier_url,
                key_env: &frontier_key_env,
            }),
        )?,
        git_author: approve::git_author(cfg)?,
        mcp: mcp::start(
            cfg,
            ws,
            run_dir,
            sandbox,
            engine
                .as_deref()
                .map(|e| e as &dyn duet_boundary::view::Presenter),
            frontier.audit(),
        )
        .await?,
        lsp: lsp::servers(cfg, ws, run_dir, sandbox)?,
        subagents: subagents::setup(cfg, manifest, &frontier, engine.as_ref(), limits)?,
        images: images::config(cfg, manifest.mode, &manifest.images)?,
        owner_instructions: Some(duet_config::owner_instructions_path()),
    };
    Ok(Prepared {
        git,
        engine,
        frontier,
        run_cfg,
    })
}

/// The disclosure report of a run, from its audit log and its cost ledger
/// (given, or read from the run's summary). `None` without an audit log.
fn read_disclosure(
    ws: &Path,
    run_id: &str,
    ledger: Option<&duet_agent::ledger::Ledger>,
) -> Option<Disclosure> {
    let log = ws.join(".duet/audit").join(format!("{run_id}.jsonl"));
    let lines = duet_boundary::audit::read(&log).ok()?;
    let stored = || -> Option<duet_agent::ledger::Ledger> {
        let path = ws.join(".duet/runs").join(run_id).join("summary.json");
        let summary: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        serde_json::from_value(summary.get("stats")?.get("ledger")?.clone()).ok()
    };
    let stored = if ledger.is_none() { stored() } else { None };
    Some(Disclosure::build(&lines, ledger.or(stored.as_ref())))
}

/// The owner's log of settings changes.
pub(crate) fn config_audit_path() -> PathBuf {
    duet_config::config_audit_log(&duet_config::owner_state_dir())
}

/// `duet audit verify`: the chain, then the anchor. Returns the exit code.
fn verify_run(path: &Path, anchors: &RunAnchors) -> Result<i32> {
    let (code, lines) = verify_report(path, anchors)
        .with_context(|| format!("no audit log at {}", path.display()))?;
    for l in lines {
        println!("{l}");
    }
    Ok(code)
}

fn purge(ws: &Path, run_id: Option<&str>, all: bool, retention_days: i64) -> Result<()> {
    use duet_agent::purge::{Scope, candidates, purge_run};
    if let Some(id) = run_id {
        purge_run(ws, id).with_context(|| format!("run {id}"))?;
        println!("purged {id}");
        return Ok(());
    }
    let scope = if all {
        Scope::All
    } else {
        Scope::OlderThan {
            days: retention_days,
        }
    };
    for id in candidates(ws, scope) {
        purge_run(ws, &id).with_context(|| format!("run {id}"))?;
        println!("purged {id}");
    }
    Ok(())
}

/// The TUI's cache-reuse probe against the configured local model: the same
/// endpoint trust rules as a run, two identical short requests, no retries
/// beyond two attempts. Prints nothing (the TUI owns the terminal).
async fn tui_cache_probe(ws: &Path) -> Result<duet_tui::CacheReport, String> {
    let cfg = load_config(ws).map_err(|e| format!("{e:#}"))?;
    if cfg.origin("local.base_url") == Some(duet_config::Origin::Default) {
        return Err(
            "no local endpoint is configured: l detects servers on loopback and u uses one".into(),
        );
    }
    let text = |k: &str| cfg.str(k).map_err(|e| e.to_string());
    let (url, model) = (text("local.base_url")?, text("local.model")?);
    let allowlist = cfg.list("local.allowlist").map_err(|e| e.to_string())?;
    let allow_plaintext = cfg
        .bool("local.allow_plaintext")
        .map_err(|e| e.to_string())?;
    let mut pc = ProviderConfig::new(
        &url,
        &model,
        Role::Local {
            allowlist,
            allow_plaintext,
        },
    );
    pc.max_attempts = Some(2);
    pc.first_byte_timeout = Duration::from_secs(120);
    let key_env = text("local.api_key_env")?;
    if !key_env.is_empty() {
        pc.api_key_env = Some(key_env);
    }
    let provider = ChatProvider::with_reqwest(pc).map_err(|e| e.message)?;
    let r = duet_provider::probe::cache_reuse(&provider)
        .await
        .map_err(|e| e.message)?;
    Ok(duet_tui::CacheReport {
        base_url: url,
        model,
        prompt_tokens: r.prompt_tokens,
        first_cached: r.first_cached,
        second_cached: r.second_cached,
        first_seconds: r.first_seconds,
        second_seconds: r.second_seconds,
        unreported: r.unreported,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // Inside a bubblewrap sandbox, as the egress bridge (before anything else).
    if let Some(code) = egress::bridge_helper() {
        std::process::exit(code);
    }
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
            image,
            image_public,
            quiet,
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
            // Refused before bootstrap probes anything.
            approve::require_terminal(&cfg)?;
            let attached = images::from_args(&image, &image_public)?;
            images::precheck(&ws, &cfg, mode, &attached)?;
            // With the local model off nothing is probed for one.
            let local = match mode {
                Mode::Hybrid | Mode::LocalOnly if local_enabled(&cfg, mode)? => {
                    match setup::bootstrap(&cfg).await {
                        Ok(found) => found.map(|b| LocalOverride {
                            base_url: b.base_url,
                            model: b.model,
                        }),
                        Err(code) => std::process::exit(code),
                    }
                }
                _ => None,
            };
            let manifest = RunManifest {
                run_id: new_run_id(),
                mode,
                objective,
                frontier_url: frontier_url.unwrap_or(cfg.str("frontier.base_url")?),
                frontier_model: frontier_model.unwrap_or(cfg.str("frontier.model")?),
                frontier_dialect: Some(frontier_dialect(&cfg)?.as_str().to_owned()),
                local,
                session: false,
                images: attached,
            };
            eprintln!("run {} ({:?})", manifest.run_id, manifest.mode);
            std::process::exit(execute(ws, manifest, false, quiet).await?);
        }
        Cmd::Resume { run_id, quiet } => {
            let run_dir = ws.join(".duet/runs").join(checked_run_id(&run_id)?);
            let manifest: RunManifest = serde_json::from_slice(
                &std::fs::read(run_dir.join("run.json"))
                    .with_context(|| format!("no run {run_id}"))?,
            )?;
            ensure!(
                manifest.run_id == run_id,
                "run {run_id}: run.json names another run"
            );
            ensure!(
                !manifest.session,
                "{run_id} is a session; continue it with `duet chat --resume {run_id}`"
            );
            if let Err(why) = duet_agent::resumable(&run_dir) {
                bail!("run {run_id} cannot be resumed: {why}");
            }
            std::process::exit(execute(ws, manifest, true, quiet).await?);
        }
        Cmd::Chat {
            message,
            mode,
            frontier_url,
            frontier_model,
            no_privacy,
            resume,
        } => {
            let args = chat::ChatArgs {
                message,
                mode,
                frontier_url,
                frontier_model,
                no_privacy,
                resume,
            };
            std::process::exit(chat::chat(ws, args).await?);
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
                        println!("{}", describe_line(line));
                    }
                }
            }
            AuditCmd::Disclosure { run_id, json } => {
                let report = read_disclosure(&ws, checked_run_id(&run_id)?, None)
                    .with_context(|| format!("no audit log for {run_id}"))?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", report.render(&run_id));
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
                        if duet_config::is_template(s.key) {
                            // One line per configured instance (`*` names it).
                            let names = cfg.instances(s.key);
                            if names.is_empty() {
                                println!("{}  (none configured; {})", s.key, s.help);
                            }
                            for n in names {
                                let key = s.key.replace('*', &n);
                                let origin = cfg
                                    .origin(&key)
                                    .map_or("?".to_owned(), |o| format!("{o:?}").to_lowercase());
                                println!("{key} = {}  ({origin}; {})", cfg.value(&key)?, s.help);
                            }
                            continue;
                        }
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
                    let v = duet_config::parse_value(&value)
                        .with_context(|| format!("{value} is not a TOML value"))?;
                    let (v, note) = duet_config::migrate(&key, v);
                    if let Some(note) = note {
                        eprintln!("note: {note}");
                    }
                    if !project {
                        std::process::exit(setup::apply_owner(
                            &mut cfg,
                            &[(key.as_str(), v)],
                            confirm,
                        )?);
                    }
                    // A project may only tighten, so nothing here needs confirming.
                    let change = cfg.apply(Target::Project, &key, v, confirm)?;
                    duet_boundary::audit::record_config_change(
                        &config_audit_path(),
                        &change,
                        Target::Project,
                        confirm,
                    )?;
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
            let reader = LocalReader::new(local_provider(&cfg, None, None)?.0);
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
        Cmd::Tui => {
            let handle = tokio::runtime::Handle::current();
            let at = ws.clone();
            let on = handle.clone();
            let doctor: duet_tui::Doctor = Box::new(move |online| {
                on.block_on(doctor::run(&at, online))
                    .into_iter()
                    .map(|c| duet_tui::DoctorLine {
                        status: format!("{:?}", c.status).to_lowercase(),
                        name: c.name.into(),
                        detail: c.detail,
                        fix: c.fix,
                    })
                    .collect()
            });
            let on = handle.clone();
            let detect = std::sync::Arc::new(move || {
                on.block_on(duet_provider::backends::discover_loopback(
                    &setup::bootstrap_ports(),
                    Duration::from_millis(1500),
                ))
                .into_iter()
                .map(|s| duet_tui::LocalServer {
                    backend: s.backend.unwrap_or("OpenAI-compatible server").into(),
                    base_url: s.base_url,
                    models: s.models,
                })
                .collect()
            });
            let (on, at) = (handle.clone(), ws.clone());
            let cache_probe = std::sync::Arc::new(move || on.block_on(tui_cache_probe(&at)));
            let services = duet_tui::Services {
                doctor,
                detect,
                cache_probe,
                duet: std::env::current_exe().ok(),
            };
            tokio::task::block_in_place(|| {
                duet_tui::run(duet_tui::Paths::for_workspace(ws), services)
            })?;
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
