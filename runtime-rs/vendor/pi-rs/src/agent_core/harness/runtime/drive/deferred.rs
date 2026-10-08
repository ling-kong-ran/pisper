//! Port of `drive/deferred.ts` (287 lines): the full deferred poll chain —
//! the deferred-source read with its invariants, the poll preparation
//! (permit, model availability, before_request hook), the durable poll-intent
//! commit with its `run_resume`/`turn_start` events, the gated
//! `streamDeferred` poll consumed through the response lifecycle, and the
//! public `runDeferredSuspended`/`recoverDeferredPoll`/`runDeferred` entry
//! points.
//!
//! Disclosed seams (transport subset of the upstream `DeferredFetchOptions`):
//! - `wait: 0` (`deferred.ts:197`) has no port field on
//!   `ModelsDeferredFetchOptions` (the port keeps the transport-only
//!   subset), so the wait hint is dropped — the upstream value is the
//!   zero-wait default's explicit form.
//! - `telemetryContext` rides on the ported
//!   [`Context`](crate::agent_core::harness::context::Context) envelope.
//! - `onPayload`/`onResponse` map onto the ai layer's request callbacks
//!   (`StreamOptions.callbacks`); the bridge for the faux provider reports
//!   the upstream synthetic 200 through `on_response`, while `on_payload`
//!   stays transport-invoked (faux builds no request payload, matching
//!   upstream faux).

use std::sync::Arc;

use super::publish::{publish_response, PublishOutcome};
use crate::agent_core::harness::runtime::drive::recovery::response_entry_id_of;
use crate::agent_core::harness::runtime::drive::response::{
    publish_configuration_failure, AssistantResponseLifecycle, AssistantResponseMetadata,
    LifecycleObserver,
};
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{DeferredScope, OperationPhase, OperationState};
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{ContinueOperationResult, Lane, OperationCommand};
use crate::agent_core::harness::session::types::{CommitResult, Entry, OperationError, Write};
use crate::agent_core::harness::session::values::{delete_list, pending_assistant_frames};
use crate::agent_core::harness::session::Session as _;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::options::{DeferredFlag, DeferredHandle};

/// Upstream `DeferredLeaf` — either deferred phase.
pub type DeferredLeaf = OperationState;

/// Upstream `PreparedDeferredPoll` (`deferred.ts:32-37`).
#[derive(Debug, Clone)]
pub struct PreparedDeferredPoll {
    pub source: DeferredHandle,
    pub model: crate::ai::types::model::Model,
    pub poll: u64,
    pub stream_options: crate::agent_core::harness::types::AgentHarnessStreamOptions,
}

/// Upstream `DeferredPreparation` (`deferred.ts:39-44`).
#[allow(clippy::large_enum_variant)]
pub enum DeferredPreparation {
    Ready(PreparedDeferredPoll),
    CancelRequested,
    Waiting { source: DeferredHandle },
    ConfigurationFailure,
}

/// Upstream `configurationError` (`deferred.ts:47-54`).
pub fn configuration_error(
    model: &crate::agent_core::harness::session::LaneModel,
) -> OperationError {
    OperationError {
        code: "model_unavailable".to_string(),
        message: "The configured model is unavailable in this process".to_string(),
        details: Some(serde_json::json!({
            "provider": model.provider,
            "modelId": model.model_id,
        })),
    }
}

/// Upstream `readDeferredSourceHandle` (`deferred.ts:57-77`): the assistant
/// entry behind the deferred source, with its handle validated against the
/// deferred configuration identity.
pub async fn read_deferred_source_handle(
    reader: &dyn crate::agent_core::harness::session::SessionMutationReader,
    deferred: &DeferredScope,
    context: crate::agent_core::harness::context::Context,
) -> anyhow::Result<DeferredHandle> {
    let stored = reader
        .get_entries(std::slice::from_ref(&deferred.source_entry_id), context)
        .await?
        .get(&deferred.source_entry_id)
        .cloned();
    let Some(Entry::Message {
        message: crate::agent_core::types::AgentMessage::Assistant(message),
        ..
    }) = stored
    else {
        anyhow::bail!(
            "Deferred source {} is missing its assistant handle",
            deferred.source_entry_id
        );
    };
    if message.stop_reason != StopReasonCheck::Deferred || message.deferred.is_none() {
        anyhow::bail!(
            "Deferred source {} is missing its assistant handle",
            deferred.source_entry_id
        );
    }
    let handle = message.deferred.expect("checked above");
    let identity = &deferred.configuration.model;
    if handle.id.is_empty()
        || handle.provider != identity.provider
        || handle.model_id != identity.model_id
        || handle.api != message.api
    {
        anyhow::bail!(
            "Deferred source {} has an invalid handle",
            deferred.source_entry_id
        );
    }
    Ok(handle)
}

mod stop_reason_check {
    // Marker alias so the invariant above stays readable; StopReason::Deferred.
    pub use crate::ai::types::primitives::StopReason as StopReasonCheck;
}
use stop_reason_check::StopReasonCheck;

/// Upstream `readSourceHandle` (`deferred.ts:80-88`): the read behind the
/// lane's serialized operation line.
pub async fn read_source_handle(
    lane: &Arc<Lane>,
    drive: &Drive,
    deferred: &OperationState,
) -> anyhow::Result<ContinueOperationResult<DeferredHandle>> {
    let context = drive.context().clone();
    let deferred = deferred.clone();
    lane.continue_operation(
        move |_state, _current, _meta, reader| {
            let deferred = deferred.clone();
            let context = context.clone();
            Box::pin(async move {
                let scope = deferred_scope_of(&deferred);
                let handle = read_deferred_source_handle(reader, &scope, context.clone()).await?;
                Ok(OperationCommand::Return { result: handle })
            })
        },
        drive.context().clone(),
    )
    .await
}

/// Upstream `prepareDeferredPoll` (`deferred.ts:90-131`): permit, model and
/// hook-shaped poll preparation without touching the provider.
pub async fn prepare_deferred_poll(
    lane: &Arc<Lane>,
    drive: &Drive,
    expected: &OperationState,
) -> anyhow::Result<DeferredPreparation> {
    let source = match read_source_handle(lane, drive, expected).await? {
        ContinueOperationResult::CancelRequested => {
            return Ok(DeferredPreparation::CancelRequested);
        }
        ContinueOperationResult::Result { value } => value,
    };
    if drive.deferred_permits() == 0 {
        return Ok(DeferredPreparation::Waiting { source });
    }
    let configuration = &expected_configuration(expected);
    let model = lane
        .models()
        .get_model(&configuration.model.provider, &configuration.model.model_id);
    let Some(model) = model else {
        return Ok(DeferredPreparation::ConfigurationFailure);
    };
    let mut base_options = expected_stream_options(expected);
    base_options.deferred = Some(DeferredFlag::Bool(false));
    let poll = match &expected.phase {
        OperationPhase::DeferredSuspended { deferred } => deferred.poll + 1,
        OperationPhase::DeferredEffectPending { deferred, .. } => deferred.poll,
        _ => 0,
    };
    let before_request = lane.hooks().run_with_gate(
        crate::agent_core::harness::hooks::HookInvocation {
            lane: lane.name().to_string(),
            run_id: drive.operation_id().to_string(),
            event: crate::agent_core::harness::hooks::HookEvent::BeforeRequest(
                crate::agent_core::harness::hooks::BeforeRequestEvent {
                    model: model.clone(),
                    step: crate::agent_core::harness::hooks::RequestStep::Deferred,
                    attempt: poll as u32,
                    stream_options: base_options.clone(),
                },
            ),
        },
        Arc::new(drive.gate().clone()),
        drive.context().clone(),
    );
    let before_request = before_request.await?;
    let stream_options = match &before_request {
        crate::agent_core::harness::hooks::HookResult::BeforeRequest(Some(result)) => {
            crate::agent_core::harness::hooks::apply_stream_options_patch(
                base_options,
                &result.stream_options,
            )
        }
        _ => base_options,
    };
    Ok(DeferredPreparation::Ready(PreparedDeferredPoll {
        source,
        model,
        poll,
        stream_options,
    }))
}

fn deferred_scope_of(expected: &OperationState) -> DeferredScope {
    match &expected.phase {
        OperationPhase::DeferredSuspended { deferred } => deferred.clone(),
        OperationPhase::DeferredEffectPending { deferred, .. } => deferred.clone(),
        _ => panic!("deferred preparation requires a deferred phase"),
    }
}

fn expected_configuration(
    expected: &OperationState,
) -> crate::agent_core::harness::session::LaneConfiguration {
    match &expected.phase {
        OperationPhase::DeferredSuspended { deferred } => deferred.configuration.clone(),
        OperationPhase::DeferredEffectPending { deferred, .. } => deferred.configuration.clone(),
        _ => panic!("deferred preparation requires a deferred phase"),
    }
}

fn expected_stream_options(
    expected: &OperationState,
) -> crate::agent_core::harness::types::AgentHarnessStreamOptions {
    match &expected.phase {
        OperationPhase::DeferredSuspended { deferred } => deferred.stream_options.clone(),
        OperationPhase::DeferredEffectPending { deferred, .. } => deferred.stream_options.clone(),
        _ => Default::default(),
    }
}

/// Upstream `publishPollIntent` (`deferred.ts:133-175`): commit the
/// deferred-effect-pending intent under fresh ids, consuming one permit at
/// the successful commit boundary and clearing the prior frames on re-polls.
pub async fn publish_poll_intent(
    lane: &Arc<Lane>,
    drive: &Drive,
    deferred: &OperationState,
    prepared: &PreparedDeferredPoll,
    recovery: bool,
) -> anyhow::Result<ContinueOperationResult<OperationState>> {
    let planner_lane = Arc::clone(lane);
    let operation_id = drive.operation_id().to_string();
    let context = drive.context().clone();
    let at = crate::ai::now_ms();
    let permit_counter = drive.deferred_permit_counter();
    // Upstream clears the frames of the INCOMING poll's response entry on
    // re-polls (`deferred.ts:156-158`: `pendingAssistantFrames(drive.operationId,
    // deferred.responseEntryId)` — the state being replaced, not the fresh ids).
    let (step_id, source_entry_id, configuration, stream_options, stale_response_entry_id, re_poll) =
        match &deferred.phase {
            OperationPhase::DeferredSuspended { deferred } => (
                deferred.step_id.clone(),
                deferred.source_entry_id.clone(),
                deferred.configuration.clone(),
                deferred.stream_options.clone(),
                None,
                false,
            ),
            OperationPhase::DeferredEffectPending {
                deferred,
                response_entry_id,
                ..
            } => (
                deferred.step_id.clone(),
                deferred.source_entry_id.clone(),
                deferred.configuration.clone(),
                deferred.stream_options.clone(),
                Some(response_entry_id.clone()),
                true,
            ),
            _ => anyhow::bail!("publishPollIntent requires a deferred phase"),
        };
    let poll = prepared.poll;
    let response_entry_id = lane.session().id_generator().next(Some(at));
    let next_usage_id = lane.session().id_generator().next(Some(at));
    let stale_frames_entry_id = stale_response_entry_id.clone().unwrap_or_default();
    let next_intent = OperationState {
        scope: deferred.scope.clone(),
        phase: OperationPhase::DeferredEffectPending {
            deferred: DeferredScope {
                step_id: step_id.clone(),
                source_entry_id: source_entry_id.clone(),
                poll,
                configuration: configuration.clone(),
                stream_options: stream_options.clone(),
            },
            response_entry_id: response_entry_id.clone(),
            usage_id: next_usage_id,
        },
    };
    let result = lane
        .continue_operation(
            move |_state, _current, _meta, _reader| {
                let lane = planner_lane.clone();
                let next_intent = next_intent.clone();
                let operation_id = operation_id.clone();
                let stale_frames_entry_id = stale_frames_entry_id.clone();
                let re_poll = re_poll;
                let recovery = recovery;
                let permit_counter = permit_counter.clone();
                Box::pin(async move {
                    let mut writes: Vec<Write> = Vec::new();
                    if re_poll {
                        writes.push(delete_list(&pending_assistant_frames(
                            &operation_id,
                            &stale_frames_entry_id,
                        )));
                    }
                    let lane_name = lane.name().to_string();
                    let run_id = operation_id.clone();
                    let turn_id = turn_id_for(&next_intent);
                    let materialize_intent = next_intent.clone();
                    let materialize_drive = Arc::clone(&permit_counter);
                    Ok(OperationCommand::Commit {
                        writes,
                        operation_state: next_intent,
                        lane: None,
                        materialize: Box::new(move |_commit| {
                            use std::sync::atomic::Ordering;
                            // Consume the permit at the successful commit
                            // boundary; a 0 counter means the upstream
                            // `deferredPermits--` underflow cannot occur.
                            let _ = materialize_drive.fetch_update(
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                                |remaining: usize| remaining.checked_sub(1),
                            );
                            materialize_intent
                        }),
                        events: Some(Box::new(move |_commit: &CommitResult| {
                            Ok(vec![
                                HarnessEvent::RunResume {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    recovery,
                                },
                                HarnessEvent::TurnStart {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    turn_id: turn_id.clone(),
                                    recovery,
                                },
                            ])
                        })),
                    })
                })
            },
            context,
        )
        .await?;
    Ok(match result {
        ContinueOperationResult::CancelRequested => ContinueOperationResult::CancelRequested,
        ContinueOperationResult::Result { value } => ContinueOperationResult::Result { value },
    })
}

fn turn_id_for(intent: &OperationState) -> String {
    match &intent.phase {
        OperationPhase::DeferredEffectPending { deferred, .. } => {
            format!("{}:poll:{}", deferred.step_id, deferred.poll)
        }
        _ => String::new(),
    }
}

/// Upstream `pollDeferred` (`deferred.ts:177-206`) — the poll driver:
/// prepare, publish the intent, perform the gated deferred poll, and publish
/// the settled response.
pub async fn poll_deferred(
    lane: &Arc<Lane>,
    drive: &Drive,
    expected: &OperationState,
    recovery: bool,
) -> anyhow::Result<ProcedureResult> {
    let prepared = prepare_deferred_poll(lane, drive, expected).await?;
    match prepared {
        DeferredPreparation::CancelRequested => Ok(ProcedureResult::Continue),
        DeferredPreparation::Waiting { source } => Ok(ProcedureResult::Waiting {
            outcome: crate::agent_core::harness::runtime::drive_pass::DriveOutcome::Waiting {
                operation_id: drive.operation_id().to_string(),
                reason: crate::agent_core::harness::runtime::drive_pass::WaitingReason::Deferred {
                    deferred: source.clone(),
                },
            },
        }),
        DeferredPreparation::ConfigurationFailure => {
            let model = expected_configuration(expected).model;
            publish_configuration_failure(lane, drive, expected, configuration_error(&model)).await
        }
        DeferredPreparation::Ready(prepared) => {
            let intent = match publish_poll_intent(lane, drive, expected, &prepared, recovery)
                .await?
            {
                ContinueOperationResult::CancelRequested => return Ok(ProcedureResult::Continue),
                ContinueOperationResult::Result { value } => value,
            };
            let response = perform_deferred_poll(lane, drive, &prepared, &intent, recovery).await?;
            match publish_response(lane, drive, &intent, &response, recovery).await? {
                PublishOutcome::Continue => Ok(ProcedureResult::Continue),
                PublishOutcome::Settled { outcome } => {
                    Ok(ProcedureResult::Settled { outcome: *outcome })
                }
            }
        }
    }
}

/// Upstream `runDeferredSuspended` (`deferred.ts:209-213`): poll one durably
/// suspended deferred response when this pass carries a permit.
pub async fn run_deferred_suspended(
    lane: &Arc<Lane>,
    drive: &Drive,
    deferred: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    poll_deferred(lane, drive, deferred, false).await
}

/// Upstream `recoverDeferredPoll` (`deferred.ts:216-220`): replace one
/// orphaned unknown-outcome poll under fresh ids when this pass carries a
/// permit.
pub async fn recover_deferred_poll(
    lane: &Arc<Lane>,
    drive: &Drive,
    deferred: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    poll_deferred(lane, drive, deferred, true).await
}

/// Upstream `runDeferred` (`deferred.ts:223-227`): advance or report the
/// wait for one deferred run phase.
pub async fn run_deferred(
    lane: &Arc<Lane>,
    drive: &Drive,
    deferred: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    if matches!(deferred.phase, OperationPhase::DeferredSuspended { .. }) {
        run_deferred_suspended(lane, drive, deferred).await
    } else {
        recover_deferred_poll(lane, drive, deferred).await
    }
}

/// Upstream `performDeferredPoll` (`deferred.ts:185-229`): one gated
/// deferred-poll stream, consumed through the response lifecycle. The
/// transport options carry the abort signal, timeout/retry limits, headers,
/// and the `onPayload`/`onResponse` hook bridges.
async fn perform_deferred_poll(
    lane: &Arc<Lane>,
    drive: &Drive,
    prepared: &PreparedDeferredPoll,
    intent: &OperationState,
    recovery: bool,
) -> anyhow::Result<AssistantMessage> {
    let intent_response_entry_id = response_entry_id_of(intent).unwrap_or_default();
    let lifecycle = Arc::new(AssistantResponseLifecycle::open(
        Arc::clone(lane),
        drive,
        &intent_response_entry_id,
        recovery,
    ));
    let metadata: Arc<std::sync::Mutex<AssistantResponseMetadata>> =
        Arc::new(std::sync::Mutex::new(AssistantResponseMetadata::default()));
    let admitted_context = crate::agent_core::harness::context::with_abort_signal(
        drive.gate().signal(),
        drive.context().clone(),
    );
    // Upstream `onPayload` (`deferred.ts:204-212`): the before_payload hook
    // behind the gate, with the hook's rewritten payload (or `None`).
    let payload_lane = Arc::clone(lane);
    let payload_gate = Arc::new(drive.gate().clone());
    let payload_run = drive.operation_id().to_string();
    let payload_context = drive.context().clone();
    // Upstream `onResponse` (`deferred.ts:213-215`): capture status/headers
    // before the body is consumed.
    let response_metadata = Arc::clone(&metadata);
    let mut stream_options = crate::ai::types::options::StreamOptions {
        signal: admitted_context.abort_signal(),
        timeout_ms: prepared.stream_options.timeout_ms,
        max_retries: prepared.stream_options.max_retries,
        max_retry_delay_ms: prepared.stream_options.max_retry_delay_ms,
        headers: prepared.stream_options.headers.clone().map(|headers| {
            headers
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .collect()
        }),
        ..Default::default()
    };
    stream_options.callbacks.on_payload = Some(
        Arc::new(
            move |payload: serde_json::Value,
                  model: crate::ai::types::model::Model|
                  -> futures::future::BoxFuture<
                'static,
                anyhow::Result<Option<serde_json::Value>>,
            > {
                let lane = Arc::clone(&payload_lane);
                let gate = Arc::clone(&payload_gate);
                let context = payload_context.clone();
                let run_id = payload_run.clone();
                Box::pin(async move {
                    let result = lane
                        .hooks()
                        .run_with_gate(
                            crate::agent_core::harness::hooks::HookInvocation {
                                lane: lane.name().to_string(),
                                run_id,
                                event: crate::agent_core::harness::hooks::HookEvent::BeforePayload(
                                    crate::agent_core::harness::hooks::BeforePayloadEvent {
                                        payload,
                                        model,
                                    },
                                ),
                            },
                            gate,
                            context,
                        )
                        .await?;
                    Ok(match result {
                        crate::agent_core::harness::hooks::HookResult::BeforePayload(Some(
                            result,
                        )) => Some(result.payload),
                        _ => None,
                    })
                })
            },
        ) as Arc<crate::ai::types::request_callbacks::PayloadHook>,
    );
    stream_options.callbacks.on_response = Some(Arc::new(
        move |response: crate::ai::types::request_callbacks::ProviderResponse, _model| {
            let metadata = Arc::clone(&response_metadata);
            Box::pin(async move {
                *metadata
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    AssistantResponseMetadata {
                        status: Some(response.status),
                        headers: Some(response.headers),
                    };
                Ok(())
            }) as futures::future::BoxFuture<'static, anyhow::Result<()>>
        },
    )
        as Arc<crate::ai::types::request_callbacks::ResponseHook>);
    let model = prepared.model.clone();
    let source = prepared.source.clone();
    let ai_context = crate::ai::transcript::Context {
        system_prompt: None,
        messages: Vec::new(),
        tools: None,
    };
    let stream = drive.gate().admit(|| {
        lane.models().stream_deferred(
            &model,
            &source,
            &ai_context,
            Some(crate::ai::models::ModelsDeferredFetchOptions {
                stream: stream_options,
                transform_headers: None,
            }),
        )
    });
    let stream = match stream {
        Ok(stream) => stream,
        Err(rejection) => anyhow::bail!("{rejection}"),
    };
    let observer: Arc<
        dyn crate::agent_core::harness::execution::assistant::AssistantStreamObserver,
    > = Arc::new(LifecycleObserver(Arc::clone(&lifecycle)));
    let metadata_for_transform = Arc::clone(&metadata);
    let lifecycle_for_transform = Arc::clone(&lifecycle);
    let gate = Arc::new(drive.gate().clone());
    let transform: Arc<crate::agent_core::harness::execution::assistant::SettleTransformFn> =
        Arc::new(move |message, context| {
            let metadata = metadata_for_transform
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let lifecycle = Arc::clone(&lifecycle_for_transform);
            let gate = Arc::clone(&gate);
            Box::pin(async move {
                lifecycle
                    .after_response(&metadata, &message, gate, context)
                    .await
            })
        });
    let settled = crate::agent_core::harness::execution::assistant::consume_assistant_stream(
        stream,
        observer,
        Some(transform),
        drive.context().clone(),
    )
    .await;
    lifecycle.close().await?;
    settled
}

#[cfg(test)]
#[path = "deferred_tests.rs"]
mod tests;
