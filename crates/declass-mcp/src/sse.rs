// SPDX-License-Identifier: GPL-3.0-or-later
//! The `data` of Server-Sent Events, fed in arbitrary chunks.

#[derive(Debug, Default)]
pub struct Events {
    buffer: Vec<u8>,
    data: Vec<String>,
}

impl Events {
    /// Feeds bytes; returns the data of every event they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
            }
            // `event:`, `id:`, `retry:` and `:` comment lines carry nothing we use.
        }
        out
    }

    /// Bytes buffered but not yet part of a complete event.
    pub fn pending(&self) -> usize {
        self.buffer.len() + self.data.iter().map(String::len).sum::<usize>()
    }

    /// The data of a last event not followed by a blank line.
    pub fn finish(&mut self) -> Option<String> {
        let mut out = self.push(b"\n\n");
        out.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_across_chunks_are_joined() {
        let mut e = Events::default();
        assert!(e.push(b"event: message\ndata: {\"a\"").is_empty());
        assert_eq!(e.push(b":1}\r\n\r\n: keepalive\n\n"), vec!["{\"a\":1}"]);
        assert_eq!(e.push(b"data: x\ndata: y\n"), Vec::<String>::new());
        assert_eq!(e.finish().as_deref(), Some("x\ny"));
    }
}
