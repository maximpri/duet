// SPDX-License-Identifier: GPL-3.0-or-later
//! Declass's own language-server client, written from the Language Server
//! Protocol 3.17 specification.
//!
//! - [`framing`]: the base protocol (`Content-Length` headers, JSON bodies).
//! - [`client`]: one connection: requests with timeouts and cancellation,
//!   the server's own requests (`workspace/configuration`, progress tokens,
//!   capability registration) answered minimally, published diagnostics.
//! - [`manager`]: the servers of a run, one per language, started lazily in
//!   the OS sandbox (`declass_sandbox::spawn`) and kept across tool calls; a
//!   server that stops is restarted once, then reported unavailable (the run
//!   goes on).
//! - [`servers`]: the built-in table (rust-analyzer, typescript-language-server,
//!   pyright/basedpyright, gopls, clangd) found on `PATH`, and the owner's
//!   `[lsp.servers.<language>]` entries.
//! - [`position`]: 1-based line and character columns to and from the
//!   protocol's 0-based UTF-16 positions.
//!
//! This crate knows nothing about privacy: which files may be sent to a
//! server and how answers are shown is decided by the caller (`declass-agent`).

pub mod client;
pub mod framing;
pub mod manager;
pub mod mock;
pub mod position;
pub mod servers;

pub use client::{CallError, Diagnostic, Published, Transport};
pub use manager::{Event, Hidden, Launcher, Launching, Lsp, LspError, SandboxLauncher};
pub use position::{Position, Range};
pub use servers::{Detected, ServerConfig, ServerSpec, Settings};
