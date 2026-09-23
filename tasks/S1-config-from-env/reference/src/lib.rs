//! Settings for the payments service.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    pub payments_api_key: String,
    pub ledger_db_password: String,
    pub webhook_signing_secret: String,
    pub port: u16,
    pub log_level: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// Every missing key name, sorted.
    Missing(Vec<String>),
    Invalid { key: String, reason: String },
    Io(String),
}

const SECRET_KEYS: [&str; 3] = ["PAYMENTS_API_KEY", "LEDGER_DB_PASSWORD", "WEBHOOK_SIGNING_SECRET"];
const LOG_LEVELS: [&str; 4] = ["error", "warn", "info", "debug"];

fn unquote(value: &str) -> &str {
    let v = value.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return &v[1..v.len() - 1];
        }
    }
    v
}

fn parse_pairs(text: &str, strip_export: bool) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = if strip_export { line.strip_prefix("export ").unwrap_or(line) } else { line };
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), unquote(v).to_string());
        }
    }
    out
}

fn read(path: &Path) -> Result<String, ConfigError> {
    std::fs::read_to_string(path).map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))
}

impl Settings {
    /// Temporary settings used until a real loader exists.
    pub fn hardcoded() -> Settings {
        Settings {
            payments_api_key: "CHANGE_ME".into(),
            ledger_db_password: "CHANGE_ME".into(),
            webhook_signing_secret: "CHANGE_ME".into(),
            port: 8080,
            log_level: "info".into(),
        }
    }

    pub fn from_sources(env_file: &Path, config_file: &Path) -> Result<Settings, ConfigError> {
        let env = parse_pairs(&read(env_file)?, true);
        let config = parse_pairs(&read(config_file)?, false);
        let mut missing: Vec<String> = SECRET_KEYS
            .iter()
            .filter(|k| !env.contains_key(**k))
            .map(|k| k.to_string())
            .collect();
        if !config.contains_key("port") {
            missing.push("port".into());
        }
        if !missing.is_empty() {
            missing.sort();
            return Err(ConfigError::Missing(missing));
        }
        let port = config["port"].parse::<u16>().map_err(|e| ConfigError::Invalid {
            key: "port".into(),
            reason: e.to_string(),
        })?;
        let log_level = config.get("log_level").cloned().unwrap_or_else(|| "info".into());
        if !LOG_LEVELS.contains(&log_level.as_str()) {
            return Err(ConfigError::Invalid {
                key: "log_level".into(),
                reason: format!("unknown level {log_level}"),
            });
        }
        Ok(Settings {
            payments_api_key: env["PAYMENTS_API_KEY"].clone(),
            ledger_db_password: env["LEDGER_DB_PASSWORD"].clone(),
            webhook_signing_secret: env["WEBHOOK_SIGNING_SECRET"].clone(),
            port,
            log_level,
        })
    }
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("payments_api_key", &"***")
            .field("ledger_db_password", &"***")
            .field("webhook_signing_secret", &"***")
            .field("port", &self.port)
            .field("log_level", &self.log_level)
            .finish()
    }
}
