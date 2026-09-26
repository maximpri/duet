// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet doctor`: one line per check (pass, warn, fail or skip) with a fix for
//! anything short of pass. Offline by default: it reads configuration, the
//! environment and local files only. `--online` also asks the configured
//! servers for their model listings and context windows, and checks prompt-cache
//! reuse by sending each model the same short built-in prompt twice (nothing from
//! the workspace), and whether a model reads images by showing it two generated
//! one-colour images (the frontier only when frontier.vision is on); it never
//! prints a credential.

use crate::{config_audit_path, run_anchor};
use duet_boundary::audit::{AnchorCheck, Verification, check_anchor, verify};
use duet_config::{Config, Origin, REGISTRY};
use duet_provider::backends;
use duet_provider::endpoint::{Trust, check_local_endpoint, host_port, is_loopback_host};
use duet_provider::{ChatProvider, Dialect, ProviderConfig, Request, Role};
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
            "online: contacted the configured servers (listings, two identical built-in prompts per model for the cache check, two generated images for the vision check)"
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
            "duet {} ({}-{}, {} build); release channel not published yet, so no update check",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            if RELEASE_BUILD {
                "release"
            } else {
                "development"
            }
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
            out.push(command_network(c));
            out.push(detection_rules(duet_boundary::rules::imported()));
            out.push(language_servers(c));
            out.push(web_search(c, online).await);
            out.extend(frontier(c, online).await);
            out.extend(local(c, online).await);
            out.extend(mcp_servers(c, ws, online).await);
        }
        None => out.push(check(
            "endpoints",
            Status::Skip,
            "configuration did not load",
        )),
    }
    out.push(release_signers(&release_signers_path(), RELEASE_BUILD));
    out.push(sandbox());
    out.extend(git(ws));
    out.push(instructions(ws));
    out.push(disk(ws));
    out.push(audit(ws));
    if let Some(c) = &cfg {
        out.push(retention(ws, c));
    }
    out
}

/// Configured MCP servers: stdio servers are started in the sandbox (with a
/// run's hidden paths), HTTP servers only contacted with `--online`; each
/// reports how many tools it offers, then is stopped.
async fn mcp_servers(c: &Config, ws: &Path, online: bool) -> Vec<Check> {
    let configured = crate::mcp::configured(c);
    if configured.is_empty() {
        return vec![check(
            "mcp",
            Status::Skip,
            "no MCP servers configured ([mcp.servers.<name>] in the owner config)",
        )];
    }
    let mut out = Vec::new();
    let mut start = Vec::new();
    for (name, s) in configured {
        match s {
            Err(e) => out.push(check("mcp", Status::Fail, format!("{e:#}")).fix(format!(
                "fix [mcp.servers.{name}] in the owner config (`duet config list`)"
            ))),
            Ok((_, false)) => out.push(check(
                "mcp",
                Status::Skip,
                format!("server `{name}`: disabled"),
            )),
            Ok((server, true)) if !online && server.transport_name() == "http" => out.push(check(
                "mcp",
                Status::Skip,
                format!("server `{name}` (http): not contacted offline; --online connects"),
            )),
            Ok((server, true)) => start.push(server),
        }
    }
    if start.is_empty() {
        return out;
    }
    let sandbox = match duet_sandbox::detect() {
        Ok(k) => k,
        Err(e) => {
            out.push(check(
                "mcp",
                Status::Fail,
                format!("servers cannot start: {e}"),
            ));
            return out;
        }
    };
    // The same hidden paths as a run, from an engine over the configured
    // policy (no local model; nothing leaves this machine).
    let dir = std::env::temp_dir().join(format!("duet-doctor-{}", uuid::Uuid::new_v4()));
    let engine = match crate::policy(c).map_err(|e| e.to_string()).and_then(|p| {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        duet_boundary::engine::Engine::open(&dir, p, None).map_err(|e| e.to_string())
    }) {
        Ok(e) => e,
        Err(e) => {
            out.push(check(
                "mcp",
                Status::Fail,
                format!("servers cannot start: {e}"),
            ));
            return out;
        }
    };
    let workspace = ws.canonicalize().unwrap_or_else(|_| ws.to_path_buf());
    let reports = duet_agent::mcp::probe(
        &start,
        &duet_agent::mcp::Setup {
            workspace: &workspace,
            run_dir: &dir,
            sandbox,
            presenter: engine.as_ref(),
            audit: None,
        },
    )
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    for r in reports {
        out.push(match r.result {
            Ok(n) => check(
                "mcp",
                Status::Pass,
                format!("server `{}` ({}): started, {n} tool(s)", r.name, r.transport),
            ),
            Err(e) => check(
                "mcp",
                Status::Warn,
                format!("server `{}` ({}): did not start: {e}", r.name, r.transport),
            )
            .fix(format!(
                "check mcp.servers.{0}.command/args or url (`duet config list`); a stdio server runs \
in the command sandbox without network unless mcp.servers.{0}.network = true, and gets only the \
environment variables named in mcp.servers.{0}.env",
                r.name
            )),
        });
    }
    out
}

fn config_loaded(c: &Config, owner: &Path, project: &Path) -> Check {
    // A template setting counts once per configured instance.
    let count = |o| -> usize {
        REGISTRY
            .iter()
            .map(|s| {
                if duet_config::is_template(s.key) {
                    c.instances(s.key)
                        .iter()
                        .filter(|n| c.origin(&s.key.replace('*', n)) == Some(o))
                        .count()
                } else {
                    usize::from(c.origin(s.key) == Some(o))
                }
            })
            .sum()
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

/// Commands' network (`sandbox.network`, `sandbox.registries`).
fn command_network(c: &Config) -> Check {
    let mut result = match c.str("sandbox.network").unwrap_or_default().as_str() {
        "off" => check(
            "command network",
            Status::Pass,
            "off: sandboxed commands have no network (package installs fail)",
        ),
        "all" => check(
            "command network",
            Status::Pass,
            "all: sandboxed commands reach any host, except sensitive_data commands and checks that \
             can read protected source (none)",
        ),
        _ => match c
            .list("sandbox.registries")
            .map_err(|e| e.to_string())
            .and_then(|l| duet_egress::Hosts::parse(&l).map(|_| l.len()))
        {
            Ok(n) => check(
                "command network",
                Status::Pass,
                format!(
                    "registries: sandboxed commands reach the {n} hosts in sandbox.registries \
                     through duet's egress proxy (every connection audited), nothing else; \
                     sensitive_data commands and checks that can read protected source have none"
                ),
            ),
            Err(e) => check("command network", Status::Fail, e).fix(
                "correct sandbox.registries in the owner config (host names, *.domain, :port)",
            ),
        },
    };
    if let Some(note) = c.notes.iter().find(|n| n.contains("sandbox.network")) {
        result.detail.push_str(&format!("; {note}"));
    }
    result
}

/// The imported detection rules: their version, how many are in use, and any
/// that did not compile or run without part of their definition (named here,
/// never dropped silently).
fn detection_rules(set: &duet_boundary::rules::RuleSet) -> Check {
    let version = duet_boundary::rules::notice_field("version").unwrap_or("of unknown version");
    let detail = format!(
        "gitleaks rule set {version}: {} rules in use with duet's own detectors",
        set.rules().len()
    );
    if set.failed().is_empty() && set.partial().is_empty() {
        return check("detection rules", Status::Pass, detail);
    }
    let list = |fs: &[duet_boundary::rules::Failure]| {
        fs.iter()
            .map(|f| format!("{} ({})", f.rule, f.reason))
            .collect::<Vec<_>>()
            .join("; ")
    };
    check(
        "detection rules",
        Status::Warn,
        format!(
            "{detail}; not compiled: {}; partly supported: {}",
            list(set.failed()),
            list(set.partial())
        ),
    )
    .fix("these rules find nothing (or more than they should); report it, or pin the previous set with tools/update-rules.sh")
}

/// Where the owner keeps the public keys trusted to sign releases.
pub fn release_signers_path() -> std::path::PathBuf {
    duet_config::owner_config_path().with_file_name("allowed_signers")
}

/// Whether this binary was built by `tools/release.sh`, which sets
/// `DUET_RELEASE_BUILD` at compile time. Any other build is a development
/// build.
pub const RELEASE_BUILD: bool = option_env!("DUET_RELEASE_BUILD").is_some();

/// Whether release verification material is present. A release build warns
/// without it; a development build only notes it (skip), since no release has
/// been published for it to verify.
fn release_signers(path: &Path, release_build: bool) -> Check {
    let (missing, dev_note) = if release_build {
        (Status::Warn, "")
    } else {
        (
            Status::Skip,
            " (development build: no release has been published, so none is needed yet)",
        )
    };
    let fix = format!(
        "save the published release signer line (`<identity> namespaces=\"duet-release\" <public key>`) to {}, then check a download with tools/verify-release.sh <release dir>",
        path.display()
    );
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let signers = text
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .count();
            if signers == 0 {
                check(
                    "release keys",
                    missing,
                    format!("{} lists no signer{dev_note}", path.display()),
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
            missing,
            format!(
                "no release signing keys at {}: a downloaded release cannot be verified{dev_note}",
                path.display()
            ),
        )
        .fix(fix),
    }
}

async fn frontier(c: &Config, online: bool) -> Vec<Check> {
    let url = c.str("frontier.base_url").unwrap_or_default();
    let model = c.str("frontier.model").unwrap_or_default();
    let dialect_name = c.str("frontier.dialect").unwrap_or_default();
    let dialect = Dialect::parse(&dialect_name).unwrap_or_default();
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
        Some(_) => check(
            "frontier",
            Status::Pass,
            format!("{url}, model {model}, dialect {}", dialect.as_str()),
        ),
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
        out.push(check(
            "frontier cache",
            Status::Skip,
            "not checked offline (use --online)",
        ));
        out.push(vision_offline("frontier vision", c, "frontier.vision"));
        return out;
    }
    let listed =
        backends::list_frontier_models(&url, dialect, key.as_deref(), ONLINE_TIMEOUT).await;
    // Only a model call can tell more when the endpoint answered but has no
    // listing; an unreachable endpoint or a refused key would fail it too.
    let reachable = match &listed {
        Ok(_) => true,
        Err(e) => e == "HTTP 404" || e == "HTTP 405",
    };
    out.push(match listed {
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
    });
    out.push(if key.is_none() && !key_env.is_empty() {
        check(
            "frontier cache",
            Status::Skip,
            format!("{key_env} is not set, so no request was sent"),
        )
    } else if !reachable {
        check(
            "frontier cache",
            Status::Skip,
            "not checked: the model listing failed (see frontier model)",
        )
    } else {
        let mut pc = ProviderConfig::new(&url, &model, Role::Frontier);
        pc.dialect = dialect;
        pc.api_key_env = (!key_env.is_empty()).then_some(key_env.clone());
        match ChatProvider::with_reqwest(online_limits(pc, Duration::from_secs(120))) {
            Ok(p) => {
                let probe = duet_provider::probe::frontier_cache_probe_request();
                cache_check("frontier cache", &p, &probe, &model).await
            }
            Err(e) => check("frontier cache", Status::Warn, e.message),
        }
    });
    let claimed = c.bool("frontier.vision").unwrap_or(false);
    out.push(if !claimed {
        check(
            "frontier vision",
            Status::Skip,
            "frontier.vision is false, so no image is sent to the frontier; not tested (a test \
costs two small requests: set it true to test it)",
        )
    } else if (key.is_none() && !key_env.is_empty()) || !reachable {
        check(
            "frontier vision",
            Status::Skip,
            "not tested: the frontier cannot be reached (see above)",
        )
    } else {
        let mut pc = ProviderConfig::new(&url, &model, Role::Frontier);
        pc.dialect = dialect;
        pc.api_key_env = (!key_env.is_empty()).then_some(key_env.clone());
        match ChatProvider::with_reqwest(online_limits(pc, Duration::from_secs(120))) {
            Ok(p) => {
                vision_check(
                    "frontier vision",
                    "frontier.vision",
                    true,
                    &p,
                    &Default::default(),
                )
                .await
            }
            Err(e) => check("frontier vision", Status::Warn, e.message),
        }
    });
    out
}

/// Offline: what the vision setting says (the model is tested with --online).
fn vision_offline(name: &'static str, c: &Config, key: &str) -> Check {
    let on = c.bool(key).unwrap_or(false);
    check(
        name,
        Status::Skip,
        format!(
            "{key} is {on}; not tested offline (--online shows the model two generated images)"
        ),
    )
}

/// Shows the model two generated one-colour images (nothing from the
/// workspace) and compares what it reads with the setting `key`.
async fn vision_check(
    name: &'static str,
    key: &str,
    claimed: bool,
    provider: &ChatProvider,
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Check {
    let probe = match duet_provider::probe::vision_probe(provider, extra).await {
        Ok(p) => p,
        Err(e) => {
            return check(
                name,
                Status::Warn,
                format!("the image test request failed: {e}"),
            )
            .fix(format!(
                "a server that does not take image parts refuses the request: keep {key} false"
            ));
        }
    };
    let answers = probe
        .answers
        .iter()
        .map(|(want, got)| format!("{want} image: \"{got}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let detail = format!(
        "{answers}; {} input tokens for the first",
        probe.prompt_tokens
    );
    match (probe.reads_images, claimed) {
        (true, true) => check(name, Status::Pass, format!("reads images ({detail})")),
        (true, false) => check(
            name,
            Status::Warn,
            format!("the model reads images, but {key} is false ({detail})"),
        )
        .fix(format!("duet config set {key} true --confirm")),
        (false, true) => check(
            name,
            Status::Fail,
            format!(
                "{key} is true, but the model did not read the test images ({detail}): it would \
answer about images it never saw"
            ),
        )
        .fix(format!(
            "duet config set {key} false, or serve a vision-language model"
        )),
        (false, false) => check(
            name,
            Status::Pass,
            format!("does not read images, and {key} is false ({detail})"),
        ),
    }
}

/// Bounded retries for the doctor's two requests.
fn online_limits(mut pc: ProviderConfig, first_byte: Duration) -> ProviderConfig {
    pc.max_attempts = Some(2);
    pc.first_byte_timeout = first_byte;
    pc.idle_timeout = Duration::from_secs(60);
    pc
}

/// Sends the same built-in prompt twice (`probe`; nothing from the workspace)
/// and reports what the repeat read from the cache.
async fn cache_check(
    name: &'static str,
    provider: &ChatProvider,
    probe: &Request,
    model: &str,
) -> Check {
    let r = match duet_provider::probe::cache_reuse_with(provider, probe).await {
        Ok(r) => r,
        Err(e) => {
            return check(
                name,
                Status::Warn,
                format!("the cache check request failed: {e}"),
            )
            .fix("check the model id and the key; `duet doctor --online` lists the models");
        }
    };
    let cost = duet_provider::price::builtin(model)
        .map(|p| {
            let second = duet_provider::Usage {
                input: r.prompt_tokens - r.second_cached.min(r.prompt_tokens),
                cache_read: r.second_cached,
                ..Default::default()
            };
            let first = duet_provider::Usage {
                input: r.prompt_tokens,
                ..Default::default()
            };
            format!(
                "; about ${:.4} at list price",
                p.cost(&first) + p.cost(&second)
            )
        })
        .unwrap_or_default();
    let timing = format!(
        "cold {:.1} s, warm {:.1} s",
        r.first_seconds, r.second_seconds
    );
    if r.second_cached > 0 {
        check(
            name,
            Status::Pass,
            format!(
                "the repeated request read {} of {} input tokens from the cache ({timing}{cost})",
                r.second_cached, r.prompt_tokens
            ),
        )
    } else {
        let why = if r.unreported {
            "the server reported no usage"
        } else {
            "the repeated request reported no cached input tokens"
        };
        check(
            name,
            Status::Warn,
            format!(
                "{why} ({} input; {timing}{cost}): every turn may pay for the whole conversation again",
                r.prompt_tokens
            ),
        )
        .fix("use an endpoint and model with prompt caching (a local server may reuse the prefix without reporting it: compare the timings)")
    }
}

async fn local(c: &Config, online: bool) -> Vec<Check> {
    if !c.bool("local.enabled").unwrap_or(true) {
        // Nothing to contact; a setting that needs a local model is a conflict.
        return vec![if c.bool("sensitivity.local_pii_pass").unwrap_or(false) {
            check(
                "local model",
                Status::Fail,
                "off (local.enabled = false), but sensitivity.local_pii_pass needs one: hybrid runs are refused",
            )
            .fix("turn one of them off: duet config set sensitivity.local_pii_pass false")
        } else {
            check(
                "local model",
                Status::Pass,
                "off (local.enabled = false): hybrid runs show sensitive content as handles only; ask_local, edit_protected and local-only mode are refused",
            )
        }];
    }
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
        out.push(check(
            "local cache",
            Status::Skip,
            "not checked offline (use --online)",
        ));
        out.push(vision_offline("local vision", c, "local.vision"));
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
    let mut pc = ProviderConfig::new(
        &url,
        &model,
        Role::Local {
            allowlist,
            allow_plaintext,
        },
    );
    pc.api_key_env = (!key_env.is_empty()).then_some(key_env);
    match ChatProvider::with_reqwest(online_limits(pc, Duration::from_secs(300))) {
        Ok(p) => {
            let probe = duet_provider::probe::cache_probe_request();
            out.push(cache_check("local cache", &p, &probe, "").await);
            // As the local roles call it: no visible thinking.
            let mut extra = serde_json::Map::new();
            extra.insert(
                "chat_template_kwargs".into(),
                serde_json::json!({"enable_thinking": false}),
            );
            let claimed = c.bool("local.vision").unwrap_or(false);
            out.push(vision_check("local vision", "local.vision", claimed, &p, &extra).await);
        }
        Err(e) => out.push(check("local cache", Status::Skip, e.message)),
    }
    out
}

/// Which language servers `code_nav` and `rename` would use (found on
/// `PATH` or configured); none is started.
fn language_servers(c: &Config) -> Check {
    const NAME: &str = "language servers";
    let s = match crate::lsp::settings(c) {
        Ok(s) => s,
        Err(e) => {
            return check(NAME, Status::Fail, format!("{e:#}")).fix(
                "correct lsp.servers in the owner config: [lsp.servers.<language>] with command, and args, \
                 extensions and env as needed",
            );
        }
    };
    if !s.enabled {
        return check(
            NAME,
            Status::Skip,
            "lsp.enabled is off: code_nav and rename are not offered",
        );
    }
    let path = std::env::var_os("PATH");
    let found = duet_lsp::servers::detect(&s, path.as_deref());
    let mut have = Vec::new();
    let mut missing = Vec::new();
    for spec in duet_lsp::servers::table(&s) {
        match found.iter().find(|d| d.spec.language == spec.language) {
            Some(d) => have.push(format!("{} ({})", spec.language, d.program.display())),
            None => {
                let names: Vec<&str> = spec.candidates.iter().map(|(c, _)| c.as_str()).collect();
                missing.push(format!("{} ({})", spec.language, names.join(" or ")));
            }
        }
    }
    if have.is_empty() {
        return check(
            NAME,
            Status::Skip,
            format!("none installed, so code_nav and rename are not offered; looked for {}", missing.join(", ")),
        )
        .fix("install a server for the project's language (for example rust-analyzer), or configure [lsp.servers.<language>]");
    }
    let mut detail = format!(
        "{}; started on first use, sandboxed without network",
        have.join(", ")
    );
    if !missing.is_empty() {
        detail.push_str(&format!("; not found: {}", missing.join(", ")));
    }
    check(NAME, Status::Pass, detail)
}

/// The search backend runs get (`web.search.backend`), who receives the
/// queries, and with `--online` whether the owner's SearXNG answers JSON (the
/// only backend queried: the others are third parties, some paid per search).
async fn web_search(c: &Config, online: bool) -> Check {
    const NAME: &str = "web search";
    match c.bool("web.enabled") {
        Ok(true) => {}
        Ok(false) => return check(NAME, Status::Skip, "web.enabled is off: no web tools"),
        Err(e) => return check(NAME, Status::Fail, e.to_string()),
    }
    let (base_url, key_env) = match (c.str("frontier.base_url"), c.str("frontier.api_key_env")) {
        (Ok(b), Ok(k)) => (b, k),
        (Err(e), _) | (_, Err(e)) => return check(NAME, Status::Fail, e.to_string()),
    };
    let frontier = crate::web::Frontier {
        base_url: &base_url,
        key_env: &key_env,
    };
    let choice = match crate::web::choose(c, Some(frontier), &|name| std::env::var(name).ok()) {
        Ok(ch) => ch,
        Err(e) => {
            return check(NAME, Status::Fail, format!("{e:#}"))
                .fix("correct web.search.searxng_url (an http or https URL)");
        }
    };
    if let Some(problem) = &choice.problem {
        return check(NAME, Status::Warn, choice.detail.clone()).fix(if problem.contains("$") {
            "export the key variable named above, or: duet config set web.search.backend '\"auto\"' --confirm"
        } else {
            "duet config preset searxng --confirm, or: duet config set web.search.backend '\"auto\"' --confirm"
        });
    }
    let Some(backend) = choice.backend else {
        return check(NAME, Status::Skip, choice.detail);
    };
    if !matches!(backend, duet_web::Backend::Searxng { .. }) {
        return check(NAME, Status::Pass, choice.detail);
    }
    if !online {
        return check(
            NAME,
            Status::Pass,
            format!("{}; not contacted (--online asks it)", choice.detail),
        );
    }
    let web = duet_web::Web::new(duet_web::WebConfig {
        max_bytes: 5_000_000,
        timeout: ONLINE_TIMEOUT,
        allowlist: duet_web::guard::Allowlist::default(),
        search: Some(backend),
    });
    match web.search("duet", 3).await {
        Ok(r) => check(
            NAME,
            Status::Pass,
            format!("{}; answered a test query ({} results)", choice.detail, r.len()),
        ),
        Err(e) => check(NAME, Status::Warn, format!("{}; test query: {e}", choice.detail)).fix(
            "start it (`duet config preset searxng` prints the command) and enable the JSON format (search.formats: [html, json] in its settings.yml)",
        ),
    }
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
            "on Linux install bubblewrap so that {} exists and allow unprivileged user \
             namespaces (in a container: a seccomp profile that permits them); on macOS {} is \
             part of the system",
            duet_sandbox::BWRAP,
            duet_sandbox::SANDBOX_EXEC
        )),
    }
}

fn git(ws: &Path) -> Vec<Check> {
    match duet_git::Git::locate() {
        Ok(g) => {
            let mut out = vec![check("git", Status::Pass, g.binary().display().to_string())];
            // Duet works without a repository; what changes is said here.
            if !g.is_repository(ws) {
                out.push(check(
                    "workspace",
                    Status::Pass,
                    format!(
                        "{} is not a git repository: files are listed by a walk that honours \
                         .gitignore and .ignore, `diff` and /diff show the files duet wrote, and the \
                         git tools are not offered (git init adds them)",
                        ws.display()
                    ),
                ));
            }
            out
        }
        Err(e) => vec![
            check("git", Status::Fail, e.to_string())
                .fix("install git at /usr/bin/git, /opt/homebrew/bin/git or /usr/local/bin/git"),
        ],
    }
}

/// Project instructions given at the start of every run and session: the
/// repository's `DUET.md` and the owner's own.
fn instructions(ws: &Path) -> Check {
    use duet_agent::instructions::{self, FILE, MAX_BYTES};
    let owner = duet_config::owner_instructions_path();
    let mut given = Vec::new();
    let mut problems = Vec::new();
    for (whose, path, read) in [
        ("repository", ws.join(FILE), instructions::project(ws)),
        ("owner", owner.clone(), instructions::owner(&owner)),
    ] {
        match read {
            None => {}
            Some(Ok(f)) => {
                given.push(format!("{whose} {} ({} bytes)", path.display(), f.bytes));
                if f.truncated {
                    problems.push(format!(
                        "the {whose} file is cut to its first {} KiB",
                        MAX_BYTES / 1024
                    ));
                }
            }
            Some(Err(e)) => problems.push(format!(
                "the {whose} file {} is not given: {e}",
                path.display()
            )),
        }
    }
    if given.is_empty() && problems.is_empty() {
        return check(
            "instructions",
            Status::Skip,
            format!(
                "no {FILE}: one at the repository root (repository text, scanned like any file) or at \
                 {} (yours) gives duet standing instructions",
                owner.display()
            ),
        );
    }
    let mut detail = if given.is_empty() {
        "none given".to_owned()
    } else {
        format!(
            "given at the start of each run and session: {}",
            given.join("; ")
        )
    };
    if problems.is_empty() {
        return check("instructions", Status::Pass, detail);
    }
    detail.push_str(&format!("; {}", problems.join("; ")));
    check("instructions", Status::Warn, detail).fix(format!(
        "keep each {FILE} a text file under {} KiB",
        MAX_BYTES / 1024
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_rules_report_their_version_and_failures() {
        let ok = detection_rules(duet_boundary::rules::imported());
        assert_eq!(ok.status, Status::Pass, "{ok:?}");
        assert!(ok.detail.contains("rules in use"), "{}", ok.detail);
        let broken = duet_boundary::rules::RuleSet::compile(
            "[[rules]]\nid = \"bad\"\nregex = '''(a)\\1'''\nkeywords = [\"a\"]\n",
        );
        let warn = detection_rules(&broken);
        assert_eq!(warn.status, Status::Warn, "{warn:?}");
        assert!(
            warn.detail.contains("not compiled: bad ("),
            "{}",
            warn.detail
        );
    }

    #[test]
    fn release_keys_warn_only_in_a_release_build() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("allowed_signers");

        // A development build: no release exists, so missing keys are a note.
        let dev = release_signers(&path, false);
        assert_eq!(dev.status, Status::Skip, "{dev:?}");
        assert!(dev.detail.contains("development build"), "{}", dev.detail);
        assert_eq!(exit_code(&[dev]), 0);

        // A release build: a downloaded release cannot be verified without them.
        let release = release_signers(&path, true);
        assert_eq!(release.status, Status::Warn, "{release:?}");
        assert!(!release.detail.contains("development build"));
        assert!(release.fix.unwrap().contains("verify-release.sh"));

        std::fs::write(&path, "# no signer yet\n").unwrap();
        assert_eq!(release_signers(&path, false).status, Status::Skip);
        assert_eq!(release_signers(&path, true).status, Status::Warn);

        std::fs::write(
            &path,
            "release@duet namespaces=\"duet-release\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample\n",
        )
        .unwrap();
        for build in [false, true] {
            let keys = release_signers(&path, build);
            assert_eq!(keys.status, Status::Pass, "{keys:?}");
            assert!(keys.detail.starts_with("1 release signer"));
        }
    }
}
