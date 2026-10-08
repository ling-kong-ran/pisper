//! Port of `src/harness/context.ts`: committed context bounds, active-entry
//! selection, and model-context derivation for one conversation.
//!
//! Divergences (structural, disclosed): the upstream functions are `async`
//! over promise storage; with the port's synchronous [`Storage`] (D3) they
//! are synchronous, and the Session-line capture (`readContext` first
//! awaits `readOnLine`) maps onto [`super::super::session::session::Session`]'s
//! synchronous on-line read surface.

use std::collections::BTreeMap;

use crate::ai::types::{Message, StopReason, TextContent, ToolCall, ToolResultMessage};

use super::super::errors::PlainError;
use super::super::ids::{ConversationId, EntryId};
use super::super::session::session::Session;
use super::super::storage::{Storage, StorageError};
use super::super::types::{ContextEdit, ContextEditAction, Cursor, EntryQuery, EntryRecord};
use super::types::ContextView;

const SCAN_PAGE_SIZE: usize = 256;
/// Stop reasons excluded from derived model context (`context.ts:8`).
const EXCLUDED_STOP_REASONS: [StopReason; 3] =
    [StopReason::Aborted, StopReason::Error, StopReason::Deferred];
const MISSING_RESULT_TEXT: &str =
    "Tool result unavailable: history ends before this call completed.";

/// Head marker and newest visible entry that fix one committed context range
/// (`context.ts` `ContextBounds`).
#[derive(Debug, Clone)]
pub struct ContextBounds {
    /// The head marker; its `head` value starts the active range.
    pub head: Option<EntryRecord>,
    /// Newest visible entry ID of the range.
    pub tail: EntryId,
}

/// Capture the bounds of the current context, or of the context cut off at
/// the visible entry `at`, with two O(1) reads (`context.ts`
/// `captureContextBounds`). Run this on the Session line; entries at or
/// below the tail are immutable, so [`derive_context`] can then scan them
/// off the line.
pub fn capture_context_bounds(
    storage: &dyn Storage,
    conversation_id: ConversationId,
    context: &crate::agent_core::chord_support::context::Context,
    at: Option<EntryId>,
) -> Result<Option<ContextBounds>, PlainError> {
    let tail = match at {
        None => {
            let page = storage
                .scan_entries(
                    EntryQuery {
                        conversation_id,
                        min_entry_id: None,
                        max_entry_id: None,
                    },
                    1,
                    None,
                    context,
                )
                .map_err(storage_error)?;
            match page.items.first() {
                Some(entry) => entry.id,
                None => return Ok(None),
            }
        }
        Some(at) => {
            if storage
                .entry_visible(conversation_id, at, context)
                .map_err(storage_error)?
                .is_none()
            {
                return Err(PlainError::new(format!(
                    "Entry {at} is not visible from conversation {conversation_id}"
                )));
            }
            at
        }
    };
    let head = storage
        .find_latest_head_marker(conversation_id, Some(tail), context)
        .map_err(storage_error)?;
    Ok(Some(ContextBounds { head, tail }))
}

fn storage_error(error: StorageError) -> PlainError {
    PlainError::new(error.to_string())
}

/// Committed context of one conversation (`context.ts` `readContext`):
/// bounds captured on the Session line, entries derived off it.
pub async fn read_context(
    session: &Session,
    storage: &dyn Storage,
    conversation_id: ConversationId,
    context: &crate::agent_core::chord_support::context::Context,
    at: Option<EntryId>,
) -> Result<ContextView, PlainError> {
    let bounds = session
        .read_on_line(|| async { capture_context_bounds(storage, conversation_id, context, at) })
        .await?;
    derive_context(storage, conversation_id, bounds, context)
}

/// Derive the active transcript and model context of one conversation within
/// captured bounds (`context.ts` `deriveContext`).
///
/// H = newest visible head marker; the range runs from `H.head` (or
/// transcript start) through the tail. Per target, the newest edit in the
/// range wins. Context entries are H followed by the range's non-head
/// entries.
pub fn derive_context(
    storage: &dyn Storage,
    conversation_id: ConversationId,
    bounds: Option<ContextBounds>,
    context: &crate::agent_core::chord_support::context::Context,
) -> Result<ContextView, PlainError> {
    let Some(bounds) = bounds else {
        return Ok(ContextView {
            head: None,
            entries: Vec::new(),
            contributions: Vec::new(),
            messages: Vec::new(),
        });
    };
    let head = bounds.head.clone();
    let range = scan_range(storage, conversation_id, &bounds, context)?;
    let mut edits: BTreeMap<EntryId, ContextEdit> = BTreeMap::new();
    // Edits of every entry in the range count, including older head markers
    // that `selectActive()` drops.
    for entry in &range {
        for edit in entry.edits.clone().unwrap_or_default() {
            edits.insert(edit.target, edit);
        }
    }
    let entries = select_active(head.as_ref(), &range);
    // Per entry (v1.0.0): its messages after edits and excluded stop reasons,
    // before tool result ordering.
    let mut messages: Vec<Message> = Vec::new();
    let mut contributions: Vec<Vec<Message>> = Vec::with_capacity(entries.len());
    for entry in &entries {
        let edit = edits.get(&entry.id);
        let contributed: Vec<Message> = if matches!(edit, Some(edit) if edit.action == ContextEditAction::Omit)
        {
            Vec::new()
        } else {
            match edit {
                Some(edit) if edit.action == ContextEditAction::Replace => {
                    edit.messages.clone().unwrap_or_default()
                }
                _ => entry.model.clone().unwrap_or_default(),
            }
        };
        let mut contribution: Vec<Message> = Vec::with_capacity(contributed.len());
        for message in contributed {
            if let Message::Assistant(assistant) = &message {
                if EXCLUDED_STOP_REASONS.contains(&assistant.stop_reason) {
                    continue;
                }
            }
            messages.push(message.clone());
            contribution.push(message);
        }
        contributions.push(contribution);
    }
    Ok(ContextView {
        head,
        entries,
        contributions,
        messages: order_tool_results(messages),
    })
}

/// The raw active entries within captured bounds, without deriving model
/// context (`context.ts` `activeEntries`).
pub fn active_entries(
    storage: &dyn Storage,
    conversation_id: ConversationId,
    bounds: Option<ContextBounds>,
    context: &crate::agent_core::chord_support::context::Context,
) -> Result<Vec<EntryRecord>, PlainError> {
    let Some(bounds) = bounds else {
        return Ok(Vec::new());
    };
    let range = scan_range(storage, conversation_id, &bounds, context)?;
    Ok(select_active(bounds.head.as_ref(), &range))
}

/// Visible entries from the head marker's head, or transcript start, through
/// the tail, oldest first (`context.ts` `scanRange`).
fn scan_range(
    storage: &dyn Storage,
    conversation_id: ConversationId,
    bounds: &ContextBounds,
    context: &crate::agent_core::chord_support::context::Context,
) -> Result<Vec<EntryRecord>, PlainError> {
    let head = bounds.head.as_ref();
    let mut range: Vec<EntryRecord> = Vec::new();
    let mut cursor: Option<Cursor> = None;
    loop {
        let query = EntryQuery {
            conversation_id,
            min_entry_id: head.map(|head| head.head.unwrap_or(0)),
            max_entry_id: Some(bounds.tail),
        };
        let page = storage
            .scan_entries(query, SCAN_PAGE_SIZE, cursor.as_ref(), context)
            .map_err(storage_error)?;
        let next = page.next.clone();
        range.extend(page.items);
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    range.reverse();
    Ok(range)
}

/// The head marker followed by the range's non-head entries, or the whole
/// range without a marker (`context.ts` `selectActive`).
fn select_active(head: Option<&EntryRecord>, range: &[EntryRecord]) -> Vec<EntryRecord> {
    match head {
        None => range.to_vec(),
        Some(head) => {
            let mut entries = vec![head.clone()];
            entries.extend(range.iter().filter(|entry| entry.head.is_none()).cloned());
            entries
        }
    }
}

/// Place each assistant's tool results directly after it in call order
/// (`context.ts` `orderToolResults`, `pub` upstream since v1.0.0 — the
/// compaction module's `summarizedMessages` reuses it). Results are taken
/// from the messages before the next assistant; a missing result is
/// synthesized and unmatched results are dropped.
pub(crate) fn order_tool_results(messages: Vec<Message>) -> Vec<Message> {
    let mut ordered: Vec<Message> = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = messages[index].clone();
        if let Message::ToolResult(_) = message {
            index += 1;
            continue;
        }
        ordered.push(message.clone());
        let Message::Assistant(assistant) = &message else {
            index += 1;
            continue;
        };
        let calls: Vec<&ToolCall> = assistant
            .content
            .iter()
            .filter_map(|block| match block {
                crate::ai::types::AssistantBlock::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect();
        if calls.is_empty() {
            index += 1;
            continue;
        }
        let mut results: BTreeMap<String, usize> = BTreeMap::new();
        let mut next = index + 1;
        while next < messages.len() && !matches!(messages[next], Message::Assistant(_)) {
            if let Message::ToolResult(candidate) = &messages[next] {
                results
                    .entry(candidate.tool_call_id.clone())
                    .or_insert(next);
            }
            next += 1;
        }
        for call in calls {
            match results.get(&call.id) {
                Some(result_index) => ordered.push(messages[*result_index].clone()),
                None => ordered.push(Message::ToolResult(missing_result(
                    call,
                    assistant.timestamp,
                ))),
            }
        }
        index += 1;
    }
    ordered
}

fn missing_result(call: &ToolCall, timestamp: i64) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content: vec![crate::ai::types::TextOrImageBlock::Text(TextContent {
            text: MISSING_RESULT_TEXT.to_string(),
            text_signature: None,
        })],
        details: Some(serde_json::json!({"reason": "missing_result"})),
        usage: None,
        is_error: true,
        timestamp,
    }
}
