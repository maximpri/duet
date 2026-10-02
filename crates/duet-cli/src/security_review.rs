// SPDX-License-Identifier: GPL-3.0-or-later
//! Review services are composed here; every frontier opinion uses its own gate.
use crate::{Config, Mode, RunLimits, limit_retries};
use anyhow::{Result, ensure};
use duet_agent::review::{Settings, external::Scanner};
use duet_boundary::{OutboundGate, audit::AuditHandle, engine::Engine, review::SecondReviewer};
use duet_provider::{ChatProvider, Dialect, ProviderConfig, Role};
use std::path::Path;
use std::sync::Arc;

pub(crate) fn settings(
    cfg: &Config,
    workspace: &Path,
    lsp: Option<Arc<duet_lsp::Lsp>>,
) -> Result<Settings> {
    let mut scanners = Vec::new();
    for name in cfg.instances("review.scanners.*.command") {
        let key = |field: &str| format!("review.scanners.{name}.{field}");
        let scanner = Scanner {
            command: cfg.str(&key("command"))?.into(),
            args: cfg.list(&key("args"))?,
            format: cfg.str(&key("format"))?,
            timeout_seconds: cfg.int(&key("timeout_seconds"))? as u64,
        };
        scanner.check(workspace).map_err(anyhow::Error::msg)?;
        scanners.push(scanner);
    }
    ensure!(
        scanners.len() <= 4,
        "at most four security scanners may be configured"
    );
    Ok(Settings {
        enabled: cfg.bool("review.enabled")?,
        block_high: cfg.bool("review.block_high")?,
        max_candidates: cfg.int("review.max_candidates")? as usize,
        local_open: cfg.bool("review.local_open")?,
        max_second_candidates: cfg.int("review.max_frontier_candidates")? as usize,
        lsp: if cfg.bool("review.references")? {
            lsp
        } else {
            None
        },
        scanners,
        ..Settings::default()
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn second(
    cfg: &Config,
    mode: Mode,
    url: &str,
    model: &str,
    dialect: Dialect,
    engine: Option<&Arc<Engine>>,
    audit: &AuditHandle,
    limits: &RunLimits,
    price: Option<duet_provider::catalog::Quote>,
) -> Result<Option<Arc<SecondReviewer>>> {
    if !cfg.bool("review.frontier")? || mode != Mode::Hybrid {
        return Ok(None);
    }
    let engine = engine
        .ok_or_else(|| anyhow::anyhow!("security second opinions require the privacy boundary"))?;
    let mut pc = ProviderConfig::new(url, model, Role::Frontier);
    pc.dialect = dialect;
    limit_retries(&mut pc, Some(limits));
    pc.max_attempts = Some(1);
    pc.api_key_env = Some(cfg.str("frontier.api_key_env")?).filter(|s| !s.is_empty());
    let provider = ChatProvider::with_reqwest(pc)?;
    let (filter, check) = engine.outbound();
    let gated = OutboundGate::with_audit(audit.clone())
        .with_filter(filter)
        .with_check(check)
        .wrap(provider);
    let price = price.ok_or_else(|| anyhow::anyhow!("OpenRouter price unavailable for security second opinion; set pricing.frontier_model to its exact slug"))?;
    let reviewer = SecondReviewer::priced(gated, cfg.float("review.frontier_usd")?, price)
        .map_err(anyhow::Error::msg)?;
    engine
        .set_review_second(&reviewer)
        .map_err(anyhow::Error::msg)?;
    Ok(Some(reviewer))
}
