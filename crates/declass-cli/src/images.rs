// SPDX-License-Identifier: GPL-3.0-or-later
//! Images the operator attaches (`declass run --image`, `--image-public`, and
//! `/image` in a `declass` session): the run's image settings, and a check before a
//! run starts that each attachment can be used and where it would go.

use crate::{Mode, policy};
use anyhow::{Context, Result, bail, ensure};
use declass_agent::images::{Attachment, ImageConfig};
use declass_config::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const MAX_PENDING: usize = 16;

/// Recognize a supported image filename without treating a sentence mentioning
/// one as an attachment. Explicit /attach paths use `named_image` directly.
pub(crate) fn is_path(raw: &str) -> bool {
    named_image(raw)
        && (crate::attachments::is_path(raw)
            || !raw.trim().contains(char::is_whitespace)
            || raw.trim().starts_with(['\'', '"'])
            || crate::attachments::path(raw).is_ok_and(|p| p.is_file()))
}

pub(crate) fn named_image(raw: &str) -> bool {
    crate::attachments::path(raw).is_ok_and(|p| {
        p.extension().and_then(|s| s.to_str()).is_some_and(|ext| {
            declass_provider::image::EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
    })
}

pub(crate) fn selected(ws: &Path, raw: &str, public: bool) -> Result<AttachedImage> {
    Ok(AttachedImage {
        path: crate::attachments::selected(ws, raw)?,
        public,
    })
}

pub(crate) fn queue(
    pending: &mut Vec<AttachedImage>,
    ws: &Path,
    cfg: &Config,
    mode: Mode,
    raw: &str,
    public: bool,
) -> Result<String> {
    ensure!(
        pending.len() < MAX_PENDING,
        "at most {MAX_PENDING} images can wait for one message"
    );
    let image = selected(ws, raw, public)?;
    precheck(ws, cfg, mode, std::slice::from_ref(&image))?;
    let name = image
        .path
        .file_name()
        .unwrap_or(image.path.as_os_str())
        .to_string_lossy();
    let notice = format!(
        "Image {name} queued for your next message{}",
        if public {
            " · marked public by you"
        } else {
            ""
        }
    );
    pending.push(image);
    Ok(notice)
}

/// Clipboard captures remain private, outside the workspace, until chat exits.
/// Keeping the immutable file alive also covers captures queued during a turn.
/// Delivered frontier images have their own transcript-linked run-store copy.
pub(crate) struct ClipboardImages {
    workspace: PathBuf,
    root: Option<PathBuf>,
    count: usize,
    bytes: usize,
    max_side: u32,
}

impl ClipboardImages {
    pub(crate) fn new(workspace: PathBuf, max_side: u32) -> Self {
        Self {
            workspace,
            root: None,
            count: 0,
            bytes: 0,
            max_side,
        }
    }

    pub(crate) fn store(&mut self, bytes: Vec<u8>) -> Result<PathBuf> {
        ensure!(
            self.count < 64,
            "clipboard image limit reached (64 captures); start a new session"
        );
        ensure!(
            bytes.len() as u64 <= declass_provider::image::MAX_INPUT_BYTES,
            "clipboard image exceeds 20 MiB"
        );
        let image =
            declass_provider::image::prepare(&bytes, self.max_side).map_err(anyhow::Error::msg)?;
        ensure!(
            self.bytes + image.data().len() <= 256 * 1024 * 1024,
            "clipboard images exceed 256 MiB; start a new session"
        );
        if self.root.is_none() {
            use std::os::unix::fs::DirBuilderExt;
            let ws = self.workspace.canonicalize()?;
            let mut base = std::env::temp_dir().canonicalize()?;
            if base.starts_with(&ws) {
                base = Path::new("/tmp").canonicalize()?;
            }
            ensure!(
                !base.starts_with(&ws),
                "no private clipboard storage outside this workspace is available"
            );
            let root = base.join(format!("declass-clipboard-{}", uuid::Uuid::new_v4()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .context("cannot create private clipboard storage")?;
            self.root = Some(root);
        }
        let path = self.root.as_ref().unwrap().join(format!(
            "clipboard-{}.{}",
            self.count + 1,
            image.extension()
        ));
        declass_fs::private::write_private(&path, image.data())?;
        self.count += 1;
        self.bytes += image.data().len();
        Ok(path)
    }
}

impl Drop for ClipboardImages {
    fn drop(&mut self) {
        if let Some(root) = &self.root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

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
/// (`frontier.vision`), or in top clearance the local model (`local.vision`).
fn driver_vision(cfg: &Config, mode: Mode) -> Result<bool> {
    Ok(match mode {
        Mode::TopClearance => cfg.bool("local.vision")?,
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
/// or that the rules would refuse (see [`declass_agent::images::precheck`]).
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
        if let Err(e) = declass_agent::images::precheck(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn image_paths_decode_quotes_escapes_without_evaluating_text() {
        let ws = tempfile::tempdir().unwrap();
        assert!(is_path("screen.PNG"));
        assert!(is_path(r"/tmp/my\ screenshot.png"));
        assert!(is_path("'my screenshot.webp'"));
        assert!(!is_path("Please describe my screenshot.png"));
        assert!(!is_path("./notes.md"));
        let image = selected(ws.path(), r"./my\ screenshot.png", false).unwrap();
        assert_eq!(image.path, ws.path().join("./my screenshot.png"));
        assert!(!image.public);
        assert!(selected(ws.path(), "'unfinished", false).is_err());
    }

    #[test]
    fn clipboard_images_are_private_external_validated_and_cleaned() {
        let ws = tempfile::tempdir().unwrap();
        let path;
        {
            let mut store = ClipboardImages::new(ws.path().to_path_buf(), 64);
            assert!(store.store(b"not an image".to_vec()).is_err());
            assert!(store.root.is_none());
            path = store
                .store(declass_provider::image::solid_png(4, 3, [1, 2, 3]))
                .unwrap();
            assert!(!path.starts_with(ws.path()));
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert!(declass_provider::image::prepare(&std::fs::read(&path).unwrap(), 64).is_ok());
            store.count = 64;
            assert!(
                store
                    .store(declass_provider::image::solid_png(1, 1, [0, 0, 0]))
                    .is_err()
            );
        }
        assert!(!path.exists());
    }
}
