//! Port of `src/tasks.ts`: `defineTask`, the executable-task constructor
//! registered in a [`crate::durable::harness`] registry so a Harness can run
//! tasks of its kind — plus the erased runtime the scheduler hands each
//! phase (`harness/scheduler.ts` `#runtime`, upstream type `TaskRuntime` in
//! root `types.ts`).
//!
//! Divergences (structural, disclosed):
//! - upstream `defineTask` is the identity constructor of the
//!   `Task<I, S, R, H>` token, with TypeScript inferring the phase-state
//!   `S extends { phase: string }`; the port's [`TaskToken`] carries the
//!   erased definition and the phase discriminant is the `phase` string of
//!   the stored checkpoint, so the constructor is likewise a bare wrapper.
//! - **D16 (async phases over a runtime trait).** Upstream phase handlers are
//!   `async` functions awaiting the runtime's promise-returning operations;
//!   the port keeps them async ([`PhaseFn`] returns a boxed future) and the
//!   runtime is the [`TaskRuntimeLike`] trait object the scheduler implements
//!   per invocation (the same shape as the harness `ToolExecutionApiLike`
//!   surface). Transaction operations are synchronous (D5), so
//!   [`TaskRuntimeLike::commit`] takes a synchronous change closure over the
//!   shared transaction and the current running record and resolves with the
//!   [`NextTaskState`] it stages.
//! - **D17 (memo read context).** Upstream `runtime.memo(name)` reads
//!   without a context; the port's memo operations carry one context
//!   parameter everywhere, so reads pass a plain background context.
//! - **D22 (token identity).** Upstream compares task tokens by object
//!   identity when a snapshot resolves a replacement definition; the port's
//!   [`TaskToken`] shares its definition behind an `Arc`, so `Arc::ptr_eq`
//!   on `definition` is the identity test.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::context::Context;

use super::documents::DocDefinition;
use super::errors::PlainError;
use super::harness::types::{
    ApiFuture, ContextView as HarnessContextView, ConversationHandle, HookHandlerFn, ModelsHandle,
    RegistrySnapshotLike,
};
use super::ids::{ConversationId, EntryId, TaskId};
use super::session::observation::CommittedWatch;
use super::session::session::Session;
use super::session::transaction::Transaction;
use super::types::{JoinPolicy, JsonObject, TaskOutcome, TaskRecord, TaskState, TaskStatus};

/// A phase handler of a task definition (`types.ts`
/// `TaskDefinition.phases`): `(task, runtime, context)`, resolved by the
/// checkpoint's `phase` name.
pub type PhaseFn = Arc<dyn Fn(PhaseArgs) -> PhaseFuture + Send + Sync>;

/// A phase handler future.
pub type PhaseFuture = Pin<Box<dyn Future<Output = Result<(), PlainFailure>> + Send>>;

/// Arguments a phase handler receives, in upstream order.
pub struct PhaseArgs {
    /// The running task record (`task`), with its input and checkpoint.
    pub record: TaskRecord,
    /// The erased invocation runtime (`runtime`).
    pub runtime: Arc<dyn TaskRuntimeLike>,
    /// The invocation context (`context`), cancelled with the invocation.
    pub context: Context,
}

/// A phase handler failure (`scheduler.ts` rule 4). Faults persist only the
/// message (`{status: "faulted", error: {message}}`).
#[derive(Debug, Clone)]
pub struct PlainFailure {
    pub message: String,
}

impl PlainFailure {
    pub fn new(message: impl Into<String>) -> Self {
        PlainFailure {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PlainFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<PlainError> for PlainFailure {
    fn from(error: PlainError) -> Self {
        PlainFailure {
            message: error.message,
        }
    }
}

/// `migrate(input, checkpoint, version)` (`types.ts`
/// `TaskDefinition.migrate`): convert a stored older task to the current
/// definition version.
pub type MigrateFn = Arc<
    dyn Fn(
            &Value,
            &serde_json::Map<String, Value>,
            i64,
        ) -> Result<(Value, serde_json::Map<String, Value>), PlainFailure>
        + Send
        + Sync,
>;

/// Erased executable task definition stored in the registry (`harness/types.ts`
/// `AnyTask.definition`).
#[derive(Clone)]
pub struct TaskDefinition {
    /// Registered task kind; the persisted record's `kind`.
    pub name: String,
    /// Definition version used to migrate live input and checkpoints.
    pub version: i64,
    /// `initial()`: first checkpoint of a fresh task.
    pub initial: Arc<dyn Fn() -> serde_json::Map<String, serde_json::Value> + Send + Sync>,
    /// Phase handlers by phase name.
    pub phases: BTreeMap<String, PhaseFn>,
    /// `abort(task, runtime, context)`: the abort handler.
    pub abort: Option<PhaseFn>,
    /// `migrate(input, checkpoint, version)`: convert a stored older task.
    pub migrate: Option<MigrateFn>,
}

impl super::session::transaction::TaskDefinitionFacet for TaskToken {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn version(&self) -> i64 {
        self.definition.version
    }

    fn initial(&self, _input: &Value) -> Value {
        Value::Object((self.definition.initial)())
    }
}

/// `Task<I, S, R, H>` (`types.ts`): the token `defineTask` returns. The
/// definition sits behind an `Arc` so clones share identity (D22).
#[derive(Clone)]
pub struct TaskToken {
    pub definition: Arc<TaskDefinition>,
}

/// `defineTask(definition)` (`tasks.ts:4-9`): define an executable task.
/// Register it in the registry so a Harness can run tasks of its kind.
pub fn define_task(definition: TaskDefinition) -> TaskToken {
    TaskToken {
        definition: Arc::new(definition),
    }
}

/// `NextTaskState<Checkpoint, R>` (`types.ts`): what a runtime commit writes
/// over the running state; `None` writes nothing.
#[derive(Debug, Clone)]
pub enum NextTaskState {
    Pending {
        checkpoint: Value,
    },
    Running {
        checkpoint: Value,
    },
    Waiting {
        checkpoint: Value,
        on: Vec<TaskId>,
        policy: JoinPolicy,
    },
    Terminal {
        outcome: TaskOutcome,
    },
}

impl NextTaskState {
    /// The state's `status` discriminant.
    pub fn status(&self) -> TaskStatus {
        match self {
            NextTaskState::Pending { .. } => TaskStatus::Pending,
            NextTaskState::Running { .. } => TaskStatus::Running,
            NextTaskState::Waiting { .. } => TaskStatus::Waiting,
            NextTaskState::Terminal { .. } => TaskStatus::Terminal,
        }
    }

    /// The state as a stored [`TaskState`].
    pub fn into_task_state(self) -> TaskState {
        match self {
            NextTaskState::Pending { checkpoint } => TaskState::Pending { checkpoint },
            NextTaskState::Running { checkpoint } => TaskState::Running { checkpoint },
            NextTaskState::Waiting {
                checkpoint,
                on,
                policy,
            } => TaskState::Waiting {
                checkpoint,
                on,
                policy,
            },
            NextTaskState::Terminal { outcome } => TaskState::Terminal { outcome },
        }
    }
}

/// Synchronous change closure of [`TaskRuntimeLike::commit`] (upstream an
/// async `(tx, current) => Next | undefined`): stage writes on the
/// transaction and resolve with the next task state, or `None` to write
/// nothing.
pub type CommitChange =
    Box<dyn FnOnce(&Transaction, &TaskRecord) -> Result<Option<NextTaskState>, PlainError> + Send>;

/// One matching hook handler invocation (`hooks.each(name, invoke)`): the
/// callback receives each handler and runs it against its payload, resolving
/// with the handler's replacement value.
pub type HookInvoke = Arc<
    dyn Fn(
            &HookHandlerFn,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Value>, PlainError>> + Send>>
        + Send
        + Sync,
>;

/// The boxed future of a runtime operation. The borrow is the runtime
/// itself; operations reject once the invocation ends.
pub type RuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, PlainError>> + Send + 'a>>;

/// Operations available to one task phase (`harness/scheduler.ts`
/// `#runtime`; upstream root `types.ts` `TaskRuntime`). Every operation
/// rejects once the invocation has ended; the scheduler implements the trait
/// once per invocation.
pub trait TaskRuntimeLike: Send + Sync {
    /// `runtime.taskId`.
    fn task_id(&self) -> TaskId;
    /// `runtime.conversationId`.
    fn conversation_id(&self) -> ConversationId;
    /// `runtime.signal`: cancelled when the invocation ends.
    fn signal(&self) -> CancellationToken;
    /// `runtime.signal.aborted`.
    fn aborted(&self) -> bool;
    /// `runtime.signal.throwIfAborted()`: fail with the abort error text.
    fn throw_if_aborted(&self) -> Result<(), PlainFailure>;
    /// `runtime.models`.
    fn models(&self) -> Option<Arc<dyn ModelsHandle>>;
    /// `runtime.env`.
    fn env(&self) -> Option<Arc<dyn super::env::ExecutionEnv>>;
    /// `runtime.hooks`: run every registered handler of `name` under the
    /// invocation's error policy.
    fn hooks(&self, name: &str, invoke: HookInvoke) -> RuntimeFuture<'_, ()>;
    /// `runtime.registry`.
    fn registry(&self) -> Arc<dyn RegistrySnapshotLike>;
    /// `runtime.commit`.
    fn commit(
        &self,
        change: CommitChange,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + '_>>;
    /// `runtime.memo(name)` / `runtime.memo(name, candidate)` (D17).
    fn memo(
        &self,
        name: &str,
        candidate: Option<Value>,
        context: Context,
    ) -> RuntimeFuture<'_, Option<Value>>;
    /// `runtime.sleep`.
    fn sleep(
        &self,
        until: f64,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + '_>>;
    /// `runtime.watchDoc`.
    fn watch_doc(
        &self,
        definition: &DocDefinition,
        owner: Option<ConversationId>,
        key: Option<&str>,
        context: Context,
    ) -> RuntimeFuture<'_, Option<Arc<CommittedWatch>>>;
    /// `runtime.snapshot`.
    fn snapshot(
        &self,
        definition: &DocDefinition,
        owner: Option<ConversationId>,
        key: Option<&str>,
        context: Context,
    ) -> RuntimeFuture<'_, Option<JsonObject>>;
    /// `runtime.snapshotAsOf`.
    fn snapshot_as_of(
        &self,
        definition: &DocDefinition,
        conversation_id: ConversationId,
        key: Option<&str>,
        at: i64,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JsonObject>, PlainError>> + Send + '_>>;
    /// `runtime.getTask`.
    fn get_task(&self, id: TaskId, context: Context) -> ApiFuture<Option<TaskRecord>>;
    /// `runtime.waitForTask`.
    fn wait_for_task(&self, id: TaskId, context: Context) -> ApiFuture<TaskRecord>;
    /// `runtime.outcomes`.
    fn outcomes(&self, ids: Vec<TaskId>, context: Context) -> ApiFuture<Vec<TaskOutcome>>;
    /// `runtime.conversation`.
    fn conversation(
        &self,
        id: ConversationId,
        context: Context,
    ) -> ApiFuture<Option<ConversationHandle>>;
    /// `runtime.entry`.
    fn entry(
        &self,
        kind: Option<String>,
        id: EntryId,
        context: Context,
    ) -> ApiFuture<Option<super::types::EntryRecord>>;
    /// `runtime.context`.
    fn context(
        &self,
        conversation_id: ConversationId,
        context: Context,
        at: Option<EntryId>,
    ) -> ApiFuture<HarnessContextView>;
    /// `runtime.now()`.
    fn now(&self) -> f64;
    /// `runtime.report(error)`.
    fn report(&self, error: &PlainError);
    /// The Session kernel behind the committed reads, for hook APIs and
    /// prompt inputs (upstream the runtime forwards the Session's
    /// `DocumentReader` surface; the port hands out the Arc directly).
    fn session(&self) -> Arc<Session>;
}
