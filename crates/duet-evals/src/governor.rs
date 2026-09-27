// SPDX-License-Identifier: GPL-3.0-or-later
//! The resource governor: a run must not take over the machine it runs on.
//!
//! Everything a lane starts (the agent, its commands and their children), and
//! then each grading command, is one process tree. The operating system cannot
//! bound a tree's memory on macOS: a memory limit given at spawn applies to that
//! process only (measured 2026-09-26: `taskpolicy -m 100` stopped a direct
//! 400 MB allocation and let the same allocation through one process down),
//! and the runaway that prompted this was a test binary three levels below the
//! lane (`duet` → `sh` → `cargo test` → the test, 5.8 GB and growing). So the
//! governor watches the tree:
//!
//! - every 2 s it reads the process table to follow the tree from its root,
//!   remembering every process it has seen (one whose parent died stays a
//!   member), and every 0.5 s it reads the members' resident memory; nothing
//!   is spawned;
//! - a member above [`Limits::process_mb`] is killed, and while the members
//!   together are above [`Limits::total_mb`] the largest is killed (the
//!   kernel's out-of-memory rule, scoped to the run). The agent sees a process
//!   killed by a signal, as on a machine out of memory;
//! - when the governed step ends (it finished, or its time limit passed), every
//!   member still alive is killed, so nothing a run started outlives it and no
//!   orphan keeps a grading command's output pipe open.
//!
//! [`Priority`] is applied when the root is started and inherited by every
//! descendant: on macOS a QoS clamp (`taskpolicy -c`), elsewhere `nice`.
//! Kills are recorded in the run's record ([`Usage`]).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often the members' memory is read (about 0.1 ms each time). A runaway
/// measured at ~300 MB/s overshoots a limit by about 150 MB at this interval.
const INTERVAL: Duration = Duration::from_millis(500);

/// Every how many intervals the whole process table is read to find new
/// children (about 12 ms of CPU for ~800 processes): with the member reads,
/// the governor costs about 0.6% of one core.
const DISCOVER_EVERY: u32 = 4;

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

/// What a run may use. Recorded in every run so reports can say what held.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Resident memory of any one process, in MiB.
    pub process_mb: u64,
    /// Resident memory of the whole tree, in MiB.
    pub total_mb: u64,
    pub priority: Priority,
}

impl Limits {
    /// An eighth of this machine's memory per process and a quarter per run
    /// (4 and 8 GiB on a 32 GiB machine; an X1 build peaks at 0.5 GB and its
    /// test suite at 0.2 GB).
    pub fn for_this_machine(priority: Priority) -> Limits {
        let mb = physical_memory_mb().unwrap_or(16 * 1024);
        Limits {
            process_mb: mb / 8,
            total_mb: mb / 4,
            priority,
        }
    }
}

/// What a governed step used, and what the governor did about it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Largest resident memory of the whole tree at one survey, in MiB.
    pub peak_total_mb: u64,
    /// Largest resident memory of one process at one survey, in MiB.
    pub peak_process_mb: u64,
    /// CPU seconds of the step's processes as surveyed: a lower bound (a
    /// process that starts and ends between two surveys is not seen).
    pub cpu_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kills: Vec<Kill>,
    /// Processes still running when the step ended; all were killed then.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stragglers_killed: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Usage {
    /// Folds another step's usage into this one (peaks as maxima).
    pub fn absorb(&mut self, other: Usage) {
        self.peak_total_mb = self.peak_total_mb.max(other.peak_total_mb);
        self.peak_process_mb = self.peak_process_mb.max(other.peak_process_mb);
        self.cpu_seconds += other.cpu_seconds;
        self.kills.extend(other.kills);
        self.stragglers_killed += other.stragglers_killed;
    }
}

/// One process the governor killed for its memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Kill {
    /// Seconds after the governed step started.
    pub after_seconds: f64,
    pub pid: u32,
    pub name: String,
    pub rss_mb: u64,
    pub rule: KillRule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillRule {
    /// The process alone was above [`Limits::process_mb`].
    ProcessLimit,
    /// The tree was above [`Limits::total_mb`] and this was its largest process.
    TotalLimit,
}

/// Watches one process tree until [`Governor::finish`].
pub struct Governor {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Usage>,
}

impl Governor {
    /// Starts watching the tree under `root`; call right after spawning it.
    pub fn watch(root: u32, limits: Limits) -> Governor {
        Governor::watch_every(root, limits, INTERVAL)
    }

    fn watch_every(root: u32, limits: Limits, interval: Duration) -> Governor {
        // The root's identity is taken now, before its id can be reused.
        let mut table = Table::new();
        table.refresh();
        let mut members = HashMap::new();
        if let Some(info) = table.info(root) {
            members.insert(
                root,
                Seen {
                    start: info.start,
                    cpu_ms: info.cpu_ms,
                },
            );
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("resource-governor".into())
            .spawn(move || watch(table, members, limits, interval, &flag))
            .expect("starting the resource governor");
        Governor { stop, thread }
    }

    /// Ends the watch: kills every member still alive and returns what the
    /// step used. Call after the root was waited for or its time limit passed.
    pub fn finish(self) -> Usage {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.thread().unpark();
        self.thread.join().unwrap_or_default()
    }
}

/// A member as last seen.
struct Seen {
    /// Start time: with the process id, its identity.
    start: u64,
    /// CPU time used so far, in milliseconds.
    cpu_ms: u64,
}

fn watch(
    mut table: Table,
    mut members: HashMap<u32, Seen>,
    limits: Limits,
    interval: Duration,
    stop: &AtomicBool,
) -> Usage {
    let started = Instant::now();
    let mut usage = Usage::default();
    let mut ended_cpu_ms = 0;
    let mut tick = 0u32;
    while !stop.load(Ordering::SeqCst) {
        // The first survey follows the tree from its root.
        let discover = tick.is_multiple_of(DISCOVER_EVERY);
        if discover {
            table.refresh();
        } else {
            table.refresh_only(&members.keys().copied().collect::<Vec<_>>());
        }
        let alive = survey(&table, &mut members, &mut ended_cpu_ms, discover);
        enforce(&alive, &limits, started, &mut usage);
        tick = tick.wrapping_add(1);
        std::thread::park_timeout(interval);
    }
    // Nothing the step started outlives it. A member may fork while it is
    // being killed, so survey again until none is left.
    let mut killed = HashSet::new();
    for _ in 0..10 {
        table.refresh();
        let alive = survey(&table, &mut members, &mut ended_cpu_ms, true);
        if alive.is_empty() {
            break;
        }
        for m in &alive {
            kill(m.pid);
            killed.insert(m.pid);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ended_cpu_ms += members.values().map(|m| m.cpu_ms).sum::<u64>();
    usage.stragglers_killed = killed.len();
    usage.cpu_seconds = ended_cpu_ms as f64 / 1000.0;
    usage
}

struct Member {
    pid: u32,
    rss_bytes: u64,
    name: String,
}

/// The members alive now: every remembered process that is still the same
/// process (its start time unchanged) and, with `discover` (after a full
/// refresh), their descendants, which join. A member that ended adds its last
/// CPU time to `ended_cpu_ms`.
fn survey(
    table: &Table,
    members: &mut HashMap<u32, Seen>,
    ended_cpu_ms: &mut u64,
    discover: bool,
) -> Vec<Member> {
    let children = if discover {
        table.children()
    } else {
        HashMap::new()
    };
    let mut queue: Vec<u32> = members.keys().copied().collect();
    let mut seen = HashSet::new();
    let mut alive = Vec::new();
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        let Some(info) = table.info(pid) else {
            *ended_cpu_ms += members.remove(&pid).map_or(0, |m| m.cpu_ms);
            continue;
        };
        match members.get_mut(&pid) {
            Some(m) if m.start == info.start => m.cpu_ms = m.cpu_ms.max(info.cpu_ms),
            // The id now belongs to another process: the member ended.
            Some(_) => {
                *ended_cpu_ms += members.remove(&pid).map_or(0, |m| m.cpu_ms);
                continue;
            }
            None => {
                members.insert(
                    pid,
                    Seen {
                        start: info.start,
                        cpu_ms: info.cpu_ms,
                    },
                );
            }
        }
        if let Some(kids) = children.get(&pid) {
            queue.extend(kids.iter().filter(|c| !seen.contains(c)));
        }
        alive.push(Member {
            pid,
            rss_bytes: info.rss_bytes,
            name: info.name,
        });
    }
    alive
}

fn enforce(alive: &[Member], limits: &Limits, started: Instant, usage: &mut Usage) {
    const MB: u64 = 1024 * 1024;
    let mut total: u64 = alive.iter().map(|m| m.rss_bytes).sum();
    usage.peak_total_mb = usage.peak_total_mb.max(total / MB);
    usage.peak_process_mb = usage
        .peak_process_mb
        .max(alive.iter().map(|m| m.rss_bytes / MB).max().unwrap_or(0));
    let mut record = |m: &Member, rule: KillRule| {
        kill(m.pid);
        let limit = match rule {
            KillRule::ProcessLimit => format!("{} MB per process", limits.process_mb),
            KillRule::TotalLimit => format!("{} MB per run", limits.total_mb),
        };
        eprintln!(
            "resource governor: killed {} (pid {}, {} MB; limit {limit})",
            m.name,
            m.pid,
            m.rss_bytes / MB
        );
        usage.kills.push(Kill {
            after_seconds: started.elapsed().as_secs_f64(),
            pid: m.pid,
            name: m.name.clone(),
            rss_mb: m.rss_bytes / MB,
            rule,
        });
    };
    let mut rest = Vec::new();
    for m in alive {
        if m.rss_bytes / MB > limits.process_mb {
            record(m, KillRule::ProcessLimit);
            total -= m.rss_bytes;
        } else {
            rest.push(m);
        }
    }
    while total / MB > limits.total_mb {
        let Some((i, _)) = rest.iter().enumerate().max_by_key(|(_, m)| m.rss_bytes) else {
            break;
        };
        let m = rest.swap_remove(i);
        record(m, KillRule::TotalLimit);
        total -= m.rss_bytes;
    }
}

fn kill(pid: u32) {
    if let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::Kill);
    }
}

fn physical_memory_mb() -> Option<u64> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    Some(sys.total_memory() / (1024 * 1024)).filter(|&mb| mb > 0)
}

struct Info {
    start: u64,
    rss_bytes: u64,
    cpu_ms: u64,
    name: String,
}

/// This machine's process table: parent, memory and CPU time of every process,
/// read in-process (a survey costs system calls, not a `ps` of ~25 ms CPU).
struct Table {
    sys: sysinfo::System,
}

impl Table {
    fn new() -> Table {
        Table {
            sys: sysinfo::System::new(),
        }
    }

    /// Every process: finds new children.
    fn refresh(&mut self) {
        self.sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            true,
            sysinfo::ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu(),
        );
    }

    /// Only `pids` (those that ended are dropped from the table).
    fn refresh_only(&mut self, pids: &[u32]) {
        let pids: Vec<sysinfo::Pid> = pids.iter().map(|&p| sysinfo::Pid::from_u32(p)).collect();
        self.sys.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&pids),
            true,
            sysinfo::ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu(),
        );
    }

    /// A running process (a zombie has ended).
    fn info(&self, pid: u32) -> Option<Info> {
        let p = self.sys.process(sysinfo::Pid::from_u32(pid))?;
        if p.status() == sysinfo::ProcessStatus::Zombie {
            return None;
        }
        Some(Info {
            start: p.start_time(),
            rss_bytes: p.memory(),
            cpu_ms: p.accumulated_cpu_time(),
            name: p.name().to_string_lossy().into_owned(),
        })
    }

    fn children(&self) -> HashMap<u32, Vec<u32>> {
        let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, p) in self.sys.processes() {
            if let Some(parent) = p.parent() {
                map.entry(parent.as_u32()).or_default().push(pid.as_u32());
            }
        }
        map
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Set in a child copy of this test binary: allocate this many MiB, touch
    /// every page, then wait to be killed.
    const HOG: &str = "DUET_EVAL_GOVERNOR_HOG_MB";

    #[test]
    fn hog_child() {
        let Some(mb) = std::env::var(HOG)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        else {
            return;
        };
        let mut v = vec![0u8; mb * 1024 * 1024];
        for i in (0..v.len()).step_by(4096) {
            v[i] = 1;
        }
        std::thread::sleep(Duration::from_secs(60));
        std::hint::black_box(v);
    }

    fn limits(process_mb: u64, total_mb: u64) -> Limits {
        Limits {
            process_mb,
            total_mb,
            priority: Priority::Normal,
        }
    }

    /// `sh -c <script>` where `$HOG` runs a copy of this binary that holds `mb` MiB.
    fn shell_with_hog(script: &str, mb: usize) -> std::process::Child {
        let exe = std::env::current_exe().unwrap();
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env(
                "HOG",
                format!(
                    "{} --exact governor::tests::hog_child --nocapture",
                    exe.display()
                ),
            )
            .env(HOG, mb.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn a_grandchild_above_the_process_limit_is_killed_and_recorded() {
        // The hog runs two levels down, as a test binary under `cargo test` does.
        // (`; true` keeps each shell from running its last command in place.)
        let mut root = shell_with_hog("/bin/sh -c \"$HOG; true\"; true", 300);
        let g = Governor::watch_every(root.id(), limits(150, 10_000), Duration::from_millis(50));
        let status = root.wait().unwrap();
        let usage = g.finish();
        assert!(status.success(), "the shell itself is not killed");
        assert_eq!(usage.kills.len(), 1, "{usage:?}");
        let k = &usage.kills[0];
        assert_eq!(k.rule, KillRule::ProcessLimit);
        assert!(k.rss_mb > 150, "{k:?}");
        assert!(usage.peak_process_mb > 150);
    }

    #[test]
    fn above_the_run_limit_the_largest_process_is_killed() {
        // Two hogs, 200 and 120 MiB, under a 250 MiB run limit: only the larger goes.
        let exe = std::env::current_exe().unwrap();
        let run = |mb: usize| {
            format!(
                "{HOG}={mb} {} --exact governor::tests::hog_child --nocapture",
                exe.display()
            )
        };
        let mut root = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("{} & {} & wait", run(200), run(120)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let g = Governor::watch_every(root.id(), limits(10_000, 250), Duration::from_millis(50));
        // Both allocate within a second; the governor surveys every 50 ms.
        std::thread::sleep(Duration::from_secs(3));
        let _ = root.kill();
        let _ = root.wait();
        let usage = g.finish();
        assert_eq!(usage.kills.len(), 1, "{usage:?}");
        assert_eq!(usage.kills[0].rule, KillRule::TotalLimit);
        assert!(usage.kills[0].rss_mb >= 190, "{usage:?}");
        assert!(
            usage.stragglers_killed >= 1,
            "the 120 MiB one is killed at the end"
        );
    }

    #[test]
    fn nothing_a_step_started_outlives_it() {
        // The root leaves a background process behind and exits.
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let mut root = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "sleep 60 & echo $! > {}; sleep 1",
                pidfile.display()
            ))
            .spawn()
            .unwrap();
        let g = Governor::watch_every(root.id(), limits(10_000, 10_000), Duration::from_millis(50));
        root.wait().unwrap();
        let orphan: u32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(running(orphan), "the orphan runs after its parent ended");
        let usage = g.finish();
        assert_eq!(usage.stragglers_killed, 1, "{usage:?}");
        assert!(wait_until(|| !running(orphan)), "the orphan was killed");
    }

    fn running(pid: u32) -> bool {
        let mut t = Table::new();
        t.refresh();
        t.info(pid).is_some()
    }

    #[test]
    fn a_reused_process_id_is_not_a_member() {
        let mut t = Table::new();
        t.refresh();
        let me = std::process::id();
        let start = t.info(me).unwrap().start;
        let mut members = HashMap::from([(
            me,
            Seen {
                start: start + 1,
                cpu_ms: 7,
            },
        )]);
        let mut ended = 0;
        assert!(survey(&t, &mut members, &mut ended, true).is_empty());
        assert!(members.is_empty());
        assert_eq!(ended, 7, "its CPU time counts as ended");
    }

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
    fn this_machine_has_limits() {
        let l = Limits::for_this_machine(Priority::Utility);
        assert!(
            l.process_mb >= 256 && l.total_mb >= 2 * l.process_mb,
            "{l:?}"
        );
    }
}
