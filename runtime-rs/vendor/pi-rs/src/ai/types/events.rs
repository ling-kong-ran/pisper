//! Assistant message event protocol from upstream `packages/ai/src/types.ts:629-661`
//! plus the live partial reducer ported from
//! `packages/ai/src/utils/assistant-message-frame.ts` (`reduceAssistantMessageFrames`).
//!
//! Successful streams emit `start` before partial updates and terminate with
//! `done`; after `start`, failures terminate with `error`, and request setup
//! may fail with a lone `error` before `start` (README "Complete Event
//! Reference"). Wire `type` values are the upstream literals (`start`,
//! `text_start`, ..., `toolcall_delta`, `done`, `error`), field names are
//! camelCase (`contentIndex`, `toolCall`), `done.reason` is one of
//! stop|length|toolUse|deferred and `error.reason` one of aborted|error.
//!
//! Documented deviation (spec section 3): upstream events carry a shared live
//! `partial` field on every event; here events carry no `partial` field.
//! [`PartialAssistant`] replaces it: feed every event through
//! [`PartialAssistant::apply`] and read [`PartialAssistant::message`] for the
//! live response-so-far, reconstructed with the same rules as upstream's
//! canonical frame reducer:
//!
//! - `start` seeds message metadata (api/provider/model/timestamp, usage,
//!   response ids, diagnostics) with empty content and `stopReason: "pending"`
//!   (frame encoder's `cloneStartMessage`). Upstream's `start.partial` is the
//!   initial assistant message structure, so it rides on the event as
//!   `message` — the only payload field the upstream start event has.
//! - `text_start`/`thinking_start` append an empty block at `contentIndex`;
//!   deltas append to that block; `*_end` content is authoritative and
//!   replaces the accumulated text.
//! - `toolcall_start` appends an empty placeholder tool call (upstream
//!   providers may already hold arguments in their live partial — that channel
//!   is the removed `partial`, so in the port arguments arrive through deltas
//!   and the authoritative end); `toolcall_delta` accumulates raw JSON
//!   fragments (the frame reducer's `state.json`, parse deferred);
//!   `toolcall_end` replaces the block with the complete tool call.
//! - Redacted thinking is complete at `thinking_start` upstream and emits no
//!   deltas; in the port its content arrives with `thinking_end`.
//! - `done` makes the message final (the event's final message replaces the
//!   accumulator); `error` keeps the partial content and settles `stopReason`
//!   from `reason` and `errorMessage` from the event's error message.
//!
//! Strictness mirrors the upstream frame encoder/reducer: duplicate starts,
//! block/done events before `start`, any event after a terminal event,
//! duplicate block starts, index gaps, block-kind mismatches, and deltas or
//! ends after a block ended all return `Err`. `error` before `start` is
//! allowed (pre-generation failure) and terminates the stream. A rejected
//! event leaves the accumulator unchanged.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::content::{TextContent, ThinkingContent, ToolCall};
use super::message::{AssistantBlock, AssistantMessage};
use super::primitives::StopReason;

/// Upstream `Extract<StopReason, "stop" | "length" | "toolUse" | "deferred">`
/// (types.ts:658): the `reason` carried by the `done` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SuccessReason {
    Stop,
    Length,
    ToolUse,
    Deferred,
}

/// Upstream `Extract<StopReason, "aborted" | "error">` (types.ts:661): the
/// `reason` carried by the `error` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorReason {
    Aborted,
    Error,
}

/// Event protocol for assistant message streams (types.ts:645-661). Wire
/// `type` values are the upstream literals; field names are camelCase.
///
/// Documented deviation (spec section 3): upstream events each carry the
/// shared live `partial` response-so-far; here they carry no `partial` field —
/// [`PartialAssistant`] reconstructs it from the event sequence with the same
/// rules. The one exception in the other direction is [`AssistantMessageEvent::Start`]:
/// upstream's `start.partial` is the initial assistant message structure
/// (metadata with empty content), which is event payload rather than a
/// redundant live snapshot, so it rides on the event as `message`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AssistantMessageEvent {
    /// Stream begins. `message` is the initial assistant message structure:
    /// metadata with empty content (upstream `start.partial`).
    Start { message: AssistantMessage },
    /// A text block starts at `content_index`; its content is empty and grows
    /// through [`AssistantMessageEvent::TextDelta`] until the authoritative
    /// [`AssistantMessageEvent::TextEnd`].
    TextStart { content_index: usize },
    /// New text for the text block at `content_index`.
    TextDelta { content_index: usize, delta: String },
    /// The text block at `content_index` is complete; `content` is the
    /// authoritative full text.
    TextEnd {
        content_index: usize,
        content: String,
    },
    /// A thinking block starts at `content_index`. Redacted thinking may be
    /// complete at start upstream and emit no deltas; in this port its content
    /// arrives with [`AssistantMessageEvent::ThinkingEnd`].
    ThinkingStart { content_index: usize },
    /// New thinking text for the thinking block at `content_index`.
    ThinkingDelta { content_index: usize, delta: String },
    /// The thinking block at `content_index` is complete; `content` is the
    /// authoritative full thinking text.
    ThinkingEnd {
        content_index: usize,
        content: String,
    },
    /// A tool call starts at `content_index` as an empty placeholder. Upstream
    /// providers may already hold initial arguments in their live partial;
    /// that channel is the removed `partial`, so in this port arguments arrive
    /// through deltas and the authoritative end.
    ToolcallStart { content_index: usize },
    /// A JSON fragment for the tool call at `content_index`.
    ToolcallDelta { content_index: usize, delta: String },
    /// The tool call at `content_index` is complete; `tool_call` replaces the
    /// block wholesale (not schema-validated upstream — run
    /// `validate_tool_call` before execution).
    ToolcallEnd {
        content_index: usize,
        tool_call: ToolCall,
    },
    /// Stream complete. `message` is the final assistant message and replaces
    /// the reconstructed partial.
    Done {
        reason: SuccessReason,
        message: AssistantMessage,
    },
    /// Stream failed. `error` carries the failed message; after `start` the
    /// reducer keeps its partial content and settles `stopReason` from `reason`
    /// and `errorMessage` from `error.error_message`.
    Error {
        reason: ErrorReason,
        error: AssistantMessage,
    },
}

impl AssistantMessageEvent {
    /// The upstream wire `type` value for this event.
    pub fn event_type(&self) -> &'static str {
        match self {
            AssistantMessageEvent::Start { .. } => "start",
            AssistantMessageEvent::TextStart { .. } => "text_start",
            AssistantMessageEvent::TextDelta { .. } => "text_delta",
            AssistantMessageEvent::TextEnd { .. } => "text_end",
            AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
            AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
            AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
            AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
            AssistantMessageEvent::ToolcallDelta { .. } => "toolcall_delta",
            AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
            AssistantMessageEvent::Done { .. } => "done",
            AssistantMessageEvent::Error { .. } => "error",
        }
    }
}

impl From<SuccessReason> for StopReason {
    fn from(reason: SuccessReason) -> Self {
        match reason {
            SuccessReason::Stop => StopReason::Stop,
            SuccessReason::Length => StopReason::Length,
            SuccessReason::ToolUse => StopReason::ToolUse,
            SuccessReason::Deferred => StopReason::Deferred,
        }
    }
}

impl From<ErrorReason> for StopReason {
    fn from(reason: ErrorReason) -> Self {
        match reason {
            ErrorReason::Aborted => StopReason::Aborted,
            ErrorReason::Error => StopReason::Error,
        }
    }
}

/// Which kind of content block a reducer block state tracks. `as_str` uses the
/// upstream block type literals (`"toolCall"`, not `"tool_call"`), matching
/// upstream error message phrasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    ToolCall,
}

impl BlockKind {
    fn as_str(self) -> &'static str {
        match self {
            BlockKind::Text => "text",
            BlockKind::Thinking => "thinking",
            BlockKind::ToolCall => "toolCall",
        }
    }
}

/// Per-open-block reducer state, the port of the frame reducer's
/// `ReducerBlockState`.
#[derive(Debug, Clone)]
struct BlockState {
    kind: BlockKind,
    ended: bool,
    /// Accumulated raw JSON fragments for tool-call blocks (the frame
    /// reducer's `state.json`); parsing is deferred and the authoritative
    /// `toolcall_end` tool call replaces the block.
    json: String,
}

/// Live reconstruction of the assistant message from an
/// [`AssistantMessageEvent`] stream — the port's replacement for upstream's
/// per-event `partial` field (spec section 3).
///
/// Feed every event in stream order through [`PartialAssistant::apply`] and
/// read [`PartialAssistant::message`] for the response-so-far. The
/// reconstruction rules are ported from upstream's canonical frame reducer
/// (`reduceAssistantMessageFrames` in utils/assistant-message-frame.ts) and
/// its strictness from the frame encoder: malformed sequences are rejected
/// with `Err` and leave the accumulator unchanged.
#[derive(Debug, Default, Clone)]
pub struct PartialAssistant {
    message: Option<AssistantMessage>,
    terminal: bool,
    blocks: HashMap<usize, BlockState>,
}

impl PartialAssistant {
    pub fn new() -> Self {
        Self::default()
    }

    /// The live message-so-far. `None` until `start` (or a pre-start `error`).
    pub fn message(&self) -> Option<&AssistantMessage> {
        self.message.as_ref()
    }

    /// Whether a terminal event (`done` or `error`) has been applied.
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Applies one event, updating the live message-so-far. Returns `Err`
    /// (with an upstream-style description) for malformed sequences; a
    /// rejected event leaves the accumulator unchanged.
    pub fn apply(&mut self, event: &AssistantMessageEvent) -> Result<(), String> {
        if self.terminal {
            return Err(format!(
                "Assistant message event {} follows a terminal event",
                event.event_type()
            ));
        }

        match event {
            AssistantMessageEvent::Start { message } => {
                if self.message.is_some() {
                    return Err(
                        "Assistant message stream contains more than one start event".into(),
                    );
                }
                self.message = Some(start_message(message));
                return Ok(());
            }
            AssistantMessageEvent::Done {
                message: final_message,
                ..
            } => {
                if self.message.is_none() {
                    return Err("Assistant message done event appears before start".into());
                }
                self.terminal = true;
                self.message = Some(final_message.clone());
                return Ok(());
            }
            AssistantMessageEvent::Error { reason, error } => {
                // Pre-generation failures terminate with `error` before
                // `start` (types.ts:633-634): the event's error message
                // becomes the message. After `start`, the partial content is
                // kept and only the settlement fields change.
                self.terminal = true;
                if let Some(message) = self.message.as_mut() {
                    message.stop_reason = StopReason::from(*reason);
                    message.error_message = error.error_message.clone();
                } else {
                    let mut message = error.clone();
                    message.stop_reason = StopReason::from(*reason);
                    self.message = Some(message);
                }
                return Ok(());
            }
            _ => {}
        }

        let event_type = event.event_type();
        let message = self
            .message
            .as_mut()
            .ok_or_else(|| format!("Assistant message {event_type} event appears before start"))?;

        match event {
            AssistantMessageEvent::TextStart { content_index } => {
                append_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    AssistantBlock::Text(TextContent {
                        text: String::new(),
                        text_signature: None,
                    }),
                    BlockKind::Text,
                )?;
            }
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                let (block, _) = active_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    BlockKind::Text,
                    event_type,
                )?;
                match block {
                    AssistantBlock::Text(text) => text.text += delta.as_str(),
                    _ => return Err("Unreachable text block state".into()),
                }
            }
            AssistantMessageEvent::TextEnd {
                content_index,
                content,
            } => {
                let (block, state) = active_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    BlockKind::Text,
                    event_type,
                )?;
                match block {
                    AssistantBlock::Text(text) => text.text = content.clone(),
                    _ => return Err("Unreachable text block state".into()),
                }
                state.ended = true;
            }
            AssistantMessageEvent::ThinkingStart { content_index } => {
                append_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    AssistantBlock::Thinking(ThinkingContent {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: None,
                    }),
                    BlockKind::Thinking,
                )?;
            }
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                let (block, _) = active_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    BlockKind::Thinking,
                    event_type,
                )?;
                match block {
                    AssistantBlock::Thinking(thinking) => thinking.thinking += delta.as_str(),
                    _ => return Err("Unreachable thinking block state".into()),
                }
            }
            AssistantMessageEvent::ThinkingEnd {
                content_index,
                content,
            } => {
                let (block, state) = active_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    BlockKind::Thinking,
                    event_type,
                )?;
                match block {
                    AssistantBlock::Thinking(thinking) => thinking.thinking = content.clone(),
                    _ => return Err("Unreachable thinking block state".into()),
                }
                state.ended = true;
            }
            AssistantMessageEvent::ToolcallStart { content_index } => {
                append_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    AssistantBlock::ToolCall(ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: serde_json::json!({}),
                        thought_signature: None,
                        namespace: None,
                    }),
                    BlockKind::ToolCall,
                )?;
            }
            AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta,
            } => {
                // Frame-reducer rule: raw JSON fragments accumulate in block
                // state; parsing is deferred and the authoritative end
                // replaces the block.
                let (_, state) = active_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    BlockKind::ToolCall,
                    event_type,
                )?;
                state.json += delta.as_str();
            }
            AssistantMessageEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                let (block, state) = active_block(
                    message,
                    &mut self.blocks,
                    *content_index,
                    BlockKind::ToolCall,
                    event_type,
                )?;
                *block = AssistantBlock::ToolCall(tool_call.clone());
                state.ended = true;
            }
            // Handled above.
            AssistantMessageEvent::Start { .. }
            | AssistantMessageEvent::Done { .. }
            | AssistantMessageEvent::Error { .. } => {}
        }
        Ok(())
    }
}

/// Port of the frame encoder's `cloneStartMessage`: the start event's message
/// seeds the accumulator as response metadata with empty content and
/// `stopReason: "pending"`.
fn start_message(message: &AssistantMessage) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: message.api.clone(),
        provider: message.provider.clone(),
        model: message.model.clone(),
        response_model: message.response_model.clone(),
        response_id: message.response_id.clone(),
        provider_thinking_level: message.provider_thinking_level.clone(),
        diagnostics: message.diagnostics.clone(),
        usage: message.usage,
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: message.timestamp,
    }
}

/// Appends a new block at `content_index`, mirroring the frame reducer's
/// `appendBlock`: the index must be exactly the next content slot.
fn append_block(
    message: &mut AssistantMessage,
    states: &mut HashMap<usize, BlockState>,
    content_index: usize,
    block: AssistantBlock,
    kind: BlockKind,
) -> Result<(), String> {
    if content_index != message.content.len() {
        let reason = if content_index < message.content.len() {
            "already exists"
        } else {
            "would leave a gap"
        };
        return Err(format!(
            "Cannot start assistant message block at index {content_index}: {reason}"
        ));
    }
    message.content.push(block);
    states.insert(
        content_index,
        BlockState {
            kind,
            ended: false,
            json: String::new(),
        },
    );
    Ok(())
}

/// The started, not-yet-ended block at `content_index`, mirroring the frame
/// reducer's `activeBlock` checks (started, kind match on both the tracked
/// state and the stored block, not ended).
fn active_block<'a>(
    message: &'a mut AssistantMessage,
    states: &'a mut HashMap<usize, BlockState>,
    content_index: usize,
    expected: BlockKind,
    event_type: &str,
) -> Result<(&'a mut AssistantBlock, &'a mut BlockState), String> {
    let mismatch = |found: BlockKind| {
        format!(
            "{event_type} event expected {} block at index {content_index}, found {}",
            expected.as_str(),
            found.as_str()
        )
    };
    let Some(state) = states.get_mut(&content_index) else {
        return Err(format!(
            "{event_type} event has no started block at index {content_index}"
        ));
    };
    if state.kind != expected {
        return Err(mismatch(state.kind));
    }
    let Some(block) = message.content.get_mut(content_index) else {
        return Err(format!(
            "{event_type} event has no started block at index {content_index}"
        ));
    };
    let found = block_kind(block);
    if found != expected {
        return Err(mismatch(found));
    }
    if state.ended {
        return Err(format!(
            "{event_type} event follows the end of block at index {content_index}"
        ));
    }
    Ok((block, state))
}

fn block_kind(block: &AssistantBlock) -> BlockKind {
    match block {
        AssistantBlock::Text(_) => BlockKind::Text,
        AssistantBlock::Thinking(_) => BlockKind::Thinking,
        AssistantBlock::ToolCall(_) => BlockKind::ToolCall,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{ThinkingContent, Usage};

    const TS: i64 = 1758240000000;

    fn assistant_message(ts: i64) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Pending,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: ts,
        }
    }

    fn text_block(text: &str) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })
    }

    fn thinking_block(text: &str) -> AssistantBlock {
        AssistantBlock::Thinking(ThinkingContent {
            thinking: text.into(),
            thinking_signature: None,
            redacted: None,
        })
    }

    fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            thought_signature: None,
            namespace: None,
        }
    }

    #[test]
    fn event_wire_type_values_match_upstream() {
        let cases = [
            (
                AssistantMessageEvent::Start {
                    message: assistant_message(TS),
                },
                "start",
            ),
            (
                AssistantMessageEvent::TextStart { content_index: 0 },
                "text_start",
            ),
            (
                AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: "x".into(),
                },
                "text_delta",
            ),
            (
                AssistantMessageEvent::TextEnd {
                    content_index: 0,
                    content: "x".into(),
                },
                "text_end",
            ),
            (
                AssistantMessageEvent::ThinkingStart { content_index: 0 },
                "thinking_start",
            ),
            (
                AssistantMessageEvent::ThinkingDelta {
                    content_index: 0,
                    delta: "x".into(),
                },
                "thinking_delta",
            ),
            (
                AssistantMessageEvent::ThinkingEnd {
                    content_index: 0,
                    content: "x".into(),
                },
                "thinking_end",
            ),
            (
                AssistantMessageEvent::ToolcallStart { content_index: 0 },
                "toolcall_start",
            ),
            (
                AssistantMessageEvent::ToolcallDelta {
                    content_index: 0,
                    delta: "x".into(),
                },
                "toolcall_delta",
            ),
            (
                AssistantMessageEvent::ToolcallEnd {
                    content_index: 0,
                    tool_call: tool_call("call_1", "bash", serde_json::json!({})),
                },
                "toolcall_end",
            ),
            (
                AssistantMessageEvent::Done {
                    reason: SuccessReason::Stop,
                    message: assistant_message(TS),
                },
                "done",
            ),
            (
                AssistantMessageEvent::Error {
                    reason: ErrorReason::Error,
                    error: assistant_message(TS),
                },
                "error",
            ),
        ];
        for (event, wire) in cases {
            assert_eq!(event.event_type(), wire);
            assert!(serde_json::to_string(&event)
                .unwrap()
                .starts_with(&format!(r#"{{"type":"{wire}""#)));
        }
    }

    #[test]
    fn reason_wire_values_match_upstream() {
        let success = [
            (SuccessReason::Stop, "\"stop\""),
            (SuccessReason::Length, "\"length\""),
            (SuccessReason::ToolUse, "\"toolUse\""),
            (SuccessReason::Deferred, "\"deferred\""),
        ];
        let error = [
            (ErrorReason::Aborted, "\"aborted\""),
            (ErrorReason::Error, "\"error\""),
        ];
        for (reason, wire) in success {
            assert_eq!(serde_json::to_string(&reason).unwrap(), wire);
            assert_eq!(serde_json::from_str::<SuccessReason>(wire).unwrap(), reason);
        }
        for (reason, wire) in error {
            assert_eq!(serde_json::to_string(&reason).unwrap(), wire);
            assert_eq!(serde_json::from_str::<ErrorReason>(wire).unwrap(), reason);
        }
    }

    #[test]
    fn events_round_trip_wire_format_without_partial() {
        let events = vec![
            AssistantMessageEvent::Start {
                message: assistant_message(TS),
            },
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
            },
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "hi".into(),
            },
            AssistantMessageEvent::ThinkingStart { content_index: 1 },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 1,
                delta: "hmm".into(),
            },
            AssistantMessageEvent::ThinkingEnd {
                content_index: 1,
                content: "hmm".into(),
            },
            AssistantMessageEvent::ToolcallStart { content_index: 2 },
            AssistantMessageEvent::ToolcallDelta {
                content_index: 2,
                delta: r#"{"a":1}"#.into(),
            },
            AssistantMessageEvent::ToolcallEnd {
                content_index: 2,
                tool_call: tool_call("call_1", "bash", serde_json::json!({"a": 1})),
            },
            AssistantMessageEvent::Done {
                reason: SuccessReason::ToolUse,
                message: assistant_message(TS),
            },
            AssistantMessageEvent::Error {
                reason: ErrorReason::Aborted,
                error: assistant_message(TS),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            // The documented deviation: no `partial` field on any event.
            assert!(!json.contains("\"partial\""), "{json}");
            let back: AssistantMessageEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, event, "{json}");
        }
        assert_eq!(
            serde_json::to_string(&events[1]).unwrap(),
            r#"{"type":"text_start","contentIndex":0}"#
        );
        assert_eq!(
            serde_json::to_string(&events[2]).unwrap(),
            r#"{"type":"text_delta","contentIndex":0,"delta":"hi"}"#
        );
        assert_eq!(
            serde_json::to_string(&events[8]).unwrap(),
            r#"{"type":"toolcall_delta","contentIndex":2,"delta":"{\"a\":1}"}"#
        );
        // Byte-pinned toolcall_end wire shape: the embedded tool call is the
        // plain ToolCall object (no block tag), camelCase `toolCall` field.
        assert_eq!(
            serde_json::to_string(&events[9]).unwrap(),
            r#"{"type":"toolcall_end","contentIndex":2,"toolCall":{"id":"call_1","name":"bash","arguments":{"a":1}}}"#
        );
        assert_eq!(
            serde_json::to_string(&events[10]).unwrap(),
            r#"{"type":"done","reason":"toolUse","message":"#.to_string()
                + r#"{"content":[],"api":"anthropic-messages","provider":"anthropic","model":"claude-sonnet-4-5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"pending","timestamp":1758240000000}}"#
        );
        assert!(
            serde_json::from_str::<AssistantMessageEvent>(r#"{"type":"mystery"}"#).is_err(),
            "unknown event type must be rejected"
        );
    }

    #[test]
    fn start_deltas_done_builds_correct_assistant_message() {
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        let started = partial.message().unwrap();
        assert_eq!(started.api, "anthropic-messages");
        assert_eq!(started.provider, "anthropic");
        assert_eq!(started.model, "claude-sonnet-4-5");
        assert_eq!(started.timestamp, TS);
        assert!(started.content.is_empty());
        assert_eq!(started.stop_reason, StopReason::Pending);

        partial
            .apply(&AssistantMessageEvent::TextStart { content_index: 0 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello,".into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: " worl".into(),
            })
            .unwrap();
        assert_eq!(
            partial.message().unwrap().content,
            vec![text_block("Hello, worl")]
        );

        // text_end is authoritative: replaces the accumulated deltas wholesale.
        partial
            .apply(&AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "Hello, world!".into(),
            })
            .unwrap();
        assert_eq!(
            partial.message().unwrap().content,
            vec![text_block("Hello, world!")]
        );

        let mut final_message = assistant_message(TS);
        final_message.content = vec![text_block("Hello, world!")];
        final_message.stop_reason = StopReason::Stop;
        final_message.usage.output = 12;
        final_message.usage.total_tokens = 12;
        partial
            .apply(&AssistantMessageEvent::Done {
                reason: SuccessReason::Stop,
                message: final_message.clone(),
            })
            .unwrap();
        assert_eq!(partial.message(), Some(&final_message));
        assert!(partial.is_terminal());
    }

    #[test]
    fn interleaved_blocks_reconstruct_via_content_index() {
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        // Blocks start in content order but their deltas interleave.
        partial
            .apply(&AssistantMessageEvent::TextStart { content_index: 0 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingStart { content_index: 1 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ToolcallStart { content_index: 2 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Run".into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ToolcallDelta {
                content_index: 2,
                delta: r#"{"cmd":"#.into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingDelta {
                content_index: 1,
                delta: "plan".into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: " ls".into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ToolcallDelta {
                content_index: 2,
                delta: r#"ls"}"#.into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingEnd {
                content_index: 1,
                content: "planned".into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "Run ls".into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ToolcallEnd {
                content_index: 2,
                tool_call: tool_call("call_1", "bash", serde_json::json!({"cmd": "ls"})),
            })
            .unwrap();

        let message = partial.message().unwrap();
        assert_eq!(
            message.content,
            vec![
                text_block("Run ls"),
                thinking_block("planned"),
                AssistantBlock::ToolCall(tool_call(
                    "call_1",
                    "bash",
                    serde_json::json!({"cmd": "ls"})
                )),
            ]
        );

        let mut final_message = assistant_message(TS);
        final_message.content = message.content.clone();
        final_message.stop_reason = StopReason::ToolUse;
        partial
            .apply(&AssistantMessageEvent::Done {
                reason: SuccessReason::ToolUse,
                message: final_message.clone(),
            })
            .unwrap();
        assert_eq!(partial.message(), Some(&final_message));
    }

    #[test]
    fn toolcall_delta_accumulates_json_then_toolcall_end_replaces() {
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ToolcallStart { content_index: 0 })
            .unwrap();
        // toolcall_start appends an empty placeholder tool call.
        assert_eq!(
            partial.message().unwrap().content,
            vec![AssistantBlock::ToolCall(tool_call(
                "",
                "",
                serde_json::json!({})
            ))]
        );

        partial
            .apply(&AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: r#"{"command":"#.into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: r#""ls","timeout":5}"#.into(),
            })
            .unwrap();
        // Frame-reducer rule: raw JSON fragments accumulate in block state and
        // parsing is deferred; arguments stay the placeholder until the
        // authoritative end.
        assert_eq!(
            partial.message().unwrap().content,
            vec![AssistantBlock::ToolCall(tool_call(
                "",
                "",
                serde_json::json!({})
            ))]
        );

        partial
            .apply(&AssistantMessageEvent::ToolcallEnd {
                content_index: 0,
                tool_call: tool_call(
                    "call_1",
                    "bash",
                    serde_json::json!({"command": "ls", "timeout": 5}),
                ),
            })
            .unwrap();
        assert_eq!(
            partial.message().unwrap().content,
            vec![AssistantBlock::ToolCall(tool_call(
                "call_1",
                "bash",
                serde_json::json!({"command": "ls", "timeout": 5})
            ))]
        );
    }

    #[test]
    fn error_after_start_keeps_partial_content_and_sets_stop_reason() {
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextStart { content_index: 0 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "partial answer".into(),
            })
            .unwrap();

        let mut failed = assistant_message(TS);
        failed.stop_reason = StopReason::Error;
        failed.error_message = Some("provider exploded".into());
        partial
            .apply(&AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: failed,
            })
            .unwrap();

        let message = partial.message().unwrap();
        assert_eq!(message.content, vec![text_block("partial answer")]);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.error_message.as_deref(), Some("provider exploded"));
        assert!(partial.is_terminal());

        // The aborted reason maps to StopReason::Aborted the same way.
        let mut aborted_partial = PartialAssistant::new();
        aborted_partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        aborted_partial
            .apply(&AssistantMessageEvent::Error {
                reason: ErrorReason::Aborted,
                error: assistant_message(TS),
            })
            .unwrap();
        assert_eq!(
            aborted_partial.message().unwrap().stop_reason,
            StopReason::Aborted
        );
    }

    #[test]
    fn error_before_start_is_allowed_and_terminates() {
        // Pre-generation failure: the stream contains only `error`
        // (types.ts:633-634); the event's error message becomes the message.
        let mut partial = PartialAssistant::new();
        let mut failed = assistant_message(TS);
        failed.error_message = Some("missing auth".into());
        partial
            .apply(&AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: failed,
            })
            .unwrap();
        let message = partial.message().unwrap();
        assert!(message.content.is_empty());
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.error_message.as_deref(), Some("missing auth"));
        assert!(partial.is_terminal());

        let start_rejected = partial.apply(&AssistantMessageEvent::Start {
            message: assistant_message(TS),
        });
        assert!(start_rejected.is_err());
    }

    #[test]
    fn events_before_start_are_rejected() {
        let mut partial = PartialAssistant::new();
        let before_start = vec![
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "x".into(),
            },
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "x".into(),
            },
            AssistantMessageEvent::ThinkingStart { content_index: 0 },
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "x".into(),
            },
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: "x".into(),
            },
            AssistantMessageEvent::ToolcallStart { content_index: 0 },
            AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "x".into(),
            },
            AssistantMessageEvent::ToolcallEnd {
                content_index: 0,
                tool_call: tool_call("call_1", "bash", serde_json::json!({})),
            },
            AssistantMessageEvent::Done {
                reason: SuccessReason::Stop,
                message: assistant_message(TS),
            },
        ];
        for event in &before_start {
            let error = partial
                .apply(event)
                .expect_err("must be rejected before start");
            assert!(error.contains("appears before start"), "{error}");
        }
        // Rejected events leave the accumulator unchanged, so the stream can
        // still start cleanly afterwards.
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        assert!(partial.message().is_some());
    }

    #[test]
    fn duplicate_start_events_are_rejected() {
        let mut partial = PartialAssistant::new();
        let start = AssistantMessageEvent::Start {
            message: assistant_message(TS),
        };
        partial.apply(&start).unwrap();
        let error = partial.apply(&start).expect_err("duplicate start");
        assert!(error.contains("more than one start event"), "{error}");
    }

    #[test]
    fn events_after_terminal_are_rejected() {
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::Done {
                reason: SuccessReason::Stop,
                message: assistant_message(TS),
            })
            .unwrap();
        let error = partial
            .apply(&AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "x".into(),
            })
            .expect_err("event after done");
        assert!(error.contains("follows a terminal event"), "{error}");

        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: assistant_message(TS),
            })
            .unwrap();
        let error = partial
            .apply(&AssistantMessageEvent::Done {
                reason: SuccessReason::Stop,
                message: assistant_message(TS),
            })
            .expect_err("event after error");
        assert!(error.contains("follows a terminal event"), "{error}");
    }

    #[test]
    fn duplicate_block_starts_and_gaps_are_rejected() {
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::TextStart { content_index: 0 })
            .unwrap();

        let duplicate = partial
            .apply(&AssistantMessageEvent::ThinkingStart { content_index: 0 })
            .expect_err("duplicate block start");
        assert!(duplicate.contains("already exists"), "{duplicate}");

        let gap = partial
            .apply(&AssistantMessageEvent::TextStart { content_index: 2 })
            .expect_err("index gap");
        assert!(gap.contains("would leave a gap"), "{gap}");
    }

    #[test]
    fn block_kind_mismatches_and_ended_blocks_are_rejected() {
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingStart { content_index: 0 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "hmm".into(),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: "hmm".into(),
            })
            .unwrap();

        // Delta with the wrong block kind.
        let mismatch = partial
            .apply(&AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "x".into(),
            })
            .expect_err("text delta on thinking block");
        assert!(
            mismatch.contains("expected text block at index 0, found thinking"),
            "{mismatch}"
        );

        // Delta after the block ended.
        let after_end = partial
            .apply(&AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "x".into(),
            })
            .expect_err("delta after thinking_end");
        assert!(
            after_end.contains("follows the end of block"),
            "{after_end}"
        );

        // toolcall_delta against the thinking block.
        let tool_mismatch = partial
            .apply(&AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "x".into(),
            })
            .expect_err("toolcall delta on thinking block");
        assert!(
            tool_mismatch.contains("expected toolCall block at index 0, found thinking"),
            "{tool_mismatch}"
        );

        // A started index can never be reused, even after its block ended.
        let reuse = partial
            .apply(&AssistantMessageEvent::ToolcallStart { content_index: 0 })
            .expect_err("reuse ended index");
        assert!(reuse.contains("already exists"), "{reuse}");

        // toolcall_end after toolcall_end.
        partial
            .apply(&AssistantMessageEvent::ToolcallStart { content_index: 1 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ToolcallEnd {
                content_index: 1,
                tool_call: tool_call("call_1", "bash", serde_json::json!({})),
            })
            .unwrap();
        let double_end = partial
            .apply(&AssistantMessageEvent::ToolcallEnd {
                content_index: 1,
                tool_call: tool_call("call_1", "bash", serde_json::json!({})),
            })
            .expect_err("toolcall_end twice");
        assert!(
            double_end.contains("follows the end of block"),
            "{double_end}"
        );
    }

    #[test]
    fn start_normalizes_to_metadata_with_empty_content() {
        // Port of the frame encoder's cloneStartMessage: content is emptied,
        // stopReason forced to "pending", settlement fields dropped.
        let mut initial = assistant_message(TS);
        initial.content = vec![text_block("stale")];
        initial.stop_reason = StopReason::Stop;
        initial.error_message = Some("stale".into());
        initial.raw_stop_reason = Some("stop".into());
        initial.end_turn = Some(false);
        initial.response_id = Some("msg_01ABC".into());

        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start { message: initial })
            .unwrap();
        let message = partial.message().unwrap();
        assert!(message.content.is_empty());
        assert_eq!(message.stop_reason, StopReason::Pending);
        assert_eq!(message.error_message, None);
        assert_eq!(message.raw_stop_reason, None);
        assert_eq!(message.end_turn, None);
        // Metadata survives.
        assert_eq!(message.response_id.as_deref(), Some("msg_01ABC"));
        assert_eq!(message.timestamp, TS);
    }

    #[test]
    fn redacted_thinking_reconstructs_from_thinking_end_without_deltas() {
        // Upstream redacted thinking is complete at thinking_start and emits
        // no deltas; in the port its content arrives at thinking_end.
        let mut partial = PartialAssistant::new();
        partial
            .apply(&AssistantMessageEvent::Start {
                message: assistant_message(TS),
            })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingStart { content_index: 0 })
            .unwrap();
        partial
            .apply(&AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: "[Reasoning redacted]".into(),
            })
            .unwrap();
        assert_eq!(
            partial.message().unwrap().content,
            vec![thinking_block("[Reasoning redacted]")]
        );
    }
}
