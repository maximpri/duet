// SPDX-License-Identifier: GPL-3.0-or-later
//! TUI state and key handling. Rendering lives in `ui` and the screen modules;
//! everything here is plain state so tests can drive it without a terminal.

use crate::Doctor;
use crate::audit::AuditView;
use crate::ip::IpView;
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
    doctor_fn: Doctor,
    /// The last doctor result and whether it included the online checks.
    pub(crate) doctor: Option<(bool, Vec<DoctorLine>)>,
    pub(crate) tester: String,
    pub(crate) ip: IpView,
    pub(crate) audit: AuditView,
    pub(crate) runs: RunView,
    pub quit: bool,
}

impl App {
    pub fn new(paths: Paths, doctor: Doctor) -> anyhow::Result<Self> {
        let cfg = Config::load(&paths.owner, Some(&paths.project))?;
        let mut app = Self {
            paths,
            cfg,
            tab: Tab::Models,
            target: Target::Owner,
            mode: Mode::Normal,
            status: String::new(),
            rows: [0; 7],
            doctor_fn: doctor,
            doctor: None,
            tester: String::new(),
            ip: IpView::default(),
            audit: AuditView::default(),
            runs: RunView::default(),
            quit: false,
        };
        app.enter_tab(Tab::Models);
        Ok(app)
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
            Tab::Run => self.runs.refresh(&ws),
            _ => {}
        }
    }

    /// Periodic refresh: the Run screen follows its run as it grows.
    pub fn tick(&mut self) {
        if self.tab == Tab::Run {
            self.runs.refresh(&self.paths.workspace);
        }
    }

    fn run_doctor(&mut self, online: bool) {
        let checks = (self.doctor_fn)(online);
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
            self.quit = true;
            return;
        }
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
                    self.status = format!("not applied: {} unchanged", p.key);
                }
                _ => self.mode = Mode::Confirm(p),
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
            Tab::Run => self.runs.scroll_by(d),
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
            (Tab::Sensitivity, KeyCode::Char('t')) => self.mode = Mode::Tester,
            (Tab::Ip, KeyCode::Enter | KeyCode::Right | KeyCode::Left) => self.ip.toggle(),
            (Tab::Ip, KeyCode::Char('i')) => self.mark("ip.interface_only"),
            (Tab::Ip, KeyCode::Char('s')) => self.mark("ip.sealed"),
            (Tab::Ip, KeyCode::Char('u')) => self.unmark(),
            (Tab::Audit, KeyCode::Enter) => self.audit.focus_records = true,
            (Tab::Audit, KeyCode::Esc) => self.audit.focus_records = false,
            (Tab::Audit, KeyCode::Char('v')) => self.audit.verify(&self.paths),
            (Tab::Audit, KeyCode::Char('r')) => self.audit.refresh(&self.paths.workspace),
            (Tab::Run, KeyCode::Char('[')) => self.runs.select_by(-1, &self.paths.workspace),
            (Tab::Run, KeyCode::Char(']')) => self.runs.select_by(1, &self.paths.workspace),
            (Tab::Run, KeyCode::Char('f')) => self.runs.follow = !self.runs.follow,
            _ => {}
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
            Err(e) => self.status = format!("refused: {e}"),
            Ok(p) if p.old == p.new => self.status = format!("{} unchanged", p.key),
            Ok(p) if p.weakens.is_some() => self.mode = Mode::Confirm(p),
            Ok(p) => self.commit(p, false),
        }
    }

    fn commit(&mut self, p: Proposal, confirmed: bool) {
        let result = self
            .cfg
            .apply(p.target, p.key, p.new.clone(), confirmed)
            .map_err(anyhow::Error::from)
            .and_then(|change| {
                record_config_change(&self.paths.config_audit(), &change, p.target, confirmed)?;
                Ok(())
            });
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
