//! The kind-execution surface of `packages/agent/src/harness/pico3/types.ts`
//! — the pieces Task 8's [`super::types`] deliberately deferred: `Step` /
//! `Closure` / `AbortClosure` / `PhaseHandler` (`types.ts:369-404`), `Kind`
//! (`types.ts:441-469`) with its `initial`/`phases`/`abort` handlers,
//! `Runtime` / `Models` / `RequestOptions` (`types.ts:808-854`), the tool
//! surface (`types.ts:860-952`), and `ProcessHost`/`ProcessStatus`
//! (`types.ts:856`, `types.ts:676-696`).
//!
//! # Disclosed substitutions
//!
//! - **Steps are typed enums.** Upstream `Step` is a JS object validated at
//!   the boundary (the scheduler's `validateStep`, `scheduler.ts:46-61`): a
//!   non-object step or a `next` that is neither function nor `{ phase }` is
//!   a `TaskContractFault`. Rust's enums make those shapes unrepresentable;
//!   the representable fault classes stay validated: completions whose
//!   `status`/payload pairing is wrong, checkpoints naming unknown or
//!   in-flight phases, and handlers for missing phases.
//! - **`Runtime` is a shared struct, not a per-invocation object literal.**
//!   Upstream builds one `Runtime` literal per invocation
//!   (`harness.ts:189-306`); the port builds the same shape as an
//!   `Arc<Runtime>` whose fields bind the invocation's invoker, so kind code
//!   calls `rt.commit(...)` exactly like upstream. Scheduler-facing
//!   operations (`sleep`, `waitForInput`, `abortTask`, ...) arrive through
//!   [`RuntimeOps`], implemented by the harness/scheduler.
//! - **`Models::stream` yields `Result` items.** Upstream streams are async
//!   iterables of events; transport failures arrive as thrown exceptions
//!   (the generation kind catches them). The port's stream items are
//!   `anyhow::Result<AssistantMessageEvent>`; provider failures remain
//!   `error` events exactly like upstream.
//! - **Tool `parameters` stay JSON schemas.** Upstream validates arguments
//!   with TypeBox (`Errors()` in `kinds/tool.ts:66-69`); the port validates
//!   the `Type.Object` subset the harness consumers author (object type,
//!   declared properties, `required`, primitive property types) and
//!   discloses the subset.
//! - `HooksOf<K>` / `KindTypes` generics are compile-time only upstream; the
//!   port's hook handlers are per-kind structs downcast through the
//!   [`crate::agent_core::harness::pico3::hooks::HookRunner`].
#![allow(clippy::type_complexity)]

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::Value;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::ConversationIndex;
use crate::agent_core::harness::pico3::session::{CommitOptions, Session, TaskRef, Tx};
use crate::agent_core::harness::pico3::types::{
    AnyKind, Completion, ContextView, DocRef, Entry, Id, Input, InvocationMode, Invoker,
    JsonObject, ModelRef, OwnedConversationSpec, SendInput, Task,
};

use super::system::{SectionRegistry, ToolRegistry};

/// The transition a `next` builder returns (`types.ts:376-380`): a
/// checkpoint, a completion, `"retry"`, or a same-batch builder evaluated in
/// the transition commit.
pub enum Next {
    /// `{ phase: …, … }` checkpoint object (`types.ts:376`).
    Checkpoint(super::types::Checkpoint),
    /// `{ status: "completed" | "failed", … }` (`types.ts:377-378`).
    Completion(Completion),
    /// `"retry"` (`types.ts:379`).
    Retry,
    /// The function form: evaluated inside the transition transaction
    /// (`types.ts:380`).
    Defer(NextFn),
}

/// Upstream `step.next`'s function form: `(tx, task, ctx) => next`.
pub type NextFn = Box<
    dyn for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<Next>> + Send,
>;

/// Upstream `Closure` (`types.ts:371-374`): the terminal builder evaluated
/// inside the closing commit.
pub type DoneFn = Box<
    dyn for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<Completion>>
        + Send,
>;

/// Upstream `Step<Checkpoint, …>` (`types.ts:375`).
pub enum Step {
    /// `{ done }`.
    Done(DoneFn),
    /// `{ next }`.
    Next(Next),
}

/// Upstream `AbortClosure` (`types.ts:369-370`): the closure the abort
/// handler returns, evaluated inside the abort invocation's closing commit;
/// the JSON result becomes `outcome.result`.
pub type AbortClosure = Box<
    dyn for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<Value>> + Send,
>;

/// Wrap a closure into an [`AbortClosure`] (the generic parameter keeps
/// HRTB inference working at the call sites).
pub fn abort_closure_from<F>(f: F) -> AbortClosure
where
    F: for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<Value>>
        + Send
        + 'static,
{
    Box::new(f)
}

/// Upstream `PhaseHandler` (`types.ts:381-403`).
pub type PhaseFn = Arc<
    dyn Fn(Arc<Task>, Arc<Runtime>, Context) -> BoxFuture<'static, anyhow::Result<Step>>
        + Send
        + Sync,
>;

/// Upstream `Kind.abort` (`types.ts:467`).
pub type AbortFn = Arc<
    dyn Fn(Arc<Task>, Arc<Runtime>, Context) -> BoxFuture<'static, anyhow::Result<AbortClosure>>
        + Send
        + Sync,
>;

/// Upstream `Kind` (`types.ts:441-469`): the erased execution surface behind
/// the metadata trait [`AnyKind`]. The scheduler is the only caller
/// (`scheduler.ts:41-45` `handlerFor`).
pub trait Kind: Send + Sync {
    /// The metadata half ([`AnyKind`]) this kind registers under.
    fn metadata(&self) -> Arc<dyn AnyKind>;
    /// Upstream `initial` (`types.ts:465`).
    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>>;
    /// Upstream `phases` (`types.ts:466`): the declared phase names.
    fn phases(&self) -> Vec<String>;
    /// Upstream `phases[phase]` (`types.ts:466`): dispatch into one phase;
    /// `Err(TaskContractFault)` when the kind declares no handler for it
    /// (`scheduler.ts:42-45`).
    fn phase(
        &self,
        phase: &str,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>>;
    /// Upstream `abort` (`types.ts:467`).
    fn abort(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>>;
}

/// A registration captures one metadata identity together with its handlers.
/// Upstream uses one JS Kind object for both. Keep this pair alive in an
/// invocation even after unregister/re-register so its leases retain authority.
pub(crate) struct RegisteredKind {
    kind: Arc<dyn Kind>,
    metadata: Arc<dyn AnyKind>,
}

impl RegisteredKind {
    pub(crate) fn capture(kind: Arc<dyn Kind>) -> Arc<dyn Kind> {
        Arc::new(Self {
            metadata: kind.metadata(),
            kind,
        })
    }
}

impl Kind for RegisteredKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        self.metadata.clone()
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        self.kind.initial(task, rt, ctx)
    }

    fn phases(&self) -> Vec<String> {
        self.kind.phases()
    }

    fn phase(
        &self,
        phase: &str,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        self.kind.phase(phase, task, rt, ctx)
    }

    fn abort(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>> {
        self.kind.abort(task, rt, ctx)
    }
}

/// An ordinary task kind authored with closures (`defineTask`'s result with
/// its execution handlers, `types.ts:484-497` + `Kind`).
pub struct TaskKind {
    metadata: Arc<dyn AnyKind>,
    name: String,
    initial: PhaseFn,
    phase_handlers: HashMap<String, PhaseFn>,
    abort: AbortFn,
}

impl TaskKind {
    /// Assemble a definition (upstream object literal).
    pub fn new(
        name: impl Into<String>,
        initial: PhaseFn,
        phase_handlers: HashMap<String, PhaseFn>,
        abort: AbortFn,
    ) -> TaskKind {
        let name = name.into();
        TaskKind {
            metadata: Arc::new(super::types::BasicKind::new(name.clone())),
            name,
            initial,
            phase_handlers,
            abort,
        }
    }

    /// Set the metadata half (config/turn/slot/describe/inflight).
    pub fn with_metadata(mut self, metadata: Arc<dyn AnyKind>) -> TaskKind {
        self.name = metadata.name().to_owned();
        self.metadata = metadata;
        self
    }
}

impl Kind for TaskKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        self.metadata.clone()
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        (self.initial)(task, rt, ctx)
    }

    fn phases(&self) -> Vec<String> {
        self.phase_handlers.keys().cloned().collect()
    }

    fn phase(
        &self,
        phase: &str,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        let registered = self.phase_handlers.get(phase).cloned();
        let kind_name = self.name.clone();
        let phase = phase.to_owned();
        Box::pin(async move {
            let result = match registered {
                Some(handler) => handler(task, rt, ctx).await,
                None => Err(super::types::task_contract_fault(
                    &kind_name,
                    &format!("no handler for phase {phase}"),
                )),
            };
            result
        })
    }

    fn abort(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>> {
        (self.abort)(task, rt, ctx)
    }
}

// ---------------------------------------------------------------------------
// Models (types.ts:810-830)
// ---------------------------------------------------------------------------

/// The resolved model metadata the generation and collapse kinds read
/// (upstream `Models.resolve` returns the pi-ai `Model`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    pub context_window: i64,
    pub max_tokens: i64,
}

impl ModelInfo {
    /// The [`ModelRef`] naming this model.
    pub fn reference(&self) -> ModelRef {
        ModelRef {
            provider: self.provider.clone(),
            model_id: self.id.clone(),
        }
    }
}

/// Upstream `rt.models.stream(model, { messages, thinkingLevel }, ctx)`
/// (`types.ts:816-820`).
#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub messages: Vec<Value>,
    pub thinking_level: String,
}

/// Upstream `AssistantMessageEvent` stream, with transport failures as
/// `Err` items (see the module docs).
pub type EventStream = std::pin::Pin<
    Box<dyn futures::Stream<Item = anyhow::Result<crate::ai::types::AssistantMessageEvent>> + Send>,
>;

/// Upstream `Models` (`types.ts:810-830`).
pub trait Models: Send + Sync {
    /// Upstream `resolve(ref)` (`types.ts:812-814`).
    fn resolve(&self, model: &ModelRef) -> Option<ModelInfo>;
    /// Upstream `stream(model, request, ctx)` (`types.ts:816-820`).
    fn stream(&self, model: ModelInfo, request: GenerationRequest, ctx: Context) -> EventStream;
    /// Upstream `fetchDeferred(model, handle, ctx)` (`types.ts:822-826`):
    /// resolves to `AssistantMessage` or `{ deferred: DeferredHandle }`.
    fn fetch_deferred(
        &self,
        model: ModelInfo,
        handle: Value,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Value>>;
    /// Upstream `cancelDeferred(model, handle, ctx)` (`types.ts:828`).
    fn cancel_deferred(
        &self,
        model: ModelInfo,
        handle: Value,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<()>>;
}

// ---------------------------------------------------------------------------
// Tools (types.ts:860-952)
// ---------------------------------------------------------------------------

/// Upstream `ToolDeclaration["output"]` (`types.ts:884-889`): the kernel
/// output bounds a tool may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputBounds {
    pub max_bytes: usize,
    pub max_lines: usize,
    pub retain: super::bounded::Retain,
}

/// Upstream `DEFAULT_BOUNDS` (`kinds/tool.ts:247`).
pub const DEFAULT_BOUNDS: OutputBounds = OutputBounds {
    max_bytes: 64 * 1024,
    max_lines: 200,
    retain: super::bounded::Retain::Head,
};

impl OutputBounds {
    /// Upstream `{ ...DEFAULT_BOUNDS, ...declaration.output }`
    /// (`kinds/tool.ts:257`).
    pub fn with_overrides(declared: Option<OutputBounds>) -> OutputBounds {
        declared.unwrap_or(DEFAULT_BOUNDS)
    }
}

/// Upstream `ToolResult` (pi-ai): kept JSON-shaped — content blocks,
/// diagnostics and control are opaque to the kernel beyond their
/// `type`/text handling in `kinds/tool.ts`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolResult {
    pub content: Option<Vec<Value>>,
    pub is_error: Option<bool>,
    pub details: Option<Value>,
    pub diagnostics: Option<Vec<Value>>,
    pub control: Option<Value>,
}

impl ToolResult {
    /// The JSON form stored inside `pi.tool_result` entries.
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        if let Some(content) = &self.content {
            object.insert("content".to_owned(), Value::Array(content.clone()));
        }
        if let Some(is_error) = self.is_error {
            object.insert("isError".to_owned(), Value::Bool(is_error));
        }
        if let Some(details) = &self.details {
            object.insert("details".to_owned(), details.clone());
        }
        if let Some(diagnostics) = &self.diagnostics {
            object.insert("diagnostics".to_owned(), Value::Array(diagnostics.clone()));
        }
        if let Some(control) = &self.control {
            object.insert("control".to_owned(), control.clone());
        }
        Value::Object(object)
    }
}

/// A streamed output chunk: upstream `string | Uint8Array` (`types.ts:900`).
#[derive(Debug, Clone)]
pub enum StreamChunk {
    Text(String),
    Bytes(Vec<u8>),
}

/// Upstream `HookInfo` (`types.ts:336-343`): the binding api every hook
/// receives, with `kind` filled by the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookApi {
    /// Upstream `info.taskId` (absent for harness-level hooks).
    pub task_id: Option<Id>,
    /// Upstream `info.conversationId`.
    pub conversation_id: Id,
    /// Upstream `info.kind`.
    pub kind: String,
}

/// Namespace-bound capability handed only to a before-tool hook.
/// Unlike ToolApi, this grants no task creation, raw runtime, or streaming access.
#[derive(Clone)]
pub struct BeforeToolApi {
    pub task_id: Option<Id>,
    pub conversation_id: Id,
    pub kind: String,
    pub call_id: String,
    waiting_hook: WaitingHook,
    memo_hook: MemoHook,
    emit_hook: EmitHook,
}

type WaitingHook = Arc<dyn Fn(Context) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;
type EmitHook =
    Arc<dyn Fn(&str, Value, Context) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;

impl BeforeToolApi {
    pub(crate) fn new(
        info: HookApi,
        call_id: String,
        waiting_hook: WaitingHook,
        memo_hook: MemoHook,
        emit_hook: EmitHook,
    ) -> Self {
        Self {
            task_id: info.task_id,
            conversation_id: info.conversation_id,
            kind: info.kind,
            call_id,
            waiting_hook,
            memo_hook,
            emit_hook,
        }
    }

    /// Publish the owning namespace as the tool's current approval wait.
    pub async fn waiting(&self, ctx: Context) -> anyhow::Result<()> {
        (self.waiting_hook)(ctx).await
    }

    /// Read without writing when candidate is None; Some(Null) is a real winner.
    pub async fn memo(
        &self,
        name: &str,
        candidate: Option<Value>,
        ctx: Context,
    ) -> anyhow::Result<Option<Value>> {
        (self.memo_hook)(name, candidate, ctx).await
    }

    /// Emit in this hook's namespace, validating its current registration token.
    pub async fn emit(&self, name: &str, data: Value, ctx: Context) -> anyhow::Result<()> {
        (self.emit_hook)(name, data, ctx).await
    }
}

/// Upstream `ToolApi` (`types.ts:894-948`): the capability surface one
/// invocation hands its tool. The port carries the invocation's
/// [`Runtime`] plus the invocation-scoped overrides (`stream`/`progress`/
/// `memo`); the base task api (`kinds/task-api.ts`) rejects the scoped ones.
/// The underlying core runtime is deliberately inaccessible to tools.
///
/// ```compile_fail
/// use pi_rust::agent_core::harness::pico3::runtime::ToolApi;
/// fn cannot_escalate(api: &ToolApi) { let _ = &api.rt; }
/// ```
#[derive(Clone)]
pub struct ToolApi {
    rt: Arc<Runtime>,
    /// Upstream `taskId` (`types.ts:896`).
    pub task_id: Id,
    /// Upstream `conversationId` (`types.ts:897`).
    pub conversation_id: Id,
    /// Upstream `callId` (`types.ts:898`): empty outside a tool invocation.
    pub call_id: String,
    stream_sink: Option<Arc<dyn Fn(StreamChunk) + Send + Sync>>,
    progress_hook: Option<ProgressHook>,
    memo_hook: Option<MemoHook>,
}

/// Upstream `progress(update, ctx)` — the update touches only the free slot
/// fields (`kinds/tool.ts:279-294`).
type ProgressHook = Arc<
    dyn Fn(Box<dyn FnOnce(&mut Value) + Send>, Context) -> BoxFuture<'static, anyhow::Result<()>>
        + Send
        + Sync,
>;
/// Upstream `memo(name, candidate?, ctx)` (`types.ts:916-930`).
type MemoHook = Arc<
    dyn Fn(&str, Option<Value>, Context) -> BoxFuture<'static, anyhow::Result<Option<Value>>>
        + Send
        + Sync,
>;

impl ToolApi {
    /// The invocation tool api (`kinds/tool.ts:276-321`).
    pub(crate) fn invocation(
        rt: Arc<Runtime>,
        task_id: Id,
        conversation_id: Id,
        call_id: String,
        stream_sink: Arc<dyn Fn(StreamChunk) + Send + Sync>,
        progress_hook: ProgressHook,
        memo_hook: MemoHook,
    ) -> ToolApi {
        ToolApi {
            rt,
            task_id,
            conversation_id,
            call_id,
            stream_sink: Some(stream_sink),
            progress_hook: Some(progress_hook),
            memo_hook: Some(memo_hook),
        }
    }

    /// The base task api (`kinds/task-api.ts:5-51`): invocation-only
    /// `stream`, `progress`, and `memo` operations all return errors.
    pub fn base(rt: Arc<Runtime>, task_id: Id, conversation_id: Id) -> ToolApi {
        ToolApi {
            rt,
            task_id,
            conversation_id,
            call_id: String::new(),
            stream_sink: None,
            progress_hook: None,
            memo_hook: None,
        }
    }

    /// Upstream `api.stream(chunk)` (`types.ts:900-905`).
    pub fn stream(&self, chunk: StreamChunk) -> anyhow::Result<()> {
        let sink = self
            .stream_sink
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("stream is only available to tools"))?;
        sink(chunk);
        Ok(())
    }

    /// Whether this api can stream (the invocation form).
    pub fn can_stream(&self) -> bool {
        self.stream_sink.is_some()
    }

    /// Upstream `api.progress(update, ctx)` (`types.ts:906-914`).
    pub async fn progress(
        &self,
        update: impl FnOnce(&mut Value) + Send + 'static,
        ctx: Context,
    ) -> anyhow::Result<()> {
        match &self.progress_hook {
            Some(hook) => hook(Box::new(update), ctx).await,
            None => anyhow::bail!("progress is only available to tools"),
        }
    }

    /// Upstream `api.memo(name, candidate?, ctx)` (`types.ts:916-930`):
    /// `candidate: None` reads (returning `None` when unset), `Some`
    /// writes-once.
    pub async fn memo(
        &self,
        name: &str,
        candidate: Option<Value>,
        ctx: Context,
    ) -> anyhow::Result<Option<Value>> {
        match &self.memo_hook {
            Some(hook) => hook(name, candidate, ctx).await,
            None => anyhow::bail!("memo is only available to tools"),
        }
    }

    /// Upstream `api.conversation(spec, ctx)` (`types.ts:932-941`).
    pub async fn conversation(
        &self,
        spec: OwnedConversationSpec,
        ctx: Context,
    ) -> anyhow::Result<ChildConversation> {
        let id = self.rt.create_owned_conversation(spec, ctx.clone()).await?;
        Ok(ChildConversation {
            api: self.clone(),
            id,
        })
    }

    /// Upstream `api.task(kind, input, opts, ctx)` (`types.ts:943-945`).
    pub async fn create_task(
        &self,
        kind: &Arc<dyn AnyKind>,
        input: Value,
        opts: crate::agent_core::harness::pico3::session::CreateTaskOptions,
        ctx: Context,
    ) -> anyhow::Result<TaskRef> {
        let kind = kind.clone();
        self.rt
            .commit_typed(ctx, move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.create_task_kind(&kind, input, opts) }.boxed()
            })
            .await
    }

    /// Upstream `api.getTask(ref, ctx)` (`types.ts:946`).
    pub async fn get_task(&self, id: Id, ctx: Context) -> anyhow::Result<Option<Task>> {
        self.rt
            .commit_typed(ctx, move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.task(id).await }.boxed()
            })
            .await
    }

    /// Upstream `api.waitForTask(ref, ctx)` (`types.ts:947`).
    pub async fn wait_for_task(&self, id: Id, ctx: Context) -> anyhow::Result<Task> {
        self.rt.wait_for_task(id, ctx).await
    }

    /// Upstream `api.slot(ref, ctx)` (`types.ts:948`): the stored slot of
    /// another task (`sticky.tasks[id]`).
    pub async fn slot(&self, id: Id, ctx: Context) -> anyhow::Result<Option<Value>> {
        let conversation_id = self
            .rt
            .commit_typed(
                ctx.clone(),
                move |tx: &mut Tx, _current: Task, _ctx: Context| {
                    async move { Ok(tx.task(id).await?.map(|task| task.conversation_id)) }.boxed()
                },
            )
            .await?
            .unwrap_or(self.conversation_id);
        self.rt
            .commit_typed(ctx, move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move {
                    let sticky = tx.snapshot(DocRef::Sticky { conversation_id })?;
                    Ok(sticky
                        .get("tasks")
                        .and_then(|tasks| tasks.get(id.to_string()))
                        .cloned())
                }
                .boxed()
            })
            .await
    }
}

/// Upstream `api.conversation(...)` result (`types.ts:936-940`).
pub struct ChildConversation {
    api: ToolApi,
    /// Upstream `id`.
    pub id: Id,
}

/// Upstream `send(...)` result (`types.ts:937-939`).
pub struct ChildInput {
    api: ToolApi,
    /// Upstream `id`.
    pub id: Id,
}

impl ChildConversation {
    /// Upstream `send(input, ctx)`.
    pub async fn send(&self, input: SendInput, ctx: Context) -> anyhow::Result<ChildInput> {
        let input_id = self.api.rt.send_owned(self.id, input, ctx.clone()).await?;
        Ok(ChildInput {
            api: self.api.clone(),
            id: input_id,
        })
    }

    /// Upstream `abort(ctx)`.
    pub async fn abort(&self, ctx: Context) -> anyhow::Result<()> {
        self.api.rt.abort_conversation(self.id, ctx).await
    }
}

impl ChildInput {
    /// Upstream `wait(ctx)`.
    pub async fn wait(&self, ctx: Context) -> anyhow::Result<Input> {
        self.api.rt.wait_for_input(self.id, ctx).await
    }

    /// Upstream `result(ctx)`.
    pub async fn result(&self, ctx: Context) -> anyhow::Result<Option<Input>> {
        let id = self.id;
        self.rt()
            .commit_typed(ctx, move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.input(id).await }.boxed()
            })
            .await
    }

    fn rt(&self) -> &Arc<Runtime> {
        &self.api.rt
    }
}

// ---------------------------------------------------------------------------
// Process host (types.ts:676-696, 856)
// ---------------------------------------------------------------------------

/// Upstream `ProcessStatus` (`types.ts:684-695`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessStatus {
    /// `{ status: "unknown" }`.
    Unknown,
    /// `{ status: "running", … }`.
    Running {
        stdout: String,
        stderr: String,
        dropped_stdout: i64,
        dropped_stderr: i64,
    },
    /// `{ status: "exited", … }`.
    Exited {
        exit_code: i64,
        stdout: String,
        stderr: String,
        dropped_stdout: i64,
        dropped_stderr: i64,
    },
}

impl ProcessStatus {
    /// Upstream `status.status`.
    pub fn stage(&self) -> &'static str {
        match self {
            ProcessStatus::Unknown => "unknown",
            ProcessStatus::Running { .. } => "running",
            ProcessStatus::Exited { .. } => "exited",
        }
    }

    /// The JSON wire shape.
    pub fn to_value(&self) -> Value {
        match self {
            ProcessStatus::Unknown => serde_json::json!({ "status": "unknown" }),
            ProcessStatus::Running {
                stdout,
                stderr,
                dropped_stdout,
                dropped_stderr,
            } => serde_json::json!({
                "status": "running",
                "stdout": stdout,
                "stderr": stderr,
                "droppedStdout": dropped_stdout,
                "droppedStderr": dropped_stderr,
            }),
            ProcessStatus::Exited {
                exit_code,
                stdout,
                stderr,
                dropped_stdout,
                dropped_stderr,
            } => serde_json::json!({
                "status": "exited",
                "exitCode": exit_code,
                "stdout": stdout,
                "stderr": stderr,
                "droppedStdout": dropped_stdout,
                "droppedStderr": dropped_stderr,
            }),
        }
    }
}

/// Upstream `ProcessHost` (`types.ts:856` — start/status/kill).
pub trait ProcessHost: Send + Sync {
    /// Upstream `start(key, spec, ctx)`.
    fn start<'a>(
        &'a self,
        key: &str,
        spec: &'a Value,
        ctx: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// Upstream `status(key, ctx)`.
    fn status<'a>(
        &'a self,
        key: &str,
        ctx: Context,
    ) -> BoxFuture<'a, anyhow::Result<ProcessStatus>>;
    /// Upstream `kill(key, signal, ctx)`.
    fn kill<'a>(
        &'a self,
        key: &str,
        signal: &str,
        ctx: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
}

// ---------------------------------------------------------------------------
// Plugins (types.ts:974-979)
// ---------------------------------------------------------------------------

/// Upstream `PluginHandler` (`types.ts:974-979`):
/// `(input, api, ctx) => JsonValue`.
pub type PluginHandler =
    Arc<dyn Fn(Value, ToolApi, Context) -> BoxFuture<'static, anyhow::Result<Value>> + Send + Sync>;

// ---------------------------------------------------------------------------
// Runtime (types.ts:808-854)
// ---------------------------------------------------------------------------

/// The scheduler/harness-provided operations (`harness.ts:227-258` object
/// members that are not plain commits).
pub trait RuntimeOps: Send + Sync {
    /// Upstream `rt.sleep(untilMs, ctx)` (`harness.ts:227-239`).
    fn sleep(&self, until_ms: i64, ctx: Context) -> BoxFuture<'static, anyhow::Result<()>>;
    /// Upstream `rt.waitForInput(id, ctx)` (`harness.ts:240`).
    fn wait_for_input(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<Input>>;
    /// Upstream `rt.waitForTask(id, ctx)` (`harness.ts:241`).
    fn wait_for_task(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<Task>>;
    /// Upstream `rt.abortTask(id, ctx)` (`harness.ts:242-250`); returns the
    /// upstream `"marked" | "terminal"` literal.
    fn abort_task(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<&'static str>>;
    /// Upstream `rt.abortConversation(id, ctx)` (`harness.ts:251-258`).
    fn abort_conversation(&self, id: Id, ctx: Context) -> BoxFuture<'static, anyhow::Result<()>>;
}

/// Upstream `Runtime` (`types.ts:808-854`), the per-invocation capability
/// object. It exposes checked operations, not its session or invocation token.
///
/// ```compile_fail
/// use pi_rust::agent_core::harness::pico3::runtime::Runtime;
/// fn cannot_bypass_checks(rt: &Runtime) { let _ = rt.session(); }
/// ```
///
/// ```compile_fail
/// use pi_rust::agent_core::harness::pico3::runtime::Runtime;
/// fn cannot_reach_scheduler(rt: &Runtime) { let _ = rt.ops(); }
/// ```
pub struct Runtime {
    session: Arc<Session>,
    invoker: Invoker,
    /// Upstream `taskId` (`types.ts:809`).
    task_id: Id,
    /// Upstream `conversationId` (`types.ts:809`).
    conversation_id: Id,
    /// Upstream `hooks` (`types.ts:822`).
    pub hooks: Arc<super::hooks::HookRunner>,
    /// Upstream `models` (`types.ts:823`).
    pub models: Arc<dyn Models>,
    /// Upstream `tools` (`types.ts:824`).
    pub tools: Arc<RwLock<HashMap<String, Arc<ToolDeclaration>>>>,
    /// Upstream `registries.sections` (`types.ts:825-828`).
    pub sections: SectionRegistry,
    /// Upstream `registries.tools` (`types.ts:829-830`).
    pub tools_registry: ToolRegistry,
    /// Upstream `kinds` (`types.ts:831`).
    pub kinds: Arc<RwLock<HashMap<String, Arc<dyn AnyKind>>>>,
    /// Upstream `processHost` (`types.ts:832`).
    pub process_host: Option<Arc<dyn ProcessHost>>,
    /// Upstream `plugins` (`types.ts:833`).
    pub plugins: Arc<RwLock<HashMap<String, PluginHandler>>>,
    /// Upstream `now` (`types.ts:834`).
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ops: Arc<dyn RuntimeOps>,
}

impl Runtime {
    /// Assemble a runtime (the harness's `runtime(task, invoker, ctx)`
    /// literal, `harness.ts:189-306`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session: Arc<Session>,
        invoker: Invoker,
        hooks: Arc<super::hooks::HookRunner>,
        models: Arc<dyn Models>,
        tools: Arc<RwLock<HashMap<String, Arc<ToolDeclaration>>>>,
        sections: SectionRegistry,
        tools_registry: ToolRegistry,
        kinds: Arc<RwLock<HashMap<String, Arc<dyn AnyKind>>>>,
        process_host: Option<Arc<dyn ProcessHost>>,
        plugins: Arc<RwLock<HashMap<String, PluginHandler>>>,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
        ops: Arc<dyn RuntimeOps>,
    ) -> Runtime {
        let task_id = invoker.task_id().unwrap_or(0);
        let conversation_id = invoker.conversation_id().unwrap_or(0);
        Runtime {
            session,
            invoker,
            task_id,
            conversation_id,
            hooks,
            models,
            tools,
            sections,
            tools_registry,
            kinds,
            process_host,
            plugins,
            now,
            ops,
        }
    }

    /// Upstream `rt.taskId`.
    pub fn task_id(&self) -> Id {
        self.task_id
    }

    /// Upstream `rt.conversationId`.
    pub fn conversation_id(&self) -> Id {
        self.conversation_id
    }

    /// Upstream `rt.now()`.
    pub fn now(&self) -> i64 {
        (self.now)()
    }

    /// Upstream `rt.kind` (`types.ts:809`): the invoker's kind metadata.
    pub fn kind(&self) -> Option<Arc<dyn AnyKind>> {
        self.invoker.task_kind().cloned()
    }

    /// The invocation mode.
    pub fn mode(&self) -> InvocationMode {
        match &self.invoker {
            Invoker::Task { mode, .. } => *mode,
            _ => InvocationMode::Run,
        }
    }

    /// Upstream `rt.tools.get(name)`.
    pub fn tool(&self, name: &str) -> Option<Arc<ToolDeclaration>> {
        self.tools
            .read()
            .expect("tools registry")
            .get(name)
            .cloned()
    }

    /// Upstream `rt.plugins.get(name)`.
    pub fn plugin(&self, name: &str) -> Option<PluginHandler> {
        self.plugins
            .read()
            .expect("plugins registry")
            .get(name)
            .cloned()
    }

    /// Upstream `rt.commit(fn, ctx)` (`harness.ts:194-205`): commit with the
    /// invocation's invoker, preloading the owned conversations' documents
    /// and passing the live task as the closure's second argument.
    pub async fn commit<T, F>(&self, f: F, ctx: Context) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<T>>
            + Send
            + 'static,
    {
        let owns = self
            .session
            .live_tasks()
            .get(&self.task_id)
            .map(|task| task.owns.clone())
            .unwrap_or_default();
        let mut docs: Vec<DocRef> = Vec::new();
        for id in &owns {
            docs.push(DocRef::Rewindable {
                conversation_id: *id,
            });
            docs.push(DocRef::Sticky {
                conversation_id: *id,
            });
        }
        let session = self.session.clone();
        let session_for_call = session.clone();
        let invoker = self.invoker.clone();
        let task_id = self.task_id;
        let result = session_for_call
            .commit(
                invoker,
                ctx,
                CommitOptions {
                    docs,
                    closing: false,
                },
                move |tx: &mut Tx, line_ctx: Context| {
                    async move {
                        let current = session
                            .live_tasks()
                            .get(&task_id)
                            .cloned()
                            .ok_or_else(|| anyhow::anyhow!("task {task_id} is not live"))?;
                        f(tx, current, line_ctx).await
                    }
                    .boxed()
                },
            )
            .await?;
        Ok(result.value)
    }

    /// The erased commit path (job/plugin-style `Value` results).
    pub async fn commit_erased(
        &self,
        ctx: Context,
        f: Box<
            dyn for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<Value>>
                + Send,
        >,
    ) -> anyhow::Result<Value> {
        self.commit(f, ctx).await
    }

    /// The erased commit path with an explicit context argument order.
    pub async fn commit_erased_with(
        &self,
        ctx: Context,
        f: Box<
            dyn for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<Value>>
                + Send,
        >,
    ) -> anyhow::Result<Value> {
        self.commit(f, ctx).await
    }

    /// Typed commit used by the [`ToolApi`] surface.
    pub async fn commit_typed<T, F>(&self, ctx: Context, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: for<'tx> FnOnce(&'tx mut Tx, Task, Context) -> BoxFuture<'tx, anyhow::Result<T>>
            + Send
            + 'static,
    {
        self.commit(f, ctx).await
    }

    /// Upstream `rt.sleep(untilMs, ctx)`.
    pub async fn sleep(&self, until_ms: i64, ctx: Context) -> anyhow::Result<()> {
        self.ops.sleep(until_ms, ctx).await
    }

    /// Upstream `rt.waitForInput(id, ctx)`.
    pub async fn wait_for_input(&self, id: Id, ctx: Context) -> anyhow::Result<Input> {
        self.ops.wait_for_input(id, ctx).await
    }

    /// Upstream `rt.waitForTask(id, ctx)`.
    pub async fn wait_for_task(&self, id: Id, ctx: Context) -> anyhow::Result<Task> {
        self.ops.wait_for_task(id, ctx).await
    }

    /// Upstream `rt.abortTask(id, ctx)`.
    pub async fn abort_task(&self, id: Id, ctx: Context) -> anyhow::Result<&'static str> {
        let session = self.session.clone();
        let invoker = self.invoker.clone();
        let task_id = self.task_id;
        self.session
            .on_line(
                move |line_ctx| {
                    async move {
                        assert_invocation(&invoker, &session.live_tasks())?;
                        let target = match session.live_tasks().get(&id).cloned() {
                            Some(target) => target,
                            None => session
                                .storage()
                                .task(id, line_ctx)
                                .await?
                                .ok_or_else(|| anyhow::anyhow!("task {id} not found"))?,
                        };
                        assert_task_conversation_scope(&session, task_id, target.conversation_id)
                    }
                    .boxed()
                },
                ctx.clone(),
            )
            .await?;
        self.ops.abort_task(id, ctx).await
    }

    /// Upstream `rt.abortConversation(id, ctx)`.
    pub async fn abort_conversation(&self, id: Id, ctx: Context) -> anyhow::Result<()> {
        let session = self.session.clone();
        let invoker = self.invoker.clone();
        self.session
            .on_line(
                move |_line_ctx| {
                    async move {
                        assert_invocation(&invoker, &session.live_tasks())?;
                        assert_owned_conversation(&session, &invoker, id)
                    }
                    .boxed()
                },
                ctx.clone(),
            )
            .await?;
        self.ops.abort_conversation(id, ctx).await
    }

    /// Upstream `rt.context(c, at, ctx)` (`harness.ts:288`).
    pub async fn context(
        &self,
        conversation_id: Id,
        at: Option<Id>,
        ctx: Context,
    ) -> anyhow::Result<ContextView> {
        self.commit(
            move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.context(conversation_id, at).await }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `rt.newestEntry(c, opts, ctx)` (`harness.ts:289-290`).
    pub async fn newest_entry(
        &self,
        conversation_id: Id,
        kind: Option<&'static str>,
        with_head: bool,
        ctx: Context,
    ) -> anyhow::Result<Option<Entry>> {
        self.commit(
            move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.newest_entry(conversation_id, kind, with_head).await }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `rt.rewindable(c, ctx)` (`harness.ts:291-296`).
    pub async fn rewindable(
        &self,
        conversation_id: Id,
        ctx: Context,
    ) -> anyhow::Result<JsonObject> {
        self.commit(
            move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.snapshot(DocRef::Rewindable { conversation_id }) }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `rt.sticky(c, ctx)` (`harness.ts:297-302`).
    pub async fn sticky(&self, conversation_id: Id, ctx: Context) -> anyhow::Result<JsonObject> {
        self.commit(
            move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.snapshot(DocRef::Sticky { conversation_id }) }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `rt.rewindableAsOf(c, at, ctx)` (`harness.ts:303-304`).
    pub async fn rewindable_as_of(
        &self,
        conversation_id: Id,
        at: Id,
        ctx: Context,
    ) -> anyhow::Result<Option<JsonObject>> {
        self.commit(
            move |tx: &mut Tx, _current: Task, _ctx: Context| {
                async move { tx.rewindable_as_of(conversation_id, at).await }.boxed()
            },
            ctx,
        )
        .await
    }

    /// Upstream `rt.createOwnedConversation(spec, ctx)` (`harness.ts:259-269`):
    /// a kernel commit after `assertInvocation`.
    pub async fn create_owned_conversation(
        &self,
        spec: OwnedConversationSpec,
        ctx: Context,
    ) -> anyhow::Result<Id> {
        let invoker = self.invoker.clone();
        let session = self.session.clone();
        let session_for_call = session.clone();
        let result = session_for_call
            .commit(
                Invoker::Kernel {
                    conversation_id: None,
                },
                ctx,
                CommitOptions::default(),
                move |tx: &mut Tx, _line_ctx: Context| {
                    let invoker = invoker.clone();
                    let spec = spec.clone();
                    async move {
                        // `assertInvocation` (`harness.ts:264`).
                        let live = session.live_tasks();
                        assert_invocation(&invoker, &live)?;
                        let source = live
                            .get(&invoker.task_id().expect("task invoker"))
                            .map(|task| task.conversation_id)
                            .expect("live task");
                        tx.create_owned_conversation(
                            invoker.task_id().expect("task invoker"),
                            source,
                            spec,
                        )
                        .await
                    }
                    .boxed()
                },
            )
            .await?;
        Ok(result.value)
    }

    /// Upstream `rt.sendOwned(id, input, ctx)` (`harness.ts:270-287`).
    pub async fn send_owned(&self, id: Id, input: SendInput, ctx: Context) -> anyhow::Result<Id> {
        let invoker = self.invoker.clone();
        let session = self.session.clone();
        let session_for_call = session.clone();
        let result = session_for_call
            .commit(
                Invoker::Kernel {
                    conversation_id: None,
                },
                ctx,
                CommitOptions {
                    docs: vec![
                        DocRef::Rewindable {
                            conversation_id: id,
                        },
                        DocRef::Sticky {
                            conversation_id: id,
                        },
                    ],
                    closing: false,
                },
                move |tx: &mut Tx, _line_ctx: Context| {
                    let invoker = invoker.clone();
                    let input = input.clone();
                    async move {
                        // `assertInvocation` + `assertOwnedConversation`
                        // (`harness.ts:275-277`).
                        let live = session.live_tasks();
                        assert_invocation(&invoker, &live)?;
                        assert_owned_conversation(&session, &invoker, id)?;
                        tx.send(id, input).await
                    }
                    .boxed()
                },
            )
            .await?;
        Ok(result.value)
    }
}

/// Upstream `assertInvocation` (`harness.ts:322-327`).
pub fn assert_invocation(invoker: &Invoker, live: &HashMap<Id, Task>) -> anyhow::Result<()> {
    let Invoker::Task {
        token, id, mode, ..
    } = invoker
    else {
        return Err(super::types::forbidden("operation outside a task"));
    };
    if !token.alive() {
        return Err(super::types::forbidden(
            "operation from a finished invocation",
        ));
    }
    let Some(task) = live.get(id) else {
        return Err(super::types::forbidden(
            "operation from a task that is not live",
        ));
    };
    if *mode == InvocationMode::Run && task.abort == Some(true) {
        return Err(super::types::forbidden(
            "operation from a marked run invocation",
        ));
    }
    Ok(())
}

/// Upstream `assertOwnedConversation` (`harness.ts:336-341`).
pub fn assert_owned_conversation(
    session: &Arc<Session>,
    invoker: &Invoker,
    conversation_id: Id,
) -> anyhow::Result<()> {
    let Invoker::Task { id, .. } = invoker else {
        return Err(super::types::forbidden("operation outside a task"));
    };
    let live = session.live_tasks();
    let owns = live
        .get(id)
        .map(|task| task.owns.clone())
        .unwrap_or_default();
    let index = session.index();
    if owns
        .iter()
        .any(|root| index.subtree(*root).contains(&conversation_id))
    {
        return Ok(());
    }
    Err(super::types::forbidden(format!(
        "conversation {conversation_id} is not owned by task {id}"
    )))
}

/// Upstream `assertTaskConversationScope` (`harness.ts:329-334`).
pub fn assert_task_conversation_scope(
    session: &Arc<Session>,
    task_id: Id,
    conversation_id: Id,
) -> anyhow::Result<()> {
    let live = session.live_tasks();
    let task = live.get(&task_id);
    if task.is_some_and(|task| task.conversation_id == conversation_id) {
        return Ok(());
    }
    let index = session.index();
    if task.is_some_and(|task| {
        task.owns
            .iter()
            .any(|root| index.subtree(*root).contains(&conversation_id))
    }) {
        return Ok(());
    }
    Err(super::types::forbidden(format!(
        "conversation {conversation_id} is outside task {task_id}'s subtree"
    )))
}

/// Hook-handler downcast support (`types.ts:344-366`): every per-kind
/// handlers struct implements this so the hook runner can hand bindings out
/// erased.
pub trait HookHandlers: Any + Send {
    fn as_any(&self) -> &dyn Any;
}

impl HookHandlers for () {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Tool declarations (`types.ts:862-882`)
// ---------------------------------------------------------------------------

/// A tool declaration (`types.ts:862-882`): name, schema, bounds, and the
/// execute function.
#[derive(Clone)]
pub struct ToolDeclaration {
    /// Upstream `name` (`types.ts:863`).
    pub name: String,
    /// Upstream `description` (`types.ts:864`).
    pub description: String,
    /// Upstream `parameters` (`types.ts:865`): the TypeBox schema as JSON.
    pub parameters: Value,
    /// Upstream `replay` (`types.ts:869`): `"safe"` allows post-crash
    /// re-invocation; default `"unsafe"`.
    pub replay: Option<String>,
    /// Upstream `output` (`types.ts:884-889`).
    pub output: Option<OutputBounds>,
    /// Upstream `execute(args, api, ctx)` (`types.ts:870-882`).
    pub execute: ToolExecuteFn,
}

/// Upstream `execute` (`types.ts:870-882`).
pub type ToolExecuteFn = Arc<
    dyn Fn(Value, ToolApi, Context) -> BoxFuture<'static, anyhow::Result<ToolResult>> + Send + Sync,
>;

impl std::fmt::Debug for ToolDeclaration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDeclaration")
            .field("name", &self.name)
            .field("replay", &self.replay)
            .finish_non_exhaustive()
    }
}

/// Upstream `invalid(declaration, call)` (`kinds/tool.ts:66-69`): validate
/// arguments against the declaration's schema. The port checks the
/// TypeBox-subset shapes the harness consumers author (object type,
/// properties, required, primitive types) — disclosed in the module docs.
pub fn invalid_arguments(declaration: &ToolDeclaration, call: &Value) -> Option<String> {
    let arguments = call.get("arguments")?;
    let errors = validate_schema(&declaration.parameters, arguments, "");
    if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    }
}

fn validate_schema(schema: &Value, value: &Value, path: &str) -> Vec<String> {
    let Some(schema_object) = schema.as_object() else {
        return Vec::new();
    };
    let schema_type = schema_object.get("type").and_then(Value::as_str);
    match schema_type {
        Some("object") => {
            let mut errors = Vec::new();
            let Some(object) = value.as_object() else {
                return vec![format!("{path}: expected object")];
            };
            if let Some(properties) = schema_object.get("properties").and_then(Value::as_object) {
                for (key, property_schema) in properties {
                    if let Some(property_value) = object.get(key) {
                        let property_path = format!("{path}/{key}");
                        errors.extend(validate_schema(
                            property_schema,
                            property_value,
                            &property_path,
                        ));
                    }
                }
            }
            if let Some(required) = schema_object.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(key) {
                        errors.push(format!("{path}/{key}: must be present"));
                    }
                }
            }
            errors
        }
        Some("string") => {
            if value.is_string() {
                Vec::new()
            } else {
                vec![format!("{path}: expected string")]
            }
        }
        Some("number") => {
            if value.is_number() {
                Vec::new()
            } else {
                vec![format!("{path}: expected number")]
            }
        }
        Some("boolean") => {
            if value.is_boolean() {
                Vec::new()
            } else {
                vec![format!("{path}: expected boolean")]
            }
        }
        _ => Vec::new(),
    }
}
