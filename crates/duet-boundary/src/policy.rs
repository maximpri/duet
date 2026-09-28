// SPDX-License-Identifier: GPL-3.0-or-later
//! Which content is sensitive, by where it came from.

use std::path::Path;

/// Glob over `/`-separated paths: `*` and `?` stay within a segment, `**`
/// matches any number of segments. A pattern without `/` matches the file name
/// at any depth.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    fn seg(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => seg(&p[1..], s) || (!s.is_empty() && seg(p, &s[1..])),
            (Some(b'?'), Some(_)) => seg(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => seg(&p[1..], &s[1..]),
            _ => false,
        }
    }
    fn walk(p: &[&str], s: &[&str]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(&"**") => walk(&p[1..], s) || (!s.is_empty() && walk(p, &s[1..])),
            Some(pat) => {
                !s.is_empty() && seg(pat.as_bytes(), s[0].as_bytes()) && walk(&p[1..], &s[1..])
            }
        }
    }
    let split = |x: &str| -> Vec<String> {
        x.split('/')
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let p = split(pattern);
    let s = split(path);
    let p: Vec<&str> = p.iter().map(String::as_str).collect();
    let s: Vec<&str> = s.iter().map(String::as_str).collect();
    if !pattern.contains('/') {
        return s
            .last()
            .is_some_and(|name| seg(pattern.as_bytes(), name.as_bytes()));
    }
    walk(&p, &s)
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub sensitive_globs: Vec<String>,
    pub protected_paths: Vec<String>,
    pub command_output_sensitive: bool,
    pub raw_ok_commands: Vec<String>,
    pub secret_sinks: Vec<String>,
    pub detect_secrets: bool,
    pub detect_pii: bool,
    pub detect_entropy: bool,
    /// The owner's and project's own detectors (`sensitivity.custom_patterns`).
    pub custom_patterns: Vec<String>,
    pub bulky_tokens: usize,
    /// Brief the frontier on the sensitive files at run start (needs a local model).
    pub local_brief: bool,
    /// The local model marks names and postal addresses in the free text of
    /// public content (`sensitivity.local_pii_pass`; needs a local model).
    pub local_pii_pass: bool,
    /// Threshold for a public file the model explicitly read (`0` disables offloading).
    pub bulky_file_tokens: usize,
    /// Test, build and install output the frontier may be shown is condensed
    /// (`context.condense_output`; [`crate::condense`]).
    pub condense_output: bool,
    /// Source the frontier sees as signatures only (`ip.interface_only`).
    pub interface_only: Vec<String>,
    /// Source whose content the frontier never sees (`ip.sealed`).
    pub sealed: Vec<String>,
    /// The local model reads images (`local.vision`): images the frontier may
    /// not see are described by it instead of refused.
    pub local_vision: bool,
    /// `images.to_frontier`: which images the frontier may receive itself.
    pub images_to_frontier: crate::images::ToFrontier,
    /// Structure views, synthetic samples, masked output and the probe
    /// budget for short output of commands that read sensitive data.
    pub structure: StructureSettings,
}

/// What the frontier is shown of the structure of sensitive data (see
/// [`crate::structure`]). The default shows nothing new and sets no probe
/// budget; `duet` reads the shipped values from the configuration.
#[derive(Debug, Clone, Default)]
pub struct StructureSettings {
    /// Structure views of sensitive content, a structure outline in the task
    /// note and masked output of commands that read sensitive data
    /// (`sensitivity.structure_views`).
    pub views: bool,
    /// Records a synthetic sample may hold (`sensitivity.synthetic_rows`);
    /// 0: none are made.
    pub synthetic_rows: usize,
    /// Small numbers masked output may show as written over a run
    /// (`sensitivity.masked_numbers`).
    pub masked_numbers: u32,
    /// Short outputs of commands that read sensitive data shown over a run
    /// (`sensitivity.output_probes`); `None`: no budget.
    pub output_probes: Option<u32>,
}

/// How much of a protected source file the frontier may see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpLevel {
    /// Signatures, types, doc comments and public constants; bodies withheld.
    InterfaceOnly,
    /// Existence only.
    Sealed,
}

impl Policy {
    pub fn is_sensitive_path(&self, path: &Path) -> bool {
        let p = path.to_string_lossy();
        self.protected_paths.iter().any(|g| glob_match(g, &p))
            || !is_env_template(path) && self.sensitive_globs.iter().any(|g| glob_match(g, &p))
    }

    /// The IP level of `path`; Sealed wins when both lists match.
    pub fn ip_level(&self, path: &Path) -> Option<IpLevel> {
        let p = path.to_string_lossy();
        if self.sealed.iter().any(|g| glob_match(g, &p)) {
            Some(IpLevel::Sealed)
        } else if self.interface_only.iter().any(|g| glob_match(g, &p)) {
            Some(IpLevel::InterfaceOnly)
        } else {
            None
        }
    }

    /// Whether any path is protected by an IP level.
    pub fn has_ip(&self) -> bool {
        !self.interface_only.is_empty() || !self.sealed.is_empty()
    }

    pub fn is_secret_sink(&self, path: &Path) -> bool {
        let p = path.to_string_lossy();
        self.secret_sinks.iter().any(|g| glob_match(g, &p))
    }

    /// Commands whose output may be shown raw (after scanning), by prefix.
    pub fn command_is_raw_ok(&self, command: &str) -> bool {
        let c = command.trim();
        self.raw_ok_commands
            .iter()
            .any(|ok| c == ok || c.starts_with(&format!("{ok} ")))
    }
}

/// Key=value style files whose values are the sensitive part (structure is safe to show).
pub fn is_secret_bearing(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    !is_env_template(path)
        && (name.starts_with(".env")
            || name.ends_with(".env")
            || name.contains("secret")
            || name.contains("credential"))
}

/// A committed template of an environment file (`.env.example`,
/// `.env.sample`, `.env.template`, `.env.dist`, `.env.defaults`): made to be
/// shared, it holds placeholders and defaults, so it is public content
/// (still scanned: a real key committed in one is found as in any file).
pub fn is_env_template(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    const TEMPLATES: &[&str] = &["example", "sample", "template", "dist", "defaults"];
    name.starts_with(".env")
        && name
            .rsplit('.')
            .next()
            .is_some_and(|last| TEMPLATES.contains(&last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_names_anywhere_and_paths_exactly() {
        assert!(glob_match(".env*", ".env"));
        assert!(glob_match(".env*", "services/api/.env.prod"));
        assert!(glob_match("*.csv", "data/customers.csv"));
        assert!(glob_match("data/**", "data/statements/a.csv"));
        assert!(glob_match("logs/**", "logs/prod.log"));
        assert!(!glob_match("logs/**", "src/logs.rs"));
        assert!(glob_match("src/pricing/**", "src/pricing/engine.rs"));
        assert!(!glob_match("*.log", "src/log.rs"));
    }

    #[test]
    fn env_templates_are_public_and_env_files_are_not() {
        let p = Policy {
            sensitive_globs: vec![".env*".into(), "**/.env*".into()],
            ..Policy::default()
        };
        for public in [
            ".env.example",
            "services/api/.env.sample",
            ".env.template",
            ".env.production.example",
            ".env.dist",
            ".env.defaults",
        ] {
            assert!(!p.is_sensitive_path(Path::new(public)), "{public}");
            assert!(!is_secret_bearing(Path::new(public)), "{public}");
        }
        for secret in [
            ".env",
            ".env.local",
            "services/api/.env.prod",
            ".env.examples.bak",
        ] {
            assert!(p.is_sensitive_path(Path::new(secret)), "{secret}");
            assert!(is_secret_bearing(Path::new(secret)), "{secret}");
        }
        // A template the owner protects stays protected.
        let p = Policy {
            protected_paths: vec![".env.example".into()],
            ..p
        };
        assert!(p.is_sensitive_path(Path::new(".env.example")));
    }

    #[test]
    fn ip_levels_come_from_their_globs_and_sealed_wins() {
        let p = Policy {
            interface_only: vec!["src/pricing/**".into()],
            sealed: vec!["src/pricing/secret_*.rs".into()],
            ..Policy::default()
        };
        assert_eq!(
            p.ip_level(Path::new("src/pricing/engine.rs")),
            Some(IpLevel::InterfaceOnly)
        );
        assert_eq!(
            p.ip_level(Path::new("src/pricing/secret_table.rs")),
            Some(IpLevel::Sealed)
        );
        assert_eq!(p.ip_level(Path::new("src/invoice.rs")), None);
        assert!(p.has_ip() && !Policy::default().has_ip());
        assert!(!p.is_sensitive_path(Path::new("src/pricing/engine.rs")));
    }

    #[test]
    fn raw_ok_commands_match_by_prefix() {
        let p = Policy {
            raw_ok_commands: vec!["cargo check".into()],
            ..Policy::default()
        };
        assert!(p.command_is_raw_ok("cargo check --offline"));
        assert!(!p.command_is_raw_ok("cargo checkout"));
        assert!(!p.command_is_raw_ok("cat .env"));
    }
}
