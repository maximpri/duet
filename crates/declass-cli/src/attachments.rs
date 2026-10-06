// SPDX-License-Identifier: GPL-3.0-or-later
//! Operator-selected text files. Snapshot at attachment time; never grant the
//! model filesystem access outside the workspace. Rendering uses file policy.

use anyhow::{Context, Result, ensure};
use declass_boundary::view::{Presenter, Source};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_BYTES: usize = 512 * 1024;
const MAX_TOTAL: usize = 2 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Attachment {
    path: PathBuf,
    selected: PathBuf,
    text: String,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachment")
            .field("path", &self.path)
            .field("bytes", &self.text.len())
            .finish()
    }
}

impl Attachment {
    pub(crate) fn label(&self) -> String {
        let name = self
            .path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy();
        format!("{name} · text · {} bytes", self.text.len())
    }
}

/// Decode one dragged/quoted filename without a shell, variable expansion or
/// command substitution. Plain spaces are accepted for `/attach path with spaces`.
pub(crate) fn path(raw: &str) -> Result<PathBuf> {
    let mut out = String::new();
    let mut quote = None;
    let mut chars = raw.trim().chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            (None, '\\') => {
                out.push(
                    chars
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("unfinished path escape"))?,
                );
            }
            (Some('"'), '\\') => {
                let next = chars
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("unfinished path escape"))?;
                if !matches!(next, '\\' | '"' | '$' | '`') {
                    out.push('\\');
                }
                out.push(next);
            }
            _ => out.push(c),
        }
    }
    ensure!(quote.is_none(), "unfinished path quote");
    ensure!(!out.is_empty(), "choose a file path");
    ensure!(
        !out.contains(['\n', '\r', '\0']),
        "attach one file per line"
    );
    if let Some(rest) = out.strip_prefix("~/") {
        let home = std::env::var_os("HOME").context("home directory unavailable")?;
        return Ok(PathBuf::from(home).join(rest));
    }
    Ok(PathBuf::from(out))
}

/// A path encoded for Declass's path parser, never for shell execution.
pub(crate) fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// Resolve an operator-selected path while retaining its spelling for policy checks.
pub(crate) fn selected(ws: &Path, raw: &str) -> Result<PathBuf> {
    // The displayed workspace is the base even when `--workspace` differs
    // from the shell directory. Never silently pick a namesake outside it.
    Ok(ws.join(path(raw)?))
}

/// Explicit path syntax is an attachment, even when it is missing (so the
/// operator gets an error instead of an ignored command). Command names win.
pub(crate) fn is_path(raw: &str) -> bool {
    path(raw).is_ok_and(|p| {
        (p.is_absolute() && (p.components().count() > 2 || p.extension().is_some() || p.is_file()))
            || ["./", "../", "~/"]
                .iter()
                .any(|prefix| raw.trim_start_matches(['\'', '"']).starts_with(prefix))
    })
}

pub(crate) fn add(pending: &mut Vec<Attachment>, ws: &Path, raw: &str) -> Result<String> {
    ensure!(
        pending.len() < 16,
        "at most 16 text files can wait for one message"
    );
    let selected = selected(ws, raw)?;
    let path = selected
        .canonicalize()
        .with_context(|| format!("cannot attach {}", selected.display()))?;
    // Reject devices/directories/FIFOs, including a swap between stat and open.
    ensure!(
        path.metadata()?.is_file(),
        "attachment must be a regular text file"
    );
    let fd = rustix::fs::open(
        &path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )?;
    let file = std::fs::File::from(fd);
    ensure!(
        file.metadata()?.is_file(),
        "attachment must be a regular text file"
    );
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "text attachment exceeds 512 KiB; split it into smaller files"
    );
    ensure!(
        pending.iter().map(|a| a.text.len()).sum::<usize>() + bytes.len() <= MAX_TOTAL,
        "text attachments exceed 2 MiB for one message"
    );
    let text =
        String::from_utf8(bytes).context("attachment must be UTF-8 text; use /image for images")?;
    ensure!(
        !text.contains('\0'),
        "attachment is binary; use a text file or /image"
    );
    let notice = format!(
        "attached text file {} ({} bytes); included with your next message",
        path.display(),
        text.len()
    );
    pending.push(Attachment {
        path,
        selected,
        text,
    });
    Ok(notice)
}

/// The snapshot, not a later version on disk, accompanies the next operator
/// message. Sensitive and bulky files become the boundary's readable handles.
pub(crate) fn message(
    message: String,
    pending: &mut Vec<Attachment>,
    ws: &Path,
    presenter: &dyn Presenter,
) -> String {
    let mut out = message;
    for a in pending.drain(..) {
        let requested = a.selected.strip_prefix(ws).unwrap_or(&a.selected);
        let resolved = a.path.strip_prefix(ws).unwrap_or(&a.path);
        let path =
            if presenter.path_sensitive(requested) || presenter.protection(requested).is_some() {
                requested
            } else {
                resolved
            }
            .to_path_buf();
        let shown = presenter.present(
            &Source::File {
                path: path.clone(),
                ranged: false,
            },
            a.text.as_bytes(),
        );
        out.push_str(&format!("\n\nOperator-attached text file: {}\nRead this attachment when carrying out the request. If it is a handle, use the indicated read tools.\n<attached_file>\n{shown}\n</attached_file>\n", path.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dragged_paths_are_decoded_without_evaluation() {
        assert_eq!(
            path(r"/Downloads/prompt\ \(1\).md").unwrap(),
            Path::new("/Downloads/prompt (1).md")
        );
        assert_eq!(
            path("'/Downloads/prompt (1).md'").unwrap(),
            Path::new("/Downloads/prompt (1).md")
        );
        assert_eq!(
            path("\"/Downloads/prompt (1).md\"").unwrap(),
            Path::new("/Downloads/prompt (1).md")
        );
        assert_eq!(
            path("'/tmp/$(touch x)`id`.md'").unwrap(),
            Path::new("/tmp/$(touch x)`id`.md")
        );
        assert!(path("'unfinished").is_err());
        let unusual = Path::new("/tmp/Pat's \\ sketch.png");
        assert_eq!(path(&quoted(unusual)).unwrap(), unusual);
        assert_eq!(
            path(r#""/tmp/a\b.png""#).unwrap(),
            Path::new(r"/tmp/a\b.png")
        );
    }
    #[test]
    fn a_snapshot_survives_changes_and_invalid_files_do_not_queue() {
        let d = tempfile::tempdir().unwrap();
        // A common name exists in the shell's repository as well. The TUI's
        // workspace must win without an existence-based fallback outside it.
        std::fs::write(d.path().join("Cargo.toml"), "selected workspace").unwrap();
        let mut selected_files = Vec::new();
        add(&mut selected_files, d.path(), "Cargo.toml").unwrap();
        assert_eq!(selected_files[0].text, "selected workspace");
        let p = d.path().join("spec (1).md");
        std::fs::write(&p, "original instructions").unwrap();
        let mut pending = vec![];
        add(&mut pending, d.path(), p.to_str().unwrap()).unwrap();
        std::fs::write(&p, "different instructions").unwrap();
        assert!(add(&mut pending, d.path(), d.path().to_str().unwrap()).is_err());
        std::fs::write(&p, [0, 255]).unwrap();
        assert!(add(&mut pending, d.path(), p.to_str().unwrap()).is_err());
        let shown = message(
            "implement".into(),
            &mut pending,
            d.path(),
            &declass_boundary::view::PassThrough {
                max_bytes: MAX_BYTES,
            },
        );
        assert!(shown.contains("original instructions"));
        assert!(!shown.contains("different instructions"));
        assert!(pending.is_empty());
    }

    #[test]
    fn sensitive_attachments_are_handles_and_public_secrets_are_redacted() {
        use declass_boundary::{engine::Engine, policy::Policy};
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let sensitive = ws.join("private.md");
        std::fs::write(
            &sensitive,
            "Private customer instruction: azure_hedgehog_canary_9482",
        )
        .unwrap();
        let e = Engine::open(
            &d.path().join("run"),
            Policy {
                sensitive_globs: vec!["private.md".into()],
                detect_secrets: true,
                detect_pii: true,
                ..Policy::default()
            },
            None,
        )
        .unwrap();
        let mut pending = vec![];
        add(&mut pending, &ws, sensitive.to_str().unwrap()).unwrap();
        let shown = message("implement".into(), &mut pending, &ws, e.as_ref());
        assert!(!shown.contains("azure_hedgehog_canary_9482"), "{shown}");
        assert!(shown.contains("h1"), "{shown}");
        let public = ws.join("spec.md");
        std::fs::write(&public, "Build a game. Card: 5293761582049377").unwrap();
        add(&mut pending, &ws, public.to_str().unwrap()).unwrap();
        let shown = message("implement".into(), &mut pending, &ws, e.as_ref());
        assert!(shown.contains("Build a game"), "{shown}");
        assert!(!shown.contains("5293761582049377"), "{shown}");
    }
}
