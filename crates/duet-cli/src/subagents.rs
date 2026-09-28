// SPDX-License-Identifier: GPL-3.0-or-later
//! The run's sub-agents (`delegate`), from the `subagents.*` settings.

use crate::{Mode, RunLimits, RunManifest, frontier_provider};
use anyhow::{Context, Result};
use duet_agent::driver::Driver;
use duet_agent::subagents::Subagents;
use duet_boundary::engine::Engine;
use duet_boundary::{GatedFrontier, OutboundGate};
use duet_config::Config;
use duet_provider::Dialect;
use std::sync::Arc;
use std::time::Duration;

/// The run's sub-agents, or `None` when `subagents.enabled` is off. With
/// `subagents.model` set to another model, sub-agents are driven by it at
/// the frontier endpoint, behind their own gate with the engine's filters
/// and check, recorded in the run's audit log.
pub(crate) fn setup(
    cfg: &Config,
    manifest: &RunManifest,
    frontier: &GatedFrontier,
    engine: Option<&Arc<Engine>>,
    limits: &RunLimits,
) -> Result<Option<Subagents>> {
    if !cfg.bool("subagents.enabled")? {
        return Ok(None);
    }
    let asked = cfg.str("subagents.model")?.trim().to_owned();
    let local_only = manifest.mode == Mode::TopClearance;
    if local_only && !asked.is_empty() {
        eprintln!(
            "warning: subagents.model is ignored in top clearance; sub-agents use the local model"
        );
    }
    let own = asked.is_empty() || asked == manifest.frontier_model || local_only;
    let (model, priced): (Option<Arc<dyn Driver>>, String) = if own {
        let priced = if local_only {
            String::new()
        } else {
            manifest.frontier_model.clone()
        };
        (None, priced)
    } else {
        let dialect = match &manifest.frontier_dialect {
            Some(name) => {
                Dialect::parse(name).with_context(|| format!("unknown frontier dialect {name}"))?
            }
            None => Dialect::Chat,
        };
        let provider = frontier_provider(cfg, &manifest.frontier_url, &asked, dialect, limits)?;
        let mut gate = OutboundGate::with_audit(frontier.audit().clone());
        if let Some(e) = engine {
            let (filter, check) = e.outbound();
            gate = gate.with_filter(filter).with_check(check);
        }
        (Some(Arc::new(gate.wrap(provider))), asked)
    };
    let price = duet_provider::price::builtin(&priced);
    if price.is_none() && !own {
        eprintln!(
            "warning: no price known for {priced}; sub-agents' dollar budget cannot be enforced"
        );
    }
    Ok(Some(Subagents {
        max_parallel: cfg.int("subagents.max_parallel")? as usize,
        max_usd: cfg.float("subagents.max_usd")?,
        max_time: Duration::from_secs(cfg.int("subagents.max_minutes")? as u64 * 60),
        model,
        price: Arc::new(move |u| price.as_ref().map_or(0.0, |p| p.cost(u))),
    }))
}
