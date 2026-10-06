//! Port of upstream `coding-agent/src/core/compaction/` — product-layer
//! context compaction for long coding-agent sessions: token estimation,
//! cut-point selection, preparation of the compacted range, summary
//! generation, and abandoned-branch summarization. Snapshot at migration:
//! 1027 lines in compaction.ts, 382 in branch-summarization.ts, 158 in
//! utils.ts, 7 in index.ts.
//!
//! This is deliberately separate from the agent-layer port
//! ([`crate::agent_core::harness::compaction`]): the two upstream packages
//! (`packages/coding-agent` vs `packages/agent`) evolve independently and the
//! coding-agent cut-point/summarization semantics differ (coding-agent entries
//! replay through `sessionEntryToContextMessages`, cut points consider custom
//! message entries, prompts differ, `prepareCompaction` budgets via
//! `buildSessionContext`). Nothing is shared between the two ports beyond the
//! ai-layer primitives.
//!
//! Upstream SHA256 at migration time:
//!
//! | upstream                   | sha256 (first 12) |
//! |----------------------------|-------------------|
//! | compaction/compaction.ts   | 21e4fc33b585      |
//! | compaction/branch-summarization.ts | 0279195d2cdd |
//! | compaction/utils.ts        | 6e9d2c0b6076      |
//! | compaction/index.ts        | f82bde781410      |
//!
//! Deterministic outputs (summarization prompt construction, summary message
//! shapes including split-turn merging and file-operation tags, token-budget
//! estimation, cut-point/turn-boundary selection, conversation serialization)
//! were captured from the real upstream TypeScript sources under node (type
//! stripping; real `contentText`/`normalizeContext`/`retryAssistantCall`/
//! `uuidv7` implementations) into
//! `tests/fixtures/core_oracle_compaction/compaction.oracle.json` and are pinned by
//! byte-comparison tests in [`compaction_tests`].
//!
//! Seams (upstream imports modules outside this slice):
//!
//! - `SessionEntry` / `sessionEntryToContextMessages` / `buildSessionContext`
//!   (upstream `../session-manager.ts`, not yet ported): the local
//!   [`SessionEntry`] seam enum carries exactly the fields compaction reads,
//!   and the projection/context builders are vendored verbatim from
//!   session-manager.ts. When the session-manager slice lands, a `From`
//!   bridge can adapt the real type.
//! - `ReadonlySessionManager` (upstream `session-manager.ts` class): the
//!   [`ReadonlySessionManager`] seam trait exposes the two reads
//!   `collectEntriesForBranchSummary` makes (`getBranch`, `getEntry`).
//! - `StreamFn` (upstream `pi-agent-core`): the [`StreamFn`] seam closure; the
//!   default transport (upstream `completeSimple` over the pi-ai default
//!   registry) has no global in the Rust ai layer, so callers bind a
//!   [`Models`](crate::ai::models::Models) collection through
//!   [`models_stream_fn`]. [`complete_summarization`] with `stream_fn: None`
//!   surfaces an unbound-transport error assistant message (upstream always
//!   has a registry there).
//! - `completeSimple` / `retryAssistantCall` / `normalizeContext` /
//!   `contentText` / `uuidv7` come from the ported ai layer
//!   ([`crate::ai::models::Models::complete_simple`],
//!   [`crate::ai::retry::retry_assistant_call`],
//!   [`crate::ai::transcript::normalize_context`],
//!   [`crate::ai::uuid::uuid_v7`]).
//! - `convertToLlm` (upstream `../messages.ts`) is the already-ported
//!   [`crate::coding_agent::core::messages::convert_to_llm`]; the typed custom
//!   message structs from that module serialize back into
//!   [`crate::agent_core::types::CustomAgentMessage`] payloads for projections.
//!
//! Disclosed substitutions:
//! - Long argument lists (`generateSummaryWithUsage`, `compact`) become the
//!   [`SummaryRequest`] / [`CompactOptions`] parameter structs; behavior is
//!   unchanged.
//! - `CompactionResult<T = unknown>` is the default instantiation with the
//!   typed [`CompactionDetails`] (the only shape this module produces).
//! - `CutPointResult.turnStartIndex` and
//!   `ContextUsageEstimate.lastUsageIndex` use `Option<usize>` for the
//!   upstream `-1`/`null` sentinels.
//! - JS `.length` counts UTF-16 code units; the port counts scalar characters
//!   (faux.rs precedent — identical for the BMP fixtures in practice).
//! - `SimpleStreamOptions.sessionId` fresh routing uses
//!   [`crate::ai::uuid::uuid_v7`]; the format matches upstream uuidv7 (pinned
//!   by the oracle's `fresh_routing_session` capture).
//! - Errors thrown upstream (`new Error(message)`) surface as
//!   `Result<_, String>` carrying the same message bytes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent_core::types::{AgentMessage, CustomAgentMessage, ThinkingLevel};
use crate::ai::models::{Models, ModelsSimpleStreamOptions};
use crate::ai::retry::{retry_assistant_call, RetryCallbacks, RetryPolicy};
use crate::ai::transcript::{
    content_text_with_separator, normalize_context, Context as AiContext, TranscriptContext,
};
use crate::ai::types::content::TextContent;
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, SystemMessage, TextOrImageBlock,
    UserMessage,
};
use crate::ai::types::model::Model;
use crate::ai::types::options::{ProviderEnv, ProviderHeaders, SimpleStreamOptions};
use crate::ai::types::primitives::{
    CacheRetention, StopReason, ThinkingLevel as RequestThinkingLevel, Usage, UsageCost,
};
use crate::ai::uuid::uuid_v7;
use crate::coding_agent::core::messages::{
    convert_to_llm, create_branch_summary_message, create_compaction_summary_message,
    create_custom_message, CustomMessageContent,
};

// ============================================================================
// File Operation Tracking (upstream compaction/utils.ts)
// ============================================================================

/// Upstream `FileOperations` (`utils.ts:12-16`): file paths touched by a
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
    /// Upstream `createFileOps` (`utils.ts:18-24`).
    pub fn new() -> Self {
        FileOperations::default()
    }
}

/// Upstream `extractFileOpsFromMessage` (`utils.ts:29-56`): add file
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

/// Upstream `computeFileLists` return shape (`utils.ts:62-66`): sorted
/// read-only and modified lists.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLists {
    /// Files read but not modified.
    pub read_files: Vec<String>,
    /// Files written or edited.
    pub modified_files: Vec<String>,
}

/// Upstream `computeFileLists` (`utils.ts:62-67`): modified = edited ∪
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

/// Upstream `formatFileOperations` (`utils.ts:72-82`): format the file lists
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

// ============================================================================
// Message Serialization (upstream compaction/utils.ts)
// ============================================================================

/// Upstream `TOOL_RESULT_MAX_CHARS` (`utils.ts:89`).
const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// JS `JSON.stringify` (compact form) over a JSON value. `serde_json::Value`
/// objects are `BTreeMap`s, so key order is sorted — upstream uses insertion
/// order (disclosed; oracle fixtures keep argument objects sorted).
fn compact_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string())
}

/// Upstream `truncateForSummary` (`utils.ts:95-99`).
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
/// (upstream `packages/ai/src/utils/text.ts` at the assistant block union).
fn assistant_blocks_text(blocks: &[AssistantBlock], separator: &str) -> String {
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
fn text_or_image_blocks_text(blocks: &[TextOrImageBlock], separator: &str) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            TextOrImageBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<&str>>()
        .join(separator)
}

/// Upstream `serializeConversation` (`utils.ts:109-150`): serialize LLM
/// messages to plain text for summarization prompts — user text, assistant
/// thinking/text/tool-calls (empty user content skipped, tool results
/// truncated at [`TOOL_RESULT_MAX_CHARS`]), joined by blank lines.
///
/// Call [`convert_to_llm`] first to handle custom message types.
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
                            let args_text = tool_call
                                .arguments
                                .as_object()
                                .map(|entries| {
                                    entries
                                        .iter()
                                        .map(|(key, value)| {
                                            format!("{key}={}", compact_json(value))
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

/// Upstream `SUMMARIZATION_SYSTEM_PROMPT` (`utils.ts:156-158`).
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

// ============================================================================
// Session entry seam (upstream session-manager.ts, not yet ported)
// ============================================================================

/// Seam for upstream `SessionEntry` (`session-manager.ts:153-163`): the entry
/// kinds compaction reads, carrying exactly the fields it consumes. When the
/// session-manager slice lands, a `From` bridge can adapt the real type.
/// The Assistant message payload dwarfs the other variants (same
/// size-difference allow as [`crate::agent_core::types::AgentMessage`]).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEntry {
    /// Upstream `type: "message"`.
    Message {
        id: String,
        parent_id: Option<String>,
        message: AgentMessage,
    },
    /// Upstream `type: "custom_message"` (participates in LLM context).
    CustomMessage {
        id: String,
        parent_id: Option<String>,
        custom_type: String,
        content: CustomMessageContent,
        display: bool,
        details: Option<serde_json::Value>,
        timestamp: String,
    },
    /// Upstream `type: "branch_summary"`.
    BranchSummary {
        id: String,
        parent_id: Option<String>,
        from_id: String,
        summary: String,
        details: Option<serde_json::Value>,
        from_hook: bool,
        timestamp: String,
    },
    /// Upstream `type: "compaction"`.
    Compaction {
        id: String,
        parent_id: Option<String>,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: i64,
        details: Option<serde_json::Value>,
        from_hook: bool,
        system_message: Option<SystemMessage>,
        timestamp: String,
    },
    /// Entry kinds that do not participate in compaction context
    /// (`thinking_level_change`, `model_change`, `custom`, `label`,
    /// `session_info`).
    Other,
}

impl SessionEntry {
    /// The entry id; `None` for kinds compaction never addresses.
    pub fn id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message { id, .. }
            | SessionEntry::CustomMessage { id, .. }
            | SessionEntry::BranchSummary { id, .. }
            | SessionEntry::Compaction { id, .. } => Some(id),
            SessionEntry::Other => None,
        }
    }

    /// The entry's tree parent (upstream `parentId: string | null`).
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message { parent_id, .. }
            | SessionEntry::CustomMessage { parent_id, .. }
            | SessionEntry::BranchSummary { parent_id, .. }
            | SessionEntry::Compaction { parent_id, .. } => parent_id.as_deref(),
            SessionEntry::Other => None,
        }
    }
}

/// Serialize a typed custom message struct into a
/// [`CustomAgentMessage`] payload (upstream declaration-merged message
/// objects). `None` when the value is not a JSON object or fails to
/// serialize (unrepresentable upstream — the types are compile-time there).
fn custom_agent_message(role: &str, value: &impl serde::Serialize) -> Option<CustomAgentMessage> {
    let value = serde_json::to_value(value).ok()?;
    let data = match value {
        serde_json::Value::Object(map) => map,
        _ => return None,
    };
    Some(CustomAgentMessage {
        role: role.to_string(),
        data,
    })
}

/// Upstream `sessionEntryToContextMessages` (`session-manager.ts:392-421`),
/// vendored: project one session entry into LLM/runtime messages. Plain
/// `custom`/`label`/`session_info`/`thinking_level_change`/`model_change`
/// entries project to nothing.
///
/// Upstream null/missing-content normalization (session files parsed without
/// validation) is unrepresentable here — the ported `AgentMessage` content
/// fields are non-optional (disclosed).
fn session_entry_to_context_messages(entry: &SessionEntry) -> Vec<AgentMessage> {
    match entry {
        SessionEntry::Message { message, .. } => vec![message.clone()],
        SessionEntry::CustomMessage {
            custom_type,
            content,
            display,
            details,
            timestamp,
            ..
        } => create_custom_message(
            custom_type,
            content.clone(),
            *display,
            details.clone(),
            timestamp,
        )
        .and_then(|message| custom_agent_message("custom", &message))
        .map(AgentMessage::Custom)
        .into_iter()
        .collect(),
        SessionEntry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } if !summary.is_empty() => create_branch_summary_message(summary, from_id, timestamp)
            .and_then(|message| custom_agent_message("branchSummary", &message))
            .map(AgentMessage::Custom)
            .into_iter()
            .collect(),
        SessionEntry::BranchSummary { .. } => Vec::new(),
        SessionEntry::Compaction {
            summary,
            tokens_before,
            system_message,
            timestamp,
            ..
        } => {
            let mut messages: Vec<AgentMessage> = Vec::new();
            if let Some(system) = system_message {
                messages.push(AgentMessage::System(system.clone()));
            }
            if let Some(message) =
                create_compaction_summary_message(summary, *tokens_before, timestamp)
                    .and_then(|message| custom_agent_message("compactionSummary", &message))
            {
                messages.push(AgentMessage::Custom(message));
            }
            messages
        }
        SessionEntry::Other => Vec::new(),
    }
}

/// Upstream `buildSessionPath` (`session-manager.ts:343-369`) with no leaf id:
/// walk the parent chain from the newest entry to the root, root-first.
fn build_session_path(entries: &[SessionEntry]) -> Vec<&SessionEntry> {
    let Some(leaf) = entries.last() else {
        return Vec::new();
    };
    let index: HashMap<&str, &SessionEntry> = entries
        .iter()
        .filter_map(|entry| entry.id().map(|id| (id, entry)))
        .collect();
    let mut path = Vec::new();
    let mut current: Option<&SessionEntry> = Some(leaf);
    while let Some(entry) = current {
        path.push(entry);
        current = entry
            .parent_id()
            .and_then(|parent_id| index.get(parent_id).copied());
    }
    path.reverse();
    path
}

/// Upstream `buildContextEntries` (`session-manager.ts:438-470`): the active,
/// compaction-aware entry list — the latest compaction on the path, the kept
/// entries from `firstKeptEntryId` up to it (minus message-role system
/// entries), then everything after the compaction entry.
fn build_context_entries<'a>(path: &[&'a SessionEntry]) -> Vec<&'a SessionEntry> {
    let Some(compaction) = path
        .iter()
        .rev()
        .find(|entry| matches!(**entry, SessionEntry::Compaction { .. }))
        .copied()
    else {
        return path.to_vec();
    };
    let Some(compaction_idx) = path.iter().position(|entry| entry.id() == compaction.id()) else {
        return path.to_vec();
    };
    let SessionEntry::Compaction {
        first_kept_entry_id,
        ..
    } = compaction
    else {
        return path.to_vec();
    };

    let mut context_entries: Vec<&SessionEntry> = vec![compaction];
    let mut found_first_kept = false;
    for entry in path[..compaction_idx].iter().copied() {
        if entry.id() == Some(first_kept_entry_id.as_str()) {
            found_first_kept = true;
        }
        if found_first_kept
            && !matches!(
                entry,
                SessionEntry::Message { message, .. } if message.role() == "system"
            )
        {
            context_entries.push(entry);
        }
    }
    context_entries.extend(path[compaction_idx + 1..].iter().copied());
    context_entries
}

/// Upstream `buildSessionContext(pathEntries).messages`
/// (`session-manager.ts:472-481`), vendored for `prepareCompaction`'s
/// `tokensBefore`. The `thinkingLevel`/`model` settings projection belongs to
/// the session-manager slice and is not carried by the seam.
fn build_session_context_messages(path_entries: &[SessionEntry]) -> Vec<AgentMessage> {
    build_context_entries(&build_session_path(path_entries))
        .iter()
        .flat_map(|entry| session_entry_to_context_messages(entry))
        .collect()
}

// ============================================================================
// File Operation Tracking (upstream compaction/compaction.ts)
// ============================================================================

/// Details stored in compaction results/entries for file tracking (upstream
/// `CompactionDetails`, `compaction.ts:47-50`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// Read a defensively-parsed string array off a details JSON value (upstream
/// checks `Array.isArray` per field; wrong-typed or missing fields collect as
/// empty).
fn details_string_array(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
        .unwrap_or_default()
}

/// Parse the pi-generated file-tracking details from a compaction entry's
/// `details` JSON (upstream reads the fields defensively after an
/// `Array.isArray` check).
fn parse_compaction_details(details: Option<&serde_json::Value>) -> CompactionDetails {
    let Some(details) = details else {
        return CompactionDetails {
            read_files: Vec::new(),
            modified_files: Vec::new(),
        };
    };
    CompactionDetails {
        read_files: details_string_array(details.get("readFiles")),
        modified_files: details_string_array(details.get("modifiedFiles")),
    }
}

/// Upstream `extractFileOperations` (`compaction.ts:55-83`): collect file
/// operations from the previous compaction's details (if pi-generated) and
/// from tool calls in the summarized messages.
fn extract_file_operations(
    messages: &[AgentMessage],
    entries: &[SessionEntry],
    prev_compaction_index: Option<usize>,
) -> FileOperations {
    let mut file_ops = FileOperations::new();

    if let Some(index) = prev_compaction_index {
        if let SessionEntry::Compaction {
            from_hook, details, ..
        } = &entries[index]
        {
            // `fromHook: true` marks extension-generated entries whose details
            // must not feed pi's tracking (field kept for file compatibility).
            if !from_hook {
                let parsed = parse_compaction_details(details.as_ref());
                for file in &parsed.read_files {
                    file_ops.read.insert(file.clone());
                }
                for file in &parsed.modified_files {
                    file_ops.edited.insert(file.clone());
                }
            }
        }
    }

    for message in messages {
        extract_file_ops_from_message(message, &mut file_ops);
    }

    file_ops
}

// ============================================================================
// Message Extraction
// ============================================================================

/// Upstream `getMessageFromEntryForCompaction` (`compaction.ts:93-100`):
/// extract the context message an entry contributes, if any. Compaction
/// entries never re-summarize; system messages are prompt state.
fn get_message_from_entry_for_compaction(entry: &SessionEntry) -> Option<AgentMessage> {
    if matches!(entry, SessionEntry::Compaction { .. }) {
        return None;
    }
    let message = session_entry_to_context_messages(entry)
        .into_iter()
        .next()?;
    if message.role() == "system" {
        None
    } else {
        Some(message)
    }
}

/// Result from `compact()` — upstream `CompactionResult<T = unknown>`
/// (`compaction.ts:103-112`) at its default instantiation; session managers
/// add uuid/parentUuid when saving.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    /// Upstream `estimatedTokensAfter?: number` (never set by `compact`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_tokens_after: Option<u64>,
    /// Usage from the LLM call(s) that generated this summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// File lists this compaction range produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<CompactionDetails>,
}

/// Upstream `combineUsage` (`compaction.ts:114-135`): componentwise sums;
/// `cacheWrite1h`/`reasoning` are present on the result exactly when present
/// on either input (`(left ?? 0) + (right ?? 0)`).
fn combine_usage(first: Usage, second: Usage) -> Usage {
    let cache_write_1h = match (first.cache_write_1h, second.cache_write_1h) {
        (None, None) => None,
        (left, right) => Some(left.unwrap_or(0).saturating_add(right.unwrap_or(0))),
    };
    let reasoning = match (first.reasoning, second.reasoning) {
        (None, None) => None,
        (left, right) => Some(left.unwrap_or(0).saturating_add(right.unwrap_or(0))),
    };
    Usage {
        input: first.input.saturating_add(second.input),
        output: first.output.saturating_add(second.output),
        cache_read: first.cache_read.saturating_add(second.cache_read),
        cache_write: first.cache_write.saturating_add(second.cache_write),
        cache_write_1h,
        reasoning,
        total_tokens: first.total_tokens.saturating_add(second.total_tokens),
        cost: UsageCost {
            input: first.cost.input + second.cost.input,
            output: first.cost.output + second.cost.output,
            cache_read: first.cost.cache_read + second.cost.cache_read,
            cache_write: first.cost.cache_write + second.cost.cache_write,
            total: first.cost.total + second.cost.total,
        },
    }
}

// ============================================================================
// Types
// ============================================================================

/// Upstream `CompactionSettings` (`compaction.ts:141-145`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

/// Upstream `DEFAULT_COMPACTION_SETTINGS` (`compaction.ts:147-151`).
pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16_384,
    keep_recent_tokens: 20_000,
};

// ============================================================================
// Token calculation
// ============================================================================

/// Upstream `calculateContextTokens` (`compaction.ts:161-163`): the
/// provider-reported total, falling back to the component sum when the total
/// is zero (JS `||`).
pub fn calculate_context_tokens(usage: Usage) -> u64 {
    if usage.total_tokens != 0 {
        usage.total_tokens
    } else {
        usage
            .input
            .saturating_add(usage.output)
            .saturating_add(usage.cache_read)
            .saturating_add(usage.cache_write)
    }
}

/// Upstream `getAssistantUsage` (`compaction.ts:169-182`): usage from a
/// settled assistant message whose context total is positive.
fn get_assistant_usage(message: &AgentMessage) -> Option<Usage> {
    let AgentMessage::Assistant(assistant) = message else {
        return None;
    };
    if assistant.stop_reason != StopReason::Aborted
        && assistant.stop_reason != StopReason::Error
        && calculate_context_tokens(assistant.usage) > 0
    {
        return Some(assistant.usage);
    }
    None
}

/// Upstream `getLastAssistantUsage` (`compaction.ts:187-196`): usage from the
/// last valid assistant message in session entries.
pub fn get_last_assistant_usage(entries: &[SessionEntry]) -> Option<Usage> {
    for entry in entries.iter().rev() {
        if let SessionEntry::Message { message, .. } = entry {
            if let Some(usage) = get_assistant_usage(message) {
                return Some(usage);
            }
        }
    }
    None
}

/// Upstream `ContextUsageEstimate` (`compaction.ts:198-203`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsageEstimate {
    pub tokens: u64,
    pub usage_tokens: u64,
    pub trailing_tokens: u64,
    /// Index of the message that provided usage (`None` when none exists;
    /// upstream `null`).
    pub last_usage_index: Option<usize>,
}

/// Upstream `getLastAssistantUsageInfo` (`compaction.ts:205-211`).
fn get_last_assistant_usage_info(messages: &[AgentMessage]) -> Option<(Usage, usize)> {
    for (index, message) in messages.iter().enumerate().rev() {
        if let Some(usage) = get_assistant_usage(message) {
            return Some((usage, index));
        }
    }
    None
}

/// Upstream `estimateContextTokens` (`compaction.ts:217-245`): provider usage
/// for the latest valid assistant message plus character estimates for the
/// messages after it.
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> ContextUsageEstimate {
    let Some((usage, index)) = get_last_assistant_usage_info(messages) else {
        let mut estimated = 0u64;
        for message in messages {
            estimated = estimated.saturating_add(estimate_tokens(message));
        }
        return ContextUsageEstimate {
            tokens: estimated,
            usage_tokens: 0,
            trailing_tokens: estimated,
            last_usage_index: None,
        };
    };

    let usage_tokens = calculate_context_tokens(usage);
    let mut trailing_tokens = 0u64;
    for message in &messages[index + 1..] {
        trailing_tokens = trailing_tokens.saturating_add(estimate_tokens(message));
    }
    ContextUsageEstimate {
        tokens: usage_tokens.saturating_add(trailing_tokens),
        usage_tokens,
        trailing_tokens,
        last_usage_index: Some(index),
    }
}

/// Upstream `shouldCompact` (`compaction.ts:250-253`): whether context usage
/// exceeds the configured compaction threshold. The comparison is i128 to
/// keep the JS subtraction semantics when `reserveTokens` exceeds the context
/// window.
pub fn should_compact(
    context_tokens: u64,
    context_window: u64,
    settings: CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens as i128 > context_window as i128 - settings.reserve_tokens as i128
}

// ============================================================================
// Cut point detection
// ============================================================================

/// Upstream `ESTIMATED_IMAGE_CHARS` (`compaction.ts:259`).
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// Upstream `estimateTextAndImageContentChars` (`compaction.ts:261-275`).
fn estimate_text_and_image_content_chars(content: &StringOrBlocks) -> usize {
    match content {
        StringOrBlocks::Text(text) => text.chars().count(),
        StringOrBlocks::Blocks(blocks) => {
            let mut chars = 0;
            for block in blocks {
                match block {
                    TextOrImageBlock::Text(text) => chars += text.text.chars().count(),
                    TextOrImageBlock::Image(_) => chars += ESTIMATED_IMAGE_CHARS,
                }
            }
            chars
        }
    }
}

/// Upstream `estimateTokens` (`compaction.ts:281-321`): conservative
/// `chars / 4` heuristic over every supported message role (overestimates).
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let chars: usize = match message {
        AgentMessage::User(user) => {
            return (estimate_text_and_image_content_chars(&user.content) as u64).div_ceil(4);
        }
        AgentMessage::Assistant(assistant) => {
            let mut chars = 0;
            for block in &assistant.content {
                match block {
                    AssistantBlock::Text(text) => chars += text.text.chars().count(),
                    AssistantBlock::Thinking(thinking) => {
                        chars += thinking.thinking.chars().count()
                    }
                    AssistantBlock::ToolCall(tool_call) => {
                        chars += tool_call.name.chars().count()
                            + compact_json(&tool_call.arguments).chars().count();
                    }
                }
            }
            chars
        }
        AgentMessage::ToolResult(tool_result) => {
            let mut chars = 0;
            for block in &tool_result.content {
                match block {
                    TextOrImageBlock::Text(text) => chars += text.text.chars().count(),
                    TextOrImageBlock::Image(_) => chars += ESTIMATED_IMAGE_CHARS,
                }
            }
            chars
        }
        AgentMessage::Custom(custom) => match custom.role.as_str() {
            "bashExecution" => {
                let command = custom.data.get("command").and_then(|v| v.as_str());
                let output = custom.data.get("output").and_then(|v| v.as_str());
                command.map(|c| c.chars().count()).unwrap_or(0)
                    + output.map(|o| o.chars().count()).unwrap_or(0)
            }
            "branchSummary" | "compactionSummary" => custom
                .data
                .get("summary")
                .and_then(|v| v.as_str())
                .map(|summary| summary.chars().count())
                .unwrap_or(0),
            "custom" => {
                let content = custom.data.get("content");
                match content {
                    Some(serde_json::Value::String(text)) => text.chars().count(),
                    Some(serde_json::Value::Array(blocks)) => blocks
                        .iter()
                        .map(|block| match block {
                            serde_json::Value::Object(fields) => {
                                let block_type = fields.get("type").and_then(|v| v.as_str());
                                let text = fields.get("text").and_then(|v| v.as_str());
                                match (block_type, text) {
                                    (Some("text"), Some(text)) => text.chars().count(),
                                    (Some("image"), _) => ESTIMATED_IMAGE_CHARS,
                                    _ => 0,
                                }
                            }
                            _ => 0,
                        })
                        .sum(),
                    _ => 0,
                }
            }
            _ => 0,
        },
        AgentMessage::System(_) => 0,
    };
    (chars as u64).div_ceil(4)
}

/// Upstream `isCutPointMessage` (`compaction.ts:323-336`).
fn is_cut_point_message(message: &AgentMessage) -> bool {
    !matches!(message.role(), "toolResult" | "system")
}

/// Upstream `isTurnStartMessage` (`compaction.ts:338-351`).
fn is_turn_start_message(message: &AgentMessage) -> bool {
    matches!(
        message.role(),
        "user" | "bashExecution" | "custom" | "branchSummary" | "compactionSummary"
    )
}

/// Upstream `isTurnStartEntry` (`compaction.ts:353-358`).
fn is_turn_start_entry(entry: &SessionEntry) -> bool {
    if matches!(entry, SessionEntry::Compaction { .. }) {
        return false;
    }
    session_entry_to_context_messages(entry)
        .iter()
        .any(is_turn_start_message)
}

/// Upstream `findValidCutPoints` (`compaction.ts:366-378`): indices of
/// context-visible cuttable entries. Never cuts at tool results (they must
/// follow their tool call).
fn find_valid_cut_points(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
) -> Vec<usize> {
    let mut cut_points = Vec::new();
    for (index, entry) in entries.iter().enumerate().take(end_index).skip(start_index) {
        if matches!(entry, SessionEntry::Compaction { .. }) {
            continue;
        }
        if session_entry_to_context_messages(entry)
            .iter()
            .any(is_cut_point_message)
        {
            cut_points.push(index);
        }
    }
    cut_points
}

/// Upstream `findTurnStartIndex` (`compaction.ts:384-391`): the
/// context-visible turn-start entry that opens the turn containing
/// `entry_index` (`None` when none precedes it; upstream `-1`).
pub fn find_turn_start_index(
    entries: &[SessionEntry],
    entry_index: usize,
    start_index: usize,
) -> Option<usize> {
    (start_index..=entry_index)
        .rev()
        .find(|&index| is_turn_start_entry(&entries[index]))
}

/// Upstream `CutPointResult` (`compaction.ts:393-400`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutPointResult {
    /// Index of the first entry retained after compaction.
    pub first_kept_entry_index: usize,
    /// Index of the turn-start entry when the cut splits a turn (`None`
    /// otherwise; upstream `-1`).
    pub turn_start_index: Option<usize>,
    /// Whether the cut lands in the middle of a turn.
    pub is_split_turn: bool,
}

/// Upstream `findCutPoint` (`compaction.ts:418-476`): walk backwards from the
/// newest entry accumulating per-message estimates until the recent-token
/// budget is crossed, take the first valid cut point at or after that entry,
/// then slide back over adjacent metadata entries that do not affect context.
pub fn find_cut_point(
    entries: &[SessionEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);

    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    }

    let mut accumulated_tokens = 0u64;
    let mut cut_index = cut_points[0];

    for index in (start_index..end_index).rev() {
        let entry = &entries[index];
        let message_tokens = session_entry_to_context_messages(entry)
            .iter()
            .map(estimate_tokens)
            .fold(0u64, |sum, tokens| sum.saturating_add(tokens));
        if message_tokens == 0 {
            continue;
        }
        accumulated_tokens = accumulated_tokens.saturating_add(message_tokens);

        if accumulated_tokens >= keep_recent_tokens {
            for &candidate in &cut_points {
                if candidate >= index {
                    cut_index = candidate;
                    break;
                }
            }
            break;
        }
    }

    while cut_index > start_index {
        let prev_entry = &entries[cut_index - 1];
        if matches!(prev_entry, SessionEntry::Compaction { .. })
            || !session_entry_to_context_messages(prev_entry).is_empty()
        {
            break;
        }
        cut_index -= 1;
    }

    let starts_turn = is_turn_start_entry(&entries[cut_index]);
    let turn_start_index = if starts_turn {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !starts_turn && turn_start_index.is_some(),
    }
}

// ============================================================================
// Summarization
// ============================================================================

/// Upstream `SUMMARIZATION_PROMPT` (`compaction.ts:482-513`).
const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Upstream `UPDATE_SUMMARIZATION_INSTRUCTIONS` (`compaction.ts:515-550`).
const UPDATE_SUMMARIZATION_INSTRUCTIONS: &str = "Update the existing structured summary with new information. RULES:\n- PRESERVE all existing information from the previous summary\n- ADD new progress, decisions, and context from the new messages\n- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed\n- UPDATE \"Next Steps\" based on what was accomplished\n- PRESERVE exact file paths, function names, and error messages\n- If something is no longer relevant, you may remove it\n\nUse this EXACT format:\n\n## Goal\n[Preserve existing goals, add new ones if the task expanded]\n\n## Constraints & Preferences\n- [Preserve existing, add new ones discovered]\n\n## Progress\n### Done\n- [x] [Include previously done items AND newly completed items]\n\n### In Progress\n- [ ] [Current work - update based on progress]\n\n### Blocked\n- [Current blockers - remove if resolved]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale] (preserve all previous, add new)\n\n## Next Steps\n1. [Update based on current state]\n\n## Critical Context\n- [Preserve important context, add new if needed]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Upstream `UPDATE_SUMMARIZATION_PROMPT` (`compaction.ts:552-554`): the
/// header above `UPDATE_SUMMARIZATION_INSTRUCTIONS`.
fn update_summarization_prompt() -> String {
    format!(
        "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\n{UPDATE_SUMMARIZATION_INSTRUCTIONS}"
    )
}

/// Upstream `getSummarizationFailure` (`compaction.ts:560-568`): an error
/// message when a summarization response cannot safely be persisted. A length
/// stop contains partial text and must not become a session checkpoint.
pub fn get_summarization_failure(response: &AssistantMessage, label: &str) -> Option<String> {
    if response.stop_reason == StopReason::Error {
        return Some(format!(
            "{label} failed: {}",
            response.error_message.as_deref().unwrap_or("Unknown error")
        ));
    }
    if response.stop_reason == StopReason::Length {
        return Some(format!(
            "{label} failed: generation hit the token cap and the summary is incomplete"
        ));
    }
    None
}

/// Upstream `createSummarizationOptions` (`compaction.ts:570-585`): the
/// per-request options for one summarization call — thinking level only for
/// reasoning-capable models with thinking not off. Argument list mirrors the
/// upstream function one-to-one.
#[allow(clippy::too_many_arguments)]
fn create_summarization_options(
    model: &Model,
    max_tokens: u64,
    api_key: Option<&str>,
    headers: Option<&ProviderHeaders>,
    env: Option<&ProviderEnv>,
    signal: Option<&tokio_util::sync::CancellationToken>,
    thinking_level: Option<ThinkingLevel>,
    session_id: Option<&str>,
) -> SimpleStreamOptions {
    let mut options = SimpleStreamOptions {
        stream: crate::ai::types::StreamOptions {
            max_tokens: Some(max_tokens),
            signal: signal.cloned(),
            api_key: api_key.map(str::to_string),
            headers: headers.cloned(),
            env: env.cloned(),
            session_id: session_id.map(str::to_string),
            ..crate::ai::types::StreamOptions::default()
        },
        ..SimpleStreamOptions::default()
    };
    if model.reasoning {
        if let Some(level) = thinking_level.filter(|level| *level != ThinkingLevel::Off) {
            options.reasoning = Some(match level {
                ThinkingLevel::Minimal => RequestThinkingLevel::Minimal,
                ThinkingLevel::Low => RequestThinkingLevel::Low,
                ThinkingLevel::Medium => RequestThinkingLevel::Medium,
                ThinkingLevel::High => RequestThinkingLevel::High,
                ThinkingLevel::Xhigh => RequestThinkingLevel::Xhigh,
                ThinkingLevel::Max => RequestThinkingLevel::Max,
                ThinkingLevel::Off => unreachable!("filtered above"),
            });
        }
    }
    options
}

/// Seam for upstream `StreamFn` (`pi-agent-core`): one LLM transport call —
/// `(model, context, options) => AssistantMessage` (upstream resolves the
/// stream's `.result()`; the port's transports complete synchronously).
pub type StreamFn = Arc<
    dyn Fn(Model, TranscriptContext, SimpleStreamOptions) -> BoxFuture<'static, AssistantMessage>
        + Send
        + Sync,
>;

/// Bind a [`Models`] collection as the default [`StreamFn`] transport — the
/// upstream `completeSimple` fallback of `completeSummarization`. Upstream
/// reaches a global default registry; the Rust ai layer has none, so callers
/// supply the collection.
pub fn models_stream_fn(models: Arc<Models>) -> StreamFn {
    Arc::new(move |model, context, options| {
        let models = models.clone();
        let context = AiContext {
            system_prompt: None,
            messages: context.messages().to_vec(),
            tools: None,
        };
        Box::pin(async move {
            models
                .complete_simple(
                    &model,
                    &context,
                    Some(ModelsSimpleStreamOptions {
                        simple: options,
                        transform_headers: None,
                    }),
                )
                .await
        })
    })
}

fn now_epoch_millis() -> i64 {
    crate::ai::now_ms()
}

/// An error-stop assistant message for the unbound default transport (see the
/// module-level `StreamFn` seam disclosure).
fn unbound_transport_message() -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(
            "compaction transport unbound: pass models_stream_fn(models) as the stream_fn seam"
                .to_string(),
        ),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_epoch_millis(),
    }
}

/// Upstream `completeSummarization` (`compaction.ts:594-614`): shared choke
/// point for every compaction/branch-summary summarization call. Wraps the
/// single LLM call in
/// [`retry_assistant_call`](crate::ai::retry::retry_assistant_call) so
/// transient stream drops honor the configured retry policy. Deterministic
/// errors and aborts return immediately. Avoids cache writes for one-off
/// summaries; callers without a session id (including branch summaries)
/// receive a fresh routing id.
pub async fn complete_summarization(
    model: &Model,
    context: TranscriptContext,
    options: SimpleStreamOptions,
    stream_fn: Option<StreamFn>,
    retry: Option<&RetryPolicy>,
    callbacks: &mut RetryCallbacks,
) -> AssistantMessage {
    let mut request_options = options.clone();
    request_options.stream.cache_retention = Some(CacheRetention::None);
    request_options.stream.session_id =
        Some(options.stream.session_id.clone().unwrap_or_else(uuid_v7));
    let signal = request_options.stream.signal.clone();

    let produce_stream_fn = stream_fn.clone();
    let produce_model = model.clone();
    let produce_context = context.clone();
    retry_assistant_call(
        move || {
            let stream_fn = produce_stream_fn.clone();
            let model = produce_model.clone();
            let context = produce_context.clone();
            let options = request_options.clone();
            async move {
                match stream_fn {
                    Some(stream_fn) => stream_fn(model, context, options).await,
                    None => unbound_transport_message(),
                }
            }
        },
        retry,
        signal.as_ref(),
        callbacks,
    )
    .await
}

/// Parameter struct for [`generate_summary`] / [`generate_summary_with_usage`]
/// (upstream passes fourteen positional arguments).
pub struct SummaryRequest<'a> {
    /// Messages to summarize.
    pub messages: &'a [AgentMessage],
    pub model: &'a Model,
    pub reserve_tokens: u64,
    pub api_key: Option<&'a str>,
    pub headers: Option<&'a ProviderHeaders>,
    pub signal: Option<&'a tokio_util::sync::CancellationToken>,
    pub custom_instructions: Option<&'a str>,
    /// Merge into the previous summary instead of writing an initial one.
    pub previous_summary: Option<&'a str>,
    pub thinking_level: Option<ThinkingLevel>,
    pub stream_fn: Option<StreamFn>,
    pub env: Option<&'a ProviderEnv>,
    pub retry: Option<&'a RetryPolicy>,
    pub callbacks: &'a mut RetryCallbacks,
    pub session_id: Option<&'a str>,
}

/// Upstream `generateSummary` (`compaction.ts:620-654`): the summary text.
pub async fn generate_summary(request: SummaryRequest<'_>) -> Result<String, String> {
    Ok(generate_summary_with_usage(request).await?.text)
}

/// Upstream `buildSummarizationContext` (`compaction.ts:657-668`).
fn build_summarization_context(prompt_text: String) -> TranscriptContext {
    normalize_context(&AiContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::User(UserMessage {
            content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                text: prompt_text,
                text_signature: None,
            })]),
            timestamp: now_epoch_millis(),
        })],
        tools: None,
    })
}

/// Generated summary text plus its provider usage (upstream anonymous
/// `{ text, usage }` return of `generateSummaryWithUsage`).
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryWithUsage {
    pub text: String,
    pub usage: Usage,
}

/// Upstream `generateSummaryWithUsage` (`compaction.ts:671-741`): generate or
/// update a conversation summary and return its provider usage.
pub async fn generate_summary_with_usage(
    request: SummaryRequest<'_>,
) -> Result<SummaryWithUsage, String> {
    let SummaryRequest {
        messages,
        model,
        reserve_tokens,
        api_key,
        headers,
        signal,
        custom_instructions,
        previous_summary,
        thinking_level,
        stream_fn,
        env,
        retry,
        callbacks,
        session_id,
    } = request;

    let max_tokens = ((0.8 * reserve_tokens as f64).floor() as u64).min(if model.max_tokens > 0 {
        model.max_tokens
    } else {
        u64::MAX
    });

    let mut base_prompt = if previous_summary.is_some() {
        update_summarization_prompt()
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };
    if let Some(custom_instructions) = custom_instructions {
        base_prompt = format!("{base_prompt}\n\nAdditional focus: {custom_instructions}");
    }

    // Serialize the conversation to text so the model doesn't try to continue
    // it. Convert to LLM messages first (handles custom message types).
    let llm_messages = convert_to_llm(messages);
    let conversation_text = serialize_conversation(&llm_messages);

    let mut prompt_text = format!("<conversation>\n{conversation_text}\n</conversation>\n\n");
    if let Some(previous_summary) = previous_summary {
        prompt_text += &format!("<previous-summary>\n{previous_summary}\n</previous-summary>\n\n");
    }
    prompt_text += &base_prompt;

    let completion_options = create_summarization_options(
        model,
        max_tokens,
        api_key,
        headers,
        env,
        signal,
        thinking_level,
        session_id,
    );

    let response = complete_summarization(
        model,
        build_summarization_context(prompt_text),
        completion_options,
        stream_fn,
        retry,
        callbacks,
    )
    .await;

    if let Some(failure) = get_summarization_failure(&response, "Summarization") {
        return Err(failure);
    }
    if response
        .content
        .iter()
        .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
    {
        return Err("Summarization attempted to call a tool".to_string());
    }

    Ok(SummaryWithUsage {
        text: assistant_blocks_text(&response.content, "\n"),
        usage: response.usage,
    })
}

// ============================================================================
// Compaction Preparation (for extensions)
// ============================================================================

/// Upstream `CompactionPreparation` (`compaction.ts:747-763`): pre-calculated
/// data for one compaction run.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionPreparation {
    /// UUID of the first entry to keep.
    pub first_kept_entry_id: String,
    /// Messages that will be summarized and discarded.
    pub messages_to_summarize: Vec<AgentMessage>,
    /// Messages that will be turned into the turn prefix summary (if
    /// splitting).
    pub turn_prefix_messages: Vec<AgentMessage>,
    /// Whether this is a split turn (cut point in the middle of a turn).
    pub is_split_turn: bool,
    pub tokens_before: u64,
    /// Summary from the previous compaction, for iterative update.
    pub previous_summary: Option<String>,
    /// File operations extracted from `messages_to_summarize`.
    pub file_ops: FileOperations,
    /// Compaction settings from settings.jsonl.
    pub settings: CompactionSettings,
}

/// Upstream `prepareCompaction` (`compaction.ts:765-844`): compute the next
/// compaction range for a session path. `None` when a trailing compaction
/// already ended the path or there is nothing valid to compact.
pub fn prepare_compaction(
    path_entries: &[SessionEntry],
    settings: CompactionSettings,
) -> Option<CompactionPreparation> {
    if let Some(last) = path_entries.last() {
        if matches!(last, SessionEntry::Compaction { .. }) {
            return None;
        }
    }

    let mut prev_compaction_index: Option<usize> = None;
    for (index, entry) in path_entries.iter().enumerate().rev() {
        if matches!(entry, SessionEntry::Compaction { .. }) {
            prev_compaction_index = Some(index);
            break;
        }
    }

    let mut previous_summary: Option<String> = None;
    let mut boundary_start = 0usize;
    if let Some(index) = prev_compaction_index {
        if let SessionEntry::Compaction {
            summary,
            first_kept_entry_id,
            ..
        } = &path_entries[index]
        {
            previous_summary = Some(summary.clone());
            let first_kept_entry_index = path_entries
                .iter()
                .position(|entry| entry.id() == Some(first_kept_entry_id.as_str()));
            boundary_start = first_kept_entry_index.unwrap_or(index + 1);
        }
    }
    let boundary_end = path_entries.len();

    let tokens_before =
        estimate_context_tokens(&build_session_context_messages(path_entries)).tokens;

    let cut_point = find_cut_point(
        path_entries,
        boundary_start,
        boundary_end,
        settings.keep_recent_tokens,
    );

    // Upstream: `pathEntries[cutPoint.firstKeptEntryIndex]` with a missing
    // entry (empty path or out of range) has no id -> session needs migration
    // (return undefined).
    let first_kept_entry = path_entries.get(cut_point.first_kept_entry_index)?;
    // Seam entries always carry ids; an empty id keeps the falsy-id check.
    let first_kept_entry_id = first_kept_entry.id().unwrap_or("").to_string();

    let history_end = if cut_point.is_split_turn {
        cut_point.turn_start_index
    } else {
        Some(cut_point.first_kept_entry_index)
    }
    .unwrap_or(boundary_end);

    let mut messages_to_summarize: Vec<AgentMessage> = Vec::new();
    if boundary_start <= history_end {
        for entry in &path_entries[boundary_start..history_end] {
            if let Some(message) = get_message_from_entry_for_compaction(entry) {
                messages_to_summarize.push(message);
            }
        }
    }

    let mut turn_prefix_messages: Vec<AgentMessage> = Vec::new();
    if cut_point.is_split_turn {
        let turn_start = cut_point.turn_start_index.unwrap_or(0);
        if turn_start <= cut_point.first_kept_entry_index {
            for entry in &path_entries[turn_start..cut_point.first_kept_entry_index] {
                if let Some(message) = get_message_from_entry_for_compaction(entry) {
                    turn_prefix_messages.push(message);
                }
            }
        }
    }

    if messages_to_summarize.is_empty() && turn_prefix_messages.is_empty() {
        return None;
    }

    let mut file_ops =
        extract_file_operations(&messages_to_summarize, path_entries, prev_compaction_index);

    if cut_point.is_split_turn {
        for message in &turn_prefix_messages {
            extract_file_ops_from_message(message, &mut file_ops);
        }
    }

    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings,
    })
}

// ============================================================================
// Main compaction function
// ============================================================================

/// Upstream `TURN_PREFIX_SUMMARIZATION_PROMPT` (`compaction.ts:850-863`).
const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.\n\nSummarize the prefix to provide context for the retained suffix:\n\n## Original Request\n[What did the user ask for in this turn?]\n\n## Early Progress\n- [Key decisions and work done in the prefix]\n\n## Context for Suffix\n- [Information needed to understand the retained recent work]\n\nBe concise. Focus on what's needed to understand the kept suffix.";

/// Parameter struct for [`compact`] (upstream passes twelve positional
/// arguments).
pub struct CompactOptions<'a> {
    pub model: &'a Model,
    pub api_key: Option<&'a str>,
    pub headers: Option<&'a ProviderHeaders>,
    /// Optional custom focus for the summary.
    pub custom_instructions: Option<&'a str>,
    pub signal: Option<&'a tokio_util::sync::CancellationToken>,
    pub thinking_level: Option<ThinkingLevel>,
    pub stream_fn: Option<StreamFn>,
    pub env: Option<&'a ProviderEnv>,
    pub retry: Option<&'a RetryPolicy>,
    pub callbacks: &'a mut RetryCallbacks,
    /// Optional routing session id forwarded without enabling prompt caching.
    pub session_id: Option<&'a str>,
}

/// Upstream `compact` (`compaction.ts:873-979`): generate summaries for a
/// prepared compaction and merge them into one result.
pub async fn compact(
    preparation: CompactionPreparation,
    options: CompactOptions<'_>,
) -> Result<CompactionResult, String> {
    let CompactOptions {
        model,
        api_key,
        headers,
        custom_instructions,
        signal,
        thinking_level,
        stream_fn,
        env,
        retry,
        callbacks,
        session_id,
    } = options;

    let CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings,
    } = preparation;

    let summary;
    let summary_usage: Usage;

    if is_split_turn && !turn_prefix_messages.is_empty() {
        let mut history_text = previous_summary
            .clone()
            .unwrap_or_else(|| "No prior history.".to_string());
        let mut history_usage: Option<Usage> = None;
        if !messages_to_summarize.is_empty() {
            let history_result = generate_summary_with_usage(SummaryRequest {
                messages: &messages_to_summarize,
                model,
                reserve_tokens: settings.reserve_tokens,
                api_key,
                headers,
                signal,
                custom_instructions,
                previous_summary: previous_summary.as_deref(),
                thinking_level,
                stream_fn: stream_fn.clone(),
                env,
                retry,
                callbacks: &mut *callbacks,
                session_id,
            })
            .await?;
            history_text = history_result.text;
            history_usage = Some(history_result.usage);
        }
        let turn_prefix_result = generate_turn_prefix_summary(
            &turn_prefix_messages,
            model,
            settings.reserve_tokens,
            api_key,
            headers,
            env,
            signal,
            thinking_level,
            stream_fn,
            retry,
            callbacks,
            session_id,
        )
        .await?;
        summary = format!(
            "{history_text}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
            turn_prefix_result.text
        );
        summary_usage = match history_usage {
            Some(history_usage) => combine_usage(history_usage, turn_prefix_result.usage),
            None => turn_prefix_result.usage,
        };
    } else {
        let result = generate_summary_with_usage(SummaryRequest {
            messages: &messages_to_summarize,
            model,
            reserve_tokens: settings.reserve_tokens,
            api_key,
            headers,
            signal,
            custom_instructions,
            previous_summary: previous_summary.as_deref(),
            thinking_level,
            stream_fn,
            env,
            retry,
            callbacks: &mut *callbacks,
            session_id,
        })
        .await?;
        summary = result.text;
        summary_usage = result.usage;
    }

    let FileLists {
        read_files,
        modified_files,
    } = compute_file_lists(&file_ops);
    let summary = format!(
        "{summary}{}",
        format_file_operations(&read_files, &modified_files)
    );

    if first_kept_entry_id.is_empty() {
        return Err("First kept entry has no UUID - session may need migration".to_string());
    }

    Ok(CompactionResult {
        summary,
        first_kept_entry_id,
        tokens_before,
        estimated_tokens_after: None,
        usage: Some(summary_usage),
        details: Some(CompactionDetails {
            read_files,
            modified_files,
        }),
    })
}

/// Upstream `generateTurnPrefixSummary` (`compaction.ts:984-1027`): a summary
/// for a turn prefix (when splitting a turn), with a smaller output budget.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the upstream parameter list"
)]
async fn generate_turn_prefix_summary(
    messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    api_key: Option<&str>,
    headers: Option<&ProviderHeaders>,
    env: Option<&ProviderEnv>,
    signal: Option<&tokio_util::sync::CancellationToken>,
    thinking_level: Option<ThinkingLevel>,
    stream_fn: Option<StreamFn>,
    retry: Option<&RetryPolicy>,
    callbacks: &mut RetryCallbacks,
    session_id: Option<&str>,
) -> Result<SummaryWithUsage, String> {
    // Smaller budget for the turn prefix.
    let max_tokens = ((0.5 * reserve_tokens as f64).floor() as u64).min(if model.max_tokens > 0 {
        model.max_tokens
    } else {
        u64::MAX
    });
    let llm_messages = convert_to_llm(messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let prompt_text = format!(
        "<conversation>\n{conversation_text}\n</conversation>\n\n{TURN_PREFIX_SUMMARIZATION_PROMPT}"
    );

    let response = complete_summarization(
        model,
        build_summarization_context(prompt_text),
        create_summarization_options(
            model,
            max_tokens,
            api_key,
            headers,
            env,
            signal,
            thinking_level,
            session_id,
        ),
        stream_fn,
        retry,
        callbacks,
    )
    .await;

    if let Some(failure) = get_summarization_failure(&response, "Turn prefix summarization") {
        return Err(failure);
    }
    if response
        .content
        .iter()
        .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
    {
        return Err("Turn prefix summarization attempted to call a tool".to_string());
    }

    Ok(SummaryWithUsage {
        text: assistant_blocks_text(&response.content, "\n"),
        usage: response.usage,
    })
}

// ============================================================================
// Branch summarization (upstream compaction/branch-summarization.ts)
// ============================================================================

/// Seam for upstream `ReadonlySessionManager`: the two reads
/// `collectEntriesForBranchSummary` makes.
pub trait ReadonlySessionManager {
    /// Upstream `getBranch(id)`: the root-first entry path to `id`.
    fn get_branch(&self, id: &str) -> Vec<SessionEntry>;
    /// Upstream `getEntry(id)`.
    fn get_entry(&self, id: &str) -> Option<SessionEntry>;
}

/// Upstream `BranchSummaryResult` (`branch-summarization.ts:34-41`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_files: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aborted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// Branch-summary details (upstream `BranchSummaryDetails`,
// `branch-summarization.ts:44-47`) carry `{ readFiles, modifiedFiles }` and
// are read defensively per field by `prepare_branch_entries` via
// `details_string_array`.

/// Upstream `BranchPreparation` (`branch-summarization.ts:51-58`).
#[derive(Debug, Clone, PartialEq)]
pub struct BranchPreparation {
    /// Messages extracted for summarization, in chronological order.
    pub messages: Vec<AgentMessage>,
    /// File operations extracted from tool calls.
    pub file_ops: FileOperations,
    /// Total estimated tokens in the selected messages.
    pub total_tokens: u64,
}

/// Upstream `CollectEntriesResult` (`branch-summarization.ts:60-65`).
#[derive(Debug, Clone, PartialEq)]
pub struct CollectEntriesResult {
    /// Entries to summarize, in chronological order.
    pub entries: Vec<SessionEntry>,
    /// Common ancestor between the old and new position, if any.
    pub common_ancestor_id: Option<String>,
}

/// Upstream `collectEntriesForBranchSummary`
/// (`branch-summarization.ts:108-146`): walk from `old_leaf_id` back to the
/// common ancestor with `target_id`, collecting entries along the way. Does
/// NOT stop at compaction boundaries — those are included and their summaries
/// become context.
pub fn collect_entries_for_branch_summary(
    session: &dyn ReadonlySessionManager,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> CollectEntriesResult {
    let Some(old_leaf_id) = old_leaf_id else {
        return CollectEntriesResult {
            entries: Vec::new(),
            common_ancestor_id: None,
        };
    };

    let old_path: HashSet<String> = session
        .get_branch(old_leaf_id)
        .iter()
        .filter_map(|entry| entry.id().map(str::to_string))
        .collect();
    let target_path = session.get_branch(target_id);

    // target_path is root-first, so iterate backwards to find the deepest
    // common ancestor.
    let mut common_ancestor_id: Option<String> = None;
    for entry in target_path.iter().rev() {
        if let Some(id) = entry.id() {
            if old_path.contains(id) {
                common_ancestor_id = Some(id.to_string());
                break;
            }
        }
    }

    let mut entries: Vec<SessionEntry> = Vec::new();
    let mut current: Option<String> = Some(old_leaf_id.to_string());
    while let Some(id) = current {
        if common_ancestor_id.as_deref() == Some(id.as_str()) {
            break;
        }
        let Some(entry) = session.get_entry(&id) else {
            break;
        };
        current = entry.parent_id().map(str::to_string);
        entries.push(entry);
    }

    entries.reverse();
    CollectEntriesResult {
        entries,
        common_ancestor_id,
    }
}

/// Upstream `getMessageFromEntry` (`branch-summarization.ts:156-180`):
/// extract the conversation message an entry contributes; unlike
/// `getMessageFromEntryForCompaction` this handles compaction entries (their
/// summaries become context) and skips tool results.
fn get_message_from_entry(entry: &SessionEntry) -> Option<AgentMessage> {
    match entry {
        SessionEntry::Message { message, .. } => {
            if message.role() == "toolResult" {
                None
            } else {
                Some(message.clone())
            }
        }
        SessionEntry::CustomMessage {
            custom_type,
            content,
            display,
            details,
            timestamp,
            ..
        } => create_custom_message(
            custom_type,
            content.clone(),
            *display,
            details.clone(),
            timestamp,
        )
        .and_then(|message| custom_agent_message("custom", &message))
        .map(AgentMessage::Custom),
        SessionEntry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } => create_branch_summary_message(summary, from_id, timestamp)
            .and_then(|message| custom_agent_message("branchSummary", &message))
            .map(AgentMessage::Custom),
        SessionEntry::Compaction {
            summary,
            tokens_before,
            timestamp,
            ..
        } => create_compaction_summary_message(summary, *tokens_before, timestamp)
            .and_then(|message| custom_agent_message("compactionSummary", &message))
            .map(AgentMessage::Custom),
        SessionEntry::Other => None,
    }
}

/// Upstream `prepareBranchEntries` (`branch-summarization.ts:195-247`): walk
/// entries from newest to oldest, adding messages until the token budget is
/// hit, so the most recent context survives an over-long branch. File
/// operations accumulate from every entry (including nested branch-summary
/// details) regardless of the budget.
pub fn prepare_branch_entries(entries: &[SessionEntry], token_budget: u64) -> BranchPreparation {
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut file_ops = FileOperations::new();
    let mut total_tokens = 0u64;

    // First pass: collect file ops from ALL entries, capturing cumulative
    // tracking from pi-generated nested branch summaries.
    for entry in entries {
        if let SessionEntry::BranchSummary {
            from_hook, details, ..
        } = entry
        {
            if !from_hook {
                for file in details_string_array(details.as_ref().and_then(|d| d.get("readFiles")))
                {
                    file_ops.read.insert(file);
                }
                // Modified files land in `edited` for proper deduplication.
                for file in
                    details_string_array(details.as_ref().and_then(|d| d.get("modifiedFiles")))
                {
                    file_ops.edited.insert(file);
                }
            }
        }
    }

    // Second pass: newest to oldest, respecting the budget.
    for entry in entries.iter().rev() {
        let Some(message) = get_message_from_entry(entry) else {
            continue;
        };
        extract_file_ops_from_message(&message, &mut file_ops);
        let tokens = estimate_tokens(&message);

        if token_budget > 0 && total_tokens + tokens > token_budget {
            // Summary entries still try to fit when the budget is at least
            // 90% free — they are important context.
            if matches!(
                entry,
                SessionEntry::Compaction { .. } | SessionEntry::BranchSummary { .. }
            ) && (total_tokens as f64) < token_budget as f64 * 0.9
            {
                messages.insert(0, message);
                total_tokens += tokens;
            }
            break;
        }

        messages.insert(0, message);
        total_tokens += tokens;
    }

    BranchPreparation {
        messages,
        file_ops,
        total_tokens,
    }
}

/// Upstream `BRANCH_SUMMARY_PREAMBLE` (`branch-summarization.ts:253-256`).
const BRANCH_SUMMARY_PREAMBLE: &str =
    "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n";

/// Upstream `BRANCH_SUMMARY_PROMPT` (`branch-summarization.ts:258-285`).
const BRANCH_SUMMARY_PROMPT: &str = "Create a structured summary of this conversation branch for context when returning later.\n\nUse this EXACT format:\n\n## Goal\n[What was the user trying to accomplish in this branch?]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Work that was started but not finished]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [What should happen next to continue this work]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Parameter struct for [`generate_branch_summary`] (upstream
/// `GenerateBranchSummaryOptions`, `branch-summarization.ts:67-90`).
pub struct GenerateBranchSummaryOptions<'a> {
    pub model: &'a Model,
    pub api_key: Option<&'a str>,
    pub headers: Option<&'a ProviderHeaders>,
    pub env: Option<&'a ProviderEnv>,
    /// Request cancellation (upstream `signal`, required there).
    pub signal: Option<&'a tokio_util::sync::CancellationToken>,
    pub custom_instructions: Option<&'a str>,
    /// Replace the default prompt instead of appending the custom focus.
    pub replace_instructions: bool,
    /// Tokens reserved when selecting branch history (upstream default
    /// 16384).
    pub reserve_tokens: Option<u64>,
    pub stream_fn: Option<StreamFn>,
    /// Retry policy for transient summarization errors.
    pub retry: Option<&'a RetryPolicy>,
    pub callbacks: &'a mut RetryCallbacks,
}

/// Upstream `generateBranchSummary` (`branch-summarization.ts:293-382`):
/// summarize the entries abandoned by a tree-navigation move so context is
/// not lost.
pub async fn generate_branch_summary(
    entries: &[SessionEntry],
    options: GenerateBranchSummaryOptions<'_>,
) -> BranchSummaryResult {
    let GenerateBranchSummaryOptions {
        model,
        api_key,
        headers,
        env,
        signal,
        custom_instructions,
        replace_instructions,
        reserve_tokens,
        stream_fn,
        retry,
        callbacks,
    } = options;
    let reserve_tokens = reserve_tokens.unwrap_or(16_384);

    // Token budget = context window minus reserved space for prompt + response.
    let context_window = if model.context_window == 0 {
        128_000
    } else {
        model.context_window
    };
    let token_budget = context_window.saturating_sub(reserve_tokens);

    let BranchPreparation {
        messages, file_ops, ..
    } = prepare_branch_entries(entries, token_budget);

    if messages.is_empty() {
        return BranchSummaryResult {
            summary: Some("No content to summarize".to_string()),
            ..BranchSummaryResult::default()
        };
    }

    // Transform to LLM-compatible messages, then serialize to text so the
    // model doesn't treat it as a conversation to continue.
    let llm_messages = convert_to_llm(&messages);
    let conversation_text = serialize_conversation(&llm_messages);

    let instructions = if replace_instructions && custom_instructions.is_some() {
        custom_instructions.unwrap_or_default().to_string()
    } else if let Some(custom_instructions) = custom_instructions {
        format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {custom_instructions}")
    } else {
        BRANCH_SUMMARY_PROMPT.to_string()
    };
    let prompt_text =
        format!("<conversation>\n{conversation_text}\n</conversation>\n\n{instructions}");

    let max_tokens = 4096u64.min(if model.max_tokens > 0 {
        model.max_tokens
    } else {
        u64::MAX
    });

    let context = normalize_context(&AiContext {
        system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages: vec![Message::User(UserMessage {
            content: StringOrBlocks::Blocks(vec![TextOrImageBlock::Text(TextContent {
                text: prompt_text,
                text_signature: None,
            })]),
            timestamp: now_epoch_millis(),
        })],
        tools: None,
    });
    let mut request_options = SimpleStreamOptions::default();
    request_options.stream.max_tokens = Some(max_tokens);
    request_options.stream.signal = signal.cloned();
    request_options.stream.api_key = api_key.map(str::to_string);
    request_options.stream.headers = headers.cloned();
    request_options.stream.env = env.cloned();

    let response =
        complete_summarization(model, context, request_options, stream_fn, retry, callbacks).await;

    if response.stop_reason == StopReason::Aborted {
        return BranchSummaryResult {
            aborted: Some(true),
            ..BranchSummaryResult::default()
        };
    }
    if let Some(failure) = get_summarization_failure(&response, "Branch summarization") {
        return BranchSummaryResult {
            error: Some(failure),
            ..BranchSummaryResult::default()
        };
    }
    if response
        .content
        .iter()
        .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
    {
        return BranchSummaryResult {
            error: Some("Branch summarization attempted to call a tool".to_string()),
            ..BranchSummaryResult::default()
        };
    }

    let mut summary = assistant_blocks_text(&response.content, "\n");
    summary = format!("{BRANCH_SUMMARY_PREAMBLE}{summary}");

    let FileLists {
        read_files,
        modified_files,
    } = compute_file_lists(&file_ops);
    summary += &format_file_operations(&read_files, &modified_files);

    BranchSummaryResult {
        summary: Some(if summary.is_empty() {
            "No summary generated".to_string()
        } else {
            summary
        }),
        usage: Some(response.usage),
        read_files: Some(read_files),
        modified_files: Some(modified_files),
        aborted: None,
        error: None,
    }
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
