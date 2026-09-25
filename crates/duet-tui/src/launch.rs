// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting a run or a session from the TUI: the objective (a session's first
//! message) and mode are asked for on the Run screen, then `duet run` or
//! `duet chat` is launched as a child process (its own process group, no
//! terminal: its output goes to a private log under `.duet/tmp`). The run id
//! is read from that log and the Run view opens on it. Passthrough needs the
//! same explicit acknowledgement as `--no-privacy` before it is passed on.
//!
//! A session's child reads the operator's further messages from a pipe the
//! TUI writes to (one line per message); an interrupt is a SIGINT to the
//! child, which stops its current turn. A run keeps going if the TUI quits;
//! a session finishes its current turn and is left open to resume.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use std::io::Read;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

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

/// A `duet run` or `duet chat` started from the TUI.
pub struct Launched {
    child: Child,
    /// A session's input (`None` for a run, or once it was closed).
    input: Option<ChildStdin>,
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
    let args = mode_args("run", mode, acknowledged)?;
    start(duet, ws, &args, Some(objective), false)
}

/// Starts a session: `duet --workspace <ws> chat --mode <mode> [--no-privacy]
/// -- <message>`, its input a pipe.
pub fn spawn_session(
    duet: &Path,
    ws: &Path,
    message: &str,
    mode: RunMode,
    acknowledged: bool,
) -> std::io::Result<Launched> {
    let args = mode_args("chat", mode, acknowledged)?;
    start(duet, ws, &args, Some(message), true)
}

/// Continues a session: `duet --workspace <ws> chat --resume <id>`.
pub fn resume_session(duet: &Path, ws: &Path, id: &str) -> std::io::Result<Launched> {
    duet_agent::purge::check_run_id(id).map_err(std::io::Error::other)?;
    let args = vec!["chat".to_owned(), "--resume".into(), id.into()];
    start(duet, ws, &args, None, true)
}

fn mode_args(command: &str, mode: RunMode, acknowledged: bool) -> std::io::Result<Vec<String>> {
    if mode == RunMode::Passthrough && !acknowledged {
        return Err(std::io::Error::other(
            "passthrough turns the privacy boundary off and needs an explicit acknowledgement",
        ));
    }
    let mut args = vec![command.to_owned(), "--mode".into(), mode.arg().into()];
    if mode == RunMode::Passthrough {
        args.push("--no-privacy".into());
    }
    Ok(args)
}

fn start(
    duet: &Path,
    ws: &Path,
    args: &[String],
    text: Option<&str>,
    session: bool,
) -> std::io::Result<Launched> {
    let tmp = ws.join(".duet/tmp");
    duet_fs::private::ensure_private_dir(&tmp).map_err(std::io::Error::other)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let kind = if session { "session" } else { "run" };
    let log = tmp.join(format!("tui-{kind}-{stamp}.log"));
    let out = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&log)?;
    let mut cmd = Command::new(duet);
    cmd.arg("--workspace").arg(ws).args(args);
    if let Some(t) = text {
        cmd.arg("--").arg(t);
    }
    cmd.current_dir(ws)
        .stdin(if session {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(out.try_clone()?)
        .stderr(out)
        // Its own process group: keys and signals meant for the TUI never reach it.
        .process_group(0);
    let mut child = cmd.spawn()?;
    Ok(Launched {
        input: child.stdin.take(),
        child,
        log,
        run_id: None,
        exit: None,
    })
}

/// The run id `duet run` or `duet chat` announces on its first line
/// (`run <id> (<mode>)`, `session <id> (<mode>)`).
pub fn run_id_in(log: &str) -> Option<String> {
    log.lines().find_map(|l| {
        let rest = l
            .strip_prefix("run ")
            .or_else(|| l.strip_prefix("session "))?;
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

    /// Whether this is a session that can still take messages.
    pub fn takes_messages(&self) -> bool {
        self.running() && self.input.is_some()
    }

    /// Sends a session one message (or command). Newlines inside it are
    /// sent as line continuations, so it arrives as one message.
    pub fn send(&mut self, text: &str) -> std::io::Result<()> {
        let Some(input) = self.input.as_mut() else {
            return Err(std::io::Error::other("this is not a live session"));
        };
        let line = text.replace('\n', "\\\n");
        let sent = writeln!(input, "{line}").and_then(|()| input.flush());
        if sent.is_err() {
            self.input = None;
        }
        sent
    }

    /// Ends a session's input: it finishes its current turn and is left
    /// open (as when the TUI quits).
    #[cfg(test)]
    pub(crate) fn close_input(&mut self) {
        self.input = None;
    }

    /// Stops a session's current turn (SIGINT to the child, as Ctrl-C in
    /// `duet chat`); the session stays open.
    pub fn interrupt(&self) -> std::io::Result<()> {
        if !self.running() {
            return Err(std::io::Error::other("it is not running"));
        }
        let pid = rustix::process::Pid::from_raw(self.child.id() as i32)
            .ok_or_else(|| std::io::Error::other("no process id"))?;
        rustix::process::kill_process(pid, rustix::process::Signal::Int)
            .map_err(std::io::Error::from)
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

/// What an exit code of `duet chat` means.
pub fn session_outcome(code: i32) -> &'static str {
    match code {
        0 => "ended (left open or closed)",
        1 => "stopped",
        2 => "did not start",
        3 => "stopped: a session budget is spent",
        _ => "ended abnormally",
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

pub(crate) fn draw_dialog(f: &mut Frame, objective: &str, mode: RunMode, session: bool) {
    let [area] = Layout::horizontal([Constraint::Percentage(80)])
        .flex(Flex::Center)
        .areas(f.area());
    let [area] = Layout::vertical([Constraint::Length(12)])
        .flex(Flex::Center)
        .areas(area);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                if session {
                    "first message: "
                } else {
                    "objective: "
                },
                Style::new().add_modifier(Modifier::BOLD),
            ),
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
            .block(Block::bordered().title(if session {
                " start a session (duet chat) "
            } else {
                " start a one-shot run (duet run) "
            })),
        area,
    );
}

pub(crate) fn draw_ack(f: &mut Frame, objective: &str, session: bool) {
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
        Line::from(if session {
            "This is what `duet chat --mode passthrough --no-privacy` acknowledges."
        } else {
            "This is what `duet run --mode passthrough --no-privacy` acknowledges."
        }),
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
        assert_eq!(
            run_id_in("session 20260925-100000-abcdef (Passthrough)\n"),
            Some("20260925-100000-abcdef".into())
        );
        assert_eq!(run_id_in("running tests\n"), None);
        assert_eq!(RunMode::Hybrid.step(-1), RunMode::Passthrough);
        assert_eq!(RunMode::Passthrough.step(1), RunMode::Hybrid);
    }
}
