//! Port of `packages/agent/src/harness/execution/tools.ts` (205 lines): the
//! phase functions one tool call passes through — prepare (resolve, argument
//! shim, validation), before-tool decision, gated execution, after-tool
//! patching, and the transcript-message conversions.
//!
//! Disclosed substitutions:
//! - **Return shapes.** Upstream returns union values
//!   (`PreparedToolCall | ImmediateToolOutcome`, `ClearedToolCall |
//!   ImmediateToolOutcome`); the port uses a [`PrepareOutcome`] enum and
//!   `Result<ClearedToolCall, ImmediateToolOutcome>` with the same variants.
//! - **`validateToolArguments`.** The ai-layer port
//!   ([`crate::ai::validation::validate_tool_arguments`]) returns
//!   `Result<Value, String>`; upstream throws. The error string is the
//!   message an immediate outcome carries.
//! - **`prepareArguments` cannot fail.** The M3a
//!   [`PrepareArgumentsFn`](crate::agent_core::types::PrepareArgumentsFn) is
//!   an infallible closure, so the upstream `catch` around the shim
//!   (`tools.ts:87-97`) collapses to validation errors alone — the same
//!   substitution the agent-loop port made (`agent_loop.rs`, upstream
//!   agent-loop.ts:678-680).
//! - **`executeToolCall` admission.** Upstream's synchronous
//!   `gate.admit(async () => ...)` throw becomes
//!   `Result<BoxFuture, GateRejection>`: the admission check still runs
//!   synchronously before the future is returned, so a refused call is
//!   observable without awaiting it and the tool is never invoked.
//! - **`AfterToolPatch`.** Field-identical to the hooks aggregate
//!   [`hooks::AfterToolHookResult`] (`agent-harness.ts:478-486`), so the port
//!   aliases it instead of duplicating the type. `BeforeToolDecision.block`
//!   likewise reuses [`hooks::ToolBlock`].
//! - `Date.now()` for the tool-result timestamp is
//!   [`crate::ai::now_ms`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::with_abort_signal;
use crate::agent_core::harness::execution::effect_gate::{EffectGate, GateRejection};
use crate::agent_core::harness::hooks;
use crate::agent_core::harness::types::{
    AgentHarnessTool, AgentHarnessToolInvocation, AgentHarnessToolUpdateCallback,
    AgentHarnessToolUpdateOptions,
};
use crate::agent_core::types::AgentToolResult;
use crate::ai::now_ms;
use crate::ai::types::content::ToolCall;
use crate::ai::types::message::ToolResultMessage;
use crate::ai::validation::validate_tool_arguments;

/// A tool call whose tool exists and whose prepared arguments passed
/// validation (upstream `PreparedToolCall`, `tools.ts:9-13`).
#[derive(Clone, Debug)]
pub struct PreparedToolCall<TContext: Send + Sync + 'static> {
    /// The provider call, preserved verbatim (upstream keeps the original
    /// `call` object, not the prepared-arguments copy).
    pub tool_call: ToolCall,
    /// The resolved harness tool.
    pub tool: AgentHarnessTool<TContext>,
    /// The validated (post-shim) arguments.
    pub args: serde_json::Value,
}

/// Synthetic result produced without crossing the external tool-effect
/// boundary (upstream `ImmediateToolOutcome`, `tools.ts:16-22`).
#[derive(Debug)]
pub struct ImmediateToolOutcome {
    pub tool_call: ToolCall,
    pub result: AgentToolResult,
    /// Always `true` upstream; kept as a field for the wire shape.
    pub is_error: bool,
    pub terminate: bool,
}

/// Upstream `prepareToolCall`'s return union (`tools.ts:78`).
pub enum PrepareOutcome<TContext: Send + Sync + 'static> {
    Prepared(PreparedToolCall<TContext>),
    Immediate(ImmediateToolOutcome),
}

/// Aggregated decision from the before-tool hook pipeline (upstream
/// `BeforeToolDecision`, `tools.ts:25-28`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeforeToolDecision {
    pub args: Option<serde_json::Value>,
    pub block: Option<hooks::ToolBlock>,
}

/// A prepared call cleared for durable intent publication and execution
/// (upstream `ClearedToolCall`, `tools.ts:31-35`).
#[derive(Clone, Debug)]
pub struct ClearedToolCall<TContext: Send + Sync + 'static> {
    pub tool_call: ToolCall,
    pub tool: AgentHarnessTool<TContext>,
    pub args: serde_json::Value,
}

/// Raw phase-two tool output before after-tool patching (upstream
/// `ExecutedToolCall`, `tools.ts:38-41`).
pub struct ExecutedToolCall {
    pub result: AgentToolResult,
    pub is_error: bool,
}

/// Aggregated patch from the after-tool hook pipeline (upstream
/// `AfterToolPatch`, `tools.ts:44-50`): field-identical to the hooks
/// aggregate, so the port aliases it.
pub type AfterToolPatch = hooks::AfterToolHookResult;

/// Final tool output ready to become a durable tool-result message (upstream
/// `FinalizedToolCall`, `tools.ts:53-58`; upstream's `TContext` parameter is
/// phantom there — no field uses it — so the port drops it).
pub struct FinalizedToolCall {
    pub tool_call: ToolCall,
    pub result: AgentToolResult,
    pub is_error: bool,
    pub terminate: bool,
}

/// Upstream `createErrorToolResult` (`tools.ts:60-65`).
fn create_error_tool_result(message: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![crate::ai::types::TextOrImageBlock::Text(
            crate::ai::types::TextContent {
                text: message.to_string(),
                text_signature: None,
            },
        )],
        details: None,
        usage: None,
        terminate: None,
    }
}

/// Upstream `immediateError` (`tools.ts:67-75`).
fn immediate_error(tool_call: ToolCall, message: &str, terminate: bool) -> ImmediateToolOutcome {
    ImmediateToolOutcome {
        tool_call,
        result: create_error_tool_result(message),
        is_error: true,
        terminate,
    }
}

/// Upstream `prepareToolCall` (`tools.ts:78-98`): resolve a tool, apply its
/// deterministic argument preparation, and validate the result. The provider
/// call is preserved verbatim even when the shim rewrote the arguments.
pub fn prepare_tool_call<TContext: Send + Sync + 'static>(
    call: ToolCall,
    tools: &[AgentHarnessTool<TContext>],
) -> PrepareOutcome<TContext> {
    let Some(tool) = tools.iter().find(|candidate| candidate.name == call.name) else {
        let unavailable = format!(
            "Tool {} is unavailable",
            serde_json::to_string(&call.name).unwrap_or_default()
        );
        return PrepareOutcome::Immediate(immediate_error(call, &unavailable, false));
    };
    let tool = tool.clone();

    let prepared_arguments = match &tool.prepare_arguments {
        Some(prepare) => prepare(call.arguments.clone()),
        None => call.arguments.clone(),
    };
    let validated = validate_tool_arguments(
        &tool.declaration(),
        &ToolCall {
            arguments: prepared_arguments,
            ..call.clone()
        },
    );
    match validated {
        Ok(args) => PrepareOutcome::Prepared(PreparedToolCall {
            tool_call: call,
            tool,
            args,
        }),
        Err(message) => PrepareOutcome::Immediate(immediate_error(call, &message, false)),
    }
}

/// Upstream `applyBeforeToolDecision` (`tools.ts:101-122`): apply an explicit
/// hook decision and revalidate replacement arguments. The refusal shape is
/// returned by value like upstream's union, so the large-Err lint is allowed
/// here rather than boxing the outcome.
#[allow(clippy::result_large_err)]
pub fn apply_before_tool_decision<TContext: Send + Sync + 'static>(
    prepared: PreparedToolCall<TContext>,
    decision: Option<&BeforeToolDecision>,
) -> Result<ClearedToolCall<TContext>, ImmediateToolOutcome> {
    let block = decision.and_then(|decision| decision.block.as_ref());
    if let Some(block) = block {
        return Err(immediate_error(
            prepared.tool_call,
            &block.reason,
            block.terminate == Some(true),
        ));
    }

    let args = decision.and_then(|decision| decision.args.clone());
    let Some(args) = args else {
        return Ok(ClearedToolCall {
            tool_call: prepared.tool_call,
            tool: prepared.tool,
            args: prepared.args,
        });
    };

    match validate_tool_arguments(
        &prepared.tool.declaration(),
        &ToolCall {
            arguments: args,
            ..prepared.tool_call.clone()
        },
    ) {
        Ok(validated_args) => Ok(ClearedToolCall {
            tool_call: prepared.tool_call,
            tool: prepared.tool,
            args: validated_args,
        }),
        Err(message) => Err(immediate_error(prepared.tool_call, &message, false)),
    }
}

/// Upstream `executeToolCall` (`tools.ts:125-158`): execute one cleared
/// external tool effect, converting expected tool throws to error output.
/// The gate's synchronous admission check runs before the returned future,
/// so a refused effect surfaces as `Err` without awaiting (and without
/// invoking the tool). Late `onUpdate` calls after settlement are ignored
/// (`acceptingUpdates`).
pub fn execute_tool_call<TContext: Send + Sync + 'static>(
    call: ClearedToolCall<TContext>,
    gate: &EffectGate,
    on_update: Arc<AgentHarnessToolUpdateCallback>,
    tool_context: TContext,
    invocation: Arc<dyn AgentHarnessToolInvocation>,
    context: Context,
) -> Result<BoxFuture<'static, ExecutedToolCall>, GateRejection> {
    gate.admit(|| ())?;
    let signal = gate.signal();
    Ok(Box::pin(async move {
        let ClearedToolCall {
            tool_call,
            tool,
            args,
        } = call;
        let admitted_context = with_abort_signal(signal, context);
        let accepting_updates = Arc::new(AtomicBool::new(true));
        let update_callback: Arc<AgentHarnessToolUpdateCallback> = {
            let accepting_updates = Arc::clone(&accepting_updates);
            let on_update = Arc::clone(&on_update);
            Arc::new(
                move |partial: &AgentToolResult, options: AgentHarnessToolUpdateOptions| {
                    if accepting_updates.load(Ordering::SeqCst) {
                        on_update(partial, options);
                    }
                },
            )
        };
        // `admittedContext.abortSignal?.throwIfAborted()` inside the try
        // (`tools.ts:136`): a cancelled admitted context surfaces as an error
        // result, not a throw.
        let executed = if admitted_context
            .abort_signal()
            .is_some_and(|signal| signal.is_cancelled())
        {
            Err(anyhow::anyhow!("the operation was aborted"))
        } else {
            (tool.execute)(
                tool_call.id.clone(),
                args.clone(),
                update_callback,
                tool_context,
                invocation,
                admitted_context,
            )
            .await
        };
        // `finally { acceptingUpdates = false }` (`tools.ts:154-156`).
        accepting_updates.store(false, Ordering::SeqCst);
        match executed {
            Ok(result) => ExecutedToolCall {
                result,
                is_error: false,
            },
            Err(error) => ExecutedToolCall {
                result: create_error_tool_result(&error.to_string()),
                is_error: true,
            },
        }
    }))
}

/// Upstream `finalizeToolCall` (`tools.ts:161-181`): apply an after-tool
/// patch field by field (`None` keeps the executed value).
pub fn finalize_tool_call<TContext: Send + Sync + 'static>(
    call: &ClearedToolCall<TContext>,
    executed: ExecutedToolCall,
    patch: Option<&AfterToolPatch>,
) -> FinalizedToolCall {
    let mut result = executed.result;
    if let Some(patch) = patch {
        if let Some(content) = &patch.content {
            result.content = content.clone();
        }
        if let Some(details) = &patch.details {
            result.details = Some(details.clone());
        }
        if patch.usage.is_some() {
            result.usage = patch.usage;
        }
        if let Some(terminate) = patch.terminate {
            result.terminate = Some(terminate);
        }
    }
    let terminate = result.terminate == Some(true);
    FinalizedToolCall {
        tool_call: call.tool_call.clone(),
        is_error: patch
            .and_then(|patch| patch.is_error)
            .unwrap_or(executed.is_error),
        result,
        terminate,
    }
}

/// Upstream `toolResultFromMessage` (`tools.ts:184-191`): reconstruct the
/// canonical tool result represented by a staged transcript message.
pub fn tool_result_from_message(message: &ToolResultMessage, terminate: bool) -> AgentToolResult {
    AgentToolResult {
        content: message.content.clone(),
        details: message.details.clone(),
        usage: message.usage,
        terminate: if terminate { Some(true) } else { None },
    }
}

/// Upstream `createToolResultMessage` (`tools.ts:194-205`): convert finalized
/// tool output to the provider-facing transcript message.
pub fn create_tool_result_message(call: &FinalizedToolCall) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: call.tool_call.id.clone(),
        tool_name: call.tool_call.name.clone(),
        content: call.result.content.clone(),
        details: call.result.details.clone(),
        usage: call.result.usage,
        is_error: call.is_error,
        timestamp: now_ms(),
    }
}

#[cfg(test)]
mod tests;
