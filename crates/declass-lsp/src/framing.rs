// SPDX-License-Identifier: GPL-3.0-or-later
//! The base protocol: each message is a header part (`Content-Length`, an
//! optional `Content-Type`, each line ending in `\r\n`), an empty line, then
//! exactly that many bytes of UTF-8 JSON.

use serde_json::Value;
use std::io::{Error, ErrorKind};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest message accepted; a server announcing more is treated as broken.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Longest header line accepted.
const MAX_HEADER_LINE: u64 = 8 * 1024;
/// Header lines accepted before the empty line.
const MAX_HEADERS: usize = 16;

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, message.into())
}

/// Reads one message. `Ok(None)` at a clean end of stream (between messages).
pub async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<Option<Value>> {
    let mut length = None;
    let mut headers = 0;
    loop {
        let mut line = Vec::new();
        let n = (&mut *r)
            .take(MAX_HEADER_LINE)
            .read_until(b'\n', &mut line)
            .await?;
        if n == 0 {
            return if headers == 0 {
                Ok(None)
            } else {
                Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "stream ended in a header",
                ))
            };
        }
        if !line.ends_with(b"\n") {
            return Err(invalid("header line too long"));
        }
        let text = std::str::from_utf8(&line).map_err(|_| invalid("header is not UTF-8"))?;
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            if headers == 0 {
                // Tolerate stray blank lines between messages.
                continue;
            }
            break;
        }
        headers += 1;
        if headers > MAX_HEADERS {
            return Err(invalid("too many header lines"));
        }
        let (name, value) = text
            .split_once(':')
            .ok_or_else(|| invalid(format!("malformed header {text:?}")))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            let n: usize = value
                .trim()
                .parse()
                .map_err(|_| invalid(format!("bad Content-Length {value:?}")))?;
            length = Some(n);
        }
    }
    let length = length.ok_or_else(|| invalid("message without Content-Length"))?;
    if length > MAX_MESSAGE_BYTES {
        return Err(invalid(format!("message of {length} bytes is too large")));
    }
    let mut body = vec![0; length];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| invalid(format!("message is not JSON: {e}")))
}

/// The framed bytes of `message`.
pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Writes one message and flushes.
pub async fn write_message<W: AsyncWrite + Unpin>(
    w: &mut W,
    message: &Value,
) -> std::io::Result<()> {
    w.write_all(&encode(message)).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn read_all(bytes: &[u8]) -> Vec<std::io::Result<Option<Value>>> {
        let mut r = tokio::io::BufReader::new(bytes);
        let mut out = Vec::new();
        loop {
            let m = read_message(&mut r).await;
            let stop = !matches!(m, Ok(Some(_)));
            out.push(m);
            if stop {
                return out;
            }
        }
    }

    #[tokio::test]
    async fn messages_round_trip_with_multibyte_bodies() {
        let a = json!({"jsonrpc": "2.0", "id": 1, "result": "héllo 😀"});
        let b = json!({"jsonrpc": "2.0", "method": "x"});
        let mut bytes = encode(&a);
        // Other headers are ignored; header names are case-insensitive.
        let body = b.to_string();
        bytes.extend_from_slice(
            format!(
                "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
        let got = read_all(&bytes).await;
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].as_ref().unwrap().as_ref().unwrap(), &a);
        assert_eq!(got[1].as_ref().unwrap().as_ref().unwrap(), &b);
        assert!(matches!(got[2], Ok(None)));
    }

    #[tokio::test]
    async fn malformed_frames_are_errors_not_hangs() {
        for bad in [
            &b"Content-Length: 10\r\n\r\n{}"[..],
            b"Content-Type: x\r\n\r\n{}",
            b"Content-Length: nope\r\n\r\n{}",
            b"garbage\r\n\r\n",
            b"Content-Length: 99999999999\r\n\r\n",
            b"Content-Length: 2\r\n\r\n{]",
            b"Content-Length: 2\r\n",
        ] {
            let got = read_all(bad).await;
            assert!(
                got.last().unwrap().is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
