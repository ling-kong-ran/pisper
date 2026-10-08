//! Port of `src/harness/types.ts`: the shared harness type surface —
//! submissions, tools, prompt sections, registries, and the handle-shaped
//! values the other harness modules exchange.
//!
//! Divergences (structural, disclosed):
//!
//! - **D7 extension (handles are concrete).** Upstream `Conversation`,
//!   `Submission`, and `ConversationHandle` are promise-returning interfaces;
//!   the port's handles are concrete structs of their owning modules,
//!   compared by `id` as upstream.
//! - **D13 (generics erased).** Upstream `Task<I, S, R, H>` and
//!   `Tool extends ToolRegistration` generics vanish at the scheduler's
//!   erased boundary; the port keeps the erased forms
//!   ([`crate::durable::tasks::TaskToken`], [`ToolRegistration`]) everywhere,
//!   matching upstream `AnyTask` / `ToolRegistration`.
//! - **D14 (wire key order of literal shapes).** Serialized literal shapes
//!   declare fields in their upstream construction order:
//!   [`ToolDiagnostic`] is `{severity, code?, message}` per the `tool.ts`
//!   `toolDiagnostic` literal; [`ConversationRetryPolicy`] is `{enabled,
//!   maxRetries, baseDelayMs, maxAgentDelayMs?}`;
//!   [`ModelRef`] is `{provider, modelId}`; task results are
//!   `{entryId, control?}` (`tool.ts` settle).

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::{Message, ModelThinkingLevel, StringOrBlocks, Tool as AiTool};

use super::super::env::ExecutionEnv;
use super::super::errors::PlainError;
use super::super::ids::{ConversationId, EntryId, SubmissionId, TaskId};
use super::super::session::session::Session;
use super::super::session::transaction::Transaction;
use super::super::tasks::TaskToken;
use super::super::types::{
    ConversationOwnership, ConversationRecord, EntryDraft, EntryQuery, EntryRecord, JsonObject,
    Page, SubmissionRecord, SubmissionType, TaskRecord, TaskStatus,
};

/// Provider and model ID resolved through pi-ai `Models` (`harness/types.ts`
/// `ModelRef`). Wire order `{provider, modelId}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub model_id: String,
}

/// `UserInput = UserMessage["content"]` (`harness/types.ts`).
pub type UserInput = StringOrBlocks;

/// Host submission: user input that may start a run, or a passive entry write
/// (`harness/types.ts` `SubmissionDraft`). Tagged by `type`.
#[derive(Debug, Clone, PartialEq)]
pub enum SubmissionDraft {
    Input {
        /// Host-provided deduplication key, scoped to the conversation.
        request_id: Option<String>,
        content: UserInput,
        when_busy: Option<WhenBusy>,
    },
    Write {
        request_id: Option<String>,
        entry: EntryDraft,
    },
}

/// `SubmissionDraft.whenBusy` discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WhenBusy {
    Steer,
    FollowUp,
    Reject,
}

impl SubmissionDraft {
    /// The draft's `type` discriminant.
    pub fn submission_type(&self) -> SubmissionType {
        match self {
            SubmissionDraft::Input { .. } => SubmissionType::Input,
            SubmissionDraft::Write { .. } => SubmissionType::Write,
        }
    }

    /// The draft's optional request ID.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            SubmissionDraft::Input { request_id, .. }
            | SubmissionDraft::Write { request_id, .. } => request_id.as_deref(),
        }
    }
}

/// `Submission.abort()` outcomes (`"aborted" | "already_placed" | "settled"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortResult {
    Aborted,
    AlreadyPlaced,
    Settled,
}

impl AbortResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            AbortResult::Aborted => "aborted",
            AbortResult::AlreadyPlaced => "already_placed",
            AbortResult::Settled => "settled",
        }
    }
}

/// `Harness.abortSubmission` adds `not_found`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortSubmissionResult {
    Found(AbortResult),
    NotFound,
}

/// `Harness.abortTask` results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortTaskResult {
    Marked,
    Terminal,
}

/// Options of `Conversation.abort()` (`harness/types.ts`
/// `ConversationAbortOptions`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConversationAbortOptions {
    /// Cross background boundaries (see the upstream doc comment).
    pub background: bool,
}

/// Post-tools controls requested by a tool result (`harness/types.ts`
/// `ToolControl`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolControl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
}

/// Remark about a call for the model and the UI (`harness/types.ts`
/// `ToolDiagnostic`). Wire order `{severity, code?, message}` per the
/// `tool.ts` `toolDiagnostic` literal (D14).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDiagnostic {
    pub severity: ToolDiagnosticSeverity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
}

/// `ToolDiagnostic.severity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolDiagnosticSeverity {
    Info,
    Warn,
    Error,
}

/// Result of one tool execution (`harness/types.ts`
/// `ToolExecutionResult`). In-memory only; the persisted form is the
/// `pi.tool-result` entry. The hook payloads serialize this shape (D14);
/// field order follows the upstream result literal (content, isError,
/// details, diagnostics, usage, control).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionResult {
    /// `None`: the retained `output()` text becomes the content.
    pub content: Option<Vec<crate::ai::types::TextOrImageBlock>>,
    pub is_error: Option<bool>,
    /// `None`: the last `details()` value becomes the details.
    pub details: Option<Value>,
    /// Added after those recorded through `api.diagnostic()`.
    pub diagnostics: Option<Vec<ToolDiagnostic>>,
    /// Spend of the execution itself, such as a model call.
    pub usage: Option<crate::ai::types::Usage>,
    pub control: Option<ToolControl>,
}

/// Whether the tools of one round run at once or one after another in call
/// order (`harness/types.ts` `ToolExecutionMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolExecutionMode {
    Parallel,
    Sequential,
}

/// How many queued items of one mode a boundary places (`harness/types.ts`
/// `QueueMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueMode {
    All,
    OneAtATime,
}

/// Curated pi-ai request options (`harness/types.ts`
/// `ConversationStreamOptions`); absent fields use pi-ai defaults. Wire key
/// order `{transport?, timeoutMs?, maxRetries?, maxRetryDelayMs?, headers?,
/// metadata?, cacheRetention?, deferred?}`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationStreamOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retry_delay_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<(String, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_retention: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<Value>,
}

/// Durable generation attempt retries (`harness/types.ts`
/// `ConversationRetryPolicy`); the JSON shape of pi-ai `RetryPolicy`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRetryPolicy {
    pub enabled: bool,
    pub max_retries: i64,
    pub base_delay_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_agent_delay_ms: Option<f64>,
}

/// Token for removing registrations (`harness/types.ts` `Registration`).
#[derive(Clone)]
pub struct Registration {
    dispose: Arc<dyn Fn() + Send + Sync>,
}

impl Registration {
    pub fn new(dispose: Arc<dyn Fn() + Send + Sync>) -> Self {
        Registration { dispose }
    }

    /// Idempotent; removes exactly the registrations this token covers.
    pub fn dispose(&self) {
        (self.dispose)();
    }
}

/// Conversation selection for a scoped hook registration (`harness/types.ts`
/// `HookScope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookScope {
    pub conversation_id: ConversationId,
    /// Also match conversations owned, transitively, by tasks of this
    /// conversation.
    pub subtree: bool,
}

/// What a hook may use (`harness/types.ts` `HookApi`): committed reads and
/// the asking task's memos, which hooks and the task share. The port carries
/// the memo surface as a boxed closure supplied by the scheduler's runtime.
#[derive(Clone)]
pub struct HookApi {
    pub task_id: TaskId,
    pub conversation_id: ConversationId,
    /// `memo(name)` with `None` reads; `Some(candidate)` claims.
    pub memo: MemoFn,
    /// Committed document reads (`DocumentReader` methods of the Session).
    pub read: Arc<Session>,
}

/// `memo(name, candidate?)` of the task runtime and `HookApi`.
pub type MemoFn =
    Arc<dyn Fn(&str, Option<Value>, Context) -> super::prompt::MemoFuture + Send + Sync>;

/// An erased hook handler invocation (`harness/types.ts` through
/// `HookRegistration`): `(payload, api, context) -> payload | undefined`.
pub type HookHandlerFn =
    Arc<dyn Fn(Value, HookApi, Context) -> super::prompt::HookFuture + Send + Sync>;

/// One registered hook handler map for a task (`harness/types.ts`
/// `HookRegistration`), erased (D13).
#[derive(Clone)]
pub struct HookRegistration {
    /// Handlers by hook name: `beforeRequest`, `afterResponse`, `onYield`,
    /// `afterTools`, `beforeTool`, `afterTool`.
    pub handlers: BTreeMap<String, HookHandlerFn>,
    pub scope: Option<HookScope>,
}

/// `ToolRegistration.prepareArguments` (`args` repaired).
pub type PrepareArgumentsFn = Arc<dyn Fn(&JsonObject) -> JsonObject + Send + Sync>;

/// `HarnessOptions.onReport` (extension-failure receiver).
pub type ReportFn = Arc<dyn Fn(&PlainError) + Send + Sync>;

/// Executable tool registered in a registry (`harness/types.ts`
/// `ToolRegistration`). Only pi-ai `Tool` fields enter the transcript.
pub struct ToolRegistration {
    /// The pi-ai tool surface that enters the transcript.
    pub tool: AiTool,
    /// Whether an interrupted execution may rerun on recovery. Default
    /// `unsafe`.
    pub replay: Option<ToolReplay>,
    /// Default: the conversation's `toolExecution`. One sequential call makes
    /// its whole round sequential.
    pub execution_mode: Option<ToolExecutionMode>,
    /// Repair arguments models commonly get wrong before validation. Must be
    /// pure and must not mutate `args`.
    pub prepare_arguments: Option<PrepareArgumentsFn>,
    pub output_limits: Option<OutputLimitsSpec>,
    pub execute: ToolExecuteFn,
}

/// `ToolRegistration.replay` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolReplay {
    Safe,
    Unsafe,
}

/// `ToolRegistration.outputLimits` (`{maxBytes?, maxLines?, retain?}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputLimitsSpec {
    pub max_bytes: Option<usize>,
    pub max_lines: Option<usize>,
    pub retain: Option<OutputRetain>,
}

/// `retain` discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRetain {
    Head,
    Tail,
}

/// `ToolRegistration.execute(args, api, context)` result future.
pub type ToolExecutionBoxFuture = Pin<
    Box<dyn std::future::Future<Output = Result<ToolExecutionResult, PlainError>> + Send + 'static>,
>;

/// `ToolRegistration.execute(args, api, context)`.
pub type ToolExecuteFn = Arc<
    dyn Fn(JsonObject, Arc<dyn ToolExecutionApiLike>, Context) -> ToolExecutionBoxFuture
        + Send
        + Sync,
>;

impl ToolRegistration {
    /// The transcript tool's name.
    pub fn name(&self) -> &str {
        &self.tool.name
    }
}

impl std::fmt::Debug for ToolRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistration")
            .field("name", &self.tool.name)
            .finish()
    }
}

/// Result future of an erased async operation.
pub type ApiFuture<T> = Pin<Box<dyn Future<Output = Result<T, PlainError>> + Send + 'static>>;

/// `ToolExecutionApi.commit(change, context)` with the change erased: the
/// closure receives the transaction and resolves with a JSON result. The
/// upstream change is `async` only because transaction operations are (D5);
/// the port's erased change is synchronous over the transaction and the
/// runtime commit awaits nothing inside it.
pub type ErasedCommitChange = Arc<dyn Fn(&Transaction) -> Result<Value, PlainError> + Send + Sync>;

/// Operations available to one tool invocation (`harness/types.ts`
/// `ToolExecutionApi`). A plain object upstream, so a wrapper can pass
/// `{ ...api, env }` to the tool it wraps. Every operation rejects after the
/// invocation ends. Futures are boxed so the trait is object safe
/// (`Arc<dyn ...>` crosses the erased registry boundary).
pub trait ToolExecutionApiLike: Send + Sync {
    fn task_id(&self) -> TaskId;
    fn conversation_id(&self) -> ConversationId;
    fn call_id(&self) -> &str;
    /// `HarnessOptions.env` unless a wrapper supplies another environment.
    fn env(&self) -> Option<Arc<dyn ExecutionEnv>>;
    /// Append running output; it becomes the result content when the result
    /// omits `content`.
    fn output(&self, chunk: &[u8]) -> Result<(), PlainError>;
    /// Record a model-visible remark about this call.
    fn diagnostic(&self, diagnostic: ToolDiagnostic) -> Result<(), PlainError>;
    /// Replace running details; the last value becomes the result details
    /// when the result omits `details`.
    fn details(&self, value: Value, context: Context) -> ApiFuture<()>;
    /// Session commit scoped to the invocation.
    fn commit(&self, change: ErasedCommitChange, context: Context) -> ApiFuture<Value>;
    /// Read the task's memo `name`, or claim it with `candidate`.
    fn memo(
        &self,
        name: &str,
        candidate: Option<Value>,
        context: Context,
    ) -> ApiFuture<Option<Value>>;
    fn create_task(
        &self,
        task: TaskToken,
        input: Value,
        options: super::super::types::TaskOptions,
        context: Context,
    ) -> ApiFuture<TaskId>;
    fn get_task(&self, id: TaskId, context: Context) -> ApiFuture<Option<TaskRecord>>;
    fn wait_for_task(&self, id: TaskId, context: Context) -> ApiFuture<TaskRecord>;
    /// Invocation-bound handle of an existing conversation, such as one this
    /// tool created in `commit()`.
    fn conversation(
        &self,
        id: ConversationId,
        context: Context,
    ) -> ApiFuture<Option<ConversationHandle>>;
}

/// `ConversationHandle` (`harness/types.ts`): invocation-bound conversation
/// operations for tasks and tools. Upstream builds it as a plain object of
/// bound closures (`boundConversation`), so the port stores the operations
/// as closure fields; each first checks the invocation and runs under its
/// signal, so it rejects once the invocation ends.
#[derive(Clone)]
pub struct ConversationHandle {
    pub id: ConversationId,
    /// Admit user input; resolves with the submission ID.
    pub submit: Arc<dyn Fn(SubmissionDraft, Context) -> ApiFuture<SubmissionId> + Send + Sync>,
    /// `Conversation.abort()`: withdraw queued inputs, abort the ordinary
    /// ownership scope, and wait until it is idle.
    pub abort:
        Arc<dyn Fn(Context, Option<ConversationAbortOptions>) -> ApiFuture<()> + Send + Sync>,
    /// Resolve when the conversation's ordinary ownership scope has no live
    /// non-background task.
    pub wait_for_idle: Arc<dyn Fn(Context) -> ApiFuture<()> + Send + Sync>,
}

/// Pure decorator over one tool (`harness/types.ts` `ToolWrapper`); returns a
/// new tool with the same name and never mutates its input.
pub type ToolWrapper = Arc<dyn Fn(Arc<ToolRegistration>) -> Arc<ToolRegistration> + Send + Sync>;

/// Input to system prompt section rendering for one request preparation
/// (`harness/types.ts` `PromptInput`).
pub struct PromptInput {
    pub conversation_id: ConversationId,
    /// Active and registered tools in configured order, as offered in this
    /// request.
    pub tools: Vec<Arc<ToolRegistration>>,
    /// Sections already in effect after replaying the active transcript.
    pub shown: BTreeMap<String, String>,
    pub model: Option<ModelRef>,
    pub thinking_level: ModelThinkingLevel,
    /// Committed document reads.
    pub read: Arc<Session>,
}

/// One registered system prompt section (`harness/types.ts`
/// `PromptSection`); sections render in registry order before each request.
#[derive(Clone)]
pub struct PromptSection {
    pub key: String,
    pub render: SectionRenderFn,
    /// Default `true`: wrap the text as `<key>\n...\n</key>`.
    pub tag: Option<bool>,
}

/// `PromptSection.render(input, context)` (`string | undefined | Promise`).
pub type SectionRenderFn =
    Arc<dyn Fn(&PromptInput, &Context) -> super::prompt::SectionRenderFuture + Send + Sync>;

/// Pure decorator over one section (`harness/types.ts`
/// `PromptSectionWrapper`); returns a new section with the same key.
pub type PromptSectionWrapper = Arc<dyn Fn(PromptSection) -> PromptSection + Send + Sync>;

/// Wrapper composition failure found while building a snapshot
/// (`harness/types.ts` `RegistryFailure`).
#[derive(Debug, Clone)]
pub struct RegistryFailure {
    pub kind: RegistryFailureKind,
    /// Tool name or section key.
    pub name: String,
    pub error: PlainError,
}

/// `RegistryFailure.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryFailureKind {
    Tool,
    Section,
}

/// Immutable view of one published registry state (`harness/types.ts`
/// `RegistrySnapshot`).
pub trait RegistrySnapshotLike: Send + Sync {
    /// Composed tools in registry order; a tool whose wrapper failed is
    /// absent.
    fn tools(&self) -> Vec<Arc<ToolRegistration>>;
    fn tool(&self, name: &str) -> Option<Arc<ToolRegistration>>;
    /// Base tool names in registry order, including tools whose wrappers
    /// fail.
    fn tool_names(&self) -> Vec<String>;
    fn task(&self, name: &str) -> Option<TaskToken>;
    /// Hooks registered for tasks with this name, in registry order.
    fn hooks(&self, task_name: &str) -> Vec<HookRegistration>;
    /// Composed sections in registry order; a section whose wrapper failed
    /// is absent.
    fn sections(&self) -> Vec<PromptSection>;
    /// Wrapper failures of this state.
    fn failures(&self) -> Vec<RegistryFailure>;
    /// Conversation setups in registry order, the built-in `pi` setup first.
    fn conversation_setups(&self) -> Vec<ConversationSetupEntry>;
}

/// One conversation setup with its ordering key.
#[derive(Clone)]
pub struct ConversationSetupEntry {
    pub key: String,
    pub setup: ConversationSetup,
}

/// Read side of a registry consumed by a Harness (`harness/types.ts`
/// `RegistryReader`).
pub trait RegistryReaderLike: Send + Sync {
    /// Immutable view of the whole current registry.
    fn snapshot(&self) -> Arc<dyn RegistrySnapshotLike>;
    /// Called synchronously after every publication; returns an unsubscribe
    /// closure.
    fn subscribe(&self, listener: Box<dyn Fn() + Send + Sync>) -> Box<dyn FnOnce() + Send>;
}

/// Stages the documents every new conversation gets (`harness/types.ts`
/// `ConversationSetup`): `(tx, conversation, registry)`.
pub type ConversationSetup = Arc<
    dyn Fn(
            &Transaction,
            &ConversationRecord,
            Arc<dyn RegistrySnapshotLike>,
        ) -> Result<(), PlainError>
        + Send
        + Sync,
>;

/// Runs inside the creating commit, after the conversation and its
/// configuration exist (`harness/types.ts` `ConversationInit`).
pub type ConversationInit =
    Arc<dyn Fn(&Transaction, ConversationId) -> Result<(), PlainError> + Send + Sync>;

/// `ConversationCreateOptions` (`harness/types.ts`).
#[derive(Clone)]
pub struct ConversationCreateOptions {
    pub ownership: ConversationOwnership,
    pub init: Option<ConversationInit>,
}

/// Options of `Harness.open` (`harness/types.ts` `HarnessOptions`).
pub struct HarnessOptions {
    /// pi-ai model access used by generation.
    pub models: Option<Arc<dyn ModelsHandle>>,
    pub registry: Arc<dyn RegistryReaderLike>,
    /// Default execution environment offered to tools as `api.env`.
    pub env: Option<Arc<dyn ExecutionEnv>>,
    pub now: Option<Arc<dyn Fn() -> f64 + Send + Sync>>,
    /// Receives extension failures that do not fail the calling operation.
    pub on_report: Option<ReportFn>,
}

/// D15 (model access). Upstream `HarnessOptions.models` is the pi-ai
/// `Models` service consumed directly by the generation task; the port
/// abstracts the operations the built-in task uses behind this handle so the
/// harness slice carries no provider coupling: resolution, `streamSimple`
/// (the simple event stream plus its terminal `result()`), and the deferred
/// fetch/cancel pair. All operations take the caller's cancellation signal
/// separately, like upstream's per-call options.
pub trait ModelsHandle: Send + Sync {
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Arc<dyn ModelHandle>>;
    /// `Models.streamSimple(model, {messages}, options)` with `signal`
    /// factored out of the options.
    fn stream_simple(
        &self,
        model: Arc<dyn ModelHandle>,
        messages: Vec<Message>,
        options: SimpleStreamRequest,
    ) -> SimpleStreamResult;
    /// `Models.fetchDeferred(model, handle, { signal })`.
    fn fetch_deferred(
        &self,
        model: Arc<dyn ModelHandle>,
        handle: crate::ai::types::DeferredHandle,
        signal: Option<tokio_util::sync::CancellationToken>,
    ) -> ApiFuture<crate::ai::types::AssistantMessage>;
    /// `Models.cancelDeferred(model, handle, { signal })`.
    fn cancel_deferred(
        &self,
        model: Arc<dyn ModelHandle>,
        handle: crate::ai::types::DeferredHandle,
        signal: Option<tokio_util::sync::CancellationToken>,
    ) -> ApiFuture<()>;
}

/// The subset of a resolved model the generation task uses.
pub trait ModelHandle: Send + Sync {
    fn provider(&self) -> &str;
    fn model_id(&self) -> &str;
}

/// Upstream `SimpleStreamOptions` (`{...streamOptions, signal, sessionId,
/// reasoning?}`): the configured request options with the thinking level the
/// configuration selected, the conversation's provider session identity, and
/// the caller's signal.
#[derive(Debug, Clone, Default)]
pub struct SimpleStreamRequest {
    pub options: ConversationStreamOptions,
    /// Absent for `thinkingLevel: "off"` and for an unconfigured level.
    pub reasoning: Option<ModelThinkingLevel>,
    /// Upstream `sessionId: await ensureProviderSessionId(runtime, context)`
    /// (generation.ts @ 200387122): the stable `pi.provider` identity of the
    /// conversation, resolved before every provider request.
    pub session_id: Option<String>,
    pub signal: Option<tokio_util::sync::CancellationToken>,
}

/// One `streamSimple` event (`pi-ai` simple mode): the accumulated partial
/// message tagged with the event kind (`done` / `error` terminate the
/// stream; every other kind carries content deltas).
#[derive(Debug, Clone)]
pub struct SimpleStreamEvent {
    pub kind: SimpleStreamEventKind,
    pub partial: crate::ai::types::AssistantMessage,
}

/// `streamSimple` event kinds the generation task distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleStreamEventKind {
    Delta,
    Done,
    Error,
}

/// The stream plus its terminal result (`events` iterator + `events.result()`).
pub struct SimpleStreamResult {
    pub events: std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<SimpleStreamEvent, PlainError>> + Send + 'static>,
    >,
    /// Resolves with the terminal message.
    pub result: ApiFuture<crate::ai::types::AssistantMessage>,
}

/// Live task and what the scheduler would do with it under the current
/// registry (`harness/types.ts` `TaskInspection`).
#[derive(Debug, Clone)]
pub struct TaskInspection {
    pub record: TaskRecord,
    pub state: TaskInspectionState,
}

/// `TaskInspection.state`.
#[derive(Debug, Clone)]
pub enum TaskInspectionState {
    /// An invocation is active.
    Running,
    /// The next scheduling pass reserves it; `migrates` when its definition
    /// is newer and has `migrate`.
    Ready { migrates: bool },
    /// Waits for these live tasks.
    Waiting { on: Vec<TaskId> },
    /// Outcome held until its ordinary owned work drains.
    Completing,
    /// No registered definition can take it; aborting it settles it as
    /// `orphaned`.
    Blocked {
        reason: BlockedReason,
        error: Option<PlainError>,
    },
}

/// Blocked reasons (`scheduler.ts` `BlockedReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedReason {
    MissingTask,
    TaskTooOld,
    MigrationFailed,
}

impl BlockedReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockedReason::MissingTask => "missing_task",
            BlockedReason::TaskTooOld => "task_too_old",
            BlockedReason::MigrationFailed => "migration_failed",
        }
    }
}

/// Point-in-time view of live work (`harness/types.ts`
/// `HarnessInspection`).
#[derive(Debug, Clone)]
pub struct HarnessInspection {
    pub scheduling: SchedulingState,
    pub tasks: Vec<TaskInspection>,
    /// Queued and placed submissions, in ID order.
    pub submissions: Vec<SubmissionRecord>,
    /// Wrapper failures of the current registry snapshot.
    pub registry: Vec<RegistryFailure>,
}

/// `HarnessInspection.scheduling`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingState {
    Paused,
    Running,
    Closing,
}

/// Raw active transcript and derived model context (`harness/types.ts`
/// `ContextView`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContextView {
    /// Newest applicable head marker, if any.
    pub head: Option<EntryRecord>,
    /// Raw active entries: the head marker followed by non-head entries from
    /// its head through the tail.
    pub entries: Vec<EntryRecord>,
    /// Per entry of `entries` (v1.0.0), its model messages after edits and
    /// excluded stop reasons, before tool result ordering.
    pub contributions: Vec<Vec<Message>>,
    /// Model context for the next provider request.
    pub messages: Vec<Message>,
}

/// Why a compaction runs (v1.0.0): `compact()`, a threshold in generation
/// preparation, or a context overflow (`harness/types.ts`
/// `CompactionReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionReason {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "threshold")]
    Threshold,
    #[serde(rename = "overflow")]
    Overflow,
}

impl CompactionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CompactionReason::Manual => "manual",
            CompactionReason::Threshold => "threshold",
            CompactionReason::Overflow => "overflow",
        }
    }
}

/// Automatic compaction thresholds (v1.0.0, spec §8.7); manual compaction
/// ignores `enabled` (`harness/types.ts` `CompactionPolicy`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionPolicy {
    /// Threshold and overflow compaction.
    pub enabled: bool,
    /// Room kept free for the answer: generation blocks to compact above
    /// `contextWindow - reserveTokens`.
    pub reserve_tokens: i64,
    /// Approximate size of the recent context a summary keeps verbatim.
    pub keep_recent_tokens: i64,
    /// Background compaction starts `backgroundTokens` below the blocking
    /// threshold; `0` disables it.
    pub background_tokens: i64,
}

/// `entryId` of a blocking compaction's summary, or the `submissionId` of a
/// conversation-owned compaction's summary write; both absent when nothing
/// was compacted (`harness/types.ts` `CompactionResult`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<EntryId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission_id: Option<SubmissionId>,
}

/// What `HarnessOptions.env` builds an environment for (v1.0.0,
/// `harness/types.ts` `EnvTarget`).
#[derive(Debug, Clone)]
pub struct EnvTarget {
    pub conversation_id: ConversationId,
    /// The conversation's agent `cwd`.
    pub cwd: Option<String>,
}

/// Committed document reads (`harness/types.ts` `DocumentReader`, erased to
/// the port's synchronous reads): read one conversation document's current
/// value by kind, and one task document's current value by kind.
pub trait DocumentReads: Send + Sync {
    fn read_conversation_doc(
        &self,
        kind: &str,
        conversation_id: ConversationId,
    ) -> Result<Value, PlainError>;
    fn read_task_doc(&self, kind: &str, task_id: TaskId) -> Result<Value, PlainError>;
}

/// `Conversation.entries` query without the conversation ID
/// (`Omit<EntryQuery, "conversationId">`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntryQueryFilter {
    pub min_entry_id: Option<EntryId>,
    pub max_entry_id: Option<EntryId>,
}

impl EntryQueryFilter {
    /// Bound into a full [`EntryQuery`] for one conversation.
    pub fn bound(self, conversation_id: ConversationId) -> EntryQuery {
        EntryQuery {
            conversation_id,
            min_entry_id: self.min_entry_id,
            max_entry_id: self.max_entry_id,
        }
    }
}

/// The `status` subset that can still run code (`scheduler.ts`
/// `RunnableTaskRecord`).
pub fn is_runnable_status(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Pending | TaskStatus::Running | TaskStatus::Waiting
    )
}

/// Structural equality of two JSON values; object key order is ignored
/// (`scheduler.ts` `jsonEqual`).
pub fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.get(key).is_some_and(|other| json_equal(value, other))
                })
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(value, other)| json_equal(value, other))
        }
        _ => left == right,
    }
}

/// `Page<EntryRecord, Cursor>` as returned by entry scans.
pub type EntryPage = Page<EntryRecord>;
