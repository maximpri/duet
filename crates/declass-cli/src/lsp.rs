// SPDX-License-Identifier: GPL-3.0-or-later
//! The run's language servers, from the `lsp.*` settings.

use anyhow::Result;
use declass_config::Config;
use declass_lsp::{Lsp, ServerConfig, Settings};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The template key that names every configured language.
const SERVERS: &str = "lsp.servers.*.command";

/// The `lsp.*` settings, with each `[lsp.servers.<language>]` checked.
pub fn settings(cfg: &Config) -> Result<Settings> {
    let mut servers = std::collections::BTreeMap::new();
    for lang in cfg.instances(SERVERS) {
        let key = |field: &str| format!("lsp.servers.{lang}.{field}");
        let server = ServerConfig {
            command: cfg.str(&key("command"))?,
            args: cfg.list(&key("args"))?,
            extensions: cfg
                .list(&key("extensions"))?
                .into_iter()
                .map(|e| e.trim_start_matches('.').to_owned())
                .collect(),
            env: cfg.list(&key("env"))?,
        };
        server.check(&lang).map_err(anyhow::Error::msg)?;
        servers.insert(lang, server);
    }
    Ok(Settings {
        enabled: cfg.bool("lsp.enabled")?,
        servers,
        request_timeout: Duration::from_secs(cfg.int("lsp.request_timeout_seconds")? as u64),
        diagnostics_wait: Duration::from_millis(cfg.int("lsp.diagnostics_wait_ms")? as u64),
    })
}

/// The run's language servers, or `None` when `lsp.enabled` is off or no
/// server is installed. Nothing is started until a tool first needs it.
pub fn servers(
    cfg: &Config,
    ws: &Path,
    run_dir: &Path,
    sandbox: declass_sandbox::SandboxKind,
) -> Result<Option<Arc<Lsp>>> {
    let s = settings(cfg)?;
    Ok(declass_agent::code_nav::language_servers(&s, ws, run_dir, sandbox).map(Arc::new))
}
