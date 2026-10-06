// SPDX-License-Identifier: GPL-3.0-or-later
//! Images in runs and sessions: `read_file` on an image in the workspace,
//! and images the operator attaches to a message (`declass run --image`,
//! `/image` in a `declass` session).
//!
//! Every image is prepared first (format checked from its bytes, decoded,
//! scaled to `images.max_side`, encoded again without metadata), then routed
//! by the presenter (`declass_boundary::images`): to the frontier, which gets
//! the image itself in the conversation; to the local model, which describes
//! it and the frontier gets the cleaned description; or nowhere, with the
//! reason. Each decision is an `image` audit event (origin, size, digest,
//! destination, rule; never content).
//!
//! An image that joins the conversation is stored in the run directory by
//! its digest (`images/<sha256>.<ext>`, private); the transcript holds only
//! the digest, and a resumed run or session loads the bytes back from there.

use crate::run::RunConfig;
use declass_boundary::audit::{AuditEvent, AuditHandle};
use declass_boundary::images::{Facts, ImageRequest, Origin, Route};
use declass_boundary::model::{
    DEFAULT_MAX_SIDE, Image, Item, MAX_INPUT_BYTES, has_image_extension, prepare_image,
};
use declass_boundary::policy::Policy;
use declass_boundary::view::{Presenter, Source};
use declass_fs::FsError;
use serde_json::{Map, Value};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The run's image settings.
#[derive(Debug, Clone)]
pub struct ImageConfig {
    /// `frontier.vision`: the frontier model accepts images.
    pub frontier_vision: bool,
    /// `images.max_side`: longest side after scaling, in pixels.
    pub max_side: u32,
    /// Images the operator attached to the task (`declass run --image`,
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
    declass_fs::private::ensure_private_dir(&store_dir(run_dir))?;
    declass_fs::private::write_private(&path, image.data())
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
            if img.sha256.len() != 64 || !img.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                eprintln!("warning: stored image has an invalid digest; continuing without it");
                continue;
            }
            let path = stored_path(run_dir, &img);
            match attachment_path(&path)
                .and_then(|path| read_image_file(&path))
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
    let rel = match declass_fs::normalize_relative(raw) {
        Ok(rel) => rel,
        Err(e) => return Placed::failed(e.to_string()),
    };
    if declass_fs::is_reserved(&rel) || !presenter.path_visible(&rel) {
        return Placed::failed(format!("{raw} is not available"));
    }
    let bytes = match declass_fs::read_file(&cfg.workspace, &rel, MAX_INPUT_BYTES) {
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

/// Checks an attachment before a run or session exists (`declass run
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
    let path = attachment_path(&attachment.path)?;
    let origin = attachment_origin(workspace, &path)?;
    let bytes = read_attachment_file(workspace, &path, &origin)?;
    let image = prepare_image(&bytes, max_side)
        .map_err(|e| format!("{shown} cannot be read as an image: {e}"))?;
    let rel = match &origin {
        Origin::Workspace(rel) => Some(rel.as_path()),
        Origin::Attached(_) => None,
    };
    let decided = declass_boundary::images::route(&Facts {
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
        Route::Refuse { rule, message } => Err(format!(
            "{shown} cannot be attached: {message}. {}",
            refusal_help(rule)
        )),
    }
}

/// A routing refusal happens before a content handle is stored. In particular,
/// `ask_local` cannot recover a refused image by accepting its file path.
fn refusal_help(rule: &str) -> String {
    let next = match rule {
        "protected_path" => {
            "This path's policy blocks image inspection; explain that limit to the operator."
        }
        "sensitive_path" => {
            "The operator can configure a local vision-capable model with local.vision enabled, \
then attach the image without --public. Sensitive-path images cannot be sent to the frontier."
        }
        "no_frontier_vision" => {
            "Ask the operator to select a frontier model that supports images and verify \
frontier.vision before retrying."
        }
        "no_local_vision" | "no_vision" => {
            "Ask the operator to configure a local vision-capable model with local.vision enabled, \
then retry the image read. Alternatively, only the operator may approve an image with no sensitive \
content using /image --public PATH in a session or declass run --image-public PATH for a new run; \
this needs a vision-capable frontier with frontier.vision enabled and never overrides sensitive \
or protected paths."
        }
        _ => "Ask the operator to resolve the stated image restriction before retrying.",
    };
    format!(
        "No content handle was created by this refusal. ask_local cannot inspect this path; \
it requires a handle returned by a successful read. Do not invent a handle or change image \
permissions to bypass the refusal. {next}"
    )
}

/// The attachment's origin and bytes. A path inside the workspace is that
/// workspace file (classified by its path); any other is named by its file
/// name only.
fn load_attachment(
    cfg: &RunConfig,
    presenter: &dyn Presenter,
    attachment: &Attachment,
) -> Result<(Origin, Vec<u8>), String> {
    let path = attachment_path(&attachment.path)?;
    let origin = attachment_origin(&cfg.workspace, &path)?;
    if let Origin::Workspace(rel) = &origin
        && !presenter.path_visible(rel)
    {
        return Err(format!("{} is not available", attachment.path.display()));
    }
    let bytes = read_attachment_file(&cfg.workspace, &path, &origin)?;
    Ok((origin, bytes))
}

/// Preserve the selected spelling for workspace policy. Resolving a symlink
/// here could turn `private/screenshot.png` into an apparently public external
/// attachment. Parent traversals are ambiguous in the presence of symlinks.
fn attachment_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("cannot resolve image path: {e}"))?
            .join(path)
    };
    if absolute
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(
            "image paths containing `..` are not supported; choose the file's full path".into(),
        );
    }
    Ok(absolute.components().collect())
}

fn attachment_origin(workspace: &Path, path: &Path) -> Result<Origin, String> {
    let given_workspace = attachment_path(workspace)?;
    let physical_workspace = workspace.canonicalize().ok();
    let platform_path = platform_path(path);
    let relative = path.strip_prefix(&given_workspace).ok().or_else(|| {
        physical_workspace
            .as_ref()
            .and_then(|root| platform_path.strip_prefix(root).ok())
    });
    if let Some(rel) = relative {
        if declass_fs::is_reserved(rel) {
            return Err(format!(
                "{} is not available as an attachment",
                path.display()
            ));
        }
        Ok(Origin::Workspace(rel.to_path_buf()))
    } else {
        Ok(Origin::Attached(path.file_name().map_or_else(
            || "image".into(),
            |name| name.to_string_lossy().into_owned(),
        )))
    }
}

/// A configured workspace root is the host's chosen directory. Resolve only
/// that root after classifying the original relative spelling; every component
/// below it still goes through the nofollow reader.
fn read_attachment_file(
    workspace: &Path,
    selected: &Path,
    origin: &Origin,
) -> Result<Vec<u8>, String> {
    let path = match origin {
        Origin::Workspace(relative) => workspace
            .canonicalize()
            .map_err(|e| format!("cannot open image workspace: {e}"))?
            .join(relative),
        Origin::Attached(_) => selected.to_path_buf(),
    };
    read_image_file(&path)
}

/// macOS exposes OS-managed temporary directories through these aliases. Map
/// only their known targets, after verifying the link itself; never resolve an
/// operator/project-controlled interior symlink. Policy comparisons use the
/// same mapping so `/var/.../workspace/private.png` cannot become "external".
fn platform_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    for (alias, target) in [("/tmp", "/private/tmp"), ("/var", "/private/var")] {
        if let Ok(relative) = path.strip_prefix(alias)
            && let Ok(link) = std::fs::read_link(alias)
            && (link == Path::new(target) || Path::new("/").join(&link) == Path::new(target))
        {
            return Path::new(target).join(relative);
        }
    }
    path.to_path_buf()
}

/// Pin every directory from `/`, refuse symlinks and special files, then bound
/// the read on that same opened handle. A concurrent growth/replacement cannot
/// turn the size check into an unbounded read or redirect it to another file.
fn read_image_file(path: &Path) -> Result<Vec<u8>, String> {
    let path = platform_path(path);
    let relative = path
        .strip_prefix(Path::new("/"))
        .map_err(|_| "image path must be absolute".to_owned())?;
    let parent = declass_fs::pinned::PinnedParent::open(Path::new("/"), relative, false)
        .map_err(|e| format!("cannot read image {}: {e}", path.display()))?;
    let file = parent
        .open_read()
        .map_err(|e| format!("cannot read image {}: {e}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("cannot inspect image {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular image file", path.display()));
    }
    if metadata.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "{} exceeds the {} MiB image limit",
            path.display(),
            MAX_INPUT_BYTES / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read image {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(format!(
            "{} exceeds the {} MiB image limit",
            path.display(),
            MAX_INPUT_BYTES / (1024 * 1024)
        ));
    }
    Ok(bytes)
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
        Route::Refuse { rule, message } => Placed {
            text: Err(format!(
                "{shown} is an image and was not shown: {message}. {}",
                refusal_help(rule)
            )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use declass_boundary::engine::Engine;
    use declass_boundary::images::ToFrontier;
    use declass_boundary::model::solid_png;
    use declass_boundary::view::PassThrough;

    fn fixture() -> (tempfile::TempDir, RunConfig, Vec<u8>) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let workspace = root.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let mut cfg = RunConfig::new(&workspace, workspace.join(".declass/runs/images"), "Task");
        cfg.images.frontier_vision = true;
        (directory, cfg, solid_png(8, 8, [12, 34, 56]))
    }

    fn selected(path: impl Into<PathBuf>, public: bool) -> Attachment {
        Attachment {
            path: path.into(),
            public,
        }
    }

    fn preflight(
        cfg: &RunConfig,
        policy: &Policy,
        attachment: &Attachment,
    ) -> Result<String, String> {
        precheck(
            &cfg.workspace,
            policy,
            true,
            true,
            DEFAULT_MAX_SIDE,
            attachment,
        )
    }

    #[test]
    fn refused_image_reads_explain_handles_and_preserve_privacy() {
        let (_directory, cfg, png) = fixture();
        std::fs::create_dir_all(cfg.workspace.join("docs")).unwrap();
        std::fs::write(cfg.workspace.join("docs/ui.png"), &png).unwrap();
        let (local, received) = declass_boundary::testing::scripted_local(Vec::new());
        let engine = Engine::open(&cfg.run_dir, Policy::default(), Some(local)).unwrap();
        let args = serde_json::json!({"path": "docs/ui.png"});
        let placed = read(&cfg, engine.as_ref(), None, args.as_object().unwrap());
        assert_eq!(placed.destination, Some("none"));
        assert!(placed.images.is_empty());
        let error = placed.text.unwrap_err();
        for expected in [
            "No content handle was created",
            "ask_local cannot inspect this path",
            "local vision-capable model",
            "only the operator may approve",
            "/image --public PATH",
            "declass run --image-public PATH",
            "never overrides sensitive or protected paths",
        ] {
            assert!(error.contains(expected), "{expected}: {error}");
        }
        assert!(
            received.bodies().is_empty(),
            "refusal must not call a model"
        );
        let ask = serde_json::json!({"handle": "h1", "question": "What is shown?"});
        assert!(
            engine
                .call_tool("ask_local", ask.as_object().unwrap())
                .unwrap()
                .unwrap_err()
                .contains("unknown handle")
        );
        let attachment = selected(cfg.workspace.join("docs/ui.png"), false);
        for error in [
            preflight(&cfg, &Policy::default(), &attachment).unwrap_err(),
            check_attachment(&cfg, engine.as_ref(), &attachment).unwrap_err(),
        ] {
            assert!(error.contains("No content handle was created"));
            assert!(error.contains("only the operator may approve"));
        }
    }

    #[test]
    fn protected_and_sensitive_refusals_do_not_offer_public_routing() {
        let (_directory, cfg, png) = fixture();
        let image = prepare_image(&png, DEFAULT_MAX_SIDE).unwrap();
        let policy = Policy {
            sensitive_globs: vec!["private/**".into()],
            sealed: vec!["sealed/**".into()],
            ..Policy::default()
        };
        let engine = Engine::open(&cfg.run_dir, policy, None).unwrap();
        for path in ["private/ui.png", "sealed/ui.png"] {
            let placed = place(
                &cfg,
                engine.as_ref(),
                None,
                &Origin::Workspace(path.into()),
                image.clone(),
                true,
                true,
            );
            assert_eq!(placed.destination, Some("none"));
            assert!(placed.images.is_empty());
            let error = placed.text.unwrap_err();
            assert!(error.contains("No content handle was created"));
            assert!(!error.contains("/image --public"), "{error}");
            assert!(!error.contains("declass run --image-public"), "{error}");
        }
    }

    #[test]
    fn attachment_origins_keep_workspace_privacy_in_precheck_and_live_session() {
        let (directory, cfg, png) = fixture();
        for path in ["docs/ui.png", "private/account.png", "sealed/design.png"] {
            let path = cfg.workspace.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &png).unwrap();
        }
        let outside = directory.path().canonicalize().unwrap().join("outside.png");
        std::fs::write(&outside, &png).unwrap();
        let policy = Policy {
            sensitive_globs: vec!["private/**".into()],
            sealed: vec!["sealed/**".into()],
            images_to_frontier: ToFrontier::Public,
            local_vision: true,
            ..Policy::default()
        };
        let (local, received) = declass_boundary::testing::scripted_local(Vec::new());
        let engine = Engine::open(&cfg.run_dir, policy.clone(), Some(local)).unwrap();
        let private = selected(cfg.workspace.join("private/account.png"), true);
        assert_eq!(
            load_attachment(&cfg, engine.as_ref(), &private).unwrap().0,
            Origin::Workspace("private/account.png".into())
        );
        for attachment in [
            private,
            selected(cfg.workspace.join("sealed/design.png"), true),
        ] {
            assert!(preflight(&cfg, &policy, &attachment).is_err());
            assert!(check_attachment(&cfg, engine.as_ref(), &attachment).is_err());
            assert!(
                attach(&cfg, engine.as_ref(), None, &attachment)
                    .images
                    .is_empty()
            );
        }
        let private = selected(cfg.workspace.join("private/account.png"), false);
        assert!(
            preflight(&cfg, &policy, &private)
                .unwrap()
                .contains("local model will describe")
        );
        assert!(
            check_attachment(&cfg, engine.as_ref(), &private)
                .unwrap()
                .contains("local model will describe")
        );
        let public = selected(cfg.workspace.join("docs/ui.png"), false);
        assert!(
            preflight(&cfg, &policy, &public)
                .unwrap()
                .contains("frontier will see")
        );
        assert!(
            check_attachment(&cfg, engine.as_ref(), &public)
                .unwrap()
                .contains("frontier will see")
        );
        let external = selected(outside, true);
        assert_eq!(
            load_attachment(&cfg, engine.as_ref(), &external).unwrap().0,
            Origin::Attached("outside.png".into())
        );
        assert!(
            preflight(&cfg, &policy, &external)
                .unwrap()
                .contains("frontier will see")
        );
        assert!(
            received.bodies().is_empty(),
            "routing checks do not call a model"
        );
        let no_local = Engine::open(&cfg.run_dir.join("no-local"), policy, None).unwrap();
        assert!(check_attachment(&cfg, no_local.as_ref(), &private).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn configured_workspace_root_alias_preserves_private_relative_paths() {
        use std::os::unix::fs::symlink;
        let (directory, mut cfg, png) = fixture();
        std::fs::create_dir(cfg.workspace.join("private")).unwrap();
        std::fs::write(cfg.workspace.join("private/ui.png"), &png).unwrap();
        symlink("ui.png", cfg.workspace.join("private/link.png")).unwrap();
        let alias = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("workspace-alias");
        symlink(&cfg.workspace, &alias).unwrap();
        cfg.workspace = alias;
        let private = selected(cfg.workspace.join("private/ui.png"), true);
        let policy = Policy {
            sensitive_globs: vec!["private/**".into()],
            ..Policy::default()
        };
        let passthrough = PassThrough { max_bytes: 100 };
        assert_eq!(
            load_attachment(&cfg, &passthrough, &private).unwrap().0,
            Origin::Workspace("private/ui.png".into())
        );
        assert!(
            preflight(&cfg, &policy, &private)
                .unwrap_err()
                .contains("sensitive path")
        );
        assert!(
            load_attachment(
                &cfg,
                &passthrough,
                &selected(cfg.workspace.join("private/link.png"), true)
            )
            .is_err()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_temporary_aliases_are_readable_without_erasing_workspace_policy() {
        // Test the two macOS aliases themselves, independent of the caller's
        // TMPDIR (which may point to another volume during a build).
        for (base, physical, alias) in [
            (PathBuf::from("/private/var/tmp"), "/private/var", "/var"),
            (PathBuf::from("/private/tmp"), "/private/tmp", "/tmp"),
        ] {
            let directory = tempfile::tempdir_in(base).unwrap();
            let root = directory.path().canonicalize().unwrap();
            let workspace = root.join("workspace");
            std::fs::create_dir_all(workspace.join("private")).unwrap();
            let cfg = RunConfig::new(&workspace, workspace.join(".declass/runs/images"), "Task");
            let png = solid_png(8, 8, [12, 34, 56]);
            let alias_root = Path::new(alias).join(root.strip_prefix(physical).unwrap());
            let actual = cfg.workspace.join("private/ui.png");
            std::fs::write(&actual, &png).unwrap();
            let aliased = selected(alias_root.join("workspace/private/ui.png"), true);
            let policy = Policy {
                sensitive_globs: vec!["private/**".into()],
                ..Policy::default()
            };
            assert_eq!(
                load_attachment(&cfg, &PassThrough { max_bytes: 100 }, &aliased)
                    .unwrap()
                    .0,
                Origin::Workspace("private/ui.png".into())
            );
            assert!(
                preflight(&cfg, &policy, &aliased)
                    .unwrap_err()
                    .contains("sensitive path")
            );
            std::fs::write(root.join("external.png"), &png).unwrap();
            let external = selected(alias_root.join("external.png"), true);
            assert!(
                preflight(&cfg, &policy, &external)
                    .unwrap()
                    .contains("frontier will see")
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn attachment_symlinks_cannot_reclassify_private_paths_as_public_images() {
        use std::os::unix::fs::symlink;
        let (directory, cfg, png) = fixture();
        let outside = directory.path().canonicalize().unwrap().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("ui.png"), &png).unwrap();
        std::fs::create_dir(cfg.workspace.join("private")).unwrap();
        symlink(
            outside.join("ui.png"),
            cfg.workspace.join("private/leaf.png"),
        )
        .unwrap();
        symlink(&outside, cfg.workspace.join("private/ancestor")).unwrap();
        symlink(
            cfg.workspace.join("private/leaf.png"),
            outside.join("alias.png"),
        )
        .unwrap();
        let policy = Policy {
            sensitive_globs: vec!["private/**".into()],
            ..Policy::default()
        };
        let presenter = PassThrough { max_bytes: 100 };
        for path in [
            cfg.workspace.join("private/leaf.png"),
            cfg.workspace.join("private/ancestor/ui.png"),
            outside.join("alias.png"),
        ] {
            let attachment = selected(path, true);
            assert!(preflight(&cfg, &policy, &attachment).is_err());
            assert!(check_attachment(&cfg, &presenter, &attachment).is_err());
            let placed = attach(&cfg, &presenter, None, &attachment);
            assert!(placed.text.is_err() && placed.images.is_empty());
            assert!(
                placed.destination.is_none(),
                "symlink must fail before routing"
            );
        }
    }

    #[test]
    fn hidden_reserved_oversized_and_nonregular_attachments_fail_closed() {
        struct Hidden;
        impl Presenter for Hidden {
            fn path_visible(&self, path: &Path) -> bool {
                !path.starts_with("hidden")
            }
            fn present(&self, _: &Source, _: &[u8]) -> String {
                panic!("hidden image was presented")
            }
        }
        let (_directory, cfg, png) = fixture();
        for folder in ["hidden", ".declass", ".git"] {
            let directory = cfg.workspace.join(folder);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("ui.png"), &png).unwrap();
            let attachment = selected(directory.join("ui.png"), true);
            assert!(load_attachment(&cfg, &Hidden, &attachment).is_err());
            if folder != "hidden" {
                assert!(preflight(&cfg, &Policy::default(), &attachment).is_err());
            }
        }
        let large = cfg.workspace.join("large.png");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(MAX_INPUT_BYTES + 1)
            .unwrap();
        let oversized = selected(large, true);
        assert!(
            load_attachment(&cfg, &Hidden, &oversized)
                .unwrap_err()
                .contains("limit")
        );
        assert!(
            preflight(&cfg, &Policy::default(), &oversized)
                .unwrap_err()
                .contains("limit")
        );
        assert!(read_image_file(&cfg.workspace).is_err());
        assert!(attachment_path(&cfg.workspace.join("../outside.png")).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn resumed_images_use_bounded_nofollow_reads_and_validate_digest_names() {
        use std::os::unix::fs::symlink;
        let (directory, cfg, png) = fixture();
        let image = prepare_image(&png, DEFAULT_MAX_SIDE).unwrap();
        store(&cfg.run_dir, &image).unwrap();
        let unloaded: Image =
            serde_json::from_value(serde_json::to_value(&image).unwrap()).unwrap();
        let reload = |image: Image| {
            let mut items = [Item::Images {
                call_id: None,
                images: vec![image],
            }];
            load(&cfg.run_dir, &mut items);
            let Item::Images { images, .. } = items.into_iter().next().unwrap() else {
                unreachable!()
            };
            images
        };
        assert!(reload(unloaded.clone())[0].is_loaded());
        let stored = stored_path(&cfg.run_dir, &image);
        std::fs::File::create(&stored)
            .unwrap()
            .set_len(MAX_INPUT_BYTES + 1)
            .unwrap();
        assert!(reload(unloaded.clone()).is_empty());
        std::fs::remove_file(&stored).unwrap();
        let outside = directory.path().canonicalize().unwrap().join("outside.png");
        std::fs::write(&outside, image.data()).unwrap();
        symlink(outside, &stored).unwrap();
        assert!(
            reload(unloaded.clone()).is_empty(),
            "matching digest does not authorize following symlinks"
        );
        let mut invalid = unloaded;
        invalid.sha256 = "é".repeat(32);
        assert!(reload(invalid).is_empty());
    }
}
