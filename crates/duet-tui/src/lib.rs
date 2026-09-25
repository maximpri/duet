// SPDX-License-Identifier: GPL-3.0-or-later
//! `duet tui`: configuration screens generated from the settings registry,
//! an audit viewer and a read-only run view.
//!
//! The TUI holds no policy logic. Every edit is checked and applied by
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
mod ui;

#[cfg(test)]
mod tests;

pub use app::{App, DoctorLine, Paths, Tab};
pub use models::{CacheReport, LocalServer};

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::sync::Arc;
use std::time::Duration;

/// Runs `duet doctor`; the argument is whether to include the `--online` checks.
pub type Doctor = Box<dyn Fn(bool) -> Vec<DoctorLine>>;

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

/// Runs the TUI on the terminal until the user quits. Fails before touching
/// the terminal when it has no usable size.
pub fn run(paths: Paths, services: Services) -> anyhow::Result<()> {
    use std::io::IsTerminal;
    anyhow::ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "no usable terminal: duet tui needs an interactive terminal on standard input and output"
    );
    let (width, height) = ratatui::crossterm::terminal::size()
        .map_err(|e| anyhow::anyhow!("no usable terminal: {e}"))?;
    check_size(width, height).map_err(anyhow::Error::msg)?;
    let mut app = App::new(paths, services)?;
    let mut terminal = ratatui::try_init().map_err(|e| {
        ratatui::restore();
        anyhow::anyhow!("no usable terminal: {e}")
    })?;
    let result = (|| -> anyhow::Result<()> {
        while !app.quit {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(500))? {
                if let Event::Key(k) = event::read()?
                    && k.kind == KeyEventKind::Press
                {
                    app.key(k.code, k.modifiers);
                }
            } else {
                app.tick();
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}
