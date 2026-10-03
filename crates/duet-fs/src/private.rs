// SPDX-License-Identifier: GPL-3.0-or-later
//! Owner-only state files: directories 0700, files 0600, appends synced.

use crate::error::FsError;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Creates `dir` (and parents) with mode 0700, refusing symlinks.
pub fn ensure_private_dir(dir: &Path) -> Result<(), FsError> {
    if let Ok(meta) = fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(FsError::Symlink(dir.to_path_buf()));
        }
        if !meta.is_dir() {
            return Err(FsError::NotAFile(dir.display().to_string()));
        }
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| FsError::io("chmod", dir, e))?;
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| FsError::io("create directory", dir, e))
}

/// Appends one line (a trailing newline is added) and syncs the data. All or
/// nothing: when the write or the sync fails (a full disk), the file is cut
/// back to its previous length, so a retry never follows a torn line.
pub fn append_line(path: &Path, line: &str) -> Result<(), FsError> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|e| FsError::io("open for append", path, e))?;
    let start = f
        .metadata()
        .map_err(|e| FsError::io("inspect", path, e))?
        .len();
    let mut buf = line.as_bytes().to_vec();
    buf.push(b'\n');
    let written = crate::fault::write_all(&mut f, "append", path, &buf)
        .map_err(|e| FsError::io("append", path, e))
        .and_then(|()| f.sync_data().map_err(|e| FsError::io("sync", path, e)));
    if written.is_err() {
        // Shrinking frees space, so this works on a full disk.
        let _ = f.set_len(start).and_then(|()| f.sync_data());
    }
    written
}

/// Writes a whole private file atomically (temp + rename). A failed attempt
/// leaves the target as it was and removes its temporary file.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), FsError> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let written = (|| {
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&tmp)
            .map_err(|e| FsError::io("create", &tmp, e))?;
        crate::fault::write_all(&mut f, "write", path, bytes)
            .map_err(|e| FsError::io("write", &tmp, e))?;
        f.sync_all().map_err(|e| FsError::io("sync", &tmp, e))?;
        drop(f);
        fs::rename(&tmp, path).map_err(|e| FsError::io("rename", path, e))
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Durable private replacement through a pinned parent. Unlike path-based
/// writes, a renamed or symlink-swapped parent cannot redirect the operation.
pub fn write_private_pinned(
    pinned: &crate::pinned::PinnedParent,
    bytes: &[u8],
) -> Result<(), FsError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = std::ffi::OsString::from(format!(
        ".private-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = pinned.create_temp(&temp, 0o600)?;
        crate::fault::write_all(&mut file, "write", pinned.path(), bytes)
            .map_err(|e| FsError::io("write", pinned.path(), e))?;
        file.sync_all()
            .map_err(|e| FsError::io("sync", pinned.path(), e))?;
        drop(file);
        pinned.rename_into_place(&temp)?;
        pinned.sync()
    })();
    if written.is_err() {
        pinned.remove_temp(&temp);
    }
    written
}

/// Reads a JSON-lines file, dropping a torn final line left by a crash.
/// Returns the complete lines; the file is truncated to them. Read and repair
/// use the same regular-file handle without following a leaf symlink. An
/// intact log can still be read when its permissions or filesystem are read-only.
pub fn read_lines_repairing(path: &Path) -> Result<Vec<String>, FsError> {
    let flags = (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32;
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(flags)
        .open(path)
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied
                || e.raw_os_error() == Some(rustix::io::Errno::ROFS.raw_os_error())
            {
                OpenOptions::new().read(true).custom_flags(flags).open(path)
            } else {
                Err(e)
            }
        }) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
            return Err(FsError::Symlink(path.to_path_buf()));
        }
        Err(e) => return Err(FsError::io("open for repair", path, e)),
    };
    if !file
        .metadata()
        .map_err(|e| FsError::io("inspect", path, e))?
        .is_file()
    {
        return Err(FsError::NotAFile(path.display().to_string()));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| FsError::io("read", path, e))?;
    let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    if complete < bytes.len() {
        file.set_len(complete as u64)
            .map_err(|e| FsError::io("truncate torn tail", path, e))?;
        file.sync_all().map_err(|e| FsError::io("sync", path, e))?;
    }
    Ok(String::from_utf8_lossy(&bytes[..complete])
        .lines()
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn private_modes_and_torn_tail_repair() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("runs/r1");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let log = dir.join("t.jsonl");
        append_line(&log, "{\"a\":1}").unwrap();
        append_line(&log, "{\"a\":2}").unwrap();
        assert_eq!(
            fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let mut f = OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(b"{\"a\":3").unwrap();
        assert_eq!(read_lines_repairing(&log).unwrap().len(), 2);
        assert!(fs::read_to_string(&log).unwrap().ends_with("}\n"));
        write_private(&dir.join("v.json"), b"{}").unwrap();
        assert_eq!(
            fs::metadata(dir.join("v.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn repair_refuses_symlinks_without_reading_or_truncating_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("original");
        let log = dir.path().join("log.jsonl");
        let content = b"complete\nkeep this incomplete tail";
        fs::write(&target, content).unwrap();
        std::os::unix::fs::symlink(&target, &log).unwrap();
        assert!(matches!(
            read_lines_repairing(&log),
            Err(FsError::Symlink(_))
        ));
        assert_eq!(fs::read(&target).unwrap(), content);
        fs::remove_file(target).unwrap();
        assert!(
            read_lines_repairing(&log).is_err(),
            "a dangling link is not a missing log"
        );
    }

    #[test]
    fn pinned_private_write_cannot_be_redirected_by_a_parent_symlink_swap() {
        let d = tempfile::tempdir().unwrap();
        let original = d.path().join("state");
        let moved = d.path().join("old-state");
        let outside = d.path().join("outside");
        ensure_private_dir(&original).unwrap();
        ensure_private_dir(&outside).unwrap();
        let pinned =
            crate::pinned::PinnedParent::open(d.path(), Path::new("state/derived.json"), false)
                .unwrap();
        fs::rename(&original, &moved).unwrap();
        std::os::unix::fs::symlink(&outside, &original).unwrap();
        write_private_pinned(&pinned, b"[\"export.txt\"]").unwrap();
        assert_eq!(
            fs::read(moved.join("derived.json")).unwrap(),
            b"[\"export.txt\"]"
        );
        assert!(!outside.join("derived.json").exists());
        assert_eq!(
            fs::metadata(moved.join("derived.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn complete_read_only_logs_remain_readable() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        fs::write(&log, b"complete\n").unwrap();
        fs::set_permissions(&log, fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(read_lines_repairing(&log).unwrap(), ["complete"]);
        assert_eq!(fs::read(&log).unwrap(), b"complete\n");
    }

    #[test]
    fn repair_refuses_a_fifo_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(
            read_lines_repairing(&path),
            Err(FsError::NotAFile(_))
        ));
        assert!(
            read_lines_repairing(&dir.path().join("missing"))
                .unwrap()
                .is_empty()
        );
    }

    /// A writer that fills the disk half way through a line.
    fn full_after(bytes: usize) -> crate::fault::Fault {
        crate::fault::Fault {
            after_bytes: bytes,
            errno: rustix::io::Errno::NOSPC,
        }
    }

    #[test]
    fn a_failed_append_or_write_leaves_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().canonicalize().unwrap();
        let log = dir.join("t.jsonl");
        append_line(&log, "{\"a\":1}").unwrap();
        let before = fs::read(&log).unwrap();
        let fails = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(2));
        let left = fails.clone();
        let _guard = crate::fault::inject(&dir, move |_, _| {
            (left
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |n| n.checked_sub(1),
                )
                .is_ok())
            .then(|| full_after(3))
        });
        let e = append_line(&log, "{\"a\":2}").unwrap_err();
        assert!(e.is_host_resource(), "{e}");
        assert_eq!(fs::read(&log).unwrap(), before, "a torn line was left");
        let e = write_private(&dir.join("v.json"), b"{\"b\":1}").unwrap_err();
        assert!(e.is_host_resource(), "{e}");
        assert!(!dir.join("v.json").exists());
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "a temporary file was left: {names:?}");
        // The same calls succeed once space is back, exactly once each.
        append_line(&log, "{\"a\":2}").unwrap();
        write_private(&dir.join("v.json"), b"{\"b\":1}").unwrap();
        assert_eq!(
            read_lines_repairing(&log).unwrap(),
            ["{\"a\":1}", "{\"a\":2}"]
        );
        assert_eq!(fs::read(dir.join("v.json")).unwrap(), b"{\"b\":1}");
    }
}
