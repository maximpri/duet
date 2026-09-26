// SPDX-License-Identifier: GPL-3.0-or-later
//! A live view of a response while it streams, for the operator's terminal.
//!
//! Observing never changes a request or a response: the stream is assembled
//! exactly as without a tap, and the tap sees what the assembler holds so
//! far, piece by piece. Each dialect's assembler lists its parts
//! ([`Part`], keyed by their position in the response); [`Differ`] turns two
//! successive views into [`StreamEvent`]s.

use std::collections::HashMap;

/// Receives a streaming response as it arrives. Called on the task that
/// sends the request, between chunks: it must return quickly.
pub trait StreamTap: Send + Sync {
    fn event(&self, event: StreamEvent<'_>);
}

/// One step of a streaming response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent<'a> {
    /// An attempt of the request starts (the first is 1); whatever an
    /// earlier attempt streamed is void.
    Attempt(u32),
    /// Text of the response (in order).
    Text(&'a str),
    /// Reasoning arrived (its size in bytes; its text is not shown).
    Reasoning(usize),
    /// A tool call started; `index` tells the calls of one response apart.
    Call { index: usize, name: &'a str },
    /// A piece of a tool call's JSON arguments.
    Arguments { index: usize, delta: &'a str },
    /// The attempt's stream ended (complete or not).
    End,
}

/// What an assembler holds so far, one part per content block, text or call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part<'a> {
    Text(&'a str),
    Reasoning(usize),
    Call { name: &'a str, arguments: &'a str },
}

/// Turns successive views of one attempt into events: what each part gained
/// since the last view. Parts only grow while a response streams.
#[derive(Default)]
pub struct Differ {
    seen: HashMap<usize, Seen>,
}

#[derive(Default)]
struct Seen {
    len: usize,
    announced: bool,
}

impl Differ {
    pub fn diff(&mut self, parts: &[(usize, Part<'_>)], tap: &dyn StreamTap) {
        for &(key, part) in parts {
            let seen = self.seen.entry(key).or_default();
            match part {
                Part::Text(t) => {
                    if let Some(new) = grown(t, seen.len) {
                        tap.event(StreamEvent::Text(new));
                        seen.len = t.len();
                    }
                }
                Part::Reasoning(n) => {
                    if n > seen.len {
                        tap.event(StreamEvent::Reasoning(n - seen.len));
                        seen.len = n;
                    }
                }
                Part::Call { name, arguments } => {
                    // A call is announced with its first arguments, when its
                    // name (which some servers split) is complete.
                    let new = grown(arguments, seen.len);
                    if !seen.announced && !name.is_empty() && new.is_some() {
                        tap.event(StreamEvent::Call { index: key, name });
                        seen.announced = true;
                    }
                    if let Some(delta) = new.filter(|_| seen.announced) {
                        tap.event(StreamEvent::Arguments { index: key, delta });
                        seen.len = arguments.len();
                    }
                }
            }
        }
    }

    /// Announces calls that never streamed arguments (a call without any).
    pub fn finish(&mut self, parts: &[(usize, Part<'_>)], tap: &dyn StreamTap) {
        self.diff(parts, tap);
        for &(key, part) in parts {
            if let Part::Call { name, .. } = part {
                let seen = self.seen.entry(key).or_default();
                if !seen.announced && !name.is_empty() {
                    tap.event(StreamEvent::Call { index: key, name });
                    seen.announced = true;
                }
            }
        }
    }
}

/// What `text` gained past byte `seen`, if anything (on a char boundary:
/// parts grow by whole strings).
fn grown(text: &str, seen: usize) -> Option<&str> {
    (text.len() > seen)
        .then(|| text.get(seen..))
        .flatten()
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);

    impl StreamTap for Log {
        fn event(&self, e: StreamEvent<'_>) {
            self.0.lock().unwrap().push(format!("{e:?}"));
        }
    }

    #[test]
    fn views_become_the_pieces_each_part_gained() {
        let log = Log::default();
        let mut d = Differ::default();
        d.diff(&[(0, Part::Text("Hel"))], &log);
        d.diff(&[(0, Part::Text("Hello"))], &log);
        d.diff(
            &[
                (0, Part::Text("Hello")),
                (
                    1,
                    Part::Call {
                        name: "reply",
                        arguments: "",
                    },
                ),
            ],
            &log,
        );
        d.diff(
            &[
                (0, Part::Text("Hello")),
                (
                    1,
                    Part::Call {
                        name: "reply",
                        arguments: "{\"me",
                    },
                ),
                (2, Part::Reasoning(7)),
            ],
            &log,
        );
        d.finish(
            &[
                (
                    1,
                    Part::Call {
                        name: "reply",
                        arguments: "{\"message\":1}",
                    },
                ),
                (
                    3,
                    Part::Call {
                        name: "list_files",
                        arguments: "",
                    },
                ),
            ],
            &log,
        );
        assert_eq!(
            *log.0.lock().unwrap(),
            [
                "Text(\"Hel\")",
                "Text(\"lo\")",
                "Call { index: 1, name: \"reply\" }",
                "Arguments { index: 1, delta: \"{\\\"me\" }",
                "Reasoning(7)",
                "Arguments { index: 1, delta: \"ssage\\\":1}\" }",
                "Call { index: 3, name: \"list_files\" }",
            ]
        );
    }
}
