// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet doctor`, `duet config preset` and the no-config bootstrap through the
//! binary, against a scripted loopback server. No test calls a real model: the
//! scripted servers answer model listings and, for the online cache check, a
//! canned stream; the one run that starts stops at the missing frontier key
//! before any request.

use duet_boundary::audit::{AuditEvent, AuditLog, Verification, run_anchors, verify};
use duet_provider::mock_http::MockServer;
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    ws: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (home, ws) = (root.join("owner"), root.join("ws"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(git.success());
    Env {
        _dir: dir,
        home,
        ws,
    }
}

fn duet_with(e: &Env, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_duet"));
    c.args(args)
        .arg("--workspace")
        .arg(&e.ws)
        .env("DUET_CONFIG_HOME", &e.home)
        .env_remove("ZAI_API_KEY")
        .env_remove("DUET_LOCAL_PORTS");
    for (k, v) in vars {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn duet(e: &Env, args: &[&str]) -> Output {
    duet_with(e, args, &[])
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn owner_config(e: &Env, toml: &str) {
    std::fs::write(e.home.join("config.toml"), toml).unwrap();
}

/// `duet doctor --json` and the status of each named check.
fn doctor(e: &Env, args: &[&str], vars: &[(&str, &str)]) -> (i32, Value) {
    let mut all = vec!["doctor", "--json"];
    all.extend_from_slice(args);
    let o = duet_with(e, &all, vars);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| panic!("{}", text(&o)));
    (o.status.code().unwrap(), v)
}

fn status<'a>(report: &'a Value, name: &str) -> &'a str {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no check {name}: {report}"))["status"]
        .as_str()
        .unwrap()
}

fn configured(e: &Env, server: &MockServer, model: &str) {
    owner_config(
        e,
        &format!(
            "[frontier]\nbase_url = \"{u}\"\n[local]\nbase_url = \"{u}\"\nmodel = \"{model}\"\n",
            u = server.base_url()
        ),
    );
}

const LISTING: &str = r#"{"data":[{"id":"coder","max_model_len":65536},{"id":"glm-5.3-flash"}]}"#;

/// A Chat Completions stream whose usage reports `cached` cached prompt tokens.
fn chat_reply(cached: u64) -> String {
    let usage = serde_json::json!({"choices": [], "usage": {"prompt_tokens": 5200,
        "completion_tokens": 1, "prompt_tokens_details": {"cached_tokens": cached}}});
    format!(
        "data: {}\n\ndata: {usage}\n\ndata: [DONE]\n\n",
        serde_json::json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]})
    )
}

fn detail<'a>(report: &'a Value, name: &str) -> &'a str {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no check {name}: {report}"))["detail"]
        .as_str()
        .unwrap()
}

#[test]
fn offline_doctor_uses_no_network_and_never_prints_the_key() {
    let e = env();
    let server = MockServer::start(&[("GET /v1/models", 200, LISTING)]);
    configured(&e, &server, "coder");
    let (code, report) = doctor(&e, &[], &[]);
    assert_eq!(status(&report, "frontier key"), "fail", "{report}");
    assert_eq!(code, 2);
    assert_eq!(status(&report, "local endpoint"), "pass");
    assert_eq!(status(&report, "local server"), "skip");
    assert_eq!(status(&report, "frontier model"), "skip");
    assert_eq!(status(&report, "frontier cache"), "skip");
    assert_eq!(status(&report, "local cache"), "skip");
    assert_eq!(status(&report, "sandbox"), "pass");
    assert_eq!(status(&report, "git"), "pass");
    assert_eq!(status(&report, "audit"), "pass");

    let secret = "sk-doctor-test-4f9a1c";
    let (_, report) = doctor(&e, &[], &[("ZAI_API_KEY", secret)]);
    assert_eq!(status(&report, "frontier key"), "pass");
    let o = duet_with(&e, &["doctor"], &[("ZAI_API_KEY", secret)]);
    assert!(!text(&o).contains(secret) && !report.to_string().contains(secret));
    assert!(text(&o).contains("PASS  frontier key"), "{}", text(&o));
    assert!(
        server.seen().is_empty(),
        "offline doctor contacted {:?}",
        server.seen()
    );
    assert!(!e.ws.join(".duet").exists(), "doctor wrote workspace state");
}

#[test]
fn online_doctor_lists_models_context_and_cache_reuse() {
    let e = env();
    let reply = chat_reply(4800);
    let server = MockServer::start(&[
        ("GET /v1/models", 200, LISTING),
        ("POST /v1/chat/completions", 200, &reply),
    ]);
    configured(&e, &server, "coder");
    let (code, report) = doctor(&e, &["--online"], &[("ZAI_API_KEY", "k")]);
    for (name, want) in [
        ("frontier model", "pass"),
        ("frontier cache", "pass"),
        ("local server", "pass"),
        ("local context", "pass"),
        ("local cache", "pass"),
    ] {
        assert_eq!(status(&report, name), want, "{name}: {report}");
    }
    assert!(
        detail(&report, "frontier cache").contains("read 4800 of 5200 input tokens"),
        "{report}"
    );
    // Loopback plain HTTP to the frontier is allowed; nothing else warns.
    assert!(code <= 1, "{report}");
    let seen = server.seen();
    // Model calls: two identical prompts each to the frontier and the local model.
    let posts: Vec<_> = seen.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(posts.len(), 4, "{seen:?}");
    assert!(posts.iter().all(|r| r.path == "/v1/chat/completions"));
    assert!(seen.iter().any(|r| r.path == "/v1/models" && r.bearer));

    // No cached tokens on the repeat: a warning with the fix.
    let uncached = chat_reply(0);
    let cold = MockServer::start(&[
        ("GET /v1/models", 200, LISTING),
        ("POST /v1/chat/completions", 200, &uncached),
    ]);
    configured(&e, &cold, "coder");
    let (code, report) = doctor(&e, &["--online"], &[("ZAI_API_KEY", "k")]);
    assert_eq!(status(&report, "frontier cache"), "warn", "{report}");
    assert_eq!(status(&report, "local cache"), "warn", "{report}");
    assert_eq!(code, 1);
    configured(&e, &server, "coder");

    // A model the server does not list fails with the fix naming one it does.
    configured(&e, &server, "missing");
    let (code, report) = doctor(&e, &["--online"], &[("ZAI_API_KEY", "k")]);
    assert_eq!(code, 2);
    assert_eq!(status(&report, "local server"), "fail");
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "local server")
        .unwrap();
    assert!(check["fix"].as_str().unwrap().contains("coder"), "{check}");
}

#[test]
fn doctor_fails_a_refused_local_endpoint_and_does_not_contact_it() {
    let e = env();
    // TEST-NET-1 (never routed), allowlisted but plain HTTP without the opt-in.
    owner_config(
        &e,
        "[local]\nbase_url = \"http://192.0.2.1:9/v1\"\nallowlist = [\"192.0.2.1:9\"]\n",
    );
    let (code, report) = doctor(&e, &["--online"], &[]);
    assert_eq!(code, 2);
    assert_eq!(status(&report, "local endpoint"), "fail");
    assert_eq!(status(&report, "local server"), "skip");
    let o = duet(&e, &["doctor"]);
    assert!(
        text(&o).contains("local.allow_plaintext = true"),
        "{}",
        text(&o)
    );
}

#[test]
fn doctor_verifies_recent_audit_logs_and_anchors() {
    let e = env();
    let write = |id: &str, anchored: bool| {
        let log = e.ws.join(".duet/audit").join(format!("{id}.jsonl"));
        let mut a = if anchored {
            AuditLog::open_anchored(&log, &run_anchors(&e.home.join("state"), &e.ws, id)).unwrap()
        } else {
            AuditLog::open(&log).unwrap()
        };
        a.event(AuditEvent::RunStart {
            mode: "hybrid".into(),
            boundary: true,
        })
        .unwrap();
        a.append("https://f/v1", "m", serde_json::json!({"n": 1}), vec![])
            .unwrap();
        log
    };
    let log = write("r1", true);
    assert_eq!(status(&doctor(&e, &[], &[]).1, "audit"), "pass");
    write("r2", false);
    assert_eq!(status(&doctor(&e, &[], &[]).1, "audit"), "warn");
    // An edited record breaks the chain.
    let edited = std::fs::read_to_string(&log)
        .unwrap()
        .replace("\"n\":1", "\"n\":2");
    std::fs::write(&log, edited).unwrap();
    let (code, report) = doctor(&e, &[], &[]);
    assert_eq!(status(&report, "audit"), "fail", "{report}");
    assert_eq!(code, 2);
}

#[test]
fn online_doctor_speaks_the_frontier_dialect() {
    let e = env();
    let stream: String = [
        r#"{"type":"message_start","message":{"id":"m","model":"claude-opus-5-5","usage":{"input_tokens":20,"cache_read_input_tokens":5100,"cache_creation_input_tokens":0,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"ok"}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .iter()
    .map(|d| format!("data: {d}\n\n"))
    .collect();
    let server = MockServer::start(&[
        (
            "GET /v1/models",
            200,
            r#"{"data":[{"id":"claude-opus-5-5","type":"model"}],"has_more":false}"#,
        ),
        ("POST /v1/messages", 200, &stream),
    ]);
    owner_config(
        &e,
        &format!(
            "[frontier]\nbase_url = \"{}\"\nmodel = \"claude-opus-5-5\"\ndialect = \"anthropic\"\n",
            server.base_url()
        ),
    );
    let (_, report) = doctor(&e, &["--online"], &[("ZAI_API_KEY", "k")]);
    assert!(
        detail(&report, "frontier").contains("dialect anthropic"),
        "{report}"
    );
    assert_eq!(status(&report, "frontier model"), "pass", "{report}");
    assert_eq!(status(&report, "frontier cache"), "pass", "{report}");
    assert!(
        detail(&report, "frontier cache").contains("read 5100 of 5120"),
        "{report}"
    );
    let seen = server.seen();
    assert_eq!(
        seen.iter().filter(|r| r.path == "/v1/messages").count(),
        2,
        "{seen:?}"
    );
    // Anthropic takes the key in x-api-key, never as a bearer token.
    assert!(seen.iter().all(|r| !r.bearer), "{seen:?}");
}

#[test]
fn frontier_presets_set_endpoint_model_key_and_dialect_through_the_audited_path() {
    let e = env();
    let list = duet(&e, &["config", "preset"]);
    for name in [
        "zai",
        "anthropic",
        "openai",
        "https://api.anthropic.com/v1",
        "responses",
    ] {
        assert!(text(&list).contains(name), "{name}: {}", text(&list));
    }
    // A new endpoint loosens privacy: refused without --confirm, nothing written.
    let refused = duet(&e, &["config", "preset", "anthropic"]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    assert!(!e.home.join("config.toml").exists());
    let ported = duet(
        &e,
        &["config", "preset", "openai", "--port", "1", "--confirm"],
    );
    assert!(!ported.status.success());

    let applied = duet_with(
        &e,
        &["config", "preset", "anthropic", "--confirm"],
        &[("ANTHROPIC_API_KEY", "")],
    );
    assert!(applied.status.success(), "{}", text(&applied));
    assert!(
        text(&applied).contains("ANTHROPIC_API_KEY is not set"),
        "{}",
        text(&applied)
    );
    for (key, want) in [
        ("frontier.base_url", "\"https://api.anthropic.com/v1\""),
        ("frontier.model", "\"claude-opus-5-5\""),
        ("frontier.api_key_env", "\"ANTHROPIC_API_KEY\""),
        ("frontier.dialect", "\"anthropic\""),
    ] {
        assert_eq!(
            text(&duet(&e, &["config", "get", key])).trim(),
            want,
            "{key}"
        );
    }
    let log = e.home.join("state/config-audit.jsonl");
    assert_eq!(verify(&log).unwrap(), Verification::Intact { records: 4 });

    // --model replaces the preset's model.
    let openai = duet(
        &e,
        &[
            "config",
            "preset",
            "openai",
            "--model",
            "gpt-5.4",
            "--confirm",
        ],
    );
    assert!(openai.status.success(), "{}", text(&openai));
    assert_eq!(
        text(&duet(&e, &["config", "get", "frontier.model"])).trim(),
        "\"gpt-5.4\""
    );
    assert_eq!(
        text(&duet(&e, &["config", "get", "frontier.dialect"])).trim(),
        "\"responses\""
    );
    // A project may not choose the frontier's dialect.
    std::fs::create_dir_all(e.ws.join(".duet")).unwrap();
    std::fs::write(
        e.ws.join(".duet/config.toml"),
        "[frontier]\ndialect = \"chat\"\n",
    )
    .unwrap();
    assert!(!duet(&e, &["config", "list"]).status.success());
}

#[test]
fn a_preset_goes_through_the_audited_loosening_path() {
    let e = env();
    let list = duet(&e, &["config", "preset"]);
    assert!(list.status.success());
    for name in ["ollama", "lmstudio", "llamacpp", "vllm", "omlx", "mlx"] {
        assert!(text(&list).contains(name), "{name}");
    }
    assert!(!duet(&e, &["config", "preset", "nope"]).status.success());

    // Changing the endpoint loosens privacy: shown, refused, nothing written.
    let refused = duet(&e, &["config", "preset", "ollama", "--model", "qwen3:8b"]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    assert!(text(&refused).contains("+ \"http://127.0.0.1:11434/v1\""));
    assert!(!e.home.join("config.toml").exists());

    let applied = duet(
        &e,
        &[
            "config",
            "preset",
            "ollama",
            "--model",
            "qwen3:8b",
            "--confirm",
        ],
    );
    assert!(applied.status.success(), "{}", text(&applied));
    let owner = std::fs::read_to_string(e.home.join("config.toml")).unwrap();
    assert!(owner.contains("http://127.0.0.1:11434/v1") && owner.contains("qwen3:8b"));
    let log = e.home.join("state/config-audit.jsonl");
    assert_eq!(verify(&log).unwrap(), Verification::Intact { records: 2 });

    let ported = duet(
        &e,
        &[
            "config",
            "preset",
            "llamacpp",
            "--port",
            "9090",
            "--confirm",
        ],
    );
    assert!(ported.status.success(), "{}", text(&ported));
    assert!(text(&ported).contains("local.model is still \"qwen3:8b\""));
    let get = duet(&e, &["config", "get", "local.base_url"]);
    assert_eq!(text(&get).trim(), "\"http://127.0.0.1:9090/v1\"");
}

#[test]
fn bootstrap_asks_when_the_choice_is_ambiguous_or_empty() {
    let e = env();
    let two = MockServer::start(&[("GET /v1/models", 200, r#"{"data":[{"id":"a"},{"id":"b"}]}"#)]);
    let port = two.port.to_string();
    let o = duet_with(&e, &["run", "task"], &[("DUET_LOCAL_PORTS", &port)]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let out = text(&o);
    for shown in [
        "no local model endpoint is configured",
        "found OpenAI-compatible server",
        &format!(
            "duet config set local.base_url '\"{}\"' --confirm",
            two.base_url()
        ),
        "duet config set local.model '\"b\"'",
    ] {
        assert!(out.contains(shown), "{shown}: {out}");
    }
    assert!(
        !e.home.join("config.toml").exists(),
        "bootstrap wrote config"
    );
    assert!(!e.ws.join(".duet").exists(), "a run started");

    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
        .to_string();
    let o = duet_with(&e, &["run", "task"], &[("DUET_LOCAL_PORTS", &closed)]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o).contains("duet config preset"), "{}", text(&o));

    // A configured endpoint is used as configured: no probing.
    owner_config(&e, "[local]\nbase_url = \"http://192.0.2.1:9/v1\"\n");
    let o = duet_with(&e, &["run", "task"], &[("DUET_LOCAL_PORTS", &port)]);
    assert!(
        !text(&o).contains("looking for a local server"),
        "{}",
        text(&o)
    );
}

#[test]
fn bootstrap_uses_a_single_server_for_this_run_only() {
    let e = env();
    let one = MockServer::start(&[("GET /v1/models", 200, r#"{"data":[{"id":"solo"}]}"#)]);
    // The frontier is a closed loopback port and has no key: the run fails
    // before any request.
    owner_config(&e, "[frontier]\nbase_url = \"http://127.0.0.1:9/v1\"\n");
    let port = one.port.to_string();
    let o = duet_with(&e, &["run", "task"], &[("DUET_LOCAL_PORTS", &port)]);
    let out = text(&o);
    assert!(
        out.contains(&format!(
            "using solo at {} for this run only",
            one.base_url()
        )),
        "{out}"
    );
    let owner = std::fs::read_to_string(e.home.join("config.toml")).unwrap();
    assert!(!owner.contains("solo"), "bootstrap wrote config");
    let run = std::fs::read_dir(e.ws.join(".duet/runs"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(run.join("run.json")).unwrap()).unwrap();
    assert_eq!(manifest["local"]["base_url"], one.base_url());
    assert_eq!(manifest["local"]["model"], "solo");
    assert!(
        one.seen().iter().all(|r| r.method == "GET"),
        "{:?}",
        one.seen()
    );
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no check {name}: {report}"))
}

#[test]
fn doctor_shows_the_approval_mode_and_notes_release_keys_in_a_development_build() {
    let e = env();
    let (_, report) = doctor(&e, &[], &[]);
    let approval = check(&report, "approval");
    assert_eq!(approval["status"], "pass");
    assert!(approval["detail"].as_str().unwrap().starts_with("off"));
    // A development build: no release has been published, so no warning.
    let keys = check(&report, "release keys");
    assert_eq!(keys["status"], "skip", "{keys}");
    assert!(
        keys["detail"]
            .as_str()
            .unwrap()
            .contains("development build")
    );
    assert!(keys["fix"].as_str().unwrap().contains("verify-release.sh"));
    assert!(
        check(&report, "version")["detail"]
            .as_str()
            .unwrap()
            .contains("development build")
    );

    owner_config(&e, "[oversight]\napprove = \"risky\"\n");
    std::fs::write(
        e.home.join("allowed_signers"),
        "# release signers\nrelease@duet namespaces=\"duet-release\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample\n",
    )
    .unwrap();
    let (_, report) = doctor(&e, &[], &[]);
    assert!(
        check(&report, "approval")["detail"]
            .as_str()
            .unwrap()
            .starts_with("risky")
    );
    let keys = check(&report, "release keys");
    assert_eq!(keys["status"], "pass", "{keys}");
    assert!(
        keys["detail"]
            .as_str()
            .unwrap()
            .starts_with("1 release signer")
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn doctor_starts_configured_mcp_servers_and_config_lists_them() {
    let e = env();
    let mut servers = toml::Table::new();
    let mut add = |name: &str, fields: &[(&str, toml::Value)]| {
        servers.insert(
            name.into(),
            toml::Value::Table(
                fields
                    .iter()
                    .map(|(k, v)| ((*k).into(), v.clone()))
                    .collect(),
            ),
        );
    };
    let s = |t: &str| toml::Value::String(t.into());
    add(
        "sh",
        &[
            ("command", s("/bin/sh")),
            (
                "args",
                toml::Value::Array(vec![s("-c"), s(duet_mcp::mock::SH_SERVER)]),
            ),
        ],
    );
    add("broken", &[("command", s("/nonexistent/mcp-server"))]);
    add("remote", &[("url", s("https://mcp.example.com/mcp"))]);
    add(
        "off",
        &[
            ("command", s("x")),
            ("enabled", toml::Value::Boolean(false)),
        ],
    );
    add("plain", &[("url", s("http://mcp.example.com/mcp"))]);
    let mut mcp = toml::Table::new();
    mcp.insert("servers".into(), toml::Value::Table(servers));
    let mut root = toml::Table::new();
    root.insert("mcp".into(), toml::Value::Table(mcp));
    owner_config(&e, &toml::to_string(&root).unwrap());

    let (code, report) = doctor(&e, &[], &[]);
    let mcp: Vec<(String, String)> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["name"] == "mcp")
        .map(|c| {
            (
                c["status"].as_str().unwrap().to_owned(),
                c["detail"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let find = |server: &str| {
        mcp.iter()
            .find(|(_, d)| d.contains(&format!("`{server}`")))
            .unwrap_or_else(|| panic!("{server}: {mcp:?}"))
            .clone()
    };
    assert_eq!(
        find("sh"),
        (
            "pass".into(),
            "server `sh` (stdio): started, 4 tool(s)".into()
        )
    );
    assert_eq!(find("broken").0, "warn");
    assert!(
        find("remote").1.contains("not contacted offline"),
        "{mcp:?}"
    );
    assert!(find("off").1.contains("disabled"), "{mcp:?}");
    let plain = find("plain");
    assert!(
        plain.0 == "fail" && plain.1.contains("loopback"),
        "{plain:?}"
    );
    assert_eq!(code, 2);

    let list = text(&duet(&e, &["config", "list"]));
    assert!(
        list.contains("mcp.servers.sh.command = \"/bin/sh\"  (owner;"),
        "{list}"
    );
    assert!(
        list.contains("mcp.servers.sh.approve = \"writes\"  (default;"),
        "{list}"
    );
}
