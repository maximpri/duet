// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit clipboard jobs, conversation search and composer mouse selection.
use super::*;
use crate::clipboard::Content;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};

pub(super) struct ClipboardJob {
    pub receiving: bool,
    rx: Receiver<ClipboardResult>,
}

enum ClipboardResult {
    Read {
        epoch: u64,
        result: Result<Content, String>,
    },
    Written {
        text: String,
        result: Result<(), String>,
    },
    Attached(Result<String, String>),
}

#[derive(Default)]
pub(super) struct Find {
    pub open: bool,
    pub query: Editor,
    pub matches: Vec<usize>,
    pub current: usize,
}

impl State {
    /// Palette mouse input takes precedence over the conversation beneath it.
    pub(super) fn palette_mouse(&mut self, m: MouseEvent) -> bool {
        if self.overlay || self.help || self.answering || self.find.open {
            return false;
        }
        let Some(popup) = self.palette_popup.as_ref() else {
            return false;
        };
        if popup.query != self.editor.buffer() || !popup.area.contains((m.column, m.row).into()) {
            return false;
        }
        let count = self.palette_items().len();
        if count == 0 {
            return false;
        }
        self.palette = self.palette.min(count - 1);
        match m.kind {
            MouseEventKind::ScrollUp => self.palette = self.palette.saturating_sub(3),
            MouseEventKind::ScrollDown => self.palette = (self.palette + 3).min(count - 1),
            MouseEventKind::Down(MouseButton::Left)
                if popup.rows.contains((m.column, m.row).into()) =>
            {
                // Selection is deliberate; Enter still runs the command.
                self.palette = (popup.first + usize::from(m.row - popup.rows.y)).min(count - 1);
                self.sel = None;
                self.selecting = false;
                self.editor_selecting = false;
            }
            _ => {}
        }
        true
    }

    pub(super) fn insert_paste(&mut self, text: &str) {
        let retained = self.editor.buffer().len() - self.editor.selected_text().map_or(0, str::len);
        if text.len() > (256 * 1024usize).saturating_sub(retained) {
            self.notice("Paste would exceed 256 KiB; use /attach PATH");
            return;
        }
        self.sel = None;
        self.editor.insert(text);
        self.palette = 0;
        self.palette_start = 0;
        self.pick = 0;
        self.notice("Text pasted · Enter sends");
    }

    pub(super) fn notice(&mut self, text: impl Into<String>) {
        self.toast = Some((Instant::now(), safe(&text.into())));
        self.dirty = true;
    }

    pub(super) fn copy_text(&mut self, text: String) {
        if self.clipboard.is_some() {
            self.notice("Clipboard busy; try again in a moment");
            return;
        }
        if text.len() > 1024 * 1024 {
            self.notice("Selection exceeds 1 MiB; select a smaller section");
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.clipboard = Some(ClipboardJob {
            receiving: false,
            rx,
        });
        std::thread::spawn(move || {
            let result = crate::clipboard::write_text(&text).map_err(|e| format!("{e:#}"));
            let _ = tx.send(ClipboardResult::Written { text, result });
        });
        self.notice("Copying…");
    }

    pub(super) fn copy_reply(&mut self) {
        let text = self.cells.list.iter().rev().find_map(|cell| {
            if !matches!(
                cell.kind,
                cells::Kind::Declass | cells::Kind::Asks | cells::Kind::Done
            ) {
                return None;
            }
            match &cell.body {
                cells::Body::Markdown(text) if !text.is_empty() => Some(text.clone()),
                cells::Body::Lines(lines) if !lines.is_empty() => Some(lines.join("\n")),
                _ => None,
            }
        });
        match text {
            Some(text) => self.copy_text(text),
            None => self.notice("No reply to copy yet"),
        }
    }

    pub(super) fn paste(&mut self) {
        if self.clipboard.is_some() {
            self.notice("Clipboard busy; try again in a moment");
            return;
        }
        let epoch = self.input_epoch;
        let (tx, rx) = mpsc::channel();
        self.clipboard = Some(ClipboardJob {
            receiving: true,
            rx,
        });
        std::thread::spawn(move || {
            let result = crate::clipboard::read().map_err(|e| format!("{e:#}"));
            let _ = tx.send(ClipboardResult::Read { epoch, result });
        });
        self.notice("Reading clipboard…");
    }

    pub(super) fn poll_clipboard(&mut self, tty: &mut File) {
        let Some(job) = self.clipboard.as_ref() else {
            return;
        };
        let result = match job.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.clipboard = None;
                self.notice("Clipboard helper stopped; try again or use /image PATH");
                return;
            }
        };
        self.clipboard = None;
        match result {
            ClipboardResult::Read { epoch, result } => {
                if epoch != self.input_epoch
                    || self.overlay
                    || self.help
                    || self.answering
                    || self.find.open
                {
                    self.notice("Paste cancelled because input changed; paste again when ready");
                    return;
                }
                match result {
                    Ok(Content::Text(text)) => {
                        if text.len() > 256 * 1024 {
                            self.notice("Clipboard text exceeds 256 KiB; attach it as a file with /attach PATH");
                        } else {
                            if self.plan.active() {
                                self.plan.paste(&text);
                            } else {
                                self.insert_paste(&text);
                            }
                            self.input_epoch = self.input_epoch.wrapping_add(1);
                        }
                    }
                    Ok(Content::Image { .. }) if self.plan.active() => {
                        self.notice("Plan fields accept text; attach images in the conversation");
                    }
                    Ok(Content::Image { bytes, .. }) => {
                        let hook = self.hooks.paste_image.clone();
                        let (tx, rx) = mpsc::channel();
                        self.clipboard = Some(ClipboardJob {
                            receiving: true,
                            rx,
                        });
                        std::thread::spawn(move || {
                            let _ = tx.send(ClipboardResult::Attached(hook(bytes)));
                        });
                        self.notice("Preparing image attachment…");
                    }
                    Err(error) => {
                        self.warn(&format!("Paste: {error}"));
                        self.notice("Paste unavailable · see conversation for help");
                    }
                }
            }
            ClipboardResult::Written { text, result } => match result {
                Ok(()) => self.notice(format!("Copied {} characters", text.chars().count())),
                Err(_) => match terminal::copy_to_terminal(&text, tty) {
                    Ok(()) => {
                        self.notice("Copy sent to terminal · clipboard permission may be needed")
                    }
                    Err(error) => self.notice(format!("Copy failed: {error}")),
                },
            },
            ClipboardResult::Attached(result) => match result {
                Ok(message) => self.notice(message),
                Err(error) => {
                    self.warn(&format!("Image paste: {error}"));
                    self.notice("Image not attached · see conversation for help");
                }
            },
        }
    }

    pub(super) fn open_find(&mut self) {
        self.find.open = true;
        self.find.query.select_all();
        self.refresh_find();
    }

    fn refresh_find(&mut self) {
        let query = self.find.query.buffer().to_lowercase();
        self.find.matches.clear();
        self.find.current = 0;
        if query.is_empty() {
            return;
        }
        let layout = self.transcript(self.width, Instant::now());
        let chunks = std::iter::once(layout.leading.as_slice())
            .chain(self.cells.row_chunks())
            .chain(std::iter::once(layout.trailing.as_slice()));
        for (row, line) in chunks.flatten().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            if text.to_lowercase().contains(&query) {
                self.find.matches.push(row);
            }
        }
        if let Some(index) = self.find.matches.iter().position(|&r| r >= self.view_top) {
            self.find.current = index;
        }
        self.jump_find();
    }

    fn jump_find(&mut self) {
        if let Some(&row) = self.find.matches.get(self.find.current) {
            let layout = self.transcript(self.width, Instant::now());
            self.total = layout.total;
            let top = row.saturating_sub(self.page / 2);
            self.scroll = self.total.saturating_sub(self.page).saturating_sub(top);
            self.sel = Some(((row, 0), (row, usize::MAX)));
            self.dirty = true;
        }
    }

    pub(super) fn next_find(&mut self, previous: bool) {
        // Recompute after resizing or new streamed output before navigating.
        let old_row = self.find.matches.get(self.find.current).copied();
        self.refresh_find();
        let count = self.find.matches.len();
        if count > 0 {
            if let Some(index) =
                old_row.and_then(|r| self.find.matches.iter().position(|&v| v == r))
            {
                self.find.current = index;
            }
            self.find.current = (self.find.current + if previous { count - 1 } else { 1 }) % count;
            self.jump_find();
        }
    }

    pub(super) fn find_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.find.open = false,
            KeyCode::Enter | KeyCode::F(3) => {
                self.next_find(k.modifiers.contains(KeyModifiers::SHIFT))
            }
            _ => {
                let before = self.find.query.buffer().to_owned();
                match self.find.query.key(k) {
                    Outcome::Interrupt | Outcome::Eof => self.find.open = false,
                    Outcome::Copy(text) => self.copy_text(text),
                    Outcome::Paste => {
                        self.notice("Use your terminal's text paste shortcut in Find")
                    }
                    _ => {}
                }
                if before != self.find.query.buffer() {
                    self.refresh_find();
                }
            }
        }
    }

    pub(super) fn find_paste(&mut self, text: &str) {
        if text.len() <= 4096 {
            self.find.query.insert(&text.replace(['\r', '\n'], " "));
            self.refresh_find();
        } else {
            self.notice("Search text is too long (maximum 4 KiB)");
        }
    }

    pub(super) fn mouse(&mut self, m: MouseEvent) {
        let input = self.composer;
        let inside_input = input.contains((m.column, m.row).into());
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if self.help {
                    self.help_scroll = if up {
                        self.help_scroll.saturating_sub(3)
                    } else {
                        self.help_scroll.saturating_add(3)
                    };
                } else if !self.answering && !self.find.open {
                    if m.column >= self.view.right().saturating_add(2) && m.row < input.y {
                        self.panel.scroll_diff(if up { -3 } else { 3 });
                    } else {
                        self.scroll_by(if up { 3 } else { -3 });
                    }
                }
            }
            _ if self.help || self.answering || self.find.open => {}
            MouseEventKind::Down(MouseButton::Left) => {
                self.editor_selecting = inside_input;
                self.selecting = false;
                if inside_input {
                    self.sel = None;
                    self.editor.select_at(
                        &[],
                        input.width as usize,
                        6,
                        (m.row - input.y) as usize,
                        (m.column - input.x) as usize,
                        m.modifiers.contains(KeyModifiers::SHIFT),
                    );
                } else {
                    self.sel = self.at(m.column, m.row, false).map(|p| (p, p));
                    self.selecting = self.sel.is_some();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.editor_selecting => {
                self.editor.select_at(
                    &[],
                    input.width as usize,
                    6,
                    m.row
                        .saturating_sub(input.y)
                        .min(input.height.saturating_sub(1)) as usize,
                    m.column.saturating_sub(input.x).min(input.width) as usize,
                    true,
                );
            }
            MouseEventKind::Drag(MouseButton::Left) if self.selecting => {
                if m.row < self.view.y {
                    self.scroll_by(1);
                } else if m.row >= self.view.bottom() {
                    self.scroll_by(-1);
                }
                if let (Some((a, _)), Some(p)) = (self.sel, self.at(m.column, m.row, true)) {
                    self.sel = Some((a, p));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.editor_selecting = false;
                self.selecting = false;
                if self.sel.is_some_and(|(a, b)| a == b) {
                    self.sel = None;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn state() -> (State, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let interrupted = Arc::new(AtomicUsize::new(0));
        let lines = sent.clone();
        let interrupts = interrupted.clone();
        let hooks = Hooks {
            plan_action: Box::new(|_| {}),
            line: Box::new(move |line| lines.lock().unwrap().push(line)),
            eof: Box::new(|| {}),
            interrupt: Box::new(move || {
                interrupts.fetch_add(1, Ordering::SeqCst);
                None
            }),
            answering: Box::new(|| false),
            paste_image: Arc::new(|_| Ok("Image queued".into())),
        };
        (State::new(false, hooks, &[]), sent, interrupted)
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn ready(s: &mut State, result: ClipboardResult) {
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        s.clipboard = Some(ClipboardJob {
            receiving: true,
            rx,
        });
    }

    #[test]
    fn pasted_text_is_atomic_unsent_and_stale_paste_never_changes_a_new_draft() {
        let (mut s, sent, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.editor.insert("old draft");
        s.editor.select_all();
        ready(
            &mut s,
            ClipboardResult::Read {
                epoch: 0,
                result: Ok(Content::Text("alpha\r\n中👩🏽‍💻\n".into())),
            },
        );
        s.poll_clipboard(&mut tty);
        assert_eq!(s.editor.buffer(), "alpha\n中👩🏽‍💻\n");
        assert!(sent.lock().unwrap().is_empty());
        s.event(key(KeyCode::Char('z'), KeyModifiers::CONTROL), &mut tty);
        assert_eq!(s.editor.buffer(), "old draft");
        ready(
            &mut s,
            ClipboardResult::Read {
                epoch: 0,
                result: Ok(Content::Text("stale".into())),
            },
        );
        s.poll_clipboard(&mut tty);
        assert_eq!(s.editor.buffer(), "old draft");
        assert!(s.toast.as_ref().unwrap().1.contains("cancelled"));
    }

    #[test]
    fn paste_waits_for_image_queue_before_enter_can_send() {
        let (mut s, sent, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        let (tx, rx) = mpsc::channel();
        s.clipboard = Some(ClipboardJob {
            receiving: true,
            rx,
        });
        s.editor.insert("Review the image");
        s.event(key(KeyCode::Enter, KeyModifiers::NONE), &mut tty);
        assert!(sent.lock().unwrap().is_empty());
        assert_eq!(s.editor.buffer(), "Review the image");
        tx.send(ClipboardResult::Attached(Ok("Image queued".into())))
            .unwrap();
        s.poll_clipboard(&mut tty);
        s.event(key(KeyCode::Enter, KeyModifiers::NONE), &mut tty);
        assert_eq!(*sent.lock().unwrap(), ["Review the image"]);
    }

    #[test]
    fn modal_paste_cannot_change_a_draft_or_answer_approval() {
        let (mut s, sent, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.editor.insert("keep");
        s.answering = true;
        s.event(Event::Paste("y\n".into()), &mut tty);
        assert_eq!(s.editor.buffer(), "keep");
        assert!(!s.approve);
        s.answering = false;
        s.help = true;
        s.event(Event::Paste("/quit\n".into()), &mut tty);
        assert_eq!(s.editor.buffer(), "keep");
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn selection_copy_never_interrupts_or_automatically_copies_on_mouse_up() {
        let (mut s, sent, interrupted) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.cells.you("select this text");
        let mut screen = Terminal::new(TestBackend::new(100, 28)).unwrap();
        screen
            .draw(|f| view::draw(f, &mut s, Instant::now()))
            .unwrap();
        s.sel = Some(((0, 0), (s.total, usize::MAX)));
        s.selecting = true;
        s.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 4,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            s.clipboard.is_none(),
            "selection alone never writes the clipboard"
        );
        // A held fake job exercises copy routing without touching the OS clipboard.
        let (_tx, rx) = mpsc::channel();
        s.clipboard = Some(ClipboardJob {
            receiving: false,
            rx,
        });
        s.event(key(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut tty);
        assert_eq!(interrupted.load(Ordering::SeqCst), 0);
        assert!(s.sel.is_some());
        s.sel = None;
        s.event(key(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut tty);
        assert_eq!(interrupted.load(Ordering::SeqCst), 1);
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn find_and_palette_preserve_drafts_and_search_never_reaches_the_model() {
        let (mut s, sent, _) = state();
        let mut tty = tempfile::tempfile().unwrap();
        s.editor.insert("unfinished draft");
        s.cells.text("First needle in the conversation");
        s.cells.text("Second NEEDLE in the conversation");
        let mut screen = Terminal::new(TestBackend::new(100, 28)).unwrap();
        screen
            .draw(|f| view::draw(f, &mut s, Instant::now()))
            .unwrap();
        s.event(key(KeyCode::Char('f'), KeyModifiers::CONTROL), &mut tty);
        s.event(Event::Paste("needle".into()), &mut tty);
        assert_eq!(s.find.matches.len(), 2);
        let first = s.find.current;
        s.event(key(KeyCode::Enter, KeyModifiers::NONE), &mut tty);
        assert_ne!(s.find.current, first);
        s.event(key(KeyCode::Esc, KeyModifiers::NONE), &mut tty);
        assert_eq!(s.editor.buffer(), "unfinished draft");
        s.event(key(KeyCode::F(4), KeyModifiers::NONE), &mut tty);
        assert_eq!(s.editor.buffer(), "/");
        s.editor.insert("image ");
        s.event(key(KeyCode::Esc, KeyModifiers::NONE), &mut tty);
        assert_eq!(s.editor.buffer(), "unfinished draft");
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn composer_mouse_selection_and_chips_render_without_terminal_controls() {
        let (mut s, _, _) = state();
        s.editor.insert("abc中👩🏽‍💻def");
        s.message(Msg::Attachments(vec![AttachmentChip {
            id: "i1".into(),
            label: "shot.png\x1b]52;c;bad\x07".into(),
        }]));
        let mut screen = Terminal::new(TestBackend::new(100, 28)).unwrap();
        screen
            .draw(|f| view::draw(f, &mut s, Instant::now()))
            .unwrap();
        let row = s.composer.y;
        let start = s.composer.x + 3;
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), start),
            (MouseEventKind::Drag(MouseButton::Left), start + 4),
        ] {
            s.mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        }
        assert_eq!(s.editor.selected_text(), Some("中👩🏽‍💻"));
        screen
            .draw(|f| view::draw(f, &mut s, Instant::now()))
            .unwrap();
        assert!(
            screen.backend().buffer()[(start, row)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
        let rendered: String = screen
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(rendered.contains("Attached"));
        assert!(rendered.contains("shot.png"));
        assert!(!rendered.contains('\x1b'));
        assert!(!rendered.contains("52;c;bad"));
        assert_eq!(columns("a👩🏽‍💻中éz", 2, 6), "👩🏽‍💻中é");
    }

    #[test]
    fn recovering_an_unsent_message_never_overwrites_new_input() {
        let (mut s, _, _) = state();
        s.message(Msg::RecoverDraft("first unsent".into()));
        assert_eq!(s.editor.buffer(), "first unsent");
        s.message(Msg::RecoverDraft("second unsent".into()));
        assert_eq!(s.editor.buffer(), "first unsent");
        assert!(s.cells.list.iter().any(|c| matches!(&c.body, cells::Body::Lines(lines) if lines.join("\n").contains("second unsent"))));
    }
}
