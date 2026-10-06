// SPDX-License-Identifier: GPL-3.0-or-later
//! The terminal approver for `oversight.approve`: asks the operator on the
//! controlling terminal before a risky action. A run with approval on and no
//! terminal refuses to start (fail closed). In an interactive `declass` session,
//! commits (`git.commit = "ask"`) are asked in the conversation even with
//! approval off.

use anyhow::Result;
use declass_agent::git_tools::CommitPolicy;
use declass_agent::oversight::{Action, Risk};
use declass_agent::{ApproveMode, Approver, Oversight};
use declass_config::Config;
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

/// What the operator is asked to approve (without the final question). A
/// commit shows its files and message, under the setting that asks for it.
pub(crate) fn describe(mode: ApproveMode, action: &Action) -> String {
    let commit = action.risk == Risk::GitCommit;
    let setting = if commit && mode != ApproveMode::All {
        "git.commit = ask".to_owned()
    } else {
        format!("oversight.approve = {}", mode.as_str())
    };
    let mut prompt = format!(
        "\ndeclass: approval needed ({setting})\n  {}: {}\n",
        action.tool,
        action.risk.describe()
    );
    if let Some(p) = &action.path {
        prompt.push_str(&format!("  {}: {p}", if commit { "files" } else { "path" }));
        if let Some(b) = action.bytes {
            prompt.push_str(&format!(" ({b} bytes)"));
        }
        prompt.push('\n');
    }
    match &action.command {
        Some(m) if commit => prompt.push_str(&format!(
            "  message: {}\n",
            m.trim_end().replace('\n', "\n           ")
        )),
        Some(c) => prompt.push_str(&format!("  command: {c}\n")),
        None => {}
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
fn git_commit(cfg: &Config) -> Result<declass_agent::git_tools::CommitPolicy> {
    cfg.str("git.commit")?.parse().map_err(anyhow::Error::msg)
}

/// `git.author` (`Name <email>`), when set.
pub fn git_author(cfg: &Config) -> Result<Option<declass_git::Identity>> {
    let text = cfg.str("git.author")?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    declass_git::Identity::parse(&text)
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
config: declass config set oversight.approve '\"off\"' --confirm",
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
    session_oversight_at(cfg, std::io::stdin().is_terminal(), ask)
}

/// [`session_oversight`], `tty` saying whether standard input is a terminal.
/// With approval off, an interactive session still asks before each commit
/// when `git.commit = "ask"`: the operator is there to answer, so
/// `git_commit` is offered (nothing else is asked). Without a terminal (the
/// TUI's pipe) it is not offered, as in a one-shot run.
fn session_oversight_at(
    cfg: &Config,
    tty: bool,
    ask: impl FnOnce(ApproveMode) -> Arc<dyn Approver>,
) -> Result<Oversight> {
    let mode = mode(cfg)?;
    let git_commit = git_commit(cfg)?;
    if mode == ApproveMode::Off {
        return Ok(Oversight {
            git_commit,
            approver: (tty && git_commit == CommitPolicy::Ask).then(|| ask(mode)),
            ..Oversight::default()
        });
    }
    if !tty {
        anyhow::bail!(
            "oversight.approve = {} asks the operator at a terminal before risky actions, but \
standard input is not a terminal; the session was not started. Run `declass` from an interactive \
terminal, or turn approval off in the owner config: declass config set oversight.approve '\"off\"' --confirm",
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

    struct Never;

    impl Approver for Never {
        fn approve(&self, _: &Action) -> bool {
            false
        }
    }

    fn config(owner: &str) -> (tempfile::TempDir, Config) {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        std::fs::write(&path, owner).unwrap();
        let cfg = Config::load(&path, None).unwrap();
        (d, cfg)
    }

    #[test]
    fn an_interactive_session_asks_for_commits_with_approval_off() {
        let never = |_: ApproveMode| -> Arc<dyn Approver> { Arc::new(Never) };
        let asks = |owner: &str, tty: bool| {
            let (_d, cfg) = config(owner);
            let o = session_oversight_at(&cfg, tty, never).unwrap();
            (o.mode, o.approver.is_some(), o.git_commit)
        };
        use ApproveMode::Off;
        // The defaults: approval off, commits asked. At a terminal someone
        // can answer, so there is an approver (and git_commit is offered).
        assert_eq!(asks("", true), (Off, true, CommitPolicy::Ask));
        // Through a pipe nobody can: no approver, git_commit not offered.
        assert_eq!(asks("", false), (Off, false, CommitPolicy::Ask));
        // Nothing to ask for otherwise.
        let allow = "[git]\ncommit = \"allow\"\n";
        assert_eq!(asks(allow, true), (Off, false, CommitPolicy::Allow));
        let off = "[git]\ncommit = \"off\"\n";
        assert_eq!(asks(off, true), (Off, false, CommitPolicy::Off));
        // Approval on without a terminal still refuses the session.
        let (_d, cfg) = config("[oversight]\napprove = \"risky\"\n");
        assert!(session_oversight_at(&cfg, false, never).is_err());
        let o = session_oversight_at(&cfg, true, never).unwrap();
        assert_eq!((o.mode, o.approver.is_some()), (ApproveMode::Risky, true));
    }

    #[test]
    fn a_commit_question_shows_its_files_message_and_setting() {
        let action = Action {
            tool: "git_commit".into(),
            risk: Risk::GitCommit,
            path: Some("src/a.rs, src/b.rs".into()),
            command: Some("Add a\n\nWith tests.".into()),
            bytes: None,
        };
        let shown = describe(ApproveMode::Off, &action);
        assert!(shown.contains("(git.commit = ask)"), "{shown}");
        assert!(shown.contains("  files: src/a.rs, src/b.rs\n"), "{shown}");
        assert!(
            shown.contains("  message: Add a\n           \n           With tests.\n"),
            "{shown}"
        );
        assert!(describe(ApproveMode::All, &action).contains("(oversight.approve = all)"));
    }

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
