//! Port of `packages/agent/src/harness/messages.ts` (169 lines): the four
//! harness-registered transcript message kinds, their summary constants, the
//! text renderers, and the harness `convertToLlm`.
//!
//! Upstream registers these kinds into `AgentMessage` by declaration merging
//! (`messages.ts:54-61`); the port's closed [`AgentMessage`] enum captures
//! non-standard roles as [`AgentMessage::Custom`] (the substitution disclosed
//! in `agent_core/types.rs`). These structs are the typed views over those
//! captured messages: each one round-trips its upstream wire shape
//! (`{"role":"bashExecution",...}` — role-tagged flat objects, camelCase
//! fields, `undefined` fields omitted) and [`convert_to_llm`] dispatches on
//! the custom role string exactly like the upstream `switch (m.role)`.
//!
//! Upstream timestamps are `number | string` on the `create*` helpers
//! (`new Date(timestamp).getTime()`); every in-tree caller passes numbers, so
//! the port takes Unix-millisecond integers only.

use serde::{Deserialize, Serialize};

use crate::agent_core::types::{AgentMessage, CustomAgentMessage};
use crate::ai::types::message::{Message, StringOrBlocks, UserMessage};

/// Upstream `COMPACTION_SUMMARY_PREFIX` (`messages.ts:4-7`).
pub const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";

/// Upstream `COMPACTION_SUMMARY_SUFFIX` (`messages.ts:9-10`).
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

/// Upstream `BRANCH_SUMMARY_PREFIX` (`messages.ts:12-15`).
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";

/// Upstream `BRANCH_SUMMARY_SUFFIX` (`messages.ts:17`): no leading newline,
/// unlike the compaction suffix.
pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

/// Upstream `BashExecutionMessage` (`messages.ts:19-29`): a bash tool run the
/// app appended to the transcript. The `role: "bashExecution"` tag is carried
/// by [`AgentMessage::Custom`]; see [`BashExecutionMessage::to_custom`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    #[serde(default)]
    pub output: String,
    /// Upstream `exitCode: number | undefined` — omitted from the wire when
    /// absent, like `JSON.stringify` drops `undefined`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_from_context: Option<bool>,
}

impl BashExecutionMessage {
    /// Parse the typed view from a captured custom message (the declaration-
    /// merging analogue). `None` when the payload does not match the shape.
    pub fn from_custom(custom: &CustomAgentMessage) -> Option<Self> {
        let value = serde_json::to_value(custom).ok()?;
        serde_json::from_value(value).ok()
    }

    /// Capture as a custom message: the flat role-tagged wire object.
    pub fn to_custom(&self) -> CustomAgentMessage {
        let mut data = serde_json::to_value(self)
            .expect("bash execution message serializes")
            .as_object()
            .cloned()
            .expect("bash execution message serializes to an object");
        data.insert("role".into(), "bashExecution".into());
        CustomAgentMessage {
            role: "bashExecution".into(),
            data,
        }
    }
}

/// Upstream `CustomMessage` (`messages.ts:31-38`): an app-defined display or
/// data message (`role: "custom"`, discriminated further by `customType`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    /// Upstream `customType` — the app's sub-discriminator.
    pub custom_type: String,
    pub content: StringOrBlocks,
    pub display: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub timestamp: i64,
}

impl CustomMessage {
    /// See [`BashExecutionMessage::from_custom`].
    pub fn from_custom(custom: &CustomAgentMessage) -> Option<Self> {
        let value = serde_json::to_value(custom).ok()?;
        serde_json::from_value(value).ok()
    }

    /// See [`BashExecutionMessage::to_custom`].
    pub fn to_custom(&self) -> CustomAgentMessage {
        let mut data = serde_json::to_value(self)
            .expect("custom message serializes")
            .as_object()
            .cloned()
            .expect("custom message serializes to an object");
        data.insert("role".into(), "custom".into());
        CustomAgentMessage {
            role: "custom".into(),
            data,
        }
    }
}

/// Upstream `BranchSummaryMessage` (`messages.ts:40-45`). `fromId` is
/// `string | null` and always present on the wire (serialized `null`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    pub from_id: Option<String>,
    pub timestamp: i64,
}

impl BranchSummaryMessage {
    /// See [`BashExecutionMessage::from_custom`].
    pub fn from_custom(custom: &CustomAgentMessage) -> Option<Self> {
        let value = serde_json::to_value(custom).ok()?;
        serde_json::from_value(value).ok()
    }

    /// See [`BashExecutionMessage::to_custom`].
    pub fn to_custom(&self) -> CustomAgentMessage {
        let mut data = serde_json::to_value(self)
            .expect("branch summary message serializes")
            .as_object()
            .cloned()
            .expect("branch summary message serializes to an object");
        data.insert("role".into(), "branchSummary".into());
        CustomAgentMessage {
            role: "branchSummary".into(),
            data,
        }
    }
}

/// Upstream `CompactionSummaryMessage` (`messages.ts:47-52`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub tokens_before: i64,
    pub timestamp: i64,
}

impl CompactionSummaryMessage {
    /// See [`BashExecutionMessage::from_custom`].
    pub fn from_custom(custom: &CustomAgentMessage) -> Option<Self> {
        let value = serde_json::to_value(custom).ok()?;
        serde_json::from_value(value).ok()
    }

    /// See [`BashExecutionMessage::to_custom`].
    pub fn to_custom(&self) -> CustomAgentMessage {
        let mut data = serde_json::to_value(self)
            .expect("compaction summary message serializes")
            .as_object()
            .cloned()
            .expect("compaction summary message serializes to an object");
        data.insert("role".into(), "compactionSummary".into());
        CustomAgentMessage {
            role: "compactionSummary".into(),
            data,
        }
    }
}

fn user_message(content: String, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: StringOrBlocks::Text(content),
        timestamp,
    })
}

/// Upstream `bashExecutionToText` (`messages.ts:63-79`): the model-visible
/// rendering of a bash execution.
pub fn bash_execution_to_text(msg: &BashExecutionMessage) -> String {
    let mut text = format!("Ran `{}`\n", msg.command);
    if !msg.output.is_empty() {
        text += &format!("```\n{}\n```", msg.output);
    } else {
        text += "(no output)";
    }
    if msg.cancelled {
        text += "\n\n(command cancelled)";
    } else if let Some(exit_code) = msg.exit_code {
        if exit_code != 0 {
            text += &format!("\n\nCommand exited with code {exit_code}");
        }
    }
    if msg.truncated {
        if let Some(full_output_path) = &msg.full_output_path {
            text += &format!("\n\n[Output truncated. Full output: {full_output_path}]");
        }
    }
    text
}

/// Upstream `createBranchSummaryMessage` (`messages.ts:81-92`).
pub fn create_branch_summary_message(
    summary: impl Into<String>,
    from_id: Option<String>,
    timestamp: i64,
) -> BranchSummaryMessage {
    BranchSummaryMessage {
        summary: summary.into(),
        from_id,
        timestamp,
    }
}

/// Upstream `createCompactionSummaryMessage` (`messages.ts:94-105`).
pub fn create_compaction_summary_message(
    summary: impl Into<String>,
    tokens_before: i64,
    timestamp: i64,
) -> CompactionSummaryMessage {
    CompactionSummaryMessage {
        summary: summary.into(),
        tokens_before,
        timestamp,
    }
}

/// Upstream `createCustomMessage` (`messages.ts:107-122`).
#[allow(clippy::too_many_arguments)]
pub fn create_custom_message(
    custom_type: impl Into<String>,
    content: StringOrBlocks,
    display: bool,
    details: Option<serde_json::Value>,
    timestamp: i64,
) -> CustomMessage {
    CustomMessage {
        custom_type: custom_type.into(),
        content,
        display,
        details,
        timestamp,
    }
}

/// Upstream `convertToLlm` (`messages.ts:124-169`): project the transcript
/// into LLM messages. The four harness roles render as user messages
/// (`bashExecution` honors `excludeFromContext`), standard roles pass
/// through, and any other custom role is dropped (the upstream `default`
/// branch).
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::System(_)
            | AgentMessage::User(_)
            | AgentMessage::Assistant(_)
            | AgentMessage::ToolResult(_) => message.to_message(),
            AgentMessage::Custom(custom) => match custom.role.as_str() {
                "bashExecution" => {
                    let msg = BashExecutionMessage::from_custom(custom)?;
                    if msg.exclude_from_context == Some(true) {
                        None
                    } else {
                        Some(user_message(bash_execution_to_text(&msg), msg.timestamp))
                    }
                }
                "custom" => {
                    let msg = CustomMessage::from_custom(custom)?;
                    Some(Message::User(UserMessage {
                        content: msg.content,
                        timestamp: msg.timestamp,
                    }))
                }
                "branchSummary" => {
                    let msg = BranchSummaryMessage::from_custom(custom)?;
                    Some(user_message(
                        format!(
                            "{BRANCH_SUMMARY_PREFIX}{}{BRANCH_SUMMARY_SUFFIX}",
                            msg.summary
                        ),
                        msg.timestamp,
                    ))
                }
                "compactionSummary" => {
                    let msg = CompactionSummaryMessage::from_custom(custom)?;
                    Some(user_message(
                        format!(
                            "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                            msg.summary
                        ),
                        msg.timestamp,
                    ))
                }
                _ => None,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests;
