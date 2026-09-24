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
//! callback supplied by the CLI (offline unless the user asks for `--online`).

mod app;
mod audit;
mod ip;
mod runs;
mod settings;
mod ui;

#[cfg(test)]
mod tests;

pub use app::{App, DoctorLine, Paths, Tab};

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::Duration;

/// Runs `duet doctor`; the argument is whether to include the `--online` checks.
pub type Doctor = Box<dyn Fn(bool) -> Vec<DoctorLine>>;

/// Runs the TUI on the terminal until the user quits.
pub fn run(paths: Paths, doctor: Doctor) -> anyhow::Result<()> {
    let mut app = App::new(paths, doctor)?;
    let mut terminal = ratatui::init();
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
