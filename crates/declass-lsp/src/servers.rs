// SPDX-License-Identifier: GPL-3.0-or-later
//! Which server handles which files: a built-in table of common servers found
//! on `PATH`, overridden or extended by the owner's `[lsp.servers.<language>]`.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A server as configured (`[lsp.servers.<language>]`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServerConfig {
    /// A program name looked up on `PATH`, or an absolute path.
    pub command: String,
    pub args: Vec<String>,
    /// File extensions (without the dot) this server handles. Optional when
    /// overriding a built-in language, which keeps its extensions.
    pub extensions: Vec<String>,
    /// Names of environment variables passed through to the server (the
    /// sandbox clears everything else outside its own allowlist).
    pub env: Vec<String>,
}

impl ServerConfig {
    /// Why this entry for `language` cannot be used, if it cannot.
    pub fn check(&self, language: &str) -> Result<(), String> {
        let at = format!("lsp.servers.{language}");
        if self.command.trim().is_empty() {
            return Err(format!("{at}.command is not set"));
        }
        if self.command.contains('/') && !Path::new(&self.command).is_absolute() {
            return Err(format!(
                "{at}.command must be a program name or an absolute path"
            ));
        }
        if self.extensions.is_empty() && builtin(language).is_none() {
            return Err(format!(
                "{at}.extensions is required for a language without a built-in server"
            ));
        }
        let env_name = |n: &String| {
            !n.is_empty()
                && !n.starts_with(|c: char| c.is_ascii_digit())
                && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        };
        if let Some(bad) = self.env.iter().find(|n| !env_name(n)) {
            return Err(format!(
                "{at}.env: {bad:?} is not an environment variable name"
            ));
        }
        Ok(())
    }
}

/// The language-server settings of a run.
#[derive(Debug, Clone)]
pub struct Settings {
    pub enabled: bool,
    /// `[lsp.servers.<language>]` from the owner's configuration.
    pub servers: BTreeMap<String, ServerConfig>,
    pub request_timeout: Duration,
    /// How long an edit waits for the server's diagnostics of the file.
    pub diagnostics_wait: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            servers: BTreeMap::new(),
            request_timeout: Duration::from_secs(30),
            diagnostics_wait: Duration::from_secs(2),
        }
    }
}

/// One server, resolved: what to start for which files.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSpec {
    /// The language key (`rust`, `typescript`, or a configured name).
    pub language: String,
    /// Commands tried in order; the first found is used.
    pub candidates: Vec<(String, Vec<String>)>,
    pub extensions: Vec<String>,
    pub env: Vec<String>,
    /// Sent as `initializationOptions`.
    pub init_options: Value,
    /// Answers to `workspace/configuration`, by section.
    pub settings: Value,
    /// Whether the entry comes from the owner's configuration.
    pub configured: bool,
}

/// A server whose program was found.
#[derive(Debug, Clone, PartialEq)]
pub struct Detected {
    pub spec: ServerSpec,
    pub program: PathBuf,
    pub args: Vec<String>,
}

fn candidate(cmd: &str, args: &[&str]) -> (String, Vec<String>) {
    (cmd.into(), args.iter().map(|a| (*a).into()).collect())
}

fn exts(list: &[&str]) -> Vec<String> {
    list.iter().map(|e| (*e).into()).collect()
}

/// The built-in languages, in the order `declass doctor` lists them.
pub const BUILTIN: &[&str] = &["rust", "typescript", "python", "go", "c"];

/// The built-in entry for `language`.
pub fn builtin(language: &str) -> Option<ServerSpec> {
    let spec = |candidates, extensions, init_options: Value, settings: Value| ServerSpec {
        language: language.into(),
        candidates,
        extensions,
        env: Vec::new(),
        init_options,
        settings,
        configured: false,
    };
    Some(match language {
        "rust" => {
            // A target directory of its own, so the server's checks never
            // wait on (or invalidate) the build lock the run's checks use.
            let ra = json!({"cargo": {"targetDir": true}});
            spec(
                vec![candidate("rust-analyzer", &[])],
                exts(&["rs"]),
                ra.clone(),
                json!({ "rust-analyzer": ra }),
            )
        }
        "typescript" => spec(
            vec![candidate("typescript-language-server", &["--stdio"])],
            exts(&["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"]),
            Value::Null,
            Value::Null,
        ),
        "python" => spec(
            vec![
                candidate("pyright-langserver", &["--stdio"]),
                candidate("basedpyright-langserver", &["--stdio"]),
            ],
            exts(&["py", "pyi"]),
            Value::Null,
            Value::Null,
        ),
        "go" => spec(
            vec![candidate("gopls", &[])],
            exts(&["go"]),
            Value::Null,
            Value::Null,
        ),
        "c" => spec(
            vec![candidate("clangd", &[])],
            exts(&["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx", "m", "mm"]),
            Value::Null,
            Value::Null,
        ),
        _ => return None,
    })
}

/// Every configured language: the built-ins, with the owner's entries
/// replacing a built-in of the same name or adding a language.
pub fn table(settings: &Settings) -> Vec<ServerSpec> {
    let mut out: Vec<ServerSpec> = BUILTIN.iter().filter_map(|l| builtin(l)).collect();
    for (lang, cfg) in &settings.servers {
        let base = builtin(lang);
        let spec = ServerSpec {
            language: lang.clone(),
            candidates: vec![(cfg.command.clone(), cfg.args.clone())],
            extensions: if cfg.extensions.is_empty() {
                base.as_ref()
                    .map(|b| b.extensions.clone())
                    .unwrap_or_default()
            } else {
                cfg.extensions
                    .iter()
                    .map(|e| e.trim_start_matches('.').to_owned())
                    .collect()
            },
            env: cfg.env.clone(),
            init_options: base
                .as_ref()
                .map_or(Value::Null, |b| b.init_options.clone()),
            settings: base.as_ref().map_or(Value::Null, |b| b.settings.clone()),
            configured: true,
        };
        match out.iter_mut().find(|s| s.language == *lang) {
            Some(slot) => *slot = spec,
            None => out.push(spec),
        }
    }
    out
}

/// Where `command` resolves: an absolute path that is an executable file, or
/// the first executable of that name on `path` (a `PATH` value).
pub fn resolve(command: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let executable = |p: &Path| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if command.contains('/') {
        let p = PathBuf::from(command);
        return (p.is_absolute() && executable(&p)).then_some(p);
    }
    std::env::split_paths(path?)
        .map(|dir| dir.join(command))
        .find(|p| executable(p))
}

/// The configured languages whose server is installed, with the program found.
pub fn detect(settings: &Settings, path: Option<&std::ffi::OsStr>) -> Vec<Detected> {
    table(settings)
        .into_iter()
        .filter_map(|spec| {
            let (program, args) = spec
                .candidates
                .iter()
                .find_map(|(cmd, args)| resolve(cmd, path).map(|p| (p, args.clone())))?;
            Some(Detected {
                spec,
                program,
                args,
            })
        })
        .collect()
}

/// The `languageId` of a document, from its extension.
pub fn language_id(language: &str, ext: &str) -> String {
    match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        _ => language,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_entries_override_or_add_languages() {
        let entry = |command: &str, extensions: &[&str], env: &[&str]| ServerConfig {
            command: command.into(),
            args: Vec::new(),
            extensions: extensions.iter().map(|e| (*e).into()).collect(),
            env: env.iter().map(|e| (*e).into()).collect(),
        };
        let servers: BTreeMap<String, ServerConfig> = [
            (
                "rust".to_owned(),
                entry("/opt/ra/bin/rust-analyzer", &[], &[]),
            ),
            ("zig".to_owned(), entry("zls", &[".zig"], &["ZIG_LIB_DIR"])),
        ]
        .into();
        for (lang, cfg) in &servers {
            cfg.check(lang).unwrap();
        }
        let settings = Settings {
            enabled: true,
            servers,
            ..Settings::default()
        };
        let t = table(&settings);
        let rust = t.iter().find(|s| s.language == "rust").unwrap();
        assert_eq!(
            rust.candidates,
            vec![("/opt/ra/bin/rust-analyzer".into(), vec![])]
        );
        assert_eq!(rust.extensions, vec!["rs"]);
        assert!(rust.configured);
        let zig = t.iter().find(|s| s.language == "zig").unwrap();
        assert_eq!(zig.extensions, vec!["zig"]);
        assert_eq!(zig.env, vec!["ZIG_LIB_DIR"]);
        for (lang, bad) in [
            ("zig", entry("zls", &[], &[])),
            ("rust", entry("", &[], &[])),
            ("rust", entry("bin/ra", &[], &[])),
            ("rust", entry("ra", &[], &["A-B"])),
        ] {
            assert!(bad.check(lang).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn programs_are_found_on_the_given_path_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("gopls");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(d.path().join("clangd"), "not executable").unwrap();
        let path = d.path().as_os_str();
        let found = detect(&Settings::default(), Some(path));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].spec.language, "go");
        assert_eq!(found[0].program, bin);
        assert!(resolve("relative/gopls", Some(path)).is_none());
        assert_eq!(resolve(bin.to_str().unwrap(), None), Some(bin));
    }
}
