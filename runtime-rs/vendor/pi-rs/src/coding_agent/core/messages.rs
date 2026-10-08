//! Port of upstream `coding-agent/src/core/messages.ts`.
//!
//! Custom message types for the coding agent (bash executions via `!`,
//! extension-injected custom messages, branch/compaction summaries) and the
//! `convertToLlm` transformer producing LLM-compatible messages.
//!
//! Upstream registers the four custom roles on `AgentMessage` by declaration
//! merging. The port's [`crate::agent_core::types::AgentMessage`] captures
//! custom roles as [`crate::agent_core::types::CustomAgentMessage`] data
//! (role + remaining JSON fields), so the ported transformer parses the
//! custom payloads into the typed structs below ([`BashExecutionMessage`],
//! [`CustomMessage`], [`BranchSummaryMessage`], [`CompactionSummaryMessage`])
//! and skips payloads that do not match (upstream cannot express that case —
//! the types are compile-time there; disclosed).
//!
//! `create*Message` constructors parse ISO-8601 timestamps with a small
//! hand-written parser (no chrono dependency): `YYYY-MM-DDTHH:MM:SS(.mmm)?`
//! with optional `Z`/`±HH(:MM)?` offset, interpreted as UTC when the offset
//! is absent. Node's `new Date(...)` uses *local* time for offset-less
//! strings — tests pin the Z form, which is byte-identical.

use serde::{Deserialize, Serialize};

use crate::agent_core::types::AgentMessage;
use crate::ai::types::{Message, StringOrBlocks, TextContent, TextOrImageBlock, UserMessage};

/// Upstream `COMPACTION_SUMMARY_PREFIX` (template literal, byte-exact).
pub const COMPACTION_SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";

/// Upstream `COMPACTION_SUMMARY_SUFFIX`.
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

/// Upstream `BRANCH_SUMMARY_PREFIX`.
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";

/// Upstream `BRANCH_SUMMARY_SUFFIX`.
pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

/// Message type for bash executions via the `!` command (upstream
/// `BashExecutionMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    /// Upstream `number | undefined`; JSON `null` also parses to `None`.
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub full_output_path: Option<String>,
    pub timestamp: i64,
    /// If true, this message is excluded from LLM context (`!!` prefix).
    #[serde(default)]
    pub exclude_from_context: Option<bool>,
}

/// Upstream `CustomMessage.content`:
/// `string | (TextContent | ImageContent)[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomMessageContent {
    Text(String),
    Blocks(Vec<TextOrImageBlock>),
}

/// Message type for extension-injected messages via sendMessage() (upstream
/// `CustomMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    pub custom_type: String,
    pub content: CustomMessageContent,
    pub display: bool,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    pub timestamp: i64,
}

/// Upstream `BranchSummaryMessage`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    /// Upstream `string | null`.
    #[serde(default)]
    pub from_id: Option<String>,
    pub timestamp: i64,
}

/// Upstream `CompactionSummaryMessage`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub tokens_before: i64,
    pub timestamp: i64,
}

/// `new Date(timestamp).getTime()` for the ISO-8601 subset described in the
/// module docs; `None` stands in for JS `NaN` (unparseable input).
pub fn parse_epoch_millis(timestamp: &str) -> Option<i64> {
    let trimmed = timestamp.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year: i64 = trimmed.get(0..4)?.parse().ok()?;
    let month: u32 = trimmed.get(5..7)?.parse().ok()?;
    let day: u32 = trimmed.get(8..10)?.parse().ok()?;
    let hour: u32 = trimmed.get(11..13)?.parse().ok()?;
    let minute: u32 = trimmed.get(14..16)?.parse().ok()?;
    let second: u32 = trimmed.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut rest = &trimmed[19..];
    let mut millis: i64 = 0;
    if let Some(fractional) = rest.strip_prefix('.') {
        let digits: String = fractional
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() {
            return None;
        }
        rest = &fractional[digits.len()..];
        let scaled = format!("{:0<3}", &digits[..digits.len().min(3)]);
        millis = scaled.parse().ok()?;
    }

    // Timezone: Z, ±HH, ±HH:MM, or absent (treated as UTC on the port —
    // disclosed: node uses local time for offset-less strings).
    let offset_ms: i64 =
        if let Some(rest) = rest.strip_prefix('Z').or_else(|| rest.strip_prefix('z')) {
            if !rest.is_empty() {
                return None;
            }
            0
        } else if rest.is_empty() {
            0
        } else {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let offset = &rest[1..];
            let (offset_hours, offset_minutes) = match offset.len() {
                2 => (offset.parse::<i64>().ok()?, 0),
                4 => (
                    offset.get(0..2)?.parse().ok()?,
                    offset.get(2..4)?.parse().ok()?,
                ),
                5 if offset.as_bytes()[2] == b':' => (
                    offset.get(0..2)?.parse().ok()?,
                    offset.get(3..5)?.parse().ok()?,
                ),
                _ => return None,
            };
            sign * (offset_hours * 60 + offset_minutes) * 60_000
        };

    let days = days_from_civil(year, month, day);
    Some(
        days * 86_400_000
            + hour as i64 * 3_600_000
            + minute as i64 * 60_000
            + second as i64 * 1_000
            + millis
            - offset_ms,
    )
}

/// Days since 1970-01-01 from a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Convert a BashExecutionMessage to user message text for LLM context.
pub fn bash_execution_to_text(msg: &BashExecutionMessage) -> String {
    let mut text = format!("Ran `{}`\n", msg.command);
    if !msg.output.is_empty() {
        text += &format!("```\n{}\n```", msg.output);
    } else {
        text += "(no output)";
    }
    if msg.cancelled {
        text += "\n\n(command cancelled)";
    } else if let Some(exit_code) = msg.exit_code.filter(|code| *code != 0) {
        text += &format!("\n\nCommand exited with code {exit_code}");
    }
    if msg.truncated {
        if let Some(full_output_path) = &msg.full_output_path {
            text += &format!("\n\n[Output truncated. Full output: {full_output_path}]");
        }
    }
    text
}

/// Upstream `createBranchSummaryMessage(summary, fromId, timestamp)`.
pub fn create_branch_summary_message(
    summary: &str,
    from_id: &str,
    timestamp: &str,
) -> Option<BranchSummaryMessage> {
    Some(BranchSummaryMessage {
        summary: summary.to_string(),
        from_id: Some(from_id.to_string()),
        timestamp: parse_epoch_millis(timestamp)?,
    })
}

/// Upstream `createCompactionSummaryMessage(summary, tokensBefore, timestamp)`.
pub fn create_compaction_summary_message(
    summary: &str,
    tokens_before: i64,
    timestamp: &str,
) -> Option<CompactionSummaryMessage> {
    Some(CompactionSummaryMessage {
        summary: summary.to_string(),
        tokens_before,
        timestamp: parse_epoch_millis(timestamp)?,
    })
}

/// Upstream `createCustomMessage(customType, content, display, details, timestamp)`.
pub fn create_custom_message(
    custom_type: &str,
    content: CustomMessageContent,
    display: bool,
    details: Option<serde_json::Value>,
    timestamp: &str,
) -> Option<CustomMessage> {
    Some(CustomMessage {
        custom_type: custom_type.to_string(),
        content,
        display,
        details,
        timestamp: parse_epoch_millis(timestamp)?,
    })
}

fn text_block(text: String) -> TextOrImageBlock {
    TextOrImageBlock::Text(TextContent {
        text,
        text_signature: None,
    })
}

fn user_message_with_text(text: String, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: StringOrBlocks::Blocks(vec![text_block(text)]),
        timestamp,
    })
}

fn parse_custom_data<T: serde::de::DeserializeOwned>(
    message: &crate::agent_core::types::CustomAgentMessage,
) -> Option<T> {
    serde_json::from_value(serde_json::Value::Object(message.data.clone())).ok()
}

/// Upstream `convertToLlm(messages)`: transform AgentMessages (including the
/// coding-agent custom types) to LLM-compatible Messages, dropping messages
/// excluded from context and unrecognized custom roles.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::System(system) => Some(Message::System(system.clone())),
            AgentMessage::User(user) => Some(Message::User(user.clone())),
            AgentMessage::Assistant(assistant) => Some(Message::Assistant(assistant.clone())),
            AgentMessage::ToolResult(tool_result) => Some(Message::ToolResult(tool_result.clone())),
            AgentMessage::Custom(custom) => match custom.role.as_str() {
                "bashExecution" => {
                    let bash: BashExecutionMessage = parse_custom_data(custom)?;
                    // Skip messages excluded from context (!! prefix)
                    if bash.exclude_from_context == Some(true) {
                        return None;
                    }
                    Some(user_message_with_text(
                        bash_execution_to_text(&bash),
                        bash.timestamp,
                    ))
                }
                "custom" => {
                    let custom_message: CustomMessage = parse_custom_data(custom)?;
                    let content = match custom_message.content {
                        CustomMessageContent::Text(text) => vec![text_block(text)],
                        CustomMessageContent::Blocks(blocks) => blocks,
                    };
                    Some(Message::User(UserMessage {
                        content: StringOrBlocks::Blocks(content),
                        timestamp: custom_message.timestamp,
                    }))
                }
                "branchSummary" => {
                    let summary: BranchSummaryMessage = parse_custom_data(custom)?;
                    Some(user_message_with_text(
                        format!(
                            "{BRANCH_SUMMARY_PREFIX}{}{BRANCH_SUMMARY_SUFFIX}",
                            summary.summary
                        ),
                        summary.timestamp,
                    ))
                }
                "compactionSummary" => {
                    let summary: CompactionSummaryMessage = parse_custom_data(custom)?;
                    Some(user_message_with_text(
                        format!(
                            "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                            summary.summary
                        ),
                        summary.timestamp,
                    ))
                }
                _ => None,
            },
        })
        .collect()
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
