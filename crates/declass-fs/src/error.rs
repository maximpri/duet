// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("path {0} is outside the workspace")]
    Outside(String),
    #[error("path {0} is reserved (.git or .declass)")]
    Reserved(String),
    #[error("{0} is a symbolic link; links are never followed")]
    Symlink(PathBuf),
    #[error("{0} is not a regular file")]
    NotAFile(String),
    #[error("{0} already exists")]
    Exists(String),
    #[error("{path} changed since it was read (expected {expected}, found {actual})")]
    Precondition {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("cannot {op} {path}: {message}")]
    Io {
        op: &'static str,
        path: PathBuf,
        message: String,
        /// The operating system's error number, when the cause was an OS error.
        errno: Option<i32>,
    },
    #[error("workspace is locked by another declass process")]
    Locked,
}

/// OS errors that report the host running short of a resource (disk space,
/// quota, file handles, kernel memory), not a defect in the operation: the
/// same operation succeeds once the resource is back.
const HOST_RESOURCE: &[rustix::io::Errno] = &[
    rustix::io::Errno::NOSPC,
    rustix::io::Errno::DQUOT,
    rustix::io::Errno::NFILE,
    rustix::io::Errno::MFILE,
    rustix::io::Errno::NOBUFS,
    rustix::io::Errno::NOMEM,
    rustix::io::Errno::AGAIN,
    // A storage device that drops out and comes back (a USB disk): the same
    // write succeeds once it is back.
    rustix::io::Errno::IO,
    rustix::io::Errno::NXIO,
];

impl FsError {
    /// An I/O failure of `op` on `path`. When `e` is an OS error
    /// (`std::io::Error` or `rustix::io::Errno`) its number is kept, so a
    /// transient host failure can be told apart ([`FsError::is_host_resource`]).
    pub fn io(
        op: &'static str,
        path: impl Into<PathBuf>,
        e: impl std::fmt::Display + 'static,
    ) -> Self {
        let any: &dyn std::any::Any = &e;
        let errno = any
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            .or_else(|| {
                any.downcast_ref::<rustix::io::Errno>()
                    .map(|n| n.raw_os_error())
            });
        Self::Io {
            op,
            path: path.into(),
            message: e.to_string(),
            errno,
        }
    }

    /// Whether this is the host running short of a resource (a full disk,
    /// exhausted quota or file handles): transient, so the operation is
    /// retried in place rather than failing the run.
    pub fn is_host_resource(&self) -> bool {
        self.host_errno().is_some()
    }

    fn host_errno(&self) -> Option<rustix::io::Errno> {
        match self {
            Self::Io { errno: Some(n), .. } => HOST_RESOURCE
                .iter()
                .copied()
                .find(|e| e.raw_os_error() == *n),
            _ => None,
        }
    }

    /// What the host is short of, for the operator: `"disk full"` for a full
    /// disk or quota. `None` unless [`FsError::is_host_resource`].
    pub fn host_shortage(&self) -> Option<&'static str> {
        use rustix::io::Errno;
        Some(match self.host_errno()? {
            Errno::NOSPC | Errno::DQUOT => "disk full",
            Errno::NFILE | Errno::MFILE => "out of file handles",
            Errno::IO | Errno::NXIO => "storage device unavailable",
            _ => "host out of memory or buffers",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_errors_keep_their_number_and_host_shortages_are_recognised() {
        let full = FsError::io(
            "append",
            "/x",
            std::io::Error::from_raw_os_error(libc_enospc()),
        );
        assert!(full.is_host_resource());
        assert_eq!(full.host_shortage(), Some("disk full"));
        let quota = FsError::io("write", "/x", rustix::io::Errno::DQUOT);
        assert_eq!(quota.host_shortage(), Some("disk full"));
        let handles = FsError::io("open", "/x", rustix::io::Errno::MFILE);
        assert_eq!(handles.host_shortage(), Some("out of file handles"));
        let dropped = FsError::io("append", "/x", rustix::io::Errno::NXIO);
        assert_eq!(dropped.host_shortage(), Some("storage device unavailable"));
        assert!(FsError::io("write", "/x", rustix::io::Errno::IO).is_host_resource());
        for other in [
            FsError::io(
                "open",
                "/x",
                std::io::Error::from(std::io::ErrorKind::NotFound),
            ),
            FsError::io("open", "/x", rustix::io::Errno::ACCESS),
            FsError::io("parse", "/x", "not json"),
            FsError::Locked,
        ] {
            assert!(!other.is_host_resource(), "{other}");
        }
        // The message is unchanged.
        assert!(full.to_string().starts_with("cannot append /x: "));
    }

    fn libc_enospc() -> i32 {
        rustix::io::Errno::NOSPC.raw_os_error()
    }
}
