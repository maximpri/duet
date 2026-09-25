// SPDX-License-Identifier: GPL-3.0-or-later
//! The terminal approver for `oversight.approve`: asks the operator on the
//! controlling terminal before a risky action. A run with approval on and no
//! terminal refuses to start (fail closed).

use anyhow::Result;
use duet_agent::oversight::Action;
use duet_agent::{ApproveMode, Approver, Oversight};
use duet_config::Config;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::sync::{Arc, Mutex};

struct Terminal {
    mode: ApproveMode,
    tty: Mutex<std::fs::File>,
}

impl Terminal {
    fn ask(&self, action: &Action) -> bool {
        let Ok(mut tty) = self.tty.lock() else {
            return false;
        };
        let mut prompt = describe(self.mode, action);
        prompt.push_str("approve? [y/N] ");
        if tty
            .write_all(prompt.as_bytes())
            .and_then(|()| tty.flush())
            .is_err()
        {
            return false;
        }
        let mut answer = String::new();
        let Ok(reader) = tty.try_clone() else {
            return false;
        };
        if BufReader::new(reader).read_line(&mut answer).is_err() {
            return false;
        }
        is_yes(&answer)
    }
}

/// What the operator is asked to approve (without the final question).
pub(crate) fn describe(mode: ApproveMode, action: &Action) -> String {
    let mut prompt = format!(
        "\nduet: approval needed (oversight.approve = {})\n  {}: {}\n",
        mode.as_str(),
        action.tool,
        action.risk.describe()
    );
    if let Some(p) = &action.path {
        prompt.push_str(&format!("  path: {p}"));
        if let Some(b) = action.bytes {
            prompt.push_str(&format!(" ({b} bytes)"));
        }
        prompt.push('\n');
    }
    if let Some(c) = &action.command {
        prompt.push_str(&format!("  command: {c}\n"));
    }
    prompt
}

/// Only an explicit yes approves.
pub(crate) fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

impl Approver for Terminal {
    fn approve(&self, action: &Action) -> bool {
        // Reading the terminal blocks; keep the runtime's other tasks running.
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => {
                tokio::task::block_in_place(|| self.ask(action))
            }
            _ => self.ask(action),
        }
    }
}

/// `git.commit`: whether `git_commit` is offered and asks the operator.
fn git_commit(cfg: &Config) -> Result<duet_agent::git_tools::CommitPolicy> {
    cfg.str("git.commit")?.parse().map_err(anyhow::Error::msg)
}

/// `git.author` (`Name <email>`), when set.
pub fn git_author(cfg: &Config) -> Result<Option<duet_git::Identity>> {
    let text = cfg.str("git.author")?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    duet_git::Identity::parse(&text)
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("git.author must look like `Name <email>`, not {text:?}"))
}

fn mode(cfg: &Config) -> Result<ApproveMode> {
    cfg.str("oversight.approve")?
        .parse()
        .map_err(anyhow::Error::msg)
}

/// With approval on, the operator's terminal: stdin must be a terminal and the
/// controlling terminal must open. Otherwise an error that refuses the run.
fn open_terminal(mode: ApproveMode) -> Result<Option<std::fs::File>> {
    if mode == ApproveMode::Off {
        return Ok(None);
    }
    let refuse = |why: String| {
        anyhow::anyhow!(
            "oversight.approve = {} asks the operator at a terminal before risky actions, but {why}; \
the run was not started. Run it from an interactive terminal, or turn approval off in the owner \
config: duet config set oversight.approve '\"off\"' --confirm",
            mode.as_str()
        )
    };
    if !std::io::stdin().is_terminal() {
        return Err(refuse("standard input is not a terminal".into()));
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(Some)
        .map_err(|e| refuse(format!("the terminal cannot be opened ({e})")))
}

/// A session's oversight: approval questions go through `ask` (the session
/// reads the operator's terminal itself, so it cannot be opened twice). With
/// approval on, standard input must be a terminal; otherwise the session is
/// refused like a run (fail closed).
pub fn session_oversight(
    cfg: &Config,
    ask: impl FnOnce(ApproveMode) -> Arc<dyn Approver>,
) -> Result<Oversight> {
    let mode = mode(cfg)?;
    let git_commit = git_commit(cfg)?;
    if mode == ApproveMode::Off {
        return Ok(Oversight {
            git_commit,
            ..Oversight::default()
        });
    }
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "oversight.approve = {} asks the operator at a terminal before risky actions, but \
standard input is not a terminal; the session was not started. Run `duet chat` from an interactive \
terminal, or turn approval off in the owner config: duet config set oversight.approve '\"off\"' --confirm",
            mode.as_str()
        );
    }
    eprintln!(
        "operator approval on ({}): risky actions wait for y/N here",
        mode.as_str()
    );
    Ok(Oversight {
        mode,
        approver: Some(ask(mode)),
        git_commit,
    })
}

/// Refuses a run that needs approval but has no terminal (fail closed).
pub fn require_terminal(cfg: &Config) -> Result<()> {
    open_terminal(mode(cfg)?).map(drop)
}

/// The run's oversight from `oversight.approve`; refuses the run when
/// approval is on and nobody can be asked.
pub fn oversight(cfg: &Config) -> Result<Oversight> {
    let mode = mode(cfg)?;
    let git_commit = git_commit(cfg)?;
    let Some(tty) = open_terminal(mode)? else {
        return Ok(Oversight {
            git_commit,
            ..Oversight::default()
        });
    };
    eprintln!(
        "operator approval on ({}): risky actions wait for y/N on this terminal",
        mode.as_str()
    );
    Ok(Oversight {
        mode,
        approver: Some(Arc::new(Terminal {
            mode,
            tty: Mutex::new(tty),
        })),
        git_commit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_yes_approves() {
        for yes in ["y", "Y\n", " yes \n", "YES"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yep", "sure", "y es"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }
}
