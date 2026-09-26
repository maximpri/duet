// SPDX-License-Identifier: GPL-3.0-or-later
//! Operator approval of risky actions (`oversight.approve`).
//!
//! Before a tool runs, [`review`] decides whether the operator must approve it.
//! With `risky`, an action needs approval when it:
//! - runs a command with `sensitive_data` (it reads sensitive files);
//! - changes protected source (`edit_protected`);
//! - writes a file (`edit_file`, `write_file`) that is not an ordinary source
//!   or test file (see [`is_source_or_test`]).
//!
//! MCP tools (see `crate::mcp`) follow their server's `approve` setting under
//! `risky`: with `writes` (the default) a tool the server does not declare
//! read-only needs approval, with `always` every tool, with `auto` none.
//!
//! With `all`, every command, every write and every MCP call needs approval as well. Reads,
//! `ask_local` and `finish` (whose checks the owner configured) never do.
//! A denied action becomes a tool error the model can adapt to. Every decision
//! is recorded as an audit event holding the tool, the risk class and a write's
//! path, never content. Without an approver (no terminal) nothing is approved.

use crate::tools::string_arg;
use duet_boundary::audit::{AuditEvent, AuditHandle};
use duet_boundary::view::Presenter;
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApproveMode {
    /// Nothing is asked (the default; evaluation lanes keep it).
    #[default]
    Off,
    Risky,
    All,
}

impl std::str::FromStr for ApproveMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "off" => Ok(Self::Off),
            "risky" => Ok(Self::Risky),
            "all" => Ok(Self::All),
            other => Err(format!(
                "oversight.approve must be off, risky or all, not {other}"
            )),
        }
    }
}

impl ApproveMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Risky => "risky",
            Self::All => "all",
        }
    }
}

/// Why an action needs approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// A command run with `sensitive_data`.
    SensitiveData,
    /// A change to protected source by the local model.
    EditProtected,
    /// A write to a file that is not an ordinary source or test file.
    WriteOutsideSources,
    /// Any other command (`all` only).
    Command,
    /// Any other write (`all` only).
    Write,
    /// A commit to the repository's history (`git.commit = "ask"`, or `all`).
    GitCommit,
    /// An MCP tool the server does not declare read-only.
    McpWrite,
    /// An MCP tool declared read-only (servers with `approve = "always"`, or `all`).
    McpCall,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::SensitiveData => "sensitive_data",
            Risk::EditProtected => "edit_protected",
            Risk::WriteOutsideSources => "write_outside_sources",
            Risk::Command => "command",
            Risk::Write => "write",
            Risk::GitCommit => "git_commit",
            Risk::McpWrite => "mcp_write",
            Risk::McpCall => "mcp_call",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Risk::SensitiveData => {
                "runs a command that can read sensitive files (its output stays local)"
            }
            Risk::EditProtected => "has the local model change protected source",
            Risk::WriteOutsideSources => {
                "writes a file that is not an ordinary source or test file"
            }
            Risk::Command => "runs a command",
            Risk::Write => "writes a file",
            Risk::GitCommit => "records a commit in the repository's history",
            Risk::McpWrite => "calls an MCP tool that may change things (not declared read-only)",
            Risk::McpCall => "calls an MCP tool",
        }
    }
}

/// An action awaiting the operator's decision. `command` and `bytes` are for
/// the operator's screen only; they are never recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub tool: String,
    pub risk: Risk,
    pub path: Option<String>,
    pub command: Option<String>,
    /// Size of the content a write would store.
    pub bytes: Option<usize>,
}

/// Asks the operator. Returns whether the action is approved.
pub trait Approver: Send + Sync {
    fn approve(&self, action: &Action) -> bool;
}

/// The run's approval setting and who is asked.
#[derive(Clone, Default)]
pub struct Oversight {
    pub mode: ApproveMode,
    /// `None` with a mode other than `off` denies every action that needs approval.
    pub approver: Option<Arc<dyn Approver>>,
    /// `git.commit`: whether each `git_commit` needs approval (`ask`; `all`
    /// asks for every commit whatever this says).
    pub git_commit: crate::git_tools::CommitPolicy,
}

/// Extensions of files that are ordinary program source or tests.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "pyi", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "go", "java", "kt",
    "scala", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "cs", "rb", "swift", "m", "mm", "php",
    "lua", "ex", "exs", "erl", "hs", "ml", "mli", "zig", "dart", "vue", "svelte",
];

/// Source files that run when the project is built or its tests are collected,
/// before any test code: build scripts, packaging and test-runner hooks.
const BUILD_HOOKS: &[&str] = &[
    "build.rs",
    "setup.py",
    "conftest.py",
    "noxfile.py",
    "fabfile.py",
    "manage.py",
    "gulpfile.js",
    "gruntfile.js",
];

/// Directories whose files are tooling, dependencies or build output, not the
/// project's sources.
const NON_SOURCE_DIRS: &[&str] = &[
    "scripts",
    "tools",
    "bin",
    "ci",
    "hooks",
    "vendor",
    "third_party",
    "node_modules",
    "target",
    "dist",
    "build",
];

/// Whether `path` (workspace-relative) is an ordinary source or test file: a
/// source extension, no hidden component (`.github/`, `.cargo/`, `.env`), not
/// under a tooling, dependency or build-output directory, not a build hook or
/// a `*.config.*` file, and not sensitive. Everything else (manifests, lock
/// files, CI, shell scripts, Makefiles, docs, data, configuration) is not.
pub fn is_source_or_test(path: &Path, sensitive: bool) -> bool {
    if sensitive {
        return false;
    }
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let Some((name, dirs)) = parts.split_last() else {
        return false;
    };
    if parts.iter().any(|p| p.starts_with('.') || p.is_empty())
        || dirs.iter().any(|d| NON_SOURCE_DIRS.contains(&d.as_str()))
        || BUILD_HOOKS.contains(&name.as_str())
    {
        return false;
    }
    let mut pieces = name.split('.');
    let _stem = pieces.next();
    let rest: Vec<&str> = pieces.collect();
    match rest.as_slice() {
        [ext] => SOURCE_EXTENSIONS.contains(ext),
        // `vite.config.ts`, `jest.config.js`: configuration that runs at build time.
        [.., "config", _] => false,
        [.., ext] => SOURCE_EXTENSIONS.contains(ext),
        [] => false,
    }
}

/// The action a tool call would take, if it needs approval under `mode`.
pub fn classify(
    mode: ApproveMode,
    tool: &str,
    args: &Map<String, Value>,
    presenter: &dyn Presenter,
) -> Option<Action> {
    if mode == ApproveMode::Off {
        return None;
    }
    let all = mode == ApproveMode::All;
    let action = |risk, path: Option<String>, command: Option<String>, bytes| Action {
        tool: tool.to_owned(),
        risk,
        path,
        command,
        bytes,
    };
    match tool {
        "run_command" => {
            let command = string_arg(args, "command").ok()?.to_owned();
            let sensitive = args
                .get("sensitive_data")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match (sensitive, all) {
                (true, _) => Some(action(Risk::SensitiveData, None, Some(command), None)),
                (false, true) => Some(action(Risk::Command, None, Some(command), None)),
                (false, false) => None,
            }
        }
        "edit_protected" => {
            let path = string_arg(args, "path").ok()?;
            let command = args.get("command").and_then(Value::as_str).map(Into::into);
            Some(action(
                Risk::EditProtected,
                Some(path.to_owned()),
                command,
                None,
            ))
        }
        // Only source and test files, checked by the tool itself; `all`
        // asks before any of them change.
        "rename" if all => {
            let rel = duet_fs::writable_relative(string_arg(args, "path").ok()?).ok()?;
            Some(action(
                Risk::Write,
                Some(rel.display().to_string()),
                None,
                None,
            ))
        }
        "edit_file" | "write_file" => {
            // A path the write path refuses is never written: nothing to approve.
            let rel = duet_fs::writable_relative(string_arg(args, "path").ok()?).ok()?;
            let bytes = match tool {
                "write_file" => args.get("content").and_then(Value::as_str).map(str::len),
                _ => None,
            };
            let shown = Some(rel.display().to_string());
            if !is_source_or_test(&rel, presenter.path_sensitive(&rel)) {
                Some(action(Risk::WriteOutsideSources, shown, None, bytes))
            } else if all {
                Some(action(Risk::Write, shown, None, bytes))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A `git_commit` that needs approval: always under `all`, and under any
/// mode when `git.commit = "ask"` (without an approver it is then denied; the
/// tool is not offered in that case). With `off` and an approver (an
/// interactive session), commits are the only actions asked about. The
/// operator sees the message and paths.
fn commit_action(oversight: &Oversight, tool: &str, args: &Map<String, Value>) -> Option<Action> {
    use crate::git_tools::CommitPolicy;
    if tool != "git_commit"
        || (oversight.mode != ApproveMode::All && oversight.git_commit != CommitPolicy::Ask)
    {
        return None;
    }
    let paths: Vec<&str> = args
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Some(Action {
        tool: tool.to_owned(),
        risk: Risk::GitCommit,
        path: Some(if paths.is_empty() {
            "(every file this run wrote that may be committed)".to_owned()
        } else {
            paths.join(", ")
        }),
        command: args.get("message").and_then(Value::as_str).map(Into::into),
        bytes: None,
    })
}

/// Asks for approval when `tool` needs it. `Err` holds the tool error for a
/// denied action; every decision is recorded in `audit`.
pub fn review(
    oversight: &Oversight,
    tool: &str,
    args: &Map<String, Value>,
    presenter: &dyn Presenter,
    audit: Option<&AuditHandle>,
) -> Result<(), String> {
    decide(
        oversight,
        classify(oversight.mode, tool, args, presenter)
            .or_else(|| commit_action(oversight, tool, args)),
        audit,
    )
}

/// Asks for approval of `action`, if there is one (see [`review`]).
pub fn decide(
    oversight: &Oversight,
    action: Option<Action>,
    audit: Option<&AuditHandle>,
) -> Result<(), String> {
    let Some(action) = action else {
        return Ok(());
    };
    let (approved, decided_by) = match &oversight.approver {
        Some(a) => (a.approve(&action), "operator"),
        None => (false, "no_terminal"),
    };
    if let Some(a) = audit {
        a.record(AuditEvent::Approval {
            tool: action.tool.clone(),
            risk: action.risk.as_str().to_owned(),
            path: action.path.clone(),
            approved,
            decided_by: decided_by.to_owned(),
        });
    }
    if approved {
        return Ok(());
    }
    Err(format!(
        "the operator did not approve this action (it {}). It was not run. Do not repeat it unchanged: \
choose another approach, or finish and explain what is left for the operator.",
        action.risk.describe()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use duet_boundary::audit::{AuditLog, Line, read};
    use duet_boundary::view::PassThrough;
    use serde_json::json;
    use std::sync::Mutex;

    fn args(v: Value) -> Map<String, Value> {
        let Value::Object(m) = v else { unreachable!() };
        m
    }

    #[test]
    fn source_and_test_files_are_defined_conservatively() {
        for ok in [
            "src/lib.rs",
            "crates/a/src/x.rs",
            "tests/it.rs",
            "app/models/user.py",
            "web/src/App.tsx",
            "pkg/server/handler_test.go",
            "src/util.test.ts",
        ] {
            assert!(is_source_or_test(Path::new(ok), false), "{ok}");
        }
        for risky in [
            "Cargo.toml",
            "Cargo.lock",
            "package.json",
            "README.md",
            "Makefile",
            "Dockerfile",
            "build.rs",
            "crates/a/build.rs",
            "setup.py",
            "tests/conftest.py",
            "vite.config.ts",
            "jest.config.js",
            ".github/workflows/ci.yml",
            ".cargo/config.toml",
            ".env",
            "src/.hidden.rs",
            "scripts/deploy.py",
            "tools/gen.rs",
            "vendor/lib/a.c",
            "node_modules/x/index.js",
            "run.sh",
            "data/orders.csv",
            "config/app.toml",
            "migrations/001.sql",
            "src/lib",
        ] {
            assert!(!is_source_or_test(Path::new(risky), false), "{risky}");
        }
        // A sensitive path is never an ordinary source file.
        assert!(!is_source_or_test(Path::new("src/lib.rs"), true));
    }

    #[test]
    fn classification_by_mode() {
        let p = PassThrough { max_bytes: 100 };
        let run = |c: &str, s: bool| args(json!({"command": c, "sensitive_data": s}));
        let write = |path: &str| args(json!({"path": path, "content": "abc"}));
        let risk = |mode, tool, a: &Map<String, Value>| classify(mode, tool, a, &p).map(|a| a.risk);
        use ApproveMode::*;
        for mode in [Off, Risky, All] {
            assert_eq!(risk(mode, "read_file", &write("src/a.rs")), None);
            assert_eq!(risk(mode, "finish", &args(json!({}))), None);
        }
        assert_eq!(risk(Off, "run_command", &run("x", true)), None);
        assert_eq!(risk(Off, "write_file", &write("Cargo.toml")), None);
        assert_eq!(
            risk(Risky, "run_command", &run("x", true)),
            Some(Risk::SensitiveData)
        );
        assert_eq!(risk(Risky, "run_command", &run("cargo test", false)), None);
        assert_eq!(
            risk(All, "run_command", &run("cargo test", false)),
            Some(Risk::Command)
        );
        assert_eq!(
            risk(Risky, "write_file", &write("Cargo.toml")),
            Some(Risk::WriteOutsideSources)
        );
        assert_eq!(risk(Risky, "edit_file", &write("src/a.rs")), None);
        assert_eq!(
            risk(All, "edit_file", &write("src/a.rs")),
            Some(Risk::Write)
        );
        // The write path refuses these anyway.
        assert_eq!(risk(Risky, "write_file", &write("../outside.rs")), None);
        assert_eq!(risk(Risky, "write_file", &write(".git/config")), None);
        assert_eq!(
            risk(
                Risky,
                "edit_protected",
                &args(json!({"path": "src/core.rs", "spec": "x"}))
            ),
            Some(Risk::EditProtected)
        );
        // A rename changes only source and test files (the tool refuses the
        // rest), so only `all` asks.
        let rename = args(json!({"path": "src/a.rs", "line": 1, "column": 4, "new_name": "b"}));
        assert_eq!(risk(Risky, "rename", &rename), None);
        assert_eq!(risk(All, "rename", &rename), Some(Risk::Write));
        assert_eq!(risk(All, "code_nav", &args(json!({"op": "hover"}))), None);
        let a = classify(Risky, "write_file", &write("Cargo.toml"), &p).unwrap();
        assert_eq!((a.path.as_deref(), a.bytes), (Some("Cargo.toml"), Some(3)));
    }

    #[test]
    fn commits_ask_by_git_commit_and_under_all() {
        use crate::git_tools::{CommitPolicy, GitTools};
        let p = PassThrough { max_bytes: 100 };
        let commit = args(json!({"message": "Fix totals", "paths": ["src/a.rs", "src/b.rs"]}));
        let yes: Arc<dyn Approver> = Arc::new(Scripted {
            answers: Mutex::new(vec![true; 8]),
            seen: Mutex::default(),
        });
        let with = |mode, git_commit, approver: Option<Arc<dyn Approver>>| Oversight {
            mode,
            approver,
            git_commit,
        };
        let asks = |o: &Oversight| commit_action(o, "git_commit", &commit).map(|a| a.risk);
        use ApproveMode::*;
        use CommitPolicy::{Allow, Ask};
        assert_eq!(asks(&with(Risky, Ask, None)), Some(Risk::GitCommit));
        assert_eq!(asks(&with(Off, Ask, None)), Some(Risk::GitCommit));
        assert_eq!(asks(&with(Risky, Allow, None)), None);
        assert_eq!(asks(&with(All, Allow, None)), Some(Risk::GitCommit));
        assert_eq!(
            commit_action(&with(All, Ask, None), "run_command", &commit),
            None
        );
        let a = commit_action(&with(Risky, Ask, None), "git_commit", &commit).unwrap();
        assert_eq!(
            (a.path.as_deref(), a.command.as_deref()),
            (Some("src/a.rs, src/b.rs"), Some("Fix totals"))
        );
        // Without an approver an `ask` commit is denied (and not offered).
        assert!(review(&with(Off, Ask, None), "git_commit", &commit, &p, None).is_err());
        assert!(
            review(
                &with(Risky, Ask, Some(yes.clone())),
                "git_commit",
                &commit,
                &p,
                None
            )
            .is_ok()
        );
        assert!(review(&with(Off, Allow, None), "git_commit", &commit, &p, None).is_ok());
        // With approval off and an approver, a commit is asked and nothing else is.
        let seen = Arc::new(Scripted {
            answers: Mutex::new(vec![false]),
            seen: Mutex::default(),
        });
        let session = with(Off, Ask, Some(seen.clone()));
        let write = args(json!({"path": "Cargo.toml", "content": "x"}));
        let run = args(json!({"command": "cat data.csv", "sensitive_data": true}));
        assert!(review(&session, "write_file", &write, &p, None).is_ok());
        assert!(review(&session, "run_command", &run, &p, None).is_ok());
        assert!(review(&session, "git_commit", &commit, &p, None).is_err());
        let asked = seen.seen.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].risk, Risk::GitCommit);
        assert_eq!(asked[0].command.as_deref(), Some("Fix totals"));

        // Whether git_commit is offered at all.
        let d = tempfile::tempdir().unwrap();
        let git = duet_git::Git::locate().unwrap();
        let offered = |o: &Oversight| GitTools::decide(&git, d.path(), o, None).map(|g| g.commit);
        assert_eq!(
            offered(&with(Risky, Allow, Some(yes.clone()))),
            None,
            "not a repository"
        );
        git.run(d.path(), &["init", "-q"], &[], None).unwrap();
        assert_eq!(offered(&with(Off, Ask, None)), Some(false));
        assert_eq!(offered(&with(Risky, Ask, Some(yes.clone()))), Some(true));
        // An interactive session: approval off, but someone to ask.
        assert_eq!(offered(&with(Off, Ask, Some(yes.clone()))), Some(true));
        assert_eq!(offered(&with(Off, Allow, None)), Some(true));
        assert_eq!(
            offered(&with(All, CommitPolicy::Off, Some(yes))),
            Some(false)
        );
        let specs = GitTools::decide(&git, d.path(), &with(Off, Allow, None), None)
            .unwrap()
            .specs();
        let names: Vec<String> = crate::tools::specs_with(specs)
            .into_iter()
            .map(|s| s.name)
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        for n in crate::git_tools::NAMES {
            assert!(names.iter().any(|x| x == n), "{n}");
        }
    }

    /// Answers from a script and remembers what it was shown.
    struct Scripted {
        answers: Mutex<Vec<bool>>,
        seen: Mutex<Vec<Action>>,
    }

    impl Approver for Scripted {
        fn approve(&self, action: &Action) -> bool {
            self.seen.lock().unwrap().push(action.clone());
            self.answers.lock().unwrap().remove(0)
        }
    }

    #[test]
    fn decisions_are_asked_enforced_and_audited_without_content() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("audit.jsonl");
        let audit = AuditHandle::new(AuditLog::open(&log).unwrap());
        let p = PassThrough { max_bytes: 100 };
        let approver = Arc::new(Scripted {
            answers: Mutex::new(vec![true, false]),
            seen: Mutex::default(),
        });
        let o = Oversight {
            mode: ApproveMode::Risky,
            approver: Some(approver.clone()),
            ..Oversight::default()
        };
        let secret_cmd = "python3 report.py --token s3cr3t-value";
        let a = args(json!({"command": secret_cmd, "sensitive_data": true}));
        assert_eq!(review(&o, "run_command", &a, &p, Some(&audit)), Ok(()));
        let w = args(json!({"path": "Cargo.toml", "content": "[package] secret-body"}));
        let denied = review(&o, "write_file", &w, &p, Some(&audit)).unwrap_err();
        assert!(denied.contains("not approve") && denied.contains("another approach"));
        // Not asked: an ordinary source write.
        let src = args(json!({"path": "src/a.rs", "content": "fn a() {}"}));
        assert_eq!(review(&o, "write_file", &src, &p, Some(&audit)), Ok(()));
        let seen = approver.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].command.as_deref(), Some(secret_cmd));

        // Without an approver nothing is approved.
        let closed = Oversight {
            mode: ApproveMode::All,
            approver: None,
            ..Oversight::default()
        };
        assert!(review(&closed, "write_file", &src, &p, Some(&audit)).is_err());

        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            !text.contains("s3cr3t") && !text.contains("secret-body"),
            "{text}"
        );
        let events: Vec<AuditEvent> = read(&log)
            .unwrap()
            .into_iter()
            .filter_map(|l| match l {
                Line::Event(e) => Some(e.event),
                Line::Request(_) => None,
            })
            .collect();
        let expect =
            |tool: &str, risk: &str, path: Option<&str>, approved, by: &str| AuditEvent::Approval {
                tool: tool.into(),
                risk: risk.into(),
                path: path.map(Into::into),
                approved,
                decided_by: by.into(),
            };
        assert_eq!(
            events,
            [
                expect("run_command", "sensitive_data", None, true, "operator"),
                expect(
                    "write_file",
                    "write_outside_sources",
                    Some("Cargo.toml"),
                    false,
                    "operator"
                ),
                expect(
                    "write_file",
                    "write",
                    Some("src/a.rs"),
                    false,
                    "no_terminal"
                ),
            ]
        );
    }
}
