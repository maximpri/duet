// SPDX-License-Identifier: GPL-3.0-or-later
//! Resources for evaluation runs: a run must not take over the machine it runs on.
//!
//! Everything a lane starts (the agent, its commands and their children), and
//! then each grading command, is one process tree, held to memory limits by
//! the same governor that holds Declass's own commands
//! ([`declass_governor`]): a process above [`Limits::process_mb`] is
//! killed, the largest while the tree is above [`Limits::total_mb`], the
//! largest when the machine is critically short of memory, and every member
//! still alive when the step ends. Each kill is printed as it happens and
//! recorded in the run's record ([`Usage`]).
//!
//! Added here: [`Priority`], applied when the root is started and inherited by
//! every descendant (on macOS a QoS clamp, `taskpolicy -c`, elsewhere `nice`),
//! and exact CPU time per step through `time` ([`timed`]).

pub use declass_governor::{Kill, MemoryLimits, Usage};
use serde::{Deserialize, Serialize};

/// The scheduling priority of everything a run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// The operator's own priority.
    Normal,
    /// Below interactive work, which keeps the machine responsive; still uses
    /// every core when nothing else wants them.
    Utility,
    /// Efficiency cores and throttled disk on macOS (`nice 19` elsewhere):
    /// slowest, for a machine in active use.
    Background,
}

impl Priority {
    /// `argv` started at this priority. The prefix runs the program in place
    /// (same process id), and every process it starts inherits the priority.
    pub fn wrap(self, argv: Vec<String>) -> Vec<String> {
        let clamp = std::path::Path::new("/usr/sbin/taskpolicy").is_file();
        let prefix: &[&str] = match (self, clamp) {
            (Priority::Normal, _) => &[],
            (Priority::Utility, true) => &["/usr/sbin/taskpolicy", "-c", "utility"],
            (Priority::Background, true) => &["/usr/sbin/taskpolicy", "-c", "background"],
            (Priority::Utility, false) => &["nice", "-n", "10"],
            (Priority::Background, false) => &["nice", "-n", "19"],
        };
        prefix.iter().map(ToString::to_string).chain(argv).collect()
    }
}

/// `argv` run under `time`, which writes to `out` the CPU time of everything
/// it waited for: the program and every descendant reaped in its tree. The
/// program stays a child (the tree's root is `time`); its exit status is
/// passed through.
pub fn timed(argv: Vec<String>, out: &std::path::Path) -> Vec<String> {
    if !std::path::Path::new("/usr/bin/time").is_file() {
        return argv;
    }
    ["/usr/bin/time", "-p", "-o", &out.to_string_lossy()]
        .into_iter()
        .map(str::to_owned)
        .chain(argv)
        .collect()
}

/// The CPU seconds (`user` + `sys`) [`timed`] wrote to `out`, and removes it;
/// `None` when it wrote nothing (`time` itself was killed).
pub fn timed_cpu(out: &std::path::Path) -> Option<f64> {
    let text = std::fs::read_to_string(out).ok()?;
    let _ = std::fs::remove_file(out);
    let mut total = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("user ").or(line.strip_prefix("sys ")) {
            *total.get_or_insert(0.0) += v.trim().parse::<f64>().ok()?;
        }
    }
    total
}

/// What a run may use. Recorded in every run so reports can say what held.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Memory (the physical footprint) of any one process, in MiB.
    pub process_mb: u64,
    /// Memory of the whole tree, in MiB.
    pub total_mb: u64,
    pub priority: Priority,
}

impl Limits {
    /// This machine's memory limits ([`MemoryLimits::for_this_machine`]: an
    /// eighth of its memory per process and a quarter per run).
    pub fn for_this_machine(priority: Priority) -> Limits {
        let m = MemoryLimits::for_this_machine();
        Limits {
            process_mb: m.process_mb,
            total_mb: m.total_mb,
            priority,
        }
    }

    fn memory(&self) -> MemoryLimits {
        MemoryLimits {
            process_mb: self.process_mb,
            total_mb: self.total_mb,
        }
    }
}

/// Watches one run step's process tree until [`Governor::finish`], printing
/// each kill as it happens.
pub struct Governor(declass_governor::Governor);

impl Governor {
    /// Starts watching the tree under `root`; call right after spawning it.
    pub fn watch(root: u32, limits: Limits) -> Governor {
        Governor(declass_governor::Governor::watch_notify(
            root,
            limits.memory(),
            Box::new(|k: &Kill, l: &MemoryLimits| {
                eprintln!(
                    "resource governor: killed {} (pid {}, {} MB; {})",
                    k.name,
                    k.pid,
                    k.mb,
                    k.limit(l)
                );
            }),
        ))
    }

    /// Ends the watch: kills every member still alive and returns what the
    /// step used. Call after the root was waited for or its time limit passed.
    pub fn finish(self) -> Usage {
        self.0.finish()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use declass_governor::KillRule;
    use std::process::{Command, Stdio};

    #[test]
    fn priority_prefixes_run_the_program_in_place() {
        assert_eq!(Priority::Normal.wrap(vec!["x".into()]), vec!["x"]);
        let w = Priority::Utility.wrap(vec!["x".into(), "y".into()]);
        assert_eq!(&w[w.len() - 2..], ["x", "y"]);
        assert!(w.len() == 5, "{w:?}");
        // The clamped program keeps the spawned process id (the governor's root).
        let argv = Priority::Utility.wrap(vec!["/bin/sh".into(), "-c".into(), "echo $$".into()]);
        let child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let id = child.id();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), id.to_string());
    }

    #[test]
    fn timed_counts_the_cpu_of_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("time.txt");
        // About 0.3 s of CPU in a grandchild.
        let argv = timed(
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "/bin/sh -c 'i=0; while [ $i -lt 100000 ]; do i=$((i+1)); done'; exit 3".into(),
            ],
            &out,
        );
        let status = Command::new(&argv[0]).args(&argv[1..]).status().unwrap();
        assert_eq!(status.code(), Some(3), "the exit status passes through");
        let cpu = timed_cpu(&out).unwrap();
        assert!(cpu > 0.05, "{cpu}");
        assert!(!out.exists());
    }

    #[test]
    fn this_machine_has_limits() {
        let l = Limits::for_this_machine(Priority::Utility);
        assert!(
            l.process_mb >= 256 && l.total_mb >= 2 * l.process_mb,
            "{l:?}"
        );
    }

    #[test]
    fn older_records_still_read() {
        // Kills recorded before the footprint measure named their size `rss_mb`.
        let k: Kill = serde_json::from_str(
            r#"{"after_seconds":1.0,"pid":7,"name":"t","rss_mb":4100,"rule":"process_limit"}"#,
        )
        .unwrap();
        assert_eq!((k.mb, k.rule), (4100, KillRule::ProcessLimit));
    }
}
