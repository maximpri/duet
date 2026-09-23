// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings registry.
//!
//! Every setting is declared once in [`REGISTRY`] with its type, default, scope
//! and direction. The loader, `duet config`, `duet doctor` and the TUI all read
//! this table; reading an unregistered key is an error.
//!
//! Files: owner `~/.config/duet/config.toml` (trusted) and project
//! `.duet/config.toml` (untrusted). A project file may never set owner-only keys
//! and may only change a setting in the direction that tightens privacy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Str,
    List,
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Only the owner's user config may set it (credentials, endpoints, loosening).
    Owner,
    /// A project may set it, subject to `Direction`.
    Project,
}

/// Which changes count as tightening when a project sets the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Any value (the setting does not affect privacy).
    Any,
    /// Lists: a project may only add entries.
    AddOnly,
    /// Booleans: a project may only turn it on.
    OnlyTrue,
    /// Booleans: a project may only turn it off.
    OnlyFalse,
    /// Numbers: a project may only lower it.
    OnlyLower,
}

pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    pub default: &'static str,
    pub scope: Scope,
    pub direction: Direction,
    /// Changing it loosens privacy or spends money; UIs must confirm and show a diff.
    pub confirm: bool,
    pub help: &'static str,
}

macro_rules! s {
    ($key:expr, $kind:expr, $default:expr, $scope:expr, $dir:expr, $confirm:expr, $help:expr) => {
        Setting {
            key: $key,
            kind: $kind,
            default: $default,
            scope: $scope,
            direction: $dir,
            confirm: $confirm,
            help: $help,
        }
    };
}

use Direction::*;
use Kind::*;
use Scope::*;

/// Defaults are TOML literals.
pub const REGISTRY: &[Setting] = &[
    s!(
        "frontier.base_url",
        Str,
        r#""https://api.z.ai/api/coding/paas/v4""#,
        Owner,
        Any,
        true,
        "Frontier endpoint (Chat Completions)."
    ),
    s!(
        "frontier.model",
        Str,
        r#""glm-5.3""#,
        Owner,
        Any,
        true,
        "Frontier model id."
    ),
    s!(
        "frontier.api_key_env",
        Str,
        r#""ZAI_API_KEY""#,
        Owner,
        Any,
        false,
        "Environment variable holding the frontier API key."
    ),
    s!(
        "local.base_url",
        Str,
        r#""http://127.0.0.1:8080/v1""#,
        Owner,
        Any,
        true,
        "Local model endpoint; must be loopback or in local.allowlist."
    ),
    s!(
        "local.model",
        Str,
        r#""omlx-coding""#,
        Owner,
        Any,
        false,
        "Local model id."
    ),
    s!(
        "local.api_key_env",
        Str,
        r#""""#,
        Owner,
        Any,
        false,
        "Environment variable holding the local server key, if any."
    ),
    s!(
        "local.allowlist",
        List,
        "[]",
        Owner,
        Any,
        true,
        "Non-loopback host:port values the local role may use."
    ),
    s!(
        "local.watts",
        Float {
            min: 0.0,
            max: 5000.0
        },
        "120.0",
        Owner,
        Any,
        false,
        "Average draw of the local model host while generating, for cost reports."
    ),
    s!(
        "sensitivity.globs",
        List,
        r#"[".env*", "**/.env*", "*.pem", "*.key", "secrets/**", "data/**", "*.csv", "*.db", "*.sqlite", "*.parquet", "logs/**", "*.log"]"#,
        Project,
        AddOnly,
        true,
        "Paths whose content never reaches the frontier (handle + local summary instead)."
    ),
    s!(
        "sensitivity.protected_paths",
        List,
        "[]",
        Project,
        AddOnly,
        true,
        "Source paths treated as sensitive even though code is normally shared."
    ),
    s!(
        "sensitivity.command_output_sensitive",
        Bool,
        "true",
        Project,
        OnlyTrue,
        true,
        "Treat command output as sensitive by default."
    ),
    s!(
        "sensitivity.raw_ok_commands",
        List,
        r#"["cargo check", "cargo build", "cargo clippy", "tsc"]"#,
        Owner,
        Any,
        true,
        "Commands whose output may be shown raw (still scanned)."
    ),
    s!(
        "sensitivity.secret_sinks",
        List,
        r#"[".env*", "**/.env*", "config/*.toml"]"#,
        Owner,
        Any,
        true,
        "Files where resolved secret values may be written."
    ),
    s!(
        "sensitivity.detect_secrets",
        Bool,
        "true",
        Project,
        OnlyTrue,
        true,
        "Secret detectors (key formats, credential assignments)."
    ),
    s!(
        "sensitivity.detect_pii",
        Bool,
        "true",
        Project,
        OnlyTrue,
        true,
        "Personal-data detectors (email, phone, card, national ids, IBAN, IP)."
    ),
    s!(
        "sensitivity.detect_entropy",
        Bool,
        "true",
        Project,
        OnlyTrue,
        true,
        "High-entropy strings next to key-like names."
    ),
    s!(
        "sensitivity.bulky_tokens",
        Int {
            min: 256,
            max: 1_000_000
        },
        "2000",
        Owner,
        Any,
        false,
        "Public results larger than this become a handle + summary."
    ),
    s!(
        "ip.interface_only",
        List,
        "[]",
        Project,
        AddOnly,
        true,
        "Paths the frontier sees as signatures only; bodies are withheld."
    ),
    s!(
        "ip.sealed",
        List,
        "[]",
        Project,
        AddOnly,
        true,
        "Paths whose existence only is visible to the frontier."
    ),
    s!(
        "limits.frontier_usd",
        Float {
            min: 0.0,
            max: 10_000.0
        },
        "5.0",
        Project,
        OnlyLower,
        true,
        "Maximum frontier spend per run (list price)."
    ),
    s!(
        "limits.wall_clock_minutes",
        Int {
            min: 1,
            max: 24 * 60
        },
        "120",
        Project,
        OnlyLower,
        false,
        "Maximum run duration."
    ),
    s!(
        "limits.max_finish_attempts",
        Int { min: 1, max: 20 },
        "4",
        Project,
        Any,
        false,
        "How many times the frontier may call finish before the run fails."
    ),
    s!(
        "limits.command_timeout_seconds",
        Int { min: 5, max: 7200 },
        "600",
        Project,
        Any,
        false,
        "Timeout for each sandboxed command."
    ),
    s!(
        "checks.commands",
        List,
        "[]",
        Project,
        Any,
        false,
        "Commands the host runs (sandboxed) when the frontier calls finish; all must pass."
    ),
    s!(
        "context.window_tokens",
        Int {
            min: 8_000,
            max: 2_000_000
        },
        "200000",
        Owner,
        Any,
        false,
        "Frontier context window used for masking decisions."
    ),
    s!(
        "context.mask_at",
        Float {
            min: 0.3,
            max: 0.95
        },
        "0.7",
        Owner,
        Any,
        false,
        "Fraction of the frontier window at which old tool results are masked."
    ),
    s!(
        "sandbox.network",
        Bool,
        "false",
        Project,
        OnlyFalse,
        true,
        "Allow network access for sandboxed commands."
    ),
    s!(
        "data.retention_days",
        Int { min: 0, max: 3650 },
        "14",
        Project,
        OnlyLower,
        false,
        "Days to keep raw run data (handles, transcripts, vault)."
    ),
    s!(
        "data.audit_retention_days",
        Int { min: 1, max: 3650 },
        "90",
        Owner,
        Any,
        false,
        "Days to keep audit logs."
    ),
];

pub fn setting(key: &str) -> Option<&'static Setting> {
    REGISTRY.iter().find(|s| s.key == key)
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("unknown setting {0}")]
    Unknown(String),
    #[error("{file}: {key} can only be set in the owner's config")]
    OwnerOnly { file: String, key: String },
    #[error("{file}: {key} = {value} would loosen privacy; a project may only {rule}")]
    Loosening {
        file: String,
        key: String,
        value: String,
        rule: &'static str,
    },
    #[error("{key}: {message}")]
    Invalid { key: String, message: String },
    #[error("{0}: {1}")]
    Parse(String, String),
    #[error(transparent)]
    Fs(#[from] duet_fs::FsError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Default,
    Owner,
    Project,
}

#[derive(Debug, Clone)]
pub struct Config {
    values: BTreeMap<&'static str, (Value, Origin)>,
    pub owner_path: PathBuf,
    pub project_path: Option<PathBuf>,
}

fn default_value(s: &Setting) -> Value {
    toml::from_str::<toml::Table>(&format!("v = {}", s.default)).expect("registry default parses")["v"].clone()
}

fn validate(s: &Setting, v: &Value) -> Result<(), ConfigError> {
    let bad = |m: &str| {
        Err(ConfigError::Invalid {
            key: s.key.into(),
            message: m.into(),
        })
    };
    match (s.kind, v) {
        (Bool, Value::Boolean(_)) | (Str, Value::String(_)) => Ok(()),
        (Int { min, max }, Value::Integer(i)) if (min..=max).contains(i) => Ok(()),
        (Int { min, max }, Value::Integer(_)) => bad(&format!("must be between {min} and {max}")),
        (Float { min, max }, Value::Float(f)) if *f >= min && *f <= max => Ok(()),
        (Float { min, max }, Value::Integer(i)) if (*i as f64) >= min && (*i as f64) <= max => {
            Ok(())
        }
        (Float { min, max }, _) => bad(&format!("must be a number between {min} and {max}")),
        (List, Value::Array(a)) if a.iter().all(Value::is_str) => Ok(()),
        (Choice(opts), Value::String(x)) if opts.contains(&x.as_str()) => Ok(()),
        (Choice(opts), _) => bad(&format!("must be one of {opts:?}")),
        _ => bad("wrong type"),
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// Whether a project value is allowed relative to `base` (default or owner value).
fn tightens(s: &Setting, base: &Value, new: &Value) -> Result<(), &'static str> {
    match s.direction {
        Any => Ok(()),
        AddOnly => {
            let base: Vec<&str> = base
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let new: Vec<&str> = new
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if base.iter().all(|b| new.contains(b)) {
                Ok(())
            } else {
                Err("add entries, never remove them")
            }
        }
        OnlyTrue => {
            if new.as_bool() == Some(true) || base.as_bool() == new.as_bool() {
                Ok(())
            } else {
                Err("turn it on")
            }
        }
        OnlyFalse => {
            if new.as_bool() == Some(false) || base.as_bool() == new.as_bool() {
                Ok(())
            } else {
                Err("turn it off")
            }
        }
        OnlyLower => match (as_f64(base), as_f64(new)) {
            (Some(b), Some(n)) if n <= b => Ok(()),
            _ => Err("lower it"),
        },
    }
}

fn flatten(prefix: &str, table: &toml::Table, out: &mut Vec<(String, Value)>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Table(t) => flatten(&key, t, out),
            other => out.push((key, other.clone())),
        }
    }
}

fn read_file(path: &Path) -> Result<Vec<(String, Value)>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(ConfigError::Parse(
                path.display().to_string(),
                e.to_string(),
            ));
        }
    };
    let table: toml::Table = toml::from_str(&text)
        .map_err(|e| ConfigError::Parse(path.display().to_string(), e.to_string()))?;
    let mut out = Vec::new();
    flatten("", &table, &mut out);
    Ok(out)
}

/// Default owner config location (`$DUET_CONFIG_HOME/config.toml` or `~/.config/duet/config.toml`).
pub fn owner_config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("DUET_CONFIG_HOME") {
        return PathBuf::from(dir).join("config.toml");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/duet/config.toml")
}

impl Config {
    /// Merges defaults, the owner file and (optionally) a project file.
    pub fn load(owner_path: &Path, project_path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut values: BTreeMap<&'static str, (Value, Origin)> = REGISTRY
            .iter()
            .map(|s| (s.key, (default_value(s), Origin::Default)))
            .collect();
        for (key, v) in read_file(owner_path)? {
            let s = setting(&key).ok_or_else(|| {
                ConfigError::Unknown(format!("{} in {}", key, owner_path.display()))
            })?;
            validate(s, &v)?;
            values.insert(s.key, (v, Origin::Owner));
        }
        if let Some(pp) = project_path {
            let file = pp.display().to_string();
            for (key, v) in read_file(pp)? {
                let s = setting(&key)
                    .ok_or_else(|| ConfigError::Unknown(format!("{key} in {file}")))?;
                if s.scope == Owner {
                    return Err(ConfigError::OwnerOnly { file, key });
                }
                validate(s, &v)?;
                let base = &values[s.key].0;
                tightens(s, base, &v).map_err(|rule| ConfigError::Loosening {
                    file: file.clone(),
                    key: key.clone(),
                    value: v.to_string(),
                    rule,
                })?;
                values.insert(s.key, (v, Origin::Project));
            }
        }
        Ok(Self {
            values,
            owner_path: owner_path.to_path_buf(),
            project_path: project_path.map(Path::to_path_buf),
        })
    }

    pub fn value(&self, key: &str) -> Result<&Value, ConfigError> {
        self.values
            .get(key)
            .map(|(v, _)| v)
            .ok_or_else(|| ConfigError::Unknown(key.into()))
    }

    pub fn origin(&self, key: &str) -> Option<Origin> {
        self.values.get(key).map(|(_, o)| *o)
    }

    pub fn str(&self, key: &str) -> Result<String, ConfigError> {
        Ok(self.value(key)?.as_str().unwrap_or_default().to_owned())
    }
    pub fn bool(&self, key: &str) -> Result<bool, ConfigError> {
        Ok(self.value(key)?.as_bool().unwrap_or(false))
    }
    pub fn int(&self, key: &str) -> Result<i64, ConfigError> {
        Ok(self.value(key)?.as_integer().unwrap_or(0))
    }
    pub fn float(&self, key: &str) -> Result<f64, ConfigError> {
        Ok(as_f64(self.value(key)?).unwrap_or(0.0))
    }
    pub fn list(&self, key: &str) -> Result<Vec<String>, ConfigError> {
        Ok(self
            .value(key)?
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect())
    }

    /// Sets a value in the owner file (validated; written atomically).
    pub fn set_owner(&mut self, key: &str, value: Value) -> Result<(), ConfigError> {
        let s = setting(key).ok_or_else(|| ConfigError::Unknown(key.into()))?;
        validate(s, &value)?;
        let mut entries = read_file(&self.owner_path)?;
        entries.retain(|(k, _)| k != key);
        entries.push((key.to_owned(), value.clone()));
        write_entries(&self.owner_path, &entries)?;
        self.values.insert(s.key, (value, Origin::Owner));
        Ok(())
    }

    /// Sets a value in the project file; refused unless the setting is
    /// project-scoped and the change tightens privacy.
    pub fn set_project(&mut self, key: &str, value: Value) -> Result<(), ConfigError> {
        let path = self
            .project_path
            .clone()
            .ok_or_else(|| ConfigError::Invalid {
                key: key.into(),
                message: "no project".into(),
            })?;
        let s = setting(key).ok_or_else(|| ConfigError::Unknown(key.into()))?;
        let file = path.display().to_string();
        if s.scope == Owner {
            return Err(ConfigError::OwnerOnly {
                file,
                key: key.into(),
            });
        }
        validate(s, &value)?;
        tightens(s, &self.values[s.key].0, &value).map_err(|rule| ConfigError::Loosening {
            file,
            key: key.into(),
            value: value.to_string(),
            rule,
        })?;
        let mut entries = read_file(&path)?;
        entries.retain(|(k, _)| k != key);
        entries.push((key.to_owned(), value.clone()));
        write_entries(&path, &entries)?;
        self.values.insert(s.key, (value, Origin::Project));
        Ok(())
    }
}

fn write_entries(path: &Path, entries: &[(String, Value)]) -> Result<(), ConfigError> {
    let mut root = toml::Table::new();
    for (key, v) in entries {
        let mut parts: Vec<&str> = key.split('.').collect();
        let last = parts.pop().unwrap_or_default();
        let mut t = &mut root;
        for p in parts {
            t = t
                .entry(p)
                .or_insert_with(|| Value::Table(toml::Table::new()))
                .as_table_mut()
                .expect("table");
        }
        t.insert(last.to_owned(), v.clone());
    }
    if let Some(parent) = path.parent() {
        duet_fs::private::ensure_private_dir(parent)?;
    }
    let text = toml::to_string_pretty(&root)
        .map_err(|e| ConfigError::Parse(path.display().to_string(), e.to_string()))?;
    duet_fs::private::write_private(path, text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(owner: &str, project: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let (o, p) = (d.path().join("owner.toml"), d.path().join("project.toml"));
        std::fs::write(&o, owner).unwrap();
        std::fs::write(&p, project).unwrap();
        (d, o, p)
    }

    #[test]
    fn every_default_is_valid() {
        for s in REGISTRY {
            validate(s, &default_value(s)).unwrap_or_else(|e| panic!("{}: {e}", s.key));
        }
        let mut keys: Vec<_> = REGISTRY.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), REGISTRY.len(), "duplicate keys");
    }

    #[test]
    fn owner_and_project_merge_with_origins() {
        let (_d, o, p) = files(
            "[frontier]\nmodel = \"glm-5.3-flash\"\n",
            "[sensitivity]\nprotected_paths = [\"src/pricing/**\"]\n[limits]\nfrontier_usd = 2.0\n",
        );
        let c = Config::load(&o, Some(&p)).unwrap();
        assert_eq!(c.str("frontier.model").unwrap(), "glm-5.3-flash");
        assert_eq!(c.origin("frontier.model"), Some(Origin::Owner));
        assert_eq!(
            c.list("sensitivity.protected_paths").unwrap(),
            vec!["src/pricing/**"]
        );
        assert_eq!(c.float("limits.frontier_usd").unwrap(), 2.0);
        assert_eq!(c.origin("data.retention_days"), Some(Origin::Default));
        assert!(c.value("no.such.key").is_err());
    }

    #[test]
    fn project_cannot_set_owner_only_keys() {
        for bad in [
            "[frontier]\nbase_url = \"https://evil.example/v1\"\n",
            "[frontier]\napi_key_env = \"HOME\"\n",
            "[local]\nbase_url = \"https://cloud.example/v1\"\n",
            "[local]\nallowlist = [\"evil.example:443\"]\n",
            "[sensitivity]\nraw_ok_commands = [\"cat\"]\n",
        ] {
            let (_d, o, p) = files("", bad);
            assert!(
                matches!(
                    Config::load(&o, Some(&p)),
                    Err(ConfigError::OwnerOnly { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn project_cannot_loosen() {
        for bad in [
            "[sensitivity]\nglobs = [\"*.pem\"]\n",
            "[sensitivity]\ndetect_pii = false\n",
            "[sandbox]\nnetwork = true\n",
            "[limits]\nfrontier_usd = 50.0\n",
            "[data]\nretention_days = 365\n",
        ] {
            let (_d, o, p) = files("", bad);
            assert!(
                matches!(
                    Config::load(&o, Some(&p)),
                    Err(ConfigError::Loosening { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn project_may_add_globs() {
        let (_d, o, p) = files("", "");
        let mut c = Config::load(&o, Some(&p)).unwrap();
        let mut globs = c.list("sensitivity.globs").unwrap();
        globs.push("reports/**".into());
        c.set_project(
            "sensitivity.globs",
            Value::Array(globs.iter().cloned().map(Value::String).collect()),
        )
        .unwrap();
        let reloaded = Config::load(&o, Some(&p)).unwrap();
        assert!(
            reloaded
                .list("sensitivity.globs")
                .unwrap()
                .contains(&"reports/**".to_string())
        );
    }

    #[test]
    fn set_owner_validates_and_persists() {
        let (_d, o, p) = files("", "");
        let mut c = Config::load(&o, Some(&p)).unwrap();
        assert!(
            c.set_owner("limits.wall_clock_minutes", Value::Integer(0))
                .is_err()
        );
        c.set_owner(
            "local.allowlist",
            Value::Array(vec![Value::String("192.168.50.132:8080".into())]),
        )
        .unwrap();
        assert_eq!(
            Config::load(&o, None)
                .unwrap()
                .list("local.allowlist")
                .unwrap(),
            vec!["192.168.50.132:8080"]
        );
    }

    #[test]
    fn unknown_keys_in_files_are_errors() {
        let (_d, o, p) = files("[frontier]\nmodle = \"x\"\n", "");
        assert!(matches!(
            Config::load(&o, Some(&p)),
            Err(ConfigError::Unknown(_))
        ));
    }
}
