//! Settings for the payments service.

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

    pub fn from_sources(_env_file: &Path, _config_file: &Path) -> Result<Settings, ConfigError> {
        todo!("load settings from the dotenv and config files")
    }
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("payments_api_key", &self.payments_api_key)
            .field("ledger_db_password", &self.ledger_db_password)
            .field("webhook_signing_secret", &self.webhook_signing_secret)
            .field("port", &self.port)
            .field("log_level", &self.log_level)
            .finish()
    }
}
