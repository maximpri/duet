// SPDX-License-Identifier: GPL-3.0-or-later
//! Live check (ignored by default; needs the internet): with default settings
//! a command in declass's sandbox creates a Rust project and adds and builds
//! dependencies from crates.io, and creates a Node project and installs a
//! package from npm, through the egress proxy; anything else is refused.
//!
//! `cargo test -p declass-cli --test live_registries -- --ignored --nocapture`

use declass_agent::egress::{Network, Registries};
use declass_agent::journal::WriteJournal;
use declass_agent::tools::{Ctx, Outcome, dispatch};
use declass_boundary::audit::{AuditEvent, AuditHandle, AuditLog, Line, read};
use declass_boundary::view::PassThrough;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: reaches crates.io and the npm registry"]
async fn default_settings_install_from_the_real_registries() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let ws = root.join("ws");
    let run = root.join("run");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    // Defaults: no owner or project file.
    let cfg = declass_config::Config::load(&root.join("no-owner.toml"), None).unwrap();
    assert_eq!(cfg.str("sandbox.network").unwrap(), "registries");
    let hosts = declass_egress::Hosts::parse(&cfg.list("sandbox.registries").unwrap()).unwrap();
    let helper = vec![
        env!("CARGO_BIN_EXE_declass").into(),
        "__sandbox-bridge".into(),
    ];
    let network = Network::Registries(Arc::new(Registries::new(hosts, helper)));
    let log = run.join("audit.jsonl");
    let audit = AuditHandle::new(AuditLog::open(&log).unwrap());
    let git = declass_git::Git::locate().unwrap();
    let presenter = PassThrough { max_bytes: 100_000 };
    let mut journal = WriteJournal::open(&run).unwrap();
    let mut ctx = Ctx {
        workspace: &ws,
        run_dir: &run,
        sandbox: declass_sandbox::detect().unwrap(),
        git: &git,
        presenter: &presenter,
        journal: &mut journal,
        command_timeout: Duration::from_secs(600),
        network: &network,
        checks: &[],
        audit: Some(&audit),
        interrupted: None,
        web: None,
        git_tools: None,
        lsp: None,
    };
    let mut outputs = Vec::new();
    for command in [
        // itoa is likely in the operator's cargo cache (linked); oorandom
        // likely not (downloaded).
        "cargo new -q demo && cd demo && cargo add itoa oorandom 2>&1 | tail -4 && \
         printf 'fn main() { let mut b = itoa::Buffer::new(); let mut r = oorandom::Rand32::new(4); \
         println!(\"{} {}\", b.format(42), r.rand_range(0..1)); }\\n' > src/main.rs && \
         cargo run -q 2>&1 | tail -3",
        "mkdir web && cd web && npm init -y >/dev/null && npm install left-pad 2>&1 | tail -3 && \
         node -e \"console.log(require('left-pad')('7', 3, '0'))\"",
        "curl -sS -m 20 https://example.com/ -o /dev/null; echo curl-exit=$?",
    ] {
        let serde_json::Value::Object(args) = json!({ "command": command }) else {
            unreachable!()
        };
        let out = match dispatch(&mut ctx, "run_command", &args).await {
            Outcome::Result(s) | Outcome::Error(s) => s,
            other => format!("{other:?}"),
        };
        eprintln!("$ {command}\n{out}\n");
        outputs.push(out);
    }
    assert!(
        outputs[0].contains("exit code 0") && outputs[0].contains("42 0"),
        "{}",
        outputs[0]
    );
    assert!(
        outputs[1].contains("exit code 0") && outputs[1].contains("007"),
        "{}",
        outputs[1]
    );
    assert!(
        outputs[2].contains("the egress proxy refused: example.com:443"),
        "{}",
        outputs[2]
    );
    let mut by_host = std::collections::BTreeMap::<(String, String), (u32, u64, u64)>::new();
    for l in read(&log).unwrap() {
        if let Line::Event(e) = l
            && let AuditEvent::Egress {
                host,
                outcome,
                bytes_up,
                bytes_down,
                ..
            } = e.event
        {
            let entry = by_host.entry((host, outcome)).or_default();
            entry.0 += 1;
            entry.1 += bytes_up;
            entry.2 += bytes_down;
        }
    }
    eprintln!("egress (host, outcome) -> (connections, bytes up, bytes down):");
    for (k, v) in &by_host {
        eprintln!("  {k:?} -> {v:?}");
    }
    assert!(by_host.contains_key(&("index.crates.io".into(), "allowed".into())));
    assert!(by_host.contains_key(&("registry.npmjs.org".into(), "allowed".into())));
    assert!(by_host.contains_key(&("example.com".into(), "refused".into())));
}
