// SPDX-License-Identifier: GPL-3.0-or-later
//! Context compaction: once the conversation sent to the frontier passes
//! `context.compact_at`, the local model condenses its older part into a
//! working summary (the task, what was done and why, what failed, the state,
//! what is open, and the exact identifiers) and the recent turns stay as they
//! are. Off by default (`context.compaction`) until measured. It needs a
//! local model ([`Presenter::can_condense`]), so a pass-through run, or a
//! hybrid one with `local.enabled` off, never compacts and masks as it
//! always did.
//!
//! **Order.** At each safe point, before a request:
//! 1. Masking first: if masking old tool results (whole turns, oldest first,
//!    as [`crate::context`] does) brings the conversation down to the target
//!    (`context.compact_to` of `compact_at`), that is the whole event. No
//!    local call; the stubs name every call exactly.
//! 2. Compaction when masking is not enough: everything between the first
//!    message and the recent turns is condensed into one message that
//!    replaces it. The recent turns are as many as fit in the target with a
//!    summary, never fewer than [`KEEP_RECENT_TURNS`]; the cut is where a
//!    turn starts, so a tool call is never separated from its result.
//! 3. Then the window's own masking ([`crate::context::mask_if_needed`]), as
//!    in every run: it stays the safety net.
//!
//! **Rare, for the prompt cache.** Every event changes the conversation, so
//! the provider's prefix cache breaks once; the system prompt, the tools and
//! the first message stay, so their cache holds. The target is well below the
//! threshold and an event needs an older part of at least [`MIN_GAIN`] of
//! `compact_at`, so the next one comes only after the conversation has grown
//! again. After an event, requests only append: the new prefix is stable.
//!
//! **Failure.** No summary (no local model, an error, an empty or too long
//! one, or one no shorter than what it would replace) leaves the conversation
//! as it was: masking goes on as before, the failure is recorded, and no new
//! attempt is made until the conversation has grown by [`MIN_GAIN`] of
//! `compact_at`. Compaction never ends a run.
//!
//! **Security.** The local model reads the older part in the form the
//! frontier was sent it (after the outbound filters,
//! [`crate::driver::Driver::as_sent`]), never the items as kept or a
//! handle's content. Its summary is cleaned as local-model output
//! ([`Presenter::condense`]), and the message that holds it goes through the
//! outbound gate like every other.
//!
//! **Resume.** An event is recorded before it changes the conversation
//! (`compacted` with the replacement text and where it went; `masked` with
//! its positions), and one that could not be recorded is not applied, so a
//! resumed run rebuilds the same conversation byte for byte.
//!
//! **Sessions and sub-agents.** Every conversation compacts on its own. The
//! first message stays, as a run's task does; in a session, the operator's
//! latest message, if it is condensed, is appended to the summary verbatim.

use crate::context::{KEEP_RECENT_TURNS, apply_mask, estimate, mask_plan_from_estimate};
use crate::transcript::Entry;
use duet_boundary::audit::AuditEvent;
use duet_boundary::local::{CHUNK_CHARS, MAX_CONDENSED};
use duet_boundary::model::Item;
use duet_boundary::view::Presenter;
use serde_json::Value;
use std::collections::HashMap;

/// The settings (`context.compact_at`, `context.compact_to`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Compaction {
    /// Estimated request tokens at which a conversation is compacted.
    pub at: u64,
    /// Fraction of `at` a compaction brings it down to.
    pub to: f64,
}

impl Compaction {
    /// What an event brings the conversation down to.
    pub fn target(&self) -> u64 {
        (self.at as f64 * self.to) as u64
    }

    /// Tokens the older part must hold for a compaction, and the growth
    /// after a failed one before the next attempt.
    pub fn gain(&self) -> u64 {
        (self.at as f64 * MIN_GAIN) as u64
    }
}

/// Share of `compact_at` the condensed part must hold at least.
pub const MIN_GAIN: f64 = 0.25;
/// How the message that replaces the older part begins.
pub const SUMMARY_PREFIX: &str = "[compacted: ";
/// Precedes the operator's latest message, appended to a summary verbatim.
const OPERATOR_MARKER: &str = "\n\n[The operator's latest message, verbatim:]\n";
/// Tokens kept free for the summary when choosing the recent turns.
const SUMMARY_TOKENS: u64 = (MAX_CONDENSED as u64 + 600).div_ceil(7) * 2;
/// The most the local model reads of the older part (four of its chunks).
const MAX_INPUT_CHARS: usize = 4 * CHUNK_CHARS;

/// How much of each item the local model reads, in characters: tool results
/// and call arguments are cut hardest (files can be read again, commands run
/// again), the engineer's own words least.
#[derive(Clone, Copy)]
struct Limits {
    user: usize,
    text: usize,
    reasoning: usize,
    argument: usize,
    result: usize,
}

const LIMITS: Limits = Limits {
    user: 16_000,
    text: 4_000,
    reasoning: 1_200,
    argument: 400,
    result: 1_200,
};
/// Used when the older part is still longer than [`MAX_INPUT_CHARS`].
const TIGHT: Limits = Limits {
    user: 8_000,
    text: 1_500,
    reasoning: 300,
    argument: 160,
    result: 300,
};

/// What a conversation keeps between events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// (Session) Position of the operator's latest message, or of the
    /// summary that carries it verbatim.
    pub operator: Option<usize>,
    /// After a failed compaction: no new attempt below this estimate.
    pub retry_at: Option<u64>,
}

/// What an event does to the conversation.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Masking alone brought the conversation to the target.
    Masked {
        positions: Vec<usize>,
        tokens_before: u64,
        tokens_after: u64,
    },
    /// Items `head..upto` are replaced by one message holding `text`.
    Compacted {
        head: usize,
        upto: usize,
        text: String,
        tokens_before: u64,
        tokens_after: u64,
        local_seconds: f64,
    },
    /// No summary: the conversation stays as it is.
    Failed {
        /// Items it would have condensed.
        items: usize,
        tokens: u64,
        retry_at: u64,
        reason: String,
        local_seconds: f64,
    },
}

impl Event {
    /// The transcript entry that records it.
    pub fn entry(&self) -> Entry {
        match self.clone() {
            Event::Masked {
                positions,
                tokens_before,
                tokens_after,
            } => Entry::Masked {
                items: positions.len(),
                tokens_before,
                tokens_after,
                positions,
            },
            Event::Compacted {
                head,
                upto,
                text,
                tokens_before,
                tokens_after,
                local_seconds,
            } => Entry::Compacted {
                head,
                upto,
                text,
                tokens_before,
                tokens_after,
                local_seconds,
            },
            Event::Failed {
                tokens,
                retry_at,
                reason,
                local_seconds,
                ..
            } => Entry::CompactionFailed {
                tokens,
                retry_at,
                reason,
                local_seconds,
            },
        }
    }

    /// The audit event of a compaction or a failed one (masking has none).
    pub fn audit(&self, child: Option<String>) -> Option<AuditEvent> {
        match self {
            Event::Masked { .. } => None,
            Event::Compacted {
                head,
                upto,
                tokens_before,
                tokens_after,
                local_seconds,
                ..
            } => Some(AuditEvent::Compaction {
                child,
                outcome: "compacted".into(),
                items: upto - head,
                tokens_before: *tokens_before,
                tokens_after: *tokens_after,
                local_seconds: *local_seconds,
            }),
            Event::Failed {
                items,
                tokens,
                local_seconds,
                ..
            } => Some(AuditEvent::Compaction {
                child,
                outcome: "failed".into(),
                items: *items,
                tokens_before: *tokens,
                tokens_after: *tokens,
                local_seconds: *local_seconds,
            }),
        }
    }
}

/// Decides the event due before the next request, if any, and for a
/// compaction has the summary written. Changes nothing: [`apply`] does,
/// once the event is recorded. `as_sent` gives items in the form they are
/// sent (after the outbound filters).
pub fn decide(
    items: &[Item],
    state: &State,
    system: &str,
    c: Compaction,
    as_sent: &dyn Fn(&[Item]) -> Vec<Item>,
    presenter: &dyn Presenter,
) -> Option<Event> {
    let before = estimate(items, system);
    if before < c.at || state.retry_at.is_some_and(|r| before < r) {
        return None;
    }
    let target = c.target();
    let (positions, masked) = mask_plan_from_estimate(items, before, target);
    if !positions.is_empty() && masked <= target {
        return Some(Event::Masked {
            positions,
            tokens_before: before,
            tokens_after: masked,
        });
    }
    let head = head_len(items);
    let upto = cut(items, system, head, target)?;
    let older = estimate(&items[head..upto], "");
    if older < c.gain() {
        return None;
    }
    let failed = |reason: String, local_seconds: f64| Event::Failed {
        items: upto - head,
        tokens: before,
        retry_at: before + c.gain(),
        reason,
        local_seconds,
    };
    let sent = as_sent(&items[head..upto]);
    let mut conversation = render(&sent, LIMITS);
    if conversation.len() > MAX_INPUT_CHARS {
        conversation = render(&sent, TIGHT);
    }
    if conversation.len() > MAX_INPUT_CHARS {
        return Some(failed(
            format!(
                "the older part is too long for the local model ({} characters; limit {MAX_INPUT_CHARS})",
                conversation.len()
            ),
            0.0,
        ));
    }
    let Some(done) = presenter.condense(&conversation) else {
        return Some(failed("no local model".into(), 0.0));
    };
    let summary = match done.summary {
        Ok(s) => s
            .trim()
            .replace(OPERATOR_MARKER.trim(), "(the operator's latest message)"),
        Err(e) => return Some(failed(e, done.seconds)),
    };
    if summary.is_empty() {
        return Some(failed("the local summary is empty".into(), done.seconds));
    }
    // Cleaning may lengthen a summary a little (placeholders); the local
    // reader holds it to MAX_CONDENSED.
    if summary.chars().count() > MAX_CONDENSED + MAX_CONDENSED / 4 {
        return Some(failed(
            format!(
                "the local summary is too long ({} characters)",
                summary.chars().count()
            ),
            done.seconds,
        ));
    }
    let operator = operator_text(&sent, head, upto, state);
    let text = summary_text(upto - head, older, &summary, operator.as_deref());
    let mut after = items[..head].to_vec();
    after.push(Item::User { text: text.clone() });
    after.extend_from_slice(&items[upto..]);
    let tokens_after = estimate(&after, system);
    if tokens_after >= before {
        return Some(failed(
            "the summary is no shorter than what it would replace".into(),
            done.seconds,
        ));
    }
    Some(Event::Compacted {
        head,
        upto,
        text,
        tokens_before: before,
        tokens_after,
        local_seconds: done.seconds,
    })
}

/// Applies a recorded event to the conversation.
pub fn apply(items: &mut Vec<Item>, state: &mut State, event: &Event) {
    match event {
        Event::Masked { positions, .. } => {
            apply_mask(items, positions);
        }
        Event::Compacted {
            head, upto, text, ..
        } => replace(items, state, *head, *upto, text),
        Event::Failed { retry_at, .. } => state.retry_at = Some(*retry_at),
    }
}

/// Replaces items `head..upto` by one user message holding `text` (a
/// `compacted` entry, live or replayed on resume).
pub fn replace(items: &mut Vec<Item>, state: &mut State, head: usize, upto: usize, text: &str) {
    if head >= upto || upto > items.len() {
        return;
    }
    items.splice(
        head..upto,
        [Item::User {
            text: text.to_owned(),
        }],
    );
    state.operator = match state.operator {
        Some(k) if k < head => Some(k),
        // Condensed: the summary carries it, when it could.
        Some(k) if k < upto => text.contains(OPERATOR_MARKER).then_some(head),
        Some(k) => Some(k + head + 1 - upto),
        None => None,
    };
    state.retry_at = None;
}

/// The first message and the images attached to it: never condensed.
fn head_len(items: &[Item]) -> usize {
    match items.first() {
        Some(Item::User { .. }) => {
            1 + items[1..]
                .iter()
                .take_while(|i| matches!(i, Item::Images { call_id: None, .. }))
                .count()
        }
        _ => 0,
    }
}

/// Where the recent turns begin: the earliest start of a turn (an assistant
/// message, or a user message between turns) from which the rest fits in
/// `target` together with the first message and a summary, but never later
/// than the start of the last [`KEEP_RECENT_TURNS`] turns. `None` when the
/// conversation has no more turns than those.
fn cut(items: &[Item], system: &str, head: usize, target: u64) -> Option<usize> {
    let budget = target.saturating_sub(estimate(&items[..head], system) + SUMMARY_TOKENS);
    let (mut recent, mut turns) = (0u64, 0usize);
    let (mut least, mut fits) = (None, None);
    for i in (head + 1..items.len()).rev() {
        recent += estimate(std::slice::from_ref(&items[i]), "");
        let start = matches!(items[i], Item::Assistant { .. } | Item::User { .. });
        if matches!(items[i], Item::Assistant { .. }) {
            turns += 1;
        }
        if !start || turns < KEEP_RECENT_TURNS {
            continue;
        }
        least.get_or_insert(i);
        if recent > budget {
            break;
        }
        fits = Some(i);
    }
    fits.or(least)
}

/// The operator's latest message, as sent, when it is among the items
/// condensed (or carried by an earlier summary among them).
fn operator_text(sent: &[Item], head: usize, upto: usize, state: &State) -> Option<String> {
    let k = state.operator.filter(|k| (head..upto).contains(k))?;
    match sent.get(k - head)? {
        Item::User { text } if text.starts_with(SUMMARY_PREFIX) => {
            text.rsplit_once(OPERATOR_MARKER).map(|(_, t)| t.to_owned())
        }
        Item::User { text } => Some(text.clone()),
        _ => None,
    }
}

/// The message that replaces `replaced` items (about `tokens` tokens).
pub fn summary_text(replaced: usize, tokens: u64, summary: &str, operator: Option<&str>) -> String {
    let mut text = format!(
        "{SUMMARY_PREFIX}{replaced} earlier items of this conversation (~{tokens} tokens) were \
condensed by the local model into the working notes below. File contents and command output \
from them are not repeated: read files and run commands again when you need them.]\n\n{summary}"
    );
    if let Some(op) = operator {
        text.push_str(OPERATOR_MARKER);
        text.push_str(op);
    }
    text
}

/// The older part as the local model reads it: every item in order, long
/// ones shortened by `limits`.
fn render(sent: &[Item], limits: Limits) -> String {
    let names: HashMap<&str, &str> = sent
        .iter()
        .filter_map(|i| match i {
            Item::Assistant { tool_calls, .. } => Some(tool_calls),
            _ => None,
        })
        .flatten()
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();
    let mut out = String::new();
    for item in sent {
        match item {
            Item::User { text } => {
                out.push_str(&format!("--- user\n{}\n\n", clip(text, limits.user)));
            }
            Item::Assistant {
                text,
                reasoning,
                tool_calls,
                ..
            } => {
                out.push_str("--- engineer\n");
                if let Some(r) = reasoning.as_deref().filter(|r| !r.trim().is_empty()) {
                    out.push_str(&format!("(thinking) {}\n", clip(r, limits.reasoning)));
                }
                if !text.trim().is_empty() {
                    out.push_str(&clip(text, limits.text));
                    out.push('\n');
                }
                for c in tool_calls {
                    let args = if c.arguments.is_empty() {
                        clip(&c.raw_arguments, limits.argument)
                    } else {
                        let mut v = Value::Object(c.arguments.clone());
                        clip_strings(&mut v, limits.argument);
                        v.to_string()
                    };
                    out.push_str(&format!("call {} {args}\n", c.name));
                }
                out.push('\n');
            }
            Item::ToolResult { call_id, content } => out.push_str(&format!(
                "--- result of {}\n{}\n\n",
                names.get(call_id.as_str()).copied().unwrap_or("a call"),
                clip(content, limits.result)
            )),
            Item::Images { images, .. } => {
                out.push_str(&format!("--- {} image(s)\n\n", images.len()));
            }
        }
    }
    out
}

/// `text`, or its beginning and end with what is left out counted.
fn clip(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max * 2 / 3).collect();
    let tail: String = text.chars().skip(n - max / 3).collect();
    format!(
        "{head}\n…[{} characters left out]…\n{tail}",
        n - max * 2 / 3 - max / 3
    )
}

fn clip_strings(v: &mut Value, max: usize) {
    match v {
        Value::String(s) if s.chars().count() > max => *s = clip(s, max),
        Value::Array(a) => a.iter_mut().for_each(|x| clip_strings(x, max)),
        Value::Object(m) => m.values_mut().for_each(|x| clip_strings(x, max)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::is_masked;
    use crate::transcript::Entry;
    use duet_boundary::model::ToolCall;
    use duet_boundary::view::{Condensed, Source};
    use serde_json::json;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// A local model stand-in: answers every summary request with the next
    /// scripted reply (the last one repeats) and keeps what it read.
    struct Local {
        replies: Mutex<Vec<Result<String, String>>>,
        read: Mutex<Vec<String>>,
    }

    impl Local {
        fn new(replies: Vec<Result<String, String>>) -> Self {
            Self {
                replies: Mutex::new(replies),
                read: Mutex::new(Vec::new()),
            }
        }

        fn ok(summary: &str) -> Self {
            Self::new(vec![Ok(summary.to_owned())])
        }

        fn calls(&self) -> usize {
            self.read.lock().unwrap().len()
        }
    }

    impl Presenter for Local {
        fn present(&self, _source: &Source, bytes: &[u8]) -> String {
            String::from_utf8_lossy(bytes).into_owned()
        }
        fn can_condense(&self) -> bool {
            true
        }
        fn condense(&self, conversation: &str) -> Option<Condensed> {
            self.read.lock().unwrap().push(conversation.to_owned());
            let mut replies = self.replies.lock().unwrap();
            let reply = if replies.len() > 1 {
                replies.remove(0)
            } else {
                replies[0].clone()
            };
            Some(Condensed {
                summary: reply,
                seconds: 1.5,
            })
        }
    }

    /// No local model (pass-through).
    struct NoLocal;
    impl Presenter for NoLocal {
        fn present(&self, _source: &Source, bytes: &[u8]) -> String {
            String::from_utf8_lossy(bytes).into_owned()
        }
    }

    fn same(items: &[Item]) -> Vec<Item> {
        items.to_vec()
    }

    const NOTES: &str = "Task: fix the parser.\nDone: src/lexer.rs (tokens).\nOpen: tests.";

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        let Value::Object(arguments) = args else {
            unreachable!()
        };
        ToolCall {
            id: id.into(),
            name: name.into(),
            raw_arguments: serde_json::to_string(&arguments).unwrap(),
            arguments,
        }
    }

    /// Appends `n` turns (numbered from `from`), each an assistant message
    /// with `per_turn` reads and their results of `size` bytes.
    fn add_turns(v: &mut Vec<Item>, from: usize, n: usize, per_turn: usize, size: usize) {
        for t in from..from + n {
            let calls: Vec<ToolCall> = (0..per_turn)
                .map(|k| {
                    call(
                        &format!("c{t}-{k}"),
                        "read_file",
                        json!({"path": format!("src/f{t}_{k}.rs")}),
                    )
                })
                .collect();
            v.push(Item::Assistant {
                text: format!("Step {t}: reading the next files. {}", "why ".repeat(40)),
                reasoning: None,
                replay: None,
                tool_calls: calls.clone(),
            });
            for c in calls {
                v.push(Item::ToolResult {
                    call_id: c.id,
                    content: "x".repeat(size),
                });
            }
        }
    }

    fn conversation(n: usize, per_turn: usize, size: usize) -> Vec<Item> {
        let mut v = vec![Item::User {
            text: "Fix the parser.".into(),
        }];
        add_turns(&mut v, 0, n, per_turn, size);
        v
    }

    /// Every result follows its call's assistant message, before the next
    /// assistant message, and every call has its result.
    fn assert_paired(items: &[Item]) {
        let mut open: HashSet<String> = HashSet::new();
        for item in items {
            match item {
                Item::Assistant { tool_calls, .. } => {
                    assert!(open.is_empty(), "calls without results: {open:?}");
                    open = tool_calls.iter().map(|c| c.id.clone()).collect();
                }
                Item::ToolResult { call_id, .. } => {
                    assert!(open.remove(call_id), "a result without its call: {call_id}");
                }
                Item::Images {
                    call_id: Some(c), ..
                } => assert!(!open.contains(c)),
                _ => {}
            }
        }
        assert!(open.is_empty(), "calls without results: {open:?}");
    }

    fn apply_all(items: &mut Vec<Item>, state: &mut State, event: &Event) {
        apply(items, state, event);
    }

    #[test]
    fn nothing_happens_below_the_threshold() {
        let v = conversation(10, 1, 2000);
        let c = Compaction {
            at: estimate(&v, "s") + 1,
            to: 0.4,
        };
        let local = Local::ok(NOTES);
        assert!(decide(&v, &State::default(), "s", c, &same, &local).is_none());
        assert_eq!(local.calls(), 0);
    }

    #[test]
    fn masking_alone_is_used_when_it_reaches_the_target() {
        // Bulky results, short messages: stubs are enough.
        let v = conversation(20, 1, 20_000);
        let c = Compaction {
            at: estimate(&v, "s") - 100,
            to: 0.4,
        };
        let local = Local::ok(NOTES);
        let Some(Event::Masked {
            positions,
            tokens_after,
            ..
        }) = decide(&v, &State::default(), "s", c, &same, &local)
        else {
            panic!("masking was enough");
        };
        assert!(!positions.is_empty() && tokens_after <= c.target());
        assert_eq!(local.calls(), 0, "no local call");
    }

    #[test]
    fn compaction_keeps_the_task_and_recent_turns_and_is_rare() {
        // Many small results: masking cannot reach the target.
        let mut v = conversation(80, 2, 300);
        let at = estimate(&v, "s") - 50;
        let c = Compaction { at, to: 0.4 };
        let local = Local::ok(NOTES);
        let mut state = State::default();
        let event = decide(&v, &state, "s", c, &same, &local).expect("an event");
        let Event::Compacted {
            head,
            upto,
            text,
            tokens_after,
            ..
        } = &event
        else {
            panic!("{event:?}");
        };
        assert_eq!(*head, 1, "the task is never condensed");
        assert!(
            text.starts_with(SUMMARY_PREFIX) && text.ends_with(NOTES),
            "{text}"
        );
        let before = v.clone();
        apply_all(&mut v, &mut state, &event);
        assert_eq!(v[0], before[0]);
        assert_eq!(v[1], Item::User { text: text.clone() });
        assert_eq!(&v[2..], &before[*upto..], "the recent turns are verbatim");
        assert!(matches!(v[2], Item::Assistant { .. }));
        assert_eq!(estimate(&v, "s"), *tokens_after);
        assert!(*tokens_after <= c.target() + 200, "{tokens_after}");
        assert_paired(&v);
        // Right after, and while the conversation grows below the
        // threshold again, nothing happens.
        assert!(decide(&v, &state, "s", c, &same, &local).is_none());
        let mut t = 80;
        while estimate(&v, "s") < at {
            assert!(decide(&v, &state, "s", c, &same, &local).is_none());
            add_turns(&mut v, t, 1, 2, 300);
            t += 1;
        }
        assert_eq!(local.calls(), 1);
        // Past it again: the next event condenses the summary with what came since.
        let event = decide(&v, &state, "s", c, &same, &local).expect("an event");
        assert!(
            matches!(event, Event::Compacted { head: 1, .. }),
            "{event:?}"
        );
        assert!(local.read.lock().unwrap()[1].contains(SUMMARY_PREFIX));
        apply_all(&mut v, &mut state, &event);
        assert_paired(&v);
    }

    #[test]
    fn the_cut_never_separates_a_call_from_its_result() {
        // Turns of one to three calls, a result with an image after it,
        // nudges between turns, text-only turns.
        let png = duet_boundary::model::solid_png(64, 48, [1, 2, 3]);
        let img = duet_boundary::model::prepare_image(&png, 1568).unwrap();
        let mut v = vec![
            Item::User {
                text: "Fix the parser.".into(),
            },
            Item::Images {
                call_id: None,
                images: vec![img.clone()],
            },
        ];
        for t in 0..60 {
            let per_turn = 1 + t % 3;
            add_turns(&mut v, t, 1, per_turn, 250 + 37 * (t % 5));
            if t % 7 == 0 {
                let id = format!("c{t}-0");
                let at = v
                    .iter()
                    .position(|i| matches!(i, Item::ToolResult { call_id, .. } if *call_id == id))
                    .unwrap();
                v.insert(
                    at + 1,
                    Item::Images {
                        call_id: Some(id),
                        images: vec![img.clone()],
                    },
                );
            }
            if t % 5 == 0 {
                v.push(Item::Assistant {
                    text: "Thinking out loud.".into(),
                    reasoning: None,
                    replay: None,
                    tool_calls: Vec::new(),
                });
                v.push(Item::User {
                    text: "Continue working with the tools.".into(),
                });
            }
        }
        assert_paired(&v);
        for to in [0.1, 0.2, 0.4, 0.6, 0.8] {
            let c = Compaction {
                at: estimate(&v, "s") / 2,
                to,
            };
            let local = Local::ok(NOTES);
            let mut state = State::default();
            if let Some(Event::Compacted { head, upto, .. }) =
                decide(&v, &state, "s", c, &same, &local)
            {
                assert_eq!(head, 2, "the task and its image stay");
                assert!(matches!(
                    v[upto],
                    Item::Assistant { .. } | Item::User { .. }
                ));
                let mut w = v.clone();
                let event = decide(&v, &state, "s", c, &same, &local).unwrap();
                apply_all(&mut w, &mut state, &event);
                assert_paired(&w);
                let turns = w
                    .iter()
                    .filter(|i| matches!(i, Item::Assistant { .. }))
                    .count();
                assert!(turns >= KEEP_RECENT_TURNS);
            }
        }
    }

    #[test]
    fn failures_leave_the_conversation_and_wait_for_growth() {
        let v = conversation(80, 2, 300);
        let at = estimate(&v, "s") - 50;
        let c = Compaction { at, to: 0.4 };
        for (presenter, why) in [
            (
                Box::new(Local::new(vec![Err("local model unreachable".into())]))
                    as Box<dyn Presenter>,
                "unreachable",
            ),
            (Box::new(Local::ok("   ")), "empty"),
            (
                Box::new(Local::ok(&"n".repeat(MAX_CONDENSED * 2))),
                "too long",
            ),
            (Box::new(NoLocal), "no local model"),
        ] {
            let mut w = v.clone();
            let mut state = State::default();
            let event = decide(&w, &state, "s", c, &same, presenter.as_ref()).expect(why);
            let Event::Failed {
                retry_at, reason, ..
            } = &event
            else {
                panic!("{why}: {event:?}");
            };
            assert!(reason.contains(why), "{reason}");
            assert_eq!(*retry_at, estimate(&w, "s") + c.gain());
            assert!(event.audit(None).is_some());
            apply_all(&mut w, &mut state, &event);
            assert_eq!(w, v, "{why}: the conversation is unchanged");
            // No new attempt until the conversation has grown.
            assert!(decide(&w, &state, "s", c, &same, presenter.as_ref()).is_none());
            let mut t = 80;
            while estimate(&w, "s") < *retry_at {
                assert!(decide(&w, &state, "s", c, &same, presenter.as_ref()).is_none());
                add_turns(&mut w, t, 1, 2, 300);
                t += 1;
            }
            assert!(decide(&w, &state, "s", c, &same, presenter.as_ref()).is_some());
        }
        // A summary no shorter than what it replaces is refused too.
        let small = conversation(12, 1, 300);
        let c = Compaction {
            at: estimate(&small, "s") - 10,
            to: 0.1,
        };
        let long = "notes ".repeat(1500);
        let event = decide(&small, &State::default(), "s", c, &same, &Local::ok(&long));
        assert!(
            matches!(&event, Some(Event::Failed { reason, .. }) if reason.contains("no shorter")),
            "{event:?}"
        );
    }

    #[test]
    fn the_local_model_reads_the_sent_form_shortened() {
        let mut v = conversation(80, 2, 300);
        if let Item::ToolResult { content, .. } = &mut v[3] {
            *content = format!("{}SECRET-RAW{}", "a".repeat(2000), "b".repeat(2000));
        }
        let c = Compaction {
            at: estimate(&v, "s") - 50,
            to: 0.4,
        };
        let local = Local::ok(NOTES);
        // A stand-in for the outbound filter.
        let as_sent = |older: &[Item]| -> Vec<Item> {
            let mut out = older.to_vec();
            for i in &mut out {
                if let Item::ToolResult { content, .. } = i {
                    *content = content.replace("SECRET-RAW", "⟨secret:KEY#1⟩");
                }
            }
            out
        };
        let event = decide(&v, &State::default(), "s", c, &as_sent, &local);
        assert!(matches!(event, Some(Event::Compacted { .. })), "{event:?}");
        let read = local.read.lock().unwrap()[0].clone();
        assert!(!read.contains("SECRET-RAW"));
        assert!(
            read.contains("characters left out"),
            "long results are shortened"
        );
        assert!(read.contains("call read_file {\"path\":\"src/f0_0.rs\"}"));
        assert!(read.contains("--- result of read_file"));
    }

    #[test]
    fn the_operators_latest_message_is_carried_verbatim() {
        let mut v = conversation(40, 2, 300);
        let ask = "Now also accept tabs as separators, please.";
        let at_msg = v.len();
        v.push(Item::User { text: ask.into() });
        add_turns(&mut v, 40, 40, 2, 300);
        let mut state = State {
            operator: Some(at_msg),
            retry_at: None,
        };
        let c = Compaction {
            at: estimate(&v, "s") - 50,
            to: 0.4,
        };
        let local = Local::ok(NOTES);
        let event = decide(&v, &state, "s", c, &same, &local).unwrap();
        let Event::Compacted { text, upto, .. } = &event else {
            panic!("{event:?}");
        };
        assert!(*upto > at_msg);
        assert!(text.ends_with(&format!("{OPERATOR_MARKER}{ask}")), "{text}");
        apply_all(&mut v, &mut state, &event);
        assert_eq!(state.operator, Some(1), "the summary carries it");
        // The next compaction carries it again.
        let mut t = 80;
        while estimate(&v, "s") < c.at {
            add_turns(&mut v, t, 1, 2, 300);
            t += 1;
        }
        let event = decide(&v, &state, "s", c, &same, &local).unwrap();
        let Event::Compacted { text, .. } = &event else {
            panic!("{event:?}");
        };
        assert!(text.ends_with(&format!("{OPERATOR_MARKER}{ask}")), "{text}");
        // A message after the cut moves with the items after it; one
        // condensed without being carried is forgotten.
        let mut w = conversation(40, 2, 300);
        let k = w.len() - 3;
        let kept = w[k].clone();
        let mut s2 = State {
            operator: Some(k),
            retry_at: Some(9),
        };
        replace(&mut w, &mut s2, 1, 5, "[compacted: x");
        assert_eq!(
            s2,
            State {
                operator: Some(k - 3),
                retry_at: None
            }
        );
        assert_eq!(w[k - 3], kept);
        let mut s3 = State {
            operator: Some(2),
            retry_at: None,
        };
        replace(&mut w, &mut s3, 1, 5, "[compacted: y");
        assert_eq!(s3.operator, None);
    }

    #[test]
    fn a_replayed_transcript_rebuilds_the_same_conversation() {
        // Events as the loop records them, replayed as a resume does.
        let mut v = conversation(80, 2, 300);
        let mut state = State::default();
        let mut entries: Vec<Entry> = v.iter().map(|i| Entry::Item { item: i.clone() }).collect();
        let c = Compaction {
            at: estimate(&v, "s") - 50,
            to: 0.4,
        };
        let local = Local::new(vec![
            Ok(NOTES.into()),
            Err("local model unreachable".into()),
            Ok(format!("{NOTES}\nMore.")),
        ]);
        let mut kinds = Vec::new();
        for t in 80..480 {
            if let Some(event) = decide(&v, &state, "s", c, &same, &local) {
                kinds.push(event.entry());
                entries.push(event.entry());
                apply(&mut v, &mut state, &event);
            }
            let from = v.len();
            add_turns(&mut v, t, 1, 2, 300 + t % 400);
            entries.extend(v[from..].iter().map(|i| Entry::Item { item: i.clone() }));
        }
        let compacted = kinds
            .iter()
            .filter(|e| matches!(e, Entry::Compacted { .. }))
            .count();
        let failed = kinds
            .iter()
            .filter(|e| matches!(e, Entry::CompactionFailed { .. }))
            .count();
        assert!(compacted >= 2 && failed == 1, "{compacted} {failed}");
        // Through JSON, as the transcript stores them.
        let entries: Vec<Entry> = entries
            .iter()
            .map(|e| serde_json::from_str(&serde_json::to_string(e).unwrap()).unwrap())
            .collect();
        let mut conv = crate::run::Conversation {
            scoped_instructions: Default::default(),
            system: "s".into(),
            specs: Vec::new(),
            git_tools: None,
            items: Vec::new(),
            classes: HashMap::new(),
            interactive: false,
            planning: false,
            steering: None,
            exchange: 0,
            child: None,
            context: State::default(),
        };
        let mut stats = crate::run::RunStats::default();
        crate::run::replay_priced(entries, &|_| 0.0, &mut conv, &mut stats);
        assert_eq!(conv.items, v, "byte for byte");
        assert_eq!(
            serde_json::to_string(&conv.items).unwrap(),
            serde_json::to_string(&v).unwrap()
        );
        assert_eq!(conv.context, state);
        assert_eq!(stats.compactions as usize, compacted);
    }

    #[test]
    fn masking_positions_replay_exactly() {
        let mut v = conversation(20, 1, 7000);
        let window = estimate(&v, "s") + 1000;
        let unmasked = v.clone();
        let positions = crate::context::mask_if_needed(&mut v, "s", window, 0.7);
        assert!(!positions.is_empty());
        let mut again = unmasked.clone();
        assert_eq!(apply_mask(&mut again, &positions), positions.len());
        assert_eq!(again, v);
        assert!(
            positions
                .iter()
                .all(|&p| matches!(&v[p], Item::ToolResult { content, .. } if is_masked(content)))
        );
    }
}
