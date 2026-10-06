// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline policy preview. Workspace files are classified from names and metadata;
//! neither contents nor model clients are opened.
use crate::{Mode, policy};
use anyhow::Result;
use declass_boundary::policy::{Policy, glob_match, is_env_template};
use declass_config::Config;
use declass_provider::{Role, endpoint::ApprovedEndpoint};
use rustix::fs::{AtFlags, Dir, FileType, Mode as FsMode, OFlags};
use serde::Serialize;
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const MAX_ENTRIES: usize = 20_000;
const MAX_DEPTH: usize = 48;
const SKIPPED: &[&str] = &[
    ".git",
    ".declass",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
];
const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(clap::Args)]
pub(crate) struct Args {
    #[arg(long)]
    json: bool,
    /// Preview this mode; clearance.required still applies.
    #[arg(long, value_enum)]
    mode: Option<Mode>,
}

#[derive(Serialize)]
pub(crate) struct Report {
    schema: u32,
    mode: &'static str,
    scope: &'static str,
    complete: bool,
    destinations: Vec<Destination>,
    command_network: String,
    command_registries: Vec<String>,
    web_enabled: bool,
    web_fetch: &'static str,
    web_private_hosts: Vec<String>,
    search_origins: Vec<String>,
    mcp: Vec<Mcp>,
    exceptions: Vec<Exception>,
    excluded_directories: &'static [&'static str],
    files: Vec<FileRule>,
    warnings: Vec<String>,
}
#[derive(Serialize)]
struct Destination {
    role: &'static str,
    origin: Option<String>,
    enabled: bool,
    admitted: bool,
    trust: &'static str,
}
#[derive(Serialize)]
struct Mcp {
    name: String,
    origin: Option<String>,
    network: bool,
    enabled: bool,
    valid: bool,
}
#[derive(Serialize)]
struct Exception {
    setting: &'static str,
    origin: String,
}
#[derive(Serialize)]
struct FileRule {
    path: String,
    disposition: &'static str,
    rule: String,
}

fn origin(raw: &str) -> Option<String> {
    url::Url::parse(raw)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(|u| u.origin().ascii_serialization())
}
fn mcp_origin(server: &declass_mcp::ServerConfig) -> Option<String> {
    match &server.launch {
        declass_mcp::Launch::Url(url) => origin(url),
        _ => None,
    }
}
fn search_origins(cfg: &Config, ws: &Path, frontier_url: &str) -> Result<Vec<String>> {
    use declass_web::search::Backend;
    let key_env = cfg.str("frontier.api_key_env")?;
    let choice = crate::web::choose(
        cfg,
        Some(crate::web::Frontier {
            base_url: frontier_url,
            key_env: &key_env,
        }),
        &|name| std::env::var(name).ok(),
        ws,
    )?;
    let mut origins = match choice.backend {
        Some(Backend::Native(n)) => n
            .sources
            .iter()
            .map(|s| s.endpoint.origin().ascii_serialization())
            .collect(),
        Some(
            Backend::Zai { endpoint, .. }
            | Backend::ZaiPlan { endpoint, .. }
            | Backend::Brave { endpoint, .. }
            | Backend::Wikipedia { endpoint },
        ) => vec![endpoint.origin().ascii_serialization()],
        Some(Backend::Searxng { base }) => vec![base.origin().ascii_serialization()],
        None => Vec::new(),
    };
    origins.sort();
    origins.dedup();
    Ok(origins)
}
fn endpoint(raw: &str, role: Role, enabled: bool) -> Destination {
    let local = matches!(role, Role::Local { .. });
    let admitted = ApprovedEndpoint::new(raw, &role).is_ok();
    // Never echo URL credentials, query strings, fragments or API path segments.
    let origin = origin(raw);
    let trust = if !admitted {
        "refused"
    } else if !local {
        "frontier provider"
    } else {
        let Role::Local {
            allowlist,
            allow_plaintext,
        } = &role
        else {
            unreachable!()
        };
        declass_provider::endpoint::check_local_endpoint(raw, allowlist, *allow_plaintext)
            .map(|t| t.as_str())
            .unwrap_or("refused")
    };
    Destination {
        role: if local { "local" } else { "frontier" },
        origin,
        enabled,
        admitted,
        trust,
    }
}

fn classify(path: &Path, p: &Policy, mode: Mode) -> (&'static str, String) {
    if mode == Mode::Passthrough {
        return ("unfiltered", "privacy boundary disabled".into());
    }
    if mode == Mode::TopClearance {
        return ("local-only", "top-clearance mode".into());
    }
    let name = path.to_string_lossy();
    for (setting, globs, disposition) in [
        ("ip.sealed", &p.sealed, "sealed"),
        ("ip.interface_only", &p.interface_only, "interface-only"),
        (
            "sensitivity.protected_paths",
            &p.protected_paths,
            "sensitive",
        ),
        ("sensitivity.globs", &p.sensitive_globs, "sensitive"),
    ] {
        if setting == "sensitivity.globs" && is_env_template(path) {
            continue;
        }
        if let Some(glob) = globs.iter().find(|g| glob_match(g, &name)) {
            return (disposition, format!("{setting}: {glob}"));
        }
    }
    (
        "content-checks",
        if is_env_template(path) {
            "environment template; runtime content checks apply"
        } else {
            "runtime content checks apply"
        }
        .into(),
    )
}

struct Inventory<'a> {
    policy: &'a Policy,
    mode: Mode,
    files: Vec<FileRule>,
    complete: bool,
    seen: usize,
}
impl Inventory<'_> {
    fn walk(&mut self, fd: impl AsFd, parent: &Path, depth: usize) -> std::io::Result<()> {
        if depth > MAX_DEPTH {
            self.complete = false;
            return Ok(());
        }
        for entry in Dir::read_from(&fd)? {
            let entry = entry?;
            let raw = entry.file_name().to_bytes();
            if raw == b"." || raw == b".." {
                continue;
            }
            self.seen += 1;
            if self.seen > MAX_ENTRIES {
                self.complete = false;
                break;
            }
            let name = std::ffi::OsStr::from_bytes(raw);
            let path = parent.join(name);
            // Fail visibly on non-UTF8 names: lossy names cannot prove which rule matched.
            let Some(text) = path.to_str() else {
                self.complete = false;
                continue;
            };
            let stat = match rustix::fs::statat(&fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(s) => s,
                Err(_) => {
                    self.complete = false;
                    continue;
                }
            };
            let kind = FileType::from_raw_mode(stat.st_mode);
            if kind == FileType::Directory {
                if SKIPPED.contains(&name.to_str().unwrap_or_default()) {
                    continue;
                }
                match rustix::fs::openat(&fd, name, DIR_FLAGS, FsMode::empty()) {
                    Ok(child) => {
                        if self.walk(child, &path, depth + 1).is_err() {
                            self.complete = false;
                        }
                    }
                    Err(_) => self.complete = false,
                }
                continue;
            }
            let (disposition, rule) = if kind == FileType::RegularFile {
                classify(&path, self.policy, self.mode)
            } else {
                ("blocked", "symlink or special file; not followed".into())
            };
            self.files.push(FileRule {
                path: text.to_owned(),
                disposition,
                rule,
            });
        }
        Ok(())
    }
}

pub(crate) fn collect(
    ws: &Path,
    cfg: &Config,
    mode: Mode,
    frontier_override: Option<&str>,
    local_override: Option<&str>,
) -> Result<Report> {
    let p = policy(cfg)?;
    let mut inventory = Inventory {
        policy: &p,
        mode,
        files: Vec::new(),
        complete: true,
        seen: 0,
    };
    let root = rustix::fs::open(ws, DIR_FLAGS, FsMode::empty())?;
    if inventory.walk(root, Path::new(""), 0).is_err() {
        inventory.complete = false;
    }
    inventory.files.sort_by(|a, b| a.path.cmp(&b.path));
    let top = mode == Mode::TopClearance;
    let local_url = cfg.str("local.base_url")?;
    let derived = declass_boundary::derived::workspace_paths(ws);
    if let Ok(paths) = &derived {
        if mode == Mode::Hybrid {
            for file in &mut inventory.files {
                if file.disposition == "content-checks" && paths.contains(Path::new(&file.path)) {
                    file.disposition = "sensitive";
                    file.rule = "persistent classification: derived from sensitive data".into();
                }
            }
        }
    } else {
        inventory.complete = false;
        for file in &mut inventory.files {
            if file.disposition == "content-checks" {
                file.disposition = "unknown";
                file.rule =
                    "derived classification unavailable; run startup requires recovery".into();
            }
        }
    }
    let frontier_url = cfg.str("frontier.base_url")?;
    let destinations = vec![
        endpoint(
            local_override.unwrap_or(&local_url),
            Role::Local {
                allowlist: cfg.list("local.allowlist")?,
                allow_plaintext: cfg.bool("local.allow_plaintext")?,
            },
            mode != Mode::Passthrough && cfg.bool("local.enabled")?,
        ),
        endpoint(
            frontier_override.unwrap_or(&frontier_url),
            Role::Frontier,
            !top,
        ),
    ];
    let mut mcp = crate::mcp::configured(cfg)
        .into_iter()
        .map(|(name, server)| match server {
            Ok((s, enabled)) => {
                let network = crate::mcp::reaches_network(&s);
                Mcp {
                    name,
                    origin: mcp_origin(&s),
                    network,
                    enabled: enabled && !(top && network),
                    valid: true,
                }
            }
            Err(_) => Mcp {
                name,
                origin: None,
                network: false,
                enabled: false,
                valid: false,
            },
        })
        .collect::<Vec<_>>();
    // Read installed plugin manifests, but never start their servers or load skills.
    let plugin_servers = if cfg.bool("extensions.plugins_enabled")? {
        crate::plugins::store()
            .and_then(|root| crate::plugins::active_packages(&root))
            .and_then(|packages| crate::plugins::servers(&packages, cfg))
    } else {
        Ok(Vec::new())
    };
    let plugin_error = plugin_servers.is_err();
    if let Ok(servers) = plugin_servers {
        for server in servers {
            let network = crate::mcp::reaches_network(&server);
            mcp.push(Mcp {
                origin: mcp_origin(&server),
                name: server.name,
                network,
                enabled: !(top && network),
                valid: true,
            });
        }
    }
    let exceptions = declass_config::REGISTRY
        .iter()
        .filter(|s| {
            !declass_config::is_template(s.key) && s.direction != declass_config::Direction::Any
        })
        .filter(|s| cfg.origin(s.key) != Some(declass_config::Origin::Default))
        .filter_map(|s| {
            let default = toml::from_str::<toml::Table>(&format!("v = {}", s.default))
                .ok()?
                .remove("v")?;
            declass_config::loosening(s, &default, cfg.value(s.key).ok()?)?;
            Some(Exception {
                setting: s.key,
                origin: format!("{:?}", cfg.origin(s.key)?),
            })
        })
        .collect();
    let mut warnings = Vec::new();
    if plugin_error {
        inventory.complete = false;
        warnings.push("Installed plugin server policies could not be inspected.".into());
    }
    if derived.is_err() {
        warnings.push("Derived-file classifications are unavailable or a sensitive command is pending; review local state before starting a run.".into());
    }
    if !inventory.complete {
        warnings.push("Inventory incomplete: entry/depth limit, unreadable directory, changed entry or non-UTF8 name.".into());
    }
    for d in &destinations {
        if d.enabled && !d.admitted {
            warnings.push(format!(
                "{} endpoint is refused by current endpoint policy.",
                d.role
            ));
        }
        if d.enabled && d.trust == "allowlisted-plaintext-opted-in" {
            warnings.push(
                "Sensitive model traffic uses an owner-approved remote endpoint over plain HTTP."
                    .into(),
            );
        }
    }
    if mcp.iter().any(|s| !s.valid) {
        warnings.push("An MCP server configuration is invalid.".into());
    }
    if mode == Mode::Passthrough {
        warnings
            .push("Passthrough sends content to the frontier without the privacy boundary.".into());
    }
    if crate::local_enabled(cfg, mode).is_err() {
        inventory.complete = false;
        warnings
            .push("The selected mode or local PII checks require an enabled local model.".into());
    }
    let command_network = if top {
        "off".into()
    } else {
        cfg.str("sandbox.network")?
    };
    if command_network == "all" {
        warnings.push("Commands have unrestricted network access.".into());
    }
    let mut command_registries = if command_network == "registries" {
        cfg.list("sandbox.registries")?
    } else {
        Vec::new()
    };
    if declass_egress::Hosts::parse(&command_registries).is_err() {
        inventory.complete = false;
        command_registries.clear();
        warnings.push("Command registry rules are invalid.".into());
    }
    let web_enabled = !top && cfg.bool("web.enabled")?;
    let mut web_private_hosts = if web_enabled {
        cfg.list("web.allowlist_private")?
    } else {
        Vec::new()
    };
    if declass_web::guard::Allowlist::parse(&web_private_hosts).is_err() {
        inventory.complete = false;
        web_private_hosts.clear();
        warnings.push("Private web host rules are invalid.".into());
    }
    let search_origins = if web_enabled {
        match search_origins(cfg, ws, frontier_override.unwrap_or(&frontier_url)) {
            Ok(origins) => origins,
            Err(_) => {
                inventory.complete = false;
                warnings.push("Search destination selection is invalid.".into());
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    Ok(Report {
        schema: 1,
        mode: mode.as_str(),
        scope: "Path-policy preview, including ignored files. Workspace file contents are not scanned; local classification and plugin metadata are read. No network requests are made. Content checks run when Declass uses a file. Excluded directories are not inventoried.",
        complete: inventory.complete,
        destinations,
        command_network,
        command_registries,
        web_enabled,
        web_fetch: if web_enabled {
            "public HTTP(S) hosts selected at runtime, plus the explicit private-host allowlist; metadata addresses refused"
        } else {
            "disabled"
        },
        web_private_hosts,
        search_origins,
        mcp,
        exceptions,
        excluded_directories: SKIPPED,
        files: inventory.files,
        warnings,
    })
}

impl Report {
    pub(crate) fn render(&self, include_files: bool) -> String {
        let mut lines = vec![
            format!("Privacy · {} · {} paths", self.mode, self.files.len()),
            self.scope.into(),
        ];
        for d in &self.destinations {
            lines.push(format!(
                "{}: {} · {} · {}",
                d.role,
                d.origin.as_deref().unwrap_or("invalid origin"),
                if d.enabled { "enabled" } else { "disabled" },
                d.trust
            ));
        }
        lines.push(format!(
            "Commands: {} network · web tools: {}",
            self.command_network,
            if self.web_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ));
        if !self.command_registries.is_empty() {
            lines.push(format!(
                "Command registries: {}",
                self.command_registries.join(", ")
            ));
        }
        lines.push(format!("Web fetch: {}", self.web_fetch));
        if !self.web_private_hosts.is_empty() {
            lines.push(format!(
                "Private web allowlist: {}",
                self.web_private_hosts.join(", ")
            ));
        }
        if !self.search_origins.is_empty() {
            lines.push(format!(
                "Search destinations: {}",
                self.search_origins.join(", ")
            ));
        }
        for m in &self.mcp {
            lines.push(format!(
                "MCP {}: {} · {} · network {}",
                m.name,
                if m.enabled { "enabled" } else { "disabled" },
                m.origin.as_deref().unwrap_or("local process"),
                m.network
            ));
        }
        for e in &self.exceptions {
            lines.push(format!("Exception: {} ({})", e.setting, e.origin));
        }
        for w in &self.warnings {
            lines.push(format!("Attention: {w}"));
        }
        lines.push(format!(
            "Excluded directories: {}",
            self.excluded_directories.join(", ")
        ));
        if include_files {
            for f in &self.files {
                lines.push(format!(
                    "{} · {} · {}",
                    f.path.escape_debug(),
                    f.disposition,
                    f.rule.escape_debug()
                ));
            }
        } else {
            lines.push(
                "/privacy shows file rules; declass privacy --json exports this offline preview."
                    .into(),
            );
        }
        declass_tui::term::safe(&lines.join("\n"))
    }
    fn code(&self) -> i32 {
        if !self.complete || self.destinations.iter().any(|d| d.enabled && !d.admitted) {
            2
        } else if !self.warnings.is_empty() || !self.exceptions.is_empty() {
            1
        } else {
            0
        }
    }
}
pub(crate) fn run(ws: &Path, args: Args, cfg: &Config) -> Result<i32> {
    let mode = crate::overrides::resolve_mode(cfg, args.mode, false)?;
    let report = collect(ws, cfg, mode, None, None)?;
    println!(
        "{}",
        if args.json {
            serde_json::to_string_pretty(&report)?
        } else {
            report.render(true)
        }
    );
    Ok(report.code())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(root: &Path, text: &str) -> Config {
        let owner = root.join("owner.toml");
        std::fs::write(&owner, text).unwrap();
        Config::load(&owner, None).unwrap()
    }
    #[test]
    fn ignored_sensitive_files_templates_and_links_are_reported_without_reading() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let cfg = config(temp.path(), "");
        for name in [".env", ".env.example", "ordinary.txt"] {
            std::fs::write(ws.join(name), "CANARY_CONTENT_MUST_NOT_APPEAR").unwrap();
        }
        std::fs::write(ws.join(".gitignore"), ".env\n").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", ws.join("link")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(ws.join("pipe"))
                .status()
                .unwrap()
                .success()
        );
        let r = collect(&ws, &cfg, Mode::Hybrid, None, None).unwrap();
        let disposition = |name| r.files.iter().find(|f| f.path == name).unwrap().disposition;
        assert_eq!(disposition(".env"), "sensitive");
        assert_eq!(disposition(".env.example"), "content-checks");
        assert_eq!(disposition("pipe"), "blocked");
        assert_eq!(disposition("link"), "blocked");
        assert!(
            !serde_json::to_string(&r)
                .unwrap()
                .contains("CANARY_CONTENT")
        );
        assert!(!ws.join(".declass").exists());
        assert!(r.complete);
    }
    #[test]
    fn endpoint_secrets_are_never_displayed_and_top_clearance_disables_cloud() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = config(temp.path(), "");
        let r = collect(
            temp.path(),
            &cfg,
            Mode::TopClearance,
            Some("https://user:SECRET@api.example/v1/PRIVATE?token=KEY"),
            None,
        )
        .unwrap();
        let text = serde_json::to_string(&r).unwrap();
        for secret in ["SECRET", "PRIVATE", "KEY"] {
            assert!(!text.contains(secret));
        }
        assert!(!r.destinations[1].enabled);
        assert!(!r.destinations[1].admitted);
        assert_eq!(r.command_network, "off");
        assert!(!r.web_enabled);
    }
    #[test]
    fn runtime_routes_match_passthrough_and_top_clearance() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = config(
            temp.path(),
            "[web.search]\nbackend = 'wikipedia'\n[extensions]\nplugins_enabled = false\n",
        );
        let passthrough = collect(temp.path(), &cfg, Mode::Passthrough, None, None).unwrap();
        assert!(!passthrough.destinations[0].enabled);
        assert!(passthrough.destinations[1].enabled);
        assert_eq!(passthrough.search_origins, vec!["https://en.wikipedia.org"]);
        assert!(!passthrough.command_registries.is_empty());
        let top = collect(temp.path(), &cfg, Mode::TopClearance, None, None).unwrap();
        assert!(top.search_origins.is_empty());
        assert!(top.command_registries.is_empty());
        assert!(top.web_private_hosts.is_empty());
    }

    #[test]
    fn ip_precedence_and_explicit_template_protection() {
        let p = Policy {
            sealed: vec!["private/**".into()],
            interface_only: vec!["**/*.rs".into()],
            protected_paths: vec![".env.example".into()],
            ..Policy::default()
        };
        assert_eq!(
            classify(Path::new("private/lib.rs"), &p, Mode::Hybrid).0,
            "sealed"
        );
        assert_eq!(
            classify(Path::new(".env.example"), &p, Mode::Hybrid).0,
            "sensitive"
        );
    }
    #[test]
    fn symlink_root_is_refused_and_terminal_names_are_escaped() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = config(temp.path(), "");
        std::os::unix::fs::symlink(temp.path(), temp.path().join("link")).unwrap();
        assert!(collect(&temp.path().join("link"), &cfg, Mode::Hybrid, None, None).is_err());
        std::fs::write(temp.path().join("escape\u{1b}[2J"), "").unwrap();
        std::fs::write(temp.path().join("name\nforged-route"), "").unwrap();
        let rendered = collect(temp.path(), &cfg, Mode::Hybrid, None, None)
            .unwrap()
            .render(true);
        assert!(rendered.contains("name\\nforged-route"));
        assert!(
            !collect(temp.path(), &cfg, Mode::Hybrid, None, None)
                .unwrap()
                .render(true)
                .contains('\u{1b}')
        );
    }
}
