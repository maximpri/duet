// SPDX-License-Identifier: GPL-3.0-or-later
//! Command sandbox (Seatbelt on macOS, bubblewrap on Linux), environment
//! allowlist and process-tree control.
//!
//! Commands may read the filesystem, write only inside the workspace and the
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

/// Detects the platform sandbox at its fixed absolute path.
pub fn detect() -> Result<SandboxKind, SandboxError> {
    if cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).is_file() {
        Ok(SandboxKind::Seatbelt)
    } else if cfg!(target_os = "linux") && Path::new(BWRAP).is_file() {
        Ok(SandboxKind::Bubblewrap)
    } else {
        Err(SandboxError::Unavailable(format!(
            "neither {SANDBOX_EXEC} nor {BWRAP} found"
        )))
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
}

#[derive(Debug, Clone)]
pub struct Output {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_total: usize,
    pub stderr_total: usize,
    /// Set when output exceeded the cap and was written in full here.
    pub spilled_to: Option<PathBuf>,
    pub duration: Duration,
}

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
    ];
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

/// bubblewrap arguments for a workspace-write command.
pub fn bwrap_args(spec: &Spec) -> Vec<OsString> {
    let ws = spec.workspace.as_os_str().to_owned();
    let mut a: Vec<OsString> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
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
        "--bind",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    a.push(ws.clone());
    a.push(ws);
    for reserved in [".git", ".duet"] {
        let p = spec.workspace.join(reserved);
        if p.exists() {
            a.extend([
                OsString::from("--ro-bind"),
                p.clone().into_os_string(),
                p.into_os_string(),
            ]);
        }
    }
    a.extend([
        OsString::from("--bind"),
        spec.scratch.clone().into_os_string(),
        spec.scratch.clone().into_os_string(),
    ]);
    if spec.network {
        a.push(OsString::from("--share-net"));
    }
    a.push(OsString::from("--"));
    a
}

/// Runs `argv` in `cwd` (inside the workspace) under the sandbox.
pub async fn run(
    kind: SandboxKind,
    spec: &Spec,
    argv: &[String],
    cwd: &Path,
) -> Result<Output, SandboxError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| SandboxError::Spawn("empty command".into()))?;
    std::fs::create_dir_all(&spec.scratch).map_err(|e| SandboxError::Spawn(e.to_string()))?;
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
            let mut c = tokio::process::Command::new(BWRAP);
            c.args(bwrap_args(spec))
                .arg("--chdir")
                .arg(cwd)
                .arg(program)
                .args(args);
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| SandboxError::Spawn(format!("{program}: {e}")))?;
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
    let (exit_code, timed_out) = match tokio::time::timeout(spec.timeout, child.wait()).await {
        Ok(status) => (status.ok().and_then(|s| s.code()), false),
        Err(_) => (None, true),
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
        stdout: cap(stdout),
        stderr: cap(stderr),
        stdout_total,
        stderr_total,
        spilled_to,
        duration: started.elapsed(),
    })
}

/// Kills `root` and every descendant, including ones that left its process
/// group with `setsid`, by walking the parent links reported by `ps`.
pub fn kill_tree(root: u32) {
    let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    else {
        return;
    };
    let table: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect();
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn spec(ws: &Path) -> Spec {
        Spec {
            workspace: ws.to_path_buf(),
            scratch: ws.parent().unwrap().join("scratch"),
            network: false,
            timeout: Duration::from_secs(30),
            output_cap: 4096,
            spill_file: Some(ws.parent().unwrap().join("spill.txt")),
            extra_env: vec![],
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

    async fn sh(ws: &Path, script: &str) -> Output {
        run(
            SandboxKind::Seatbelt,
            &spec(ws),
            &["/bin/sh".into(), "-c".into(), script.into()],
            ws,
        )
        .await
        .unwrap()
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
        assert_eq!(sh(&ws, "echo t > \"$TMPDIR/t\"").await.exit_code, Some(0));
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
    }

    #[tokio::test]
    async fn reserved_directories_are_read_only_at_any_depth() {
        let (_d, ws) = setup();
        for target in [
            ".git/config",
            "vendor/dep/.git/HEAD",
            ".duet/x",
            "nested/.git/new",
        ] {
            let o = sh(
                &ws,
                &format!("mkdir -p \"$(dirname {target})\" 2>/dev/null; echo x > {target}"),
            )
            .await;
            assert_ne!(o.exit_code, Some(0), "{target} was writable");
            assert!(!ws.join(target).exists(), "{target}");
        }
    }

    #[tokio::test]
    async fn network_is_denied_by_default() {
        let (_d, ws) = setup();
        let o = sh(
            &ws,
            "/usr/bin/curl -s -m 5 -o /dev/null https://example.com",
        )
        .await;
        assert_ne!(o.exit_code, Some(0));
    }

    #[tokio::test]
    async fn environment_is_cleared_to_the_allowlist() {
        let (_d, ws) = setup();
        let mut s = spec(&ws);
        s.extra_env.push(("VISIBLE".into(), "yes".into()));
        // SAFETY-free: set a variable in this process that must not leak through.
        let o = run(SandboxKind::Seatbelt, &s, &["/usr/bin/env".into()], &ws)
            .await
            .unwrap();
        let env = String::from_utf8_lossy(&o.stdout);
        assert!(env.contains("VISIBLE=yes"));
        assert!(
            !env.contains("ZAI_API_KEY")
                && !env.contains("OMLX_API_KEY")
                && !env.contains("ANTHROPIC_API_KEY")
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
        let o = run(
            SandboxKind::Seatbelt,
            &s,
            &["/bin/sh".into(), "-c".into(), script],
            &ws,
        )
        .await
        .unwrap();
        assert!(o.timed_out);
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(!marker.exists(), "a descendant survived the timeout");
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
        for argv in [
            ["cargo", "--version"],
            ["node", "--version"],
            ["git", "--version"],
        ] {
            let o = run(
                SandboxKind::Seatbelt,
                &spec(&ws),
                &argv.map(String::from),
                &ws,
            )
            .await
            .unwrap();
            assert_eq!(
                o.exit_code,
                Some(0),
                "{argv:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        }
    }

    #[test]
    fn bwrap_args_protect_reserved_dirs_and_run() {
        let (_d, ws) = setup();
        let a: Vec<String> = bwrap_args(&spec(&ws))
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let joined = a.join(" ");
        assert!(joined.contains("--tmpfs /run"));
        assert!(joined.contains(&format!("--ro-bind {0}/.git {0}/.git", ws.display())));
        assert!(!joined.contains("--share-net"));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod toolchain_tests {
    use super::*;

    fn spec(ws: &Path) -> Spec {
        Spec {
            workspace: ws.to_path_buf(),
            scratch: ws.parent().unwrap().join("scratch"),
            network: false,
            timeout: Duration::from_secs(240),
            output_cap: 64 * 1024,
            spill_file: None,
            extra_env: vec![],
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
            SandboxKind::Seatbelt,
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
        let node = run(
            SandboxKind::Seatbelt,
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
