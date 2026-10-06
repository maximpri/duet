// SPDX-License-Identifier: GPL-3.0-or-later
//! The settings registry.
//!
//! Every setting is declared once in [`REGISTRY`] with its type, default, scope
//! and direction. The loader and `declass config` read this table (as will
//! `declass doctor` and the TUI); reading an unregistered key is an error.
//!
//! Files: owner `~/.config/declass/config.toml` (trusted) and project
//! `.declass/config.toml` (untrusted). A project file may never set owner-only keys
//! and may only change a setting in the direction that tightens privacy.
//!
//! A program that embeds Declass may add a policy layer above both
//! ([`Config::load_with`], [`policy`]): every setting it holds bounds every
//! other layer. Declass's own command line loads none.

pub mod policy;

pub use policy::{Policy, PolicyError, PolicyFile, PolicyMeta, PolicySource};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
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
    /// Lists of what is allowed: only removing entries tightens (adding
    /// one loosens).
    RemoveOnly,
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

/// Package registries commands reach by default (`sandbox.registries`): the
/// hosts `cargo`, `npm`/`yarn`/`pnpm`, `pip`, `go`, Maven and Gradle, and
/// `gem`/`bundle` download from, and GitHub's download hosts (release assets
/// and source archives). Not github.com itself, which also takes pushes.
const REGISTRY_HOSTS: &str = r#"[
    "crates.io", "index.crates.io", "static.crates.io",
    "registry.npmjs.org", "registry.yarnpkg.com",
    "pypi.org", "files.pythonhosted.org",
    "proxy.golang.org", "sum.golang.org",
    "repo.maven.apache.org", "repo1.maven.org", "plugins.gradle.org", "plugins-artifacts.gradle.org",
    "rubygems.org", "index.rubygems.org",
    "codeload.github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com",
]"#;

/// Defaults are TOML literals.
pub const REGISTRY: &[Setting] = &[
    s!(
        "extensions.skills_enabled",
        Bool,
        "true",
        Project,
        OnlyFalse,
        false,
        "Discover portable SKILL.md workflows. Repositories may disable skills, never grant extra tool permissions."
    ),
    s!(
        "extensions.plugins_enabled",
        Bool,
        "true",
        Project,
        OnlyFalse,
        false,
        "Load owner-installed plugin packages. Repositories may disable them; they cannot install or enable packages."
    ),
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
        "frontier.resend_reasoning",
        Bool,
        "true",
        Owner,
        Any,
        false,
        "Send the frontier's earlier reasoning back with each request (Z.ai's Coding Plan preserves it by default). Off, past reasoning is not sent. Measured on X2 (2026-09-29): off, the frontier lost its thread (4-6x the requests, every run stopped by the wall clock, one at 27/55); keep it on."
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
        "frontier.vision",
        Bool,
        "false",
        Project,
        OnlyFalse,
        true,
        "The frontier model accepts images. Pass-through: images are sent when true and refused when false. Hybrid: the frontier gets an image itself only when it is public (images.to_frontier, or attached with --image-public), never from a sensitive path. The anthropic and openai presets turn it on; `declass doctor --online` tests it."
    ),
    s!(
        "frontier.allow_passthrough",
        Bool,
        "true",
        Project,
        OnlyFalse,
        true,
        "Allow `--mode passthrough` in `declass run` and a `declass` session: the privacy boundary off, everything the model reads sent to the frontier unfiltered (each use still needs --no-privacy). false refuses such runs and sessions, and resuming one. A project may turn it off for its repository; turning it back on is the owner's, confirmed."
    ),
    s!(
        "clearance.required",
        Choice(&["standard", "top"]),
        r#""standard""#,
        Project,
        OnlyLaterChoice,
        true,
        "`top`: every run and session here is in top clearance: only the local model works, and nothing leaves this machine but its requests to the local model (no frontier, no web tools, no network for commands, no MCP server with network). `declass` and `declass run` start in it without --mode; other modes are refused, and resuming a session of another mode too. A project may require it for its repository; lifting it is the owner's, confirmed. `standard`: every mode is available (top clearance with --mode top-clearance or /mode in a session)."
    ),
    s!(
        "pricing.offline",
        Bool,
        "false",
        Owner,
        Any,
        false,
        "Use cached/bundled OpenRouter token prices without refreshing the public catalog. Top clearance and loopback frontier endpoints never refresh it."
    ),
    s!(
        "pricing.frontier_model",
        Str,
        r#""""#,
        Owner,
        Any,
        false,
        "Optional exact OpenRouter pricing slug for an aliased frontier model. Empty resolves frontier.model (exact slug or unique unqualified model id)."
    ),
    s!(
        "pricing.manual_model",
        Str,
        r#""""#,
        Owner,
        Any,
        false,
        "Exact frontier model id to which the owner-supplied rates below apply. Set pricing.manual_base_url too; rates are bound to both."
    ),
    s!(
        "pricing.manual_base_url",
        Str,
        r#""""#,
        Owner,
        Any,
        false,
        "Exact frontier base URL to which the owner-supplied rates apply. Empty disables manual rates; this prevents a rate from silently carrying to another provider."
    ),
    s!(
        "pricing.manual_input_usd_per_million",
        Float {
            min: 0.0,
            max: 1_000_000.0
        },
        "0.0",
        Owner,
        Any,
        false,
        "Owner-supplied input USD per million tokens for pricing.manual_model (paired with output). Use the direct provider's current rate."
    ),
    s!(
        "pricing.manual_output_usd_per_million",
        Float {
            min: 0.0,
            max: 1_000_000.0
        },
        "0.0",
        Owner,
        Any,
        false,
        "Owner-supplied output USD per million tokens for pricing.manual_model. Required with the input rate."
    ),
    s!(
        "pricing.manual_cache_read_usd_per_million",
        Float {
            min: 0.0,
            max: 1_000_000.0
        },
        "0.0",
        Owner,
        Any,
        false,
        "Optional cached-input rate for pricing.manual_model; zero uses the input rate conservatively."
    ),
    s!(
        "pricing.manual_cache_write_usd_per_million",
        Float {
            min: 0.0,
            max: 1_000_000.0
        },
        "0.0",
        Owner,
        Any,
        false,
        "Optional cache-write rate for pricing.manual_model; zero uses the input rate."
    ),
    s!(
        "local.input_usd_per_million",
        Float {
            min: 0.0,
            max: 1_000_000.0
        },
        "0.0",
        Owner,
        Any,
        false,
        "Estimated local operating cost in USD per million input tokens, including cached input. Defaults to zero. Applied to future runs; does not change frontier spend limits."
    ),
    s!(
        "local.output_usd_per_million",
        Float {
            min: 0.0,
            max: 1_000_000.0
        },
        "0.0",
        Owner,
        Any,
        false,
        "Estimated local operating cost in USD per million output tokens. Defaults to zero. Input and output costs are added and shown separately from frontier estimates."
    ),
    s!(
        "local.enabled",
        Bool,
        "true",
        Project,
        OnlyFalse,
        true,
        "Use a local model. false: hybrid runs without one: no local server is probed, no model reads sensitive content, the frontier sees it only as handles (their sanitized error lines and line shapes), and ask_local and edit_protected are refused; top clearance and sensitivity.local_pii_pass refuse to start. Measures what the local model adds (evaluation lane declass-hybrid-nolocal)."
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
        "local.vision",
        Bool,
        "false",
        Project,
        OnlyFalse,
        true,
        "The local model reads images (a vision-language model). Hybrid: an image the frontier may not see is described by it and the frontier gets the description, cleaned like any local output; text in an image is only as safe as that description, since the detectors cannot read images. Without it such images are refused. `declass doctor --online` tests it with a generated image."
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
        "Secret detectors: key formats, credential assignments, and the imported gitleaks rule set (221 service-specific rules; see SECURITY.md, Detection)."
    ),
    s!(
        "sensitivity.detect_pii",
        Bool,
        "true",
        Project,
        OnlyTrue,
        true,
        "Personal-data detectors: email, phone (US, international, labelled), card, US/UK/EU national ids with their checks, IBAN, IPv4/IPv6, labelled postal addresses."
    ),
    s!(
        "sensitivity.detect_entropy",
        Bool,
        "true",
        Project,
        OnlyTrue,
        true,
        "High-entropy tokens (keys of no known format); paths, hashed build files and identifiers are judged by their parts."
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
        "sensitivity.local_pii_pass",
        Bool,
        "false",
        Project,
        OnlyTrue,
        true,
        "The local model reads the free text of public content (docs, comments, web pages, public tool results) and marks people's names and postal addresses, which no pattern finds; they become placeholders like detected values. Costs local model time on every such result."
    ),
    s!(
        "sensitivity.structure_views",
        Bool,
        "true",
        Project,
        OnlyFalse,
        true,
        "Sensitive files and the output of sensitive_data commands come with a structure view computed here without a model: format, records, schema (JSON paths, columns, keys, log line templates), types, value shapes (letters as A/a, digits as 9, date layouts as pictures), presence, null and empty counts, distinct-count buckets, lengths and anomalies; counts and shapes, never values. The task note gets an outline of the sensitive files, and short sensitive_data output is shown with every value masked by its shape."
    ),
    s!(
        "sensitivity.synthetic_rows",
        Int { min: 0, max: 200 },
        "20",
        Project,
        OnlyLower,
        true,
        "Records a synthetic sample may hold: synthetic_sample(handle, rows) returns a rule-generated fake of a sensitive data file (same schema, formats, lengths, nulls and quoting; valid dates, card numbers and IBANs; no real value, checked before it is shown), usable as a test fixture, and the file's view shows its first record. 0 turns samples off."
    ),
    s!(
        "sensitivity.masked_numbers",
        Int { min: 0, max: 1000 },
        "24",
        Project,
        OnlyLower,
        true,
        "Small numbers (0-99) masked sensitive_data output may show as written over a run; every other number, and small ones past this budget, show as their shape (9 per digit). Numbers carry information one comparison at a time, so they are counted."
    ),
    s!(
        "sensitivity.output_probes",
        Int { min: 0, max: 1000 },
        "12",
        Project,
        OnlyLower,
        true,
        "Short outputs (at most 200 characters) of sensitive_data commands shown over a run. Each is a probe of the data (a count, a match, a yes or no); past this budget such output is withheld and the view does not depend on it. Every probe is an output_probe audit event."
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
        "images.to_frontier",
        Choice(&["public", "never"]),
        r#""never""#,
        Project,
        OnlyLaterChoice,
        true,
        "Hybrid: which images the frontier may receive itself (not a description). `never`: only images the operator attaches with --image-public (or /image --public). `public`: also workspace images read with read_file or attached from paths that are neither sensitive nor protected. Images cannot be scanned for secrets or personal data, hence `never`; sensitive-path images never go to the frontier."
    ),
    s!(
        "images.max_side",
        Int {
            min: 256,
            max: 8192
        },
        "1568",
        Project,
        Any,
        false,
        "Longest side, in pixels, an image is scaled to before a model sees it (every image is also re-encoded, which drops its metadata). Larger images cost more tokens; providers scale beyond about 1568 anyway."
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
        "limits.process_memory_mb",
        Int {
            min: 0,
            max: 1_048_576
        },
        "0",
        Owner,
        Any,
        false,
        "Most memory one process started by a command, an MCP server or a language server may use, in MB: its physical footprint (resident plus compressed or swapped memory). A process above it is stopped and the command's output says why. 0: an eighth of this machine's memory. When the whole machine is critically short of memory, the command's largest process is stopped whatever its size. Declass only ever stops processes it started."
    ),
    s!(
        "limits.command_memory_mb",
        Int {
            min: 0,
            max: 1_048_576
        },
        "0",
        Owner,
        Any,
        false,
        "Most memory all the processes of one command (or one MCP or language server) may use together, in MB; above it the largest is stopped. 0: a quarter of this machine's memory."
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
        "Maximum frontier spend over a whole session (the `declass` workspace); each turn is also held to limits.frontier_usd."
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
        "subagents.enabled",
        Bool,
        "true",
        Project,
        OnlyFalse,
        false,
        "Offer the `delegate` tool: the frontier hands a sub-task to a sub-agent with a fresh context (the same boundary, sandbox and tools, never more): read-only ones, several at a time, or one that writes only the paths it was given. Its spend and time count against this run's limits."
    ),
    s!(
        "subagents.max_parallel",
        Int { min: 1, max: 8 },
        "3",
        Project,
        Any,
        false,
        "How many read-only sub-agents run at the same time when the frontier delegates several at once (writing ones always run alone)."
    ),
    s!(
        "subagents.max_usd",
        Float {
            min: 0.0,
            max: 1_000.0
        },
        "1.0",
        Project,
        OnlyLower,
        true,
        "Maximum frontier spend of one sub-agent (list price); it also never exceeds what is left of the run's limits.frontier_usd. A sub-agent that reaches it stops as budget_stopped and the frontier is told."
    ),
    s!(
        "subagents.max_minutes",
        Int {
            min: 1,
            max: 24 * 60
        },
        "15",
        Project,
        OnlyLower,
        false,
        "Maximum duration of one sub-agent; it also ends when the run's own wall clock does."
    ),
    s!(
        "subagents.model",
        Str,
        r#""""#,
        Owner,
        Any,
        true,
        "Frontier model id for sub-agents (for example a cheaper one at the same endpoint); empty uses frontier.model. Its requests pass the same outbound gate and audit log."
    ),
    s!(
        "explore.enabled",
        Bool,
        "false",
        Project,
        OnlyFalse,
        false,
        "Offer the `explore` tool when a local model is enabled (hybrid and pass-through): the local model answers a where/what/how question about the repository by reading and searching it itself (read-only, sensitive files included, no commands, no network), and the frontier gets one result: a short answer and checked file:line references. In hybrid mode the report is cleaned as local-model output before the frontier sees it. Off until measured."
    ),
    s!(
        "explore.quick_steps",
        Int { min: 1, max: 100 },
        "12",
        Project,
        Any,
        false,
        "Local model requests one quick `explore` call may make (the default depth); then it must report."
    ),
    s!(
        "explore.thorough_steps",
        Int { min: 1, max: 200 },
        "30",
        Project,
        Any,
        false,
        "Local model requests one thorough `explore` call may make; then it must report."
    ),
    s!(
        "explore.max_seconds",
        Int { min: 10, max: 3600 },
        "600",
        Project,
        Any,
        false,
        "Longest time one thorough `explore` call may take (a quick one gets a third); a call still exploring then ends and the frontier is told which files it read. The run's own wall clock also ends it."
    ),
    s!(
        "explore.max_read_kb",
        Int { min: 8, max: 4096 },
        "96",
        Project,
        Any,
        false,
        "Most tool output one thorough `explore` call's local model is shown, in KB (a quick one half); then it must report. Keep it within the local model's context: data files take about one token per 2 bytes."
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
        "review.enabled",
        Bool,
        "false",
        Project,
        OnlyTrue,
        false,
        "Experimental security auditor at finish. Reviews changes from a private run-start snapshot with bounded syntax rules and available local-model opinions; off until measured."
    ),
    s!(
        "review.block_high",
        Bool,
        "false",
        Project,
        OnlyTrue,
        false,
        "Block finish for new rule-confirmed high-severity security findings. Local opinions cannot create or dismiss a blocker; requires review.enabled."
    ),
    s!(
        "review.max_candidates",
        Int { min: 1, max: 64 },
        "16",
        Owner,
        Any,
        false,
        "Maximum local security opinions per finish attempt. Remaining candidates still receive rule findings."
    ),
    s!(
        "review.local_open",
        Bool,
        "false",
        Owner,
        Any,
        false,
        "Request local opinions on open, non-privacy code too. Off: development measurements found no gain there; protected code and privacy candidates remain local."
    ),
    s!(
        "review.references",
        Bool,
        "true",
        Owner,
        Any,
        false,
        "Gather bounded definition/reference context from installed sandboxed language servers for security candidates."
    ),
    s!(
        "review.frontier",
        Bool,
        "false",
        Owner,
        Any,
        true,
        "Optional fresh-context frontier security opinions on eligible open code. Never sends protected code, privacy flows or the working conversation. Advisory only."
    ),
    s!(
        "review.frontier_usd",
        Float {
            min: 0.001,
            max: 100.0
        },
        "0.5",
        Owner,
        Any,
        false,
        "Maximum frontier spend on security second opinions per invocation, also bounded by the run budget."
    ),
    s!(
        "review.max_frontier_candidates",
        Int { min: 1, max: 64 },
        "4",
        Owner,
        Any,
        false,
        "Maximum eligible frontier opinions per finish or repository scan."
    ),
    s!(
        "review.scanners.*.command",
        Str,
        r#""""#,
        Owner,
        Any,
        true,
        "Optional installed scanner: absolute executable outside the workspace. Runs on a private read-only snapshot, without network."
    ),
    s!(
        "review.scanners.*.args",
        List,
        "[]",
        Owner,
        Any,
        true,
        "Scanner arguments; {workspace} names its private snapshot. Supply JSON/SARIF and offline flags appropriate to the installed scanner."
    ),
    s!(
        "review.scanners.*.format",
        Str,
        r#""sarif""#,
        Owner,
        Any,
        false,
        "Scanner output format: sarif, bandit, gosec, cargo-audit, npm-audit or pip-audit. Output is advisory and untrusted."
    ),
    s!(
        "review.scanners.*.timeout_seconds",
        Int { min: 1, max: 300 },
        "60",
        Owner,
        Any,
        false,
        "Maximum seconds per scanner snapshot; interrupted processes are killed."
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
        "context.compaction",
        Bool,
        "false",
        Owner,
        Any,
        false,
        "Condense older turns into a working summary written by the local model once the conversation passes context.compact_at (hybrid mode with local.enabled; without a local model nothing is attempted). Masking is tried first and stays the fallback. Off until measured."
    ),
    s!(
        "context.compact_at",
        Int {
            min: 8_000,
            max: 2_000_000
        },
        "100000",
        Owner,
        Any,
        false,
        "Estimated request tokens at which context.compaction condenses older turns; keep it below context.mask_at of the window."
    ),
    s!(
        "context.compact_to",
        Float { min: 0.1, max: 0.8 },
        "0.4",
        Owner,
        Any,
        false,
        "Fraction of context.compact_at a compaction brings the conversation down to (the recent turns kept verbatim fill it)."
    ),
    s!(
        "context.condense_output",
        Bool,
        "true",
        Project,
        Any,
        false,
        "Show test, build and install output condensed: failing tests with their assertions, the first error of each kind with its location, and the summary counts; passing tests, progress and repeated lines are left out. The whole output stays readable with read_raw. Privacy mode only, and only for output the frontier may see: output held as sensitive is never condensed, and pass-through mode shows output as it is."
    ),
    s!(
        "sandbox.network",
        Choice(&["all", "registries", "off"]),
        r#""registries""#,
        Project,
        OnlyLaterChoice,
        true,
        "Network for sandboxed commands and checks. `registries`: only through declass's egress proxy to the package registries in sandbox.registries (every connection audited as `egress`), plus servers the command starts on loopback. `off`: none. `all`: unrestricted (a command could send anything it can read anywhere). Commands run with sensitive_data, and checks that can read protected source, never get network. The old true and false still read as all and off."
    ),
    s!(
        "sandbox.registries",
        List,
        REGISTRY_HOSTS,
        Owner,
        RemoveOnly,
        true,
        "Hosts sandboxed commands may reach through the egress proxy (sandbox.network = registries): host names, `*.domain` for the names under a domain, optionally `:port` (without one, 443 and 80). Only these names are resolved; an address that is private, loopback, link-local or a cloud metadata service is refused, and a TLS tunnel must name its host. github.com is not listed by default (it also accepts pushes); its download hosts are."
    ),
    s!(
        "web.enabled",
        Bool,
        "true",
        Project,
        OnlyFalse,
        true,
        "Offer the web tools (`web_fetch`, and `web_search` when a search backend is available, which with web.search.backend = `auto` is always). Requests are made by the host, GET only, to public addresses only; pages are scanned like public content and shown as untrusted data. Commands' own network is sandbox.network."
    ),
    s!(
        "web.search.backend",
        Choice(&[
            "auto",
            "native",
            "brave",
            "searxng",
            "wikipedia",
            "zai",
            "none"
        ]),
        r#""auto""#,
        Owner,
        OnlyLaterChoice,
        true,
        "Search backend for `web_search`: `auto` (the default: `zai` when the frontier is Z.ai and its key is set, else `native`), `native` (this machine asks public sources with open APIs itself, no search provider in between: Stack Overflow, Wikipedia, GitHub and the package registries of the workspace's languages by default, see web.search.sources; each source asked receives the query), `brave` (Brave Search API, key from web.search.brave_key_env), `searxng` (your own instance at web.search.searxng_url; `declass config preset searxng` sets one up), `wikipedia` (English Wikipedia's search only), `zai` (Z.ai's search with the frontier's key; see web.search.zai_engine) or `none` (no web_search tool). `auto` never picks brave, searxng or zai: they are used only when named here. Queries are checked for sensitive values before they are sent; `declass doctor` shows the backend in use and who receives the queries."
    ),
    s!(
        "web.search.sources",
        List,
        r#"["auto"]"#,
        Owner,
        Any,
        true,
        "Sources the native search backend may ask; each one asked receives the query. `auto`: stackoverflow, wikipedia and github (repositories) by default, plus crates for a Cargo.toml, npm for a package.json and pypi (exact package names) for a Python project at the workspace root; the frontier may name any other source in `sources`. A named source is asked by default too; without `auto` only the named sources are asked. Sources: stackoverflow, serverfault, superuser, askubuntu, unix (Stack Exchange API), wikipedia, github, github_issues (GitHub REST search, never with a token), crates (crates.io), npm, pypi, hackernews (Algolia's HN Search API), arxiv."
    ),
    s!(
        "web.search.zai_engine",
        Choice(&["auto", "plan", "search_pro_jina", "search-prime"]),
        r#""auto""#,
        Owner,
        Any,
        true,
        "How the `zai` search backend (only when web.search.backend = `zai`) searches: `plan` (the GLM Coding Plan's search server, counted in the plan's credits), `search_pro_jina` or `search-prime` (Z.ai's Web Search API with that engine, billed per search to the account balance, not by the plan) or `auto` (`plan` when the frontier is the coding plan endpoint, `search_pro_jina` otherwise). In tests (2026-09) `search_pro_jina` gave the most relevant hits, with page addresses; the plan's search often gave only a hit's site."
    ),
    s!(
        "web.search.searxng_url",
        Str,
        r#""""#,
        Owner,
        Any,
        true,
        "Base URL of a SearXNG instance with the JSON format enabled (for example http://127.0.0.1:8888; `declass config preset searxng` sets up a local one in Docker); queries go to <url>/search."
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
        "lsp.enabled",
        Bool,
        "true",
        Project,
        OnlyFalse,
        false,
        "Offer the code_nav and rename tools when a language server is installed (rust-analyzer, typescript-language-server, pyright-langserver or basedpyright-langserver, gopls, clangd on PATH, or lsp.servers). Servers start on first use, sandboxed without network; they can read protected source like checks, never sensitive files."
    ),
    // Language servers: one `[lsp.servers.<language>]` table per language;
    // `*` stands for the language (a built-in one is replaced).
    s!(
        "lsp.servers.*.command",
        Str,
        r#""""#,
        Owner,
        Any,
        true,
        "Language server for this language (rust, typescript, python, go and c are built in; any other name adds one): a program on PATH or an absolute path, started in the command sandbox without network. It reads the workspace, protected source included."
    ),
    s!(
        "lsp.servers.*.args",
        List,
        "[]",
        Owner,
        Any,
        true,
        "Arguments of the language server's command (for example [\"--stdio\"])."
    ),
    s!(
        "lsp.servers.*.extensions",
        List,
        "[]",
        Owner,
        Any,
        true,
        "File extensions the language server handles, without the dot; required for a language that is not built in."
    ),
    s!(
        "lsp.servers.*.env",
        List,
        "[]",
        Owner,
        Any,
        true,
        "Names of environment variables passed to the language server; every other variable is cleared (except the sandbox's base set). Values are never stored."
    ),
    s!(
        "lsp.request_timeout_seconds",
        Int { min: 1, max: 600 },
        "30",
        Project,
        Any,
        false,
        "Seconds a language-server request (and a server's start) may take before the tool reports a timeout."
    ),
    s!(
        "lsp.diagnostics_wait_ms",
        Int { min: 0, max: 30000 },
        "2000",
        Project,
        Any,
        false,
        "Milliseconds an edit waits for the language server's diagnostics of the file before its result is returned without them (0: never wait)."
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
        "Days before audit logs are due for retention review (`declass audit retention`). Audit logs and anchors are not deleted automatically."
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
        "Whether the frontier may commit files it wrote in the run (git_commit; never sensitive or protected files, never a push). `ask`: each commit waits for the operator's approval, so git_commit is offered only where someone can answer: in an interactive `declass` session (asked inline), or when oversight.approve is risky or all. `allow`: commits without asking (audited). `off`: never offered."
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
    Fs(#[from] declass_fs::FsError),
    /// A change or override the policy layer does not allow.
    #[error("{key} = {value} is not allowed by the policy {policy}: {rule}")]
    Policy {
        key: String,
        value: String,
        policy: String,
        rule: String,
    },
    /// A policy source is configured but its policy cannot be used: nothing
    /// runs until it can (fail closed).
    #[error("{0}; declass does not run without its policy")]
    PolicyLoad(PolicyError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Default,
    Owner,
    Project,
    /// The policy layer set it: it is fixed, or another layer's value was
    /// looser than the policy allows (see [`policy`]).
    Policy,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Every setting by key; a template's instances under their own keys.
    values: BTreeMap<String, (Value, Origin)>,
    pub owner_path: PathBuf,
    pub project_path: Option<PathBuf>,
    /// Values read in a deprecated form and what they now mean (see
    /// [`migrate`]), and values the policy layer overrode, for the operator.
    pub notes: Vec<String>,
    /// The policy layer, when one was loaded.
    policy: Option<Arc<Policy>>,
    /// Keys whose owner or project value the policy layer replaced.
    policy_overrides: Vec<String>,
}

/// A value written in a form an earlier version used, as it reads now, and a
/// note saying so: `sandbox.network = true` is `"all"` and `false` is
/// `"off"` (it was a boolean before the egress proxy). Anything else is
/// returned as it is.
pub fn migrate(key: &str, value: Value) -> (Value, Option<String>) {
    match (key, &value) {
        ("sandbox.network", Value::Boolean(b)) => {
            let now = if *b { "all" } else { "off" };
            (
                Value::String(now.into()),
                Some(format!(
                    "sandbox.network = {b} is deprecated; it reads as \"{now}\" (write sandbox.network = \"{now}\", or \"registries\" for package registries only)"
                )),
            )
        }
        _ => (value, None),
    }
}

// Only immutable registry literals are cached. Owner, project and policy
// sources are still read on every load, and each config owns its values.
static DEFAULT_VALUES: LazyLock<BTreeMap<&'static str, Value>> = LazyLock::new(|| {
    REGISTRY
        .iter()
        .map(|s| {
            let mut table = toml::from_str::<toml::Table>(&format!("v = {}", s.default))
                .expect("registry default parses");
            (s.key, table.remove("v").expect("registry default value"))
        })
        .collect()
});

fn default_value(s: &Setting) -> Value {
    DEFAULT_VALUES[s.key].clone()
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
        RemoveOnly => {
            let base: Vec<&str> = base
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let added = new
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|n| !base.contains(&n));
            if added {
                Err("remove entries, never add them")
            } else {
                Ok(())
            }
        }
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

/// Default owner config location (`$DECLASS_CONFIG_HOME/config.toml` or `~/.config/declass/config.toml`).
pub fn owner_config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("DECLASS_CONFIG_HOME") {
        return PathBuf::from(dir).join("config.toml");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/declass/config.toml")
}

/// The owner's own project instructions for every repository: `DECLASS.md`
/// next to the owner config (`$DECLASS_CONFIG_HOME/DECLASS.md` or
/// `~/.config/declass/DECLASS.md`). Optional.
pub fn owner_instructions_path() -> PathBuf {
    owner_config_path().with_file_name("DECLASS.md")
}

/// The owner's private state directory (audit anchors, the config audit log),
/// outside every workspace: `$DECLASS_CONFIG_HOME/state` when that is set, else
/// `$XDG_STATE_HOME/declass`, else `~/.local/state/declass`.
pub fn owner_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DECLASS_CONFIG_HOME") {
        return PathBuf::from(dir).join("state");
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("declass");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".local/state/declass")
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
    /// The project's `.declass/config.toml`: project-scoped keys, tightening only.
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
        Self::load_with(owner_path, project_path, None)
    }

    /// Like [`Config::load`], with the policy layer of `policy` above the
    /// files (see [`policy`]). A source that fails to load or verify its
    /// policy, or a policy that is not valid, is
    /// [`ConfigError::PolicyLoad`]: the caller must not run without it.
    pub fn load_with(
        owner_path: &Path,
        project_path: Option<&Path>,
        policy: Option<&dyn PolicySource>,
    ) -> Result<Self, ConfigError> {
        let policy = match policy.map(PolicySource::load).transpose() {
            Ok(loaded) => loaded
                .flatten()
                .map(|(text, meta)| Policy::parse(&text, meta))
                .transpose()
                .map_err(ConfigError::PolicyLoad)?,
            Err(e) => return Err(ConfigError::PolicyLoad(e)),
        };
        let mut values: BTreeMap<String, (Value, Origin)> = REGISTRY
            .iter()
            .filter(|s| !is_template(s.key))
            .map(|s| (s.key.to_owned(), (default_value(s), Origin::Default)))
            .collect();
        let mut notes = Vec::new();
        let mut migrated = |file: &Path, key: &str, v: Value| {
            let (v, note) = migrate(key, v);
            if let Some(n) = note {
                notes.push(format!("{}: {n}", file.display()));
            }
            v
        };
        for (key, v) in read_file(owner_path)? {
            let v = migrated(owner_path, &key, v);
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
                let v = migrated(pp, &key, v);
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
        let policy_overrides = match &policy {
            Some(p) => bound_by(p, &mut values, &mut notes, owner_path, project_path),
            None => Vec::new(),
        };
        Ok(Self {
            values,
            owner_path: owner_path.to_path_buf(),
            project_path: project_path.map(Path::to_path_buf),
            notes,
            policy: policy.map(Arc::new),
            policy_overrides,
        })
    }

    /// The policy layer, when one was loaded.
    pub fn policy(&self) -> Option<&Policy> {
        self.policy.as_deref()
    }

    /// Keys whose owner or project value the policy layer replaced (each
    /// also has a note).
    pub fn policy_overrides(&self) -> &[String] {
        &self.policy_overrides
    }

    /// How the policy layer bounds `key`, for the operator (`declass config
    /// list`, `declass doctor`); `None` without a policy or when it does not
    /// hold the key.
    pub fn policy_rule(&self, key: &str) -> Option<String> {
        self.policy.as_ref()?.rule(key)
    }

    /// Whether the policy layer allows `value` for `key` in place of the
    /// configured value: for command-line overrides (`--frontier-url`) and
    /// values found at run time. Always `Ok` without a policy.
    pub fn allows(&self, key: &str, value: &Value) -> Result<(), ConfigError> {
        let Some(p) = &self.policy else {
            return Ok(());
        };
        p.allows(key, value).map_err(|rule| ConfigError::Policy {
            key: key.into(),
            value: value.to_string(),
            policy: p.meta().to_string(),
            rule,
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
        let value = migrate(key, value).0;
        let s = concrete(key)?;
        validate(s, &value)?;
        self.allows(key, &value)?;
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
        let value = migrate(key, value).0;
        let s = concrete(key)?;
        validate(s, &value)?;
        self.allows(key, &value)?;
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

    /// Sets a value in the owner file (validated, within the policy layer;
    /// written atomically).
    pub fn set_owner(&mut self, key: &str, value: Value) -> Result<(), ConfigError> {
        let value = migrate(key, value).0;
        let s = concrete(key)?;
        validate(s, &value)?;
        self.allows(key, &value)?;
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
        let value = migrate(key, value).0;
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
        self.allows(key, &value)?;
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

/// Applies the policy layer to the merged values: a fixed key takes the
/// policy's value, any other the tighter of the two (see [`policy`]). Values
/// the owner or project set that give way are noted for the operator; their
/// keys are returned.
fn bound_by(
    p: &Policy,
    values: &mut BTreeMap<String, (Value, Origin)>,
    notes: &mut Vec<String>,
    owner_path: &Path,
    project_path: Option<&Path>,
) -> Vec<String> {
    let mut overridden = Vec::new();
    for (key, bound) in p.values() {
        // A template instance the other layers do not configure stays absent.
        let (Some((lower, origin)), Some(s)) = (values.get(key).cloned(), setting(key)) else {
            continue;
        };
        let fixed = p.fixes(key);
        let effective = if fixed {
            bound.clone()
        } else {
            policy::tightest(s, bound, &lower)
        };
        let changed = !policy::same(&effective, &lower);
        let file = match origin {
            Origin::Owner => Some(owner_path.display().to_string()),
            Origin::Project => project_path.map(|p| p.display().to_string()),
            Origin::Default | Origin::Policy => None,
        };
        if let (true, Some(file)) = (changed, file) {
            notes.push(format!(
                "{file}: {key} = {lower} {} the policy {}; {effective} applies",
                if fixed {
                    "is fixed by"
                } else {
                    "is looser than"
                },
                p.meta()
            ));
            overridden.push(key.clone());
        }
        if changed || fixed {
            values.insert(key.clone(), (effective, Origin::Policy));
        }
    }
    overridden
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
        declass_fs::private::ensure_private_dir(parent)?;
    }
    let text = toml::to_string_pretty(&root)
        .map_err(|e| ConfigError::Parse(path.display().to_string(), e.to_string()))?;
    declass_fs::private::write_private(path, text.as_bytes())?;
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
    fn reloading_config_reads_new_files_and_preserves_independent_defaults() {
        let (_dir, owner, project) = files("[local]\nmodel='first'\n", "");
        let first = Config::load(&owner, Some(&project)).unwrap();
        std::fs::write(
            &owner,
            "[local]\nmodel='second'\n[mcp.servers.files]\ncommand='files'\n",
        )
        .unwrap();
        let second = Config::load(&owner, Some(&project)).unwrap();
        assert_eq!(first.str("local.model").unwrap(), "first");
        assert_eq!(second.str("local.model").unwrap(), "second");
        assert_eq!(second.int("mcp.servers.files.timeout_seconds").unwrap(), 60);
        assert_eq!(
            second.origin("mcp.servers.files.timeout_seconds"),
            Some(Origin::Default)
        );

        std::fs::remove_file(&owner).unwrap();
        let defaults = Config::load(&owner, Some(&project)).unwrap();
        assert_eq!(defaults.origin("local.model"), Some(Origin::Default));
        assert_eq!(
            defaults.value("local.model").unwrap(),
            &default_value(setting("local.model").unwrap())
        );
        assert!(defaults.instances("mcp.servers.*.command").is_empty());
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
            "[lsp.servers.rust]\ncommand = \"/tmp/evil\"\n",
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
    fn language_servers_are_template_settings_of_the_owner() {
        let (_d, o, p) = files(
            "[lsp.servers.rust]\ncommand = \"/opt/ra\"\n[lsp.servers.zig]\ncommand = \"zls\"\nextensions = [\"zig\"]\n",
            "[lsp]\nenabled = false\n",
        );
        let mut c = Config::load(&o, Some(&p)).unwrap();
        assert_eq!(c.instances("lsp.servers.*.command"), ["rust", "zig"]);
        assert_eq!(c.list("lsp.servers.zig.extensions").unwrap(), ["zig"]);
        assert!(c.list("lsp.servers.rust.args").unwrap().is_empty());
        assert!(!c.bool("lsp.enabled").unwrap());
        let v = |t: &str| parse_value(t).unwrap();
        assert!(matches!(
            c.set_owner_checked("lsp.servers.go.command", v("\"/opt/gopls\""), false),
            Err(ConfigError::NeedsConfirm { .. })
        ));
        c.set_owner_checked("lsp.servers.go.command", v("\"/opt/gopls\""), true)
            .unwrap();
        assert_eq!(c.instances("lsp.servers.*.command"), ["go", "rust", "zig"]);
        assert!(
            c.propose(Target::Project, "lsp.servers.go.command", v("\"x\""))
                .is_err()
        );
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
    fn a_project_may_turn_the_local_model_off_and_only_the_owner_back_on() {
        let (_d, o, p) = files("", "[local]\nenabled = false\n");
        let c = Config::load(&o, Some(&p)).unwrap();
        assert!(!c.bool("local.enabled").unwrap());
        assert_eq!(c.origin("local.enabled"), Some(Origin::Project));
        let (_d, o, p) = files("[local]\nenabled = false\n", "[local]\nenabled = true\n");
        assert!(matches!(
            Config::load(&o, Some(&p)),
            Err(ConfigError::Loosening { .. })
        ));
        // Back on, the local model reads sensitive content again: confirmed.
        let (_d, o, _p) = files("[local]\nenabled = false\n", "");
        let mut c = Config::load(&o, None).unwrap();
        assert!(matches!(
            c.set_owner_checked("local.enabled", Value::Boolean(true), false),
            Err(ConfigError::NeedsConfirm { .. })
        ));
        c.set_owner_checked("local.enabled", Value::Boolean(true), true)
            .unwrap();
        assert!(c.bool("local.enabled").unwrap());
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

    #[test]
    fn sandbox_network_reads_old_booleans_and_loosens_only_with_confirmation() {
        let v = |t: &str| parse_value(t).unwrap();
        let c = Config::load(Path::new("/nonexistent/owner.toml"), None).unwrap();
        assert_eq!(c.str("sandbox.network").unwrap(), "registries");
        assert!(
            c.list("sandbox.registries")
                .unwrap()
                .contains(&"index.crates.io".to_owned())
        );
        assert!(
            !c.list("sandbox.registries")
                .unwrap()
                .contains(&"github.com".to_owned())
        );

        // Booleans from earlier versions: true is all, false is off, with a note.
        for (text, now) in [("true", "all"), ("false", "off")] {
            let (_d, o, p) = files(&format!("[sandbox]\nnetwork = {text}\n"), "");
            let c = Config::load(&o, Some(&p)).unwrap();
            assert_eq!(c.str("sandbox.network").unwrap(), now);
            assert_eq!(c.notes.len(), 1, "{:?}", c.notes);
            assert!(c.notes[0].contains("deprecated") && c.notes[0].contains(now));
        }
        let (_d, o, p) = files("", "[sandbox]\nnetwork = false\n");
        let c = Config::load(&o, Some(&p)).unwrap();
        assert_eq!(c.str("sandbox.network").unwrap(), "off");

        // A project may only tighten: off always, registries only from all.
        for (owner, project, ok) in [
            ("", "\"off\"", true),
            ("", "\"all\"", false),
            ("", "true", false),
            ("[sandbox]\nnetwork = \"off\"\n", "\"registries\"", false),
            ("[sandbox]\nnetwork = \"all\"\n", "\"registries\"", true),
        ] {
            let (_d, o, p) = files(owner, &format!("[sandbox]\nnetwork = {project}\n"));
            let loaded = Config::load(&o, Some(&p));
            assert_eq!(loaded.is_ok(), ok, "{owner} / {project}: {loaded:?}");
            if !ok {
                assert!(matches!(loaded, Err(ConfigError::Loosening { .. })));
            }
        }

        // The owner loosens with confirmation (off < registries < all).
        let (_d, o, p) = files("", "");
        let mut c = Config::load(&o, Some(&p)).unwrap();
        for (from, to) in [("registries", "all"), ("off", "registries"), ("off", "all")] {
            c.set_owner("sandbox.network", v(&format!("\"{from}\"")))
                .unwrap();
            assert!(
                matches!(
                    c.set_owner_checked("sandbox.network", v(&format!("\"{to}\"")), false),
                    Err(ConfigError::NeedsConfirm { .. })
                ),
                "{from} -> {to}"
            );
        }
        c.set_owner("sandbox.network", v("\"all\"")).unwrap();
        let change = c
            .set_owner_checked("sandbox.network", v("\"registries\""), false)
            .unwrap();
        assert!(change.weakens.is_none());
        // `config set sandbox.network true` still works, as all.
        let change = c
            .set_owner_checked("sandbox.network", v("true"), true)
            .unwrap();
        assert_eq!(change.new, v("\"all\""));
        assert!(change.weakens.is_some());
        assert!(matches!(
            c.set_owner_checked("sandbox.network", v("\"registry\""), true),
            Err(ConfigError::Invalid { .. })
        ));

        // Registries: adding a host loosens, removing one tightens; owner only.
        let hosts = c.list("sandbox.registries").unwrap();
        let mut more = hosts.clone();
        more.push("npm.pkg.github.com".into());
        let as_value = |l: &[String]| Value::Array(l.iter().cloned().map(Value::String).collect());
        assert!(matches!(
            c.set_owner_checked("sandbox.registries", as_value(&more), false),
            Err(ConfigError::NeedsConfirm { .. })
        ));
        let fewer = &hosts[1..];
        assert!(
            c.set_owner_checked("sandbox.registries", as_value(fewer), false)
                .unwrap()
                .weakens
                .is_none()
        );
        let (_d, o, p) = files("", "[sandbox]\nregistries = []\n");
        assert!(matches!(
            Config::load(&o, Some(&p)),
            Err(ConfigError::OwnerOnly { .. })
        ));
    }
}
