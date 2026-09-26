// SPDX-License-Identifier: GPL-3.0-or-later
//! Images in runs and sessions: `read_file` on an image in the workspace,
//! and images the operator attaches to a message (`duet run --image`,
//! `/image` in `duet chat` and the TUI).
//!
//! Every image is prepared first (format checked from its bytes, decoded,
//! scaled to `images.max_side`, encoded again without metadata), then routed
//! by the presenter (`duet_boundary::images`): to the frontier, which gets
//! the image itself in the conversation; to the local model, which describes
//! it and the frontier gets the cleaned description; or nowhere, with the
//! reason. Each decision is an `image` audit event (origin, size, digest,
//! destination, rule; never content).
//!
//! An image that joins the conversation is stored in the run directory by
//! its digest (`images/<sha256>.<ext>`, private); the transcript holds only
//! the digest, and a resumed run or session loads the bytes back from there.

use crate::run::RunConfig;
use duet_boundary::audit::{AuditEvent, AuditHandle};
use duet_boundary::images::{Facts, ImageRequest, Origin, Route};
use duet_boundary::model::{
    DEFAULT_MAX_SIDE, Image, Item, MAX_INPUT_BYTES, has_image_extension, prepare_image,
};
use duet_boundary::policy::Policy;
use duet_boundary::view::{Presenter, Source};
use duet_fs::FsError;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The run's image settings.
#[derive(Debug, Clone)]
pub struct ImageConfig {
    /// `frontier.vision`: the frontier model accepts images.
    pub frontier_vision: bool,
    /// `images.max_side`: longest side after scaling, in pixels.
    pub max_side: u32,
    /// Images the operator attached to the task (`duet run --image`,
    /// `--image-public`); a session attaches these to its first message.
    pub attached: Vec<Attachment>,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            frontier_vision: false,
            max_side: DEFAULT_MAX_SIDE,
            attached: Vec::new(),
        }
    }
}

/// An image file the operator attached, and whether they marked it public.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub path: PathBuf,
    pub public: bool,
}

/// What became of one image.
#[derive(Debug)]
pub struct Placed {
    /// Text for the conversation: the tool result, or the note in the
    /// operator's message. `Err` when the image was not used (refused, or
    /// not a readable image).
    pub text: Result<String, String>,
    /// The image itself, when it goes to the frontier.
    pub images: Vec<Image>,
    /// `frontier`, `local` or `none` once routed; `None` when the file could
    /// not be used as an image.
    pub destination: Option<&'static str>,
    /// Size of the prepared image.
    pub bytes: u64,
}

impl Placed {
    fn failed(message: String) -> Self {
        Self {
            text: Err(message),
            images: Vec::new(),
            destination: None,
            bytes: 0,
        }
    }
}

/// The directory of a run's stored images.
pub fn store_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("images")
}

fn stored_path(run_dir: &Path, image: &Image) -> PathBuf {
    store_dir(run_dir).join(format!("{}.{}", image.sha256, image.extension()))
}

/// Keeps the image's bytes in the run directory under its digest.
pub fn store(run_dir: &Path, image: &Image) -> Result<(), FsError> {
    let path = stored_path(run_dir, image);
    if path.exists() {
        return Ok(());
    }
    duet_fs::private::ensure_private_dir(&store_dir(run_dir))?;
    duet_fs::private::write_private(&path, image.data())
}

/// Loads the bytes of every image in `items` from the run's store (images
/// read back from a transcript carry only their digests). An image that is
/// missing or does not match its digest is dropped, with a warning: the
/// conversation continues without it.
pub fn load(run_dir: &Path, items: &mut [Item]) {
    for item in items {
        let Item::Images { images, .. } = item else {
            continue;
        };
        let mut loaded = Vec::with_capacity(images.len());
        for img in images.drain(..) {
            if img.is_loaded() {
                loaded.push(img);
                continue;
            }
            let path = stored_path(run_dir, &img);
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| img.clone().with_data(b))
            {
                Ok(img) => loaded.push(img),
                Err(e) => eprintln!(
                    "warning: image {} is not available ({e}); continuing without it",
                    &img.sha256[..img.sha256.len().min(12)]
                ),
            }
        }
        *images = loaded;
    }
}

/// Whether a `read_file` call asks for an image (by the path's extension).
pub fn wants(args: &Map<String, Value>) -> bool {
    args.get("path")
        .and_then(Value::as_str)
        .is_some_and(has_image_extension)
}

/// `read_file` on an image in the workspace.
pub fn read(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    audit: Option<&AuditHandle>,
    args: &Map<String, Value>,
) -> Placed {
    let Some(raw) = args.get("path").and_then(Value::as_str) else {
        return Placed::failed("missing string argument `path`".into());
    };
    let rel = match duet_fs::normalize_relative(raw) {
        Ok(rel) => rel,
        Err(e) => return Placed::failed(e.to_string()),
    };
    if duet_fs::is_reserved(&rel) || !presenter.path_visible(&rel) {
        return Placed::failed(format!("{raw} is not available"));
    }
    let bytes = match duet_fs::read_file(&cfg.workspace, &rel, MAX_INPUT_BYTES) {
        Ok(b) => b,
        Err(e) => return Placed::failed(e.to_string()),
    };
    let image = match prepare_image(&bytes, cfg.images.max_side) {
        Ok(img) => img,
        Err(e) => return Placed::failed(format!("{raw} cannot be read as an image: {e}")),
    };
    let placed = place(
        cfg,
        presenter,
        audit,
        &Origin::Workspace(rel),
        image,
        false,
        false,
    );
    Placed {
        text: placed.text.map(|t| match placed.destination {
            Some("frontier") => format!("{t}; it is attached to this result."),
            _ => t,
        }),
        ..placed
    }
}

/// An image the operator attached (a file anywhere; inside the workspace it
/// is classified by its path there). The text is the note for the message.
pub fn attach(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    audit: Option<&AuditHandle>,
    attachment: &Attachment,
) -> Placed {
    let (origin, bytes) = match load_attachment(cfg, presenter, attachment) {
        Ok(found) => found,
        Err(e) => return Placed::failed(e),
    };
    let image = match prepare_image(&bytes, cfg.images.max_side) {
        Ok(img) => img,
        Err(e) => {
            return Placed::failed(format!(
                "{} cannot be read as an image: {e}",
                origin.shown()
            ));
        }
    };
    let placed = place(
        cfg,
        presenter,
        audit,
        &origin,
        image,
        true,
        attachment.public,
    );
    Placed {
        text: placed.text.map(|t| match placed.destination {
            Some("frontier") => format!("[image attached by the operator: {t}]"),
            _ => format!("[image attached by the operator]\n{t}"),
        }),
        ..placed
    }
}

/// Checks an attachment before it is sent: the file is an image that can
/// be used, and where it would go. Nothing is recorded or described.
pub fn check_attachment(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    attachment: &Attachment,
) -> Result<String, String> {
    let (origin, bytes) = load_attachment(cfg, presenter, attachment)?;
    let image = prepare_image(&bytes, cfg.images.max_side)
        .map_err(|e| format!("{} cannot be read as an image: {e}", origin.shown()))?;
    let route = presenter.route_image(&request(cfg, &origin, &image, true, attachment.public));
    said(&origin.shown(), &image, route)
}

/// Checks an attachment before a run or session exists (`duet run
/// --image`, `/image` before the first message, the TUI): a usable image,
/// and where the rules would send it, from the policy alone. The engine
/// applies the same rule again when the image is attached (then a file a
/// `sensitive_data` command wrote counts as sensitive too).
pub fn precheck(
    workspace: &Path,
    policy: &Policy,
    boundary: bool,
    frontier_vision: bool,
    max_side: u32,
    attachment: &Attachment,
) -> Result<String, String> {
    let shown = attachment.path.display();
    let path = attachment
        .path
        .canonicalize()
        .map_err(|e| format!("cannot read {shown}: {e}"))?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "{shown} is not a file of at most {} MB",
            MAX_INPUT_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let image = prepare_image(&bytes, max_side)
        .map_err(|e| format!("{shown} cannot be read as an image: {e}"))?;
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let rel = path.strip_prefix(&workspace).ok();
    let decided = duet_boundary::images::route(&Facts {
        boundary,
        frontier_vision,
        local_vision: policy.local_vision,
        to_frontier: policy.images_to_frontier,
        attached: true,
        operator_public: attachment.public,
        in_workspace: rel.is_some(),
        sensitive: rel.is_some_and(|r| policy.is_sensitive_path(r)),
        protected: rel.is_some_and(|r| policy.ip_level(r).is_some()),
    });
    said(&shown.to_string(), &image, decided)
}

/// What the operator is told about an attachment's route.
fn said(shown: &str, image: &Image, route: Route) -> Result<String, String> {
    match route {
        Route::Frontier { .. } => Ok(format!(
            "{shown} ({}): the frontier will see the image with your next message",
            image.describe()
        )),
        Route::Describe { .. } => Ok(format!(
            "{shown} ({}): the local model will describe it with your next message; the frontier \
gets the description",
            image.describe()
        )),
        Route::Refuse { message, .. } => Err(format!("{shown} cannot be attached: {message}")),
    }
}

/// The attachment's origin and bytes. A path inside the workspace is that
/// workspace file (classified by its path); any other is named by its file
/// name only.
fn load_attachment(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    attachment: &Attachment,
) -> Result<(Origin, Vec<u8>), String> {
    let shown = attachment.path.display();
    let path = attachment
        .path
        .canonicalize()
        .map_err(|e| format!("cannot read {shown}: {e}"))?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file"));
    }
    if meta.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "{shown} is larger than {} MB",
            MAX_INPUT_BYTES / (1024 * 1024)
        ));
    }
    let workspace = cfg
        .workspace
        .canonicalize()
        .unwrap_or_else(|_| cfg.workspace.clone());
    let origin = match path.strip_prefix(&workspace) {
        Ok(rel) => {
            if !presenter.path_visible(rel) {
                return Err(format!("{shown} is not available"));
            }
            Origin::Workspace(rel.to_path_buf())
        }
        Err(_) => Origin::Attached(
            path.file_name()
                .map_or_else(|| "image".into(), |n| n.to_string_lossy().into_owned()),
        ),
    };
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    Ok((origin, bytes))
}

fn request<'a>(
    cfg: &RunConfig,
    origin: &'a Origin,
    image: &'a Image,
    attached: bool,
    public: bool,
) -> ImageRequest<'a> {
    ImageRequest {
        origin,
        attached,
        operator_public: public,
        sha256: &image.sha256,
        frontier_vision: cfg.images.frontier_vision,
    }
}

/// Routes a prepared image, records the decision and carries it out.
fn place(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    audit: Option<&AuditHandle>,
    origin: &Origin,
    image: Image,
    attached: bool,
    public: bool,
) -> Placed {
    let route = presenter.route_image(&request(cfg, origin, &image, attached, public));
    if let Some(a) = audit {
        a.record(AuditEvent::Image {
            origin: origin.label(),
            bytes: image.bytes,
            sha256: image.sha256.clone(),
            destination: route.destination().into(),
            decision: route.rule().into(),
            operator_public: public && matches!(route, Route::Frontier { .. }),
        });
    }
    let shown = origin.shown();
    let bytes = image.bytes;
    let destination = Some(route.destination());
    match route {
        Route::Frontier { .. } => match store(&cfg.run_dir, &image) {
            Ok(()) => Placed {
                text: Ok(format!("{shown} (image, {})", image.describe())),
                images: vec![image],
                destination,
                bytes,
            },
            Err(e) => Placed::failed(format!("{shown}: the image could not be stored: {e}")),
        },
        Route::Describe { .. } => Placed {
            text: Ok(presenter.present(&Source::Image { origin: shown }, image.data())),
            images: Vec::new(),
            destination,
            bytes,
        },
        Route::Refuse { message, .. } => Placed {
            text: Err(format!("{shown} is an image and was not shown: {message}")),
            images: Vec::new(),
            destination,
            bytes,
        },
    }
}

/// Attaches the operator's images to their message: `text` gets the notes
/// (and the descriptions of images described locally), and the images the
/// frontier may see are returned. Any image that cannot be attached stops
/// the message with the reason: nothing is sent without it silently.
pub fn attach_all(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    audit: Option<&AuditHandle>,
    attachments: &[Attachment],
    ledger: &mut crate::ledger::Ledger,
    text: &mut String,
) -> Result<Vec<Image>, String> {
    let mut images = Vec::new();
    for a in attachments {
        let placed = attach(cfg, presenter, audit, a);
        ledger.on_image(placed.destination, placed.bytes);
        let note = placed.text?;
        text.push_str("\n\n");
        text.push_str(&note);
        images.extend(placed.images);
    }
    Ok(images)
}
