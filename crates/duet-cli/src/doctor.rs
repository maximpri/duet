// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet doctor`: one line per check (pass, warn, fail or skip) with a fix for
//! anything short of pass. Offline by default: it reads configuration, the
//! environment and local files only. `--online` also asks the configured
//! servers for their model listings and context windows; it never calls a model
//! and never prints a credential.

use crate::{config_audit_path, run_anchor};
use duet_boundary::audit::{AnchorCheck, Verification, check_anchor, verify};
use duet_config::{Config, Origin, REGISTRY};
use duet_provider::backends;
use duet_provider::endpoint::{Trust, check_local_endpoint, host_port, is_loopback_host};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

/// Smallest local context window that fits a reading chunk (about 33K tokens),
/// the prompt and the answer.
pub const MIN_LOCAL_CONTEXT: u64 = 40_960;
/// Free space below which runs may fail (warn) or will fail (fail).
const DISK_WARN: u64 = 5 << 30;
const DISK_FAIL: u64 = 1 << 30;
/// How many of the most recent run audit logs are verified.
const RECENT_RUNS: usize = 5;
const ONLINE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Skip,
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

fn check(name: &'static str, status: Status, detail: impl Into<String>) -> Check {
    Check {
        name,
        status,
        detail: detail.into(),
        fix: None,
    }
}

impl Check {
    fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

pub fn worst(checks: &[Check]) -> Status {
    checks
        .iter()
        .map(|c| c.status)
        .max()
        .unwrap_or(Status::Pass)
        .max(Status::Pass)
}

/// 0 when nothing is worse than pass, 1 for a warning, 2 for a failure.
pub fn exit_code(checks: &[Check]) -> i32 {
    match worst(checks) {
        Status::Skip | Status::Pass => 0,
        Status::Warn => 1,
        Status::Fail => 2,
    }
}

pub fn render_text(checks: &[Check], online: bool) -> String {
    let mut out = format!(
        "duet doctor ({})\n\n",
        if online {
            "online: contacted the configured servers for listings only"
        } else {
            "offline: no network; --online also checks the servers"
        }
    );
    for c in checks {
        let tag = match c.status {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
        };
        out.push_str(&format!("{tag}  {:<16} {}\n", c.name, c.detail));
        if let Some(f) = &c.fix {
            for (i, line) in f.lines().enumerate() {
                let lead = if i == 0 { "fix:" } else { "    " };
                out.push_str(&format!("      {lead} {line}\n"));
            }
        }
    }
    out.push_str(&format!(
        "\nresult: {}\n",
        format!("{:?}", worst(checks)).to_lowercase()
    ));
    out
}

pub fn render_json(checks: &[Check], online: bool) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "online": online,
        "result": worst(checks),
        "checks": checks,
    }))
    .unwrap_or_default()
}

/// Runs every check against the workspace `ws`.
pub async fn run(ws: &Path, online: bool) -> Vec<Check> {
    let mut out = vec![check(
        "version",
        Status::Pass,
        format!(
            "duet {} ({}-{}); release channel not published yet, so no update check",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    )];
    let owner = duet_config::owner_config_path();
    let project = ws.join(".duet/config.toml");
    let cfg = match Config::load(&owner, Some(&project)) {
        Ok(c) => {
            out.push(config_loaded(&c, &owner, &project));
            Some(c)
        }
        Err(e) => {
            out.push(check("config", Status::Fail, e.to_string()).fix(format!(
                "edit {} or {}; `duet config list` shows every valid key (a project may only tighten)",
                owner.display(),
                project.display()
            )));
            None
        }
    };
    out.push(config_audit());
    match &cfg {
        Some(c) => {
            out.push(posture(c));
            out.push(approval(c));
            out.extend(frontier(c, online).await);
            out.extend(local(c, online).await);
        }
        None => out.push(check(
            "endpoints",
            Status::Skip,
            "configuration did not load",
        )),
    }
    out.push(release_signers());
    out.push(sandbox());
    out.extend(git(ws));
    out.push(disk(ws));
    out.push(audit(ws));
    if let Some(c) = &cfg {
        out.push(retention(ws, c));
    }
    out
}

fn config_loaded(c: &Config, owner: &Path, project: &Path) -> Check {
    let count = |o| {
        REGISTRY
            .iter()
            .filter(|s| c.origin(s.key) == Some(o))
            .count()
    };
    let file = |p: &Path, n: usize| {
        if p.exists() {
            format!("{} ({n} set)", p.display())
        } else {
            format!("{} (none)", p.display())
        }
    };
    let detail = format!(
        "owner {}; project {}",
        file(owner, count(Origin::Owner)),
        file(project, count(Origin::Project))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(owner)
            && m.permissions().mode() & 0o077 != 0
        {
            return check(
                "config",
                Status::Warn,
                format!("{detail}; the owner config is readable by other users"),
            )
            .fix(format!("chmod 600 {}", owner.display()));
        }
    }
    check("config", Status::Pass, detail)
}

fn config_audit() -> Check {
    let path = config_audit_path();
    if !path.exists() {
        return check("config audit", Status::Pass, "no settings changes recorded");
    }
    match verify(&path) {
        Ok(Verification::Intact { records }) => check(
            "config audit",
            Status::Pass,
            format!("{records} change(s), chain intact ({})", path.display()),
        ),
        Ok(Verification::Broken { at_seq, reason }) => check(
            "config audit",
            Status::Fail,
            format!("chain broken at record {at_seq}: {reason}"),
        )
        .fix(format!(
            "{} was altered; review the owner config by hand (`duet config list`) before trusting it",
            path.display()
        )),
        Err(e) => check("config audit", Status::Fail, e.to_string())
            .fix(format!("check the permissions of {}", path.display())),
    }
}

/// Settings the owner or project moved against their privacy direction.
fn posture(c: &Config) -> Check {
    let loosened: Vec<String> = REGISTRY
        .iter()
        .filter(|s| s.direction != duet_config::Direction::Any)
        .filter(|s| c.origin(s.key) != Some(Origin::Default))
        .filter_map(|s| {
            let default = toml::from_str::<toml::Table>(&format!("v = {}", s.default))
                .ok()?
                .remove("v")?;
            let now = c.value(s.key).ok()?;
            duet_config::loosening(s, &default, now).map(|_| format!("{} = {now}", s.key))
        })
        .collect();
    if loosened.is_empty() {
        return check(
            "privacy policy",
            Status::Pass,
            "no setting is looser than its default",
        );
    }
    check(
        "privacy policy",
        Status::Warn,
        format!(
            "looser than the defaults (each change is in the config audit log): {}",
            loosened.join(", ")
        ),
    )
    .fix("review with `duet config list`; restore a default with `duet config set <key> <default>`")
}

/// Whether the operator approves risky actions (`oversight.approve`).
fn approval(c: &Config) -> Check {
    match c.str("oversight.approve").unwrap_or_default().as_str() {
        "off" => check(
            "approval",
            Status::Pass,
            "off: actions run without asking (set oversight.approve to risky or all to be asked at the terminal)",
        ),
        "risky" => check(
            "approval",
            Status::Pass,
            "risky: sensitive_data commands, edit_protected and writes outside source/test files wait for y/N; runs need a terminal",
        ),
        other => check(
            "approval",
            Status::Pass,
            format!("{other}: every command and write waits for y/N; runs need a terminal"),
        ),
    }
}

/// Where the owner keeps the public keys trusted to sign releases.
pub fn release_signers_path() -> std::path::PathBuf {
    duet_config::owner_config_path().with_file_name("allowed_signers")
}

/// Whether release verification material is present (warn-only).
fn release_signers() -> Check {
    let path = release_signers_path();
    let fix = format!(
        "save the published release signer line (`<identity> namespaces=\"duet-release\" <public key>`) to {}, then check a download with tools/verify-release.sh <release dir>",
        path.display()
    );
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let signers = text
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .count();
            if signers == 0 {
                check(
                    "release keys",
                    Status::Warn,
                    format!("{} lists no signer", path.display()),
                )
                .fix(fix)
            } else {
                check(
                    "release keys",
                    Status::Pass,
                    format!(
                        "{signers} release signer(s) in {} (tools/verify-release.sh uses them)",
                        path.display()
                    ),
                )
            }
        }
        Err(_) => check(
            "release keys",
            Status::Warn,
            format!(
                "no release signing keys at {}: a downloaded release cannot be verified",
                path.display()
            ),
        )
        .fix(fix),
    }
}

async fn frontier(c: &Config, online: bool) -> Vec<Check> {
    let url = c.str("frontier.base_url").unwrap_or_default();
    let model = c.str("frontier.model").unwrap_or_default();
    let mut out = Vec::new();
    let endpoint = match host_port(&url) {
        None => check(
            "frontier",
            Status::Fail,
            format!("cannot parse frontier.base_url {url}"),
        )
        .fix("duet config set frontier.base_url '\"https://...\"' --confirm"),
        Some((host, _)) if url.starts_with("http://") && !is_loopback_host(&host) => check(
            "frontier",
            Status::Fail,
            format!("{url} is plain HTTP: the API key and every request cross the network unencrypted"),
        )
        .fix("use the provider's https:// endpoint: duet config set frontier.base_url '\"https://...\"' --confirm"),
        Some(_) => check("frontier", Status::Pass, format!("{url}, model {model}")),
    };
    out.push(endpoint);
    let key_env = c.str("frontier.api_key_env").unwrap_or_default();
    let key = std::env::var(&key_env).ok().filter(|k| !k.is_empty());
    out.push(if key_env.is_empty() {
        check(
            "frontier key",
            Status::Warn,
            "frontier.api_key_env is empty: requests carry no credential",
        )
        .fix("duet config set frontier.api_key_env '\"<VARIABLE>\"'")
    } else if key.is_some() {
        check(
            "frontier key",
            Status::Pass,
            format!("{key_env} is set (value not shown)"),
        )
    } else {
        check(
            "frontier key",
            Status::Fail,
            format!("{key_env} is not set in this environment"),
        )
        .fix(format!(
            "export {key_env}=<your key> (Duet stores only the variable's name)"
        ))
    });
    out.push(match duet_provider::price::builtin(&model) {
        Some(_) => check(
            "frontier price",
            Status::Pass,
            format!("list price known for {model}"),
        ),
        None => check(
            "frontier price",
            Status::Warn,
            format!("no price known for {model}: limits.frontier_usd cannot be enforced"),
        )
        .fix("use a priced model, or rely on limits.wall_clock_minutes"),
    });
    if !online {
        out.push(check(
            "frontier model",
            Status::Skip,
            "not contacted offline (use --online)",
        ));
        return out;
    }
    out.push(
        match backends::list_models(&url, key.as_deref(), ONLINE_TIMEOUT).await {
            Ok(listing) => {
                let ids = backends::model_ids(&listing);
                if ids.iter().any(|m| m == &model) {
                    check(
                        "frontier model",
                        Status::Pass,
                        format!("reachable; {model} is listed"),
                    )
                } else {
                    check(
                        "frontier model",
                        Status::Warn,
                        format!(
                            "reachable, but {model} is not among the {} listed model(s)",
                            ids.len()
                        ),
                    )
                    .fix("check frontier.model against the provider's model names")
                }
            }
            Err(e) if e == "HTTP 404" || e == "HTTP 405" => check(
                "frontier model",
                Status::Warn,
                "reachable, but the endpoint offers no model listing; the model could not be confirmed",
            ),
            Err(e) if e == "HTTP 401" || e == "HTTP 403" => check(
                "frontier model",
                Status::Fail,
                format!("the provider rejected the key ({e})"),
            )
            .fix(format!("check the key in {key_env}")),
            Err(e) => check(
                "frontier model",
                Status::Fail,
                format!("cannot reach {url}: {e}"),
            )
            .fix("check the network and frontier.base_url"),
        },
    );
    out
}

async fn local(c: &Config, online: bool) -> Vec<Check> {
    let url = c.str("local.base_url").unwrap_or_default();
    let model = c.str("local.model").unwrap_or_default();
    let allowlist = c.list("local.allowlist").unwrap_or_default();
    let allow_plaintext = c.bool("local.allow_plaintext").unwrap_or(false);
    let unconfigured = c.origin("local.base_url") == Some(Origin::Default);
    let mut out = Vec::new();
    let trusted = match check_local_endpoint(&url, &allowlist, allow_plaintext) {
        _ if unconfigured => {
            out.push(
                check(
                    "local endpoint",
                    Status::Warn,
                    format!(
                        "not configured (default {url}); `duet run` will look for a server on loopback ports {:?} and ask before choosing among several",
                        backends::loopback_ports()
                    ),
                )
                .fix("duet config preset <name> --model <id> --confirm   (`duet config preset` lists them)"),
            );
            true
        }
        Ok(Trust::Loopback) => {
            out.push(check(
                "local endpoint",
                Status::Pass,
                format!("{url} is loopback; model {model}"),
            ));
            true
        }
        Ok(Trust::AllowlistedTls) => {
            out.push(check(
                "local endpoint",
                Status::Pass,
                format!("{url} is allowlisted and uses TLS; model {model}"),
            ));
            true
        }
        Ok(Trust::AllowlistedPlaintext) => {
            out.push(
                check(
                    "local endpoint",
                    Status::Warn,
                    format!(
                        "{url} is a remote host over plain HTTP, allowed by local.allow_plaintext: sensitive content crosses the network unencrypted"
                    ),
                )
                .fix("serve it over TLS, or tunnel it to loopback (ssh -N -L), then duet config set local.allow_plaintext false"),
            );
            true
        }
        Err(e) => {
            let fix = if e.message.contains("allowlisted") {
                let hp = host_port(&url)
                    .map(|(h, p)| format!("{h}:{p}"))
                    .unwrap_or_default();
                format!(
                    "use a loopback endpoint (or an SSH tunnel to one), or allowlist the host: duet config set local.allowlist '[\"{hp}\"]' --confirm"
                )
            } else {
                "see the message above".to_owned()
            };
            out.push(check("local endpoint", Status::Fail, e.message).fix(fix));
            false
        }
    };
    let key_env = c.str("local.api_key_env").unwrap_or_default();
    let key = std::env::var(&key_env).ok().filter(|k| !k.is_empty());
    if !key_env.is_empty() && key.is_none() {
        out.push(
            check(
                "local key",
                Status::Fail,
                format!("local.api_key_env names {key_env}, which is not set"),
            )
            .fix(format!("export {key_env}=<server key>")),
        );
    }
    if !online {
        out.push(check(
            "local server",
            Status::Skip,
            "not contacted offline (use --online)",
        ));
        return out;
    }
    if unconfigured {
        let found =
            backends::discover_loopback(&backends::loopback_ports(), Duration::from_millis(1500))
                .await;
        let names: Vec<String> = found
            .iter()
            .map(|s| {
                format!(
                    "{} at {} ({})",
                    s.backend.unwrap_or("OpenAI-compatible server"),
                    s.base_url,
                    s.models.join(", ")
                )
            })
            .collect();
        out.push(if found.is_empty() {
            check(
                "local server",
                Status::Fail,
                "no local server answered on the loopback preset ports",
            )
            .fix("start one (Ollama, LM Studio, llama.cpp, vLLM, oMLX, mlx_lm.server), then `duet config preset <name>`")
        } else {
            check(
                "local server",
                Status::Warn,
                format!("found {}", names.join("; ")),
            )
            .fix("duet config preset <name> --model <id> --confirm")
        });
        return out;
    }
    if !trusted {
        out.push(check(
            "local server",
            Status::Skip,
            "the endpoint is refused, so it was not contacted",
        ));
        return out;
    }
    let listing = match backends::list_models(&url, key.as_deref(), ONLINE_TIMEOUT).await {
        Ok(l) => l,
        Err(e) => {
            out.push(
                check(
                    "local server",
                    Status::Fail,
                    format!("cannot list models at {url}: {e}"),
                )
                .fix("start the server, or point Duet at it: duet config preset <name> --confirm"),
            );
            return out;
        }
    };
    let backend = backends::identify(&url, &listing, key.as_deref(), ONLINE_TIMEOUT)
        .await
        .unwrap_or("OpenAI-compatible server");
    let ids = backends::model_ids(&listing);
    if !ids.iter().any(|m| m == &model) {
        out.push(
            check(
                "local server",
                Status::Fail,
                format!(
                    "{backend} at {url} does not list {model}; it lists: {}",
                    ids.join(", ")
                ),
            )
            .fix(format!(
                "duet config set local.model '\"{}\"'",
                ids.first().map_or("<id>", String::as_str)
            )),
        );
        return out;
    }
    out.push(check(
        "local server",
        Status::Pass,
        format!("{backend} at {url} lists {model}"),
    ));
    out.push(
        match duet_provider::probe::probe_context_window(&url, &model, key.as_deref()).await {
            Some(n) if n >= MIN_LOCAL_CONTEXT => check(
                "local context",
                Status::Pass,
                format!("{n} tokens (needs {MIN_LOCAL_CONTEXT})"),
            ),
            Some(n) => check(
                "local context",
                Status::Warn,
                format!("{n} tokens: a reading chunk, its prompt and the answer need {MIN_LOCAL_CONTEXT}"),
            )
            .fix("serve the model with a larger context window (see the backend's context setting)"),
            None => check(
                "local context",
                Status::Warn,
                "the server does not report a context window",
            )
            .fix(format!("make sure it serves at least {MIN_LOCAL_CONTEXT} tokens")),
        },
    );
    out
}

fn sandbox() -> Check {
    match duet_sandbox::detect() {
        Ok(kind) => check(
            "sandbox",
            Status::Pass,
            match kind {
                duet_sandbox::SandboxKind::Seatbelt => {
                    format!("Seatbelt ({})", duet_sandbox::SANDBOX_EXEC)
                }
                duet_sandbox::SandboxKind::Bubblewrap => {
                    format!("bubblewrap ({})", duet_sandbox::BWRAP)
                }
            },
        ),
        Err(e) => check("sandbox", Status::Fail, e.to_string()).fix(format!(
            "on Linux install bubblewrap so that {} exists; on macOS {} is part of the system",
            duet_sandbox::BWRAP,
            duet_sandbox::SANDBOX_EXEC
        )),
    }
}

fn git(ws: &Path) -> Vec<Check> {
    match duet_git::Git::locate() {
        Ok(g) => {
            let mut out = vec![check("git", Status::Pass, g.binary().display().to_string())];
            if !g.is_repository(ws) {
                out.push(
                    check(
                        "workspace",
                        Status::Warn,
                        format!("{} is not a git repository", ws.display()),
                    )
                    .fix("run duet inside a repository (git init) so it can list files and checkpoint edits"),
                );
            }
            out
        }
        Err(e) => vec![
            check("git", Status::Fail, e.to_string())
                .fix("install git at /usr/bin/git, /opt/homebrew/bin/git or /usr/local/bin/git"),
        ],
    }
}

fn disk(ws: &Path) -> Check {
    let free = match rustix::fs::statvfs(ws) {
        Ok(s) => s.f_bavail.saturating_mul(s.f_frsize),
        Err(e) => return check("disk", Status::Warn, format!("cannot read free space: {e}")),
    };
    let gib = free as f64 / (1u64 << 30) as f64;
    let detail = format!("{gib:.1} GiB free for run data under {}", ws.display());
    if free < DISK_FAIL {
        check("disk", Status::Fail, detail).fix("free space, or `duet purge` old runs")
    } else if free < DISK_WARN {
        check("disk", Status::Warn, detail).fix("free space, or `duet purge` old runs")
    } else {
        check("disk", Status::Pass, detail)
    }
}

/// Verifies the chain and anchor of the most recent run audit logs.
fn audit(ws: &Path) -> Check {
    let mut logs: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(ws.join(".duet/audit"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".jsonl")?.to_owned();
            Some((e.metadata().and_then(|m| m.modified()).ok()?, id))
        })
        .collect();
    if logs.is_empty() {
        return check("audit", Status::Pass, "no runs in this workspace yet");
    }
    logs.sort_by_key(|l| std::cmp::Reverse(l.0));
    let (mut bad, mut unanchored, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    for (_, id) in logs.iter().take(RECENT_RUNS) {
        let path = ws.join(".duet/audit").join(format!("{id}.jsonl"));
        match verify(&path) {
            Ok(Verification::Intact { .. }) => {}
            Ok(Verification::Broken { at_seq, reason }) => {
                bad.push(format!("{id} (chain broken at {at_seq}: {reason})"));
                continue;
            }
            Err(e) => {
                bad.push(format!("{id} ({e})"));
                continue;
            }
        }
        match check_anchor(&path, &run_anchor(ws, id)) {
            Ok(AnchorCheck::Matches) => {}
            Ok(AnchorCheck::Extends { unanchored }) => {
                notes.push(format!("{id}: {unanchored} unanchored record(s)"))
            }
            Ok(AnchorCheck::Mismatch(reason)) => bad.push(format!("{id} ({reason})")),
            Ok(AnchorCheck::Missing) => unanchored.push(id.clone()),
            Err(e) => bad.push(format!("{id} ({e})")),
        }
    }
    let n = logs.len().min(RECENT_RUNS);
    if !bad.is_empty() {
        return check(
            "audit",
            Status::Fail,
            format!("altered since the run: {}", bad.join(", ")),
        )
        .fix("inspect with `duet audit verify <run>`; treat those runs' records as untrusted");
    }
    if !unanchored.is_empty() {
        return check(
            "audit",
            Status::Warn,
            format!(
                "no anchor for {} (a rewrite of those logs would go unnoticed)",
                unanchored.join(", ")
            ),
        )
        .fix("anchors live under the owner state directory; runs from before anchoring, or with another DUET_CONFIG_HOME, have none");
    }
    let mut detail = format!("{n} most recent run(s): chains intact, anchors match");
    if !notes.is_empty() {
        detail.push_str(&format!(" ({})", notes.join("; ")));
    }
    check("audit", Status::Pass, detail)
}

fn retention(ws: &Path, c: &Config) -> Check {
    let days = c.int("data.retention_days").unwrap_or(14).max(0) as u64;
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(days * 86_400);
    let old = std::fs::read_dir(ws.join(".duet/runs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t < cutoff)
        })
        .count();
    if old == 0 {
        check(
            "run data",
            Status::Pass,
            format!("no raw run data older than {days} day(s)"),
        )
    } else {
        check(
            "run data",
            Status::Warn,
            format!("{old} run(s) keep raw data past data.retention_days ({days})"),
        )
        .fix("duet purge")
    }
}
