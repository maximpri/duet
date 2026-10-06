// SPDX-License-Identifier: GPL-3.0-or-later
//! Models screen actions beside the settings table: `declass doctor` (offline,
//! or online for connection and context window), loopback detection of local
//! servers with a pick that proposes `local.base_url` / `local.model` through
//! the audited path, and the cache-reuse probe. Detection and the probe run
//! on a worker thread (they wait on the network) and only when asked.

use crate::app::App;
use crate::ui::status_style;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use std::sync::mpsc::{Receiver, TryRecvError};

/// A local server that answered on a loopback port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalServer {
    pub base_url: String,
    /// The identified backend, or a generic description.
    pub backend: String,
    pub models: Vec<String>,
}

/// The result of the cache-reuse probe (two identical requests).
#[derive(Debug, Clone, PartialEq)]
pub struct CacheReport {
    pub base_url: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub first_cached: u64,
    pub second_cached: u64,
    pub first_seconds: f64,
    pub second_seconds: f64,
    /// The server reported no usage.
    pub unreported: bool,
}

/// What the side panel shows: the result of the last action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Panel {
    #[default]
    Doctor,
    Servers,
    Cache,
}

/// A job on a worker thread; polled on every tick and key.
pub(crate) struct Job<T>(Receiver<T>);

impl<T: Send + 'static> Job<T> {
    pub(crate) fn spawn(work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        Self(rx)
    }

    /// `Some` once the job finished (a job that died reports `None` forever,
    /// so the caller also gets `Err(())` then).
    pub(crate) fn poll(&self) -> Option<Result<T, ()>> {
        match self.0.try_recv() {
            Ok(v) => Some(Ok(v)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(())),
        }
    }
}

#[derive(Default)]
pub struct ModelsView {
    pub(crate) panel: Panel,
    pub(crate) servers: Option<Vec<LocalServer>>,
    /// Selected (server, model) choice.
    pub(crate) pick: usize,
    pub(crate) cache: Option<Result<CacheReport, String>>,
    pub(crate) detecting: Option<Job<Vec<LocalServer>>>,
    pub(crate) probing: Option<Job<Result<CacheReport, String>>>,
}

impl ModelsView {
    /// Every (server, model) pair detection found, in order.
    pub fn choices(&self) -> Vec<(String, String)> {
        self.servers
            .iter()
            .flatten()
            .flat_map(|s| s.models.iter().map(|m| (s.base_url.clone(), m.clone())))
            .collect()
    }

    pub fn busy(&self) -> bool {
        self.detecting.is_some() || self.probing.is_some()
    }

    /// Collects finished jobs; returns a status line for each.
    pub(crate) fn poll(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(r) = self.detecting.as_ref().and_then(Job::poll) {
            self.detecting = None;
            let servers = r.unwrap_or_default();
            out.push(format!(
                "detection: {} local server(s) on loopback",
                servers.len()
            ));
            self.servers = Some(servers);
            self.pick = 0;
        }
        if let Some(r) = self.probing.as_ref().and_then(Job::poll) {
            self.probing = None;
            let r = r.unwrap_or_else(|()| Err("the probe stopped unexpectedly".into()));
            out.push(match &r {
                Ok(c) => format!(
                    "cache probe: {} of {} prompt tokens reused",
                    c.second_cached, c.prompt_tokens
                ),
                Err(e) => format!("cache probe failed: {e}"),
            });
            self.cache = Some(r);
        }
        out
    }
}

pub(crate) fn draw(f: &mut Frame, app: &App, area: Rect) {
    match app.models.panel {
        Panel::Doctor => draw_doctor(f, app, area),
        Panel::Servers => draw_servers(f, app, area),
        Panel::Cache => draw_cache(f, app, area),
    }
}

fn draw_doctor(f: &mut Frame, app: &App, area: Rect) {
    let (title, lines) = match &app.doctor {
        None => (" doctor ".to_owned(), vec![Line::from("d runs the checks")]),
        Some((online, checks)) => {
            let mut lines = Vec::new();
            for c in checks {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<5}", c.status), status_style(&c.status)),
                    Span::raw(format!("{:<16} {}", c.name, c.detail)),
                ]));
                if let Some(fix) = &c.fix {
                    lines.push(Line::styled(
                        format!("      fix: {}", fix.replace('\n', "; ")),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
            }
            let mode = if *online {
                "online: connection, context window; never a model call"
            } else {
                "offline: no network; o checks the servers"
            };
            (format!(" doctor ({mode}) "), lines)
        }
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(crate::ui::frame().title(title)),
        area,
    );
}

fn draw_servers(f: &mut Frame, app: &App, area: Rect) {
    let view = &app.models;
    let mut lines = Vec::new();
    if view.detecting.is_some() {
        lines.push(Line::from("looking on 127.0.0.1 (preset ports only)…"));
    }
    match &view.servers {
        None => {}
        Some(servers) if servers.is_empty() => lines.push(Line::from(
            "no local server answered on the loopback preset ports; start Ollama, LM Studio, \
llama.cpp, vLLM, oMLX or mlx_lm.server, then l again",
        )),
        Some(servers) => {
            let mut i = 0;
            for s in servers {
                lines.push(Line::styled(
                    format!("{} at {}", s.backend, s.base_url),
                    Style::new().add_modifier(Modifier::BOLD),
                ));
                if s.models.is_empty() {
                    lines.push(Line::styled(
                        "  lists no models",
                        Style::new().fg(Color::DarkGray),
                    ));
                }
                for m in &s.models {
                    let selected = i == view.pick;
                    let style = if selected {
                        Style::new().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::new()
                    };
                    lines.push(Line::styled(
                        format!("  {} {m}", if selected { ">" } else { " " }),
                        style,
                    ));
                    i += 1;
                }
            }
        }
    }
    let configured = format!(
        "configured: {} {}",
        app.cfg.str("local.base_url").unwrap_or_default(),
        app.cfg.str("local.model").unwrap_or_default()
    );
    lines.push(Line::styled(configured, Style::new().fg(Color::DarkGray)));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(crate::ui::frame().title(" local servers ([ ] pick, u use) ")),
        area,
    );
}

fn draw_cache(f: &mut Frame, app: &App, area: Rect) {
    let view = &app.models;
    let lines: Vec<Line> = match (&view.probing, &view.cache) {
        (Some(_), _) => vec![Line::from(
            "sending the same short request twice to the local model…",
        )],
        (None, None) => vec![Line::from("c runs the probe")],
        (None, Some(Err(e))) => vec![Line::styled(e.clone(), Style::new().fg(Color::Red))],
        (None, Some(Ok(c))) => {
            let share = if c.prompt_tokens == 0 {
                0.0
            } else {
                100.0 * c.second_cached as f64 / c.prompt_tokens as f64
            };
            let (verdict, color) = if c.unreported {
                (
                    "the server reports no usage: cache reuse cannot be measured",
                    Color::Yellow,
                )
            } else if c.second_cached > 0 {
                ("the server reuses its prompt cache", Color::Green)
            } else {
                (
                    "no reuse: every turn pays the whole prompt again (enable prompt caching on the server)",
                    Color::Yellow,
                )
            };
            vec![
                Line::from(format!("{} at {}", c.model, c.base_url)),
                Line::from(format!(
                    "first request:  {:>6} cached of {} prompt tokens, {:.1}s",
                    c.first_cached, c.prompt_tokens, c.first_seconds
                )),
                Line::from(format!(
                    "second request: {:>6} cached ({share:.0}%), {:.1}s",
                    c.second_cached, c.second_seconds
                )),
                Line::styled(verdict, Style::new().fg(color)),
            ]
        }
    };
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            crate::ui::frame().title(" cache reuse (two identical requests to the local model) "),
        ),
        area,
    );
}
