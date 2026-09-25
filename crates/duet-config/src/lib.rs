// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings registry.
//!
//! Every setting is declared once in [`REGISTRY`] with its type, default, scope
//! and direction. The loader and `duet config` read this table (as will
//! `duet doctor` and the TUI); reading an unregistered key is an error.
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
    Int {
        min: i64,
        max: i64,
    },
    Float {
        min: f64,
        max: f64,
    },
    Str,
    List,
    /// A list of regular expressions; every entry must compile.
    Patterns,
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
    /// Choices: only toward a later option (the options are listed from the
    /// least to the most strict).
    OnlyLaterChoice,
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
        "Frontier endpoint base URL; the path follows frontier.dialect."
    ),
    s!(
        "frontier.model",
        Str,
        r#""glm-5.3-flash""#,
        Owner,
        Any,
        true,
        "Frontier model id."
    ),
    s!(
        "frontier.reasoning_effort",
        Choice(&["default", "low", "medium", "high"]),
        r#""medium""#,
        Owner,
        Any,
        false,
        "Reasoning effort requested from the frontier (`default` sends none and lets the provider choose)."
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
        "frontier.dialect",
        Choice(&["chat", "anthropic", "responses"]),
        r#""chat""#,
        Owner,
        Any,
        false,
        "API the frontier endpoint speaks: `chat` (OpenAI-compatible Chat Completions), `anthropic` (Anthropic Messages) or `responses` (OpenAI Responses)."
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
        "local.allow_plaintext",
        Bool,
        "false",
        Owner,
        OnlyFalse,
        true,
        "Allow plain HTTP to an allowlisted non-loopback local model host (sensitive content crosses the network unencrypted); prefer TLS or an SSH tunnel."
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
        "sensitivity.custom_patterns",
        Patterns,
        "[]",
        Project,
        AddOnly,
        true,
        "Regular expressions for your own sensitive values (customer ids, internal hostnames); every match becomes a `data` placeholder in sensitive and public text."
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
        "sensitivity.local_brief",
        Bool,
        "false",
        Owner,
        Any,
        false,
        "At run start the local model reads the sensitive files against the task and briefs the frontier (values withheld)."
    ),
    s!(
        "sensitivity.bulky_file_tokens",
        Int {
            min: 256,
            max: 1_000_000
        },
        "12000",
        Owner,
        Any,
        false,
        "A public file the model asked to read is shown whole up to this size; larger files become a handle with an outline."
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
        "session.frontier_usd",
        Float {
            min: 0.0,
            max: 10_000.0
        },
        "20.0",
        Project,
        OnlyLower,
        true,
        "Maximum frontier spend over a whole session (`duet chat`); each turn is also held to limits.frontier_usd."
    ),
    s!(
        "session.wall_clock_minutes",
        Int {
            min: 1,
            max: 7 * 24 * 60
        },
        "480",
        Project,
        OnlyLower,
        false,
        "Maximum time the agent works over a whole session (waiting for the operator does not count); each turn is also held to limits.wall_clock_minutes."
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
        "web.enabled",
        Bool,
        "true",
        Project,
        OnlyFalse,
        true,
        "Offer the web tools (`web_fetch`, and `web_search` when a backend is set). Requests are made by the host, GET only, to public addresses only; pages are scanned like public content and shown as untrusted data. Commands keep no network either way."
    ),
    s!(
        "web.search.backend",
        Choice(&["brave", "searxng", "none"]),
        r#""none""#,
        Owner,
        OnlyLaterChoice,
        true,
        "Search backend for `web_search`: `searxng` (your own instance, web.search.searxng_url), `brave` (Brave Search API, key from web.search.brave_key_env) or `none` (no web_search tool). Queries are checked for sensitive values before they are sent."
    ),
    s!(
        "web.search.searxng_url",
        Str,
        r#""""#,
        Owner,
        Any,
        true,
        "Base URL of a SearXNG instance with the JSON format enabled (for example http://127.0.0.1:8888); queries go to <url>/search."
    ),
    s!(
        "web.search.brave_key_env",
        Str,
        r#""BRAVE_API_KEY""#,
        Owner,
        Any,
        false,
        "Environment variable holding the Brave Search API key (the key itself is never stored)."
    ),
    s!(
        "web.allowlist_private",
        List,
        "[]",
        Owner,
        Any,
        true,
        "Private hosts `web_fetch` may reach despite the public-address rule: host names, `*.domain`, IP addresses or CIDR networks (for example wiki.corp, 10.20.0.0/16). Cloud metadata addresses stay refused."
    ),
    s!(
        "web.max_bytes",
        Int {
            min: 1024,
            max: 50_000_000
        },
        "5000000",
        Project,
        OnlyLower,
        false,
        "Most bytes read of one web response; the rest is not downloaded and the page is marked truncated."
    ),
    s!(
        "web.timeout_secs",
        Int { min: 1, max: 300 },
        "30",
        Project,
        Any,
        false,
        "Seconds one web request (with its redirects) may take."
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
    s!(
        "oversight.approve",
        Choice(&["off", "risky", "all"]),
        r#""off""#,
        Owner,
        OnlyLaterChoice,
        true,
        "Ask the operator at the terminal before an action: `risky` (sensitive_data commands, edit_protected, writes outside source/test files) or `all` (every command and write). Runs without a terminal then refuse to start."
    ),
    s!(
        "git.commit",
        Choice(&["allow", "ask", "off"]),
        r#""ask""#,
        Project,
        OnlyLaterChoice,
        true,
        "Whether the frontier may commit files it wrote in the run (git_commit; never sensitive or protected files, never a push). `ask`: each commit waits for the operator's approval, so git_commit is offered only when oversight.approve is risky or all. `allow`: commits without asking (audited). `off`: never offered."
    ),
    s!(
        "git.author",
        Str,
        r#""""#,
        Owner,
        Any,
        false,
        "Author and committer of git_commit commits, as `Name <email>`. Empty: user.name and user.email from the repository's git config, else from ~/.gitconfig or ~/.config/git/config (only those two keys are read); without either, git_commit refuses."
    ),
    // MCP servers: one `[mcp.servers.<name>]` table per server; `*` stands
    // for the name (letters, digits, `_`, `-`; at most 32 characters).
    s!(
        "mcp.servers.*.command",
        Str,
        r#""""#,
        Owner,
        Any,
        true,
        "Program that starts a stdio MCP server, run in the command sandbox with the workspace as working directory (same hidden paths as commands). Set this or url."
    ),
    s!(
        "mcp.servers.*.args",
        List,
        "[]",
        Owner,
        Any,
        true,
        "Arguments of the server's command."
    ),
    s!(
        "mcp.servers.*.url",
        Str,
        r#""""#,
        Owner,
        Any,
        true,
        "Streamable HTTP endpoint of a remote MCP server (https, or http to a loopback address). Set this or command."
    ),
    s!(
        "mcp.servers.*.env",
        List,
        "[]",
        Owner,
        Any,
        true,
        "Names of environment variables passed to a stdio server; every other variable is cleared (except the sandbox's base set: PATH, HOME, locale). Values are never stored."
    ),
    s!(
        "mcp.servers.*.headers_env",
        List,
        "[]",
        Owner,
        Any,
        true,
        "HTTP headers taken from environment variables, as `Header=VARIABLE` (the variable holds the whole value, e.g. `Bearer ...`)."
    ),
    s!(
        "mcp.servers.*.trust",
        Choice(&["public", "sensitive"]),
        r#""public""#,
        Owner,
        Any,
        true,
        "`public`: results are scanned and shown to the frontier, and arguments holding placeholders or sensitive values are refused. `sensitive`: results stay on this machine (handle + local summary); for a stdio server, placeholders in arguments are resolved to their values."
    ),
    s!(
        "mcp.servers.*.network",
        Bool,
        "false",
        Owner,
        Any,
        true,
        "Network access for a stdio server's sandbox."
    ),
    s!(
        "mcp.servers.*.approve",
        Choice(&["auto", "writes", "always"]),
        r#""writes""#,
        Owner,
        OnlyLaterChoice,
        true,
        "Which of the server's tools need approval when oversight.approve is `risky`: `writes` (tools not declared read-only), `always` (every tool) or `auto` (none). With `all`, every call is asked."
    ),
    s!(
        "mcp.servers.*.enabled",
        Bool,
        "true",
        Owner,
        Any,
        true,
        "Start the server for runs."
    ),
    s!(
        "mcp.servers.*.timeout_seconds",
        Int { min: 1, max: 3600 },
        "60",
        Owner,
        Any,
        false,
        "Limit for starting the server and for each tool call."
    ),
];

/// The setting `key` names: a registered key, or an instance of a template
/// key (`mcp.servers.*.command` for `mcp.servers.files.command`).
pub fn setting(key: &str) -> Option<&'static Setting> {
    REGISTRY
        .iter()
        .find(|s| s.key == key)
        .or_else(|| REGISTRY.iter().find(|s| instance_of(s.key, key)))
}

/// Whether a registry key stands for many settings (it holds a `*` segment).
pub fn is_template(key: &str) -> bool {
    key.split('.').any(|p| p == "*")
}

/// Names that may fill a template's `*`.
fn valid_instance_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn instance_of(template: &str, key: &str) -> bool {
    if !is_template(template) {
        return false;
    }
    let (t, k): (Vec<&str>, Vec<&str>) = (template.split('.').collect(), key.split('.').collect());
    t.len() == k.len()
        && t.iter().zip(&k).all(|(t, k)| {
            if *t == "*" {
                valid_instance_name(k)
            } else {
                t == k
            }
        })
}

fn template_slot(template: &str) -> Option<usize> {
    template.split('.').position(|p| p == "*")
}

/// `template` with its `*` filled by the name `instance` (a concrete key of
/// any template with the same prefix) holds there.
fn instantiate(template: &str, instance: &str) -> Option<String> {
    let t: Vec<&str> = template.split('.').collect();
    let at = t.iter().position(|p| *p == "*")?;
    let k: Vec<&str> = instance.split('.').collect();
    if k.len() <= at || t[..at] != k[..at] {
        return None;
    }
    let mut out: Vec<&str> = t.clone();
    out[at] = k[at];
    Some(out.join("."))
}

/// The setting a change to `key` targets; a template itself cannot be set.
fn concrete(key: &str) -> Result<&'static Setting, ConfigError> {
    if is_template(key) {
        return Err(ConfigError::Invalid {
            key: key.into(),
            message: "name the instance in place of `*` (for example mcp.servers.files.command)"
                .into(),
        });
    }
    setting(key).ok_or_else(|| ConfigError::Unknown(key.into()))
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
    #[error("{key}: {old} -> {new} loosens privacy ({weakens}); re-run with --confirm to apply it")]
    NeedsConfirm {
        key: String,
        old: String,
        new: String,
        weakens: String,
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
    /// Every setting by key; a template's instances under their own keys.
    values: BTreeMap<String, (Value, Origin)>,
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
        (Patterns, Value::Array(a)) if a.iter().all(Value::is_str) => {
            for p in a.iter().filter_map(Value::as_str) {
                if p.is_empty() {
                    return bad("an empty pattern would match everywhere");
                }
                if let Err(e) = regex::Regex::new(p) {
                    return bad(&format!("{p:?} is not a valid regular expression: {e}"));
                }
            }
            Ok(())
        }
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
        OnlyLaterChoice => {
            let Choice(opts) = s.kind else {
                return Err("choose a stricter option");
            };
            let rank = |v: &Value| v.as_str().and_then(|x| opts.iter().position(|o| *o == x));
            match (rank(base), rank(new)) {
                (Some(b), Some(n)) if n >= b => Ok(()),
                _ => Err("choose a stricter option"),
            }
        }
    }
}

/// What changing `s` from `old` to `new` would weaken, or `None` when the
/// change keeps or tightens privacy. A change against the setting's tighten
/// direction always loosens; for a setting without a direction, any change to a
/// `confirm` setting counts (it redirects content or spends money).
pub fn loosening(s: &Setting, old: &Value, new: &Value) -> Option<String> {
    if old == new {
        return None;
    }
    match tightens(s, old, new) {
        Err(rule) => Some(format!("{}; tightening would {rule}", s.help)),
        Ok(()) if s.direction == Any && s.confirm => Some(s.help.to_owned()),
        Ok(()) => None,
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

/// The owner's private state directory (audit anchors, the config audit log),
/// outside every workspace: `$DUET_CONFIG_HOME/state` when that is set, else
/// `$XDG_STATE_HOME/duet`, else `~/.local/state/duet`.
pub fn owner_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DUET_CONFIG_HOME") {
        return PathBuf::from(dir).join("state");
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("duet");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local/state/duet")
}

/// The owner's log of settings changes (`config-audit.jsonl`) in `state_dir`.
pub fn config_audit_log(state_dir: &Path) -> PathBuf {
    state_dir.join("config-audit.jsonl")
}

/// Parses a value written as a TOML literal (`5.0`, `true`, `"text"`, `["a", "b"]`).
pub fn parse_value(text: &str) -> Result<Value, ConfigError> {
    toml::from_str::<toml::Table>(&format!("v = {text}"))
        .ok()
        .and_then(|mut t| t.remove("v"))
        .ok_or_else(|| ConfigError::Parse(text.into(), "not a TOML value".into()))
}

/// Which file a change is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The owner's config (trusted): any registered key; loosening needs confirmation.
    Owner,
    /// The project's `.duet/config.toml`: project-scoped keys, tightening only.
    Project,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Owner => "owner",
            Target::Project => "project",
        }
    }
}

/// A checked but unapplied change: the effective value it replaces and, when
/// it loosens privacy, what it weakens (then applying it needs confirmation).
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub key: String,
    pub target: Target,
    pub old: Value,
    pub new: Value,
    pub weakens: Option<String>,
}

/// An applied owner-config change, for the owner's audit log.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub key: String,
    pub old: Value,
    pub new: Value,
    /// What it weakened, when it loosened privacy (then it was confirmed).
    pub weakens: Option<String>,
}

impl Config {
    /// Merges defaults, the owner file and (optionally) a project file.
    pub fn load(owner_path: &Path, project_path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut values: BTreeMap<String, (Value, Origin)> = REGISTRY
            .iter()
            .filter(|s| !is_template(s.key))
            .map(|s| (s.key.to_owned(), (default_value(s), Origin::Default)))
            .collect();
        for (key, v) in read_file(owner_path)? {
            let s = setting(&key)
                .filter(|_| !is_template(&key))
                .ok_or_else(|| {
                    ConfigError::Unknown(format!("{} in {}", key, owner_path.display()))
                })?;
            validate(s, &v)?;
            values.insert(key, (v, Origin::Owner));
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
                values.insert(key, (v, Origin::Project));
            }
        }
        // Every field of a configured instance has a value.
        let instances: Vec<String> = values
            .keys()
            .filter(|k| REGISTRY.iter().any(|s| instance_of(s.key, k)))
            .cloned()
            .collect();
        for key in instances {
            for s in REGISTRY.iter().filter(|s| is_template(s.key)) {
                if let Some(field) = instantiate(s.key, &key) {
                    values
                        .entry(field)
                        .or_insert_with(|| (default_value(s), Origin::Default));
                }
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

    /// The configured instances of a template key, by the name in place of
    /// `*` (`mcp.servers.*.command` gives the server names), sorted.
    pub fn instances(&self, template: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .values
            .keys()
            .filter(|k| setting(k).is_some_and(|s| is_template(s.key)))
            .filter_map(|k| {
                instantiate(template, k).map(|_| k.split('.').nth(template_slot(template)?))
            })
            .flatten()
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The effective value of a setting to be changed (its default when unset).
    fn current(&self, key: &str, s: &Setting) -> Value {
        self.values
            .get(key)
            .map_or_else(|| default_value(s), |(v, _)| v.clone())
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

    /// Checks a change without writing it. The project file refuses owner-only
    /// keys and any loosening outright; for the owner file the proposal says
    /// what the change would weaken (see [`loosening`]).
    pub fn propose(
        &self,
        target: Target,
        key: &str,
        value: Value,
    ) -> Result<Proposal, ConfigError> {
        let s = concrete(key)?;
        validate(s, &value)?;
        let old = self.current(key, s);
        let weakens = match target {
            Target::Owner => loosening(s, &old, &value),
            Target::Project => {
                let file = self
                    .project_path
                    .as_ref()
                    .map_or("project config".into(), |p| p.display().to_string());
                if s.scope == Owner {
                    return Err(ConfigError::OwnerOnly {
                        file,
                        key: key.into(),
                    });
                }
                tightens(s, &old, &value).map_err(|rule| ConfigError::Loosening {
                    file,
                    key: key.into(),
                    value: value.to_string(),
                    rule,
                })?;
                None
            }
        };
        Ok(Proposal {
            key: key.to_owned(),
            target,
            old,
            new: value,
            weakens,
        })
    }

    /// Applies a change to its target file (see [`Config::set_owner_checked`]
    /// and [`Config::set_project`]); `confirmed` is required for loosening.
    pub fn apply(
        &mut self,
        target: Target,
        key: &str,
        value: Value,
        confirmed: bool,
    ) -> Result<Change, ConfigError> {
        match target {
            Target::Owner => self.set_owner_checked(key, value, confirmed),
            Target::Project => {
                let old = self.value(key)?.clone();
                self.set_project(key, value.clone())?;
                Ok(Change {
                    key: key.into(),
                    old,
                    new: value,
                    weakens: None,
                })
            }
        }
    }

    /// Sets a value in the owner file like [`Config::set_owner`], but refuses a
    /// change that loosens privacy (see [`loosening`]) unless `confirmed`.
    /// Tightening needs no confirmation.
    pub fn set_owner_checked(
        &mut self,
        key: &str,
        value: Value,
        confirmed: bool,
    ) -> Result<Change, ConfigError> {
        let s = concrete(key)?;
        validate(s, &value)?;
        let old = self.current(key, s);
        let weakens = loosening(s, &old, &value);
        if let (Some(w), false) = (&weakens, confirmed) {
            return Err(ConfigError::NeedsConfirm {
                key: key.into(),
                old: old.to_string(),
                new: value.to_string(),
                weakens: w.clone(),
            });
        }
        self.set_owner(key, value.clone())?;
        Ok(Change {
            key: key.into(),
            old,
            new: value,
            weakens,
        })
    }

    /// Sets a value in the owner file (validated; written atomically).
    pub fn set_owner(&mut self, key: &str, value: Value) -> Result<(), ConfigError> {
        let s = concrete(key)?;
        validate(s, &value)?;
        let mut entries = read_file(&self.owner_path)?;
        entries.retain(|(k, _)| k != key);
        entries.push((key.to_owned(), value.clone()));
        write_entries(&self.owner_path, &entries)?;
        self.values.insert(key.to_owned(), (value, Origin::Owner));
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
        let s = concrete(key)?;
        let file = path.display().to_string();
        if s.scope == Owner {
            return Err(ConfigError::OwnerOnly {
                file,
                key: key.into(),
            });
        }
        validate(s, &value)?;
        tightens(s, &self.current(key, s), &value).map_err(|rule| ConfigError::Loosening {
            file,
            key: key.into(),
            value: value.to_string(),
            rule,
        })?;
        let mut entries = read_file(&path)?;
        entries.retain(|(k, _)| k != key);
        entries.push((key.to_owned(), value.clone()));
        write_entries(&path, &entries)?;
        self.values.insert(key.to_owned(), (value, Origin::Project));
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
            "[local]\nallow_plaintext = true\n",
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
    fn custom_patterns_must_compile_and_only_tighten_from_a_project() {
        let (_d, o, p) = files("", "[sensitivity]\ncustom_patterns = [\"CUST-[0-9]{6}\"]\n");
        let c = Config::load(&o, Some(&p)).unwrap();
        assert_eq!(
            c.list("sensitivity.custom_patterns").unwrap(),
            vec!["CUST-[0-9]{6}"]
        );
        assert_eq!(
            c.origin("sensitivity.custom_patterns"),
            Some(Origin::Project)
        );
        for bad in [r#"["CUST-(["]"#, r#"[""]"#, "[1]"] {
            let v = parse_value(bad).unwrap();
            assert!(
                matches!(
                    c.propose(Target::Owner, "sensitivity.custom_patterns", v),
                    Err(ConfigError::Invalid { .. })
                ),
                "{bad}"
            );
        }
        let (_d2, o2, p2) = files("", "[sensitivity]\ncustom_patterns = [\"(\"]\n");
        assert!(matches!(
            Config::load(&o2, Some(&p2)),
            Err(ConfigError::Invalid { .. })
        ));
        // Adding tightens; removing loosens (refused in the project file,
        // confirmed in the owner's).
        let more = parse_value(r#"["CUST-[0-9]{6}", "int\\.corp\\.example"]"#).unwrap();
        let add = c
            .propose(Target::Project, "sensitivity.custom_patterns", more)
            .unwrap();
        assert_eq!(add.weakens, None);
        let none = Value::Array(vec![]);
        assert!(matches!(
            c.propose(Target::Project, "sensitivity.custom_patterns", none.clone()),
            Err(ConfigError::Loosening { .. })
        ));
        let (_d3, o3, _) = files("[sensitivity]\ncustom_patterns = [\"x+\"]\n", "");
        let owner = Config::load(&o3, None).unwrap();
        let p = owner
            .propose(Target::Owner, "sensitivity.custom_patterns", none)
            .unwrap();
        assert!(p.weakens.is_some());
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
    fn loosening_needs_confirmation_and_tightening_does_not() {
        let (_d, o, p) = files("", "");
        let mut c = Config::load(&o, Some(&p)).unwrap();
        // Against the tighten direction.
        for (key, v) in [
            ("sensitivity.detect_pii", Value::Boolean(false)),
            ("sandbox.network", Value::Boolean(true)),
            ("limits.frontier_usd", Value::Float(50.0)),
            ("local.allow_plaintext", Value::Boolean(true)),
            ("sensitivity.globs", Value::Array(vec![])),
        ] {
            let e = c.set_owner_checked(key, v.clone(), false).unwrap_err();
            assert!(matches!(e, ConfigError::NeedsConfirm { .. }), "{key}: {e}");
            assert!(e.to_string().contains("--confirm"), "{e}");
            assert_eq!(c.origin(key), Some(Origin::Default), "{key} was applied");
        }
        // A `confirm` setting without a direction: any change.
        assert!(matches!(
            c.set_owner_checked(
                "frontier.base_url",
                Value::String("https://other.example/v1".into()),
                false
            ),
            Err(ConfigError::NeedsConfirm { .. })
        ));
        // Tightening and privacy-neutral changes apply directly.
        let mut globs = c.list("sensitivity.globs").unwrap();
        globs.push("reports/**".into());
        let globs = Value::Array(globs.into_iter().map(Value::String).collect());
        let change = c
            .set_owner_checked("sensitivity.globs", globs, false)
            .unwrap();
        assert_eq!(change.weakens, None);
        c.set_owner_checked("limits.frontier_usd", Value::Float(1.0), false)
            .unwrap();
        c.set_owner_checked("local.model", Value::String("other".into()), false)
            .unwrap();
        // Confirmed loosening applies and says what it weakened.
        let change = c
            .set_owner_checked("local.allow_plaintext", Value::Boolean(true), true)
            .unwrap();
        assert!(change.weakens.unwrap().contains("plain HTTP"));
        assert!(
            Config::load(&o, None)
                .unwrap()
                .bool("local.allow_plaintext")
                .unwrap()
        );
    }

    #[test]
    fn proposals_match_what_apply_enforces() {
        let (_d, o, p) = files("", "");
        let mut c = Config::load(&o, Some(&p)).unwrap();
        let off = parse_value("false").unwrap();
        let owner = c
            .propose(Target::Owner, "sensitivity.detect_pii", off.clone())
            .unwrap();
        assert!(owner.weakens.is_some());
        assert!(matches!(
            c.propose(Target::Project, "sensitivity.detect_pii", off.clone()),
            Err(ConfigError::Loosening { .. })
        ));
        assert!(matches!(
            c.propose(
                Target::Project,
                "local.model",
                parse_value("\"x\"").unwrap()
            ),
            Err(ConfigError::OwnerOnly { .. })
        ));
        assert!(parse_value("not toml").is_err());
        assert!(matches!(
            c.apply(Target::Owner, "sensitivity.detect_pii", off.clone(), false),
            Err(ConfigError::NeedsConfirm { .. })
        ));
        let lower = parse_value("1.5").unwrap();
        let tighter = c
            .propose(Target::Project, "limits.frontier_usd", lower.clone())
            .unwrap();
        assert_eq!(tighter.weakens, None);
        c.apply(Target::Project, "limits.frontier_usd", lower, false)
            .unwrap();
        assert_eq!(
            Config::load(&o, Some(&p))
                .unwrap()
                .origin("limits.frontier_usd"),
            Some(Origin::Project)
        );
    }

    #[test]
    fn approval_is_owner_only_and_turning_it_down_needs_confirmation() {
        let (_d, o, p) = files("", "[oversight]\napprove = \"all\"\n");
        assert!(matches!(
            Config::load(&o, Some(&p)),
            Err(ConfigError::OwnerOnly { .. })
        ));
        let (_d, o, p) = files("", "");
        let mut c = Config::load(&o, Some(&p)).unwrap();
        assert_eq!(c.str("oversight.approve").unwrap(), "off");
        let v = |s: &str| Value::String(s.into());
        assert!(
            c.set_owner_checked("oversight.approve", v("bogus"), false)
                .is_err()
        );
        // Stricter needs no confirmation; less strict does.
        c.set_owner_checked("oversight.approve", v("risky"), false)
            .unwrap();
        c.set_owner_checked("oversight.approve", v("all"), false)
            .unwrap();
        for down in ["risky", "off"] {
            assert!(matches!(
                c.set_owner_checked("oversight.approve", v(down), false),
                Err(ConfigError::NeedsConfirm { .. })
            ));
        }
        let change = c
            .set_owner_checked("oversight.approve", v("off"), true)
            .unwrap();
        assert!(change.weakens.unwrap().contains("operator"));
    }

    #[test]
    fn unknown_keys_in_files_are_errors() {
        let (_d, o, p) = files("[frontier]\nmodle = \"x\"\n", "");
        assert!(matches!(
            Config::load(&o, Some(&p)),
            Err(ConfigError::Unknown(_))
        ));
    }

    #[test]
    fn mcp_servers_are_template_settings_of_the_owner() {
        let (_d, o, p) = files(
            "[mcp.servers.files]\ncommand = \"npx\"\nargs = [\"-y\", \"server\"]\nenv = [\"TOKEN_A\"]\n\
             [mcp.servers.tickets]\nurl = \"https://mcp.example.com/mcp\"\ntrust = \"sensitive\"\n",
            "",
        );
        let mut c = Config::load(&o, Some(&p)).unwrap();
        assert_eq!(c.instances("mcp.servers.*.command"), ["files", "tickets"]);
        assert_eq!(c.str("mcp.servers.files.command").unwrap(), "npx");
        assert_eq!(c.list("mcp.servers.files.args").unwrap(), ["-y", "server"]);
        // Unset fields of a configured server have their defaults.
        assert_eq!(c.str("mcp.servers.files.approve").unwrap(), "writes");
        assert_eq!(c.str("mcp.servers.files.trust").unwrap(), "public");
        assert!(!c.bool("mcp.servers.files.network").unwrap());
        assert_eq!(c.origin("mcp.servers.files.url"), Some(Origin::Default));
        assert_eq!(c.str("mcp.servers.tickets.trust").unwrap(), "sensitive");
        assert!(c.value("mcp.servers.other.command").is_err());

        // Starting a program or reaching a server is a confirmed owner change.
        let v = |t: &str| parse_value(t).unwrap();
        assert!(matches!(
            c.propose(Target::Owner, "mcp.servers.git.command", v("\"git-mcp\"")),
            Ok(Proposal {
                weakens: Some(_),
                ..
            })
        ));
        c.set_owner_checked("mcp.servers.git.command", v("\"git-mcp\""), true)
            .unwrap();
        assert_eq!(
            c.instances("mcp.servers.*.command"),
            ["files", "git", "tickets"]
        );
        assert!(
            c.propose(Target::Owner, "mcp.servers.*.command", v("\"x\""))
                .is_err()
        );
        assert!(
            c.propose(Target::Owner, "mcp.servers.bad name.command", v("\"x\""))
                .is_err()
        );
        assert!(
            c.propose(Target::Project, "mcp.servers.git.approve", v("\"always\""))
                .is_err()
        );
        assert!(matches!(
            c.set_owner_checked("mcp.servers.files.approve", v("\"auto\""), false),
            Err(ConfigError::NeedsConfirm { .. })
        ));
        c.set_owner_checked("mcp.servers.files.approve", v("\"always\""), true)
            .unwrap();
        let reloaded = Config::load(&o, Some(&p)).unwrap();
        assert_eq!(reloaded.str("mcp.servers.git.command").unwrap(), "git-mcp");
        assert_eq!(reloaded.str("mcp.servers.files.approve").unwrap(), "always");

        // A project may not add or change servers.
        for bad in [
            "[mcp.servers.evil]\ncommand = \"sh\"\n",
            "[mcp.servers.files]\nnetwork = true\n",
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
        for bad in [
            "[mcp.servers.files]\ncomand = \"x\"\n",
            "[mcp.servers.\"a.b\"]\ncommand = \"x\"\n",
            "[mcp.servers.files]\ntrust = \"trusted\"\n",
        ] {
            let (_d, o, p) = files(bad, "");
            assert!(Config::load(&o, Some(&p)).is_err(), "{bad}");
        }
    }
}
