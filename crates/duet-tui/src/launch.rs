// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting a run from the TUI: the objective and mode are asked for on the
//! Run screen, then `duet run` is launched as a child process (its own process
//! group, no terminal: its output goes to a private log under `.duet/tmp`).
//! The run id is read from that log and the Run view opens on it. Passthrough
//! needs the same explicit acknowledgement as `--no-privacy` before it is
//! passed on. The run keeps going if the TUI quits.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Hybrid,
    LocalOnly,
    Passthrough,
}

impl RunMode {
    pub const ALL: [RunMode; 3] = [RunMode::Hybrid, RunMode::LocalOnly, RunMode::Passthrough];

    pub fn arg(self) -> &'static str {
        match self {
            RunMode::Hybrid => "hybrid",
            RunMode::LocalOnly => "local-only",
            RunMode::Passthrough => "passthrough",
        }
    }

    fn help(self) -> &'static str {
        match self {
            RunMode::Hybrid => "frontier decides; sensitive content stays with the local model",
            RunMode::LocalOnly => "the local model does everything",
            RunMode::Passthrough => "frontier only, privacy boundary OFF",
        }
    }

    pub(crate) fn step(self, d: isize) -> Self {
        let i = RunMode::ALL.iter().position(|m| *m == self).unwrap_or(0) as isize;
        RunMode::ALL[(i + d).rem_euclid(RunMode::ALL.len() as isize) as usize]
    }
}

/// The same warning `duet run --mode passthrough` prints.
pub const PASSTHROUGH_WARNING: &str = "PRIVACY BOUNDARY OFF (passthrough mode). File contents, command \
output, secrets and personal data are sent to the frontier provider unfiltered. Use it only on \
repositories with nothing sensitive, as a reference lane.";

/// A `duet run` started from the TUI.
pub struct Launched {
    child: Child,
    pub log: PathBuf,
    pub run_id: Option<String>,
    /// Exit code once it ended (`-1`: killed by a signal).
    pub exit: Option<i32>,
}

/// Starts `duet --workspace <ws> run --mode <mode> [--no-privacy] -- <objective>`.
/// `acknowledged` is required for passthrough.
pub fn spawn(
    duet: &Path,
    ws: &Path,
    objective: &str,
    mode: RunMode,
    acknowledged: bool,
) -> std::io::Result<Launched> {
    if mode == RunMode::Passthrough && !acknowledged {
        return Err(std::io::Error::other(
            "passthrough turns the privacy boundary off and needs an explicit acknowledgement",
        ));
    }
    let tmp = ws.join(".duet/tmp");
    duet_fs::private::ensure_private_dir(&tmp).map_err(std::io::Error::other)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let log = tmp.join(format!("tui-run-{stamp}.log"));
    let out = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&log)?;
    let mut cmd = Command::new(duet);
    cmd.arg("--workspace")
        .arg(ws)
        .args(["run", "--mode", mode.arg()]);
    if mode == RunMode::Passthrough {
        cmd.arg("--no-privacy");
    }
    cmd.arg("--")
        .arg(objective)
        .current_dir(ws)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        // Its own process group: keys and signals meant for the TUI never reach it.
        .process_group(0);
    Ok(Launched {
        child: cmd.spawn()?,
        log,
        run_id: None,
        exit: None,
    })
}

/// The run id `duet run` announces on its first line (`run <id> (<mode>)`).
pub fn run_id_in(log: &str) -> Option<String> {
    log.lines().find_map(|l| {
        let rest = l.strip_prefix("run ")?;
        let (id, tail) = rest.split_once(' ')?;
        (tail.starts_with('(') && duet_agent::purge::check_run_id(id).is_ok()).then(|| id.into())
    })
}

impl Launched {
    fn read_log(&self) -> String {
        let mut text = String::new();
        if let Ok(f) = std::fs::File::open(&self.log) {
            let _ = f.take(1 << 20).read_to_string(&mut text);
        }
        text
    }

    /// Picks up the run id and the exit. Returns true when something changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if self.run_id.is_none() {
            self.run_id = run_id_in(&self.read_log());
            changed |= self.run_id.is_some();
        }
        if self.exit.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exit = Some(status.code().unwrap_or(-1));
            if self.run_id.is_none() {
                self.run_id = run_id_in(&self.read_log());
            }
            changed = true;
        }
        changed
    }

    pub fn running(&self) -> bool {
        self.exit.is_none()
    }

    /// The last non-empty line of the child's output (why it stopped).
    pub fn last_line(&self) -> String {
        self.read_log()
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .chars()
            .take(200)
            .collect()
    }
}

/// What an exit code of `duet run` means.
pub fn outcome(code: i32) -> &'static str {
    match code {
        0 => "completed",
        1 => "failed",
        2 => "stopped before starting",
        3 => "stopped by its budget",
        _ => "ended abnormally",
    }
}

pub(crate) fn draw_dialog(f: &mut Frame, objective: &str, mode: RunMode) {
    let [area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(f.area());
    let [area] = Layout::vertical([Constraint::Length(12)])
        .flex(Flex::Center)
        .areas(area);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("objective: ", Style::new().add_modifier(Modifier::BOLD)),
            Span::raw(objective.to_owned()),
            Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
        ]),
        Line::from(""),
        Line::styled("mode (↑↓):", Style::new().add_modifier(Modifier::BOLD)),
    ];
    for m in RunMode::ALL {
        let chosen = m == mode;
        let style = match (chosen, m) {
            (true, RunMode::Passthrough) => {
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
            }
            (true, _) => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            _ => Style::new().fg(Color::DarkGray),
        };
        lines.push(Line::styled(
            format!(
                "  {} {:<12} {}",
                if chosen { ">" } else { " " },
                m.arg(),
                m.help()
            ),
            style,
        ));
    }
    lines.extend([
        Line::from(""),
        Line::styled(
            "Enter start · Esc cancel",
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" start a run (duet run) ")),
        area,
    );
}

pub(crate) fn draw_ack(f: &mut Frame, objective: &str) {
    let [area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(f.area());
    let [area] = Layout::vertical([Constraint::Length(12)])
        .flex(Flex::Center)
        .areas(area);
    let lines = vec![
        Line::styled(
            PASSTHROUGH_WARNING,
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::from(format!("objective: {objective}")),
        Line::from("This is what `duet run --mode passthrough --no-privacy` acknowledges."),
        Line::from(""),
        Line::styled(
            "y start with the boundary off    n cancel",
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ];
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .title(" acknowledge: no privacy ")
                .border_style(Style::new().fg(Color::Red)),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_announced_run_id_only() {
        assert_eq!(
            run_id_in(
                "no local model endpoint is configured\nrun 20260925-100000-abcdef (Hybrid)\n"
            ),
            Some("20260925-100000-abcdef".into())
        );
        assert_eq!(run_id_in("run ../x (Hybrid)\n"), None);
        assert_eq!(run_id_in("running tests\n"), None);
        assert_eq!(RunMode::Hybrid.step(-1), RunMode::Passthrough);
        assert_eq!(RunMode::Passthrough.step(1), RunMode::Hybrid);
    }
}
