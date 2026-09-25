// SPDX-License-Identifier: GPL-3.0-or-later
//! Command sandbox (Seatbelt on macOS, bubblewrap on Linux), environment
//! allowlist and process-tree control.
//!
//! Commands may read the filesystem (except paths the caller denies), write only inside the workspace and the
//! run's scratch directory, never write `.git` or `.duet` at any depth, and have
//! no network unless the run allows it. Resolution fails closed: without a
//! sandbox binary at its fixed absolute path, no command runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
pub const BWRAP: &str = "/usr/bin/bwrap";

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("no sandbox is available ({0}); commands are refused")]
    Unavailable(String),
    #[error("invalid sandbox path {0}")]
    Path(String),
    #[error("cannot start command: {0}")]
    Spawn(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxKind {
    Seatbelt,
    Bubblewrap,
}

/// Detects the platform sandbox at its fixed absolute path. On Linux the
/// sandbox must also work: bubblewrap is started once to check that it can
/// create its namespaces, so a kernel or container that forbids them is
/// reported here instead of on the first command.
pub fn detect() -> Result<SandboxKind, SandboxError> {
    if cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).is_file() {
        Ok(SandboxKind::Seatbelt)
    } else if cfg!(target_os = "linux") && Path::new(BWRAP).is_file() {
        probe_bwrap(Path::new(BWRAP)).map(|()| SandboxKind::Bubblewrap)
    } else {
        Err(SandboxError::Unavailable(format!(
            "neither {SANDBOX_EXEC} nor {BWRAP} found"
        )))
    }
}

/// Starts `bwrap` with the namespaces and mounts every command uses and
/// fails unless the sandboxed `/bin/true` ran.
fn probe_bwrap(bwrap: &Path) -> Result<(), SandboxError> {
    let out = std::process::Command::new(bwrap)
        .args([
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--ro-bind",
            "/",
            "/",
            "--tmpfs",
            "/tmp",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--",
            "/bin/true",
        ])
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .map_err(|e| SandboxError::Unavailable(format!("{}: {e}", bwrap.display())))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(SandboxError::Unavailable(format!(
            "{} cannot create its sandbox: {}",
            bwrap.display(),
            first_line(&out.stderr)
        )))
    }
}

fn first_line(b: &[u8]) -> String {
    let text = String::from_utf8_lossy(b);
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        "no error message".to_owned()
    } else {
        line.to_owned()
    }
}

/// Environment variables passed through to commands; everything else is cleared.
pub const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TERM",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "GOPATH",
    "GOROOT",
    "JAVA_HOME",
    "NVM_DIR",
    "PYENV_ROOT",
    "SDKROOT",
    "DEVELOPER_DIR",
];

/// macOS services toolchains need. Everything else (keychain, pasteboard,
/// arbitrary launchd agents) is unreachable.
const MACH_SERVICES: &[&str] = &[
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.SystemConfiguration.configd",
    "com.apple.coreservices.launchservicesd",
    "com.apple.CoreServices.coreservicesd",
    "com.apple.lsd.mapdb",
    "com.apple.FSEvents",
    "com.apple.diagnosticd",
    "com.apple.analyticsd",
    "com.apple.trustd",
    "com.apple.trustd.agent",
    "com.apple.logd",
];

#[derive(Debug, Clone)]
pub struct Spec {
    /// Canonical workspace root.
    pub workspace: PathBuf,
    /// Scratch directory for `TMPDIR`; must be outside `.git`.
    pub scratch: PathBuf,
    pub network: bool,
    pub timeout: Duration,
    /// Bytes of stdout/stderr kept in memory each; the full output is spilled.
    pub output_cap: usize,
    /// Where the full combined output goes when it exceeds the cap.
    pub spill_file: Option<PathBuf>,
    pub extra_env: Vec<(String, String)>,
    /// Absolute paths the command may not read (directories: everything under them).
    /// Used to keep sensitive files out of commands whose output the frontier sees.
    pub deny_read: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// Stopped by the caller (the run was interrupted) before it exited.
    pub interrupted: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_total: usize,
    pub stderr_total: usize,
    /// Set when output exceeded the cap and was written in full here.
    pub spilled_to: Option<PathBuf>,
    /// `.git` and `.duet` entries the command created and the sandbox removed
    /// afterwards (bubblewrap only; Seatbelt refuses to create them).
    pub removed_reserved: Vec<PathBuf>,
    pub duration: Duration,
}

impl Output {
    /// Whether the output shows the sandbox refusing an operation. Seatbelt
    /// refuses with `EPERM` ("Operation not permitted"); under bubblewrap a
    /// denied path is replaced by an empty file or directory that nobody may
    /// open, so the refusal is `EACCES` ("Permission denied").
    pub fn shows_denial(&self) -> bool {
        let needle = DENIAL_MESSAGE.as_bytes();
        [&self.stdout, &self.stderr]
            .iter()
            .any(|b| b.windows(needle.len()).any(|w| w == needle))
    }
}

/// How programs report a read the sandbox refused on this platform.
pub const DENIAL_MESSAGE: &str = if cfg!(target_os = "linux") {
    "Permission denied"
} else {
    "Operation not permitted"
};

fn quote(p: &Path) -> Result<String, SandboxError> {
    let s = p
        .to_str()
        .ok_or_else(|| SandboxError::Path(p.display().to_string()))?;
    if s.contains('"') || s.contains('\\') {
        return Err(SandboxError::Path(s.to_owned()));
    }
    Ok(format!("\"{s}\""))
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if ".^$|()[]{}*+?\\".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Seatbelt profile for a workspace-write command.
pub fn seatbelt_profile(spec: &Spec) -> Result<String, SandboxError> {
    let ws = quote(&spec.workspace)?;
    let scratch = quote(&spec.scratch)?;
    let root = spec
        .workspace
        .to_str()
        .ok_or_else(|| SandboxError::Path(spec.workspace.display().to_string()))?;
    let reserved = format!("^{}(/.*)?/\\.(git|duet)(/.*)?$", regex_escape(root));
    let mut rules = vec![
        "(version 1)".to_owned(),
        "(deny default)".to_owned(),
        "(allow process-fork)".to_owned(),
        "(allow process-exec)".to_owned(),
        "(allow signal (target same-sandbox))".to_owned(),
        "(allow file-read*)".to_owned(),
        "(allow sysctl-read)".to_owned(),
        "(allow ipc-posix-shm-read-data)".to_owned(),
        "(allow ipc-posix-shm-read-metadata)".to_owned(),
        "(allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\") (literal \"/dev/dtracehelper\"))".to_owned(),
        format!("(allow file-write* (subpath {ws}))"),
        format!("(allow file-write* (subpath {scratch}))"),
        format!("(deny file-write* (regex #\"{reserved}\"))"),
        // Apple's toolchain helper caches here regardless of TMPDIR; allow only its cache files.
        "(allow file-write* (regex #\"^/private/var/folders/[^/]+/[^/]+/T/xcrun_db\"))".to_owned(),
    ];
    // Later rules win: these override the blanket read allowance above.
    for p in &spec.deny_read {
        // A denied symlink is denied at its target too, so the content is not
        // readable under the target's own path.
        let target = p.canonicalize().ok().filter(|t| t != p);
        for p in std::iter::once(p).chain(target.as_ref()) {
            let q = quote(p)?;
            rules.push(if p.is_dir() {
                format!("(deny file-read* (subpath {q}))")
            } else {
                format!("(deny file-read* (literal {q}))")
            });
        }
    }
    let services: Vec<String> = MACH_SERVICES
        .iter()
        .map(|s| format!("(global-name \"{s}\")"))
        .collect();
    rules.push(format!("(allow mach-lookup {})", services.join(" ")));
    if spec.network {
        rules.push("(allow network*)".to_owned());
    }
    Ok(rules.join("\n"))
}

/// A `.git` or `.duet` entry in the workspace, identified by its inode so a
/// replacement at the same path is told apart from the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reserved {
    pub path: PathBuf,
    id: (u64, u64),
}

/// Every `.git` and `.duet` entry (directory, file or symlink) under the
/// workspace, at any depth. Symlinks are not followed and reserved
/// directories are not entered.
pub fn reserved_entries(workspace: &Path) -> Vec<Reserved> {
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let name = entry.file_name();
            if name == ".git" || name == ".duet" {
                out.push(Reserved {
                    path: entry.path(),
                    id: (meta.dev(), meta.ino()),
                });
            } else if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    out.sort_by(|x, y| x.path.cmp(&y.path));
    out
}

/// Stand-ins mounted over denied paths: an empty file and an empty
/// directory, both mode 000 and mounted read-only, so opening, listing or
/// copying a denied path fails with `EACCES` and the mode cannot be changed.
/// They live in a private directory outside anything a command may write.
#[derive(Debug, Clone)]
pub struct DenyStubs {
    pub file: PathBuf,
    pub dir: PathBuf,
}

impl DenyStubs {
    /// Creates (or checks) the stand-ins under `parent`.
    pub fn ensure(parent: &Path) -> Result<Self, SandboxError> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let err = |e: std::io::Error| SandboxError::Spawn(format!("deny stubs: {e}"));
        let uid = rustix::process::getuid().as_raw();
        let root = parent.join(format!("duet-sandbox-{uid}"));
        match std::fs::create_dir(&root) {
            Ok(()) => std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(err)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(err(e)),
        }
        // Someone else's directory could hold readable stand-ins: refuse it.
        let meta = std::fs::symlink_metadata(&root).map_err(err)?;
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(SandboxError::Unavailable(format!(
                "{} is not a private directory owned by this user",
                root.display()
            )));
        }
        let stubs = Self {
            file: root.join("denied-file"),
            dir: root.join("denied-dir"),
        };
        // Concurrent runs share the stand-ins: create each at most once.
        let created = |r: std::io::Result<()>| match r {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => Err(err(e)),
            _ => Ok(()),
        };
        created(std::fs::create_dir(&stubs.dir))?;
        created(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&stubs.file)
                .map(drop),
        )?;
        for p in [&stubs.file, &stubs.dir] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o000)).map_err(err)?;
        }
        Ok(stubs)
    }
}

fn push_bind(a: &mut Vec<OsString>, op: &str, src: &Path, dest: &Path) {
    a.extend([
        OsString::from(op),
        src.as_os_str().to_owned(),
        dest.as_os_str().to_owned(),
    ]);
}

/// bubblewrap options for a workspace-write command (the caller adds
/// `--chdir`, `--` and the command).
///
/// The root is mounted read-only, with empty read-only `/tmp` and `/run`; the
/// workspace and the scratch directory are mounted writable; every existing `.git` and `.duet` (`reserved`, at any
/// depth) is mounted back read-only; every existing path in
/// `spec.deny_read` is covered by a `stubs` stand-in. Paths are resolved
/// first, so a symlink is covered at its target. A denied path that does not
/// exist yet is skipped: there is nothing to read, and a mount point would
/// create it in the workspace. `.git` and `.duet` entries a command creates
/// cannot be prevented by mounts; [`run_until`] removes them afterwards.
pub fn bwrap_args(spec: &Spec, reserved: &[Reserved], stubs: &DenyStubs) -> Vec<OsString> {
    let mut a: Vec<OsString> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        // Run by root, bubblewrap would keep every capability (enough to
        // unmount the stand-ins); nobody's commands get any.
        "--cap-drop",
        "ALL",
        "--ro-bind",
        "/",
        "/",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/run",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    if spec.network {
        // Name resolution on systemd hosts goes through a stub under /run.
        push_bind(
            &mut a,
            "--ro-bind-try",
            Path::new("/run/systemd/resolve"),
            Path::new("/run/systemd/resolve"),
        );
    }
    push_bind(&mut a, "--bind", &spec.workspace, &spec.workspace);
    for r in reserved {
        if let Ok(target) = r.path.canonicalize() {
            push_bind(&mut a, "--ro-bind", &target, &target);
        }
    }
    for p in &spec.deny_read {
        let Ok(target) = p.canonicalize() else {
            continue;
        };
        let stub = if target.is_dir() {
            &stubs.dir
        } else {
            &stubs.file
        };
        push_bind(&mut a, "--ro-bind", stub, &target);
    }
    push_bind(&mut a, "--bind", &spec.scratch, &spec.scratch);
    // The empty /tmp and /run only hold mount points: nothing else is writable.
    a.extend(["--remount-ro", "/tmp", "--remount-ro", "/run"].map(OsString::from));
    if spec.network {
        a.push(OsString::from("--share-net"));
    }
    a
}

/// The seccomp filter bubblewrap installs when the network is off.
///
/// A new network namespace cuts off IP, but Unix sockets are files: one
/// under `$HOME` (a container engine's, an agent's) stays reachable through
/// the read-only root. Seatbelt counts connecting to one as network access;
/// here the filter refuses to create `AF_UNIX` sockets, and `io_uring`
/// (which can create sockets without the `socket` call), with `EACCES`.
/// Socket pairs (`socketpair`) stay available. System calls from another
/// architecture's ABI are refused outright.
mod seccomp {
    use super::SandboxError;

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const ARCH: Option<(u32, u32)> = Some((0xC000_003E, 41));
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const ARCH: Option<(u32, u32)> = Some((0xC000_00B7, 198));
    #[cfg(not(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )))]
    const ARCH: Option<(u32, u32)> = None;

    const IO_URING_SETUP: u32 = 425;
    const AF_UNIX: u32 = 1;
    const X32_BIT: u32 = 0x4000_0000;
    const RET_ALLOW: u32 = 0x7fff_0000;
    const RET_EACCES: u32 = 0x0005_0000 | 13;

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn op(code: u16, jt: u8, jf: u8, k: u32) -> [u8; 8] {
        let mut b = [0; 8];
        b[..2].copy_from_slice(&code.to_ne_bytes());
        b[2] = jt;
        b[3] = jf;
        b[4..].copy_from_slice(&k.to_ne_bytes());
        b
    }

    /// The classic BPF program, or `None` where no filter is defined.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn program() -> Option<Vec<u8>> {
        const LD_ABS: u16 = 0x20;
        const JEQ: u16 = 0x15;
        const JGE: u16 = 0x35;
        const RET: u16 = 0x06;
        let (arch, socket) = ARCH?;
        // `seccomp_data`: nr at 0, arch at 4, args[0] (low half) at 16.
        let ops = [
            op(LD_ABS, 0, 0, 4),
            op(JEQ, 1, 0, arch),
            op(RET, 0, 0, RET_EACCES),
            op(LD_ABS, 0, 0, 0),
            op(JGE, 0, 1, X32_BIT),
            op(RET, 0, 0, RET_EACCES),
            op(JEQ, 0, 1, IO_URING_SETUP),
            op(RET, 0, 0, RET_EACCES),
            op(JEQ, 0, 3, socket),
            op(LD_ABS, 0, 0, 16),
            op(JEQ, 0, 1, AF_UNIX),
            op(RET, 0, 0, RET_EACCES),
            op(RET, 0, 0, RET_ALLOW),
        ];
        Some(ops.concat())
    }

    /// The program in an anonymous file, positioned at its start.
    #[cfg(target_os = "linux")]
    pub fn filter() -> Result<std::fs::File, SandboxError> {
        use std::io::{Seek, Write};
        let err = |e: std::io::Error| SandboxError::Spawn(format!("seccomp filter: {e}"));
        let program = program().ok_or_else(|| {
            SandboxError::Unavailable(
                "no seccomp filter for this architecture, so the network cannot be \
                 turned off"
                    .into(),
            )
        })?;
        let fd = rustix::fs::memfd_create("duet-seccomp", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(|e| err(e.into()))?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&program).map_err(err)?;
        file.rewind().map_err(err)?;
        Ok(file)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn filter() -> Result<std::fs::File, SandboxError> {
        Err(SandboxError::Unavailable(
            "bubblewrap runs only on Linux".into(),
        ))
    }
}

/// Removes the `.git` and `.duet` entries a command created (those not in
/// `before`), returning their paths.
fn remove_new_reserved(workspace: &Path, before: &[Reserved]) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    for r in reserved_entries(workspace) {
        if before.iter().any(|b| b.id == r.id) {
            continue;
        }
        let gone = match std::fs::symlink_metadata(&r.path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&r.path).or_else(|_| {
                // The command may have made parts of it unwritable.
                make_owner_writable(&r.path);
                std::fs::remove_dir_all(&r.path)
            }),
            Ok(_) => std::fs::remove_file(&r.path),
            Err(e) => Err(e),
        };
        removed.push(match gone {
            Ok(()) => r.path,
            Err(e) => PathBuf::from(format!("{} (could not remove: {e})", r.path.display())),
        });
    }
    removed
}

fn make_owner_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(entry.path());
            }
        }
    }
}

/// Runs `argv` in `cwd` (inside the workspace) under the sandbox.
pub async fn run(
    kind: SandboxKind,
    spec: &Spec,
    argv: &[String],
    cwd: &Path,
) -> Result<Output, SandboxError> {
    run_until(kind, spec, argv, cwd, std::future::pending()).await
}

/// [`run`], stopped early when `stop` resolves: the command's whole process
/// tree is killed at once and the output so far is returned with
/// `interrupted` set.
pub async fn run_until(
    kind: SandboxKind,
    spec: &Spec,
    argv: &[String],
    cwd: &Path,
    stop: impl std::future::Future<Output = ()>,
) -> Result<Output, SandboxError> {
    run_with(kind, Path::new(BWRAP), spec, argv, cwd, stop).await
}

static MARKERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn run_with(
    kind: SandboxKind,
    bwrap: &Path,
    spec: &Spec,
    argv: &[String],
    cwd: &Path,
    stop: impl std::future::Future<Output = ()>,
) -> Result<Output, SandboxError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| SandboxError::Spawn("empty command".into()))?;
    std::fs::create_dir_all(&spec.scratch).map_err(|e| SandboxError::Spawn(e.to_string()))?;
    // Profiles match resolved paths (`/var` is `/private/var` on macOS).
    let resolved = Spec {
        scratch: spec
            .scratch
            .canonicalize()
            .map_err(|e| SandboxError::Spawn(e.to_string()))?,
        ..spec.clone()
    };
    let spec = &resolved;
    let mut reserved = Vec::new();
    let mut marker = None;
    let mut stdin = Stdio::null();
    let mut cmd = match kind {
        SandboxKind::Seatbelt => {
            let mut c = tokio::process::Command::new(SANDBOX_EXEC);
            c.arg("-p")
                .arg(seatbelt_profile(spec)?)
                .arg(program)
                .args(args);
            c
        }
        SandboxKind::Bubblewrap => {
            reserved = reserved_entries(&spec.workspace);
            let stubs = DenyStubs::ensure(&std::env::temp_dir())?;
            let m = spec.scratch.join(format!(
                ".duet-sandbox-started-{}-{}",
                std::process::id(),
                MARKERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let mut c = tokio::process::Command::new(bwrap);
            c.args(bwrap_args(spec, &reserved, &stubs));
            if !spec.network {
                // bubblewrap reads the filter from its standard input.
                let filter = seccomp::filter()?;
                c.args(["--seccomp", "0"]);
                stdin = Stdio::from(filter);
            }
            c.arg("--chdir")
                .arg(cwd)
                .arg("--")
                // Proves the sandbox was set up: bubblewrap only reaches this
                // shell once every namespace and mount is in place. The
                // command's standard input is empty, as under Seatbelt.
                .args([
                    "/bin/sh",
                    "-c",
                    "exec </dev/null; : > \"$0\" && exec \"$@\"",
                ])
                .arg(&m)
                .arg(program)
                .args(args);
            marker = Some(m);
            c
        }
    };
    cmd.current_dir(cwd).env_clear();
    for key in ENV_ALLOWLIST {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("TMPDIR", &spec.scratch)
        .envs(spec.extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| match kind {
        SandboxKind::Bubblewrap => {
            SandboxError::Unavailable(format!("cannot start {}: {e}", bwrap.display()))
        }
        SandboxKind::Seatbelt => SandboxError::Spawn(format!("{program}: {e}")),
    })?;
    let pid = child.id();
    let mut out = child
        .stdout
        .take()
        .ok_or_else(|| SandboxError::Spawn("no stdout".into()))?;
    let mut err = child
        .stderr
        .take()
        .ok_or_else(|| SandboxError::Spawn("no stderr".into()))?;
    let read_out = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b).await;
        b
    });
    let read_err = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b).await;
        b
    });
    let (exit_code, timed_out, interrupted) = tokio::select! {
        waited = tokio::time::timeout(spec.timeout, child.wait()) => match waited {
            Ok(status) => (status.ok().and_then(|s| s.code()), false, false),
            Err(_) => (None, true, false),
        },
        () = stop => (None, false, true),
    };
    // Always sweep the tree: backgrounded or detached descendants must not outlive the command.
    if let Some(pid) = pid {
        kill_tree(pid);
    }
    let _ = child.kill().await;
    let stdout = tokio::time::timeout(Duration::from_secs(5), read_out)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let stderr = tokio::time::timeout(Duration::from_secs(5), read_err)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let mut stderr = stderr;
    let mut removed_reserved = Vec::new();
    if let Some(m) = marker {
        let sandboxed = m.exists();
        let _ = std::fs::remove_file(&m);
        if !sandboxed && !timed_out && !interrupted {
            // bubblewrap exited before running the command: never fall back to
            // running it unsandboxed.
            return Err(SandboxError::Unavailable(format!(
                "{} could not start the sandbox: {}",
                bwrap.display(),
                first_line(&stderr)
            )));
        }
        removed_reserved = remove_new_reserved(&spec.workspace, &reserved);
        for p in &removed_reserved {
            stderr.extend_from_slice(
                format!(
                    "\n[sandbox] removed {}: commands may not create .git or .duet\n",
                    p.display()
                )
                .as_bytes(),
            );
        }
    }
    let (stdout_total, stderr_total) = (stdout.len(), stderr.len());
    let mut spilled_to = None;
    if (stdout_total > spec.output_cap || stderr_total > spec.output_cap)
        && spec.spill_file.is_some()
    {
        let path = spec.spill_file.clone().unwrap_or_default();
        let mut full = stdout.clone();
        full.extend_from_slice(b"\n--- stderr ---\n");
        full.extend_from_slice(&stderr);
        if std::fs::write(&path, full).is_ok() {
            spilled_to = Some(path);
        }
    }
    let cap = |mut v: Vec<u8>| {
        v.truncate(spec.output_cap);
        v
    };
    Ok(Output {
        exit_code,
        timed_out,
        interrupted,
        stdout: cap(stdout),
        stderr: cap(stderr),
        stdout_total,
        stderr_total,
        spilled_to,
        removed_reserved,
        duration: started.elapsed(),
    })
}

/// `(pid, ppid)` of every process, as `ps` reports them.
#[cfg(not(target_os = "linux"))]
fn process_table() -> Vec<(u32, u32)> {
    let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect()
}

/// `(pid, ppid)` of every process, from `/proc/<pid>/stat` (no `ps` needed).
#[cfg(target_os = "linux")]
fn process_table() -> Vec<(u32, u32)> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            // `pid (comm) state ppid ...`; comm may contain spaces and parentheses.
            let rest = &stat[stat.rfind(')')? + 1..];
            let ppid = rest.split_whitespace().nth(1)?.parse().ok()?;
            Some((pid, ppid))
        })
        .collect()
}

/// Kills `root` and every descendant, including ones that left its process
/// group with `setsid`, by walking the parent links reported by `ps`.
pub fn kill_tree(root: u32) {
    let table = process_table();
    let mut victims = vec![root];
    let mut i = 0;
    while i < victims.len() {
        let parent = victims[i];
        victims.extend(
            table
                .iter()
                .filter(|(_, pp)| *pp == parent)
                .map(|(p, _)| *p),
        );
        i += 1;
    }
    for pid in victims.into_iter().rev() {
        if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
            let _ = rustix::process::kill_process(p, rustix::process::Signal::Kill);
        }
    }
}

/// Behavioural tests against the real platform sandbox: Seatbelt on macOS,
/// bubblewrap on Linux (run them there with `tools/linux-check.sh`).
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    const KIND: SandboxKind = if cfg!(target_os = "linux") {
        SandboxKind::Bubblewrap
    } else {
        SandboxKind::Seatbelt
    };

    fn spec(ws: &Path) -> Spec {
        Spec {
            workspace: ws.to_path_buf(),
            scratch: ws.parent().unwrap().join("scratch"),
            network: false,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: Some(ws.parent().unwrap().join("spill.txt")),
            extra_env: vec![],
            deny_read: Vec::new(),
        }
    }

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        std::fs::create_dir_all(ws.join("vendor/dep/.git")).unwrap();
        std::fs::create_dir_all(ws.join(".duet")).unwrap();
        (d, ws)
    }

    async fn sh_with(s: &Spec, script: &str) -> Output {
        run(
            KIND,
            s,
            &["/bin/sh".into(), "-c".into(), script.into()],
            &s.workspace,
        )
        .await
        .unwrap()
    }

    async fn sh(ws: &Path, script: &str) -> Output {
        sh_with(&spec(ws), script).await
    }

    fn text(o: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    }

    #[test]
    fn sandbox_is_detected() {
        assert_eq!(detect().unwrap(), KIND);
    }

    #[tokio::test]
    async fn denied_paths_cannot_be_read_by_any_means() {
        let (_d, ws) = setup();
        std::fs::create_dir_all(ws.join("data/nested")).unwrap();
        std::fs::write(ws.join("data/nested/c.csv"), "id,balance\n1,8977066\n").unwrap();
        std::fs::write(ws.join("data/unnamed.csv"), "x\n").unwrap();
        std::fs::write(ws.join(".env"), "TOKEN=Qx7pL2mN9vR4\n").unwrap();
        std::fs::write(ws.join("src.txt"), "public\n").unwrap();
        let mut s = spec(&ws);
        s.deny_read = vec![ws.join("data"), ws.join(".env")];
        let script = "cat data/nested/c.csv; od -c .env; ls data; cp .env copy.txt; cat src.txt";
        let o = sh_with(&s, script).await;
        let all = text(&o);
        for secret in ["8977066", "Qx7p", "unnamed"] {
            assert!(!all.contains(secret), "{secret} readable: {all}");
        }
        assert!(all.contains("public"), "{all}");
        assert!(!ws.join("copy.txt").exists());
        assert!(o.shows_denial(), "{all}");
    }

    #[tokio::test]
    async fn denied_paths_stay_denied_through_symlinks() {
        let (d, ws) = setup();
        let outside = d.path().canonicalize().unwrap().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("key.pem"), "PRIVATE-4471\n").unwrap();
        std::fs::create_dir_all(ws.join("data")).unwrap();
        std::fs::write(ws.join("data/c.csv"), "8977066\n").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("keys")).unwrap();
        std::os::unix::fs::symlink("data/c.csv", ws.join("link.csv")).unwrap();
        let mut s = spec(&ws);
        s.deny_read = vec![ws.join("data"), ws.join("keys")];
        // Links made before and during the command, and the targets' own paths.
        let o = sh_with(
            &s,
            &format!(
                "cat link.csv; cat keys/key.pem; cat {0}/key.pem; ls {0}; \
                 ln -s data/c.csv new.csv; cat new.csv; ln data/c.csv hard.csv; cat hard.csv",
                outside.display()
            ),
        )
        .await;
        let all = text(&o);
        for secret in ["8977066", "PRIVATE-4471", "key.pem\n"] {
            assert!(!all.contains(secret), "{secret} readable: {all}");
        }
        assert!(o.shows_denial(), "{all}");
    }

    #[tokio::test]
    async fn a_denied_path_that_does_not_exist_is_not_created() {
        let (_d, ws) = setup();
        let mut s = spec(&ws);
        s.deny_read = vec![ws.join("later"), ws.join("later.env")];
        let o = sh_with(&s, "echo ok").await;
        assert_eq!(o.exit_code, Some(0), "{}", text(&o));
        assert!(!ws.join("later").exists() && !ws.join("later.env").exists());
    }

    #[tokio::test]
    async fn git_and_run_state_are_unreadable_when_denied() {
        let (_d, ws) = setup();
        std::fs::write(ws.join(".git/config"), "[core]\n\tsecret = 5521\n").unwrap();
        std::fs::write(ws.join(".duet/vault.json"), "{\"v\":\"9083\"}").unwrap();
        std::fs::write(ws.join("vendor/dep/.git/HEAD"), "ref: 7730\n").unwrap();
        std::fs::write(ws.join(".git/zz-listed-name"), "").unwrap();
        let mut s = spec(&ws);
        s.deny_read = vec![
            ws.join(".git"),
            ws.join(".duet"),
            ws.join("vendor/dep/.git"),
        ];
        let o = sh_with(
            &s,
            "cat .git/config; od -c .duet/vault.json; ls .git .duet; cat vendor/dep/.git/HEAD; \
             cp -r .git copy; echo x > .git/new",
        )
        .await;
        let all = text(&o);
        for secret in ["5521", "9083", "7730", "zz-listed-name"] {
            assert!(!all.contains(secret), "{secret} readable: {all}");
        }
        assert!(o.shows_denial(), "{all}");
        assert!(!ws.join(".git/new").exists());
        assert!(!ws.join("copy/config").exists());
    }

    #[tokio::test]
    async fn writes_inside_workspace_and_scratch_only() {
        let (_d, ws) = setup();
        assert_eq!(
            sh(&ws, "echo hi > a.txt && mkdir -p src && echo x > src/b.rs")
                .await
                .exit_code,
            Some(0)
        );
        let o = sh(&ws, "echo t > \"$TMPDIR/t\" && cat \"$TMPDIR/t\"").await;
        assert_eq!(o.exit_code, Some(0), "{}", text(&o));
        assert_eq!(o.stdout, b"t\n");
        assert_ne!(
            sh(&ws, "echo x > \"$HOME/.duet-sandbox-probe\"")
                .await
                .exit_code,
            Some(0)
        );
        assert!(
            !Path::new(&std::env::var("HOME").unwrap())
                .join(".duet-sandbox-probe")
                .exists()
        );
        let outside = ws.parent().unwrap().join("outside.txt");
        let o = sh(&ws, &format!("echo x > {}", outside.display())).await;
        assert_ne!(o.exit_code, Some(0));
        assert!(!outside.exists());
    }

    #[tokio::test]
    async fn reserved_directories_are_read_only_at_any_depth() {
        let (_d, ws) = setup();
        for target in [".git/config", "vendor/dep/.git/HEAD", ".duet/x"] {
            let o = sh(&ws, &format!("echo x > {target}")).await;
            assert_ne!(o.exit_code, Some(0), "{target} was writable");
            assert!(!ws.join(target).exists(), "{target}");
        }
        // Moving or deleting them is refused too.
        let o = sh(&ws, "mv .git moved || rm -rf vendor/dep/.git").await;
        assert_ne!(o.exit_code, Some(0), "{}", text(&o));
        assert!(ws.join(".git").is_dir() && ws.join("vendor/dep/.git").is_dir());
        // New ones: Seatbelt refuses to create them; bubblewrap cannot, and
        // removes them once the command has ended, saying so.
        for target in ["nested/.git/new", "deep/er/.duet/x"] {
            let o = sh(
                &ws,
                &format!("mkdir -p \"$(dirname {target})\" 2>/dev/null; echo x > {target}"),
            )
            .await;
            if cfg!(target_os = "macos") {
                assert_ne!(o.exit_code, Some(0), "{target} was writable");
            } else {
                assert_eq!(o.removed_reserved.len(), 1, "{:?}", o.removed_reserved);
                assert!(text(&o).contains("may not create .git or .duet"));
            }
            assert!(!ws.join(target).exists(), "{target}");
            assert!(!ws.join(target).parent().unwrap().exists(), "{target}");
        }
        let o = sh(&ws, "mkdir sub && ln -s /tmp sub/.git; true").await;
        assert!(std::fs::symlink_metadata(ws.join("sub/.git")).is_err());
        if cfg!(target_os = "linux") {
            assert_eq!(o.removed_reserved.len(), 1, "{:?}", o.removed_reserved);
        }
        // Existing ones are left alone.
        assert!(ws.join(".git").is_dir() && ws.join(".duet").is_dir());
    }

    #[tokio::test]
    async fn network_is_denied_by_default() {
        let (_d, ws) = setup();
        for url in ["https://example.com", "http://1.1.1.1"] {
            let o = sh(&ws, &format!("curl -s -m 5 -o /dev/null {url}")).await;
            assert_ne!(o.exit_code, Some(0), "{url}");
        }
    }

    /// Even when duet runs as root, commands hold no capabilities, so they
    /// cannot unmount or remount what hides and protects paths.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn commands_hold_no_capabilities() {
        let (_d, ws) = setup();
        let o = sh(&ws, "grep -E '^Cap(Prm|Eff|Amb):' /proc/self/status").await;
        let status = String::from_utf8_lossy(&o.stdout);
        assert_eq!(status.lines().count(), 3, "{status}");
        for line in status.lines() {
            assert!(line.ends_with("0000000000000000"), "{status}");
        }
        let o = sh(&ws, "umount .git || mount -o remount,rw .git").await;
        assert_ne!(o.exit_code, Some(0), "{}", text(&o));
    }

    #[tokio::test]
    async fn unix_sockets_are_unreachable_without_network() {
        let (_d, ws) = setup();
        // A socket the command can see (bubblewrap hides /tmp and /run), like a
        // container engine's under $HOME.
        let path = ws.join("engine.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let curl = format!(
            "curl -s -m 3 --unix-socket {} http://engine/version",
            path.display()
        );
        let o = sh(&ws, &curl).await;
        assert_ne!(o.exit_code, Some(0), "{}", text(&o));
        let accepted = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
        assert!(accepted.is_err(), "the command connected to the socket");
        // With the network allowed, the same connection is made.
        let mut s = spec(&ws);
        s.network = true;
        let (_, accepted) = tokio::join!(
            sh_with(&s, &curl),
            tokio::time::timeout(Duration::from_secs(5), listener.accept())
        );
        assert!(matches!(accepted, Ok(Ok(_))), "{accepted:?}");
    }

    #[tokio::test]
    async fn environment_is_cleared_to_the_allowlist() {
        let (_d, ws) = setup();
        let mut s = spec(&ws);
        s.extra_env.push(("VISIBLE".into(), "yes".into()));
        // `cargo test` gives this process CARGO_PKG_NAME; it must not pass through.
        assert!(std::env::var_os("CARGO_PKG_NAME").is_some());
        let o = run(KIND, &s, &["/usr/bin/env".into()], &ws).await.unwrap();
        let env = String::from_utf8_lossy(&o.stdout);
        assert!(env.contains("VISIBLE=yes"), "{env}");
        assert!(env.contains(&format!(
            "TMPDIR={}",
            s.scratch.canonicalize().unwrap().display()
        )));
        for line in env.lines() {
            let key = line.split('=').next().unwrap_or("");
            assert!(
                ENV_ALLOWLIST.contains(&key)
                    || ["VISIBLE", "TMPDIR", "__CF_USER_TEXT_ENCODING"].contains(&key)
                    // Set by the shell that starts the command under bubblewrap.
                    || (KIND == SandboxKind::Bubblewrap && key == "PWD"),
                "{key} passed through: {env}"
            );
        }
    }

    #[tokio::test]
    async fn timeout_kills_detached_descendants() {
        let (_d, ws) = setup();
        let mut s = spec(&ws);
        s.timeout = Duration::from_secs(2);
        let marker = ws.join("survivor");
        let script = format!(
            "/usr/bin/nohup /bin/sh -c 'sleep 4; echo alive > {}' >/dev/null 2>&1 & sleep 30",
            marker.display()
        );
        let o = sh_with(&s, &script).await;
        assert!(o.timed_out);
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(!marker.exists(), "a descendant survived the timeout");
    }

    #[tokio::test]
    async fn a_stop_kills_the_whole_tree_at_once() {
        let (_d, ws) = setup();
        let marker = ws.join("survivor");
        // macOS has no `setsid` command; `nohup` alone keeps the process group.
        let session = if cfg!(target_os = "linux") {
            format!(
                "setsid /bin/sh -c 'sleep 3; echo alive > {}.2' & ",
                marker.display()
            )
        } else {
            String::new()
        };
        let script = format!(
            "/usr/bin/nohup /bin/sh -c 'sleep 3; echo alive > {}' >/dev/null 2>&1 & {session}sleep 30",
            marker.display()
        );
        let started = Instant::now();
        let o = run_until(
            KIND,
            &spec(&ws),
            &["/bin/sh".into(), "-c".into(), script],
            &ws,
            tokio::time::sleep(Duration::from_millis(500)),
        )
        .await
        .unwrap();
        assert!(o.interrupted && !o.timed_out && o.exit_code.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(!marker.exists(), "a descendant survived the stop");
        assert!(
            !ws.join("survivor.2").exists(),
            "a new session survived the stop"
        );
    }

    #[tokio::test]
    async fn large_output_is_capped_and_spilled() {
        let (_d, ws) = setup();
        let o = sh(
            &ws,
            "i=0; while [ $i -lt 2000 ]; do echo line-$i; i=$((i+1)); done",
        )
        .await;
        assert_eq!(o.stdout.len(), 4096);
        assert!(o.stdout_total > 4096);
        let spilled = std::fs::read_to_string(o.spilled_to.unwrap()).unwrap();
        assert!(spilled.contains("line-1999"));
    }

    #[tokio::test]
    async fn real_toolchains_run() {
        let (_d, ws) = setup();
        let tools: &[&str] = if cfg!(target_os = "macos") {
            &["cargo", "node", "git"]
        } else {
            &["cargo", "git"]
        };
        for tool in tools {
            let argv = [tool.to_string(), "--version".to_string()];
            let o = run(KIND, &spec(&ws), &argv, &ws).await.unwrap();
            assert_eq!(
                o.exit_code,
                Some(0),
                "{argv:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        }
    }
}

/// The sandbox fails closed: when bubblewrap is missing or cannot set up its
/// namespaces, commands are refused with an error and never run.
#[cfg(all(test, target_os = "linux"))]
mod fail_closed_tests {
    use super::*;

    fn spec(root: &Path) -> Spec {
        Spec {
            workspace: root.join("ws"),
            scratch: root.join("scratch"),
            network: false,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
        }
    }

    /// A stand-in for `bwrap` that fails the way it does without namespaces.
    fn broken_bwrap(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("bwrap");
        std::fs::write(
            &p,
            "#!/bin/sh\necho 'bwrap: No permissions to create new namespace' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    async fn canary_run(bwrap: &Path, root: &Path) -> Result<Output, SandboxError> {
        let s = spec(root);
        std::fs::create_dir_all(&s.workspace).unwrap();
        run_with(
            SandboxKind::Bubblewrap,
            bwrap,
            &s,
            &["/bin/sh".into(), "-c".into(), "echo ran > canary".into()],
            &s.workspace,
            std::future::pending(),
        )
        .await
    }

    #[tokio::test]
    async fn commands_are_refused_when_namespaces_are_unavailable() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let bwrap = broken_bwrap(&root);
        let err = canary_run(&bwrap, &root).await.unwrap_err();
        assert!(matches!(err, SandboxError::Unavailable(_)), "{err}");
        let msg = err.to_string();
        assert!(
            msg.contains("No permissions to create new namespace") && msg.contains("refused"),
            "{msg}"
        );
        assert!(!root.join("ws/canary").exists(), "the command ran");
        let probe = probe_bwrap(&bwrap).unwrap_err().to_string();
        assert!(probe.contains("cannot create its sandbox"), "{probe}");
    }

    #[tokio::test]
    async fn commands_are_refused_when_bubblewrap_is_missing() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let err = canary_run(&root.join("no-such-bwrap"), &root)
            .await
            .unwrap_err();
        assert!(matches!(err, SandboxError::Unavailable(_)), "{err}");
        assert!(!root.join("ws/canary").exists(), "the command ran");
    }

    /// Set by `tools/linux-check.sh` in a container without user namespaces:
    /// the real bubblewrap must be reported unusable and run nothing.
    #[tokio::test]
    async fn real_bubblewrap_without_namespaces_refuses() {
        if std::env::var_os("DUET_EXPECT_NO_NAMESPACES").is_none() {
            return;
        }
        let err = detect().unwrap_err().to_string();
        assert!(err.contains("cannot create its sandbox"), "{err}");
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let err = canary_run(Path::new(BWRAP), &root).await.unwrap_err();
        assert!(matches!(err, SandboxError::Unavailable(_)), "{err}");
        assert!(!root.join("ws/canary").exists(), "the command ran");
    }
}

#[cfg(all(test, unix))]
mod bwrap_args_tests {
    use super::*;

    #[test]
    fn bwrap_args_protect_reserved_dirs_and_deny_reads() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let mut s = Spec {
            workspace: root.join("ws"),
            scratch: root.join("scratch"),
            network: false,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
        };
        for dir in ["ws/.git", "ws/vendor/dep/.git", "ws/.duet", "ws/data"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("ws/.env"), "x").unwrap();
        s.deny_read = vec![
            root.join("ws/data"),
            root.join("ws/.env"),
            root.join("ws/absent"),
        ];
        let stubs = DenyStubs::ensure(&root).unwrap();
        let reserved = reserved_entries(&s.workspace);
        assert_eq!(
            reserved.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            [".duet", ".git", "vendor/dep/.git"].map(|p| s.workspace.join(p))
        );
        let joined = bwrap_args(&s, &reserved, &stubs)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let ws = s.workspace.display();
        assert!(joined.contains("--cap-drop ALL") && joined.contains("--tmpfs /run"));
        assert!(joined.contains(&format!("--ro-bind {ws}/.git {ws}/.git")));
        assert!(joined.contains(&format!(
            "--ro-bind {ws}/vendor/dep/.git {ws}/vendor/dep/.git"
        )));
        assert!(joined.contains(&format!("--ro-bind {} {ws}/data", stubs.dir.display())));
        assert!(joined.contains(&format!("--ro-bind {} {ws}/.env", stubs.file.display())));
        assert!(!joined.contains("absent"));
        assert!(!joined.contains("--share-net"));
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod toolchain_tests {
    use super::*;

    fn kind() -> SandboxKind {
        detect().unwrap()
    }

    fn spec(ws: &Path) -> Spec {
        Spec {
            workspace: ws.to_path_buf(),
            scratch: ws.parent().unwrap().join("scratch"),
            network: false,
            timeout: Duration::from_secs(240),
            output_cap: 64 * 1024,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
        }
    }

    #[tokio::test]
    async fn cargo_test_and_node_test_run_inside_the_sandbox() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("Cargo.toml"),
            "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("src/lib.rs"),
            "#[test] fn t() { assert_eq!(1 + 1, 2); }\n",
        )
        .unwrap();
        std::fs::write(
            ws.join("a.test.ts"),
            "import { test } from 'node:test';\ntest('x', () => {});\n",
        )
        .unwrap();
        let cargo = run(
            kind(),
            &spec(&ws),
            &["cargo".into(), "test".into(), "--offline".into()],
            &ws,
        )
        .await
        .unwrap();
        assert_eq!(
            cargo.exit_code,
            Some(0),
            "{}",
            String::from_utf8_lossy(&cargo.stderr)
        );
        if cfg!(target_os = "linux") {
            // The Linux check image has no Node.js.
            return;
        }
        let node = run(
            kind(),
            &spec(&ws),
            &["node".into(), "--test".into(), "a.test.ts".into()],
            &ws,
        )
        .await
        .unwrap();
        assert_eq!(
            node.exit_code,
            Some(0),
            "{}",
            String::from_utf8_lossy(&node.stderr)
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod linker_tests {
    use super::*;

    #[tokio::test]
    async fn builds_produce_no_xcrun_cache_errors() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("Cargo.toml"),
            "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();
        let spec = Spec {
            workspace: ws.clone(),
            scratch: d.path().join("scratch"),
            network: false,
            timeout: Duration::from_secs(240),
            output_cap: 64 * 1024,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
        };
        let o = run(
            SandboxKind::Seatbelt,
            &spec,
            &["cargo".into(), "build".into(), "--offline".into()],
            &ws,
        )
        .await
        .unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.exit_code, Some(0), "{err}");
        assert!(!err.contains("couldn't create cache file"), "{err}");
    }
}
