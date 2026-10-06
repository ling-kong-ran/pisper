//! Port of `src/harness/compaction.ts` (v1.0.0): the built-in compaction
//! machinery — range selection (`selectCut`), the summarizer source and
//! prompt assembly (`summarizedMessages`, `serializeConversation`,
//! `summaryPrompt`), context-size estimation (`estimateContext`), and the
//! summary/failure classification of a summarization response — plus the
//! pinned prompt constants and request/checkpoint data shapes.
//!
//! Port scope (disclosed): this module carries every pure, deterministic
//! part of `compaction.ts` with unit tests ported from the upstream
//! `harness-compaction.test.ts` "range selection" and "serialization"
//! suites. The `CompactionTask` phase machine itself (its `select`,
//! `summarize`, `retry`, and `abort` phases, `createCompaction`, `place`,
//! `placeSummary`, `complete`, and `failNoModel`) drives the v1.0.0 task
//! runtime — `runtime.agent`, `runtime.settings`, extension hooks through
//! the agent, task-owned compactions, and `pi.live.compactions` statuses —
//! which belongs to the v1.0.0 harness rework (the `pi.agent` document,
//! `HarnessOptions.settings`, and the Extension registry replacing
//! `pi.conversation.config` and the batch registry). This port keeps the
//! received base harness architecture (`pi.conversation.config`), so the
//! phase machine has no runtime to run on and is not ported; its data types
//! (`CompactionInput`, `SummaryRequest`, `CompactionCheckpoint`) and every
//! decision procedure it delegates to are here.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::estimate::{calculate_context_tokens, estimate_message_tokens};
use crate::ai::types::message::{AssistantBlock, AssistantMessage, Message, StringOrBlocks};
use crate::ai::types::primitives::StopReason;

use super::context::order_tool_results;
use super::types::CompactionReason;

/// Upstream `CompactionInput` (compaction.ts:30): the task input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionInput {
    pub reason: CompactionReason,
    /// Extra focus passed to the summarizer's prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Upstream `SummaryRequest` (compaction.ts:33-44): the pinned
/// summarization request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRequest {
    pub attempt: i64,
    /// The summarization model reference: `[provider, modelId]`, as the
    /// durable `ModelRef` serializes.
    pub model: (String, String),
    /// Upstream `ModelThinkingLevel` string (`"off"` included).
    pub thinking_level: String,
    /// Forwarded request options (`ConversationStreamOptions` JSON).
    pub stream_options: Value,
    pub max_tokens: i64,
    /// Newest entry of the context the range was selected from.
    pub tail: super::super::types::EntryId,
    /// First entry kept verbatim; the summary's `head`.
    pub first_kept: super::super::types::EntryId,
}

/// Upstream `CompactionCheckpoint` (compaction.ts:45-48): the task's phases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum CompactionCheckpoint {
    /// Select the range to summarize.
    #[serde(rename = "select")]
    Select,
    /// Summarize with the pinned request.
    #[serde(rename = "summarize")]
    Summarize(SummaryRequest),
    /// Wait out the retry backoff, then summarize again.
    #[serde(rename = "retry")]
    Retry {
        /// Epoch millis of the next attempt.
        until: i64,
        #[serde(flatten)]
        request: SummaryRequest,
    },
}

/// Upstream `TOOL_RESULT_MAX_CHARS` (compaction.ts:54): longest tool result
/// text a serialized summary source keeps.
pub const TOOL_RESULT_MAX_CHARS: usize = 2000;

/// Upstream `SUMMARY_PREFIX` (compaction.ts:56-57).
pub const SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
/// Upstream `SUMMARY_SUFFIX` (compaction.ts:58).
pub const SUMMARY_SUFFIX: &str = "\n</summary>";

/// Upstream `SUMMARIZATION_SYSTEM_PROMPT` (compaction.ts:60-63).
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

/// Upstream `SUMMARIZATION_PROMPT` (compaction.ts:65-93).
pub const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work. If the conversation starts with an earlier summary, preserve its information and fold the newer messages into it.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Index in `view.entries` of the first entry a summary keeps, or `None`
/// when there is nothing to compact (spec §8.7) — upstream `selectCut`
/// (compaction.ts:253-271). Walks back from the tail until
/// `keepRecentTokens` are kept, then cuts at the first candidate at or after
/// that entry: an entry whose contribution starts with a user or assistant
/// message, never a tool result, and never a user entry that a result of the
/// preceding assistant's calls still follows.
pub fn select_cut(view: &super::types::ContextView, keep_recent_tokens: i64) -> Option<usize> {
    let contributions = &view.contributions;
    let start = if view.head.is_none() { 0 } else { 1 };
    let mut candidates: Vec<usize> = Vec::new();
    for index in start..contributions.len() {
        if is_candidate(contributions, index) {
            candidates.push(index);
        }
    }
    let mut kept: i64 = 0;
    let mut cut: Option<usize> = None;
    for index in (start..contributions.len()).rev() {
        for message in &contributions[index] {
            kept += estimate_message_tokens(message) as i64;
        }
        if kept < keep_recent_tokens {
            continue;
        }
        cut = Some(
            candidates
                .iter()
                .find(|candidate| **candidate >= index)
                .copied()
                .or_else(|| candidates.last().copied())?,
        );
        break;
    }
    let cut = cut?;
    for contribution in contributions.iter().take(cut).skip(start) {
        if !contribution.is_empty() {
            return Some(cut);
        }
    }
    None
}

/// Upstream `isCandidate` (compaction.ts:273-293): whether entry `index` may
/// carry the summary's cut.
fn is_candidate(contributions: &[Vec<Message>], index: usize) -> bool {
    let first = contributions[index].first();
    match first {
        Some(Message::Assistant(_)) => return true,
        Some(Message::User(_)) => {}
        _ => return false,
    }
    // A result of the preceding assistant's calls that follows this entry,
    // before the next assistant, belongs before it.
    let mut calls: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for before in (0..index).rev() {
        let Some(assistant) = contributions[before]
            .iter()
            .rev()
            .find(|message| matches!(message, Message::Assistant(_)))
        else {
            continue;
        };
        if let Message::Assistant(assistant) = assistant {
            calls = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::ToolCall(call) => Some(call.id.clone()),
                    _ => None,
                })
                .collect();
        }
        break;
    }
    if calls.is_empty() {
        return true;
    }
    for (after, contribution) in contributions.iter().enumerate().skip(index) {
        for (position, message) in contribution.iter().enumerate() {
            match message {
                Message::Assistant(_) if after > index || position > 0 => return true,
                Message::ToolResult(result) if calls.contains(&result.tool_call_id) => {
                    return false;
                }
                _ => {}
            }
        }
    }
    true
}

/// Model messages of the entries before `cut`: the head marker first, ordered
/// like model context (spec §2.1) — upstream `summarizedMessages`
/// (compaction.ts:296-298).
pub fn summarized_messages(view: &super::types::ContextView, cut: usize) -> Vec<Message> {
    let flattened: Vec<Message> = view
        .contributions
        .iter()
        .take(cut)
        .flatten()
        .cloned()
        .collect();
    order_tool_results(flattened)
}

/// Size of a request over `view` followed by `extra` (spec §8.3) — upstream
/// `estimateContext` (compaction.ts:305-321): the usage of the newest
/// assistant appended after the head marker, whose request included the
/// marker, plus estimates of the messages after it; without one, estimates
/// of every message.
pub fn estimate_context(view: &super::types::ContextView, extra: &[Message]) -> i64 {
    let mut measured: Option<&AssistantMessage> = None;
    let after = view.head.as_ref().map_or(i64::MIN, |head| head.id);
    for index in (0..view.entries.len()).rev() {
        if measured.is_some() {
            break;
        }
        if view.entries[index].id <= after {
            continue;
        }
        measured = view.contributions[index]
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Assistant(assistant) if calculate_context_tokens(&assistant.usage) > 0 => {
                    Some(assistant)
                }
                _ => None,
            });
    }
    let (mut tokens, from) = match measured {
        None => (0i64, 0usize),
        Some(measured) => {
            let from = view
                .messages
                .iter()
                .rposition(|message| message == &Message::Assistant(measured.clone()))
                .map_or(0, |position| position + 1);
            (calculate_context_tokens(&measured.usage) as i64, from)
        }
    };
    for message in view.messages.iter().skip(from) {
        tokens += estimate_message_tokens(message) as i64;
    }
    for message in extra {
        tokens += estimate_message_tokens(message) as i64;
    }
    tokens
}

/// The summary of a clean `stop` with text and no tool call; anything else is
/// not a summary — upstream `summaryText` (compaction.ts:323-331).
pub fn summary_text(message: &AssistantMessage) -> Option<String> {
    if message.stop_reason != StopReason::Stop
        || message
            .content
            .iter()
            .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
    {
        return None;
    }
    let text = text_blocks(message).join("\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Upstream `summaryFailure` (compaction.ts:333-341): why the response is
/// not a usable summary.
pub fn summary_failure(message: &AssistantMessage) -> String {
    if message.stop_reason == StopReason::Error || message.stop_reason == StopReason::Aborted {
        return format!(
            "Summarization failed: {}",
            message
                .error_message
                .as_deref()
                .unwrap_or(match message.stop_reason {
                    StopReason::Error => "error",
                    StopReason::Aborted => "aborted",
                    _ => "",
                })
        );
    }
    if message.stop_reason == StopReason::Length {
        return String::from("Summarization hit the token limit; the summary is incomplete");
    }
    if message
        .content
        .iter()
        .any(|block| matches!(block, AssistantBlock::ToolCall(_)))
    {
        return String::from("Summarization attempted to call a tool");
    }
    String::from("Summarization produced no text")
}

/// The summarizer's user message: the serialized conversation, the prompt,
/// and any instructions — upstream `summaryPrompt` (compaction.ts:343-347).
pub fn summary_prompt(messages: &[Message], instructions: Option<&str>) -> String {
    let focus = instructions
        .map(|instructions| format!("\n\nAdditional focus: {instructions}"))
        .unwrap_or_default();
    format!(
        "<conversation>\n{}\n</conversation>\n\n{}{}",
        serialize_conversation(messages),
        SUMMARIZATION_PROMPT,
        focus
    )
}

/// Messages as plain text, so the summarizer reads a transcript instead of
/// continuing it; system messages are omitted — upstream
/// `serializeConversation` (compaction.ts:349-376).
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for message in messages {
        match message {
            Message::User(user) => {
                let text = content_text(&user.content);
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            Message::Assistant(assistant) => {
                let thinking: Vec<&str> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::Thinking(thinking) => Some(thinking.thinking.as_str()),
                        _ => None,
                    })
                    .collect();
                let text = text_blocks(assistant);
                let calls: Vec<String> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantBlock::ToolCall(call) => {
                            let arguments = match &call.arguments {
                                Value::Object(map) => map
                                    .iter()
                                    .map(|(key, value)| {
                                        format!(
                                            "{}={}",
                                            key,
                                            serde_json::to_string(value).unwrap_or_default()
                                        )
                                    })
                                    .collect::<Vec<String>>()
                                    .join(", "),
                                value => serde_json::to_string(value).unwrap_or_default(),
                            };
                            Some(format!("{}({})", call.name, arguments))
                        }
                        _ => None,
                    })
                    .collect();
                if !thinking.is_empty() {
                    parts.push(format!("[Assistant thinking]: {}", thinking.join("\n")));
                }
                if !text.is_empty() {
                    parts.push(format!("[Assistant]: {}", text.join("\n")));
                }
                if !calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
                }
            }
            Message::ToolResult(result) => {
                let text = text_or_image_text(&result.content);
                if !text.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate(&text, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            Message::System(_) => {}
        }
    }
    parts.join("\n\n")
}

/// The assistant's text blocks in order (upstream's inline
/// `content.flatMap(... type === "text" ...)`).
fn text_blocks(assistant: &AssistantMessage) -> Vec<&str> {
    assistant
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

/// Upstream `contentText` (compaction.ts:378-384).
fn content_text(content: &StringOrBlocks) -> String {
    match content {
        StringOrBlocks::Text(text) => text.clone(),
        StringOrBlocks::Blocks(blocks) => text_or_image_text(blocks),
    }
}

/// The text of text blocks joined by newlines, as upstream's `contentText`
/// block branch.
fn text_or_image_text(blocks: &[crate::ai::types::message::TextOrImageBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            crate::ai::types::message::TextOrImageBlock::Text(text) => Some(text.text.as_str()),
            crate::ai::types::message::TextOrImageBlock::Image(_) => None,
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

/// Upstream `truncate` (compaction.ts:385-389); lengths count UTF-16 code
/// units like upstream's string `.length`.
fn truncate(text: &str, max_chars: usize) -> String {
    let total = text.encode_utf16().count();
    if total <= max_chars {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut units = 0;
    for character in text.chars() {
        let length = character.len_utf16();
        if units + length > max_chars {
            break;
        }
        units += length;
        kept.push(character);
    }
    format!(
        "{kept}\n\n[... {} more characters truncated]",
        total - max_chars
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::models::faux::{
        faux_assistant_message, faux_text, faux_tool_call, FauxMessageOptions, FauxToolCallOptions,
    };
    use crate::ai::types::content::{TextContent, ThinkingContent};
    use crate::ai::types::message::{
        StringOrBlocks, SystemMessage, TextOrImageBlock, ToolResultMessage, UserMessage,
    };
    use crate::ai::types::primitives::{StopReason, Usage, UsageCost};
    use crate::durable::types::{EntryId, EntryRecord};

    // ── Upstream `harness-compaction.test.ts` helpers (lines 176-218). ──────

    fn next_id() -> EntryId {
        use std::sync::atomic::{AtomicI64, Ordering};
        static NEXT: AtomicI64 = AtomicI64::new(1);
        NEXT.fetch_add(1, Ordering::SeqCst)
    }

    fn entry(kind: &str, model: Vec<Message>, head: Option<EntryId>) -> EntryRecord {
        EntryRecord {
            kind: kind.to_string(),
            model: Some(model),
            data: None,
            edits: None,
            id: next_id(),
            conversation_id: 1,
            head,
            by_task_id: None,
        }
    }

    /// Text of about `tokens` estimated tokens, starting with `label`.
    fn text(label: &str, tokens: usize) -> String {
        let repeat = tokens.saturating_mul(4).saturating_sub(label.len() + 1);
        format!("{label} {}", "x".repeat(repeat))
    }

    fn user(content: &str) -> Message {
        Message::User(UserMessage {
            content: StringOrBlocks::Text(content.to_string()),
            timestamp: 0,
        })
    }

    fn assistant(content: &str, calls: &[&str]) -> AssistantMessage {
        let mut blocks = vec![faux_text(content)];
        for call in calls {
            blocks.push(faux_tool_call(
                "read",
                serde_json::json!({}),
                FauxToolCallOptions {
                    id: Some((*call).to_string()),
                },
            ));
        }
        faux_assistant_message(
            blocks,
            FauxMessageOptions {
                timestamp: Some(0),
                ..FauxMessageOptions::default()
            },
        )
    }

    fn assistant_stopped(content: &str, stop_reason: StopReason) -> AssistantMessage {
        faux_assistant_message(
            content,
            FauxMessageOptions {
                stop_reason: Some(stop_reason),
                timestamp: Some(0),
                ..FauxMessageOptions::default()
            },
        )
    }

    fn tool_result(call_id: &str, content: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id.to_string(),
            tool_name: "read".to_string(),
            content: vec![TextOrImageBlock::Text(TextContent {
                text: content.to_string(),
                text_signature: None,
            })],
            details: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        })
    }

    /// A view over `entries` whose contributions are their models, with
    /// excluded assistants removed (upstream test `view()`).
    fn view(
        entries: Vec<EntryRecord>,
        head: Option<EntryRecord>,
    ) -> super::super::types::ContextView {
        let mut all = Vec::new();
        if let Some(head) = &head {
            all.push(head.clone());
        }
        all.extend(entries);
        let contributions: Vec<Vec<Message>> = all
            .iter()
            .map(|record| {
                record
                    .model
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|message| match message {
                        Message::Assistant(assistant) => !matches!(
                            assistant.stop_reason,
                            StopReason::Error | StopReason::Aborted | StopReason::Deferred
                        ),
                        _ => true,
                    })
                    .collect()
            })
            .collect();
        let messages = order_tool_results(contributions.iter().flatten().cloned().collect());
        super::super::types::ContextView {
            head,
            entries: all,
            contributions,
            messages,
        }
    }

    fn usage_of(total: u64) -> Usage {
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

    // ── Upstream "range selection" suite (harness-compaction.test.ts). ─────

    #[test]
    fn keeps_about_keep_recent_tokens_and_cuts_at_the_first_candidate_at_or_after_the_budget() {
        let entries = vec![
            entry("pi.user", vec![user(&text("1", 10))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant("2", &["c1"]))],
                None,
            ),
            entry(
                "pi.tool-result",
                vec![tool_result("c1", &text("3", 3000))],
                None,
            ),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("4", 10), &[]))],
                None,
            ),
            entry("pi.user", vec![user(&text("5", 10))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("6", 10), &[]))],
                None,
            ),
        ];
        assert_eq!(select_cut(&view(entries, None), 2000), Some(3));
    }

    #[test]
    fn cuts_at_a_user_entry() {
        let entries = vec![
            entry("pi.user", vec![user(&text("u1", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a1", 100), &[]))],
                None,
            ),
            entry("pi.user", vec![user(&text("u2", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a2", 100), &[]))],
                None,
            ),
        ];
        assert_eq!(select_cut(&view(entries, None), 150), Some(2));
    }

    #[test]
    fn cuts_at_an_assistant_in_the_middle_of_one_long_run_and_never_at_a_tool_result() {
        let mut entries = vec![entry("pi.user", vec![user("do it")], None)];
        for index in 0..5 {
            entries.push(entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(
                    &format!("step {index}"),
                    &[&format!("c{index}")],
                ))],
                None,
            ));
            entries.push(entry(
                "pi.tool-result",
                vec![tool_result(
                    &format!("c{index}"),
                    &text(&format!("r{index}"), 100),
                )],
                None,
            ));
        }
        let selected = view(entries, None);
        let cut = select_cut(&selected, 150).unwrap();
        // The budget is reached at the fourth result; the cut is the last
        // call, whose result it keeps.
        assert_eq!(selected.entries[cut].kind, "pi.assistant");
        assert_eq!(cut, 9);
    }

    #[test]
    fn keeps_a_huge_last_tool_result_together_with_its_assistant() {
        let entries = vec![
            entry("pi.user", vec![user("u")], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant("a", &["c"]))],
                None,
            ),
            entry(
                "pi.tool-result",
                vec![tool_result("c", &text("big", 5000))],
                None,
            ),
        ];
        assert_eq!(select_cut(&view(entries, None), 100), Some(1));
    }

    #[test]
    fn never_cuts_at_a_system_entry_or_an_excluded_error_or_aborted_answer() {
        let entries = vec![
            entry("pi.user", vec![user(&text("u1", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a1", 100), &[]))],
                None,
            ),
            entry(
                "pi.system",
                vec![Message::System(SystemMessage {
                    content: StringOrBlocks::Text(String::new()),
                    sections: Some(crate::ai::types::message::Sections::new(vec![(
                        "s".to_string(),
                        Some(text("s", 100)),
                    )])),
                    tools_added: None,
                    tools_removed: None,
                    timestamp: 0,
                })],
                None,
            ),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant_stopped(
                    &text("err", 100),
                    StopReason::Error,
                ))],
                None,
            ),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant_stopped(
                    &text("stopped", 100),
                    StopReason::Aborted,
                ))],
                None,
            ),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a2", 100), &[]))],
                None,
            ),
        ];
        // The walk reaches 150 at the system entry; the excluded answers
        // after it contribute nothing.
        assert_eq!(select_cut(&view(entries, None), 150), Some(5));
    }

    #[test]
    fn follows_edited_contributions_an_omitted_entry_adds_nothing_and_is_no_candidate() {
        let entries = vec![
            entry("pi.user", vec![user(&text("u1", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a1", 100), &[]))],
                None,
            ),
            entry("pi.user", vec![user(&text("u2", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a2", 100), &[]))],
                None,
            ),
        ];
        let plain = view(entries, None);
        assert_eq!(select_cut(&plain, 150), Some(2));
        let contributions: Vec<Vec<Message>> = plain
            .contributions
            .iter()
            .enumerate()
            .map(|(index, messages)| {
                if index == 2 {
                    Vec::new()
                } else {
                    messages.clone()
                }
            })
            .collect();
        let omitted = super::super::types::ContextView {
            messages: order_tool_results(contributions.iter().flatten().cloned().collect()),
            contributions,
            ..plain
        };
        assert_eq!(select_cut(&omitted, 150), Some(1));
    }

    #[test]
    fn does_not_cut_at_a_user_entry_that_a_result_of_the_preceding_call_still_follows() {
        let entries = vec![
            entry("pi.user", vec![user(&text("u1", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant("a", &["c"]))],
                None,
            ),
            entry("pi.user", vec![user(&text("steer", 100))], None),
            entry(
                "pi.tool-result",
                vec![tool_result("c", &text("r", 100))],
                None,
            ),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a2", 100), &[]))],
                None,
            ),
        ];
        // The budget is reached at the steer; its result follows it, so the
        // cut moves to the next assistant.
        assert_eq!(select_cut(&view(entries, None), 250), Some(4));
    }

    #[test]
    fn finds_nothing_when_the_budget_is_never_reached_or_only_the_marker_precedes_the_cut() {
        let small = vec![
            entry("pi.user", vec![user("hi")], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant("hello", &[]))],
                None,
            ),
        ];
        assert_eq!(select_cut(&view(small, None), 150), None);
        let marker = entry("pi.compaction", vec![user("summary")], Some(0));
        // The budget is reached at the only entry after the marker, so the
        // marker alone would be summarized.
        let after = vec![entry("pi.user", vec![user(&text("u", 200))], None)];
        assert_eq!(select_cut(&view(after, Some(marker)), 150), None);
    }

    #[test]
    fn summarizes_an_earlier_summary_marker_first() {
        let marker = entry("pi.compaction", vec![user("EARLIER")], Some(0));
        let kept = vec![
            entry("pi.user", vec![user(&text("u1", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a1", 100), &[]))],
                None,
            ),
            entry("pi.user", vec![user(&text("u2", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant(&text("a2", 100), &[]))],
                None,
            ),
        ];
        let selected = view(kept, Some(marker));
        assert_eq!(select_cut(&selected, 150), Some(3));
        let serialized = serialize_conversation(
            &selected
                .contributions
                .iter()
                .take(3)
                .flatten()
                .cloned()
                .collect::<Vec<_>>(),
        );
        assert!(serialized.starts_with("[User]: EARLIER"), "{serialized}");
    }

    // ── Upstream "serialization" suite. ─────────────────────────────────────

    #[test]
    fn writes_a_transcript_truncates_tool_results_and_omits_system_messages() {
        let call = faux_tool_call(
            "read",
            serde_json::json!({ "path": "a.ts" }),
            FauxToolCallOptions {
                id: Some("c".into()),
            },
        );
        let messages = vec![
            Message::System(SystemMessage {
                content: StringOrBlocks::Text(String::new()),
                sections: Some(crate::ai::types::message::Sections::new(vec![(
                    "s".to_string(),
                    Some("hidden".to_string()),
                )])),
                tools_added: None,
                tools_removed: None,
                timestamp: 0,
            }),
            user("hello"),
            Message::Assistant(faux_assistant_message(
                vec![
                    AssistantBlock::Thinking(ThinkingContent {
                        thinking: "hmm".to_string(),
                        thinking_signature: None,
                        redacted: None,
                    }),
                    faux_text("sure"),
                    call,
                ],
                FauxMessageOptions::default(),
            )),
            tool_result("c", &"y".repeat(2500)),
        ];
        let serialized = serialize_conversation(&messages);
        assert!(!serialized.contains("hidden"), "{serialized}");
        assert!(serialized.contains("[User]: hello"), "{serialized}");
        assert!(
            serialized.contains("[Assistant thinking]: hmm"),
            "{serialized}"
        );
        assert!(serialized.contains("[Assistant]: sure"), "{serialized}");
        assert!(
            serialized.contains("[Assistant tool calls]: read(path=\"a.ts\")"),
            "{serialized}"
        );
        assert!(
            serialized.contains(&format!(
                "[Tool result]: {}\n\n[... 500 more characters truncated]",
                "y".repeat(2000)
            )),
            "{serialized}"
        );
    }

    // ── Prompt assembly, estimation, and classification. ───────────────────

    #[test]
    fn summary_prompt_wraps_the_conversation_and_appends_focus() {
        let messages = vec![user("hello")];
        let prompt = summary_prompt(&messages, None);
        assert!(prompt.starts_with("<conversation>\n[User]: hello\n</conversation>\n\n"));
        assert!(prompt.ends_with(SUMMARIZATION_PROMPT));
        let focused = summary_prompt(&messages, Some("the database"));
        assert!(focused.ends_with("\n\nAdditional focus: the database"));
    }

    #[test]
    fn estimate_context_uses_the_newest_kept_usage_plus_trailing_estimates() {
        let mut answered = assistant_stopped("answer", StopReason::Stop);
        answered.usage = usage_of(500);
        let entries = vec![
            entry("pi.user", vec![user(&text("u", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(answered.clone())],
                None,
            ),
            entry("pi.user", vec![user(&text("u2", 100))], None),
        ];
        let view = view(entries, None);
        let tokens = estimate_context(&view, &[]);
        // The newest assistant's usage (500) covers everything through its
        // own request; only the trailing user entry is estimated on top.
        let trailing = estimate_message_tokens(&user(&text("u2", 100))) as i64;
        assert_eq!(tokens, 500 + trailing);
    }

    #[test]
    fn estimate_context_without_usage_estimates_every_message() {
        let entries = vec![
            entry("pi.user", vec![user(&text("u", 100))], None),
            entry(
                "pi.assistant",
                vec![Message::Assistant(assistant("hello", &[]))],
                None,
            ),
        ];
        let view = view(entries, None);
        let expected: i64 = view
            .messages
            .iter()
            .map(|m| estimate_message_tokens(m) as i64)
            .sum();
        assert_eq!(estimate_context(&view, &[]), expected);
    }

    #[test]
    fn summary_text_and_failure_classify_summarization_responses() {
        let mut good = assistant_stopped("  SUMMARY  ", StopReason::Stop);
        good.usage = usage_of(1);
        assert_eq!(summary_text(&good).as_deref(), Some("SUMMARY"));

        let calls_tool = faux_assistant_message(
            vec![
                faux_text("x"),
                faux_tool_call(
                    "read",
                    serde_json::json!({}),
                    FauxToolCallOptions::default(),
                ),
            ],
            FauxMessageOptions::default(),
        );
        assert_eq!(summary_text(&calls_tool), None);
        assert_eq!(
            summary_failure(&calls_tool),
            "Summarization attempted to call a tool"
        );

        let mut failed = assistant_stopped("", StopReason::Error);
        failed.error_message = Some("boom".into());
        assert_eq!(summary_text(&failed), None);
        assert_eq!(summary_failure(&failed), "Summarization failed: boom");

        let aborted = assistant_stopped("", StopReason::Aborted);
        assert_eq!(summary_failure(&aborted), "Summarization failed: aborted");

        let length = assistant_stopped("partial", StopReason::Length);
        assert_eq!(summary_text(&length), None);
        assert_eq!(
            summary_failure(&length),
            "Summarization hit the token limit; the summary is incomplete"
        );

        let empty = assistant_stopped("   ", StopReason::Stop);
        assert_eq!(summary_text(&empty), None);
        assert_eq!(summary_failure(&empty), "Summarization produced no text");
    }

    #[test]
    fn checkpoint_wire_shape_matches_upstream_field_names() {
        let checkpoint = CompactionCheckpoint::Retry {
            until: 42,
            request: SummaryRequest {
                attempt: 2,
                model: ("anthropic".into(), "claude".into()),
                thinking_level: "off".into(),
                stream_options: serde_json::json!({}),
                max_tokens: 1000,
                tail: 7,
                first_kept: 3,
            },
        };
        let json = serde_json::to_value(&checkpoint).unwrap();
        assert_eq!(json["phase"], "retry");
        assert_eq!(json["until"], 42);
        assert_eq!(json["attempt"], 2);
        assert_eq!(json["maxTokens"], 1000);
        assert_eq!(json["firstKept"], 3);
        let select = serde_json::to_value(CompactionCheckpoint::Select).unwrap();
        assert_eq!(select["phase"], "select");
    }
}
