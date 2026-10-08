//! Port of `packages/ai/src/utils/estimate.ts` (117 lines), the context
//! token estimator. Upstream home is the pi-ai package; the port lands
//! beside its only M3b consumer (the pico3 generation and collapse kinds),
//! whose request messages are stored-JSON shapes — the estimator works over
//! the same JSON message objects.
//!
//! `getSystemMessageText` reuses the pi-ai port
//! ([`crate::ai::transcript::get_system_message_text`]).

use serde_json::Value;

use crate::ai::transcript::get_system_message_text;

/// Upstream `CHARS_PER_TOKEN` / `ESTIMATED_IMAGE_CHARS` (`estimate.ts:15-16`).
const CHARS_PER_TOKEN: f64 = 4.0;
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// Upstream `ContextUsageEstimate` (`estimate.ts:4-13`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContextUsageEstimate {
    /// Estimated total context tokens.
    pub tokens: i64,
    /// Tokens reported by the most recent applicable assistant usage block.
    pub usage_tokens: i64,
    /// Estimated tokens after the most recent applicable assistant usage
    /// block.
    pub trailing_tokens: i64,
    /// Index of the applicable message that provided usage, or `None`.
    pub last_usage_index: Option<usize>,
}

/// Upstream `calculateContextTokens` (`estimate.ts:18-20`).
pub fn calculate_context_tokens(usage: &Value) -> i64 {
    let total = usage
        .get("totalTokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if total != 0 {
        return total;
    }
    input_output_cache(usage)
}

fn input_output_cache(usage: &Value) -> i64 {
    let field = |key: &str| usage.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    (field("input") + field("output") + field("cacheRead") + field("cacheWrite")) as i64
}

/// Upstream `estimateTextTokens` (`estimate.ts:38-40`); `text.length` is the
/// UTF-16 code-unit count upstream, so the port counts UTF-16 units.
pub fn estimate_text_tokens(text: &str) -> i64 {
    (text.encode_utf16().count() as f64 / CHARS_PER_TOKEN).ceil() as i64
}

/// Upstream `estimateTextAndImageContentTokens` (`estimate.ts:42-44`).
fn estimate_text_and_image_content_tokens(content: &Value) -> i64 {
    let chars = estimate_text_and_image_content_chars(content);
    (chars as f64 / CHARS_PER_TOKEN).ceil() as i64
}

fn estimate_text_and_image_content_chars(content: &Value) -> usize {
    match content {
        Value::String(text) => text.encode_utf16().count(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| text.encode_utf16().count())
                        .unwrap_or(0)
                } else {
                    ESTIMATED_IMAGE_CHARS
                }
            })
            .sum(),
        _ => 0,
    }
}

/// Upstream `estimateMessageTokens` (`estimate.ts:46-69`).
pub fn estimate_message_tokens(message: &Value) -> i64 {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match role {
        "system" => {
            let text = serde_json::from_value::<crate::ai::types::SystemMessage>(message.clone())
                .map(|system| get_system_message_text(&system))
                .unwrap_or_default();
            estimate_text_tokens(&text)
                + estimate_tools_tokens(message.get("toolsAdded"))
                + estimate_tools_tokens(message.get("toolsRemoved"))
        }
        "user" | "toolResult" => message
            .get("content")
            .map(estimate_text_and_image_content_tokens)
            .unwrap_or(0),
        _ => {
            // assistant
            let mut chars: usize = 0;
            for block in message
                .get("content")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        chars += block
                            .get("text")
                            .and_then(Value::as_str)
                            .map(|text| text.encode_utf16().count())
                            .unwrap_or(0);
                    }
                    Some("thinking") => {
                        chars += block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .map(|text| text.encode_utf16().count())
                            .unwrap_or(0);
                    }
                    Some("toolCall") => {
                        chars += block
                            .get("name")
                            .and_then(Value::as_str)
                            .map(|name| name.len())
                            .unwrap_or(0);
                        if let Some(arguments) = block.get("arguments") {
                            chars += safe_json_stringify(arguments).len();
                        }
                    }
                    _ => {}
                }
            }
            (chars as f64 / CHARS_PER_TOKEN).ceil() as i64
        }
    }
}

/// Upstream `getLastAssistantUsageInfo` (`estimate.ts:71-95`).
fn get_last_assistant_usage_info(messages: &[Value]) -> Option<(Value, usize)> {
    let mut latest_prefix_timestamp = f64::NEG_INFINITY;
    let mut usage_info: Option<(Value, usize)> = None;
    for (index, message) in messages.iter().enumerate() {
        if message.get("role").and_then(Value::as_str) == Some("assistant") {
            // A newer prefix message was inserted after this response (for
            // example, a compaction summary), so its usage cannot describe
            // the current prefix.
            let timestamp = message
                .get("timestamp")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let usage_applies_to_prefix = timestamp >= latest_prefix_timestamp;
            let stop_reason = message.get("stopReason").and_then(Value::as_str);
            let usage = message.get("usage").cloned().unwrap_or(Value::Null);
            if usage_applies_to_prefix
                && stop_reason != Some("aborted")
                && stop_reason != Some("error")
                && calculate_context_tokens(&usage) > 0
            {
                usage_info = Some((usage, index));
            }
        }
        latest_prefix_timestamp = latest_prefix_timestamp.max(
            message
                .get("timestamp")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        );
    }
    usage_info
}

/// Upstream `estimateContextTokens` (`estimate.ts:97-112`).
pub fn estimate_context_tokens(messages: &[Value]) -> ContextUsageEstimate {
    if let Some((usage, index)) = get_last_assistant_usage_info(messages) {
        let usage_tokens = calculate_context_tokens(&usage);
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

/// Upstream `estimateToolsTokens` (`estimate.ts:114-117`).
fn estimate_tools_tokens(tools: Option<&Value>) -> i64 {
    let Some(tools) = tools else {
        return 0;
    };
    match tools {
        Value::Array(array) if array.is_empty() => 0,
        _ => estimate_text_tokens(&safe_json_stringify(tools)),
    }
}

/// Upstream `safeJsonStringify` (`estimate.ts:22-28`).
fn safe_json_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_owned())
}
