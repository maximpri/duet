// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline config reload timing: `cargo run -p declass-config --release --example config_load_benchmark`.

use declass_config::Config;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let owner = workspace.path().join("owner.toml");
    let project = workspace.path().join("project.toml");
    std::fs::write(
        &owner,
        "[local]\nmodel='example-coder'\n[mcp.servers.files]\ncommand='files'\n",
    )
    .expect("owner config");
    std::fs::write(&project, "[limits]\nfrontier_usd=1.0\n").expect("project config");
    let first = Instant::now();
    black_box(Config::load(&owner, Some(&project)).expect("initial load"));
    println!(
        "first load: {:.3} ms",
        first.elapsed().as_secs_f64() * 1_000.0
    );
    let iterations = 2_000;
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(Config::load(&owner, Some(&project)).expect("reload"));
    }
    let elapsed = start.elapsed();
    println!(
        "{iterations} reloads: {:.3} ms; {:.3} us/reload",
        elapsed.as_secs_f64() * 1_000.0,
        elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64,
    );
}
