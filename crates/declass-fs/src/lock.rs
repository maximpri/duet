// SPDX-License-Identifier: GPL-3.0-or-later
//! One Declass run per workspace at a time.

use crate::error::FsError;
use crate::private::ensure_private_dir;
use rustix::fs::FlockOperation;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Held for the life of a run; released on drop.
#[derive(Debug)]
pub struct WorkspaceLock {
    _file: File,
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        // Release the advisory lock before closing the descriptor. On macOS,
        // close alone can leave a brief stale lock visible to another thread.
        let _ = rustix::fs::flock(&self._file, FlockOperation::Unlock);
    }
}

impl WorkspaceLock {
    pub fn acquire(workspace: &Path) -> Result<Self, FsError> {
        let dir = workspace.join(crate::registry::DECLASS_DIR);
        ensure_private_dir(&dir)?;
        let path = workspace.join(".declass/lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&path)
            .map_err(|e| FsError::io("open lock", &path, e))?;
        match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self { _file: file }),
            Err(rustix::io::Errno::WOULDBLOCK) => Err(FsError::Locked),
            Err(e) => Err(FsError::io("lock", &path, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_fails_until_first_drops() {
        let d = tempfile::tempdir().unwrap();
        let a = WorkspaceLock::acquire(d.path()).unwrap();
        assert!(matches!(
            WorkspaceLock::acquire(d.path()),
            Err(FsError::Locked)
        ));
        drop(a);
        let reacquired = WorkspaceLock::acquire(d.path());
        assert!(
            reacquired.is_ok(),
            "lock stayed held after drop: {reacquired:?}"
        );
    }
}
