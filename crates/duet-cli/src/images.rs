// SPDX-License-Identifier: GPL-3.0-or-later
//! Images the operator attaches (`duet run --image`, `--image-public`, and
//! `/image` in `duet chat`): the run's image settings, and a check before a
//! run starts that each attachment can be used and where it would go.

use crate::{Mode, policy};
use anyhow::{Result, bail};
use duet_agent::images::{Attachment, ImageConfig};
use duet_boundary::images::{Facts, Route, route};
use duet_boundary::model::{MAX_INPUT_BYTES, prepare_image};
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
/// or that the rules would refuse (the same rule the run applies; files a
/// run marks sensitive later are checked again when it attaches them).
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
        let shown = a.path.display();
        let Ok(path) = a.path.canonicalize() else {
            bail!("--image {shown}: no such file");
        };
        let size = std::fs::metadata(&path)?.len();
        if size > MAX_INPUT_BYTES {
            bail!(
                "--image {shown}: larger than {} MB",
                MAX_INPUT_BYTES / (1024 * 1024)
            );
        }
        if let Err(e) = prepare_image(&std::fs::read(&path)?, max_side) {
            bail!("--image {shown} cannot be read as an image: {e}");
        }
        let rel = path.strip_prefix(ws).ok();
        let decided = route(&Facts {
            boundary: mode == Mode::Hybrid,
            frontier_vision,
            local_vision: policy.local_vision,
            to_frontier: policy.images_to_frontier,
            attached: true,
            operator_public: a.public,
            in_workspace: rel.is_some(),
            sensitive: rel.is_some_and(|r| policy.is_sensitive_path(r)),
            protected: rel.is_some_and(|r| policy.ip_level(r).is_some()),
        });
        if let Route::Refuse { message, .. } = decided {
            bail!("--image {shown} cannot be attached: {message}");
        }
    }
    Ok(())
}
