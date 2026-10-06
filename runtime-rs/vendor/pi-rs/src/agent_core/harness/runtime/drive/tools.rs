//! Port of `packages/agent/src/harness/runtime/drive/tools.ts` (692 lines):
//! execute, recover, stage, and source-order one complete durable tool batch
//! ([`run_tools`]) with sequential and parallel scheduling, hook interaction,
//! memo-backed invocations, checkpointed progress, and cancel/replay
//! semantics.
//!
//! Disclosed substitutions and seams:
//! - **Tool events ([`ToolEvent`]).** Upstream attaches `tool_start`/
//!   `tool_update`/`tool_end` objects to commits via the lane's
//!   `events?(commit)` hook and the `HarnessEvent` union. This module delivers
//!   the same wire shapes ([`ToolEvent`]) through an injected emitter
//!   ([`ToolEventEmit`]); installed lanes forward them to the public harness
//!   event bus after the corresponding lane command resolves. Event delivery
//!   is awaited before the next procedure, as in upstream `lane.ts:379`.
//! - **Configured tools and tool context.** [`run_tools`] retains the explicit
//!   snapshot API for direct callers. Installed lanes use
//!   [`run_tools_from_config`] to read native executors and their context
//!   source from the current runtime config after materialization and the
//!   cancellation check. Both paths apply the upstream active-tool filter
//!   and lazily resolve one context per tool batch, never per generation or
//!   per executor. Native values and callbacks are not serialized.
//! - **`Drive` is shared as `Arc`.** Tool-call job completions outlive the
//!   `runTools` call frame (parallel scheduling), so every procedure takes
//!   `&Arc<Drive>` (the ownership the lane's installed-drive claim already
//!   uses) instead of sibling modules' `&Drive`.
//! - **Parallel job failures.** Upstream `Promise.all(jobs)` rejects with the
//!   first settlement; the port awaits every spawned job and reports the
//!   first stored failure, then the final materialization. Serialization of
//!   `materializeReady` through the catch-swallowed chain is preserved.
//! - **`AbortRequested` detection.** Gate refusals downcast through
//!   [`GateRejection`]; the hooks port's already-cancelled admitted-context
//!   error (fixed message "the operation was aborted", unreachable for an
//!   `EffectGate` in the aborting state) is also recognized as an abort
//!   ([`AbortRefusal::CancelledContext`]) — it carries no cancellation token
//!   to await, so the upstream `await error.cancellation` degenerates to a
//!   no-op there.
//! - **`getMemo`/`setMemo` rejections.** The `AgentHarnessToolInvocation`
//!   trait surface returns `Option`/`()` rather than fallible futures: an
//!   expired or disowned invocation resolves `getMemo` to `None` and makes
//!   `setMemo` a no-op instead of rejecting with `ToolInvocationEnded`.
//!   `validateMemoName` contract violations panic with the upstream
//!   `TypeError` messages (the `session/values.rs` precedent).
//! - **Recovery checkpoints.** `readCheckpoint` deserializes the stored
//!   pending tool output leniently; an unparseable snapshot reads as absent
//!   (upstream's duck-typed `checkpoint?.content ?? []`).
//! - **TContext cloning.** Upstream passes one `toolContext` object by
//!   reference into every concurrent tool; Rust jobs need owned values, so
//!   `TContext: Clone` and each job receives a clone.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use serde::Serialize;
use tokio::task::JoinHandle;

use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::execution::effect_gate::{AbortRequested, GateRejection};
use crate::agent_core::harness::execution::tools::{
    apply_before_tool_decision, create_tool_result_message, execute_tool_call, finalize_tool_call,
    prepare_tool_call, tool_result_from_message, AfterToolPatch, BeforeToolDecision,
    ClearedToolCall, ExecutedToolCall, FinalizedToolCall, ImmediateToolOutcome, PrepareOutcome,
};
use crate::agent_core::harness::hooks::{
    AfterToolEvent, BeforeToolEvent, HookEvent, HookInvocation, HookName, HookResult,
};
use crate::agent_core::harness::runtime::drive::tool_placement::{
    materialize_ready, read_tool_batch_source, tool_call_for, with_tool_batch, ToolBatchSource,
};
use crate::agent_core::harness::runtime::drive_pass::{Drive, ProcedureResult};
use crate::agent_core::harness::runtime::durable::{
    LaneState, OperationPhase, OperationState, ToolBatch, ToolCall, ToolCallState,
};
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{
    ContinueOperationResult, Lane, LaneCommand, OperationCommand,
};
use crate::agent_core::harness::runtime::progress::open_tool_progress;
use crate::agent_core::harness::session::types::CommitResult;
use crate::agent_core::harness::session::values::{
    delete_value, operation_tool_args, operation_tool_memo, operation_tool_memo_prefix,
    pending_entry, pending_tool_output, set_value,
};
use crate::agent_core::harness::session::{Control, SessionInvariantError};
use crate::agent_core::harness::types::{
    AgentHarnessTool, AgentHarnessToolInvocation, AgentHarnessToolUpdateCallback,
    AgentHarnessToolUpdateOptions,
};
use crate::agent_core::types::{
    AgentMessage, AgentToolCall, AgentToolResult, ToolExecutionMode, ToolReplay,
};
use crate::ai::types::message::{TextOrImageBlock, ToolResultMessage};
use crate::ai::types::primitives::{StopReason, Usage};
use crate::ai::types::TextContent;

/// Upstream `INTERRUPTION_MARKER` (`tools.ts:44-45`).
const INTERRUPTION_MARKER: &str = "[Tool execution was interrupted. The preceding output is the latest durable progress snapshot; newer live output may be missing, and the external outcome is unknown.]";

/// Upstream tool-event literals from `publishToolIntent`/`publishToolOutcome`
/// (`tools.ts:211-222`, `tools.ts:261-288`) and `performToolInvocation`
/// (`tools.ts:362-379`), matching the `tool_start`/`tool_update`/`tool_end`
/// members of the upstream `HarnessEvent` union (`agent-harness.ts:306-330`)
/// including the runtime `lane` envelope. Delivered through an injected
/// emitter until the `HarnessEvent` port absorbs the tool variants.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolEvent {
    ToolStart {
        lane: String,
        run_id: String,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    ToolUpdate {
        lane: String,
        run_id: String,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
        partial_result: AgentToolResult,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    ToolEnd {
        lane: String,
        run_id: String,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
        result: AgentToolResult,
        is_error: bool,
        terminate: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
}

/// One tool-event delivery (upstream `lane.emitBatch` returns a promise).
/// Injected until the `HarnessEvent` port carries tool events.
pub type ToolEventEmit =
    Arc<dyn Fn(Vec<ToolEvent>, Context) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;

/// Upstream `Config["toolContext"]` (`agent-harness.ts`): a resolved context
/// value or a provider invoked once per [`run_tools`] call.
pub enum ToolContextSource<TContext: Clone + Send + Sync + 'static> {
    Value(TContext),
    Provider(Arc<dyn Fn(Context) -> BoxFuture<'static, TContext> + Send + Sync>),
    /// A rejected context future propagates as a harness fault, not a tool
    /// result. The infallible provider remains source-compatible.
    FallibleProvider(
        Arc<dyn Fn(Context) -> BoxFuture<'static, anyhow::Result<TContext>> + Send + Sync>,
    ),
}

impl<TContext: Clone + Send + Sync + 'static> Clone for ToolContextSource<TContext> {
    fn clone(&self) -> Self {
        match self {
            ToolContextSource::Value(value) => ToolContextSource::Value(value.clone()),
            ToolContextSource::Provider(provider) => {
                ToolContextSource::Provider(Arc::clone(provider))
            }
            ToolContextSource::FallibleProvider(provider) => {
                ToolContextSource::FallibleProvider(Arc::clone(provider))
            }
        }
    }
}

/// Upstream `ToolOutcome` (`tools.ts:39`).
#[derive(Clone)]
struct ToolOutcome {
    tool_call: AgentToolCall,
    message: ToolResultMessage,
    terminate: bool,
}

/// Upstream `ToolCallTask` (`tools.ts:38`): the completion of one started
/// tool call (execution plus outcome publication).
struct ToolCallTask {
    completion: BoxFuture<'static, anyhow::Result<()>>,
}

/// Upstream `PreparedToolInvocation` (`tools.ts:40-42`).
enum PreparedToolInvocation<TContext: Send + Sync + 'static> {
    Ready { cleared: ClearedToolCall<TContext> },
    Outcome { outcome: ToolOutcome },
}

/// Upstream's inline `execution` object of `runSequential` (`tools.ts:547`).
struct ToolExecutables<TContext: Send + Sync + 'static> {
    tools: Vec<AgentHarnessTool<TContext>>,
    tools_by_name: HashMap<String, AgentHarnessTool<TContext>>,
    tool_context: TContext,
}

/// An abort refusal recognized at a hook/gate boundary (upstream
/// `error instanceof AbortRequested`).
enum AbortRefusal {
    /// A `GateRejection::AbortRequested` carrying the cancellation procedure
    /// to await (upstream `await error.cancellation`).
    Requested(AbortRequested),
    /// The hooks port's already-cancelled admitted-context error; nothing to
    /// await (see the module docs).
    CancelledContext,
}

fn abort_refusal(error: &anyhow::Error) -> Option<AbortRefusal> {
    if let Some(rejection) = error.downcast_ref::<GateRejection>() {
        if let GateRejection::AbortRequested(abort) = rejection {
            return Some(AbortRefusal::Requested(AbortRequested {
                cancellation: abort.cancellation.clone(),
            }));
        }
        return None;
    }
    if error.to_string() == "the operation was aborted" {
        return Some(AbortRefusal::CancelledContext);
    }
    None
}

/// Upstream `ToolInvocationEnded` (`tools.ts:47-52`): the expected rejection
/// of a disowned invocation's memo command.
fn invocation_ended() -> anyhow::Error {
    anyhow::anyhow!("Tool invocation no longer owns its durable effect")
}

fn invariant(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(SessionInvariantError(message.into()))
}

/// Upstream `currentBatch` (`tools.ts:54-60`): the live operation state while
/// it still sits at the tools phase.
fn current_batch(lane: &Lane) -> Option<OperationState> {
    let state = lane.state();
    let operation = state.operation?;
    if !matches!(operation.state.phase, OperationPhase::Tools { .. }) {
        return None;
    }
    Some(operation.state)
}

/// Upstream `findCall` (`tools.ts:62-64`).
fn find_call<'a>(
    batch: &'a ToolBatch,
    source_index: usize,
    result_entry_id: &str,
) -> Option<&'a ToolCall> {
    batch
        .calls
        .iter()
        .find(|call| call.source_index == source_index && call.result_entry_id == result_entry_id)
}

/// Upstream `replaceCall` (`tools.ts:66-75`).
fn replace_call(batch: ToolBatch, replacement: ToolCall) -> ToolBatch {
    let mut calls = batch.calls;
    for call in calls.iter_mut() {
        if call.source_index == replacement.source_index
            && call.result_entry_id == replacement.result_entry_id
        {
            *call = replacement.clone();
        }
    }
    ToolBatch { calls, ..batch }
}

/// Upstream `validateMemoName` (`tools.ts:77-80`): a `TypeError` contract
/// violation, so the port panics with the same messages (the
/// `session/values.rs` precedent).
fn validate_memo_name(name: &str) {
    if name.is_empty() {
        panic!("Tool invocation memo name must not be empty");
    }
    if name.contains(':') {
        panic!("Tool invocation memo name must not contain ':'");
    }
}

/// Upstream `ownsEffect` (`tools.ts:89-93`).
fn owns_effect(state: &LaneState, source_index: usize, result_entry_id: &str) -> bool {
    let Some(operation) = &state.operation else {
        return false;
    };
    let OperationPhase::Tools { batch } = &operation.state.phase else {
        return false;
    };
    matches!(
        find_call(batch, source_index, result_entry_id).map(|call| &call.state),
        Some(ToolCallState::EffectPending { .. })
    )
}

/// The lane-backed [`AgentHarnessToolInvocation`] (upstream the `invocation`
/// object of `invocationCapability`, `tools.ts:96-126`).
struct LaneToolInvocation {
    lane: Arc<Lane>,
    context: Context,
    operation_id: String,
    turn_id: String,
    invocation_id: String,
    source_index: usize,
    active: Arc<std::sync::atomic::AtomicBool>,
}

impl AgentHarnessToolInvocation for LaneToolInvocation {
    fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    fn operation_id(&self) -> &str {
        &self.operation_id
    }

    fn turn_id(&self) -> &str {
        &self.turn_id
    }

    fn get_memo<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<serde_json::Value>> {
        validate_memo_name(name);
        if !self.active.load(std::sync::atomic::Ordering::SeqCst) {
            // Upstream: `Promise.reject(ended())`.
            return Box::pin(async { None });
        }
        let lane = Arc::clone(&self.lane);
        let context = self.context.clone();
        let operation_id = self.operation_id.clone();
        let invocation_id = self.invocation_id.clone();
        let source_index = self.source_index;
        let name = name.to_owned();
        Box::pin(async move {
            let command_context = context.clone();
            let stored = lane
                .command(
                    move |state, reader| {
                        let context = context.clone();
                        let operation_id = operation_id.clone();
                        let invocation_id = invocation_id.clone();
                        let name = name.clone();
                        Box::pin(async move {
                            if !owns_effect(state, source_index, &invocation_id) {
                                return Ok(LaneCommand::Reject {
                                    error: invocation_ended(),
                                });
                            }
                            let stored = reader
                                .get_value(
                                    &operation_tool_memo(&operation_id, &invocation_id, &name),
                                    context,
                                )
                                .await?;
                            Ok(LaneCommand::Return {
                                result: stored.map(|stored| stored.value),
                            })
                        })
                    },
                    command_context,
                )
                .await;
            // Upstream rejection (`ToolInvocationEnded`) reads as an absent
            // memo on this trait surface.
            stored.unwrap_or_default()
        })
    }

    fn set_memo<'a>(
        &'a self,
        name: &'a str,
        value: Option<serde_json::Value>,
    ) -> BoxFuture<'a, ()> {
        validate_memo_name(name);
        if !self.active.load(std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(async {});
        }
        let lane = Arc::clone(&self.lane);
        let context = self.context.clone();
        let operation_id = self.operation_id.clone();
        let invocation_id = self.invocation_id.clone();
        let source_index = self.source_index;
        let name = name.to_owned();
        Box::pin(async move {
            let _ = lane
                .command(
                    move |state, _reader| {
                        let operation_id = operation_id.clone();
                        let invocation_id = invocation_id.clone();
                        let name = name.clone();
                        let value = value.clone();
                        Box::pin(async move {
                            if !owns_effect(state, source_index, &invocation_id) {
                                return Ok(LaneCommand::Reject {
                                    error: invocation_ended(),
                                });
                            }
                            let address = operation_tool_memo(&operation_id, &invocation_id, &name);
                            let write = match value {
                                Some(value) => set_value(&address, value),
                                None => delete_value(&address),
                            };
                            Ok(LaneCommand::Commit {
                                writes: vec![write],
                                next: state.clone(),
                                materialize: Box::new(|_commit: &CommitResult| {}),
                                events: None,
                            })
                        })
                    },
                    context,
                )
                .await;
        })
    }
}

/// Upstream `invocationCapability` (`tools.ts:82-131`).
fn invocation_capability(
    lane: &Arc<Lane>,
    drive: &Drive,
    batch: &ToolBatch,
    call: &ToolCall,
) -> (
    Arc<dyn AgentHarnessToolInvocation>,
    Box<dyn Fn() + Send + Sync>,
) {
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let invocation = Arc::new(LaneToolInvocation {
        lane: Arc::clone(lane),
        context: drive.context().clone(),
        operation_id: drive.operation_id().to_owned(),
        turn_id: batch.turn_id.clone(),
        invocation_id: call.result_entry_id.clone(),
        source_index: call.source_index,
        active: Arc::clone(&active),
    });
    let expire = move || {
        active.store(false, std::sync::atomic::Ordering::SeqCst);
    };
    (invocation, Box::new(expire))
}

/// Upstream `syntheticMessage` (`tools.ts:133-148`).
fn synthetic_message(
    tool_call: &AgentToolCall,
    content: Vec<TextOrImageBlock>,
    details: Option<serde_json::Value>,
    usage: Option<Usage>,
) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        content,
        details,
        usage,
        is_error: true,
        timestamp: crate::ai::now_ms(),
    }
}

fn text_block(text: impl Into<String>) -> TextOrImageBlock {
    TextOrImageBlock::Text(TextContent {
        text: text.into(),
        text_signature: None,
    })
}

/// Upstream `abortedOutcome` (`tools.ts:150-156`).
fn aborted_outcome(tool_call: &AgentToolCall) -> ToolOutcome {
    ToolOutcome {
        tool_call: tool_call.clone(),
        message: synthetic_message(
            tool_call,
            vec![text_block(
                "Tool execution was cancelled before completion.",
            )],
            None,
            None,
        ),
        terminate: false,
    }
}

/// Upstream `interruptedOutcome` (`tools.ts:158-168`).
fn interrupted_outcome(
    tool_call: &AgentToolCall,
    checkpoint: Option<AgentToolResult>,
) -> ToolOutcome {
    let mut content = checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.content.clone())
        .unwrap_or_default();
    content.push(text_block(INTERRUPTION_MARKER));
    let (details, usage) = match &checkpoint {
        Some(checkpoint) => (checkpoint.details.clone(), checkpoint.usage),
        None => (None, None),
    };
    ToolOutcome {
        tool_call: tool_call.clone(),
        message: synthetic_message(tool_call, content, details, usage),
        terminate: false,
    }
}

/// Upstream `truncatedOutcome` (`tools.ts:170-181`).
fn truncated_outcome(tool_call: &AgentToolCall) -> ToolOutcome {
    let quoted = serde_json::to_string(&tool_call.name).unwrap_or_default();
    ToolOutcome {
        tool_call: tool_call.clone(),
        message: synthetic_message(
            tool_call,
            vec![text_block(format!(
                "Tool call {quoted} was not executed because the assistant response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
            ))],
            None,
            None,
        ),
        terminate: false,
    }
}

/// Upstream `outcomeFromFinalizedCall` (`tools.ts:183-185`) narrowed to the
/// immediate outcome (the execution path returns the finalized shape
/// directly).
fn outcome_from_immediate(outcome: ImmediateToolOutcome) -> ToolOutcome {
    let finalized = FinalizedToolCall {
        tool_call: outcome.tool_call,
        result: outcome.result,
        is_error: outcome.is_error,
        terminate: outcome.terminate,
    };
    ToolOutcome {
        tool_call: finalized.tool_call.clone(),
        message: create_tool_result_message(&finalized),
        terminate: finalized.terminate,
    }
}

/// Upstream `publishToolIntent` (`tools.ts:187-227`): durably record the
/// effect-pending call with its persisted arguments and emit `tool_start`.
#[allow(clippy::too_many_arguments)] // parameter list mirrors the upstream procedure
async fn publish_tool_intent(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    planned: &ToolCall,
    tool_call: &AgentToolCall,
    args: serde_json::Value,
    replay: ToolReplay,
    recovery: bool,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ContinueOperationResult<ToolCall>> {
    let context = drive.context().clone();
    let planned = planned.clone();
    let tool_call = tool_call.clone();
    let args_for_planner = args.clone();
    let operation_id = drive.operation_id().to_owned();
    let lane_name = lane.name().to_owned();
    let run_id = drive.operation_id().to_owned();
    let emit = Arc::clone(emit_tool_event);
    let committed = lane
        .continue_operation(
            move |_state, latest, _meta, _reader| {
                let planned = planned.clone();
                let tool_call = tool_call.clone();
                let args = args_for_planner.clone();
                let operation_id = operation_id.clone();
                let lane_name = lane_name.clone();
                let run_id = run_id.clone();
                Box::pin(async move {
                    let OperationPhase::Tools { batch: current } = &latest.phase else {
                        anyhow::bail!("Tool intent requires a tools operation");
                    };
                    let effect_pending = ToolCall {
                        source_index: planned.source_index,
                        result_entry_id: planned.result_entry_id.clone(),
                        state: ToolCallState::EffectPending { replay },
                    };
                    let turn_id = current.turn_id.clone();
                    let writes = vec![set_value(
                        &operation_tool_args(&operation_id, &turn_id, planned.source_index as i64),
                        args.clone(),
                    )];
                    let events = vec![ToolEvent::ToolStart {
                        lane: lane_name,
                        run_id,
                        turn_id: turn_id.clone(),
                        tool_call_id: tool_call.id.clone(),
                        tool_name: tool_call.name.clone(),
                        args: args.clone(),
                        recovery,
                    }];
                    Ok(OperationCommand::Commit {
                        writes,
                        operation_state: with_tool_batch(
                            latest,
                            replace_call(current.clone(), effect_pending.clone()),
                        ),
                        lane: None,
                        materialize: Box::new(move |_commit: &CommitResult| {
                            (effect_pending, events)
                        }),
                        events: None,
                    })
                })
            },
            context,
        )
        .await?;
    match committed {
        ContinueOperationResult::CancelRequested => Ok(ContinueOperationResult::CancelRequested),
        ContinueOperationResult::Result { value } => {
            let (effect_pending, events) = value;
            emit(events, drive.context().clone()).await?;
            Ok(ContinueOperationResult::Result {
                value: effect_pending,
            })
        }
    }
}

/// Upstream `publishToolOutcome` (`tools.ts:229-293`): stage the tool-result
/// payload, drop the checkpoint and memos, advance the batch to
/// `outcome_ready`, and emit `tool_start` (planned calls only) plus
/// `tool_end`.
async fn publish_tool_outcome(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    call: &ToolCall,
    finalized: ToolOutcome,
    recovery: bool,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<()> {
    let context = drive.context().clone();
    let call = call.clone();
    let was_planned = matches!(call.state, ToolCallState::Planned);
    let lane_name = lane.name().to_owned();
    let run_id = drive.operation_id().to_owned();
    let operation_id = drive.operation_id().to_owned();
    let emit = Arc::clone(emit_tool_event);
    let command_context = context.clone();
    let events = lane
        .settle_operation(
            move |_state, latest, _meta, reader| {
                let call = call.clone();
                let finalized = finalized.clone();
                let context = context.clone();
                let lane_name = lane_name.clone();
                let run_id = run_id.clone();
                let operation_id = operation_id.clone();
                Box::pin(async move {
                    let OperationPhase::Tools { batch: current } = &latest.phase else {
                        anyhow::bail!("Tool outcome requires a tools operation");
                    };
                    let memos = reader
                        .scan_values(
                            &operation_tool_memo_prefix(&operation_id, Some(&call.result_entry_id)),
                            context.clone(),
                        )
                        .await?;
                    let durable_terminate =
                        matches!(latest.scope.control, Control::Running) && finalized.terminate;
                    let outcome = ToolCall {
                        source_index: call.source_index,
                        result_entry_id: call.result_entry_id.clone(),
                        state: ToolCallState::OutcomeReady {
                            terminate: durable_terminate,
                        },
                    };
                    // Upstream stages `{ type: "message", payload: message }`;
                    // the ported tool-placement reader inspects the stored
                    // JSON's top-level `role` field, so the message is staged
                    // directly (see that module's disclosed substitutions).
                    let staged =
                        serde_json::to_value(AgentMessage::ToolResult(finalized.message.clone()))?;
                    let mut writes = vec![
                        set_value(&pending_entry(&call.result_entry_id), staged),
                        delete_value(&pending_tool_output(&operation_id, &call.result_entry_id)),
                    ];
                    for memo in &memos {
                        writes.push(delete_value(&memo.address));
                    }
                    let turn_id = current.turn_id.clone();
                    let mut events = Vec::new();
                    if was_planned {
                        events.push(ToolEvent::ToolStart {
                            lane: lane_name.clone(),
                            run_id: run_id.clone(),
                            turn_id: turn_id.clone(),
                            tool_call_id: finalized.tool_call.id.clone(),
                            tool_name: finalized.tool_call.name.clone(),
                            args: finalized.tool_call.arguments.clone(),
                            recovery,
                        });
                    }
                    events.push(ToolEvent::ToolEnd {
                        lane: lane_name,
                        run_id,
                        turn_id,
                        tool_call_id: finalized.tool_call.id.clone(),
                        tool_name: finalized.tool_call.name.clone(),
                        result: tool_result_from_message(&finalized.message, durable_terminate),
                        is_error: finalized.message.is_error,
                        terminate: durable_terminate,
                        recovery,
                    });
                    Ok(OperationCommand::Commit {
                        writes,
                        operation_state: with_tool_batch(
                            latest,
                            replace_call(current.clone(), outcome),
                        ),
                        lane: None,
                        materialize: Box::new(move |_commit: &CommitResult| events),
                        events: None,
                    })
                })
            },
            command_context,
        )
        .await?;
    emit(events, drive.context().clone()).await
}

/// Upstream `clearReplayCheckpoint` (`tools.ts:295-329`): drop the replay
/// checkpoint, return the persisted arguments, and emit the recovery
/// `tool_start`.
async fn clear_replay_checkpoint(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    batch: &ToolBatch,
    call: &ToolCall,
    tool_call: &AgentToolCall,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<serde_json::Value> {
    let context = drive.context().clone();
    let call = call.clone();
    let tool_call = tool_call.clone();
    let lane_name = lane.name().to_owned();
    let run_id = drive.operation_id().to_owned();
    let operation_id = drive.operation_id().to_owned();
    let turn_id = batch.turn_id.clone();
    let emit = Arc::clone(emit_tool_event);
    let (args, events) = lane
        .command(
            move |state, reader| {
                let context = context.clone();
                let call = call.clone();
                let lane_name = lane_name.clone();
                let run_id = run_id.clone();
                let operation_id = operation_id.clone();
                let turn_id = turn_id.clone();
                let tool_call = tool_call.clone();
                Box::pin(async move {
                    let stored = reader
                        .get_value(
                            &operation_tool_args(&operation_id, &turn_id, call.source_index as i64),
                            context.clone(),
                        )
                        .await?;
                    let Some(stored) = stored else {
                        return Err(invariant(format!(
                            "Tool call {} is missing persisted arguments",
                            call.result_entry_id
                        )));
                    };
                    let value = stored.value.clone();
                    let events = vec![ToolEvent::ToolStart {
                        lane: lane_name,
                        run_id,
                        turn_id,
                        tool_call_id: tool_call.id.clone(),
                        tool_name: tool_call.name.clone(),
                        args: value.clone(),
                        recovery: true,
                    }];
                    Ok(LaneCommand::Commit {
                        writes: vec![delete_value(&pending_tool_output(
                            &operation_id,
                            &call.result_entry_id,
                        ))],
                        next: state.clone(),
                        materialize: Box::new(move |_commit: &CommitResult| (value, events)),
                        events: None,
                    })
                })
            },
            drive.context().clone(),
        )
        .await?;
    emit(events, drive.context().clone()).await?;
    Ok(args)
}

/// Upstream `readCheckpoint` (`tools.ts:331-340`): the staged pending tool
/// output, if any (deserialized leniently like the upstream duck-typed read).
async fn read_checkpoint(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    call: &ToolCall,
) -> anyhow::Result<Option<AgentToolResult>> {
    let context = drive.context().clone();
    let call = call.clone();
    let operation_id = drive.operation_id().to_owned();
    lane.command(
        move |_state, reader| {
            let context = context.clone();
            let call = call.clone();
            let operation_id = operation_id.clone();
            Box::pin(async move {
                let stored = reader
                    .get_value(
                        &pending_tool_output(&operation_id, &call.result_entry_id),
                        context,
                    )
                    .await?;
                Ok(LaneCommand::Return {
                    result: stored.and_then(|stored| serde_json::from_value(stored.value).ok()),
                })
            })
        },
        drive.context().clone(),
    )
    .await
}

/// Upstream `resolveToolContext` (`tools.ts:342-348`): resolve the configured
/// tool context once per tool batch.
async fn resolve_tool_context<TContext: Clone + Send + Sync + 'static>(
    source: &ToolContextSource<TContext>,
    drive: &Arc<Drive>,
) -> anyhow::Result<TContext> {
    match source {
        ToolContextSource::Value(value) => Ok(value.clone()),
        ToolContextSource::Provider(source) => Ok(source(drive.context().clone()).await),
        ToolContextSource::FallibleProvider(source) => source(drive.context().clone()).await,
    }
}

/// Upstream `performToolInvocation` (`tools.ts:350-434`): run one cleared
/// external effect with checkpointed progress, then apply the after-tool
/// hook patch.
#[allow(clippy::too_many_arguments)] // parameter list mirrors the upstream procedure
async fn perform_tool_invocation<TContext: Clone + Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    batch: &ToolBatch,
    call: &ToolCall,
    cleared: ClearedToolCall<TContext>,
    tool_context: TContext,
    recovery: bool,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ToolOutcome> {
    let (invocation, expire) = invocation_capability(lane, drive, batch, call);
    let progress = Arc::new(open_tool_progress(
        lane,
        drive,
        &batch.turn_id,
        call.source_index,
        &call.result_entry_id,
    ));
    let latest_update_delivery: Arc<Mutex<Option<JoinHandle<anyhow::Result<()>>>>> =
        Arc::new(Mutex::new(None));
    let publish_update: Arc<AgentHarnessToolUpdateCallback> = {
        let lane_name = lane.name().to_owned();
        let run_id = drive.operation_id().to_owned();
        let turn_id = batch.turn_id.clone();
        let tool_call = cleared.tool_call.clone();
        let emit = Arc::clone(emit_tool_event);
        let context = drive.context().clone();
        let latest = Arc::clone(&latest_update_delivery);
        let progress = Arc::clone(&progress);
        Arc::new(
            move |partial: &AgentToolResult, options: AgentHarnessToolUpdateOptions| {
                let event = ToolEvent::ToolUpdate {
                    lane: lane_name.clone(),
                    run_id: run_id.clone(),
                    turn_id: turn_id.clone(),
                    tool_call_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    partial_result: partial.clone(),
                    recovery,
                };
                let delivery = emit(vec![event], context.clone());
                let handle = tokio::spawn(delivery);
                *latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
                // Upstream: `if (options?.checkpoint === true) progress.write(partial)`.
                if options.checkpoint {
                    progress.write(serde_json::to_value(partial).expect("tool result serializes"));
                }
            },
        )
    };

    let cleared_tool_call = cleared.tool_call.clone();
    let execution = match execute_tool_call(
        cleared.clone(),
        drive.gate(),
        publish_update,
        tool_context,
        Arc::clone(&invocation),
        drive.context().clone(),
    ) {
        Ok(execution) => execution,
        Err(rejection) => {
            expire();
            progress.seal();
            progress.drain().await?;
            match rejection {
                GateRejection::AbortRequested(abort) => {
                    abort.cancellation.cancelled().await;
                    return Ok(if recovery {
                        interrupted_outcome(&cleared_tool_call, None)
                    } else {
                        aborted_outcome(&cleared_tool_call)
                    });
                }
                other => return Err(anyhow::Error::new(other)),
            }
        }
    };

    // `execution.finally(() => { expire(); seal(); })`: the executed future
    // itself converts tool failures to error output, so only completion
    // remains.
    let executed: ExecutedToolCall = execution.await;
    expire();
    progress.seal();
    let latest = latest_update_delivery
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(latest) = latest {
        latest.await.map_err(anyhow::Error::new)??;
    }
    progress.drain().await?;

    let patch: Option<AfterToolPatch> = match lane
        .hooks()
        .run_tool_with_gate(
            HookInvocation {
                lane: lane.name().to_owned(),
                run_id: drive.operation_id().to_owned(),
                event: HookEvent::AfterTool(AfterToolEvent {
                    tool_call_id: cleared.tool_call.id.clone(),
                    tool_name: cleared.tool_call.name.clone(),
                    args: cleared.args.clone(),
                    content: executed.result.content.clone(),
                    details: executed.result.details.clone(),
                    is_error: executed.is_error,
                    usage: executed.result.usage,
                }),
            },
            Arc::new(drive.gate().clone()),
            drive.context().clone(),
        )
        .await
    {
        Ok(HookResult::AfterTool(patch)) => patch,
        Ok(other) => {
            return Err(anyhow::anyhow!(
                "hook registered for {} returned a {} result",
                HookName::AfterTool.as_str(),
                other.hook_name().as_str()
            ));
        }
        Err(error) => {
            if abort_refusal(&error).is_some() {
                None
            } else {
                return Err(error);
            }
        }
    };
    let finalized = finalize_tool_call(&cleared, executed, patch.as_ref());
    Ok(ToolOutcome {
        tool_call: finalized.tool_call.clone(),
        message: create_tool_result_message(&finalized),
        terminate: finalized.terminate,
    })
}

/// Upstream `prepareToolInvocation` (`tools.ts:436-473`): resolve and clear
/// one planned call, short-circuiting to synthetic outcomes.
async fn prepare_tool_invocation<TContext: Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    sources: &ToolBatchSource,
    call: &ToolCall,
    tools: &[AgentHarnessTool<TContext>],
) -> anyhow::Result<PreparedToolInvocation<TContext>> {
    let tool_call = tool_call_for(sources, call)?.clone();
    if sources.assistant.stop_reason == StopReason::Length {
        return Ok(PreparedToolInvocation::Outcome {
            outcome: truncated_outcome(&tool_call),
        });
    }
    let prepared = match prepare_tool_call(tool_call.clone(), tools) {
        PrepareOutcome::Prepared(prepared) => prepared,
        PrepareOutcome::Immediate(immediate) => {
            return Ok(PreparedToolInvocation::Outcome {
                outcome: outcome_from_immediate(immediate),
            });
        }
    };

    let decision = match lane
        .hooks()
        .run_tool_with_gate(
            HookInvocation {
                lane: lane.name().to_owned(),
                run_id: drive.operation_id().to_owned(),
                event: HookEvent::BeforeTool(BeforeToolEvent {
                    tool_call_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    args: prepared.args.clone(),
                }),
            },
            Arc::new(drive.gate().clone()),
            drive.context().clone(),
        )
        .await
    {
        Ok(HookResult::BeforeTool(result)) => result.map(|result| BeforeToolDecision {
            args: result.args,
            block: result.block,
        }),
        Ok(other) => {
            return Err(anyhow::anyhow!(
                "hook registered for {} returned a {} result",
                HookName::BeforeTool.as_str(),
                other.hook_name().as_str()
            ));
        }
        Err(error) => match abort_refusal(&error) {
            Some(AbortRefusal::Requested(abort)) => {
                abort.cancellation.cancelled().await;
                return Ok(PreparedToolInvocation::Outcome {
                    outcome: aborted_outcome(&tool_call),
                });
            }
            Some(AbortRefusal::CancelledContext) => {
                return Ok(PreparedToolInvocation::Outcome {
                    outcome: aborted_outcome(&tool_call),
                });
            }
            None => return Err(error),
        },
    };
    let cleared = apply_before_tool_decision(prepared, decision.as_ref());
    Ok(match cleared {
        Ok(cleared) => PreparedToolInvocation::Ready { cleared },
        Err(immediate) => PreparedToolInvocation::Outcome {
            outcome: outcome_from_immediate(immediate),
        },
    })
}

/// Upstream `startToolInvocation` (`tools.ts:475-513`): prepare, publish the
/// durable intent, and chain execution plus outcome publication.
#[allow(clippy::too_many_arguments)] // parameter list mirrors the upstream procedure
async fn start_tool_invocation<TContext: Clone + Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    run: &OperationState,
    sources: &ToolBatchSource,
    call: &ToolCall,
    tools: &[AgentHarnessTool<TContext>],
    tool_context: &TContext,
    recovery: bool,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ToolCallTask> {
    let batch = match &run.phase {
        OperationPhase::Tools { batch } => batch.clone(),
        _ => anyhow::bail!("Tool start requires a tools operation"),
    };
    let prepared = prepare_tool_invocation(lane, drive, sources, call, tools).await?;
    let cleared = match prepared {
        PreparedToolInvocation::Outcome { outcome } => {
            let lane = Arc::clone(lane);
            let drive = Arc::clone(drive);
            let call = call.clone();
            let emit = Arc::clone(emit_tool_event);
            return Ok(ToolCallTask {
                completion: Box::pin(async move {
                    publish_tool_outcome(&lane, &drive, &call, outcome, recovery, &emit).await
                }),
            });
        }
        PreparedToolInvocation::Ready { cleared } => cleared,
    };
    let effect_pending = publish_tool_intent(
        lane,
        drive,
        call,
        &cleared.tool_call,
        cleared.args.clone(),
        cleared.tool.replay.unwrap_or(ToolReplay::Never),
        recovery,
        emit_tool_event,
    )
    .await?;
    match effect_pending {
        ContinueOperationResult::CancelRequested => {
            let outcome = aborted_outcome(&cleared.tool_call);
            let lane = Arc::clone(lane);
            let drive = Arc::clone(drive);
            let call = call.clone();
            let emit = Arc::clone(emit_tool_event);
            Ok(ToolCallTask {
                completion: Box::pin(async move {
                    publish_tool_outcome(&lane, &drive, &call, outcome, recovery, &emit).await
                }),
            })
        }
        ContinueOperationResult::Result { value } => {
            let lane = Arc::clone(lane);
            let drive = Arc::clone(drive);
            let effect_pending = value;
            let tool_context = tool_context.clone();
            let emit = Arc::clone(emit_tool_event);
            Ok(ToolCallTask {
                completion: Box::pin(async move {
                    let outcome = perform_tool_invocation(
                        &lane,
                        &drive,
                        &batch,
                        &effect_pending,
                        cleared,
                        tool_context,
                        recovery,
                        &emit,
                    )
                    .await?;
                    publish_tool_outcome(&lane, &drive, &effect_pending, outcome, recovery, &emit)
                        .await
                }),
            })
        }
    }
}

/// Upstream `recoverToolInvocation` (`tools.ts:515-540`): safe-replay a
/// persisted call or finalize it from its checkpoint.
#[allow(clippy::too_many_arguments)] // parameter list mirrors the upstream procedure
async fn recover_tool_invocation<TContext: Clone + Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    run: &OperationState,
    sources: &ToolBatchSource,
    call: &ToolCall,
    tools_by_name: &HashMap<String, AgentHarnessTool<TContext>>,
    tool_context: &TContext,
    cancelled: bool,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ToolCallTask> {
    let tool_call = tool_call_for(sources, call)?.clone();
    let batch = match &run.phase {
        OperationPhase::Tools { batch } => batch.clone(),
        _ => anyhow::bail!("Tool recovery requires a tools operation"),
    };
    let ToolCallState::EffectPending { replay } = call.state else {
        anyhow::bail!("Tool recovery requires an effect-pending call");
    };
    let tool = tools_by_name.get(&tool_call.name);
    if !cancelled
        && replay == ToolReplay::Safe
        && tool.is_some_and(|tool| tool.replay == Some(ToolReplay::Safe))
    {
        let tool = tool.cloned().expect("tool presence checked above");
        let args =
            clear_replay_checkpoint(lane, drive, &batch, call, &tool_call, emit_tool_event).await?;
        let cleared = ClearedToolCall {
            tool_call: tool_call.clone(),
            tool,
            args,
        };
        let lane = Arc::clone(lane);
        let drive = Arc::clone(drive);
        let call = call.clone();
        let tool_context = tool_context.clone();
        let emit = Arc::clone(emit_tool_event);
        return Ok(ToolCallTask {
            completion: Box::pin(async move {
                let outcome = perform_tool_invocation(
                    &lane,
                    &drive,
                    &batch,
                    &call,
                    cleared,
                    tool_context,
                    true,
                    &emit,
                )
                .await?;
                publish_tool_outcome(&lane, &drive, &call, outcome, true, &emit).await
            }),
        });
    }
    let checkpoint = read_checkpoint(lane, drive, call).await?;
    let outcome = interrupted_outcome(&tool_call, checkpoint);
    let lane = Arc::clone(lane);
    let drive = Arc::clone(drive);
    let call = call.clone();
    let emit = Arc::clone(emit_tool_event);
    Ok(ToolCallTask {
        completion: Box::pin(async move {
            publish_tool_outcome(&lane, &drive, &call, outcome, true, &emit).await
        }),
    })
}

/// Upstream `runSequential` (`tools.ts:542-609`).
async fn run_sequential<TContext: Clone + Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    run: &OperationState,
    sources: &ToolBatchSource,
    execution: Option<ToolExecutables<TContext>>,
    recovery: bool,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ProcedureResult> {
    let bound = match &run.phase {
        OperationPhase::Tools { batch } => batch.calls.len() * 2 + 1,
        _ => anyhow::bail!("Sequential tools requires a tools operation"),
    };
    for _transition in 0..=bound {
        materialize_ready(lane, drive, run, sources, recovery).await?;
        let Some(current) = current_batch(lane) else {
            return Ok(ProcedureResult::Continue);
        };
        let current_calls = match &current.phase {
            OperationPhase::Tools { batch } => batch.calls.clone(),
            _ => anyhow::bail!("Sequential tools requires a tools operation"),
        };
        let Some(call) = current_calls
            .iter()
            .find(|candidate| !matches!(candidate.state, ToolCallState::Completed { .. }))
            .cloned()
        else {
            return Err(invariant(
                "Tool batch remained open after every call completed",
            ));
        };
        if matches!(call.state, ToolCallState::OutcomeReady { .. }) {
            return Err(invariant("Ready tool outcome was not materialized"));
        }

        if matches!(current.scope.control, Control::CancelRequested { .. }) {
            let tool_call = tool_call_for(sources, &call)?.clone();
            if matches!(call.state, ToolCallState::Planned) {
                publish_tool_outcome(
                    lane,
                    drive,
                    &call,
                    aborted_outcome(&tool_call),
                    recovery,
                    emit_tool_event,
                )
                .await?;
            } else {
                let checkpoint = read_checkpoint(lane, drive, &call).await?;
                publish_tool_outcome(
                    lane,
                    drive,
                    &call,
                    interrupted_outcome(&tool_call, checkpoint),
                    recovery,
                    emit_tool_event,
                )
                .await?;
            }
            continue;
        }

        let Some(execution) = &execution else {
            return Err(invariant("Running tool batch is missing execution context"));
        };
        let started = if matches!(call.state, ToolCallState::Planned) {
            start_tool_invocation(
                lane,
                drive,
                &current,
                sources,
                &call,
                &execution.tools,
                &execution.tool_context,
                recovery,
                emit_tool_event,
            )
            .await?
        } else {
            recover_tool_invocation(
                lane,
                drive,
                &current,
                sources,
                &call,
                &execution.tools_by_name,
                &execution.tool_context,
                false,
                emit_tool_event,
            )
            .await?
        };
        started.completion.await?;
    }
    Err(invariant(
        "Sequential tool batch exceeded its bounded transition count",
    ))
}

/// One serialized `materializeReady` continuation for parallel scheduling
/// (upstream the `materialization` promise chain of `runParallel`).
#[derive(Clone)]
struct MaterializationScheduler {
    lane: Arc<Lane>,
    drive: Arc<Drive>,
    run: OperationState,
    sources: ToolBatchSource,
    recovery: bool,
    chain: Arc<Mutex<Shared<BoxFuture<'static, ()>>>>,
}

/// A shared materialization failure replayed as a typed error.
#[derive(Debug)]
struct SharedMaterializationError(Arc<anyhow::Error>);

impl fmt::Display for SharedMaterializationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SharedMaterializationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}

type ScheduledMaterialization = Shared<BoxFuture<'static, Result<(), Arc<anyhow::Error>>>>;

impl MaterializationScheduler {
    fn schedule(&self) -> ScheduledMaterialization {
        let previous = self
            .chain
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let lane = Arc::clone(&self.lane);
        let drive = Arc::clone(&self.drive);
        let run = self.run.clone();
        let sources = self.sources.clone();
        let recovery = self.recovery;
        let scheduled = async move {
            previous.await;
            materialize_ready(&lane, &drive, &run, &sources, recovery)
                .await
                .map_err(Arc::new)
        }
        .boxed()
        .shared();
        // Upstream: `materialization = scheduled.catch(() => {})` — the stored
        // chain swallows failures so later schedules still run.
        let swallowed = {
            let scheduled = scheduled.clone();
            async move {
                let _ = scheduled.await;
            }
            .boxed()
            .shared()
        };
        *self.chain.lock().unwrap_or_else(PoisonError::into_inner) = swallowed;
        scheduled
    }
}

/// Upstream `runParallel` (`tools.ts:611-653`): start every pending call,
/// overlapping executions while serializing outcome materialization.
#[allow(clippy::too_many_arguments)] // parameter list mirrors the upstream procedure
async fn run_parallel<TContext: Clone + Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    run: &OperationState,
    sources: &ToolBatchSource,
    tools: Vec<AgentHarnessTool<TContext>>,
    tools_by_name: HashMap<String, AgentHarnessTool<TContext>>,
    tool_context: TContext,
    recovery: bool,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ProcedureResult> {
    let batch = match &run.phase {
        OperationPhase::Tools { batch } => batch.clone(),
        _ => anyhow::bail!("Parallel tools requires a tools operation"),
    };
    let scheduler = MaterializationScheduler {
        lane: Arc::clone(lane),
        drive: Arc::clone(drive),
        run: run.clone(),
        sources: sources.clone(),
        recovery,
        chain: Arc::new(Mutex::new(futures::future::ready(()).boxed().shared())),
    };
    let mut jobs: Vec<JoinHandle<anyhow::Result<()>>> = Vec::new();
    for call in &batch.calls {
        if matches!(
            call.state,
            ToolCallState::Completed { .. } | ToolCallState::OutcomeReady { .. }
        ) {
            continue;
        }
        let call = call.clone();
        // Upstream `lane.state.operation!.state.control.status`; a missing
        // operation reads as not cancelled (the recovery path fails on its
        // own invariant otherwise).
        let cancelled = lane
            .state()
            .operation
            .map(|operation| {
                matches!(
                    operation.state.scope.control,
                    Control::CancelRequested { .. }
                )
            })
            .unwrap_or(false);
        let started = if matches!(call.state, ToolCallState::Planned) {
            start_tool_invocation(
                lane,
                drive,
                run,
                sources,
                &call,
                &tools,
                &tool_context,
                recovery,
                emit_tool_event,
            )
            .await?
        } else {
            recover_tool_invocation(
                lane,
                drive,
                run,
                sources,
                &call,
                &tools_by_name,
                &tool_context,
                cancelled,
                emit_tool_event,
            )
            .await?
        };
        let job_scheduler = scheduler.clone();
        jobs.push(tokio::spawn(async move {
            started.completion.await?;
            // `started.completion.then(async () => { await
            // scheduleMaterialization(); })`: scheduling happens only after a
            // fulfilled completion.
            job_scheduler
                .schedule()
                .await
                .map_err(|error| anyhow::Error::new(SharedMaterializationError(error)))?;
            anyhow::Ok(())
        }));
    }
    let mut first_error: Option<anyhow::Error> = None;
    for job in jobs {
        let outcome = match job.await {
            Ok(outcome) => outcome,
            Err(error) => Err(anyhow::Error::new(error)),
        };
        if let Err(error) = outcome {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    scheduler
        .schedule()
        .await
        .map_err(|error| anyhow::Error::new(SharedMaterializationError(error)))?;
    Ok(ProcedureResult::Continue)
}

/// Execute, recover, stage, and source-order one complete durable tool batch
/// (upstream `runTools`, `tools.ts:655-692`).
pub async fn run_tools<TContext: Clone + Send + Sync + 'static>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    run: &OperationState,
    tools: &[AgentHarnessTool<TContext>],
    tool_context_source: &ToolContextSource<TContext>,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ProcedureResult> {
    run_tools_from_config(
        lane,
        drive,
        run,
        || (tools.to_vec(), tool_context_source.clone()),
        emit_tool_event,
    )
    .await
}

/// Installed drives read live configuration at upstream's `runTools`
/// boundary, after source reads/materialization and cancellation. Direct
/// `run_tools` callers retain their explicit snapshot environment.
pub(crate) async fn run_tools_from_config<TContext, F>(
    lane: &Arc<Lane>,
    drive: &Arc<Drive>,
    run: &OperationState,
    read_config: F,
    emit_tool_event: &ToolEventEmit,
) -> anyhow::Result<ProcedureResult>
where
    TContext: Clone + Send + Sync + 'static,
    F: FnOnce() -> (Vec<AgentHarnessTool<TContext>>, ToolContextSource<TContext>) + Send,
{
    let batch = match &run.phase {
        OperationPhase::Tools { batch } => batch.clone(),
        _ => anyhow::bail!("runTools requires a tools operation"),
    };
    let recovery = batch.calls.iter().any(|call| {
        matches!(
            call.state,
            ToolCallState::EffectPending { .. } | ToolCallState::OutcomeReady { .. }
        )
    });
    if recovery {
        lane.emit_batch(
            vec![HarnessEvent::TurnStart {
                lane: lane.name().to_owned(),
                run_id: drive.operation_id().to_owned(),
                turn_id: batch.turn_id.clone(),
                recovery: true,
            }],
            drive.context().clone(),
        )
        .await?;
    }
    let sources = read_tool_batch_source(lane, drive, &batch).await?;
    materialize_ready(lane, drive, run, &sources, recovery).await?;
    let Some(current) = current_batch(lane) else {
        return Ok(ProcedureResult::Continue);
    };
    if matches!(current.scope.control, Control::CancelRequested { .. }) {
        return run_sequential::<TContext>(
            lane,
            drive,
            &current,
            &sources,
            None,
            recovery,
            emit_tool_event,
        )
        .await;
    }
    // No configuration lock crosses an await. A later batch gets a fresh
    // snapshot and resolves its context once, independently of this batch.
    let (tools, tool_context_source) = read_config();
    let active: HashSet<&str> = batch
        .configuration
        .active_tool_names
        .iter()
        .map(String::as_str)
        .collect();
    let tools: Vec<AgentHarnessTool<TContext>> = tools
        .iter()
        .filter(|tool| active.contains(tool.name.as_str()))
        .cloned()
        .collect();
    let tools_by_name: HashMap<String, AgentHarnessTool<TContext>> = tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.clone()))
        .collect();
    let tool_context = resolve_tool_context(&tool_context_source, drive).await?;
    if run.scope.settings.tool_execution == ToolExecutionMode::Sequential {
        run_sequential(
            lane,
            drive,
            &current,
            &sources,
            Some(ToolExecutables {
                tools,
                tools_by_name,
                tool_context,
            }),
            recovery,
            emit_tool_event,
        )
        .await
    } else {
        run_parallel(
            lane,
            drive,
            &current,
            &sources,
            tools,
            tools_by_name,
            tool_context,
            recovery,
            emit_tool_event,
        )
        .await
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
