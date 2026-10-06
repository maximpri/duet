// SPDX-License-Identifier: GPL-3.0-or-later
//! Data screen purge: the runs `declass purge` would delete, a confirmation that
//! lists them, and the deletion itself (the same selection and deletion as
//! the CLI). The workspace lock is held while deleting, so a run in progress
//! is never purged and none starts meanwhile. Audit logs are kept.

use declass_agent::purge::{self, Scope};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Wrap};
use std::path::Path;

/// Runs listed in the confirmation.
const LISTED: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgePlan {
    pub scope: Scope,
    pub runs: Vec<String>,
}

impl PurgePlan {
    pub fn new(ws: &Path, scope: Scope) -> Self {
        Self {
            scope,
            runs: purge::candidates(ws, scope),
        }
    }

    fn describe(&self) -> String {
        match self.scope {
            Scope::All => "every run".into(),
            Scope::OlderThan { days } => {
                format!("runs older than data.retention_days ({days} days)")
            }
        }
    }
}

/// Deletes the planned runs under the workspace lock. Returns the status line.
pub fn execute(ws: &Path, plan: &PurgePlan) -> String {
    let _lock = match declass_fs::lock::WorkspaceLock::acquire(ws) {
        Ok(l) => l,
        Err(declass_fs::FsError::Locked) => {
            return "not purged: a run is in progress in this workspace; purge after it ends"
                .into();
        }
        Err(e) => return format!("not purged: {e}"),
    };
    let mut purged = 0;
    let mut failed = Vec::new();
    for id in &plan.runs {
        match purge::purge_run(ws, id) {
            Ok(()) => purged += 1,
            // Already gone (purged elsewhere) is not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => failed.push(format!("{id}: {e}")),
        }
    }
    if failed.is_empty() {
        format!("purged the raw data of {purged} run(s); audit logs kept")
    } else {
        format!("purged {purged} run(s); failed: {}", failed.join("; "))
    }
}

pub(crate) fn draw_confirm(f: &mut Frame, plan: &PurgePlan) {
    let [area] = Layout::horizontal([Constraint::Percentage(70)])
        .flex(Flex::Center)
        .areas(f.area());
    let [area] = Layout::vertical([Constraint::Length(LISTED as u16 + 9)])
        .flex(Flex::Center)
        .areas(area);
    let mut lines = vec![
        Line::styled(
            format!(
                "Delete the raw data of {} run(s): {}.",
                plan.runs.len(),
                plan.describe()
            ),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Line::from("Handles, transcripts, vault and write journal go; audit logs stay."),
        Line::from(""),
    ];
    for id in plan.runs.iter().take(LISTED) {
        lines.push(Line::from(format!("  {id}")));
    }
    if plan.runs.len() > LISTED {
        lines.push(Line::from(format!(
            "  … and {} more",
            plan.runs.len() - LISTED
        )));
    }
    lines.extend([
        Line::from(""),
        Line::styled(
            "y delete    n cancel",
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            crate::ui::frame()
                .title(" confirm purge ")
                .border_style(Style::new().fg(Color::Red)),
        ),
        area,
    );
}
