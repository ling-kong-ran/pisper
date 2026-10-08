//! Port of `packages/agent/src/harness/compaction/` (snapshot commit 5901446,
//! 1297 lines across three files): the compaction decision and summary
//! generation (this module, upstream `compaction/compaction.ts`), the
//! abandoned-branch summarization ([`branch_summarization`]), and the shared
//! file-operation/conversation serialization helpers ([`utils`]).
//!
//! Split-turn compaction also needs `addUsage` (upstream home
//! `harness/utils/usage.ts:18-43`); it lives in [`utils`] until Task 6 ports
//! the remaining `harness/utils` modules.
//!
//! The session vocabulary these modules consume ([`Entry`], the context
//! builders) lives in [`crate::agent_core::harness::session`], the compaction
//! settings in [`crate::agent_core::harness::config`] (re-exported here),
//! and the error types in [`crate::agent_core::harness::types`].
//!
//! Upstream `compaction.ts` specifics (866 lines): compaction thresholds and
//! token estimation, cut-point selection, preparation of the compacted range,
//! and the summary generation that runs through the existing agent-core
//! request path
//! ([`Models::complete_simple`](crate::ai::models::Models::complete_simple)
//! wrapped in
//! [`retry_assistant_call`](crate::ai::retry::retry_assistant_call), the
//! upstream `completeSimple` + `retryAssistantCall` pair).
//!
//! Every caller-facing generation function pairs a "with models" convenience
//! ([`compact`], [`generate_summary`], [`generate_summary_with_usage`]) with
//! a request-boundary variant ([`compact_with_request`],
//! [`generate_summary_with_request`]) that takes a [`SummaryRequest`] — the
//! upstream one-request seam harness callers and tests script.
//!
//! Disclosed substitutions:
//! - Upstream `Result<T, CompactionError>` is [`std::result::Result`] with
//!   [`CompactionError`](crate::agent_core::harness::types::CompactionError)
//!   (the harness `types.rs` ruling).
//! - `prepareCompaction` returns `Ok(None)` for the upstream
//!   `ok(undefined)`; `CutPointResult.turnStartIndex` /
//!   `ContextUsageEstimate.lastUsageIndex` use `Option<usize>` for the
//!   upstream `-1`/`null` sentinels.
//! - `CompactionSettings`/`DEFAULT_COMPACTION_SETTINGS` live in
//!   [`crate::agent_core::harness::config`] and are re-exported here.
//! - `createSummaryRequestOptions`'s `telemetryContext` is deferred with the
//!   telemetry module (M3b Task 10); `signal`, `cacheRetention: "none"`, and
//!   the fresh `sessionId` (via the shared [`crate::ai::uuid::uuid_v7`]) are
//!   applied as upstream.
//! - `CompactResult<T = JsonValue>` is the default instantiation with the
//!   typed [`CompactionDetails`] (the only shape this module produces).
//! - Token estimates count scalar characters (the JS `.length` stand-in; see
//!   the `utils` module docs). Numeric sums saturate instead of wrapping.

pub mod branch_summarization;
pub mod utils;

#[cfg(test)]
mod tests;

pub use branch_summarization::*;
pub use utils::*;

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::messages::{
    convert_to_llm, create_branch_summary_message, create_compaction_summary_message,
};
use crate::agent_core::harness::session::context::{
    build_context_entries, session_entry_to_context_messages,
};
use crate::agent_core::harness::session::types::Entry;
use crate::agent_core::harness::types::{CompactionError, CompactionErrorCode};
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
use crate::ai::models::{Models, ModelsSimpleStreamOptions};
use crate::ai::retry::{retry_assistant_call, RetryCallbacks, RetryPolicy};
use crate::ai::transcript::Context as AiContext;
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Message, StringOrBlocks, TextOrImageBlock, UserMessage,
};
use crate::ai::types::model::Model;
use crate::ai::types::options::SimpleStreamOptions;
use crate::ai::types::primitives::{
    CacheRetention, StopReason, ThinkingLevel as RequestThinkingLevel, Usage,
};
use crate::ai::uuid::uuid_v7;

// The `utils` re-export above also brings the shared helpers into this
// module's scope; no separate import is needed.

// ---------------------------------------------------------------------------
// File-operation details on generated compaction entries
// ---------------------------------------------------------------------------

/// Upstream `CompactionDetails` (`compaction.ts:31-37`): file-operation
/// details stored on generated compaction entries. Wire shape is the
/// upstream camelCase object.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    /// Files read in the compacted history.
    pub read_files: Vec<String>,
    /// Files modified in the compacted history.
    pub modified_files: Vec<String>,
}

/// Scalar-character length (the JS `.length` stand-in; see the module docs).
fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// The message timestamp accessor upstream reads as `message.timestamp`
/// (always present on the standard roles and harness custom kinds).
fn agent_message_timestamp(message: &AgentMessage) -> i64 {
    match message {
        AgentMessage::System(system) => system.timestamp,
        AgentMessage::User(user) => user.timestamp,
        AgentMessage::Assistant(assistant) => assistant.timestamp,
        AgentMessage::ToolResult(tool_result) => tool_result.timestamp,
        AgentMessage::Custom(custom) => custom
            .data
            .get("timestamp")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    }
}

/// Upstream `extractFileOperations` (`compaction.ts:46-76`): seed the
/// accumulator with the previous compaction's `details` lists, then extract
/// from every summarized message.
fn extract_file_operations(
    messages: &[AgentMessage],
    entries: &[Entry],
    prev_compaction_index: Option<usize>,
) -> FileOperations {
    let mut file_ops = FileOperations::new();
    if let Some(index) = prev_compaction_index {
        let Entry::Compaction { details, .. } = &entries[index] else {
            panic!("prevCompactionIndex points at a compaction entry");
        };
        if let Some(details) = details {
            if let Some(read_files) = details
                .get("readFiles")
                .and_then(serde_json::Value::as_array)
            {
                for path in read_files {
                    if let Some(path) = path.as_str() {
                        file_ops.read.insert(path.to_string());
                    }
                }
            }
            if let Some(modified_files) = details
                .get("modifiedFiles")
                .and_then(serde_json::Value::as_array)
            {
                for path in modified_files {
                    if let Some(path) = path.as_str() {
                        file_ops.edited.insert(path.to_string());
                    }
                }
            }
        }
    }
    for message in messages {
        extract_file_ops_from_message(message, &mut file_ops);
    }
    file_ops
}

/// Upstream `getMessageFromEntry` (`compaction.ts:77-88`): the conversation
/// message an entry contributes to summarization inputs.
fn get_message_from_entry(entry: &Entry) -> Option<AgentMessage> {
    match entry {
        Entry::Message { message, .. } => Some(message.clone()),
        Entry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } => Some(AgentMessage::Custom(
            create_branch_summary_message(summary.clone(), from_id.clone(), *timestamp).to_custom(),
        )),
        Entry::Compaction {
            summary,
            tokens_before,
            timestamp,
            ..
        } => Some(AgentMessage::Custom(
            create_compaction_summary_message(summary.clone(), *tokens_before, *timestamp)
                .to_custom(),
        )),
        Entry::Custom { .. } => None,
    }
}

/// Upstream `getMessageFromEntryForCompaction` (`compaction.ts:90-95`): the
/// compaction checkpoint itself never re-summarizes.
fn get_message_from_entry_for_compaction(entry: &Entry) -> Option<AgentMessage> {
    if matches!(entry, Entry::Compaction { .. }) {
        return None;
    }
    get_message_from_entry(entry)
}

// ---------------------------------------------------------------------------
// Generated compaction data
// ---------------------------------------------------------------------------

/// Upstream `CompactResult<T = JsonValue>` (`compaction.ts:98-109`):
/// generated compaction data ready to be persisted as a compaction entry.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactResult {
    /// Summary text that replaces compacted history in future context.
    pub summary: String,
    /// Estimated context tokens before compaction.
    pub tokens_before: u64,
    /// Usage from the LLM call(s) that generated this summary, if available.
    pub usage: Option<Usage>,
    /// Retained recent messages stored directly on the compaction entry.
    pub retained_tail: Vec<AgentMessage>,
    /// Implementation-specific details stored with the compaction entry.
    pub details: Option<CompactionDetails>,
}

// ---------------------------------------------------------------------------
// Summary request plumbing
// ---------------------------------------------------------------------------

/// Upstream `SummaryRequest` (`compaction.ts:111-115`): one caller-owned
/// summary-request boundary — `(aiContext, options, context) =>
/// AssistantMessage`. Errors are stop-reason error messages, like upstream
/// (the request never throws past this boundary).
pub type SummaryRequest = Arc<
    dyn Fn(AiContext, SimpleStreamOptions, Context) -> BoxFuture<'static, AssistantMessage>
        + Send
        + Sync,
>;

/// Upstream `createSummaryRequestOptions` (`compaction.ts:117-125`):
/// summaries are standalone requests — the context's abort signal, no cache
/// retention, and an isolated session id (fresh uuidv7 unless the caller
/// supplied one). `telemetryContext` lands with the telemetry module.
pub fn create_summary_request_options(
    mut options: SimpleStreamOptions,
    context: &Context,
) -> SimpleStreamOptions {
    options.stream.signal = context.abort_signal();
    options.stream.cache_retention = Some(CacheRetention::None);
    options.stream.session_id = options.stream.session_id.or_else(|| Some(uuid_v7()));
    options
}

/// Upstream `completeSimpleWithRetries` (`compaction.ts:127-144`): the
/// default [`SummaryRequest`] transport — `completeSimple` through the
/// [`Models`] collection with per-request retry. Routing and cache
/// isolation are applied by [`create_summary_request_options`].
pub async fn complete_simple_with_retries(
    models: &Models,
    model: &Model,
    ai_context: AiContext,
    options: SimpleStreamOptions,
    retry: Option<&RetryPolicy>,
    callbacks: &mut RetryCallbacks,
    context: Context,
) -> AssistantMessage {
    let request_options = create_summary_request_options(options, &context);
    let signal = request_options.stream.signal.clone();
    retry_assistant_call(
        || {
            models.complete_simple(
                model,
                &ai_context,
                Some(ModelsSimpleStreamOptions {
                    simple: request_options.clone(),
                    transform_headers: None,
                }),
            )
        },
        retry,
        signal.as_ref(),
        callbacks,
    )
    .await
}

/// The default [`SummaryRequest`] factory over a [`Models`] collection — the
/// upstream closure passed at `compaction.ts:540-542`, `740-742`, and
/// `branch-summarization.ts:230-232`. `callbacks` default to no-op hooks
/// (upstream `undefined`). The callbacks are shared across every request the
/// boundary serves (the tokio mutex keeps the guard `Send` across the retry
/// awaits, which a std guard cannot).
pub fn models_summary_request(
    models: Arc<Models>,
    model: Model,
    retry: Option<RetryPolicy>,
    callbacks: RetryCallbacks,
) -> SummaryRequest {
    use tokio::sync::Mutex;

    let callbacks = Arc::new(Mutex::new(callbacks));
    Arc::new(move |ai_context, options, context| {
        let models = Arc::clone(&models);
        let model = model.clone();
        let retry = retry.clone();
        let callbacks = Arc::clone(&callbacks);
        Box::pin(async move {
            let mut callbacks = callbacks.lock().await;
            complete_simple_with_retries(
                &models,
                &model,
                ai_context,
                options,
                retry.as_ref(),
                &mut callbacks,
                context,
            )
            .await
        })
    })
}

// ---------------------------------------------------------------------------
// Thresholds, estimation, and cut points
// ---------------------------------------------------------------------------

/// Upstream re-export (`compaction.ts:147-161`): the settings live in
/// [`crate::agent_core::harness::config`] (the Task 2 validator home) and are
/// re-exported here like upstream.
pub use crate::agent_core::harness::config::{CompactionSettings, DEFAULT_COMPACTION_SETTINGS};

/// Upstream `calculateContextTokens` (`compaction.ts:164-166`): the
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

/// Upstream `getAssistantUsage` (`compaction.ts:167-180`): usage from a
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

/// Upstream `getLastAssistantUsage` (`compaction.ts:183-192`): usage from the
/// last valid assistant message in session entries.
pub fn get_last_assistant_usage(entries: &[Entry]) -> Option<Usage> {
    for entry in entries.iter().rev() {
        if let Entry::Message { message, .. } = entry {
            if let Some(usage) = get_assistant_usage(message) {
                return Some(usage);
            }
        }
    }
    None
}

/// Upstream `ContextUsageEstimate` (`compaction.ts:195-204`): estimated
/// context-token usage for a message list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsageEstimate {
    /// Estimated total context tokens.
    pub tokens: u64,
    /// Tokens reported by the most recent assistant usage block.
    pub usage_tokens: u64,
    /// Estimated tokens after the most recent assistant usage block.
    pub trailing_tokens: u64,
    /// Index of the message that provided usage (`None` when none exists;
    /// upstream `null`).
    pub last_usage_index: Option<usize>,
}

/// Upstream `getLastAssistantUsageInfo` (`compaction.ts:206-212`).
fn get_last_assistant_usage_info(messages: &[AgentMessage]) -> Option<(Usage, usize)> {
    for (index, message) in messages.iter().enumerate().rev() {
        if let Some(usage) = get_assistant_usage(message) {
            return Some((usage, index));
        }
    }
    None
}

/// Upstream `estimateContextTokens` (`compaction.ts:215-243`): provider usage
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

/// Upstream `shouldCompact` (`compaction.ts:246-249`): whether context usage
/// exceeds the configured compaction threshold. The comparison is i128 to
/// keep the JS subtraction semantics when `reserveTokens` exceeds the
/// context window (negative threshold compacts everything).
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

/// Upstream `ESTIMATED_IMAGE_CHARS` (`compaction.ts:251`).
const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// Upstream `estimateTextAndImageContentChars` (`compaction.ts:253-267`).
fn estimate_text_and_image_content_chars(content: &StringOrBlocks) -> usize {
    match content {
        StringOrBlocks::Text(text) => char_len(text),
        StringOrBlocks::Blocks(blocks) => {
            let mut chars = 0;
            for block in blocks {
                match block {
                    TextOrImageBlock::Text(text) => chars += char_len(&text.text),
                    TextOrImageBlock::Image(_) => chars += ESTIMATED_IMAGE_CHARS,
                }
            }
            chars
        }
    }
}

/// Upstream `estimateTokens` (`compaction.ts:270-310`): a conservative
/// character heuristic over every supported message role (`chars / 4`
/// rounded up; unknown roles count zero).
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let chars: usize = match message {
        AgentMessage::User(user) => {
            return estimate_text_and_image_content_chars(&user.content).div_ceil(4) as u64;
        }
        AgentMessage::Assistant(assistant) => {
            let mut chars = 0;
            for block in &assistant.content {
                match block {
                    AssistantBlock::Text(text) => chars += char_len(&text.text),
                    AssistantBlock::Thinking(thinking) => chars += char_len(&thinking.thinking),
                    AssistantBlock::ToolCall(tool_call) => {
                        chars += char_len(&tool_call.name)
                            + char_len(
                                &serde_json::to_string(&tool_call.arguments)
                                    .unwrap_or_else(|_| "[unserializable]".to_string()),
                            );
                    }
                }
            }
            chars
        }
        AgentMessage::ToolResult(tool_result) => {
            let mut chars = 0;
            for block in &tool_result.content {
                match block {
                    TextOrImageBlock::Text(text) => chars += char_len(&text.text),
                    TextOrImageBlock::Image(_) => chars += ESTIMATED_IMAGE_CHARS,
                }
            }
            chars
        }
        AgentMessage::Custom(custom) => match custom.role.as_str() {
            "custom" => custom_message_view(custom)
                .map(|message| estimate_text_and_image_content_chars(&message.content))
                .unwrap_or(0),
            "bashExecution" => bash_execution_view(custom)
                .map(|message| char_len(&message.command) + char_len(&message.output))
                .unwrap_or(0),
            "branchSummary" => branch_summary_view(custom)
                .map(|message| char_len(&message.summary))
                .unwrap_or(0),
            "compactionSummary" => compaction_summary_view(custom)
                .map(|message| char_len(&message.summary))
                .unwrap_or(0),
            _ => 0,
        },
        AgentMessage::System(_) => 0,
    };
    chars.div_ceil(4) as u64
}

/// Upstream `findValidCutPoints` (`compaction.ts:311-340`): entry indices a
/// cut may land on — message entries of turn-start/continuation roles (never
/// `toolResult`) and `branch_summary` entries.
fn find_valid_cut_points(entries: &[Entry], start_index: usize, end_index: usize) -> Vec<usize> {
    let mut cut_points = Vec::new();
    for (index, entry) in entries.iter().enumerate().take(end_index).skip(start_index) {
        match entry {
            Entry::Message { message, .. } => match message.role() {
                "bashExecution" | "custom" | "branchSummary" | "compactionSummary" | "user"
                | "assistant" => cut_points.push(index),
                "toolResult" => {}
                _ => {}
            },
            Entry::Compaction { .. } | Entry::BranchSummary { .. } | Entry::Custom { .. } => {}
        }
        if matches!(entry, Entry::BranchSummary { .. }) {
            cut_points.push(index);
        }
    }
    cut_points
}

/// Upstream `findTurnStartIndex` (`compaction.ts:343-357`): the user-visible
/// message that starts the turn containing `entry_index` (`None` past
/// `start_index`; upstream `-1`).
pub fn find_turn_start_index(
    entries: &[Entry],
    entry_index: usize,
    start_index: usize,
) -> Option<usize> {
    for index in (start_index..=entry_index).rev() {
        let entry = &entries[index];
        if matches!(entry, Entry::BranchSummary { .. }) {
            return Some(index);
        }
        if let Entry::Message { message, .. } = entry {
            if matches!(message.role(), "user" | "bashExecution") {
                return Some(index);
            }
        }
    }
    None
}

/// Upstream `CutPointResult` (`compaction.ts:360-367`): the cut point
/// selected for compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutPointResult {
    /// Index of the first entry retained after compaction.
    pub first_kept_entry_index: usize,
    /// Index of the turn-start entry when the cut splits a turn (`None`
    /// otherwise; upstream `-1`).
    pub turn_start_index: Option<usize>,
    /// Whether the selected cut point splits an in-progress turn.
    pub is_split_turn: bool,
}

/// Upstream `findCutPoint` (`compaction.ts:370-418`): walk backwards
/// accumulating per-message estimates until the recent-token budget is
/// crossed, take the first valid cut point at or after that index, then slide
/// back over trailing non-message entries without passing a compaction
/// checkpoint or message.
pub fn find_cut_point(
    entries: &[Entry],
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
        let Entry::Message { message, .. } = entry else {
            continue;
        };
        accumulated_tokens = accumulated_tokens.saturating_add(estimate_tokens(message));
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
        if matches!(prev_entry, Entry::Compaction { .. } | Entry::Message { .. }) {
            break;
        }
        cut_index -= 1;
    }
    let is_user_message =
        matches!(&entries[cut_index], Entry::Message { message, .. } if message.role() == "user");
    let turn_start_index = if is_user_message {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user_message && turn_start_index.is_some(),
    }
}

// ---------------------------------------------------------------------------
// Summary prompts
// ---------------------------------------------------------------------------

/// Upstream `SUMMARIZATION_SYSTEM_PROMPT` (`compaction.ts:420-422`).
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// Upstream `SUMMARIZATION_PROMPT` (`compaction.ts:424-455`).
const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Upstream `UPDATE_SUMMARIZATION_PROMPT` (`compaction.ts:457-494`).
const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\nUpdate the existing structured summary with new information. RULES:\n- PRESERVE all existing information from the previous summary\n- ADD new progress, decisions, and context from the new messages\n- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed\n- UPDATE \"Next Steps\" based on what was accomplished\n- PRESERVE exact file paths, function names, and error messages\n- If something is no longer relevant, you may remove it\n\nUse this EXACT format:\n\n## Goal\n[Preserve existing goals, add new ones if the task expanded]\n\n## Constraints & Preferences\n- [Preserve existing, add new ones discovered]\n\n## Progress\n### Done\n- [x] [Include previously done items AND newly completed items]\n\n### In Progress\n- [ ] [Current work - update based on progress]\n\n### Blocked\n- [Current blockers - remove if resolved]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale] (preserve all previous, add new)\n\n## Next Steps\n1. [Update based on current state]\n\n## Critical Context\n- [Preserve important context, add new if needed]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Upstream `TURN_PREFIX_SUMMARIZATION_PROMPT` (`compaction.ts:709-722`).
const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.\n\nSummarize the prefix to provide context for the retained suffix:\n\n## Original Request\n[What did the user ask for in this turn?]\n\n## Early Progress\n- [Key decisions and work done in the prefix]\n\n## Context for Suffix\n- [Information needed to understand the retained recent work]\n\nBe concise. Focus on what's needed to understand the kept suffix.";

/// Upstream `generateSummary` (`compaction.ts:497-522`): generate or update a
/// conversation summary for compaction, text only.
#[allow(clippy::too_many_arguments)]
pub async fn generate_summary(
    current_messages: &[AgentMessage],
    models: Arc<Models>,
    model: Model,
    reserve_tokens: u64,
    custom_instructions: Option<String>,
    previous_summary: Option<String>,
    thinking_level: Option<ThinkingLevel>,
    retry: Option<RetryPolicy>,
    callbacks: RetryCallbacks,
    context: Context,
) -> Result<String, CompactionError> {
    let result = generate_summary_with_usage(
        current_messages,
        models,
        model,
        reserve_tokens,
        custom_instructions,
        previous_summary,
        thinking_level,
        retry,
        callbacks,
        context,
    )
    .await?;
    Ok(result.text)
}

/// The `ok({ text, usage })` payload of the upstream
/// `generateSummaryWithUsage` result.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryWithUsage {
    /// The generated summary text.
    pub text: String,
    /// Provider usage of the summarization request.
    pub usage: Usage,
}

/// Upstream `generateSummaryWithUsage` (`compaction.ts:525-544`): generate or
/// update a conversation summary and return its provider usage, through the
/// default [`Models`] transport.
#[allow(clippy::too_many_arguments)]
pub async fn generate_summary_with_usage(
    current_messages: &[AgentMessage],
    models: Arc<Models>,
    model: Model,
    reserve_tokens: u64,
    custom_instructions: Option<String>,
    previous_summary: Option<String>,
    thinking_level: Option<ThinkingLevel>,
    retry: Option<RetryPolicy>,
    callbacks: RetryCallbacks,
    context: Context,
) -> Result<SummaryWithUsage, CompactionError> {
    let request = models_summary_request(Arc::clone(&models), model.clone(), retry, callbacks);
    generate_summary_with_request(
        current_messages,
        &SummaryGenerationOptions {
            model,
            reserve_tokens,
            custom_instructions,
            previous_summary,
            thinking_level,
        },
        &request,
        context,
    )
    .await
}

/// Upstream `SummaryGenerationOptions` (`compaction.ts:546-552`).
#[derive(Debug, Clone)]
pub struct SummaryGenerationOptions {
    /// Model used for the summary.
    pub model: Model,
    /// Tokens reserved for prompt and model output.
    pub reserve_tokens: u64,
    /// Optional focus instructions appended to the default prompt.
    pub custom_instructions: Option<String>,
    /// Previous compaction summary driving the update prompt.
    pub previous_summary: Option<String>,
    /// Thinking level applied to reasoning-capable models.
    pub thinking_level: Option<ThinkingLevel>,
}

/// One user text message carrying the prompt (the upstream
/// `summarizationMessages` literal, timestamped `Date.now()`).
fn summarization_request_messages(prompt_text: String) -> Vec<Message> {
    vec![Message::User(UserMessage {
        content: StringOrBlocks::Blocks(vec![crate::ai::types::TextOrImageBlock::Text(
            crate::ai::types::TextContent {
                text: prompt_text,
                text_signature: None,
            },
        )]),
        timestamp: crate::ai::now_ms(),
    })]
}

/// The `{ maxTokens, reasoning? }` completion options
/// (`compaction.ts:586-589`, `840-843`): reasoning applies only to
/// reasoning-capable models with a level other than `"off"`. The port's
/// `SimpleStreamOptions.reasoning` carries the six non-`"off"` levels
/// (ai `ThinkingLevel`, types.ts:83), so the agent-level level narrows by
/// construction.
fn completion_options(
    max_tokens: u64,
    model: &Model,
    thinking_level: Option<ThinkingLevel>,
) -> SimpleStreamOptions {
    let mut options = SimpleStreamOptions {
        stream: crate::ai::types::StreamOptions {
            max_tokens: Some(max_tokens),
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

/// The aborted/error result mapping shared by all three summarization
/// requesters (`compaction.ts:596-606`, `849-859`;
/// `branch-summarization.ts:277-287` builds its own messages on top).
fn summary_result(
    response: AssistantMessage,
    aborted_message: &str,
    failed_prefix: &str,
) -> Result<(String, Usage), CompactionError> {
    if response.stop_reason == StopReason::Aborted {
        return Err(CompactionError::new(
            CompactionErrorCode::Aborted,
            response
                .error_message
                .unwrap_or_else(|| aborted_message.to_string()),
        ));
    }
    if response.stop_reason == StopReason::Error {
        return Err(CompactionError::new(
            CompactionErrorCode::SummarizationFailed,
            format!(
                "{failed_prefix}: {}",
                response
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_string())
            ),
        ));
    }
    Ok((
        assistant_blocks_text(&response.content, "\n"),
        response.usage,
    ))
}

/// Upstream `generateSummaryWithRequest` (`compaction.ts:555-611`): generate
/// one summary through a caller-owned one-request boundary.
pub async fn generate_summary_with_request(
    current_messages: &[AgentMessage],
    options: &SummaryGenerationOptions,
    request: &SummaryRequest,
    context: Context,
) -> Result<SummaryWithUsage, CompactionError> {
    let SummaryGenerationOptions {
        model,
        reserve_tokens,
        custom_instructions,
        previous_summary,
        thinking_level,
    } = options;
    let output_cap = if model.max_tokens > 0 {
        model.max_tokens
    } else {
        u64::MAX
    };
    let max_tokens = ((0.8 * *reserve_tokens as f64).floor() as u64).min(output_cap);
    let mut base_prompt: String = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_string()
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };
    if let Some(custom_instructions) = custom_instructions {
        base_prompt = format!("{base_prompt}\n\nAdditional focus: {custom_instructions}");
    }
    let llm_messages = convert_to_llm(current_messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let mut prompt_text = format!("<conversation>\n{conversation_text}\n</conversation>\n\n");
    if let Some(previous_summary) = previous_summary {
        prompt_text.push_str(&format!(
            "<previous-summary>\n{previous_summary}\n</previous-summary>\n\n"
        ));
    }
    prompt_text.push_str(&base_prompt);

    let completion = completion_options(max_tokens, model, *thinking_level);
    let response = request(
        AiContext {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: summarization_request_messages(prompt_text),
            tools: None,
        },
        create_summary_request_options(completion, &context),
        context.clone(),
    )
    .await;
    let (text, usage) = summary_result(response, "Summarization aborted", "Summarization failed")?;
    Ok(SummaryWithUsage { text, usage })
}

// ---------------------------------------------------------------------------
// Preparation and compaction
// ---------------------------------------------------------------------------

/// Upstream `CompactionPreparation` (`compaction.ts:614-631`): prepared
/// inputs for a compaction run.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionPreparation {
    /// Messages summarized into the history summary.
    pub messages_to_summarize: Vec<AgentMessage>,
    /// Prefix messages summarized separately when compaction splits a turn.
    pub turn_prefix_messages: Vec<AgentMessage>,
    /// Recent messages retained after compaction and stored on the
    /// compaction entry.
    pub retained_tail: Vec<AgentMessage>,
    /// Whether compaction splits a turn.
    pub is_split_turn: bool,
    /// Estimated context tokens before compaction.
    pub tokens_before: u64,
    /// Previous compaction summary used for iterative updates.
    pub previous_summary: Option<String>,
    /// File operations extracted from summarized history.
    pub file_ops: FileOperations,
    /// Settings used to prepare compaction.
    pub settings: CompactionSettings,
}

/// Upstream `prepareCompaction` (`compaction.ts:634-707`): prepare session
/// entries for compaction, or `Ok(None)` when compaction is not applicable —
/// an empty path or a trailing compaction checkpoint.
pub fn prepare_compaction(
    path_entries: &[Entry],
    settings: CompactionSettings,
) -> Result<Option<CompactionPreparation>, CompactionError> {
    if path_entries.is_empty()
        || matches!(
            path_entries[path_entries.len() - 1],
            Entry::Compaction { .. }
        )
    {
        return Ok(None);
    }

    let mut prev_compaction_index: Option<usize> = None;
    for (index, entry) in path_entries.iter().enumerate().rev() {
        if matches!(entry, Entry::Compaction { .. }) {
            prev_compaction_index = Some(index);
            break;
        }
    }

    let mut previous_summary: Option<String> = None;
    let compactable_entries: Vec<Entry>;
    if let Some(index) = prev_compaction_index {
        let Entry::Compaction {
            id,
            seq,
            summary,
            retained_tail,
            ..
        } = &path_entries[index]
        else {
            unreachable!("checked above");
        };
        previous_summary = Some(summary.clone());
        // The retained tail rejoins the compactable range as virtual message
        // entries chained under the checkpoint (`compaction.ts:655-662`).
        let virtual_retained_entries: Vec<Entry> = retained_tail
            .iter()
            .enumerate()
            .map(|(retained_index, message)| Entry::Message {
                id: format!("{id}:retained:{retained_index}"),
                parent_id: Some(if retained_index == 0 {
                    id.clone()
                } else {
                    format!("{id}:retained:{}", retained_index - 1)
                }),
                seq: *seq,
                timestamp: agent_message_timestamp(message),
                message: message.clone(),
                terminate: None,
            })
            .collect();
        let mut entries = virtual_retained_entries;
        entries.extend(path_entries[index + 1..].iter().cloned());
        compactable_entries = entries;
    } else {
        compactable_entries = path_entries.to_vec();
    }
    let boundary_end = compactable_entries.len();

    let tokens_before = estimate_context_tokens(
        &build_context_entries(path_entries)
            .iter()
            .flat_map(session_entry_to_context_messages)
            .collect::<Vec<AgentMessage>>(),
    )
    .tokens;

    let cut_point = find_cut_point(
        &compactable_entries,
        0,
        boundary_end,
        settings.keep_recent_tokens,
    );
    let history_end = if cut_point.is_split_turn {
        cut_point
            .turn_start_index
            .expect("split turn carries a turn-start index")
    } else {
        cut_point.first_kept_entry_index
    };
    let mut messages_to_summarize: Vec<AgentMessage> = Vec::new();
    for entry in &compactable_entries[..history_end] {
        if let Some(message) = get_message_from_entry_for_compaction(entry) {
            messages_to_summarize.push(message);
        }
    }
    let mut turn_prefix_messages: Vec<AgentMessage> = Vec::new();
    if cut_point.is_split_turn {
        for entry in &compactable_entries[cut_point.turn_start_index.expect("split turn index")
            ..cut_point.first_kept_entry_index]
        {
            if let Some(message) = get_message_from_entry_for_compaction(entry) {
                turn_prefix_messages.push(message);
            }
        }
    }
    let mut retained_tail: Vec<AgentMessage> = Vec::new();
    for entry in &compactable_entries[cut_point.first_kept_entry_index..boundary_end] {
        if let Some(message) = get_message_from_entry_for_compaction(entry) {
            retained_tail.push(message);
        }
    }
    let mut file_ops =
        extract_file_operations(&messages_to_summarize, path_entries, prev_compaction_index);
    if cut_point.is_split_turn {
        for message in &turn_prefix_messages {
            extract_file_ops_from_message(message, &mut file_ops);
        }
    }

    Ok(Some(CompactionPreparation {
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings,
    }))
}

/// Upstream `compact` (`compaction.ts:727-744`): generate compaction summary
/// data from prepared session history through the default [`Models`]
/// transport.
#[allow(clippy::too_many_arguments)]
pub async fn compact(
    preparation: CompactionPreparation,
    models: Arc<Models>,
    model: Model,
    custom_instructions: Option<String>,
    thinking_level: Option<ThinkingLevel>,
    retry: Option<RetryPolicy>,
    callbacks: RetryCallbacks,
    context: Context,
) -> Result<CompactResult, CompactionError> {
    let request = models_summary_request(Arc::clone(&models), model.clone(), retry, callbacks);
    compact_with_request(
        preparation,
        &CompactGenerationOptions {
            model,
            custom_instructions,
            thinking_level,
        },
        &request,
        context,
    )
    .await
}

/// Upstream `CompactGenerationOptions` (`compaction.ts:746-750`).
#[derive(Debug, Clone)]
pub struct CompactGenerationOptions {
    /// Model used for the summaries.
    pub model: Model,
    /// Optional focus instructions appended to the default prompt.
    pub custom_instructions: Option<String>,
    /// Thinking level applied to reasoning-capable models.
    pub thinking_level: Option<ThinkingLevel>,
}

/// Upstream `compactWithRequest` (`compaction.ts:753-816`): generate
/// compaction data through a caller-owned boundary for each provider request.
/// Split turns summarize history and the turn prefix separately, then join
/// them with the upstream separator and combine usage.
pub async fn compact_with_request(
    preparation: CompactionPreparation,
    options: &CompactGenerationOptions,
    request: &SummaryRequest,
    context: Context,
) -> Result<CompactResult, CompactionError> {
    let CompactGenerationOptions {
        model,
        custom_instructions,
        thinking_level,
    } = options;
    let CompactionPreparation {
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings,
    } = preparation;

    let (mut summary, summary_usage): (String, Usage);
    if is_split_turn && !turn_prefix_messages.is_empty() {
        let mut history_text = "No prior history.".to_string();
        let mut history_usage: Option<Usage> = None;
        if !messages_to_summarize.is_empty() {
            let history_result = generate_summary_with_request(
                &messages_to_summarize,
                &SummaryGenerationOptions {
                    model: model.clone(),
                    reserve_tokens: settings.reserve_tokens,
                    custom_instructions: custom_instructions.clone(),
                    previous_summary: previous_summary.clone(),
                    thinking_level: *thinking_level,
                },
                request,
                context.clone(),
            )
            .await?;
            history_text = history_result.text;
            history_usage = Some(history_result.usage);
        }
        let turn_prefix_result = generate_turn_prefix_summary(
            &turn_prefix_messages,
            model,
            settings.reserve_tokens,
            *thinking_level,
            request,
            &context,
        )
        .await?;
        summary = format!(
            "{history_text}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
            turn_prefix_result.text
        );
        summary_usage = match history_usage {
            Some(history_usage) => add_usage(history_usage, turn_prefix_result.usage),
            None => turn_prefix_result.usage,
        };
    } else {
        let generated = generate_summary_with_request(
            &messages_to_summarize,
            &SummaryGenerationOptions {
                model: model.clone(),
                reserve_tokens: settings.reserve_tokens,
                custom_instructions: custom_instructions.clone(),
                previous_summary: previous_summary.clone(),
                thinking_level: *thinking_level,
            },
            request,
            context,
        )
        .await?;
        summary = generated.text;
        summary_usage = generated.usage;
    }

    let FileLists {
        read_files,
        modified_files,
    } = compute_file_lists(&file_ops);
    summary.push_str(&format_file_operations(&read_files, &modified_files));
    let details = CompactionDetails {
        read_files,
        modified_files,
    };

    Ok(CompactResult {
        summary,
        tokens_before,
        usage: Some(summary_usage),
        retained_tail,
        details: Some(details),
    })
}

/// Upstream `generateTurnPrefixSummary` (`compaction.ts:817-865`): the
/// split-turn prefix prompt at half the reserve budget.
async fn generate_turn_prefix_summary(
    messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    thinking_level: Option<ThinkingLevel>,
    request: &SummaryRequest,
    context: &Context,
) -> Result<SummaryWithUsage, CompactionError> {
    let output_cap = if model.max_tokens > 0 {
        model.max_tokens
    } else {
        u64::MAX
    };
    let max_tokens = ((0.5 * reserve_tokens as f64).floor() as u64).min(output_cap);
    let llm_messages = convert_to_llm(messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let prompt_text = format!("<conversation>\n{conversation_text}\n</conversation>\n\n{TURN_PREFIX_SUMMARIZATION_PROMPT}");
    let completion = completion_options(max_tokens, model, thinking_level);
    let response = request(
        AiContext {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: summarization_request_messages(prompt_text),
            tools: None,
        },
        create_summary_request_options(completion, context),
        context.clone(),
    )
    .await;
    let (text, usage) = summary_result(
        response,
        "Turn prefix summarization aborted",
        "Turn prefix summarization failed",
    )?;
    Ok(SummaryWithUsage { text, usage })
}
