// SPDX-License-Identifier: GPL-3.0-or-later
//! The run's local explorer (`explore`), from the `explore.*` settings.

use crate::{Mode, RunLimits, RunManifest, local_enabled, local_provider};
use anyhow::Result;
use declass_agent::explore::{Caps, Explorer};
use declass_boundary::audit::AuditHandle;
use declass_boundary::local::LocalAgent;
use declass_config::Config;
use std::sync::Arc;
use std::time::Duration;

/// The run's explorer, or `None` when `explore.enabled` is off or nothing
/// local can drive it: top clearance (the local model is the one working
/// already) or `local.enabled` off. Its model is the configured local model
/// under the same endpoint rules as the engine's, with the run's deadline and
/// interrupt; in pass-through, where nothing else reaches it, the endpoint's
/// trust is recorded here.
pub(crate) fn setup(
    cfg: &Config,
    manifest: &RunManifest,
    audit: &AuditHandle,
    limits: &RunLimits,
) -> Result<Option<Arc<Explorer>>> {
    if !cfg.bool("explore.enabled")? || manifest.mode == Mode::TopClearance {
        return Ok(None);
    }
    if !local_enabled(cfg, manifest.mode)? {
        eprintln!("explore.enabled is on but local.enabled is off: the explorer is not offered");
        return Ok(None);
    }
    let (provider, trust) = local_provider(cfg, manifest.local.as_ref(), Some(limits))?;
    if manifest.mode == Mode::Passthrough {
        audit.record(trust);
    }
    let agent = LocalAgent::new(provider, audit.clone()).map_err(|e| anyhow::anyhow!(e.message))?;
    let seconds = cfg.int("explore.max_seconds")? as u64;
    let bytes = cfg.int("explore.max_read_kb")? as u64 * 1024;
    Ok(Some(Arc::new(Explorer {
        model: Arc::new(agent),
        quick: Caps {
            steps: cfg.int("explore.quick_steps")? as u32,
            time: Duration::from_secs((seconds / 3).max(10)),
            bytes: bytes / 2,
        },
        thorough: Caps {
            steps: cfg.int("explore.thorough_steps")? as u32,
            time: Duration::from_secs(seconds),
            bytes,
        },
    })))
}
