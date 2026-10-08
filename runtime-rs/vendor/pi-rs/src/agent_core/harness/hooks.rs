//! Port of `packages/agent/src/harness/hooks.ts` (533 lines): the ordered
//! harness hook registry, its per-hook aggregates, and the stream-options
//! patch algebra.
//!
//! Semantics preserved from upstream, structure adapted to Rust futures:
//! - **Handler surface.** Upstream types each hook through
//!   `HookMap[TName]["event"]`/`["result"]`; the port packs the same twelve
//!   event and result shapes into the [`HookEvent`]/[`HookResult`] enums
//!   dispatched by [`HookName`]. Upstream casts handler results blindly
//!   (`result as HookMap[TName]["result"]`, `hooks.ts:86`); the port treats a
//!   result variant that does not match the run's hook name as a port bug and
//!   surfaces it as an error.
//! - **Errors.** Upstream `try { await handler() } catch` around every
//!   handler maps onto the handler's `Result` error channel (the handler
//!   signature is fallible, so panics are bugs, not control flow — unlike the
//!   infallible event-bus listeners). Upstream's
//!   `error instanceof Error ? error : new Error(String(error))`
//!   normalization has no JS throw channel to normalize; errors flow as
//!   [`anyhow::Error`].
//! - **Gate.** Upstream `Gate` (`execution/effect-gate.ts:13-16`) becomes the
//!   [`Gate`] trait hosted here; `admit(invoke)`'s synchronous check becomes
//!   an [`Gate::admit`] predicate the run calls in its synchronous prologue
//!   before any handler starts (upstream `admit` throws before invoking the
//!   wrapped effect). `createGate`/`GateControl`/`AbortRequested` stay with
//!   the execution module (M3b Task 6), which implements this trait.
//!   `admittedContext.abortSignal?.throwIfAborted()` becomes a cancelled-check
//!   with the fixed `"the operation was aborted"` message
//!   (`CancellationToken` carries no reason; the `await_with_context`
//!   precedent).
//! - **Telemetry.** Upstream wraps every `before_tool`/`after_tool` handler in
//!   a `pi.harness.hook` span (`hooks.ts:370-403`, schema
//!   `telemetry.ts:453-489`). The port keeps the exact seam
//!   ([`HookRegistry::invoke_tool_registration`]); the span wrapper lands with
//!   the telemetry module (M3b Task 10). Span attributes are telemetry-only
//!   and have no functional effect.
//! - **Deferred types.** The compaction-side hook payloads
//!   ([`CompactionPreparation`], [`CompactResult`], [`BranchPreparation`],
//!   [`BranchSummaryResult`]) are re-exported from the compaction module
//!   (M3b Task 5) — upstream `firstStructural` only inspects `decline` and
//!   the result field (`hooks.ts:337-368`), so the hook layer treats them as
//!   opaque values. [`SettledAssistantMessage`] aliases `AssistantMessage`
//!   with the upstream invariant (`stopReason` narrowed to non-`pending`,
//!   `session/types.ts:12-14`) kept by construction sites. [`Resources`]
//!   re-exports the foundation `AgentHarnessResources` (`agent-harness.ts:426`).
//! - **Identity vs value.** Upstream compares object identity in two places:
//!   `args === event.args` before omitting `args` from the `before_tool`
//!   aggregate (`hooks.ts:183`) and `base.headers !== value.headers` when
//!   diffing maps (`hooks.ts:504`). The port compares by value — equivalent
//!   for consumers, since equal values carry no diff information.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::anyhow;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::with_abort_signal;
use crate::agent_core::harness::events::Unsubscribe;
use crate::agent_core::harness::types::{
    AgentHarnessResources, AgentHarnessStreamOptions, AgentHarnessStreamOptionsPatch,
};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::message::{AssistantMessage, TextOrImageBlock};
use crate::ai::types::model::Model;
use crate::ai::types::primitives::Usage;

/// Upstream `Resources` (`agent-harness.ts:426`): the foundation resource
/// shapes.
pub type Resources = AgentHarnessResources;

/// Upstream `SettledAssistantMessage` (`session/types.ts:12-14`):
/// `AssistantMessage` whose `stop_reason` is not [`crate::ai::types::primitives::StopReason::Pending`].
/// The refinement is an invariant of the producers (the assistant execution
/// path), not a separate type.
pub type SettledAssistantMessage = AssistantMessage;

/// Upstream `CompactionPreparation` (`compaction/compaction.ts:614-638`),
/// re-exported from the compaction module (M3b Task 5); opaque to the hook
/// layer.
pub use crate::agent_core::harness::compaction::CompactionPreparation;

/// Upstream `CompactResult<T>` (`compaction/compaction.ts:98-112`),
/// re-exported from the compaction module (M3b Task 5); opaque to the hook
/// layer.
pub use crate::agent_core::harness::compaction::CompactResult;

/// Upstream `BranchPreparation`
/// (`compaction/branch-summarization.ts:51-60`), re-exported from the
/// compaction module (M3b Task 5); opaque to the hook layer.
pub use crate::agent_core::harness::compaction::BranchPreparation;

/// Upstream `BranchSummaryResult`
/// (`compaction/branch-summarization.ts:33-39`), re-exported from the
/// compaction module (M3b Task 5); opaque to the hook layer.
pub use crate::agent_core::harness::compaction::BranchSummaryResult;

/// Upstream `HookName` (`agent-harness.ts:502`): the twelve hook names, with
/// their upstream wire literals (the `HOOK_NAMES` telemetry vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookName {
    BeforeRun,
    BeforeDrive,
    BeforeRunEnd,
    TransformContext,
    BeforeRequest,
    BeforePayload,
    AfterResponse,
    BeforeTool,
    AfterTool,
    BeforeCompaction,
    BeforeNavigation,
}

impl HookName {
    /// The upstream literal (`"before_run"`, ...), used in error messages and
    /// telemetry attributes.
    pub fn as_str(&self) -> &'static str {
        match self {
            HookName::BeforeRun => "before_run",
            HookName::BeforeDrive => "before_drive",
            HookName::BeforeRunEnd => "before_run_end",
            HookName::TransformContext => "transform_context",
            HookName::BeforeRequest => "before_request",
            HookName::BeforePayload => "before_payload",
            HookName::AfterResponse => "after_response",
            HookName::BeforeTool => "before_tool",
            HookName::AfterTool => "after_tool",
            HookName::BeforeCompaction => "before_compaction",
            HookName::BeforeNavigation => "before_navigation",
        }
    }
}

/// Upstream `operation: "run" | "compaction" | "navigation"` of the
/// `before_drive` event (`agent-harness.ts:436`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveOperation {
    Run,
    Compaction,
    Navigation,
}

/// Upstream `step: "assistant" | "deferred" | "compaction" | "branch_summary"`
/// of the `before_request` event (`agent-harness.ts:450`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStep {
    Assistant,
    Deferred,
    Compaction,
    BranchSummary,
}

/// Upstream `reason: "manual" | "threshold" | "overflow"` shared by the
/// `before_compaction` event (`agent-harness.ts:490`) and the compaction
/// events (`agent-harness.ts:359`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

// ---------------------------------------------------------------------------
// Hook events (upstream `HookMap[...]["event"]`, `agent-harness.ts:430-500`)
// ---------------------------------------------------------------------------

/// `before_run` event (`agent-harness.ts:432`).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeRunEvent {
    pub prompt: Vec<AgentMessage>,
    pub resources: Resources,
}

/// `before_drive` event (`agent-harness.ts:435-437`).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeDriveEvent {
    pub operation: DriveOperation,
}

/// `before_run_end` event (`agent-harness.ts:438-440`).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeRunEndEvent {
    pub messages: Vec<AgentMessage>,
}

/// `transform_context` event (`agent-harness.ts:443-445`).
#[derive(Debug, Clone, PartialEq)]
pub struct TransformContextEvent {
    pub messages: Vec<AgentMessage>,
    pub system_prompt: String,
}

/// `before_request` event (`agent-harness.ts:446-452`).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeRequestEvent {
    pub model: Model,
    pub step: RequestStep,
    pub attempt: u32,
    pub stream_options: AgentHarnessStreamOptions,
}

/// `before_payload` event (`agent-harness.ts:456-458`); the payload is the
/// provider request body (erased to JSON, like upstream `unknown`).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforePayloadEvent {
    pub model: Model,
    pub payload: serde_json::Value,
}

/// `after_response` event (`agent-harness.ts:460-462`).
#[derive(Debug, Clone, PartialEq)]
pub struct AfterResponseEvent {
    pub status: Option<u16>,
    pub headers: Option<BTreeMap<String, String>>,
    pub message: SettledAssistantMessage,
}

/// `before_tool` event (`agent-harness.ts:464-466`); args are the validated
/// tool-call arguments (`ToolCall.arguments` JSON convention).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeToolEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
}

/// `after_tool` event (`agent-harness.ts:467-477`).
#[derive(Debug, Clone, PartialEq)]
pub struct AfterToolEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub content: Vec<TextOrImageBlock>,
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
    pub usage: Option<Usage>,
}

/// `before_compaction` event (`agent-harness.ts:488-492`).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeCompactionEvent {
    pub reason: CompactionReason,
    pub preparation: CompactionPreparation,
    pub custom_instructions: Option<String>,
}

/// `before_navigation` event (`agent-harness.ts:496-498`).
#[derive(Debug, Clone, PartialEq)]
pub struct BeforeNavigationEvent {
    pub target_id: String,
    pub preparation: BranchPreparation,
    pub custom_instructions: Option<String>,
}

/// One hook event, tagged by [`HookName`] (upstream: the `HookMap` event
/// intersection with `{ lane, runId }`, carried by [`HookInvocation`]).
#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    BeforeRun(BeforeRunEvent),
    BeforeDrive(BeforeDriveEvent),
    BeforeRunEnd(BeforeRunEndEvent),
    TransformContext(TransformContextEvent),
    BeforeRequest(BeforeRequestEvent),
    BeforePayload(BeforePayloadEvent),
    AfterResponse(AfterResponseEvent),
    BeforeTool(BeforeToolEvent),
    AfterTool(AfterToolEvent),
    BeforeCompaction(BeforeCompactionEvent),
    BeforeNavigation(BeforeNavigationEvent),
}

impl HookEvent {
    /// The hook this event belongs to.
    pub fn hook_name(&self) -> HookName {
        match self {
            HookEvent::BeforeRun(_) => HookName::BeforeRun,
            HookEvent::BeforeDrive(_) => HookName::BeforeDrive,
            HookEvent::BeforeRunEnd(_) => HookName::BeforeRunEnd,
            HookEvent::TransformContext(_) => HookName::TransformContext,
            HookEvent::BeforeRequest(_) => HookName::BeforeRequest,
            HookEvent::BeforePayload(_) => HookName::BeforePayload,
            HookEvent::AfterResponse(_) => HookName::AfterResponse,
            HookEvent::BeforeTool(_) => HookName::BeforeTool,
            HookEvent::AfterTool(_) => HookName::AfterTool,
            HookEvent::BeforeCompaction(_) => HookName::BeforeCompaction,
            HookEvent::BeforeNavigation(_) => HookName::BeforeNavigation,
        }
    }
}

/// Upstream `HookInvocation<TName>` (`agent-harness.ts:503-506`): the event
/// plus the lane and run identifiers every hook observes.
#[derive(Debug, Clone, PartialEq)]
pub struct HookInvocation {
    pub lane: String,
    pub run_id: String,
    pub event: HookEvent,
}

// ---------------------------------------------------------------------------
// Hook results (upstream `HookMap[...]["result"]`, `agent-harness.ts:430-500`)
// ---------------------------------------------------------------------------

/// `before_run` handler result (`agent-harness.ts:433`). Handlers inject
/// messages; the aggregate carries the accumulated injections, or `None` when
/// every handler returned without messages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeforeRunHookResult {
    pub messages: Option<Vec<AgentMessage>>,
}

/// `before_run_end` handler result (`agent-harness.ts:441`); the aggregate
/// fills `follow_up` with the last follow-up seen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FollowUpHookResult {
    pub follow_up: Option<String>,
}

/// `transform_context` handler patch (`agent-harness.ts:444-445`). The
/// aggregate fills both fields with the chained values (upstream always
/// returns `{ messages, systemPrompt }`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransformContextPatch {
    pub messages: Option<Vec<AgentMessage>>,
    pub system_prompt: Option<String>,
}

/// `before_request` handler result (`agent-harness.ts:453-454`): a patch
/// applied over the snapshot the next handler observes; the aggregate carries
/// the created base-to-final diff.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeforeRequestHookResult {
    pub stream_options: AgentHarnessStreamOptionsPatch,
}

/// `before_payload` handler result (`agent-harness.ts:458`); the aggregate
/// always carries the current payload.
#[derive(Debug, Clone, PartialEq)]
pub struct PayloadHookResult {
    pub payload: serde_json::Value,
}

/// `after_response` handler result (`agent-harness.ts:462`); the aggregate
/// always carries the current message.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageHookResult {
    pub message: SettledAssistantMessage,
}

/// Upstream `block: { reason: string; terminate?: boolean }`
/// (`agent-harness.ts:466`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolBlock {
    pub reason: String,
    pub terminate: Option<bool>,
}

/// `before_tool` handler result (`agent-harness.ts:466`). The aggregate
/// omits `args` when unchanged and carries `block` when some handler blocked
/// or failed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeforeToolHookResult {
    pub args: Option<serde_json::Value>,
    pub block: Option<ToolBlock>,
}

/// `after_tool` handler result (`agent-harness.ts:478-486`): field-by-field
/// patch, `None` keeps the current value. The aggregate carries every field
/// set by any handler (or `None` in [`HookResult::AfterTool`] when no handler
/// set anything).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AfterToolHookResult {
    pub content: Option<Vec<TextOrImageBlock>>,
    pub details: Option<serde_json::Value>,
    pub is_error: Option<bool>,
    pub usage: Option<Usage>,
    pub terminate: Option<bool>,
}

/// `before_compaction` handler result (`agent-harness.ts:493-494`):
/// decline-or-result; returning both is reported and skipped (upstream
/// `firstStructural`, `hooks.ts:337-368`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeforeCompactionHookResult {
    pub decline: Option<bool>,
    pub compaction: Option<CompactResult>,
}

/// `before_navigation` handler result (`agent-harness.ts:498`), same
/// decline-or-result semantics as [`BeforeCompactionHookResult`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BeforeNavigationHookResult {
    pub decline: Option<bool>,
    pub summary: Option<BranchSummaryResult>,
}

/// One hook result, tagged by [`HookName`]. Handler returns and aggregate
/// outputs share the shapes (as upstream); the aggregate fill invariants are
/// documented on each payload struct.
///
/// The `BeforeRun`/`TransformContext` variants carry the largest payloads
/// (message arrays on every run); boxing them would add indirection at every
/// use site for no functional gain (same precedent as `AgentMessage`).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum HookResult {
    BeforeRun(Option<BeforeRunHookResult>),
    BeforeDrive,
    BeforeRunEnd(Option<FollowUpHookResult>),
    TransformContext(Option<TransformContextPatch>),
    BeforeRequest(Option<BeforeRequestHookResult>),
    BeforePayload(Option<PayloadHookResult>),
    AfterResponse(Option<MessageHookResult>),
    BeforeTool(Option<BeforeToolHookResult>),
    AfterTool(Option<AfterToolHookResult>),
    BeforeCompaction(Option<BeforeCompactionHookResult>),
    BeforeNavigation(Option<BeforeNavigationHookResult>),
}

impl HookResult {
    /// The hook this result belongs to.
    pub fn hook_name(&self) -> HookName {
        match self {
            HookResult::BeforeRun(_) => HookName::BeforeRun,
            HookResult::BeforeDrive => HookName::BeforeDrive,
            HookResult::BeforeRunEnd(_) => HookName::BeforeRunEnd,
            HookResult::TransformContext(_) => HookName::TransformContext,
            HookResult::BeforeRequest(_) => HookName::BeforeRequest,
            HookResult::BeforePayload(_) => HookName::BeforePayload,
            HookResult::AfterResponse(_) => HookName::AfterResponse,
            HookResult::BeforeTool(_) => HookName::BeforeTool,
            HookResult::AfterTool(_) => HookName::AfterTool,
            HookResult::BeforeCompaction(_) => HookName::BeforeCompaction,
            HookResult::BeforeNavigation(_) => HookName::BeforeNavigation,
        }
    }
}

fn variant_mismatch(expected: HookName, got: HookName) -> anyhow::Error {
    anyhow!(
        "hook registered for {} returned a {} result",
        expected.as_str(),
        got.as_str()
    )
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// The `(event, context) => result` handler (upstream `HookHandler`,
/// `agent-harness.ts:507-510`). The invocation is an owned snapshot; the
/// registry rebuilds it before each handler with the values earlier handlers
/// replaced (upstream `{ ...event, prompt }` spreads).
pub type HookHandlerFn =
    dyn Fn(HookInvocation, Context) -> BoxFuture<'static, anyhow::Result<HookResult>> + Send + Sync;
pub type HookHandler = Arc<HookHandlerFn>;

/// Upstream `HookErrorReporter` (`hooks.ts:12`): called for every reported
/// handler failure with the error, the hook name, the lane, and the context.
pub type HookErrorReporterFn =
    dyn Fn(anyhow::Error, HookName, String, Context) -> BoxFuture<'static, ()> + Send + Sync;
pub type HookErrorReporter = Arc<HookErrorReporterFn>;

/// One registration: the optional metadata id plus the handler (upstream
/// `HookRegistration`, `hooks.ts:7-10`).
struct HookRegistration {
    id: Option<String>,
    handler: HookHandler,
}

/// Upstream `Gate` (`execution/effect-gate.ts:13-16`): the procedure-facing
/// admission capability for one drive pass. Implemented by the execution
/// module's gate (M3b Task 6).
pub trait Gate: Send + Sync {
    /// The gate's cancellation signal (upstream `signal`); admitted hooks run
    /// with this signal joined into their context.
    fn signal(&self) -> CancellationToken;
    /// The synchronous admission check (upstream `admit(invoke)`'s state
    /// check): `Err` refuses the effect — an aborting gate ("abort requested"
    /// analogue) or a closed one — before any handler runs.
    fn admit(&self) -> anyhow::Result<()>;
}

/// Lock helper that recovers from poisoning (same rationale as the event bus:
/// reported handler failures must not wedge the registry).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct RegistryInner {
    /// Registrations in call order by hook name (`hooks.ts:16`).
    registrations: Mutex<HashMap<HookName, Vec<Arc<HookRegistration>>>>,
    report_error: HookErrorReporter,
    /// The first `close` error message (`hooks.ts:18`).
    closed_error: Mutex<Option<String>>,
}

/// Ordered harness hook registry and aggregate runner (upstream
/// `HookRegistry`, `hooks.ts:15-444`). Clone to share one registry.
#[derive(Clone)]
pub struct HookRegistry {
    inner: Arc<RegistryInner>,
}

impl HookRegistry {
    /// `new HookRegistry(reportError)`.
    pub fn new(report_error: HookErrorReporter) -> Self {
        HookRegistry {
            inner: Arc::new(RegistryInner {
                registrations: Mutex::new(HashMap::new()),
                report_error,
                closed_error: Mutex::new(None),
            }),
        }
    }

    /// Upstream `on` (`hooks.ts:24-37`): register a handler for one hook, in
    /// call order. `id` is optional metadata (upstream `options.id`; ids are
    /// not deduplicated). Fails after [`Self::close`].
    pub fn on(
        &self,
        name: HookName,
        handler: HookHandler,
        id: Option<String>,
    ) -> anyhow::Result<Unsubscribe> {
        if let Some(message) = lock(&self.inner.closed_error).clone() {
            return Err(anyhow!(message));
        }
        let registration = Arc::new(HookRegistration { id, handler });
        lock(&self.inner.registrations)
            .entry(name)
            .or_default()
            .push(Arc::clone(&registration));
        let inner = Arc::clone(&self.inner);
        Ok(Unsubscribe::new(Box::new(move || {
            if let Some(registrations) = lock(&inner.registrations).get_mut(&name) {
                registrations.retain(|existing| !Arc::ptr_eq(existing, &registration));
            }
        })))
    }

    /// Upstream `has` (`hooks.ts:39-41`).
    pub fn has(&self, name: HookName) -> bool {
        !self.registrations_for(name).is_empty()
    }

    /// Upstream `runWithGate` (`hooks.ts:43-55`): invoke one accepted-operation
    /// aggregate after the gate admits. The admission check and the
    /// admitted-context derivation run before any handler starts; the run
    /// refuses with the gate's error when admission fails and with the fixed
    /// abort message when the admitted context starts already cancelled.
    pub fn run_with_gate(
        &self,
        invocation: HookInvocation,
        gate: Arc<dyn Gate>,
        context: Context,
    ) -> BoxFuture<'static, anyhow::Result<HookResult>> {
        if let Err(error) = gate.admit() {
            return Box::pin(async move { Err(error) });
        }
        let admitted = with_abort_signal(gate.signal(), context);
        if let Some(signal) = admitted.abort_signal() {
            if signal.is_cancelled() {
                return Box::pin(async { Err(anyhow!("the operation was aborted")) });
            }
        }
        let registry = self.clone();
        Box::pin(async move { registry.run_admitted(invocation, admitted).await })
    }

    /// Upstream `runToolWithGate` (`hooks.ts:57-73`): the tool-hook entry
    /// point, restricted to `before_tool`/`after_tool` (upstream narrows the
    /// name in the type system; the port guards at run time).
    pub fn run_tool_with_gate(
        &self,
        invocation: HookInvocation,
        gate: Arc<dyn Gate>,
        context: Context,
    ) -> BoxFuture<'static, anyhow::Result<HookResult>> {
        if !matches!(
            invocation.event,
            HookEvent::BeforeTool(_) | HookEvent::AfterTool(_)
        ) {
            return Box::pin(async {
                Err(anyhow!(
                    "run_tool_with_gate accepts only before_tool and after_tool invocations"
                ))
            });
        }
        self.run_with_gate(invocation, gate, context)
    }

    /// Upstream `close` (`hooks.ts:75-77`): store the first error; later
    /// registrations and runs fail with it while in-flight pipelines complete.
    pub fn close(&self, error: anyhow::Error) {
        let mut closed = lock(&self.inner.closed_error);
        if closed.is_some() {
            return;
        }
        *closed = Some(error.to_string());
    }

    /// Upstream `runAdmitted` (`hooks.ts:79-87`): the post-admission closed
    /// check plus the aggregate dispatch.
    fn run_admitted(
        &self,
        invocation: HookInvocation,
        context: Context,
    ) -> BoxFuture<'static, anyhow::Result<HookResult>> {
        if let Some(message) = lock(&self.inner.closed_error).clone() {
            return Box::pin(async move { Err(anyhow!(message)) });
        }
        let registry = self.clone();
        Box::pin(async move {
            let lane = invocation.lane;
            let run_id = invocation.run_id;
            match invocation.event {
                HookEvent::BeforeRun(event) => {
                    registry.before_run(&lane, &run_id, event, context).await
                }
                HookEvent::BeforeDrive(event) => {
                    registry.before_drive(&lane, &run_id, event, context).await
                }
                HookEvent::BeforeRunEnd(event) => {
                    registry
                        .before_run_end(&lane, &run_id, event, context)
                        .await
                }
                HookEvent::TransformContext(event) => {
                    registry
                        .transform_context(&lane, &run_id, event, context)
                        .await
                }
                HookEvent::BeforeRequest(event) => {
                    registry
                        .before_request(&lane, &run_id, event, context)
                        .await
                }
                HookEvent::BeforePayload(event) => {
                    registry
                        .before_payload(&lane, &run_id, event, context)
                        .await
                }
                HookEvent::AfterResponse(event) => {
                    registry
                        .after_response(&lane, &run_id, event, context)
                        .await
                }
                HookEvent::BeforeTool(event) => {
                    registry.before_tool(&lane, &run_id, event, context).await
                }
                HookEvent::AfterTool(event) => {
                    registry.after_tool(&lane, &run_id, event, context).await
                }
                HookEvent::BeforeCompaction(event) => {
                    registry
                        .before_compaction(&lane, &run_id, event, context)
                        .await
                }
                HookEvent::BeforeNavigation(event) => {
                    registry
                        .before_navigation(&lane, &run_id, event, context)
                        .await
                }
            }
        })
    }

    /// Upstream `registrationsFor` (`hooks.ts:405-407`): snapshot, so handlers
    /// registering during a run never join the running pipeline.
    fn registrations_for(&self, name: HookName) -> Vec<Arc<HookRegistration>> {
        lock(&self.inner.registrations)
            .get(&name)
            .cloned()
            .unwrap_or_default()
    }

    /// Upstream `aggregate` (`hooks.ts:89-126`), inlined per hook below.
    ///
    /// `before_run` (`hooks.ts:128-154`): each handler sees the prompt with
    /// all earlier injections; failures are reported and skipped; the
    /// aggregate carries the accumulated injections or nothing.
    async fn before_run(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforeRunEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        let mut prompt = event.prompt;
        let mut injected: Vec<AgentMessage> = Vec::new();
        for registration in self.registrations_for(HookName::BeforeRun) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::BeforeRun(BeforeRunEvent {
                    prompt: prompt.clone(),
                    resources: event.resources.clone(),
                }),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(HookResult::BeforeRun(Some(result))) => {
                    if let Some(messages) = result.messages {
                        injected.extend(messages.iter().cloned());
                        prompt.extend(messages);
                    }
                }
                Ok(HookResult::BeforeRun(None)) => {}
                Ok(other) => return Err(variant_mismatch(HookName::BeforeRun, other.hook_name())),
                Err(error) => {
                    (self.inner.report_error)(
                        error,
                        HookName::BeforeRun,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                }
            }
        }
        Ok(HookResult::BeforeRun(if injected.is_empty() {
            None
        } else {
            Some(BeforeRunHookResult {
                messages: Some(injected),
            })
        }))
    }

    /// `before_drive` (`hooks.ts:93-94, 409-423`): fail closed — the first
    /// handler failure is reported and aborts the whole run.
    async fn before_drive(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforeDriveEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        for registration in self.registrations_for(HookName::BeforeDrive) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::BeforeDrive(event.clone()),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(HookResult::BeforeDrive) => {}
                Ok(other) => {
                    return Err(variant_mismatch(HookName::BeforeDrive, other.hook_name()))
                }
                Err(error) => {
                    let message = error.to_string();
                    (self.inner.report_error)(
                        anyhow!(message.clone()),
                        HookName::BeforeDrive,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                    return Err(anyhow!(message));
                }
            }
        }
        Ok(HookResult::BeforeDrive)
    }

    /// `before_run_end` (`hooks.ts:95-108, 425-443`): the last follow-up wins;
    /// failures are reported and skipped.
    async fn before_run_end(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforeRunEndEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        let mut follow_up: Option<String> = None;
        for registration in self.registrations_for(HookName::BeforeRunEnd) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::BeforeRunEnd(event.clone()),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(HookResult::BeforeRunEnd(Some(result))) => {
                    if let Some(returned) = result.follow_up {
                        follow_up = Some(returned);
                    }
                }
                Ok(HookResult::BeforeRunEnd(None)) => {}
                Ok(other) => {
                    return Err(variant_mismatch(HookName::BeforeRunEnd, other.hook_name()))
                }
                Err(error) => {
                    (self.inner.report_error)(
                        error,
                        HookName::BeforeRunEnd,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                }
            }
        }
        Ok(HookResult::BeforeRunEnd(follow_up.map(|follow_up| {
            FollowUpHookResult {
                follow_up: Some(follow_up),
            }
        })))
    }

    /// `transform_context` (`hooks.ts:109-110, 188-216`): chain messages and
    /// system prompt; failures are reported and skipped; the aggregate always
    /// carries the final pair.
    async fn transform_context(
        &self,
        lane: &str,
        run_id: &str,
        event: TransformContextEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        let mut messages = event.messages;
        let mut system_prompt = event.system_prompt;
        for registration in self.registrations_for(HookName::TransformContext) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::TransformContext(TransformContextEvent {
                    messages: messages.clone(),
                    system_prompt: system_prompt.clone(),
                }),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(HookResult::TransformContext(Some(patch))) => {
                    if let Some(returned) = patch.messages {
                        messages = returned;
                    }
                    if let Some(returned) = patch.system_prompt {
                        system_prompt = returned;
                    }
                }
                Ok(HookResult::TransformContext(None)) => {}
                Ok(other) => {
                    return Err(variant_mismatch(
                        HookName::TransformContext,
                        other.hook_name(),
                    ))
                }
                Err(error) => {
                    (self.inner.report_error)(
                        error,
                        HookName::TransformContext,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                }
            }
        }
        Ok(HookResult::TransformContext(Some(TransformContextPatch {
            messages: Some(messages),
            system_prompt: Some(system_prompt),
        })))
    }

    /// `before_request` (`hooks.ts:111-112, 218-244`): apply every returned
    /// patch before the next handler runs; the aggregate carries the created
    /// base-to-final diff, or nothing when no handler patched.
    async fn before_request(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforeRequestEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        let mut stream_options = event.stream_options.clone();
        let mut changed = false;
        for registration in self.registrations_for(HookName::BeforeRequest) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::BeforeRequest(BeforeRequestEvent {
                    model: event.model.clone(),
                    step: event.step,
                    attempt: event.attempt,
                    stream_options: stream_options.clone(),
                }),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(HookResult::BeforeRequest(Some(result))) => {
                    stream_options =
                        apply_stream_options_patch(stream_options, &result.stream_options);
                    changed = true;
                }
                Ok(HookResult::BeforeRequest(None)) => {}
                Ok(other) => {
                    return Err(variant_mismatch(HookName::BeforeRequest, other.hook_name()))
                }
                Err(error) => {
                    (self.inner.report_error)(
                        error,
                        HookName::BeforeRequest,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                }
            }
        }
        Ok(HookResult::BeforeRequest(if changed {
            Some(BeforeRequestHookResult {
                stream_options: create_stream_options_patch(&event.stream_options, &stream_options),
            })
        } else {
            None
        }))
    }

    /// `before_payload` (`hooks.ts:113-114, 246-268`): the aggregate always
    /// carries the current payload; failures are reported and skipped.
    async fn before_payload(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforePayloadEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        let mut payload = event.payload;
        for registration in self.registrations_for(HookName::BeforePayload) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::BeforePayload(BeforePayloadEvent {
                    model: event.model.clone(),
                    payload: payload.clone(),
                }),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(HookResult::BeforePayload(Some(result))) => payload = result.payload,
                Ok(HookResult::BeforePayload(None)) => {}
                Ok(other) => {
                    return Err(variant_mismatch(HookName::BeforePayload, other.hook_name()))
                }
                Err(error) => {
                    (self.inner.report_error)(
                        error,
                        HookName::BeforePayload,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                }
            }
        }
        Ok(HookResult::BeforePayload(Some(PayloadHookResult {
            payload,
        })))
    }

    /// `after_response` (`hooks.ts:115-116, 270-292`): the aggregate always
    /// carries the current message; failures are reported and skipped.
    async fn after_response(
        &self,
        lane: &str,
        run_id: &str,
        event: AfterResponseEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        let mut message = event.message;
        for registration in self.registrations_for(HookName::AfterResponse) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::AfterResponse(AfterResponseEvent {
                    status: event.status,
                    headers: event.headers.clone(),
                    message: message.clone(),
                }),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(HookResult::AfterResponse(Some(result))) => message = result.message,
                Ok(HookResult::AfterResponse(None)) => {}
                Ok(other) => {
                    return Err(variant_mismatch(HookName::AfterResponse, other.hook_name()))
                }
                Err(error) => {
                    (self.inner.report_error)(
                        error,
                        HookName::AfterResponse,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                }
            }
        }
        Ok(HookResult::AfterResponse(Some(MessageHookResult {
            message,
        })))
    }

    /// `before_tool` (`hooks.ts:117-118, 156-186`): chain args; the first
    /// block (or handler failure, reported and turned into a block with its
    /// message) stops the pipeline; the aggregate omits unchanged args.
    async fn before_tool(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforeToolEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        let mut args = event.args;
        let original_args = args.clone();
        let mut block: Option<ToolBlock> = None;
        for registration in self.registrations_for(HookName::BeforeTool) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::BeforeTool(BeforeToolEvent {
                    tool_call_id: event.tool_call_id.clone(),
                    tool_name: event.tool_name.clone(),
                    args: args.clone(),
                }),
            };
            let outcome = self
                .invoke_tool_registration(
                    HookName::BeforeTool,
                    registration,
                    invocation,
                    context.clone(),
                )
                .await;
            match outcome {
                Ok(HookResult::BeforeTool(Some(result))) => {
                    if let Some(returned) = result.args {
                        args = returned;
                    }
                    if let Some(returned) = result.block {
                        block = Some(returned);
                        break;
                    }
                }
                Ok(HookResult::BeforeTool(None)) => {}
                Ok(other) => return Err(variant_mismatch(HookName::BeforeTool, other.hook_name())),
                Err(error) => {
                    let message = error.to_string();
                    (self.inner.report_error)(
                        anyhow!(message.clone()),
                        HookName::BeforeTool,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                    block = Some(ToolBlock {
                        reason: message,
                        terminate: None,
                    });
                    break;
                }
            }
        }
        Ok(HookResult::BeforeTool(Some(BeforeToolHookResult {
            args: if args == original_args {
                None
            } else {
                Some(args)
            },
            block,
        })))
    }

    /// `after_tool` (`hooks.ts:119-120, 294-335`): field-by-field patch chain;
    /// the aggregate carries every field set by any handler, or nothing when
    /// no handler set anything; failures are reported and skipped.
    async fn after_tool(
        &self,
        lane: &str,
        run_id: &str,
        event: AfterToolEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        struct Current {
            content: Vec<TextOrImageBlock>,
            details: Option<serde_json::Value>,
            is_error: bool,
            usage: Option<Usage>,
        }
        let mut current = Current {
            content: event.content,
            details: event.details,
            is_error: event.is_error,
            usage: event.usage,
        };
        let mut aggregate = AfterToolHookResult::default();
        for registration in self.registrations_for(HookName::AfterTool) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: HookEvent::AfterTool(AfterToolEvent {
                    tool_call_id: event.tool_call_id.clone(),
                    tool_name: event.tool_name.clone(),
                    args: event.args.clone(),
                    content: current.content.clone(),
                    details: current.details.clone(),
                    is_error: current.is_error,
                    usage: current.usage,
                }),
            };
            let outcome = self
                .invoke_tool_registration(
                    HookName::AfterTool,
                    registration,
                    invocation,
                    context.clone(),
                )
                .await;
            match outcome {
                Ok(HookResult::AfterTool(Some(result))) => {
                    let AfterToolHookResult {
                        content,
                        details,
                        is_error,
                        usage,
                        terminate,
                    } = result;
                    if content.is_some() {
                        aggregate.content = content.clone();
                    }
                    if details.is_some() {
                        aggregate.details = details.clone();
                    }
                    if is_error.is_some() {
                        aggregate.is_error = is_error;
                    }
                    if usage.is_some() {
                        aggregate.usage = usage;
                    }
                    if terminate.is_some() {
                        aggregate.terminate = terminate;
                    }
                    if let Some(returned) = content {
                        current.content = returned;
                    }
                    if let Some(returned) = details {
                        current.details = Some(returned);
                    }
                    if let Some(returned) = is_error {
                        current.is_error = returned;
                    }
                    if let Some(returned) = usage {
                        current.usage = Some(returned);
                    }
                }
                Ok(HookResult::AfterTool(None)) => {}
                Ok(other) => return Err(variant_mismatch(HookName::AfterTool, other.hook_name())),
                Err(error) => {
                    (self.inner.report_error)(
                        error,
                        HookName::AfterTool,
                        lane.to_string(),
                        context.clone(),
                    )
                    .await;
                }
            }
        }
        Ok(HookResult::AfterTool(
            if aggregate == AfterToolHookResult::default() {
                None
            } else {
                Some(aggregate)
            },
        ))
    }

    /// `before_compaction` (`hooks.ts:121-122` via `firstStructural`): the
    /// first decline-or-result wins; returning both is reported and skipped.
    async fn before_compaction(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforeCompactionEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        self.first_structural::<BeforeCompactionHookResult>(
            HookName::BeforeCompaction,
            lane,
            run_id,
            HookEvent::BeforeCompaction(event),
            context,
        )
        .await
    }

    /// `before_navigation` (`hooks.ts:123-124` via `firstStructural`).
    async fn before_navigation(
        &self,
        lane: &str,
        run_id: &str,
        event: BeforeNavigationEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        self.first_structural::<BeforeNavigationHookResult>(
            HookName::BeforeNavigation,
            lane,
            run_id,
            HookEvent::BeforeNavigation(event),
            context,
        )
        .await
    }

    /// Upstream `firstStructural` (`hooks.ts:337-368`): handlers observe the
    /// event unchanged; the first decline-or-result admission wins; a
    /// decline-and-result conflict is reported and skipped (upstream's
    /// non-object-result skip is unrepresentable with typed results).
    async fn first_structural<T: StructuralOutcome>(
        &self,
        name: HookName,
        lane: &str,
        run_id: &str,
        event: HookEvent,
        context: Context,
    ) -> anyhow::Result<HookResult> {
        for registration in self.registrations_for(name) {
            let invocation = HookInvocation {
                lane: lane.to_string(),
                run_id: run_id.to_string(),
                event: event.clone(),
            };
            match (registration.handler)(invocation, context.clone()).await {
                Ok(result) => match T::unwrap(result) {
                    Err(got) => return Err(variant_mismatch(name, got)),
                    Ok(Some(outcome)) => {
                        if outcome.decline() == Some(true) && outcome.has_result() {
                            (self.inner.report_error)(
                                anyhow!(
                                    "{} hook cannot return both decline and {}",
                                    name.as_str(),
                                    T::result_field()
                                ),
                                name,
                                lane.to_string(),
                                context.clone(),
                            )
                            .await;
                            continue;
                        }
                        if outcome.decline() == Some(true) || outcome.has_result() {
                            return Ok(T::wrap(Some(outcome)));
                        }
                    }
                    Ok(None) => {}
                },
                Err(error) => {
                    (self.inner.report_error)(error, name, lane.to_string(), context.clone()).await;
                }
            }
        }
        Ok(T::wrap(None))
    }

    /// Upstream `invokeToolRegistration` (`hooks.ts:370-403`): the seam where
    /// each tool-hook handler runs inside its `pi.harness.hook` telemetry span
    /// (schema `telemetry.ts:453-489`; outcome attributes `completed` /
    /// `blocked` / `failed`). The span wrapper lands with the telemetry module
    /// (M3b Task 10); until then the handler runs directly — the span has no
    /// functional effect on the aggregate.
    fn invoke_tool_registration(
        &self,
        name: HookName,
        registration: Arc<HookRegistration>,
        invocation: HookInvocation,
        context: Context,
    ) -> BoxFuture<'static, anyhow::Result<HookResult>> {
        let _ = (&name, &registration.id);
        let handler = Arc::clone(&registration.handler);
        Box::pin(async move { (handler)(invocation, context).await })
    }
}

/// The shared `firstStructural` contract of the two structural hook results.
trait StructuralOutcome: Clone + Send + 'static {
    /// The `decline` flag, when the handler set it.
    fn decline(&self) -> Option<bool>;
    /// Whether the hook's result field (`compaction` / `summary`) is present.
    fn has_result(&self) -> bool;
    /// The upstream result field name, for the conflict error message.
    fn result_field() -> &'static str;
    /// Pack an aggregate outcome into a [`HookResult`].
    fn wrap(outcome: Option<Self>) -> HookResult;
    /// Extract the outcome from a handler result; `Err` reports a
    /// variant/name mismatch (upstream casts blindly).
    fn unwrap(result: HookResult) -> Result<Option<Self>, HookName>;
}

impl StructuralOutcome for BeforeCompactionHookResult {
    fn decline(&self) -> Option<bool> {
        self.decline
    }

    fn has_result(&self) -> bool {
        self.compaction.is_some()
    }

    fn result_field() -> &'static str {
        "compaction"
    }

    fn wrap(outcome: Option<Self>) -> HookResult {
        HookResult::BeforeCompaction(outcome)
    }

    fn unwrap(result: HookResult) -> Result<Option<Self>, HookName> {
        match result {
            HookResult::BeforeCompaction(outcome) => Ok(outcome),
            other => Err(other.hook_name()),
        }
    }
}

impl StructuralOutcome for BeforeNavigationHookResult {
    fn decline(&self) -> Option<bool> {
        self.decline
    }

    fn has_result(&self) -> bool {
        self.summary.is_some()
    }

    fn result_field() -> &'static str {
        "summary"
    }

    fn wrap(outcome: Option<Self>) -> HookResult {
        HookResult::BeforeNavigation(outcome)
    }

    fn unwrap(result: HookResult) -> Result<Option<Self>, HookName> {
        match result {
            HookResult::BeforeNavigation(outcome) => Ok(outcome),
            other => Err(other.hook_name()),
        }
    }
}

// ---------------------------------------------------------------------------
// Stream-options patch algebra (hooks.ts:446-533)
// ---------------------------------------------------------------------------

/// Apply one scalar patch field (upstream `hooks.ts:451-463`): absent leaves
/// the base value, explicit `undefined` deletes, a value replaces.
fn apply_scalar<T>(field: &mut Option<T>, patch: Option<Option<T>>) {
    match patch {
        None => {}
        Some(None) => *field = None,
        Some(Some(value)) => *field = Some(value),
    }
}

/// Apply one map patch field (upstream `hooks.ts:464-485`): absent leaves the
/// base map, explicit `undefined` clears it, a map merges with `undefined`
/// entries deleting their keys.
fn apply_map<V>(
    field: &mut Option<BTreeMap<String, V>>,
    patch: Option<Option<BTreeMap<String, Option<V>>>>,
) {
    match patch {
        None => {}
        Some(None) => *field = None,
        Some(Some(entries)) => {
            let mut map = field.take().unwrap_or_default();
            for (key, value) in entries {
                match value {
                    Some(value) => {
                        map.insert(key, value);
                    }
                    None => {
                        map.remove(&key);
                    }
                }
            }
            *field = Some(map);
        }
    }
}

/// Upstream `applyStreamOptionsPatch` (`hooks.ts:446-487`): fold a patch over
/// the base options.
pub fn apply_stream_options_patch(
    base: AgentHarnessStreamOptions,
    patch: &AgentHarnessStreamOptionsPatch,
) -> AgentHarnessStreamOptions {
    let mut next = base;
    apply_scalar(&mut next.transport, patch.transport);
    apply_scalar(&mut next.timeout_ms, patch.timeout_ms);
    apply_scalar(&mut next.max_retries, patch.max_retries);
    apply_scalar(&mut next.max_retry_delay_ms, patch.max_retry_delay_ms);
    apply_map(&mut next.headers, patch.headers.clone());
    apply_map(&mut next.metadata, patch.metadata.clone());
    apply_scalar(&mut next.cache_retention, patch.cache_retention);
    apply_scalar(&mut next.deferred, patch.deferred);
    next
}

/// Diff one scalar pair (upstream `hooks.ts:494-505`): `Some(value)` when the
/// pair differs, including `Some(None)` for deletions.
fn diff_scalar<T: Clone + PartialEq>(base: &Option<T>, value: &Option<T>) -> Option<Option<T>> {
    if base != value {
        Some(value.clone())
    } else {
        None
    }
}

/// Diff one map pair (upstream `hooks.ts:504-517`): the per-key diff with
/// deletions for dropped keys; a whole-map clear when the value drops the map;
/// an explicit empty map when headers appear where the base had none and the
/// diff is empty (upstream `patch.headers = {}`).
fn diff_map<V: Clone + PartialEq>(
    base: &Option<BTreeMap<String, V>>,
    value: &Option<BTreeMap<String, V>>,
) -> Option<Option<BTreeMap<String, Option<V>>>> {
    if base == value {
        return None;
    }
    let value = match value {
        None => return Some(None),
        Some(value) => value,
    };
    let mut diff: BTreeMap<String, Option<V>> = BTreeMap::new();
    if let Some(base) = base {
        for key in base.keys() {
            if !value.contains_key(key) {
                diff.insert(key.clone(), None);
            }
        }
    }
    for (key, current) in value {
        if base.as_ref().and_then(|map| map.get(key)) != Some(current) {
            diff.insert(key.clone(), Some(current.clone()));
        }
    }
    if base.is_none() && diff.is_empty() {
        Some(Some(BTreeMap::new()))
    } else if !diff.is_empty() {
        Some(Some(diff))
    } else {
        None
    }
}

/// Upstream `createStreamOptionsPatch` (`hooks.ts:489-533`): describe the
/// base-to-value diff as a patch, so
/// `apply_stream_options_patch(base, create_stream_options_patch(base, value))`
/// equals `value`.
pub fn create_stream_options_patch(
    base: &AgentHarnessStreamOptions,
    value: &AgentHarnessStreamOptions,
) -> AgentHarnessStreamOptionsPatch {
    AgentHarnessStreamOptionsPatch {
        transport: diff_scalar(&base.transport, &value.transport),
        timeout_ms: diff_scalar(&base.timeout_ms, &value.timeout_ms),
        max_retries: diff_scalar(&base.max_retries, &value.max_retries),
        max_retry_delay_ms: diff_scalar(&base.max_retry_delay_ms, &value.max_retry_delay_ms),
        headers: diff_map(&base.headers, &value.headers),
        metadata: diff_map(&base.metadata, &value.metadata),
        cache_retention: diff_scalar(&base.cache_retention, &value.cache_retention),
        deferred: diff_scalar(&base.deferred, &value.deferred),
    }
}

#[cfg(test)]
mod tests;
