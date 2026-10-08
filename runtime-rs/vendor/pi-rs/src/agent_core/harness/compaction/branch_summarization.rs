//! Port of
//! `packages/agent/src/harness/compaction/branch-summarization.ts` (301
//! lines): collect the abandoned side of a session-tree branch, prepare it
//! within a token budget, and generate the summary that greets the
//! conversation when it returns from the branch.
//!
//! The LLM call goes through the same request path as compaction —
//! [`Models::complete_simple`](crate::ai::models::Models::complete_simple)
//! under
//! [`retry_assistant_call`](crate::ai::retry::retry_assistant_call), via the
//! shared [`SummaryRequest`](super::SummaryRequest) boundary and
//! [`models_summary_request`](super::models_summary_request)
//! factory.
//!
//! Disclosed substitutions mirror `compaction.rs`: `Result` is
//! [`std::result::Result`] with
//! [`BranchSummaryError`](crate::agent_core::harness::types::BranchSummaryError);
//! `commonAncestorId: string | null` is `Option<String>`; token estimates
//! count scalar characters.

use std::sync::Arc;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::messages::{
    convert_to_llm, create_branch_summary_message, create_compaction_summary_message,
};
use crate::agent_core::harness::session::types::{BranchReader, BranchScan, Entry, SessionReader};
use crate::agent_core::harness::types::{BranchSummaryError, BranchSummaryErrorCode};
use crate::agent_core::types::AgentMessage;
use crate::ai::models::Models;
use crate::ai::retry::{RetryCallbacks, RetryPolicy};
use crate::ai::types::message::Message;
use crate::ai::types::model::Model;
use crate::ai::types::options::SimpleStreamOptions;
use crate::ai::types::primitives::{StopReason, Usage};

use super::utils::{
    assistant_blocks_text, compute_file_lists, extract_file_ops_from_message,
    format_file_operations, serialize_conversation, FileLists, FileOperations,
};
use super::{
    create_summary_request_options, estimate_tokens, models_summary_request, SummaryRequest,
    SUMMARIZATION_SYSTEM_PROMPT,
};

/// Upstream `BranchSummaryResult` (`branch-summarization.ts:33-38`):
/// generated branch summary data ready to be persisted as a branch-summary
/// entry.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchSummaryResult {
    /// The full summary text (preamble plus file tags included).
    pub summary: String,
    /// Usage of the summarization request (`None` only on the no-content
    /// early return, like upstream's absent `usage`).
    pub usage: Option<Usage>,
    /// Files read while exploring the summarized branch.
    pub read_files: Vec<String>,
    /// Files modified while exploring the summarized branch.
    pub modified_files: Vec<String>,
}

/// Upstream `BranchPreparation` (`branch-summarization.ts:51-58`): prepared
/// branch content for summarization.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchPreparation {
    /// Messages selected for the branch summary.
    pub messages: Vec<AgentMessage>,
    /// File operations extracted from the branch.
    pub file_ops: FileOperations,
    /// Estimated token count for selected messages.
    pub total_tokens: u64,
}

/// Upstream `CollectEntriesResult` (`branch-summarization.ts:61-66`): entries
/// selected for branch summarization.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectEntriesResult {
    /// Entries to summarize in chronological order.
    pub entries: Vec<Entry>,
    /// Deepest common ancestor between the previous tip and target entry
    /// (`None` when there is none; upstream `null`).
    pub common_ancestor_id: Option<String>,
}

/// Upstream `GenerateBranchSummaryOptions` (`branch-summarization.ts:69-84`).
pub struct GenerateBranchSummaryOptions {
    /// Provider collection the summarization request goes through; owns auth
    /// resolution.
    pub models: Arc<Models>,
    /// Model used for summarization.
    pub model: Model,
    /// Optional instructions appended to (or replacing) the default prompt.
    pub custom_instructions: Option<String>,
    /// Replace the default prompt with custom instructions instead of
    /// appending them.
    pub replace_instructions: bool,
    /// Tokens reserved for prompt and model output; defaults to 16384.
    pub reserve_tokens: Option<u64>,
    /// Optional retry policy for transient summarization errors.
    pub retry: Option<RetryPolicy>,
    /// Optional callbacks for retry reporting (no-op when defaulted).
    pub callbacks: RetryCallbacks,
}

/// Upstream `PreparedBranchSummaryOptions` (`branch-summarization.ts:236-239`).
#[derive(Debug, Clone, Default)]
pub struct PreparedBranchSummaryOptions {
    pub custom_instructions: Option<String>,
    pub replace_instructions: bool,
}

/// Upstream `collectEntriesForBranchSummary`
/// (`branch-summarization.ts:87-118`): collect the entries that should be
/// summarized before navigating away from `old_tip_id` toward `target_id`.
/// A missing entry on the walk is a corrupt session
/// (`Err`, the upstream thrown `Error`).
pub async fn collect_entries_for_branch_summary(
    branch: &dyn BranchReader,
    session: &dyn SessionReader,
    old_tip_id: Option<&str>,
    target_id: &str,
    context: Context,
) -> anyhow::Result<CollectEntriesResult> {
    let Some(old_tip_id) = old_tip_id else {
        return Ok(CollectEntriesResult {
            entries: Vec::new(),
            common_ancestor_id: None,
        });
    };
    let old_path = branch
        .find_entries(
            Some(&BranchScan {
                start: Some(old_tip_id.to_string()),
                ..BranchScan::default()
            }),
            context.clone(),
        )
        .await?
        .into_iter()
        .map(|entry| entry.id().to_string())
        .collect::<std::collections::HashSet<_>>();
    let target_path = branch
        .find_entries(
            Some(&BranchScan {
                start: Some(target_id.to_string()),
                ..BranchScan::default()
            }),
            context.clone(),
        )
        .await?;
    let mut common_ancestor_id: Option<String> = None;
    for entry in &target_path {
        if old_path.contains(entry.id()) {
            common_ancestor_id = Some(entry.id().to_string());
            break;
        }
    }

    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Option<String> = Some(old_tip_id.to_string());
    while let Some(id) = current {
        if Some(&id) == common_ancestor_id.as_ref() {
            break;
        }
        let entry = session
            .get_entry(&id, context.clone())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Corrupt session: entry {id} not found"))?;
        current = entry.parent_id().map(str::to_string);
        entries.push(entry);
    }
    entries.reverse();

    Ok(CollectEntriesResult {
        entries,
        common_ancestor_id,
    })
}

/// Upstream `getMessageFromEntry` (`branch-summarization.ts:119-133`): unlike
/// the compaction variant, tool-result messages are skipped entirely.
fn get_message_from_entry(entry: &Entry) -> Option<AgentMessage> {
    match entry {
        Entry::Message { message, .. } => {
            if message.role() == "toolResult" {
                return None;
            }
            Some(message.clone())
        }
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

/// Upstream `prepareBranchEntries` (`branch-summarization.ts:136-182`):
/// prepare branch entries for summarization within an optional token budget
/// (`0` = unlimited). The walk is newest-first and `messages` is built by
/// prepending, so the result is chronological; a budget crossing stops the
/// walk, admitting the boundary entry only for compaction/branch-summary
/// checkpoints while under 90% of the budget.
pub fn prepare_branch_entries(entries: &[Entry], token_budget: u64) -> BranchPreparation {
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut file_ops = FileOperations::new();
    let mut total_tokens = 0u64;

    // Prior branch summaries contribute their recorded file lists.
    for entry in entries {
        let Entry::BranchSummary { details, .. } = entry else {
            continue;
        };
        let Some(details) = details else {
            continue;
        };
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

    for entry in entries.iter().rev() {
        let Some(message) = get_message_from_entry(entry) else {
            continue;
        };
        extract_file_ops_from_message(&message, &mut file_ops);

        let tokens = estimate_tokens(&message);
        if token_budget > 0 && total_tokens.saturating_add(tokens) > token_budget {
            // Upstream admits the boundary checkpoint only while under 90%
            // of the budget (`totalTokens < tokenBudget * 0.9`, f64 compare).
            if matches!(
                entry,
                Entry::Compaction { .. } | Entry::BranchSummary { .. }
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

/// Upstream `BRANCH_SUMMARY_PREAMBLE` (`branch-summarization.ts:184-187`).
const BRANCH_SUMMARY_PREAMBLE: &str = "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n";

/// Upstream `BRANCH_SUMMARY_PROMPT` (`branch-summarization.ts:189-216`).
const BRANCH_SUMMARY_PROMPT: &str = "Create a structured summary of this conversation branch for context when returning later.\n\nUse this EXACT format:\n\n## Goal\n[What was the user trying to accomplish in this branch?]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Work that was started but not finished]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [What should happen next to continue this work]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Upstream `generateBranchSummary` (`branch-summarization.ts:219-234`):
/// generate a summary for abandoned branch entries through the default
/// transport, budgeting to the model's context window minus the reserve.
pub async fn generate_branch_summary(
    entries: &[Entry],
    options: GenerateBranchSummaryOptions,
    context: Context,
) -> Result<BranchSummaryResult, BranchSummaryError> {
    let GenerateBranchSummaryOptions {
        models,
        model,
        custom_instructions,
        replace_instructions,
        reserve_tokens,
        retry,
        callbacks,
    } = options;
    let reserve_tokens = reserve_tokens.unwrap_or(16_384);
    let context_window = if model.context_window != 0 {
        model.context_window
    } else {
        128_000
    };
    let preparation =
        prepare_branch_entries(entries, context_window.saturating_sub(reserve_tokens));
    let request = models_summary_request(models, model, retry, callbacks);
    generate_branch_summary_with_request(
        preparation,
        &PreparedBranchSummaryOptions {
            custom_instructions,
            replace_instructions,
        },
        &request,
        context,
    )
    .await
}

/// Upstream `generateBranchSummaryWithRequest`
/// (`branch-summarization.ts:242-300`): generate a prepared branch summary
/// through a caller-owned one-request boundary.
pub async fn generate_branch_summary_with_request(
    preparation: BranchPreparation,
    options: &PreparedBranchSummaryOptions,
    request: &SummaryRequest,
    context: Context,
) -> Result<BranchSummaryResult, BranchSummaryError> {
    let PreparedBranchSummaryOptions {
        custom_instructions,
        replace_instructions,
    } = options;
    let BranchPreparation {
        messages, file_ops, ..
    } = preparation;
    if messages.is_empty() {
        return Ok(BranchSummaryResult {
            summary: "No content to summarize".to_string(),
            usage: None,
            read_files: Vec::new(),
            modified_files: Vec::new(),
        });
    }
    let llm_messages = convert_to_llm(&messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let instructions = match custom_instructions {
        // `replaceInstructions && customInstructions` upstream.
        Some(custom_instructions) if *replace_instructions => custom_instructions.clone(),
        Some(custom_instructions) => {
            format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {custom_instructions}")
        }
        None => BRANCH_SUMMARY_PROMPT.to_string(),
    };
    let prompt_text =
        format!("<conversation>\n{conversation_text}\n</conversation>\n\n{instructions}");

    let response = request(
        crate::ai::transcript::Context {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: vec![Message::User(crate::ai::types::UserMessage {
                content: crate::ai::types::StringOrBlocks::Blocks(vec![
                    crate::ai::types::TextOrImageBlock::Text(crate::ai::types::TextContent {
                        text: prompt_text,
                        text_signature: None,
                    }),
                ]),
                timestamp: crate::ai::now_ms(),
            })],
            tools: None,
        },
        create_summary_request_options(
            SimpleStreamOptions {
                stream: crate::ai::types::StreamOptions {
                    max_tokens: Some(2048),
                    ..crate::ai::types::StreamOptions::default()
                },
                ..SimpleStreamOptions::default()
            },
            &context,
        ),
        context.clone(),
    )
    .await;
    if response.stop_reason == StopReason::Aborted {
        return Err(BranchSummaryError::new(
            BranchSummaryErrorCode::Aborted,
            response
                .error_message
                .unwrap_or_else(|| "Branch summary aborted".to_string()),
        ));
    }
    if response.stop_reason == StopReason::Error {
        return Err(BranchSummaryError::new(
            BranchSummaryErrorCode::SummarizationFailed,
            format!(
                "Branch summary failed: {}",
                response
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_string())
            ),
        ));
    }

    let mut summary = assistant_blocks_text(&response.content, "\n");
    summary = format!("{BRANCH_SUMMARY_PREAMBLE}{summary}");
    let FileLists {
        read_files,
        modified_files,
    } = compute_file_lists(&file_ops);
    summary.push_str(&format_file_operations(&read_files, &modified_files));

    Ok(BranchSummaryResult {
        summary: if summary.is_empty() {
            "No summary generated".to_string()
        } else {
            summary
        },
        usage: Some(response.usage),
        read_files,
        modified_files,
    })
}

#[cfg(test)]
mod tests;
