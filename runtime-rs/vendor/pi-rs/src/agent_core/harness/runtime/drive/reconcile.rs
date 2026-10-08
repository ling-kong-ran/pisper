//! Port of `drive/reconcile.ts` (173 lines): advance one cancelled durable
//! leaf without starting new ordinary work — effect recovery, the tools
//! branch, deferred remote-cancel best effort, and aborted terminal
//! publication per intent kind.
//!
//! Disclosed seams:
//! - **Remote deferred cancellation ([`DeferredCancelFn`]).** Upstream calls
//!   `lane.models.cancelDeferred(model, handle, options)`, which throws the
//!   upstream `ModelsError("provider", ...)` for providers without the
//!   capability and is swallowed by `cancelDeferredBestEffort`'s try/catch.
//!   The ported ai layer now carries the real capability
//!   ([`Models::cancel_deferred`](crate::ai::models::Models::cancel_deferred));
//!   the capability stays an injected [`DeferredCancelFn`] so tests can wire
//!   recording/throwing stand-ins, and both default injection points (the
//!   lane-drive `drive_env_for` and [`DriveEnv::for_lane`]) supply the real
//!   `Models::cancel_deferred` route. Every provider without the capability
//!   errors-and-is-swallowed exactly like upstream.
//! - **Tool events.** The tools branch delegates to [`run_tools`], whose
//!   `ToolEventEmit` seam (upstream `tool_start`/`tool_update`/`tool_end`
//!   HarnessEvent members, not yet in the ported event union) is threaded
//!   through from [`reconcile_operation`]'s caller.
//! - **Telemetry context.** `getTelemetryContext(...)` rides on the ported
//!   [`Context`](crate::agent_core::harness::context::Context) envelope; no
//!   separate option field exists on the injected cancel request.

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::agent_core::harness::runtime::drive::recovery::recover_cancelled_assistant_effect;
use crate::agent_core::harness::runtime::drive::terminal::{
    operation_cleanup_writes, operation_result_record,
};
use crate::agent_core::harness::runtime::drive::tools::{
    run_tools, ToolContextSource, ToolEventEmit,
};
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{
    OperationIntent, OperationPhase, OperationState, ResultBoundary, SummaryReason,
};
use crate::agent_core::harness::runtime::events::{
    CompactionEndStatus, HarnessEvent, RunEndStatus,
};
use crate::agent_core::harness::runtime::lane::{Lane, LanePatch, OperationCommand};
use crate::agent_core::harness::session::types::CommitResult;
use crate::agent_core::harness::session::types::TerminalStatus;
use crate::agent_core::harness::session::Control;
use crate::ai::types::options::DeferredHandle;

/// Upstream `DeferredCancelOptions` subset (`deferred.ts:27-33`): everything
/// `cancelDeferredBestEffort` forwards to `lane.models.cancelDeferred`. The
/// telemetry context rides on `context`.
pub struct DeferredCancelRequest {
    pub model: crate::ai::types::model::Model,
    pub handle: DeferredHandle,
    /// Upstream `signal: drive.closeSignal` — the close signal, distinct
    /// from the abort gate.
    pub signal: CancellationToken,
    pub context: crate::agent_core::harness::context::Context,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub headers: Option<std::collections::BTreeMap<String, String>>,
}

/// Injected `Models.cancelDeferred` capability (`models.ts:743-755`): the
/// future ai-layer surface; see the module docs for why this is a seam.
pub type DeferredCancelFn =
    dyn Fn(DeferredCancelRequest) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync;

/// Upstream `cancelDeferredBestEffort` (`reconcile.ts:17-38`): resolve the
/// model, cancel remotely, and swallow every failure — remote cancellation
/// is best-effort; durable local reconciliation must continue.
async fn cancel_deferred_best_effort(
    lane: &Arc<Lane>,
    drive: &Drive,
    deferred: &OperationState,
    handle: &DeferredHandle,
    cancel: &DeferredCancelFn,
) {
    let identity = &deferred_configuration_of(deferred).model;
    let Some(model) = lane
        .models()
        .get_model(&identity.provider, &identity.model_id)
    else {
        return;
    };
    let stream_options = deferred_stream_options_of(deferred);
    let request = DeferredCancelRequest {
        model,
        handle: handle.clone(),
        signal: drive.close_signal(),
        context: drive.context().clone(),
        timeout_ms: stream_options.timeout_ms,
        max_retries: stream_options.max_retries,
        max_retry_delay_ms: stream_options.max_retry_delay_ms,
        headers: stream_options.headers.clone(),
    };
    // Upstream try/catch: any provider error is ignored.
    let _ = cancel(request).await;
}

/// Upstream `readDeferredHandle` (`reconcile.ts:40-53`): the deferred source
/// handle read behind the lane's serialized settle line.
async fn read_deferred_handle(
    lane: &Arc<Lane>,
    drive: &Drive,
    deferred: &OperationState,
) -> anyhow::Result<DeferredHandle> {
    let scope = deferred_scope_of(deferred);
    let context = drive.context().clone();
    lane.settle_operation(
        move |_state, _current, _meta, reader| {
            let scope = scope.clone();
            let context = context.clone();
            Box::pin(async move {
                let handle = crate::agent_core::harness::runtime::drive::deferred::read_deferred_source_handle(
                    reader, &scope, context,
                )
                .await?;
                Ok(OperationCommand::Return { result: handle })
            })
        },
        drive.context().clone(),
    )
    .await
}

/// Upstream `publishAbortedTerminal` (`reconcile.ts:55-129`): settle the
/// cancelled operation as aborted, with intent-shaped terminal events.
#[allow(clippy::too_many_lines)]
pub async fn publish_aborted_terminal(
    lane: &Arc<Lane>,
    drive: &Drive,
    _capability: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    let context = drive.context().clone();
    let call_context = context.clone();
    let operation_id = drive.operation_id().to_string();
    let planner_lane = Arc::clone(lane);
    let result = lane
        .settle_operation(
            move |state, current, meta, reader| {
                let lane = planner_lane.clone();
                let context = context.clone();
                let operation_id = operation_id.clone();
                let run_id = operation_id.clone();
                let lane_name = lane.name().to_string();
                let source_tip_id = meta.source_tip_id.clone();
                let intent = meta.intent.clone();
                let tip_id = state.tip_id.clone();
                let context = context.clone();
                Box::pin(async move {
                    let cancel_requested =
                        matches!(current.scope.control, Control::CancelRequested { .. });
                    if !cancel_requested {
                        anyhow::bail!(
                            "Cancellation reconciliation requires cancelled durable control"
                        );
                    }
                    // Upstream validates the run-summary result boundary
                    // inside the run-intent branch (`reconcile.ts:70-77`);
                    // compaction/navigation intents skip it.
                    let summary_reason = if matches!(intent, OperationIntent::Run { .. }) {
                        run_summary_reason(current)?
                    } else {
                        None
                    };
                    let record = operation_result_record(
                        meta,
                        TerminalStatus::Aborted,
                        tip_id.clone(),
                        None,
                    )?;
                    let cleanup =
                        operation_cleanup_writes(reader, &operation_id, current, context.clone())
                            .await?;
                    let ended_at = record.ended_at;
                    Ok(OperationCommand::Finish {
                        writes: cleanup,
                        record: record.clone(),
                        lane: Some(LanePatch {
                            tip_id: None,
                            configuration: None,
                            inbox: None,
                        }),
                        materialize: Box::new(move |_commit| ProcedureResult::Settled {
                            outcome: record,
                        }),
                        events: Some(Box::new(move |_commit: &CommitResult| {
                            let mut events: Vec<HarnessEvent> = Vec::new();
                            if matches!(intent, OperationIntent::Run { .. }) {
                                if let Some(reason) = summary_reason {
                                    events.push(HarnessEvent::CompactionEnd {
                                        lane: lane_name.clone(),
                                        run_id: run_id.clone(),
                                        reason,
                                        status: CompactionEndStatus::Aborted,
                                        entry_id: None,
                                        ended_at,
                                    });
                                }
                                events.push(HarnessEvent::RunEnd {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    status: RunEndStatus::Aborted,
                                    error: None,
                                    from_tip_id: source_tip_id.clone(),
                                    tip_id: tip_id.clone(),
                                    ended_at,
                                });
                            } else if matches!(intent, OperationIntent::Compaction { .. }) {
                                events.push(HarnessEvent::CompactionEnd {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    reason: SummaryReason::Manual,
                                    status: CompactionEndStatus::Aborted,
                                    entry_id: None,
                                    ended_at,
                                });
                            } else {
                                events.push(HarnessEvent::NavigationEnd {
                                    lane: lane_name.clone(),
                                    run_id: run_id.clone(),
                                    status: RunEndStatus::Aborted,
                                    from_tip_id: source_tip_id.clone(),
                                    tip_id: tip_id.clone(),
                                    ended_at,
                                });
                            }
                            Ok(events)
                        })),
                    })
                })
            },
            call_context,
        )
        .await?;
    Ok(result)
}

/// The run-summary compaction reason behind the upstream boundary validation
/// (`reconcile.ts:70-89`): a cancelled run summary phase must carry a
/// `resume_checkpoint` boundary with a reason.
fn run_summary_reason(current: &OperationState) -> anyhow::Result<Option<SummaryReason>> {
    Ok(match &current.phase {
        OperationPhase::SummaryDeciding { task }
        | OperationPhase::SummaryReady { task, .. }
        | OperationPhase::SummaryEffectPending { task, .. }
        | OperationPhase::SummaryRetryWait { task, .. } => {
            if !matches!(task.boundary, ResultBoundary::ResumeCheckpoint { .. })
                || task.reason.is_none()
            {
                anyhow::bail!("Cancelled run summary has an invalid result boundary");
            }
            task.reason
        }
        _ => None,
    })
}

fn deferred_scope_of(
    deferred: &OperationState,
) -> crate::agent_core::harness::runtime::durable::DeferredScope {
    match &deferred.phase {
        OperationPhase::DeferredSuspended { deferred } => deferred.clone(),
        OperationPhase::DeferredEffectPending { deferred, .. } => deferred.clone(),
        _ => panic!("deferred reconciliation requires a deferred phase"),
    }
}

fn deferred_configuration_of(
    deferred: &OperationState,
) -> crate::agent_core::harness::session::LaneConfiguration {
    match &deferred.phase {
        OperationPhase::DeferredSuspended { deferred } => deferred.configuration.clone(),
        OperationPhase::DeferredEffectPending { deferred, .. } => deferred.configuration.clone(),
        _ => panic!("deferred reconciliation requires a deferred phase"),
    }
}

fn deferred_stream_options_of(
    deferred: &OperationState,
) -> crate::agent_core::harness::types::AgentHarnessStreamOptions {
    match &deferred.phase {
        OperationPhase::DeferredSuspended { deferred } => deferred.stream_options.clone(),
        OperationPhase::DeferredEffectPending { deferred, .. } => deferred.stream_options.clone(),
        _ => Default::default(),
    }
}

/// Upstream `reconcileOperation` (`reconcile.ts:132-173`): dispatch one
/// cancelled durable leaf without starting new ordinary work.
#[allow(clippy::too_many_lines)]
pub async fn reconcile_operation(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    emit_tool_event: &ToolEventEmit,
    cancel_deferred: &DeferredCancelFn,
) -> anyhow::Result<ProcedureResult> {
    let (operation_id, state) = lane
        .read_lane(
            |state, _reader| {
                Box::pin(async move {
                    Ok(state.operation.as_ref().map(|operation| {
                        (operation.meta.operation_id.clone(), operation.state.clone())
                    }))
                })
            },
            drive.context().clone(),
        )
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Drive {} has no matching operation to reconcile",
                drive.operation_id()
            )
        })?;
    if operation_id != drive.operation_id() {
        anyhow::bail!(
            "Drive {} has no matching operation to reconcile",
            drive.operation_id()
        );
    }
    let cancel_requested = matches!(state.scope.control, Control::CancelRequested { .. });
    if !cancel_requested {
        anyhow::bail!("Operation {} is not cancelled", drive.operation_id());
    }
    // Upstream `drive.beginAbort(Promise.resolve()); drive.signalAbort();` —
    // the pre-cancelled token is the port's immediately-resolved promise.
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    drive.begin_abort(cancellation);
    drive.signal_abort();

    match &state.phase {
        OperationPhase::AssistantEffectPending { .. } => {
            recover_cancelled_assistant_effect(lane, drive, &state).await
        }
        OperationPhase::Tools { .. } => {
            // Upstream `runTools(lane, drive, state)`; the cancelled-batch
            // path never executes tools, so the (executorless) runtime tool
            // list and the unit tool context match upstream's filtered
            // empty set. Tool events ride the injected emitter.
            run_tools::<()>(
                lane,
                drive,
                &state,
                &[],
                &ToolContextSource::Value(()),
                emit_tool_event,
            )
            .await
        }
        OperationPhase::DeferredSuspended { .. } => {
            let handle = read_deferred_handle(lane, drive, &state).await?;
            cancel_deferred_best_effort(lane, drive, &state, &handle, cancel_deferred).await;
            publish_aborted_terminal(lane, drive, &state).await
        }
        OperationPhase::DeferredEffectPending { .. } => {
            let handle = read_deferred_handle(lane, drive, &state).await?;
            cancel_deferred_best_effort(lane, drive, &state, &handle, cancel_deferred).await;
            recover_cancelled_assistant_effect(lane, drive, &state).await
        }
        _ => publish_aborted_terminal(lane, drive, &state).await,
    }
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
