// SPDX-License-Identifier: GPL-3.0-or-later
//! Owner-only state files: directories 0700, files 0600, appends synced.

use crate::error::FsError;
use std::fs::{self, OpenOptions};
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

/// Reads a JSON-lines file, dropping a torn final line left by a crash.
/// Returns the complete lines; the file is truncated to them.
pub fn read_lines_repairing(path: &Path) -> Result<Vec<String>, FsError> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(FsError::io("read", path, e)),
    };
    let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    if complete < bytes.len() {
        let f = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| FsError::io("open", path, e))?;
        f.set_len(complete as u64)
            .map_err(|e| FsError::io("truncate torn tail", path, e))?;
        f.sync_all().map_err(|e| FsError::io("sync", path, e))?;
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
