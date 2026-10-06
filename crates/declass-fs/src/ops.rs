// SPDX-License-Identifier: GPL-3.0-or-later
//! Guarded reads and all-or-nothing writes.

use crate::error::FsError;
use crate::pinned::PinnedParent;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// What the writer expects the target to be before replacing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// No check (the caller accepts any current content).
    Any,
    /// The target must not exist.
    Absent,
    /// The target must currently hash to this digest.
    Sha256(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReceipt {
    /// Digest before the write (`None` if the file did not exist).
    pub before: Option<String>,
    pub after: String,
    /// Content before the write, for rollback.
    pub before_bytes: Option<Vec<u8>>,
}

/// Reads a regular file under `root` without following links. `max` caps the size.
pub fn read_file(root: &Path, rel: &Path, max: u64) -> Result<Vec<u8>, FsError> {
    let pinned = PinnedParent::open(root, rel, false)?;
    let file = pinned.open_read()?;
    let mut out = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut out)
        .map_err(|e| FsError::io("read", pinned.path(), e))?;
    if out.len() as u64 > max {
        return Err(FsError::io(
            "read",
            pinned.path(),
            format!("file exceeds {max} bytes"),
        ));
    }
    Ok(out)
}

/// Reads if present.
pub fn read_optional(root: &Path, rel: &Path, max: u64) -> Result<Option<Vec<u8>>, FsError> {
    let pinned = match PinnedParent::open(root, rel, false) {
        Ok(p) => p,
        Err(FsError::Io { .. }) => return Ok(None),
        Err(e) => return Err(e),
    };
    if pinned.file_type()?.is_none() {
        return Ok(None);
    }
    read_file(root, rel, max).map(Some)
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Writes `bytes` to `rel` atomically: temp file in the pinned parent, fsync,
/// rename over the target, fsync the directory. The precondition is checked
/// against the current content immediately before the rename.
pub fn atomic_write(
    root: &Path,
    rel: &Path,
    bytes: &[u8],
    pre: &Precondition,
    mode: u32,
) -> Result<WriteReceipt, FsError> {
    let pinned = PinnedParent::open(root, rel, true)?;
    let before_bytes = match pinned.file_type()? {
        None => None,
        Some(rustix::fs::FileType::RegularFile) => {
            let mut buf = Vec::new();
            pinned
                .open_read()?
                .read_to_end(&mut buf)
                .map_err(|e| FsError::io("read", pinned.path(), e))?;
            Some(buf)
        }
        Some(rustix::fs::FileType::Symlink) => {
            return Err(FsError::Symlink(pinned.path().to_path_buf()));
        }
        Some(_) => return Err(FsError::NotAFile(rel.display().to_string())),
    };
    let before = before_bytes.as_deref().map(sha256_hex);
    match (pre, &before) {
        (Precondition::Any, _) | (Precondition::Absent, None) => {}
        (Precondition::Absent, Some(_)) => return Err(FsError::Exists(rel.display().to_string())),
        (Precondition::Sha256(expected), actual)
            if actual.as_deref() != Some(expected.as_str()) =>
        {
            return Err(FsError::Precondition {
                path: rel.display().to_string(),
                expected: expected.clone(),
                actual: actual.clone().unwrap_or_else(|| "absent".into()),
            });
        }
        _ => {}
    }
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = OsString::from(format!(".declass-tmp-{}-{n}", std::process::id()));
    let result = (|| {
        let mut f = pinned.create_temp(&temp, mode)?;
        crate::fault::write_all(&mut f, "atomic write", pinned.path(), bytes)
            .map_err(|e| FsError::io("write", pinned.path(), e))?;
        f.sync_all()
            .map_err(|e| FsError::io("sync", pinned.path(), e))?;
        pinned.rename_into_place(&temp)?;
        pinned.sync()
    })();
    if result.is_err() {
        pinned.remove_temp(&temp);
    }
    result?;
    Ok(WriteReceipt {
        before,
        after: sha256_hex(bytes),
        before_bytes,
    })
}

/// Removes a regular file under `root`.
pub fn remove_file(root: &Path, rel: &Path) -> Result<(), FsError> {
    let pinned = PinnedParent::open(root, rel, false)?;
    match pinned.file_type()? {
        Some(rustix::fs::FileType::RegularFile) => {}
        Some(rustix::fs::FileType::Symlink) => {
            return Err(FsError::Symlink(pinned.path().to_path_buf()));
        }
        Some(_) => return Err(FsError::NotAFile(rel.display().to_string())),
        None => return Ok(()),
    }
    pinned.remove()?;
    pinned.sync()
}

/// Restores a file to its pre-write state (`None` removes it).
pub fn restore(root: &Path, rel: &Path, before: Option<&[u8]>) -> Result<(), FsError> {
    match before {
        Some(bytes) => atomic_write(root, rel, bytes, &Precondition::Any, 0o644).map(|_| ()),
        None => remove_file(root, rel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn writes_reads_and_checks_preconditions() {
        let d = root();
        let p = d.path().canonicalize().unwrap();
        let rel = PathBuf::from("src/new/file.txt");
        let r = atomic_write(&p, &rel, b"one", &Precondition::Absent, 0o644).unwrap();
        assert_eq!(r.before, None);
        assert_eq!(read_file(&p, &rel, 100).unwrap(), b"one");
        assert!(matches!(
            atomic_write(&p, &rel, b"x", &Precondition::Absent, 0o644),
            Err(FsError::Exists(_))
        ));
        let stale = Precondition::Sha256(sha256_hex(b"other"));
        assert!(matches!(
            atomic_write(&p, &rel, b"x", &stale, 0o644),
            Err(FsError::Precondition { .. })
        ));
        let r2 = atomic_write(&p, &rel, b"two", &Precondition::Sha256(r.after), 0o644).unwrap();
        assert_eq!(r2.before_bytes.as_deref(), Some(&b"one"[..]));
        restore(&p, &rel, r2.before_bytes.as_deref()).unwrap();
        assert_eq!(read_file(&p, &rel, 100).unwrap(), b"one");
        assert!(std::fs::read_dir(p.join("src/new")).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".declass-tmp")
        }));
    }

    #[test]
    fn refuses_symlinks_on_the_path_and_at_the_leaf() {
        let d = root();
        let p = d.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"s").unwrap();
        std::os::unix::fs::symlink(outside.path(), p.join("linkdir")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), p.join("linkfile")).unwrap();
        assert!(matches!(
            read_file(&p, Path::new("linkdir/secret"), 10),
            Err(FsError::Symlink(_))
        ));
        assert!(matches!(
            read_file(&p, Path::new("linkfile"), 10),
            Err(FsError::Symlink(_))
        ));
        assert!(matches!(
            atomic_write(
                &p,
                Path::new("linkdir/secret"),
                b"x",
                &Precondition::Any,
                0o644
            ),
            Err(FsError::Symlink(_))
        ));
        assert_eq!(std::fs::read(outside.path().join("secret")).unwrap(), b"s");
    }

    #[test]
    fn size_cap_and_missing_files() {
        let d = root();
        let p = d.path().canonicalize().unwrap();
        std::fs::write(p.join("big"), vec![b'x'; 50]).unwrap();
        assert!(read_file(&p, Path::new("big"), 10).is_err());
        assert_eq!(read_optional(&p, Path::new("nope/none"), 10).unwrap(), None);
        remove_file(&p, Path::new("big")).unwrap();
        assert!(!p.join("big").exists());
    }
}
