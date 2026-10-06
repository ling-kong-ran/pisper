//! Port of `packages/agent/src/harness/compaction/utils.ts` (133 lines): the
//! file-operation accumulator, its summary rendering, and the conversation
//! serializer that feeds the summarization prompts.
//!
//! Also hosts [`add_usage`] from upstream `harness/utils/usage.ts:18-43` —
//! split-turn compaction is its only consumer so far; it moves to a
//! `harness::utils` module when Task 6 ports the remaining util files.
//!
//! Disclosed substitutions:
//! - Upstream `FileOperations` holds `Set<string>`s; the port uses
//!   `HashSet<String>` (`compute_file_lists` sorts before returning, so
//!   iteration order never escapes).
//! - JS `.length` counts UTF-16 code units; the port counts scalar
//!   characters (`faux.rs` precedent — identical for the BMP fixtures the
//!   prompts carry in practice).
//! - `contentText` over ai-layer block arrays: the shared transcript helper
//!   covers `StringOrBlocks` only, so the two block-array joins here are
//!   local (`assistant_blocks_text`, `text_or_image_blocks_text`).

use std::collections::HashSet;

use crate::agent_core::types::{AgentMessage, CustomAgentMessage};
use crate::ai::transcript::content_text_with_separator;
use crate::ai::types::message::{AssistantBlock, Message, TextOrImageBlock};
use crate::ai::types::primitives::Usage;

use crate::agent_core::harness::messages::{
    BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage, CustomMessage,
};

/// Upstream `FileOperations` (`utils.ts:5-12`): file paths touched by a
/// session branch or compaction range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOperations {
    /// Files read but not necessarily modified.
    pub read: HashSet<String>,
    /// Files written by full-file write operations.
    pub written: HashSet<String>,
    /// Files modified by edit operations.
    pub edited: HashSet<String>,
}

impl FileOperations {
    /// Upstream `createFileOps` (`utils.ts:15-21`).
    pub fn new() -> Self {
        FileOperations::default()
    }
}

/// Upstream `extractFileOpsFromMessage` (`utils.ts:24-51`): add file
/// operations from assistant `read`/`write`/`edit` tool calls to an
/// accumulator. Non-assistant messages and calls without a string `path`
/// contribute nothing.
pub fn extract_file_ops_from_message(message: &AgentMessage, file_ops: &mut FileOperations) {
    let AgentMessage::Assistant(assistant) = message else {
        return;
    };
    for block in &assistant.content {
        let AssistantBlock::ToolCall(tool_call) = block else {
            continue;
        };
        let Some(path) = tool_call
            .arguments
            .get("path")
            .and_then(|path| path.as_str())
        else {
            continue;
        };
        match tool_call.name.as_str() {
            "read" => {
                file_ops.read.insert(path.to_string());
            }
            "write" => {
                file_ops.written.insert(path.to_string());
            }
            "edit" => {
                file_ops.edited.insert(path.to_string());
            }
            _ => {}
        }
    }
}

/// Upstream `computeFileLists` return shape (`utils.ts:54-59`): sorted
/// read-only and modified lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLists {
    /// Files read but not modified.
    pub read_files: Vec<String>,
    /// Files written or edited.
    pub modified_files: Vec<String>,
}

/// Upstream `computeFileLists` (`utils.ts:54-59`): modified = edited ∪
/// written; read-only excludes modified; both lists sort.
pub fn compute_file_lists(file_ops: &FileOperations) -> FileLists {
    let mut modified: HashSet<String> = file_ops.edited.clone();
    modified.extend(file_ops.written.iter().cloned());
    let read_files: Vec<String> = file_ops
        .read
        .iter()
        .filter(|file| !modified.contains(*file))
        .cloned()
        .collect();
    let mut read_files = read_files;
    read_files.sort();
    let mut modified_files: Vec<String> = modified.into_iter().collect();
    modified_files.sort();
    FileLists {
        read_files,
        modified_files,
    }
}

/// Upstream `formatFileOperations` (`utils.ts:62-72`): format the file lists
/// as summary metadata tags, blank-line-prefixed; empty when both lists are.
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

/// Upstream `TOOL_RESULT_MAX_CHARS` (`utils.ts:74`).
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// Upstream `safeJsonStringify` (`utils.ts:76-82`): compact JSON, with a
/// placeholder for values that fail to serialize (serde has no `undefined`,
/// so only serialization errors land there).
fn safe_json_stringify(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string())
}

/// Upstream `truncateForSummary` (`utils.ts:84-88`).
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let length = text.chars().count();
    if length <= max_chars {
        return text.to_string();
    }
    let truncated_chars = length - max_chars;
    let cut = text
        .char_indices()
        .nth(max_chars)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len());
    format!(
        "{}\n\n[... {truncated_chars} more characters truncated]",
        &text[..cut]
    )
}

/// The `contentText` join over assistant content blocks
/// (`packages/ai/src/utils/text.ts:6-12` at the assistant block union).
pub(crate) fn assistant_blocks_text(blocks: &[AssistantBlock], separator: &str) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<&str>>()
        .join(separator)
}

/// The `contentText` join over tool-result content blocks.
pub(crate) fn text_or_image_blocks_text(blocks: &[TextOrImageBlock], separator: &str) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            TextOrImageBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<&str>>()
        .join(separator)
}

/// Upstream `serializeConversation` (`utils.ts:91-132`): serialize LLM
/// messages to plain text for summarization prompts — user text, assistant
/// thinking/text/tool-calls (empty user content skipped, tool results
/// truncated at [`TOOL_RESULT_MAX_CHARS`]), joined by blank lines.
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for message in messages {
        match message {
            Message::User(user) => {
                let content = content_text_with_separator(&user.content, "");
                if !content.is_empty() {
                    parts.push(format!("[User]: {content}"));
                }
            }
            Message::Assistant(assistant) => {
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<String> = Vec::new();

                for block in &assistant.content {
                    match block {
                        AssistantBlock::Thinking(thinking) => {
                            thinking_parts.push(thinking.thinking.clone());
                        }
                        AssistantBlock::ToolCall(tool_call) => {
                            let arguments = &tool_call.arguments;
                            let args_text = arguments
                                .as_object()
                                .map(|entries| {
                                    entries
                                        .iter()
                                        .map(|(key, value)| {
                                            format!("{key}={}", safe_json_stringify(value))
                                        })
                                        .collect::<Vec<String>>()
                                        .join(", ")
                                })
                                .unwrap_or_default();
                            tool_calls.push(format!("{}({args_text})", tool_call.name));
                        }
                        AssistantBlock::Text(_) => {}
                    }
                }

                if !thinking_parts.is_empty() {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        thinking_parts.join("\n")
                    ));
                }
                let has_text = assistant
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::Text(_)));
                if has_text {
                    parts.push(format!(
                        "[Assistant]: {}",
                        assistant_blocks_text(&assistant.content, "\n")
                    ));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Message::ToolResult(tool_result) => {
                let content = text_or_image_blocks_text(&tool_result.content, "");
                if !content.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&content, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            Message::System(_) => {}
        }
    }

    parts.join("\n\n")
}

/// Upstream `addUsage` (`harness/utils/usage.ts:18-43`): componentwise sums;
/// `cacheWrite1h`/`reasoning` are present on the result exactly when present
/// on either input (`(left ?? 0) + (right ?? 0)`).
pub fn add_usage(left: Usage, right: Usage) -> Usage {
    let cache_write_1h = match (left.cache_write_1h, right.cache_write_1h) {
        (None, None) => None,
        (left_value, right_value) => Some(
            left_value
                .unwrap_or(0)
                .saturating_add(right_value.unwrap_or(0)),
        ),
    };
    let reasoning = match (left.reasoning, right.reasoning) {
        (None, None) => None,
        (left_value, right_value) => Some(
            left_value
                .unwrap_or(0)
                .saturating_add(right_value.unwrap_or(0)),
        ),
    };
    Usage {
        input: left.input.saturating_add(right.input),
        output: left.output.saturating_add(right.output),
        cache_read: left.cache_read.saturating_add(right.cache_read),
        cache_write: left.cache_write.saturating_add(right.cache_write),
        cache_write_1h,
        reasoning,
        total_tokens: left.total_tokens.saturating_add(right.total_tokens),
        cost: crate::ai::types::primitives::UsageCost {
            input: left.cost.input + right.cost.input,
            output: left.cost.output + right.cost.output,
            cache_read: left.cost.cache_read + right.cost.cache_read,
            cache_write: left.cost.cache_write + right.cost.cache_write,
            total: left.cost.total + right.cost.total,
        },
    }
}

// Typed views over the captured harness custom messages (upstream reads the
// declaration-merged fields directly); `pub(crate)` so the sibling modules in
// this package reuse the same parse path.
pub(crate) fn custom_message_view(custom: &CustomAgentMessage) -> Option<CustomMessage> {
    CustomMessage::from_custom(custom)
}

pub(crate) fn bash_execution_view(custom: &CustomAgentMessage) -> Option<BashExecutionMessage> {
    BashExecutionMessage::from_custom(custom)
}

pub(crate) fn branch_summary_view(custom: &CustomAgentMessage) -> Option<BranchSummaryMessage> {
    BranchSummaryMessage::from_custom(custom)
}

pub(crate) fn compaction_summary_view(
    custom: &CustomAgentMessage,
) -> Option<CompactionSummaryMessage> {
    CompactionSummaryMessage::from_custom(custom)
}

#[cfg(test)]
mod tests;
