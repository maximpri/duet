// SPDX-License-Identifier: GPL-3.0-or-later
//! One configured server (`[mcp.servers.<name>]` in the owner's config).

use std::time::Duration;

/// How the server is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// A program started for the run (in the sandbox), spoken to over stdio.
    Command { command: String, args: Vec<String> },
    /// A streamable HTTP endpoint.
    Url(String),
}

/// What the server's results are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Results may be shown to the frontier (after scanning); nothing
    /// sensitive may be sent to it.
    Public,
    /// Results are sensitive data: they stay on this machine. A stdio server
    /// of this kind is local, so placeholders in its arguments are resolved.
    Sensitive,
}

impl Trust {
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Public => "public",
            Trust::Sensitive => "sensitive",
        }
    }
}

/// Which of the server's tools need the operator's approval when
/// `oversight.approve` is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approve {
    /// None (the owner trusts every tool of this server).
    Auto,
    /// Tools the server does not declare read-only.
    Writes,
    /// Every tool.
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub name: String,
    pub launch: Launch,
    /// Environment variables passed to a stdio server by name (on top of the
    /// sandbox's base allowlist); every other variable is cleared.
    pub env: Vec<String>,
    /// HTTP headers whose values come from environment variables:
    /// (header name, variable name).
    pub headers_env: Vec<(String, String)>,
    pub trust: Trust,
    /// Network access for a stdio server's sandbox.
    pub network: bool,
    pub approve: Approve,
    /// Limit for starting the server and for each call.
    pub timeout: Duration,
}

/// Server names: letters, digits, `_` and `-`, at most 32 characters (they
/// become part of tool names).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl ServerConfig {
    /// Whether placeholders in arguments may be resolved for this server: it
    /// runs on this machine and its results stay here.
    pub fn is_local_sensitive(&self) -> bool {
        self.trust == Trust::Sensitive && matches!(self.launch, Launch::Command { .. })
    }

    pub fn transport_name(&self) -> &'static str {
        match self.launch {
            Launch::Command { .. } => "stdio",
            Launch::Url(_) => "http",
        }
    }
}
