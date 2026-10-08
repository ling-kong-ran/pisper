//! Port of `ai/src/utils/estimate.ts`: character-based token estimation for
//! model context. Consumed by the durable harness's compaction range
//! selection and context-size estimates (`harness/compaction.ts`), which
//! import `calculateContextTokens` and `estimateMessageTokens`; the rest of
//! the upstream public surface is ported alongside it so the module mirrors
//! `estimate.ts` one-to-one.
//!
//! Divergences (structural, disclosed): `safeJsonStringify` catches nothing
//! in the port — `serde_json` serializes [`Value`] infallibly, so the
//! `[unserializable]` fallback is unreachable; ceiling division uses integer
//! arithmetic instead of `Math.ceil`; and `Number.NEGATIVE_INFINITY` in
//! `getLastAssistantUsageInfo` becomes [`i64::MIN`], which every real
//! millisecond timestamp exceeds exactly as upstream's `-Infinity` does.

use serde_json::Value;

use crate::ai::transcript::get_system_message_text;
use crate::ai::types::message::{AssistantBlock, Message, StringOrBlocks, TextOrImageBlock};
use crate::ai::types::primitives::{StopReason, Usage};

/// Upstream `CHARS_PER_TOKEN` (estimate.ts:12).
const CHARS_PER_TOKEN: usize = 4;
/// Upstream `ESTIMATED_IMAGE_CHARS` (estimate.ts:13).
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// Upstream `ContextUsageEstimate` (estimate.ts:6-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsageEstimate {
    /// Estimated total context tokens.
    pub tokens: u64,
    /// Tokens reported by the most recent applicable assistant usage block.
    pub usage_tokens: u64,
    /// Estimated tokens after the most recent applicable assistant usage block.
    pub trailing_tokens: u64,
    /// Index of the applicable message that provided usage, or `None` when none exists.
    pub last_usage_index: Option<usize>,
}

/// Upstream `calculateContextTokens` (estimate.ts:15): the reported context
/// size of a response, preferring the provider's `totalTokens`.
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens != 0 {
        return usage.total_tokens;
    }
    usage.input + usage.output + usage.cache_read + usage.cache_write
}

/// Upstream `estimateTextTokens` (estimate.ts:39).
pub fn estimate_text_tokens(text: &str) -> u64 {
    ceil_div_4(text.len())
}

/// Upstream `estimateTextAndImageContentChars` (estimate.ts:29-35).
fn text_and_image_content_chars(content: &StringOrBlocks) -> usize {
    match content {
        StringOrBlocks::Text(text) => text.len(),
        StringOrBlocks::Blocks(blocks) => text_or_image_chars(blocks),
    }
}

/// The block-array half of upstream `estimateTextAndImageContentChars`.
fn text_or_image_chars(blocks: &[TextOrImageBlock]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            TextOrImageBlock::Text(text) => text.text.len(),
            TextOrImageBlock::Image(_) => ESTIMATED_IMAGE_CHARS,
        })
        .sum()
}

/// Upstream `estimateTextAndImageContentTokens` (estimate.ts:43).
pub fn estimate_text_and_image_content_tokens(content: &StringOrBlocks) -> u64 {
    ceil_div_4(text_and_image_content_chars(content))
}

/// Upstream `estimateMessageTokens` (estimate.ts:47-69).
pub fn estimate_message_tokens(message: &Message) -> u64 {
    match message {
        Message::System(system) => {
            estimate_text_tokens(&get_system_message_text(system))
                + estimate_tools_tokens(system.tools_added.as_deref())
                + estimate_tools_tokens_references(system.tools_removed.as_deref())
        }
        Message::User(user) => estimate_text_and_image_content_tokens(&user.content),
        Message::ToolResult(result) => ceil_div_4(text_or_image_chars(&result.content)),
        Message::Assistant(assistant) => {
            let mut chars = 0;
            for block in &assistant.content {
                match block {
                    AssistantBlock::Text(text) => chars += text.text.len(),
                    AssistantBlock::Thinking(thinking) => chars += thinking.thinking.len(),
                    AssistantBlock::ToolCall(call) => {
                        chars += call.name.len();
                        chars += serde_json::to_string(&call.arguments)
                            .map(|json| json.len())
                            .unwrap_or("[unserializable]".len());
                    }
                }
            }
            ceil_div_4(chars)
        }
    }
}

/// Upstream `estimateToolsTokens` (estimate.ts:111-116) for added tool
/// definitions.
fn estimate_tools_tokens(tools: Option<&[crate::ai::types::tool::Tool]>) -> u64 {
    let Some(tools) = tools else {
        return 0;
    };
    if tools.is_empty() {
        return 0;
    }
    estimate_text_tokens(&json_len_string(tools))
}

/// The `toolsRemoved` half of upstream `estimateToolsTokens`.
fn estimate_tools_tokens_references(
    tools: Option<&[crate::ai::types::tool::ToolReference]>,
) -> u64 {
    let Some(tools) = tools else {
        return 0;
    };
    if tools.is_empty() {
        return 0;
    }
    estimate_text_tokens(&json_len_string(tools))
}

/// Serialize like upstream `safeJsonStringify` and report the JSON length.
fn json_len_string(value: &(impl serde::ser::Serialize + ?Sized)) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| String::from("[unserializable]"))
}

/// Upstream `getLastAssistantUsageInfo` (estimate.ts:71-98): the newest
/// assistant usage that still describes the current prefix — an assistant at
/// or after every later prefix message (a compaction summary, for example),
/// whose response did not fail or abort and that carries context tokens.
fn last_assistant_usage_info(messages: &[Message]) -> Option<(&Usage, usize)> {
    let mut latest_prefix_timestamp = i64::MIN;
    let mut usage_info: Option<(&Usage, usize)> = None;
    for (index, message) in messages.iter().enumerate() {
        if let Message::Assistant(assistant) = message {
            let usage_applies_to_prefix = assistant.timestamp >= latest_prefix_timestamp;
            if usage_applies_to_prefix
                && assistant.stop_reason != StopReason::Aborted
                && assistant.stop_reason != StopReason::Error
                && calculate_context_tokens(&assistant.usage) > 0
            {
                usage_info = Some((&assistant.usage, index));
            }
        }
        latest_prefix_timestamp = latest_prefix_timestamp.max(message_timestamp(message));
    }
    usage_info
}

/// The message's `timestamp` across the variants.
fn message_timestamp(message: &Message) -> i64 {
    match message {
        Message::System(system) => system.timestamp,
        Message::User(user) => user.timestamp,
        Message::Assistant(assistant) => assistant.timestamp,
        Message::ToolResult(result) => result.timestamp,
    }
}

/// Upstream `estimateContextTokens` (estimate.ts:100-109).
pub fn estimate_context_tokens(messages: &[Message]) -> ContextUsageEstimate {
    if let Some((usage, index)) = last_assistant_usage_info(messages) {
        let usage_tokens = calculate_context_tokens(usage);
        let mut trailing_tokens = 0;
        for message in &messages[index + 1..] {
            trailing_tokens += estimate_message_tokens(message);
        }
        return ContextUsageEstimate {
            tokens: usage_tokens + trailing_tokens,
            usage_tokens,
            trailing_tokens,
            last_usage_index: Some(index),
        };
    }

    let mut tokens = 0;
    for message in messages {
        tokens += estimate_message_tokens(message);
    }
    ContextUsageEstimate {
        tokens,
        usage_tokens: 0,
        trailing_tokens: tokens,
        last_usage_index: None,
    }
}

/// `Math.ceil(chars / CHARS_PER_TOKEN)` in integer arithmetic.
fn ceil_div_4(chars: usize) -> u64 {
    chars.div_ceil(CHARS_PER_TOKEN) as u64
}

/// JSON text length is what the estimation reads; kept for the doc linkage to
/// upstream's `safeJsonStringify` (whose fallback the port cannot reach —
/// `serde_json` serializes a [`Value`] infallibly).
#[allow(dead_code)]
fn safe_json_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| String::from("[unserializable]"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::faux::{
        faux_assistant_message, faux_tool_call, FauxMessageOptions, FauxToolCallOptions,
    };
    use crate::ai::types::content::{TextContent, ThinkingContent};
    use crate::ai::types::message::{
        AssistantBlock, AssistantMessage, StringOrBlocks, SystemMessage, ToolResultMessage,
        UserMessage,
    };
    use crate::ai::types::primitives::{StopReason, Usage, UsageCost};

    fn usage(total: u64) -> Usage {
        Usage {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: total,
            cost: UsageCost::default(),
        }
    }

    fn text_block(text: &str) -> AssistantBlock {
        AssistantBlock::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })
    }

    fn user(content: &str) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Text(content.to_string()),
            timestamp: 0,
        })
    }

    fn assistant(content: &str, total_tokens: u64, stop_reason: StopReason) -> AssistantMessage {
        let mut message = faux_assistant_message(
            content,
            FauxMessageOptions {
                stop_reason: Some(stop_reason),
                timestamp: Some(0),
                ..FauxMessageOptions::default()
            },
        );
        message.usage = usage(total_tokens);
        message
    }

    #[test]
    fn calculate_context_tokens_prefers_total() {
        let mut usage = usage(50);
        assert_eq!(calculate_context_tokens(&usage), 50);
        usage.total_tokens = 0;
        usage.input = 10;
        usage.output = 5;
        usage.cache_read = 2;
        usage.cache_write = 1;
        assert_eq!(calculate_context_tokens(&usage), 18);
    }

    #[test]
    fn estimates_messages_like_upstream() {
        // Text: ceil(len / 4).
        assert_eq!(estimate_text_tokens("abcd"), 1);
        assert_eq!(estimate_text_tokens("abcde"), 2);
        // User text.
        assert_eq!(estimate_message_tokens(&user("12345678")), 2);
        // Assistant text + tool call (name + JSON of arguments).
        let mut call = faux_assistant_message(
            vec![
                text_block("abcd"),
                faux_tool_call(
                    "read",
                    serde_json::json!({ "path": "a.ts" }),
                    FauxToolCallOptions {
                        id: Some("c".into()),
                    },
                ),
            ],
            FauxMessageOptions::default(),
        );
        call.timestamp = 0;
        // text 4 chars + name 4 + JSON `{"path":"a.ts"}` 15 = 23 chars -> 6.
        assert_eq!(estimate_message_tokens(&Message::Assistant(call)), 6);
        // Thinking counts.
        let mut thinking = faux_assistant_message(
            vec![AssistantBlock::Thinking(ThinkingContent {
                thinking: "12345678".to_string(),
                thinking_signature: None,
                redacted: None,
            })],
            FauxMessageOptions::default(),
        );
        thinking.timestamp = 0;
        assert_eq!(estimate_message_tokens(&Message::Assistant(thinking)), 2);
        // Tool result text.
        let result = Message::ToolResult(ToolResultMessage {
            tool_call_id: "c".into(),
            tool_name: "read".into(),
            content: vec![crate::ai::types::message::TextOrImageBlock::Text(
                TextContent {
                    text: "123456".to_string(),
                    text_signature: None,
                },
            )],
            details: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        });
        assert_eq!(estimate_message_tokens(&result), 2);
    }

    #[test]
    fn system_messages_estimate_their_replayed_text_and_tools() {
        let system = Message::System(SystemMessage {
            content: StringOrBlocks::Text("12345678".to_string()),
            sections: None,
            tools_added: None,
            tools_removed: None,
            timestamp: 0,
        });
        assert_eq!(estimate_message_tokens(&system), 2);
    }

    #[test]
    fn context_estimates_from_the_newest_applicable_usage() {
        let messages = vec![
            user("12345678"),                                             // 2 estimated
            Message::Assistant(assistant("abcd", 100, StopReason::Stop)), // usage 100
            user("12345678"),                                             // trailing 2
        ];
        let estimate = estimate_context_tokens(&messages);
        assert_eq!(estimate.usage_tokens, 100);
        assert_eq!(estimate.trailing_tokens, 2);
        assert_eq!(estimate.tokens, 102);
        assert_eq!(estimate.last_usage_index, Some(1));

        // An errored newest response is skipped; an older good one applies.
        let messages = vec![
            user("12345678"),
            Message::Assistant(assistant("abcd", 100, StopReason::Stop)),
            Message::Assistant(assistant("abcd", 999, StopReason::Error)),
        ];
        let estimate = estimate_context_tokens(&messages);
        assert_eq!(estimate.last_usage_index, Some(1));
        assert_eq!(estimate.tokens, 101);
    }

    #[test]
    fn context_without_usable_usage_estimates_everything() {
        let messages = vec![user("12345678"), user("12345678")];
        let estimate = estimate_context_tokens(&messages);
        assert_eq!(estimate.tokens, 4);
        assert_eq!(estimate.usage_tokens, 0);
        assert_eq!(estimate.last_usage_index, None);
    }
}
