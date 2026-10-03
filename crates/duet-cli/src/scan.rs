// SPDX-License-Identifier: GPL-3.0-or-later
//! Foreground/background repository scans with private, commit-labelled records.
use crate::{Embedding, Mode, RunLimits, checked_run_id, load_config, new_run_id};
use anyhow::{Result, ensure};
use duet_boundary::{
    audit::{AuditEvent, AuditHandle},
    engine::Engine,
    view::Presenter,
};
use serde_json::json;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Start a detached scan; inspect it with --status SCAN_ID.
    #[arg(long, conflicts_with = "status")]
    background: bool,
    /// Inspect an earlier scan's status and report location.
    #[arg(long)]
    status: Option<String>,
    /// Use rules and configured offline scanners without model opinions.
    #[arg(long)]
    rules_only: bool,
    /// Print the private report as JSON.
    #[arg(long)]
    json: bool,
    /// Exit 2 when rule-confirmed high-severity findings exist.
    #[arg(long)]
    fail_on_high: bool,
    #[arg(long, hide = true, conflicts_with_all = ["background", "status"])]
    worker: Option<String>,
}

fn write(path: &Path, value: &serde_json::Value) -> Result<()> {
    duet_fs::private::write_private(path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

pub(crate) async fn run(ws: &Path, args: Args, emb: &Embedding) -> Result<i32> {
    if let Some(id) = &args.status {
        ensure!(id.starts_with("scan-"), "expected a scan id");
        let dir = ws.join(".duet/runs").join(checked_run_id(id)?);
        let bytes = duet_fs::read_file(&dir, Path::new("scan-status.json"), 1024 * 1024)?;
        let status: serde_json::Value = serde_json::from_slice(&bytes)?;
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(0);
    }
    let id = args
        .worker
        .clone()
        .unwrap_or_else(|| format!("scan-{}", new_run_id()));
    ensure!(id.starts_with("scan-"), "expected a scan id");
    let dir = ws.join(".duet/runs").join(checked_run_id(&id)?);
    duet_fs::private::ensure_private_dir(&dir)?;
    let status_path = dir.join("scan-status.json");
    if args.background {
        write(&status_path, &json!({"id":id,"state":"queued"}))?;
        let log = dir.join("scan.log");
        duet_fs::private::write_private(&log, b"")?;
        let file = std::fs::OpenOptions::new().append(true).open(&log)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--workspace")
            .arg(ws)
            .args(["scan", "--worker", &id]);
        if args.rules_only {
            command.arg("--rules-only");
        }
        if args.fail_on_high {
            command.arg("--fail-on-high");
        }
        command
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(file);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(child) => {
                if args.json {
                    println!("{}", json!({"id":id,"pid":child.id(),"state":"started"}));
                } else {
                    println!("started {id}; inspect with: duet scan --status {id}");
                }
                return Ok(0);
            }
            Err(e) => {
                write(
                    &status_path,
                    &json!({"id":id,"state":"failed","reason":"could not start scan worker"}),
                )?;
                return Err(e.into());
            }
        }
    }
    write(
        &status_path,
        &json!({"id":id,"state":"running","pid":std::process::id()}),
    )?;
    let result = scan(ws, &dir, &id, &args, emb).await;
    match result {
        Ok((reviewed, metadata)) => {
            write(&status_path, &metadata)?;
            if args.json {
                let report =
                    duet_fs::read_file(&dir, Path::new("security-review.json"), 2 * 1024 * 1024)?;
                println!("{}", String::from_utf8(report)?);
            } else {
                println!(
                    "{}\nReport: {}",
                    reviewed.text,
                    dir.join("security-review.json").display()
                );
            }
            Ok(if args.fail_on_high && reviewed.blocked {
                2
            } else {
                0
            })
        }
        Err(e) => {
            // Raw endpoint/tool errors may contain private text. Keep the scan
            // state and CLI error fixed, while preserving the diagnostic locally.
            duet_fs::private::write_private(&dir.join("diagnostic.txt"), e.to_string().as_bytes())?;
            write(
                &status_path,
                &json!({"id":id,"state":"failed","reason":"scan unavailable, interrupted, or workspace changed; inspect configuration and retry"}),
            )?;
            anyhow::bail!(
                "security scan did not complete; status: {}",
                status_path.display()
            )
        }
    }
}

async fn scan(
    ws: &Path,
    dir: &Path,
    id: &str,
    args: &Args,
    emb: &Embedding,
) -> Result<(duet_agent::review::Reviewed, serde_json::Value)> {
    let start = Instant::now();
    let cfg = load_config(ws, emb)?;
    let interrupted = Arc::new(AtomicBool::new(false));
    let signal = interrupted.clone();
    let watcher = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.store(true, Ordering::SeqCst);
        }
    });
    let limits = RunLimits {
        deadline: tokio::time::Instant::now()
            + Duration::from_secs(cfg.int("limits.wall_clock_minutes")? as u64 * 60),
        interrupted: interrupted.clone(),
        local_meter: crate::pricing::meter(&cfg)?,
    };
    let audit = AuditHandle::new(crate::open_audit(ws, id, emb.hooks())?);
    audit.record(AuditEvent::RunStart {
        mode: "security-scan".into(),
        boundary: true,
    });
    let local = if !args.rules_only && cfg.bool("local.enabled")? {
        let (provider, trust) = crate::local_provider(&cfg, None, Some(&limits))?;
        audit.record(trust);
        Some(duet_boundary::local::LocalReader::new(provider))
    } else {
        None
    };
    let engine = Engine::open_for_workspace(ws, dir, crate::policy(&cfg)?, local)?;
    let git = duet_git::Git::locate()?;
    let commit = || {
        git.run(ws, &["rev-parse", "--verify", "HEAD"], &[], None)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .map(|s| s.trim().to_owned())
    };
    let head = commit();
    let dirty = git
        .run(ws, &["diff", "--quiet", "HEAD", "--"], &[], None)
        .is_err()
        || git
            .run(
                ws,
                &["ls-files", "--others", "--exclude-standard", "-z"],
                &[],
                None,
            )
            .map_or(true, |s| {
                s.split(|b| *b == 0).filter(|b| !b.is_empty()).any(|p| {
                    std::str::from_utf8(p).map_or(true, |p| !duet_fs::is_reserved(Path::new(p)))
                })
            });
    engine.prime(
        ws,
        &git.list_files(ws).unwrap_or_default(),
        "Review repository security",
    );
    let lsp = if args.rules_only {
        None
    } else {
        crate::lsp::servers(&cfg, ws, dir, duet_sandbox::detect()?)?
    };
    let mut settings = crate::security_review::settings(&cfg, ws, lsp.clone())?;
    settings.enabled = true;
    settings.rules_only = args.rules_only;
    settings.block_high = args.fail_on_high;
    let mode = if cfg.str("clearance.required")? == "top" {
        Mode::TopClearance
    } else {
        Mode::Hybrid
    };
    let (catalog, price_source) = crate::pricing::catalog(
        &cfg,
        &cfg.str("frontier.base_url")?,
        args.rules_only || mode == Mode::TopClearance || !cfg.bool("review.frontier")?,
    )
    .await;
    let price = crate::pricing::quote(
        &catalog,
        &cfg,
        &cfg.str("frontier.model")?,
        &cfg.str("frontier.base_url")?,
    );
    crate::pricing::attach(dir, &limits.local_meter, price.clone(), price_source)?;
    if !args.rules_only {
        settings.second = crate::security_review::second(
            &cfg,
            mode,
            &cfg.str("frontier.base_url")?,
            &cfg.str("frontier.model")?,
            crate::frontier_dialect(&cfg)?,
            Some(&engine),
            &audit,
            &limits,
            price,
        )?;
    }
    settings.remaining_usd = cfg.float("review.frontier_usd")?;
    let _second = settings.second.clone();
    let stopped =
        || interrupted.load(Ordering::SeqCst) || tokio::time::Instant::now() >= limits.deadline;
    let result =
        duet_agent::review::repository(ws, dir, settings, engine.as_ref(), &audit, stopped).await;
    if let Some(lsp) = lsp {
        lsp.shutdown().await;
    }
    watcher.abort();
    let frontier_usage = engine.take_review_usage();
    let local_usage = engine.take_local_stats();
    // Keep metering even when a race or cancellation prevents a completed report.
    write(
        &dir.join("scan-usage.json"),
        &json!({"local":local_usage,"frontier":frontier_usage}),
    )?;
    let result = result.and_then(|reviewed| {
        if commit() == head {
            Ok(reviewed)
        } else {
            Err("repository revision changed during review".into())
        }
    });
    // Models cannot turn an incomplete/error review into a success.
    match &result {
        Ok(_) => audit.record(AuditEvent::RunEnd {
            terminal: "completed".into(),
        }),
        Err(_) => {
            audit.record(AuditEvent::SecurityReviewAborted {
                reason: "repository scan did not complete or its captured revision changed".into(),
            });
            audit.record(AuditEvent::RunEnd {
                terminal: "failed".into(),
            });
        }
    }
    let reviewed = result.map_err(anyhow::Error::msg)?;
    let metadata = json!({"id":id,"state":"completed","commit":head,"dirty":dirty,
        "report":dir.join("security-review.json"),"seconds":start.elapsed().as_secs_f64(),
        "local":local_usage,"frontier":frontier_usage,"rules_only":args.rules_only});
    Ok((reviewed, metadata))
}
