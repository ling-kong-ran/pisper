//! Port of `packages/ai/src/utils/assistant-message-frame.ts` (490 lines):
//! the compact, replayable assistant-message progress vocabulary
//! ([`AssistantMessageFrame`]), the stream encoder
//! ([`AssistantMessageFrameEncoder`]), and the replay reducer
//! ([`reduce_assistant_message_frames`]). Terminal settlement is excluded by
//! design and persisted separately.
//!
//! Adaptation to this port's event protocol (see
//! `ai::types::events` module docs: events carry no live `partial`): the
//! encoder needs no `eventBlock` reads of a shared partial — text/thinking
//! blocks start empty (`coveredChars` 0), tool calls start as an empty
//! placeholder (always caught up, so the upstream start-snapshot comparison
//! path cannot trigger and is kept only structurally), `start` clones
//! `event.message`, and the `*_end` signature passthrough fields are absent
//! from the Rust events (frames carry `None`).

use serde::{Deserialize, Serialize};

use crate::ai::api::openai_completions::stream::parse_streaming_json;
use crate::ai::types::content::{TextContent, ThinkingContent, ToolCall};
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::message::{AssistantBlock, AssistantMessage};
use crate::ai::types::primitives::StopReason;

/// Upstream `AssistantMessageFrame` (`assistant-message-frame.ts:11-38`).
#[allow(clippy::large_enum_variant)] // mirrors the upstream object union sizes
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AssistantMessageFrame {
    Start {
        partial: AssistantMessage,
    },
    TextStart {
        content_index: usize,
        content: TextContent,
    },
    TextDelta {
        content_index: usize,
        delta: String,
    },
    TextEnd {
        content_index: usize,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    ThinkingStart {
        content_index: usize,
        content: ThinkingContent,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        content_index: usize,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        redacted: Option<bool>,
    },
    ToolcallStart {
        content_index: usize,
        tool_call: ToolCall,
    },
    ToolcallCheckpoint {
        content_index: usize,
        json: String,
    },
    ToolcallDelta {
        content_index: usize,
        delta: String,
    },
    ToolcallEnd {
        content_index: usize,
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
    },
}

impl AssistantMessageFrame {
    /// Upstream `frame.type` reads used by the reducer's before-start guard.
    fn kind(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::TextStart { .. } => "text_start",
            Self::TextDelta { .. } => "text_delta",
            Self::TextEnd { .. } => "text_end",
            Self::ThinkingStart { .. } => "thinking_start",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ThinkingEnd { .. } => "thinking_end",
            Self::ToolcallStart { .. } => "toolcall_start",
            Self::ToolcallCheckpoint { .. } => "toolcall_checkpoint",
            Self::ToolcallDelta { .. } => "toolcall_delta",
            Self::ToolcallEnd { .. } => "toolcall_end",
        }
    }
}

/// Upstream `cloneStartMessage` (`assistant-message-frame.ts:81-97`): the
/// start frame's partial — metadata and usage only, empty content, pending
/// stop reason.
fn clone_start_message(message: &AssistantMessage) -> AssistantMessage {
    let mut partial = message.clone();
    partial.content = Vec::new();
    partial.stop_reason = StopReason::Pending;
    partial.error_message = None;
    partial.deferred = None;
    partial
}

#[derive(Debug, Clone)]
enum EncoderBlockState {
    Text {
        covered_chars: usize,
        delta_chars: usize,
    },
    Thinking {
        covered_chars: usize,
        delta_chars: usize,
    },
    ToolCall {
        caught_up: bool,
        /// Upstream accumulates a catch-up JSON when the start snapshot was
        /// ahead of the delta stream; unreachable in this port (tool calls
        /// always start caught up) and retained for structural parity.
        #[allow(dead_code)]
        catchup_json: String,
    },
}

/// Upstream `AssistantMessageFrameEncoder` (`assistant-message-frame.ts:142-330`):
/// encodes one assistant stream into compact frames.
#[derive(Default)]
pub struct AssistantMessageFrameEncoder {
    started: bool,
    terminal: bool,
    blocks: std::collections::HashMap<usize, EncoderBlockState>,
}

impl AssistantMessageFrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upstream `encode` (`assistant-message-frame.ts:145-327`): fold one
    /// event into at most one frame; `Ok(None)` for events that produce no
    /// frame (deltas fully covered, empty deltas, done/error).
    pub fn encode(
        &mut self,
        event: &AssistantMessageEvent,
    ) -> anyhow::Result<Option<AssistantMessageFrame>> {
        if self.terminal {
            anyhow::bail!(
                "Assistant message event {} follows a terminal event",
                event.event_type()
            );
        }
        match event {
            AssistantMessageEvent::Start { message } => {
                if self.started {
                    anyhow::bail!("Assistant message stream contains more than one start event");
                }
                self.started = true;
                return Ok(Some(AssistantMessageFrame::Start {
                    partial: clone_start_message(message),
                }));
            }
            AssistantMessageEvent::Done { .. } => {
                if !self.started {
                    anyhow::bail!("Assistant message done event appears before start");
                }
                self.terminal = true;
                return Ok(None);
            }
            AssistantMessageEvent::Error { .. } => {
                self.terminal = true;
                return Ok(None);
            }
            _ => {}
        }
        if !self.started {
            anyhow::bail!(
                "Assistant message {} event appears before start",
                event.event_type()
            );
        }

        match event {
            AssistantMessageEvent::TextStart { content_index } => {
                self.start_block(
                    *content_index,
                    EncoderBlockState::Text {
                        covered_chars: 0,
                        delta_chars: 0,
                    },
                )?;
                Ok(Some(AssistantMessageFrame::TextStart {
                    content_index: *content_index,
                    content: TextContent {
                        text: String::new(),
                        text_signature: None,
                    },
                }))
            }
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => Ok(self.encode_text_delta(*content_index, delta, Kind::Text)?),
            AssistantMessageEvent::TextEnd {
                content_index,
                content,
            } => {
                self.end_block(*content_index, Kind::Text)?;
                Ok(Some(AssistantMessageFrame::TextEnd {
                    content_index: *content_index,
                    content: content.clone(),
                    text_signature: None,
                }))
            }
            AssistantMessageEvent::ThinkingStart { content_index } => {
                self.start_block(
                    *content_index,
                    EncoderBlockState::Thinking {
                        covered_chars: 0,
                        delta_chars: 0,
                    },
                )?;
                Ok(Some(AssistantMessageFrame::ThinkingStart {
                    content_index: *content_index,
                    content: ThinkingContent {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: None,
                    },
                }))
            }
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => Ok(self.encode_text_delta(*content_index, delta, Kind::Thinking)?),
            AssistantMessageEvent::ThinkingEnd {
                content_index,
                content,
            } => {
                self.end_block(*content_index, Kind::Thinking)?;
                Ok(Some(AssistantMessageFrame::ThinkingEnd {
                    content_index: *content_index,
                    content: content.clone(),
                    thinking_signature: None,
                    redacted: None,
                }))
            }
            AssistantMessageEvent::ToolcallStart { content_index } => {
                // Upstream snapshots the live arguments; the Rust protocol
                // starts with an empty placeholder, so the encoder is always
                // caught up and the snapshot comparison path is structural.
                self.start_block(
                    *content_index,
                    EncoderBlockState::ToolCall {
                        caught_up: true,
                        catchup_json: String::new(),
                    },
                )?;
                Ok(Some(AssistantMessageFrame::ToolcallStart {
                    content_index: *content_index,
                    tool_call: ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: serde_json::Value::Object(Default::default()),
                        thought_signature: None,
                        namespace: None,
                    },
                }))
            }
            AssistantMessageEvent::ToolcallDelta {
                content_index,
                delta,
            } => {
                let EncoderBlockState::ToolCall { caught_up, .. } =
                    self.block(*content_index, Kind::ToolCall)?
                else {
                    anyhow::bail!("Unreachable tool-call encoder state");
                };
                if *caught_up {
                    Ok(if delta.is_empty() {
                        None
                    } else {
                        Some(AssistantMessageFrame::ToolcallDelta {
                            content_index: *content_index,
                            delta: delta.clone(),
                        })
                    })
                } else {
                    // Unreachable in this port (tool calls always start
                    // caught up); kept for upstream structural parity.
                    anyhow::bail!("Unreachable tool-call encoder state");
                }
            }
            AssistantMessageEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                self.end_block(*content_index, Kind::ToolCall)?;
                Ok(Some(AssistantMessageFrame::ToolcallEnd {
                    content_index: *content_index,
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: tool_call.arguments.clone(),
                    thought_signature: tool_call.thought_signature.clone(),
                    namespace: tool_call.namespace.clone(),
                }))
            }
            AssistantMessageEvent::Start { .. }
            | AssistantMessageEvent::Done { .. }
            | AssistantMessageEvent::Error { .. } => unreachable!(),
        }
    }

    fn start_block(
        &mut self,
        content_index: usize,
        state: EncoderBlockState,
    ) -> anyhow::Result<()> {
        if self.blocks.insert(content_index, state).is_some() {
            anyhow::bail!("Assistant message block {content_index} starts more than once");
        }
        Ok(())
    }

    fn block(
        &mut self,
        content_index: usize,
        kind: Kind,
    ) -> anyhow::Result<&mut EncoderBlockState> {
        let state = self.blocks.get_mut(&content_index).ok_or_else(|| {
            anyhow::anyhow!("Assistant message {kind} block {content_index} has not started")
        })?;
        if state.kind() != kind {
            anyhow::bail!(
                "Assistant message block {content_index} is {}, not {kind}",
                state.kind()
            );
        }
        Ok(state)
    }

    fn end_block(&mut self, content_index: usize, kind: Kind) -> anyhow::Result<()> {
        self.block(content_index, kind)?;
        self.blocks.remove(&content_index);
        Ok(())
    }

    fn encode_text_delta(
        &mut self,
        content_index: usize,
        delta: &str,
        kind: Kind,
    ) -> anyhow::Result<Option<AssistantMessageFrame>> {
        let state = self.block(content_index, kind)?;
        let (covered_chars, delta_start) = match state {
            EncoderBlockState::Text {
                covered_chars,
                delta_chars,
            }
            | EncoderBlockState::Thinking {
                covered_chars,
                delta_chars,
            } => (*covered_chars, *delta_chars),
            EncoderBlockState::ToolCall { .. } => anyhow::bail!("Unreachable text encoder state"),
        };
        if let Some(state) = self.blocks.get_mut(&content_index) {
            match state {
                EncoderBlockState::Text { delta_chars, .. }
                | EncoderBlockState::Thinking { delta_chars, .. } => {
                    *delta_chars += delta.chars().count();
                }
                EncoderBlockState::ToolCall { .. } => {}
            }
        }
        let covered = covered_chars.saturating_sub(delta_start);
        let uncovered: String = delta.chars().skip(covered).collect();
        if covered >= delta.chars().count() {
            return Ok(None);
        }
        Ok(Some(match kind {
            Kind::Text => AssistantMessageFrame::TextDelta {
                content_index,
                delta: uncovered,
            },
            Kind::Thinking => AssistantMessageFrame::ThinkingDelta {
                content_index,
                delta: uncovered,
            },
            Kind::ToolCall => unreachable!("handled above"),
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Thinking,
    ToolCall,
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Kind::Text => "text",
            Kind::Thinking => "thinking",
            Kind::ToolCall => "toolCall",
        })
    }
}

impl EncoderBlockState {
    fn kind(&self) -> Kind {
        match self {
            EncoderBlockState::Text { .. } => Kind::Text,
            EncoderBlockState::Thinking { .. } => Kind::Thinking,
            EncoderBlockState::ToolCall { .. } => Kind::ToolCall,
        }
    }
}

#[derive(Debug, Clone)]
enum ReducerBlockState {
    Text { ended: bool },
    Thinking { ended: bool },
    ToolCall { ended: bool, json: String },
}

impl ReducerBlockState {
    fn ended(&self) -> bool {
        match self {
            ReducerBlockState::Text { ended } | ReducerBlockState::Thinking { ended } => *ended,
            ReducerBlockState::ToolCall { ended, .. } => *ended,
        }
    }

    fn set_ended(&mut self) {
        match self {
            ReducerBlockState::Text { ended } | ReducerBlockState::Thinking { ended } => {
                *ended = true
            }
            ReducerBlockState::ToolCall { ended, .. } => *ended = true,
        }
    }
}

/// Upstream `reduceAssistantMessageFrames` (`assistant-message-frame.ts:363-489`):
/// replay compact frames without mutating them. Returns `Ok(None)` when the
/// frames contain no start frame; frames before the start frame are skipped
/// (the first such kind is remembered for the upstream invariant error).
pub fn reduce_assistant_message_frames(
    frames: &[AssistantMessageFrame],
) -> anyhow::Result<Option<AssistantMessage>> {
    let mut message: Option<AssistantMessage> = None;
    let mut frame_before_start: Option<&'static str> = None;
    let mut states: std::collections::HashMap<usize, ReducerBlockState> =
        std::collections::HashMap::new();

    for frame in frames {
        if let AssistantMessageFrame::Start { partial } = frame {
            if message.is_some() {
                anyhow::bail!(
                    "Assistant message frame sequence contains more than one start frame"
                );
            }
            if let Some(kind) = frame_before_start {
                anyhow::bail!("{kind} frame appears before the start frame");
            }
            message = Some(partial.clone());
            continue;
        }
        let Some(current) = message.as_mut() else {
            if frame_before_start.is_none() {
                frame_before_start = Some(frame.kind());
            }
            continue;
        };

        match frame {
            AssistantMessageFrame::Start { .. } => unreachable!("handled above"),
            AssistantMessageFrame::TextStart {
                content_index,
                content,
            } => {
                append_block(
                    current,
                    *content_index,
                    AssistantBlock::Text(content.clone()),
                )?;
                states.insert(*content_index, ReducerBlockState::Text { ended: false });
            }
            AssistantMessageFrame::TextDelta {
                content_index,
                delta,
            } => {
                let block =
                    active_block(current, &states, *content_index, Kind::Text, frame.kind())?;
                if let AssistantBlock::Text(text) = block {
                    text.text.push_str(delta);
                }
            }
            AssistantMessageFrame::TextEnd {
                content_index,
                content,
                text_signature,
            } => {
                let block =
                    active_block(current, &states, *content_index, Kind::Text, frame.kind())?;
                if let AssistantBlock::Text(text) = block {
                    text.text = content.clone();
                    text.text_signature = text_signature.clone();
                }
                if let Some(state) = states.get_mut(content_index) {
                    state.set_ended();
                }
            }
            AssistantMessageFrame::ThinkingStart {
                content_index,
                content,
            } => {
                append_block(
                    current,
                    *content_index,
                    AssistantBlock::Thinking(content.clone()),
                )?;
                states.insert(*content_index, ReducerBlockState::Thinking { ended: false });
            }
            AssistantMessageFrame::ThinkingDelta {
                content_index,
                delta,
            } => {
                let block = active_block(
                    current,
                    &states,
                    *content_index,
                    Kind::Thinking,
                    frame.kind(),
                )?;
                if let AssistantBlock::Thinking(thinking) = block {
                    thinking.thinking.push_str(delta);
                }
            }
            AssistantMessageFrame::ThinkingEnd {
                content_index,
                content,
                thinking_signature,
                redacted,
            } => {
                let block = active_block(
                    current,
                    &states,
                    *content_index,
                    Kind::Thinking,
                    frame.kind(),
                )?;
                if let AssistantBlock::Thinking(thinking) = block {
                    thinking.thinking = content.clone();
                    thinking.thinking_signature = thinking_signature.clone();
                    thinking.redacted = *redacted;
                }
                if let Some(state) = states.get_mut(content_index) {
                    state.set_ended();
                }
            }
            AssistantMessageFrame::ToolcallStart {
                content_index,
                tool_call,
            } => {
                append_block(
                    current,
                    *content_index,
                    AssistantBlock::ToolCall(tool_call.clone()),
                )?;
                states.insert(
                    *content_index,
                    ReducerBlockState::ToolCall {
                        ended: false,
                        json: String::new(),
                    },
                );
            }
            AssistantMessageFrame::ToolcallCheckpoint {
                content_index,
                json,
            } => {
                let block = active_block(
                    current,
                    &states,
                    *content_index,
                    Kind::ToolCall,
                    frame.kind(),
                )?;
                if let AssistantBlock::ToolCall(call) = block {
                    call.arguments = parse_streaming_json(json);
                }
                if let Some(ReducerBlockState::ToolCall {
                    json: state_json, ..
                }) = states.get_mut(content_index)
                {
                    *state_json = json.clone();
                }
            }
            AssistantMessageFrame::ToolcallDelta {
                content_index,
                delta,
            } => {
                let block = active_block(
                    current,
                    &states,
                    *content_index,
                    Kind::ToolCall,
                    frame.kind(),
                )?;
                if let AssistantBlock::ToolCall(call) = block {
                    let _ = call;
                }
                if let Some(ReducerBlockState::ToolCall { json, .. }) =
                    states.get_mut(content_index)
                {
                    json.push_str(delta);
                }
            }
            AssistantMessageFrame::ToolcallEnd {
                content_index,
                id,
                name,
                arguments,
                thought_signature,
                namespace,
            } => {
                let block = active_block(
                    current,
                    &states,
                    *content_index,
                    Kind::ToolCall,
                    frame.kind(),
                )?;
                if let AssistantBlock::ToolCall(call) = block {
                    call.id = id.clone();
                    call.name = name.clone();
                    call.arguments = arguments.clone();
                    call.thought_signature = thought_signature.clone();
                    call.namespace = namespace.clone();
                }
                if let Some(state) = states.get_mut(content_index) {
                    state.set_ended();
                }
            }
        }
    }

    let Some(mut message) = message else {
        return Ok(None);
    };
    // Settle any tool call left with buffered JSON but no end frame.
    for (content_index, state) in &states {
        let ReducerBlockState::ToolCall { ended, json } = state else {
            continue;
        };
        if *ended || json.is_empty() {
            continue;
        }
        let block = message
            .content
            .get_mut(*content_index)
            .ok_or_else(|| anyhow::anyhow!("Unreachable tool-call frame state"))?;
        if let AssistantBlock::ToolCall(call) = block {
            call.arguments = parse_streaming_json(json);
        }
    }
    Ok(Some(message))
}

fn append_block(
    message: &mut AssistantMessage,
    content_index: usize,
    block: AssistantBlock,
) -> anyhow::Result<()> {
    if content_index != message.content.len() {
        let reason = if content_index < message.content.len() {
            "already exists"
        } else {
            "would leave a gap"
        };
        anyhow::bail!("Cannot start assistant message block at index {content_index}: {reason}");
    }
    message.content.push(block);
    Ok(())
}

fn active_block<'a>(
    message: &'a mut AssistantMessage,
    states: &std::collections::HashMap<usize, ReducerBlockState>,
    content_index: usize,
    expected_kind: Kind,
    frame_type: &str,
) -> anyhow::Result<&'a mut AssistantBlock> {
    let state = states.get(&content_index).ok_or_else(|| {
        anyhow::anyhow!("{frame_type} frame has no started block at index {content_index}")
    })?;
    if state.kind_label() != expected_kind {
        anyhow::bail!(
            "{frame_type} frame expected {} block at index {content_index}, found {}",
            expected_kind,
            state.kind_label()
        );
    }
    if state.ended() {
        anyhow::bail!("{frame_type} frame follows the end of block at index {content_index}");
    }
    let block = message.content.get_mut(content_index).ok_or_else(|| {
        anyhow::anyhow!("{frame_type} frame has no started block at index {content_index}")
    })?;
    let block_kind_matches = matches!(
        (&*block, expected_kind),
        (AssistantBlock::Text(_), Kind::Text)
            | (AssistantBlock::Thinking(_), Kind::Thinking)
            | (AssistantBlock::ToolCall(_), Kind::ToolCall)
    );
    if !block_kind_matches {
        anyhow::bail!(
            "{frame_type} frame expected {expected_kind} block at index {content_index}, found mismatched block"
        );
    }
    Ok(block)
}

impl ReducerBlockState {
    fn kind_label(&self) -> Kind {
        match self {
            ReducerBlockState::Text { .. } => Kind::Text,
            ReducerBlockState::Thinking { .. } => Kind::Thinking,
            ReducerBlockState::ToolCall { .. } => Kind::ToolCall,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_event() -> AssistantMessageEvent {
        serde_json::from_value(serde_json::json!({
            "role": "assistant", "content": [], "api": "faux", "provider": "faux",
            "model": "faux-1", "stopReason": "pending",
            "usage": {"input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 1, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                "cacheWrite": 0, "total": 0}},
            "timestamp": 5
        }))
        .map(|message: AssistantMessage| AssistantMessageEvent::Start { message })
        .unwrap()
    }

    fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
            thought_signature: None,
            namespace: None,
        }
    }

    #[test]
    fn encoder_and_reducer_round_trip_text() {
        let mut encoder = AssistantMessageFrameEncoder::new();
        let mut frames = Vec::new();
        for event in [
            start_event(),
            serde_json::from_value(serde_json::json!({"type": "text_start", "contentIndex": 0}))
                .unwrap(),
            serde_json::from_value(
                serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "he"}),
            )
            .unwrap(),
            serde_json::from_value(
                serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "llo"}),
            )
            .unwrap(),
            serde_json::from_value(
                serde_json::json!({"type": "text_end", "contentIndex": 0, "content": "hello"}),
            )
            .unwrap(),
        ] {
            if let Some(frame) = encoder.encode(&event).unwrap() {
                frames.push(frame);
            }
        }
        assert_eq!(frames.len(), 5, "start + 4 block frames");
        let rebuilt = reduce_assistant_message_frames(&frames).unwrap().unwrap();
        assert_eq!(rebuilt.content.len(), 1);
        assert!(matches!(&rebuilt.content[0], AssistantBlock::Text(t) if t.text == "hello"));
        assert_eq!(rebuilt.stop_reason, StopReason::Pending);
        // done terminates the encoder with no frame.
        let done: AssistantMessageEvent = serde_json::from_value(
            serde_json::json!({"type": "done", "reason": "stop", "message": {
                "role": "assistant", "content": [{"type": "text", "text": "hello"}],
                "api": "faux", "provider": "faux", "model": "faux-1",
                "stopReason": "stop",
                "usage": {"input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 1, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                    "cacheWrite": 0, "total": 0}},
                "timestamp": 6
            }}),
        )
        .unwrap();
        assert!(encoder.encode(&done).unwrap().is_none());
        let error = encoder.encode(&start_event()).unwrap_err();
        assert!(error.to_string().contains("follows a terminal event"));
    }

    #[test]
    fn encoder_toolcall_deltas_pass_through_and_reducer_replays() {
        let mut encoder = AssistantMessageFrameEncoder::new();
        let mut frames = Vec::new();
        for event in [
            start_event(),
            serde_json::from_value(serde_json::json!({"type": "toolcall_start", "contentIndex": 0})).unwrap(),
            serde_json::from_value(
                serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "{\"cmd\":"}),
            )
            .unwrap(),
            serde_json::from_value(
                serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": " \"ls\"}"}),
            )
            .unwrap(),
            serde_json::from_value(
                serde_json::json!({"type": "toolcall_end", "contentIndex": 0,
                    "toolCall": {"id": "c1", "name": "bash", "arguments": {"cmd": "ls"}}}),
            )
            .unwrap(),
        ] {
            if let Some(frame) = encoder.encode(&event).unwrap() {
                frames.push(frame);
            }
        }
        assert_eq!(frames.len(), 5);
        assert!(matches!(
            frames[2],
            AssistantMessageFrame::ToolcallDelta { .. }
        ));
        let rebuilt = reduce_assistant_message_frames(&frames).unwrap().unwrap();
        assert!(matches!(
            &rebuilt.content[0],
            AssistantBlock::ToolCall(call) if call.id == "c1" && call.arguments["cmd"] == "ls"
        ));
    }

    #[test]
    fn encoder_invariants_follow_upstream() {
        let mut encoder = AssistantMessageFrameEncoder::new();
        let delta: AssistantMessageEvent = serde_json::from_value(
            serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "x"}),
        )
        .unwrap();
        let error = encoder.encode(&delta).unwrap_err();
        assert!(error.to_string().contains("appears before start"));
        // The rejected pre-start event does not poison the encoder: start
        // still begins the stream (upstream throws without latching state).
        assert!(encoder.encode(&start_event()).unwrap().is_some());
    }

    #[test]
    fn reducer_settles_buffered_tool_json_without_end() {
        let frames = vec![
            AssistantMessageFrame::Start {
                partial: serde_json::from_value(serde_json::json!({
                    "role": "assistant", "content": [], "api": "faux", "provider": "faux",
                    "model": "faux-1", "stopReason": "pending",
                    "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                        "totalTokens": 0, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                        "cacheWrite": 0, "total": 0}},
                    "timestamp": 5
                }))
                .unwrap(),
            },
            AssistantMessageFrame::ToolcallStart {
                content_index: 0,
                tool_call: tool_call("", "", serde_json::Value::Object(Default::default())),
            },
            AssistantMessageFrame::ToolcallCheckpoint {
                content_index: 0,
                json: "{\"cmd\":\"ls\"".to_string(),
            },
        ];
        let rebuilt = reduce_assistant_message_frames(&frames).unwrap().unwrap();
        assert!(matches!(
            &rebuilt.content[0],
            AssistantBlock::ToolCall(call) if call.arguments["cmd"] == "ls"
        ));
    }

    #[test]
    fn reducer_invariants_follow_upstream() {
        // Two start frames are rejected.
        let start = AssistantMessageFrame::Start {
            partial: serde_json::from_value(serde_json::json!({
                "role": "assistant", "content": [], "api": "a", "provider": "p",
                "model": "m", "stopReason": "pending",
                "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 0, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                    "cacheWrite": 0, "total": 0}},
                "timestamp": 1
            }))
            .unwrap(),
        };
        let error = reduce_assistant_message_frames(&[start.clone(), start]).unwrap_err();
        assert!(error.to_string().contains("more than one start frame"));
        // A delta before any start frame errors with the recorded prefix kind.
        let delta = AssistantMessageFrame::TextDelta {
            content_index: 0,
            delta: "x".to_string(),
        };
        let error = reduce_assistant_message_frames(&[
            delta,
            AssistantMessageFrame::Start {
                partial: serde_json::from_value(serde_json::json!({
                    "role": "assistant", "content": [], "api": "a", "provider": "p",
                    "model": "m", "stopReason": "pending",
                    "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                        "totalTokens": 0, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                        "cacheWrite": 0, "total": 0}},
                    "timestamp": 1
                }))
                .unwrap(),
            },
        ])
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("text_delta frame appears before the start frame"));
    }
}
