// SPDX-License-Identifier: GPL-3.0-or-later
//! Private, durable goal state for one session. This module never calls a model.
//! Raw objectives and progress stay in the run directory; prompts must enter
//! the session through its ordinary operator-message privacy boundary.

use anyhow::{Context, Result, bail, ensure};
use duet_agent::TurnEnd;
use duet_fs::pinned::PinnedParent;
use rustix::fs::FileType;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Component, Path};

pub(crate) const DEFAULT_MAX_TURNS: u64 = 20;
const MAX_TURNS: u64 = 1_000;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_HISTORY: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Active,
    Waiting,
    Paused,
    Completed,
    Exhausted,
    Cancelled,
}

impl State {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Exhausted | Self::Cancelled)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Active => "Working",
            Self::Waiting => "Waiting for your answer",
            Self::Paused => "Paused",
            Self::Completed => "Completed",
            Self::Exhausted => "Turn limit reached",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Goal {
    pub objective: String,
    pub state: State,
    /// Turns reserved before a model request, including interrupted attempts.
    pub turns: u64,
    pub max_turns: u64,
    pub summary: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    current: Option<Goal>,
    history: Vec<Goal>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            version: 1,
            current: None,
            history: Vec::new(),
        }
    }
}

pub(crate) struct Store {
    file: PinnedParent,
    saved: Saved,
    recovered: bool,
}

impl Store {
    /// `run_dir` must already exist. Open every path component without links,
    /// retaining the parent handle for subsequent reads and atomic writes.
    /// Loading a crashed active goal pauses it in memory without writing.
    pub(crate) fn load(run_dir: &Path) -> Result<Self> {
        let absolute = if run_dir.is_absolute() {
            run_dir.to_owned()
        } else {
            std::env::current_dir()?.join(run_dir)
        };
        ensure!(
            absolute
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
            "goal directory must not contain parent traversal"
        );
        let relative = absolute.strip_prefix(Path::new("/"))?.join("goals.json");
        let file = PinnedParent::open(Path::new("/"), &relative, false)
            .context("cannot open the session goal directory")?;
        let mut saved = match file.file_type()? {
            None => Saved::default(),
            Some(FileType::RegularFile) => {
                let input = file.open_read().context("cannot open session goals")?;
                ensure!(
                    input.metadata()?.is_file(),
                    "session goals must be a regular file"
                );
                let mut bytes = Vec::new();
                input
                    .take((MAX_FILE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .context("cannot read session goals")?;
                ensure!(
                    bytes.len() <= MAX_FILE_BYTES,
                    "session goal history exceeds 4 MiB"
                );
                serde_json::from_slice::<Saved>(&bytes).context("session goal state is invalid")?
            }
            Some(_) => bail!("session goals must be a regular file, not a link or device"),
        };
        validate(&saved)?;
        let recovered = saved
            .current
            .as_ref()
            .is_some_and(|g| g.state == State::Active);
        if let Some(goal) = &mut saved.current
            && goal.state == State::Active
        {
            goal.state = State::Paused;
            goal.reason =
                Some("Duet stopped while this goal was active. Resume it when ready.".into());
        }
        Ok(Self {
            file,
            saved,
            recovered,
        })
    }

    pub(crate) fn current(&self) -> Option<&Goal> {
        self.saved.current.as_ref()
    }

    pub(crate) fn active(&self) -> bool {
        self.current().is_some_and(|g| g.state == State::Active)
    }

    /// A goal is in progress: working, or waiting for the operator's answer.
    pub(crate) fn running(&self) -> bool {
        self.current()
            .is_some_and(|g| matches!(g.state, State::Active | State::Waiting))
    }

    /// Persist a recovery pause once the caller is ready to resume the session.
    /// Ordinary read-only loads do not change anything on disk.
    pub(crate) fn checkpoint(&mut self) -> Result<()> {
        if self.recovered {
            self.update(|_| Ok(()))?;
        }
        Ok(())
    }

    pub(crate) fn start(&mut self, objective: &str, max_turns: u64) -> Result<()> {
        ensure!(
            !objective.trim().is_empty(),
            "describe what you want Duet to accomplish"
        );
        ensure!(
            objective.len() <= MAX_TEXT_BYTES,
            "goal objective exceeds 64 KiB"
        );
        ensure!(
            (1..=MAX_TURNS).contains(&max_turns),
            "goal turn limit must be between 1 and 1000"
        );
        self.update(|saved| {
            ensure!(
                saved.current.as_ref().is_none_or(|g| g.state.terminal()),
                "a goal is already unfinished; resume or cancel it before starting another"
            );
            if let Some(previous) = saved.current.take() {
                ensure!(
                    saved.history.len() < MAX_HISTORY,
                    "goal history is full; start a new session"
                );
                saved.history.push(previous);
            }
            saved.current = Some(Goal {
                objective: objective.trim().to_owned(),
                state: State::Active,
                turns: 0,
                max_turns,
                summary: None,
                reason: None,
            });
            Ok(())
        })
    }

    pub(crate) fn pause(&mut self, reason: &str) -> Result<()> {
        if self.current().is_none_or(|g| g.state.terminal()) {
            return Ok(());
        }
        self.change_goal(|goal| {
            goal.state = State::Paused;
            goal.reason = Some(bounded(reason));
            Ok(())
        })
    }

    pub(crate) fn resume(&mut self) -> Result<()> {
        self.change_goal(|goal| {
            ensure!(
                !goal.state.terminal(),
                "this goal has already ended; start a new goal"
            );
            ensure!(
                goal.turns < goal.max_turns,
                "this goal has no turns remaining; cancel it to start another"
            );
            goal.state = State::Active;
            goal.reason = None;
            Ok(())
        })
    }

    pub(crate) fn cancel(&mut self) -> Result<()> {
        if self.current().is_none_or(|g| g.state.terminal()) {
            return Ok(());
        }
        self.change_goal(|goal| {
            goal.state = State::Cancelled;
            goal.reason = Some("You cancelled this goal.".into());
            Ok(())
        })
    }

    /// Reserve a turn durably before a request can start. Persistence failures
    /// leave the in-memory counter untouched and must prevent that request.
    pub(crate) fn begin_turn(&mut self) -> Result<()> {
        self.change_goal(|goal| {
            ensure!(
                goal.state == State::Active,
                "resume the goal before continuing"
            );
            ensure!(
                goal.turns < goal.max_turns,
                "this goal has reached its turn limit"
            );
            goal.turns += 1;
            Ok(())
        })
    }

    pub(crate) fn finish_turn(&mut self, end: &TurnEnd) -> Result<()> {
        self.change_goal(|goal| {
            ensure!(
                goal.state == State::Active && goal.turns > 0,
                "no active goal turn to finish"
            );
            match end {
                TurnEnd::Completed { summary } => {
                    goal.state = State::Completed;
                    goal.summary = Some(bounded(summary));
                    goal.reason = None;
                }
                TurnEnd::Replied { message } => {
                    goal.summary = Some(bounded(message));
                    goal.reason = None;
                    if goal.turns == goal.max_turns {
                        goal.state = State::Exhausted;
                        goal.reason =
                            Some("The goal reached its turn limit before completion.".into());
                    }
                }
                TurnEnd::Asked { question, .. } => {
                    goal.state = State::Waiting;
                    goal.reason = Some(bounded(question));
                }
                TurnEnd::Failed { reason } => {
                    goal.state = State::Paused;
                    goal.reason = Some(bounded(reason));
                }
                TurnEnd::BudgetStopped { which } => {
                    goal.state = State::Paused;
                    goal.reason = Some(bounded(&format!("A session limit stopped work: {which}")));
                }
                TurnEnd::Interrupted | TurnEnd::Stopped => {
                    goal.state = State::Paused;
                    goal.reason = Some("You stopped work. Resume the goal when ready.".into());
                }
            }
            Ok(())
        })
    }

    /// Raw local text: the caller must sanitize this as an operator message.
    pub(crate) fn prompt(&self) -> Option<String> {
        let goal = self.current().filter(|g| g.state == State::Active)?;
        let mut prompt = format!(
            "Continue working toward this goal:\n\n{}\n\n{} of {} allowed turns have started. Make concrete progress and verify the result. Use finish only when the objective is achieved and the required checks pass. If you need the operator's input, use ask_operator. A reply alone does not complete the goal.",
            goal.objective, goal.turns, goal.max_turns
        );
        if let Some(summary) = &goal.summary {
            prompt.push_str("\n\nProgress from the previous turn:\n");
            prompt.push_str(summary);
        }
        Some(prompt)
    }

    /// Safe to print to a terminal. This local status may contain private text
    /// and must not be copied into public logs or an unchecked model request.
    pub(crate) fn status_line(&self) -> Option<String> {
        let goal = self.current()?;
        let objective = duet_tui::term::safe(&goal.objective)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut title: String = objective.chars().take(100).collect();
        if objective.chars().count() > 100 {
            title.push('…');
        }
        Some(format!(
            "Goal: {} · {}/{} turns · {title}",
            goal.state.label(),
            goal.turns,
            goal.max_turns
        ))
    }

    pub(crate) fn describe(&self) -> String {
        let Some(goal) = self.current() else {
            return "No goal yet. Start one with /goal <what you want to accomplish>.".into();
        };
        let mut description = format!(
            "Goal: {}\n{}\nTurns: {} of {} used · {} remaining",
            goal.state.label(),
            goal.objective,
            goal.turns,
            goal.max_turns,
            goal.max_turns.saturating_sub(goal.turns)
        );
        if let Some(summary) = &goal.summary {
            description.push_str("\nProgress: ");
            description.push_str(summary);
        }
        if let Some(reason) = &goal.reason {
            description.push('\n');
            description.push_str(reason);
        }
        duet_tui::term::safe(&description)
    }

    fn change_goal(&mut self, change: impl FnOnce(&mut Goal) -> Result<()>) -> Result<()> {
        self.update(|saved| {
            let goal = saved
                .current
                .as_mut()
                .context("there is no goal in this session")?;
            change(goal)
        })
    }

    fn update(&mut self, change: impl FnOnce(&mut Saved) -> Result<()>) -> Result<()> {
        self.update_with(change, |file, bytes| file.write_all(bytes))
    }

    fn update_with(
        &mut self,
        change: impl FnOnce(&mut Saved) -> Result<()>,
        write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
    ) -> Result<()> {
        let mut next = self.saved.clone();
        change(&mut next)?;
        validate(&next)?;
        let bytes = serde_json::to_vec(&next)?;
        ensure!(
            bytes.len() <= MAX_FILE_BYTES,
            "session goal history exceeds 4 MiB; start a new session"
        );
        match self.file.file_type()? {
            None | Some(FileType::RegularFile) => {}
            Some(_) => bail!("session goals must be a regular file, not a link or device"),
        }
        let temporary = OsString::from(format!(".goals-{}.tmp", uuid::Uuid::new_v4()));
        let written = (|| -> Result<()> {
            let mut file = self.file.create_temp(&temporary, 0o600)?;
            write(&mut file, &bytes)?;
            file.sync_all()?;
            self.file.rename_into_place(&temporary)?;
            self.file.sync()?;
            Ok(())
        })();
        if written.is_err() {
            self.file.remove_temp(&temporary);
        }
        written.context("cannot save session goal state")?;
        self.saved = next;
        self.recovered = false;
        Ok(())
    }
}

fn bounded(text: &str) -> String {
    let mut end = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn validate(saved: &Saved) -> Result<()> {
    ensure!(saved.version == 1, "unsupported session goal state version");
    ensure!(
        saved.history.len() <= MAX_HISTORY,
        "session goal history has too many entries"
    );
    ensure!(
        saved.history.iter().all(|g| g.state.terminal()),
        "session goal history contains an unfinished goal"
    );
    for goal in saved.current.iter().chain(&saved.history) {
        ensure!(
            !goal.objective.trim().is_empty() && goal.objective.len() <= MAX_TEXT_BYTES,
            "session goal objective is invalid"
        );
        ensure!(
            (1..=MAX_TURNS).contains(&goal.max_turns) && goal.turns <= goal.max_turns,
            "session goal turn counts are invalid"
        );
        ensure!(
            goal.summary
                .iter()
                .chain(&goal.reason)
                .all(|s| s.len() <= MAX_TEXT_BYTES),
            "session goal progress is too large"
        );
        ensure!(
            goal.state != State::Exhausted || goal.turns == goal.max_turns,
            "session goal exhaustion state is invalid"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn directory() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().canonicalize().unwrap();
        (directory, path)
    }

    #[test]
    fn completed_goals_are_preserved_and_only_finish_completes() {
        let (_directory, path) = directory();
        let mut store = Store::load(&path).unwrap();
        assert!(store.current().is_none());
        store
            .start("Improve the parser", DEFAULT_MAX_TURNS)
            .unwrap();
        assert!(store.start("Replace the current goal", 1).is_err());
        store.begin_turn().unwrap();
        store
            .finish_turn(&TurnEnd::Replied {
                message: "Implemented the first change".into(),
            })
            .unwrap();
        assert!(store.active());
        assert!(
            store
                .prompt()
                .unwrap()
                .contains("Implemented the first change")
        );
        store.begin_turn().unwrap();
        store
            .finish_turn(&TurnEnd::Completed {
                summary: "Verified the completed parser".into(),
            })
            .unwrap();
        assert_eq!(store.current().unwrap().state, State::Completed);
        assert!(store.resume().is_err());
        store.start("A second objective", 2).unwrap();
        assert_eq!(store.saved.history.len(), 1);
        assert_eq!(store.saved.history[0].objective, "Improve the parser");
        assert_eq!(store.saved.history[0].turns, 2);
        assert_eq!(
            fs::metadata(path.join("goals.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn questions_stops_errors_and_cancellation_are_not_completion() {
        let (_directory, path) = directory();
        let mut store = Store::load(&path).unwrap();
        store.start("Do the task", 20).unwrap();
        store.begin_turn().unwrap();
        store
            .finish_turn(&TurnEnd::Asked {
                question: "Which format?".into(),
                options: None,
            })
            .unwrap();
        assert_eq!(store.current().unwrap().state, State::Waiting);
        assert!(!store.active());
        assert!(store.prompt().is_none());
        for end in [
            TurnEnd::Interrupted,
            TurnEnd::Stopped,
            TurnEnd::Failed {
                reason: "provider unavailable".into(),
            },
            TurnEnd::BudgetStopped {
                which: "session.frontier_usd".into(),
            },
        ] {
            store.resume().unwrap();
            store.begin_turn().unwrap();
            store.finish_turn(&end).unwrap();
            assert_eq!(store.current().unwrap().state, State::Paused);
        }
        store.cancel().unwrap();
        assert_eq!(store.current().unwrap().state, State::Cancelled);
        assert!(store.resume().is_err());
        store.start("New task", 1).unwrap();
        assert_eq!(store.saved.history[0].state, State::Cancelled);
    }

    #[test]
    fn turn_limit_and_crash_recovery_are_durable_and_bounded() {
        let (_directory, path) = directory();
        let mut store = Store::load(&path).unwrap();
        assert!(store.start("task", 0).is_err());
        assert!(store.start("task", 1001).is_err());
        store.start("task", 2).unwrap();
        store.begin_turn().unwrap();
        let before = fs::read(path.join("goals.json")).unwrap();
        let mut recovered = Store::load(&path).unwrap();
        assert_eq!(
            fs::read(path.join("goals.json")).unwrap(),
            before,
            "loading never writes recovery state"
        );
        assert_eq!(recovered.current().unwrap().state, State::Paused);
        assert_eq!(recovered.current().unwrap().turns, 1);
        recovered.checkpoint().unwrap();
        let checkpoint: Saved =
            serde_json::from_slice(&fs::read(path.join("goals.json")).unwrap()).unwrap();
        assert_eq!(checkpoint.current.unwrap().state, State::Paused);
        recovered.resume().unwrap();
        recovered.begin_turn().unwrap();
        recovered
            .finish_turn(&TurnEnd::Replied {
                message: "Still needs work".into(),
            })
            .unwrap();
        assert_eq!(recovered.current().unwrap().state, State::Exhausted);
        assert!(recovered.begin_turn().is_err());
        assert!(recovered.resume().is_err());
        let reopened = Store::load(&path).unwrap();
        assert_eq!(reopened.current().unwrap().state, State::Exhausted);
        assert_eq!(reopened.current().unwrap().turns, 2);
    }

    #[test]
    fn failed_persistence_never_reserves_a_turn_or_replaces_good_state() {
        let (_directory, path) = directory();
        let mut store = Store::load(&path).unwrap();
        store.start("Keep this objective", 3).unwrap();
        let before = fs::read(path.join("goals.json")).unwrap();
        let result = store.update_with(
            |saved| {
                saved.current.as_mut().unwrap().turns += 1;
                Ok(())
            },
            |file, bytes| {
                file.write_all(&bytes[..17])?;
                Err(std::io::Error::from_raw_os_error(
                    rustix::io::Errno::NOSPC.raw_os_error(),
                ))
            },
        );
        assert!(result.is_err());
        assert_eq!(store.current().unwrap().turns, 0);
        assert_eq!(fs::read(path.join("goals.json")).unwrap(), before);
        assert_eq!(
            fs::read_dir(&path).unwrap().count(),
            1,
            "temporary write cleaned up"
        );
        store.begin_turn().unwrap();
        assert_eq!(store.current().unwrap().turns, 1);
    }

    #[test]
    fn links_and_unbounded_or_invalid_state_are_refused() {
        let (_directory, path) = directory();
        let real = path.join("real");
        fs::create_dir(&real).unwrap();
        let alias = path.join("alias");
        symlink(&real, &alias).unwrap();
        assert!(Store::load(&alias).is_err());
        let target = path.join("target.json");
        fs::write(&target, b"do not touch").unwrap();
        symlink(&target, real.join("goals.json")).unwrap();
        assert!(Store::load(&real).is_err());
        fs::remove_file(real.join("goals.json")).unwrap();
        let mut store = Store::load(&real).unwrap();
        symlink(&target, real.join("goals.json")).unwrap();
        assert!(store.start("task", 1).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"do not touch");
        fs::remove_file(real.join("goals.json")).unwrap();
        fs::write(real.join("goals.json"), vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
        assert!(Store::load(&real).is_err());
        fs::write(
            real.join("goals.json"),
            b"{\"version\":1,\"current\":null,\"history\":[]",
        )
        .unwrap();
        assert!(Store::load(&real).is_err());
    }

    #[test]
    fn private_text_is_preserved_but_terminal_status_has_no_escape_sequences() {
        let (_directory, path) = directory();
        let mut store = Store::load(&path).unwrap();
        let objective = "Keep café private \x1b]52;c;c2VjcmV0\x07";
        store.start(objective, 3).unwrap();
        assert_eq!(store.current().unwrap().objective, objective);
        assert!(!store.describe().contains('\x1b'));
        assert!(!store.describe().contains('\x07'));
        store.pause("Wait\u{202e}secret").unwrap();
        assert!(!store.describe().contains('\u{202e}'));
        let recovered = Store::load(&path).unwrap();
        assert_eq!(recovered.current().unwrap().objective, objective);
        assert_eq!(recovered.current().unwrap().state, State::Paused);
    }

    #[test]
    fn pausing_and_cancelling_without_an_unfinished_goal_are_noops() {
        let (_directory, path) = directory();
        let mut store = Store::load(&path).unwrap();
        store.pause("stop").unwrap();
        store.cancel().unwrap();
        store.checkpoint().unwrap();
        assert!(!path.join("goals.json").exists());
        store.start("Finish this", 1).unwrap();
        store.begin_turn().unwrap();
        store
            .finish_turn(&TurnEnd::Completed {
                summary: "Done".into(),
            })
            .unwrap();
        let before = fs::read(path.join("goals.json")).unwrap();
        store.pause("stop").unwrap();
        store.cancel().unwrap();
        store.checkpoint().unwrap();
        assert_eq!(fs::read(path.join("goals.json")).unwrap(), before);
        assert_eq!(store.current().unwrap().state, State::Completed);
    }
}
