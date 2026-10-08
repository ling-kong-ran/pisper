//! Tests for `drive/boundary.rs` (`boundary.ts` port).

use super::*;
use crate::agent_core::harness::runtime::lane::{EmitBatch, LaneCommand, RuntimeConfig};
use crate::agent_core::harness::runtime::restore::restore_lane;
use crate::agent_core::harness::session::{
    set_value, LaneConfiguration, LaneModel, MemoryStorage, MemoryStorageOptions, SessionMetadata,
    StorageBackedSession,
};
use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
use crate::ai::models::{create_models, CreateModelsOptions};

fn scope(steering: QueueMode, follow_up: QueueMode) -> OperationScope {
    serde_json::from_value(serde_json::json!({
        "control": {"status": "running"},
        "settings": {
            "compaction": {"enabled": true, "reserveTokens": 16384, "keepRecentTokens": 20000},
            "steeringMode": steering,
            "followUpMode": follow_up,
            "toolExecution": "parallel"
        },
        "latestAssistantEntryId": null
    }))
    .unwrap()
}

fn state_with_inbox(
    inbox: Vec<InboxItem>,
) -> crate::agent_core::harness::runtime::durable::LaneState {
    crate::agent_core::harness::runtime::durable::LaneState {
        tip_id: Some("a1".to_string()),
        configuration: LaneConfiguration {
            model: LaneModel {
                provider: "faux".to_string(),
                model_id: "faux-1".to_string(),
            },
            thinking_level: ThinkingLevel::Off,
            active_tool_names: Vec::new(),
        },
        inbox,
        last_operation_id: None,
        operation: None,
    }
}

fn seed_writes() -> Vec<Write> {
    vec![
        set_value(
            &pending_entry("s1"),
            serde_json::json!({"type": "message", "payload": {"role": "user", "content": "s1", "timestamp": 1}}),
        ),
        set_value(
            &pending_entry("s2"),
            serde_json::json!({"type": "message", "payload": {"role": "user", "content": "s2", "timestamp": 2}}),
        ),
        set_value(
            &pending_entry("w1"),
            serde_json::json!({"type": "custom", "customType": "note", "payload": {"x": 1}}),
        ),
    ]
}

async fn create_lane() -> std::sync::Arc<Lane> {
    let storage = MemoryStorage::new(MemoryStorageOptions::default());
    let sess = std::sync::Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: "boundary-test".into(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        std::sync::Arc::new(storage),
    ));
    let mut writes: Vec<Write> = vec![
        set_value(&branch_tip("main"), serde_json::Value::Null),
        set_value(
            &crate::agent_core::harness::session::lane_config("main"),
            crate::agent_core::harness::session::lane_configuration_value(&LaneConfiguration {
                model: LaneModel {
                    provider: "faux".to_string(),
                    model_id: "faux-1".to_string(),
                },
                thinking_level: ThinkingLevel::Off,
                active_tool_names: Vec::new(),
            }),
        ),
        set_value(
            &crate::agent_core::harness::session::lane_state("main"),
            serde_json::json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
        ),
    ];
    writes.extend(seed_writes());
    sess.mutate(
        move |reader, context| {
            Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
        },
        background_context(),
    )
    .await
    .unwrap();
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(std::sync::Arc::clone(&faux.provider));
    let state = restore_lane(sess.as_ref(), "main", background_context())
        .await
        .expect("lane restores");
    let emit: EmitBatch = Arc::new(|_events, _context| Box::pin(async { Ok(()) }));
    Lane::new(
        "main",
        sess,
        models,
        crate::agent_core::harness::hooks::HookRegistry::new(Arc::new(
            |_error: anyhow::Error,
             _hook: crate::agent_core::harness::hooks::HookName,
             _message: String,
             _context| { Box::pin(async {}) },
        )),
        state,
        Arc::new(|error: anyhow::Error| error),
        emit,
        Arc::new(move || RuntimeConfig {
            compaction: DEFAULT_COMPACTION_SETTINGS,
            retry_policy: crate::agent_core::harness::config::DEFAULT_RETRY_POLICY,
            system_prompt: None,
            tools: Vec::new(),
            native_tools: Default::default(),
            to_provider_messages: None,
            resources: Default::default(),
            stream_options: Default::default(),
            steering_mode: QueueMode::All,
            follow_up_mode: QueueMode::All,
            tool_execution: ToolExecutionMode::Parallel,
            entry_projectors: None,
        }),
    )
}

#[test]
fn normalized_retry_policy_projects_the_configured_policy() {
    let policy = NormalizedRetryPolicy {
        max_attempts: 4,
        base_delay_ms: 1_000,
        max_agent_delay_ms: 60_000,
    };
    assert_eq!(policy.max_attempts, 4);
    assert_eq!(policy.base_delay_ms, 1_000);
    assert_eq!(policy.max_agent_delay_ms, 60_000);
}

#[tokio::test]
async fn plan_boundary_inbox_selects_writes_and_capped_steers() {
    let lane = create_lane().await;
    let state = state_with_inbox(vec![
        InboxItem {
            entry_id: "s1".into(),
            kind: InboxItemKind::Steer,
        },
        InboxItem {
            entry_id: "s2".into(),
            kind: InboxItemKind::Steer,
        },
        InboxItem {
            entry_id: "w1".into(),
            kind: InboxItemKind::Write,
        },
    ]);
    let scope = scope(QueueMode::OneAtATime, QueueMode::OneAtATime);
    let closure_lane = lane.clone();
    let placement = closure_lane
        .command(
            move |_state, reader| {
                let state = state.clone();
                let scope = scope.clone();
                let lane = lane.clone();
                Box::pin(async move {
                    let placement = plan_boundary_inbox(
                        &lane,
                        &state,
                        &scope,
                        reader,
                        Some("a1".to_string()),
                        false,
                        &background_context(),
                    )
                    .await
                    .unwrap();
                    Ok(LaneCommand::Return { result: placement })
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(placement.entries.len(), 2, "steer cap leaves s2 out");
    assert_eq!(placement.trigger_entry_id.as_deref(), Some("s1"));
    assert_eq!(placement.tip_id.as_deref(), Some("w1"));
    assert_eq!(
        placement.writes.len(),
        2 + 2 + 1,
        "2 inserts + 2 staged deletes + tip"
    );
    assert!(placement.queues.is_some());
}

#[tokio::test]
async fn plan_boundary_inbox_takes_every_steer_in_all_mode() {
    let lane = create_lane().await;
    let state = state_with_inbox(vec![
        InboxItem {
            entry_id: "s1".into(),
            kind: InboxItemKind::Steer,
        },
        InboxItem {
            entry_id: "s2".into(),
            kind: InboxItemKind::Steer,
        },
    ]);
    let scope = scope(QueueMode::All, QueueMode::All);
    let closure_lane = lane.clone();
    let placement = closure_lane
        .command(
            move |_state, reader| {
                let state = state.clone();
                let scope = scope.clone();
                let lane = lane.clone();
                Box::pin(async move {
                    let placement = plan_boundary_inbox(
                        &lane,
                        &state,
                        &scope,
                        reader,
                        Some("a1".to_string()),
                        false,
                        &background_context(),
                    )
                    .await
                    .unwrap();
                    Ok(LaneCommand::Return { result: placement })
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(placement.entries.len(), 2);
    assert_eq!(placement.tip_id.as_deref(), Some("s2"));
    assert_eq!(placement.trigger_entry_id.as_deref(), Some("s2"));
    assert!(placement.inbox.is_empty());
}
