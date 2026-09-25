// SPDX-License-Identifier: GPL-3.0-or-later
//! TUI state and key handling. Rendering lives in `ui` and the screen modules;
//! everything here is plain state so tests can drive it without a terminal.

use crate::Services;
use crate::audit::AuditView;
use crate::data::PurgePlan;
use crate::ip::IpView;
use crate::launch::{Launched, RunMode};
use crate::models::{Job, ModelsView, Panel};
use crate::runs::RunView;
use crate::settings;
use duet_boundary::audit::record_config_change;
use duet_boundary::policy::Policy;
use duet_config::{Config, Kind, Proposal, Setting, Target, parse_value};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::path::PathBuf;
use toml::Value;

/// Where the TUI reads and writes.
#[derive(Debug, Clone)]
pub struct Paths {
    /// The workspace (canonical, as `duet` resolves it).
    pub workspace: PathBuf,
    /// The owner's config file.
    pub owner: PathBuf,
    /// The project's `.duet/config.toml`.
    pub project: PathBuf,
    /// The owner's state directory (config audit log, audit anchors).
    pub state: PathBuf,
}

impl Paths {
    /// The paths `duet` itself uses for `workspace`.
    pub fn for_workspace(workspace: PathBuf) -> Self {
        Self {
            project: workspace.join(".duet/config.toml"),
            workspace,
            owner: duet_config::owner_config_path(),
            state: duet_config::owner_state_dir(),
        }
    }

    pub fn config_audit(&self) -> PathBuf {
        duet_config::config_audit_log(&self.state)
    }
}

/// One `duet doctor` check, as the TUI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorLine {
    pub status: String,
    pub name: String,
    pub detail: String,
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Models,
    Sensitivity,
    Ip,
    Limits,
    Data,
    Audit,
    Run,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Models,
        Tab::Sensitivity,
        Tab::Ip,
        Tab::Limits,
        Tab::Data,
        Tab::Audit,
        Tab::Run,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Models => "Models",
            Tab::Sensitivity => "Sensitivity",
            Tab::Ip => "IP levels",
            Tab::Limits => "Limits",
            Tab::Data => "Data",
            Tab::Audit => "Audit",
            Tab::Run => "Run",
        }
    }

    pub(crate) fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Screens that are a table of registry settings.
    pub(crate) fn is_settings(self) -> bool {
        matches!(
            self,
            Tab::Models | Tab::Sensitivity | Tab::Limits | Tab::Data
        )
    }
}

pub(crate) enum Mode {
    Normal,
    /// Typing a value for `key`; with `append`, one entry to add to a list.
    Edit {
        key: &'static str,
        buffer: String,
        append: bool,
    },
    /// A change that loosens privacy, waiting for an explicit yes.
    Confirm(Proposal),
    /// Typing a path into the sensitivity tester.
    Tester,
    /// Typing sample text into the detector tester.
    Sample,
    /// A purge waiting for an explicit yes.
    ConfirmPurge(PurgePlan),
    /// Asking for a new run's objective (a session's first message) and mode.
    Launch {
        objective: String,
        mode: RunMode,
        session: bool,
    },
    /// A passthrough run or session waiting for the no-privacy acknowledgement.
    AckPassthrough {
        objective: String,
        session: bool,
    },
    /// Typing a message to the live session.
    Message {
        buffer: String,
    },
}

pub struct App {
    pub(crate) paths: Paths,
    pub(crate) cfg: Config,
    pub(crate) tab: Tab,
    /// The file edits are written to.
    pub(crate) target: Target,
    pub(crate) mode: Mode,
    pub(crate) status: String,
    /// Selected row of each settings screen.
    pub(crate) rows: [usize; 7],
    services: Services,
    /// The last doctor result and whether it included the online checks.
    pub(crate) doctor: Option<(bool, Vec<DoctorLine>)>,
    pub(crate) models: ModelsView,
    /// Changes still to propose after the current one (a detected server
    /// sets `local.base_url`, then `local.model`).
    queued: Vec<(&'static str, Value)>,
    pub(crate) tester: String,
    /// Sample text for the detector tester, and whether its panel is shown.
    pub(crate) sample: String,
    pub(crate) sample_panel: bool,
    pub(crate) ip: IpView,
    pub(crate) audit: AuditView,
    pub(crate) runs: RunView,
    /// The run or session started from this TUI, if any.
    pub(crate) launched: Option<Launched>,
    /// Whether `launched` is a session.
    pub(crate) launched_session: bool,
    pub quit: bool,
}

impl App {
    pub fn new(paths: Paths, services: Services) -> anyhow::Result<Self> {
        let cfg = Config::load(&paths.owner, Some(&paths.project))?;
        let mut app = Self {
            paths,
            cfg,
            tab: Tab::Models,
            target: Target::Owner,
            mode: Mode::Normal,
            status: String::new(),
            rows: [0; 7],
            services,
            doctor: None,
            models: ModelsView::default(),
            queued: Vec::new(),
            tester: String::new(),
            sample: String::new(),
            sample_panel: false,
            ip: IpView::default(),
            audit: AuditView::default(),
            runs: RunView::default(),
            launched: None,
            launched_session: false,
            quit: false,
        };
        app.enter_tab(Tab::Models);
        Ok(app)
    }

    #[cfg(test)]
    pub(crate) fn services_mut(&mut self) -> &mut Services {
        &mut self.services
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub(crate) fn enter_tab(&mut self, tab: Tab) {
        self.tab = tab;
        let ws = self.paths.workspace.clone();
        match tab {
            Tab::Models if self.doctor.is_none() => self.run_doctor(false),
            Tab::Ip => self.ip.load(&ws),
            Tab::Audit => self.audit.refresh(&ws),
            Tab::Run => {
                let policy = self.policy();
                self.runs.refresh(&ws, &policy)
            }
            _ => {}
        }
    }

    /// Periodic refresh: the Run screen follows its run as it grows, and
    /// finished background jobs are collected.
    pub fn tick(&mut self) {
        self.poll_jobs();
        if self.tab == Tab::Run {
            let policy = self.policy();
            self.runs.refresh(&self.paths.workspace, &policy);
        }
    }

    /// Whether a background job (detection, cache probe) is still running.
    pub fn busy(&self) -> bool {
        self.models.busy()
    }

    /// Whether the run started from this TUI is still running.
    pub fn launched_running(&self) -> bool {
        self.launched.as_ref().is_some_and(Launched::running)
    }

    pub(crate) fn poll_jobs(&mut self) {
        for line in self.models.poll() {
            self.status = line;
        }
        let Some(l) = self.launched.as_mut() else {
            return;
        };
        let had_id = l.run_id.is_some();
        if !l.poll() {
            return;
        }
        let (id, exit, last) = (l.run_id.clone(), l.exit, l.last_line());
        if let (Some(id), false) = (&id, had_id) {
            let policy = self.policy();
            self.tab = Tab::Run;
            self.runs.show(id, &self.paths.workspace, &policy);
        }
        let (what, command) = if self.launched_session {
            ("session", "duet chat")
        } else {
            ("run", "duet run")
        };
        if let (Some(id), false) = (&id, had_id) {
            self.status = format!("{what} {id} started; following it");
        }
        if let Some(code) = exit {
            if matches!(self.mode, Mode::Message { .. }) {
                self.mode = Mode::Normal;
            }
            let outcome = if self.launched_session {
                crate::launch::session_outcome(code)
            } else {
                crate::launch::outcome(code)
            };
            self.status = match &id {
                Some(id) => format!("{what} {id} {outcome} (exit code {code})"),
                None => {
                    format!("{command} exited with code {code} before a {what} started: {last}")
                }
            };
        }
    }

    /// Whether a run or session may be started here now; says why not.
    fn may_start(&mut self, session: bool) -> bool {
        if self.launched_running() {
            self.status =
                "a run or session started here is still running; one per workspace".into();
            return false;
        }
        if self.services.duet.is_none() {
            self.status = "starting runs is not available here".into();
            return false;
        }
        let approve = self.cfg.str("oversight.approve").unwrap_or_default();
        if approve != "off" {
            self.status = if session {
                format!(
                    "oversight.approve is {approve}: approvals need a terminal, so use `duet chat`"
                )
            } else {
                format!(
                    "oversight.approve is {approve}: approvals need a terminal, so start this run with `duet run`"
                )
            };
            return false;
        }
        true
    }

    fn start_launch(&mut self, session: bool) {
        if self.may_start(session) {
            self.mode = Mode::Launch {
                objective: String::new(),
                mode: RunMode::Hybrid,
                session,
            };
        }
    }

    fn launch(&mut self, objective: &str, mode: RunMode, acknowledged: bool, session: bool) {
        let Some(duet) = self.services.duet.clone() else {
            return;
        };
        let ws = &self.paths.workspace;
        let (started, command) = if session {
            (
                crate::launch::spawn_session(&duet, ws, objective, mode, acknowledged),
                "duet chat",
            )
        } else {
            (
                crate::launch::spawn(&duet, ws, objective, mode, acknowledged),
                "duet run",
            )
        };
        match started {
            Ok(l) => {
                self.status = format!(
                    "starting {command} ({}); output in {}",
                    mode.arg(),
                    l.log.display()
                );
                self.launched = Some(l);
                self.launched_session = session;
                self.enter_tab(Tab::Run);
            }
            Err(e) => self.status = format!("could not start {command}: {e}"),
        }
    }

    /// The live session, when it is the run the Run view shows.
    pub(crate) fn live_session(&self) -> Option<&Launched> {
        let l = self.launched.as_ref()?;
        (self.launched_session
            && l.takes_messages()
            && l.run_id.is_some()
            && l.run_id.as_deref() == self.runs.selected_id())
        .then_some(l)
    }

    /// `r`: continues the selected session (`duet chat --resume`).
    fn resume_selected(&mut self) {
        if self.live_session().is_some() {
            self.mode = Mode::Message {
                buffer: String::new(),
            };
            return;
        }
        let Some(id) = self.runs.selected_id().map(str::to_owned) else {
            self.status = "no session to resume".into();
            return;
        };
        match &self.runs.session {
            None => {
                self.status = format!("{id} is a one-shot run; only sessions take messages");
                return;
            }
            Some(s) if s.closed => {
                self.status = format!("session {id} was closed; n starts a new one");
                return;
            }
            Some(_) => {}
        }
        if !self.may_start(true) {
            return;
        }
        let Some(duet) = self.services.duet.clone() else {
            return;
        };
        match crate::launch::resume_session(&duet, &self.paths.workspace, &id) {
            Ok(mut l) => {
                // The id is known: the view stays on it.
                l.run_id = Some(id.clone());
                self.launched = Some(l);
                self.launched_session = true;
                self.status = format!("session {id} resumed; type your message");
                self.mode = Mode::Message {
                    buffer: String::new(),
                };
            }
            Err(e) => self.status = format!("could not resume session {id}: {e}"),
        }
    }

    /// Sends the live session a message, or handles a command the TUI shows
    /// itself (`/status`; `/diff` is the side panel).
    fn send_message(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match text {
            "/status" => {
                self.status = match &self.runs.session {
                    Some(s) => format!(
                        "{} turn(s), {} frontier request(s), ${:.4}{}",
                        s.turns,
                        s.requests,
                        s.cost_usd,
                        if s.working { "; duet is working" } else { "" }
                    ),
                    None => "no session".into(),
                };
                return;
            }
            "/diff" => {
                self.status = "the changed files and their diffs are in the side panel (→)".into();
                return;
            }
            _ => {}
        }
        let live = self.launched_session;
        let Some(l) = self.launched.as_mut().filter(|_| live) else {
            self.status = "the session is not running here; r resumes it".into();
            return;
        };
        self.status = match l.send(text) {
            Ok(()) if text.starts_with('/') && !text.starts_with("//") => format!("sent {text}"),
            Ok(()) => "sent; duet works on it (the reply appears here)".into(),
            Err(e) => format!("could not send: {e}"),
        };
    }

    fn interrupt_session(&mut self) {
        let Some(l) = self.live_session() else {
            self.status = "no live session here to interrupt".into();
            return;
        };
        self.status = match l.interrupt() {
            Ok(()) => "interrupt sent: duet stops this turn; the session stays open".into(),
            Err(e) => format!("could not interrupt: {e}"),
        };
    }

    fn run_doctor(&mut self, online: bool) {
        self.models.panel = Panel::Doctor;
        let checks = (self.services.doctor)(online);
        self.status = format!(
            "doctor ({}): {} check(s)",
            if online { "online" } else { "offline" },
            checks.len()
        );
        self.doctor = Some((online, checks));
    }

    /// The policy fields the tester and the IP screen evaluate, from the
    /// effective configuration (the engine's own matching functions decide).
    pub(crate) fn policy(&self) -> Policy {
        let list = |k: &str| self.cfg.list(k).unwrap_or_default();
        Policy {
            sensitive_globs: list("sensitivity.globs"),
            protected_paths: list("sensitivity.protected_paths"),
            secret_sinks: list("sensitivity.secret_sinks"),
            raw_ok_commands: list("sensitivity.raw_ok_commands"),
            custom_patterns: list("sensitivity.custom_patterns"),
            interface_only: list("ip.interface_only"),
            sealed: list("ip.sealed"),
            ..Policy::default()
        }
    }

    pub(crate) fn selected_setting(&self) -> Option<&'static Setting> {
        settings::keys(self.tab)
            .get(self.rows[self.tab.index()])
            .copied()
    }

    pub fn key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
            // While typing to a session, Ctrl-C stops its turn (as in `duet chat`).
            if matches!(self.mode, Mode::Message { .. }) {
                self.interrupt_session();
            } else {
                self.quit = true;
            }
            return;
        }
        self.poll_jobs();
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.normal_key(code),
            Mode::Edit {
                key,
                mut buffer,
                append,
            } => match code {
                KeyCode::Esc => self.status = format!("edit cancelled; {key} unchanged"),
                KeyCode::Enter => self.submit_text(key, &buffer, append),
                KeyCode::Backspace | KeyCode::Char(_) => {
                    match code {
                        KeyCode::Char(c) => buffer.push(c),
                        _ => {
                            buffer.pop();
                        }
                    }
                    self.mode = Mode::Edit {
                        key,
                        buffer,
                        append,
                    };
                }
                _ => {
                    self.mode = Mode::Edit {
                        key,
                        buffer,
                        append,
                    }
                }
            },
            Mode::Confirm(p) => match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.commit(p, true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.queued.clear();
                    self.status = format!("not applied: {} unchanged", p.key);
                }
                _ => self.mode = Mode::Confirm(p),
            },
            Mode::ConfirmPurge(plan) => match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.status = crate::data::execute(&self.paths.workspace, &plan);
                    self.runs.forget();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.status = "purge cancelled; nothing deleted".into();
                }
                _ => self.mode = Mode::ConfirmPurge(plan),
            },
            Mode::Launch {
                mut objective,
                mode,
                session,
            } => match code {
                KeyCode::Esc => {
                    self.status = if session {
                        "no session started".into()
                    } else {
                        "no run started".into()
                    }
                }
                KeyCode::Enter if objective.trim().is_empty() => {
                    self.status = if session {
                        "type the first message first".into()
                    } else {
                        "type the objective first".into()
                    };
                    self.mode = Mode::Launch {
                        objective,
                        mode,
                        session,
                    };
                }
                KeyCode::Enter if mode == RunMode::Passthrough => {
                    self.mode = Mode::AckPassthrough { objective, session }
                }
                KeyCode::Enter => self.launch(objective.trim(), mode, false, session),
                KeyCode::Up | KeyCode::Down => {
                    let d = if code == KeyCode::Up { -1 } else { 1 };
                    self.mode = Mode::Launch {
                        objective,
                        mode: mode.step(d),
                        session,
                    };
                }
                KeyCode::Backspace | KeyCode::Char(_) => {
                    match code {
                        KeyCode::Char(c) => objective.push(c),
                        _ => {
                            objective.pop();
                        }
                    }
                    self.mode = Mode::Launch {
                        objective,
                        mode,
                        session,
                    };
                }
                _ => {
                    self.mode = Mode::Launch {
                        objective,
                        mode,
                        session,
                    }
                }
            },
            Mode::AckPassthrough { objective, session } => match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.launch(objective.trim(), RunMode::Passthrough, true, session)
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.status = "nothing started; the privacy boundary stays on".into();
                }
                _ => self.mode = Mode::AckPassthrough { objective, session },
            },
            Mode::Message { mut buffer } => match code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    self.send_message(&buffer);
                    if self.live_session().is_some() {
                        self.mode = Mode::Message {
                            buffer: String::new(),
                        };
                    }
                }
                KeyCode::Backspace | KeyCode::Char(_) => {
                    match code {
                        KeyCode::Char(c) => buffer.push(c),
                        _ => {
                            buffer.pop();
                        }
                    }
                    self.mode = Mode::Message { buffer };
                }
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    self.runs.scroll_by(match code {
                        KeyCode::Up => -1,
                        KeyCode::Down => 1,
                        KeyCode::PageUp => -10,
                        _ => 10,
                    });
                    self.mode = Mode::Message { buffer };
                }
                _ => self.mode = Mode::Message { buffer },
            },
            Mode::Sample => match code {
                KeyCode::Esc | KeyCode::Enter => {}
                KeyCode::Backspace => {
                    self.sample.pop();
                    self.mode = Mode::Sample;
                }
                KeyCode::Char(c) => {
                    self.sample.push(c);
                    self.mode = Mode::Sample;
                }
                _ => self.mode = Mode::Sample,
            },
            Mode::Tester => match code {
                KeyCode::Esc | KeyCode::Enter => {}
                KeyCode::Backspace => {
                    self.tester.pop();
                    self.mode = Mode::Tester;
                }
                KeyCode::Char(c) => {
                    self.tester.push(c);
                    self.mode = Mode::Tester;
                }
                _ => self.mode = Mode::Tester,
            },
        }
    }

    fn normal_key(&mut self, code: KeyCode) {
        let i = self.tab.index();
        match code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Tab => self.enter_tab(Tab::ALL[(i + 1) % Tab::ALL.len()]),
            KeyCode::BackTab => self.enter_tab(Tab::ALL[(i + Tab::ALL.len() - 1) % Tab::ALL.len()]),
            KeyCode::Char(c @ '1'..='7') => self.enter_tab(Tab::ALL[c as usize - '1' as usize]),
            KeyCode::Char('p') => {
                self.target = match self.target {
                    Target::Owner => Target::Project,
                    Target::Project => Target::Owner,
                };
                self.status = match self.target {
                    Target::Owner => "edits now go to the owner config".into(),
                    Target::Project => {
                        "edits now go to the project config (project keys only, tighten only)"
                            .into()
                    }
                };
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::PageDown => self.move_by(10),
            _ => self.tab_key(code),
        }
    }

    fn move_by(&mut self, d: isize) {
        let step = |cur: usize, len: usize| cur.saturating_add_signed(d).min(len.saturating_sub(1));
        match self.tab {
            Tab::Ip => self.ip.selected = step(self.ip.selected, self.ip.rows().len()),
            Tab::Audit => self.audit.move_by(d, &self.paths.workspace),
            Tab::Run => self.runs.move_by(d),
            tab => {
                let i = tab.index();
                self.rows[i] = step(self.rows[i], settings::keys(tab).len());
            }
        }
    }

    fn tab_key(&mut self, code: KeyCode) {
        match (self.tab, code) {
            (t, KeyCode::Enter) if t.is_settings() => self.edit_selected(),
            (t, KeyCode::Char('a')) if t.is_settings() => match self.selected_setting() {
                Some(s) if matches!(s.kind, Kind::List | Kind::Patterns) => {
                    self.mode = Mode::Edit {
                        key: s.key,
                        buffer: String::new(),
                        append: true,
                    }
                }
                _ => self.status = "a adds an entry to a list setting".into(),
            },
            (Tab::Models, KeyCode::Char('d')) => self.run_doctor(false),
            (Tab::Models, KeyCode::Char('o')) => self.run_doctor(true),
            (Tab::Models, KeyCode::Char('l')) => self.detect(),
            (Tab::Models, KeyCode::Char('c')) => self.probe_cache(),
            (Tab::Models, KeyCode::Char('[')) => self.pick_by(-1),
            (Tab::Models, KeyCode::Char(']')) => self.pick_by(1),
            (Tab::Models, KeyCode::Char('u')) => self.use_picked(),
            (Tab::Sensitivity, KeyCode::Char('t')) => {
                self.sample_panel = false;
                self.mode = Mode::Tester;
            }
            (Tab::Sensitivity, KeyCode::Char('s')) => {
                self.sample_panel = true;
                self.mode = Mode::Sample;
            }
            (Tab::Data, KeyCode::Char('x')) => {
                let days = self.cfg.int("data.retention_days").unwrap_or(14);
                self.plan_purge(duet_agent::purge::Scope::OlderThan { days });
            }
            (Tab::Data, KeyCode::Char('X')) => self.plan_purge(duet_agent::purge::Scope::All),
            (Tab::Ip, KeyCode::Enter | KeyCode::Right | KeyCode::Left) => self.ip.toggle(),
            (Tab::Ip, KeyCode::Char('i')) => self.mark("ip.interface_only"),
            (Tab::Ip, KeyCode::Char('s')) => self.mark("ip.sealed"),
            (Tab::Ip, KeyCode::Char('u')) => self.unmark(),
            (Tab::Audit, KeyCode::Enter) => self.audit.focus_records = true,
            (Tab::Audit, KeyCode::Esc) => self.audit.focus_records = false,
            (Tab::Audit, KeyCode::Char('v')) => self.audit.verify(&self.paths),
            (Tab::Audit, KeyCode::Char('r')) => self.audit.refresh(&self.paths.workspace),
            (Tab::Run, KeyCode::Char('[')) => {
                let policy = self.policy();
                self.runs.select_by(-1, &self.paths.workspace, &policy)
            }
            (Tab::Run, KeyCode::Char(']')) => {
                let policy = self.policy();
                self.runs.select_by(1, &self.paths.workspace, &policy)
            }
            (Tab::Run, KeyCode::Char('f')) => {
                self.runs.follow = !self.runs.follow;
                self.tick();
            }
            (Tab::Run, KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l')) => {
                self.runs.toggle_focus()
            }
            (Tab::Run, KeyCode::Char('n')) => self.start_launch(true),
            (Tab::Run, KeyCode::Char('o')) => self.start_launch(false),
            (Tab::Run, KeyCode::Char('r')) => self.resume_selected(),
            (Tab::Run, KeyCode::Char('i') | KeyCode::Enter) => {
                if self.live_session().is_some() {
                    self.mode = Mode::Message {
                        buffer: String::new(),
                    };
                } else if self.runs.session.is_some() {
                    self.status = "this session is not running here; r resumes it".into();
                }
            }
            (Tab::Run, KeyCode::Char('x')) => self.interrupt_session(),
            (Tab::Run, KeyCode::Char('s')) => {
                if self.live_session().is_some() {
                    self.send_message("/stop");
                } else {
                    self.status = "no live session here to stop".into();
                }
            }
            (Tab::Run, KeyCode::Char('J')) => self.runs.diff_by(1),
            (Tab::Run, KeyCode::Char('K')) => self.runs.diff_by(-1),
            _ => {}
        }
    }

    fn detect(&mut self) {
        self.models.panel = Panel::Servers;
        if self.models.detecting.is_some() {
            return;
        }
        let detect = self.services.detect.clone();
        self.models.detecting = Some(Job::spawn(move || detect()));
        self.status = "looking for local servers on 127.0.0.1 (preset ports only)".into();
    }

    fn probe_cache(&mut self) {
        self.models.panel = Panel::Cache;
        if self.models.probing.is_some() {
            return;
        }
        let probe = self.services.cache_probe.clone();
        self.models.probing = Some(Job::spawn(move || probe()));
        self.status = "cache probe: two identical short requests to the local model".into();
    }

    fn pick_by(&mut self, d: isize) {
        self.models.panel = Panel::Servers;
        let n = self.models.choices().len();
        self.models.pick = self
            .models
            .pick
            .saturating_add_signed(d)
            .min(n.saturating_sub(1));
    }

    /// Proposes the picked server and model; changing the endpoint loosens
    /// privacy, so it goes through the confirmation like any edit.
    fn use_picked(&mut self) {
        let Some((url, model)) = self.models.choices().get(self.models.pick).cloned() else {
            self.status = "l detects local servers first".into();
            return;
        };
        if self.target != Target::Owner {
            self.status =
                "the local endpoint is owner-only: p switches edits to the owner config".into();
            return;
        }
        self.submit_all(vec![
            ("local.base_url", Value::String(url)),
            ("local.model", Value::String(model)),
        ]);
    }

    fn plan_purge(&mut self, scope: duet_agent::purge::Scope) {
        let plan = PurgePlan::new(&self.paths.workspace, scope);
        if plan.runs.is_empty() {
            self.status = "nothing to purge".into();
        } else {
            self.mode = Mode::ConfirmPurge(plan);
        }
    }

    /// Proposes `changes` one after another; a refusal or a cancelled
    /// confirmation drops the rest.
    fn submit_all(&mut self, mut changes: Vec<(&'static str, Value)>) {
        if changes.is_empty() {
            return;
        }
        let (key, value) = changes.remove(0);
        self.queued = changes;
        self.submit(key, value);
    }

    fn next_queued(&mut self) {
        let rest = std::mem::take(&mut self.queued);
        if matches!(self.mode, Mode::Normal) {
            self.submit_all(rest);
        }
    }

    fn edit_selected(&mut self) {
        let Some(s) = self.selected_setting() else {
            return;
        };
        let Ok(cur) = self.cfg.value(s.key).cloned() else {
            return;
        };
        match s.kind {
            Kind::Bool => self.submit(s.key, Value::Boolean(!cur.as_bool().unwrap_or(false))),
            Kind::Choice(opts) => {
                let next = opts
                    .iter()
                    .position(|o| Some(*o) == cur.as_str())
                    .map_or(0, |i| (i + 1) % opts.len());
                self.submit(s.key, Value::String(opts[next].into()));
            }
            Kind::Str => {
                self.mode = Mode::Edit {
                    key: s.key,
                    buffer: cur.as_str().unwrap_or_default().into(),
                    append: false,
                }
            }
            _ => {
                self.mode = Mode::Edit {
                    key: s.key,
                    buffer: cur.to_string(),
                    append: false,
                }
            }
        }
    }

    fn submit_text(&mut self, key: &'static str, buffer: &str, append: bool) {
        let value = if append {
            let entry = buffer.trim();
            let mut list = self.cfg.list(key).unwrap_or_default();
            if entry.is_empty() || list.iter().any(|e| e == entry) {
                self.status = format!("{key} unchanged");
                return;
            }
            list.push(entry.into());
            Value::Array(list.into_iter().map(Value::String).collect())
        } else if duet_config::setting(key).is_some_and(|s| s.kind == Kind::Str) {
            Value::String(buffer.into())
        } else {
            match parse_value(buffer) {
                Ok(v) => v,
                Err(e) => {
                    self.status = format!("refused: {e}");
                    return;
                }
            }
        };
        self.submit(key, value);
    }

    /// Checks a change through the registry rules: refused, applied directly,
    /// or held for confirmation when it loosens privacy.
    pub(crate) fn submit(&mut self, key: &str, value: Value) {
        match self.cfg.propose(self.target, key, value) {
            Err(e) => {
                self.queued.clear();
                self.status = format!("refused: {e}");
            }
            Ok(p) if p.old == p.new => {
                self.status = format!("{} unchanged", p.key);
                self.next_queued();
            }
            Ok(p) if p.weakens.is_some() => self.mode = Mode::Confirm(p),
            Ok(p) => self.commit(p, false),
        }
    }

    fn commit(&mut self, p: Proposal, confirmed: bool) {
        let result = self
            .cfg
            .apply(p.target, &p.key, p.new.clone(), confirmed)
            .map_err(anyhow::Error::from)
            .and_then(|change| {
                record_config_change(&self.paths.config_audit(), &change, p.target, confirmed)?;
                Ok(())
            });
        let applied = result.is_ok();
        self.status = match result {
            Ok(()) => format!(
                "{} = {} written to the {} config{}; recorded in the config audit log",
                p.key,
                p.new,
                p.target.as_str(),
                if confirmed { " (confirmed)" } else { "" }
            ),
            Err(e) => format!("not applied: {e}"),
        };
        match Config::load(&self.paths.owner, Some(&self.paths.project)) {
            Ok(c) => self.cfg = c,
            Err(e) => self
                .status
                .push_str(&format!(" (reloading the config failed: {e})")),
        }
        if applied {
            self.next_queued();
        } else {
            self.queued.clear();
        }
    }

    fn mark(&mut self, list: &'static str) {
        let Some(entry) = self.ip.selected_entry() else {
            return;
        };
        let mut v = self.cfg.list(list).unwrap_or_default();
        if v.contains(&entry) {
            self.status = format!("{entry} is already in {list}");
            return;
        }
        v.push(entry);
        self.submit(
            list,
            Value::Array(v.into_iter().map(Value::String).collect()),
        );
    }

    fn unmark(&mut self) {
        let Some(entry) = self.ip.selected_entry() else {
            return;
        };
        for list in ["ip.sealed", "ip.interface_only"] {
            let mut v = self.cfg.list(list).unwrap_or_default();
            if let Some(i) = v.iter().position(|e| *e == entry) {
                v.remove(i);
                self.submit(
                    list,
                    Value::Array(v.into_iter().map(Value::String).collect()),
                );
                return;
            }
        }
        self.status = format!(
            "{entry} has no entry of its own in ip.sealed or ip.interface_only (a broader pattern may still cover it)"
        );
    }
}
