// SPDX-License-Identifier: GPL-3.0-or-later
//! The files a run changed, for the Run view's side panel: taken from the
//! run's write journal (each file's content before the run's first write to
//! it) and diffed against the workspace now with Declass's hardened git helper.
//! Everything here reads; nothing is written. Files that are sensitive by
//! policy, or derived from sensitive data by a command, are listed as held
//! locally and never diffed, so their content is not shown.

use declass_agent::journal;
use declass_boundary::policy::Policy;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Diff rows kept per file (a larger diff is cut with a note).
const MAX_ROWS: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Context,
    Added,
    Removed,
    /// The start of a hunk (`text` says where).
    Hunk,
    /// A remark from git or from the viewer (binary file, cut diff).
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: RowKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The run created it.
    Created,
    Edited,
    /// It existed before the run and is gone now.
    Deleted,
    /// Written, but back to its content before the run.
    Unchanged,
    /// Changed by a command that read sensitive data (not journaled).
    ByCommand,
}

/// Why a file's content is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Matches the sensitivity rules (globs, protected paths).
    Sensitive,
    /// Created or changed by a command that could read sensitive data.
    Derived,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangedFile {
    pub path: String,
    pub status: Status,
    pub held: Option<Held>,
    pub added: usize,
    pub removed: usize,
    pub rows: Vec<Row>,
    /// Journal number of the latest write (0: not journaled).
    pub last: u64,
    /// (modified, length) of the workspace file when last diffed.
    stamp: Option<(SystemTime, u64)>,
    diffed: bool,
    before: Option<PathBuf>,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
}

/// Parses `git diff` unified output into numbered rows and counts.
pub fn parse_unified(text: &str) -> (Vec<Row>, usize, usize) {
    let mut rows = Vec::new();
    let (mut added, mut removed) = (0, 0);
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            let Some((ranges, _)) = rest.split_once(" @@") else {
                continue;
            };
            let mut parts = ranges.split_whitespace();
            let start = |p: Option<&str>, sign: char| -> u32 {
                p.and_then(|p| p.strip_prefix(sign))
                    .and_then(|p| p.split(',').next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0)
            };
            old = start(parts.next(), '-');
            new = start(parts.next(), '+');
            in_hunk = true;
            rows.push(Row {
                old: None,
                new: None,
                kind: RowKind::Hunk,
                text: format!("from line {}", new.max(1)),
            });
            continue;
        }
        if !in_hunk {
            if line.starts_with("Binary files") {
                rows.push(Row {
                    old: None,
                    new: None,
                    kind: RowKind::Note,
                    text: "binary file: no line diff".into(),
                });
            }
            continue;
        }
        if rows.len() >= MAX_ROWS {
            rows.push(Row {
                old: None,
                new: None,
                kind: RowKind::Note,
                text: format!("diff cut at {MAX_ROWS} rows"),
            });
            break;
        }
        let (kind, body) = match line.chars().next() {
            Some('+') => (RowKind::Added, &line[1..]),
            Some('-') => (RowKind::Removed, &line[1..]),
            Some(' ') => (RowKind::Context, &line[1..]),
            Some('\\') => (RowKind::Note, line.trim_start_matches("\\ ")),
            _ => (RowKind::Context, line),
        };
        let row = match kind {
            RowKind::Added => {
                added += 1;
                new += 1;
                Row {
                    old: None,
                    new: Some(new - 1),
                    kind,
                    text: body.into(),
                }
            }
            RowKind::Removed => {
                removed += 1;
                old += 1;
                Row {
                    old: Some(old - 1),
                    new: None,
                    kind,
                    text: body.into(),
                }
            }
            RowKind::Context => {
                old += 1;
                new += 1;
                Row {
                    old: Some(old - 1),
                    new: Some(new - 1),
                    kind,
                    text: body.into(),
                }
            }
            _ => Row {
                old: None,
                new: None,
                kind,
                text: body.into(),
            },
        };
        rows.push(row);
    }
    (rows, added, removed)
}

/// Files derived from sensitive data in this run: `derived.json` (kept by
/// the security engine) and the audit log's sensitive-command events.
pub fn derived_files(run_dir: &Path, from_audit: &[String]) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = std::fs::read(run_dir.join("derived.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<PathBuf>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    out.extend(from_audit.iter().cloned());
    out
}

/// Brings `files` up to date with the run's journal and the workspace:
/// adds newly written files, re-diffs files whose content changed since the
/// last look, and lists derived files. Returns whether anything changed.
pub fn update(
    files: &mut Vec<ChangedFile>,
    ws: &Path,
    run_dir: &Path,
    policy: &Policy,
    derived: &BTreeSet<String>,
) -> bool {
    let git = declass_git::Git::locate().ok();
    let mut changed = false;
    let held = |path: &str| {
        if policy.is_sensitive_path(Path::new(path)) {
            Some(Held::Sensitive)
        } else if derived.contains(path) {
            Some(Held::Derived)
        } else {
            None
        }
    };
    for w in journal::written(run_dir) {
        let path = w.path.to_string_lossy().into_owned();
        let i = match files.iter().position(|f| f.path == path) {
            Some(i) => i,
            None => {
                files.push(ChangedFile {
                    path: path.clone(),
                    status: Status::Edited,
                    held: None,
                    added: 0,
                    removed: 0,
                    rows: Vec::new(),
                    last: 0,
                    stamp: None,
                    diffed: false,
                    before: w.before.clone(),
                });
                changed = true;
                files.len() - 1
            }
        };
        let f = &mut files[i];
        if f.last != w.last {
            f.last = w.last;
            changed = true;
        }
        f.held = held(&path);
        let now = stamp(&ws.join(&path));
        let status = match (&f.before, &now) {
            (None, Some(_)) => Status::Created,
            (Some(_), None) => Status::Deleted,
            _ => Status::Edited,
        };
        if f.held.is_some() {
            f.rows.clear();
            (f.added, f.removed, f.status) = (0, 0, status);
            continue;
        }
        if f.diffed && f.stamp == now {
            continue;
        }
        f.stamp = now;
        f.diffed = true;
        changed = true;
        let text = match &git {
            Some(g) => g
                .diff_files(ws, f.before.as_deref(), &ws.join(&path))
                .map_err(|e| e.to_string()),
            None => Err("git is not installed: no diff".into()),
        };
        match text {
            Ok(t) => {
                (f.rows, f.added, f.removed) = parse_unified(&t);
                f.status = if t.is_empty() {
                    Status::Unchanged
                } else {
                    status
                };
            }
            Err(e) => {
                f.rows = vec![Row {
                    old: None,
                    new: None,
                    kind: RowKind::Note,
                    text: e,
                }];
                f.status = status;
            }
        }
    }
    for path in derived {
        if !files.iter().any(|f| &f.path == path) {
            files.push(ChangedFile {
                path: path.clone(),
                status: Status::ByCommand,
                held: Some(Held::Derived),
                added: 0,
                removed: 0,
                rows: Vec::new(),
                last: 0,
                stamp: None,
                diffed: false,
                before: None,
            });
            changed = true;
        }
    }
    changed
}

fn status_mark(file: &ChangedFile) -> (&'static str, Color) {
    match (file.held, file.status) {
        (Some(_), _) => ("held", Color::Yellow),
        (None, Status::Created) => ("new ", Color::Green),
        (None, Status::Deleted) => ("gone", Color::Red),
        (None, Status::Unchanged) => ("same", Color::DarkGray),
        (None, Status::ByCommand) => ("cmd ", Color::Yellow),
        (None, Status::Edited) => ("edit", Color::Cyan),
    }
}

/// The changed files and, below them, the selected file's diff.
pub(crate) fn draw_files(
    f: &mut Frame,
    files: &[ChangedFile],
    selected: usize,
    diff_scroll: usize,
    focused: bool,
    area: Rect,
) {
    let list_height = (files.len() as u16 + 2).clamp(3, (area.height / 2).max(3));
    let [list_area, diff_area] =
        Layout::vertical([Constraint::Length(list_height), Constraint::Min(3)]).areas(area);
    let (added, removed) = files
        .iter()
        .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
    let items: Vec<ListItem> = files
        .iter()
        .map(|file| {
            let (mark, color) = status_mark(file);
            let mut spans = vec![
                Span::styled(format!("{mark} "), Style::new().fg(color)),
                Span::raw(file.path.clone()),
            ];
            if file.held.is_some() {
                spans.push(Span::styled(
                    "  held locally",
                    Style::new().fg(Color::Yellow),
                ));
            } else {
                if file.added > 0 {
                    spans.push(Span::styled(
                        format!("  +{}", file.added),
                        Style::new().fg(Color::Green),
                    ));
                }
                if file.removed > 0 {
                    spans.push(Span::styled(
                        format!("  -{}", file.removed),
                        Style::new().fg(Color::Red),
                    ));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let title = if files.is_empty() {
        " changed files: none yet ".to_owned()
    } else {
        format!(" changed files {} (+{added} -{removed}) ", files.len())
    };
    let mut state = ListState::default().with_selected((!files.is_empty()).then_some(selected));
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(
                crate::ui::frame()
                    .title(title)
                    .border_style(focus_style(focused)),
            ),
        list_area,
        &mut state,
    );
    draw_diff(f, files.get(selected), diff_scroll, focused, diff_area);
}

/// One file's diff (a held file only says why it is not shown).
pub(crate) fn draw_diff(
    f: &mut Frame,
    file: Option<&ChangedFile>,
    diff_scroll: usize,
    focused: bool,
    area: Rect,
) {
    let Some(file) = file else {
        f.render_widget(
            Paragraph::new("The files this run writes appear here with their diffs.")
                .wrap(Wrap { trim: false })
                .block(crate::ui::frame().title(" diff ")),
            area,
        );
        return;
    };
    if let Some(held) = file.held {
        let why = match held {
            Held::Sensitive => "it matches the sensitivity rules",
            Held::Derived => "a command that could read sensitive data created or changed it",
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::styled("held locally", Style::new().fg(Color::Yellow)),
                Line::from(format!("{}: {why}; its content is not shown.", file.path)),
            ])
            .wrap(Wrap { trim: false })
            .block(crate::ui::frame().title(format!(" {} ", file.path))),
            area,
        );
        return;
    }
    let width = file
        .rows
        .iter()
        .filter_map(|r| r.old.max(r.new))
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let number = |n: Option<u32>| n.map_or(" ".repeat(width), |n| format!("{n:>width$}"));
    let visible = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = file
        .rows
        .iter()
        .skip(diff_scroll)
        .take(visible)
        .map(|r| {
            let gutter = format!("{} {} ", number(r.old), number(r.new));
            match r.kind {
                RowKind::Hunk => Line::styled(
                    format!("{:┄<w$} {}", "", r.text, w = width * 2 + 1),
                    Style::new().fg(Color::DarkGray),
                ),
                RowKind::Note => Line::styled(
                    format!("{gutter}  {}", r.text),
                    Style::new()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
                RowKind::Added => Line::from(vec![
                    Span::styled(gutter, Style::new().fg(Color::DarkGray)),
                    Span::styled(
                        format!("+ {}", r.text),
                        Style::new().fg(Color::Green).bg(Color::Rgb(16, 48, 24)),
                    ),
                ]),
                RowKind::Removed => Line::from(vec![
                    Span::styled(gutter, Style::new().fg(Color::DarkGray)),
                    Span::styled(
                        format!("- {}", r.text),
                        Style::new().fg(Color::Red).bg(Color::Rgb(56, 16, 16)),
                    ),
                ]),
                RowKind::Context => Line::from(vec![
                    Span::styled(gutter, Style::new().fg(Color::DarkGray)),
                    Span::raw(format!("  {}", r.text)),
                ]),
            }
        })
        .collect();
    let what = match file.status {
        Status::Created => "created",
        Status::Deleted => "deleted",
        Status::Unchanged => "back to its content before the run",
        Status::ByCommand => "changed by a command",
        Status::Edited => "edited",
    };
    let position = if file.rows.is_empty() {
        String::new()
    } else {
        format!(
            "; rows {}-{} of {}",
            diff_scroll + 1,
            (diff_scroll + visible).min(file.rows.len()),
            file.rows.len()
        )
    };
    f.render_widget(
        Paragraph::new(lines).block(
            crate::ui::frame()
                .title(format!(" {} ({what}{position}) ", file.path))
                .border_style(focus_style(focused)),
        ),
        area,
    );
}

fn focus_style(on: bool) -> Style {
    if on {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunks_with_line_numbers_and_counts() {
        let text = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n@@ -2,3 +2,4 @@ fn f\n a\n-b\n+B\n+C\n c\n\\ No newline at end of file\n@@ -10 +11 @@\n-z\n+Z\n";
        let (rows, added, removed) = parse_unified(text);
        assert_eq!((added, removed), (3, 2));
        let shown: Vec<(Option<u32>, Option<u32>, RowKind, &str)> = rows
            .iter()
            .map(|r| (r.old, r.new, r.kind, r.text.as_str()))
            .collect();
        assert_eq!(
            shown,
            vec![
                (None, None, RowKind::Hunk, "from line 2"),
                (Some(2), Some(2), RowKind::Context, "a"),
                (Some(3), None, RowKind::Removed, "b"),
                (None, Some(3), RowKind::Added, "B"),
                (None, Some(4), RowKind::Added, "C"),
                (Some(4), Some(5), RowKind::Context, "c"),
                (None, None, RowKind::Note, "No newline at end of file"),
                (None, None, RowKind::Hunk, "from line 11"),
                (Some(10), None, RowKind::Removed, "z"),
                (None, Some(11), RowKind::Added, "Z"),
            ]
        );
        let (rows, ..) = parse_unified("diff --git a/x b/x\nBinary files a/x and b/x differ\n");
        assert_eq!(rows[0].kind, RowKind::Note);
    }
}
