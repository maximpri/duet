// SPDX-License-Identifier: GPL-3.0-or-later
//! Command sandbox (Seatbelt on macOS, bubblewrap on Linux), environment
//! allowlist and process-tree control.
//!
//! Commands may read the filesystem (except paths the caller denies, and
//! `.declass` at any depth: Declass's own run state), write only inside the workspace
//! and the run's scratch directory, never write `.git` or `.declass` at any depth,
//! and reach the network only as [`Network`] says: not at all, only through the
//! host's egress proxy, or freely. Resolution fails closed: without a sandbox
//! binary at its fixed absolute path, no command runs. Every command's and
//! server's process tree is held to memory limits ([`governor`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

pub mod bridge;
pub use declass_governor as governor;
mod home_secrets;

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

pub use home_secrets::HOME_SECRETS;

/// The [`HOME_SECRETS`] that exist under `$HOME`, except any holding
/// `workspace` (denying it would deny the work itself).
pub fn home_secrets(workspace: &Path) -> Vec<PathBuf> {
    match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        Some(home) => home_secrets_in(Path::new(&home), workspace),
        None => Vec::new(),
    }
}

/// [`home_secrets`] for the home directory `home`.
pub fn home_secrets_in(home: &Path, workspace: &Path) -> Vec<PathBuf> {
    HOME_SECRETS
        .iter()
        .map(|p| home.join(p))
        .filter(|p| p.symlink_metadata().is_ok() && !workspace.starts_with(p))
        .collect()
}

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

/// What a sandboxed process may reach over the network.
#[derive(Debug, Clone, Default)]
pub enum Network {
    /// Nothing: no IP, no Unix sockets.
    #[default]
    Off,
    /// Only the host's egress proxy, which decides every connection, and
    /// servers the process starts itself on loopback (any port under
    /// bubblewrap, the [`DEV_PORTS`] under Seatbelt). The proxy variables
    /// (`HTTPS_PROXY` and the like, [`proxy_env`]) point at it; name
    /// resolution is left to the proxy.
    Proxy(ProxyRoute),
    /// Unrestricted.
    All,
}

impl From<bool> for Network {
    /// `true`: [`Network::All`]; `false`: [`Network::Off`].
    fn from(all: bool) -> Self {
        if all { Network::All } else { Network::Off }
    }
}

impl Network {
    pub fn is_off(&self) -> bool {
        matches!(self, Network::Off)
    }
}

/// How a command reaches the egress proxy.
#[derive(Debug, Clone)]
pub enum ProxyRoute {
    /// Seatbelt: the proxy listens on this port of the host's loopback.
    /// Commands may connect to it and to the [`DEV_PORTS`] nobody on the
    /// host listened on when they started (their own servers); every other
    /// address is refused.
    Loopback { port: u16 },
    /// bubblewrap: the command keeps its own network namespace (only its own
    /// loopback) and is started under a helper that forwards its proxy
    /// connections over `channel` (see [`bridge`]). `helper` is the helper's
    /// command line (a program and its leading arguments, e.g. `declass
    /// __sandbox-bridge`); the program is mounted into the sandbox.
    Bridge {
        channel: Arc<std::os::fd::OwnedFd>,
        helper: Vec<OsString>,
    },
}

/// The variables that point HTTP clients at a proxy on `127.0.0.1:port`:
/// the usual `HTTP(S)_PROXY` family, and the tools that read their own
/// (npm, yarn, cargo, pip, Node's own `fetch`, Maven and Gradle). Loopback is
/// excluded (`NO_PROXY`), so a command reaches the servers it starts itself.
pub fn proxy_env(port: u16) -> Vec<(String, String)> {
    let url = format!("http://127.0.0.1:{port}");
    let local = "localhost,127.0.0.1,::1";
    let java = format!(
        "-Dhttp.proxyHost=127.0.0.1 -Dhttp.proxyPort={port} -Dhttps.proxyHost=127.0.0.1 \
         -Dhttps.proxyPort={port} -Dhttp.nonProxyHosts=localhost|127.0.0.1"
    );
    let mut env: Vec<(String, String)> = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "npm_config_proxy",
        "npm_config_https_proxy",
        "YARN_HTTP_PROXY",
        "YARN_HTTPS_PROXY",
        "CARGO_HTTP_PROXY",
        "PIP_PROXY",
    ]
    .iter()
    .map(|k| ((*k).to_owned(), url.clone()))
    .collect();
    for k in ["NO_PROXY", "no_proxy", "npm_config_noproxy"] {
        env.push((k.to_owned(), local.to_owned()));
    }
    env.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
    env.push(("MAVEN_OPTS".into(), java.clone()));
    env.push(("GRADLE_OPTS".into(), java));
    env
}

#[derive(Debug, Clone)]
pub struct Spec {
    /// Canonical workspace root.
    pub workspace: PathBuf,
    /// Scratch directory for `TMPDIR`; must be outside `.git`.
    pub scratch: PathBuf,
    pub network: Network,
    pub timeout: Duration,
    /// Bytes of stdout/stderr kept in memory each; the full output is spilled.
    pub output_cap: usize,
    /// Where the full combined output goes when it exceeds the cap.
    pub spill_file: Option<PathBuf>,
    pub extra_env: Vec<(String, String)>,
    /// Absolute paths the command may not read (directories: everything under them).
    /// Used to keep sensitive files out of commands whose output the frontier sees.
    pub deny_read: Vec<PathBuf>,
    /// The workspace is read-only as well: the command may write only its
    /// scratch directory (the commands of read-only sub-agents).
    pub read_only: bool,
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
    /// `.git` and `.declass` entries the command created and the sandbox removed
    /// afterwards (bubblewrap only; Seatbelt refuses to create them).
    pub removed_reserved: Vec<PathBuf>,
    /// Processes the memory governor stopped (see [`governor`]); each is also
    /// named at the end of `stderr`.
    pub memory_kills: Vec<governor::Kill>,
    pub duration: Duration,
}

impl Output {
    /// Whether the output shows the sandbox refusing an operation. Seatbelt
    /// refuses with `EPERM` ("Operation not permitted"); under bubblewrap a
    /// denied path is replaced by an empty file or directory that nobody may
    /// open, so the refusal is `EACCES` ("Permission denied").
    pub fn shows_denial(&self) -> bool {
        [&self.stdout, &self.stderr].iter().any(|bytes| {
            ["operation not permitted", "permission denied"]
                .iter()
                .any(|message| {
                    bytes
                        .windows(message.len())
                        .any(|window| window.eq_ignore_ascii_case(message.as_bytes()))
                })
        })
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

/// Seatbelt profile for a workspace-write command. `host_ports` matter only
/// for [`ProxyRoute::Loopback`]: the TCP ports something on the host
/// listened on when the command started ([`listening_ports`]), which stay
/// closed to it (the proxy's excepted); `None` (unknown) closes every
/// loopback port but the proxy's.
pub fn seatbelt_profile(spec: &Spec, host_ports: Option<&[u16]>) -> Result<String, SandboxError> {
    let ws = quote(&spec.workspace)?;
    let scratch = quote(&spec.scratch)?;
    let root = spec
        .workspace
        .to_str()
        .ok_or_else(|| SandboxError::Path(spec.workspace.display().to_string()))?;
    let reserved = format!("^{}(/.*)?/\\.(git|declass)(/.*)?$", regex_escape(root));
    // Run state (the vault maps every placeholder to its real value; handles,
    // transcripts, the audit log) is unreadable whatever the caller denies.
    let run_state = format!("^{}(/.*)?/\\.declass(/.*)?$", regex_escape(root));
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
    ];
    if !spec.read_only {
        rules.push(format!("(allow file-write* (subpath {ws}))"));
    }
    rules.extend([
        format!("(allow file-write* (subpath {scratch}))"),
        format!("(deny file-write* (regex #\"{reserved}\"))"),
        // Apple's toolchain helper caches here regardless of TMPDIR; allow only its cache files.
        "(allow file-write* (regex #\"^/private/var/folders/[^/]+/[^/]+/T/xcrun_db\"))".to_owned(),
    ]);
    // Later rules win: these override the blanket read allowance above.
    rules.push(format!("(deny file-read* (regex #\"{run_state}\"))"));
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
    match &spec.network {
        Network::Off => {}
        Network::All => rules.push("(allow network*)".to_owned()),
        Network::Proxy(ProxyRoute::Loopback { port }) => {
            rules.extend(proxy_rules(*port, host_ports));
        }
        Network::Proxy(ProxyRoute::Bridge { .. }) => {
            return Err(SandboxError::Unavailable(
                "the bridge route to the egress proxy needs bubblewrap".into(),
            ));
        }
    }
    Ok(rules.join("\n"))
}

/// Loopback ports a command may connect to under Seatbelt with the proxy
/// route, besides the proxy's: the usual ports of development servers
/// (`npm run dev`, Vite, Rails, Django, Flask, Storybook, Expo, debuggers).
/// Inclusive ranges. Seatbelt cannot give a command loopback ports of its own
/// the way a network namespace does, and cannot reliably carve the host's
/// ports out of an allowed range (a later `deny` for one port is ignored for
/// some ports), so the rule is a positive list, minus the ports the host
/// already uses when the command starts.
pub const DEV_PORTS: &[(u16, u16)] = &[
    (1234, 1234),
    (1313, 1313),
    (1337, 1337),
    (3000, 3099),
    (4000, 4099),
    (4173, 4173),
    (4200, 4200),
    (4321, 4321),
    (5000, 5099),
    (5173, 5199),
    (5500, 5510),
    (6006, 6007),
    (7000, 7099),
    (8000, 8099),
    (8443, 8443),
    (8787, 8788),
    (8888, 8889),
    (9000, 9099),
    (9229, 9230),
    (19000, 19006),
    (24678, 24678),
];

/// Seatbelt rules for [`ProxyRoute::Loopback`]: the command may listen on
/// loopback, and connect over TCP to the proxy and to the [`DEV_PORTS`] no
/// host process held when it started (`host_ports`; `None`: unknown, so the
/// proxy only). Nothing else: no other address, no UDP (so no resolver,
/// local or not), no Unix socket. Only `allow` rules, one with every port.
fn proxy_rules(port: u16, host_ports: Option<&[u16]>) -> Vec<String> {
    let mut ports = vec![port];
    if let Some(busy) = host_ports {
        ports.extend(
            DEV_PORTS
                .iter()
                .flat_map(|&(a, b)| a..=b)
                .filter(|p| !busy.contains(p) && *p != port),
        );
    }
    let filters: Vec<String> = ports
        .iter()
        .map(|p| format!("(remote tcp \"localhost:{p}\")"))
        .collect();
    vec![
        "(allow network-bind (local ip \"localhost:*\"))".to_owned(),
        "(allow network-inbound (local ip \"localhost:*\"))".to_owned(),
        format!("(allow network-outbound {})", filters.join(" ")),
    ]
}

/// The TCP ports something on this machine listens on (any address: a port
/// bound to all addresses is reachable on loopback too): those of the
/// operator's processes, from `lsof`, and whatever `netstat` reports besides
/// (it lists every user's sockets, but only for some callers: started by an
/// unsigned program it prints no TCP sockets at all). `None` when neither
/// could be read.
pub fn listening_ports() -> Option<Vec<u16>> {
    let run = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    // `-Fn`: one `n<address>:<port>` line per socket; exit 1 when there are none.
    let lsof = run("/usr/sbin/lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fn"]);
    let netstat = run("/usr/sbin/netstat", &["-an"]);
    if lsof.is_none() && netstat.is_none() {
        return None;
    }
    let mut ports: Vec<u16> = lsof
        .iter()
        .flat_map(|t| t.lines())
        .filter_map(|l| l.strip_prefix('n')?.rsplit(':').next()?.parse().ok())
        .chain(
            netstat
                .iter()
                .flat_map(|t| t.lines())
                .filter(|l| l.starts_with("tcp") && l.split_whitespace().any(|w| w == "LISTEN"))
                // `tcp4 0 0 127.0.0.1.8080 *.* LISTEN`: the port ends the local address.
                .filter_map(|l| {
                    l.split_whitespace()
                        .nth(3)?
                        .rsplit('.')
                        .next()?
                        .parse()
                        .ok()
                }),
        )
        .collect();
    ports.sort_unstable();
    ports.dedup();
    Some(ports)
}

/// A `.git` or `.declass` entry in the workspace, identified by its inode so a
/// replacement at the same path is told apart from the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reserved {
    pub path: PathBuf,
    id: (u64, u64),
}

/// Every `.git` and `.declass` entry (directory, file or symlink) under the
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
            if name == ".git" || name == ".declass" {
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
        let root = parent.join(format!("declass-sandbox-{uid}"));
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
/// workspace and the scratch directory are mounted writable; every existing `.git` (`reserved`, at any
/// depth) is mounted back read-only and every existing `.declass` is covered by a
/// `stubs` stand-in (run state is never readable); every existing path in
/// `spec.deny_read` is covered by a `stubs` stand-in. Paths are resolved
/// first, so a symlink is covered at its target. A denied path that does not
/// exist yet is skipped: there is nothing to read, and a mount point would
/// create it in the workspace. `.git` and `.declass` entries a command creates
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
    let share_net = matches!(spec.network, Network::All);
    if share_net {
        // Name resolution on systemd hosts goes through a stub under /run.
        push_bind(
            &mut a,
            "--ro-bind-try",
            Path::new("/run/systemd/resolve"),
            Path::new("/run/systemd/resolve"),
        );
    }
    let workspace_bind = if spec.read_only {
        "--ro-bind"
    } else {
        "--bind"
    };
    push_bind(&mut a, workspace_bind, &spec.workspace, &spec.workspace);
    for r in reserved {
        if let Ok(target) = r.path.canonicalize() {
            // Run state is unreadable, not only read-only.
            let source = match (r.path.file_name(), target.is_dir()) {
                (Some(n), true) if n == ".declass" => &stubs.dir,
                (Some(n), false) if n == ".declass" => &stubs.file,
                _ => &target,
            };
            push_bind(&mut a, "--ro-bind", source, &target);
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
    if let Network::Proxy(ProxyRoute::Bridge { helper, .. }) = &spec.network
        && let Some(program) = helper.first()
    {
        // At a fixed place, whatever the host path (under /tmp it would be hidden).
        push_bind(
            &mut a,
            "--ro-bind",
            Path::new(program),
            Path::new(bridge::HELPER_PATH),
        );
    }
    // The empty /tmp and /run only hold mount points: nothing else is writable.
    a.extend(["--remount-ro", "/tmp", "--remount-ro", "/run"].map(OsString::from));
    if share_net {
        a.push(OsString::from("--share-net"));
    }
    a
}

/// The seccomp filter bubblewrap installs unless the network is unrestricted
/// (with the egress proxy too: its bridge needs only socket pairs).
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
        let fd = rustix::fs::memfd_create("declass-seccomp", rustix::fs::MemfdFlags::CLOEXEC)
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

/// Removes the `.git` and `.declass` entries a command created (those not in
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
    run_with(
        kind,
        Path::new(BWRAP),
        spec,
        argv,
        cwd,
        stop,
        governor::limits(),
    )
    .await
}

static MARKERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Held while a sandboxed process is started. A descriptor meant for one
/// sandbox is made inheritable only under it (see [`prepare`]), so no other
/// sandboxed process started meanwhile inherits it.
static SPAWNING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn spawning() -> std::sync::MutexGuard<'static, ()> {
    SPAWNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An inheritable descriptor for the process about to start, and the lock
/// that keeps every other sandboxed start waiting until it is closed.
struct Inherited {
    _fd: std::os::fd::OwnedFd,
    _starting: std::sync::MutexGuard<'static, ()>,
}

/// A sandboxed command ready to start.
struct Prepared {
    cmd: tokio::process::Command,
    /// The spec with its scratch directory resolved.
    spec: Spec,
    reserved: Vec<Reserved>,
    marker: Option<PathBuf>,
    /// The named pipe a server reads its input from (bubblewrap).
    fifo: Option<PathBuf>,
    /// Drop right after starting the command.
    inherited: Option<Inherited>,
}

impl Prepared {
    /// Starts the command; no other sandboxed process starts meanwhile.
    fn start(&mut self) -> std::io::Result<tokio::process::Child> {
        let starting = self.inherited.is_none().then(spawning);
        let child = self.cmd.spawn();
        drop(self.inherited.take());
        drop(starting);
        child
    }
}

/// Builds the sandboxed command for `argv`. With `server_input`, the command
/// reads the host's messages on its standard input (a pipe under Seatbelt, a
/// named pipe in the scratch directory under bubblewrap, whose own standard
/// input carries the seccomp filter); otherwise its input is empty.
fn prepare(
    kind: SandboxKind,
    bwrap: &Path,
    spec: &Spec,
    argv: &[String],
    cwd: &Path,
    server_input: bool,
) -> Result<Prepared, SandboxError> {
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
    let mut fifo = None;
    let mut inherited = None;
    let mut stdin = if server_input {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    if server_input && matches!(spec.network, Network::Proxy(_)) {
        return Err(SandboxError::Spawn(
            "a server process cannot use the egress proxy".into(),
        ));
    }
    let mut cmd = match kind {
        SandboxKind::Seatbelt => {
            let host_ports = match spec.network {
                Network::Proxy(_) => listening_ports(),
                _ => None,
            };
            let mut c = tokio::process::Command::new(SANDBOX_EXEC);
            c.arg("-p")
                .arg(seatbelt_profile(spec, host_ports.as_deref())?)
                .arg(program)
                .args(args);
            c
        }
        SandboxKind::Bubblewrap => {
            let bridge = match &spec.network {
                Network::Proxy(ProxyRoute::Bridge { channel, helper }) => {
                    Some((channel.clone(), helper.clone()))
                }
                Network::Proxy(ProxyRoute::Loopback { .. }) => {
                    return Err(SandboxError::Unavailable(
                        "bubblewrap reaches the egress proxy only through the bridge".into(),
                    ));
                }
                _ => None,
            };
            reserved = reserved_entries(&spec.workspace);
            let stubs = DenyStubs::ensure(&std::env::temp_dir())?;
            let n = MARKERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let m = spec.scratch.join(format!(
                ".declass-sandbox-started-{}-{n}",
                std::process::id(),
            ));
            let mut c = tokio::process::Command::new(bwrap);
            c.args(bwrap_args(spec, &reserved, &stubs));
            match (&spec.network, &bridge) {
                (Network::All, _) => {
                    if server_input {
                        stdin = Stdio::null();
                    }
                }
                (_, None) => {
                    // bubblewrap reads the filter from its standard input.
                    let filter = seccomp::filter()?;
                    c.args(["--seccomp", "0"]);
                    stdin = Stdio::from(filter);
                }
                (_, Some((channel, _))) => {
                    // The standard input carries the channel to the bridge
                    // helper, so the filter goes by descriptor number: an
                    // inheritable copy, open only while this command starts.
                    use std::os::fd::AsRawFd;
                    let err = |e: rustix::io::Errno| SandboxError::Spawn(format!("bridge: {e}"));
                    let filter = seccomp::filter()?;
                    let starting = spawning();
                    let fd = rustix::io::dup(&filter).map_err(err)?;
                    c.arg("--seccomp").arg(fd.as_raw_fd().to_string());
                    stdin =
                        Stdio::from(rustix::io::fcntl_dupfd_cloexec(&**channel, 0).map_err(err)?);
                    inherited = Some(Inherited {
                        _fd: fd,
                        _starting: starting,
                    });
                }
            }
            c.arg("--chdir").arg(cwd).arg("--");
            // Proves the sandbox was set up: bubblewrap only reaches this
            // shell once every namespace and mount is in place. A command's
            // standard input is empty, as under Seatbelt (a bridged one's is
            // the channel, which the helper keeps and replaces with an empty
            // input for the command); a server's is the named pipe the host
            // writes to.
            if let Some((_, helper)) = &bridge {
                c.args(["/bin/sh", "-c", ": > \"$0\" && exec \"$@\""])
                    .arg(&m)
                    .arg(bridge::HELPER_PATH)
                    .args(helper.iter().skip(1));
            } else if server_input {
                let f = spec
                    .scratch
                    .join(format!(".declass-sandbox-input-{}-{n}", std::process::id()));
                make_fifo(&f)?;
                c.args([
                    "/bin/sh",
                    "-c",
                    "exec <\"$1\"; : > \"$0\" && shift && exec \"$@\"",
                ])
                .arg(&m)
                .arg(&f);
                fifo = Some(f);
            } else {
                c.args([
                    "/bin/sh",
                    "-c",
                    "exec </dev/null; : > \"$0\" && exec \"$@\"",
                ])
                .arg(&m);
            }
            c.arg(program).args(args);
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
        .envs(spec.extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    // Under bubblewrap the bridge helper sets them, with the port it listens on.
    if let Network::Proxy(ProxyRoute::Loopback { port }) = spec.network {
        cmd.envs(proxy_env(port));
    }
    cmd.stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    Ok(Prepared {
        cmd,
        spec: resolved,
        reserved,
        marker,
        fifo,
        inherited,
    })
}

#[cfg(target_os = "linux")]
fn make_fifo(path: &Path) -> Result<(), SandboxError> {
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .map_err(|e| SandboxError::Spawn(format!("input pipe: {e}")))
}

#[cfg(not(target_os = "linux"))]
fn make_fifo(_path: &Path) -> Result<(), SandboxError> {
    Err(SandboxError::Unavailable(
        "bubblewrap runs only on Linux".into(),
    ))
}

async fn run_with(
    kind: SandboxKind,
    bwrap: &Path,
    spec: &Spec,
    argv: &[String],
    cwd: &Path,
    stop: impl std::future::Future<Output = ()>,
    limits: governor::MemoryLimits,
) -> Result<Output, SandboxError> {
    let program = argv.first().cloned().unwrap_or_default();
    // In a block of its own: what `prepare` holds is gone before any await.
    let (spawned, resolved, reserved, marker) = {
        let mut prepared = prepare(kind, bwrap, spec, argv, cwd, false)?;
        let spawned = prepared.start();
        let Prepared {
            spec,
            reserved,
            marker,
            ..
        } = prepared;
        (spawned, spec, reserved, marker)
    };
    let spec = &resolved;
    let started = Instant::now();
    let mut child = spawned.map_err(|e| match kind {
        SandboxKind::Bubblewrap => {
            SandboxError::Unavailable(format!("cannot start {}: {e}", bwrap.display()))
        }
        SandboxKind::Seatbelt => SandboxError::Spawn(format!("{program}: {e}")),
    })?;
    let pid = child.id();
    let watch = pid.map(|p| governor::Governor::watch(p, limits));
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
    let memory = match watch {
        Some(g) => tokio::task::spawn_blocking(move || g.finish())
            .await
            .unwrap_or_default(),
        None => governor::Usage::default(),
    };
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
                    "\n[sandbox] removed {}: commands may not create .git or .declass\n",
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
    // After the cap, so a long output cannot hide why a process stopped.
    let mut stderr = cap(stderr);
    for k in &memory.kills {
        stderr.extend_from_slice(memory_note(k, &limits).as_bytes());
    }
    Ok(Output {
        exit_code,
        timed_out,
        interrupted,
        stdout: cap(stdout),
        stderr,
        stdout_total,
        stderr_total,
        spilled_to,
        removed_reserved,
        memory_kills: memory.kills,
        duration: started.elapsed(),
    })
}

/// The line a command's error output ends with for each process the memory
/// governor stopped.
fn memory_note(k: &governor::Kill, limits: &governor::MemoryLimits) -> String {
    const GROWING: &str =
        "A process that keeps growing is usually looping or collecting without bound.";
    let why = match k.rule {
        governor::KillRule::MachinePressure => format!(
            "{}, and it was the command's largest process.",
            k.limit(limits)
        ),
        governor::KillRule::ProcessLimit => format!(
            "over the limit of {} (limits.process_memory_mb). {GROWING}",
            k.limit(limits)
        ),
        governor::KillRule::TotalLimit => format!(
            "the command's processes were over the limit of {} (limits.command_memory_mb) and it was the largest. {GROWING}",
            k.limit(limits)
        ),
    };
    format!(
        "\n[sandbox] stopped {} (pid {}) at {} MB of memory: {why}\n",
        k.name, k.pid, k.mb
    )
}

/// A long-running sandboxed program that talks to the host over its standard
/// input and output (an MCP server). Same sandbox as a command: hidden paths
/// denied, writes limited to the workspace and scratch directory, network as
/// the spec says. Its whole process tree is killed when it is stopped or
/// dropped.
pub struct Process {
    child: tokio::process::Child,
    pid: Option<u32>,
    stdin: Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    workspace: PathBuf,
    reserved: Vec<Reserved>,
    marker: Option<PathBuf>,
    fifo: Option<PathBuf>,
    /// `.git` and `.declass` entries it creates are removed when it stops (bubblewrap).
    sweep: bool,
    /// Holds its tree to the memory limits while it runs.
    governor: Option<governor::Governor>,
}

/// The standard streams of a [`Process`]: its input, output and error output.
pub type Streams = (
    Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
    tokio::process::ChildStdout,
    tokio::process::ChildStderr,
);

/// Starts `argv` in `cwd` under the sandbox as a [`Process`]. `spec.timeout`,
/// `output_cap` and `spill_file` do not apply (the caller bounds each exchange).
pub async fn spawn(
    kind: SandboxKind,
    spec: &Spec,
    argv: &[String],
    cwd: &Path,
) -> Result<Process, SandboxError> {
    let program = argv.first().cloned().unwrap_or_default();
    let (spawned, spec, reserved, marker, fifo, pipe) = {
        let mut prepared = prepare(kind, Path::new(BWRAP), spec, argv, cwd, true)?;
        let fifo = prepared.fifo.take();
        // The host holds the named pipe open for reading and writing, so the
        // server's open of it never waits and its input ends when the host closes it.
        #[cfg(target_os = "linux")]
        let pipe: Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>> = match &fifo {
            Some(f) => Some(Box::new(
                tokio::net::unix::pipe::OpenOptions::new()
                    .read_write(true)
                    .open_sender(f)
                    .map_err(|e| SandboxError::Spawn(format!("input pipe: {e}")))?,
            )),
            None => None,
        };
        #[cfg(not(target_os = "linux"))]
        let pipe: Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>> = None;
        let spawned = prepared.start();
        let Prepared {
            spec,
            reserved,
            marker,
            ..
        } = prepared;
        (spawned, spec, reserved, marker, fifo, pipe)
    };
    let mut child = spawned.map_err(|e| match kind {
        SandboxKind::Bubblewrap => SandboxError::Unavailable(format!("cannot start {BWRAP}: {e}")),
        SandboxKind::Seatbelt => SandboxError::Spawn(format!("{program}: {e}")),
    })?;
    let stdin = match pipe {
        Some(p) => Some(p),
        None => child
            .stdin
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>),
    };
    let pid = child.id();
    Ok(Process {
        governor: pid.map(|p| governor::Governor::watch(p, governor::limits())),
        pid,
        stdout: child.stdout.take(),
        stderr: child.stderr.take(),
        child,
        stdin,
        workspace: spec.workspace,
        reserved,
        marker,
        fifo,
        sweep: kind == SandboxKind::Bubblewrap,
    })
}

impl Process {
    /// Its standard streams (once).
    pub fn take_streams(&mut self) -> Option<Streams> {
        Some((self.stdin.take()?, self.stdout.take()?, self.stderr.take()?))
    }

    /// Whether the sandbox was fully set up before the program started (under
    /// bubblewrap, known once the program has run; always true under Seatbelt).
    pub fn sandboxed(&self) -> bool {
        self.marker.as_ref().is_none_or(|m| m.exists())
    }

    /// Its exit code once it has exited (`Some(None)`: killed by a signal).
    pub fn exited(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|s| s.code())
    }

    /// Stops it: waits up to `grace` for it to exit (its input should be
    /// closed first), then kills its whole tree. Returns the `.git` and
    /// `.declass` entries it created, which are removed (bubblewrap).
    pub async fn stop(&mut self, grace: Duration) -> Vec<PathBuf> {
        let _ = tokio::time::timeout(grace, self.child.wait()).await;
        if let Some(pid) = self.pid.take() {
            kill_tree(pid);
        }
        let _ = self.child.kill().await;
        if let Some(g) = self.governor.take() {
            let _ = tokio::task::spawn_blocking(move || g.finish()).await;
        }
        for p in [self.marker.take(), self.fifo.take()].into_iter().flatten() {
            let _ = std::fs::remove_file(p);
        }
        if !std::mem::take(&mut self.sweep) {
            return Vec::new();
        }
        remove_new_reserved(&self.workspace, &std::mem::take(&mut self.reserved))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            kill_tree(pid);
        }
        if let Some(g) = self.governor.take() {
            g.abandon();
        }
        for p in [self.marker.take(), self.fifo.take()].into_iter().flatten() {
            let _ = std::fs::remove_file(p);
        }
    }
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

/// Stops the discovered tree from root down, then kills it from leaves up.
/// Freezing ancestors first prevents a waiting shell from running its next
/// command when its child is killed. Rescans include forks racing the first
/// snapshot; the bounded parent-link walk cannot recover processes already
/// reparented outside the tree before it starts.
pub fn kill_tree(root: u32) {
    use std::collections::{HashMap, HashSet};
    let signal = |pid: u32, signal| {
        if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
            let _ = rustix::process::kill_process(p, signal);
        }
    };
    let mut victims = vec![root];
    let mut seen = HashSet::from([root]);
    let mut stopped = 0;
    // Freeze before each rescan. A process racing a fork cannot keep the
    // caller scanning indefinitely; any last discovered members are stopped
    // below before the kill pass as well.
    for _ in 0..8 {
        for &pid in &victims[stopped..] {
            signal(pid, rustix::process::Signal::Stop);
        }
        stopped = victims.len();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, parent) in process_table() {
            children.entry(parent).or_default().push(pid);
        }
        let mut i = 0;
        while i < victims.len() {
            if let Some(descendants) = children.get(&victims[i]) {
                victims.extend(descendants.iter().copied().filter(|pid| seen.insert(*pid)));
            }
            i += 1;
        }
        if victims.len() == stopped {
            break;
        }
    }
    for &pid in &victims[stopped..] {
        signal(pid, rustix::process::Signal::Stop);
    }
    for pid in victims.into_iter().rev() {
        signal(pid, rustix::process::Signal::Kill);
    }
}

/// Behavioural tests against the real platform sandbox: Seatbelt on macOS,
/// bubblewrap on Linux (run them there with `tools/linux-check.sh`).
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn command_denials_match_native_and_runtime_error_casing() {
        let mut output = Output {
            exit_code: Some(1),
            timed_out: false,
            interrupted: false,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_total: 0,
            stderr_total: 0,
            spilled_to: None,
            removed_reserved: Vec::new(),
            memory_kills: Vec::new(),
            duration: Duration::ZERO,
        };
        for message in [
            "Operation not permitted",
            "Error: EPERM: operation not permitted, open '/tmp/maps.json'",
            "Permission denied",
            "permission denied (os error 13)",
        ] {
            output.stderr = message.as_bytes().to_vec();
            assert!(output.shows_denial(), "{message}");
            output.stdout = std::mem::take(&mut output.stderr);
            assert!(output.shows_denial(), "stdout: {message}");
            output.stdout.clear();
        }
        output.stderr = b"AssertionError: expected 4, got 3".to_vec();
        assert!(!output.shows_denial());
    }

    const KIND: SandboxKind = if cfg!(target_os = "linux") {
        SandboxKind::Bubblewrap
    } else {
        SandboxKind::Seatbelt
    };

    fn spec(ws: &Path) -> Spec {
        Spec {
            workspace: ws.to_path_buf(),
            scratch: ws.parent().unwrap().join("scratch"),
            network: Network::Off,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: Some(ws.parent().unwrap().join("spill.txt")),
            extra_env: vec![],
            deny_read: Vec::new(),
            read_only: false,
        }
    }

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap().join("ws");
        std::fs::create_dir_all(ws.join(".git")).unwrap();
        std::fs::create_dir_all(ws.join("vendor/dep/.git")).unwrap();
        std::fs::create_dir_all(ws.join(".declass")).unwrap();
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
    async fn a_read_only_command_reads_the_workspace_and_writes_only_its_scratch() {
        let (_d, ws) = setup();
        std::fs::write(ws.join("a.txt"), "original\n").unwrap();
        let s = Spec {
            read_only: true,
            ..spec(&ws)
        };
        let o = sh_with(
            &s,
            "cat a.txt; echo changed > a.txt; echo new > b.txt; mkdir d; rm a.txt; \
             echo tmp > \"$TMPDIR/t\" && cat \"$TMPDIR/t\"",
        )
        .await;
        let out = text(&o);
        assert!(out.contains("original") && out.contains("tmp"), "{out}");
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).unwrap(),
            "original\n"
        );
        assert!(!ws.join("b.txt").exists() && !ws.join("d").exists());
        // The same command without the flag writes the workspace.
        sh(&ws, "echo new > b.txt").await;
        assert!(ws.join("b.txt").exists());
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
    async fn a_server_process_talks_over_its_streams_under_the_same_sandbox() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (_d, ws) = setup();
        std::fs::write(ws.join(".env"), "TOKEN=Qx7pL2mN9vR4\n").unwrap();
        let mut s = spec(&ws);
        s.deny_read = vec![ws.join(".env")];
        // Echoes each line it reads, prefixed with what reading `.env` gave.
        let script = "while IFS= read -r l; do v=$(cat .env 2>&1); echo \"$l|$v\"; done";
        let mut p = spawn(
            KIND,
            &s,
            &["/bin/sh".into(), "-c".into(), script.into()],
            &ws,
        )
        .await
        .unwrap();
        let (mut input, output, _err) = p.take_streams().unwrap();
        let mut lines = BufReader::new(output).lines();
        for n in 0..2 {
            input.write_all(format!("m{n}\n").as_bytes()).await.unwrap();
            input.flush().await.unwrap();
            let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(line.starts_with(&format!("m{n}|")), "{line}");
            assert!(
                !line.contains("Qx7p") && line.contains(DENIAL_MESSAGE),
                "{line}"
            );
        }
        assert!(p.sandboxed());
        // Closing its input ends it.
        drop(input);
        assert!(p.stop(Duration::from_secs(5)).await.is_empty());
        assert!(p.exited().is_some());
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
        std::fs::write(ws.join(".declass/vault.json"), "{\"v\":\"9083\"}").unwrap();
        std::fs::write(ws.join("vendor/dep/.git/HEAD"), "ref: 7730\n").unwrap();
        std::fs::write(ws.join(".git/zz-listed-name"), "").unwrap();
        let mut s = spec(&ws);
        s.deny_read = vec![
            ws.join(".git"),
            ws.join(".declass"),
            ws.join("vendor/dep/.git"),
        ];
        let o = sh_with(
            &s,
            "cat .git/config; od -c .declass/vault.json; ls .git .declass; cat vendor/dep/.git/HEAD; \
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
    async fn run_state_is_unreadable_even_when_nothing_is_denied() {
        // A command allowed to read sensitive data gets no deny list; it must
        // still not reach the vault, the transcripts or the audit log.
        let (_d, ws) = setup();
        std::fs::create_dir_all(ws.join(".declass/runs/r1")).unwrap();
        std::fs::create_dir_all(ws.join("sub/.declass")).unwrap();
        std::fs::write(ws.join(".declass/runs/r1/vault.json"), "{\"v\":\"4539\"}").unwrap();
        std::fs::write(ws.join("sub/.declass/audit.jsonl"), "6121\n").unwrap();
        std::fs::write(ws.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
        let o = sh(
            &ws,
            "cat .declass/runs/r1/vault.json; ls -R .declass; cat sub/.declass/audit.jsonl; \
             grep -r 4539 . ; echo x >> sub/.declass/audit.jsonl; cat .git/config",
        )
        .await;
        let all = text(&o);
        for secret in ["4539", "6121", "r1:"] {
            assert!(!all.contains(secret), "{secret} readable: {all}");
        }
        assert!(o.shows_denial(), "{all}");
        assert_eq!(
            std::fs::read_to_string(ws.join("sub/.declass/audit.jsonl")).unwrap(),
            "6121\n"
        );
        // Git metadata stays readable unless the caller denies it.
        assert!(all.contains("bare = false"), "{all}");
        // A long-lived server process (MCP, language server) is held to the same rule.
        use tokio::io::AsyncReadExt;
        let mut p = spawn(
            KIND,
            &spec(&ws),
            &[
                "/bin/sh".into(),
                "-c".into(),
                "cat .declass/runs/r1/vault.json 2>&1".into(),
            ],
            &ws,
        )
        .await
        .unwrap();
        let (input, mut output, _err) = p.take_streams().unwrap();
        let mut served = String::new();
        tokio::time::timeout(Duration::from_secs(10), output.read_to_string(&mut served))
            .await
            .unwrap()
            .unwrap();
        drop(input);
        p.stop(Duration::from_secs(5)).await;
        assert!(
            !served.contains("4539") && served.contains(DENIAL_MESSAGE),
            "{served}"
        );
    }

    #[tokio::test]
    async fn credential_stores_in_the_home_directory_are_unreadable() {
        let (_d, ws) = setup();
        // Inside the workspace: bubblewrap hides the rest of /tmp.
        let home = ws.join("home");
        for (p, text) in [
            (".npmrc", "//registry.npmjs.org/:_authToken=npm_Tk7Qx2Lp9\n"),
            (
                ".cargo/credentials.toml",
                "[registry]\ntoken = \"cio_Rz4Wm8\"\n",
            ),
            (".cargo/config.toml", "[build]\njobs = 3\n"),
            (".ssh/id_ed25519", "PRIVATE-KEY-5521\n"),
            (".zsh_history", "export OPENAI_API_KEY=sk-proj-Vb61\n"),
        ] {
            std::fs::create_dir_all(home.join(p).parent().unwrap()).unwrap();
            std::fs::write(home.join(p), text).unwrap();
        }
        let mut s = spec(&ws);
        s.deny_read = home_secrets_in(&home, &ws);
        assert_eq!(s.deny_read.len(), 4, "{:?}", s.deny_read);
        let o = sh_with(
            &s,
            &format!(
                "cd {}; cat .npmrc .cargo/credentials.toml .ssh/id_ed25519 .zsh_history; \
                 ls .ssh; cat .cargo/config.toml",
                home.display()
            ),
        )
        .await;
        let all = text(&o);
        for secret in [
            "npm_Tk7Qx2Lp9",
            "cio_Rz4Wm8",
            "PRIVATE-KEY-5521",
            "sk-proj-Vb61",
            "id_ed25519\n",
        ] {
            assert!(!all.contains(secret), "{secret} readable: {all}");
        }
        // Configuration next to them stays readable.
        assert!(all.contains("jobs = 3"), "{all}");
        // A workspace inside a listed directory is not denied with it.
        assert!(
            home_secrets_in(&home, &home.join(".ssh/work"))
                .iter()
                .all(|p| !p.ends_with(".ssh"))
        );
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
            sh(&ws, "echo x > \"$HOME/.declass-sandbox-probe\"")
                .await
                .exit_code,
            Some(0)
        );
        assert!(
            !Path::new(&std::env::var("HOME").unwrap())
                .join(".declass-sandbox-probe")
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
        for target in [".git/config", "vendor/dep/.git/HEAD", ".declass/x"] {
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
        for target in ["nested/.git/new", "deep/er/.declass/x"] {
            let o = sh(
                &ws,
                &format!("mkdir -p \"$(dirname {target})\" 2>/dev/null; echo x > {target}"),
            )
            .await;
            if cfg!(target_os = "macos") {
                assert_ne!(o.exit_code, Some(0), "{target} was writable");
            } else {
                assert_eq!(o.removed_reserved.len(), 1, "{:?}", o.removed_reserved);
                assert!(text(&o).contains("may not create .git or .declass"));
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
        assert!(ws.join(".git").is_dir() && ws.join(".declass").is_dir());
    }

    #[tokio::test]
    async fn network_is_denied_by_default() {
        let (_d, ws) = setup();
        for url in ["https://example.com", "http://1.1.1.1"] {
            let o = sh(&ws, &format!("curl -s -m 5 -o /dev/null {url}")).await;
            assert_ne!(o.exit_code, Some(0), "{url}");
        }
    }

    /// Even when declass runs as root, commands hold no capabilities, so they
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
        s.network = Network::All;
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
    async fn a_runaway_process_is_stopped_and_the_output_says_why() {
        let (_d, ws) = setup();
        let s = spec(&ws);
        // `dd` holds a 300 MB block two levels below the command while `sleep`
        // leaves it blocked on the pipe; the shells around it are small.
        let script = "/bin/sh -c 'dd if=/dev/zero bs=314572800 count=1 2>/dev/null | sleep 5'; echo survived";
        let limits = governor::MemoryLimits {
            process_mb: 64,
            total_mb: 10_000,
        };
        let o = run_with(
            KIND,
            Path::new(BWRAP),
            &s,
            &["/bin/sh".into(), "-c".into(), script.into()],
            &ws,
            std::future::pending(),
            limits,
        )
        .await
        .unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(
            String::from_utf8_lossy(&o.stdout).trim(),
            "survived",
            "{err}"
        );
        assert_eq!(o.exit_code, Some(0), "the command itself goes on: {err}");
        assert_eq!(o.memory_kills.len(), 1, "{:?}", o.memory_kills);
        let k = &o.memory_kills[0];
        assert_eq!(
            (k.name.as_str(), k.rule),
            ("dd", governor::KillRule::ProcessLimit)
        );
        assert!(k.mb > 64, "{k:?}");
        assert!(
            err.contains(&format!(
                "[sandbox] stopped dd (pid {}) at {} MB of memory: over the limit of 64 MB per process (limits.process_memory_mb).",
                k.pid, k.mb
            )),
            "{err}"
        );
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
            network: Network::Off,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
            read_only: false,
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
            governor::limits(),
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
        if std::env::var_os("DECLASS_EXPECT_NO_NAMESPACES").is_none() {
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
            network: Network::Off,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
            read_only: false,
        };
        for dir in ["ws/.git", "ws/vendor/dep/.git", "ws/.declass", "ws/data"] {
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
            [".declass", ".git", "vendor/dep/.git"].map(|p| s.workspace.join(p))
        );
        let joined = bwrap_args(&s, &reserved, &stubs)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let ws = s.workspace.display();
        assert!(joined.contains("--cap-drop ALL") && joined.contains("--tmpfs /run"));
        assert!(joined.contains(&format!("--ro-bind {ws}/.git {ws}/.git")));
        assert!(joined.contains(&format!("--ro-bind {} {ws}/.declass", stubs.dir.display())));
        assert!(joined.contains(&format!(
            "--ro-bind {ws}/vendor/dep/.git {ws}/vendor/dep/.git"
        )));
        assert!(joined.contains(&format!("--ro-bind {} {ws}/data", stubs.dir.display())));
        assert!(joined.contains(&format!("--ro-bind {} {ws}/.env", stubs.file.display())));
        assert!(!joined.contains("absent"));
        assert!(!joined.contains("--share-net"));
        assert!(joined.contains(&format!("--bind {ws} {ws}")));

        // Read-only: the workspace is mounted read-only; scratch stays writable.
        s.read_only = true;
        let joined = bwrap_args(&s, &reserved, &stubs)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains(&format!("--ro-bind {ws} {ws}")), "{joined}");
        assert!(!joined.contains(&format!("--bind {ws} {ws}")), "{joined}");
        let scratch = s.scratch.display();
        assert!(joined.contains(&format!("--bind {scratch} {scratch}")));
    }
}

#[cfg(all(test, unix))]
mod network_rule_tests {
    use super::*;

    fn spec(root: &Path, network: Network) -> Spec {
        Spec {
            workspace: root.join("ws"),
            scratch: root.join("scratch"),
            network,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
            read_only: false,
        }
    }

    fn network_lines(profile: &str) -> Vec<&str> {
        profile.lines().filter(|l| l.contains("network")).collect()
    }

    #[tokio::test]
    async fn seatbelt_rules_follow_the_network_mode() {
        let root = Path::new("/private/tmp/declass-rules");
        let off = seatbelt_profile(&spec(root, Network::Off), None).unwrap();
        assert!(network_lines(&off).is_empty(), "{off}");
        let all = seatbelt_profile(&spec(root, Network::All), None).unwrap();
        assert_eq!(network_lines(&all), ["(allow network*)"]);

        let proxied = spec(root, Network::Proxy(ProxyRoute::Loopback { port: 41000 }));
        // Only allow rules: Seatbelt does not reliably honour a port deny.
        let p = seatbelt_profile(&proxied, Some(&[3000, 8080, 41000])).unwrap();
        let lines = network_lines(&p);
        assert!(
            lines.iter().all(|l| l.starts_with("(allow network-")),
            "{p}"
        );
        let outbound = lines
            .iter()
            .find(|l| l.starts_with("(allow network-outbound"))
            .unwrap();
        for open in ["localhost:41000\"", "localhost:3001\"", "localhost:5173\""] {
            assert!(outbound.contains(open), "{open}: {outbound}");
        }
        // The host's ports stay closed; nothing but loopback TCP opens.
        for closed in [
            "localhost:3000\"",
            "localhost:8080\"",
            "localhost:5432\"",
            "*:",
        ] {
            assert!(!outbound.contains(closed), "{closed}: {outbound}");
        }
        assert!(!p.contains("remote ip") && !p.contains("remote udp"), "{p}");
        // Unknown host ports: the proxy only.
        let p = seatbelt_profile(&proxied, None).unwrap();
        assert!(p.contains("(allow network-outbound (remote tcp \"localhost:41000\"))"));
        // The bridge is bubblewrap's.
        let (route, _incoming) = bridge::channel(vec!["/bin/true".into()]).unwrap();
        assert!(seatbelt_profile(&spec(root, Network::Proxy(route)), None).is_err());
    }

    #[tokio::test]
    async fn bwrap_keeps_its_network_namespace_with_the_bridge() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("ws")).unwrap();
        let stubs = DenyStubs::ensure(&root).unwrap();
        let join = |s: &Spec| {
            bwrap_args(s, &[], &stubs)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (route, _incoming) = bridge::channel(vec!["/opt/declass".into(), "x".into()]).unwrap();
        let bridged = join(&spec(&root, Network::Proxy(route)));
        assert!(!bridged.contains("--share-net"), "{bridged}");
        assert!(bridged.contains(&format!("--ro-bind /opt/declass {}", bridge::HELPER_PATH)));
        assert!(!bridged.contains("/run/systemd/resolve"), "{bridged}");
        let all = join(&spec(&root, Network::All));
        assert!(all.contains("--share-net") && all.contains("/run/systemd/resolve"));
        assert!(!join(&spec(&root, Network::Off)).contains("--share-net"));
    }

    #[test]
    fn proxy_variables_point_at_loopback_and_spare_it() {
        let env: std::collections::BTreeMap<_, _> = proxy_env(4321).into_iter().collect();
        for k in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "ALL_PROXY",
            "CARGO_HTTP_PROXY",
        ] {
            assert_eq!(env[k], "http://127.0.0.1:4321", "{k}");
        }
        assert_eq!(env["NO_PROXY"], "localhost,127.0.0.1,::1");
        assert_eq!(env["npm_config_https_proxy"], "http://127.0.0.1:4321");
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
            network: Network::Off,
            timeout: Duration::from_secs(240),
            output_cap: 64 * 1024,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
            read_only: false,
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
            network: Network::Off,
            timeout: Duration::from_secs(240),
            output_cap: 64 * 1024,
            spill_file: None,
            extra_env: vec![],
            deny_read: Vec::new(),
            read_only: false,
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
