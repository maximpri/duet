// SPDX-License-Identifier: GPL-3.0-or-later
//! `declass-sbom`: writes a CycloneDX 1.5 JSON SBOM for a workspace package, from
//! `cargo metadata --locked --offline` (no network) and `Cargo.lock`.
//!
//!   declass-sbom [--package declass-cli] [--target <triple>] [--manifest-path Cargo.toml] [--out FILE]
//!
//! The target defaults to the host (`rustc -vV`). The timestamp is
//! `SOURCE_DATE_EPOCH` when set (reproducible releases), else now.

use anyhow::{Context, Result, bail, ensure};
use declass_release::sbom::{Options, cyclonedx};
use std::path::PathBuf;
use std::process::Command;

fn host_triple() -> Result<String> {
    let out = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("-vV")
        .output()
        .context("running rustc -vV")?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_owned))
        .context("rustc -vV printed no host")
}

fn timestamp() -> Result<String> {
    let now = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(s) => time::OffsetDateTime::from_unix_timestamp(
            s.trim().parse().context("SOURCE_DATE_EPOCH")?,
        )?,
        Err(_) => time::OffsetDateTime::now_utc(),
    };
    Ok(now
        .replace_nanosecond(0)?
        .format(&time::format_description::well_known::Rfc3339)?)
}

fn main() -> Result<()> {
    let mut package = "declass-cli".to_owned();
    let mut target = None;
    let mut manifest = PathBuf::from("Cargo.toml");
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--package" => package = value()?,
            "--target" => target = Some(value()?),
            "--manifest-path" => manifest = value()?.into(),
            "--out" => out = Some(PathBuf::from(value()?)),
            "-h" | "--help" => {
                println!(
                    "declass-sbom [--package declass-cli] [--target <triple>] [--manifest-path Cargo.toml] [--out FILE]"
                );
                return Ok(());
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let target = match target {
        Some(t) => t,
        None => host_triple()?,
    };
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let meta = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .args(["--filter-platform", &target, "--manifest-path"])
        .arg(&manifest)
        .output()
        .context("running cargo metadata")?;
    ensure!(
        meta.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&meta.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&meta.stdout)?;
    let root = metadata["workspace_root"]
        .as_str()
        .context("metadata has no workspace_root")?;
    let lock_path = PathBuf::from(root).join("Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path)
        .with_context(|| format!("reading {}", lock_path.display()))?;
    let ts = timestamp()?;
    let bom = cyclonedx(
        &metadata,
        &lock,
        &Options {
            root: &package,
            timestamp: &ts,
            tool_version: env!("CARGO_PKG_VERSION"),
        },
    )
    .map_err(anyhow::Error::msg)?;
    let text = serde_json::to_string_pretty(&bom)? + "\n";
    match out {
        Some(p) => std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))?,
        None => print!("{text}"),
    }
    Ok(())
}
