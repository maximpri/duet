// SPDX-License-Identifier: GPL-3.0-or-later
//! Commands' network for a run, from `sandbox.network` and
//! `sandbox.registries`, and the bridge helper declass itself serves as inside
//! a bubblewrap sandbox (`declass __sandbox-bridge <command...>`).

use anyhow::{Result, anyhow};
use declass_agent::egress::{Network, Registries};
use declass_config::Config;
use std::sync::Arc;

/// The hidden subcommand that runs the bridge helper (see
/// `declass_sandbox::bridge`); never typed by anyone.
pub(crate) const BRIDGE_ARG: &str = "__sandbox-bridge";

/// When declass was started as the bridge helper (`args` is its command line,
/// program name first): its exit code.
pub(crate) fn bridge_helper(args: &[std::ffi::OsString]) -> Option<i32> {
    let mut args = args.iter().skip(1);
    if *args.next()? != BRIDGE_ARG {
        return None;
    }
    Some(declass_sandbox::bridge::main(args.cloned().collect()))
}

/// The run's `sandbox.network`. With `registries`, the proxy starts on the
/// first command that needs it.
pub(crate) fn network(cfg: &Config) -> Result<Network> {
    Ok(match cfg.str("sandbox.network")?.as_str() {
        "off" => Network::Off,
        "all" => Network::All,
        _ => {
            let hosts = declass_egress::Hosts::parse(&cfg.list("sandbox.registries")?)
                .map_err(|e| anyhow!("sandbox.registries: {e}"))?;
            let helper = vec![std::env::current_exe()?.into_os_string(), BRIDGE_ARG.into()];
            Network::Registries(Arc::new(Registries::new(hosts, helper)))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(owner: &str) -> (tempfile::TempDir, Config) {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(&path, owner).unwrap();
        let cfg = Config::load(&path, None).unwrap();
        (d, cfg)
    }

    #[test]
    fn the_mode_follows_the_configuration() {
        let (_d, cfg) = config("");
        assert_eq!(network(&cfg).unwrap().name(), "registries");
        let (_d, cfg) = config("[sandbox]\nnetwork = true\n");
        assert_eq!(network(&cfg).unwrap().name(), "all");
        let (_d, cfg) = config("[sandbox]\nnetwork = \"off\"\n");
        assert_eq!(network(&cfg).unwrap().name(), "off");
        let (_d, cfg) = config("[sandbox]\nregistries = [\"https://x.example/\"]\n");
        let err = network(&cfg).unwrap_err().to_string();
        assert!(
            err.contains("sandbox.registries") && err.contains("x.example"),
            "{err}"
        );
    }

    #[test]
    fn the_built_in_credential_list_matches_the_sandbox() {
        // The same list the agent denies; kept in one place (declass-sandbox).
        assert!(declass_sandbox::HOME_SECRETS.contains(&".npmrc"));
        assert!(declass_sandbox::HOME_SECRETS.contains(&".cargo/credentials.toml"));
    }
}
