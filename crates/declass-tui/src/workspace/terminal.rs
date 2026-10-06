// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace owns the terminal while it runs.
//!
//! It draws on the controlling terminal (`/dev/tty`), in the alternate screen,
//! with raw keys, mouse wheel and bracketed paste. Meanwhile the process's
//! standard output and error are a pipe whose lines the workspace shows in the
//! conversation ([`Capture`]): a line printed by any code (a warning, a
//! setup message, a child process that inherited standard error) can neither
//! corrupt the screen nor be lost. Everything is given back on exit, on a
//! panic and for Ctrl-Z.

use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};

/// Whether the terminal is taken, and whether the keyboard flags were pushed.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static KEYBOARD: AtomicBool = AtomicBool::new(false);

/// The process's own standard output and error while they are captured.
static SAVED: Mutex<Option<(OwnedFd, OwnedFd)>> = Mutex::new(None);

/// The controlling terminal, for drawing.
pub(super) fn tty() -> std::io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
}

/// Takes the terminal: raw keys, the alternate screen, mouse wheel, bracketed
/// paste, and Shift-Enter told from Enter where the terminal can.
pub(super) fn enter(out: &mut File) -> std::io::Result<()> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            give_back();
            release_capture();
            previous(info);
        }));
    });
    ratatui::crossterm::terminal::enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    if ratatui::crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KEYBOARD.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// Gives the terminal back as it was (idempotent).
pub(super) fn give_back() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Ok(mut out) = tty() {
        if KEYBOARD.swap(false, Ordering::SeqCst) {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            out,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen,
            cursor::Show
        );
        let _ = out.flush();
    }
    let _ = ratatui::crossterm::terminal::disable_raw_mode();
}

/// Ctrl-Alt-Z: the terminal is given back and the job stops, as the shell
/// expects; it is taken again when the job continues.
pub(super) fn suspend(out: &mut File) {
    give_back();
    let _ = rustix::process::kill_current_process_group(rustix::process::Signal::Tstp);
    let _ = enter(out);
}

/// Sends an explicit copy request to the client terminal (including over SSH).
/// This is write-only OSC 52; the terminal may require clipboard permission.
pub(super) fn copy_to_terminal(text: &str, tty: &mut File) -> std::io::Result<()> {
    if text.len() > 1024 * 1024 {
        return Err(std::io::Error::other(
            "selection exceeds the 1 MiB copy limit",
        ));
    }
    write!(tty, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    tty.flush()
}

/// Standard base64 (RFC 4648), for OSC 52.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard output and error redirected to a pipe; each line read from it is
/// handed to `line`.
pub(super) struct Capture;

impl Capture {
    pub(super) fn start(line: impl Fn(String) + Send + 'static) -> std::io::Result<Capture> {
        let (read, write) = rustix::pipe::pipe()?;
        let out = rustix::io::dup(std::io::stdout())?;
        let err = rustix::io::dup(std::io::stderr())?;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        rustix::stdio::dup2_stdout(&write)?;
        if let Err(e) = rustix::stdio::dup2_stderr(&write) {
            let _ = rustix::stdio::dup2_stdout(&out);
            return Err(e.into());
        }
        *SAVED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((out, err));
        drop(write);
        std::thread::Builder::new()
            .name("workspace-capture".into())
            .spawn(move || {
                let reader = BufReader::new(File::from(read));
                for l in reader.split(b'\n').map_while(Result::ok) {
                    line(String::from_utf8_lossy(&l).into_owned());
                }
            })?;
        Ok(Capture)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        release_capture();
    }
}

/// Standard output and error are the process's own again (idempotent). What
/// was printed but not yet read stays in the pipe for the reader.
fn release_capture() {
    let saved = SAVED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some((out, err)) = saved {
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        let _ = rustix::stdio::dup2_stdout(&out);
        let _ = rustix::stdio::dup2_stderr(&err);
    }
}

#[cfg(test)]
mod copy_tests {
    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64("declass ◆".as_bytes()), "ZGVjbGFzcyDil4Y=");
    }
}
