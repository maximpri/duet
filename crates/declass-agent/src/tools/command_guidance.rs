// SPDX-License-Identifier: GPL-3.0-or-later
//! Explain command boundaries without weakening them or exposing raw output.

use crate::egress::Network;
use declass_sandbox::Output;

const FILES: &str = "Commands can write to the workspace and their private $TMPDIR. Use \
\"$TMPDIR/name\" for temporary files; hard-coded /tmp paths and home-directory writes are not \
available. Use list_files/search for discovery and read_file for files denied to commands; \
.declass and .git remain unavailable. A skill does not grant access to a browser daemon, its \
home-directory state, or host sockets.";

const LIFETIME: &str = "Start and test a development server in the same run_command call. \
Every child process is stopped when the command ends, including nohup/background processes. \
Use an unused loopback port; do not stop an existing host service to take its port.";

const MAC_PORTS: &str = "On macOS with registry-only networking, use an unused loopback \
development port such as 8000 or 5173. Arbitrary ports and existing host listeners are blocked.";

pub(super) fn description(network: &Network) -> String {
    let mut description = format!(
        "Run a shell command in the repository root (sandboxed: {}). Returns the exit code and \
output. {FILES} {LIFETIME} Shell pipelines return the last command's status; use explicit status \
checks so a final cat, head or tail cannot hide a failed build or test.",
        network.describe()
    );
    if cfg!(target_os = "macos") && matches!(network, Network::Registries(_)) {
        description.push(' ');
        description.push_str(MAC_PORTS);
    }
    description
}

/// Only call this for output that is allowed to reach the frontier. The
/// added text is constant guidance: no stderr, path, command or private value
/// is copied past the presenter. A hint describes constraints, not a diagnosis.
pub(super) fn append_recovery(
    shown: &mut String,
    command: &str,
    network: &Network,
    output: &Output,
) {
    let mut hints = Vec::new();
    if output.shows_denial() {
        hints.push(FILES);
    }
    let curl_command = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '('))
        .any(|word| word == "curl" || word.ends_with("/curl"));
    let connection_error = (curl_command && output.exit_code == Some(7))
        || [&output.stdout, &output.stderr].iter().any(|bytes| {
            ["EADDRINUSE", "address already in use", "connection refused"]
                .iter()
                .any(|message| {
                    bytes
                        .windows(message.len())
                        .any(|window| window.eq_ignore_ascii_case(message.as_bytes()))
                })
        });
    if connection_error {
        match network {
            Network::Off => hints.push(
                "Command networking is disabled for this run; network clients and development \
servers cannot communicate. Report this verification limit to the operator.",
            ),
            Network::Registries(_) if cfg!(target_os = "macos") => {
                hints.push(LIFETIME);
                hints.push(MAC_PORTS);
            }
            _ => hints.push(LIFETIME),
        }
    }
    if !hints.is_empty() {
        shown.push_str("\n[command guidance] ");
        shown.push_str(&hints.join(" "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn output(code: i32, stderr: &str) -> Output {
        Output {
            exit_code: Some(code),
            timed_out: false,
            interrupted: false,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            stdout_total: 0,
            stderr_total: stderr.len(),
            spilled_to: None,
            removed_reserved: Vec::new(),
            memory_kills: Vec::new(),
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn denied_command_guidance_never_repeats_the_raw_error() {
        let mut shown = "filtered output".to_owned();
        append_recovery(
            &mut shown,
            "node probe.js",
            &Network::Off,
            &output(1, "private-name: operation not permitted; secret-value"),
        );
        assert!(shown.contains("$TMPDIR") && shown.contains("read_file"));
        assert!(!shown.contains("private-name") && !shown.contains("secret-value"));
    }

    #[test]
    fn connection_guidance_respects_the_active_network_mode() {
        let error = output(7, "");
        let mut off = String::new();
        append_recovery(
            &mut off,
            "curl -s http://localhost:8641",
            &Network::Off,
            &error,
        );
        assert!(off.contains("networking is disabled"));
        assert!(!off.contains("8000"));
        let mut all = String::new();
        append_recovery(
            &mut all,
            "/usr/bin/curl -s http://localhost:8641",
            &Network::All,
            &error,
        );
        assert!(all.contains("same run_command") && all.contains("existing host service"));
        assert!(!all.contains("ports and existing host listeners are blocked"));
    }

    #[test]
    fn successful_commands_and_ordinary_test_failures_get_no_sandbox_hint() {
        for error in [
            output(0, ""),
            output(1, "AssertionError: expected 4, got 3"),
            output(7, ""),
        ] {
            let mut shown = "original".to_owned();
            append_recovery(&mut shown, "node tests.js", &Network::All, &error);
            assert_eq!(shown, "original");
        }
    }
}
