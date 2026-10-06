// SPDX-License-Identifier: GPL-3.0-or-later
//! The memory governor: nothing Declass starts may take over the machine it runs on.
//!
//! Every sandboxed command (and every long-running sandboxed server) is one
//! process tree, and so is each step of an evaluation run. The operating system
//! cannot bound a tree's memory on macOS: a memory limit given at spawn applies
//! to that process only (measured 2026-09-26: `taskpolicy -m 100` stopped a
//! direct 400 MB allocation and let the same allocation through one process
//! down), and the runaway that prompted this was a test binary three levels
//! below the command (`sh` → `cargo test` → the test, 5.8 GB and growing). So
//! the governor watches the tree:
//!
//! - every 2 s it reads the process table to follow the tree from its root,
//!   remembering every process it has seen (one whose parent died stays a
//!   member), and every 0.5 s it reads the members' memory;
//! - memory is the physical footprint: resident plus compressed on macOS
//!   (what the kernel's own memory limits count), resident plus swapped on
//!   Linux. Resident memory alone is not enough: on 2026-09-27 a runaway test
//!   binary reached a 62 GB footprint on a 32 GB Mac while its resident memory
//!   was 1.4 GB, then 9 MB (the rest compressed), so a resident limit never
//!   fired, and the kernel killed system services 120 times each instead of
//!   it (the binary ran in the foreground band) until the Mac restarted;
//! - a member above [`MemoryLimits::process_mb`] is killed, and while the
//!   members together are above [`MemoryLimits::total_mb`] the largest is
//!   killed (the kernel's out-of-memory rule, scoped to the tree). The command
//!   sees a process killed by a signal, as on a machine out of memory;
//! - when the kernel reports critical memory pressure (the machine as a
//!   whole), the tree's largest process is killed whatever its size, so
//!   nothing Declass started can be what takes the machine down. Only the tree's
//!   own processes are ever touched, whatever else is using the memory;
//! - when the watch ends (the root exited, or its time limit passed), every
//!   member still alive is killed, so nothing the tree started outlives it.
//!
//! Kills are returned in the tree's [`Usage`].

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often the members' memory is read (about 0.1 ms each time). A runaway
/// measured at ~300 MB/s overshoots a limit by about 150 MB at this interval.
const INTERVAL: Duration = Duration::from_millis(500);

/// Every how many intervals the whole process table is read to find new
/// children (about 12 ms of CPU for ~800 processes): with the member reads,
/// the governor costs about 0.6% of one core.
const DISCOVER_EVERY: u32 = 4;

/// What one process tree may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLimits {
    /// Memory (the physical footprint) of any one process, in MiB.
    pub process_mb: u64,
    /// Memory of the whole tree, in MiB.
    pub total_mb: u64,
}

impl MemoryLimits {
    /// An eighth of this machine's memory per process and a quarter per tree
    /// (4 and 8 GiB on a 32 GiB machine; an XL task's build peaks at 0.5 GB
    /// and its test suite at 0.2 GB).
    pub fn for_this_machine() -> MemoryLimits {
        static MACHINE: OnceLock<MemoryLimits> = OnceLock::new();
        *MACHINE.get_or_init(|| {
            let mb = physical_memory_mb().unwrap_or(16 * 1024);
            MemoryLimits {
                process_mb: mb / 8,
                total_mb: mb / 4,
            }
        })
    }

    /// These limits with `0` in either field meaning this machine's default.
    pub fn or_machine(self) -> MemoryLimits {
        let machine = MemoryLimits::for_this_machine();
        MemoryLimits {
            process_mb: if self.process_mb == 0 {
                machine.process_mb
            } else {
                self.process_mb
            },
            total_mb: if self.total_mb == 0 {
                machine.total_mb
            } else {
                self.total_mb
            },
        }
    }
}

static CONFIGURED: RwLock<Option<MemoryLimits>> = RwLock::new(None);

/// Sets the limits every sandboxed command and server of this process gets
/// (`0` in a field: this machine's default). The owner's configuration sets
/// them once, before the first command.
pub fn set_limits(limits: MemoryLimits) {
    if let Ok(mut l) = CONFIGURED.write() {
        *l = Some(limits.or_machine());
    }
}

/// The limits sandboxed commands and servers get: as set by [`set_limits`],
/// else this machine's defaults.
pub fn limits() -> MemoryLimits {
    CONFIGURED
        .read()
        .ok()
        .and_then(|l| *l)
        .unwrap_or_else(MemoryLimits::for_this_machine)
}

/// What a governed tree used, and what the governor did about it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Largest memory (footprint) of the whole tree at one survey, in MiB.
    pub peak_total_mb: u64,
    /// Largest memory of one process at one survey, in MiB.
    pub peak_process_mb: u64,
    /// CPU seconds (user and system) of the tree's processes from the surveys:
    /// a lower bound (short-lived processes between two surveys, such as
    /// compiler runs, are not seen).
    pub cpu_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kills: Vec<Kill>,
    /// Processes still running when the watch ended; all were killed then.
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
    /// Seconds after the watch started.
    pub after_seconds: f64,
    pub pid: u32,
    pub name: String,
    /// Its memory (footprint) when killed.
    #[serde(alias = "rss_mb")]
    pub mb: u64,
    pub rule: KillRule,
}

impl Kill {
    /// Which limit it passed, e.g. `4096 MB per process`.
    pub fn limit(&self, limits: &MemoryLimits) -> String {
        match self.rule {
            KillRule::ProcessLimit => format!("{} MB per process", limits.process_mb),
            KillRule::TotalLimit => format!("{} MB for the whole tree", limits.total_mb),
            KillRule::MachinePressure => "the machine is critically short of memory".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillRule {
    /// The process alone was above [`MemoryLimits::process_mb`].
    ProcessLimit,
    /// The tree was above [`MemoryLimits::total_mb`] and this was its largest process.
    TotalLimit,
    /// The kernel reported critical memory pressure and this was the tree's
    /// largest process.
    MachinePressure,
}

/// Told of each kill as it happens (the evaluation harness prints them).
pub type Notify = Box<dyn Fn(&Kill, &MemoryLimits) + Send>;

/// Watches one process tree until [`Governor::finish`].
pub struct Governor {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Usage>,
}

impl Governor {
    /// Starts watching the tree under `root`; call right after spawning it,
    /// before it can have been waited for (its identity is taken now).
    pub fn watch(root: u32, limits: MemoryLimits) -> Governor {
        Governor::start(root, limits, INTERVAL, None)
    }

    /// [`Governor::watch`], calling `notify` for each kill as it happens.
    pub fn watch_notify(root: u32, limits: MemoryLimits, notify: Notify) -> Governor {
        Governor::start(root, limits, INTERVAL, Some(notify))
    }

    fn start(
        root: u32,
        limits: MemoryLimits,
        interval: Duration,
        notify: Option<Notify>,
    ) -> Governor {
        // The root's identity is taken now, before its id can be reused; the
        // thread's first survey reads the whole table.
        let mut table = Table::new();
        table.refresh_only(&[root]);
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
            .name("memory-governor".into())
            .spawn(move || watch(table, members, limits, interval, &flag, notify))
            .expect("starting the memory governor");
        Governor { stop, thread }
    }

    /// Ends the watch: kills every member still alive and returns what the
    /// tree used. Call after the root was waited for or its time limit passed.
    /// Blocks for a survey or two (tens of milliseconds).
    pub fn finish(self) -> Usage {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.thread().unpark();
        self.thread.join().unwrap_or_default()
    }

    /// Ends the watch without waiting for it: the watcher still kills every
    /// member left alive, on its own thread.
    pub fn abandon(self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.thread().unpark();
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
    limits: MemoryLimits,
    interval: Duration,
    stop: &AtomicBool,
    notify: Option<Notify>,
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
        // The machine's pressure is read with the full table (every 2 s).
        let critical = discover && machine_critical();
        for k in enforce(&alive, &limits, critical, started, &mut usage) {
            if let Some(n) = &notify {
                n(&k, &limits);
            }
        }
        tick = tick.wrapping_add(1);
        std::thread::park_timeout(interval);
    }
    // Nothing the tree started outlives it. A member may fork while it is
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
    /// The physical footprint (never less than the resident size).
    bytes: u64,
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
            bytes: info.rss_bytes.max(footprint(pid).unwrap_or(0)),
            name: info.name,
        });
    }
    alive
}

/// Applies the limits to the members alive now; returns the kills it made
/// (also recorded in `usage`).
fn enforce(
    alive: &[Member],
    limits: &MemoryLimits,
    critical: bool,
    started: Instant,
    usage: &mut Usage,
) -> Vec<Kill> {
    const MB: u64 = 1024 * 1024;
    let mut total: u64 = alive.iter().map(|m| m.bytes).sum();
    usage.peak_total_mb = usage.peak_total_mb.max(total / MB);
    usage.peak_process_mb = usage
        .peak_process_mb
        .max(alive.iter().map(|m| m.bytes / MB).max().unwrap_or(0));
    let mut made = Vec::new();
    let mut record = |m: &Member, rule: KillRule| {
        kill(m.pid);
        made.push(Kill {
            after_seconds: started.elapsed().as_secs_f64(),
            pid: m.pid,
            name: m.name.clone(),
            mb: m.bytes / MB,
            rule,
        });
    };
    let mut rest = Vec::new();
    for m in alive {
        if m.bytes / MB > limits.process_mb {
            record(m, KillRule::ProcessLimit);
            total -= m.bytes;
        } else {
            rest.push(m);
        }
    }
    while total / MB > limits.total_mb {
        let Some((i, _)) = rest.iter().enumerate().max_by_key(|(_, m)| m.bytes) else {
            break;
        };
        let m = rest.swap_remove(i);
        record(m, KillRule::TotalLimit);
        total -= m.bytes;
    }
    if critical && let Some(m) = pressure_victim(&rest) {
        record(m, KillRule::MachinePressure);
    }
    usage.kills.extend(made.iter().cloned());
    made
}

/// The member killed when the machine is critically short of memory: the
/// largest, when it holds enough to matter (a shell or an agent of a few tens
/// of MB is left alone).
fn pressure_victim<'a>(alive: &[&'a Member]) -> Option<&'a Member> {
    const MATTERS: u64 = 256 * 1024 * 1024;
    alive
        .iter()
        .copied()
        .filter(|m| m.bytes >= MATTERS)
        .max_by_key(|m| m.bytes)
}

/// A process's physical footprint in bytes: resident and compressed memory on
/// macOS (`proc_pid_rusage`), resident and swapped on Linux.
fn footprint(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        use libproc::libproc::pid_rusage::{RUsageInfoV2, pidrusage};
        pidrusage::<RUsageInfoV2>(i32::try_from(pid).ok()?)
            .ok()
            .map(|r| r.ri_phys_footprint)
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let kb = |key: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(key))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0)
        };
        Some((kb("VmRSS:") + kb("VmSwap:")) * 1024)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// Whether the kernel reports critical memory pressure for the whole machine:
/// on macOS its pressure level (`kern.memorystatus_vm_pressure_level`, 4 is
/// critical); on Linux less than a twentieth of memory available.
fn machine_critical() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "kern.memorystatus_vm_pressure_level"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|t| t.trim().parse::<u32>().ok())
            .is_some_and(|level| level >= 4)
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(info) = std::fs::read_to_string("/proc/meminfo") else {
            return false;
        };
        let kb = |key: &str| {
            info.lines()
                .find_map(|l| l.strip_prefix(key))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|n| n.parse::<u64>().ok())
        };
        matches!((kb("MemAvailable:"), kb("MemTotal:")), (Some(a), Some(t)) if a < t / 20)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
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
    pub(crate) const HOG: &str = "DECLASS_GOVERNOR_HOG_MB";

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

    fn limits(process_mb: u64, total_mb: u64) -> MemoryLimits {
        MemoryLimits {
            process_mb,
            total_mb,
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
                format!("{} --exact tests::hog_child --nocapture", exe.display()),
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

    fn quick(root: u32, limits: MemoryLimits, notify: Option<Notify>) -> Governor {
        Governor::start(root, limits, Duration::from_millis(50), notify)
    }

    #[test]
    fn a_grandchild_above_the_process_limit_is_killed_and_recorded() {
        // The hog runs two levels down, as a test binary under `cargo test` does.
        // (`; true` keeps each shell from running its last command in place.)
        let mut root = shell_with_hog("/bin/sh -c \"$HOG; true\"; true", 300);
        let told = Arc::new(std::sync::Mutex::new(Vec::new()));
        let t = told.clone();
        let g = quick(
            root.id(),
            limits(150, 10_000),
            Some(Box::new(move |k: &Kill, l: &MemoryLimits| {
                t.lock().unwrap().push(k.limit(l));
            })),
        );
        let status = root.wait().unwrap();
        let usage = g.finish();
        assert!(status.success(), "the shell itself is not killed");
        assert_eq!(usage.kills.len(), 1, "{usage:?}");
        let k = &usage.kills[0];
        assert_eq!(k.rule, KillRule::ProcessLimit);
        assert!(k.mb > 150, "{k:?}");
        assert!(usage.peak_process_mb > 150);
        assert_eq!(*told.lock().unwrap(), ["150 MB per process"]);
    }

    #[test]
    fn above_the_tree_limit_the_largest_process_is_killed() {
        // Two hogs, 200 and 120 MiB, under a 250 MiB tree limit: only the larger goes.
        let exe = std::env::current_exe().unwrap();
        let run = |mb: usize| {
            format!(
                "{HOG}={mb} {} --exact tests::hog_child --nocapture",
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
        let g = quick(root.id(), limits(10_000, 250), None);
        // Both allocate within a second; the governor surveys every 50 ms.
        std::thread::sleep(Duration::from_secs(3));
        let _ = root.kill();
        let _ = root.wait();
        let usage = g.finish();
        assert_eq!(usage.kills.len(), 1, "{usage:?}");
        assert_eq!(usage.kills[0].rule, KillRule::TotalLimit);
        assert!(usage.kills[0].mb >= 190, "{usage:?}");
        assert!(
            usage.stragglers_killed >= 1,
            "the 120 MiB one is killed at the end"
        );
    }

    #[test]
    fn nothing_a_tree_started_outlives_it() {
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
        let g = quick(root.id(), limits(10_000, 10_000), None);
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
    fn the_footprint_counts_what_is_resident_at_least() {
        // This process has touched memory: a footprint is read, and a member's
        // memory is never below its resident size.
        let mut t = Table::new();
        t.refresh();
        let me = std::process::id();
        let rss = t.info(me).unwrap().rss_bytes;
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let fp = footprint(me).expect("a footprint on this platform");
            assert!(fp > 0);
        }
        let mut members = HashMap::from([(
            me,
            Seen {
                start: t.info(me).unwrap().start,
                cpu_ms: 0,
            },
        )]);
        let alive = survey(&t, &mut members, &mut 0, false);
        assert!(alive[0].bytes >= rss);
    }

    #[test]
    fn under_critical_pressure_the_largest_process_that_matters_is_killed() {
        let m = |pid, mb: u64| Member {
            pid,
            bytes: mb * 1024 * 1024,
            name: format!("p{pid}"),
        };
        let (shell, build, test) = (m(1, 40), m(2, 600), m(3, 3000));
        assert_eq!(
            pressure_victim(&[&shell, &build, &test]).map(|v| v.pid),
            Some(3)
        );
        assert!(
            pressure_victim(&[&shell]).is_none(),
            "a small shell is left alone"
        );
    }

    #[test]
    fn this_machine_has_limits_and_zero_means_its_default() {
        let l = MemoryLimits::for_this_machine();
        assert!(
            l.process_mb >= 256 && l.total_mb >= 2 * l.process_mb,
            "{l:?}"
        );
        let set = MemoryLimits {
            process_mb: 0,
            total_mb: 5000,
        }
        .or_machine();
        assert_eq!(set.process_mb, l.process_mb);
        assert_eq!(set.total_mb, 5000);
    }
}
