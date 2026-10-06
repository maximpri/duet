// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, local clipboard actions. No polling, network or OSC 52 queries.
//!
//! Native commands use fixed arguments, bounded pipes and one action deadline.
//! Text is stdin data, never script source. Image output is decoded and encoded
//! as PNG before returning; ordinary attachment privacy rules still apply.
//!
//! macOS uses its system osascript; Homebrew is unnecessary. Linux helpers are
//! accepted from /usr/bin, /bin, /usr/local/bin or /run/current-system/sw/bin
//! only when their resolved executable and ancestors are root-owned and not
//! group/world writable. User-managed PATH and Linuxbrew helpers are not used.
//!
//! Platform contracts:
//! <https://developer.apple.com/documentation/appkit/nspasteboard>
//! <https://github.com/bugaevc/wl-clipboard/blob/master/data/wl-clipboard.1>
//! <https://github.com/astrand/xclip/blob/master/xclip.1>
//! <https://github.com/kfish/xsel/blob/master/xsel.1x>

use anyhow::{Context, Result, bail, ensure};
use image::{ImageEncoder, ImageFormat, ImageReader, Limits};
use std::fmt;
use std::io::{Cursor, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const MAX_TEXT: usize = 1024 * 1024;
const MAX_IMAGE: usize = 16 * 1024 * 1024;
const MAX_TYPES: usize = 16 * 1024;
const MAX_STDERR: usize = 16 * 1024;
const MAX_SIDE: u32 = 16_384;
const MAX_PIXELS: u64 = 40_000_000;
const TIMEOUT: Duration = Duration::from_secs(3);
const IMAGE_FALLBACK: &str = "Save the image and attach it with /image PATH.";

#[derive(PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image { bytes: Vec<u8>, media_type: String },
}

// Clipboard text must not be accidentally disclosed by debug logging.
impl fmt::Debug for Content {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.debug_struct("Text").field("bytes", &text.len()).finish(),
            Self::Image { bytes, media_type } => f
                .debug_struct("Image")
                .field("bytes", &bytes.len())
                .field("media_type", media_type)
                .finish(),
        }
    }
}

/// Read only in response to the operator's paste action, on a worker thread.
pub fn read() -> Result<Content> {
    read_with(&Environment::current(), &Native, Instant::now() + TIMEOUT)
}

/// Copy on a worker thread. Failure is returned so the host can report it or
/// choose a write-only terminal clipboard mechanism; success is never assumed.
pub fn write_text(text: &str) -> Result<()> {
    write_with(
        &Environment::current(),
        &Native,
        text,
        Instant::now() + TIMEOUT,
    )
}

#[derive(Clone, Copy)]
enum Platform {
    Mac,
    Linux,
    Unsupported,
}

struct Environment {
    platform: Platform,
    ssh: bool,
    wayland: bool,
    local_x11: bool,
}

impl Environment {
    fn current() -> Self {
        let present = |key: &str| std::env::var_os(key).is_some_and(|v| !v.is_empty());
        Self {
            platform: if cfg!(target_os = "macos") {
                Platform::Mac
            } else if cfg!(target_os = "linux") {
                Platform::Linux
            } else {
                Platform::Unsupported
            },
            ssh: ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
                .iter()
                .any(|key| present(key)),
            wayland: present("WAYLAND_DISPLAY"),
            local_x11: std::env::var("DISPLAY").is_ok_and(|display| local_display(&display)),
        }
    }

    fn check(&self) -> Result<()> {
        ensure!(
            !self.ssh,
            "Native clipboard access is unavailable over SSH. Paste text through your terminal. {IMAGE_FALLBACK}"
        );
        Ok(())
    }
}

fn local_display(display: &str) -> bool {
    let display = display.strip_prefix("unix").unwrap_or(display);
    let Some(number) = display.strip_prefix(':') else {
        return false;
    };
    let mut parts = number.split('.');
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    parts.next().is_some_and(digits) && parts.next().is_none_or(digits) && parts.next().is_none()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Osascript,
    WlPaste,
    WlCopy,
    Xclip,
    Xsel,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Self::Osascript => "osascript",
            Self::WlPaste => "wl-paste",
            Self::WlCopy => "wl-copy",
            Self::Xclip => "xclip",
            Self::Xsel => "xsel",
        }
    }
}

trait Runner {
    fn available(&self, tool: Tool) -> bool;
    fn run(
        &self,
        tool: Tool,
        args: &[&str],
        input: Option<&[u8]>,
        cap: usize,
        deadline: Instant,
    ) -> Result<Vec<u8>>;
}

fn unavailable() -> anyhow::Error {
    anyhow::anyhow!(
        "No trusted local clipboard helper is available. Install wl-clipboard (Wayland) or xclip/xsel (X11) with your system package manager. Paste text through your terminal. {IMAGE_FALLBACK}"
    )
}

fn read_with(env: &Environment, runner: &dyn Runner, deadline: Instant) -> Result<Content> {
    env.check()?;
    match env.platform {
        Platform::Mac => {
            let packet = runner.run(
                Tool::Osascript,
                &["-l", "JavaScript", "-e", MAC_READ],
                None,
                MAX_IMAGE + 64,
                deadline,
            )?;
            let separator = packet
                .iter()
                .position(|b| *b == b'\n')
                .context("Clipboard returned an invalid response")?;
            let (kind, bytes) = (&packet[..separator], &packet[separator + 1..]);
            match kind {
                b"text/plain" => text_content(bytes.to_vec()),
                b"image/png" => normalize_image(bytes),
                b"image/tiff" => {
                    let expected = tiff_dimensions(bytes)?;
                    check_dimensions(expected)?;
                    let png = runner.run(
                        Tool::Osascript,
                        &["-l", "JavaScript", "-e", MAC_TIFF],
                        Some(bytes),
                        MAX_IMAGE,
                        deadline,
                    )?;
                    let actual = ImageReader::with_format(Cursor::new(&png), ImageFormat::Png)
                        .into_dimensions()
                        .context("Clipboard TIFF conversion failed")?;
                    ensure!(
                        actual == expected,
                        "Clipboard image dimensions changed during conversion"
                    );
                    normalize_image(&png)
                }
                _ => bail!("Clipboard has no supported text or image. {IMAGE_FALLBACK}"),
            }
        }
        Platform::Linux if env.wayland && runner.available(Tool::WlPaste) => {
            let types = runner.run(Tool::WlPaste, &["--list-types"], None, MAX_TYPES, deadline)?;
            let kind = select_type(&types)?;
            let bytes = runner.run(
                Tool::WlPaste,
                &["--no-newline", "--type", &kind],
                None,
                content_cap(&kind),
                deadline,
            )?;
            typed_content(&kind, bytes)
        }
        Platform::Linux if env.local_x11 && runner.available(Tool::Xclip) => {
            let types = runner.run(
                Tool::Xclip,
                &["-selection", "clipboard", "-out", "-target", "TARGETS"],
                None,
                MAX_TYPES,
                deadline,
            )?;
            let kind = select_type(&types)?;
            let bytes = runner.run(
                Tool::Xclip,
                &["-selection", "clipboard", "-out", "-target", &kind],
                None,
                content_cap(&kind),
                deadline,
            )?;
            typed_content(&kind, bytes)
        }
        Platform::Linux if env.local_x11 && runner.available(Tool::Xsel) => {
            text_content(runner.run(
                Tool::Xsel,
                &["--clipboard", "--output", "--selectionTimeout", "1500"],
                None,
                MAX_TEXT,
                deadline,
            )?)
        }
        _ => Err(unavailable()),
    }
}

fn write_with(env: &Environment, runner: &dyn Runner, text: &str, deadline: Instant) -> Result<()> {
    env.check()?;
    ensure!(
        text.len() <= MAX_TEXT,
        "Selection is too large to copy (1 MiB limit)"
    );
    let input = Some(text.as_bytes());
    match env.platform {
        Platform::Mac => runner.run(
            Tool::Osascript,
            &["-l", "JavaScript", "-e", MAC_WRITE],
            input,
            MAX_TYPES,
            deadline,
        )?,
        Platform::Linux if env.wayland && runner.available(Tool::WlCopy) => runner.run(
            Tool::WlCopy,
            &["--type", "text/plain;charset=utf-8"],
            input,
            MAX_TYPES,
            deadline,
        )?,
        Platform::Linux if env.local_x11 && runner.available(Tool::Xclip) => runner.run(
            Tool::Xclip,
            &["-selection", "clipboard", "-in", "-target", "UTF8_STRING"],
            input,
            MAX_TYPES,
            deadline,
        )?,
        Platform::Linux if env.local_x11 && runner.available(Tool::Xsel) => runner.run(
            Tool::Xsel,
            &["--clipboard", "--input", "--logfile", "/dev/null"],
            input,
            MAX_TYPES,
            deadline,
        )?,
        _ => return Err(unavailable()),
    };
    Ok(())
}

fn select_type(bytes: &[u8]) -> Result<String> {
    ensure!(
        bytes.len() <= MAX_TYPES,
        "Clipboard format list is too large"
    );
    let text = std::str::from_utf8(bytes).context("Clipboard returned invalid format names")?;
    let types: Vec<_> = text.lines().map(str::trim).collect();
    for preferred in [
        "image/png",
        "image/jpeg",
        "image/webp",
        "image/gif",
        "text/plain;charset=utf-8",
        "UTF8_STRING",
        "text/plain",
        "STRING",
    ] {
        if types.contains(&preferred) {
            return Ok(preferred.to_owned());
        }
    }
    bail!("Clipboard has no supported text or image. {IMAGE_FALLBACK}")
}

fn content_cap(kind: &str) -> usize {
    if kind.starts_with("image/") {
        MAX_IMAGE
    } else {
        MAX_TEXT
    }
}

fn typed_content(kind: &str, bytes: Vec<u8>) -> Result<Content> {
    if kind.starts_with("image/") {
        normalize_image(&bytes)
    } else {
        text_content(bytes)
    }
}

fn text_content(bytes: Vec<u8>) -> Result<Content> {
    ensure!(!bytes.is_empty(), "Clipboard is empty. {IMAGE_FALLBACK}");
    ensure!(
        bytes.len() <= MAX_TEXT,
        "Clipboard text is too large (1 MiB limit)"
    );
    ensure!(!bytes.contains(&0), "Clipboard does not contain plain text");
    Ok(Content::Text(
        String::from_utf8(bytes).context("Clipboard text is not UTF-8")?,
    ))
}

fn check_dimensions((width, height): (u32, u32)) -> Result<()> {
    ensure!(
        width > 0
            && height > 0
            && width <= MAX_SIDE
            && height <= MAX_SIDE
            && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "Clipboard image is too large (40 megapixels or 16384 pixels per side). {IMAGE_FALLBACK}"
    );
    Ok(())
}

fn normalize_image(bytes: &[u8]) -> Result<Content> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_IMAGE,
        "Clipboard image exceeds 16 MiB. {IMAGE_FALLBACK}"
    );
    let format = image::guess_format(bytes).context("Clipboard image format is not recognized")?;
    ensure!(
        matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
        ),
        "Clipboard image format is unsupported. {IMAGE_FALLBACK}"
    );
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    check_dimensions(
        ImageReader::with_format(Cursor::new(bytes), format)
            .into_dimensions()
            .context("Clipboard image header is invalid")?,
    )?;
    let image = reader
        .decode()
        .context("Clipboard image is damaged or exceeds decoding limits")?
        .into_rgba8();
    let mut encoded = BoundedBytes {
        bytes: Vec::new(),
        cap: MAX_IMAGE,
    };
    image::codecs::png::PngEncoder::new(&mut encoded)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("Clipboard image cannot fit in a 16 MiB PNG")?;
    Ok(Content::Image {
        bytes: encoded.bytes,
        media_type: "image/png".into(),
    })
}

struct BoundedBytes {
    bytes: Vec<u8>,
    cap: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("clipboard output limit exceeded"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Classic TIFF dimensions are checked before asking AppKit to decode it.
/// Multiple-page and exotic TIFF layouts are refused; a saved PNG remains an option.
fn tiff_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    ensure!(
        bytes.len() <= MAX_IMAGE && bytes.len() >= 8,
        "Invalid clipboard TIFF"
    );
    let little = match &bytes[..4] {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => bail!("Unsupported clipboard TIFF. {IMAGE_FALLBACK}"),
    };
    let u16_at = |offset: usize| -> Result<u16> {
        let raw: [u8; 2] = bytes
            .get(offset..offset + 2)
            .context("Truncated TIFF")?
            .try_into()?;
        Ok(if little {
            u16::from_le_bytes(raw)
        } else {
            u16::from_be_bytes(raw)
        })
    };
    let u32_at = |offset: usize| -> Result<u32> {
        let raw: [u8; 4] = bytes
            .get(offset..offset + 4)
            .context("Truncated TIFF")?
            .try_into()?;
        Ok(if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    };
    let ifd = u32_at(4)? as usize;
    ensure!(
        ifd >= 8 && ifd <= bytes.len().saturating_sub(2),
        "Invalid TIFF directory"
    );
    let count = usize::from(u16_at(ifd)?);
    ensure!(count <= 256, "Clipboard TIFF has too many fields");
    let end = ifd + 2 + count * 12;
    ensure!(
        u32_at(end)? == 0,
        "Multiple-page clipboard TIFF is unsupported. {IMAGE_FALLBACK}"
    );
    let mut width = None;
    let mut height = None;
    let mut samples = 1_u32;
    let mut bits = 1_u32;
    for index in 0..count {
        let offset = ifd + 2 + index * 12;
        let tag = u16_at(offset)?;
        if tag == 258 {
            let n = u32_at(offset + 4)? as usize;
            ensure!(
                u16_at(offset + 2)? == 3 && (1..=4).contains(&n),
                "Unsupported TIFF sample layout"
            );
            let start = if n <= 2 {
                offset + 8
            } else {
                u32_at(offset + 8)? as usize
            };
            ensure!(
                start <= bytes.len().saturating_sub(n * 2),
                "Invalid TIFF sample layout"
            );
            for sample in 0..n {
                let depth = u32::from(u16_at(start + sample * 2)?);
                ensure!(
                    matches!(depth, 1 | 2 | 4 | 8 | 16),
                    "Unsupported TIFF bit depth"
                );
                bits = bits.max(depth);
            }
            continue;
        }
        if tag == 277 {
            ensure!(
                u16_at(offset + 2)? == 3 && u32_at(offset + 4)? == 1,
                "Unsupported TIFF sample layout"
            );
            samples = u32::from(u16_at(offset + 8)?);
            ensure!((1..=4).contains(&samples), "Unsupported TIFF sample count");
            continue;
        }
        if !matches!(tag, 256 | 257) {
            continue;
        }
        ensure!(u32_at(offset + 4)? == 1, "Invalid TIFF dimensions");
        let value = match u16_at(offset + 2)? {
            3 => u32::from(u16_at(offset + 8)?),
            4 => u32_at(offset + 8)?,
            _ => bail!("Invalid TIFF dimension type"),
        };
        let slot = if tag == 256 { &mut width } else { &mut height };
        ensure!(slot.replace(value).is_none(), "Duplicate TIFF dimensions");
    }
    let size = (
        width.context("TIFF width missing")?,
        height.context("TIFF height missing")?,
    );
    check_dimensions(size)?;
    ensure!(
        u64::from(size.0) * u64::from(size.1) * u64::from(samples) * u64::from(bits) / 8
            <= 256 * 1024 * 1024,
        "Clipboard TIFF exceeds decoding memory limits"
    );
    Ok(size)
}

struct Native;
impl Runner for Native {
    fn available(&self, tool: Tool) -> bool {
        helper(tool).is_some()
    }
    fn run(
        &self,
        tool: Tool,
        args: &[&str],
        input: Option<&[u8]>,
        cap: usize,
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        let program = helper(tool).ok_or_else(unavailable)?;
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "en_US.UTF-8");
        for key in [
            "HOME",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "XAUTHORITY",
            "TMPDIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        run_command(command, input, cap, deadline).with_context(|| {
            format!(
                "{} could not access the local clipboard. {IMAGE_FALLBACK}",
                tool.name()
            )
        })
    }
}

/// Do not search PATH: a repository-local executable must not receive clipboard data.
fn helper(tool: Tool) -> Option<PathBuf> {
    let directories: &[&str] = if tool == Tool::Osascript {
        &["/usr/bin"]
    } else {
        &[
            "/usr/bin",
            "/bin",
            "/usr/local/bin",
            "/run/current-system/sw/bin",
        ]
    };
    directories.iter().find_map(|dir| {
        let path = Path::new(dir).join(tool.name()).canonicalize().ok()?;
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
        if path.ancestors().all(|part| {
            std::fs::metadata(part)
                .is_ok_and(|m| m.uid() == 0 && m.permissions().mode() & 0o022 == 0)
        }) {
            Some(path)
        } else {
            None
        }
    })
}

fn nonblocking(fd: &impl AsFd) -> Result<()> {
    let flags = rustix::fs::fcntl_getfl(fd)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}

struct Running(Option<Child>);
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
            }
            let _ = child.kill();
            // A stuck helper must not hold the UI worker while it is being reaped.
            let _ = std::thread::Builder::new()
                .name("declass-clipboard-reap".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

fn drain(pipe: &mut impl Read, output: &mut Vec<u8>, cap: usize) -> Result<()> {
    let mut buffer = [0; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                ensure!(
                    n <= cap.saturating_sub(output.len()),
                    "Clipboard output exceeds its size limit"
                );
                output.extend_from_slice(&buffer[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn run_command(
    mut command: Command,
    input: Option<&[u8]>,
    cap: usize,
    deadline: Instant,
) -> Result<Vec<u8>> {
    ensure!(Instant::now() < deadline, "Clipboard access timed out");
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut running = Running(Some(command.spawn()?));
    let child = running
        .0
        .as_mut()
        .context("Clipboard helper did not start")?;
    let mut stdin = child.stdin.take();
    let mut stdout = child
        .stdout
        .take()
        .context("Clipboard output is unavailable")?;
    let mut stderr = child
        .stderr
        .take()
        .context("Clipboard error output is unavailable")?;
    if let Some(pipe) = &stdin {
        nonblocking(pipe)?;
    }
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let input = input.unwrap_or_default();
    let mut sent = 0;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    loop {
        ensure!(Instant::now() < deadline, "Clipboard access timed out");
        if let Some(pipe) = &mut stdin {
            while sent < input.len() {
                ensure!(Instant::now() < deadline, "Clipboard access timed out");
                match pipe.write(&input[sent..]) {
                    Ok(0) => bail!("Clipboard helper stopped accepting text"),
                    Ok(n) => sent += n,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if sent == input.len() {
                stdin = None;
            }
        }
        drain(&mut stdout, &mut output, cap)?;
        drain(&mut stderr, &mut errors, MAX_STDERR)?;
        if let Some(status) = child.try_wait()? {
            // try_wait reaped the process; its PID may now be reused.
            // Keep the guard from signalling that PID on later validation errors.
            running.0.take();
            drain(&mut stdout, &mut output, cap)?;
            drain(&mut stderr, &mut errors, MAX_STDERR)?;
            ensure!(
                status.success() && sent == input.len(),
                "Clipboard helper failed"
            );
            // Copy helpers intentionally leave a daemon serving the selection.
            // Do not await inherited pipe EOF or kill that daemon on success.
            return Ok(output);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

const MAC_READ: &str = r#"
ObjC.import('AppKit'); ObjC.import('Foundation');
function run() {
    const board = $.NSPasteboard.generalPasteboard;
    const generation = board.changeCount;
    const kind = ObjC.unwrap(board.availableTypeFromArray(
        $(['public.png', 'public.tiff', 'public.utf8-plain-text'])));
    if (!kind) throw Error('Clipboard has no text or image');
    const text = kind === 'public.utf8-plain-text';
    const data = text ? board.stringForType($(kind)).dataUsingEncoding($.NSUTF8StringEncoding)
                      : board.dataForType($(kind));
    if (data.length > (text ? 1048576 : 16777216)) throw Error('Clipboard exceeds size limit');
    if (board.changeCount !== generation) throw Error('Clipboard changed; paste again');
    const type = text ? 'text/plain' : (kind === 'public.png' ? 'image/png' : 'image/tiff');
    const output = $.NSFileHandle.fileHandleWithStandardOutput;
    output.writeData($(type + '\n').dataUsingEncoding($.NSUTF8StringEncoding));
    output.writeData(data);
}
"#;

const MAC_TIFF: &str = r#"
ObjC.import('AppKit'); ObjC.import('Foundation');
function run() {
    const data = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
    if (data.length > 16777216) throw Error('Clipboard exceeds size limit');
    const bitmap = $.NSBitmapImageRep.alloc.initWithData(data);
    const w = bitmap.pixelsWide, h = bitmap.pixelsHigh;
    if (!(w > 0 && h > 0 && w <= 16384 && h <= 16384 && w * h <= 40000000))
        throw Error('Clipboard image exceeds dimensions limit');
    const png = bitmap.representationUsingTypeProperties($.NSPNGFileType, $({}));
    if (png.length > 16777216) throw Error('Clipboard image exceeds size limit');
    $.NSFileHandle.fileHandleWithStandardOutput.writeData(png);
}
"#;

const MAC_WRITE: &str = r#"
ObjC.import('AppKit'); ObjC.import('Foundation');
function run() {
    const data = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
    if (data.length > 1048576) throw Error('Selection exceeds size limit');
    const text = $.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding);
    const board = $.NSPasteboard.generalPasteboard;
    board.clearContents;
    if (!board.setStringForType(text, $.NSPasteboardTypeString)) throw Error('Clipboard write failed');
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct Call {
        tool: Tool,
        args: Vec<String>,
        input: Option<Vec<u8>>,
        deadline: Instant,
    }
    struct Fake {
        available: Vec<Tool>,
        replies: RefCell<VecDeque<Result<Vec<u8>, &'static str>>>,
        calls: RefCell<Vec<Call>>,
    }
    impl Fake {
        fn new(available: &[Tool], replies: Vec<Result<Vec<u8>, &'static str>>) -> Self {
            Self {
                available: available.to_vec(),
                replies: RefCell::new(replies.into()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }
    impl Runner for Fake {
        fn available(&self, tool: Tool) -> bool {
            self.available.contains(&tool)
        }
        fn run(
            &self,
            tool: Tool,
            args: &[&str],
            input: Option<&[u8]>,
            cap: usize,
            deadline: Instant,
        ) -> Result<Vec<u8>> {
            self.calls.borrow_mut().push(Call {
                tool,
                args: args.iter().map(|s| (*s).to_owned()).collect(),
                input: input.map(<[u8]>::to_vec),
                deadline,
            });
            let bytes = self
                .replies
                .borrow_mut()
                .pop_front()
                .expect("unexpected helper call")
                .map_err(anyhow::Error::msg)?;
            ensure!(bytes.len() <= cap, "mock clipboard output exceeds cap");
            Ok(bytes)
        }
    }
    fn env(platform: Platform) -> Environment {
        Environment {
            platform,
            ssh: false,
            wayland: false,
            local_x11: false,
        }
    }
    fn deadline() -> Instant {
        Instant::now() + TIMEOUT
    }
    fn png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(2, 1, image::Rgba([14, 24, 34, 255]));
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(image.as_raw(), 2, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    }
    fn tiff(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"II\x2a\0\x08\0\0\0\x02\0".to_vec();
        for (tag, value) in [(256_u16, width), (257, height)] {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&4_u16.to_le_bytes());
            out.extend_from_slice(&1_u32.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&0_u32.to_le_bytes());
        out
    }

    #[test]
    fn ssh_and_remote_x11_never_touch_the_native_clipboard() {
        let mut environment = env(Platform::Mac);
        environment.ssh = true;
        let fake = Fake::new(&[Tool::Osascript], vec![]);
        assert!(
            read_with(&environment, &fake, deadline())
                .unwrap_err()
                .to_string()
                .contains("/image PATH")
        );
        assert!(write_with(&environment, &fake, "private selection", deadline()).is_err());
        assert!(fake.calls.borrow().is_empty());
        for display in [":0", ":12.3", "unix:0"] {
            assert!(local_display(display));
        }
        for display in [
            "localhost:10.0",
            "example.com:0",
            "tcp/:0",
            ":x",
            ":0.",
            ":0.1.2",
            "",
        ] {
            assert!(!local_display(display), "{display}");
        }
        assert!(read_with(&env(Platform::Linux), &fake, deadline()).is_err());
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn mac_plain_text_preserves_unicode_newlines_and_does_not_expose_it_in_debug() {
        let expected = "café\n日本語\n";
        let fake = Fake::new(
            &[Tool::Osascript],
            vec![Ok(format!("text/plain\n{expected}").into_bytes())],
        );
        let content = read_with(&env(Platform::Mac), &fake, deadline()).unwrap();
        assert_eq!(content, Content::Text(expected.into()));
        assert!(!format!("{content:?}").contains(expected));
        assert_eq!(fake.calls.borrow()[0].tool, Tool::Osascript);
        assert!(fake.calls.borrow()[0].input.is_none());
    }

    #[test]
    fn wayland_prefers_images_and_reencodes_png_with_one_action_deadline() {
        let mut environment = env(Platform::Linux);
        environment.wayland = true;
        environment.local_x11 = true;
        let fake = Fake::new(
            &[Tool::WlPaste, Tool::Xclip],
            vec![
                Ok(b"text/plain\nimage/png\nimage/jpeg\n".to_vec()),
                Ok(png()),
            ],
        );
        let deadline = deadline();
        let Content::Image { bytes, media_type } =
            read_with(&environment, &fake, deadline).unwrap()
        else {
            panic!("image expected");
        };
        assert_eq!(media_type, "image/png");
        assert_eq!(
            ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png)
                .into_dimensions()
                .unwrap(),
            (2, 1)
        );
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 2);
        assert!(
            calls
                .iter()
                .all(|call| call.tool == Tool::WlPaste && call.deadline == deadline)
        );
        assert_eq!(calls[1].args, ["--no-newline", "--type", "image/png"]);
    }

    #[test]
    fn x11_uses_clipboard_selection_and_xsel_is_text_fallback() {
        let mut environment = env(Platform::Linux);
        environment.local_x11 = true;
        let fake = Fake::new(
            &[Tool::Xclip],
            vec![
                Ok(b"TARGETS\nUTF8_STRING\n".to_vec()),
                Ok(b"hello\n".to_vec()),
            ],
        );
        assert_eq!(
            read_with(&environment, &fake, deadline()).unwrap(),
            Content::Text("hello\n".into())
        );
        let calls = fake.calls.borrow();
        assert_eq!(
            calls[0].args,
            ["-selection", "clipboard", "-out", "-target", "TARGETS"]
        );
        assert_eq!(
            calls[1].args,
            ["-selection", "clipboard", "-out", "-target", "UTF8_STRING"]
        );
        let fake = Fake::new(&[Tool::Xsel], vec![Ok(b"plain".to_vec())]);
        assert_eq!(
            read_with(&environment, &fake, deadline()).unwrap(),
            Content::Text("plain".into())
        );
        assert_eq!(fake.calls.borrow()[0].tool, Tool::Xsel);
    }

    #[test]
    fn writes_pass_text_only_through_stdin_and_report_helper_failure() {
        let text = "'; $(do not execute)\n--option\n";
        for (platform, tool) in [
            (Platform::Mac, Tool::Osascript),
            (Platform::Linux, Tool::WlCopy),
        ] {
            let mut environment = env(platform);
            environment.wayland = true;
            let fake = Fake::new(&[tool], vec![Ok(Vec::new())]);
            write_with(&environment, &fake, text, deadline()).unwrap();
            let calls = fake.calls.borrow();
            assert_eq!(calls.len(), 1, "copy must not probe clipboard contents");
            assert_eq!(calls[0].input.as_deref(), Some(text.as_bytes()));
            assert!(!calls[0].args.iter().any(|arg| arg.contains(text)));
        }
        let fake = Fake::new(&[Tool::Osascript], vec![Err("native write refused")]);
        assert!(write_with(&env(Platform::Mac), &fake, "text", deadline()).is_err());
        let fake = Fake::new(&[Tool::Osascript], vec![]);
        assert!(
            write_with(
                &env(Platform::Mac),
                &fake,
                &"a".repeat(MAX_TEXT + 1),
                deadline()
            )
            .is_err()
        );
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn mac_tiff_is_checked_before_conversion_and_must_keep_dimensions() {
        let raw = tiff(2, 1);
        let mut packet = b"image/tiff\n".to_vec();
        packet.extend_from_slice(&raw);
        let fake = Fake::new(&[Tool::Osascript], vec![Ok(packet.clone()), Ok(png())]);
        assert!(matches!(
            read_with(&env(Platform::Mac), &fake, deadline()).unwrap(),
            Content::Image { .. }
        ));
        assert_eq!(
            fake.calls.borrow()[1].input.as_deref(),
            Some(raw.as_slice())
        );
        let mut oversized = b"image/tiff\n".to_vec();
        oversized.extend_from_slice(&tiff(u32::MAX, 1));
        let fake = Fake::new(&[Tool::Osascript], vec![Ok(oversized)]);
        assert!(read_with(&env(Platform::Mac), &fake, deadline()).is_err());
        assert_eq!(
            fake.calls.borrow().len(),
            1,
            "reject before native image decoding"
        );
        let mut mismatch = b"image/tiff\n".to_vec();
        mismatch.extend_from_slice(&tiff(3, 1));
        let fake = Fake::new(&[Tool::Osascript], vec![Ok(mismatch), Ok(png())]);
        assert!(read_with(&env(Platform::Mac), &fake, deadline()).is_err());
    }

    #[test]
    fn malformed_and_oversized_content_is_rejected_without_partial_success() {
        for bytes in [vec![], vec![0], vec![0xff], vec![b'a'; MAX_TEXT + 1]] {
            assert!(text_content(bytes).is_err());
        }
        for bytes in [vec![], b"not an image".to_vec(), vec![0; MAX_IMAGE + 1]] {
            assert!(normalize_image(&bytes).is_err());
        }
        assert!(normalize_image(&png()[..20]).is_err());
        assert!(check_dimensions((0, 1)).is_err());
        assert!(check_dimensions((10_000, 10_000)).is_err());
        assert!(select_type(b"text/html\nimage/svg+xml\n").is_err());
        assert!(select_type(&vec![b'x'; MAX_TYPES + 1]).is_err());
        let mut out = BoundedBytes {
            bytes: Vec::new(),
            cap: 4,
        };
        out.write_all(b"1234").unwrap();
        assert!(out.write_all(b"5").is_err());
        assert_eq!(out.bytes, b"1234");
        let mut multiple = tiff(2, 1);
        let len = multiple.len();
        multiple[len - 4..].copy_from_slice(&8_u32.to_le_bytes());
        assert!(tiff_dimensions(&multiple).is_err());
        for end in 0..tiff(2, 1).len() {
            assert!(
                tiff_dimensions(&tiff(2, 1)[..end]).is_err(),
                "accepted truncated TIFF at {end}"
            );
        }
    }

    #[test]
    fn bounded_runner_handles_binary_output_failure_and_timeout_without_clipboard() {
        // These fixed mock helpers never call clipboard programs or inspect user data.
        let mut success = Command::new("/bin/cat");
        success.env_clear();
        assert_eq!(
            run_command(success, Some(b"a\0b\n"), 16, deadline()).unwrap(),
            b"a\0b\n"
        );
        let mut too_large = Command::new("/bin/cat");
        too_large.env_clear();
        assert!(run_command(too_large, Some(b"12345"), 4, deadline()).is_err());
        let mut failure = Command::new("/bin/sh");
        failure.args(["-c", "exit 1"]).env_clear();
        assert!(run_command(failure, None, 16, deadline()).is_err());
        let mut sleeping = Command::new("/bin/sleep");
        sleeping.arg("5").env_clear();
        let start = Instant::now();
        assert!(run_command(sleeping, None, 16, start + Duration::from_millis(50)).is_err());
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "worker waited for sleeping helper"
        );
        let mut expired = Command::new("/no/such/helper");
        expired.env_clear();
        assert!(
            run_command(expired, None, 16, Instant::now())
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }
}
