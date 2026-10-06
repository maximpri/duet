// SPDX-License-Identifier: GPL-3.0-or-later
//! Commands' network (`sandbox.network`): none, package registries only
//! through the host's egress proxy (`declass_egress`), or unrestricted.
//!
//! Which command gets what: ordinary commands, sub-agents' read-only
//! commands and checks get the run's mode; a `sensitive_data` command never
//! gets network (it reads sensitive files), and neither does a check that can
//! read protected source (checks may read it so the project builds). MCP and
//! language servers keep their own settings.
//!
//! The proxy is created on a run's (or session's) first command that needs
//! it; each command gets its own route, closed when the command ends, and
//! every connection is an `egress` audit event. A command that has network
//! also gets package caches in the run's scratch directory
//! ([`cache_env`]): the operator's own caches under the home directory are
//! read-only to commands, and must stay so (a command writing them could
//! change code the operator later builds outside the sandbox).

use declass_boundary::audit::{AuditEvent, AuditHandle};
use declass_boundary::third_party::Guard;
use declass_egress::{Hosts, Proxy, Route, Rules};
use declass_sandbox::SandboxKind;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// A run's `sandbox.network`.
#[derive(Clone, Default)]
pub enum Network {
    #[default]
    Off,
    /// Package registries through the egress proxy.
    Registries(Arc<Registries>),
    All,
}

impl Network {
    /// The mode's name, as in the configuration.
    pub fn name(&self) -> &'static str {
        match self {
            Network::Off => "off",
            Network::Registries(_) => "registries",
            Network::All => "all",
        }
    }

    /// How `run_command`'s description names the network.
    pub(crate) fn describe(&self) -> &'static str {
        match self {
            Network::Off => "no network",
            Network::Registries(_) => {
                "network only to package registries (npm, crates.io, PyPI, Go, Maven, RubyGems) \
through a proxy, and to servers the command itself starts on localhost"
            }
            Network::All => "network access",
        }
    }
}

impl std::fmt::Debug for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The registries mode of a run: the proxy's rules, created on first use.
pub struct Registries {
    rules: Rules,
    resolver: Arc<dyn declass_web::Resolve>,
    /// The bridge helper's command line under bubblewrap (`declass
    /// __sandbox-bridge`); unused under Seatbelt.
    helper: Vec<OsString>,
    proxy: OnceLock<Proxy>,
}

impl Registries {
    /// Registries `hosts`, names resolved by the system.
    pub fn new(hosts: Hosts, helper: Vec<OsString>) -> Self {
        Self::with_rules(
            Rules::new(hosts),
            Arc::new(declass_web::SystemResolver),
            helper,
        )
    }

    /// Any rules and resolver (tests route a listed name to a loopback server).
    pub fn with_rules(
        rules: Rules,
        resolver: Arc<dyn declass_web::Resolve>,
        helper: Vec<OsString>,
    ) -> Self {
        Self {
            rules,
            resolver,
            helper,
            proxy: OnceLock::new(),
        }
    }

    /// The run's proxy; the first caller's audit log receives its events
    /// (sub-agents share their parent's log), and its presenter's guard
    /// checks what the proxy forwards (sub-agents share the engine).
    fn proxy(&self, audit: Option<&AuditHandle>, guard: &Guard) -> &Proxy {
        self.proxy.get_or_init(|| {
            let audit = audit.cloned();
            let sink: declass_egress::Sink = Arc::new(move |e: declass_egress::Event| {
                if let Some(a) = &audit {
                    a.record(AuditEvent::Egress {
                        host: e.host,
                        port: e.port,
                        bytes_up: e.bytes_up,
                        bytes_down: e.bytes_down,
                        outcome: e.outcome.as_str().to_owned(),
                        reason: e.reason,
                    });
                }
            });
            Proxy::new(
                self.rules.clone(),
                self.resolver.clone(),
                sink,
                guard.clone(),
            )
        })
    }

    /// A route for one command.
    pub(crate) async fn route(
        &self,
        kind: SandboxKind,
        audit: Option<&AuditHandle>,
        guard: &Guard,
    ) -> Result<Route, String> {
        self.proxy(audit, guard)
            .route(kind, &self.helper)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The note added to a command's output when the proxy refused requests.
pub(crate) fn refused_note(refused: &[String]) -> Option<String> {
    (!refused.is_empty()).then(|| {
        format!(
            "\n[sandbox] the egress proxy refused: {}. Commands reach only the package registries \
in sandbox.registries (an owner setting).\n",
            refused.join("; ")
        )
    })
}

/// The package caches of a run's commands, under `scratch`. `writable`: the
/// command has network and may fill them; otherwise it only reads what the
/// run's commands already downloaded, and gets them only if there is some.
/// Cargo's home is seeded with links to the crates the operator's own cargo
/// home already holds, so only new ones are downloaded.
pub(crate) fn cache_env(scratch: &Path, writable: bool) -> Vec<(String, String)> {
    let root = scratch.join("pkg");
    let cargo = root.join("cargo");
    let path = |p: PathBuf| p.display().to_string();
    if !writable {
        return if cargo.join("registry/index").is_dir() {
            vec![("CARGO_HOME".into(), path(cargo))]
        } else {
            Vec::new()
        };
    }
    seed_cargo_home(&cargo);
    vec![
        ("CARGO_HOME".into(), path(cargo)),
        ("npm_config_cache".into(), path(root.join("npm"))),
        ("YARN_CACHE_FOLDER".into(), path(root.join("yarn"))),
        ("XDG_CACHE_HOME".into(), path(root.join("xdg"))),
        ("PIP_CACHE_DIR".into(), path(root.join("pip"))),
        ("UV_CACHE_DIR".into(), path(root.join("uv"))),
        ("GOMODCACHE".into(), path(root.join("go/mod"))),
        ("GOCACHE".into(), path(root.join("go/build"))),
        ("GRADLE_USER_HOME".into(), path(root.join("gradle"))),
        (
            "MAVEN_ARGS".into(),
            format!("-Dmaven.repo.local={}", root.join("m2").display()),
        ),
        ("BUN_INSTALL_CACHE_DIR".into(), path(root.join("bun"))),
        ("DENO_DIR".into(), path(root.join("deno"))),
    ]
}

/// The operator's cargo home (`CARGO_HOME`, else `~/.cargo`).
fn operator_cargo_home() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".cargo"))
        })
}

/// Links in `dst` to the operator's cargo configuration and to each crate
/// archive and unpacked source it holds. The links are read-only in effect
/// (commands cannot write the operator's home); new crates land in `dst`.
fn seed_cargo_home(dst: &Path) {
    let Some(src) = operator_cargo_home().filter(|s| s != dst) else {
        return;
    };
    let _ = std::fs::create_dir_all(dst);
    let link = |from: &Path, to: &Path| {
        if std::fs::symlink_metadata(to).is_err() {
            let _ = std::os::unix::fs::symlink(from, to);
        }
    };
    for name in ["config.toml", "config"] {
        if src.join(name).is_file() {
            link(&src.join(name), &dst.join(name));
        }
    }
    for kind in ["cache", "src"] {
        let Ok(registries) = std::fs::read_dir(src.join("registry").join(kind)) else {
            continue;
        };
        for registry in registries.flatten() {
            let into = dst.join("registry").join(kind).join(registry.file_name());
            if std::fs::create_dir_all(&into).is_err() {
                continue;
            }
            for entry in std::fs::read_dir(registry.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                link(&entry.path(), &into.join(entry.file_name()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_home_links_the_operators_crates_and_takes_new_ones() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("scratch");
        // Offline commands get nothing until the run has downloaded something.
        assert!(cache_env(&scratch, false).is_empty());
        let env: std::collections::BTreeMap<_, _> = cache_env(&scratch, true).into_iter().collect();
        let cargo = PathBuf::from(&env["CARGO_HOME"]);
        assert!(cargo.starts_with(&scratch) && cargo.is_dir());
        assert!(env["npm_config_cache"].starts_with(&*scratch.to_string_lossy()));
        if let Some(home) = operator_cargo_home().filter(|h| h.join("registry/cache").is_dir()) {
            let registry = std::fs::read_dir(home.join("registry/cache"))
                .unwrap()
                .flatten()
                .next()
                .unwrap();
            let seeded = cargo.join("registry/cache").join(registry.file_name());
            assert!(seeded.is_dir() && !seeded.is_symlink());
            if let Some(krate) = std::fs::read_dir(registry.path()).unwrap().flatten().next() {
                assert!(seeded.join(krate.file_name()).is_symlink());
            }
        }
        std::fs::create_dir_all(cargo.join("registry/index")).unwrap();
        assert_eq!(
            cache_env(&scratch, false),
            [("CARGO_HOME".into(), env["CARGO_HOME"].clone())]
        );
    }

    #[test]
    fn the_refusal_note_names_hosts_only() {
        assert!(refused_note(&[]).is_none());
        let note = refused_note(&["pastebin.example:443 (not an allowed package registry)".into()])
            .unwrap();
        assert!(note.contains("pastebin.example:443") && note.contains("sandbox.registries"));
    }
}
