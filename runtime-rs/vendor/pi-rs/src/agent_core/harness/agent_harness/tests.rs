//! Behavior tests for the public AgentHarness shell.
//!
//! Ported from `pi/packages/agent/test/harness/runtime/harness.test.ts`
//! (SHA256 of the upstream file at capture time; the behavior authority for
//! the public shell) and from the dispatcher-level cases of
//! `drive-public.test.ts`, plus a serialization-seam oracle test whose
//! expected JSON lines were captured by running the upstream event literals
//! with node `--experimental-strip-types`
//! (`tests/fixtures/agent_harness_oracle/agent_harness_oracle.ts`).
//!
//! The lane-slice cases of both upstream files (the queue/append/drive-install
//! surface and the drive-public `lane.*` convenience compositions) are ported
//! in the "un-pended upstream cases" section at the bottom, one test per
//! upstream `it` (the `it.each` cases stay per-parameter), running against the
//! landed `runtime/lane.rs` surface.

use std::sync::{Arc, Mutex, PoisonError};

use super::drive_operation::{drive_operation, DriveEnv};
use super::harness_impl::{create_agent_harness, Harness};
use super::{
    AcquireLaneOptions, AgentHarness as _, AgentHarnessOptions, AgentLane, CancelQueuedOutcome,
    HarnessEvent, IdleCallback, OperationRequest, PromptPayload, PublicConfigProperty, QueueInput,
    RecordUsageOptions, Resources, RunOutcome, TaggedError, ValueUpdatePayload,
};
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::hooks::{HookName, HookResult};
use crate::agent_core::harness::result::HarnessFault;
use crate::agent_core::harness::runtime::drive_pass::{Drive, DriveOptions, DriveOutcome};
use crate::agent_core::harness::runtime::events::CompactionEndStatus;
use crate::agent_core::harness::runtime::lane::{
    Lane, LaneCommand, NavigationOptions as RuntimeNavigationOptions,
};
use crate::agent_core::harness::session::types::{
    Session as SessionTrait, SessionCreateOptions, Storage,
};
use crate::agent_core::harness::session::{
    branch_tip, insert_entry, lane_config, lane_state, pending_entry, set_value, BranchScan,
    BranchScanOrder, InboxItem, InboxItemKind, MemorySessionRepo, MemorySessionRepoOptions,
    MemoryStorage, MemoryStorageOptions, NewEntry, OperationResultRecord, SessionMetadata,
    SessionMutator, StorageBackedSession, TerminalStatus, Write,
};
use crate::agent_core::harness::session::{LaneConfiguration, LaneModel};
use crate::agent_core::harness::types::{AgentHarnessStreamOptions, PromptTemplate, Skill};
use crate::agent_core::types::{
    AgentMessage, QueueMode as QueueModeType, ThinkingLevel as ThinkingLevelType,
};
use crate::ai::api::ApiImpl as ApiImplTrait;
use crate::ai::auth::resolve::ModelsError;
use crate::ai::auth::types::ProviderAuth;
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderHandle,
    FauxProviderOptions, FauxResponseStep,
};
use crate::ai::models::{create_models, CreateModelsOptions, Provider};
use crate::ai::types::message::{StringOrBlocks, UserMessage};
use crate::ai::types::options::{DeferredFlag, DeferredHandle};
use crate::ai::types::primitives::{Usage, UsageCost};

type Log = Arc<Mutex<Vec<String>>>;

fn push(log: &Log, value: String) {
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(value);
}

fn drain(log: &Log) -> Vec<String> {
    log.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

async fn create_session(id: &str) -> Arc<StorageBackedSession> {
    Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: id.to_owned(),
            created_at: 1,
            storage_version: 1,
            ..SessionMetadata::default()
        },
        Arc::new(MemoryStorage::new(MemoryStorageOptions::default())) as Arc<dyn Storage>,
    ))
}

fn harness_options(
    session: Arc<StorageBackedSession>,
    models: crate::ai::models::Models,
    model: crate::ai::types::model::Model,
) -> AgentHarnessOptions<()> {
    AgentHarnessOptions {
        session,
        models,
        model,
        thinking_level: Some(ThinkingLevelType::Medium),
        active_tool_names: Some(vec!["read".to_owned(), "bash".to_owned()]),
        tools: None,
        tool_context: None,
        system_prompt: None,
        resources: None,
        stream_options: None,
        retry: None,
        compaction: None,
        steering_mode: None,
        follow_up_mode: None,
        tool_execution: None,
        to_provider_messages: None,
        entry_projectors: None,
    }
}

/// Upstream drive-public `createFixture` options: no explicit
/// `activeToolNames`, so the seed derives them from the (empty) tools list.
fn dispatcher_options(
    session: Arc<StorageBackedSession>,
    models: crate::ai::models::Models,
    model: crate::ai::types::model::Model,
) -> AgentHarnessOptions<()> {
    let mut options = harness_options(session, models, model);
    options.active_tool_names = None;
    options
}

/// Upstream `createHarness` helper: faux provider + models + fresh session.
async fn create_harness(session_id: &str) -> Harness<()> {
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux provider exposes faux-1");
    let session = create_session(session_id).await;
    let (harness, open) = create_agent_harness(
        harness_options(session, models, model),
        background_context(),
    )
    .await
    .expect("harness attaches");
    assert!(open.is_empty());
    harness
}

async fn commit_writes(
    session: &StorageBackedSession,
    writes: Vec<Write>,
    context: crate::agent_core::harness::context::Context,
) -> anyhow::Result<()> {
    SessionTrait::mutate(
        session,
        move |mutator: &dyn SessionMutator, context| {
            Box::pin(async move {
                SessionMutator::commit(mutator, writes, context)
                    .await
                    .map(|_| ())
            })
        },
        context,
    )
    .await
    .map(|_| ())
}

const CONFIGURED: fn() -> LaneConfiguration = || LaneConfiguration {
    model: LaneModel {
        provider: "faux".to_owned(),
        model_id: "faux-1".to_owned(),
    },
    thinking_level: ThinkingLevelType::Low,
    active_tool_names: vec!["read".to_owned()],
};

// ---------------------------------------------------------------------------
// serialization-seam oracle (node-captured upstream event literals)
// ---------------------------------------------------------------------------

fn oracle_lines() -> Vec<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/agent_harness_oracle/oracle_output.txt"
    );
    let oracle = std::fs::read_to_string(path).expect("oracle output file");
    oracle.lines().map(str::to_owned).collect()
}

fn wire(event: &HarnessEvent) -> String {
    serde_json::to_string(event).expect("event serializes")
}

fn sample_usage() -> Usage {
    Usage {
        input: 1,
        output: 2,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 3,
        cost: UsageCost::default(),
        ..Usage::default()
    }
}

fn tool_result(text: &str) -> crate::agent_core::types::AgentToolResult {
    crate::agent_core::types::AgentToolResult {
        content: vec![crate::ai::types::message::TextOrImageBlock::Text(
            crate::ai::types::content::TextContent {
                text: text.to_owned(),
                text_signature: None,
            },
        )],
        ..crate::agent_core::types::AgentToolResult::default()
    }
}

#[test]
fn public_event_wire_matches_node_oracle() {
    let lines = oracle_lines();
    assert_eq!(lines.len(), 21);

    let events: Vec<HarnessEvent> = vec![
        // 1. run_start
        HarnessEvent::RunStart {
            run_id: "op-1".to_owned(),
            started_at: 100,
            lane: "main".to_owned(),
        },
        // 2. lane_created
        HarnessEvent::LaneCreated {
            lane: "main".to_owned(),
            at: None,
        },
        // 3. value_update session_name
        HarnessEvent::ValueUpdate {
            update: ValueUpdatePayload::SessionName {
                name: Some("named".to_owned()),
            },
        },
        // 4. value_update entry_label (label deleted → omitted)
        HarnessEvent::ValueUpdate {
            update: ValueUpdatePayload::EntryLabel {
                target_id: "e1".to_owned(),
                label: None,
            },
        },
        // 5. global config_update streamOptions
        HarnessEvent::ConfigUpdate {
            lane: None,
            recovery: false,
            property: PublicConfigProperty::StreamOptions {
                value: crate::agent_core::harness::types::AgentHarnessStreamOptions {
                    timeout_ms: Some(123),
                    ..Default::default()
                },
                previous: Default::default(),
            },
        },
        // 6. global config_update steeringMode
        HarnessEvent::ConfigUpdate {
            lane: None,
            recovery: false,
            property: PublicConfigProperty::SteeringMode {
                value: QueueModeType::OneAtATime,
                previous: QueueModeType::All,
            },
        },
        // 7. global config_update tools (no value payload)
        HarnessEvent::ConfigUpdate {
            lane: None,
            recovery: false,
            property: PublicConfigProperty::Tools,
        },
        // 8. lane config_update model
        HarnessEvent::ConfigUpdate {
            lane: Some("main".to_owned()),
            recovery: false,
            property: PublicConfigProperty::Model {
                value: LaneModel {
                    provider: "faux".to_owned(),
                    model_id: "faux-2".to_owned(),
                },
                // Use the same input order as this legacy literal capture.
                // A differently ordered input must not be silently sorted.
                previous: serde_json::json!({"modelId": "faux-1", "provider": "faux"}),
            },
        },
        // 9. fault
        HarnessEvent::Fault {
            code: "harness_fault".to_owned(),
            message: "AgentHarness storage or invariant fault".to_owned(),
        },
        // 10. handler_error hook lane-scoped
        HarnessEvent::HandlerError {
            kind: super::HandlerErrorKind::Hook,
            hook: Some("before_run".to_owned()),
            event: None,
            error: "boom".to_owned(),
            stack: None,
            lane: Some("main".to_owned()),
        },
        // 11. handler_error event global with stack
        HarnessEvent::HandlerError {
            kind: super::HandlerErrorKind::Event,
            hook: None,
            event: Some("message_update".to_owned()),
            error: "listener panicked".to_owned(),
            stack: Some("Error: listener panicked".to_owned()),
            lane: None,
        },
        // 12. operation_abort
        HarnessEvent::OperationAbort {
            operation_id: "op-1".to_owned(),
            steer: Vec::new(),
            follow_up: Vec::new(),
            lane: "main".to_owned(),
        },
        // 13. navigation_start
        HarnessEvent::NavigationStart {
            lane: "main".to_owned(),
            run_id: "op-1".to_owned(),
            target_id: Some("e3".to_owned()),
            started_at: 100,
        },
        // 14. tool_start
        HarnessEvent::ToolStart {
            lane: "main".to_owned(),
            run_id: "op-1".to_owned(),
            turn_id: "t1".to_owned(),
            tool_call_id: "c1".to_owned(),
            tool_name: "read".to_owned(),
            args: serde_json::json!({"path": "a"}),
            recovery: false,
        },
        // 15. tool_update with recovery
        HarnessEvent::ToolUpdate {
            lane: "main".to_owned(),
            run_id: "op-1".to_owned(),
            turn_id: "t1".to_owned(),
            tool_call_id: "c1".to_owned(),
            tool_name: "read".to_owned(),
            partial_result: tool_result("partial"),
            recovery: true,
        },
        // 16. tool_end
        HarnessEvent::ToolEnd {
            lane: "main".to_owned(),
            run_id: "op-1".to_owned(),
            turn_id: "t1".to_owned(),
            tool_call_id: "c1".to_owned(),
            tool_name: "read".to_owned(),
            result: tool_result("done"),
            is_error: false,
            terminate: false,
            recovery: false,
        },
        // 17. run_suspend with a deferred handle and recovery
        HarnessEvent::RunSuspend {
            lane: "main".to_owned(),
            run_id: "op-1".to_owned(),
            deferred: DeferredHandle {
                provider: "faux".to_owned(),
                model_id: "faux-1".to_owned(),
                api: "faux-api".to_owned(),
                id: "resp-1".to_owned(),
                expires_at: None,
                poll_after_ms: None,
                data: None,
            },
            poll: 2,
            recovery: true,
        },
        // 18. usage
        HarnessEvent::Usage {
            lane: "main".to_owned(),
            row: crate::agent_core::harness::session::UsageRow {
                id: "u1".to_owned(),
                seq: 1,
                usage: sample_usage(),
                entry_id: None,
                adjustment: true,
                details: None,
            },
            totals: sample_usage(),
        },
        // 19. queue_update with a queued message item
        HarnessEvent::QueueUpdate {
            lane: "main".to_owned(),
            queues: vec![
                crate::agent_core::harness::runtime::projection::LaneQueuedItem::Message {
                    entry_id: "q1".to_owned(),
                    kind: crate::agent_core::harness::session::InboxItemKind::Steer,
                    message: AgentMessage::User(UserMessage {
                        content: StringOrBlocks::Text("hi".to_owned()),
                        timestamp: 1,
                    }),
                },
            ],
        },
        // 20. retry_scheduled
        HarnessEvent::RetryScheduled {
            lane: "main".to_owned(),
            run_id: "op-1".to_owned(),
            step: "assistant".to_owned(),
            attempt: 1,
            max_attempts: 3,
            delay_ms: 1000,
            not_before: 1100,
            error_message: "rate limited".to_owned(),
        },
        // 21. navigation_end completed
        HarnessEvent::NavigationEnd {
            lane: "main".to_owned(),
            run_id: "op-1".to_owned(),
            status: crate::agent_core::harness::runtime::events::RunEndStatus::Completed,
            from_tip_id: Some("e1".to_owned()),
            tip_id: Some("e3".to_owned()),
            ended_at: 200,
        },
    ];

    for (index, event) in events.iter().enumerate() {
        assert_eq!(
            wire(event),
            lines[index],
            "event {index} ({}) diverges from the node oracle",
            event.event_type()
        );
    }
}

// ---------------------------------------------------------------------------
// runtime Harness lane management (harness.test.ts)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn attaches_to_a_fresh_session_without_creating_an_implicit_main_lane() {
    let harness = create_harness("attach-fresh").await;

    assert!(harness
        .lanes(background_context())
        .await
        .unwrap()
        .is_empty());
    // Upstream also asserts the harness object carries no lane-level members
    // (`"accept" in harness`); the split AgentHarness/AgentLane interfaces
    // make that a compile-time fact here.
    assert!(harness
        .session()
        .branch("main", background_context())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn atomically_gets_or_creates_a_complete_agent_lane() {
    let harness = create_harness("attach-atomic").await;
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    harness
        .events()
        .on("lane_created", move |event: Arc<HarnessEvent>, _context| {
            let sink = Arc::clone(&sink);
            Box::pin(async move {
                if let HarnessEvent::LaneCreated { lane, at } = &*event {
                    push(
                        &sink,
                        format!("lane_created:{lane}:{}", at.as_deref().unwrap_or("<null>")),
                    );
                }
            })
        })
        .expect("bus open");

    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();
    let same = harness
        .lane(
            "main",
            AcquireLaneOptions {
                create_at: Some("ignored".to_owned()),
            },
            background_context(),
        )
        .await
        .unwrap();

    assert!(Arc::ptr_eq(&lane, &same));
    assert_eq!(
        AgentLane::get_tip_id(&*lane, background_context())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        AgentLane::get_thinking_level(&*lane, background_context())
            .await
            .unwrap(),
        ThinkingLevelType::Medium
    );
    assert_eq!(
        AgentLane::get_active_tools(&*lane, background_context())
            .await
            .unwrap(),
        vec!["read".to_owned(), "bash".to_owned()]
    );
    let stored = harness
        .session()
        .get_value(&lane_state("main"), background_context())
        .await
        .unwrap()
        .expect("lane state stored");
    assert_eq!(
        stored.value,
        serde_json::json!({
            "currentOperationId": null,
            "lastOperationId": null,
            "inbox": [],
        })
    );
    assert_eq!(drain(&log), vec!["lane_created:main:<null>"]);
    let lanes = harness.lanes(background_context()).await.unwrap();
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0].name, "main");
    assert_eq!(lanes[0].tip_id, None);
    assert_eq!(lanes[0].operation, None);
}

#[tokio::test]
async fn returns_one_published_agent_lane_under_concurrent_acquisition() {
    let harness = create_harness("attach-concurrent").await;
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    harness
        .events()
        .on(
            "lane_created",
            move |_event: Arc<HarnessEvent>, _context| {
                let sink = Arc::clone(&sink);
                Box::pin(async move { push(&sink, "lane_created".to_owned()) })
            },
        )
        .expect("bus open");

    let context = background_context();
    let (first, second, third) = tokio::join!(
        harness.lane("main", AcquireLaneOptions::default(), context.clone()),
        harness.lane("main", AcquireLaneOptions::default(), context.clone()),
        harness.lane("main", AcquireLaneOptions::default(), context),
    );
    let (first, second, third) = (first.unwrap(), second.unwrap(), third.unwrap());

    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&first, &third));
    assert_eq!(drain(&log).len(), 1);
}

#[tokio::test]
async fn serializes_commands_from_different_agent_lanes_on_the_one_session_line() {
    let harness = create_harness("attach-serialize").await;
    let main = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();
    let review = harness
        .lane(
            "review",
            AcquireLaneOptions::default(),
            background_context(),
        )
        .await
        .unwrap();

    let order: Log = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Notify::new());

    let order1 = Arc::clone(&order);
    let started1 = Arc::clone(&started);
    let gate1 = Arc::clone(&gate);
    let first = tokio::spawn(async move {
        main.command(
            move |_state, _reader| {
                let order = Arc::clone(&order1);
                let started = Arc::clone(&started1);
                let gate = Arc::clone(&gate1);
                Box::pin(async move {
                    push(&order, "main:start".to_owned());
                    started.notify_one();
                    gate.notified().await;
                    push(&order, "main:end".to_owned());
                    Ok(LaneCommand::Return { result: () })
                })
            },
            background_context(),
        )
        .await
        .unwrap()
    });
    started.notified().await;

    let order2 = Arc::clone(&order);
    let second = tokio::spawn(async move {
        review
            .command(
                move |_state, _reader| {
                    let order = Arc::clone(&order2);
                    Box::pin(async move {
                        push(&order, "review".to_owned());
                        Ok(LaneCommand::Return { result: () })
                    })
                },
                background_context(),
            )
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;
    assert_eq!(drain(&order), vec!["main:start"]);

    gate.notify_one();
    let (first, second) = tokio::join!(first, second);
    first.unwrap();
    second.unwrap();
    assert_eq!(drain(&order), vec!["main:start", "main:end", "review"]);
}

#[tokio::test]
async fn uses_create_at_only_for_a_missing_lane_and_validates_the_target() {
    let session = create_session("attach-createat").await;
    commit_writes(
        &session,
        vec![insert_entry(NewEntry::Custom {
            id: "target".to_owned(),
            parent_id: None,
            custom_type: "target".to_owned(),
            data: None,
        })],
        background_context(),
    )
    .await
    .unwrap();
    let harness = create_harness_for(session).await;
    let lane = harness
        .lane(
            "review",
            AcquireLaneOptions {
                create_at: Some("target".to_owned()),
            },
            background_context(),
        )
        .await
        .unwrap();

    assert_eq!(
        AgentLane::get_tip_id(&*lane, background_context())
            .await
            .unwrap(),
        Some("target".to_owned())
    );
    let same = harness
        .lane(
            "review",
            AcquireLaneOptions {
                create_at: Some("missing".to_owned()),
            },
            background_context(),
        )
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&lane, &same));

    let unknown = harness
        .lane(
            "missing",
            AcquireLaneOptions {
                create_at: Some("unknown".to_owned()),
            },
            background_context(),
        )
        .await
        .unwrap_err();
    let unknown = unknown
        .downcast_ref::<TaggedError>()
        .expect("UnknownTarget");
    assert!(matches!(unknown, TaggedError::UnknownTarget { .. }));

    for bad in ["", "bad\u{0000}name"] {
        let error = harness
            .lane(bad, AcquireLaneOptions::default(), background_context())
            .await
            .unwrap_err();
        let tagged = error.downcast_ref::<TaggedError>().expect("InvalidLane");
        assert!(matches!(tagged, TaggedError::InvalidLane { .. }), "{bad}");
    }
}

async fn create_harness_for(session: Arc<StorageBackedSession>) -> Harness<()> {
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux model");
    let (harness, open) = create_agent_harness(
        harness_options(session, models, model),
        background_context(),
    )
    .await
    .expect("harness attaches");
    assert!(open.is_empty());
    harness
}

#[tokio::test]
async fn attaches_agent_state_to_a_data_only_branch_without_moving_its_tip() {
    let session = create_session("attach-branch").await;
    commit_writes(
        &session,
        vec![
            insert_entry(NewEntry::Custom {
                id: "target".to_owned(),
                parent_id: None,
                custom_type: "target".to_owned(),
                data: None,
            }),
            set_value(
                &branch_tip("main"),
                serde_json::Value::String("target".to_owned()),
            ),
        ],
        background_context(),
    )
    .await
    .unwrap();
    let harness = create_harness_for(session).await;

    assert!(harness
        .lanes(background_context())
        .await
        .unwrap()
        .is_empty());
    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();
    assert_eq!(
        AgentLane::get_tip_id(&*lane, background_context())
            .await
            .unwrap(),
        Some("target".to_owned())
    );
    let stored = harness
        .session()
        .get_value(&lane_config("main"), background_context())
        .await
        .unwrap()
        .expect("lane config stored");
    assert_eq!(
        stored.value,
        serde_json::json!({
            "model": {"provider": "faux", "modelId": "faux-1"},
            "thinkingLevel": "medium",
            "activeToolNames": ["read", "bash"],
        })
    );
}

#[tokio::test]
async fn restores_complete_lanes_without_requiring_main() {
    let session = create_session("attach-restore").await;
    commit_writes(
        &session,
        vec![
            set_value(&branch_tip("review"), serde_json::Value::Null),
            set_value(
                &lane_config("review"),
                serde_json::to_value(CONFIGURED()).unwrap(),
            ),
            set_value(
                &lane_state("review"),
                serde_json::json!({
                    "currentOperationId": null,
                    "lastOperationId": null,
                    "inbox": [],
                }),
            ),
        ],
        background_context(),
    )
    .await
    .unwrap();

    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux model");
    let (harness, open) = create_agent_harness(
        harness_options(Arc::clone(&session), models, model),
        background_context(),
    )
    .await
    .unwrap();
    assert!(open.is_empty());
    let names: Vec<String> = harness
        .lanes(background_context())
        .await
        .unwrap()
        .into_iter()
        .map(|lane| lane.name)
        .collect();
    assert_eq!(names, vec!["review".to_owned()]);
    harness
        .lane(
            "review",
            AcquireLaneOptions::default(),
            background_context(),
        )
        .await
        .unwrap();
    assert!(session
        .branch("main", background_context())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn rejects_partial_durable_lane_state_as_a_harness_fault() {
    let session = create_session("attach-partial").await;
    commit_writes(
        &session,
        vec![
            set_value(&branch_tip("main"), serde_json::Value::Null),
            set_value(
                &lane_config("main"),
                serde_json::to_value(CONFIGURED()).unwrap(),
            ),
        ],
        background_context(),
    )
    .await
    .unwrap();

    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux model");
    let error = create_agent_harness(
        harness_options(session, models, model),
        background_context(),
    )
    .await
    .unwrap_err();
    assert!(
        error.downcast_ref::<HarnessFault>().is_some(),
        "expected HarnessFault, got {error:#}"
    );
}

// ---------------------------------------------------------------------------
// runtime Harness global metadata (harness.test.ts)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preserves_value_update_publication_and_delivery() {
    let session = create_session("metadata-values").await;
    let harness = create_harness_for(Arc::clone(&session)).await;
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    let session_for_listener = Arc::clone(&session);
    harness
        .events()
        .on("value_update", move |event: Arc<HarnessEvent>, context| {
            let sink = Arc::clone(&sink);
            let session = Arc::clone(&session_for_listener);
            Box::pin(async move {
                if let HarnessEvent::ValueUpdate { update } = &*event {
                    match update {
                        ValueUpdatePayload::SessionName { .. } => {
                            let name = session.get_name(context).await.unwrap();
                            push(&sink, format!("name:{}", name.unwrap_or_default()));
                        }
                        ValueUpdatePayload::EntryLabel { .. } => {
                            push(&sink, "entry_label".to_owned());
                        }
                    }
                }
            })
        })
        .expect("bus open");

    harness
        .set_name(Some("named".to_owned()), background_context())
        .await
        .unwrap();
    harness
        .set_label("entry", Some("label".to_owned()), background_context())
        .await
        .unwrap();

    assert_eq!(
        harness.get_name(background_context()).await.unwrap(),
        Some("named".to_owned())
    );
    assert_eq!(
        harness
            .get_label("entry", background_context())
            .await
            .unwrap(),
        Some("label".to_owned())
    );
    assert_eq!(drain(&log), vec!["name:named", "entry_label"]);
}

#[tokio::test]
async fn publishes_previous_and_current_data_bearing_global_configuration() {
    let harness = create_harness("metadata-config").await;
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    harness
        .events()
        .on(
            "config_update",
            move |event: Arc<HarnessEvent>, _context| {
                let sink = Arc::clone(&sink);
                Box::pin(async move {
                    if let HarnessEvent::ConfigUpdate { property, .. } = &*event {
                        match property {
                            PublicConfigProperty::StreamOptions { value, previous } => push(
                                &sink,
                                format!(
                                    "streamOptions value.timeout_ms={:?} previous.timeout_ms={:?}",
                                    value.timeout_ms, previous.timeout_ms
                                ),
                            ),
                            PublicConfigProperty::SteeringMode { value, previous } => {
                                push(&sink, format!("steeringMode {value:?} from {previous:?}"))
                            }
                            other => push(&sink, format!("unexpected {other:?}")),
                        }
                    }
                })
            },
        )
        .expect("bus open");

    harness
        .set_stream_options(
            crate::agent_core::harness::types::AgentHarnessStreamOptions {
                timeout_ms: Some(123),
                ..Default::default()
            },
            background_context(),
        )
        .await
        .unwrap();
    harness
        .set_steering_mode(QueueModeType::OneAtATime, background_context())
        .await
        .unwrap();

    assert_eq!(
        drain(&log),
        vec![
            "streamOptions value.timeout_ms=Some(123) previous.timeout_ms=None",
            "steeringMode OneAtATime from All",
        ]
    );
}

#[tokio::test]
async fn closes_every_lane_and_rejects_later_acquisition() {
    let harness = create_harness("metadata-close").await;
    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();
    harness.close(background_context()).await.unwrap();

    let error = harness
        .lane("other", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("closed"), "{error}");
    let lane_error = AgentLane::get_tip_id(&*lane, background_context())
        .await
        .unwrap_err();
    assert!(lane_error.to_string().contains("closed"), "{lane_error}");
}

#[tokio::test]
async fn memory_session_repo_creation_also_remains_branchless() {
    let repo = MemorySessionRepo::new(MemorySessionRepoOptions::default());
    let session = repo
        .create(
            SessionCreateOptions {
                id: Some("repo-session".to_owned()),
                parent_session_id: None,
            },
            background_context(),
        )
        .await
        .unwrap();
    assert!(session
        .branch("main", background_context())
        .await
        .unwrap()
        .is_none());
    session.close(background_context()).await.unwrap();
    repo.close(background_context()).await.unwrap();
}

// ---------------------------------------------------------------------------
// public drive dispatcher (drive-public.test.ts dispatcher-level cases)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settles_one_accepted_run_through_the_public_dispatcher() {
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux model");
    faux.set_responses(vec![
        faux_assistant_message("answer", Default::default()).into()
    ]);
    let session = create_session("public-dispatch").await;
    let (harness, open) = create_agent_harness(
        dispatcher_options(session, models, model),
        background_context(),
    )
    .await
    .unwrap();
    assert!(open.is_empty());
    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();

    let admission = AgentLane::accept(
        &*lane,
        &OperationRequest::Prompt {
            operation_id: None,
            payload: PromptPayload::Text {
                prompt: "run".to_owned(),
                images: None,
            },
        },
        background_context(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(admission.kind, "run");

    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: admission.operation_id.clone(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let outcome = drive_operation(&lane, &drive, &DriveEnv::for_lane(&lane))
        .await
        .expect("drive settles");
    match outcome {
        DriveOutcome::Settled { outcome: record } => {
            assert_eq!(record.operation_id, admission.operation_id);
            assert_eq!(record.status, TerminalStatus::Completed);
            assert_eq!(record.kind, "run");
        }
        other => panic!("expected settled outcome, got {other:#?}"),
    }
    assert_eq!(faux.state().lock().unwrap().call_count, 1);
}

#[tokio::test]
async fn isolates_stale_operation_ids_in_the_dispatcher() {
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux model");
    let session = create_session("public-stale").await;
    let (harness, _open) = create_agent_harness(
        harness_options(session, models, model),
        background_context(),
    )
    .await
    .unwrap();
    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();

    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: "stale".to_owned(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let error = drive_operation(&lane, &drive, &DriveEnv::for_lane(&lane))
        .await
        .expect_err("stale drive rejected");
    assert!(
        error
            .to_string()
            .contains("has no matching current operation"),
        "{error}"
    );
    assert_eq!(faux.state().lock().unwrap().call_count, 0);
}

#[tokio::test]
async fn failing_before_drive_hook_reports_and_fails_the_pass() {
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux model");
    let session = create_session("public-hookfault").await;
    let (harness, _open) = create_agent_harness(
        harness_options(session, models, model),
        background_context(),
    )
    .await
    .unwrap();

    // Upstream drive-public.test.ts "faults the harness when a detached pass
    // fails": the hook throws, the pass rejects. The lane fault-install
    // mapping is the unported lane.drive() slice; the dispatcher propagates
    // the hook failure and the hook error reporter publishes handler_error.
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    harness
        .events()
        .on(
            "handler_error",
            move |event: Arc<HarnessEvent>, _context| {
                let sink = Arc::clone(&sink);
                Box::pin(async move {
                    if let HarnessEvent::HandlerError {
                        kind,
                        hook,
                        lane,
                        error,
                        ..
                    } = &*event
                    {
                        push(
                            &sink,
                            format!(
                                "handler_error {kind:?} hook={hook:?} lane={lane:?} error={error}",
                                kind = kind,
                                hook = hook,
                                lane = lane,
                                error = error
                            ),
                        );
                    }
                })
            },
        )
        .expect("bus open");
    harness
        .hooks()
        .on(
            HookName::BeforeDrive,
            Arc::new(|_invocation, _context| {
                Box::pin(async { Err::<HookResult, _>(anyhow::anyhow!("drive failed")) })
            }),
            None,
        )
        .expect("hooks open");

    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();
    let admission = AgentLane::accept(
        &*lane,
        &OperationRequest::Prompt {
            operation_id: None,
            payload: PromptPayload::Text {
                prompt: "run".to_owned(),
                images: None,
            },
        },
        background_context(),
    )
    .await
    .unwrap()
    .unwrap();
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: admission.operation_id,
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let error = drive_operation(&lane, &drive, &DriveEnv::for_lane(&lane))
        .await
        .expect_err("hook failure fails the pass");
    assert!(error.to_string().contains("drive failed"), "{error}");
    let log = drain(&log);
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].contains("hook=Some(\"before_drive\")"), "{log:?}");
    assert!(log[0].contains("lane=Some(\"main\")"), "{log:?}");
    assert_eq!(faux.state().lock().unwrap().call_count, 0);
}

// ---------------------------------------------------------------------------
// Un-pended upstream lane-slice cases. Behavior authorities:
// `harness.test.ts` ("uses one stable provider session id per lane", the two
// append cases) and `drive-public.test.ts` (the runtime public drive block),
// one test per upstream `it` (`it.each` cases stay per-parameter).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PublicFixtureOptions {
    /// Upstream `createFixture({ deferred: true })` — the harness
    /// `streamOptions.deferred` flag (the faux provider needs no extra
    /// options: its `pendingFetches` default is zero, so a deferred
    /// submission suspends with a handle and the first permitted poll
    /// resolves it).
    deferred: bool,
    /// Upstream `createFixture({ resources })`.
    resources: Option<Resources>,
}

/// Upstream `createFixture` (drive-public.test.ts:20-48): faux provider +
/// fresh session + no explicit `activeToolNames`, and the runtime lane
/// "main" already acquired.
struct PublicFixture {
    lane: Arc<Lane>,
    harness: Harness<()>,
    faux: Arc<FauxProviderHandle>,
    session: Arc<StorageBackedSession>,
}

async fn create_public_fixture(session_id: &str, options: PublicFixtureOptions) -> PublicFixture {
    let faux = Arc::new(faux_provider(FauxProviderOptions::default()));
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux provider exposes faux-1");
    let session = create_session(session_id).await;
    let mut harness_options = dispatcher_options(Arc::clone(&session), models, model);
    if options.deferred {
        harness_options.stream_options = Some(AgentHarnessStreamOptions {
            deferred: Some(DeferredFlag::Bool(true)),
            ..AgentHarnessStreamOptions::default()
        });
    }
    harness_options.resources = options.resources;
    let (harness, open) = create_agent_harness(harness_options, background_context())
        .await
        .expect("harness attaches");
    assert!(open.is_empty());
    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .expect("main lane");
    PublicFixture {
        lane,
        harness,
        faux,
        session,
    }
}

/// Upstream `acceptRun` (drive-public.test.ts:50-54): an acceptance with an
/// explicit operation id.
async fn accept_run(lane: &Lane, operation_id: &str) -> super::OperationAdmission {
    AgentLane::accept(
        lane,
        &OperationRequest::Prompt {
            operation_id: Some(operation_id.to_owned()),
            payload: PromptPayload::Text {
                prompt: operation_id.to_owned(),
                images: None,
            },
        },
        background_context(),
    )
    .await
    .expect("accept runs")
    .expect("prompt accepted")
}

fn user_entry_message(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_owned()),
        timestamp,
    })
}

/// Upstream's gated async faux response (`async () => { started.resolve();
/// await release.promise; return fauxAssistantMessage(...) }`).
fn gated_response(
    text: &str,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
) -> FauxResponseStep {
    let text = text.to_owned();
    FauxResponseStep::Factory(Arc::new(move |_args| {
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        let text = text.clone();
        Box::pin(async move {
            started.notify_one();
            release.notified().await;
            Ok(faux_assistant_message(text, FauxMessageOptions::default()))
        })
    }))
}

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    for _ in 0..5_000 {
        if predicate() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("condition was not reached");
}

fn settled_record(outcome: RunOutcome, label: &str) -> OperationResultRecord {
    match outcome {
        RunOutcome::Record(record) => record,
        other => panic!("{label}: expected a settled record, got {other:?}"),
    }
}

fn faux_call_count(fixture: &PublicFixture) -> u64 {
    fixture
        .faux
        .state()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .call_count
}

/// harness.test.ts "uses one stable provider session id per lane": a
/// provider wrapper records the `sessionId` each simple request carries
/// (`options?.sessionId` upstream).
struct SessionRecordingApi {
    inner: Arc<dyn ApiImplTrait>,
    session_ids: Arc<Mutex<Vec<Option<String>>>>,
}

impl ApiImplTrait for SessionRecordingApi {
    fn supports_request_callbacks(&self) -> bool {
        self.inner.supports_request_callbacks()
    }

    fn supports_deferred_cancel(&self) -> bool {
        self.inner.supports_deferred_cancel()
    }

    fn stream(
        &self,
        cfg: &crate::ai::ProviderConfig,
        model: &crate::ai::types::Model,
        ctx: &crate::ai::transcript::TranscriptContext,
        options: &crate::ai::types::options::StreamOptions,
    ) -> tokio::sync::mpsc::Receiver<crate::ai::types::events::AssistantMessageEvent> {
        self.inner.stream(cfg, model, ctx, options)
    }

    fn stream_simple(
        &self,
        cfg: &crate::ai::ProviderConfig,
        model: &crate::ai::types::Model,
        ctx: &crate::ai::transcript::TranscriptContext,
        options: &crate::ai::types::options::SimpleStreamOptions,
    ) -> tokio::sync::mpsc::Receiver<crate::ai::types::events::AssistantMessageEvent> {
        self.session_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(options.stream.session_id.clone());
        self.inner.stream_simple(cfg, model, ctx, options)
    }
}

struct SessionRecordingProvider {
    inner: Arc<dyn Provider>,
    session_ids: Arc<Mutex<Vec<Option<String>>>>,
}

impl Provider for SessionRecordingProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn auth(&self) -> &ProviderAuth {
        self.inner.auth()
    }

    fn get_models(&self) -> Result<Vec<crate::ai::types::Model>, ModelsError> {
        self.inner.get_models()
    }

    fn api_for(&self, model: &crate::ai::types::Model) -> Option<Arc<dyn ApiImplTrait>> {
        Some(Arc::new(SessionRecordingApi {
            inner: self.inner.api_for(model)?,
            session_ids: Arc::clone(&self.session_ids),
        }))
    }
}

#[tokio::test]
async fn uses_one_stable_provider_session_id_per_lane() {
    let faux = Arc::new(faux_provider(FauxProviderOptions::default()));
    faux.set_responses(vec![
        faux_assistant_message("main one", FauxMessageOptions::default()).into(),
        faux_assistant_message("main two", FauxMessageOptions::default()).into(),
        faux_assistant_message("review", FauxMessageOptions::default()).into(),
    ]);
    let session_ids: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::new(SessionRecordingProvider {
        inner: Arc::clone(&faux.provider),
        session_ids: Arc::clone(&session_ids),
    }));
    let model = faux.get_model(None).expect("faux model");
    let session = create_session("shared-session").await;
    // Upstream harnessOptions configures `activeToolNames: ["read", "bash"]`;
    // the port fails a run whose configured tools have no process executors
    // (`configured_tools_unavailable`), and the test drives real runs, so the
    // fixture uses the no-active-tools options (upstream assertions here only
    // observe provider session ids).
    let (harness, _open) = create_agent_harness(
        dispatcher_options(Arc::clone(&session), models, model),
        background_context(),
    )
    .await
    .unwrap();
    let main = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .unwrap();
    let review = harness
        .lane(
            "review",
            AcquireLaneOptions::default(),
            background_context(),
        )
        .await
        .unwrap();

    AgentLane::prompt(&*main, "one", None, background_context())
        .await
        .unwrap()
        .expect("main one settles");
    AgentLane::prompt(&*main, "two", None, background_context())
        .await
        .unwrap()
        .expect("main two settles");
    AgentLane::prompt(&*review, "review", None, background_context())
        .await
        .unwrap()
        .expect("review settles");

    let recorded: Vec<Option<String>> = session_ids
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(
        recorded,
        vec![
            Some("shared-session:main".to_owned()),
            Some("shared-session:main".to_owned()),
            Some("shared-session:review".to_owned()),
        ]
    );
}

/// harness.test.ts "keeps AgentLane appends operation-aware while exposing
/// the Branch surface directly".
#[tokio::test]
async fn keeps_agent_lane_appends_operation_aware_while_exposing_the_branch_surface() {
    let fixture = create_public_fixture("lane-append-aware", PublicFixtureOptions::default()).await;
    let idle_id = AgentLane::append_custom_entry(
        &*fixture.lane,
        "idle".to_owned(),
        None,
        background_context(),
    )
    .await
    .unwrap();
    assert_eq!(
        AgentLane::get_tip_id(&*fixture.lane, background_context())
            .await
            .unwrap(),
        Some(idle_id.clone())
    );

    let admission = accept_run(&fixture.lane, "run").await;
    let accepted_tip = AgentLane::get_tip_id(&*fixture.lane, background_context())
        .await
        .unwrap();
    let pending_id = AgentLane::append_custom_entry(
        &*fixture.lane,
        "pending".to_owned(),
        Some(serde_json::json!({"queued": true})),
        background_context(),
    )
    .await
    .unwrap();

    // The operation-aware append stages into the inbox; the tip stays put.
    assert_eq!(
        AgentLane::get_tip_id(&*fixture.lane, background_context())
            .await
            .unwrap(),
        accepted_tip
    );
    assert_eq!(
        fixture
            .session
            .get_value(&branch_tip("main"), background_context())
            .await
            .unwrap()
            .expect("branch tip stored")
            .value,
        serde_json::json!(accepted_tip)
    );
    let stored = fixture
        .session
        .get_value(&pending_entry(&pending_id), background_context())
        .await
        .unwrap()
        .expect("pending entry stored");
    assert_eq!(
        stored.value,
        serde_json::json!({
            "type": "custom",
            "customType": "pending",
            "payload": {"queued": true},
        })
    );
    assert_eq!(
        fixture.lane.state().inbox,
        vec![InboxItem {
            entry_id: pending_id,
            kind: InboxItemKind::Write,
        }],
        "operation admission {}",
        admission.operation_id
    );
}

/// harness.test.ts "flushes queued writes before a new idle append in one
/// commit".
#[tokio::test]
async fn flushes_queued_writes_before_a_new_idle_append_in_one_commit() {
    let fixture = create_public_fixture("lane-append-flush", PublicFixtureOptions::default()).await;
    let first = fixture.session.id_generator().next(None);
    let second = fixture.session.id_generator().next(None);
    let first_id = first.clone();
    let second_id = second.clone();
    let inbox = vec![
        InboxItem {
            entry_id: first.clone(),
            kind: InboxItemKind::Write,
        },
        InboxItem {
            entry_id: second.clone(),
            kind: InboxItemKind::Write,
        },
    ];
    fixture
        .lane
        .command(
            move |state, _reader| {
                let inbox = inbox.clone();
                let first = first.clone();
                let second = second.clone();
                Box::pin(async move {
                    let writes = vec![
                        set_value(
                            &pending_entry(&first),
                            serde_json::json!({"type": "custom", "customType": "queued-first"}),
                        ),
                        set_value(
                            &pending_entry(&second),
                            serde_json::json!({"type": "custom", "customType": "queued-second"}),
                        ),
                        set_value(
                            &lane_state("main"),
                            serde_json::json!({
                                "currentOperationId": null,
                                "lastOperationId": null,
                                "inbox": inbox,
                            }),
                        ),
                    ];
                    let mut next = state.clone();
                    next.inbox = inbox;
                    Ok(LaneCommand::Commit {
                        writes,
                        next,
                        materialize: Box::new(|_commit| ()),
                        events: None,
                    })
                })
            },
            background_context(),
        )
        .await
        .unwrap();

    let appended = AgentLane::append_custom_entry(
        &*fixture.lane,
        "new".to_owned(),
        None,
        background_context(),
    )
    .await
    .unwrap();

    let scan = BranchScan {
        order: Some(BranchScanOrder::OldestFirst),
        ..BranchScan::default()
    };
    let ids: Vec<String> =
        AgentLane::find_entries(&*fixture.lane, Some(&scan), background_context())
            .await
            .unwrap()
            .into_iter()
            .map(|entry| entry.id().to_owned())
            .collect();
    assert_eq!(ids, [first_id, second_id, appended]);
    assert!(
        fixture.lane.state().inbox.is_empty(),
        "the idle append flushed the queued writes"
    );
}

/// drive-public.test.ts "composes prompt, skill, and template acceptance
/// with drive".
#[tokio::test]
async fn composes_prompt_skill_and_template_acceptance_with_drive() {
    let resources = Resources {
        skills: Some(vec![Skill {
            name: "review".to_owned(),
            description: "Review".to_owned(),
            content: "Inspect it".to_owned(),
            file_path: "/skills/review/SKILL.md".to_owned(),
            disable_model_invocation: None,
        }]),
        prompt_templates: Some(vec![PromptTemplate {
            name: "fix".to_owned(),
            description: None,
            content: "Fix $1".to_owned(),
        }]),
    };
    let fixture = create_public_fixture(
        "public-compose",
        PublicFixtureOptions {
            resources: Some(resources),
            ..PublicFixtureOptions::default()
        },
    )
    .await;
    fixture.faux.set_responses(vec![
        faux_assistant_message("prompt answer", FauxMessageOptions::default()).into(),
        faux_assistant_message("skill answer", FauxMessageOptions::default()).into(),
        faux_assistant_message("template answer", FauxMessageOptions::default()).into(),
    ]);

    for (label, result) in [
        (
            "prompt",
            AgentLane::prompt(&*fixture.lane, "prompt", None, background_context())
                .await
                .unwrap(),
        ),
        (
            "skill",
            AgentLane::skill(
                &*fixture.lane,
                "review",
                Some("strict".to_owned()),
                background_context(),
            )
            .await
            .unwrap(),
        ),
        (
            "template",
            AgentLane::prompt_from_template(
                &*fixture.lane,
                "fix",
                Some(vec!["it".to_owned()]),
                background_context(),
            )
            .await
            .unwrap(),
        ),
    ] {
        let result = result.unwrap_or_else(|error| panic!("{label} accepted: {error}"));
        let record = settled_record(result, label);
        assert_eq!(record.kind, "run", "{label}");
        assert_eq!(record.status, TerminalStatus::Completed, "{label}");
    }
    assert_eq!(faux_call_count(&fixture), 3);
}

/// drive-public.test.ts "returns a convenience-only suspension observation".
#[tokio::test]
async fn returns_a_convenience_only_suspension_observation() {
    let fixture = create_public_fixture(
        "public-suspend",
        PublicFixtureOptions {
            deferred: true,
            ..PublicFixtureOptions::default()
        },
    )
    .await;
    fixture.faux.set_responses(vec![faux_assistant_message(
        "eventual answer",
        FauxMessageOptions::default(),
    )
    .into()]);

    let result = AgentLane::prompt(&*fixture.lane, "defer", None, background_context())
        .await
        .unwrap()
        .expect("prompt accepted");
    let suspended = match result {
        RunOutcome::Suspended(suspended) => suspended,
        other => panic!("expected a suspended observation, got {other:?}"),
    };
    assert_eq!(suspended.deferred.provider, "faux");
    assert_eq!(suspended.deferred.model_id, "faux-1");
    let operation = fixture
        .lane
        .state()
        .operation
        .expect("the deferred operation stays open");
    assert_eq!(operation.state.at(), "deferred.suspended");
}

/// drive-public.test.ts "records caller usage as an adjustment and publishes
/// committed totals".
#[tokio::test]
async fn records_caller_usage_as_an_adjustment_and_publishes_committed_totals() {
    struct UsageObservation {
        lane: String,
        id: String,
        adjustment: bool,
        entry_id: Option<String>,
        details: Option<serde_json::Value>,
        totals: Usage,
    }

    let fixture = create_public_fixture("public-usage", PublicFixtureOptions::default()).await;
    let usage = faux_assistant_message("usage", FauxMessageOptions::default()).usage;
    let observations: Arc<Mutex<Vec<UsageObservation>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&observations);
    fixture
        .harness
        .events()
        .on("usage", move |event: Arc<HarnessEvent>, _context| {
            let sink = Arc::clone(&sink);
            Box::pin(async move {
                if let HarnessEvent::Usage { lane, row, totals } = &*event {
                    sink.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(UsageObservation {
                            lane: lane.clone(),
                            id: row.id.clone(),
                            adjustment: row.adjustment,
                            entry_id: row.entry_id.clone(),
                            details: row.details.clone(),
                            totals: *totals,
                        });
                }
            })
        })
        .expect("bus open");

    let recorded = AgentLane::record_usage(
        &*fixture.lane,
        usage,
        Some(&RecordUsageOptions {
            entry_id: Some("external".to_owned()),
            details: Some(serde_json::json!({"source": "test"})),
        }),
        background_context(),
    )
    .await
    .unwrap()
    .expect("usage recorded");

    let totals = SessionTrait::get_stats(&*fixture.session, background_context())
        .await
        .unwrap()
        .usage;
    let mut observed = observations
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain(..)
        .collect::<Vec<_>>();
    assert_eq!(observed.len(), 1, "exactly one usage event");
    let seen = observed.remove(0);
    assert_eq!(seen.lane, "main");
    assert_eq!(seen.id, recorded.usage_id);
    assert!(seen.adjustment, "the caller row is an adjustment");
    assert_eq!(seen.entry_id.as_deref(), Some("external"));
    assert_eq!(seen.details, Some(serde_json::json!({"source": "test"})));
    assert_eq!(
        serde_json::to_value(seen.totals).unwrap(),
        serde_json::to_value(totals).unwrap(),
        "the event totals match the session stats"
    );
}

/// drive-public.test.ts "starts an ordinary continuation run from queued
/// {steer,followUp,nextRun} input" (`it.each`).
async fn continuation_run_from_queued_input(kind: &str) {
    let fixture = create_public_fixture(
        &format!("public-continuation-{kind}"),
        PublicFixtureOptions::default(),
    )
    .await;
    AgentLane::append_message(
        &*fixture.lane,
        user_entry_message("history", 1),
        background_context(),
    )
    .await
    .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fixture.faux.set_responses(vec![
        gated_response("summary", Arc::clone(&started), Arc::clone(&release)),
        faux_assistant_message("continuation answer", FauxMessageOptions::default()).into(),
    ]);
    let lane = Arc::clone(&fixture.lane);
    let compacting = tokio::spawn(async move {
        Lane::compact(&lane, None, background_context())
            .await
            .expect("compact runs")
    });
    started.notified().await;
    let queued = match kind {
        "steer" => {
            fixture
                .lane
                .steer(
                    QueueInput::Text("continue".to_owned()),
                    None,
                    background_context(),
                )
                .await
        }
        "followUp" => {
            fixture
                .lane
                .follow_up(
                    QueueInput::Text("continue".to_owned()),
                    None,
                    background_context(),
                )
                .await
        }
        "nextRun" => {
            fixture
                .lane
                .next_run(
                    QueueInput::Text("continue".to_owned()),
                    None,
                    background_context(),
                )
                .await
        }
        other => panic!("unknown queue kind {other}"),
    };
    queued.expect("queue runs").expect("queued input accepted");
    release.notify_one();

    let compacted = compacting
        .await
        .expect("compaction joins")
        .expect("compaction accepted");
    assert_eq!(compacted.compaction.kind, "compaction");
    assert_eq!(compacted.compaction.status, TerminalStatus::Completed);
    let run = compacted
        .run
        .expect("the queued input continues into a run");
    let record = settled_record(run, "continuation run");
    assert_eq!(record.kind, "run");
    assert_eq!(record.status, TerminalStatus::Completed);
    assert_eq!(faux_call_count(&fixture), 2);
}

#[tokio::test]
async fn starts_an_ordinary_continuation_run_from_queued_steer_input() {
    continuation_run_from_queued_input("steer").await;
}

#[tokio::test]
async fn starts_an_ordinary_continuation_run_from_queued_follow_up_input() {
    continuation_run_from_queued_input("followUp").await;
}

#[tokio::test]
async fn starts_an_ordinary_continuation_run_from_queued_next_run_input() {
    continuation_run_from_queued_input("nextRun").await;
}

/// drive-public.test.ts "lets a competing acceptance win the structural
/// continuation window".
#[tokio::test]
async fn lets_a_competing_acceptance_win_the_structural_continuation_window() {
    let fixture = create_public_fixture("public-competing", PublicFixtureOptions::default()).await;
    AgentLane::append_message(
        &*fixture.lane,
        user_entry_message("history", 1),
        background_context(),
    )
    .await
    .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fixture.faux.set_responses(vec![
        gated_response("summary", Arc::clone(&started), Arc::clone(&release)),
        faux_assistant_message("competitor answer", FauxMessageOptions::default()).into(),
    ]);
    let competing: Arc<
        Mutex<Option<tokio::task::JoinHandle<anyhow::Result<super::OperationAdmissionResult>>>>,
    > = Arc::new(Mutex::new(None));
    let competing_sink = Arc::clone(&competing);
    let listener_lane = Arc::clone(&fixture.lane);
    fixture
        .harness
        .events()
        .on(
            "compaction_end",
            move |event: Arc<HarnessEvent>, _context| {
                let lane = Arc::clone(&listener_lane);
                let competing_sink = Arc::clone(&competing_sink);
                Box::pin(async move {
                    if let HarnessEvent::CompactionEnd {
                        status: CompactionEndStatus::Completed,
                        ..
                    } = &*event
                    {
                        let handle = tokio::spawn(async move {
                            AgentLane::accept(
                                &*lane,
                                &OperationRequest::Prompt {
                                    operation_id: None,
                                    payload: PromptPayload::Text {
                                        prompt: "competitor".to_owned(),
                                        images: None,
                                    },
                                },
                                background_context(),
                            )
                            .await
                        });
                        *competing_sink
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = Some(handle);
                    }
                })
            },
        )
        .expect("bus open");
    let lane = Arc::clone(&fixture.lane);
    let compacting = tokio::spawn(async move {
        Lane::compact(&lane, None, background_context())
            .await
            .expect("compact runs")
    });
    started.notified().await;
    fixture
        .lane
        .next_run(
            QueueInput::Text("queued".to_owned()),
            None,
            background_context(),
        )
        .await
        .expect("nextRun runs")
        .expect("queued input accepted");
    release.notify_one();

    let compacted = compacting
        .await
        .expect("compaction joins")
        .expect("compaction accepted");
    assert_eq!(compacted.compaction.status, TerminalStatus::Completed);
    assert!(
        compacted.run.is_none(),
        "the competing acceptance wins the continuation window: {:?}",
        compacted.run
    );
    let competing_handle = competing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("competing acceptance did not start");
    let admission = competing_handle
        .await
        .expect("acceptance joins")
        .expect("acceptance resolves")
        .expect("competing acceptance admitted");
    let driven = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: admission.operation_id,
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("drive runs")
        .expect("drive settles");
    match driven {
        DriveOutcome::Settled { outcome } => {
            assert_eq!(outcome.status, TerminalStatus::Completed);
        }
        other => panic!("expected a settled outcome, got {other:?}"),
    }
}

/// drive-public.test.ts "cancels queued input and reports consumed or
/// missing ids".
#[tokio::test]
async fn cancels_queued_input_and_reports_consumed_or_missing_ids() {
    let fixture =
        create_public_fixture("public-cancel-queued", PublicFixtureOptions::default()).await;
    let cancelled = fixture
        .lane
        .next_run(
            QueueInput::Text("cancel".to_owned()),
            None,
            background_context(),
        )
        .await
        .expect("nextRun runs")
        .expect("queued input accepted");
    assert_eq!(
        fixture
            .lane
            .cancel_queued(&cancelled.entry_id, background_context())
            .await
            .expect("cancel runs"),
        Ok(CancelQueuedOutcome::Cancelled)
    );
    assert_eq!(
        fixture
            .lane
            .cancel_queued(&cancelled.entry_id, background_context())
            .await
            .expect("second cancel runs"),
        Ok(CancelQueuedOutcome::NotFound)
    );

    let consumed = fixture
        .lane
        .next_run(
            QueueInput::Text("consume".to_owned()),
            None,
            background_context(),
        )
        .await
        .expect("nextRun runs")
        .expect("queued input accepted");
    // Upstream accepts the empty prompt, which consumes the queued nextRun
    // input as its payload.
    let admission = AgentLane::accept(
        &*fixture.lane,
        &OperationRequest::Prompt {
            operation_id: None,
            payload: PromptPayload::Text {
                prompt: String::new(),
                images: None,
            },
        },
        background_context(),
    )
    .await
    .expect("accept runs")
    .expect("empty prompt admitted with the queued input");
    assert_eq!(
        fixture
            .lane
            .cancel_queued(&consumed.entry_id, background_context())
            .await
            .expect("cancel runs"),
        Ok(CancelQueuedOutcome::AlreadyConsumed)
    );
    fixture.faux.set_responses(vec![faux_assistant_message(
        "answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let driven = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: admission.operation_id,
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("drive runs")
        .expect("drive settles");
    assert!(matches!(driven, DriveOutcome::Settled { .. }));
}

/// drive-public.test.ts "admits input after cancellation and preserves it
/// through reconciliation".
#[tokio::test]
async fn admits_input_after_cancellation_and_preserves_it_through_reconciliation() {
    let fixture =
        create_public_fixture("public-cancel-reconcile", PublicFixtureOptions::default()).await;
    accept_run(&fixture.lane, "cancelled").await;
    fixture
        .lane
        .request_abort("cancelled", background_context())
        .await
        .expect("requestAbort runs")
        .expect("abort requested");
    let late = fixture
        .lane
        .steer(
            QueueInput::Text("late".to_owned()),
            None,
            background_context(),
        )
        .await
        .expect("steer runs");
    assert!(late.is_ok());
    let driven = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: "cancelled".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("drive runs")
        .expect("drive settles");
    match driven {
        DriveOutcome::Settled { outcome } => {
            assert_eq!(outcome.status, TerminalStatus::Aborted);
        }
        other => panic!("expected an aborted settlement, got {other:?}"),
    }
    assert_eq!(fixture.lane.state().inbox.len(), 1);

    fixture.faux.set_responses(vec![faux_assistant_message(
        "late answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let record = settled_record(
        AgentLane::prompt(&*fixture.lane, "", None, background_context())
            .await
            .unwrap()
            .expect("prompt accepted"),
        "late run",
    );
    assert_eq!(record.status, TerminalStatus::Completed);
}

/// drive-public.test.ts "composes standalone compaction acceptance with
/// drive".
#[tokio::test]
async fn composes_standalone_compaction_acceptance_with_drive() {
    let fixture = create_public_fixture("public-compact", PublicFixtureOptions::default()).await;
    AgentLane::append_message(
        &*fixture.lane,
        user_entry_message("history", 1),
        background_context(),
    )
    .await
    .unwrap();
    fixture.faux.set_responses(vec![faux_assistant_message(
        "summary",
        FauxMessageOptions::default(),
    )
    .into()]);

    let compacted = Lane::compact(&fixture.lane, None, background_context())
        .await
        .expect("compact runs")
        .expect("compaction accepted");
    assert_eq!(compacted.compaction.kind, "compaction");
    assert_eq!(compacted.compaction.status, TerminalStatus::Completed);
    assert_eq!(faux_call_count(&fixture), 1);
}

/// drive-public.test.ts "composes {false,true} summarized navigation
/// acceptance with drive" (`it.each`).
async fn summarized_navigation_acceptance(summarize: bool) {
    let fixture = create_public_fixture(
        &format!("public-navigate-{summarize}"),
        PublicFixtureOptions::default(),
    )
    .await;
    let root_id = AgentLane::append_message(
        &*fixture.lane,
        user_entry_message("root", 1),
        background_context(),
    )
    .await
    .unwrap();
    AgentLane::append_message(
        &*fixture.lane,
        user_entry_message("source", 2),
        background_context(),
    )
    .await
    .unwrap();
    commit_writes(
        &fixture.session,
        vec![insert_entry(NewEntry::Message {
            id: "target".to_owned(),
            parent_id: Some(root_id),
            message: user_entry_message("target", 3),
            terminate: None,
        })],
        background_context(),
    )
    .await
    .unwrap();
    if summarize {
        fixture.faux.set_responses(vec![faux_assistant_message(
            "branch summary",
            FauxMessageOptions::default(),
        )
        .into()]);
    }

    let navigated = fixture
        .lane
        .navigate_tree(
            Some("target".to_owned()),
            Some(RuntimeNavigationOptions {
                summarize: Some(summarize),
                label: Some("chosen".to_owned()),
                custom_instructions: None,
            }),
            background_context(),
        )
        .await
        .expect("navigate runs")
        .expect("navigation accepted");
    assert_eq!(navigated.navigation.kind, "navigation");
    assert_eq!(navigated.navigation.status, TerminalStatus::Completed);
    assert_eq!(
        faux_call_count(&fixture),
        u64::from(summarize),
        "summarized navigation runs exactly the branch summary"
    );
    assert!(
        fixture.lane.state().tip_id.is_some(),
        "navigation moved the tip"
    );
}

#[tokio::test]
async fn composes_false_summarized_navigation_acceptance_with_drive() {
    summarized_navigation_acceptance(false).await;
}

#[tokio::test]
async fn composes_true_summarized_navigation_acceptance_with_drive() {
    summarized_navigation_acceptance(true).await;
}

/// drive-public.test.ts "resumes any current operation after acceptance or
/// reopen".
#[tokio::test]
async fn resumes_any_current_operation_after_acceptance_or_reopen() {
    let fixture = create_public_fixture("public-resume", PublicFixtureOptions::default()).await;
    accept_run(&fixture.lane, "run").await;
    fixture.faux.set_responses(vec![faux_assistant_message(
        "answer",
        FauxMessageOptions::default(),
    )
    .into()]);

    let resumed = Lane::resume(&fixture.lane, background_context())
        .await
        .expect("resume runs")
        .expect("resume accepted");
    let record = settled_record(resumed, "resumed operation");
    assert_eq!(record.operation_id, "run");
    assert_eq!(record.kind, "run");
    assert_eq!(record.status, TerminalStatus::Completed);
}

/// drive-public.test.ts "polls one deferred permit through resume".
#[tokio::test]
async fn polls_one_deferred_permit_through_resume() {
    let fixture = create_public_fixture(
        "public-resume-deferred",
        PublicFixtureOptions {
            deferred: true,
            ..PublicFixtureOptions::default()
        },
    )
    .await;
    fixture.faux.set_responses(vec![faux_assistant_message(
        "eventual answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let suspended = match AgentLane::prompt(&*fixture.lane, "defer", None, background_context())
        .await
        .unwrap()
        .expect("prompt accepted")
    {
        RunOutcome::Suspended(suspended) => suspended,
        other => panic!("expected a suspended run, got {other:?}"),
    };

    let resumed = Lane::resume(&fixture.lane, background_context())
        .await
        .expect("resume runs")
        .expect("resume accepted");
    let record = settled_record(resumed, "deferred poll");
    assert_eq!(record.operation_id, suspended.operation_id);
    assert_eq!(record.kind, "run");
    assert_eq!(record.status, TerminalStatus::Completed);
    assert!(fixture.lane.state().operation.is_none());
    assert_eq!(
        fixture
            .faux
            .state()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .deferred_fetch_count,
        1
    );
    let again = Lane::resume(&fixture.lane, background_context())
        .await
        .expect("resume runs");
    assert!(
        matches!(again, Err(TaggedError::NothingToResume { .. })),
        "{again:?}"
    );
}

/// drive-public.test.ts "aborts and reconciles the current operation".
#[tokio::test]
async fn aborts_and_reconciles_the_current_operation() {
    let fixture = create_public_fixture("public-abort", PublicFixtureOptions::default()).await;
    accept_run(&fixture.lane, "run").await;

    let aborted = Lane::abort(&fixture.lane, background_context())
        .await
        .expect("abort runs")
        .expect("abort accepted");
    assert_eq!(aborted.operation_id, "run");
    assert!(aborted.steer.is_empty());
    assert!(aborted.follow_up.is_empty());
    assert!(fixture.lane.state().operation.is_none());
    assert_eq!(faux_call_count(&fixture), 0);
    let again = Lane::abort(&fixture.lane, background_context())
        .await
        .expect("abort runs");
    assert!(
        matches!(again, Err(TaggedError::NoActiveOperation { .. })),
        "{again:?}"
    );
}

/// drive-public.test.ts "waits for an operation that has no installed drive".
#[tokio::test]
async fn waits_for_an_operation_that_has_no_installed_drive() {
    let fixture = create_public_fixture("public-wait-idle", PublicFixtureOptions::default()).await;
    accept_run(&fixture.lane, "run").await;
    let idle = Arc::new(Mutex::new(false));
    let idle_sink = Arc::clone(&idle);
    let lane = Arc::clone(&fixture.lane);
    let waiting = tokio::spawn(async move {
        Lane::wait_for_idle(&lane, background_context())
            .await
            .expect("waitForIdle resolves");
        *idle_sink.lock().unwrap_or_else(PoisonError::into_inner) = true;
    });
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert!(
        !*idle.lock().unwrap_or_else(PoisonError::into_inner),
        "the waiter stays parked while the operation is open"
    );

    fixture.faux.set_responses(vec![faux_assistant_message(
        "answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let driven = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("drive runs")
        .expect("drive settles");
    assert!(matches!(driven, DriveOutcome::Settled { .. }));
    waiting.await.expect("waiter joins");
    assert!(*idle.lock().unwrap_or_else(PoisonError::into_inner));
}

/// drive-public.test.ts "serializes concurrent runWhenIdle callbacks".
#[tokio::test]
async fn serializes_concurrent_run_when_idle_callbacks() {
    let fixture =
        create_public_fixture("public-idle-serialize", PublicFixtureOptions::default()).await;
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let order: Log = Arc::new(Mutex::new(Vec::new()));
    let order1 = Arc::clone(&order);
    let started1 = Arc::clone(&first_started);
    let gate1 = Arc::clone(&release_first);
    let first_callback: IdleCallback = Arc::new(move |_context| {
        let order = Arc::clone(&order1);
        let started = Arc::clone(&started1);
        let gate = Arc::clone(&gate1);
        Box::pin(async move {
            push(&order, "first:start".to_owned());
            started.notify_one();
            gate.notified().await;
            push(&order, "first:end".to_owned());
            Ok(())
        })
    });
    let lane = Arc::clone(&fixture.lane);
    let first = tokio::spawn(async move {
        Lane::run_when_idle(&lane, first_callback, background_context())
            .await
            .expect("first callback runs")
    });
    first_started.notified().await;
    let order2 = Arc::clone(&order);
    let second_callback: IdleCallback = Arc::new(move |_context| {
        let order = Arc::clone(&order2);
        Box::pin(async move {
            push(&order, "second".to_owned());
            Ok(())
        })
    });
    let lane = Arc::clone(&fixture.lane);
    let second = tokio::spawn(async move {
        Lane::run_when_idle(&lane, second_callback, background_context())
            .await
            .expect("second callback runs")
    });
    tokio::task::yield_now().await;
    assert_eq!(drain(&order), vec!["first:start"]);

    release_first.notify_one();
    let (first_done, second_done) = tokio::join!(first, second);
    first_done.expect("first callback joins");
    second_done.expect("second callback joins");
    assert_eq!(drain(&order), vec!["first:start", "first:end", "second"]);
}

/// drive-public.test.ts "owns the idle window while runWhenIdle executes".
#[tokio::test]
async fn owns_the_idle_window_while_run_when_idle_executes() {
    let fixture =
        create_public_fixture("public-idle-window", PublicFixtureOptions::default()).await;
    let callback_started = Arc::new(tokio::sync::Notify::new());
    let release_callback = Arc::new(tokio::sync::Notify::new());
    let callback: IdleCallback = {
        let started = Arc::clone(&callback_started);
        let release = Arc::clone(&release_callback);
        Arc::new(move |_context| {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                Ok(())
            })
        })
    };
    let lane = Arc::clone(&fixture.lane);
    let callback_task = tokio::spawn(async move {
        Lane::run_when_idle(&lane, callback, background_context())
            .await
            .expect("callback runs")
    });
    callback_started.notified().await;
    let accepted: Arc<Mutex<Option<anyhow::Result<super::OperationAdmissionResult>>>> =
        Arc::new(Mutex::new(None));
    let accepted_sink = Arc::clone(&accepted);
    let lane = Arc::clone(&fixture.lane);
    let acceptance = tokio::spawn(async move {
        let result = AgentLane::accept(
            &*lane,
            &OperationRequest::Prompt {
                operation_id: None,
                payload: PromptPayload::Text {
                    prompt: "after".to_owned(),
                    images: None,
                },
            },
            background_context(),
        )
        .await;
        *accepted_sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
    });
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert!(
        accepted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none(),
        "the idle window blocks acceptance"
    );

    release_callback.notify_one();
    callback_task.await.expect("callback joins");
    acceptance.await.expect("acceptance joins");
    let admission = accepted
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("acceptance resolved")
        .expect("acceptance resolves")
        .expect("admission accepted after the idle window");
    fixture.faux.set_responses(vec![faux_assistant_message(
        "answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let driven = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: admission.operation_id,
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("drive runs")
        .expect("drive settles");
    assert!(matches!(driven, DriveOutcome::Settled { .. }));
}

/// drive-public.test.ts "allows coherent lane reads from an idle callback".
#[tokio::test]
async fn allows_coherent_lane_reads_from_an_idle_callback() {
    let fixture = create_public_fixture("public-idle-reads", PublicFixtureOptions::default()).await;
    let lane = Arc::clone(&fixture.lane);
    let callback: IdleCallback = Arc::new(move |context| {
        let lane = Arc::clone(&lane);
        Box::pin(async move {
            let execution = AgentLane::inspect_execution(&*lane, context.clone()).await?;
            assert!(execution.current.is_none());
            let watch = AgentLane::watch(&*lane, context).await?;
            let snapshot = watch.snapshot();
            assert!(snapshot.operation.is_none());
            watch.unsubscribe();
            Ok(())
        })
    });
    Lane::run_when_idle(&fixture.lane, callback, background_context())
        .await
        .expect("callback runs");
}

/// drive-public.test.ts "releases idle ownership when the callback fails".
#[tokio::test]
async fn releases_idle_ownership_when_the_callback_fails() {
    let fixture =
        create_public_fixture("public-idle-failure", PublicFixtureOptions::default()).await;
    let callback: IdleCallback =
        Arc::new(|_context| Box::pin(async { Err(anyhow::anyhow!("callback failed")) }));

    let error = Lane::run_when_idle(&fixture.lane, callback, background_context())
        .await
        .expect_err("the callback failure propagates");
    assert!(error.to_string().contains("callback failed"), "{error}");
    let admission = AgentLane::accept(
        &*fixture.lane,
        &OperationRequest::Prompt {
            operation_id: None,
            payload: PromptPayload::Text {
                prompt: "after".to_owned(),
                images: None,
            },
        },
        background_context(),
    )
    .await
    .unwrap()
    .expect("the idle ownership was released");
    assert!(!admission.operation_id.is_empty());
}

/// drive-public.test.ts "close waits for an already-running idle callback".
#[tokio::test]
async fn close_waits_for_an_already_running_idle_callback() {
    let fixture = create_public_fixture("public-idle-close", PublicFixtureOptions::default()).await;
    let callback_started = Arc::new(tokio::sync::Notify::new());
    let release_callback = Arc::new(tokio::sync::Notify::new());
    let callback: IdleCallback = {
        let started = Arc::clone(&callback_started);
        let release = Arc::clone(&release_callback);
        Arc::new(move |_context| {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                Ok(())
            })
        })
    };
    let lane = Arc::clone(&fixture.lane);
    let callback_task = tokio::spawn(async move {
        Lane::run_when_idle(&lane, callback, background_context())
            .await
            .expect("callback runs")
    });
    callback_started.notified().await;
    let closed = Arc::new(Mutex::new(false));
    let closed_sink = Arc::clone(&closed);
    // The harness handle moves into the closer task (no Clone on the shell).
    let harness = fixture.harness;
    let closing = tokio::spawn(async move {
        harness
            .close(background_context())
            .await
            .expect("close runs");
        *closed_sink.lock().unwrap_or_else(PoisonError::into_inner) = true;
    });
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert!(
        !*closed.lock().unwrap_or_else(PoisonError::into_inner),
        "close waits for the running idle callback"
    );

    release_callback.notify_one();
    callback_task.await.expect("callback joins");
    closing.await.expect("close joins");
    assert!(*closed.lock().unwrap_or_else(PoisonError::into_inner));
}

/// drive-public.test.ts "installs one pass and joins same-operation callers".
#[tokio::test]
async fn installs_one_pass_and_joins_same_operation_callers() {
    let fixture = create_public_fixture("public-drive-join", PublicFixtureOptions::default()).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fixture.faux.set_responses(vec![gated_response(
        "answer",
        Arc::clone(&started),
        Arc::clone(&release),
    )]);
    accept_run(&fixture.lane, "run").await;

    let lane = Arc::clone(&fixture.lane);
    let first = tokio::spawn(async move {
        Lane::drive(
            &lane,
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("first drive runs")
    });
    started.notified().await;
    let lane = Arc::clone(&fixture.lane);
    let second = tokio::spawn(async move {
        Lane::drive(
            &lane,
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("second drive runs")
    });
    assert_eq!(
        fixture
            .lane
            .active_drive()
            .expect("the pass is installed")
            .operation_id(),
        "run"
    );
    release.notify_one();

    let first_result = first.await.expect("first joins");
    let second_result = second.await.expect("second joins");
    for (label, result) in [("first", first_result), ("second", second_result)] {
        match result.expect("{label} drive settles") {
            DriveOutcome::Settled { outcome } => {
                assert_eq!(outcome.operation_id, "run");
                assert_eq!(outcome.status, TerminalStatus::Completed);
            }
            other => panic!("{label}: expected a settled outcome, got {other:?}"),
        }
    }
    assert_eq!(faux_call_count(&fixture), 1);
    assert!(
        fixture.lane.active_drive().is_none(),
        "the installed pass cleared"
    );
}

/// drive-public.test.ts "returns old result records without disturbing the
/// current operation".
#[tokio::test]
async fn returns_old_result_records_without_disturbing_the_current_operation() {
    let fixture = create_public_fixture("public-drive-old", PublicFixtureOptions::default()).await;
    fixture.faux.set_responses(vec![faux_assistant_message(
        "first",
        FauxMessageOptions::default(),
    )
    .into()]);
    accept_run(&fixture.lane, "first").await;
    let first = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: "first".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .unwrap()
        .unwrap();
    let DriveOutcome::Settled {
        outcome: first_record,
    } = first
    else {
        panic!("expected the first run to settle");
    };
    assert_eq!(first_record.operation_id, "first");
    accept_run(&fixture.lane, "second").await;

    // The stale id returns its stored record without touching the open
    // operation.
    let stale = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: "first".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .unwrap()
        .unwrap();
    let DriveOutcome::Settled {
        outcome: stale_record,
    } = stale
    else {
        panic!("expected the stale id to return its stored record");
    };
    assert_eq!(stale_record, first_record);
    assert_eq!(
        fixture
            .lane
            .state()
            .operation
            .expect("the second operation stays open")
            .meta
            .operation_id,
        "second"
    );
    assert!(fixture.lane.active_drive().is_none());

    fixture.faux.set_responses(vec![faux_assistant_message(
        "second",
        FauxMessageOptions::default(),
    )
    .into()]);
    let second = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: "second".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .unwrap()
        .unwrap();
    let DriveOutcome::Settled {
        outcome: second_record,
    } = second
    else {
        panic!("expected the second run to settle");
    };
    assert_eq!(second_record.operation_id, "second");
}

/// drive-public.test.ts "does not install for a caller already cancelled".
#[tokio::test]
async fn does_not_install_for_a_caller_already_cancelled() {
    let fixture =
        create_public_fixture("public-drive-cancelled", PublicFixtureOptions::default()).await;
    accept_run(&fixture.lane, "run").await;
    let caller_token = tokio_util::sync::CancellationToken::new();
    caller_token.cancel();
    let caller =
        crate::agent_core::harness::context::with_abort_signal(caller_token, background_context());

    let error = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            caller,
        )
        .await
        .expect_err("the cancelled caller is rejected");
    assert!(
        error.to_string().to_lowercase().contains("abort"),
        "{error}"
    );
    assert!(fixture.lane.active_drive().is_none());
    assert_eq!(faux_call_count(&fixture), 0);
}

/// drive-public.test.ts "caller cancellation stops only that caller's
/// observation".
#[tokio::test]
async fn caller_cancellation_stops_only_that_callers_observation() {
    let fixture =
        create_public_fixture("public-drive-observer", PublicFixtureOptions::default()).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fixture.faux.set_responses(vec![gated_response(
        "answer",
        Arc::clone(&started),
        Arc::clone(&release),
    )]);
    accept_run(&fixture.lane, "run").await;
    let lane = Arc::clone(&fixture.lane);
    let owner = tokio::spawn(async move {
        Lane::drive(
            &lane,
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("owner drive runs")
    });
    started.notified().await;
    let observer_token = tokio_util::sync::CancellationToken::new();
    let caller = crate::agent_core::harness::context::with_abort_signal(
        observer_token.clone(),
        background_context(),
    );
    let lane = Arc::clone(&fixture.lane);
    let observer = tokio::spawn(async move {
        // The aborted observation rejects through the outer error channel.
        Lane::drive(
            &lane,
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            caller,
        )
        .await
    });
    observer_token.cancel();

    let observer_error = observer
        .await
        .expect("observer joins")
        .expect_err("the cancelled observer stops");
    assert!(
        observer_error.to_string().to_lowercase().contains("abort"),
        "{observer_error}"
    );
    assert_eq!(
        fixture
            .lane
            .active_drive()
            .expect("the owned pass stays installed")
            .operation_id(),
        "run"
    );
    release.notify_one();
    let owner_result = owner.await.expect("owner joins");
    assert!(matches!(
        owner_result.expect("owner drive settles"),
        DriveOutcome::Settled { .. }
    ));
    assert_eq!(faux_call_count(&fixture), 1);
}

/// drive-public.test.ts "close rejects observation without waiting for a
/// non-cooperative effect".
#[tokio::test]
async fn close_rejects_observation_without_waiting_for_a_non_cooperative_effect() {
    let fixture =
        create_public_fixture("public-drive-close", PublicFixtureOptions::default()).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    fixture.faux.set_responses(vec![gated_response(
        "late",
        Arc::clone(&started),
        Arc::clone(&release),
    )]);
    accept_run(&fixture.lane, "run").await;
    let lane = Arc::clone(&fixture.lane);
    let observation = tokio::spawn(async move {
        // The outer error channel carries the sealed-lane failure (upstream
        // rejects the observation with HarnessClosed).
        Lane::drive(
            &lane,
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
    });
    started.notified().await;

    fixture
        .harness
        .close(background_context())
        .await
        .expect("close resolves while the effect is parked");
    let error = observation
        .await
        .expect("observation joins")
        .expect_err("the closed lane rejects the observation");
    assert!(
        error
            .downcast_ref::<Arc<crate::agent_core::harness::runtime::lane::LaneSealed>>()
            .is_some_and(
                |sealed| sealed.kind == crate::agent_core::harness::runtime::lane::SealKind::Closed
            ),
        "{error}"
    );
    release.notify_one();
    wait_for(|| fixture.lane.active_drive().is_none()).await;
}

/// drive-public.test.ts "exposes durable abort and reconciles through public
/// drive".
#[tokio::test]
async fn exposes_durable_abort_and_reconciles_through_public_drive() {
    let fixture =
        create_public_fixture("public-durable-abort", PublicFixtureOptions::default()).await;
    accept_run(&fixture.lane, "run").await;

    let requested = fixture
        .lane
        .request_abort("run", background_context())
        .await
        .expect("requestAbort runs")
        .expect("abort requested");
    assert!(requested.newly_requested);
    assert!(requested.steer.is_empty());
    assert!(requested.follow_up.is_empty());
    assert!(fixture.lane.active_drive().is_none());
    let driven = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: "run".to_owned(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("drive runs")
        .expect("drive settles");
    match driven {
        DriveOutcome::Settled { outcome } => {
            assert_eq!(outcome.operation_id, "run");
            assert_eq!(outcome.status, TerminalStatus::Aborted);
        }
        other => panic!("expected an aborted settlement, got {other:?}"),
    }
    assert_eq!(faux_call_count(&fixture), 0);
    let again = fixture
        .lane
        .request_abort("run", background_context())
        .await
        .expect("requestAbort runs");
    match again {
        Err(TaggedError::OperationMismatch {
            expected_operation_id,
            current_operation_id,
            last_operation_id,
            ..
        }) => {
            assert_eq!(expected_operation_id, "run");
            assert_eq!(current_operation_id, None);
            assert_eq!(last_operation_id.as_deref(), Some("run"));
        }
        other => panic!("expected an operation mismatch, got {other:?}"),
    }
}

/// The port's seam-proof for the `DeferredCancelFn` default injection
/// (upstream `cancelDeferredBestEffort`, reconcile.ts:17-38): cancelling a
/// deferred-suspended operation through the public drive routes the remote
/// cancel through the lane's `Models` into the provider capability — the
/// faux provider records the cancelled handle. The default environment
/// (the lane-drive `drive_env_for` injection) supplies the real
/// `Models::cancel_deferred` route; no test-local wiring is involved.
#[tokio::test]
async fn reconcile_routes_remote_deferred_cancellation_through_models() {
    let fixture = create_public_fixture(
        "public-cancel-deferred-route",
        PublicFixtureOptions {
            deferred: true,
            ..PublicFixtureOptions::default()
        },
    )
    .await;
    fixture.faux.set_responses(vec![faux_assistant_message(
        "eventual answer",
        FauxMessageOptions::default(),
    )
    .into()]);
    let suspended = match AgentLane::prompt(&*fixture.lane, "defer", None, background_context())
        .await
        .unwrap()
        .expect("prompt accepted")
    {
        RunOutcome::Suspended(suspended) => suspended,
        other => panic!("expected a suspended run, got {other:?}"),
    };
    fixture
        .lane
        .request_abort(&suspended.operation_id, background_context())
        .await
        .expect("requestAbort runs")
        .expect("abort requested");
    let driven = fixture
        .lane
        .drive(
            &DriveOptions {
                operation_id: suspended.operation_id.clone(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        )
        .await
        .expect("drive runs")
        .expect("drive settles");
    match driven {
        DriveOutcome::Settled { outcome } => {
            assert_eq!(outcome.status, TerminalStatus::Aborted);
        }
        other => panic!("expected an aborted settlement, got {other:?}"),
    }
    let cancelled = fixture
        .faux
        .state()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .cancelled_deferred
        .clone();
    assert_eq!(cancelled.len(), 1, "{cancelled:?}");
    assert_eq!(cancelled[0].id, suspended.deferred.id);
    assert_eq!(cancelled[0].provider, "faux");
    assert_eq!(cancelled[0].model_id, "faux-1");
}
