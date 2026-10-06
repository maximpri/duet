// SPDX-License-Identifier: GPL-3.0-or-later
//! Local plan review/editing. Only typed controller actions leave this module;
//! rendering, keyboard input and pasted text never authorize work implicitly.
use super::{Editor, Outcome, ansi, safe};
use declass_agent::plans::{Check, Draft, Step};
use declass_agent::questions::QuestionOptions;
use ratatui::Frame;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanIdentity {
    pub revision: String,
    pub digest: String,
}

#[derive(Debug, Clone)]
pub struct PlanStepProgress {
    pub id: String,
    pub status: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PlanSnapshot {
    pub identity: PlanIdentity,
    pub draft: Draft,
    pub status: String,
    pub approved: bool,
    pub can_implement: bool,
    pub can_resume: bool,
    pub steps: Vec<PlanStepProgress>,
    pub location: String,
    pub changes: Option<String>,
    pub verification: Vec<String>,
    pub questions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlanQuestion {
    pub id: String,
    pub identity: Option<PlanIdentity>,
    pub question: String,
    pub options: QuestionOptions,
}

#[derive(Debug, Clone)]
pub enum PlanAction {
    Edit {
        identity: PlanIdentity,
    },
    Approve {
        identity: PlanIdentity,
    },
    Implement {
        identity: PlanIdentity,
    },
    Pause {
        identity: PlanIdentity,
    },
    Resume {
        identity: PlanIdentity,
    },
    Revise {
        identity: PlanIdentity,
        feedback: String,
    },
    Save {
        identity: PlanIdentity,
        draft: Draft,
    },
    Answer {
        question_id: String,
        identity: Option<PlanIdentity>,
        choice_id: Option<String>,
        text: String,
    },
}

pub(super) enum Effect {
    None,
    Action(PlanAction),
    Copy(String),
    Paste,
    Interrupt,
}

#[derive(Clone, Copy)]
enum Field {
    Title,
    Objective,
    Scope,
    NonGoals,
    Assumptions,
    Decisions,
    StepTitle(usize),
    StepDescription(usize),
    Paths(usize),
    Acceptance(usize),
    CheckCommand(usize, usize),
    CheckDescription(usize, usize),
}
impl Field {
    fn step(self) -> Option<usize> {
        match self {
            Self::StepTitle(i)
            | Self::StepDescription(i)
            | Self::Paths(i)
            | Self::Acceptance(i)
            | Self::CheckCommand(i, _)
            | Self::CheckDescription(i, _) => Some(i),
            _ => None,
        }
    }
    fn label(self) -> String {
        match self {
            Self::Title => "Title".into(),
            Self::Objective => "Objective".into(),
            Self::Scope => "Scope — one item per line".into(),
            Self::NonGoals => "Non-goals — one item per line".into(),
            Self::Assumptions => "Assumptions — one item per line".into(),
            Self::Decisions => "Decisions — one item per line".into(),
            Self::StepTitle(i) => format!("Step {} title", i + 1),
            Self::StepDescription(i) => format!("Step {} description", i + 1),
            Self::Paths(i) => format!("Step {} affected paths — one per line", i + 1),
            Self::Acceptance(i) => format!("Step {} acceptance criteria — one per line", i + 1),
            Self::CheckCommand(i, j) => format!("Step {} check {} command", i + 1, j + 1),
            Self::CheckDescription(i, j) => format!("Step {} check {} description", i + 1, j + 1),
        }
    }
    fn text(self, d: &Draft) -> String {
        match self {
            Self::Title => d.title.clone(),
            Self::Objective => d.objective.clone(),
            Self::Scope => d.scope.join("\n"),
            Self::NonGoals => d.non_goals.join("\n"),
            Self::Assumptions => d.assumptions.join("\n"),
            Self::Decisions => d.decisions.join("\n"),
            Self::StepTitle(i) => d.steps[i].title.clone(),
            Self::StepDescription(i) => d.steps[i].description.clone(),
            Self::Paths(i) => d.steps[i].paths.join("\n"),
            Self::Acceptance(i) => d.steps[i].acceptance.join("\n"),
            Self::CheckCommand(i, j) => d.steps[i].checks[j].command.clone(),
            Self::CheckDescription(i, j) => d.steps[i].checks[j].description.clone(),
        }
    }
    fn put(self, d: &mut Draft, text: &str) {
        let list = || {
            text.lines()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .collect()
        };
        match self {
            Self::Title => d.title = text.into(),
            Self::Objective => d.objective = text.into(),
            Self::Scope => d.scope = list(),
            Self::NonGoals => d.non_goals = list(),
            Self::Assumptions => d.assumptions = list(),
            Self::Decisions => d.decisions = list(),
            Self::StepTitle(i) => d.steps[i].title = text.into(),
            Self::StepDescription(i) => d.steps[i].description = text.into(),
            Self::Paths(i) => d.steps[i].paths = list(),
            Self::Acceptance(i) => d.steps[i].acceptance = list(),
            Self::CheckCommand(i, j) => d.steps[i].checks[j].command = text.into(),
            Self::CheckDescription(i, j) => d.steps[i].checks[j].description = text.into(),
        }
    }
}
fn fields(d: &Draft) -> Vec<Field> {
    let mut out = vec![
        Field::Title,
        Field::Objective,
        Field::Scope,
        Field::NonGoals,
        Field::Assumptions,
        Field::Decisions,
    ];
    for (i, step) in d.steps.iter().enumerate() {
        out.extend([
            Field::StepTitle(i),
            Field::StepDescription(i),
            Field::Paths(i),
            Field::Acceptance(i),
        ]);
        for j in 0..step.checks.len() {
            out.extend([Field::CheckCommand(i, j), Field::CheckDescription(i, j)]);
        }
    }
    out
}
struct Editing {
    draft: Draft,
    field: usize,
    editor: Editor,
}
impl Editing {
    fn new(draft: Draft) -> Self {
        let mut e = Self {
            draft,
            field: 0,
            editor: Editor::default(),
        };
        e.load();
        e
    }
    fn selected(&self) -> Field {
        fields(&self.draft)[self.field]
    }
    fn flush(&mut self) {
        self.selected().put(&mut self.draft, self.editor.buffer());
    }
    fn load(&mut self) {
        self.field = self.field.min(fields(&self.draft).len() - 1);
        self.editor.clear();
        self.editor
            .insert(&safe(&self.selected().text(&self.draft)));
    }
    fn navigate(&mut self, backward: bool) {
        self.flush();
        let n = fields(&self.draft).len();
        self.field = if backward {
            (self.field + n - 1) % n
        } else {
            (self.field + 1) % n
        };
        self.load();
    }
}
struct Question {
    snapshot: PlanQuestion,
    open: bool,
    selected: Option<usize>,
    scroll: u16,
    freeform: bool,
    editor: Editor,
}

#[derive(Default)]
pub(super) struct Panel {
    latest: Option<PlanSnapshot>,
    shown: Option<PlanSnapshot>,
    pub open: bool,
    scroll: u16,
    focus: usize,
    editing: Option<Editing>,
    feedback: Option<Editor>,
    question: Option<Question>,
    notice: String,
    pending_save: bool,
}
impl Panel {
    pub fn snapshot(&mut self, snapshot: Option<PlanSnapshot>) {
        let changed =
            self.shown.as_ref().map(|s| &s.identity) != snapshot.as_ref().map(|s| &s.identity);
        if self.open && changed {
            self.notice="Plan changed. Close and reopen to review the latest revision; stale actions are disabled.".into();
        } else if self.open {
            // Progress and approval updates do not discard the operator's draft.
            self.shown = snapshot.clone();
        }
        self.latest = snapshot;
    }
    pub fn error(&mut self, error: String) {
        self.pending_save = false;
        if self.editing.is_some() {
            self.open = true;
        }
        self.notice = format!("Save failed: {}", safe(&error));
    }
    pub fn saved(&mut self, snapshot: PlanSnapshot) {
        self.pending_save = false;
        self.latest = Some(snapshot);
        self.show(false);
        self.notice = "Revision saved; review before approving or implementing.".into();
    }
    pub fn show(&mut self, edit: bool) {
        if self.pending_save {
            self.open = true;
            return;
        }
        self.shown = self.latest.clone();
        self.open = true;
        self.scroll = 0;
        self.focus = 0;
        self.feedback = None;
        self.notice.clear();
        self.editing = if edit {
            self.shown.as_ref().map(|s| Editing::new(s.draft.clone()))
        } else {
            None
        };
    }
    /// Returns true only when a genuinely new question is opened.
    pub fn question(&mut self, question: Option<PlanQuestion>) -> bool {
        if let (Some(old), Some(new)) = (&mut self.question, &question)
            && old.snapshot.id == new.id
            && old.snapshot.identity == new.identity
            && old.snapshot.question == new.question
            && old.snapshot.options == new.options
        {
            return false;
        }
        self.question = question.map(|snapshot| Question {
            snapshot,
            open: true,
            selected: None,
            scroll: 0,
            freeform: false,
            editor: Editor::default(),
        });
        self.question_active()
    }
    pub fn question_active(&self) -> bool {
        self.question.as_ref().is_some_and(|q| q.open)
    }
    pub fn reopen_question(&mut self) {
        if let Some(q) = &mut self.question {
            q.open = true;
        }
    }
    pub fn active(&self) -> bool {
        self.open || self.question_active()
    }
    pub fn summary(&self) -> Option<String> {
        self.latest
            .as_ref()
            .map(|s| format!("{} · {}", s.identity.revision, s.status))
    }
    fn current(&self) -> bool {
        self.shown.is_some()
            && self.shown.as_ref().map(|s| &s.identity) == self.latest.as_ref().map(|s| &s.identity)
    }
    pub fn paste(&mut self, text: &str) {
        if self.pending_save {
            return;
        }
        let editor = if let Some(q) = &mut self.question
            && q.open
        {
            if q.freeform {
                Some(&mut q.editor)
            } else {
                None
            }
        } else if let Some(e) = &mut self.editing {
            Some(&mut e.editor)
        } else {
            self.feedback.as_mut()
        };
        if let Some(editor) = editor {
            editor.insert(&safe(text));
        }
    }
    pub fn event(&mut self, event: Event) -> Effect {
        if let Event::Paste(text) = event {
            self.paste(&text);
            return Effect::None;
        }
        let Event::Key(k) = event else {
            return Effect::None;
        };
        if k.kind == KeyEventKind::Release {
            return Effect::None;
        }
        if self.question.as_ref().is_some_and(|q| q.open) {
            return self.question_key(k);
        }
        if !self.open {
            return Effect::None;
        }
        if self.pending_save {
            if k.code == KeyCode::Esc {
                self.open = false;
            }
            if k.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(k.code, KeyCode::Char('c' | 'C'))
            {
                return Effect::Interrupt;
            }
            return Effect::None;
        }
        if k.code == KeyCode::Esc {
            if self.editing.take().is_none() && self.feedback.take().is_none() {
                self.open = false;
            }
            self.notice.clear();
            return Effect::None;
        }
        if !self.current() {
            self.notice = "Revision changed; close and reopen before taking action.".into();
            return Effect::None;
        }
        let identity = self.shown.as_ref().unwrap().identity.clone();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(editor) = &mut self.feedback {
            if ctrl && k.code == KeyCode::Char('s') && !editor.buffer().trim().is_empty() {
                let feedback = editor.buffer().to_owned();
                self.feedback = None;
                self.open = false;
                return Effect::Action(PlanAction::Revise { identity, feedback });
            }
            return edit_key(editor, k);
        }
        if let Some(e) = &mut self.editing {
            match k.code {
                KeyCode::Char('s') if ctrl => {
                    e.flush();
                    let draft = e.draft.clone();
                    self.pending_save = true;
                    self.notice =
                        "Saving revision… Esc hides the editor; your draft is retained.".into();
                    return Effect::Action(PlanAction::Save { identity, draft });
                }
                KeyCode::Tab | KeyCode::BackTab => e.navigate(
                    k.code == KeyCode::BackTab || k.modifiers.contains(KeyModifiers::SHIFT),
                ),
                KeyCode::F(7) => {
                    e.flush();
                    e.draft.steps.push(Step::default());
                    e.field = fields(&e.draft)
                        .iter()
                        .position(|f| matches!(f,Field::StepTitle(i) if *i==e.draft.steps.len()-1))
                        .unwrap();
                    e.load();
                }
                KeyCode::F(8) => {
                    e.flush();
                    if let Some(i) = e.selected().step() {
                        e.draft.steps[i].checks.push(Check::default());
                        let j = e.draft.steps[i].checks.len() - 1;
                        e.field = fields(&e.draft)
                            .iter()
                            .position(|f| matches!(f,Field::CheckCommand(a,b) if *a==i&&*b==j))
                            .unwrap();
                        e.load();
                    }
                }
                KeyCode::F(9) => {
                    e.flush();
                    match e.selected() {
                        Field::CheckCommand(i, j) | Field::CheckDescription(i, j) => {
                            e.draft.steps[i].checks.remove(j);
                        }
                        field => {
                            if let Some(i) = field.step() {
                                e.draft.steps.remove(i);
                            }
                        }
                    }
                    e.load();
                }
                KeyCode::Up | KeyCode::Down if ctrl => {
                    e.flush();
                    if let Some(i) = e.selected().step() {
                        let j = if k.code == KeyCode::Up {
                            i.saturating_sub(1)
                        } else {
                            (i + 1).min(e.draft.steps.len() - 1)
                        };
                        e.draft.steps.swap(i, j);
                        e.field = fields(&e.draft)
                            .iter()
                            .position(|f| matches!(f,Field::StepTitle(n)if *n==j))
                            .unwrap();
                        e.load();
                    }
                }
                _ => return edit_key(&mut e.editor, k),
            }
            return Effect::None;
        }
        match k.code {
            KeyCode::Tab => self.focus = (self.focus + 1) % 7,
            KeyCode::BackTab => self.focus = (self.focus + 6) % 7,
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(8),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
            KeyCode::Home => self.scroll = 0,
            KeyCode::Enter => {
                let s = self.shown.as_ref().unwrap();
                let action = match self.focus {
                    0 => {
                        self.open = false;
                        None
                    }
                    1 => Some(PlanAction::Edit { identity }),
                    2 => {
                        self.feedback = Some(Editor::default());
                        None
                    }
                    3 => Some(PlanAction::Approve { identity }),
                    4 if s.can_implement => Some(PlanAction::Implement { identity }),
                    5 => Some(PlanAction::Pause { identity }),
                    6 if s.can_resume => Some(PlanAction::Resume { identity }),
                    _ => {
                        self.notice="This action is unavailable; check plan status and unresolved questions.".into();
                        None
                    }
                };
                if let Some(action) = action {
                    self.open = false;
                    return Effect::Action(action);
                }
            }
            _ => {}
        }
        Effect::None
    }
    fn question_key(&mut self, k: KeyEvent) -> Effect {
        let q = self.question.as_mut().unwrap();
        if k.code == KeyCode::Esc {
            if q.freeform {
                q.freeform = false;
            } else {
                q.open = false;
            }
            return Effect::None;
        }
        if q.snapshot
            .identity
            .as_ref()
            .is_some_and(|id| self.latest.as_ref().map(|s| &s.identity) != Some(id))
        {
            self.notice =
                "Clarification belongs to an older revision; wait for a current question.".into();
            return Effect::None;
        }
        let n = q.snapshot.options.choices.len();
        if q.freeform {
            if k.modifiers.contains(KeyModifiers::CONTROL)
                && k.code == KeyCode::Char('s')
                && !q.editor.buffer().trim().is_empty()
            {
                q.open = false;
                return Effect::Action(PlanAction::Answer {
                    question_id: q.snapshot.id.clone(),
                    identity: q.snapshot.identity.clone(),
                    choice_id: None,
                    text: q.editor.buffer().to_owned(),
                });
            }
            return edit_key(&mut q.editor, k);
        }
        match k.code {
            KeyCode::PageDown => q.scroll = q.scroll.saturating_add(5),
            KeyCode::PageUp => q.scroll = q.scroll.saturating_sub(5),
            KeyCode::Home => q.scroll = 0,
            KeyCode::Char('f') if q.snapshot.options.allow_freeform => q.freeform = true,
            KeyCode::Down | KeyCode::Tab if n > 0 => {
                q.selected = Some(q.selected.map_or(0, |i| (i + 1) % n))
            }
            KeyCode::Up | KeyCode::BackTab if n > 0 => {
                q.selected = Some(q.selected.map_or(n - 1, |i| (i + n - 1) % n))
            }
            KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                let i = c as usize - '1' as usize;
                if i < n {
                    q.selected = Some(i);
                }
            }
            KeyCode::Enter => {
                if let Some(i) = q.selected {
                    let choice = &q.snapshot.options.choices[i];
                    q.open = false;
                    return Effect::Action(PlanAction::Answer {
                        question_id: q.snapshot.id.clone(),
                        identity: q.snapshot.identity.clone(),
                        choice_id: Some(choice.id.clone()),
                        text: choice.label.clone(),
                    });
                }
            }
            _ => {}
        }
        Effect::None
    }
    pub fn draw(&self, f: &mut Frame<'_>, area: Rect) {
        if self.open {
            self.draw_plan(f, area);
        }
        if let Some(q) = &self.question
            && q.open
        {
            draw_question(f, area, q, &self.notice);
        }
    }
    fn draw_plan(&self, f: &mut Frame<'_>, area: Rect) {
        let title = self
            .shown
            .as_ref()
            .map_or("Plan — no saved revision".into(), |s| {
                format!(
                    "Plan {} · {}{}",
                    safe(&s.identity.revision),
                    safe(&s.status),
                    if s.approved { " · approved" } else { "" }
                )
            });
        let inner = frame(f, area, &title);
        if let Some(e) = &self.editing {
            draw_editor(
                f,
                inner,
                &e.selected().label(),
                &e.editor,
                if self.notice.is_empty() {
                    "Tab fields · F7 +step · F8 +check · F9 remove\nCtrl-↑/↓ reorder · Ctrl-S save revision · Esc cancel"
                } else {
                    &self.notice
                },
            );
        } else if let Some(editor) = &self.feedback {
            draw_editor(
                f,
                inner,
                "Revision feedback",
                editor,
                "Ctrl-S request revision · Esc cancel",
            );
        } else {
            let [body, actions, note] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(3),
                Constraint::Length(2),
            ])
            .areas(inner);
            let text = self.shown.as_ref().map(review_text).unwrap_or_else(|| {
                "No saved plan. Use /plan <task> to draft one. Esc closes.".into()
            });
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .scroll((self.scroll, 0)),
                body,
            );
            let labels = [
                "Close",
                "Edit",
                "Revise",
                "Approve only",
                "Run all steps",
                "Pause",
                "Resume",
            ];
            let spans = labels
                .iter()
                .enumerate()
                .map(|(i, label)| {
                    Span::styled(
                        format!(" {} ", label),
                        if i == self.focus {
                            Style::default().add_modifier(Modifier::REVERSED)
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect::<Vec<_>>();
            f.render_widget(
                Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false }),
                actions,
            );
            f.render_widget(
                Paragraph::new(if self.notice.is_empty() {
                    "Tab action · Enter select · ↑/↓ scroll · Esc close".into()
                } else {
                    safe(&self.notice)
                })
                .wrap(Wrap { trim: false }),
                note,
            );
        }
    }
}
fn edit_key(editor: &mut Editor, k: KeyEvent) -> Effect {
    // Enter edits a multiline field; only explicit Ctrl-S commits it.
    if k.code == KeyCode::Enter {
        editor.insert("\n");
        return Effect::None;
    }
    match editor.key(k) {
        Outcome::Copy(text) => Effect::Copy(text),
        Outcome::Paste => Effect::Paste,
        Outcome::Interrupt => Effect::Interrupt,
        _ => Effect::None,
    }
}
fn frame(f: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    let area = area.inner(ratatui::layout::Margin::new(1, 1));
    let block = Block::bordered().title(safe(title));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    inner
}
fn draw_editor(f: &mut Frame<'_>, area: Rect, label: &str, editor: &Editor, hints: &str) {
    let [label_area, body, help] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(safe(label)).wrap(Wrap { trim: false }),
        label_area,
    );
    let (rows, (row, col)) = editor.display(
        &[],
        false,
        body.width.max(1) as usize,
        body.height.max(1) as usize,
    );
    f.render_widget(
        Paragraph::new(rows.iter().map(|r| ansi::line(r)).collect::<Vec<_>>()),
        body,
    );
    if body.width > 0 && body.height > 0 {
        f.set_cursor_position(Position {
            x: body.x + (col as u16).min(body.width - 1),
            y: body.y + (row as u16).min(body.height - 1),
        });
    }
    f.render_widget(Paragraph::new(hints).wrap(Wrap { trim: false }), help);
}
fn review_text(s: &PlanSnapshot) -> String {
    let d = &s.draft;
    let mut out = format!(
        "{}\nSaved plan: {}\n\nObjective\n{}\n",
        d.title, s.location, d.objective
    );
    if let Some(changes) = &s.changes {
        out.push_str(&format!("\nChanges from previous revision\n{changes}\n"));
    }
    for (title, items) in [
        ("Questions", &s.questions),
        ("Check evidence", &s.verification),
    ] {
        if !items.is_empty() {
            out.push_str(&format!("\n{title}\n"));
            for item in items {
                out.push_str(&format!("• {item}\n"));
            }
        }
    }
    for (title, items) in [
        ("Scope", &d.scope),
        ("Non-goals", &d.non_goals),
        ("Assumptions", &d.assumptions),
        ("Decisions", &d.decisions),
    ] {
        if !items.is_empty() {
            out.push_str(&format!("\n{title}\n"));
            for item in items {
                out.push_str(&format!("• {item}\n"));
            }
        }
    }
    for (i, step) in d.steps.iter().enumerate() {
        let progress = s.steps.iter().find(|p| p.id == step.id);
        out.push_str(&format!(
            "\n{}. {} [{}]\n{}\n",
            i + 1,
            step.title,
            progress.map_or("Pending", |p| p.status.as_str()),
            step.description
        ));
        if let Some(note) = progress.and_then(|p| p.note.as_ref()) {
            out.push_str(&format!("{note}\n"));
        }
        for p in &step.paths {
            out.push_str(&format!("  File: {p}\n"));
        }
        for a in &step.acceptance {
            out.push_str(&format!("  Acceptance: {a}\n"));
        }
        if step.checks.is_empty() {
            out.push_str("  No checks: completion remains unverified.\n");
        }
        for c in &step.checks {
            out.push_str(&format!(
                "  Check {}: {}\n    {}\n",
                c.id, c.description, c.command
            ));
        }
    }
    safe(&out)
}
fn draw_question(f: &mut Frame<'_>, area: Rect, q: &Question, notice: &str) {
    let inner = frame(
        f,
        area,
        &format!("Clarification {} — not tool approval", q.snapshot.id),
    );
    if q.freeform {
        draw_editor(
            f,
            inner,
            &q.snapshot.question,
            &q.editor,
            "Ctrl-S submit answer · Esc return to choices",
        );
        return;
    }
    let [body, help] = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(inner);
    let mut lines = vec![Line::from(safe(&q.snapshot.question)), Line::default()];
    for (i, c) in q.snapshot.options.choices.iter().enumerate() {
        let text = format!(
            "{}. {}{}{}",
            i + 1,
            c.label,
            if q.snapshot.options.recommended.as_ref() == Some(&c.id) {
                " (recommended)"
            } else {
                ""
            },
            c.description
                .as_ref()
                .map_or(String::new(), |d| format!(" — {d}"))
        );
        lines.push(Line::styled(
            safe(&text),
            if q.selected == Some(i) {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            },
        ));
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((q.scroll, 0)),
        body,
    );
    let hints = if q.snapshot.options.allow_freeform {
        "↑/↓ or number selects · Enter submits\nf text · PgUp/Dn scroll · Esc defer · F6 reopen"
    } else {
        "↑/↓ or number selects · Enter submits\nPgUp/Dn scroll · Esc defer · F6 reopen"
    };
    f.render_widget(
        Paragraph::new(if notice.is_empty() {
            hints.to_owned()
        } else {
            safe(notice)
        })
        .wrap(Wrap { trim: false }),
        help,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use declass_agent::questions::QuestionOption;
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::from(code))
    }
    fn save() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
    }
    fn snapshot() -> PlanSnapshot {
        PlanSnapshot {
            identity: PlanIdentity {
                revision: "r1".into(),
                digest: "a".repeat(64),
            },
            draft: Draft {
                title: "Fix billing".into(),
                objective: "Calculate totals".into(),
                decisions: vec!["Keep API".into()],
                steps: vec![Step {
                    id: "s1".into(),
                    title: "Implement".into(),
                    description: "Use exact cents".into(),
                    paths: vec!["src/billing.rs".into()],
                    acceptance: vec!["Stable API".into()],
                    checks: vec![Check {
                        id: "c1".into(),
                        command: "cargo test".into(),
                        description: "Totals".into(),
                    }],
                }],
                ..Default::default()
            },
            location: "plans/r1.md".into(),
            changes: Some("Added exact cents".into()),
            verification: vec!["c1 passed: exit 0".into()],
            questions: vec!["q1 answered: keep API".into()],
            status: "Draft".into(),
            approved: false,
            can_implement: true,
            can_resume: false,
            steps: vec![],
        }
    }
    fn panel() -> Panel {
        let mut p = Panel::default();
        p.snapshot(Some(snapshot()));
        p.show(false);
        p
    }
    fn question() -> PlanQuestion {
        PlanQuestion {
            id: "q1".into(),
            identity: Some(snapshot().identity),
            question: "Choose storage".into(),
            options: QuestionOptions {
                id: "q1".into(),
                choices: vec![
                    QuestionOption {
                        id: "a".into(),
                        label: "Local".into(),
                        description: Some("Keep existing files".into()),
                    },
                    QuestionOption {
                        id: "b".into(),
                        label: "Database".into(),
                        description: None,
                    },
                ],
                allow_freeform: true,
                recommended: Some("a".into()),
            },
        }
    }
    fn draw(p: &Panel, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| p.draw(f, f.area())).unwrap();
        let b = t.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| b[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn review_defaults_to_close_and_execution_requires_explicit_selection() {
        let mut p = panel();
        assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        assert!(!p.open);
        p.show(false);
        for _ in 0..4 {
            p.event(key(KeyCode::Tab));
        }
        assert!(
            matches!(p.event(key(KeyCode::Enter)),Effect::Action(PlanAction::Implement{identity}) if identity==snapshot().identity)
        );
        let mut s = snapshot();
        s.can_implement = false;
        p.snapshot(Some(s));
        p.show(false);
        for _ in 0..4 {
            p.event(key(KeyCode::Tab));
        }
        assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        assert!(p.open);
    }
    #[test]
    fn edit_waits_for_controller_permission_before_opening_buffer() {
        let mut p = panel();
        p.event(key(KeyCode::Tab));
        assert!(
            matches!(p.event(key(KeyCode::Enter)),Effect::Action(PlanAction::Edit{identity}) if identity==snapshot().identity)
        );
        assert!(p.editing.is_none());
        assert!(!p.open);
        p.show(true);
        assert!(p.editing.is_some());
    }
    #[test]
    fn failed_save_retains_draft_and_requires_explicit_success_acknowledgment() {
        let mut p = panel();
        p.show(true);
        p.paste(" edited");
        assert!(matches!(
            p.event(save()),
            Effect::Action(PlanAction::Save { .. })
        ));
        assert!(matches!(p.event(save()), Effect::None));
        p.paste(" must not enter pending save");
        p.event(key(KeyCode::Esc));
        assert!(!p.open);
        assert!(p.editing.is_some());
        p.show(false);
        assert!(p.open);
        assert!(p.editing.is_some());
        assert!(matches!(
            p.event(Event::Key(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL
            ))),
            Effect::Interrupt
        ));
        p.error("Title is invalid".into());
        assert!(!p.pending_save);
        assert!(p.open);
        assert_eq!(
            p.editing.as_ref().unwrap().editor.buffer(),
            "Fix billing edited"
        );
        assert!(draw(&p, 80, 24).contains("Save failed: Title is invalid"));
        p.paste(" corrected");
        assert!(
            matches!(p.event(save()),Effect::Action(PlanAction::Save{draft,..}) if draft.title=="Fix billing edited corrected")
        );
        let mut newer = snapshot();
        newer.identity.revision = "r2".into();
        newer.identity.digest = "b".repeat(64);
        p.snapshot(Some(newer.clone()));
        assert!(p.editing.is_some());
        assert!(p.pending_save);
        p.saved(newer);
        assert!(p.editing.is_none());
        assert!(!p.pending_save);
        assert!(p.open);
        assert_eq!(p.focus, 0);
        assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        assert!(!p.open);
    }
    #[test]
    fn review_includes_location_previous_changes_questions_and_real_check_evidence() {
        let text = review_text(&snapshot());
        for expected in [
            "Saved plan: plans/r1.md",
            "Changes from previous revision",
            "Added exact cents",
            "Questions",
            "q1 answered: keep API",
            "Check evidence",
            "c1 passed: exit 0",
        ] {
            assert!(text.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn save_is_typed_and_keeps_structured_fields_and_ids() {
        let mut p = panel();
        p.show(true);
        p.paste(" updated");
        assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        let Effect::Action(PlanAction::Save { identity, draft }) = p.event(save()) else {
            panic!("expected save");
        };
        assert_eq!(identity, snapshot().identity);
        assert!(draft.title.ends_with(" updated\n"));
        assert_eq!(draft.steps, snapshot().draft.steps);
        assert_eq!(draft.decisions, snapshot().draft.decisions);
        assert!(p.open);
        assert!(p.pending_save);
        assert!(p.editing.is_some());
    }
    #[test]
    fn stale_revision_blocks_save_approval_implementation_and_answers() {
        let mut p = panel();
        p.show(true);
        p.paste(" unsaved");
        let mut newer = snapshot();
        newer.identity.revision = "r2".into();
        newer.identity.digest = "b".repeat(64);
        p.snapshot(Some(newer));
        assert!(matches!(p.event(save()), Effect::None));
        assert!(
            p.editing
                .as_ref()
                .unwrap()
                .editor
                .buffer()
                .ends_with(" unsaved")
        );
        p.editing = None;
        for focus in [3, 4, 5, 6] {
            p.focus = focus;
            assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        }
        p.question(Some(question()));
        p.event(key(KeyCode::Char('1')));
        assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        assert!(p.question.as_ref().unwrap().open);
    }
    #[test]
    fn structural_changes_are_local_until_save_and_cancel_discards_them() {
        let mut p = panel();
        p.show(true);
        p.event(key(KeyCode::F(7)));
        p.paste("Second");
        p.event(key(KeyCode::F(8)));
        p.paste("cargo check");
        let Effect::Action(PlanAction::Save { draft, .. }) = p.event(save()) else {
            panic!("save");
        };
        assert_eq!(draft.steps.len(), 2);
        assert_eq!(draft.steps[1].title, "Second");
        assert_eq!(draft.steps[1].id, "");
        assert_eq!(draft.steps[1].checks[0].command, "cargo check");
        p.saved(snapshot());
        p.show(true);
        p.event(key(KeyCode::F(7)));
        p.event(key(KeyCode::Esc));
        assert!(p.editing.is_none());
        assert_eq!(p.latest.unwrap().draft.steps.len(), 1);
    }
    #[test]
    fn clarification_requires_a_choice_and_never_selects_recommended_implicitly() {
        let mut p = panel();
        p.question(Some(question()));
        assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        p.event(key(KeyCode::Char('0')));
        assert!(p.question.as_ref().unwrap().selected.is_none());
        p.event(key(KeyCode::Char('2')));
        assert!(
            matches!(p.event(key(KeyCode::Enter)),Effect::Action(PlanAction::Answer{question_id,identity:Some(identity),choice_id:Some(choice_id),text}) if question_id=="q1"&&identity==snapshot().identity&&choice_id=="b"&&text=="Database")
        );
    }
    #[test]
    fn identical_question_refresh_preserves_dismissal_selection_and_text() {
        let mut p = panel();
        assert!(p.question(Some(question())));
        p.event(key(KeyCode::Char('2')));
        p.event(key(KeyCode::Char('f')));
        p.paste("Custom answer");
        p.event(key(KeyCode::Esc));
        p.event(key(KeyCode::Esc));
        assert!(!p.question_active());
        assert!(!p.question(Some(question())));
        assert!(!p.question_active());
        let q = p.question.as_ref().unwrap();
        assert_eq!(q.selected, Some(1));
        assert_eq!(q.editor.buffer(), "Custom answer");
        p.reopen_question();
        assert!(p.question_active());
        let mut next = question();
        next.id = "q2".into();
        next.options.id = "q2".into();
        assert!(p.question(Some(next)));
        assert!(p.question.as_ref().unwrap().selected.is_none());
        assert!(!p.question(None));
        assert!(!p.question_active());
        assert!(p.open);
    }

    #[test]
    fn clarification_defer_reopen_and_freeform_do_not_change_plan_draft() {
        let mut p = panel();
        p.show(true);
        let original = p.editing.as_ref().unwrap().editor.buffer().to_owned();
        p.question(Some(question()));
        p.paste("must not change hidden editor");
        assert_eq!(p.editing.as_ref().unwrap().editor.buffer(), original);
        p.event(key(KeyCode::Esc));
        p.reopen_question();
        p.event(key(KeyCode::Char('f')));
        p.paste("Use encrypted files");
        assert!(matches!(p.event(key(KeyCode::Enter)), Effect::None));
        assert!(
            matches!(p.event(save()),Effect::Action(PlanAction::Answer{choice_id:None,text,..}) if text=="Use encrypted files\n")
        );
        assert_eq!(p.editing.as_ref().unwrap().editor.buffer(), original);
    }
    #[test]
    fn narrow_review_editor_and_question_render_and_escape_text() {
        let mut p = panel();
        p.latest.as_mut().unwrap().draft.title = "Plan\x1b[2J".into();
        p.show(false);
        for (w, h) in [(40, 10), (60, 15), (100, 30)] {
            let text = draw(&p, w, h);
            assert!(text.contains("Plan r1"));
            assert!(!text.contains('\x1b'));
            p.show(true);
            assert!(draw(&p, w, h).contains("Title"));
            p.event(key(KeyCode::Esc));
            p.question(Some(question()));
            assert!(draw(&p, w, h).contains("Clarification"));
            p.question(None);
        }
        p.shown.as_mut().unwrap().draft.steps[0].checks.clear();
        assert!(review_text(p.shown.as_ref().unwrap()).contains("completion remains unverified"));
    }
    #[test]
    fn progress_refresh_preserves_unsaved_editor_and_revision_identity() {
        let mut p = panel();
        p.show(true);
        p.paste(" draft");
        let mut updated = snapshot();
        updated.status = "Paused".into();
        updated.steps.push(PlanStepProgress {
            id: "s1".into(),
            status: "Completed-unverified".into(),
            note: Some("No checks".into()),
        });
        p.snapshot(Some(updated));
        assert!(
            p.editing
                .as_ref()
                .unwrap()
                .editor
                .buffer()
                .ends_with(" draft")
        );
        assert!(p.current());
        assert!(review_text(p.shown.as_ref().unwrap()).contains("Completed-unverified"));
    }
}
