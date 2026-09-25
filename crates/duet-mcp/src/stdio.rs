// SPDX-License-Identifier: GPL-3.0-or-later
//! The stdio transport: one JSON-RPC message per line on the server's
//! standard input and output (the server's standard error is its log and is
//! the caller's business).
//!
//! Requests are sent one at a time. While waiting for an answer, messages from
//! the server are handled in order: requests it makes are answered at once,
//! notifications and answers to earlier (cancelled) requests are dropped.

use crate::{MAX_MESSAGE_BYTES, McpError, answers, reply_to};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub struct Stdio {
    input: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    output: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
    /// The part of a line read so far; kept here so a read cut short by a
    /// timeout resumes where it stopped.
    partial: Vec<u8>,
    /// Bytes of an oversized line still being skipped.
    skipping: bool,
}

fn closed(e: std::io::Error) -> McpError {
    McpError::Closed(e.to_string())
}

impl Stdio {
    pub fn new(
        input: Box<dyn AsyncWrite + Send + Unpin>,
        output: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Self {
        Self {
            input: Some(input),
            output: BufReader::new(output),
            partial: Vec::new(),
            skipping: false,
        }
    }

    pub async fn send(&mut self, message: &Value) -> Result<(), McpError> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| McpError::Closed("its input was closed".into()))?;
        // `serde_json` escapes newlines inside strings, so the line is one message.
        let mut line =
            serde_json::to_vec(message).map_err(|e| McpError::Protocol(e.to_string()))?;
        line.push(b'\n');
        input.write_all(&line).await.map_err(closed)?;
        input.flush().await.map_err(closed)
    }

    /// The next line, without its newline. A line longer than
    /// [`MAX_MESSAGE_BYTES`] is skipped and reported.
    async fn line(&mut self) -> Result<Vec<u8>, McpError> {
        loop {
            let buf = self.output.fill_buf().await.map_err(closed)?;
            if buf.is_empty() {
                return Err(McpError::Closed("its output ended".into()));
            }
            let (chunk, done) = match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (&buf[..i], Some(i + 1)),
                None => (buf, None),
            };
            if !self.skipping {
                self.partial.extend_from_slice(chunk);
            }
            let used = done.unwrap_or(buf.len());
            self.output.consume(used);
            if self.partial.len() > MAX_MESSAGE_BYTES {
                self.partial.clear();
                self.skipping = true;
            }
            if done.is_some() {
                if std::mem::take(&mut self.skipping) {
                    return Err(McpError::Protocol(format!(
                        "a message was larger than {MAX_MESSAGE_BYTES} bytes"
                    )));
                }
                return Ok(std::mem::take(&mut self.partial));
            }
        }
    }

    /// Sends request `id` and returns the server's answer to it.
    pub async fn request(&mut self, id: u64, message: &Value) -> Result<Value, McpError> {
        self.send(message).await?;
        loop {
            let line = self.line().await?;
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            // A line that is not JSON (a server printing to its output by
            // mistake) is skipped; the answer may still follow.
            let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let messages = match parsed {
                Value::Array(batch) => batch,
                single => vec![single],
            };
            let mut found = None;
            for m in messages {
                if answers(&m, id) {
                    found = Some(m);
                } else if let Some(reply) = reply_to(&m) {
                    self.send(&reply).await?;
                }
            }
            if let Some(m) = found {
                return Ok(m);
            }
        }
    }

    /// Closes the server's input, which asks it to exit.
    pub async fn close(&mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = input.shutdown().await;
        }
    }
}
