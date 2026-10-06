//! Remaining ports of `packages/agent/test/harness/jsonl-v3-migration.test.ts`
//! (review round 1): lane-configuration reconstruction, branch summaries,
//! custom/custom_message import, session-info names, label edges, embedded
//! usage, the importedUsage upgrade fixture, publication-failure atomicity,
//! and the open-v3 fork rejection. Helpers come from the sibling `tests`
//! module (the same fixtures, byte-exact).

use super::tests::*;
use super::JsonlSessionRepo;
use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::env::NodeExecutionEnv;
use crate::agent_core::harness::session::jsonl::types::JsonlSessionListOptions;
use crate::agent_core::harness::session::session::StorageBackedSession;
use crate::agent_core::harness::session::types::{
    AscDescOrder, Entry, EntryQuery, ForkOptions, Session as _, Storage, UsageRow,
};
use crate::agent_core::harness::session::values::{
    lane_config, lane_state, session_name, set_value,
};
use crate::agent_core::harness::types::{FileContent, FileError, FileSystem};
use futures::future::BoxFuture;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const NOW: i64 = 1_700_000_000_000;

#[allow(dead_code)]
fn user_text(content: &str, offset_ms: i64) -> serde_json::Value {
    serde_json::json!({
        "role": "user",
        "content": [{ "type": "text", "text": content }],
        "timestamp": NOW + offset_ms,
    })
}

fn entry_record(
    type_name: &str,
    id: &str,
    parent_id: Option<&str>,
    offset_ms: i64,
    fields: serde_json::Value,
) -> String {
    let mut record = serde_json::json!({
        "type": type_name,
        "id": id,
        "parentId": parent_id,
        "timestamp": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(NOW + offset_ms),
    });
    for (key, value) in fields.as_object().unwrap() {
        record[key] = value.clone();
    }
    record.to_string()
}

async fn open_single_legacy(
    repo: &JsonlSessionRepo,
    dir: &tempfile::TempDir,
    records: &[String],
) -> anyhow::Result<Arc<StorageBackedSession>> {
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    write_legacy_v3_fixture(dir, records, None, &cwd).await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions { cwd: Some(cwd) }),
            background_context(),
        )
        .await?
        .remove(0);
    repo.open(&metadata, background_context()).await
}

async fn main_tip(session: &StorageBackedSession) -> Option<String> {
    session
        .get_branch_tip("main", background_context())
        .await
        .unwrap()
}

async fn entries_asc(session: &StorageBackedSession) -> Vec<Entry> {
    session
        .find_entries(
            Some(&EntryQuery {
                order: Some(AscDescOrder::Asc),
                ..Default::default()
            }),
            background_context(),
        )
        .await
        .unwrap()
}

/// "reparents a retained child through configuration changes"
/// (`jsonl-v3-migration.test.ts:761-812`).
#[tokio::test]
async fn reparents_retained_child_through_configuration_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let mut records = vec![legacy_message("message-1", None, "first", 1_000)];
    records.extend(vec![
        entry_record(
            "model_change",
            "model-change",
            Some("message-1"),
            2_000,
            serde_json::json!({ "provider": "anthropic", "modelId": "claude-sonnet-4-5" }),
        ),
        entry_record(
            "thinking_level_change",
            "thinking-change",
            Some("model-change"),
            3_000,
            serde_json::json!({ "thinkingLevel": "high" }),
        ),
        entry_record(
            "active_tools_change",
            "active-tools-change",
            Some("thinking-change"),
            4_000,
            serde_json::json!({ "activeToolNames": ["read", "bash"] }),
        ),
    ]);
    records.push(legacy_message(
        "message-2",
        Some("active-tools-change"),
        "second",
        5_000,
    ));
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 2);
    let first = &entries[0];
    let second = &entries[1];
    assert_eq!(first.parent_id(), None);
    assert_eq!(second.parent_id(), Some(first.id()));
    assert!(second.seq() > first.seq());
    assert_eq!(main_tip(&session).await.as_deref(), Some(second.id()));
    assert_eq!(
        session
            .get_value(&lane_config("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({
            "model": { "provider": "anthropic", "modelId": "claude-sonnet-4-5" },
            "thinkingLevel": "high",
            "activeToolNames": ["read", "bash"],
        })
    );
    assert_eq!(
        session
            .get_value(&lane_state("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({ "currentOperationId": null, "lastOperationId": null, "inbox": [] })
    );
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "retains only configuration changes on the selected physical branch"
/// (`jsonl-v3-migration.test.ts:814-869`).
#[tokio::test]
async fn retains_only_selected_branch_configuration_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![
        legacy_message("root", None, "first", 1_000),
        entry_record(
            "model_change",
            "selected-model",
            Some("root"),
            2_000,
            serde_json::json!({ "provider": "anthropic", "modelId": "selected" }),
        ),
        entry_record(
            "thinking_level_change",
            "selected-thinking",
            Some("selected-model"),
            3_000,
            serde_json::json!({ "thinkingLevel": "high" }),
        ),
        entry_record(
            "model_change",
            "abandoned-model",
            Some("root"),
            4_000,
            serde_json::json!({ "provider": "openai", "modelId": "abandoned" }),
        ),
        legacy_message("selected-tip", Some("selected-thinking"), "second", 5_000),
    ];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    assert_eq!(
        session
            .get_value(&lane_config("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({
            "model": { "provider": "anthropic", "modelId": "selected" },
            "thinkingLevel": "high",
            "activeToolNames": [],
        })
    );
    assert_eq!(
        session
            .get_value(&lane_state("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({ "currentOperationId": null, "lastOperationId": null, "inbox": [] })
    );
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "leaves main data-only for missing model / missing thinking level"
/// (`jsonl-v3-migration.test.ts:871-922`, both it.each variants).
#[tokio::test]
async fn leaves_main_data_only_for_incomplete_configuration() {
    for changes in [
        // missing model
        vec![entry_record(
            "thinking_level_change",
            "thinking",
            Some("root"),
            3_000,
            serde_json::json!({ "thinkingLevel": "high" }),
        )],
        // missing thinking level
        vec![entry_record(
            "model_change",
            "model",
            Some("root"),
            2_000,
            serde_json::json!({ "provider": "anthropic", "modelId": "selected" }),
        )],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_in(&dir);
        let mut records = vec![legacy_message("root", None, "first", 1_000)];
        records.extend(changes);
        records.push(legacy_message("tip", Some("root"), "second", 5_000));
        let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
        assert!(session
            .get_value(&lane_config("main"), background_context())
            .await
            .unwrap()
            .is_none());
        assert!(session
            .get_value(&lane_state("main"), background_context())
            .await
            .unwrap()
            .is_none());
        session.close(background_context()).await.unwrap();
        repo.close(background_context()).await.unwrap();
    }
}

/// "resolves the main tip through trailing configuration changes"
/// (`jsonl-v3-migration.test.ts:924-955`).
#[tokio::test]
async fn resolves_main_tip_through_trailing_configuration_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let mut records = vec![legacy_message("message-1", None, "first", 1_000)];
    records.extend(vec![
        entry_record(
            "model_change",
            "model-change",
            Some("message-1"),
            2_000,
            serde_json::json!({ "provider": "anthropic", "modelId": "claude-sonnet-4-5" }),
        ),
        entry_record(
            "thinking_level_change",
            "thinking-change",
            Some("model-change"),
            3_000,
            serde_json::json!({ "thinkingLevel": "high" }),
        ),
        entry_record(
            "active_tools_change",
            "active-tools-change",
            Some("thinking-change"),
            4_000,
            serde_json::json!({ "activeToolNames": ["read", "bash"] }),
        ),
    ]);
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(main_tip(&session).await.as_deref(), Some(entries[0].id()));
    assert_eq!(
        session
            .get_value(&lane_config("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({
            "model": { "provider": "anthropic", "modelId": "claude-sonnet-4-5" },
            "thinkingLevel": "high",
            "activeToolNames": ["read", "bash"],
        })
    );
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "imports session info as the current name without retaining a tree entry"
/// (`jsonl-v3-migration.test.ts:958-976`).
#[tokio::test]
async fn imports_session_info_as_current_name() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![entry_record(
        "session_info",
        "session-info",
        None,
        1_000,
        serde_json::json!({ "name": "Imported session" }),
    )];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    assert_eq!(
        session.get_name(background_context()).await.unwrap(),
        Some("Imported session".to_string())
    );
    assert!(session
        .find_entries(None, background_context())
        .await
        .unwrap()
        .is_empty());
    assert_eq!(main_tip(&session).await, None);
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "uses the latest session info and resolves tree structure through
/// discarded records" (`jsonl-v3-migration.test.ts:978-1031`).
#[tokio::test]
async fn uses_latest_session_info_and_resolves_through_discarded_records() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![
        legacy_message("message-1", None, "first", 1_000),
        entry_record(
            "session_info",
            "session-info-1",
            Some("message-1"),
            2_000,
            serde_json::json!({ "name": "Earlier name" }),
        ),
        legacy_message("message-2", Some("session-info-1"), "second", 3_000),
        entry_record(
            "session_info",
            "session-info-2",
            Some("message-2"),
            4_000,
            serde_json::json!({ "name": "Latest name" }),
        ),
    ];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].parent_id(), Some(entries[0].id()));
    assert_eq!(
        session.get_name(background_context()).await.unwrap(),
        Some("Latest name".to_string())
    );
    assert_eq!(main_tip(&session).await.as_deref(), Some(entries[1].id()));
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "clears the session name with undefined / empty session_info name"
/// (`jsonl-v3-migration.test.ts:1033-1056`, both it.each variants).
#[tokio::test]
async fn clears_session_name_with_latest_session_info() {
    for name in [None::<String>, Some(String::new())] {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_in(&dir);
        let mut second_info = serde_json::json!({
            "type": "session_info",
            "id": "session-info-2",
            "parentId": "session-info-1",
            "timestamp": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(NOW + 2_000),
        });
        if let Some(name) = &name {
            second_info["name"] = serde_json::json!(name);
        }
        let records = vec![
            entry_record(
                "session_info",
                "session-info-1",
                None,
                1_000,
                serde_json::json!({ "name": "Earlier name" }),
            ),
            second_info.to_string(),
        ];
        let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
        assert!(session
            .get_value(&session_name(), background_context())
            .await
            .unwrap()
            .is_none());
        session.close(background_context()).await.unwrap();
        repo.close(background_context()).await.unwrap();
    }
}

/// "skips a label whose target has no retained ancestor"
/// (`jsonl-v3-migration.test.ts:1094-1135`).
#[tokio::test]
async fn skips_label_without_retained_ancestor() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![
        entry_record(
            "session_info",
            "root-session-info",
            None,
            1_000,
            serde_json::json!({}),
        ),
        entry_record(
            "label",
            "root-label",
            Some("root-session-info"),
            2_000,
            serde_json::json!({ "targetId": "root-session-info", "label": "Skipped label" }),
        ),
        legacy_message("message-1", Some("root-label"), "retained message", 3_000),
    ];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].parent_id(), None);
    assert!(session
        .get_label(entries[0].id(), background_context())
        .await
        .unwrap()
        .is_none());
    assert_eq!(main_tip(&session).await.as_deref(), Some(entries[0].id()));
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "uses the latest label after remapping discarded targets"
/// (`jsonl-v3-migration.test.ts:1137-1198`).
#[tokio::test]
async fn uses_latest_label_after_remapping_discarded_targets() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![
        legacy_message("message-1", None, "first", 1_000),
        entry_record(
            "session_info",
            "session-info",
            Some("message-1"),
            2_000,
            serde_json::json!({}),
        ),
        entry_record(
            "label",
            "label-1",
            Some("session-info"),
            3_000,
            serde_json::json!({ "targetId": "session-info", "label": "Earlier label" }),
        ),
        entry_record(
            "label",
            "label-2",
            Some("label-1"),
            4_000,
            serde_json::json!({ "targetId": "message-1", "label": "Latest label" }),
        ),
        legacy_message("message-2", Some("label-2"), "second", 5_000),
    ];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].parent_id(), Some(entries[0].id()));
    assert_eq!(
        session
            .get_label(entries[0].id(), background_context())
            .await
            .unwrap(),
        Some("Latest label".to_string())
    );
    assert_eq!(main_tip(&session).await.as_deref(), Some(entries[1].id()));
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "clears a label with undefined / empty label" (`jsonl-v3-migration.test.ts:1200-1241`,
/// both it.each variants).
#[tokio::test]
async fn clears_label_with_latest_label_entry() {
    for label in [None::<String>, Some(String::new())] {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_in(&dir);
        let mut label_2 = serde_json::json!({
            "type": "label",
            "id": "label-2",
            "parentId": "label-1",
            "timestamp": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(NOW + 3_000),
            "targetId": "message-1",
        });
        if let Some(label) = &label {
            label_2["label"] = serde_json::json!(label);
        }
        let records = vec![
            legacy_message("message-1", None, "clear my label", 1_000),
            entry_record(
                "label",
                "label-1",
                Some("message-1"),
                2_000,
                serde_json::json!({ "targetId": "message-1", "label": "Earlier label" }),
            ),
            label_2.to_string(),
        ];
        let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
        let entries = session
            .find_entries(None, background_context())
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(session
            .get_label(entries[0].id(), background_context())
            .await
            .unwrap()
            .is_none());
        assert_eq!(main_tip(&session).await.as_deref(), Some(entries[0].id()));
        session.close(background_context()).await.unwrap();
        repo.close(background_context()).await.unwrap();
    }
}

/// "imports a custom entry without rewriting opaque data references"
/// (`jsonl-v3-migration.test.ts:1243-1295`).
#[tokio::test]
async fn imports_custom_entry_with_opaque_data() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let data = serde_json::json!({
        "legacyReference": "message-1",
        "nested": { "legacyReference": "custom-1" },
    });
    let records = vec![
        legacy_message("message-1", None, "first", 1_000),
        entry_record(
            "custom",
            "custom-1",
            Some("message-1"),
            2_000,
            serde_json::json!({ "customType": "checkpoint", "data": data }),
        ),
    ];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 2);
    let message_entry = &entries[0];
    let custom_entry = &entries[1];
    assert_eq!(message_entry.seq(), 1);
    assert_eq!(custom_entry.parent_id(), Some(message_entry.id()));
    assert_eq!(custom_entry.seq(), 2);
    assert_eq!(custom_entry.timestamp(), NOW + 2_000);
    assert_eq!(custom_entry.custom_type(), Some("checkpoint"));
    let crate::agent_core::harness::session::types::Entry::Custom {
        data: entry_data, ..
    } = custom_entry
    else {
        panic!("expected custom entry");
    };
    assert_eq!(*entry_data, Some(data));
    let hex = custom_entry.id().replace('-', "");
    assert_eq!(i64::from_str_radix(&hex[..12], 16).unwrap(), NOW + 2_000);
    assert_eq!(main_tip(&session).await.as_deref(), Some(custom_entry.id()));
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "imports a custom message as a current message entry"
/// (`jsonl-v3-migration.test.ts:1297-1356`): the custom-role wire shape
/// through `CustomAgentMessage` is pinned here.
#[tokio::test]
async fn imports_custom_message_as_current_message_entry() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let content = serde_json::json!([{ "type": "text", "text": "legacy custom message" }]);
    let details = serde_json::json!({ "status": "complete" });
    let records = vec![
        legacy_message("message-1", None, "first", 1_000),
        entry_record(
            "custom_message",
            "custom-message-1",
            Some("message-1"),
            2_000,
            serde_json::json!({
                "customType": "status",
                "content": content,
                "details": details,
                "display": false,
            }),
        ),
    ];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 2);
    let parent_entry = &entries[0];
    let custom_message_entry = &entries[1];
    assert_eq!(parent_entry.seq(), 1);
    assert_eq!(custom_message_entry.parent_id(), Some(parent_entry.id()));
    assert_eq!(custom_message_entry.seq(), 2);
    assert_eq!(custom_message_entry.timestamp(), NOW + 2_000);
    // The imported custom-role message wire shape.
    let message = custom_message_entry.message().unwrap();
    assert_eq!(message.role(), "custom");
    let crate::agent_core::types::AgentMessage::Custom(custom) = message else {
        panic!("expected custom message");
    };
    let mut expected = serde_json::Map::new();
    expected.insert("customType".into(), serde_json::json!("status"));
    expected.insert("content".into(), content);
    expected.insert("details".into(), details);
    expected.insert("display".into(), serde_json::json!(false));
    expected.insert("timestamp".into(), serde_json::json!(NOW + 2_000));
    assert_eq!(custom.data, expected);
    let hex = custom_message_entry.id().replace('-', "");
    assert_eq!(i64::from_str_radix(&hex[..12], 16).unwrap(), NOW + 2_000);
    assert_eq!(
        main_tip(&session).await.as_deref(),
        Some(custom_message_entry.id())
    );
    assert_eq!(
        session
            .get_stats(background_context())
            .await
            .unwrap()
            .message_count,
        2
    );
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

// --- branch summaries (`jsonl-v3-migration.test.ts:1358-1553`) ---------------

fn branch_summary_details() -> serde_json::Value {
    serde_json::json!({ "reason": "navigation" })
}
fn branch_summary_usage() -> serde_json::Value {
    serde_json::json!({
        "input": 11, "output": 7, "cacheRead": 3, "cacheWrite": 2, "totalTokens": 23,
        "cost": { "input": 0.11, "output": 0.07, "cacheRead": 0.03, "cacheWrite": 0.02, "total": 0.23 },
    })
}

async fn open_branch_summary_fixture(
    dir: &tempfile::TempDir,
    repo: &JsonlSessionRepo,
    from_hook: Option<bool>,
) -> (Arc<StorageBackedSession>, Entry, Entry, Entry) {
    let mut summary = serde_json::json!({
        "type": "branch_summary",
        "id": "summary",
        "parentId": "branch-point",
        "timestamp": crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(NOW + 3_000),
        "fromId": "branch-point",
        "summary": "Summary of the abandoned branch",
        "details": branch_summary_details(),
        "usage": branch_summary_usage(),
    });
    if from_hook == Some(true) {
        summary["fromHook"] = serde_json::json!(true);
    }
    let records = vec![
        legacy_message("branch-point", None, "Try the first approach", 1_000),
        entry_record(
            "message",
            "abandoned-response",
            Some("branch-point"),
            2_000,
            serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "Implemented the first approach" }],
                    "api": "anthropic-messages",
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-5",
                    "usage": {
                        "input": 20, "output": 10, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 30,
                        "cost": { "input": 0.2, "output": 0.1, "cacheRead": 0, "cacheWrite": 0, "total": 0.3 },
                    },
                    "stopReason": "stop",
                    "timestamp": NOW + 2_000,
                },
            }),
        ),
        summary.to_string(),
    ];
    let session = open_single_legacy(repo, dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 3);
    let (branch_point, abandoned, branch_summary) =
        (entries[0].clone(), entries[1].clone(), entries[2].clone());
    (session, branch_point, abandoned, branch_summary)
}

/// "preserves payload and remaps references" (`jsonl-v3-migration.test.ts:1447-1466`).
#[tokio::test]
async fn branch_summary_preserves_payload_and_remaps_references() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let (session, branch_point, abandoned, branch_summary) =
        open_branch_summary_fixture(&dir, &repo, None).await;
    assert_eq!(abandoned.parent_id(), Some(branch_point.id()));
    assert_eq!(branch_summary.parent_id(), Some(branch_point.id()));
    assert_eq!(branch_summary.seq(), 3);
    assert_eq!(branch_summary.timestamp(), NOW + 3_000);
    let crate::agent_core::harness::session::types::Entry::BranchSummary {
        from_id,
        summary,
        details,
        usage,
        from_hook,
        ..
    } = &branch_summary
    else {
        panic!("expected branch_summary entry");
    };
    assert_eq!(from_id.as_deref(), Some(branch_point.id()));
    assert_eq!(summary, "Summary of the abandoned branch");
    assert_eq!(*details, Some(branch_summary_details()));
    assert_eq!(
        serde_json::to_value(usage).unwrap(),
        serde_json::json!({
            "input": 11, "output": 7, "cacheRead": 3, "cacheWrite": 2, "totalTokens": 23,
            "cost": { "input": 0.11, "output": 0.07, "cacheRead": 0.03, "cacheWrite": 0.02, "total": 0.23 },
        })
    );
    assert!(!*from_hook);
    let hex = branch_summary.id().replace('-', "");
    assert_eq!(i64::from_str_radix(&hex[..12], 16).unwrap(), NOW + 3_000);
    assert_eq!(
        main_tip(&session).await.as_deref(),
        Some(branch_summary.id())
    );
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "preserves an explicit fromHook flag" (`jsonl-v3-migration.test.ts:1468-1476`).
#[tokio::test]
async fn branch_summary_preserves_explicit_from_hook() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let (session, _branch_point, _abandoned, branch_summary) =
        open_branch_summary_fixture(&dir, &repo, Some(true)).await;
    let crate::agent_core::harness::session::types::Entry::BranchSummary { from_hook, .. } =
        &branch_summary
    else {
        panic!("expected branch_summary entry");
    };
    assert!(*from_hook);
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// 'normalizes fromId "root" to null' (`jsonl-v3-migration.test.ts:1478-1500`).
#[tokio::test]
async fn branch_summary_normalizes_root_from_id_to_null() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![entry_record(
        "branch_summary",
        "summary",
        None,
        3_000,
        serde_json::json!({
            "fromId": "root",
            "summary": "Summary from the root",
        }),
    )];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 1);
    let crate::agent_core::harness::session::types::Entry::BranchSummary { from_id, .. } =
        &entries[0]
    else {
        panic!("expected branch_summary entry");
    };
    assert_eq!(*from_id, None);
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "rejects a missing fromId" (`jsonl-v3-migration.test.ts:1502-1519`).
#[tokio::test]
async fn branch_summary_rejects_missing_from_id() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let _ = &repo;
    let records = vec![entry_record(
        "branch_summary",
        "summary",
        None,
        3_000,
        serde_json::json!({
            "fromId": "missing-legacy-entry",
            "summary": "Summary from a missing source",
        }),
    )];
    let error = open_single_legacy(&repo, &dir, &records).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Missing legacy v3 entry reference: missing-legacy-entry"),
        "{error}"
    );
    repo.close(background_context()).await.unwrap();
}

/// "keeps fromId null when a discarded source has no retained ancestor"
/// (`jsonl-v3-migration.test.ts:1521-1552`).
#[tokio::test]
async fn branch_summary_keeps_null_from_id_for_discarded_source() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![
        entry_record(
            "model_change",
            "model-change",
            None,
            1_000,
            serde_json::json!({ "provider": "anthropic", "modelId": "claude-sonnet-4-5" }),
        ),
        entry_record(
            "branch_summary",
            "summary",
            Some("model-change"),
            3_000,
            serde_json::json!({
                "fromId": "model-change",
                "summary": "Summary from the root",
            }),
        ),
    ];
    let session = open_single_legacy(&repo, &dir, &records).await.unwrap();
    let entries = entries_asc(&session).await;
    assert_eq!(entries.len(), 1);
    let crate::agent_core::harness::session::types::Entry::BranchSummary {
        parent_id, from_id, ..
    } = &entries[0]
    else {
        panic!("expected branch_summary entry");
    };
    assert_eq!(*parent_id, None);
    assert_eq!(*from_id, None);
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "reports embedded legacy usage without creating usage rows or rewriting
/// the source" (`jsonl-v3-migration.test.ts:442-525`).
#[tokio::test]
async fn reports_embedded_usage_without_usage_rows() {
    let dir = tempfile::tempdir().unwrap();
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let usage = |factor: i64| {
        serde_json::json!({
            "input": factor,
            "output": factor * 2,
            "cacheRead": factor * 3,
            "cacheWrite": factor * 4,
            "cacheWrite1h": factor * 5,
            "reasoning": factor * 6,
            "totalTokens": factor * 10,
            "cost": {
                "input": factor, "output": factor * 2, "cacheRead": factor * 3,
                "cacheWrite": factor * 4, "total": factor * 10,
            },
        })
    };
    let records = vec![
        entry_record(
            "message",
            "assistant",
            None,
            1_000,
            serde_json::json!({
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "answer" }],
                    "api": "anthropic-messages",
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-5",
                    "usage": usage(1),
                    "stopReason": "stop",
                    "timestamp": NOW + 1_000,
                },
            }),
        ),
        entry_record(
            "message",
            "tool-result",
            Some("assistant"),
            2_000,
            serde_json::json!({
                "message": {
                    "role": "toolResult",
                    "toolCallId": "call-1",
                    "toolName": "test",
                    "content": [{ "type": "text", "text": "result" }],
                    "usage": usage(10),
                    "isError": false,
                    "timestamp": NOW + 2_000,
                },
            }),
        ),
        entry_record(
            "compaction",
            "compaction",
            Some("tool-result"),
            3_000,
            serde_json::json!({
                "summary": "Earlier context",
                "firstKeptEntryId": "assistant",
                "tokensBefore": 1_000,
                "usage": usage(100),
            }),
        ),
        entry_record(
            "branch_summary",
            "branch-summary",
            Some("compaction"),
            4_000,
            serde_json::json!({
                "fromId": "assistant",
                "summary": "Abandoned branch",
                "usage": usage(1_000),
            }),
        ),
    ];
    let cwd = resolved_cwd(&file_system).await;
    let (path, content) = write_legacy_v3_fixture(&dir, &records, None, &cwd).await;

    let storage = crate::agent_core::harness::session::jsonl::storage::JsonlStorage::open(
        crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
            file_system: Arc::new(file_system.clone()),
            path: path.clone(),
            now: Some(Arc::new(|| NOW)),
        },
        background_context(),
    )
    .await
    .unwrap();

    let stats = storage.get_stats(background_context()).await.unwrap();
    assert_eq!(stats.message_count, 2);
    // 1 + 10 + 100 + 1000 = 1111 (assistant message, tool result, compaction,
    // branch summary).
    assert_eq!(stats.usage.input, 1_111);
    assert_eq!(stats.usage.output, 2_222);
    assert_eq!(stats.usage.cache_read, 3_333);
    assert_eq!(stats.usage.cache_write, 4_444);
    assert_eq!(stats.usage.cache_write_1h, Some(5_555));
    assert_eq!(stats.usage.reasoning, Some(6_666));
    assert_eq!(stats.usage.total_tokens, 11_110);
    assert!(storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap()
        .is_empty());
    storage.close(background_context()).await.unwrap();
    assert_eq!(
        file_system
            .read_text_file(&path, background_context())
            .await
            .unwrap(),
        content
    );
}

/// The importedUsage upgrade fixture bytes
/// (`jsonl-v3-migration.test.ts:527-541`): the usage adjustment written on
/// conversion carries exactly input 10 / output 5 / cacheRead 2 /
/// cacheWrite 1 / totalTokens 18 with the fixture cost.
#[tokio::test]
async fn upgrade_adjustment_carries_exact_imported_usage_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let imported_usage = serde_json::json!({
        "input": 10,
        "output": 5,
        "cacheRead": 2,
        "cacheWrite": 1,
        "totalTokens": 18,
        "cost": {
            "input": 0.1,
            "output": 0.05,
            "cacheRead": 0.02,
            "cacheWrite": 0.01,
            "total": 0.18,
        },
    });
    let records = vec![entry_record(
        "message",
        "assistant",
        None,
        1_000,
        serde_json::json!({
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "imported answer" }],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude-sonnet-4-5",
                "usage": imported_usage,
                "stopReason": "stop",
                "timestamp": NOW + 1_000,
            },
        }),
    )];
    let cwd = resolved_cwd(&NodeExecutionEnv::new(
        dir.path().to_string_lossy().to_string(),
    ))
    .await;
    let (path, _content) = write_legacy_v3_fixture(&dir, &records, None, &cwd).await;
    let storage = crate::agent_core::harness::session::jsonl::storage::JsonlStorage::open(
        crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
            file_system: Arc::new(NodeExecutionEnv::new(
                dir.path().to_string_lossy().to_string(),
            )),
            path: path.clone(),
            now: Some(Arc::new(|| NOW)),
        },
        background_context(),
    )
    .await
    .unwrap();
    storage
        .commit(
            vec![set_value(
                &session_name(),
                serde_json::json!("Converted session"),
            )],
            background_context(),
        )
        .await
        .unwrap();

    // The written transaction line carries the adjustment bytes verbatim.
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let content = file_system
        .read_text_file(&path, background_context())
        .await
        .unwrap();
    let lines: Vec<&str> = content.trim_end().split('\n').collect();
    let transaction: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    let adjustment = &transaction.as_array().unwrap()[0];
    assert_eq!(
        adjustment,
        &serde_json::json!({
            "kind": "usage",
            "id": adjustment["id"],
            "seq": adjustment["seq"],
            "usage": imported_usage,
            "adjustment": true,
            "details": { "source": "v3-import" },
        })
    );
    // And the in-memory row round-trips the same totals.
    let rows = storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let row: &UsageRow = &rows[0];
    assert_eq!(row.usage.input, 10);
    assert_eq!(row.usage.output, 5);
    assert_eq!(row.usage.cache_read, 2);
    assert_eq!(row.usage.cache_write, 1);
    assert_eq!(row.usage.total_tokens, 18);
    assert!((row.usage.cost.input - 0.1).abs() < 1e-9);
    assert!((row.usage.cost.total - 0.18).abs() < 1e-9);
    storage.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// `FailableRenameNodeExecutionEnv`
/// (`jsonl-v3-migration.test.ts:20-33`): injects rename failures.
struct FailableRenameEnv {
    inner: Arc<NodeExecutionEnv>,
    fail_rename: AtomicBool,
}

impl FailableRenameEnv {
    fn new(inner: Arc<NodeExecutionEnv>) -> Arc<Self> {
        Arc::new(FailableRenameEnv {
            inner,
            fail_rename: AtomicBool::new(false),
        })
    }
}

impl FileSystem for FailableRenameEnv {
    fn cwd(&self) -> &str {
        self.inner.cwd()
    }

    fn absolute_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.absolute_path(path, context)
    }

    fn join_path<'a>(
        &'a self,
        parts: &[String],
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.join_path(parts, context)
    }

    fn read_text_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.read_text_file(path, context)
    }

    fn open_text_line_reader<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Arc<dyn crate::agent_core::harness::types::TextLineReader>, FileError>>
    {
        self.inner.open_text_line_reader(path, context)
    }

    fn read_text_lines<'a>(
        &'a self,
        path: &str,
        options: Option<&crate::agent_core::harness::types::ReadTextLinesOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        self.inner.read_text_lines(path, options, context)
    }

    fn read_binary_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        self.inner.read_binary_file(path, context)
    }

    fn write_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.write_file(path, content, context)
    }

    fn append_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.append_file(path, content, context)
    }

    fn rename_file<'a>(
        &'a self,
        source_path: &str,
        destination_path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        if self.fail_rename.load(Ordering::SeqCst) {
            let source_path = source_path.to_string();
            return Box::pin(async move {
                Err(FileError::new(
                    crate::agent_core::harness::types::FileErrorCode::Unknown,
                    "Injected rename failure",
                    Some(source_path),
                ))
            });
        }
        self.inner
            .rename_file(source_path, destination_path, context)
    }

    fn file_info<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<crate::agent_core::harness::types::FileInfo, FileError>> {
        self.inner.file_info(path, context)
    }

    fn list_dir<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<crate::agent_core::harness::types::FileInfo>, FileError>> {
        self.inner.list_dir(path, context)
    }

    fn canonical_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.canonical_path(path, context)
    }

    fn exists<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<bool, FileError>> {
        self.inner.exists(path, context)
    }

    fn create_dir<'a>(
        &'a self,
        path: &str,
        options: Option<&crate::agent_core::harness::types::CreateDirOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.create_dir(path, options, context)
    }

    fn remove<'a>(
        &'a self,
        path: &str,
        options: Option<&crate::agent_core::harness::types::RemoveOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        self.inner.remove(path, options, context)
    }

    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&str>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.create_temp_dir(prefix, context)
    }

    fn create_temp_file<'a>(
        &'a self,
        options: Option<&crate::agent_core::harness::types::TempFileOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        self.inner.create_temp_file(options, context)
    }

    fn cleanup<'a>(&'a self, context: Context) -> BoxFuture<'a, ()> {
        self.inner.cleanup(context)
    }
}

/// "leaves the v3 source and live state unchanged when atomic publication
/// fails" (`jsonl-v3-migration.test.ts:670-717`).
#[tokio::test]
async fn leaves_source_and_state_unchanged_when_publication_fails() {
    let dir = tempfile::tempdir().unwrap();
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    // The oracle's writeUsageFixture: an assistant message carrying the
    // importedUsage (input 10 / output 5 / totalTokens 18).
    let imported_message = entry_record(
        "message",
        "assistant",
        None,
        1_000,
        serde_json::json!({
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": "imported answer" }],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude-sonnet-4-5",
                "usage": {
                    "input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 1, "totalTokens": 18,
                    "cost": { "input": 0.1, "output": 0.05, "cacheRead": 0.02, "cacheWrite": 0.01, "total": 0.18 },
                },
                "stopReason": "stop",
                "timestamp": NOW + 1_000,
            },
        }),
    );
    write_legacy_v3_fixture(&dir, &[imported_message], None, &cwd).await;
    let fail_env = FailableRenameEnv::new(Arc::new(file_system.clone()));
    let path = {
        let env = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
        let directory = env
            .join_path(
                &[
                    "sessions".to_string(),
                    format!(
                        "--{}--",
                        cwd.trim_start_matches(['/', '\\'])
                            .replace(['/', '\\', ':'], "-")
                    ),
                ],
                background_context(),
            )
            .await
            .unwrap();
        let joined = env
            .join_path(
                &[directory, "legacy.jsonl".to_string()],
                background_context(),
            )
            .await
            .unwrap();
        env.absolute_path(&joined, background_context())
            .await
            .unwrap()
    };
    let options = crate::agent_core::harness::session::jsonl::types::JsonlStorageOptions {
        file_system: Arc::clone(&fail_env) as Arc<dyn FileSystem>,
        path: path.clone(),
        now: Some(Arc::new(|| NOW)),
    };
    let storage = crate::agent_core::harness::session::jsonl::storage::JsonlStorage::open(
        options,
        background_context(),
    )
    .await
    .unwrap();
    let entries_before = storage
        .scan_entries(&Default::default(), background_context())
        .await
        .unwrap();
    let leaf_before = storage
        .get_value(
            &crate::agent_core::harness::session::values::branch_tip("main"),
            background_context(),
        )
        .await
        .unwrap();
    assert!(
        leaf_before.is_some(),
        "Normalized main Branch tip is missing"
    );
    let lane_state_before = storage
        .get_value(&lane_state("main"), background_context())
        .await
        .unwrap();
    assert!(lane_state_before.is_none());
    let name_before = storage
        .get_value(&session_name(), background_context())
        .await
        .unwrap();
    let stats_before = storage.get_stats(background_context()).await.unwrap();
    let usage_before = storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();

    fail_env.fail_rename.store(true, Ordering::SeqCst);
    let error = storage
        .commit(
            vec![set_value(
                &session_name(),
                serde_json::json!("Converted session"),
            )],
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("Failed to publish JSONL storage {path}")),
        "{error}"
    );
    fail_env.fail_rename.store(false, Ordering::SeqCst);

    // Source and live state unchanged.
    assert_eq!(
        storage
            .scan_entries(&Default::default(), background_context())
            .await
            .unwrap(),
        entries_before
    );
    assert_eq!(
        storage
            .get_value(
                &crate::agent_core::harness::session::values::branch_tip("main"),
                background_context()
            )
            .await
            .unwrap(),
        leaf_before
    );
    assert_eq!(
        storage
            .get_value(&lane_state("main"), background_context())
            .await
            .unwrap(),
        lane_state_before
    );
    assert_eq!(
        storage
            .get_value(&session_name(), background_context())
            .await
            .unwrap(),
        name_before
    );
    assert_eq!(
        storage.get_stats(background_context()).await.unwrap(),
        stats_before
    );
    assert_eq!(
        storage
            .scan_usage(&Default::default(), background_context())
            .await
            .unwrap(),
        usage_before
    );

    // Retry after the failure succeeds at the leaf's seq + 2 (adjustment +
    // caller write), and only the caller sequence is exposed.
    let committed = storage
        .commit(
            vec![set_value(
                &session_name(),
                serde_json::json!("Converted session"),
            )],
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(committed.first_seq, leaf_before.unwrap().seq + 2);
    assert_eq!(committed.seqs, vec![committed.first_seq]);
    assert_eq!(
        storage
            .get_value(&session_name(), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!("Converted session")
    );
    let usage_rows = storage
        .scan_usage(&Default::default(), background_context())
        .await
        .unwrap();
    assert_eq!(usage_rows.len(), 1);
    assert!(usage_rows[0].adjustment);
    assert_eq!(usage_rows[0].usage.input, 10);
    assert_eq!(
        storage.get_stats(background_context()).await.unwrap(),
        stats_before
    );
    storage.close(background_context()).await.unwrap();
}

/// "forks a configured closed source at its main tip when entryId is
/// omitted" (`jsonl-v3-migration.test.ts:306-353`).
#[tokio::test]
async fn forks_configured_closed_source_at_main_tip() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![
        entry_record(
            "model_change",
            "model",
            None,
            1_000,
            serde_json::json!({ "provider": "anthropic", "modelId": "claude-sonnet-4-5" }),
        ),
        entry_record(
            "thinking_level_change",
            "thinking",
            Some("model"),
            2_000,
            serde_json::json!({ "thinkingLevel": "high" }),
        ),
        legacy_message("tip", Some("thinking"), "fork me", 3_000),
    ];
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    write_legacy_v3_fixture(&dir, &records, None, &cwd).await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions { cwd: Some(cwd) }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);

    let fork = repo
        .fork(
            &metadata,
            &ForkOptions::Branch {
                branch: "main".to_string(),
                entry_id: None,
                position: None,
                id: Some("branch-fork".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap();
    let entries = entries_asc(&fork).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(main_tip(&fork).await.as_deref(), Some(entries[0].id()));
    assert_eq!(
        fork.get_value(&lane_config("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({
            "model": { "provider": "anthropic", "modelId": "claude-sonnet-4-5" },
            "thinkingLevel": "high",
            "activeToolNames": [],
        })
    );
    assert_eq!(
        fork.get_value(&lane_state("main"), background_context())
            .await
            .unwrap()
            .unwrap()
            .value,
        serde_json::json!({ "currentOperationId": null, "lastOperationId": null, "inbox": [] })
    );
    fork.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "forks a configured closed source at an original legacy entry id"
/// (`jsonl-v3-migration.test.ts:355-407`).
#[tokio::test]
async fn forks_configured_closed_source_at_legacy_entry_id() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let records = vec![
        entry_record(
            "model_change",
            "model",
            None,
            1_000,
            serde_json::json!({ "provider": "anthropic", "modelId": "claude-sonnet-4-5" }),
        ),
        entry_record(
            "thinking_level_change",
            "thinking",
            Some("model"),
            2_000,
            serde_json::json!({ "thinkingLevel": "high" }),
        ),
        legacy_message("message-1", Some("thinking"), "fork me", 3_000),
        legacy_message("message-2", Some("message-1"), "forked", 4_000),
        legacy_message("message-3", Some("message-2"), "fork me again", 5_000),
    ];
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let cwd = resolved_cwd(&file_system).await;
    write_legacy_v3_fixture(&dir, &records, None, &cwd).await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions { cwd: Some(cwd) }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);

    let fork = repo
        .fork(
            &metadata,
            &ForkOptions::Branch {
                branch: "main".to_string(),
                entry_id: Some("message-2".to_string()),
                position: None,
                id: Some("entry-fork".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap();
    let entries = entries_asc(&fork).await;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].parent_id(), Some(entries[0].id()));
    assert_eq!(
        entries[1]
            .message()
            .map(crate::agent_core::types::AgentMessage::role),
        Some("user")
    );
    assert_eq!(main_tip(&fork).await.as_deref(), Some(entries[1].id()));
    fork.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

/// "rejects an open v3 source until a non-empty commit persists its
/// format-4 ids" (`jsonl-v3-migration.test.ts:409-426`).
#[tokio::test]
async fn rejects_fork_of_open_v3_source_until_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);
    let file_system = NodeExecutionEnv::new(dir.path().to_string_lossy().to_string());
    let records = vec![
        legacy_message("message-1", None, "fork me", 1_000),
        entry_record(
            "label",
            "label-1",
            Some("message-1"),
            2_000,
            serde_json::json!({ "targetId": "message-1", "label": "Fork point" }),
        ),
        entry_record(
            "session_info",
            "session-info",
            Some("label-1"),
            3_000,
            serde_json::json!({ "name": "Imported fork" }),
        ),
        legacy_message("message-2", Some("session-info"), "forked", 4_000),
    ];
    let cwd = resolved_cwd(&file_system).await;
    let (path, content) = write_legacy_v3_fixture(&dir, &records, None, &cwd).await;
    let metadata = repo
        .list(
            Some(&JsonlSessionListOptions { cwd: Some(cwd) }),
            background_context(),
        )
        .await
        .unwrap()
        .remove(0);
    let source = repo.open(&metadata, background_context()).await.unwrap();
    let source_entries = entries_asc(&source).await;

    let error = repo
        .fork(
            &metadata,
            &ForkOptions::Tree {
                id: Some("open-fork".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Cannot fork an open legacy v3 JSONL session"),
        "{error}"
    );
    assert_eq!(
        file_system
            .read_text_file(&path, background_context())
            .await
            .unwrap(),
        content
    );

    // A non-empty commit upgrades the source; the fork then succeeds and
    // exposes the same entries plus the upgraded name.
    source
        .set_name(Some("Upgraded source".to_string()), background_context())
        .await
        .unwrap();
    let fork = repo
        .fork(
            &metadata,
            &ForkOptions::Tree {
                id: Some("open-fork".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(fork.metadata().id, "open-fork");
    assert_eq!(entries_asc(&fork).await, source_entries);
    assert_eq!(
        fork.get_name(background_context()).await.unwrap(),
        Some("Upgraded source".to_string())
    );
    source.close(background_context()).await.unwrap();
    fork.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}
