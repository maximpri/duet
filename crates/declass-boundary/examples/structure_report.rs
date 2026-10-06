// SPDX-License-Identifier: GPL-3.0-or-later
//! What the frontier would be shown of a workspace's sensitive files with
//! structure views on: the task note (with the structure outline), each
//! sensitive file's view and a synthetic sample of each data file. Nothing
//! is sent anywhere and no model runs; the workspace is only read.
//!
//! ```text
//! cargo run -p declass-boundary --example structure_report -- <workspace> <objective file> <state dir> [rows] [commands.json]
//! ```
//!
//! `commands.json`, when given, is a list of `{"command": …, "output": <file>}`:
//! each recorded output is then shown as the output of that command run with
//! `sensitive_data` would be (masked output, probes), in order.
//!
//! `<state dir>` receives the engine's run state (vault, handles); it must
//! be outside the workspace. Used to measure how many of the questions a
//! run put to `ask_local` the views answer (ARCHITECTURE.md, "Structure views, synthetic samples, masked output").

use declass_boundary::engine::Engine;
use declass_boundary::policy::{Policy, StructureSettings};
use declass_boundary::view::{Presenter, Source};
use std::path::{Path, PathBuf};

const GLOBS: &[&str] = &[
    ".env*",
    "**/.env*",
    "*.pem",
    "*.key",
    "secrets/**",
    "data/**",
    "*.csv",
    "*.db",
    "*.sqlite",
    "*.parquet",
    "logs/**",
    "*.log",
];
const SKIP: &[&str] = &[".git", ".declass", "node_modules", "target"];

fn files(root: &Path, rel: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let child = rel.join(&name);
        match e.file_type() {
            Ok(t) if t.is_dir() && !SKIP.contains(&name.as_str()) => files(root, &child, out),
            Ok(t) if t.is_file() => out.push(child.display().to_string()),
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: structure_report <workspace> <objective file> <state dir> [rows]");
        std::process::exit(2);
    }
    let ws = PathBuf::from(&args[1]);
    let objective = std::fs::read_to_string(&args[2]).expect("objective file");
    let state = PathBuf::from(&args[3]);
    let rows: u64 = args.get(4).and_then(|r| r.parse().ok()).unwrap_or(5);
    std::fs::create_dir_all(&state).expect("state dir");
    let policy = Policy {
        sensitive_globs: GLOBS.iter().map(|g| (*g).to_owned()).collect(),
        command_output_sensitive: true,
        detect_secrets: true,
        detect_pii: true,
        detect_entropy: true,
        bulky_tokens: 2000,
        bulky_file_tokens: 12000,
        structure: StructureSettings {
            views: true,
            synthetic_rows: 20,
            masked_numbers: 24,
            output_probes: Some(12),
        },
        ..Policy::default()
    };
    let engine = Engine::open(&state, policy, None).expect("engine");
    let mut all = Vec::new();
    files(&ws, Path::new(""), &mut all);
    engine.prime(&ws, &all, &objective);
    println!(
        "=== task note ===\n{}\n",
        engine.sanitize_objective(&objective)
    );
    for f in &all {
        let path = Path::new(f);
        if !engine.path_sensitive(path) {
            continue;
        }
        let bytes = std::fs::read(ws.join(path)).expect("read");
        let view = engine.present(
            &Source::File {
                path: path.to_owned(),
                ranged: false,
            },
            &bytes,
        );
        println!("=== read_file {f} ({} bytes) ===\n{view}", view.len());
        let handle = view
            .split_whitespace()
            .next()
            .filter(|h| h.starts_with('h'))
            .map(str::to_owned);
        if let Some(h) = handle {
            let args = serde_json::json!({"handle": h, "rows": rows});
            match engine.call_tool("synthetic_sample", args.as_object().expect("object")) {
                Some(Ok(t)) => println!("=== synthetic_sample {h} ({} bytes) ===\n{t}", t.len()),
                Some(Err(e)) => println!("=== synthetic_sample {h} ===\n[error] {e}\n"),
                None => {}
            }
        }
    }
    if let Some(list) = args.get(5) {
        let list: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(list).expect("commands file"))
                .expect("a JSON list");
        for c in list {
            let command = c["command"].as_str().unwrap_or_default();
            let Ok(output) = std::fs::read(c["output"].as_str().unwrap_or_default()) else {
                continue;
            };
            let view = engine.present(
                &Source::SensitiveCommand {
                    command: command.to_owned(),
                    exit_code: Some(0),
                },
                &output,
            );
            println!(
                "=== run_command (sensitive_data) {} ===\n{view}",
                command.lines().next().unwrap_or_default()
            );
        }
    }
    for e in engine.take_events() {
        println!("audit: {}", serde_json::to_string(&e).unwrap_or_default());
    }
}
