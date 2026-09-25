// SPDX-License-Identifier: GPL-3.0-or-later
//! Injected write failures, for tests of how callers survive a full disk.
//!
//! With the `fault-injection` feature, a test registers a rule for the files
//! under a directory it owns; the state writers ([`crate::private`],
//! [`crate::atomic_write`]) consult it before writing and, when it fires, write
//! the given number of bytes and then fail with the given OS error, as a disk
//! filling up mid-write does. Without the feature nothing is ever injected.

use std::path::Path;

pub use rustix::io::Errno;

/// One injected failure: write `after_bytes` bytes, then fail with `errno`.
#[derive(Debug, Clone, Copy)]
pub struct Fault {
    pub after_bytes: usize,
    pub errno: Errno,
}

#[cfg(any(test, feature = "fault-injection"))]
mod rules {
    use super::Fault;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub type Rule = Box<dyn FnMut(&'static str, &Path) -> Option<Fault> + Send>;

    static RULES: Mutex<Vec<(u64, PathBuf, Rule)>> = Mutex::new(Vec::new());
    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// Removes its rule when dropped.
    pub struct Injected(u64);

    impl Drop for Injected {
        fn drop(&mut self) {
            RULES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(id, _, _)| *id != self.0);
        }
    }

    /// Calls `rule` with the operation and path of every state write under
    /// `scope` until the returned guard is dropped.
    pub fn inject(
        scope: &Path,
        rule: impl FnMut(&'static str, &Path) -> Option<Fault> + Send + 'static,
    ) -> Injected {
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        RULES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((id, scope.to_path_buf(), Box::new(rule)));
        Injected(id)
    }

    pub(crate) fn check(op: &'static str, path: &Path) -> Option<Fault> {
        let mut rules = RULES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rules
            .iter_mut()
            .filter(|(_, scope, _)| path.starts_with(scope))
            .find_map(|(_, _, rule)| rule(op, path))
    }
}

#[cfg(any(test, feature = "fault-injection"))]
pub use rules::{Injected, inject};

#[cfg(any(test, feature = "fault-injection"))]
use rules::check;

#[cfg(not(any(test, feature = "fault-injection")))]
#[inline]
fn check(_op: &'static str, _path: &Path) -> Option<Fault> {
    None
}

/// Writes all of `buf` to `w`, unless a fault is injected for `op` on `path`.
pub(crate) fn write_all(
    w: &mut impl std::io::Write,
    op: &'static str,
    path: &Path,
    buf: &[u8],
) -> std::io::Result<()> {
    if let Some(fault) = check(op, path) {
        w.write_all(&buf[..fault.after_bytes.min(buf.len())])?;
        return Err(std::io::Error::from_raw_os_error(
            fault.errno.raw_os_error(),
        ));
    }
    w.write_all(buf)
}
