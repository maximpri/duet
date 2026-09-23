// SPDX-License-Identifier: GPL-3.0-or-later
//! Owner-only state files: directories 0700, files 0600, appends synced.

use crate::error::FsError;
use std::fs::{self, OpenOptions};
use std::io::Write;
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

/// Appends one line (a trailing newline is added) and syncs the data.
pub fn append_line(path: &Path, line: &str) -> Result<(), FsError> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|e| FsError::io("open for append", path, e))?;
    let mut buf = line.as_bytes().to_vec();
    buf.push(b'\n');
    f.write_all(&buf)
        .map_err(|e| FsError::io("append", path, e))?;
    f.sync_data().map_err(|e| FsError::io("sync", path, e))
}

/// Writes a whole private file atomically (temp + rename).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), FsError> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&tmp)
            .map_err(|e| FsError::io("create", &tmp, e))?;
        f.write_all(bytes)
            .map_err(|e| FsError::io("write", &tmp, e))?;
        f.sync_all().map_err(|e| FsError::io("sync", &tmp, e))?;
    }
    fs::rename(&tmp, path).map_err(|e| FsError::io("rename", path, e))
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
}
