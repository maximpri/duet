// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet-eval preflight`: checks a lane's prerequisites without calling a model.
//!
//! Only local facts are checked: the program is on PATH (and what `--version`
//! says), required environment variables are present (their values are never
//! read into output), login files exist and are not copies of the operator's
//! main login, the lane routes its model traffic through the leak proxy, and
//! the proxy's upstream host resolves in DNS. Nothing connects to a provider.

use super::{Lane, LaneKind};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Per-lane prerequisites (`[lane.preflight]` in lanes.toml). Paths may use
/// `{operator_home}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    /// Environment variables that must be set and non-empty.
    #[serde(default)]
    pub env: Vec<String>,
    /// Files or directories that must exist.
    #[serde(default)]
    pub paths: Vec<String>,
    /// `[a, b]`: `a` must not be `b` or a byte-for-byte copy of it.
    #[serde(default)]
    pub not_copy_of: Vec<[String; 2]>,
    /// What the operator does once to satisfy the lane.
    #[serde(default)]
    pub setup: Vec<String>,
    /// Caveats printed on every preflight (e.g. routing still to be smoke-tested).
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub status: Status,
    pub what: &'static str,
    pub detail: String,
}

/// What the checks may look at; tests substitute the environment and skip DNS.
pub struct Context<'a> {
    pub operator_home: &'a Path,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Resolve the upstream host (a DNS query; no connection is made).
    pub resolve: bool,
}

fn expand_home(template: &str, home: &Path) -> PathBuf {
    PathBuf::from(template.replace("{operator_home}", &home.to_string_lossy()))
}

/// Whether any part of the lane points the agent at the leak proxy.
pub fn routes_through_proxy(lane: &Lane) -> bool {
    let p = "{proxy_url}";
    lane.argv.iter().any(|a| a.contains(p))
        || lane.env.values().any(|v| v.contains(p))
        || lane.files.iter().any(|f| f.content.contains(p))
}

/// `host:port` of an `http(s)://` URL.
pub fn upstream_host(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default = match scheme {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    if authority.is_empty() {
        return None;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => default,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, p)) => Some((host.to_owned(), p.parse().ok()?)),
        None => Some((authority.to_owned(), default)),
    }
}

fn sha256_file(path: &Path) -> Option<[u8; 32]> {
    Some(Sha256::digest(fs::read(path).ok()?).into())
}

pub async fn check(lane: &Lane, cx: &Context<'_>) -> Vec<Check> {
    let mut out = Vec::new();
    let mut push = |status, what, detail: String| {
        out.push(Check {
            status,
            what,
            detail,
        })
    };

    match super::lane_program(lane) {
        Err(e) => push(Status::Fail, "program", format!("{e:#}")),
        Ok(program) => match super::which(&program) {
            None => push(Status::Fail, "program", format!("{program} is not on PATH")),
            Some(path) => match super::program_version(&path.to_string_lossy()).await {
                Some(v) => push(Status::Ok, "program", format!("{} ({v})", path.display())),
                None => push(
                    Status::Warn,
                    "program",
                    format!("{} gives no --version output", path.display()),
                ),
            },
        },
    }

    for key in &lane.preflight.env {
        match (cx.env)(key).filter(|v| !v.trim().is_empty()) {
            Some(_) => push(Status::Ok, "env", format!("{key} is set")),
            None => push(Status::Fail, "env", format!("{key} is not set")),
        }
    }
    for key in lane
        .env_passthrough
        .iter()
        .filter(|k| !lane.preflight.env.contains(k))
    {
        if (cx.env)(key).is_none() {
            push(
                Status::Warn,
                "env",
                format!("{key} is not set (passed through when present)"),
            );
        }
    }

    for p in &lane.preflight.paths {
        let path = expand_home(p, cx.operator_home);
        if path.exists() {
            push(Status::Ok, "path", format!("{} exists", path.display()));
        } else {
            push(
                Status::Fail,
                "path",
                format!("{} is missing", path.display()),
            );
        }
    }
    for dir in &lane.writable {
        let path = expand_home(dir, cx.operator_home);
        if !path.is_dir() {
            push(
                Status::Fail,
                "path",
                format!("writable directory {} is missing", path.display()),
            );
        }
    }

    for [a, b] in &lane.preflight.not_copy_of {
        let (a, b) = (
            expand_home(a, cx.operator_home),
            expand_home(b, cx.operator_home),
        );
        let same_file = matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(x), Ok(y)) if x == y
        );
        let same_bytes = matches!(
            (sha256_file(&a), sha256_file(&b)),
            (Some(x), Some(y)) if x == y
        );
        if same_file || same_bytes {
            push(
                Status::Fail,
                "login",
                format!(
                    "{} is {} {}",
                    a.display(),
                    if same_file {
                        "the same file as"
                    } else {
                        "a copy of"
                    },
                    b.display()
                ),
            );
        } else if a.exists() {
            push(
                Status::Ok,
                "login",
                format!("{} is its own login (not {})", a.display(), b.display()),
            );
        }
    }

    if lane.kind == LaneKind::External && !lane.sandbox {
        push(
            Status::Fail,
            "sandbox",
            "external lanes must run in the harness sandbox".into(),
        );
    }
    if routes_through_proxy(lane) {
        push(
            Status::Ok,
            "routing",
            format!(
                "model traffic goes to the leak proxy, upstream {}",
                lane.upstream
            ),
        );
    } else {
        push(
            Status::Warn,
            "routing",
            "no {proxy_url} in the lane: it sends no frontier traffic through the proxy".into(),
        );
    }

    match upstream_host(&lane.upstream) {
        None => push(
            Status::Fail,
            "upstream",
            format!("cannot parse {}", lane.upstream),
        ),
        Some((host, port)) if cx.resolve => {
            let lookup = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((host.as_str(), port)),
            )
            .await
            .map(|r| r.map(Iterator::count));
            match lookup {
                Ok(Ok(n)) if n > 0 => push(
                    Status::Ok,
                    "upstream",
                    format!("{host} resolves (DNS only)"),
                ),
                Ok(Ok(_)) | Ok(Err(_)) => {
                    push(Status::Fail, "upstream", format!("{host} does not resolve"))
                }
                Err(_) => push(
                    Status::Fail,
                    "upstream",
                    format!("{host}: DNS lookup timed out"),
                ),
            }
        }
        Some((host, _)) => push(Status::Ok, "upstream", format!("{host} (not resolved)")),
    }

    for note in &lane.preflight.notes {
        push(Status::Warn, "note", note.clone());
    }
    out
}

/// Human-readable result for one lane; the operator's to-do list when anything failed.
pub fn render(lane: &Lane, checks: &[Check]) -> String {
    let mut s = format!("{} ({:?})\n", lane.name, lane.kind).to_lowercase();
    for c in checks {
        let tag = match c.status {
            Status::Ok => "ok  ",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
        };
        let _ = writeln!(s, "  {tag}  {:<9} {}", c.what, c.detail);
    }
    if checks.iter().any(|c| c.status == Status::Fail) {
        if lane.preflight.setup.is_empty() {
            s.push_str("  to do: fix the failures above\n");
        } else {
            s.push_str("  to do:\n");
            for step in &lane.preflight.setup {
                let _ = writeln!(s, "    - {step}");
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lanes::{find_lane, load_lanes};

    fn cx<'a>(home: &'a Path, env: &'a dyn Fn(&str) -> Option<String>) -> Context<'a> {
        Context {
            operator_home: home,
            env,
            resolve: false,
        }
    }

    fn fails(checks: &[Check]) -> Vec<String> {
        checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .map(|c| format!("{}: {}", c.what, c.detail))
            .collect()
    }

    #[test]
    fn parses_upstream_hosts() {
        assert_eq!(
            upstream_host("https://api.anthropic.com"),
            Some(("api.anthropic.com".into(), 443))
        );
        assert_eq!(
            upstream_host("https://chatgpt.com/backend-api/codex"),
            Some(("chatgpt.com".into(), 443))
        );
        assert_eq!(
            upstream_host("http://192.168.50.132:8080/v1"),
            Some(("192.168.50.132".into(), 8080))
        );
        assert_eq!(upstream_host("[::1]:80"), None);
        assert_eq!(
            upstream_host("http://[::1]:8080/x"),
            Some(("::1".into(), 8080))
        );
        assert_eq!(upstream_host("https://[::1]"), Some(("::1".into(), 443)));
    }

    #[test]
    fn subscription_lanes_use_no_api_key_and_route_through_the_proxy() {
        let lanes = load_lanes(None).unwrap();
        let claude = find_lane(&lanes, "claude-code").unwrap();
        assert_eq!(claude.env_passthrough, ["CLAUDE_CODE_OAUTH_TOKEN"]);
        assert_eq!(claude.preflight.env, ["CLAUDE_CODE_OAUTH_TOKEN"]);
        assert_eq!(claude.env["ANTHROPIC_BASE_URL"], "{proxy_url}");
        assert!(claude.env["CLAUDE_CONFIG_DIR"].starts_with("{run_dir}/"));
        let codex = find_lane(&lanes, "codex").unwrap();
        assert!(codex.env_passthrough.is_empty());
        assert_eq!(codex.env["CODEX_HOME"], "{operator_home}/.duet-eval/codex");
        assert_eq!(codex.writable, ["{operator_home}/.duet-eval/codex"]);
        assert!(
            codex
                .argv
                .windows(2)
                .any(|w| w == ["--disable", "enable_request_compression"]),
            "compressed requests cannot be scanned"
        );
        for lane in [claude, codex] {
            assert!(!lane.probe, "{}", lane.name);
            assert!(lane.sandbox, "{}", lane.name);
            assert!(routes_through_proxy(lane), "{}", lane.name);
            assert!(!lane.preflight.setup.is_empty(), "{}", lane.name);
            for key in lane.env_passthrough.iter().chain(lane.env.keys()) {
                assert!(
                    !key.ends_with("API_KEY"),
                    "{}: {key} is an API-key credential",
                    lane.name
                );
            }
        }
    }

    #[test]
    fn every_frontier_lane_routes_through_the_proxy() {
        for lane in load_lanes(None).unwrap() {
            let local_only = lane.argv.windows(2).any(|w| w == ["--mode", "local-only"]);
            assert_eq!(routes_through_proxy(&lane), !local_only, "{}", lane.name);
            if lane.kind == LaneKind::External {
                assert!(lane.sandbox, "{}", lane.name);
            }
        }
    }

    #[tokio::test]
    async fn missing_prerequisites_fail_without_revealing_values() {
        let home = tempfile::tempdir().unwrap();
        let lanes = load_lanes(None).unwrap();
        let mut claude = find_lane(&lanes, "claude-code").unwrap().clone();
        claude.argv[0] = "/nonexistent/agent".into();
        let none = |_: &str| None;
        let checks = check(&claude, &cx(home.path(), &none)).await;
        let f = fails(&checks);
        assert!(
            f.iter()
                .any(|x| x.contains("/nonexistent/agent is not on PATH")),
            "{f:?}"
        );
        assert!(
            f.iter()
                .any(|x| x.contains("CLAUDE_CODE_OAUTH_TOKEN is not set")),
            "{f:?}"
        );
        let text = render(&claude, &checks);
        assert!(
            text.contains("to do:") && text.contains("claude setup-token"),
            "{text}"
        );

        let secret = "sk-ant-oat01-SHOULD-NEVER-PRINT";
        let set = move |k: &str| (k == "CLAUDE_CODE_OAUTH_TOKEN").then(|| secret.to_owned());
        let checks = check(&claude, &cx(home.path(), &set)).await;
        assert!(
            !fails(&checks).iter().any(|x| x.contains("CLAUDE_CODE")),
            "{checks:?}"
        );
        assert!(!render(&claude, &checks).contains(secret));
    }

    #[tokio::test]
    async fn a_copied_main_login_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let lanes = load_lanes(None).unwrap();
        let mut codex = find_lane(&lanes, "codex").unwrap().clone();
        codex.argv[0] = "/nonexistent/agent".into();
        let codex = &codex;
        let none = |_: &str| None;
        let f = fails(&check(codex, &cx(home.path(), &none)).await);
        assert!(
            f.iter()
                .any(|x| x.contains(".duet-eval/codex/auth.json is missing")),
            "{f:?}"
        );
        assert!(f.iter().any(|x| x.contains("writable directory")), "{f:?}");

        let eval = home.path().join(".duet-eval/codex");
        let main = home.path().join(".codex");
        fs::create_dir_all(&eval).unwrap();
        fs::create_dir_all(&main).unwrap();
        fs::write(main.join("auth.json"), "{\"tokens\":\"main\"}").unwrap();
        fs::write(eval.join("auth.json"), "{\"tokens\":\"main\"}").unwrap();
        let f = fails(&check(codex, &cx(home.path(), &none)).await);
        assert!(f.iter().any(|x| x.contains("a copy of")), "{f:?}");

        fs::write(eval.join("auth.json"), "{\"tokens\":\"eval\"}").unwrap();
        let checks = check(codex, &cx(home.path(), &none)).await;
        assert!(
            !fails(&checks)
                .iter()
                .any(|x| x.starts_with("login") || x.starts_with("path")),
            "{checks:?}"
        );
        assert!(
            checks
                .iter()
                .any(|c| c.what == "login" && c.status == Status::Ok)
        );
    }
}
