// SPDX-License-Identifier: GPL-3.0-or-later
//! Incremental Server-Sent Events decoder.

/// One SSE event: the `event:` name (if any) and its joined `data:` lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    /// Feeds bytes and returns every event completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line).into_owned();
            self.line(&line, &mut out);
        }
        out
    }

    /// Flushes a final event not followed by a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.buffer.is_empty() {
            let rest = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
            self.line(rest.trim_end_matches('\r'), &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
        } else if let Some(rest) = line.strip_prefix("data:") {
            self.data
                .push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
        } else if let Some(rest) = line.strip_prefix("event:") {
            self.event = Some(rest.trim().to_owned());
        }
        // `id:`, `retry:` and `:` comments are ignored.
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if !self.data.is_empty() {
            out.push(SseEvent {
                event: self.event.take(),
                data: self.data.join("\n"),
            });
            self.data.clear();
        }
        self.event = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_events_split_across_chunks() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"data: {\"a\"").is_empty());
        let e = d.push(b":1}\n\nevent: message_stop\r\ndata: x\r\n\r\n: comment\n");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].data, "{\"a\":1}");
        assert_eq!(e[1].event.as_deref(), Some("message_stop"));
        assert!(d.finish().is_empty());
    }

    #[test]
    fn multi_line_data_and_unterminated_tail() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"data: a\ndata: b\n").is_empty());
        let e = d.finish();
        assert_eq!(e[0].data, "a\nb");
    }
}
