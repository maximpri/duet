// SPDX-License-Identifier: GPL-3.0-or-later
//! The policy layer: settings a program that embeds Duet imposes above the
//! owner's and the project's (ARCHITECTURE.md, "Embedding Duet").
//!
//! A [`PolicySource`] supplies the policy as TOML text in the settings' own
//! layout, after checking it however it must: [`PolicyFile`] reads an
//! unsigned local file; an embedding program may verify a signature, an
//! expiry or who issued it. An optional `[policy]` table names the policy
//! (`name`, `version`) and lists the keys it `required`s.
//!
//! Every setting the policy holds bounds every other layer (owner file,
//! project file, `duet config set`, presets, command-line overrides):
//! - a setting with a tighten direction may only be as tight as the policy's
//!   value or tighter: a looser value from another layer gives way (the
//!   tighter of the two applies, with a note), and a change that would loosen
//!   past it is refused;
//! - a setting without a direction, and every key in `required`, is fixed at
//!   the policy's value: nothing may change it.
//!
//! A source that is configured but cannot load or verify its policy is an
//! error ([`crate::ConfigError::PolicyLoad`]): the caller refuses to run.
//! Values of template settings (`mcp.servers.<name>.*`) apply to an instance
//! the other layers configure; a policy does not create one.

use crate::{Direction, Setting, Value, as_f64, flatten, is_template, migrate, setting, validate};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// What is known about a policy besides its settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[non_exhaustive]
pub struct PolicyMeta {
    /// Where the policy came from (a path, a service). Set by the source.
    pub origin: String,
    /// How the source checked it (`unsigned file`, a signature and its key).
    /// Set by the source.
    pub verified_by: String,
    /// The `[policy]` table's `name` (empty without one). Set when loaded.
    pub name: String,
    /// The `[policy]` table's `version` (empty without one). Set when loaded.
    pub version: String,
    /// SHA-256 of the policy text as loaded. Set when loaded.
    pub sha256: String,
}

impl PolicyMeta {
    /// What a source says about the policy it loaded; the rest is filled in
    /// from the text when it is parsed.
    pub fn new(origin: impl Into<String>, verified_by: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            verified_by: verified_by.into(),
            ..Self::default()
        }
    }
}

impl std::fmt::Display for PolicyMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match (self.name.as_str(), self.version.as_str()) {
            ("", "") => "(unnamed)".to_owned(),
            (n, "") => n.to_owned(),
            ("", v) => format!("version {v}"),
            (n, v) => format!("{n} {v}"),
        };
        write!(f, "{name} from {}, {}", self.origin, self.verified_by)
    }
}

/// A policy that is configured but cannot be used: not found, unreadable,
/// failed verification, or invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("policy from {origin}: {reason}")]
pub struct PolicyError {
    pub origin: String,
    pub reason: String,
}

impl PolicyError {
    pub fn new(origin: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            reason: reason.into(),
        }
    }
}

/// Supplies the policy layer. Called once per configuration load.
pub trait PolicySource: Send + Sync {
    /// The policy as TOML text with what the source knows about it, or
    /// `None` when no policy applies. An error means a policy should apply
    /// but cannot be used; the configuration then does not load.
    fn load(&self) -> Result<Option<(String, PolicyMeta)>, PolicyError>;
}

/// Core's policy source: an unsigned TOML file. A missing or unreadable file
/// is an error, since a policy that was configured must apply. Nothing checks
/// who wrote the file: protect it with file permissions, or use a source that
/// verifies a signature.
#[derive(Debug, Clone)]
pub struct PolicyFile {
    path: PathBuf,
}

impl PolicyFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl PolicySource for PolicyFile {
    fn load(&self) -> Result<Option<(String, PolicyMeta)>, PolicyError> {
        let origin = self.path.display().to_string();
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| PolicyError::new(&origin, e.to_string()))?;
        Ok(Some((text, PolicyMeta::new(origin, "unsigned file"))))
    }
}

/// A loaded, validated policy.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    meta: PolicyMeta,
    values: BTreeMap<String, Value>,
    required: BTreeSet<String>,
}

impl Policy {
    /// Parses and validates a policy: every key is a registered setting (or
    /// an instance of a template setting) with a valid value, the `[policy]`
    /// table holds only `name`, `version` and `required`, and every required
    /// key has a value.
    pub fn parse(text: &str, mut meta: PolicyMeta) -> Result<Self, PolicyError> {
        let origin = meta.origin.clone();
        let bad = |reason: String| PolicyError::new(&origin, reason);
        let mut table: toml::Table = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
        let mut required = BTreeSet::new();
        if let Some(head) = table.remove("policy") {
            let Value::Table(head) = head else {
                return Err(bad("[policy] must be a table".into()));
            };
            for (k, v) in head {
                match (k.as_str(), v) {
                    ("name", Value::String(s)) => meta.name = s,
                    ("version", Value::String(s)) => meta.version = s,
                    ("required", Value::Array(keys)) => {
                        for key in keys {
                            let Value::String(key) = key else {
                                return Err(bad("policy.required must list setting keys".into()));
                            };
                            required.insert(key);
                        }
                    }
                    (k, _) => {
                        return Err(bad(format!(
                            "policy.{k}: the [policy] table holds name, version and required (as text, text and a list)"
                        )));
                    }
                }
            }
        }
        let mut flat = Vec::new();
        flatten("", &table, &mut flat);
        let mut values = BTreeMap::new();
        for (key, v) in flat {
            let s = setting(&key)
                .filter(|_| !is_template(&key))
                .ok_or_else(|| bad(format!("unknown setting {key}")))?;
            let v = migrate(&key, v).0;
            validate(s, &v).map_err(|e| bad(e.to_string()))?;
            values.insert(key, v);
        }
        if let Some(missing) = required.iter().find(|k| !values.contains_key(*k)) {
            return Err(bad(format!(
                "policy.required names {missing}, which the policy does not set"
            )));
        }
        meta.sha256 = hex::encode(Sha256::digest(text.as_bytes()));
        Ok(Self {
            meta,
            values,
            required,
        })
    }

    pub fn meta(&self) -> &PolicyMeta {
        &self.meta
    }

    /// The settings the policy holds, by key.
    pub fn values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }

    /// The policy's value of `key`, when it holds one.
    pub fn value(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Keys listed in `[policy] required`.
    pub fn required(&self) -> &BTreeSet<String> {
        &self.required
    }

    /// Whether `key` is fixed at the policy's value: required, or a setting
    /// without a tighten direction.
    pub fn fixes(&self, key: &str) -> bool {
        self.values.contains_key(key)
            && (self.required.contains(key)
                || setting(key).is_none_or(|s| s.direction == Direction::Any))
    }

    /// Why `value` is not allowed for `key`, or `Ok` when the policy does not
    /// hold the key or the value is as tight as its bound (see the module
    /// documentation).
    pub fn allows(&self, key: &str, value: &Value) -> Result<(), String> {
        let (Some(bound), Some(s)) = (self.values.get(key), setting(key)) else {
            return Ok(());
        };
        if self.fixes(key) {
            return match same(bound, value) {
                true => Ok(()),
                false if self.required.contains(key) => {
                    Err(format!("it is required, fixed at {bound}"))
                }
                false => Err(format!("it is fixed at {bound}")),
            };
        }
        crate::tightens(s, bound, value)
            .map_err(|rule| format!("its bound is {bound}; you may only {rule}"))
    }

    /// How the policy bounds `key`, for the operator; `None` when it does not.
    pub fn rule(&self, key: &str) -> Option<String> {
        let bound = self.values.get(key)?;
        Some(if self.required.contains(key) {
            format!("required by the policy at {bound}")
        } else if self.fixes(key) {
            format!("fixed by the policy at {bound}")
        } else {
            let rule = setting(key)
                .and_then(|s| tighten_rule(s.direction))
                .unwrap_or("tighten it");
            format!("the policy bounds it at {bound}; other layers may only {rule}")
        })
    }
}

/// What tightening a setting with direction `d` means, in words; `None` for
/// [`Direction::Any`].
pub fn tighten_rule(d: Direction) -> Option<&'static str> {
    Some(match d {
        Direction::Any => return None,
        Direction::AddOnly => "add entries",
        Direction::RemoveOnly => "remove entries",
        Direction::OnlyTrue => "turn it on",
        Direction::OnlyFalse => "turn it off",
        Direction::OnlyLower => "lower it",
        Direction::OnlyLaterChoice => "choose a stricter option",
    })
}

/// Equal values, numbers compared by value (`5` and `5.0`).
pub(crate) fn same(a: &Value, b: &Value) -> bool {
    a == b
        || matches!((a, b), (Value::Integer(_) | Value::Float(_), Value::Integer(_) | Value::Float(_)) if as_f64(a) == as_f64(b))
}

fn strings(v: &Value) -> Vec<&str> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The value at least as tight as both the policy's `bound` and `other` (a
/// lower layer's), for a setting with a tighten direction: the union of lists
/// one may only add to, the intersection of allowlists, the lower number, the
/// stricter choice, the tighter boolean.
pub(crate) fn tightest(s: &Setting, bound: &Value, other: &Value) -> Value {
    let list = |items: Vec<&str>| {
        Value::Array(
            items
                .into_iter()
                .map(|x| Value::String(x.to_owned()))
                .collect(),
        )
    };
    match s.direction {
        Direction::Any => bound.clone(),
        Direction::AddOnly => {
            let mut all = strings(bound);
            for x in strings(other) {
                if !all.contains(&x) {
                    all.push(x);
                }
            }
            list(all)
        }
        Direction::RemoveOnly => {
            let allowed = strings(bound);
            list(
                strings(other)
                    .into_iter()
                    .filter(|x| allowed.contains(x))
                    .collect(),
            )
        }
        Direction::OnlyTrue => {
            Value::Boolean(bound.as_bool() == Some(true) || other.as_bool() == Some(true))
        }
        Direction::OnlyFalse => {
            Value::Boolean(bound.as_bool() == Some(true) && other.as_bool() == Some(true))
        }
        Direction::OnlyLower | Direction::OnlyLaterChoice => match crate::tightens(s, bound, other)
        {
            Ok(()) => other.clone(),
            Err(_) => bound.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ConfigError, Origin, REGISTRY, Target, parse_value};
    use std::path::Path;

    /// A policy source that returns fixed text.
    struct Text(&'static str);

    impl PolicySource for Text {
        fn load(&self) -> Result<Option<(String, PolicyMeta)>, PolicyError> {
            Ok(Some((
                self.0.into(),
                PolicyMeta::new("test", "not verified"),
            )))
        }
    }

    const POLICY: &str = r#"
[policy]
name = "acme"
version = "7"
required = ["local.enabled"]

[frontier]
base_url = "https://gateway.acme.example/v1"
allow_passthrough = false

[local]
enabled = true

[limits]
frontier_usd = 2.0

[sensitivity]
globs = ["*.pem", "customers/**"]
detect_pii = true

[sandbox]
network = "registries"
registries = ["crates.io", "index.crates.io"]

[oversight]
approve = "risky"
"#;

    /// Owner settings looser than the policy on every key it holds.
    const LOOSE_OWNER: &str = r#"
[frontier]
base_url = "https://elsewhere.example/v1"
allow_passthrough = true

[local]
enabled = false

[limits]
frontier_usd = 50.0

[sensitivity]
globs = ["*.key"]
detect_pii = false

[sandbox]
network = "all"
registries = ["crates.io", "uploads.example"]

[oversight]
approve = "off"
"#;

    fn files(owner: &str, project: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let (o, p) = (d.path().join("owner.toml"), d.path().join("project.toml"));
        std::fs::write(&o, owner).unwrap();
        std::fs::write(&p, project).unwrap();
        (d, o, p)
    }

    fn load(o: &Path, p: &Path) -> Config {
        Config::load_with(o, Some(p), Some(&Text(POLICY))).unwrap()
    }

    fn v(text: &str) -> Value {
        parse_value(text).unwrap()
    }

    #[test]
    fn looser_values_from_the_files_give_way_to_the_policy() {
        // Two owner files, as DUET_CONFIG_HOME can point at another one: the
        // policy bounds whichever is loaded.
        for owner in [LOOSE_OWNER, ""] {
            let (_d, o, p) = files(owner, "[limits]\nfrontier_usd = 1.5\n");
            let c = load(&o, &p);
            assert_eq!(
                c.str("frontier.base_url").unwrap(),
                "https://gateway.acme.example/v1"
            );
            assert!(!c.bool("frontier.allow_passthrough").unwrap());
            assert!(c.bool("local.enabled").unwrap());
            assert!(c.bool("sensitivity.detect_pii").unwrap());
            assert_eq!(c.str("sandbox.network").unwrap(), "registries");
            assert_eq!(c.str("oversight.approve").unwrap(), "risky");
            // A project value tighter than the policy applies.
            assert_eq!(c.float("limits.frontier_usd").unwrap(), 1.5);
            assert_eq!(c.origin("limits.frontier_usd"), Some(Origin::Project));
            assert_eq!(c.origin("frontier.base_url"), Some(Origin::Policy));
        }
        let (_d, o, p) = files(LOOSE_OWNER, "[limits]\nfrontier_usd = 10.0\n");
        let c = load(&o, &p);
        assert_eq!(c.float("limits.frontier_usd").unwrap(), 2.0);
        assert_eq!(c.origin("limits.frontier_usd"), Some(Origin::Policy));
        // Lists: entries one may only add are kept from both; an allowlist
        // keeps only what both allow.
        assert_eq!(
            c.list("sensitivity.globs").unwrap(),
            ["*.pem", "customers/**", "*.key"]
        );
        assert_eq!(c.list("sandbox.registries").unwrap(), ["crates.io"]);
        // Every owner and project value that gave way is noted.
        for key in [
            "frontier.base_url",
            "frontier.allow_passthrough",
            "local.enabled",
            "limits.frontier_usd",
            "sensitivity.detect_pii",
            "sandbox.network",
            "sandbox.registries",
            "oversight.approve",
        ] {
            assert!(
                c.notes
                    .iter()
                    .any(|n| n.contains(key) && n.contains("acme 7")),
                "{key}: {:?}",
                c.notes
            );
        }
        assert!(
            c.notes
                .iter()
                .any(|n| n.contains("project.toml") && n.contains("limits.frontier_usd"))
        );
        assert_eq!(c.policy_overrides().len(), 9, "{:?}", c.policy_overrides());
    }

    #[test]
    fn tighter_values_from_the_files_apply() {
        let (_d, o, p) = files(
            "[sandbox]\nnetwork = \"off\"\nregistries = [\"crates.io\"]\n[oversight]\napprove = \"all\"\n\
             [limits]\nfrontier_usd = 1.0\n[sensitivity]\nglobs = [\"*.pem\"]\n",
            "[sensitivity]\nglobs = [\"*.pem\", \"customers/**\", \"reports/**\"]\n",
        );
        let c = load(&o, &p);
        assert_eq!(c.str("sandbox.network").unwrap(), "off");
        assert_eq!(c.origin("sandbox.network"), Some(Origin::Owner));
        assert_eq!(c.str("oversight.approve").unwrap(), "all");
        assert_eq!(c.float("limits.frontier_usd").unwrap(), 1.0);
        assert_eq!(c.list("sandbox.registries").unwrap(), ["crates.io"]);
        assert_eq!(
            c.list("sensitivity.globs").unwrap(),
            ["*.pem", "customers/**", "reports/**"]
        );
        assert!(c.notes.is_empty(), "{:?}", c.notes);
    }

    #[test]
    fn changes_past_the_policy_are_refused_and_tightening_is_not() {
        let (_d, o, p) = files("", "");
        let mut c = load(&o, &p);
        fn refused<T>(r: Result<T, ConfigError>) -> bool {
            matches!(r, Err(ConfigError::Policy { .. }))
        }
        // A project file refuses an owner-only key before the policy is asked.
        fn refused_in_project<T>(key: &str, r: Result<T, ConfigError>) -> bool {
            match r {
                Err(ConfigError::OwnerOnly { .. }) => {
                    crate::setting(key).unwrap().scope == crate::Scope::Owner
                }
                r => refused(r),
            }
        }
        for (key, value) in [
            ("frontier.base_url", "\"https://elsewhere.example/v1\""),
            ("frontier.allow_passthrough", "true"),
            ("limits.frontier_usd", "3.0"),
            ("sensitivity.globs", "[\"*.pem\"]"),
            ("sensitivity.detect_pii", "false"),
            ("sandbox.network", "\"all\""),
            ("sandbox.registries", "[\"crates.io\", \"uploads.example\"]"),
            ("oversight.approve", "\"off\""),
            // Required: even the tighter value is refused.
            ("local.enabled", "false"),
        ] {
            assert!(refused(c.propose(Target::Owner, key, v(value))), "{key}");
            assert!(
                refused(c.apply(Target::Owner, key, v(value), true)),
                "{key}"
            );
            assert!(refused(c.set_owner_checked(key, v(value), true)), "{key}");
            assert!(refused(c.set_owner(key, v(value))), "{key}");
            let project = c.propose(Target::Project, key, v(value));
            assert!(refused_in_project(key, project), "{key}");
            let project = c.apply(Target::Project, key, v(value), true);
            assert!(refused_in_project(key, project), "{key}");
            assert!(
                refused_in_project(key, c.set_project(key, v(value))),
                "{key}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&o).unwrap(),
            "",
            "nothing was written"
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "");
        let e = c
            .propose(Target::Owner, "local.enabled", v("false"))
            .unwrap_err();
        assert!(
            e.to_string().contains("required") && e.to_string().contains("acme 7"),
            "{e}"
        );

        // Tighter than the policy, or its own value: allowed.
        for (key, value) in [
            ("frontier.base_url", "\"https://gateway.acme.example/v1\""),
            ("local.enabled", "true"),
            ("limits.frontier_usd", "1.0"),
            ("sandbox.network", "\"off\""),
            ("sandbox.registries", "[\"crates.io\"]"),
            ("oversight.approve", "\"all\""),
        ] {
            c.apply(Target::Owner, key, v(value), false)
                .unwrap_or_else(|e| panic!("{key}: {e}"));
        }
        let mut globs = c.list("sensitivity.globs").unwrap();
        globs.push("reports/**".into());
        let globs = Value::Array(globs.into_iter().map(Value::String).collect());
        c.apply(Target::Project, "sensitivity.globs", globs, false)
            .unwrap();
        // Keys the policy does not hold are unaffected.
        c.apply(Target::Owner, "local.model", v("\"other\""), false)
            .unwrap();
        let again = load(&o, &p);
        assert_eq!(again.float("limits.frontier_usd").unwrap(), 1.0);
        assert!(
            again
                .list("sensitivity.globs")
                .unwrap()
                .contains(&"reports/**".to_owned())
        );
    }

    #[test]
    fn command_line_overrides_are_checked_against_the_policy() {
        let (_d, o, p) = files("", "");
        let c = load(&o, &p);
        assert!(
            c.allows(
                "frontier.base_url",
                &v("\"https://gateway.acme.example/v1\"")
            )
            .is_ok()
        );
        assert!(matches!(
            c.allows("frontier.base_url", &v("\"https://elsewhere.example/v1\"")),
            Err(ConfigError::Policy { .. })
        ));
        assert!(c.allows("frontier.model", &v("\"any\"")).is_ok());
        assert!(c.allows("limits.frontier_usd", &v("1")).is_ok());
        assert!(c.allows("limits.frontier_usd", &v("2")).is_ok());
        assert!(c.allows("limits.frontier_usd", &v("2.5")).is_err());
        // Without a policy everything is allowed.
        let plain = Config::load(&o, Some(&p)).unwrap();
        assert!(
            plain
                .allows("frontier.base_url", &v("\"https://elsewhere.example/v1\""))
                .is_ok()
        );
        assert!(plain.policy().is_none());
    }

    #[test]
    fn a_policy_that_cannot_be_used_fails_closed() {
        struct Failing;
        impl PolicySource for Failing {
            fn load(&self) -> Result<Option<(String, PolicyMeta)>, PolicyError> {
                Err(PolicyError::new("vault", "signature does not verify"))
            }
        }
        struct NoPolicy;
        impl PolicySource for NoPolicy {
            fn load(&self) -> Result<Option<(String, PolicyMeta)>, PolicyError> {
                Ok(None)
            }
        }
        let (d, o, p) = files(LOOSE_OWNER, "");
        let e = Config::load_with(&o, Some(&p), Some(&Failing)).unwrap_err();
        assert!(matches!(&e, ConfigError::PolicyLoad(_)));
        assert!(e.to_string().contains("signature does not verify"), "{e}");
        for bad in [
            "[frontier]\nbase_urll = \"x\"\n",
            "[limits]\nfrontier_usd = \"a lot\"\n",
            "[policy]\nrequired = [\"limits.frontier_usd\"]\n",
            "[policy]\nexpires = \"never\"\n",
            "[policy]\nrequired = [1]\n",
            "policy = 1\n",
            "[mcp.servers.\"*\"]\nnetwork = false\n",
            "not toml at all",
        ] {
            let text: &'static str = Box::leak(bad.to_owned().into_boxed_str());
            assert!(
                matches!(
                    Config::load_with(&o, Some(&p), Some(&Text(text))),
                    Err(ConfigError::PolicyLoad(_))
                ),
                "{bad}"
            );
        }
        let missing = PolicyFile::new(d.path().join("no-policy.toml"));
        assert!(matches!(
            Config::load_with(&o, Some(&p), Some(&missing)),
            Err(ConfigError::PolicyLoad(_))
        ));
        // A source may say that no policy applies.
        let none = Config::load_with(&o, Some(&p), Some(&NoPolicy)).unwrap();
        assert!(none.policy().is_none());
        assert_eq!(none.float("limits.frontier_usd").unwrap(), 50.0);
    }

    #[test]
    fn the_file_source_reads_an_unsigned_policy_and_its_meta() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("policy.toml");
        std::fs::write(&path, POLICY).unwrap();
        let (_d, o, p) = files("", "");
        let c = Config::load_with(&o, Some(&p), Some(&PolicyFile::new(&path))).unwrap();
        let meta = c.policy().unwrap().meta();
        assert_eq!(meta.origin, path.display().to_string());
        assert_eq!(meta.verified_by, "unsigned file");
        assert_eq!((meta.name.as_str(), meta.version.as_str()), ("acme", "7"));
        assert_eq!(meta.sha256, hex::encode(Sha256::digest(POLICY.as_bytes())));
        assert!(meta.to_string().starts_with("acme 7 from "));
        assert_eq!(c.policy().unwrap().required().len(), 1);
    }

    #[test]
    fn template_instances_are_bounded_but_not_created() {
        const SERVERS: &str = "[mcp.servers.files]\nnetwork = false\n";
        let (_d, o, p) = files("", "");
        let c = Config::load_with(&o, Some(&p), Some(&Text(SERVERS))).unwrap();
        assert!(c.instances("mcp.servers.*.command").is_empty());
        let (_d, o, p) = files(
            "[mcp.servers.files]\ncommand = \"srv\"\nnetwork = true\n",
            "",
        );
        let mut c = Config::load_with(&o, Some(&p), Some(&Text(SERVERS))).unwrap();
        assert!(!c.bool("mcp.servers.files.network").unwrap());
        assert_eq!(c.origin("mcp.servers.files.network"), Some(Origin::Policy));
        assert!(matches!(
            c.set_owner_checked("mcp.servers.files.network", Value::Boolean(true), true),
            Err(ConfigError::Policy { .. })
        ));
    }

    #[test]
    fn rules_explain_the_bound_of_each_key() {
        let (_d, o, p) = files("", "");
        let c = load(&o, &p);
        assert!(
            c.policy_rule("local.enabled")
                .unwrap()
                .starts_with("required by the policy")
        );
        assert!(
            c.policy_rule("frontier.base_url")
                .unwrap()
                .starts_with("fixed by the policy")
        );
        let bound = c.policy_rule("limits.frontier_usd").unwrap();
        assert!(
            bound.contains("2.0") && bound.contains("lower it"),
            "{bound}"
        );
        assert!(c.policy_rule("frontier.model").is_none());
    }

    #[test]
    fn no_setting_uses_the_policy_table_name() {
        assert!(REGISTRY.iter().all(|s| !s.key.starts_with("policy.")));
    }
}
