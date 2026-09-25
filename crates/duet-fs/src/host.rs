// SPDX-License-Identifier: GPL-3.0-or-later
//! Retrying state writes in place while the host is short of a resource.
//!
//! A full disk is an infrastructure failure, not a decision: a write that
//! fails with one ([`FsError::is_host_resource`]) is retried after the caller's
//! [`HostWait`] has waited, until the wait gives up (the run's budget ended or
//! it was interrupted). Every operation retried this way must be idempotent;
//! the writes in [`crate::private`] and [`crate::atomic_write`] are, because a
//! failed attempt leaves nothing behind.

use crate::error::FsError;

/// Decides whether and when a write that failed for lack of a host resource
/// is tried again.
pub trait HostWait: Send + Sync {
    /// Waits before the next attempt of an operation that failed with `err`.
    /// Returns `false` to give up; the operation then fails with `err`.
    fn wait(&self, err: &FsError) -> bool;

    /// The operation succeeded after at least one wait.
    fn recovered(&self) {}
}

/// Runs `op`, retrying it in place while it fails for lack of a host resource
/// and `wait` agrees. Without a `wait`, or for any other error, the first
/// result is returned.
pub fn persist<T>(
    wait: Option<&dyn HostWait>,
    mut op: impl FnMut() -> Result<T, FsError>,
) -> Result<T, FsError> {
    let mut waited = false;
    loop {
        match op() {
            Err(e) if e.is_host_resource() && wait.is_some_and(|w| w.wait(&e)) => {
                waited = true;
            }
            result => {
                if waited
                    && result.is_ok()
                    && let Some(w) = wait
                {
                    w.recovered();
                }
                return result;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Count {
        waits: AtomicU32,
        recovered: AtomicU32,
        allow: u32,
    }
    impl HostWait for Count {
        fn wait(&self, _: &FsError) -> bool {
            self.waits.fetch_add(1, Ordering::SeqCst) < self.allow
        }
        fn recovered(&self) {
            self.recovered.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn full() -> FsError {
        FsError::io("append", "/x", rustix::io::Errno::NOSPC)
    }

    #[test]
    fn host_shortages_are_retried_until_the_wait_gives_up() {
        let w = Count {
            waits: AtomicU32::new(0),
            recovered: AtomicU32::new(0),
            allow: 10,
        };
        let mut failures = 3;
        let r = persist(Some(&w), || {
            if failures > 0 {
                failures -= 1;
                return Err(full());
            }
            Ok(7)
        });
        assert_eq!(r.unwrap(), 7);
        assert_eq!(w.waits.load(Ordering::SeqCst), 3);
        assert_eq!(w.recovered.load(Ordering::SeqCst), 1);

        let w = Count {
            waits: AtomicU32::new(0),
            recovered: AtomicU32::new(0),
            allow: 2,
        };
        let r: Result<(), _> = persist(Some(&w), || Err(full()));
        assert!(r.unwrap_err().is_host_resource());
        assert_eq!(w.waits.load(Ordering::SeqCst), 3);
        assert_eq!(w.recovered.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn other_errors_and_no_wait_fail_at_once() {
        let mut calls = 0;
        let r: Result<(), _> = persist(None, || {
            calls += 1;
            Err(full())
        });
        assert!(r.is_err());
        assert_eq!(calls, 1);
        let w = Count {
            waits: AtomicU32::new(0),
            recovered: AtomicU32::new(0),
            allow: 10,
        };
        let r: Result<(), _> = persist(Some(&w), || Err(FsError::Locked));
        assert!(matches!(r, Err(FsError::Locked)));
        assert_eq!(w.waits.load(Ordering::SeqCst), 0);
    }
}
