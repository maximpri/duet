// SPDX-License-Identifier: GPL-3.0-or-later
//! How a run waits out a host that is short of a resource (a full disk).
//!
//! A full disk is an infrastructure failure, so it never ends a run as
//! `Failed`: a state write that fails with one (the transcript, the write
//! journal, the audit log, a tool's workspace write, the summary) pauses the
//! run, says so once on stderr, and is retried in place with backoff until
//! it succeeds, the run is interrupted, or the wall clock ends it
//! (`BudgetStopped{wall_clock}`). The writes are all or nothing (see
//! `declass_fs::private`), so no state is lost or half-written in between.
//!
//! The wait blocks the calling thread (writes are synchronous); the run's
//! interrupt flag is checked every 100 ms while it waits.

use declass_fs::FsError;
use declass_fs::host::HostWait;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// First wait between attempts; it doubles up to [`MAX_WAIT`].
const FIRST_WAIT: Duration = Duration::from_millis(250);
/// Longest wait between attempts.
const MAX_WAIT: Duration = Duration::from_secs(30);
/// How often a wait checks the interrupt flag.
const POLL: Duration = Duration::from_millis(100);
/// How long the records that end a run (its transcript end entry, summary and
/// audit end event) keep waiting for space after the run's own deadline.
pub const FINAL_GRACE: Duration = Duration::from_secs(30);

type Notify = Box<dyn Fn(&str) + Send + Sync>;

/// Waits for space until a deadline or an interrupt; see the module docs.
pub struct HostPolicy {
    deadline: Instant,
    interrupted: Option<Arc<AtomicBool>>,
    /// Waits in the current shortage (0 when none).
    waits: AtomicU32,
    /// Set when a wait gave up because of the interrupt flag.
    gave_up_interrupted: AtomicBool,
    notify: Mutex<Notify>,
}

impl HostPolicy {
    /// Retries until `deadline`, or until `interrupted` is set.
    pub fn until(deadline: Instant, interrupted: Option<Arc<AtomicBool>>) -> Self {
        Self {
            deadline,
            interrupted,
            waits: AtomicU32::new(0),
            gave_up_interrupted: AtomicBool::new(false),
            notify: Mutex::new(Box::new(|line| eprintln!("{line}"))),
        }
    }

    /// For the records that end a run: retries for [`FINAL_GRACE`] from now,
    /// whatever the interrupt flag says.
    pub fn finishing() -> Self {
        Self::until(Instant::now() + FINAL_GRACE, None)
    }

    /// Sends notices to `notify` instead of stderr.
    pub fn with_notices(self, notify: impl Fn(&str) + Send + Sync + 'static) -> Self {
        *self.lock_notify() = Box::new(notify);
        self
    }

    fn lock_notify(&self) -> std::sync::MutexGuard<'_, Notify> {
        self.notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn is_interrupted(&self) -> bool {
        self.interrupted
            .as_ref()
            .is_some_and(|f| f.load(Ordering::SeqCst))
    }

    /// Whether the last wait that gave up did so because of an interrupt
    /// (otherwise the deadline passed).
    pub fn gave_up_on_interrupt(&self) -> bool {
        self.gave_up_interrupted.load(Ordering::SeqCst)
    }
}

impl HostWait for HostPolicy {
    fn wait(&self, err: &FsError) -> bool {
        if self.is_interrupted() {
            self.gave_up_interrupted.store(true, Ordering::SeqCst);
            return false;
        }
        let now = Instant::now();
        if now >= self.deadline {
            return false;
        }
        let n = self.waits.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            let shortage = err.host_shortage().unwrap_or("host resource exhausted");
            let what = if shortage == "disk full" {
                "waiting for space"
            } else {
                "waiting"
            };
            (self.lock_notify())(&format!("{shortage}: {what} ({err})"));
        }
        let delay = FIRST_WAIT
            .saturating_mul(1u32 << n.min(8))
            .min(MAX_WAIT)
            .min(self.deadline - now);
        let wake = now + delay;
        loop {
            let now = Instant::now();
            if now >= wake {
                return true;
            }
            if self.is_interrupted() {
                self.gave_up_interrupted.store(true, Ordering::SeqCst);
                return false;
            }
            std::thread::sleep((wake - now).min(POLL));
        }
    }

    fn recovered(&self) {
        if self.waits.swap(0, Ordering::SeqCst) > 0 {
            (self.lock_notify())("space is available again; continuing");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use declass_fs::host::persist;

    fn full() -> FsError {
        FsError::io(
            "append",
            "/run/transcript.jsonl",
            std::io::Error::from_raw_os_error(28), // ENOSPC on Linux and macOS
        )
    }

    fn notices(p: HostPolicy) -> (HostPolicy, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        (
            p.with_notices(move |l| sink.lock().unwrap().push(l.to_owned())),
            seen,
        )
    }

    #[test]
    fn a_shortage_is_reported_once_and_retried_until_it_clears() {
        let (p, seen) = notices(HostPolicy::until(
            Instant::now() + Duration::from_secs(60),
            None,
        ));
        let mut failures = 3;
        let r = persist(Some(&p), || {
            if failures > 0 {
                failures -= 1;
                return Err(full());
            }
            Ok(())
        });
        assert!(r.is_ok());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(
            seen[0].starts_with("disk full: waiting for space"),
            "{seen:?}"
        );
        assert_eq!(seen[1], "space is available again; continuing");
    }

    #[test]
    fn it_gives_up_at_the_deadline_or_on_an_interrupt() {
        let started = Instant::now();
        let (p, _) = notices(HostPolicy::until(
            started + Duration::from_millis(600),
            None,
        ));
        let r: Result<(), _> = persist(Some(&p), || Err(full()));
        assert!(r.unwrap_err().is_host_resource());
        assert!(started.elapsed() >= Duration::from_millis(600));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!p.gave_up_on_interrupt());

        let flag = Arc::new(AtomicBool::new(false));
        let (p, _) = notices(HostPolicy::until(
            Instant::now() + Duration::from_secs(600),
            Some(flag.clone()),
        ));
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let r: Result<(), _> = persist(Some(&p), || Err(full()));
        setter.join().unwrap();
        assert!(r.is_err());
        assert!(p.gave_up_on_interrupt());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
