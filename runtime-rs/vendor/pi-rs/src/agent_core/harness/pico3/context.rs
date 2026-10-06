//! Port of `packages/agent/src/harness/pico3/context.ts` (105 lines): the
//! model-context projection (`deriveContext`), ported here because
//! `pico3/session.ts:4` imports it directly (it is Task 9's `system.ts`
//! sibling, but has no consumers outside the storage engine).
//!
//! pico §1.3, verbatim (`context.ts:4-10`):
//! ```text
//! H       = newest fork-visible entry at or before T with a head
//! from    = H ? H.head : transcript start
//! range   = fork-visible entries from `from` through T
//! edits   = per target, newest edit in range wins
//! entries = H ? [H, ...range without any head entries] : range
//! model   = concat(entries.map(e => edits[e.id] ? apply : e.model)), then reorder tool results
//! ```
//!
//! Display-only entries (aborted/error assistants, pi.usage, model-less
//! plugin entries) have no `model` and contribute nothing. Nothing here
//! inspects an error string.
//!
//! Disclosed substitution: `StoredMessage` stays a JSON value; the
//! assistant/tool-result discrimination in [`reorder_tool_results`] reads the
//! `role`/`content`/`toolCallId` fields off the JSON objects exactly as the
//! upstream `StoredAssistant`/`StoredToolResult`/`StoredToolCall` extracts do
//! (`context.ts:68-70`).

use serde_json::Value;

use crate::agent_core::chord_support::Context;

use super::types::{ContextEdit, ContextView, Entry, EntryScan, Id, Storage};

/// Upstream `deriveContext` (`context.ts:16-65`).
pub async fn derive_context(
    storage: &dyn Storage,
    conversation_id: Id,
    at: Option<Id>,
    ctx: Context,
) -> anyhow::Result<ContextView> {
    let head_scan = EntryScan {
        conversation_id,
        with_head: true,
        before: at.map(|at| at + 1),
        limit: 1,
        ..EntryScan::default()
    };
    let head = storage
        .scan_entries(&head_scan, ctx.clone())
        .await?
        .into_iter()
        .next();
    let from = head.as_ref().and_then(|entry| entry.head);

    // Walk newest-first until we pass `from` (`context.ts:28-46`).
    let mut range: Vec<Entry> = Vec::new();
    let mut before = at.map(|at| at + 1);
    loop {
        let page = storage
            .scan_entries(
                &EntryScan {
                    conversation_id,
                    before,
                    limit: 256,
                    ..EntryScan::default()
                },
                ctx.clone(),
            )
            .await?;
        let mut done = page.len() < 256;
        for entry in page {
            if from.is_some_and(|from| entry.id < from) {
                done = true;
                break;
            }
            range.push(entry);
        }
        if done {
            break;
        }
        before = Some(range.last().expect("page was non-empty").id);
    }
    range.reverse();

    // Per target, newest edit in range wins (`context.ts:49-50`).
    let mut edits: std::collections::HashMap<Id, &ContextEdit> = std::collections::HashMap::new();
    for entry in &range {
        for edit in entry.edits.iter().flatten() {
            edits.insert(edit.target, edit);
        }
    }

    let entries = match &head {
        Some(head) => {
            let mut entries = vec![head.clone()];
            entries.extend(range.iter().filter(|entry| entry.head.is_none()).cloned());
            entries
        }
        None => range.clone(),
    };

    let mut messages = Vec::new();
    for entry in &entries {
        let edit = edits.get(&entry.id);
        if edit.is_some_and(|edit| edit.action == "omit") {
            continue;
        }
        if let Some(edit) = edit {
            if edit.action == "replace" {
                messages.extend(edit.messages.iter().flatten().cloned());
                continue;
            }
        }
        if let Some(model) = &entry.model {
            messages.extend(model.iter().cloned());
        }
    }
    Ok(ContextView {
        head,
        entries,
        messages: reorder_tool_results(messages),
    })
}

/// Upstream `reorderToolResults` (`context.ts:71-105`): results are appended
/// as tools finish; put them back in call order, synthesising a missing one
/// after a fork cut.
pub fn reorder_tool_results(messages: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = messages[index].clone();
        out.push(message.clone());
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            index += 1;
            continue;
        }
        let calls: Vec<&Value> = message
            .get("content")
            .and_then(Value::as_array)
            .map(|content| {
                content
                    .iter()
                    .filter(|item| item.get("type").and_then(Value::as_str) == Some("toolCall"))
                    .collect()
            })
            .unwrap_or_default();
        if calls.is_empty() {
            index += 1;
            continue;
        }
        let mut results: std::collections::HashMap<String, &Value> =
            std::collections::HashMap::new();
        let mut cursor = index + 1;
        while cursor < messages.len()
            && messages[cursor].get("role").and_then(Value::as_str) == Some("toolResult")
        {
            if let Some(tool_call_id) = messages[cursor].get("toolCallId").and_then(Value::as_str) {
                results.insert(tool_call_id.to_owned(), &messages[cursor]);
            }
            cursor += 1;
        }
        for call in calls {
            let call_id = call.get("id").and_then(Value::as_str).unwrap_or_default();
            let call_name = call.get("name").and_then(Value::as_str).unwrap_or_default();
            if let Some(result) = results.get(call_id) {
                out.push((*result).clone());
                continue;
            }
            // The synthetic missing result (`context.ts:88-99`).
            let mut detail = serde_json::Map::new();
            detail.insert("reason".into(), Value::String("missing_after_fork".into()));
            let mut text = serde_json::Map::new();
            text.insert("type".into(), Value::String("text".into()));
            text.insert(
                "text".into(),
                Value::String(
                    "Tool result unavailable: history ends before this call completed.".into(),
                ),
            );
            let mut synthetic = serde_json::Map::new();
            synthetic.insert("role".into(), Value::String("toolResult".into()));
            synthetic.insert("toolCallId".into(), Value::String(call_id.to_owned()));
            synthetic.insert("toolName".into(), Value::String(call_name.to_owned()));
            synthetic.insert("content".into(), Value::Array(vec![Value::Object(text)]));
            synthetic.insert("isError".into(), Value::Bool(true));
            synthetic.insert("details".into(), Value::Object(detail));
            synthetic.insert(
                "timestamp".into(),
                message.get("timestamp").cloned().unwrap_or(Value::Null),
            );
            out.push(Value::Object(synthetic));
        }
        index = cursor;
    }
    out
}
