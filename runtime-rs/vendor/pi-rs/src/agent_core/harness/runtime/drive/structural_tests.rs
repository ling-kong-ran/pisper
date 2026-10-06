//! Port of `packages/agent/test/harness/runtime/drive-structural.test.ts`
//! (sha256 `3e96860d693c851237127542ce01ce45fe9738475753706e6893d5c98f5a6483`)
//! plus node-oracle comparisons over the upstream pure literals
//! (scratch `structural_oracle.mjs`, run with
//! `node --experimental-strip-types`).
//!
//! Disclosed substitutions kept from the implementation module:
//! - `lane.appendCustomEntry` is not ported; the queue-write tests install the
//!   pending entry and inbox item (and, where upstream relied on
//!   appendCustomEntry's `queue_update` emission, build the same event through
//!   `read_lane_queues`).
//! - `vi.setSystemTime` becomes a durable rewrite of the retry-wait state's
//!   `notBefore` (the clock input of the ported `retry_not_before`).
//! - `InstrumentedStorage`/`MemoryStorage({ now: () => 100 })` map to the
//!   ported decorators.
//! - The fixed `HarnessEvent` union drops `retry_scheduled.recovery`,
//!   failed `compaction_end.error`, and declined `navigation_end`
//!   (`RunEndStatus` has no `Declined`); assertions cover the representable
//!   fields and mark each gap.
//! - The landed `finish_run_boundary` chains its pending events after its own
//!   batch (upstream prepends them), so the may-finish decline test asserts
//!   the landed order with a marked comment.

use super::*;
use std::collections::HashSet;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::agent_core::harness::compaction::{CompactionDetails, FileOperations};
use crate::agent_core::harness::hooks::{BeforeCompactionHookResult, BeforeNavigationHookResult};
use crate::agent_core::harness::runtime::drive::checkpoint::run_checkpoint;
use crate::agent_core::harness::runtime::drive::generation::run_generation;
use crate::agent_core::harness::runtime::drive::publish::PublishOutcome as CheckpointPublishOutcome;
use crate::agent_core::harness::runtime::drive_pass::DriveOptions;
use crate::agent_core::harness::runtime::durable::{
    CheckpointData, Continuation, GenerationContext, Operation, OperationIntent, OperationMeta,
    OperationPhase, OperationScope, ResultBoundary, RunSettings,
    SummaryContext as DurableSummaryContext, SummaryTask,
};
use crate::agent_core::harness::runtime::lane::{
    EmitBatch, LaneCommand, OperationRequest, RuntimeConfig,
};
use crate::agent_core::harness::runtime::projection::{LaneQueuedItem, WriteKind};
use crate::agent_core::harness::runtime::restore::restore_lane;
use crate::agent_core::harness::runtime::transcript::read_lane_queues;
use crate::agent_core::harness::session as session_mod;
use crate::agent_core::harness::session::commit::insert_entry;
use crate::agent_core::harness::session::types::{EntryType, Storage, ValueWrite};
use crate::agent_core::harness::session::values::{
    branch_tip, lane_state, operation_meta, operation_preparation, operation_state, pending_entry,
    set_value,
};
use crate::agent_core::harness::session::{
    Control, InboxItem, InboxItemKind, LaneConfiguration, LaneModel, MemoryStorage,
    MemoryStorageOptions, PendingEntry, SessionMetadata, SessionReader as _, StorageBackedSession,
    Write,
};
use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, FauxFactoryArgs, FauxMessageOptions, FauxProviderHandle,
    FauxProviderOptions, FauxResponseFactory, FauxResponseStep, FauxTokenSize,
};
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::ai::types::message::{StringOrBlocks, UserMessage};
use crate::ai::types::primitives::{StopReason, Usage};

const OPERATION_ID: &str = "01950000-0000-7000-8000-000000000001";

/// Upstream `deferred()` (`test-utils.ts`): the tests use raw `Notify`
/// latches, which store a permit, so a test may release before or after the
/// hook observes it.
struct Fixture {
    lane: Arc<Lane>,
    drive: Arc<Drive>,
    session: Arc<StorageBackedSession>,
    storage: Arc<crate::agent_core::harness::session::testing::InstrumentedStorage>,
    faux: FauxProviderHandle,
    events: Arc<Mutex<Vec<HarnessEvent>>>,
    configuration: LaneConfiguration,
}

fn user(content: &str) -> AgentMessage {
    user_at(content, 1)
}

fn user_at(content: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(content.to_string()),
        timestamp,
    })
}

async fn create_fixture(name: &str) -> Fixture {
    create_fixture_with(
        name,
        FauxProviderOptions {
            token_size: Some(FauxTokenSize {
                min: Some(1),
                max: Some(1),
            }),
            ..Default::default()
        },
    )
    .await
}

async fn create_fixture_with(name: &str, provider_options: FauxProviderOptions) -> Fixture {
    let config = Arc::new(Mutex::new(RuntimeConfig {
        retry_policy: crate::ai::retry::RetryPolicy {
            enabled: true,
            max_retries: 1,
            base_delay_ms: 10,
            max_agent_delay_ms: None,
        },
        ..RuntimeConfig::default()
    }));
    let storage = crate::agent_core::harness::session::testing::InstrumentedStorage::new(Arc::new(
        MemoryStorage::new(MemoryStorageOptions {
            now: Some(Arc::new(|| 100)),
        }),
    ));
    let session = Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: name.to_string(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        Arc::clone(&storage) as Arc<dyn Storage>,
    ));
    let faux = faux_provider(provider_options);
    let model = faux.get_model(None).expect("faux model");
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let configuration = LaneConfiguration {
        model: LaneModel {
            provider: model.provider.clone(),
            model_id: model.id.clone(),
        },
        thinking_level: ThinkingLevel::Off,
        active_tool_names: Vec::new(),
    };
    let writes = vec![
        set_value(&session_mod::branch_tip("main"), serde_json::Value::Null),
        set_value(
            &session_mod::lane_config("main"),
            session_mod::lane_configuration_value(&configuration),
        ),
        set_value(
            &session_mod::lane_state("main"),
            serde_json::json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
        ),
    ];
    session
        .mutate(
            move |reader, context| {
                let writes = writes.clone();
                Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
            },
            background_context(),
        )
        .await
        .unwrap();
    let state = restore_lane(session.as_ref(), "main", background_context())
        .await
        .expect("lane restores");
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let emit: EmitBatch = Arc::new(move |batch, _context| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded.lock().unwrap().extend(batch);
            Ok(())
        })
    });
    let lane = Lane::new(
        "main",
        Arc::clone(&session),
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
        Arc::new(move || config.lock().unwrap().clone()),
    );
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: OPERATION_ID.to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    storage.clear_commit_attempts();
    Fixture {
        lane,
        drive,
        session,
        storage,
        faux,
        events,
        configuration,
    }
}

fn run_scope(compaction: CompactionSettings) -> OperationScope {
    OperationScope {
        control: Control::Running,
        settings: RunSettings {
            compaction,
            steering_mode: QueueMode::All,
            follow_up_mode: QueueMode::All,
            tool_execution: ToolExecutionMode::Parallel,
        },
        latest_assistant_entry_id: None,
    }
}

fn standalone_compaction_task() -> SummaryTask {
    SummaryTask {
        task_id: "task".to_string(),
        reason: Some(crate::agent_core::harness::runtime::durable::SummaryReason::Manual),
        custom_instructions: None,
        boundary: ResultBoundary::Finish,
    }
}

fn run_compaction_task(
    reason: crate::agent_core::harness::runtime::durable::SummaryReason,
    resume_after: CheckpointData,
) -> SummaryTask {
    SummaryTask {
        task_id: "task".to_string(),
        reason: Some(reason),
        custom_instructions: None,
        boundary: ResultBoundary::ResumeCheckpoint { resume_after },
    }
}

fn navigation_summary_task(target_id: &str) -> SummaryTask {
    SummaryTask {
        task_id: "task".to_string(),
        reason: None,
        custom_instructions: None,
        boundary: ResultBoundary::CommitNavigation {
            target_id: target_id.to_string(),
            label: None,
        },
    }
}

fn summary_ready(
    scope: OperationScope,
    task: SummaryTask,
    configuration: &LaneConfiguration,
) -> OperationState {
    summary_ready_with_policy(
        scope,
        task,
        configuration,
        crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy {
            max_attempts: 2,
            base_delay_ms: 10,
            max_agent_delay_ms: 30_000,
        },
    )
}

fn summary_ready_with_policy(
    scope: OperationScope,
    task: SummaryTask,
    configuration: &LaneConfiguration,
    retry_policy: crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy,
) -> OperationState {
    OperationState {
        scope,
        phase: OperationPhase::SummaryReady {
            task,
            summary_context: DurableSummaryContext {
                result_entry_id: "summary-entry".to_string(),
                configuration: configuration.clone(),
                stream_options: Default::default(),
                retry_policy,
            },
            next_attempt: 1,
        },
    }
}

fn test_compaction_preparation() -> CompactionPreparation {
    CompactionPreparation {
        messages_to_summarize: vec![user("history")],
        turn_prefix_messages: vec![],
        retained_tail: vec![user_at("tail", 2)],
        is_split_turn: false,
        tokens_before: 1_000,
        previous_summary: None,
        file_ops: FileOperations::new(),
        settings: CompactionSettings {
            enabled: true,
            reserve_tokens: 1_000,
            keep_recent_tokens: 10,
        },
    }
}

fn durable_compaction_preparation_of(
    preparation: &CompactionPreparation,
) -> DurableCompactionPreparation {
    durable_compaction_preparation(preparation)
}

fn test_branch_preparation() -> DurableBranchPreparation {
    DurableBranchPreparation {
        messages: vec![user("abandoned")],
        file_ops: DurableFileOperations {
            read: Vec::new(),
            written: Vec::new(),
            edited: Vec::new(),
        },
        total_tokens: 10,
    }
}

fn message_entry(id: &str, parent_id: Option<&str>, content: &str) -> NewEntry {
    NewEntry::Message {
        id: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        message: user(content),
        terminate: None,
    }
}

struct InstallOptions {
    entries: Vec<NewEntry>,
    tip_id: Option<String>,
    preparation: Option<(String, DurableStructuralPreparation)>,
}

async fn install_operation(
    fixture: &Fixture,
    state: OperationState,
    intent: OperationIntent,
    options: InstallOptions,
) {
    let tip_id = options
        .tip_id
        .or_else(|| options.entries.last().map(|entry| entry.id().to_string()));
    let meta = OperationMeta {
        operation_id: OPERATION_ID.to_string(),
        lane: "main".to_string(),
        source_tip_id: tip_id.clone(),
        started_at: 1,
        intent,
    };
    let entries = options.entries;
    let preparation = options.preparation;
    fixture
        .lane
        .command(
            move |projection, _reader| {
                let entries = entries.clone();
                let tip_id = tip_id.clone();
                let meta = meta.clone();
                let state = state.clone();
                let preparation = preparation.clone();
                Box::pin(async move {
                    let mut writes: Vec<Write> =
                        entries.iter().cloned().map(insert_entry).collect();
                    writes.push(set_value(
                        &branch_tip("main"),
                        tip_id
                            .clone()
                            .map(serde_json::Value::String)
                            .unwrap_or(serde_json::Value::Null),
                    ));
                    writes.push(set_value(
                        &operation_meta(OPERATION_ID),
                        serde_json::to_value(&meta)?,
                    ));
                    writes.push(set_value(
                        &operation_state(OPERATION_ID),
                        serde_json::to_value(&state)?,
                    ));
                    writes.push(set_value(
                        &lane_state("main"),
                        serde_json::json!({
                            "currentOperationId": OPERATION_ID,
                            "lastOperationId": null,
                            "inbox": projection.inbox,
                        }),
                    ));
                    if let Some((task_id, value)) = &preparation {
                        writes.push(set_value(
                            &operation_preparation(OPERATION_ID, task_id),
                            serde_json::to_value(value)?,
                        ));
                    }
                    let mut next = projection.clone();
                    next.tip_id = tip_id;
                    next.operation = Some(Operation { meta, state });
                    Ok(LaneCommand::Commit {
                        writes,
                        next,
                        materialize: Box::new(|_: &session_mod::CommitResult| ()),
                        events: None,
                    })
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    fixture.storage.clear_commit_attempts();
}

fn current_state(fixture: &Fixture) -> OperationState {
    fixture
        .lane
        .state()
        .operation
        .expect("fixture has an operation")
        .state
}

async fn cancel_operation(fixture: &Fixture) {
    let operation = fixture.lane.state().operation.expect("operation");
    let mut state = operation.state.clone();
    state.scope.control = Control::CancelRequested { requested_at: 2 };
    fixture
        .lane
        .settle_operation(
            move |_, _, _, _| {
                let state = state.clone();
                Box::pin(async move {
                    Ok(OperationCommand::Commit {
                        writes: Vec::new(),
                        operation_state: state,
                        lane: None,
                        materialize: Box::new(|_: &session_mod::CommitResult| ()),
                        events: None,
                    })
                })
            },
            background_context(),
        )
        .await
        .unwrap();
}

async fn queue_write(
    fixture: &Fixture,
    entry_id: &str,
    pending: PendingEntry,
    kind: InboxItemKind,
) -> String {
    let entry_id = entry_id.to_string();
    let closure_entry_id = entry_id.clone();
    fixture
        .lane
        .command(
            move |projection, reader| {
                let entry_id = closure_entry_id.clone();
                let pending = pending.clone();
                let kind = kind;
                let context = background_context();
                Box::pin(async move {
                    // The planner reader sees pre-commit storage, so the new
                    // item's queue projection is appended locally (the
                    // upstream appendCustomEntry emits the post-commit
                    // snapshot; this substitute reconstructs it).
                    let mut queues =
                        read_lane_queues(reader, &projection.inbox, context.clone()).await?;
                    let new_queue = match &pending {
                        PendingEntry::Message { payload } => LaneQueuedItem::Message {
                            entry_id: entry_id.clone(),
                            kind,
                            message: payload.clone(),
                        },
                        PendingEntry::Custom {
                            custom_type,
                            payload,
                        } => LaneQueuedItem::Custom {
                            entry_id: entry_id.clone(),
                            kind: WriteKind::Write,
                            custom_type: custom_type.clone(),
                            data: payload.clone(),
                        },
                    };
                    queues.push(new_queue);
                    let mut inbox = projection.inbox.clone();
                    inbox.push(InboxItem {
                        entry_id: entry_id.clone(),
                        kind,
                    });
                    let mut writes: Vec<Write> = vec![set_value(
                        &pending_entry(&entry_id),
                        serde_json::to_value(&pending)?,
                    )];
                    writes.push(set_value(
                        &lane_state("main"),
                        serde_json::json!({
                            "currentOperationId": projection
                                .operation
                                .as_ref()
                                .map(|operation| operation.meta.operation_id.clone()),
                            "lastOperationId": projection.last_operation_id,
                            "inbox": inbox,
                        }),
                    ));
                    let mut next = projection.clone();
                    next.inbox = inbox;
                    Ok(LaneCommand::Commit {
                        writes,
                        next,
                        materialize: Box::new(|_: &session_mod::CommitResult| ()),
                        events: Some(Box::new(move |_: &session_mod::CommitResult| {
                            Ok(vec![HarnessEvent::QueueUpdate {
                                lane: "main".to_string(),
                                queues,
                            }])
                        })),
                    })
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    entry_id
}

fn commit_attempts(fixture: &Fixture) -> Vec<Vec<Write>> {
    fixture.storage.get_commit_attempts()
}

fn usage_write_count(fixture: &Fixture) -> usize {
    commit_attempts(fixture)
        .iter()
        .flatten()
        .filter(|write| matches!(write, Write::Usage { .. }))
        .count()
}

fn events(fixture: &Fixture) -> Vec<HarnessEvent> {
    fixture.events.lock().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Upstream test port (drive-structural.test.ts)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn routes_a_declined_threshold_directly_to_assistant_generation() {
    let fixture = create_fixture("structural-declined-threshold").await;
    let model = fixture.faux.get_model(None).unwrap();
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: model.context_window,
        keep_recent_tokens: 1,
    };
    let checkpoint = OperationState {
        scope: run_scope(settings),
        phase: OperationPhase::Checkpoint {
            checkpoint: CheckpointData {
                continuation: Continuation::NeedAssistant {
                    overflow_recovery_used: false,
                },
                trigger_entry_id: "assistant".to_string(),
            },
        },
    };
    install_operation(
        &fixture,
        checkpoint.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["user".to_string()],
        },
        InstallOptions {
            entries: vec![
                message_entry("user", None, "question"),
                NewEntry::Message {
                    id: "assistant".to_string(),
                    parent_id: Some("user".to_string()),
                    message: AgentMessage::Assistant(faux_assistant_message(
                        "answer",
                        Default::default(),
                    )),
                    terminate: None,
                },
            ],
            tip_id: None,
            preparation: None,
        },
    )
    .await;

    assert!(matches!(
        run_checkpoint(&fixture.lane, &fixture.drive, &checkpoint)
            .await
            .unwrap(),
        CheckpointPublishOutcome::Continue
    ));
    let deciding = current_state(&fixture);
    let OperationPhase::SummaryDeciding { task } = &deciding.phase else {
        panic!("threshold did not enter compaction");
    };
    assert!(matches!(
        task.boundary,
        ResultBoundary::ResumeCheckpoint { .. }
    ));
    assert!(fixture
        .session
        .get_value(
            &operation_preparation(OPERATION_ID, &task.task_id),
            background_context()
        )
        .await
        .unwrap()
        .is_some());

    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: Some(true),
                                compaction: None,
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();
    assert!(matches!(
        run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    let routed = current_state(&fixture);
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &routed.phase
    else {
        panic!("threshold decline did not route to generation");
    };
    assert_eq!(generation_context.trigger_entry_id, "assistant");
    assert!(!generation_context.overflow_recovery_used);
    assert_eq!(
        events(&fixture)
            .iter()
            .filter(|event| matches!(event, HarnessEvent::CompactionStart { .. }))
            .count(),
        1
    );
    assert_eq!(
        events(&fixture)
            .iter()
            .filter(|event| matches!(event, HarnessEvent::CompactionEnd { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn uses_a_newer_compaction_entry_as_the_durable_threshold_guard() {
    let fixture = create_fixture("structural-guarded-threshold").await;
    let model = fixture.faux.get_model(None).unwrap();
    let checkpoint = OperationState {
        scope: run_scope(CompactionSettings {
            enabled: true,
            reserve_tokens: model.context_window,
            keep_recent_tokens: 1,
        }),
        phase: OperationPhase::Checkpoint {
            checkpoint: CheckpointData {
                continuation: Continuation::NeedAssistant {
                    overflow_recovery_used: false,
                },
                trigger_entry_id: "trigger".to_string(),
            },
        },
    };
    install_operation(
        &fixture,
        checkpoint.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["trigger".to_string()],
        },
        InstallOptions {
            entries: vec![
                message_entry("trigger", None, "history"),
                NewEntry::Compaction {
                    id: "compacted".to_string(),
                    parent_id: Some("trigger".to_string()),
                    summary: "already compacted".to_string(),
                    retained_tail: Vec::new(),
                    tokens_before: model.context_window as i64,
                    details: None,
                    usage: None,
                    from_hook: false,
                },
            ],
            tip_id: None,
            preparation: None,
        },
    )
    .await;

    assert!(matches!(
        run_checkpoint(&fixture.lane, &fixture.drive, &checkpoint)
            .await
            .unwrap(),
        CheckpointPublishOutcome::Continue
    ));
    let ready = current_state(&fixture);
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &ready.phase
    else {
        panic!("newer compaction did not guard threshold re-entry");
    };
    assert_eq!(generation_context.trigger_entry_id, "trigger");
    assert!(!events(&fixture)
        .iter()
        .any(|event| matches!(event, HarnessEvent::CompactionStart { .. })));
}

#[tokio::test]
async fn rejects_a_missing_threshold_trigger_when_no_newer_compaction_guards_it() {
    let fixture = create_fixture("structural-missing-trigger").await;
    let model = fixture.faux.get_model(None).unwrap();
    let checkpoint = OperationState {
        scope: run_scope(CompactionSettings {
            enabled: true,
            reserve_tokens: model.context_window,
            keep_recent_tokens: 1,
        }),
        phase: OperationPhase::Checkpoint {
            checkpoint: CheckpointData {
                continuation: Continuation::NeedAssistant {
                    overflow_recovery_used: false,
                },
                trigger_entry_id: "missing-trigger".to_string(),
            },
        },
    };
    install_operation(
        &fixture,
        checkpoint.clone(),
        OperationIntent::Run {
            prompt_entry_ids: Vec::new(),
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: None,
        },
    )
    .await;

    let error = run_checkpoint(&fixture.lane, &fixture.drive, &checkpoint)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Checkpoint trigger missing-trigger is missing from its Branch"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn finishes_a_may_finish_run_directly_after_threshold_decline() {
    let fixture = create_fixture("structural-may-finish-decline").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: run_compaction_task(
                crate::agent_core::harness::runtime::durable::SummaryReason::Threshold,
                CheckpointData {
                    continuation: Continuation::MayFinish {
                        include_final_assistant: false,
                    },
                    trigger_entry_id: "tip".to_string(),
                },
            ),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    let finish_hooks = Arc::new(Mutex::new(0u32));
    let counted = Arc::clone(&finish_hooks);
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: Some(true),
                                compaction: None,
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeRunEnd,
            Arc::new(move |_invocation, _context| {
                let counted = Arc::clone(&counted);
                Box::pin(async move {
                    *counted.lock().unwrap() += 1;
                    Ok(crate::agent_core::harness::hooks::HookResult::BeforeRunEnd(
                        None,
                    ))
                })
            }),
            None,
        )
        .unwrap();

    let result = run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.operation_id, OPERATION_ID);
            assert_eq!(outcome.kind, "run");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Completed
            );
            assert_eq!(outcome.tip_id.as_deref(), Some("tip"));
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert_eq!(commit_attempts(&fixture).len(), 1);
    assert_eq!(*finish_hooks.lock().unwrap(), 1);
    assert!(fixture.lane.state().operation.is_none());
    // Upstream prepends its pending compaction_end before the run_end; the
    // landed finish_run_boundary chains pending events last (disclosure).
    let last_two = &events(&fixture)[events(&fixture).len() - 2..];
    assert!(matches!(
        last_two[0],
        HarnessEvent::RunEnd {
            status: RunEndStatus::Completed,
            ..
        }
    ));
    assert!(matches!(
        last_two[1],
        HarnessEvent::CompactionEnd {
            status: CompactionEndStatus::Declined,
            reason: crate::agent_core::harness::runtime::durable::SummaryReason::Threshold,
            ..
        }
    ));
}

#[tokio::test]
async fn routes_queued_follow_up_before_before_run_end_after_threshold_decline() {
    let fixture = create_fixture("structural-follow-up-first").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: run_compaction_task(
                crate::agent_core::harness::runtime::durable::SummaryReason::Threshold,
                CheckpointData {
                    continuation: Continuation::MayFinish {
                        include_final_assistant: false,
                    },
                    trigger_entry_id: "tip".to_string(),
                },
            ),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    // Queue a follow-up while the operation is deciding.
    let follow_up_id = queue_write(
        &fixture,
        "follow-up",
        PendingEntry::Message {
            payload: user("continue"),
        },
        InboxItemKind::FollowUp,
    )
    .await;
    fixture.storage.clear_commit_attempts();
    let finish_hooks = Arc::new(Mutex::new(0u32));
    let counted = Arc::clone(&finish_hooks);
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: Some(true),
                                compaction: None,
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeRunEnd,
            Arc::new(move |_invocation, _context| {
                let counted = Arc::clone(&counted);
                Box::pin(async move {
                    *counted.lock().unwrap() += 1;
                    Ok(crate::agent_core::harness::hooks::HookResult::BeforeRunEnd(
                        None,
                    ))
                })
            }),
            None,
        )
        .unwrap();

    assert!(matches!(
        run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    assert_eq!(commit_attempts(&fixture).len(), 1);
    assert_eq!(*finish_hooks.lock().unwrap(), 0);
    let ready = current_state(&fixture);
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &ready.phase
    else {
        panic!("follow-up did not route to generation");
    };
    assert_eq!(generation_context.trigger_entry_id, follow_up_id);
    assert!(events(&fixture)
        .iter()
        .any(|event| matches!(event, HarnessEvent::CompactionEnd { .. })));
}

async fn install_in_run_compaction_fixture(
    name: &str,
    include_final_assistant: bool,
) -> (Fixture, OperationState) {
    let fixture = create_fixture(name).await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: run_compaction_task(
                crate::agent_core::harness::runtime::durable::SummaryReason::Threshold,
                CheckpointData {
                    continuation: Continuation::MayFinish {
                        include_final_assistant,
                    },
                    trigger_entry_id: "tip".to_string(),
                },
            ),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    (fixture, deciding)
}

#[tokio::test]
async fn continues_to_an_assistant_turn_when_steer_arrives_during_in_run_compaction() {
    let (fixture, deciding) =
        install_in_run_compaction_fixture("structural-steer-in-run", true).await;
    let started_signal = Arc::new(tokio::sync::Notify::new());
    let hook_started = Arc::clone(&started_signal);
    let release = Arc::new(tokio::sync::Notify::new());
    let release_notify = Arc::clone(&release);
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(move |_invocation, _context| {
                let hook_started = Arc::clone(&hook_started);
                let release_notify = Arc::clone(&release_notify);
                Box::pin(async move {
                    hook_started.notify_one();
                    release_notify.notified().await;
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: None,
                                compaction: Some(CompactResult {
                                    summary: "hook summary".to_string(),
                                    tokens_before: 1_000,
                                    usage: None,
                                    retained_tail: vec![user_at("tail", 2)],
                                    details: None,
                                }),
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    let lane = Arc::clone(&fixture.lane);
    let drive = Arc::clone(&fixture.drive);
    let deciding_for_run = deciding.clone();
    let running =
        tokio::spawn(
            async move { run_structural_decision(&lane, &drive, &deciding_for_run).await },
        );
    started_signal.notified().await;
    let queued_id = queue_write(
        &fixture,
        "queued",
        PendingEntry::Message {
            payload: user("steer during compaction"),
        },
        InboxItemKind::Steer,
    )
    .await;
    release.notify_one();
    assert!(matches!(
        running.await.unwrap().unwrap(),
        ProcedureResult::Continue
    ));

    let routed = current_state(&fixture);
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &routed.phase
    else {
        panic!("steer did not route directly to generation");
    };
    assert_eq!(generation_context.trigger_entry_id, queued_id);
    assert!(!generation_context.overflow_recovery_used);
    assert!(fixture.lane.state().inbox.is_empty());
    assert!(fixture
        .session
        .get_value(&pending_entry(&queued_id), background_context())
        .await
        .unwrap()
        .is_none());
    let entry = fixture
        .session
        .get_entry(&queued_id, background_context())
        .await
        .unwrap()
        .expect("queued entry committed");
    let parent_id = entry.parent_id().expect("queued parent").to_string();
    let parent = fixture
        .session
        .get_entry(&parent_id, background_context())
        .await
        .unwrap()
        .expect("queued parent entry");
    assert_eq!(parent.entry_type(), EntryType::Compaction);
}

#[tokio::test]
async fn continues_to_an_assistant_turn_when_follow_up_arrives_during_in_run_compaction() {
    let (fixture, deciding) =
        install_in_run_compaction_fixture("structural-follow-up-in-run", true).await;
    let started_signal = Arc::new(tokio::sync::Notify::new());
    let hook_started = Arc::clone(&started_signal);
    let release = Arc::new(tokio::sync::Notify::new());
    let release_notify = Arc::clone(&release);
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(move |_invocation, _context| {
                let hook_started = Arc::clone(&hook_started);
                let release_notify = Arc::clone(&release_notify);
                Box::pin(async move {
                    hook_started.notify_one();
                    release_notify.notified().await;
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: None,
                                compaction: Some(CompactResult {
                                    summary: "hook summary".to_string(),
                                    tokens_before: 1_000,
                                    usage: None,
                                    retained_tail: vec![user_at("tail", 2)],
                                    details: None,
                                }),
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    let lane = Arc::clone(&fixture.lane);
    let drive = Arc::clone(&fixture.drive);
    let deciding_for_run = deciding.clone();
    let running =
        tokio::spawn(
            async move { run_structural_decision(&lane, &drive, &deciding_for_run).await },
        );
    started_signal.notified().await;
    let queued_id = queue_write(
        &fixture,
        "queued",
        PendingEntry::Message {
            payload: user("followUp during compaction"),
        },
        InboxItemKind::FollowUp,
    )
    .await;
    release.notify_one();
    assert!(matches!(
        running.await.unwrap().unwrap(),
        ProcedureResult::Continue
    ));

    let routed = current_state(&fixture);
    let OperationPhase::Checkpoint { .. } = &routed.phase else {
        panic!("follow-up did not reach the finish checkpoint");
    };
    assert_eq!(
        fixture.lane.state().inbox,
        vec![InboxItem {
            entry_id: queued_id.clone(),
            kind: InboxItemKind::FollowUp,
        }]
    );
    assert!(matches!(
        run_checkpoint(&fixture.lane, &fixture.drive, &routed)
            .await
            .unwrap(),
        CheckpointPublishOutcome::Continue
    ));
    let ready = current_state(&fixture);
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &ready.phase
    else {
        panic!("follow-up did not route directly to generation");
    };
    assert_eq!(generation_context.trigger_entry_id, queued_id);
    assert!(!generation_context.overflow_recovery_used);
    assert!(fixture.lane.state().inbox.is_empty());
}

#[tokio::test]
async fn publishes_structural_output_and_mixed_write_steer_input_in_one_admission_ordered_commit() {
    let fixture = create_fixture("structural-mixed-input").await;
    let mut scope = run_scope(DEFAULT_COMPACTION_SETTINGS);
    scope.settings.steering_mode = QueueMode::OneAtATime;
    let deciding = OperationState {
        scope,
        phase: OperationPhase::SummaryDeciding {
            task: run_compaction_task(
                crate::agent_core::harness::runtime::durable::SummaryReason::Threshold,
                CheckpointData {
                    continuation: Continuation::NeedAssistant {
                        overflow_recovery_used: false,
                    },
                    trigger_entry_id: "tip".to_string(),
                },
            ),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    for (entry_id, kind, pending) in [
        (
            "write-1",
            InboxItemKind::Write,
            PendingEntry::Custom {
                custom_type: "note".to_string(),
                payload: Some(serde_json::json!({"order": 1})),
            },
        ),
        (
            "steer",
            InboxItemKind::Steer,
            PendingEntry::Message {
                payload: user("steer"),
            },
        ),
        (
            "write-2",
            InboxItemKind::Write,
            PendingEntry::Custom {
                custom_type: "note".to_string(),
                payload: Some(serde_json::json!({"order": 2})),
            },
        ),
        (
            "steer-2",
            InboxItemKind::Steer,
            PendingEntry::Message {
                payload: user("next steer"),
            },
        ),
    ] {
        let entry_id = queue_write(&fixture, entry_id, pending, kind).await;
        let _ = entry_id;
    }
    fixture.storage.clear_commit_attempts();
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: None,
                                compaction: Some(CompactResult {
                                    summary: "summary".to_string(),
                                    tokens_before: 1_000,
                                    usage: None,
                                    retained_tail: vec![user_at("tail", 2)],
                                    details: None,
                                }),
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    assert!(matches!(
        run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    assert_eq!(commit_attempts(&fixture).len(), 1);
    assert_eq!(
        fixture.lane.state().inbox,
        vec![InboxItem {
            entry_id: "steer-2".to_string(),
            kind: InboxItemKind::Steer,
        }]
    );
    assert!(fixture
        .session
        .get_value(&pending_entry("steer-2"), background_context())
        .await
        .unwrap()
        .is_some());
    let routed = current_state(&fixture);
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &routed.phase
    else {
        panic!("mixed input did not route to generation");
    };
    assert_eq!(generation_context.trigger_entry_id, "steer");
    let write_1 = fixture
        .session
        .get_entry("write-1", background_context())
        .await
        .unwrap()
        .expect("write-1 committed");
    assert!(write_1.parent_id().is_some());
    let steer = fixture
        .session
        .get_entry("steer", background_context())
        .await
        .unwrap()
        .expect("steer committed");
    assert_eq!(steer.parent_id(), Some("write-1"));
    let write_2 = fixture
        .session
        .get_entry("write-2", background_context())
        .await
        .unwrap()
        .expect("write-2 committed");
    assert_eq!(write_2.parent_id(), Some("steer"));
    assert_eq!(fixture.lane.state().tip_id.as_deref(), Some("write-2"));
}

#[tokio::test]
async fn queues_writes_during_standalone_structural_work_without_changing_the_operation() {
    let fixture = create_fixture("structural-queue-writes").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: standalone_compaction_task(),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;

    let entry_id = queue_write(
        &fixture,
        "queued-note",
        PendingEntry::Custom {
            custom_type: "note".to_string(),
            payload: Some(serde_json::json!({"pending": true})),
        },
        InboxItemKind::Write,
    )
    .await;

    assert_eq!(current_state(&fixture), deciding);
    assert_eq!(fixture.lane.state().tip_id.as_deref(), Some("tip"));
    assert_eq!(
        fixture.lane.state().inbox,
        vec![InboxItem {
            entry_id: entry_id.clone(),
            kind: InboxItemKind::Write,
        }]
    );
    let stored = fixture
        .session
        .get_value(&lane_state("main"), background_context())
        .await
        .unwrap()
        .expect("lane state");
    assert_eq!(
        stored.value,
        serde_json::json!({
            "currentOperationId": OPERATION_ID,
            "lastOperationId": null,
            "inbox": [{"entryId": entry_id, "kind": "write"}],
        })
    );
    assert!(fixture
        .session
        .get_value(&pending_entry(&entry_id), background_context())
        .await
        .unwrap()
        .is_some());
    let last = events(&fixture).last().cloned().expect("events");
    match last {
        HarnessEvent::QueueUpdate { queues, .. } => {
            assert_eq!(queues.len(), 1);
            let value = serde_json::to_value(&queues[0]).unwrap();
            assert_eq!(value["entryId"], serde_json::Value::String(entry_id));
            assert_eq!(value["kind"], "write");
            assert_eq!(value["type"], "custom");
        }
        other => panic!("expected queue_update, got {other:?}"),
    }
}

#[tokio::test]
async fn publishes_overflow_preparation_with_the_normalized_response_settlement() {
    let fixture = create_fixture("structural-overflow-preparation").await;
    let ready = OperationState {
        scope: run_scope(CompactionSettings {
            enabled: true,
            reserve_tokens: 1_000,
            keep_recent_tokens: 1,
        }),
        phase: OperationPhase::AssistantReady {
            generation_context: GenerationContext {
                step_id: "step".to_string(),
                trigger_entry_id: "tip".to_string(),
                configuration: fixture.configuration.clone(),
                stream_options: Default::default(),
                retry_policy: crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy {
                    max_attempts: 2,
                    base_delay_ms: 10,
                    max_agent_delay_ms: 30_000,
                },
                overflow_recovery_used: false,
            },
            next_attempt: 1,
        },
    };
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "large prompt")],
            tip_id: None,
            preparation: None,
        },
    )
    .await;
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message(
                "",
                crate::ai::models::faux::FauxMessageOptions {
                    stop_reason: Some(StopReason::Error),
                    error_message: Some("prompt exceeds the context window".to_string()),
                    ..Default::default()
                },
            ),
        ))]);

    assert!(matches!(
        run_generation(&fixture.lane, &fixture.drive, &ready)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    let deciding = current_state(&fixture);
    let OperationPhase::SummaryDeciding { task } = &deciding.phase else {
        panic!("overflow did not enter compaction");
    };
    assert_eq!(
        task.reason,
        Some(crate::agent_core::harness::runtime::durable::SummaryReason::Overflow)
    );
    let ResultBoundary::ResumeCheckpoint { resume_after } = &task.boundary else {
        panic!("overflow has wrong boundary");
    };
    assert!(matches!(
        resume_after.continuation,
        Continuation::NeedAssistant {
            overflow_recovery_used: true
        }
    ));
    assert_eq!(resume_after.trigger_entry_id, "tip");
    assert!(fixture
        .session
        .get_value(
            &operation_preparation(OPERATION_ID, &task.task_id),
            background_context()
        )
        .await
        .unwrap()
        .is_some());
    let tip = fixture.lane.state().tip_id.clone().expect("tip");
    let response = fixture
        .session
        .get_entry(&tip, background_context())
        .await
        .unwrap()
        .expect("response entry");
    match &response {
        crate::agent_core::harness::session::Entry::Message { message, .. } => {
            let AgentMessage::Assistant(assistant) = message else {
                panic!("response is an assistant message");
            };
            assert_eq!(assistant.stop_reason, StopReason::Error);
        }
        other => panic!("expected message entry, got {other:?}"),
    }
    assert!(events(&fixture)
        .iter()
        .any(|event| matches!(event, HarnessEvent::CompactionStart { .. })));
}

#[tokio::test]
async fn publishes_a_hook_compaction_and_terminal_cleanup_atomically_without_assistant_lifecycle() {
    let fixture = create_fixture("structural-hook-compaction").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: SummaryTask {
                task_id: "task".to_string(),
                reason: None,
                custom_instructions: Some("focus".to_string()),
                boundary: ResultBoundary::Finish,
            },
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Compaction {
            custom_instructions: Some("focus".to_string()),
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    let queued_id = queue_write(
        &fixture,
        "retained-queued",
        PendingEntry::Custom {
            custom_type: "retained".to_string(),
            payload: Some(serde_json::json!({"after": "compaction"})),
        },
        InboxItemKind::Write,
    )
    .await;
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: None,
                                compaction: Some(CompactResult {
                                    summary: "hook summary".to_string(),
                                    tokens_before: 1_000,
                                    usage: None,
                                    retained_tail: vec![user_at("tail", 2)],
                                    // The landed hook result carries the typed
                                    // CompactionDetails; upstream tests returned
                                    // arbitrary detail JSON ({source: "hook"}).
                                    details: Some(CompactionDetails {
                                        read_files: Vec::new(),
                                        modified_files: Vec::new(),
                                    }),
                                }),
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    let result = run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.operation_id, OPERATION_ID);
            assert_eq!(outcome.kind, "compaction");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Completed
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert!(fixture.lane.state().operation.is_none());
    assert_eq!(
        fixture.lane.state().inbox,
        vec![InboxItem {
            entry_id: queued_id.clone(),
            kind: InboxItemKind::Write,
        }]
    );
    assert!(fixture
        .session
        .get_value(&pending_entry(&queued_id), background_context())
        .await
        .unwrap()
        .is_some());
    let tip = fixture.lane.state().tip_id.clone().expect("tip");
    let entry = fixture
        .session
        .get_entry(&tip, background_context())
        .await
        .unwrap()
        .expect("compaction entry");
    match &entry {
        crate::agent_core::harness::session::Entry::Compaction {
            summary,
            from_hook,
            timestamp,
            ..
        } => {
            assert_eq!(summary, "hook summary");
            assert!(from_hook);
            assert_eq!(*timestamp, 100);
        }
        other => panic!("expected compaction entry, got {other:?}"),
    }
    assert!(fixture
        .session
        .get_value(&operation_meta(OPERATION_ID), background_context())
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .session
        .get_value(
            &operation_preparation(OPERATION_ID, "task"),
            background_context()
        )
        .await
        .unwrap()
        .is_none());
    assert!(!events(&fixture).iter().any(|event| matches!(
        event,
        HarnessEvent::MessageStart { .. } | HarnessEvent::MessageEnd { .. }
    )));
    assert!(events(&fixture)
        .iter()
        .any(|event| matches!(event, HarnessEvent::EntryAdded { .. })));
    let last = events(&fixture).last().cloned().expect("events");
    match last {
        HarnessEvent::CompactionEnd {
            reason: crate::agent_core::harness::runtime::durable::SummaryReason::Manual,
            status: CompactionEndStatus::Completed,
            ..
        } => {}
        other => panic!("expected compaction_end completed, got {other:?}"),
    }
}

#[tokio::test]
async fn terminal_declines_standalone_compaction_without_publishing_an_entry() {
    let fixture = create_fixture("structural-terminal-decline").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: standalone_compaction_task(),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: Some(true),
                                compaction: None,
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    let result = run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "compaction");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Declined
            );
            assert_eq!(outcome.tip_id.as_deref(), Some("tip"));
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert!(fixture.lane.state().operation.is_none());
    assert_eq!(fixture.lane.state().tip_id.as_deref(), Some("tip"));
    let last = events(&fixture).last().cloned().expect("events");
    assert!(matches!(
        last,
        HarnessEvent::CompactionEnd {
            status: CompactionEndStatus::Declined,
            ..
        }
    ));
}

#[tokio::test]
async fn terminal_fails_overflow_decline_while_preserving_lane_owned_input() {
    let fixture = create_fixture("structural-overflow-decline").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: run_compaction_task(
                crate::agent_core::harness::runtime::durable::SummaryReason::Overflow,
                CheckpointData {
                    continuation: Continuation::NeedAssistant {
                        overflow_recovery_used: true,
                    },
                    trigger_entry_id: "tip".to_string(),
                },
            ),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    let queued_id = queue_write(
        &fixture,
        "retained-queued",
        PendingEntry::Custom {
            custom_type: "retained".to_string(),
            payload: Some(serde_json::json!({"value": true})),
        },
        InboxItemKind::Write,
    )
    .await;
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeCompaction,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeCompaction(Some(
                            BeforeCompactionHookResult {
                                decline: Some(true),
                                compaction: None,
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    let result = run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "run");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Failed
            );
            assert_eq!(
                outcome
                    .error
                    .as_ref()
                    .expect("failed carries an error")
                    .code,
                "compaction_declined"
            );
            assert_eq!(
                outcome.error.as_ref().expect("error").message,
                "Overflow compaction was declined"
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert_eq!(
        fixture.lane.state().inbox,
        vec![InboxItem {
            entry_id: queued_id.clone(),
            kind: InboxItemKind::Write,
        }]
    );
    assert!(fixture
        .session
        .get_value(&pending_entry(&queued_id), background_context())
        .await
        .unwrap()
        .is_some());
    let last_two = &events(&fixture)[events(&fixture).len() - 2..];
    assert!(matches!(
        last_two[0],
        HarnessEvent::CompactionEnd {
            status: CompactionEndStatus::Declined,
            ..
        }
    ));
    assert!(matches!(
        last_two[1],
        HarnessEvent::RunEnd {
            status: RunEndStatus::Failed,
            ..
        }
    ));
}

#[tokio::test]
async fn gives_each_split_turn_provider_request_its_own_durable_intent_and_usage_row() {
    let fixture = create_fixture("structural-split-turn").await;
    let ready = summary_ready(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        standalone_compaction_task(),
        &fixture.configuration,
    );
    let mut preparation = test_compaction_preparation();
    preparation.messages_to_summarize = vec![user("old history")];
    preparation.turn_prefix_messages = vec![user("large turn")];
    preparation.is_split_turn = true;
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &preparation,
                )),
            )),
        },
    )
    .await;
    fixture.faux.set_responses(vec![
        FauxResponseStep::Message(Box::new(faux_assistant_message(
            "history summary",
            Default::default(),
        ))),
        FauxResponseStep::Message(Box::new(faux_assistant_message(
            "turn prefix summary",
            Default::default(),
        ))),
    ]);

    let result = run_structural_generation(&fixture.lane, &fixture.drive, &ready)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "compaction");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Completed
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    let attempts = commit_attempts(&fixture);
    let request_indices: Vec<i64> = attempts
        .iter()
        .flatten()
        .filter_map(|write| match write {
            Write::Value(ValueWrite::Set {
                namespace, value, ..
            }) if namespace == "pi.op.state" => value
                .get("request")
                .and_then(|request| request.get("index"))
                .and_then(|index| index.as_i64()),
            _ => None,
        })
        .collect();
    assert_eq!(request_indices, vec![0, 1]);
    assert_eq!(usage_write_count(&fixture), 2);
    let has_assistant_frame_list = attempts.iter().flatten().any(|write| match write {
        Write::List(crate::agent_core::harness::session::types::ListWrite::Append {
            namespace,
            ..
        })
        | Write::List(crate::agent_core::harness::session::types::ListWrite::Delete {
            namespace,
            ..
        }) => namespace == "pi.pending.assistant_frame",
        _ => false,
    });
    assert!(!has_assistant_frame_list);
    assert!(!events(&fixture).iter().any(|event| matches!(
        event,
        HarnessEvent::MessageStart { .. }
            | HarnessEvent::MessageUpdate { .. }
            | HarnessEvent::MessageEnd { .. }
    )));
    assert_eq!(
        events(&fixture)
            .iter()
            .filter(|event| matches!(event, HarnessEvent::Usage { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn preserves_the_overflow_recovery_bound_when_compaction_resumes_generation() {
    let fixture = create_fixture("structural-overflow-resume").await;
    let ready = summary_ready(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        run_compaction_task(
            crate::agent_core::harness::runtime::durable::SummaryReason::Overflow,
            CheckpointData {
                continuation: Continuation::NeedAssistant {
                    overflow_recovery_used: true,
                },
                trigger_entry_id: "tip".to_string(),
            },
        ),
        &fixture.configuration,
    );
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message("generated summary", Default::default()),
        ))]);

    assert!(matches!(
        run_structural_generation(&fixture.lane, &fixture.drive, &ready)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    let routed = current_state(&fixture);
    let OperationPhase::AssistantReady {
        generation_context, ..
    } = &routed.phase
    else {
        panic!("generated compaction did not route to generation");
    };
    assert_eq!(generation_context.trigger_entry_id, "tip");
    assert!(generation_context.overflow_recovery_used);
    assert_eq!(
        fixture.lane.state().tip_id.as_deref(),
        Some("summary-entry")
    );
    let entry = fixture
        .session
        .get_entry("summary-entry", background_context())
        .await
        .unwrap()
        .expect("summary entry");
    match &entry {
        crate::agent_core::harness::session::Entry::Compaction {
            summary, from_hook, ..
        } => {
            assert_eq!(summary, "generated summary");
            assert!(!from_hook);
        }
        other => panic!("expected compaction entry, got {other:?}"),
    }
}

#[tokio::test]
async fn settles_structural_usage_without_faulting_when_durable_cancellation_aborts_the_request() {
    let fixture = create_fixture("structural-cancel-usage").await;
    let ready = summary_ready(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        standalone_compaction_task(),
        &fixture.configuration,
    );
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    // The faux factory parks mid-request, then answers with the stop reason
    // the (admitted) request signal produced — mirroring the upstream
    // signal.aborted probe.
    let started = Arc::new(tokio::sync::Notify::new());
    let started_watcher = Arc::clone(&started);
    let release = Arc::new(tokio::sync::Notify::new());
    let release_handle = Arc::clone(&release);
    let factory: FauxResponseFactory = Arc::new(move |args: FauxFactoryArgs| {
        let started = Arc::clone(&started_watcher);
        let release = Arc::clone(&release_handle);
        Box::pin(async move {
            started.notify_one();
            release.notified().await;
            let aborted = args
                .options
                .as_ref()
                .and_then(|options| options.stream.signal.as_ref())
                .map(|signal| signal.is_cancelled())
                .unwrap_or(false);
            Ok(faux_assistant_message(
                "",
                FauxMessageOptions {
                    stop_reason: Some(if aborted {
                        StopReason::Aborted
                    } else {
                        StopReason::Stop
                    }),
                    error_message: aborted.then(|| "cancelled".to_string()),
                    ..Default::default()
                },
            ))
        })
    });
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Factory(factory)]);

    let lane = Arc::clone(&fixture.lane);
    let drive = Arc::clone(&fixture.drive);
    let ready_for_run = ready.clone();
    let running =
        tokio::spawn(async move { run_structural_generation(&lane, &drive, &ready_for_run).await });
    started.notified().await;
    let cancellation = CancellationToken::new();
    fixture.drive.begin_abort(cancellation.clone());
    cancel_operation(&fixture).await;
    cancellation.cancel();
    fixture.drive.signal_abort();
    release.notify_one();

    let result = running.await.unwrap().unwrap();
    assert!(matches!(result, ProcedureResult::Continue));
    let cancelled = current_state(&fixture);
    let OperationPhase::SummaryEffectPending {
        request, usage_ids, ..
    } = &cancelled.phase
    else {
        panic!("cancelled generation did not remain effect-pending for reconciliation");
    };
    assert!(matches!(
        cancelled.scope.control,
        Control::CancelRequested { .. }
    ));
    assert!(request.is_none());
    assert_eq!(usage_ids.len(), 1);
    assert_eq!(usage_write_count(&fixture), 1);
}

#[tokio::test]
async fn fails_missing_in_run_structural_models_with_configuration_provenance() {
    let fixture = create_fixture("structural-model-missing").await;
    let mut configuration = fixture.configuration.clone();
    configuration.model = LaneModel {
        provider: "missing".to_string(),
        model_id: "missing".to_string(),
    };
    let ready = summary_ready(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        run_compaction_task(
            crate::agent_core::harness::runtime::durable::SummaryReason::Threshold,
            CheckpointData {
                continuation: Continuation::NeedAssistant {
                    overflow_recovery_used: false,
                },
                trigger_entry_id: "tip".to_string(),
            },
        ),
        &configuration,
    );
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Run {
            prompt_entry_ids: vec!["tip".to_string()],
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;

    let result = run_structural_generation(&fixture.lane, &fixture.drive, &ready)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "run");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Failed
            );
            let error = outcome.error.expect("failed carries an error");
            assert_eq!(error.code, "model_unavailable");
            assert_eq!(
                error.details,
                Some(serde_json::json!({"provider": "missing", "modelId": "missing"}))
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert!(fixture.lane.state().operation.is_none());
    assert_eq!(usage_write_count(&fixture), 0);

    let standalone = create_fixture("structural-model-missing-standalone").await;
    let mut standalone_configuration = standalone.configuration.clone();
    standalone_configuration.model = LaneModel {
        provider: "missing".to_string(),
        model_id: "missing".to_string(),
    };
    let standalone_ready = summary_ready(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        standalone_compaction_task(),
        &standalone_configuration,
    );
    install_operation(
        &standalone,
        standalone_ready.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("standalone-tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    let result = run_structural_generation(&standalone.lane, &standalone.drive, &standalone_ready)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "compaction");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Failed
            );
            assert_eq!(outcome.error.expect("error").code, "model_unavailable");
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert!(standalone.lane.state().operation.is_none());
}

#[tokio::test]
async fn durably_brackets_a_delayed_structural_retry_through_success() {
    let fixture = create_fixture("structural-retry-bracket").await;
    let ready = summary_ready_with_policy(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        standalone_compaction_task(),
        &fixture.configuration,
        crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy {
            max_attempts: 2,
            base_delay_ms: 10,
            max_agent_delay_ms: 30_000,
        },
    );
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message(
                "",
                crate::ai::models::faux::FauxMessageOptions {
                    stop_reason: Some(StopReason::Error),
                    error_message: Some("rate limit exceeded".to_string()),
                    ..Default::default()
                },
            ),
        ))]);

    assert!(matches!(
        run_structural_generation(&fixture.lane, &fixture.drive, &ready)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    let retry = current_state(&fixture);
    let OperationPhase::SummaryRetryWait { retry: wait, .. } = &retry.phase else {
        panic!("retryable failure did not wait");
    };
    assert_eq!(wait.next_attempt, 2);
    // First wait: not yet due and no drive wait-for-retry → Waiting outcome.
    let result = run_structural_retry_wait(&fixture.lane, &fixture.drive, &retry)
        .await
        .unwrap();
    match result {
        ProcedureResult::Waiting {
            outcome:
                DriveOutcome::Waiting {
                    operation_id,
                    reason: WaitingReason::Retry { not_before },
                },
        } => {
            assert_eq!(operation_id, OPERATION_ID);
            assert_eq!(not_before, wait.not_before);
        }
        other => panic!("expected waiting, got {other:?}"),
    }
    // Upstream advances the fake clock to retry.notBefore; the port rewrites
    // the durable notBefore into the past (clock substitution).
    let retry = rewrite_not_before(&fixture, &retry, crate::ai::now_ms() - 1).await;
    assert!(matches!(
        run_structural_retry_wait(&fixture.lane, &fixture.drive, &retry)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    let second = current_state(&fixture);
    assert!(matches!(
        &second.phase,
        OperationPhase::SummaryReady {
            next_attempt: 2,
            ..
        }
    ));
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message("summary", Default::default()),
        ))]);
    let result = run_structural_generation(&fixture.lane, &fixture.drive, &second)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Completed
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    let retry_events: Vec<(&'static str, Option<bool>)> = events(&fixture)
        .iter()
        .filter_map(|event| match event {
            HarnessEvent::RetryScheduled { .. } => Some(("retry_scheduled", None)),
            HarnessEvent::RetryStart { .. } => Some(("retry_start", None)),
            HarnessEvent::RetryEnd { success, .. } => Some(("retry_end", Some(*success))),
            _ => None,
        })
        .collect();
    assert_eq!(
        retry_events,
        vec![
            ("retry_scheduled", None),
            ("retry_start", None),
            ("retry_end", Some(true)),
        ]
    );
}

async fn rewrite_not_before(
    fixture: &Fixture,
    retry: &OperationState,
    not_before: i64,
) -> OperationState {
    let mut updated = retry.clone();
    if let OperationPhase::SummaryRetryWait { retry: wait, .. } = &mut updated.phase {
        wait.not_before = not_before;
    }
    let updated_for_plan = updated.clone();
    fixture
        .lane
        .settle_operation(
            move |_, _, _, _| {
                let updated = updated_for_plan.clone();
                Box::pin(async move {
                    Ok(OperationCommand::Commit {
                        writes: Vec::new(),
                        operation_state: updated,
                        lane: None,
                        materialize: Box::new(|_: &session_mod::CommitResult| ()),
                        events: None,
                    })
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    updated
}

#[tokio::test]
async fn closes_an_exhausted_structural_retry_with_its_final_error() {
    let fixture = create_fixture("structural-retry-exhausted").await;
    let ready = summary_ready_with_policy(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        standalone_compaction_task(),
        &fixture.configuration,
        crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy {
            max_attempts: 2,
            base_delay_ms: 10,
            max_agent_delay_ms: 30_000,
        },
    );
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message(
                "",
                crate::ai::models::faux::FauxMessageOptions {
                    stop_reason: Some(StopReason::Error),
                    error_message: Some("rate limit exceeded".to_string()),
                    ..Default::default()
                },
            ),
        ))]);
    run_structural_generation(&fixture.lane, &fixture.drive, &ready)
        .await
        .unwrap();
    let retry = current_state(&fixture);
    assert!(matches!(
        &retry.phase,
        OperationPhase::SummaryRetryWait { .. }
    ));
    let retry = rewrite_not_before(&fixture, &retry, crate::ai::now_ms() - 1).await;
    run_structural_retry_wait(&fixture.lane, &fixture.drive, &retry)
        .await
        .unwrap();
    let second = current_state(&fixture);
    assert!(matches!(
        &second.phase,
        OperationPhase::SummaryReady {
            next_attempt: 2,
            ..
        }
    ));
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message(
                "",
                crate::ai::models::faux::FauxMessageOptions {
                    stop_reason: Some(StopReason::Error),
                    error_message: Some("rate limit exceeded".to_string()),
                    ..Default::default()
                },
            ),
        ))]);

    let result = run_structural_generation(&fixture.lane, &fixture.drive, &second)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Failed
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    let retry_ends: Vec<(u32, bool)> = events(&fixture)
        .iter()
        .filter_map(|event| match event {
            HarnessEvent::RetryEnd {
                attempt, success, ..
            } => Some((*attempt, *success)),
            _ => None,
        })
        .collect();
    assert_eq!(retry_ends, vec![(2, false)]);
    let final_error = events(&fixture)
        .iter()
        .find_map(|event| match event {
            HarnessEvent::RetryEnd { final_error, .. } => final_error.clone(),
            _ => None,
        })
        .expect("final error");
    assert!(final_error.contains("rate limit exceeded"));
}

#[tokio::test]
async fn finishes_a_standalone_structural_failure_at_the_retry_cap() {
    let fixture = create_fixture("structural-retry-cap").await;
    let ready = summary_ready_with_policy(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        standalone_compaction_task(),
        &fixture.configuration,
        crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy {
            max_attempts: 1,
            base_delay_ms: 10,
            max_agent_delay_ms: 30_000,
        },
    );
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message(
                "",
                crate::ai::models::faux::FauxMessageOptions {
                    stop_reason: Some(StopReason::Error),
                    error_message: Some("rate limit exceeded".to_string()),
                    ..Default::default()
                },
            ),
        ))]);

    let result = run_structural_generation(&fixture.lane, &fixture.drive, &ready)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "compaction");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Failed
            );
            let error = outcome.error.expect("error");
            assert_eq!(error.code, "summarization_failed");
            assert_eq!(error.message, "Summarization failed: rate limit exceeded");
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert!(fixture.lane.state().operation.is_none());
    assert!(!events(&fixture)
        .iter()
        .any(|event| matches!(event, HarnessEvent::RetryScheduled { .. })));
}

#[tokio::test]
async fn rejects_invalid_unsummarized_navigation_state_without_committing() {
    for invalid in ["missing", "source", "root_label"] {
        let fixture = create_fixture("structural-navigation-invalid").await;
        let (target_id, label) = match invalid {
            "missing" => (Some("missing".to_string()), None),
            "source" => (Some("source".to_string()), None),
            _ => (None, Some("invalid".to_string())),
        };
        let navigation = OperationState {
            scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
            phase: OperationPhase::NavigationReadyToCommit {
                target_id: target_id.clone(),
                label: label.clone(),
            },
        };
        install_operation(
            &fixture,
            navigation.clone(),
            OperationIntent::Navigation {
                target_id: target_id.clone(),
                summarize: false,
                label: label.clone(),
                custom_instructions: None,
            },
            InstallOptions {
                entries: vec![message_entry("source", None, "source")],
                tip_id: Some("source".to_string()),
                preparation: None,
            },
        )
        .await;

        let error = commit_navigation(&fixture.lane, &fixture.drive, &navigation)
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert!(commit_attempts(&fixture).is_empty());
        assert_eq!(fixture.lane.state().tip_id.as_deref(), Some("source"));
    }
}

#[tokio::test]
async fn moves_an_unsummarized_navigation_and_cleans_up_in_one_terminal_transaction() {
    let fixture = create_fixture("structural-navigation-move").await;
    let navigation = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::NavigationReadyToCommit {
            target_id: Some("target".to_string()),
            label: Some("chosen".to_string()),
        },
    };
    install_operation(
        &fixture,
        navigation.clone(),
        OperationIntent::Navigation {
            target_id: Some("target".to_string()),
            summarize: false,
            label: Some("chosen".to_string()),
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![
                message_entry("root", None, "root"),
                message_entry("source", Some("root"), "source"),
                message_entry("target", Some("root"), "target"),
            ],
            tip_id: Some("source".to_string()),
            preparation: None,
        },
    )
    .await;
    let result = commit_navigation(&fixture.lane, &fixture.drive, &navigation)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.operation_id, OPERATION_ID);
            assert_eq!(outcome.kind, "navigation");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Completed
            );
            assert_eq!(outcome.from_tip_id.as_deref(), Some("source"));
            assert_eq!(outcome.tip_id.as_deref(), Some("target"));
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert_eq!(fixture.lane.state().tip_id.as_deref(), Some("target"));
    let label = fixture
        .session
        .get_label("target", background_context())
        .await
        .unwrap();
    assert_eq!(label.as_deref(), Some("chosen"));
    let writes = commit_attempts(&fixture).last().cloned().expect("writes");
    assert!(writes.iter().any(|write| match write {
        Write::Value(ValueWrite::Delete { namespace, .. }) => namespace == "pi.op.state",
        _ => false,
    }));
    assert!(writes.iter().any(|write| match write {
        Write::Value(ValueWrite::Set { namespace, .. }) => namespace == "pi.result",
        _ => false,
    }));
    assert!(writes.iter().any(|write| match write {
        Write::Value(ValueWrite::Set { namespace, .. }) => namespace == "pi.lane.state",
        _ => false,
    }));
}

#[tokio::test]
async fn publishes_a_hook_navigation_summary_with_the_target_parent_and_source_identity() {
    let fixture = create_fixture("structural-hook-navigation").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: navigation_summary_task("target"),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Navigation {
            target_id: Some("target".to_string()),
            summarize: true,
            label: None,
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![
                message_entry("root", None, "root"),
                message_entry("source", Some("root"), "source"),
                message_entry("target", Some("root"), "target"),
            ],
            tip_id: Some("source".to_string()),
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::BranchSummary(test_branch_preparation()),
            )),
        },
    )
    .await;
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeNavigation,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeNavigation(Some(
                            BeforeNavigationHookResult {
                                decline: None,
                                summary: Some(BranchSummaryResult {
                                    summary: "branch summary".to_string(),
                                    usage: None,
                                    read_files: vec!["read.ts".to_string()],
                                    modified_files: vec!["edit.ts".to_string()],
                                }),
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    let result = run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "navigation");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Completed
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    let tip = fixture.lane.state().tip_id.clone().expect("tip");
    let entry = fixture
        .session
        .get_entry(&tip, background_context())
        .await
        .unwrap()
        .expect("branch summary entry");
    match &entry {
        crate::agent_core::harness::session::Entry::BranchSummary {
            parent_id,
            from_id,
            summary,
            from_hook,
            details,
            ..
        } => {
            assert_eq!(parent_id.as_deref(), Some("target"));
            assert_eq!(from_id.as_deref(), Some("source"));
            assert_eq!(summary, "branch summary");
            assert!(*from_hook);
            assert_eq!(
                details.clone().expect("details"),
                serde_json::json!({"readFiles": ["read.ts"], "modifiedFiles": ["edit.ts"]})
            );
        }
        other => panic!("expected branch summary entry, got {other:?}"),
    }
}

#[tokio::test]
async fn terminal_declines_summarized_navigation_without_moving_the_tip() {
    let fixture = create_fixture("structural-navigation-decline").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: navigation_summary_task("target"),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Navigation {
            target_id: Some("target".to_string()),
            summarize: true,
            label: None,
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![
                message_entry("root", None, "root"),
                message_entry("source", Some("root"), "source"),
                message_entry("target", Some("root"), "target"),
            ],
            tip_id: Some("source".to_string()),
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::BranchSummary(test_branch_preparation()),
            )),
        },
    )
    .await;
    let hooks = fixture.lane.hooks();
    hooks
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeNavigation,
            Arc::new(|_invocation, _context| {
                Box::pin(async {
                    Ok(
                        crate::agent_core::harness::hooks::HookResult::BeforeNavigation(Some(
                            BeforeNavigationHookResult {
                                decline: Some(true),
                                summary: None,
                            },
                        )),
                    )
                })
            }),
            None,
        )
        .unwrap();

    let result = run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.kind, "navigation");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Declined
            );
            assert_eq!(outcome.tip_id.as_deref(), Some("source"));
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert_eq!(fixture.lane.state().tip_id.as_deref(), Some("source"));
    let last = events(&fixture).last().cloned().expect("events");
    // Upstream status is "declined"; the fixed RunEndStatus union has no
    // Declined so the port emits "aborted" (module disclosure).
    assert!(matches!(last, HarnessEvent::NavigationEnd { .. }));
}

#[tokio::test]
async fn generates_and_atomically_publishes_a_navigation_summary() {
    let fixture = create_fixture("structural-generated-navigation").await;
    let ready = summary_ready(
        run_scope(DEFAULT_COMPACTION_SETTINGS),
        navigation_summary_task("target"),
        &fixture.configuration,
    );
    install_operation(
        &fixture,
        ready.clone(),
        OperationIntent::Navigation {
            target_id: Some("target".to_string()),
            summarize: true,
            label: None,
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![
                message_entry("root", None, "root"),
                message_entry("source", Some("root"), "source"),
                message_entry("target", Some("root"), "target"),
            ],
            tip_id: Some("source".to_string()),
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::BranchSummary(test_branch_preparation()),
            )),
        },
    )
    .await;
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(
            faux_assistant_message("generated branch summary", Default::default()),
        ))]);

    let result = run_structural_generation(&fixture.lane, &fixture.drive, &ready)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(outcome.operation_id, OPERATION_ID);
            assert_eq!(outcome.kind, "navigation");
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Completed
            );
            assert_eq!(outcome.from_tip_id.as_deref(), Some("source"));
            assert_eq!(outcome.tip_id.as_deref(), Some("summary-entry"));
        }
        other => panic!("expected settled, got {other:?}"),
    }
    let entry = fixture
        .session
        .get_entry("summary-entry", background_context())
        .await
        .unwrap()
        .expect("branch summary entry");
    match &entry {
        crate::agent_core::harness::session::Entry::BranchSummary {
            parent_id,
            from_id,
            from_hook,
            ..
        } => {
            assert_eq!(parent_id.as_deref(), Some("target"));
            assert_eq!(from_id.as_deref(), Some("source"));
            assert!(!*from_hook);
        }
        other => panic!("expected branch summary entry, got {other:?}"),
    }
    assert_eq!(
        events(&fixture)
            .iter()
            .filter(|event| matches!(event, HarnessEvent::Usage { .. }))
            .count(),
        1
    );
}

fn orphan_effect(fixture: &Fixture, max_attempts: u32, attempt: u32) -> OperationState {
    OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryEffectPending {
            task: standalone_compaction_task(),
            summary_context: DurableSummaryContext {
                result_entry_id: "summary-entry".to_string(),
                configuration: fixture.configuration.clone(),
                stream_options: Default::default(),
                retry_policy: crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy {
                    max_attempts,
                    base_delay_ms: 10,
                    max_agent_delay_ms: 30_000,
                },
            },
            attempt,
            request: Some(
                crate::agent_core::harness::runtime::durable::SummaryRequest {
                    index: 1,
                    usage_id: "abandoned-usage".to_string(),
                },
            ),
            usage_ids: vec!["settled-usage".to_string()],
        },
    }
}

#[tokio::test]
async fn consumes_an_orphaned_structural_attempt_and_never_resumes_its_nested_request() {
    let fixture = create_fixture("structural-orphan-recovery").await;
    let effect = orphan_effect(&fixture, 2, 1);
    install_operation(
        &fixture,
        effect.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;

    assert!(matches!(
        recover_structural_generation(&fixture.lane, &fixture.drive, &effect)
            .await
            .unwrap(),
        ProcedureResult::Continue
    ));
    let retry = current_state(&fixture);
    let OperationPhase::SummaryRetryWait { retry: wait, .. } = &retry.phase else {
        panic!("orphan did not enter retry wait");
    };
    assert_eq!(wait.next_attempt, 2);
    // The durable request marker is gone with the effect-pending phase.
    assert!(!matches!(
        retry.phase,
        OperationPhase::SummaryEffectPending { .. }
    ));
    assert_eq!(usage_write_count(&fixture), 0);
    let last = events(&fixture).last().cloned().expect("events");
    match last {
        // Upstream also carries recovery: true (dropped by the fixed event
        // union; module disclosure).
        HarnessEvent::RetryScheduled { attempt: 2, .. } => {}
        other => panic!("expected retry_scheduled attempt 2, got {other:?}"),
    }
}

#[tokio::test]
async fn terminal_fails_an_orphaned_structural_attempt_at_the_retry_cap() {
    let fixture = create_fixture("structural-orphan-cap").await;
    let effect = orphan_effect(&fixture, 1, 1);
    install_operation(
        &fixture,
        effect.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::Compaction(durable_compaction_preparation_of(
                    &test_compaction_preparation(),
                )),
            )),
        },
    )
    .await;

    let result = recover_structural_generation(&fixture.lane, &fixture.drive, &effect)
        .await
        .unwrap();
    match result {
        ProcedureResult::Settled { outcome } => {
            assert_eq!(
                outcome.status,
                crate::agent_core::harness::session::TerminalStatus::Failed
            );
            let error = outcome.error.expect("error");
            assert_eq!(error.code, "structural_interrupted");
            assert_eq!(
                error.message,
                "Structural summary attempt was interrupted and its external outcome is unknown"
            );
        }
        other => panic!("expected settled, got {other:?}"),
    }
    assert!(fixture.lane.state().operation.is_none());
    assert!(!events(&fixture)
        .iter()
        .any(|event| matches!(event, HarnessEvent::RetryScheduled { .. })));
}

#[tokio::test]
async fn rejects_a_preparation_whose_durable_kind_contradicts_the_structural_state() {
    let fixture = create_fixture("structural-kind-contradiction").await;
    let deciding = OperationState {
        scope: run_scope(DEFAULT_COMPACTION_SETTINGS),
        phase: OperationPhase::SummaryDeciding {
            task: standalone_compaction_task(),
        },
    };
    install_operation(
        &fixture,
        deciding.clone(),
        OperationIntent::Compaction {
            custom_instructions: None,
        },
        InstallOptions {
            entries: vec![message_entry("tip", None, "history")],
            tip_id: None,
            preparation: Some((
                "task".to_string(),
                DurableStructuralPreparation::BranchSummary(test_branch_preparation()),
            )),
        },
    )
    .await;

    let error = run_structural_decision(&fixture.lane, &fixture.drive, &deciding)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Structural task task is missing its compaction preparation"),
        "unexpected error: {error}"
    );
}

// ---------------------------------------------------------------------------
// Node-oracle comparisons (scratch structural_oracle.mjs)
// ---------------------------------------------------------------------------

fn zero_cost() -> serde_json::Value {
    serde_json::json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0})
}

fn oracle_usage(input: u64, output: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output,
        cost: Default::default(),
    }
}

#[test]
fn oracle_durable_structural_preparations() {
    // Upstream scratch: durableCompactionPreparation / durableBranchPreparation.
    // Disclosed substitution: file-op arrays are sorted (HashSet order).
    let mut read = HashSet::new();
    read.insert("a.ts".to_string());
    read.insert("b.ts".to_string());
    let preparation = CompactionPreparation {
        messages_to_summarize: vec![user("history")],
        turn_prefix_messages: vec![],
        retained_tail: vec![user_at("tail", 2)],
        is_split_turn: false,
        tokens_before: 1000,
        previous_summary: None,
        file_ops: FileOperations {
            read,
            written: HashSet::new(),
            edited: HashSet::new(),
        },
        settings: CompactionSettings {
            enabled: true,
            reserve_tokens: 1000,
            keep_recent_tokens: 10,
        },
    };
    assert_eq!(
        serde_json::to_value(durable_compaction_preparation(&preparation)).unwrap(),
        serde_json::json!({
            "kind": "compaction",
            "messagesToSummarize": [{"role": "user", "content": "history", "timestamp": 1}],
            "turnPrefixMessages": [],
            "retainedTail": [{"role": "user", "content": "tail", "timestamp": 2}],
            "isSplitTurn": false,
            "tokensBefore": 1000,
            "fileOps": {"read": ["a.ts", "b.ts"], "written": [], "edited": []},
            "settings": {"enabled": true, "reserveTokens": 1000, "keepRecentTokens": 10},
        })
    );

    let with_previous = CompactionPreparation {
        messages_to_summarize: vec![],
        turn_prefix_messages: vec![],
        retained_tail: vec![],
        is_split_turn: true,
        tokens_before: 7,
        previous_summary: Some("earlier".to_string()),
        file_ops: FileOperations::new(),
        settings: CompactionSettings {
            enabled: true,
            reserve_tokens: 1000,
            keep_recent_tokens: 10,
        },
    };
    assert_eq!(
        serde_json::to_value(durable_compaction_preparation(&with_previous)).unwrap(),
        serde_json::json!({
            "kind": "compaction",
            "messagesToSummarize": [],
            "turnPrefixMessages": [],
            "retainedTail": [],
            "isSplitTurn": true,
            "tokensBefore": 7,
            "previousSummary": "earlier",
            "fileOps": {"read": [], "written": [], "edited": []},
            "settings": {"enabled": true, "reserveTokens": 1000, "keepRecentTokens": 10},
        })
    );

    let branch = BranchPreparation {
        messages: vec![user("abandoned")],
        file_ops: FileOperations {
            read: HashSet::new(),
            written: HashSet::from(["w.ts".to_string()]),
            edited: HashSet::from(["e.ts".to_string()]),
        },
        total_tokens: 10,
    };
    assert_eq!(
        serde_json::to_value(durable_branch_preparation(&branch)).unwrap(),
        serde_json::json!({
            "kind": "branch_summary",
            "messages": [{"role": "user", "content": "abandoned", "timestamp": 1}],
            "fileOps": {"read": [], "written": ["w.ts"], "edited": ["e.ts"]},
            "totalTokens": 10,
        })
    );
}

#[test]
fn oracle_summary_phase_transitions() {
    // Upstream scratch: effectPendingFromReady / retryWaitFromEffect /
    // readyFromRetryWait over the fixture scope/task/summaryContext.
    let configuration = LaneConfiguration {
        model: LaneModel {
            provider: "faux".to_string(),
            model_id: "faux-1".to_string(),
        },
        thinking_level: ThinkingLevel::Off,
        active_tool_names: Vec::new(),
    };
    let scope = run_scope(CompactionSettings {
        enabled: true,
        reserve_tokens: 1000,
        keep_recent_tokens: 10,
    });
    let task = standalone_compaction_task();
    let summary_context = DurableSummaryContext {
        result_entry_id: "summary-entry".to_string(),
        configuration: configuration.clone(),
        stream_options: Default::default(),
        retry_policy: crate::agent_core::harness::runtime::durable::NormalizedRetryPolicy {
            max_attempts: 2,
            base_delay_ms: 10,
            max_agent_delay_ms: 30_000,
        },
    };
    let ready = OperationState {
        scope: scope.clone(),
        phase: OperationPhase::SummaryReady {
            task: task.clone(),
            summary_context: summary_context.clone(),
            next_attempt: 1,
        },
    };
    let effect = effect_pending_from_ready(&ready).unwrap();
    assert_eq!(
        serde_json::to_value(&effect).unwrap(),
        serde_json::json!({
            "control": {"status": "running"},
            "settings": {
                "compaction": {"enabled": true, "reserveTokens": 1000, "keepRecentTokens": 10},
                "steeringMode": "all", "followUpMode": "all", "toolExecution": "parallel",
            },
            "latestAssistantEntryId": null,
            "at": "summary.effect_pending",
            "task": {"taskId": "task", "reason": "manual", "boundary": {"kind": "finish"}},
            "summaryContext": {
                "resultEntryId": "summary-entry",
                "configuration": {
                    "model": {"provider": "faux", "modelId": "faux-1"},
                    "thinkingLevel": "off", "activeToolNames": [],
                },
                "streamOptions": {},
                "retryPolicy": {"maxAttempts": 2, "baseDelayMs": 10, "maxAgentDelayMs": 30000},
            },
            "attempt": 1,
            "usageIds": [],
        })
    );

    // notBefore stubbed to the literal 4242 in the scratch oracle.
    let mut retry_state = retry_wait_from_effect(
        &effect,
        "Summarization failed: rate limit exceeded".to_string(),
    )
    .unwrap();
    if let OperationPhase::SummaryRetryWait { retry: wait, .. } = &mut retry_state.phase {
        wait.not_before = 4242;
    }
    assert_eq!(
        serde_json::to_value(&retry_state).unwrap(),
        serde_json::json!({
            "control": {"status": "running"},
            "settings": {
                "compaction": {"enabled": true, "reserveTokens": 1000, "keepRecentTokens": 10},
                "steeringMode": "all", "followUpMode": "all", "toolExecution": "parallel",
            },
            "latestAssistantEntryId": null,
            "at": "summary.retry_wait",
            "task": {"taskId": "task", "reason": "manual", "boundary": {"kind": "finish"}},
            "summaryContext": {
                "resultEntryId": "summary-entry",
                "configuration": {
                    "model": {"provider": "faux", "modelId": "faux-1"},
                    "thinkingLevel": "off", "activeToolNames": [],
                },
                "streamOptions": {},
                "retryPolicy": {"maxAttempts": 2, "baseDelayMs": 10, "maxAgentDelayMs": 30000},
            },
            "nextAttempt": 2,
            "notBefore": 4242,
            "errorMessage": "Summarization failed: rate limit exceeded",
        })
    );

    let retry_input = OperationState {
        scope,
        phase: OperationPhase::SummaryRetryWait {
            task,
            summary_context,
            retry: crate::agent_core::harness::runtime::durable::RetryWait {
                next_attempt: 2,
                not_before: 4242,
                error_message: "boom".to_string(),
            },
        },
    };
    assert_eq!(
        serde_json::to_value(ready_from_retry_wait(&retry_input).unwrap()).unwrap(),
        serde_json::json!({
            "control": {"status": "running"},
            "settings": {
                "compaction": {"enabled": true, "reserveTokens": 1000, "keepRecentTokens": 10},
                "steeringMode": "all", "followUpMode": "all", "toolExecution": "parallel",
            },
            "latestAssistantEntryId": null,
            "at": "summary.ready",
            "task": {"taskId": "task", "reason": "manual", "boundary": {"kind": "finish"}},
            "summaryContext": {
                "resultEntryId": "summary-entry",
                "configuration": {
                    "model": {"provider": "faux", "modelId": "faux-1"},
                    "thinkingLevel": "off", "activeToolNames": [],
                },
                "streamOptions": {},
                "retryPolicy": {"maxAttempts": 2, "baseDelayMs": 10, "maxAgentDelayMs": 30000},
            },
            "nextAttempt": 2,
        })
    );
}

#[test]
fn oracle_event_shapes() {
    let commit = CommitResult {
        first_seq: 11,
        seqs: vec![11, 12, 13],
        timestamp: 100,
        stats: crate::agent_core::harness::session::SessionStats {
            message_count: 0,
            usage: oracle_usage(5, 7),
        },
    };
    let row = NewUsageRow {
        id: "u1".to_string(),
        usage: oracle_usage(5, 7),
        entry_id: None,
        adjustment: false,
        details: None,
    };
    // Upstream Usage serializes cost only when present; the ported Usage
    // always carries the cost object, so the oracle grows the zero cost.
    let usage_with_cost = |usage: Usage| {
        let mut value = serde_json::to_value(usage).unwrap();
        value["cost"] = zero_cost();
        value
    };
    let totals = usage_with_cost(oracle_usage(5, 7));
    let mut row_value = serde_json::to_value(&row).unwrap();
    row_value["seq"] = serde_json::Value::Number(12.into());
    row_value["usage"] = usage_with_cost(oracle_usage(5, 7));
    assert_eq!(
        serde_json::to_value(usage_event(&row, 1, &commit, "main").unwrap()).unwrap(),
        serde_json::json!({
            "type": "usage",
            "lane": "main",
            "row": row_value,
            "totals": totals,
        })
    );

    // retry_end (success / failed).
    assert_eq!(
        serde_json::to_value(HarnessEvent::RetryEnd {
            lane: "main".into(),
            run_id: "op1".into(),
            step: "task".into(),
            attempt: 2,
            success: true,
            final_error: None,
        })
        .unwrap(),
        serde_json::json!({
            "type": "retry_end", "lane": "main", "runId": "op1", "step": "task",
            "attempt": 2, "success": true,
        })
    );
    assert_eq!(
        serde_json::to_value(HarnessEvent::RetryEnd {
            lane: "main".into(),
            run_id: "op1".into(),
            step: "task".into(),
            attempt: 2,
            success: false,
            final_error: Some("Summarization failed: rate limit exceeded".into()),
        })
        .unwrap(),
        serde_json::json!({
            "type": "retry_end", "lane": "main", "runId": "op1", "step": "task",
            "attempt": 2, "success": false,
            "finalError": "Summarization failed: rate limit exceeded",
        })
    );

    // compaction_end completed / declined / failed.
    assert_eq!(
        serde_json::to_value(HarnessEvent::CompactionEnd {
            lane: "main".into(),
            run_id: "op1".into(),
            reason: crate::agent_core::harness::runtime::durable::SummaryReason::Manual,
            status: CompactionEndStatus::Completed,
            entry_id: Some("summary-entry".into()),
            ended_at: 4321,
        })
        .unwrap(),
        serde_json::json!({
            "type": "compaction_end", "lane": "main", "runId": "op1",
            "reason": "manual", "status": "completed",
            "entryId": "summary-entry", "endedAt": 4321,
        })
    );
    assert_eq!(
        serde_json::to_value(HarnessEvent::CompactionEnd {
            lane: "main".into(),
            run_id: "op1".into(),
            reason: crate::agent_core::harness::runtime::durable::SummaryReason::Threshold,
            status: CompactionEndStatus::Declined,
            entry_id: None,
            ended_at: commit.timestamp,
        })
        .unwrap(),
        serde_json::json!({
            "type": "compaction_end", "lane": "main", "runId": "op1",
            "reason": "threshold", "status": "declined", "endedAt": 100,
        })
    );
    // Upstream declined-with-error / failed events also carry `error`; the
    // fixed HarnessEvent union has no such field (module disclosure), so only
    // the representable suffix is compared.
    let declined_overflow = serde_json::to_value(HarnessEvent::CompactionEnd {
        lane: "main".into(),
        run_id: "op1".into(),
        reason: crate::agent_core::harness::runtime::durable::SummaryReason::Overflow,
        status: CompactionEndStatus::Declined,
        entry_id: None,
        ended_at: 4321,
    })
    .unwrap();
    assert_eq!(
        declined_overflow,
        serde_json::json!({
            "type": "compaction_end", "lane": "main", "runId": "op1",
            "reason": "overflow", "status": "declined", "endedAt": 4321,
        })
    );
    let failed_manual = serde_json::to_value(HarnessEvent::CompactionEnd {
        lane: "main".into(),
        run_id: "op1".into(),
        reason: crate::agent_core::harness::runtime::durable::SummaryReason::Manual,
        status: CompactionEndStatus::Failed,
        entry_id: None,
        ended_at: 4321,
    })
    .unwrap();
    assert_eq!(
        failed_manual,
        serde_json::json!({
            "type": "compaction_end", "lane": "main", "runId": "op1",
            "reason": "manual", "status": "failed", "endedAt": 4321,
        })
    );

    // run_end failed.
    assert_eq!(
        serde_json::to_value(HarnessEvent::RunEnd {
            lane: "main".into(),
            run_id: "op1".into(),
            status: RunEndStatus::Failed,
            error: Some(OperationError {
                code: "compaction_declined".into(),
                message: "Overflow compaction was declined".into(),
                details: None,
            }),
            from_tip_id: Some("tip".into()),
            tip_id: Some("tip".into()),
            ended_at: 4321,
        })
        .unwrap(),
        serde_json::json!({
            "type": "run_end", "lane": "main", "runId": "op1", "status": "failed",
            "error": {"code": "compaction_declined", "message": "Overflow compaction was declined"},
            "fromTipId": "tip", "tipId": "tip", "endedAt": 4321,
        })
    );

    // navigation_end completed (failed drops `error`; declined substitutes
    // "aborted" — both module disclosures).
    assert_eq!(
        serde_json::to_value(HarnessEvent::NavigationEnd {
            lane: "main".into(),
            run_id: "op1".into(),
            status: RunEndStatus::Completed,
            from_tip_id: Some("source".into()),
            tip_id: Some("summary-entry".into()),
            ended_at: 4321,
        })
        .unwrap(),
        serde_json::json!({
            "type": "navigation_end", "lane": "main", "runId": "op1",
            "status": "completed", "fromTipId": "source",
            "tipId": "summary-entry", "endedAt": 4321,
        })
    );

    // retry_scheduled (recovery flag dropped — module disclosure) and
    // retry_start.
    assert_eq!(
        serde_json::to_value(HarnessEvent::RetryScheduled {
            lane: "main".into(),
            run_id: "op1".into(),
            step: "task".into(),
            attempt: 2,
            max_attempts: 2,
            delay_ms: 10,
            not_before: 4242,
            error_message: "Summarization failed: rate limit exceeded".into(),
        })
        .unwrap(),
        serde_json::json!({
            "type": "retry_scheduled", "lane": "main", "runId": "op1", "step": "task",
            "attempt": 2, "maxAttempts": 2, "delayMs": 10, "notBefore": 4242,
            "errorMessage": "Summarization failed: rate limit exceeded",
        })
    );
    assert_eq!(
        serde_json::to_value(HarnessEvent::RetryStart {
            lane: "main".into(),
            run_id: "op1".into(),
            step: "task".into(),
            attempt: 2,
        })
        .unwrap(),
        serde_json::json!({
            "type": "retry_start", "lane": "main", "runId": "op1",
            "step": "task", "attempt": 2,
        })
    );
}

// ---------------------------------------------------------------------------
// First-slice prepare tests (kept from the original structural.rs tests)
// ---------------------------------------------------------------------------

async fn create_prepare_lane() -> std::sync::Arc<Lane> {
    let storage = MemoryStorage::new(MemoryStorageOptions::default());
    let sess = Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: "structural-prepare-test".into(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        Arc::new(storage),
    ));
    let writes: Vec<Write> = vec![
        session_mod::set_value(&session_mod::branch_tip("main"), serde_json::Value::Null),
        session_mod::set_value(
            &session_mod::lane_config("main"),
            session_mod::lane_configuration_value(&LaneConfiguration {
                model: LaneModel {
                    provider: "faux".to_string(),
                    model_id: "faux-1".to_string(),
                },
                thinking_level: ThinkingLevel::Off,
                active_tool_names: Vec::new(),
            }),
        ),
        session_mod::set_value(
            &session_mod::lane_state("main"),
            serde_json::json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
        ),
    ];
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
    models.set_provider(Arc::clone(&faux.provider));
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

fn checkpoint_capability(trigger: &str) -> OperationState {
    serde_json::from_value(serde_json::json!({
        "control": {"status": "running"},
        "settings": {"compaction": {"enabled": true, "reserveTokens": 16384,
            "keepRecentTokens": 20000},
            "steeringMode": "all", "followUpMode": "all", "toolExecution": "parallel"},
        "latestAssistantEntryId": null,
        "at": "checkpoint",
        "continuation": {"kind": "need_assistant", "overflowRecoveryUsed": false},
        "triggerEntryId": trigger
    }))
    .unwrap()
}

#[tokio::test]
async fn overflow_returns_none_when_recovery_already_used() {
    let lane = create_prepare_lane().await;
    let drive = Drive::new(
        &DriveOptions {
            operation_id: "op1".to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    );
    let generation: OperationState = serde_json::from_value(serde_json::json!({
        "control": {"status": "running"},
        "settings": {"compaction": {"enabled": true, "reserveTokens": 16384,
            "keepRecentTokens": 20000},
            "steeringMode": "all", "followUpMode": "all", "toolExecution": "parallel"},
        "latestAssistantEntryId": null,
        "at": "assistant.effect_pending",
        "generationContext": {
            "stepId": "s1", "triggerEntryId": "t1",
            "configuration": {"model": {"provider": "faux", "modelId": "faux-1"},
                "thinkingLevel": "off", "activeToolNames": []},
            "streamOptions": {},
            "retryPolicy": {"maxAttempts": 4, "baseDelayMs": 1000,
                "maxAgentDelayMs": 60000},
            "overflowRecoveryUsed": true
        },
        "attempt": 1,
        "responseEntryId": "r1",
        "usageId": "u1",
        "intendedOutputLimit": 1000,
        "contextWindow": 100000
    }))
    .unwrap();
    assert!(prepare_overflow_compaction(&lane, &drive, &generation)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn threshold_skips_when_compaction_disabled() {
    let lane = create_prepare_lane().await;
    let drive = Drive::new(
        &DriveOptions {
            operation_id: "op1".to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    );
    let mut capability = checkpoint_capability("whatever");
    capability.scope.settings.compaction.enabled = false;
    let result = prepare_compaction_threshold(&lane, &drive, &capability)
        .await
        .unwrap();
    assert!(matches!(
        result,
        ContinueOperationResult::Result { value: None }
    ));
}

#[tokio::test]
async fn threshold_guarded_by_trailing_compaction_and_missing_trigger_invariant() {
    let lane = create_prepare_lane().await;
    let seed = vec![insert_entry(NewEntry::Compaction {
        id: "c1".to_string(),
        parent_id: None,
        summary: "earlier".to_string(),
        retained_tail: Vec::new(),
        tokens_before: 1_000,
        details: None,
        usage: None,
        from_hook: false,
    })];
    lane.session()
        .mutate(
            move |reader, context| {
                Box::pin(async move { reader.commit(seed, context).await.map(|_| ()) })
            },
            background_context(),
        )
        .await
        .unwrap();
    let admission = lane
        .accept(
            &OperationRequest::Prompt {
                prompt: "hello".to_string(),
            },
            background_context(),
        )
        .await
        .unwrap()
        .unwrap();
    let drive = Drive::new(
        &DriveOptions {
            operation_id: admission.operation_id.clone(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    );
    let tip = lane
        .read_lane(
            |state, _reader| Box::pin(async move { Ok(state.tip_id.clone()) }),
            background_context(),
        )
        .await
        .unwrap()
        .unwrap_or_else(|| "missing".to_string());
    let result = prepare_compaction_threshold(&lane, &drive, &checkpoint_capability(&tip))
        .await
        .unwrap();
    assert!(matches!(
        result,
        ContinueOperationResult::Result { value: None }
    ));
    // The seeded compaction entry is off-branch (the seed never moved the
    // branch tip), so no newer compaction guards this checkpoint and the
    // missing-trigger invariant fires (upstream checks the guard first; the
    // guarded case is covered by uses_a_newer_compaction_entry_...).
    let error = prepare_compaction_threshold(&lane, &drive, &checkpoint_capability("fabricated"))
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("Checkpoint trigger fabricated is missing"));
}
