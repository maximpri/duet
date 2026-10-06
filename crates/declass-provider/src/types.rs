// SPDX-License-Identifier: GPL-3.0-or-later
//! Dialect-neutral request and response types.

pub use crate::image::Image;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A tool the model may call. Tools are always sent sorted by name so the
/// request prefix stays byte-identical across turns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Parsed arguments; always a JSON object.
    pub arguments: Map<String, Value>,
    /// Arguments exactly as the model produced them, replayed verbatim so the
    /// conversation prefix never changes.
    pub raw_arguments: String,
}

/// One conversation item. The conversation is append-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    User {
        text: String,
    },
    Assistant {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
        /// Provider state the next request must carry back unchanged (signed or
        /// encrypted reasoning), for the dialect that produced it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay: Option<Replay>,
    },
    ToolResult {
        call_id: String,
        content: String,
    },
    /// Images that belong to the item right before it: the operator's
    /// message (`call_id` `None`) or the result of the tool call `call_id`.
    /// Kept apart from that item so text items keep one shape; each dialect
    /// puts the images where its API takes them. Transcripts hold only their
    /// digests (see [`Image`]); an empty list (masked) sends nothing.
    Images {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        images: Vec<Image>,
    },
}

/// The loaded images attached to `items[i]` (by the `Images` item after it).
pub fn images_after(items: &[Item], i: usize) -> Vec<&Image> {
    match (items.get(i), items.get(i + 1)) {
        (Some(owner), Some(Item::Images { call_id, images })) if belongs(owner, call_id) => {
            images.iter().filter(|img| img.is_loaded()).collect()
        }
        _ => Vec::new(),
    }
}

/// Whether the `Images` item `items[i]` belongs to the item before it (and
/// is sent with that item); a stray one is sent on its own.
pub fn attached_to_previous(items: &[Item], i: usize) -> bool {
    match (i.checked_sub(1).and_then(|p| items.get(p)), items.get(i)) {
        (Some(owner), Some(Item::Images { call_id, .. })) => belongs(owner, call_id),
        _ => false,
    }
}

fn belongs(owner: &Item, call_id: &Option<String>) -> bool {
    match (owner, call_id) {
        (Item::User { .. }, None) => true,
        (Item::ToolResult { call_id: id, .. }, Some(c)) => id == c,
        _ => false,
    }
}

/// One part of a response's output, in the order the provider produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    /// A provider block kept verbatim (a signed thinking block, an encrypted
    /// reasoning item). It holds no text Declass shows or edits.
    Opaque { block: Value },
    /// A text block of `bytes` bytes of the item's text.
    Text { bytes: usize },
    /// The tool call with this id.
    Call { id: String },
}

/// What a dialect needs to replay an assistant turn exactly: its output
/// layout with the opaque reasoning blocks. The text and tool calls themselves
/// stay in the item, so outbound filters see and edit them as usual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Replay {
    /// The dialect that produced it; any other dialect ignores it.
    pub dialect: String,
    pub parts: Vec<Part>,
}

/// A piece of an assistant turn to put on the wire, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Piece<'a> {
    Opaque(&'a Value),
    Text(&'a str),
    Call(&'a ToolCall),
}

/// The assistant turn as the dialect `dialect` should send it: in the
/// original order with its opaque blocks when the turn came from that dialect,
/// otherwise text then tool calls. Text split across several blocks is split
/// the same way again while its length is unchanged; edited text (an outbound
/// filter replaced a value) goes out as one block where the first one was.
pub fn assistant_pieces<'a>(
    dialect: &str,
    text: &'a str,
    tool_calls: &'a [ToolCall],
    replay: Option<&'a Replay>,
) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    let Some(replay) = replay.filter(|r| r.dialect == dialect) else {
        if !text.is_empty() {
            out.push(Piece::Text(text));
        }
        out.extend(tool_calls.iter().map(Piece::Call));
        return out;
    };
    let lengths: Vec<usize> = replay
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { bytes } => Some(*bytes),
            _ => None,
        })
        .collect();
    let split = lengths.iter().sum::<usize>() == text.len() && {
        let mut at = 0;
        lengths.iter().all(|n| {
            at += n;
            text.is_char_boundary(at)
        })
    };
    let (mut at, mut text_sent) = (0usize, false);
    let mut used = vec![false; tool_calls.len()];
    for part in &replay.parts {
        match part {
            Part::Opaque { block } => out.push(Piece::Opaque(block)),
            Part::Text { bytes } if split => {
                if *bytes > 0 {
                    out.push(Piece::Text(&text[at..at + bytes]));
                }
                at += bytes;
                text_sent = true;
            }
            Part::Text { .. } => {
                if !text_sent && !text.is_empty() {
                    out.push(Piece::Text(text));
                }
                text_sent = true;
            }
            Part::Call { id } => {
                if let Some(i) = tool_calls
                    .iter()
                    .enumerate()
                    .position(|(i, c)| !used[i] && &c.id == id)
                {
                    used[i] = true;
                    out.push(Piece::Call(&tool_calls[i]));
                }
            }
        }
    }
    if !text_sent && !text.is_empty() {
        out.push(Piece::Text(text));
    }
    for (i, c) in tool_calls.iter().enumerate() {
        if !used[i] {
            out.push(Piece::Call(c));
        }
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub system: String,
    pub items: Vec<Item>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// Constrain the reply to a JSON Schema (structured output).
    pub response_schema: Option<Value>,
    /// Provider-specific top-level fields merged into the request body.
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    ToolCalls,
    /// Output hit the length limit. Tool calls in such a response must not run.
    Length,
    ContentFilter,
    Other(String),
}

impl StopReason {
    pub fn from_wire(s: &str) -> Self {
        match s {
            "stop" | "end_turn" | "stop_sequence" | "completed" => Self::Stop,
            "tool_calls" | "function_call" | "tool_use" => Self::ToolCalls,
            "length" | "max_tokens" | "max_output_tokens" | "incomplete" => Self::Length,
            "content_filter" | "refusal" | "sensitive" => Self::ContentFilter,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Whether tool calls in the response may be executed.
    pub fn allows_tools(&self) -> bool {
        matches!(self, Self::Stop | Self::ToolCalls)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageStatus {
    #[default]
    Reported,
    /// The provider sent no usage; counts are estimated from bytes.
    Estimated,
}

/// Token usage for one response. `input` excludes cache reads and writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// Reasoning tokens, already included in `output`.
    pub reasoning: u64,
    pub status: UsageStatus,
}

impl Usage {
    pub fn total_input(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }
}

/// Usage across every attempt of one logical request. Failed attempts never
/// poison the total: attempts that failed before producing output bill nothing,
/// and attempts that failed mid-stream are estimated and kept separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptUsage {
    pub billed: Usage,
    pub estimated_failed: Usage,
    pub attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: Option<String>,
    pub model: String,
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop: StopReason,
    pub usage: Usage,
    pub attempts: AttemptUsage,
    /// Opaque reasoning to carry into the next request (see [`Replay`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<Replay>,
}

impl Response {
    /// The assistant item to append to the conversation.
    pub fn to_item(&self) -> Item {
        Item::Assistant {
            text: self.text.clone(),
            reasoning: self.reasoning.clone(),
            tool_calls: self.tool_calls.clone(),
            replay: self.replay.clone(),
        }
    }
}

/// Estimates tokens from bytes (roughly 3.5 bytes per token for code and prose).
pub fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(7) * 2
}
