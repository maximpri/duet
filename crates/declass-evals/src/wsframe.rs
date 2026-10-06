// SPDX-License-Identifier: GPL-3.0-or-later
//! WebSocket frame reader (RFC 6455) for the leak proxy.
//!
//! The proxy relays an upgraded connection byte for byte and feeds each
//! direction through a [`Reader`] to recover whole messages: it decodes 7-,
//! 16- and 64-bit payload lengths, unmasks masked (client) frames, joins
//! continuation frames into one message and hands control frames back as they
//! come (they may sit between the fragments of a message). No extension is
//! implemented: a frame with a reserved bit set is a [`ProtocolError`], because
//! its payload (a compressed one, say) cannot be read as sent.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    Text,
    Binary,
}

impl DataKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Binary => "binary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    Close,
    Ping,
    Pong,
}

/// A complete (or, from [`Reader::unfinished`], interrupted) data message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: DataKind,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// A data frame; `Some` when it completed a message.
    Data(Option<Message>),
    Control {
        kind: ControlKind,
        payload: Vec<u8>,
    },
}

/// One frame: its bytes exactly as received, and what it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub raw: Vec<u8>,
    pub content: Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub String);

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn violation<T>(what: impl Into<String>) -> Result<T, ProtocolError> {
    Err(ProtocolError(what.into()))
}

/// Incremental reader for one direction of a connection.
pub struct Reader {
    buf: Vec<u8>,
    message: Option<Message>,
    max_message: usize,
}

impl Reader {
    /// A reader that refuses messages larger than `max_message` bytes.
    pub fn new(max_message: usize) -> Self {
        Self {
            buf: Vec::new(),
            message: None,
            max_message,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, `None` until more bytes arrive. After an
    /// error the reader's state is unspecified; stop feeding it.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        let b = &self.buf;
        if b.len() < 2 {
            return Ok(None);
        }
        let fin = b[0] & 0x80 != 0;
        if b[0] & 0x70 != 0 {
            return violation(format!(
                "reserved bits {:#05b} set: the frame uses an extension",
                (b[0] & 0x70) >> 4
            ));
        }
        let opcode = b[0] & 0x0f;
        let control = match opcode {
            0..=2 => None,
            8 => Some(ControlKind::Close),
            9 => Some(ControlKind::Ping),
            10 => Some(ControlKind::Pong),
            other => return violation(format!("reserved opcode {other:#x}")),
        };
        let masked = b[1] & 0x80 != 0;
        let (len, mut at) = match b[1] & 0x7f {
            126 => {
                if b.len() < 4 {
                    return Ok(None);
                }
                (u64::from(u16::from_be_bytes([b[2], b[3]])), 4)
            }
            127 => {
                if b.len() < 10 {
                    return Ok(None);
                }
                let mut n = [0u8; 8];
                n.copy_from_slice(&b[2..10]);
                let len = u64::from_be_bytes(n);
                if len >> 63 != 0 {
                    return violation("64-bit payload length with the high bit set");
                }
                (len, 10)
            }
            n => (u64::from(n), 2),
        };
        if control.is_some() && (!fin || len > 125) {
            return violation("control frame fragmented or longer than 125 bytes");
        }
        let so_far = self.message.as_ref().map_or(0, |m| m.payload.len());
        if control.is_none() && so_far as u64 + len > self.max_message as u64 {
            return violation(format!("message larger than {} bytes", self.max_message));
        }
        let mask = if masked {
            if b.len() < at + 4 {
                return Ok(None);
            }
            let key = [b[at], b[at + 1], b[at + 2], b[at + 3]];
            at += 4;
            Some(key)
        } else {
            None
        };
        // `len` fits: it is at most `max_message` or 125 here.
        let total = at + len as usize;
        if b.len() < total {
            return Ok(None);
        }
        let raw: Vec<u8> = self.buf.drain(..total).collect();
        let mut payload = raw[at..].to_vec();
        if let Some(key) = mask {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= key[i % 4];
            }
        }
        let content = match (control, opcode) {
            (Some(kind), _) => Content::Control { kind, payload },
            (None, 0) => {
                let Some(message) = self.message.as_mut() else {
                    return violation("continuation frame outside a message");
                };
                message.payload.extend_from_slice(&payload);
                Content::Data(if fin { self.message.take() } else { None })
            }
            (None, op) => {
                if self.message.is_some() {
                    return violation("new message before the previous one finished");
                }
                let kind = if op == 1 {
                    DataKind::Text
                } else {
                    DataKind::Binary
                };
                let message = Message { kind, payload };
                if fin {
                    Content::Data(Some(message))
                } else {
                    self.message = Some(message);
                    Content::Data(None)
                }
            }
        };
        Ok(Some(Frame { raw, content }))
    }

    /// Whether a fragmented message has started and not finished.
    pub fn in_message(&self) -> bool {
        self.message.is_some()
    }

    /// Bytes received that do not yet form a complete frame.
    pub fn take_buffered(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }

    /// The fragments received so far of a message that has not finished.
    pub fn unfinished(&mut self) -> Option<Message> {
        self.message.take()
    }
}

/// The status code of a Close frame's payload, if it carries one.
pub fn close_code(payload: &[u8]) -> Option<u16> {
    (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]))
}

/// Encodes one frame, masked with `mask` when given (client frames).
#[cfg(test)]
pub fn encode(fin: bool, opcode: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = vec![(u8::from(fin) << 7) | opcode];
    let m = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(m | n as u8),
        n if n <= usize::from(u16::MAX) => {
            out.push(m | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(m | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(key) => {
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: Option<[u8; 4]> = Some([0x37, 0xfa, 0x21, 0x3d]);

    fn frames(reader: &mut Reader) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Some(f) = reader.next_frame().unwrap() {
            out.push(f);
        }
        out
    }

    #[test]
    fn rfc_examples_masked_and_unmasked() {
        // RFC 6455 §5.7: a single-frame unmasked and masked text message "Hello".
        let mut r = Reader::new(1 << 20);
        r.push(&[0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]);
        r.push(&[
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ]);
        let got = frames(&mut r);
        assert_eq!(got.len(), 2);
        for f in got {
            assert_eq!(
                f.content,
                Content::Data(Some(Message {
                    kind: DataKind::Text,
                    payload: b"Hello".to_vec()
                }))
            );
        }
    }

    #[test]
    fn lengths_fragments_and_interleaved_control_frames() {
        let big = vec![b'a'; 300];
        let huge = vec![b'b'; 70_000];
        let mut wire = Vec::new();
        wire.extend(encode(false, 1, b"Hel", KEY));
        wire.extend(encode(true, 9, b"ping", KEY));
        wire.extend(encode(false, 0, b"lo ", KEY));
        wire.extend(encode(true, 0, &big, KEY));
        wire.extend(encode(true, 2, &huge, None));
        wire.extend(encode(true, 8, &1000u16.to_be_bytes(), KEY));
        // Fed one byte at a time, the reader yields the same frames.
        let mut r = Reader::new(1 << 20);
        let mut got = Vec::new();
        for byte in &wire {
            r.push(std::slice::from_ref(byte));
            got.extend(frames(&mut r));
        }
        assert_eq!(got.len(), 6);
        assert_eq!(
            got.iter().flat_map(|f| f.raw.clone()).collect::<Vec<_>>(),
            wire,
            "raw bytes are exactly what was received"
        );
        assert_eq!(
            got[1].content,
            Content::Control {
                kind: ControlKind::Ping,
                payload: b"ping".to_vec()
            }
        );
        let mut text = b"Hello ".to_vec();
        text.extend(&big);
        assert_eq!(got[0].content, Content::Data(None));
        assert_eq!(
            got[3].content,
            Content::Data(Some(Message {
                kind: DataKind::Text,
                payload: text
            }))
        );
        assert_eq!(
            got[4].content,
            Content::Data(Some(Message {
                kind: DataKind::Binary,
                payload: huge
            }))
        );
        let Content::Control { kind, payload } = &got[5].content else {
            panic!("close");
        };
        assert_eq!(*kind, ControlKind::Close);
        assert_eq!(close_code(payload), Some(1000));
        assert!(r.take_buffered().is_empty());
    }

    #[test]
    fn sixty_four_bit_length() {
        let payload = vec![7u8; 65_536];
        let wire = encode(true, 2, &payload, KEY);
        assert_eq!(wire[1] & 0x7f, 127);
        let mut r = Reader::new(1 << 20);
        r.push(&wire);
        assert_eq!(
            frames(&mut r)[0].content,
            Content::Data(Some(Message {
                kind: DataKind::Binary,
                payload
            }))
        );
    }

    #[test]
    fn protocol_violations() {
        let bad = |wire: Vec<u8>| {
            let mut r = Reader::new(1024);
            r.push(&wire);
            r.next_frame().unwrap_err().0
        };
        // RSV1: a compressed (permessage-deflate) frame.
        let mut deflated = encode(true, 1, b"x", KEY);
        deflated[0] |= 0x40;
        assert!(bad(deflated).contains("extension"));
        assert!(bad(encode(true, 3, b"", None)).contains("opcode"));
        assert!(bad(encode(true, 0, b"x", None)).contains("continuation"));
        assert!(bad(encode(false, 9, b"", None)).contains("control"));
        assert!(bad(encode(true, 2, &[0; 2000], None)).contains("larger"));
        let mut two = encode(false, 1, b"a", None);
        two.extend(encode(true, 1, b"b", None));
        let mut r = Reader::new(1024);
        r.push(&two);
        assert!(r.next_frame().unwrap().is_some());
        assert!(r.next_frame().is_err());
        let mut huge = vec![0x82, 127];
        huge.extend(u64::MAX.to_be_bytes());
        assert!(bad(huge).contains("high bit"));
    }

    #[test]
    fn unfinished_message_and_partial_frame() {
        let mut r = Reader::new(1024);
        r.push(&encode(false, 1, b"part", KEY));
        let tail = encode(true, 0, b"rest", KEY);
        r.push(&tail[..3]);
        assert_eq!(frames(&mut r).len(), 1);
        assert_eq!(r.take_buffered(), tail[..3].to_vec());
        assert_eq!(r.unfinished().unwrap().payload, b"part".to_vec());
    }
}
