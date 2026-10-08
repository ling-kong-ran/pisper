//! Behavior tests for the deferred poll chain, ported from
//! `pi/packages/agent/test/harness/runtime/drive-retry-deferred.test.ts`
//! (613 lines, the "runtime deferred polling" describe block) as the
//! behavior authority.
//!
//! Ported substitutions (disclosed):
//! - The upstream `fetchOptions` capture (wrapping `provider.fetchDeferred`
//!   to observe `{wait: 0, signal}`) is not portable: the port's provider
//!   trait has no overridable `fetchDeferred`, and
//!   [`ModelsDeferredFetchOptions`](crate::ai::models::ModelsDeferredFetchOptions)
//!   keeps only the transport subset (`wait: 0` has no port field). The
//!   observable poll behavior (`deferredFetchCount`, durable state, events)
//!   is asserted in full instead.
//! - `gating.waitPending()` becomes a bounded poll loop over
//!   [`GatingStorage::pending`], and vitest's `expect.poll` a sleep loop.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{recover_deferred_poll, run_deferred, run_deferred_suspended};
use crate::agent_core::harness::background_context;
use crate::agent_core::harness::hooks::{
    HookEvent, HookHandler, HookName, HookRegistry, HookResult, RequestStep,
};
use crate::agent_core::harness::runtime::drive::checkpoint::{run_checkpoint, start_run};
use crate::agent_core::harness::runtime::drive::generation::run_generation;
use crate::agent_core::harness::runtime::drive_pass::{
    Drive, DriveOptions, DriveOutcome, ProcedureResult, WaitingReason,
};
use crate::agent_core::harness::runtime::durable::Operation;
use crate::agent_core::harness::runtime::durable::OperationState;
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{
    EmitBatch, Lane, LaneCommand, OperationRequest, RuntimeConfig,
};
use crate::agent_core::harness::runtime::restore::restore_lane;
use crate::agent_core::harness::session::testing::{GatingStorage, InstrumentedStorage};
use crate::agent_core::harness::session::types::{Control, Write};
use crate::agent_core::harness::session::values::{
    append_list, operation_state, pending_assistant_frames, set_value,
};
use crate::agent_core::harness::session::{
    self as session, MemoryStorage, MemoryStorageOptions, Session as _, SessionMetadata,
    SessionReader as _, Storage, StorageBackedSession,
};
use crate::agent_core::harness::types::AgentHarnessStreamOptions;
use crate::agent_core::types::ThinkingLevel;
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, faux_tool_call, FauxContent, FauxDeferredOptions,
    FauxMessageOptions, FauxProviderHandle, FauxProviderOptions, FauxResponseStep, FauxTokenSize,
};
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::ai::types::options::DeferredFlag;
use crate::ai::types::primitives::StopReason;
use serde_json::json;

struct FixtureOptions {
    gated: bool,
    pending_fetches: u32,
    deferred_submission: bool,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            gated: false,
            pending_fetches: 0,
            deferred_submission: true,
        }
    }
}

struct Fixture {
    backend: Arc<MemoryStorage>,
    gating: Option<Arc<GatingStorage>>,
    storage: Arc<InstrumentedStorage>,
    session: Arc<StorageBackedSession>,
    lane: Arc<Lane>,
    drive: Arc<Drive>,
    faux: Arc<FauxProviderHandle>,
    events: Arc<Mutex<Vec<HarnessEvent>>>,
    operation_id: String,
}

async fn create_fixture(options: FixtureOptions) -> Fixture {
    let backend = Arc::new(MemoryStorage::new(MemoryStorageOptions::default()));
    let gating = if options.gated {
        Some(GatingStorage::new(Arc::clone(&backend) as Arc<dyn Storage>))
    } else {
        None
    };
    let storage = InstrumentedStorage::new(match &gating {
        Some(gating) => Arc::clone(gating) as Arc<dyn Storage>,
        None => Arc::clone(&backend) as Arc<dyn Storage>,
    });
    let sess = Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: "retry-deferred-test".into(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        Arc::clone(&storage) as Arc<dyn Storage>,
    ));
    let writes: Vec<Write> = vec![
        set_value(&session::branch_tip("main"), serde_json::Value::Null),
        set_value(
            &session::lane_config("main"),
            session::lane_configuration_value(&session::LaneConfiguration {
                model: session::LaneModel {
                    provider: "faux".to_string(),
                    model_id: "faux-1".to_string(),
                },
                thinking_level: ThinkingLevel::Off,
                active_tool_names: Vec::new(),
            }),
        ),
        set_value(
            &session::lane_state("main"),
            json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
        ),
    ];
    sess.mutate(
        move |reader, context| {
            Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
        },
        background_context(),
    )
    .await
    .expect("baseline commit");

    let faux = Arc::new(faux_provider(FauxProviderOptions {
        api: Some("faux".into()),
        token_size: Some(FauxTokenSize {
            min: Some(1),
            max: Some(1),
        }),
        deferred: Some(FauxDeferredOptions {
            pending_fetches: Some(options.pending_fetches),
            poll_after_ms: None,
        }),
        ..FauxProviderOptions::default()
    }));
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let state = restore_lane(sess.as_ref(), "main", background_context())
        .await
        .expect("lane restores");
    let events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let emit: EmitBatch = Arc::new(move |batch, _context| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(batch);
            Ok(())
        })
    });
    let mut config = RuntimeConfig::default();
    config.stream_options.deferred = Some(DeferredFlag::Bool(options.deferred_submission));
    let lane = Lane::new(
        "main",
        Arc::clone(&sess),
        models,
        HookRegistry::new(Arc::new(
            |_error: anyhow::Error, _hook: HookName, _message: String, _context| Box::pin(async {}),
        )),
        state,
        Arc::new(|error: anyhow::Error| error),
        emit,
        Arc::new(move || config.clone()),
    );
    let admission = lane
        .accept(
            &OperationRequest::Prompt {
                prompt: "question".to_string(),
            },
            background_context(),
        )
        .await
        .expect("accept runs")
        .expect("prompt accepted");
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: admission.operation_id.clone(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    storage.clear_commit_attempts();
    Fixture {
        backend,
        gating,
        storage,
        session: sess,
        lane,
        drive,
        faux,
        events,
        operation_id: admission.operation_id.clone(),
    }
}

fn current_run(fixture: &Fixture) -> OperationState {
    fixture
        .lane
        .state()
        .operation
        .expect("fixture has an operation")
        .state
}

async fn advance_to_ready(fixture: &Fixture) {
    let starting = current_run(fixture);
    assert_eq!(
        starting.at(),
        "starting",
        "run starts at its initial boundary"
    );
    start_run(&fixture.lane, &fixture.drive, &starting)
        .await
        .expect("start run");
    let run = current_run(fixture);
    assert_eq!(run.at(), "checkpoint", "run reaches checkpoint");
    run_checkpoint(&fixture.lane, &fixture.drive, &run)
        .await
        .expect("run checkpoint");
    assert_eq!(current_run(fixture).at(), "assistant.ready");
}

async fn submit_deferred(
    fixture: &Fixture,
    response: crate::ai::types::message::AssistantMessage,
) -> OperationState {
    fixture
        .faux
        .set_responses(vec![FauxResponseStep::Message(Box::new(response))]);
    advance_to_ready(fixture).await;
    run_generation(&fixture.lane, &fixture.drive, &current_run(fixture))
        .await
        .expect("generation runs");
    let run = current_run(fixture);
    assert_eq!(run.at(), "deferred.suspended", "initial request suspends");
    run
}

async fn replace_run_state(fixture: &Fixture, next_state: OperationState, extra: Vec<Write>) {
    let lane = Arc::clone(&fixture.lane);
    let operation_id = fixture.operation_id.clone();
    lane.command(
        move |state, _reader| {
            let next_state = next_state.clone();
            let extra = extra.clone();
            let operation_id = operation_id.clone();
            Box::pin(async move {
                let mut next = state.clone();
                let operation = next.operation.clone().expect("operation installed");
                next.operation = Some(Operation {
                    meta: operation.meta,
                    state: next_state.clone(),
                });
                // Upstream replaceRunState persists the replaced state
                // (`storedValues.setValue(storedValues.operationState(...))`).
                let mut writes = vec![set_value(
                    &operation_state(&operation_id),
                    serde_json::to_value(&next_state)?,
                )];
                writes.extend(extra);
                Ok(LaneCommand::Commit {
                    writes,
                    next,
                    materialize: Box::new(|_commit: &session::CommitResult| ()),
                    events: None,
                })
            })
        },
        background_context(),
    )
    .await
    .expect("run state replaced");
}

async fn install_unknown_poll(fixture: &Fixture, suspended: &OperationState) -> OperationState {
    let crate::agent_core::harness::runtime::durable::OperationPhase::DeferredSuspended {
        deferred,
    } = &suspended.phase
    else {
        panic!("suspended state expected");
    };
    let response_entry_id = fixture.session.id_generator().next(None);
    let usage_id = fixture.session.id_generator().next(None);
    let effect_pending = OperationState {
        scope: suspended.scope.clone(),
        phase:
            crate::agent_core::harness::runtime::durable::OperationPhase::DeferredEffectPending {
                deferred: deferred.clone(),
                response_entry_id: response_entry_id.clone(),
                usage_id: usage_id.clone(),
            },
    };
    replace_run_state(
        fixture,
        effect_pending.clone(),
        vec![append_list(
            &pending_assistant_frames(&fixture.operation_id, &response_entry_id),
            json!({"type": "text_delta", "contentIndex": 0, "delta": "old"}),
        )],
    )
    .await;
    effect_pending
}

fn poll_drive(operation_id: &str) -> Arc<Drive> {
    Arc::new(Drive::new(
        &DriveOptions {
            operation_id: operation_id.to_string(),
            wait_for_retry: None,
            poll_deferred: Some(true),
        },
        background_context(),
    ))
}

fn no_permit_drive(operation_id: &str) -> Arc<Drive> {
    Arc::new(Drive::new(
        &DriveOptions {
            operation_id: operation_id.to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ))
}

async fn expect_projection_restores(fixture: &Fixture) {
    let restored = restore_lane(fixture.session.as_ref(), "main", background_context())
        .await
        .expect("projection restores");
    assert_eq!(restored, fixture.lane.state());
}

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    for _ in 0..5_000 {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("condition was not reached");
}

fn event_type(event: &HarnessEvent) -> &'static str {
    match event {
        HarnessEvent::TurnEnd { .. } => "turn_end",
        HarnessEvent::RunSuspend { .. } => "run_suspend",
        HarnessEvent::Usage { .. } => "usage",
        HarnessEvent::RunEnd { .. } => "run_end",
        _ => "other",
    }
}

fn write_shape(write: &Write) -> String {
    match write {
        Write::Value(session::ValueWrite::Set { namespace, .. }) => {
            format!("value:set:{namespace}")
        }
        Write::Value(session::ValueWrite::Delete { namespace, .. }) => {
            format!("value:delete:{namespace}")
        }
        Write::List(session::ListWrite::Append { namespace, .. }) => {
            format!("list:append:{namespace}")
        }
        Write::List(session::ListWrite::Delete { namespace, .. }) => {
            format!("list:delete:{namespace}")
        }
        Write::Entry { .. } => "entry".to_string(),
        Write::Usage { .. } => "usage".to_string(),
    }
}

/// Upstream `submitDeferred` uses `fauxAssistantMessage("done", {timestamp: 20})`.
fn done_message() -> crate::ai::types::message::AssistantMessage {
    faux_assistant_message(
        "done",
        FauxMessageOptions {
            timestamp: Some(20),
            ..FauxMessageOptions::default()
        },
    )
}

#[tokio::test]
async fn waits_without_a_permit_then_performs_at_most_one_pending_poll() {
    let fixture = create_fixture(FixtureOptions {
        pending_fetches: 1,
        ..FixtureOptions::default()
    })
    .await;
    let suspended = submit_deferred(&fixture, done_message()).await;
    fixture.storage.clear_commit_attempts();

    let no_permit = no_permit_drive(&fixture.operation_id);
    let result = run_deferred_suspended(&fixture.lane, &no_permit, &suspended)
        .await
        .expect("first poll");
    let ProcedureResult::Waiting { outcome } = result else {
        panic!("poll without a permit waits");
    };
    let DriveOutcome::Waiting {
        operation_id,
        reason: WaitingReason::Deferred { deferred },
    } = outcome
    else {
        panic!("waiting outcome reports the deferred reason");
    };
    assert_eq!(operation_id, fixture.operation_id.clone());
    assert!(deferred.id.starts_with("deferred"));
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 0);
    assert!(fixture.storage.get_commit_attempts().is_empty());

    // before_request captures the deferred-step stream options.
    let captured: Arc<Mutex<Option<AgentHarnessStreamOptions>>> = Arc::new(Mutex::new(None));
    let captured_for_hook = Arc::clone(&captured);
    let handler: HookHandler = Arc::new(move |invocation, _context| {
        let captured = Arc::clone(&captured_for_hook);
        Box::pin(async move {
            if let HookEvent::BeforeRequest(event) = &invocation.event {
                if matches!(event.step, RequestStep::Deferred) {
                    *captured
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(event.stream_options.clone());
                }
            }
            Ok(HookResult::BeforeRequest(None))
        })
    });
    fixture
        .lane
        .hooks()
        .on(HookName::BeforeRequest, handler, None)
        .expect("hook registers");

    let drive = poll_drive(&fixture.operation_id);
    let current = current_run(&fixture);
    let result = run_deferred(&fixture.lane, &drive, &current)
        .await
        .expect("permitted poll");
    assert!(matches!(result, ProcedureResult::Continue));
    assert_eq!(drive.deferred_permits(), 0);
    let captured_options = captured
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .expect("deferred before_request hook ran");
    assert_eq!(captured_options.deferred, Some(DeferredFlag::Bool(false)));
    let resumed = current_run(&fixture);
    assert_eq!(resumed.at(), "deferred.suspended");
    let crate::agent_core::harness::runtime::durable::OperationPhase::DeferredSuspended {
        deferred,
    } = &resumed.phase
    else {
        panic!("suspended state expected");
    };
    assert_eq!(deferred.poll, 1);
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 1);
    let events = fixture
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let last_two: Vec<&'static str> = events.iter().rev().take(2).rev().map(event_type).collect();
    assert_eq!(last_two, ["turn_end", "run_suspend"]);

    // The settled poll suspended again; the next pass waits (the permit was
    // consumed and the drive is not re-armed).
    let current = current_run(&fixture);
    let result = run_deferred(&fixture.lane, &drive, &current)
        .await
        .expect("next poll");
    let ProcedureResult::Waiting { outcome } = result else {
        panic!("spent permit waits");
    };
    let DriveOutcome::Waiting {
        reason: WaitingReason::Deferred { .. },
        ..
    } = outcome
    else {
        panic!("waiting outcome reports the deferred reason");
    };
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 1);
    expect_projection_restores(&fixture).await;
}

#[tokio::test]
async fn declines_poll_intent_when_cancellation_wins_preparation() {
    let fixture = create_fixture(FixtureOptions::default()).await;
    let suspended = submit_deferred(&fixture, done_message()).await;
    let drive = poll_drive(&fixture.operation_id);

    let hook_started = Arc::new(tokio::sync::Notify::new());
    let release_hook = Arc::new(tokio::sync::Notify::new());
    let started = Arc::clone(&hook_started);
    let release = Arc::clone(&release_hook);
    let handler: HookHandler = Arc::new(move |invocation, _context| {
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        Box::pin(async move {
            if let HookEvent::BeforeRequest(event) = &invocation.event {
                if matches!(event.step, RequestStep::Deferred) {
                    started.notify_one();
                    release.notified().await;
                }
            }
            Ok(HookResult::BeforeRequest(None))
        })
    });
    fixture
        .lane
        .hooks()
        .on(HookName::BeforeRequest, handler, None)
        .expect("hook registers");

    let lane = Arc::clone(&fixture.lane);
    let drive_for_poll = Arc::clone(&drive);
    let polling =
        tokio::spawn(
            async move { run_deferred_suspended(&lane, &drive_for_poll, &suspended).await },
        );
    hook_started.notified().await;

    // Cancel while the poll is parked in the before_request hook.
    let mut cancelled = current_run(&fixture);
    cancelled.scope.control = Control::CancelRequested { requested_at: 10 };
    let lane_for_cancel = Arc::clone(&fixture.lane);
    let write_state = cancelled.clone();
    let cancelled_id = fixture.operation_id.clone();
    lane_for_cancel
        .command(
            move |state, _reader| {
                let write_state = write_state.clone();
                let operation_id = cancelled_id.clone();
                Box::pin(async move {
                    let mut next = state.clone();
                    let operation = next.operation.clone().expect("operation installed");
                    next.operation = Some(Operation {
                        meta: operation.meta,
                        state: write_state.clone(),
                    });
                    Ok(LaneCommand::Commit {
                        writes: vec![set_value(
                            &operation_state(&operation_id),
                            serde_json::to_value(&write_state).expect("state serializes"),
                        )],
                        next,
                        materialize: Box::new(|_commit: &session::CommitResult| ()),
                        events: None,
                    })
                })
            },
            background_context(),
        )
        .await
        .expect("cancellation commits");
    release_hook.notify_one();

    let result = polling
        .await
        .expect("polling joins")
        .expect("poll declines");
    assert!(matches!(result, ProcedureResult::Continue));
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 0);
    let run = current_run(&fixture);
    assert_eq!(run.at(), "deferred.suspended");
    assert!(matches!(run.scope.control, Control::CancelRequested { .. }));
}

#[tokio::test]
async fn plans_ready_deferred_tool_calls_with_the_poll_turn_identity() {
    let fixture = create_fixture(FixtureOptions::default()).await;
    let suspended = submit_deferred(
        &fixture,
        faux_assistant_message(
            FauxContent::Block(faux_tool_call(
                "lookup",
                json!({"query": "value"}),
                crate::ai::models::faux::FauxToolCallOptions::default(),
            )),
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                timestamp: Some(20),
                ..FauxMessageOptions::default()
            },
        ),
    )
    .await;
    let drive = poll_drive(&fixture.operation_id);

    let result = run_deferred_suspended(&fixture.lane, &drive, &suspended)
        .await
        .expect("poll runs");
    assert!(matches!(result, ProcedureResult::Continue));
    let run = current_run(&fixture);
    assert_eq!(run.at(), "tools", "deferred tool response creates a batch");
    let crate::agent_core::harness::runtime::durable::OperationPhase::Tools { batch } = &run.phase
    else {
        panic!("tools phase expected");
    };
    // Upstream: `turnId: \`${suspended.stepId}:poll:1\`` — the step id is the
    // one the real generation flow minted.
    let suspended_step_id = match &suspended.phase {
        crate::agent_core::harness::runtime::durable::OperationPhase::DeferredSuspended {
            deferred,
        } => deferred.step_id.clone(),
        _ => panic!("suspended state expected"),
    };
    assert_eq!(batch.turn_id, format!("{suspended_step_id}:poll:1"));
    assert_eq!(batch.calls.len(), 1);
    assert_eq!(batch.calls[0].source_index, 0);
    assert!(matches!(
        batch.calls[0].state,
        crate::agent_core::harness::runtime::durable::ToolCallState::Planned
    ));
    let latest = run
        .scope
        .latest_assistant_entry_id
        .as_deref()
        .expect("latest assistant");
    assert_eq!(
        &batch.calls[0].result_entry_id[..13],
        &latest[..13],
        "follower result id shares the assistant timestamp segment"
    );
    let events = fixture
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(events.last().map(event_type), Some("usage"));
    expect_projection_restores(&fixture).await;
}

#[tokio::test]
async fn consumes_its_permit_only_after_the_fresh_intent_commit_lands() {
    let fixture = create_fixture(FixtureOptions {
        gated: true,
        ..FixtureOptions::default()
    })
    .await;
    let suspended = submit_deferred(
        &fixture,
        faux_assistant_message(
            "",
            FauxMessageOptions {
                stop_reason: Some(StopReason::Error),
                error_message: Some("failed".to_string()),
                timestamp: Some(20),
                ..FauxMessageOptions::default()
            },
        ),
    )
    .await;
    let gating = fixture.gating.as_ref().expect("fixture is gated").clone();
    let drive = poll_drive(&fixture.operation_id);
    gating.arm();
    let lane = Arc::clone(&fixture.lane);
    let drive_for_poll = Arc::clone(&drive);
    let polling =
        tokio::spawn(
            async move { run_deferred_suspended(&lane, &drive_for_poll, &suspended).await },
        );

    wait_for(|| gating.pending() > 0).await;
    assert_eq!(drive.deferred_permits(), 1);
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 0);
    gating.next(1).await.expect("release the intent commit");
    assert_eq!(drive.deferred_permits(), 0);
    wait_for(|| fixture.faux.state().lock().unwrap().deferred_fetch_count == 1).await;
    // Release the remaining response commits until the settlement lands
    // (upstream `gating.next(2)` counts the ported response choreography;
    // the release loop keeps the assertion order-free across the response
    // lifecycle's commit granularity).
    for _ in 0..8 {
        if polling.is_finished() {
            break;
        }
        if gating.pending() > 0 {
            gating.next(1).await.expect("release the response commits");
        } else {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    let result = polling.await.expect("polling joins").expect("poll settles");
    let ProcedureResult::Settled { outcome } = result else {
        panic!("error settlement settles the run");
    };
    assert_eq!(outcome.operation_id, fixture.operation_id.clone());
    assert_eq!(outcome.kind, "run");
    assert_eq!(
        outcome.status,
        crate::agent_core::harness::session::types::TerminalStatus::Failed
    );
    assert!(fixture.lane.state().operation.is_none());
}

#[tokio::test]
async fn leaves_suspended_state_unchanged_when_the_fresh_intent_is_discarded() {
    let fixture = create_fixture(FixtureOptions {
        gated: true,
        ..FixtureOptions::default()
    })
    .await;
    let suspended = submit_deferred(&fixture, done_message()).await;
    let gating = fixture.gating.as_ref().expect("fixture is gated").clone();
    let drive = poll_drive(&fixture.operation_id);
    gating.arm();
    let lane = Arc::clone(&fixture.lane);
    let drive_for_poll = Arc::clone(&drive);
    let polling = tokio::spawn(async move {
        let result = run_deferred_suspended(&lane, &drive_for_poll, &suspended).await;
        result
    });

    wait_for(|| gating.pending() > 0).await;
    gating.discard();
    let result = polling.await.expect("polling joins");
    let error = result.expect_err("discarded commit rejects");
    assert!(error.to_string().contains("storage discarded"));
    assert_eq!(drive.deferred_permits(), 1);
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 0);
    let durable = fixture
        .backend
        .get_value(
            &operation_state(&fixture.operation_id),
            background_context(),
        )
        .await
        .expect("backend read")
        .expect("durable operation state");
    let durable_state: OperationState =
        serde_json::from_value(durable.value).expect("durable state parses");
    assert_eq!(durable_state.at(), "deferred.suspended");
    let crate::agent_core::harness::runtime::durable::OperationPhase::DeferredSuspended {
        deferred,
    } = &durable_state.phase
    else {
        panic!("suspended state expected");
    };
    assert_eq!(deferred.poll, 0);
}

#[tokio::test]
async fn replaces_an_unknown_poll_under_fresh_ids_at_the_same_poll_number() {
    let fixture = create_fixture(FixtureOptions::default()).await;
    let suspended = submit_deferred(&fixture, done_message()).await;
    let unknown = install_unknown_poll(&fixture, &suspended).await;
    let crate::agent_core::harness::runtime::durable::OperationPhase::DeferredEffectPending {
        deferred: _,
        response_entry_id: unknown_response,
        usage_id: unknown_usage,
    } = &unknown.phase
    else {
        panic!("effect pending expected");
    };
    let unknown_response = unknown_response.clone();
    let unknown_usage = unknown_usage.clone();
    fixture.storage.clear_commit_attempts();

    let no_permit = no_permit_drive(&fixture.operation_id);
    let result = recover_deferred_poll(&fixture.lane, &no_permit, &unknown)
        .await
        .expect("recovery without a permit");
    let ProcedureResult::Waiting { outcome } = result else {
        panic!("recovery without a permit waits");
    };
    let DriveOutcome::Waiting {
        reason: WaitingReason::Deferred { .. },
        ..
    } = outcome
    else {
        panic!("waiting outcome reports the deferred reason");
    };
    let frames = fixture
        .session
        .read_list(
            &pending_assistant_frames(&fixture.operation_id, &unknown_response),
            None,
            background_context(),
        )
        .await
        .expect("frames read");
    assert_eq!(frames.len(), 1);
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 0);

    let replacement = poll_drive(&fixture.operation_id);
    let result = recover_deferred_poll(&fixture.lane, &replacement, &unknown)
        .await
        .expect("recovery with a permit");
    assert!(matches!(result, ProcedureResult::Continue));
    let attempts = fixture.storage.get_commit_attempts();
    let intent = attempts
        .iter()
        .find(|attempt| {
            attempt.iter().any(|write| {
                matches!(
                    write,
                    Write::List(session::ListWrite::Delete { namespace, key })
                        if namespace == "pi.pending.assistant_frame"
                            && key.ends_with(&unknown_response)
                )
            })
        })
        .expect("fresh intent commit recorded");
    let shapes: Vec<String> = intent.iter().map(write_shape).collect();
    assert_eq!(
        shapes,
        [
            "list:delete:pi.pending.assistant_frame",
            "value:set:pi.op.state"
        ]
    );
    let intent_state = intent
        .iter()
        .find_map(|write| match write {
            Write::Value(session::ValueWrite::Set {
                namespace, value, ..
            }) if namespace == "pi.op.state" => Some(value.clone()),
            _ => None,
        })
        .expect("operation state write");
    assert_eq!(intent_state["at"], "deferred.effect_pending");
    assert_eq!(
        intent_state["poll"],
        serde_json::to_value(&unknown).unwrap()["poll"]
    );
    assert_ne!(
        intent_state["responseEntryId"]
            .as_str()
            .expect("response id"),
        unknown_response,
        "fresh response id"
    );
    assert_ne!(
        intent_state["usageId"].as_str().expect("usage id"),
        unknown_usage,
        "fresh usage id"
    );
    assert!(
        fixture
            .session
            .get_entry(&unknown_response, background_context())
            .await
            .expect("entry read")
            .is_none(),
        "unknown response entry abandoned"
    );
    let usage_rows = fixture
        .storage
        .scan_usage(&session::UsageScan::default(), background_context())
        .await
        .expect("usage scan");
    assert!(
        !usage_rows.iter().any(|row| row.id == unknown_usage),
        "unknown usage abandoned"
    );
    let frames = fixture
        .session
        .read_list(
            &pending_assistant_frames(&fixture.operation_id, &unknown_response),
            None,
            background_context(),
        )
        .await
        .expect("frames read");
    assert!(frames.is_empty(), "old frames deleted");
    assert_eq!(current_run(&fixture).at(), "checkpoint");
    let events = fixture
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        events.iter().any(|event| matches!(
            event,
            HarnessEvent::RunResume { recovery: true, .. }
                | HarnessEvent::TurnStart { recovery: true, .. }
        )),
        "recovery flagged on the resume events"
    );
    expect_projection_restores(&fixture).await;
}

#[tokio::test]
async fn abandons_unknown_ids_and_frames_into_configuration_failure() {
    let fixture = create_fixture(FixtureOptions::default()).await;
    let suspended = submit_deferred(&fixture, done_message()).await;
    let unknown = install_unknown_poll(&fixture, &suspended).await;
    let crate::agent_core::harness::runtime::durable::OperationPhase::DeferredEffectPending {
        ..
    } = &unknown.phase
    else {
        panic!("effect pending expected");
    };
    fixture.storage.clear_commit_attempts();

    // Upstream `fixture.models.deleteProvider(fixture.faux.provider.id)` —
    // the port's Models is owned by the lane with no mutation surface, so
    // the same "model no longer resolves" state is reached by rebuilding
    // the lane over the same session with an empty provider registry.
    let models = create_models(CreateModelsOptions::default());
    let restored = restore_lane(fixture.session.as_ref(), "main", background_context())
        .await
        .expect("projection restores");
    let events = Arc::clone(&fixture.events);
    let emit: EmitBatch = Arc::new(move |batch, _context| {
        let events = Arc::clone(&events);
        Box::pin(async move {
            events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(batch);
            Ok(())
        })
    });
    let lane = Lane::new(
        "main",
        Arc::clone(&fixture.session),
        models,
        HookRegistry::new(Arc::new(
            |_error: anyhow::Error, _hook: HookName, _message: String, _context| Box::pin(async {}),
        )),
        restored,
        Arc::new(|error: anyhow::Error| error),
        emit,
        Arc::new(RuntimeConfig::default),
    );
    let drive = poll_drive(&fixture.operation_id);

    let result = recover_deferred_poll(&lane, &drive, &unknown)
        .await
        .expect("configuration failure settles");
    let ProcedureResult::Settled { outcome } = result else {
        panic!("configuration failure settles the run");
    };
    assert_eq!(outcome.operation_id, fixture.operation_id.clone());
    assert_eq!(outcome.kind, "run");
    assert_eq!(
        outcome.status,
        crate::agent_core::harness::session::types::TerminalStatus::Failed
    );
    let error = outcome.error.as_ref().expect("error recorded");
    assert_eq!(error.code, "model_unavailable");
    assert_eq!(drive.deferred_permits(), 1);
    assert_eq!(fixture.faux.state().lock().unwrap().deferred_fetch_count, 0);
    assert!(lane.state().operation.is_none(), "operation cleared");
    let attempts = fixture.storage.get_commit_attempts();
    let shapes: Vec<String> = attempts
        .last()
        .expect("terminal commit recorded")
        .iter()
        .map(write_shape)
        .collect();
    assert_eq!(
        shapes,
        [
            "value:delete:pi.op.meta",
            "value:delete:pi.op.state",
            "list:delete:pi.pending.assistant_frame",
            "value:set:pi.result",
            "value:set:pi.lane.state",
        ]
    );
    assert!(
        !attempts
            .iter()
            .flatten()
            .any(|write| matches!(write, Write::Entry { .. } | Write::Usage { .. })),
        "no entry or usage writes on abandonment"
    );
    // The reconciliation ran on the rebuilt lane; compare against it.
    let restored = restore_lane(fixture.session.as_ref(), "main", background_context())
        .await
        .expect("projection restores");
    assert_eq!(restored, lane.state());
}

/// The faux deferred bridge: `Models::stream_deferred` reaches the faux
/// deferred surface, and an unknown handle yields the upstream error
/// (faux.ts:588 `Unknown faux deferred response: {id}`) as a setup stream.
#[tokio::test]
async fn stream_deferred_bridge_reaches_the_faux_deferred_surface() {
    let faux = faux_provider(FauxProviderOptions {
        api: Some("faux".into()),
        ..FauxProviderOptions::default()
    });
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = models.get_model("faux", "faux-1").expect("faux model");
    let unknown: crate::ai::types::options::DeferredHandle = serde_json::from_value(json!({
        "provider": "faux", "modelId": "faux-1", "api": "faux", "id": "missing-job"
    }))
    .unwrap();
    let context = crate::ai::transcript::Context {
        system_prompt: None,
        messages: Vec::new(),
        tools: None,
    };
    let mut rx = models.stream_deferred(&model, &unknown, &context, None);
    let first = rx.recv().await.expect("error event");
    let crate::ai::types::events::AssistantMessageEvent::Error { error, .. } = first else {
        panic!("unknown handle yields the upstream error event");
    };
    assert!(error
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("Unknown faux deferred response: missing-job"));
}

/// Serialization-seam oracle test whose expected JSON strings were captured
/// by running the upstream pure functions (`configurationError` from
/// deferred.ts:47-54, the `pollDeferred` waiting-outcome literal, the
/// `publishPollIntent` turn-id composition, and the `DeferredHandle` wire
/// shape) with node `--experimental-strip-types`
/// (`tests/fixtures/deferred_oracle/deferred_oracle.ts`, timestamp-free seams).
#[test]
fn serialization_seams_match_the_upstream_node_oracle() {
    let expected = [
        // configurationError({provider:"faux",modelId:"faux-1"})
        r#"{"code":"model_unavailable","message":"The configured model is unavailable in this process","details":{"provider":"faux","modelId":"faux-1"}}"#,
        // pollDeferred's waiting outcome literal
        r#"{"kind":"waiting","operationId":"01950000-0000-7000-8000-000000000001","reason":"deferred","deferred":{"provider":"faux","modelId":"faux-1","api":"faux","id":"deferred-job"}}"#,
        // publishPollIntent's turn id composition
        "step:poll:1",
        // DeferredHandle wire shape
        r#"{"provider":"faux","modelId":"faux-1","api":"faux","id":"deferred-job"}"#,
    ];
    let captured = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/deferred_oracle/oracle_output.txt"
    ))
    .expect("oracle capture");
    let lines: Vec<&str> = captured.lines().map(str::trim_end).collect();
    assert_eq!(lines, expected, "oracle capture drifted");

    // Compare the complete wire bytes, including upstream details insertion order.
    let error = super::configuration_error(&session::LaneModel {
        provider: "faux".to_string(),
        model_id: "faux-1".to_string(),
    });
    let error_bytes = serde_json::to_string(&error).expect("error serializes");
    assert_eq!(
        error_bytes, lines[0],
        "configurationError wire matches upstream"
    );

    // The waiting outcome byte comparison.
    let handle: crate::ai::types::options::DeferredHandle = serde_json::from_value(json!({
        "provider": "faux", "modelId": "faux-1", "api": "faux", "id": "deferred-job"
    }))
    .unwrap();
    let outcome = DriveOutcome::Waiting {
        operation_id: "01950000-0000-7000-8000-000000000001".to_string(),
        reason: WaitingReason::Deferred {
            deferred: handle.clone(),
        },
    };
    assert_eq!(
        serde_json::to_string(&outcome).expect("outcome serializes"),
        lines[1]
    );

    // The deferred handle byte comparison.
    assert_eq!(
        serde_json::to_string(&handle).expect("handle serializes"),
        lines[3]
    );
    // The turn id composition is byte-asserted end to end by
    // `plans_ready_deferred_tool_calls_with_the_poll_turn_identity`
    // (lines[2] == "step:poll:1" through the published batch).
    assert_eq!(lines[2], "step:poll:1");
}
