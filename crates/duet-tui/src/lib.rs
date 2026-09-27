// SPDX-License-Identifier: GPL-3.0-or-later
//! Everything the operator sees of duet on a terminal: the workspace
//! ([`workspace`], what `duet` opens: the conversation, the side panel, the
//! status bar), how duet's output looks ([`term`]), and the settings overlay
//! inside the workspace: screens generated from the settings registry, IP
//! levels, the data screen, an audit viewer and a read-only view of the
//! workspace's runs and sessions (each beside the files it changed and their
//! diffs).
//!
//! The settings screens hold no policy logic. Every edit is checked and applied by
//! `duet_config::Config::propose` / `apply` (the path `duet config set` uses):
//! a change that loosens privacy shows its policy diff and needs an explicit
//! confirmation, the project file only ever tightens, owner-only keys never
//! reach it, and each applied change is appended to the owner's config audit
//! log. Nothing here contacts a model; `duet doctor` checks run through a
//! callback supplied by the CLI (offline unless the user asks for `--online`);
//! local-server detection and the cache-reuse probe run only on request.

mod app;
mod audit;
mod changes;
mod data;
mod ip;
mod models;
mod runs;
mod settings;
pub mod term;
mod ui;
pub mod workspace;

#[cfg(test)]
mod tests;

pub use app::{App, DoctorLine, Paths, Tab};
pub use models::{CacheReport, LocalServer};

use std::sync::Arc;

/// Runs `duet doctor`; the argument is whether to include the `--online` checks.
pub type Doctor = Box<dyn Fn(bool) -> Vec<DoctorLine> + Send>;

/// What the TUI asks of the rest of Duet. The CLI supplies these (the TUI
/// holds no provider code); tests supply fakes. Nothing here runs unless the
/// user asks for it on a screen.
pub struct Services {
    /// `duet doctor`, offline or with `--online` (connection, context window).
    pub doctor: Doctor,
    /// Loopback discovery of local servers (preset ports; never a model call).
    pub detect: Arc<dyn Fn() -> Vec<LocalServer> + Send + Sync>,
    /// The cache-reuse probe against the configured local model (two model calls).
    pub cache_probe: Arc<dyn Fn() -> Result<CacheReport, String> + Send + Sync>,
}

/// The smallest terminal the screens are laid out for (columns, rows).
pub const MIN_SIZE: (u16, u16) = (80, 24);

/// Refuses a terminal smaller than [`MIN_SIZE`], including one that reports
/// no size at all (0x0), with a message saying what is needed.
pub fn check_size(width: u16, height: u16) -> Result<(), String> {
    let (w, h) = MIN_SIZE;
    if width >= w && height >= h {
        Ok(())
    } else {
        Err(format!(
            "terminal too small: need at least {w}x{h}, this one is {width}x{height}"
        ))
    }
}
