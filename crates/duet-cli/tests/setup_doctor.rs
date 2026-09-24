// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet config preset` and the no-config bootstrap through the binary, against
//! a scripted loopback server. No test calls a model: servers answer model
//! listings only, and the one run that starts stops at the missing
//! frontier key before any request.

use duet_boundary::audit::{Verification, verify};
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
