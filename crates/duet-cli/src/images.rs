// SPDX-License-Identifier: GPL-3.0-or-later
//! Images the operator attaches (`duet run --image`, `--image-public`, and
//! `/image` in `duet chat`): the run's image settings, and a check before a
//! run starts that each attachment can be used and where it would go.

use crate::{Mode, policy};
use anyhow::{Result, bail};
use duet_agent::images::{Attachment, ImageConfig};
use duet_config::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// An image attached to a run's task, as `run.json` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttachedImage {
    pub path: PathBuf,
    /// Marked public by the operator (`--image-public`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub public: bool,
}

impl AttachedImage {
    pub fn attachment(&self) -> Attachment {
        Attachment {
            path: self.path.clone(),
            public: self.public,
        }
    }
}

/// The attachments of `--image` and `--image-public`, with absolute paths.
pub(crate) fn from_args(images: &[PathBuf], public: &[PathBuf]) -> Result<Vec<AttachedImage>> {
    let cwd = std::env::current_dir()?;
    Ok(images
        .iter()
        .map(|p| (p, false))
        .chain(public.iter().map(|p| (p, true)))
        .map(|(p, public)| AttachedImage {
            path: cwd.join(p),
            public,
        })
        .collect())
}

/// Whether the model that reads the run's images accepts them: the frontier
/// (`frontier.vision`), or in local-only mode the local model (`local.vision`).
fn driver_vision(cfg: &Config, mode: Mode) -> Result<bool> {
    Ok(match mode {
        Mode::LocalOnly => cfg.bool("local.vision")?,
        Mode::Passthrough | Mode::Hybrid => cfg.bool("frontier.vision")?,
    })
}

/// The run's image settings.
pub(crate) fn config(cfg: &Config, mode: Mode, attached: &[AttachedImage]) -> Result<ImageConfig> {
    Ok(ImageConfig {
        frontier_vision: driver_vision(cfg, mode)?,
        max_side: cfg.int("images.max_side")? as u32,
        attached: attached.iter().map(AttachedImage::attachment).collect(),
    })
}

/// Refuses, before anything starts, an attachment that is not a usable image
/// or that the rules would refuse (see [`duet_agent::images::precheck`]).
pub(crate) fn precheck(
    ws: &Path,
    cfg: &Config,
    mode: Mode,
    attached: &[AttachedImage],
) -> Result<()> {
    let policy = policy(cfg)?;
    let frontier_vision = driver_vision(cfg, mode)?;
    let max_side = cfg.int("images.max_side")? as u32;
    for a in attached {
        if let Err(e) = duet_agent::images::precheck(
            ws,
            &policy,
            mode == Mode::Hybrid,
            frontier_vision,
            max_side,
            &a.attachment(),
        ) {
            bail!("{e}");
        }
    }
    Ok(())
}
