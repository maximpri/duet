// SPDX-License-Identifier: GPL-3.0-or-later
//! IP levels: the workspace file tree (git's view, so `.gitignore` applies)
//! with each path's level, marks that add entries to `ip.interface_only` /
//! `ip.sealed`, and a preview of the skeleton the frontier would see.

use crate::app::App;
use declass_boundary::policy::IpLevel;
use declass_boundary::skeleton::{Lang, SpanKind, render, withheld};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

/// Largest file previewed.
const PREVIEW_BYTES: u64 = 512 * 1024;

#[derive(Default)]
pub struct IpView {
    /// Workspace files, or why they could not be listed.
    files: Option<Result<Vec<String>, String>>,
    open: BTreeSet<String>,
    pub(crate) selected: usize,
    /// The previewed path and its preview text.
    preview: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub dir: bool,
    pub depth: usize,
}

pub(crate) fn level_name(level: Option<IpLevel>) -> &'static str {
    match level {
        None => "open",
        Some(IpLevel::InterfaceOnly) => "interface-only",
        Some(IpLevel::Sealed) => "sealed",
    }
}

impl IpView {
    /// Lists the workspace files once (tracked and untracked, minus ignored).
    pub fn load(&mut self, ws: &Path) {
        if self.files.is_none() {
            self.files = Some(
                declass_git::Git::locate()
                    .and_then(|g| g.list_files(ws))
                    .map_err(|e| e.to_string()),
            );
        }
    }

    fn files(&self) -> &[String] {
        match &self.files {
            Some(Ok(f)) => f,
            _ => &[],
        }
    }

    /// Visible rows: directories, and files whose directories are all open.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        let mut seen = BTreeSet::new();
        for f in self.files() {
            let parts: Vec<&str> = f.split('/').collect();
            let mut visible = true;
            for depth in 0..parts.len() - 1 {
                let dir = parts[..=depth].join("/");
                if visible && seen.insert(dir.clone()) {
                    rows.push(TreeRow {
                        path: dir.clone(),
                        dir: true,
                        depth,
                    });
                }
                visible = visible && self.open.contains(&dir);
            }
            if visible {
                rows.push(TreeRow {
                    path: f.clone(),
                    dir: false,
                    depth: parts.len() - 1,
                });
            }
        }
        rows
    }

    fn selected_row(&self) -> Option<TreeRow> {
        self.rows().get(self.selected).cloned()
    }

    /// Opens or closes the selected directory.
    pub fn toggle(&mut self) {
        if let Some(r) = self.selected_row().filter(|r| r.dir)
            && !self.open.remove(&r.path)
        {
            self.open.insert(r.path);
        }
    }

    /// The IP list entry for the selected row: the file, or `dir/**`.
    pub fn selected_entry(&self) -> Option<String> {
        self.selected_row().map(|r| {
            if r.dir {
                format!("{}/**", r.path)
            } else {
                r.path
            }
        })
    }
}

/// What the frontier would see of `path` if it were interface-only.
pub(crate) fn preview(ws: &Path, path: &str) -> String {
    let Some(lang) = Lang::for_path(Path::new(path)) else {
        return "no interface view for this file type: marked interface-only it is withheld like a sealed file".into();
    };
    let mut text = String::new();
    let read = std::fs::File::open(ws.join(path))
        .and_then(|f| f.take(PREVIEW_BYTES).read_to_string(&mut text));
    if let Err(e) = read {
        return format!("cannot read {path}: {e}");
    }
    match withheld(lang, &text) {
        Ok(spans) => render(&text, &spans, |s| match s.kind {
            SpanKind::Value => format!("⟨value of {}⟩", s.item),
            _ => format!("⟨body of {}⟩", s.item),
        }),
        Err(e) => format!("no skeleton: {e}"),
    }
}

pub(crate) fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let [head, body] = Layout::vertical([Constraint::Length(4), Constraint::Min(4)]).areas(area);
    let list_line = |key: &str| {
        Line::from(format!(
            "{key} = {}  ({})",
            app.cfg
                .value(key)
                .map(ToString::to_string)
                .unwrap_or_default(),
            crate::ui::origin_name(app.cfg.origin(key))
        ))
    };
    f.render_widget(
        Paragraph::new(vec![list_line("ip.interface_only"), list_line("ip.sealed")])
            .block(crate::ui::frame().title(" IP levels ")),
        head,
    );
    let [tree_area, preview_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(body);
    let policy = app.policy();
    let rows = app.ip.rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| {
            let level = if r.dir {
                let entry = format!("{}/**", r.path);
                if policy.sealed.contains(&entry) {
                    Some(IpLevel::Sealed)
                } else if policy.interface_only.contains(&entry) {
                    Some(IpLevel::InterfaceOnly)
                } else {
                    None
                }
            } else {
                policy.ip_level(Path::new(&r.path))
            };
            let name = r.path.rsplit('/').next().unwrap_or(&r.path);
            let marker = match (r.dir, app.ip.open.contains(&r.path)) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                _ => "  ",
            };
            let mut spans = vec![Span::raw(format!(
                "{}{marker}{name}{}",
                "  ".repeat(r.depth),
                if r.dir { "/" } else { "" }
            ))];
            if level.is_some() {
                spans.push(Span::styled(
                    format!("  [{}]", level_name(level)),
                    Style::new().fg(Color::Yellow),
                ));
            }
            if !r.dir && policy.is_sensitive_path(Path::new(&r.path)) {
                spans.push(Span::styled("  [sensitive]", Style::new().fg(Color::Red)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let title = match &app.ip.files {
        Some(Err(e)) => format!(" files: {e} "),
        _ => " files (.gitignore applies) ".into(),
    };
    let mut state = ListState::default().with_selected(Some(app.ip.selected));
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(crate::ui::frame().title(title)),
        tree_area,
        &mut state,
    );

    let (title, text) = match rows.get(app.ip.selected) {
        Some(r) if !r.dir => {
            let cached = app.ip.preview.as_ref().filter(|(p, _)| *p == r.path);
            let text = match cached {
                Some((_, t)) => t.clone(),
                None => {
                    let t = preview(&app.paths.workspace, &r.path);
                    app.ip.preview = Some((r.path.clone(), t.clone()));
                    t
                }
            };
            let level = policy.ip_level(Path::new(&r.path));
            let title = match level {
                Some(IpLevel::Sealed) => {
                    format!(
                        " {} is sealed: the frontier sees only that it exists ",
                        r.path
                    )
                }
                _ => format!(" {}: interface-only view ({}) ", r.path, level_name(level)),
            };
            (title, text)
        }
        _ => (
            " preview ".to_owned(),
            "Select a file to preview its interface-only skeleton.".to_owned(),
        ),
    };
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(crate::ui::frame().title(title)),
        preview_area,
    );
}
