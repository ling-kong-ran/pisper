//! Port of `packages/agent/test/harness/session-context.test.ts` (the
//! executable spec for `session/context.ts`): context filtering, branch
//! summaries, compaction checkpoints, and custom-entry projectors.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::types::CustomAgentMessage;
use crate::ai::types::content::{TextContent, ToolCall};
use crate::ai::types::message::{AssistantBlock, AssistantMessage, StringOrBlocks, UserMessage};
use crate::ai::types::primitives::{StopReason, Usage, UsageCost};

const NOW: i64 = 1_700_000_000_000;

fn usage() -> Usage {
    Usage {
        input: 0,
        output: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 0,
        cost: UsageCost::default(),
    }
}

fn user_message(text: &str) -> crate::agent_core::types::AgentMessage {
    crate::agent_core::types::AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_string()),
        timestamp: NOW,
    })
}

fn assistant_message(stop_reason: StopReason, text: &str) -> AssistantMessage {
    AssistantMessage {
        content: if stop_reason == StopReason::ToolUse {
            vec![AssistantBlock::ToolCall(ToolCall {
                id: "call".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({}),
                thought_signature: None,
                namespace: None,
            })]
        } else {
            vec![AssistantBlock::Text(TextContent {
                text: text.to_string(),
                text_signature: None,
            })]
        },
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-5".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: usage(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: NOW,
    }
}

fn assistant_agent_message(message: AssistantMessage) -> crate::agent_core::types::AgentMessage {
    crate::agent_core::types::AgentMessage::Assistant(message)
}

fn message_entry(
    id: &str,
    parent_id: Option<&str>,
    seq: i64,
    message: crate::agent_core::types::AgentMessage,
) -> Entry {
    Entry::Message {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        seq,
        timestamp: NOW,
        message,
        terminate: None,
    }
}

fn compaction_entry(
    id: &str,
    parent_id: Option<&str>,
    seq: i64,
    summary: &str,
    retained_tail: Vec<crate::agent_core::types::AgentMessage>,
) -> Entry {
    Entry::Compaction {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        seq,
        timestamp: NOW,
        summary: summary.to_string(),
        retained_tail,
        tokens_before: 100,
        details: None,
        usage: None,
        from_hook: false,
    }
}

fn branch_summary_entry(id: &str, parent_id: &str, seq: i64) -> Entry {
    Entry::BranchSummary {
        id: id.to_string(),
        parent_id: Some(parent_id.to_string()),
        seq,
        timestamp: NOW,
        from_id: Some("source-leaf".to_string()),
        summary: "work on the abandoned branch".to_string(),
        details: None,
        usage: None,
        from_hook: false,
    }
}

fn custom_entry(id: &str, parent_id: Option<&str>, seq: i64, custom_type: &str) -> Entry {
    Entry::Custom {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        seq,
        timestamp: NOW,
        custom_type: custom_type.to_string(),
        data: None,
    }
}

/// The `role`-tagged custom messages the summaries project to.
fn custom_role_message(
    role: &str,
    fields: serde_json::Value,
) -> crate::agent_core::types::AgentMessage {
    let serde_json::Value::Object(object) = fields else {
        panic!("expected object");
    };
    let mut custom = CustomAgentMessage::new(role);
    custom.data = object;
    crate::agent_core::types::AgentMessage::Custom(custom)
}

fn roles(messages: &[crate::agent_core::types::AgentMessage]) -> Vec<&str> {
    messages
        .iter()
        .map(crate::agent_core::types::AgentMessage::role)
        .collect()
}

/// Oracle "filters non-context assistant response entries while preserving
/// valid messages".
#[tokio::test]
async fn filters_non_context_assistant_entries_while_preserving_valid_messages() {
    let failed = assistant_agent_message(assistant_message(StopReason::Error, "failed"));
    let stopped = assistant_agent_message(assistant_message(StopReason::Stop, "answer"));
    let aborted = assistant_agent_message(assistant_message(StopReason::Aborted, "aborted"));
    let tool_use = assistant_agent_message(assistant_message(StopReason::ToolUse, ""));
    let deferred = assistant_agent_message(assistant_message(StopReason::Deferred, ""));
    let length = assistant_agent_message(assistant_message(StopReason::Length, "truncated answer"));
    let entries = vec![
        message_entry("user", None, 1, user_message("question")),
        message_entry("failed", Some("user"), 2, failed.clone()),
        message_entry("stopped", Some("failed"), 3, stopped.clone()),
        message_entry("aborted", Some("stopped"), 4, aborted.clone()),
        message_entry("tool-use", Some("aborted"), 5, tool_use.clone()),
        message_entry("deferred", Some("tool-use"), 6, deferred.clone()),
        message_entry("length", Some("deferred"), 7, length.clone()),
    ];

    let messages = build_session_context(&entries, None, background_context())
        .await
        .unwrap();
    assert_eq!(
        messages,
        vec![user_message("question"), stopped, tool_use, length]
    );
}

/// Oracle "projects branch summaries in branch order".
#[tokio::test]
async fn projects_branch_summaries_in_branch_order() {
    let before = message_entry("before", None, 1, user_message("before summary"));
    let summary = branch_summary_entry("branch-summary", "before", 2);
    let after = message_entry(
        "after",
        Some("branch-summary"),
        3,
        user_message("after summary"),
    );

    let messages = build_session_context(&[before, summary, after], None, background_context())
        .await
        .unwrap();
    assert_eq!(
        messages,
        vec![
            user_message("before summary"),
            custom_role_message(
                "branchSummary",
                serde_json::json!({
                    "role": "branchSummary",
                    "summary": "work on the abandoned branch",
                    "fromId": "source-leaf",
                    "timestamp": NOW,
                }),
            ),
            user_message("after summary"),
        ],
    );
}

/// Oracle "filters retained-tail responses without hiding the compaction
/// summary".
#[tokio::test]
async fn filters_retained_tail_responses_without_hiding_the_compaction_summary() {
    let failed = assistant_agent_message(assistant_message(StopReason::Error, "failed"));
    let user = user_message("kept user");
    let aborted = assistant_agent_message(assistant_message(StopReason::Aborted, "aborted"));
    let stopped = assistant_agent_message(assistant_message(StopReason::Stop, "kept answer"));
    let deferred = assistant_agent_message(assistant_message(StopReason::Deferred, ""));
    let tool_use = assistant_agent_message(assistant_message(StopReason::ToolUse, ""));
    let length = assistant_agent_message(assistant_message(
        StopReason::Length,
        "kept truncated answer",
    ));
    let compaction = Entry::Compaction {
        id: "compaction".to_string(),
        parent_id: None,
        seq: 1,
        timestamp: NOW,
        summary: "summary".to_string(),
        retained_tail: vec![
            failed,
            user.clone(),
            aborted,
            stopped.clone(),
            deferred,
            tool_use.clone(),
            length.clone(),
        ],
        tokens_before: 100,
        details: None,
        usage: None,
        from_hook: false,
    };

    let messages = build_session_context(&[compaction], None, background_context())
        .await
        .unwrap();
    assert_eq!(
        roles(&messages),
        [
            "compactionSummary",
            "user",
            "assistant",
            "assistant",
            "assistant"
        ]
    );
    assert_eq!(
        messages,
        vec![
            custom_role_message(
                "compactionSummary",
                serde_json::json!({
                    "role": "compactionSummary",
                    "summary": "summary",
                    "tokensBefore": 100,
                    "timestamp": NOW,
                }),
            ),
            user,
            stopped,
            tool_use,
            length,
        ],
    );
}

/// Oracle "uses only the latest compaction checkpoint and entries after it".
#[tokio::test]
async fn uses_only_the_latest_compaction_checkpoint_and_entries_after_it() {
    let before_first = message_entry("before-first", None, 1, user_message("before first"));
    let first = compaction_entry(
        "first-compaction",
        Some("before-first"),
        2,
        "stale summary",
        vec![user_message("stale tail")],
    );
    let between = message_entry(
        "between",
        Some("first-compaction"),
        3,
        user_message("between compactions"),
    );
    let mut latest = compaction_entry(
        "latest-compaction",
        Some("between"),
        4,
        "latest summary",
        vec![user_message("latest tail")],
    );
    if let Entry::Compaction { tokens_before, .. } = &mut latest {
        *tokens_before = 200;
    }
    let after = message_entry(
        "after",
        Some("latest-compaction"),
        5,
        user_message("after latest"),
    );

    let messages = build_session_context(
        &[before_first, first, between, latest, after],
        None,
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(
        messages,
        vec![
            custom_role_message(
                "compactionSummary",
                serde_json::json!({
                    "role": "compactionSummary",
                    "summary": "latest summary",
                    "tokensBefore": 200,
                    "timestamp": NOW,
                }),
            ),
            user_message("latest tail"),
            user_message("after latest"),
        ],
    );
}

/// Oracle "projects custom entries through synchronous and asynchronous
/// canonical projectors in branch order".
#[tokio::test]
async fn projects_custom_entries_through_registered_projectors_in_branch_order() {
    use std::sync::Mutex;

    let old_custom = custom_entry("old-custom", None, 1, "sync");
    let compaction = compaction_entry("compaction", Some("old-custom"), 2, "summary", vec![]);
    let sync_custom = custom_entry("sync-custom", Some("compaction"), 3, "sync");
    let omitted_custom = custom_entry("omitted-custom", Some("sync-custom"), 4, "omitted");
    let async_custom = custom_entry("async-custom", Some("omitted-custom"), 5, "async");
    let projected_ids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let sync_ids = Arc::clone(&projected_ids);
    let sync_projector: EntryProjector = Arc::new(move |entry, _context| {
        let ids = Arc::clone(&sync_ids);
        Box::pin(async move {
            ids.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(entry.id().to_string());
            Ok(vec![user_message(&format!("projected:{}", entry.id()))])
        })
    });
    let async_ids = Arc::clone(&projected_ids);
    let async_projector: EntryProjector = Arc::new(move |entry, _context| {
        let ids = Arc::clone(&async_ids);
        Box::pin(async move {
            // Simulate async work before recording, like the oracle's
            // `await Promise.resolve()`.
            tokio::task::yield_now().await;
            ids.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(entry.id().to_string());
            Ok(vec![user_message(&format!("projected:{}", entry.id()))])
        })
    });
    let options = SessionContextBuildOptions {
        entry_projectors: Some(BTreeMap::from([
            ("sync".to_string(), sync_projector),
            ("async".to_string(), async_projector),
        ])),
    };

    let messages = build_session_context(
        &[
            old_custom,
            compaction,
            sync_custom.clone(),
            omitted_custom,
            async_custom.clone(),
        ],
        Some(&options),
        background_context(),
    )
    .await
    .unwrap();

    assert_eq!(
        *projected_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        ["sync-custom".to_string(), "async-custom".to_string()],
    );
    assert_eq!(
        messages,
        vec![
            custom_role_message(
                "compactionSummary",
                serde_json::json!({
                    "role": "compactionSummary",
                    "summary": "summary",
                    "tokensBefore": 100,
                    "timestamp": NOW,
                }),
            ),
            user_message("projected:sync-custom"),
            user_message("projected:async-custom"),
        ],
    );
}

/// Oracle "propagates custom projector failures".
#[tokio::test]
async fn propagates_custom_projector_failures() {
    let custom = custom_entry("custom", None, 1, "broken");
    let failing: EntryProjector =
        Arc::new(|_entry, _context| Box::pin(async { Err(anyhow::anyhow!("projector failed")) }));
    let options = SessionContextBuildOptions {
        entry_projectors: Some(BTreeMap::from([("broken".to_string(), failing)])),
    };

    let error = build_session_context(&[custom], Some(&options), background_context())
        .await
        .expect_err("projector failure propagates");
    assert!(error.to_string().contains("projector failed"), "{error}");
}

/// `buildContextEntries` narrows to the latest compaction plus its suffix and
/// `sessionEntryToContextMessages` handles each entry kind.
#[test]
fn build_context_entries_and_single_entry_projection() {
    let first = compaction_entry("first", None, 1, "stale", vec![user_message("stale tail")]);
    let latest = compaction_entry(
        "latest",
        Some("first"),
        2,
        "latest",
        vec![user_message("kept")],
    );
    let after = message_entry("after", Some("latest"), 3, user_message("after"));

    assert_eq!(build_context_entries(&[]), Vec::<Entry>::new());
    let narrowed = build_context_entries(&[first.clone(), latest.clone(), after.clone()]);
    assert_eq!(narrowed, vec![latest.clone(), after.clone()]);
    let no_compaction = build_context_entries(std::slice::from_ref(&after));
    assert_eq!(no_compaction, vec![after.clone()]);

    // A branch summary with an empty summary contributes nothing
    // (`entry.summary ? [...] : []` upstream).
    let empty_summary = Entry::BranchSummary {
        id: "b".to_string(),
        parent_id: None,
        seq: 1,
        timestamp: NOW,
        from_id: None,
        summary: String::new(),
        details: None,
        usage: None,
        from_hook: false,
    };
    assert!(session_entry_to_context_messages(&empty_summary).is_empty());
    assert!(session_entry_to_context_messages(&custom_entry("c", None, 2, "note")).is_empty());

    // A compaction entry always leads with its summary message, then the
    // filtered retained tail.
    let projected = session_entry_to_context_messages(&latest);
    assert_eq!(roles(&projected), ["compactionSummary", "user"]);
}
