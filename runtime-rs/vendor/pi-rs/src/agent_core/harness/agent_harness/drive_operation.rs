//! Port of `packages/agent/src/harness/runtime/drive.ts` (106 lines):
//! [`drive_operation`] — "drive one installed pass through direct durable
//! procedures until settlement or a durable wait" — the lane-bound dispatcher
//! that ties the landed drive procedures together.
//!
//! Installed lanes supply a live tool environment; native tools and context
//! are read at the upstream tool-phase boundary, not at pass creation.
//! Explicit [`DriveEnv`] callers retain their snapshot tools/context and
//! custom event/cancellation callbacks. Both use the same dispatcher and
//! durable procedures below.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::execution::AbortRequested;
use crate::agent_core::harness::hooks::{
    BeforeDriveEvent, DriveOperation, HookEvent, HookInvocation,
};
use crate::agent_core::harness::runtime::drive::checkpoint::{run_checkpoint, start_run};
use crate::agent_core::harness::runtime::drive::deferred::run_deferred;
use crate::agent_core::harness::runtime::drive::generation::run_generation;
use crate::agent_core::harness::runtime::drive::publish::PublishOutcome;
use crate::agent_core::harness::runtime::drive::reconcile::{
    reconcile_operation, DeferredCancelFn, DeferredCancelRequest,
};
use crate::agent_core::harness::runtime::drive::recovery::recover_assistant_generation;
use crate::agent_core::harness::runtime::drive::structural::{
    commit_navigation, recover_structural_generation, run_structural_decision,
    run_structural_generation, run_structural_retry_wait,
};
use crate::agent_core::harness::runtime::drive::tools::{
    run_tools, ToolContextSource, ToolEvent, ToolEventEmit,
};
use crate::agent_core::harness::runtime::drive_pass::{Drive, DriveOutcome, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{
    Operation, OperationIntent, OperationPhase, OperationState,
};
use crate::agent_core::harness::runtime::lane::Lane;
use crate::agent_core::harness::session::{Control, SessionInvariantError};
use crate::agent_core::harness::types::AgentHarnessTool;

/// One dispatcher supports both snapshot and live native tool environments.
pub(crate) trait DriveEnvironment: Sync {
    fn emit_tool_event(&self) -> &ToolEventEmit;
    fn cancel_deferred(&self) -> &DeferredCancelFn;
    fn run_tools<'a>(
        &'a self,
        lane: &'a Arc<Lane>,
        drive: &'a Arc<Drive>,
        state: &'a OperationState,
    ) -> BoxFuture<'a, anyhow::Result<ProcedureResult>>;
}

/// The process-local environment the dispatcher hands to the tool-executing
/// procedures (upstream reads these from the lane's config store; see the
/// module docs).
pub struct DriveEnv<TContext: Clone + Send + Sync + 'static> {
    pub tools: Vec<AgentHarnessTool<TContext>>,
    pub tool_context: ToolContextSource<TContext>,
    pub emit_tool_event: ToolEventEmit,
    pub cancel_deferred: Arc<DeferredCancelFn>,
}

impl<TContext: Clone + Send + Sync + 'static> Clone for DriveEnv<TContext> {
    fn clone(&self) -> Self {
        DriveEnv {
            tools: self.tools.clone(),
            tool_context: self.tool_context.clone(),
            emit_tool_event: Arc::clone(&self.emit_tool_event),
            cancel_deferred: Arc::clone(&self.cancel_deferred),
        }
    }
}

impl<TContext: Clone + Send + Sync + 'static> DriveEnvironment for DriveEnv<TContext> {
    fn emit_tool_event(&self) -> &ToolEventEmit {
        &self.emit_tool_event
    }

    fn cancel_deferred(&self) -> &DeferredCancelFn {
        self.cancel_deferred.as_ref()
    }

    fn run_tools<'a>(
        &'a self,
        lane: &'a Arc<Lane>,
        drive: &'a Arc<Drive>,
        state: &'a OperationState,
    ) -> BoxFuture<'a, anyhow::Result<ProcedureResult>> {
        Box::pin(run_tools(
            lane,
            drive,
            state,
            &self.tools,
            &self.tool_context,
            &self.emit_tool_event,
        ))
    }
}

impl DriveEnv<()> {
    /// The default environment for a lane without executor-bearing tools.
    /// This explicit no-tool helper intentionally drops tool event batches;
    /// installed lanes instead supply their real event bus. The
    /// remote deferred cancellation routes through the lane's
    /// [`Models`](crate::ai::models::Models) (`Models::cancel_deferred`),
    /// matching the injected [`DeferredCancelFn`] default in the lane-drive
    /// slice; failures stay unswallowed here because the reconcile caller is
    /// best-effort.
    pub fn for_lane(lane: &Arc<Lane>) -> Self {
        let models = lane.models().clone();
        DriveEnv {
            tools: Vec::new(),
            tool_context: ToolContextSource::Value(()),
            emit_tool_event: Arc::new(|_events: Vec<ToolEvent>, _context: Context| {
                Box::pin(async { Ok(()) }) as BoxFuture<'static, anyhow::Result<()>>
            }),
            cancel_deferred: Arc::new(move |request: DeferredCancelRequest| {
                let models = models.clone();
                Box::pin(async move {
                    let options = crate::ai::models::ModelsDeferredCancelOptions {
                        stream: crate::ai::types::options::StreamOptions {
                            signal: Some(request.signal),
                            timeout_ms: request.timeout_ms,
                            max_retries: request.max_retries,
                            max_retry_delay_ms: request.max_retry_delay_ms,
                            headers: request.headers.map(|headers| {
                                headers
                                    .into_iter()
                                    .map(|(key, value)| (key, Some(value)))
                                    .collect()
                            }),
                            ..Default::default()
                        },
                        transform_headers: None,
                    };
                    models
                        .cancel_deferred(&request.model, &request.handle, Some(options))
                        .await
                })
            }),
        }
    }
}

/// Upstream `currentOperation` (`drive.ts:20-26`): the installed pass's
/// matching durable operation, read from the lane's process-local state.
fn current_operation(lane: &Arc<Lane>, drive: &Drive) -> anyhow::Result<Operation> {
    match lane.state().operation {
        Some(operation) if operation.meta.operation_id == drive.operation_id() => Ok(operation),
        _ => Err(anyhow::Error::new(SessionInvariantError(format!(
            "Drive {} has no matching current operation",
            drive.operation_id()
        )))),
    }
}

fn before_drive_operation(intent: &OperationIntent) -> DriveOperation {
    match intent {
        OperationIntent::Run { .. } => DriveOperation::Run,
        OperationIntent::Compaction { .. } => DriveOperation::Compaction,
        OperationIntent::Navigation { .. } => DriveOperation::Navigation,
    }
}

fn publish_outcome(outcome: PublishOutcome) -> ProcedureResult {
    match outcome {
        PublishOutcome::Continue => ProcedureResult::Continue,
        PublishOutcome::Settled { outcome } => ProcedureResult::Settled { outcome: *outcome },
    }
}

/// Upstream `driveOperation` (`drive.ts:29-106`). Returns the drive outcome:
/// a durable settlement, or a durable wait (retry / deferred poll).
pub async fn drive_operation<TContext: Clone + Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    env: &DriveEnv<TContext>,
) -> anyhow::Result<DriveOutcome> {
    drive_operation_with_env(lane, drive, env).await
}

/// Shared by the public explicit-environment entry and Lane's installed pass.
pub(crate) async fn drive_operation_with_env<E: DriveEnvironment>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    env: &E,
) -> anyhow::Result<DriveOutcome> {
    let operation = current_operation(lane, drive)?;
    if matches!(operation.state.scope.control, Control::Running) {
        // Upstream catches only AbortRequested around before_drive and waits
        // out the cancellation.
        let hook = lane.hooks().run_with_gate(
            HookInvocation {
                lane: lane.name().to_owned(),
                run_id: drive.operation_id().to_owned(),
                event: HookEvent::BeforeDrive(BeforeDriveEvent {
                    operation: before_drive_operation(&operation.meta.intent),
                }),
            },
            Arc::new(drive.gate().clone()),
            drive.context().clone(),
        );
        if let Err(error) = hook.await {
            match error.downcast::<AbortRequested>() {
                Ok(abort) => abort.cancellation.cancelled().await,
                Err(other) => return Err(other),
            }
        }
    }

    loop {
        let operation = current_operation(lane, drive)?;
        let state = operation.state;
        let result = dispatch(lane, drive, env, &state).await;
        let result = match result {
            Ok(result) => result,
            Err(error) => match error.downcast::<AbortRequested>() {
                Ok(abort) => {
                    abort.cancellation.cancelled().await;
                    ProcedureResult::Continue
                }
                Err(other) => return Err(other),
            },
        };
        match result {
            ProcedureResult::Settled { outcome } => {
                return Ok(DriveOutcome::Settled { outcome });
            }
            ProcedureResult::Waiting { outcome } => return Ok(outcome),
            ProcedureResult::Continue => {}
        }
        let next = current_operation(lane, drive)?.state;
        if next == state && !matches!(next.scope.control, Control::CancelRequested { .. }) {
            return Err(anyhow::Error::new(SessionInvariantError(format!(
                "Drive procedure made no progress from {}",
                state.at()
            ))));
        }
    }
}

async fn dispatch<E: DriveEnvironment>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    env: &E,
    state: &OperationState,
) -> anyhow::Result<ProcedureResult> {
    if matches!(state.scope.control, Control::CancelRequested { .. }) {
        return reconcile_operation(lane, drive, env.emit_tool_event(), env.cancel_deferred())
            .await;
    }
    match &state.phase {
        OperationPhase::Starting => Ok(publish_outcome(start_run(lane, drive, state).await?)),
        OperationPhase::Checkpoint { .. } => {
            Ok(publish_outcome(run_checkpoint(lane, drive, state).await?))
        }
        OperationPhase::AssistantReady { .. } | OperationPhase::AssistantRetryWait { .. } => {
            run_generation(lane, drive, state).await
        }
        OperationPhase::AssistantEffectPending { .. } => {
            recover_assistant_generation(lane, drive, state).await
        }
        OperationPhase::Tools { .. } => env.run_tools(lane, drive, state).await,
        OperationPhase::DeferredSuspended { .. } | OperationPhase::DeferredEffectPending { .. } => {
            run_deferred(lane, drive, state).await
        }
        OperationPhase::SummaryDeciding { .. } => run_structural_decision(lane, drive, state).await,
        OperationPhase::SummaryReady { .. } => run_structural_generation(lane, drive, state).await,
        OperationPhase::SummaryEffectPending { .. } => {
            recover_structural_generation(lane, drive, state).await
        }
        OperationPhase::SummaryRetryWait { .. } => {
            run_structural_retry_wait(lane, drive, state).await
        }
        OperationPhase::NavigationReadyToCommit { .. } => {
            commit_navigation(lane, drive, state).await
        }
    }
}
