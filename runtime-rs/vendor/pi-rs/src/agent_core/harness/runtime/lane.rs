//! The real serialized command line of one lane, from upstream
//! `runtime/lane.ts` (`readLane`/`command`/`settleOperation`/
//! `continueOperation`, run admission, configuration mutation, seal) and
//! `runtime/types.ts` (`Config`/`LaneCommand`/`OperationCommand`/
//! `ContinueOperationResult`; `Drive`/`ProcedureResult` live in
//! `drive_pass.rs`, the owned [`LaneState`] in `durable.rs`).
//!
//! Disclosed substitutions for review:
//! - Upstream single-threaded field reads (`this.state`) become snapshots
//!   cloned from a `Mutex`; the owned state is still updated only by
//!   replacement, only while the session mutation line is held.
//! - The `stateChange` promise and the `idleOwner` promise become a `watch`
//!   generation channel plus a `CancellationToken`. An already-signalled
//!   generation wakes immediately (`has_changed`), preserving the upstream
//!   resolved-promise liveness.
//! - `materialize` is synchronous at the type level: `FnOnce(&CommitResult)
//!   -> T` accepts no async closure, standing in for the upstream thenable
//!   TypeError (see the [`LaneCommand::Commit`] compile-fail doctest).
//! - `settleOperation`'s upstream `_capability` parameter is a JS type
//!   narrowing aid; the Rust planner matches on [`OperationState`] directly.
//!   No pico3-style permission identity is injected here.
//! - Installed drives share the dispatcher with explicit `DriveEnv` callers,
//!   but read live native tools/context at each tool batch. Type erasure
//!   preserves callbacks and application handles; tool events still flow
//!   through the lane's emit facility.
//! - `requestOperationAbort`'s admission-gate promise becomes a
//!   `CancellationToken` passed to `Drive::begin_abort`; the promise's
//!   rejection value (which only releases waiters upstream) has no carrier,
//!   so the failing gate releases by cancellation alone.
//! - `watch()` reaches the harness event bus through an injected
//!   [`LaneWatchInstaller`] (`new Lane`'s upstream `installWatch` parameter,
//!   supplied by the harness at lane construction); the initial snapshot is
//!   captured with the bus's `watchFromSnapshot` install-capture-mark
//!   primitive instead of the upstream assign-then-buffer dance.
//! - Not ported in this slice: the public AgentHarness shell's own remaining
//!   seams (`watchSession`).

pub mod native_tools;

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use futures::future::BoxFuture;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use self::native_tools::NativeRuntimeTools;
use crate::agent_core::harness::agent_harness::drive_operation::{
    drive_operation_with_env, DriveEnvironment,
};
use crate::agent_core::harness::agent_harness::{
    CancelQueuedOutcome, CancelQueuedResult, CompactionOutcome, CompactionResult, DriveResult,
    NavigationOutcome, NavigationResult, QueueInput, QueueOutcome, QueueResult, RecordUsageOptions,
    RecordUsageOutcome, RecordUsageResult, ResumeResult, RunOutcome, RunResult,
    SliceNotImplemented, SuspendedRun, SuspendedStatus,
};
use crate::agent_core::harness::compaction::branch_summarization::{
    prepare_branch_entries, BranchPreparation,
};
use crate::agent_core::harness::compaction::prepare_compaction;
use crate::agent_core::harness::config::{
    CompactionSettings, DEFAULT_COMPACTION_SETTINGS, DEFAULT_RETRY_POLICY,
};
use crate::agent_core::harness::context::{await_with_context, Context};
use crate::agent_core::harness::events::{BusEvent, SnapshotCapture, WatchHandle};
use crate::agent_core::harness::execution::tools::tool_result_from_message;
use crate::agent_core::harness::hooks::HookRegistry;
use crate::agent_core::harness::result::TaggedError;
use crate::agent_core::harness::runtime::drive::reconcile::DeferredCancelFn;
use crate::agent_core::harness::runtime::drive::structural::{
    durable_branch_preparation, durable_compaction_preparation,
};
use crate::agent_core::harness::runtime::drive::tools::{
    run_tools_from_config, ToolEvent, ToolEventEmit,
};
use crate::agent_core::harness::runtime::drive_pass::{
    Drive, DriveOptions, DriveOutcome, WaitingReason,
};
use crate::agent_core::harness::runtime::durable::{
    LaneState, Operation, OperationIntent, OperationMeta, OperationPhase, OperationScope,
    OperationState, ResultBoundary, RunSettings, SummaryReason, SummaryTask,
};
use crate::agent_core::harness::runtime::events::{ConfigUpdateProperty, HarnessEvent};
use crate::agent_core::harness::runtime::progress::read_assistant_frames;
use crate::agent_core::harness::runtime::projection::{
    DeferredSnapshot, LaneOperationSnapshot, LaneQueuedItem, LaneSnapshot, LaneSnapshotTool,
    OperationKind, RetrySnapshot, SnapshotToolState, WriteKind,
};
use crate::agent_core::harness::runtime::transcript::{
    chain_entries, committed_entry_events, read_lane_queues,
};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::harness::session::{
    delete_value, insert_entry, insert_usage, lane_config, lane_state, operation_meta,
    operation_preparation, operation_result, operation_state, operation_tool_args, pending_entry,
    pending_tool_output, set_value, BranchScanOrder, CommitResult, Control, EntryType, InboxItem,
    InboxItemKind, LaneConfiguration, LaneModel, LaneState as DurableLaneState, NewEntry,
    NewUsageRow, OperationResultRecord, PendingEntry, SessionInvariantError, SessionMutator,
    SessionPendingAssistantMessageError, StorageBackedSession, StorageBranchScan, UsageRow, Write,
};
use crate::agent_core::types::{AgentMessage, QueueMode, ThinkingLevel, ToolExecutionMode};
use crate::ai::frame::{reduce_assistant_message_frames, AssistantMessageFrame};
use crate::ai::models::Models;
use crate::ai::types::content::{ImageContent, TextContent};
use crate::ai::types::message::{StringOrBlocks, TextOrImageBlock, UserMessage};
use crate::ai::types::primitives::StopReason;
use crate::ai::types::primitives::Usage;

/// Upstream `FaultHandler` (`lane.ts:113`): maps a planner/commit failure to
/// the error thrown at the fault boundary. The test default is the identity.
pub type FaultHandler = Arc<dyn Fn(anyhow::Error) -> anyhow::Error + Send + Sync>;

/// One asynchronous event batch delivery, awaited only after the mutation
/// line was released (upstream `EmitBatch`).
pub type EmitBatchFuture = BoxFuture<'static, anyhow::Result<()>>;
pub type EmitBatch = Arc<dyn Fn(Vec<HarnessEvent>, Context) -> EmitBatchFuture + Send + Sync>;

/// Upstream process-local `Config<TContext>`. Native tools/context retain
/// their typed functions through [`NativeRuntimeTools`]; declarations remain
/// separate for provider requests. The callable system-prompt form remains a
/// separate migration boundary.
#[derive(Clone)]
pub struct RuntimeConfig {
    pub compaction: CompactionSettings,
    /// Upstream `AgentOptions.retry` (`agent-harness.ts:529`): the retry
    /// policy `normalizedRetryPolicy` projects for new generations.
    pub retry_policy: crate::ai::retry::RetryPolicy,
    /// Upstream `AgentOptions.streamOptions` (`agent-harness.ts:528`).
    pub stream_options: crate::agent_core::harness::types::AgentHarnessStreamOptions,
    /// Upstream `AgentOptions.resources` (`agent-harness.ts:527`): the skills
    /// and prompt templates the before_run/skill surfaces read.
    pub resources: crate::agent_core::harness::hooks::Resources,
    /// Upstream `AgentOptions.systemPrompt` (`agent-harness.ts:526`): the
    /// string form only; the callable form is a separate migration boundary.
    pub system_prompt: Option<String>,
    /// Upstream `AgentOptions.tools` (`agent-harness.ts:524`): LLM-facing
    /// declarations, projected from the same tools as `native_tools`.
    pub tools: Vec<crate::ai::types::tool::Tool>,
    /// Native executors and lazy context for the current configuration.
    /// Use `with_native_tools` to keep declarations and executors together.
    pub native_tools: NativeRuntimeTools,
    /// Upstream `AgentOptions.toProviderMessages` (`agent-harness.ts:534`);
    /// `None` uses the port's default AgentMessage-to-provider conversion.
    pub to_provider_messages: Option<
        std::sync::Arc<crate::agent_core::harness::execution::assistant::ToProviderMessagesFn>,
    >,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub tool_execution: ToolExecutionMode,
    pub entry_projectors:
        Option<BTreeMap<String, crate::agent_core::harness::session::context::EntryProjector>>,
}

impl Default for RuntimeConfig {
    /// The upstream option-level defaults (`AgentOptions`), not any test
    /// fixture's values: `"one-at-a-time"` queues, `"parallel"` tools, and
    /// `DEFAULT_COMPACTION_SETTINGS`.
    fn default() -> Self {
        Self {
            compaction: DEFAULT_COMPACTION_SETTINGS,
            retry_policy: DEFAULT_RETRY_POLICY,
            stream_options: Default::default(),
            resources: Default::default(),
            system_prompt: None,
            tools: Vec::new(),
            native_tools: NativeRuntimeTools::default(),
            to_provider_messages: None,
            steering_mode: QueueMode::DEFAULT,
            follow_up_mode: QueueMode::DEFAULT,
            tool_execution: ToolExecutionMode::DEFAULT,
            entry_projectors: None,
        }
    }
}
impl RuntimeConfig {
    /// Configure executors and their provider-facing declarations together.
    /// No context provider is evaluated while projecting the snapshot.
    pub fn with_native_tools<T: Clone + Send + Sync + 'static>(
        mut self,
        tools: Vec<crate::agent_core::harness::types::AgentHarnessTool<T>>,
        context: Option<crate::agent_core::harness::runtime::drive::tools::ToolContextSource<T>>,
    ) -> Self {
        self.native_tools = NativeRuntimeTools::new(tools, context);
        self.tools = self.native_tools.declarations();
        self
    }
}

pub type ReadRuntimeConfig = Arc<dyn Fn() -> RuntimeConfig + Send + Sync>;

/// The first seal error of a lane (upstream `closedError`). Every rejected
/// caller observes the same `Arc` — the port of the upstream shared error
/// object identity.
#[derive(Debug)]
pub struct LaneSealed {
    pub kind: SealKind,
    pub message: String,
}
impl fmt::Display for LaneSealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for LaneSealed {}

/// Upstream `HarnessClosed` vs `HarnessFault`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealKind {
    Closed,
    Fault,
}

/// Upstream `awaitWithContext` abort rejection (DOMException AbortError).
#[derive(Debug)]
pub struct AbortedError;
impl fmt::Display for AbortedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "The operation was aborted")
    }
}
impl std::error::Error for AbortedError {}

/// Expected admission rejections (upstream `Result.err` values): the lane is
/// sealed-closed, already busy, or the request carries no usable message.
/// The new admission vocabulary (skill/template/compaction/navigation) maps
/// onto [`TaggedError`] at the acceptance boundary.
#[derive(Debug)]
pub enum AcceptanceError {
    Closed {
        message: String,
    },
    LaneBusy {
        lane: String,
        operation_id: String,
        operation_kind: &'static str,
    },
    InvalidMessage {
        lane: String,
        reason: &'static str,
        message: String,
    },
}
impl fmt::Display for AcceptanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed { message } => write!(f, "{message}"),
            Self::LaneBusy {
                lane,
                operation_id,
                operation_kind,
            } => write!(
                f,
                "Lane {} already has an active operation ({} {})",
                serde_json::to_string(lane).unwrap_or_default(),
                operation_kind,
                operation_id
            ),
            Self::InvalidMessage { message, .. } => write!(f, "{message}"),
        }
    }
}
impl std::error::Error for AcceptanceError {}

/// Upstream `OperationRequest` (`agent-harness.ts:111-117`): the full
/// discriminated admission vocabulary. The `prompt` arms (string, string with
/// images, message input) mirror the upstream `prompt` overloads; the caller
/// may pin the operation id through [`Lane::accept_with_id`].
#[derive(Debug, Clone, PartialEq)]
pub enum OperationRequest {
    /// Upstream `{ kind: "prompt", prompt: string }` (no images).
    Prompt { prompt: String },
    /// Upstream `{ kind: "prompt", prompt: string, images }`.
    PromptWithImages {
        prompt: String,
        images: Vec<ImageContent>,
    },
    /// Upstream `{ kind: "prompt", prompt: AgentMessage | AgentMessage[] }`.
    PromptMessages { messages: Vec<AgentMessage> },
    /// Upstream `{ kind: "skill", name, additionalInstructions? }`.
    Skill {
        name: String,
        additional_instructions: Option<String>,
    },
    /// Upstream `{ kind: "prompt_template", name, args? }`.
    PromptTemplate {
        name: String,
        args: Option<Vec<String>>,
    },
    /// Upstream `{ kind: "compaction", customInstructions? }`.
    Compaction { custom_instructions: Option<String> },
    /// Upstream `{ kind: "navigation", targetId, options? }`.
    Navigation {
        target_id: Option<String>,
        options: Option<NavigationOptions>,
    },
}

/// Upstream `NavigateOptions` (`agent-harness.ts:105-109`), narrowed to the
/// members the navigation admission reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavigationOptions {
    pub summarize: Option<bool>,
    pub label: Option<String>,
    pub custom_instructions: Option<String>,
}

/// Upstream `OperationAdmission` (`agent-harness.ts`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationAdmission {
    pub operation_id: String,
    /// Upstream `"run" | "compaction" | "navigation"` literal; only `"run"`
    /// is admitted in this slice.
    pub kind: &'static str,
    pub started_at: i64,
}

/// Synchronous event builders over the commit metadata (upstream
/// `events?(commit): HarnessEvent[]`).
pub(crate) type CommitEventsFn =
    Box<dyn FnOnce(&CommitResult) -> anyhow::Result<Vec<HarnessEvent>> + Send>;

/// Upstream `setConfiguration`'s event builder: maps the previous and next
/// configurations to a config-update event payload.
pub(crate) type ConfigUpdateEventFn =
    Arc<dyn Fn(&LaneConfiguration, &LaneConfiguration) -> ConfigUpdateProperty + Send + Sync>;

/// The public event union the lane's watchers subscribe to (upstream
/// `HarnessEvent` of `agent-harness.ts`, the bus element type).
pub type PublicHarnessEvent = crate::agent_core::harness::agent_harness::HarnessEvent;

/// The watch handle a lane hands out (upstream `WatchHandle<LaneSnapshot>`).
pub type LaneWatchHandle = WatchHandle<LaneSnapshot, PublicHarnessEvent>;

/// Upstream `WatchHandler`'s event filter argument, narrowed to the lane
/// snapshot element type. `Box<dyn Fn>` because the bus's install API takes
/// a generic `F: Fn(&E) -> bool` and `Box`ed trait objects implement `Fn`.
pub type LaneWatchFilter = Box<dyn Fn(&PublicHarnessEvent) -> bool + Send + Sync>;

/// Upstream `new Lane(...)`'s `installWatch` parameter: the harness-provided
/// bridge from a lane to the shared event bus. It receives the (already
/// lane-filtering) capture, the event filter, and the caller context, and
/// installs the watcher with its install-capture-mark primitive (upstream
/// `events.watchWithResnapshot`).
pub type LaneWatchInstaller = Arc<
    dyn Fn(
            SnapshotCapture<LaneSnapshot>,
            LaneWatchFilter,
            Context,
        ) -> BoxFuture<'static, anyhow::Result<LaneWatchHandle>>
        + Send
        + Sync,
>;

/// One effect-free decision made on the lane's serialized mutation line
/// (upstream `LaneCommand`). `materialize` is synchronous by construction.
/// The `Commit` variant intentionally stores the large owned `next` state by
/// value like the upstream object literal, so the variant-size lint is
/// accepted rather than adding a boxing indirection upstream does not have.
#[allow(clippy::large_enum_variant)]
pub enum LaneCommand<T> {
    Commit {
        writes: Vec<Write>,
        next: LaneState,
        /// Synchronous materialization of the caller result from
        /// storage-assigned commit metadata (upstream
        /// `materialize(commit): Synchronous<TResult>`). The upstream runtime
        /// TypeError for thenable materializers is enforced here by the type
        /// system:
        ///
        /// ```compile_fail
        /// use pi_rust::agent_core::harness::runtime::lane::LaneCommand;
        /// use pi_rust::agent_core::harness::session::CommitResult;
        /// // The pinned result type makes a future-producing materialize
        /// // fail to typecheck, standing in for the upstream synchronous
        /// // guard without awaiting anything on the mutation line.
        /// fn build() -> LaneCommand<String> {
        ///     LaneCommand::Commit {
        ///         writes: Vec::new(),
        ///         next: unimplemented!(),
        ///         materialize: Box::new(|_commit: &CommitResult| async { "late".to_string() }),
        ///         events: None,
        ///     }
        /// }
        /// # fn main() { let _ = build(); }
        /// ```
        materialize: Box<dyn FnOnce(&CommitResult) -> T + Send>,
        events: Option<CommitEventsFn>,
    },
    Return {
        result: T,
    },
    Reject {
        error: anyhow::Error,
    },
}

/// A durable operation transition decided against the current operation
/// (upstream `OperationCommand`). The Lane pairs the state write with
/// projection publication. As with [`LaneCommand`], the large `Commit`
/// variant keeps the owned state by value, matching the upstream literal.
#[allow(clippy::large_enum_variant)]
pub enum OperationCommand<T> {
    Commit {
        writes: Vec<Write>,
        operation_state: OperationState,
        lane: Option<LanePatch>,
        materialize: Box<dyn FnOnce(&CommitResult) -> T + Send>,
        events: Option<CommitEventsFn>,
    },
    Finish {
        writes: Vec<Write>,
        record: OperationResultRecord,
        lane: Option<LanePatch>,
        materialize: Box<dyn FnOnce(&CommitResult) -> T + Send>,
        events: Option<CommitEventsFn>,
    },
    Return {
        result: T,
    },
}

/// Upstream `LanePatch`: the durable lane fields an operation command may
/// update alongside its operation-state write.
#[derive(Debug, Clone, Default)]
pub struct LanePatch {
    pub tip_id: Option<String>,
    pub configuration: Option<LaneConfiguration>,
    pub inbox: Option<Vec<InboxItem>>,
}

/// Upstream `ContinueOperationResult`.
#[derive(Debug, Clone, PartialEq)]
pub enum ContinueOperationResult<T> {
    CancelRequested,
    Result { value: T },
}

/// One settled command attempt, internally tagged before the line is released.
enum CommandOutcome<T> {
    Return {
        result: T,
        delivery: Option<EmitBatchFuture>,
    },
    Reject {
        error: anyhow::Error,
    },
    IdleBlocked {
        owner: CancellationToken,
    },
}

/// The mutable lane projection, guarded by a short critical-section `Mutex`
/// (never locked across `.await`).
struct LaneShared {
    state: LaneState,
    closed_error: Option<Arc<LaneSealed>>,
    active_drive: Option<Arc<Drive>>,
    idle_owner: Option<CancellationToken>,
    /// The harness-installed event-bus bridge (upstream the `installWatch`
    /// constructor parameter). `None` until the harness installs it.
    watch_install: Option<LaneWatchInstaller>,
}

/// Owned handle to the lane's shared coordination state, moved into `mutate`
/// callbacks: the session `mutate` HRTB produces `for<'a>` futures, which
/// cannot borrow `&self` or a borrowed planner, so each callback moves owned
/// clones of exactly the facilities it needs.
#[derive(Clone)]
struct LaneLease {
    session: Arc<StorageBackedSession>,
    shared: Arc<Mutex<LaneShared>>,
    state_change: watch::Sender<u64>,
    on_fault: FaultHandler,
    emit_batch: EmitBatch,
}

impl LaneLease {
    fn lock_shared(&self) -> std::sync::MutexGuard<'_, LaneShared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state(&self) -> LaneState {
        self.lock_shared().state.clone()
    }

    fn assert_open(&self) -> anyhow::Result<()> {
        match &self.lock_shared().closed_error {
            None => Ok(()),
            Some(sealed) => Err(anyhow::Error::new(sealed.clone())),
        }
    }

    fn fault(&self, error: anyhow::Error) -> anyhow::Error {
        if let Some(sealed) = self.lock_shared().closed_error.clone() {
            return anyhow::Error::new(sealed);
        }
        (self.on_fault)(error)
    }

    fn signal_state_change(&self) {
        self.state_change.send_modify(|generation| *generation += 1);
    }
}

/// Runtime implementation of one configured lane.
pub struct Lane {
    name: String,
    session: Arc<StorageBackedSession>,
    models: Arc<Models>,
    /// Stored for the drive-procedures slice (upstream `this.hooks`).
    hooks: HookRegistry,
    shared: Arc<Mutex<LaneShared>>,
    /// Weak self-handle for the detached drive pass (upstream `drive()`
    /// detaches `driveOperation(this, ...)` while `this` stays alive).
    self_weak: Weak<Lane>,
    /// Generation counter bumped by every owned-state replacement; replaces
    /// the upstream `stateChange` promise. The private receiver keeps the
    /// channel alive for `send_modify` when no waiter is subscribed.
    state_change: watch::Sender<u64>,
    /// Deliberately unread: holding this receiver open is what keeps the
    /// `state_change` channel alive for `send_modify` (upstream keeps the
    /// promise self-referenced for the same reason).
    #[allow(dead_code)]
    state_change_self: watch::Receiver<u64>,
    on_fault: FaultHandler,
    emit_batch: EmitBatch,
    read_config: ReadRuntimeConfig,
}

impl fmt::Debug for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lane")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

fn invariant(message: String) -> anyhow::Error {
    SessionInvariantError(message).into()
}

fn captured_settings(config: &RuntimeConfig) -> RunSettings {
    RunSettings {
        compaction: config.compaction,
        steering_mode: config.steering_mode,
        follow_up_mode: config.follow_up_mode,
        tool_execution: config.tool_execution,
    }
}

/// Upstream `durableLaneState`: the durable projection with explicit
/// fallbacks to the owned state (upstream default parameters).
fn durable_lane_state(
    state: &LaneState,
    current_operation_id: Option<&str>,
    inbox: Option<&[InboxItem]>,
    last_operation_id: Option<&str>,
) -> DurableLaneState {
    DurableLaneState {
        current_operation_id: current_operation_id.map(str::to_owned),
        last_operation_id: last_operation_id
            .map(str::to_owned)
            .or_else(|| state.last_operation_id.clone()),
        inbox: match inbox {
            Some(inbox) => serde_json::to_value(inbox).expect("inbox serializes"),
            None => serde_json::to_value(&state.inbox).expect("inbox serializes"),
        },
    }
}

fn lane_next(state: &LaneState, lane: Option<&LanePatch>) -> LaneState {
    let mut next = state.clone();
    if let Some(lane) = lane {
        if let Some(tip_id) = &lane.tip_id {
            next.tip_id = Some(tip_id.clone());
        }
        if let Some(configuration) = &lane.configuration {
            next.configuration = configuration.clone();
        }
        if let Some(inbox) = &lane.inbox {
            next.inbox = inbox.clone();
        }
    }
    next
}

/// Upstream `selectAcceptedInbox`: steering/follow-up one-at-a-time limits.
fn select_accepted_inbox(
    inbox: &[InboxItem],
    steering_mode: &QueueMode,
    follow_up_mode: &QueueMode,
) -> (Vec<InboxItem>, Vec<InboxItem>) {
    let mut steer_taken = false;
    let mut follow_up_taken = false;
    let mut selected = Vec::new();
    let mut remainder = Vec::new();
    for item in inbox {
        let eligible = item.kind == InboxItemKind::Write
            || item.kind == InboxItemKind::NextRun
            || (item.kind == InboxItemKind::Steer
                && (*steering_mode == QueueMode::All || !steer_taken))
            || (item.kind == InboxItemKind::FollowUp
                && (*follow_up_mode == QueueMode::All || !follow_up_taken));
        if eligible {
            if item.kind == InboxItemKind::Steer {
                steer_taken = true;
            }
            if item.kind == InboxItemKind::FollowUp {
                follow_up_taken = true;
            }
            selected.push(item.clone());
        } else {
            remainder.push(item.clone());
        }
    }
    (selected, remainder)
}

/// Upstream `pendingEntryWrite`: staged payload as a chainable new entry.
fn pending_entry_write(entry_id: &str, pending: &PendingEntry) -> NewEntry {
    match pending {
        PendingEntry::Message { payload } => NewEntry::Message {
            id: entry_id.to_owned(),
            parent_id: None,
            message: payload.clone(),
            terminate: None,
        },
        PendingEntry::Custom {
            custom_type,
            payload,
        } => NewEntry::Custom {
            id: entry_id.to_owned(),
            parent_id: None,
            custom_type: custom_type.clone(),
            data: payload.clone(),
        },
    }
}

fn inbox_kind_name(kind: InboxItemKind) -> &'static str {
    match kind {
        InboxItemKind::Steer => "steer",
        InboxItemKind::FollowUp => "followUp",
        InboxItemKind::NextRun => "nextRun",
        InboxItemKind::Write => "write",
    }
}

/// Upstream `withoutInboxItems` (lane.ts:138-141).
fn without_inbox_items(inbox: &[InboxItem], removed: &[InboxItem]) -> Vec<InboxItem> {
    let removed_ids: HashSet<&str> = removed.iter().map(|item| item.entry_id.as_str()).collect();
    inbox
        .iter()
        .filter(|item| !removed_ids.contains(item.entry_id.as_str()))
        .cloned()
        .collect()
}

/// The upstream `prompt`-kind message construction (lane.ts:505-527): a user
/// message whose content is the block array `[text?, ...images]`, or no
/// message at all when both are empty. The upstream literal always produces
/// the block-array form, so the port does too (fixing the landed
/// `StringOrBlocks::Text` short form).
fn prompt_text_messages(
    prompt: &str,
    images: &[ImageContent],
    started_at: i64,
) -> Vec<AgentMessage> {
    if prompt.is_empty() && images.is_empty() {
        return Vec::new();
    }
    let mut blocks: Vec<TextOrImageBlock> = Vec::new();
    if !prompt.is_empty() {
        blocks.push(TextOrImageBlock::Text(TextContent {
            text: prompt.to_owned(),
            text_signature: None,
        }));
    }
    blocks.extend(images.iter().cloned().map(TextOrImageBlock::Image));
    vec![AgentMessage::User(UserMessage {
        content: StringOrBlocks::Blocks(blocks),
        timestamp: started_at,
    })]
}

/// Upstream `Lane.mismatch` (lane.ts:280-288).
fn mismatch_error(
    lane: &str,
    expected: &str,
    current_operation_id: Option<&str>,
    last_operation_id: Option<&str>,
) -> TaggedError {
    TaggedError::OperationMismatch {
        lane: lane.to_owned(),
        expected_operation_id: expected.to_owned(),
        current_operation_id: current_operation_id.map(str::to_owned),
        last_operation_id: last_operation_id.map(str::to_owned),
        message: format!(
            "Operation {expected} does not own lane {}",
            serde_json::to_string(lane).unwrap_or_default()
        ),
    }
}

/// The internal [`AcceptanceError`] onto the public tagged-error vocabulary
/// (upstream the shared `Result.err` channel).
fn acceptance_error_to_tagged(error: AcceptanceError) -> TaggedError {
    match error {
        AcceptanceError::Closed { message } => TaggedError::Closed { message },
        AcceptanceError::LaneBusy {
            lane,
            operation_id,
            operation_kind,
        } => {
            let lane_json = serde_json::to_string(&lane).unwrap_or_default();
            TaggedError::LaneBusy {
                lane,
                operation_id,
                operation_kind: match operation_kind {
                    "run" => crate::agent_core::harness::result::OperationKind::Run,
                    "compaction" => crate::agent_core::harness::result::OperationKind::Compaction,
                    _ => crate::agent_core::harness::result::OperationKind::Navigation,
                },
                message: format!("Lane {lane_json} already has an active operation"),
            }
        }
        AcceptanceError::InvalidMessage {
            lane,
            reason,
            message,
        } => TaggedError::InvalidMessage {
            lane,
            reason: reason.to_owned(),
            message,
        },
    }
}

/// Upstream `DriveOutcome.reason` spelling (the `WaitingReason` wire tag).
fn waiting_reason_name(reason: &WaitingReason) -> &'static str {
    match reason {
        WaitingReason::Retry { .. } => "retry",
        WaitingReason::Deferred { .. } => "deferred",
    }
}

/// The `RunResult` value side of a driven run (upstream the
/// `driven.value.kind === "settled"` / `reason === "deferred"` mapping of
/// `driveRunRequest`, `resume`, and `continueAfterStructural`). An unwaited
/// retry faults with the caller-prefixed invariant.
fn run_outcome_from_drive(outcome: &DriveOutcome, label: &str) -> anyhow::Result<RunOutcome> {
    match outcome {
        DriveOutcome::Settled { outcome } => Ok(RunOutcome::Record(outcome.clone())),
        DriveOutcome::Waiting {
            operation_id,
            reason,
        } => match reason {
            WaitingReason::Deferred { deferred } => Ok(RunOutcome::Suspended(SuspendedRun {
                operation_id: operation_id.clone(),
                status: SuspendedStatus::Suspended,
                deferred: deferred.clone(),
            })),
            WaitingReason::Retry { .. } => Err(anyhow::Error::new(SessionInvariantError(format!(
                "{label} {operation_id} returned an unwaited retry"
            )))),
        },
    }
}

impl Lane {
    /// Upstream `new Lane(...)`: the event-watch bridge arrives separately
    /// through [`Lane::install_watch_handler`] (upstream it is an optional
    /// constructor argument supplied by the harness).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        session: Arc<StorageBackedSession>,
        models: Models,
        hooks: HookRegistry,
        state: LaneState,
        on_fault: FaultHandler,
        emit_batch: EmitBatch,
        read_config: ReadRuntimeConfig,
    ) -> Arc<Self> {
        let (state_change, state_change_self) = watch::channel(0u64);
        Arc::new_cyclic(|self_weak| Lane {
            name: name.to_owned(),
            session,
            models: Arc::new(models),
            hooks,
            shared: Arc::new(Mutex::new(LaneShared {
                state,
                closed_error: None,
                active_drive: None,
                idle_owner: None,
                watch_install: None,
            })),
            self_weak: self_weak.clone(),
            state_change,
            state_change_self,
            on_fault,
            emit_batch,
            read_config,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn session(&self) -> &StorageBackedSession {
        &self.session
    }

    /// A value snapshot of the authoritative owned state. Upstream hands out
    /// the live immutable object; the port clones (state is only ever replaced).
    pub fn state(&self) -> LaneState {
        self.lock_shared().state.clone()
    }

    pub fn read_config(&self) -> RuntimeConfig {
        (self.read_config)()
    }

    fn lock_shared(&self) -> std::sync::MutexGuard<'_, LaneShared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The seal error, if any (the first one wins).
    /// Owned handle for `mutate` callbacks (see [`LaneLease`]).
    fn lease(&self) -> LaneLease {
        LaneLease {
            session: Arc::clone(&self.session),
            shared: Arc::clone(&self.shared),
            state_change: self.state_change.clone(),
            on_fault: Arc::clone(&self.on_fault),
            emit_batch: Arc::clone(&self.emit_batch),
        }
    }

    pub fn sealed_error(&self) -> Option<Arc<LaneSealed>> {
        self.lock_shared().closed_error.clone()
    }

    pub fn assert_open(&self) -> anyhow::Result<()> {
        match &self.lock_shared().closed_error {
            None => Ok(()),
            Some(sealed) => Err(anyhow::Error::new(sealed.clone())),
        }
    }

    /// The fault boundary inside the line: a seal converts any failure to the
    /// sealed error, otherwise the fault handler maps the cause. The current
    /// command/read paths fault through [`LaneLease::fault`]; this `&self`
    /// form serves the detached drive pass.
    fn fault(&self, error: anyhow::Error) -> anyhow::Error {
        if let Some(sealed) = self.sealed_error() {
            return anyhow::Error::new(sealed);
        }
        (self.on_fault)(error)
    }

    fn signal_state_change(&self) {
        self.state_change.send_modify(|generation| *generation += 1);
    }

    /// `true` when a state change was signalled since the receiver last
    /// observed one — the upstream already-resolved `stateChange` promise.
    async fn wait_state_change(state_change: &mut watch::Receiver<u64>) {
        match state_change.has_changed() {
            Ok(true) => {
                let _ = state_change.borrow_and_update();
            }
            Ok(false) => {
                let _ = state_change.changed().await;
            }
            // Sender dropped with the lane; wake like a resolved promise.
            Err(_) => {}
        }
    }

    /// `Promise.race([owner, change])` under `awaitWithContext`.
    async fn wait_idle_or_change(
        owner: &CancellationToken,
        state_change: &mut watch::Receiver<u64>,
        context: &Context,
    ) -> anyhow::Result<()> {
        let abort = context.abort_signal();
        tokio::select! {
            _ = owner.cancelled() => Ok(()),
            _ = Self::wait_state_change(state_change) => Ok(()),
            _ = async {
                match abort {
                    Some(abort) => abort.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            } => Err(anyhow::Error::new(AbortedError)),
        }
    }

    /// Wait while an idle-claim owner holds the lane (upstream the
    /// `while (this.idleOwner !== undefined)` preamble of `command`).
    async fn wait_for_idle_line(
        &self,
        state_change: &mut watch::Receiver<u64>,
        context: &Context,
    ) -> anyhow::Result<()> {
        loop {
            // Read the seal/idle state in one critical section; the upstream
            // checks `closedError` and `idleOwner` as separate field reads.
            let (sealed, owner) = {
                let shared = self.lock_shared();
                (shared.closed_error.clone(), shared.idle_owner.clone())
            };
            if let Some(sealed) = sealed {
                return Err(anyhow::Error::new(sealed));
            }
            let Some(owner) = owner else { return Ok(()) };
            Self::wait_idle_or_change(&owner, state_change, context).await?;
            self.assert_open()?;
        }
    }

    /// Run one effect-free command on this lane's serialized mutation line.
    /// The planner receives the current owned state plus the mutator for
    /// bounded payload lookups; state-independent validation belongs before
    /// `command`, every state-dependent decision inside the planner.
    ///
    /// Exactly one outcome: `Commit` commits once, publishes `next`, then
    /// synchronously materializes the caller result (events are delivered
    /// after the line is released); `Return` returns without a commit;
    /// `Reject` fails outside the mutation/fault boundary as an expected
    /// caller error. Planner, commit, and event-build failures fault the lane
    /// before the line is released. Close/fault gates are checked before
    /// queueing and when the callback starts: close-first rejects, while a
    /// callback admitted before the seal may finish its commit, publish
    /// memory, and resolve without another open check.
    pub async fn command<T, F>(&self, plan: F, context: Context) -> anyhow::Result<T>
    where
        for<'a> F: Fn(
            &'a LaneState,
            &'a dyn SessionMutator,
        ) -> BoxFuture<'a, anyhow::Result<LaneCommand<T>>>,
        F: Send + Sync + 'static,
        T: Send + 'static,
    {
        let plan = Arc::new(plan);
        let mut state_change = self.state_change.subscribe();
        loop {
            self.wait_for_idle_line(&mut state_change, &context).await?;
            match self.run_command_once(Arc::clone(&plan), &context).await? {
                CommandOutcome::IdleBlocked { owner } => {
                    Self::wait_idle_or_change(&owner, &mut state_change, &context).await?;
                    self.assert_open()?;
                }
                CommandOutcome::Reject { error } => return Err(error),
                CommandOutcome::Return { result, delivery } => {
                    if let Some(delivery) = delivery {
                        delivery.await?;
                    }
                    return Ok(result);
                }
            }
        }
    }

    async fn run_command_once<T, F>(
        &self,
        plan: Arc<F>,
        context: &Context,
    ) -> anyhow::Result<CommandOutcome<T>>
    where
        for<'a> F: Fn(
            &'a LaneState,
            &'a dyn SessionMutator,
        ) -> BoxFuture<'a, anyhow::Result<LaneCommand<T>>>,
        F: Send + Sync + 'static,
        T: Send + 'static,
    {
        let lease = self.lease();
        let session = Arc::clone(&lease.session);
        let outcome = session
            .mutate(
                move |mutator, context| {
                    Box::pin(async move {
                        // Callback-start gates: close-first rejects; an idle
                        // owner re-checked on the line blocks this attempt.
                        lease.assert_open()?;
                        if let Some(owner) = lease.lock_shared().idle_owner.clone() {
                            return Ok(CommandOutcome::IdleBlocked { owner });
                        }
                        let state = lease.state();
                        let decision = match plan(&state, mutator).await {
                            Ok(decision) => decision,
                            Err(error) => return Err(lease.fault(error)),
                        };
                        match decision {
                            LaneCommand::Return { result } => Ok(CommandOutcome::Return {
                                result,
                                delivery: None,
                            }),
                            LaneCommand::Reject { error } => Ok(CommandOutcome::Reject { error }),
                            LaneCommand::Commit {
                                writes,
                                next,
                                materialize,
                                events,
                            } => {
                                let commit = match mutator.commit(writes, context.clone()).await {
                                    Ok(commit) => commit,
                                    Err(error) => return Err(lease.fault(error)),
                                };
                                // Publish memory, then materialize synchronously.
                                lease.lock_shared().state = next;
                                lease.signal_state_change();
                                let result = materialize(&commit);
                                let events = match events {
                                    None => Vec::new(),
                                    Some(build) => match build(&commit) {
                                        Ok(events) => events,
                                        Err(error) => return Err(lease.fault(error)),
                                    },
                                };
                                let delivery = if events.is_empty() {
                                    None
                                } else {
                                    Some((lease.emit_batch)(events, context.clone()))
                                };
                                Ok(CommandOutcome::Return { result, delivery })
                            }
                        }
                    })
                },
                context.clone(),
            )
            .await;
        match outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) => match self.sealed_error() {
                Some(sealed) => Err(anyhow::Error::new(sealed)),
                None => Err(error),
            },
        }
    }

    /// Run one effect-free read on the serialized mutation line (upstream
    /// `readLane`): no commit, no idle-owner wait, same fault boundary.
    pub async fn read_lane<T, F>(&self, read: F, context: Context) -> anyhow::Result<T>
    where
        for<'a> F: Fn(&'a LaneState, &'a dyn SessionMutator) -> BoxFuture<'a, anyhow::Result<T>>,
        F: Send + Sync + 'static,
        T: Send + 'static,
    {
        self.assert_open()?;
        let lease = self.lease();
        let session = Arc::clone(&lease.session);
        let outcome = session
            .mutate(
                move |mutator, _context| {
                    Box::pin(async move {
                        lease.assert_open()?;
                        let state = lease.state();
                        match read(&state, mutator).await {
                            Ok(value) => Ok(value),
                            Err(error) => Err(lease.fault(error)),
                        }
                    })
                },
                context.clone(),
            )
            .await;
        match outcome {
            Ok(value) => Ok(value),
            Err(error) => match self.sealed_error() {
                Some(sealed) => Err(anyhow::Error::new(sealed)),
                None => Err(error),
            },
        }
    }

    /// Upstream `lane.models` (`agent-harness.ts`): the shared model catalog.
    pub fn models(&self) -> &crate::ai::models::Models {
        &self.models
    }

    /// Upstream `lane.emitBatch(events, context)`: publish one harness-event
    /// batch through the injected emit facility.
    pub async fn emit_batch(
        &self,
        events: Vec<crate::agent_core::harness::runtime::events::HarnessEvent>,
        context: Context,
    ) -> anyhow::Result<()> {
        (self.emit_batch)(events, context).await
    }

    /// Run a command against the current operation even after cancellation is
    /// requested: settle admitted effects, finish the operation, or update
    /// concurrent child state. The Drive continuation remains the sole
    /// top-level state writer.
    /// Upstream `this.hooks` (`agent-harness.ts:609`): the shared hook
    /// registry the drive procedures and public adapter read.
    pub fn hooks(&self) -> &HookRegistry {
        &self.hooks
    }

    pub async fn settle_operation<T, F>(&self, plan: F, context: Context) -> anyhow::Result<T>
    where
        for<'a> F: Fn(
            &'a LaneState,
            &'a OperationState,
            &'a OperationMeta,
            &'a dyn SessionMutator,
        ) -> BoxFuture<'a, anyhow::Result<OperationCommand<T>>>,
        F: Send + Sync + 'static,
        T: Send + 'static,
    {
        let name = self.name.clone();
        let plan = Arc::new(plan);
        self.command(
            move |state, reader| {
                let name = name.clone();
                let plan = Arc::clone(&plan);
                Box::pin(async move {
                    let Some(operation) = &state.operation else {
                        return Err(invariant(format!(
                            "Lane {} has no operation to settle",
                            serde_json::to_string(&name).unwrap_or_default()
                        )));
                    };
                    let decision = plan(state, &operation.state, &operation.meta, reader).await?;
                    Ok(match decision {
                        OperationCommand::Return { result } => LaneCommand::Return { result },
                        OperationCommand::Commit {
                            writes,
                            operation_state: next_operation_state,
                            lane,
                            materialize,
                            events,
                        } => {
                            let mut writes = writes;
                            writes.push(set_value(
                                &operation_state(&operation.meta.operation_id),
                                serde_json::to_value(&next_operation_state)?,
                            ));
                            if let Some(inbox) = lane.as_ref().and_then(|lane| lane.inbox.as_ref())
                            {
                                writes.push(set_value(
                                    &lane_state(&name),
                                    serde_json::to_value(durable_lane_state(
                                        state,
                                        Some(&operation.meta.operation_id),
                                        Some(inbox),
                                        None,
                                    ))?,
                                ));
                            }
                            let mut next = lane_next(state, lane.as_ref());
                            next.operation = Some(Operation {
                                meta: operation.meta.clone(),
                                state: next_operation_state,
                            });
                            LaneCommand::Commit {
                                writes,
                                next,
                                materialize,
                                events,
                            }
                        }
                        OperationCommand::Finish {
                            writes,
                            record,
                            lane,
                            materialize,
                            events,
                        } => {
                            let inbox = lane
                                .as_ref()
                                .and_then(|lane| lane.inbox.clone())
                                .unwrap_or_else(|| state.inbox.clone());
                            let mut writes = writes;
                            writes.push(set_value(
                                &operation_result(&operation.meta.operation_id),
                                serde_json::to_value(&record)?,
                            ));
                            writes.push(set_value(
                                &lane_state(&name),
                                serde_json::to_value(durable_lane_state(
                                    state,
                                    None,
                                    Some(&inbox),
                                    Some(&operation.meta.operation_id),
                                ))?,
                            ));
                            let mut next = lane_next(state, lane.as_ref());
                            next.inbox = inbox;
                            next.last_operation_id = Some(operation.meta.operation_id.clone());
                            next.operation = None;
                            LaneCommand::Commit {
                                writes,
                                next,
                                materialize,
                                events,
                            }
                        }
                    })
                })
            },
            context,
        )
        .await
    }

    /// Run an ordinary operation command only while durable control is
    /// running: returns [`ContinueOperationResult::CancelRequested`] without
    /// invoking the planner once cancellation is requested.
    pub async fn continue_operation<T, F>(
        &self,
        plan: F,
        context: Context,
    ) -> anyhow::Result<ContinueOperationResult<T>>
    where
        for<'a> F: Fn(
            &'a LaneState,
            &'a OperationState,
            &'a OperationMeta,
            &'a dyn SessionMutator,
        ) -> BoxFuture<'a, anyhow::Result<OperationCommand<T>>>,
        F: Send + Sync + 'static,
        T: Send + 'static,
    {
        let plan = Arc::new(plan);
        self.settle_operation(
            move |state, latest, meta, reader| {
                let plan = Arc::clone(&plan);
                Box::pin(async move {
                    if matches!(latest.scope.control, Control::CancelRequested { .. }) {
                        return Ok(OperationCommand::Return {
                            result: ContinueOperationResult::CancelRequested,
                        });
                    }
                    let decision = plan(state, latest, meta, reader).await?;
                    Ok(match decision {
                        OperationCommand::Return { result } => OperationCommand::Return {
                            result: ContinueOperationResult::Result { value: result },
                        },
                        OperationCommand::Commit {
                            writes,
                            operation_state,
                            lane,
                            materialize,
                            events,
                        } => OperationCommand::Commit {
                            writes,
                            operation_state,
                            lane,
                            materialize: Box::new(move |commit: &CommitResult| {
                                ContinueOperationResult::Result {
                                    value: materialize(commit),
                                }
                            }),
                            events,
                        },
                        OperationCommand::Finish {
                            writes,
                            record,
                            lane,
                            materialize,
                            events,
                        } => OperationCommand::Finish {
                            writes,
                            record,
                            lane,
                            materialize: Box::new(move |commit: &CommitResult| {
                                ContinueOperationResult::Result {
                                    value: materialize(commit),
                                }
                            }),
                            events,
                        },
                    })
                })
            },
            context,
        )
        .await
    }

    /// Upstream `accept`: the expected rejections are the inner `Err`
    /// (upstream `Result.err` carrying the tagged error), faults/invariants
    /// the outer. The operation id is generated here.
    pub async fn accept(
        &self,
        request: &OperationRequest,
        context: Context,
    ) -> anyhow::Result<Result<OperationAdmission, TaggedError>> {
        self.accept_with_id(request, None, context).await
    }

    /// Upstream `accept` with the caller-supplied `request.operationId`.
    pub async fn accept_with_id(
        &self,
        request: &OperationRequest,
        requested_id: Option<String>,
        context: Context,
    ) -> anyhow::Result<Result<OperationAdmission, TaggedError>> {
        if let Some(sealed) = self.sealed_error() {
            return if sealed.kind == SealKind::Closed {
                Ok(Err(TaggedError::Closed {
                    message: sealed.message.clone(),
                }))
            } else {
                Err(anyhow::Error::new(sealed))
            };
        }
        self.assert_open()?;
        let started_at = crate::ai::now_ms();
        let operation_id =
            requested_id.unwrap_or_else(|| self.session.id_generator().next(Some(started_at)));
        let acceptance_config = self.read_config();
        match request {
            OperationRequest::Prompt { prompt } => {
                let messages = prompt_text_messages(prompt, &[], started_at);
                self.accept_run_messages(
                    messages,
                    operation_id,
                    started_at,
                    acceptance_config,
                    context,
                )
                .await
            }
            OperationRequest::PromptWithImages { prompt, images } => {
                let messages = prompt_text_messages(prompt, images, started_at);
                self.accept_run_messages(
                    messages,
                    operation_id,
                    started_at,
                    acceptance_config,
                    context,
                )
                .await
            }
            OperationRequest::PromptMessages { messages } => {
                let messages = messages.clone();
                self.accept_run_messages(
                    messages,
                    operation_id,
                    started_at,
                    acceptance_config,
                    context,
                )
                .await
            }
            OperationRequest::Skill {
                name,
                additional_instructions,
            } => {
                let skill = acceptance_config
                    .resources
                    .skills
                    .as_ref()
                    .and_then(|skills| skills.iter().find(|candidate| candidate.name == *name));
                let Some(skill) = skill else {
                    return Ok(Err(TaggedError::UnknownSkill {
                        name: name.clone(),
                        message: format!("Unknown skill: {name}"),
                    }));
                };
                let text = crate::agent_core::harness::skills::format_skill_invocation(
                    skill,
                    additional_instructions.as_deref(),
                );
                let messages = prompt_text_messages(&text, &[], started_at);
                self.accept_run_messages(
                    messages,
                    operation_id,
                    started_at,
                    acceptance_config,
                    context,
                )
                .await
            }
            OperationRequest::PromptTemplate { name, args } => {
                let template = acceptance_config
                    .resources
                    .prompt_templates
                    .as_ref()
                    .and_then(|templates| {
                        templates.iter().find(|candidate| candidate.name == *name)
                    });
                let Some(template) = template else {
                    return Ok(Err(TaggedError::UnknownTemplate {
                        name: name.clone(),
                        message: format!("Unknown prompt template: {name}"),
                    }));
                };
                let content =
                    crate::agent_core::harness::prompt_templates::format_prompt_template_invocation(
                        template,
                        args.as_deref().unwrap_or(&[]),
                    );
                let messages = prompt_text_messages(&content, &[], started_at);
                self.accept_run_messages(
                    messages,
                    operation_id,
                    started_at,
                    acceptance_config,
                    context,
                )
                .await
            }
            OperationRequest::Compaction {
                custom_instructions,
            } => {
                self.accept_compaction(
                    custom_instructions.clone(),
                    operation_id,
                    started_at,
                    &acceptance_config,
                    context,
                )
                .await
            }
            OperationRequest::Navigation { target_id, options } => {
                self.accept_navigation(
                    target_id.clone(),
                    options.clone(),
                    operation_id,
                    started_at,
                    acceptance_config,
                    context,
                )
                .await
            }
        }
    }

    /// The upstream `acceptRun` preamble (lane.ts:505-572): build the prompt
    /// message list from the request kind, reject pending assistants, and
    /// assign prompt entry ids.
    async fn accept_run_messages(
        &self,
        messages: Vec<AgentMessage>,
        operation_id: String,
        started_at: i64,
        acceptance_config: RuntimeConfig,
        context: Context,
    ) -> anyhow::Result<Result<OperationAdmission, TaggedError>> {
        for message in &messages {
            if let AgentMessage::Assistant(assistant) = message {
                if assistant.stop_reason == StopReason::Pending {
                    return Ok(Err(TaggedError::InvalidMessage {
                        lane: self.name.clone(),
                        reason: "pending_assistant".to_owned(),
                        message: "Cannot accept a pending assistant message".to_owned(),
                    }));
                }
            }
        }
        let prompt_entries: Vec<(String, AgentMessage)> = messages
            .into_iter()
            .map(|message| (self.session.id_generator().next(Some(started_at)), message))
            .collect();
        self.accept_run(
            prompt_entries,
            operation_id,
            started_at,
            acceptance_config,
            context,
        )
        .await
        .map(|inner| inner.map_err(acceptance_error_to_tagged))
    }

    async fn accept_run(
        &self,
        prompt: Vec<(String, AgentMessage)>,
        operation_id: String,
        started_at: i64,
        acceptance_config: RuntimeConfig,
        context: Context,
    ) -> anyhow::Result<Result<OperationAdmission, AcceptanceError>> {
        let name = self.name.clone();
        let planner_context = context.clone();
        let admission = self
            .command(
                move |state, reader| {
                    let name = name.clone();
                    let acceptance_config = acceptance_config.clone();
                    let prompt = prompt.clone();
                    let operation_id = operation_id.clone();
                    let context = planner_context.clone();
                    Box::pin(async move {
                        if let Some(active) = &state.operation {
                            return Ok(LaneCommand::Return {
                                result: Err(AcceptanceError::LaneBusy {
                                    lane: name.clone(),
                                    operation_id: active.meta.operation_id.clone(),
                                    operation_kind: active.meta.intent.kind(),
                                }),
                            });
                        }
                        let (selected, inbox) = select_accepted_inbox(
                            &state.inbox,
                            &acceptance_config.steering_mode,
                            &acceptance_config.follow_up_mode,
                        );
                        let mut captured = Vec::with_capacity(selected.len());
                        for item in &selected {
                            let stored = reader
                                .get_value(&pending_entry(&item.entry_id), context.clone())
                                .await?
                                .ok_or_else(|| {
                                    invariant(format!(
                                        "Pending {} entry {} is missing its payload",
                                        inbox_kind_name(item.kind),
                                        item.entry_id
                                    ))
                                })?;
                            let pending: PendingEntry = serde_json::from_value(stored.value)?;
                            if item.kind != InboxItemKind::Write {
                                if !matches!(pending, PendingEntry::Message { .. }) {
                                    return Err(invariant(format!(
                                        "Pending {} entry {} is not a message",
                                        inbox_kind_name(item.kind),
                                        item.entry_id
                                    )));
                                }
                                if let PendingEntry::Message {
                                    payload: AgentMessage::Assistant(assistant),
                                } = &pending
                                {
                                    if assistant.stop_reason == StopReason::Pending {
                                        return Err(invariant(format!(
                                            "Pending {} entry {} contains a pending assistant",
                                            inbox_kind_name(item.kind),
                                            item.entry_id
                                        )));
                                    }
                                }
                            }
                            captured.push((item.clone(), pending));
                        }
                        let has_captured_conversation = selected
                            .iter()
                            .any(|item| item.kind != InboxItemKind::Write);
                        if prompt.is_empty() && !has_captured_conversation {
                            return Ok(LaneCommand::Return {
                                result: Err(AcceptanceError::InvalidMessage {
                                    lane: name.clone(),
                                    reason: "empty",
                                    message: "Acceptance must append at least one message".into(),
                                }),
                            });
                        }

                        let mut staged: Vec<NewEntry> = captured
                            .iter()
                            .map(|(item, pending)| pending_entry_write(&item.entry_id, pending))
                            .collect();
                        staged.extend(prompt.iter().map(|(id, message)| NewEntry::Message {
                            id: id.clone(),
                            parent_id: None,
                            message: message.clone(),
                            terminate: None,
                        }));
                        let entries = chain_entries(state.tip_id.as_deref(), &staged);
                        let parent_id = entries.last().expect("chained entries").id().to_owned();
                        let meta = OperationMeta {
                            operation_id: operation_id.clone(),
                            lane: name.clone(),
                            source_tip_id: state.tip_id.clone(),
                            started_at,
                            intent: OperationIntent::Run {
                                prompt_entry_ids: prompt.iter().map(|(id, _)| id.clone()).collect(),
                            },
                        };
                        let starting_operation_state = OperationState {
                            scope: OperationScope {
                                control: Control::Running,
                                settings: captured_settings(&acceptance_config),
                                latest_assistant_entry_id: None,
                            },
                            phase: OperationPhase::Starting,
                        };
                        let remaining_queues =
                            read_lane_queues(reader, &inbox, context.clone()).await?;
                        let mut next = state.clone();
                        next.tip_id = Some(parent_id);
                        next.inbox = inbox.clone();
                        next.operation = Some(Operation {
                            meta: meta.clone(),
                            state: starting_operation_state.clone(),
                        });
                        let mut writes: Vec<Write> =
                            entries.iter().cloned().map(insert_entry).collect();
                        writes.extend(
                            selected
                                .iter()
                                .map(|item| delete_value(&pending_entry(&item.entry_id))),
                        );
                        writes.push(set_value(
                            &crate::agent_core::harness::session::branch_tip(&name),
                            serde_json::Value::String(next.tip_id.clone().expect("tip set above")),
                        ));
                        writes.push(set_value(
                            &operation_meta(&operation_id),
                            serde_json::to_value(&meta)?,
                        ));
                        writes.push(set_value(
                            &operation_state(&operation_id),
                            serde_json::to_value(&starting_operation_state)?,
                        ));
                        writes.push(set_value(
                            &lane_state(&name),
                            serde_json::to_value(durable_lane_state(
                                state,
                                Some(&operation_id),
                                Some(&inbox),
                                None,
                            ))?,
                        ));
                        let selected_count = selected.len();
                        Ok(LaneCommand::Commit {
                            writes,
                            next,
                            materialize: Box::new(move |_| {
                                Ok(OperationAdmission {
                                    operation_id,
                                    kind: "run",
                                    started_at,
                                })
                            }),
                            events: Some(Box::new(move |commit: &CommitResult| {
                                let mut events = vec![HarnessEvent::RunStart {
                                    run_id: meta.operation_id.clone(),
                                    started_at,
                                    lane: name.clone(),
                                }];
                                events.extend(
                                    committed_entry_events(
                                        &entries,
                                        commit,
                                        &name,
                                        Some(&meta.operation_id),
                                        0,
                                    )?
                                    .into_iter()
                                    .map(HarnessEvent::from),
                                );
                                if selected_count > 0 {
                                    events.push(HarnessEvent::QueueUpdate {
                                        queues: remaining_queues,
                                        lane: name.clone(),
                                    });
                                }
                                Ok(events)
                            })),
                        })
                    })
                },
                context,
            )
            .await?;
        Ok(admission)
    }

    /// Upstream `acceptCompaction` (lane.ts:679-760).
    async fn accept_compaction(
        &self,
        custom_instructions: Option<String>,
        operation_id: String,
        started_at: i64,
        acceptance_config: &RuntimeConfig,
        context: Context,
    ) -> anyhow::Result<Result<OperationAdmission, TaggedError>> {
        let task_id = self.session.id_generator().next(Some(started_at));
        let name = self.name.clone();
        let acceptance_config = acceptance_config.clone();
        let planner_context = context.clone();
        let admission = self
            .command(
                move |state, reader| {
                    let name = name.clone();
                    let acceptance_config = acceptance_config.clone();
                    let custom_instructions = custom_instructions.clone();
                    let operation_id = operation_id.clone();
                    let task_id = task_id.clone();
                    let context = planner_context.clone();
                    Box::pin(async move {
                        if let Some(active) = &state.operation {
                            return Ok(LaneCommand::Return {
                                result: Err(acceptance_error_to_tagged(
                                    AcceptanceError::LaneBusy {
                                        lane: name.clone(),
                                        operation_id: active.meta.operation_id.clone(),
                                        operation_kind: active.meta.intent.kind(),
                                    },
                                )),
                            });
                        }
                        let path = match &state.tip_id {
                            None => Vec::new(),
                            Some(tip) => {
                                let mut entries = reader
                                    .scan_branch(
                                        &StorageBranchScan {
                                            start: tip.clone(),
                                            stop_at_type: Some(EntryType::Compaction),
                                            order: Some(BranchScanOrder::NewestFirst),
                                            ..Default::default()
                                        },
                                        context.clone(),
                                    )
                                    .await?;
                                entries.reverse();
                                entries
                            }
                        };
                        let prepared = prepare_compaction(&path, acceptance_config.compaction)
                            .map_err(anyhow::Error::new)?;
                        let Some(preparation) = prepared else {
                            return Ok(LaneCommand::Return {
                                result: Err(TaggedError::NothingToCompact {
                                    lane: name.clone(),
                                    message: format!(
                                        "Lane {} has nothing to compact",
                                        serde_json::to_string(&name).unwrap_or_default()
                                    ),
                                }),
                            });
                        };
                        let meta = OperationMeta {
                            operation_id: operation_id.clone(),
                            lane: name.clone(),
                            source_tip_id: state.tip_id.clone(),
                            started_at,
                            intent: OperationIntent::Compaction {
                                custom_instructions: custom_instructions.clone(),
                            },
                        };
                        let starting_operation_state = OperationState {
                            scope: OperationScope {
                                control: Control::Running,
                                settings: captured_settings(&acceptance_config),
                                latest_assistant_entry_id: None,
                            },
                            phase: OperationPhase::SummaryDeciding {
                                task: SummaryTask {
                                    task_id: task_id.clone(),
                                    reason: Some(SummaryReason::Manual),
                                    custom_instructions: custom_instructions.clone(),
                                    boundary: ResultBoundary::Finish,
                                },
                            },
                        };
                        let next = {
                            let mut next = state.clone();
                            next.operation = Some(Operation {
                                meta: meta.clone(),
                                state: starting_operation_state.clone(),
                            });
                            next
                        };
                        Ok(LaneCommand::Commit {
                            writes: vec![
                                set_value(
                                    &operation_preparation(&operation_id, &task_id),
                                    serde_json::to_value(durable_compaction_preparation(
                                        &preparation,
                                    ))?,
                                ),
                                set_value(
                                    &operation_meta(&operation_id),
                                    serde_json::to_value(&meta)?,
                                ),
                                set_value(
                                    &operation_state(&operation_id),
                                    serde_json::to_value(&starting_operation_state)?,
                                ),
                                set_value(
                                    &lane_state(&name),
                                    serde_json::to_value(durable_lane_state(
                                        state,
                                        Some(&operation_id),
                                        None,
                                        None,
                                    ))?,
                                ),
                            ],
                            next,
                            materialize: Box::new(move |_| {
                                Ok(OperationAdmission {
                                    operation_id,
                                    kind: "compaction",
                                    started_at,
                                })
                            }),
                            events: Some(Box::new(move |_commit: &CommitResult| {
                                Ok(vec![HarnessEvent::CompactionStart {
                                    lane: name.clone(),
                                    run_id: meta.operation_id.clone(),
                                    reason: SummaryReason::Manual,
                                    started_at,
                                }])
                            })),
                        })
                    })
                },
                context,
            )
            .await?;
        Ok(admission)
    }

    /// Upstream `acceptNavigation` (lane.ts:762-919): the prepare-then-claim
    /// loop. The preparation is observed outside the mutation line; the
    /// planner re-checks the observed tip before committing.
    #[allow(clippy::too_many_arguments)]
    async fn accept_navigation(
        &self,
        target_id: Option<String>,
        options: Option<NavigationOptions>,
        operation_id: String,
        started_at: i64,
        acceptance_config: RuntimeConfig,
        context: Context,
    ) -> anyhow::Result<Result<OperationAdmission, TaggedError>> {
        let task_id = self.session.id_generator().next(Some(started_at));
        let options = options.unwrap_or_default();
        let summarize = options.summarize.unwrap_or(false);
        loop {
            // Fresh per-iteration clones: the `move` planner consumes its own
            // copies while the outer bindings survive the retry loop.
            let acceptance_config = acceptance_config.clone();
            let options = options.clone();
            let target_id = target_id.clone();
            let operation_id = operation_id.clone();
            let task_id = task_id.clone();
            let planner_context = context.clone();
            let closure_context = planner_context.clone();
            let observed_tip_id = self.state().tip_id;
            let mut preparation: Option<BranchPreparation> = None;
            if summarize && observed_tip_id.is_some() && target_id.is_some() {
                let target_id_value = target_id.clone().expect("checked above");
                let target = self
                    .session
                    .get_entries(
                        std::slice::from_ref(&target_id_value),
                        planner_context.clone(),
                    )
                    .await?;
                if target.contains_key(&target_id_value) {
                    let (old_path, target_path) = tokio::join!(
                        self.session.scan_branch(
                            &StorageBranchScan {
                                start: observed_tip_id.clone().expect("checked above"),
                                order: Some(BranchScanOrder::NewestFirst),
                                ..Default::default()
                            },
                            planner_context.clone(),
                        ),
                        self.session.scan_branch(
                            &StorageBranchScan {
                                start: target_id_value.clone(),
                                order: Some(BranchScanOrder::NewestFirst),
                                ..Default::default()
                            },
                            planner_context.clone(),
                        ),
                    );
                    let (old_path, target_path) = (old_path?, target_path?);
                    let old_ids: HashSet<&str> = old_path.iter().map(|entry| entry.id()).collect();
                    let common_ancestor_id = target_path
                        .iter()
                        .find(|entry| old_ids.contains(entry.id()))
                        .map(|entry| entry.id().to_owned());
                    let cut = match &common_ancestor_id {
                        None => old_path.len(),
                        Some(id) => old_path
                            .iter()
                            .position(|entry| entry.id() == id)
                            .unwrap_or(old_path.len()),
                    };
                    let mut selected = old_path[..cut].to_vec();
                    selected.reverse();
                    preparation = Some(prepare_branch_entries(&selected, 0));
                }
            }
            let name = self.name.clone();
            let accepted: Option<Result<OperationAdmission, TaggedError>> =
                self.command(
                    move |state, reader| {
                        let name = name.clone();
                        let acceptance_config = acceptance_config.clone();
                        let target_id = target_id.clone();
                        let options = options.clone();
                        let preparation = preparation.clone();
                        let operation_id = operation_id.clone();
                        let task_id = task_id.clone();
                        let observed_tip_id = observed_tip_id.clone();
                        let context = closure_context.clone();
                        Box::pin(async move {
                            if let Some(active) = &state.operation {
                                return Ok(LaneCommand::Return {
                                    result: Some(Err(acceptance_error_to_tagged(
                                        AcceptanceError::LaneBusy {
                                            lane: name.clone(),
                                            operation_id: active.meta.operation_id.clone(),
                                            operation_kind: active.meta.intent.kind(),
                                        },
                                    ))),
                                });
                            }
                            if state.tip_id != observed_tip_id {
                                return Ok(LaneCommand::Return { result: None });
                            }
                            if target_id == state.tip_id {
                                return Ok(LaneCommand::Return {
                                    result: Some(Err(TaggedError::InvalidNavigation {
                                        lane: name.clone(),
                                        reason: "current_tip".to_owned(),
                                        message:
                                            "Navigation target must differ from the current tip"
                                                .to_owned(),
                                    })),
                                });
                            }
                            if target_id.is_none() && options.label.is_some() {
                                return Ok(LaneCommand::Return {
                                    result: Some(Err(TaggedError::InvalidNavigation {
                                        lane: name.clone(),
                                        reason: "root_label".to_owned(),
                                        message: "Root navigation cannot set a label".to_owned(),
                                    })),
                                });
                            }
                            if summarize && (state.tip_id.is_none() || target_id.is_none()) {
                                return Ok(LaneCommand::Return {
                                    result: Some(Err(TaggedError::InvalidNavigation {
                                        lane: name.clone(),
                                        reason: if state.tip_id.is_none() {
                                            "source_root"
                                        } else {
                                            "target_root"
                                        }
                                        .to_owned(),
                                        message:
                                            "Summarized navigation requires non-root source and target entries"
                                                .to_owned(),
                                    })),
                                });
                            }
                            if let Some(target) = &target_id {
                                let found = reader
                                    .get_entries(std::slice::from_ref(target), context.clone())
                                    .await?
                                    .contains_key(target);
                                if !found {
                                    return Ok(LaneCommand::Return {
                                        result: Some(Err(TaggedError::UnknownTarget {
                                            target_id: target.clone(),
                                            message: format!("Unknown target: {target}"),
                                        })),
                                    });
                                }
                            }

                            let intent = OperationIntent::Navigation {
                                target_id: target_id.clone(),
                                summarize,
                                label: options.label.clone(),
                                custom_instructions: options.custom_instructions.clone(),
                            };
                            let meta = OperationMeta {
                                operation_id: operation_id.clone(),
                                lane: name.clone(),
                                source_tip_id: state.tip_id.clone(),
                                started_at,
                                intent,
                            };
                            let scope = OperationScope {
                                control: Control::Running,
                                settings: captured_settings(&acceptance_config),
                                latest_assistant_entry_id: None,
                            };
                            let mut writes: Vec<Write> = Vec::new();
                            let navigation_operation_state = if summarize {
                                let Some(preparation) = &preparation else {
                                    return Err(invariant(
                                        "Validated summarized navigation is missing its preparation"
                                            .to_owned(),
                                    ));
                                };
                                writes.push(set_value(
                                    &operation_preparation(&operation_id, &task_id),
                                    serde_json::to_value(durable_branch_preparation(preparation))?,
                                ));
                                OperationState {
                                    scope,
                                    phase: OperationPhase::SummaryDeciding {
                                        task: SummaryTask {
                                            task_id: task_id.clone(),
                                            reason: None,
                                            custom_instructions: options.custom_instructions.clone(),
                                            boundary: ResultBoundary::CommitNavigation {
                                                target_id: target_id.clone().expect("checked above"),
                                                label: options.label.clone(),
                                            },
                                        },
                                    },
                                }
                            } else {
                                OperationState {
                                    scope,
                                    phase: OperationPhase::NavigationReadyToCommit {
                                        target_id: target_id.clone(),
                                        label: options.label.clone(),
                                    },
                                }
                            };
                            writes.push(set_value(
                                &operation_meta(&operation_id),
                                serde_json::to_value(&meta)?,
                            ));
                            writes.push(set_value(
                                &operation_state(&operation_id),
                                serde_json::to_value(&navigation_operation_state)?,
                            ));
                            writes.push(set_value(
                                &lane_state(&name),
                                serde_json::to_value(durable_lane_state(
                                    state,
                                    Some(&operation_id),
                                    None,
                                    None,
                                ))?,
                            ));
                            let next = {
                                let mut next = state.clone();
                                next.operation = Some(Operation {
                                    meta: meta.clone(),
                                    state: navigation_operation_state.clone(),
                                });
                                next
                            };
                            Ok(LaneCommand::Commit {
                                writes,
                                next,
                                materialize: Box::new(move |_| {
                                    Some(Ok(OperationAdmission {
                                        operation_id,
                                        kind: "navigation",
                                        started_at,
                                    }))
                                }),
                                events: Some(Box::new(move |_commit: &CommitResult| {
                                    Ok(vec![HarnessEvent::NavigationStart {
                                        lane: name.clone(),
                                        run_id: meta.operation_id.clone(),
                                        target_id: target_id.clone(),
                                        started_at,
                                    }])
                                })),
                            })
                        })
                    },
                    planner_context,
                )
                .await?;
            match accepted {
                Some(Ok(admission)) => return Ok(Ok(admission)),
                Some(Err(tagged)) => return Ok(Err(tagged)),
                None => continue,
            }
        }
    }

    /// Current process-local model (upstream `getModel`).
    pub async fn get_model(
        &self,
        _context: Context,
    ) -> anyhow::Result<Option<crate::ai::types::Model>> {
        self.assert_open()?;
        let configuration = self.state().configuration;
        Ok(self
            .models
            .get_model(&configuration.model.provider, &configuration.model.model_id))
    }

    pub async fn get_thinking_level(&self, _context: Context) -> anyhow::Result<ThinkingLevel> {
        self.assert_open()?;
        Ok(self.state().configuration.thinking_level)
    }

    pub async fn get_active_tools(&self, _context: Context) -> anyhow::Result<Vec<String>> {
        self.assert_open()?;
        Ok(self.state().configuration.active_tool_names)
    }

    pub async fn set_model(&self, model: LaneModel, context: Context) -> anyhow::Result<()> {
        self.set_configuration(
            Arc::new(move |configuration: &LaneConfiguration| LaneConfiguration {
                model: model.clone(),
                ..configuration.clone()
            }),
            Arc::new(|previous: &LaneConfiguration, value: &LaneConfiguration| {
                ConfigUpdateProperty::Model {
                    value: value.model.clone(),
                    previous: serde_json::to_value(previous.model.clone())
                        .expect("model serializes"),
                }
            }),
            context,
        )
        .await
    }

    pub async fn set_thinking_level(
        &self,
        thinking_level: ThinkingLevel,
        context: Context,
    ) -> anyhow::Result<()> {
        self.set_configuration(
            Arc::new(move |configuration: &LaneConfiguration| LaneConfiguration {
                thinking_level,
                ..configuration.clone()
            }),
            Arc::new(|previous: &LaneConfiguration, value: &LaneConfiguration| {
                ConfigUpdateProperty::ThinkingLevel {
                    value: value.thinking_level,
                    previous: previous.thinking_level,
                }
            }),
            context,
        )
        .await
    }

    pub async fn set_active_tools(
        &self,
        active_tool_names: Vec<String>,
        context: Context,
    ) -> anyhow::Result<()> {
        self.set_configuration(
            Arc::new(move |configuration: &LaneConfiguration| LaneConfiguration {
                active_tool_names: active_tool_names.clone(),
                ..configuration.clone()
            }),
            Arc::new(|previous: &LaneConfiguration, value: &LaneConfiguration| {
                ConfigUpdateProperty::ActiveTools {
                    value: value.active_tool_names.clone(),
                    previous: previous.active_tool_names.clone(),
                }
            }),
            context,
        )
        .await
    }

    async fn set_configuration(
        &self,
        update: Arc<dyn Fn(&LaneConfiguration) -> LaneConfiguration + Send + Sync>,
        event: ConfigUpdateEventFn,
        context: Context,
    ) -> anyhow::Result<()> {
        let name = self.name.clone();
        self.command(
            move |state, _reader| {
                let name = name.clone();
                let update = Arc::clone(&update);
                let event = Arc::clone(&event);
                Box::pin(async move {
                    let configuration = update(&state.configuration);
                    let previous = state.configuration.clone();
                    let mut next = state.clone();
                    next.configuration = configuration.clone();
                    Ok(LaneCommand::Commit {
                        writes: vec![set_value(
                            &lane_config(&name),
                            serde_json::to_value(&configuration)?,
                        )],
                        next,
                        materialize: Box::new(|_commit: &CommitResult| ()),
                        events: Some(Box::new(move |_commit: &CommitResult| {
                            Ok(vec![HarnessEvent::ConfigUpdate {
                                lane: name.clone(),
                                property: event(&previous, &configuration),
                            }])
                        })),
                    })
                })
            },
            context,
        )
        .await
    }

    /// Upstream `seal`: record the first seal error, close the installed
    /// drive's gate, and wake waiters. Returns the idle owner's release token
    /// to await (the upstream returned `idleOwner` promise); `None` when no
    /// owner was installed (`Promise.resolve()`).
    pub fn seal(&self, kind: SealKind, message: impl Into<String>) -> Option<CancellationToken> {
        let owner = {
            let mut shared = self.lock_shared();
            if shared.closed_error.is_none() {
                shared.closed_error = Some(Arc::new(LaneSealed {
                    kind,
                    message: message.into(),
                }));
            }
            if let Some(drive) = shared.active_drive.clone() {
                let sealed = shared.closed_error.clone().expect("just installed");
                let drive_error: Arc<dyn std::error::Error + Send + Sync> = sealed;
                drive.close_gate(drive_error);
            }
            shared.idle_owner.clone()
        };
        self.signal_state_change();
        owner
    }
}

// ---------------------------------------------------------------------------
// Queue / append / drive-install / watch surface (upstream lane.ts, second
// half)
// ---------------------------------------------------------------------------

/// One `drive()` claim decision, read on the mutation line (upstream
/// `DriveClaim`).
enum DriveClaim {
    Observe { drive: Arc<Drive>, installed: bool },
    Occupied { drive: Arc<Drive> },
    Settled { outcome: OperationResultRecord },
    Mismatch { error: TaggedError },
}

/// One `waitForIdle` observation (upstream `IdleObservation`).
enum IdleObservation {
    Idle,
    Wait { drive: Option<Arc<Drive>> },
}

/// One `runWhenIdle` observation (upstream `IdleClaimObservation`).
enum IdleClaimObservation {
    Claimed { owner: CancellationToken },
    Wait { drive: Option<Arc<Drive>> },
}

/// Pass-local event/deferred-cancel callbacks. Tools are deliberately not
/// captured here: upstream reads live configuration at the tool phase.
struct InstalledDriveEnv {
    emit_tool_event: ToolEventEmit,
    cancel_deferred: Arc<DeferredCancelFn>,
}

impl DriveEnvironment for InstalledDriveEnv {
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
    ) -> BoxFuture<
        'a,
        anyhow::Result<crate::agent_core::harness::runtime::drive_pass::ProcedureResult>,
    > {
        Box::pin(run_tools_from_config(
            lane,
            drive,
            state,
            || lane.read_config().native_tools.into_parts(),
            &self.emit_tool_event,
        ))
    }
}

fn drive_env_for(lane: &Arc<Lane>) -> InstalledDriveEnv {
    let lane_for_events = Arc::clone(lane);
    let models = lane.models().clone();
    InstalledDriveEnv {
        emit_tool_event: Arc::new(move |events: Vec<ToolEvent>, context: Context| {
            let lane = Arc::clone(&lane_for_events);
            Box::pin(async move {
                let batch: Vec<crate::agent_core::harness::runtime::events::HarnessEvent> =
                    events.into_iter().map(HarnessEvent::from).collect();
                lane.emit_batch(batch, context).await
            })
        }),
        cancel_deferred: Arc::new(move |request| {
            let models = models.clone();
            Box::pin(async move {
                // Upstream forwards `signal: drive.closeSignal` plus the
                // deferred streamOptions limits (reconcile.ts:28-33); the
                // telemetry context rides the ported Context envelope.
                let options = crate::ai::models::ModelsDeferredCancelOptions {
                    stream: crate::ai::types::options::StreamOptions {
                        signal: Some(request.signal),
                        timeout_ms: request.timeout_ms,
                        max_retries: request.max_retries,
                        max_retry_delay_ms: request.max_retry_delay_ms,
                        // The cancel request carries plain header pairs; the
                        // StreamOptions wire allows `None` values to suppress
                        // provider defaults, so forwarded pairs stay set.
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

/// The upstream `settleGate` closure of `requestOperationAbort`: one-shot
/// release of the admission-gate cancellation, optionally followed by the
/// drive's `signalAbort`.
fn settle_abort_gate(
    gate: &std::sync::atomic::AtomicBool,
    cancellation: &CancellationToken,
    drive: &Option<Arc<Drive>>,
    signal: bool,
) {
    if gate.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    cancellation.cancel();
    if signal {
        if let Some(drive) = drive {
            drive.signal_abort();
        }
    }
}

/// The completion wait shared by `drive()`, `waitForIdle`, and
/// `runWhenIdle`: abort-aware, with the sealed error re-derived on the
/// caller side so its identity survives (a `DriveError` carries no
/// downcastable payload across the completion channel).
async fn wait_drive_outcome(
    lane: &Lane,
    completion: &crate::agent_core::harness::runtime::drive_pass::DriveCompletion,
    context: &Context,
) -> anyhow::Result<DriveOutcome> {
    let completion = completion.clone();
    let settled =
        await_with_context(async move { completion.wait().await }, context.clone()).await?;
    match settled {
        Ok(outcome) => Ok((*outcome).clone()),
        Err(failure) => match lane.sealed_error() {
            Some(sealed) => Err(anyhow::Error::new(sealed)),
            None => Err(anyhow::Error::msg(failure.to_string())),
        },
    }
}

impl Lane {
    /// The upstream `closedError instanceof HarnessClosed` early return.
    fn closed_result(&self) -> Option<TaggedError> {
        let sealed = self.sealed_error()?;
        if sealed.kind == SealKind::Closed {
            Some(TaggedError::Closed {
                message: sealed.message.clone(),
            })
        } else {
            None
        }
    }

    /// The installed drive pass, if any (upstream the public `activeDrive`
    /// field; deterministic procedure tests install exact owners directly).
    pub fn active_drive(&self) -> Option<Arc<Drive>> {
        self.lock_shared().active_drive.clone()
    }

    /// Clear the installed pass only when it is still the given one
    /// (upstream `if (this.activeDrive === claim.drive)`).
    fn clear_active_drive_if(&self, drive: &Arc<Drive>) -> bool {
        let mut shared = self.lock_shared();
        let matches = shared
            .active_drive
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, drive));
        if matches {
            shared.active_drive = None;
        }
        matches
    }

    /// Install the harness-provided event-bus bridge (upstream the
    /// `installWatch` constructor parameter).
    pub fn install_watch_handler(&self, installer: LaneWatchInstaller) {
        self.lock_shared().watch_install = Some(installer);
    }

    /// Upstream `drive` (lane.ts:921-1003): claim or join the installed
    /// pass for one operation, returning its settled outcome or durable wait.
    pub async fn drive(
        &self,
        options: &DriveOptions,
        context: Context,
    ) -> anyhow::Result<DriveResult> {
        if let Some(closed) = self.closed_result() {
            return Ok(Err(closed));
        }
        self.assert_open()?;

        loop {
            let lease = self.lease();
            let name = self.name.clone();
            let options = options.clone();
            let planner_context = context.clone();
            let claim: DriveClaim = self
                .command(
                    move |state, reader| {
                        let lease = lease.clone();
                        let name = name.clone();
                        let options = options.clone();
                        let context = planner_context.clone();
                        Box::pin(async move {
                            if let Some(signal) = context.abort_signal() {
                                if signal.is_cancelled() {
                                    return Ok(LaneCommand::Reject {
                                        error: anyhow::Error::new(AbortedError),
                                    });
                                }
                            }
                            if state
                                .operation
                                .as_ref()
                                .map(|operation| operation.meta.operation_id.as_str())
                                == Some(options.operation_id.as_str())
                            {
                                let existing = lease.lock_shared().active_drive.clone();
                                return Ok(match existing {
                                    None => {
                                        let drive = Arc::new(Drive::new(&options, context.clone()));
                                        lease.lock_shared().active_drive = Some(Arc::clone(&drive));
                                        lease.signal_state_change();
                                        LaneCommand::Return {
                                            result: DriveClaim::Observe {
                                                drive,
                                                installed: true,
                                            },
                                        }
                                    }
                                    Some(active) => {
                                        if active.operation_id() == options.operation_id {
                                            LaneCommand::Return {
                                                result: DriveClaim::Observe {
                                                    drive: active,
                                                    installed: false,
                                                },
                                            }
                                        } else {
                                            LaneCommand::Return {
                                                result: DriveClaim::Occupied { drive: active },
                                            }
                                        }
                                    }
                                });
                            }

                            let current = state
                                .operation
                                .as_ref()
                                .map(|operation| operation.meta.operation_id.as_str());
                            let stored = reader
                                .get_value(
                                    &operation_result(&options.operation_id),
                                    context.clone(),
                                )
                                .await?;
                            Ok(match stored {
                                Some(stored) => LaneCommand::Return {
                                    result: DriveClaim::Settled {
                                        outcome: serde_json::from_value(stored.value)?,
                                    },
                                },
                                None => LaneCommand::Return {
                                    result: DriveClaim::Mismatch {
                                        error: mismatch_error(
                                            &name,
                                            &options.operation_id,
                                            current,
                                            state.last_operation_id.as_deref(),
                                        ),
                                    },
                                },
                            })
                        })
                    },
                    context.clone(),
                )
                .await?;

            match claim {
                DriveClaim::Settled { outcome } => {
                    return Ok(Ok(DriveOutcome::Settled { outcome }));
                }
                DriveClaim::Mismatch { error } => return Ok(Err(error)),
                DriveClaim::Occupied { drive } => {
                    let completion = drive.completion();
                    wait_drive_outcome(self, &completion, &context).await?;
                    continue;
                }
                DriveClaim::Observe { drive, installed } => {
                    if installed {
                        self.spawn_pass(Arc::clone(&drive));
                    }
                    let completion = drive.completion();
                    let outcome = wait_drive_outcome(self, &completion, &context).await?;
                    return Ok(Ok(outcome));
                }
            }
        }
    }

    /// Detach the dispatcher for one installed pass (upstream the
    /// `void driveOperation(this, claim.drive).then(...)` continuation):
    /// unset the claim, settle or fail the completion handle.
    fn spawn_pass(&self, drive: Arc<Drive>) {
        let Some(lane) = self.self_weak.upgrade() else {
            return;
        };
        tokio::spawn(async move {
            let env = drive_env_for(&lane);
            match drive_operation_with_env(&lane, &drive, &env).await {
                Ok(outcome) => {
                    if lane.clear_active_drive_if(&drive) {
                        lane.signal_state_change();
                    }
                    drive.settle(outcome);
                }
                Err(error) => {
                    let failure: crate::agent_core::harness::runtime::drive_pass::DriveError =
                        match lane.sealed_error() {
                            Some(sealed) => sealed,
                            None => {
                                let boxed: Box<dyn std::error::Error + Send + Sync> =
                                    lane.fault(error).into();
                                Arc::from(boxed)
                            }
                        };
                    if lane.clear_active_drive_if(&drive) {
                        lane.signal_state_change();
                    }
                    drive.fail(failure);
                }
            }
        });
    }

    /// Upstream `requestOperationAbort` (lane.ts:1005-1100): the
    /// package-private durable cancellation primitive.
    pub async fn request_abort(
        &self,
        operation_id: &str,
        context: Context,
    ) -> anyhow::Result<crate::agent_core::harness::agent_harness::AbortRequestResult> {
        if let Some(closed) = self.closed_result() {
            return Ok(Err(closed));
        }
        self.assert_open()?;

        let drive = self
            .active_drive()
            .filter(|active| active.operation_id() == operation_id);
        let cancellation = CancellationToken::new();
        if let Some(active) = &drive {
            active.begin_abort(cancellation.clone());
        }
        let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let name = self.name.clone();
        let operation_id = operation_id.to_owned();
        let planner_context = context.clone();
        let gate_for_materialize = Arc::clone(&gate);
        let cancellation_for_materialize = cancellation.clone();
        let drive_for_materialize = drive.clone();
        let result: anyhow::Result<crate::agent_core::harness::agent_harness::AbortRequestResult> =
            self.command(
                move |state, reader| {
                    let name = name.clone();
                    let operation_id = operation_id.clone();
                    let context = planner_context.clone();
                    let gate = Arc::clone(&gate_for_materialize);
                    let cancellation = cancellation_for_materialize.clone();
                    let drive = drive_for_materialize.clone();
                    Box::pin(async move {
                        let current = state
                            .operation
                            .as_ref()
                            .map(|operation| operation.meta.operation_id.as_str());
                        if current != Some(operation_id.as_str()) {
                            return Ok(LaneCommand::Return {
                                result: Err(mismatch_error(
                                    &name,
                                    &operation_id,
                                    current,
                                    state.last_operation_id.as_deref(),
                                )),
                            });
                        }
                        let operation = state
                            .operation
                            .as_ref()
                            .expect("current operation checked above");
                        if matches!(
                            operation.state.scope.control,
                            Control::CancelRequested { .. }
                        ) {
                            return Ok(LaneCommand::Return {
                                result: Ok(
                                    crate::agent_core::harness::agent_harness::AbortRequestOutcome {
                                        operation_id,
                                        newly_requested: false,
                                        steer: Vec::new(),
                                        follow_up: Vec::new(),
                                    },
                                ),
                            });
                        }

                        let removed: Vec<InboxItem> = state
                            .inbox
                            .iter()
                            .filter(|item| {
                                matches!(item.kind, InboxItemKind::Steer | InboxItemKind::FollowUp)
                            })
                            .cloned()
                            .collect();
                        let mut steer = Vec::new();
                        let mut follow_up = Vec::new();
                        for item in &removed {
                            let stored = reader
                                .get_value(&pending_entry(&item.entry_id), context.clone())
                                .await?
                                .ok_or_else(|| {
                                    invariant(format!(
                                        "Pending {} entry {} is missing its message",
                                        inbox_kind_name(item.kind),
                                        item.entry_id
                                    ))
                                })?;
                            let payload =
                                match serde_json::from_value::<PendingEntry>(stored.value)? {
                                    PendingEntry::Message { payload } => payload,
                                    PendingEntry::Custom { .. } => {
                                        return Err(invariant(format!(
                                            "Pending {} entry {} is missing its message",
                                            inbox_kind_name(item.kind),
                                            item.entry_id
                                        )));
                                    }
                                };
                            if item.kind == InboxItemKind::Steer {
                                steer.push(payload);
                            } else {
                                follow_up.push(payload);
                            }
                        }
                        let inbox = without_inbox_items(&state.inbox, &removed);
                        let queues = read_lane_queues(reader, &inbox, context.clone()).await?;
                        let cancelling_operation_state = OperationState {
                            scope: OperationScope {
                                control: Control::CancelRequested {
                                    requested_at: crate::ai::now_ms(),
                                },
                                ..operation.state.scope.clone()
                            },
                            phase: operation.state.phase.clone(),
                        };
                        let next = {
                            let mut next = state.clone();
                            next.inbox = inbox.clone();
                            next.operation = Some(Operation {
                                meta: operation.meta.clone(),
                                state: cancelling_operation_state.clone(),
                            });
                            next
                        };
                        let steer_for_materialize = steer.clone();
                        let follow_up_for_materialize = follow_up.clone();
                        let operation_id_for_materialize = operation_id.clone();
                        Ok(LaneCommand::Commit {
                            writes: [
                                removed
                                    .iter()
                                    .map(|item| delete_value(&pending_entry(&item.entry_id)))
                                    .collect::<Vec<Write>>(),
                                vec![
                                    set_value(
                                        &operation_state(&operation_id),
                                        serde_json::to_value(&cancelling_operation_state)?,
                                    ),
                                    set_value(
                                        &lane_state(&name),
                                        serde_json::to_value(durable_lane_state(
                                            state,
                                            Some(&operation_id),
                                            Some(&inbox),
                                            None,
                                        ))?,
                                    ),
                                ],
                            ]
                            .concat(),
                            next,
                            materialize: Box::new(move |_commit: &CommitResult| {
                                settle_abort_gate(&gate, &cancellation, &drive, true);
                                Ok(crate::agent_core::harness::agent_harness::AbortRequestOutcome {
                                    operation_id: operation_id_for_materialize,
                                    newly_requested: true,
                                    steer: steer_for_materialize,
                                    follow_up: follow_up_for_materialize,
                                })
                            }),
                            events: Some(Box::new(move |_commit: &CommitResult| {
                                let mut events = vec![HarnessEvent::OperationAbort {
                                    operation_id: operation_id.clone(),
                                    steer: steer.clone(),
                                    follow_up: follow_up.clone(),
                                    lane: name.clone(),
                                }];
                                if !removed.is_empty() {
                                    events.push(HarnessEvent::QueueUpdate {
                                        queues,
                                        lane: name.clone(),
                                    });
                                }
                                Ok(events)
                            })),
                        })
                    })
                },
                context.clone(),
            )
            .await;
        match result {
            Ok(inner) => {
                if let Ok(outcome) = &inner {
                    settle_abort_gate(&gate, &cancellation, &drive, !outcome.newly_requested);
                }
                Ok(inner)
            }
            Err(error) => {
                // Upstream rejects the admission-gate promise with the error;
                // the port's gate releases by cancellation alone.
                settle_abort_gate(&gate, &cancellation, &drive, false);
                Err(error)
            }
        }
    }

    /// Upstream `prompt` (the string + images form).
    pub async fn prompt(
        &self,
        text: &str,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> anyhow::Result<RunResult> {
        let request = match images {
            None => OperationRequest::Prompt {
                prompt: text.to_owned(),
            },
            Some(images) if images.is_empty() => OperationRequest::Prompt {
                prompt: text.to_owned(),
            },
            Some(images) => OperationRequest::PromptWithImages {
                prompt: text.to_owned(),
                images,
            },
        };
        self.drive_run_request(request, context).await
    }

    /// Upstream `prompt` (the message form).
    pub async fn prompt_messages(
        &self,
        messages: Vec<AgentMessage>,
        context: Context,
    ) -> anyhow::Result<RunResult> {
        self.drive_run_request(OperationRequest::PromptMessages { messages }, context)
            .await
    }

    /// Upstream `skill`.
    pub async fn skill(
        &self,
        name: &str,
        additional_instructions: Option<String>,
        context: Context,
    ) -> anyhow::Result<RunResult> {
        self.drive_run_request(
            OperationRequest::Skill {
                name: name.to_owned(),
                additional_instructions,
            },
            context,
        )
        .await
    }

    /// Upstream `promptFromTemplate`.
    pub async fn prompt_from_template(
        &self,
        name: &str,
        args: Option<Vec<String>>,
        context: Context,
    ) -> anyhow::Result<RunResult> {
        self.drive_run_request(
            OperationRequest::PromptTemplate {
                name: name.to_owned(),
                args,
            },
            context,
        )
        .await
    }

    /// Upstream `driveRunRequest` (lane.ts:1158-1198).
    async fn drive_run_request(
        &self,
        request: OperationRequest,
        context: Context,
    ) -> anyhow::Result<RunResult> {
        let admission = self.accept(&request, context.clone()).await?;
        let admission = match admission {
            Ok(admission) => admission,
            // The run-acceptance error set is exactly
            // LaneBusy | InvalidMessage | UnknownSkill | UnknownTemplate |
            // Closed (upstream filters those tags and faults on the rest;
            // the lane-level accept cannot produce any other tag here).
            Err(error) => return Ok(Err(error)),
        };
        let driven = self
            .drive(
                &DriveOptions {
                    operation_id: admission.operation_id.clone(),
                    wait_for_retry: Some(true),
                    poll_deferred: None,
                },
                context.clone(),
            )
            .await?;
        match driven {
            Ok(outcome) => Ok(Ok(run_outcome_from_drive(&outcome, "Run")?)),
            Err(error) => {
                if matches!(error, TaggedError::Closed { .. }) {
                    return Ok(Err(error));
                }
                Err(anyhow::Error::new(SessionInvariantError(format!(
                    "Accepted run {} no longer matches its lane",
                    admission.operation_id
                ))))
            }
        }
    }

    /// Upstream `compact` (lane.ts:1200-1230).
    pub async fn compact(
        &self,
        custom_instructions: Option<String>,
        context: Context,
    ) -> anyhow::Result<CompactionResult> {
        let admission = self
            .accept(
                &OperationRequest::Compaction {
                    custom_instructions,
                },
                context.clone(),
            )
            .await?;
        let admission = match admission {
            Ok(admission) => admission,
            Err(error) => match error.tag() {
                "LaneBusy" | "NothingToCompact" | "Closed" => return Ok(Err(error)),
                tag => {
                    return Err(anyhow::Error::new(SessionInvariantError(format!(
                        "Compaction acceptance returned {tag}"
                    ))));
                }
            },
        };
        let compacted = match self
            .drive_structural_admission(&admission, context.clone())
            .await?
        {
            Ok(record) => record,
            Err(closed) => return Ok(Err(closed)),
        };
        let continuation = self
            .continue_after_structural(&compacted, context.clone())
            .await?;
        Ok(match continuation {
            Ok(run) => Ok(CompactionOutcome {
                compaction: compacted,
                run,
            }),
            Err(closed) => Err(closed),
        })
    }

    /// Upstream `navigateTree` (lane.ts:1232-1264).
    pub async fn navigate_tree(
        &self,
        target_id: Option<String>,
        options: Option<NavigationOptions>,
        context: Context,
    ) -> anyhow::Result<NavigationResult> {
        let admission = self
            .accept(
                &OperationRequest::Navigation { target_id, options },
                context.clone(),
            )
            .await?;
        let admission = match admission {
            Ok(admission) => admission,
            Err(error) => match error.tag() {
                "LaneBusy" | "InvalidNavigation" | "UnknownTarget" | "Closed" => {
                    return Ok(Err(error));
                }
                tag => {
                    return Err(anyhow::Error::new(SessionInvariantError(format!(
                        "Navigation acceptance returned {tag}"
                    ))));
                }
            },
        };
        let navigated = match self
            .drive_structural_admission(&admission, context.clone())
            .await?
        {
            Ok(record) => record,
            Err(closed) => return Ok(Err(closed)),
        };
        let continuation = self
            .continue_after_structural(&navigated, context.clone())
            .await?;
        Ok(match continuation {
            Ok(run) => Ok(NavigationOutcome {
                navigation: navigated,
                run,
            }),
            Err(closed) => Err(closed),
        })
    }

    /// Upstream `driveStructuralAdmission` (lane.ts:1266-1283).
    async fn drive_structural_admission(
        &self,
        admission: &OperationAdmission,
        context: Context,
    ) -> anyhow::Result<Result<OperationResultRecord, TaggedError>> {
        let driven = self
            .drive(
                &DriveOptions {
                    operation_id: admission.operation_id.clone(),
                    wait_for_retry: Some(true),
                    poll_deferred: None,
                },
                context.clone(),
            )
            .await?;
        match driven {
            Ok(DriveOutcome::Settled { outcome }) => Ok(Ok(outcome)),
            Ok(DriveOutcome::Waiting { reason, .. }) => {
                Err(anyhow::Error::new(SessionInvariantError(format!(
                    "{} {} returned {}",
                    admission.kind,
                    admission.operation_id,
                    waiting_reason_name(&reason)
                ))))
            }
            Err(error) => {
                if matches!(error, TaggedError::Closed { .. }) {
                    return Ok(Err(error));
                }
                Err(anyhow::Error::new(SessionInvariantError(format!(
                    "Accepted {} {} no longer matches its lane",
                    admission.kind, admission.operation_id
                ))))
            }
        }
    }

    /// Upstream `continueAfterStructural` (lane.ts:1285-1325): the ordinary
    /// continuation run after a structural operation, `Ok(None)` when the
    /// record aborted or the continuation admission was rejected.
    async fn continue_after_structural(
        &self,
        record: &OperationResultRecord,
        context: Context,
    ) -> anyhow::Result<Result<Option<RunOutcome>, TaggedError>> {
        if record.status == crate::agent_core::harness::session::TerminalStatus::Aborted {
            return Ok(Ok(None));
        }
        let admission = self
            .accept(
                &OperationRequest::Prompt {
                    prompt: String::new(),
                },
                context.clone(),
            )
            .await?;
        let admission = match admission {
            Ok(admission) => admission,
            Err(error) => match error.tag() {
                "InvalidMessage" | "LaneBusy" => return Ok(Ok(None)),
                "Closed" => return Ok(Err(error)),
                tag => {
                    return Err(anyhow::Error::new(SessionInvariantError(format!(
                        "Structural continuation acceptance returned {tag}"
                    ))));
                }
            },
        };
        let driven = self
            .drive(
                &DriveOptions {
                    operation_id: admission.operation_id.clone(),
                    wait_for_retry: Some(true),
                    poll_deferred: None,
                },
                context.clone(),
            )
            .await?;
        match driven {
            Ok(outcome) => Ok(Ok(Some(run_outcome_from_drive(
                &outcome,
                "Continuation run",
            )?))),
            Err(error) => {
                if matches!(error, TaggedError::Closed { .. }) {
                    return Ok(Err(error));
                }
                Err(anyhow::Error::new(SessionInvariantError(format!(
                    "Continuation run {} no longer matches its lane",
                    admission.operation_id
                ))))
            }
        }
    }

    /// Upstream `resume` (lane.ts:1327-1371).
    pub async fn resume(&self, context: Context) -> anyhow::Result<ResumeResult> {
        if let Some(closed) = self.closed_result() {
            return Ok(Err(closed));
        }
        self.assert_open()?;
        let name = self.name.clone();
        let inspected: Result<String, TaggedError> = self
            .command(
                move |state, _reader| {
                    let name = name.clone();
                    Box::pin(async move {
                        Ok(LaneCommand::Return {
                            result: match &state.operation {
                                None => Err(TaggedError::NothingToResume {
                                    lane: name.clone(),
                                    message: format!(
                                        "Lane {} has no active operation to resume",
                                        serde_json::to_string(&name).unwrap_or_default()
                                    ),
                                }),
                                Some(operation) => Ok(operation.meta.operation_id.clone()),
                            },
                        })
                    })
                },
                context.clone(),
            )
            .await?;
        let operation_id = match inspected {
            Ok(id) => id,
            Err(error) => return Ok(Err(error)),
        };
        let driven = self
            .drive(
                &DriveOptions {
                    operation_id: operation_id.clone(),
                    wait_for_retry: Some(true),
                    poll_deferred: Some(true),
                },
                context.clone(),
            )
            .await?;
        match driven {
            Ok(outcome) => Ok(Ok(run_outcome_from_drive(&outcome, "Operation")?)),
            Err(error) => {
                if matches!(error, TaggedError::Closed { .. }) {
                    return Ok(Err(error));
                }
                Err(anyhow::Error::new(SessionInvariantError(format!(
                    "Operation {} no longer matches its lane",
                    operation_id
                ))))
            }
        }
    }

    /// Upstream `abort` (lane.ts:1373-1416).
    pub async fn abort(
        &self,
        context: Context,
    ) -> anyhow::Result<crate::agent_core::harness::agent_harness::AbortResult> {
        if let Some(closed) = self.closed_result() {
            return Ok(Err(closed));
        }
        self.assert_open()?;
        let name = self.name.clone();
        let operation_id: Option<String> = self
            .command(
                move |state, _reader| {
                    Box::pin(async move {
                        Ok(LaneCommand::Return {
                            result: state
                                .operation
                                .as_ref()
                                .map(|operation| operation.meta.operation_id.clone()),
                        })
                    })
                },
                context.clone(),
            )
            .await?;
        let Some(operation_id) = operation_id else {
            return Ok(Err(TaggedError::NoActiveOperation {
                lane: name.clone(),
                message: format!(
                    "Lane {} has no active operation",
                    serde_json::to_string(&name).unwrap_or_default()
                ),
            }));
        };
        let requested = self.request_abort(&operation_id, context.clone()).await?;
        let requested = match requested {
            Ok(outcome) => outcome,
            Err(error) => {
                if matches!(error, TaggedError::Closed { .. }) {
                    return Ok(Err(error));
                }
                return Ok(Err(TaggedError::NoActiveOperation {
                    lane: name.clone(),
                    message: format!(
                        "Lane {} no longer has the inspected operation",
                        serde_json::to_string(&name).unwrap_or_default()
                    ),
                }));
            }
        };
        let driven = self
            .drive(
                &DriveOptions {
                    operation_id: operation_id.clone(),
                    wait_for_retry: None,
                    poll_deferred: None,
                },
                context.clone(),
            )
            .await?;
        match driven {
            Ok(_outcome) => Ok(Ok(
                crate::agent_core::harness::agent_harness::AbortOutcome {
                    operation_id,
                    steer: requested.steer,
                    follow_up: requested.follow_up,
                },
            )),
            Err(error) => {
                if matches!(error, TaggedError::Closed { .. }) {
                    return Ok(Err(error));
                }
                Err(anyhow::Error::new(SessionInvariantError(format!(
                    "Cancelled operation {} no longer matches its lane",
                    operation_id
                ))))
            }
        }
    }

    /// Upstream `steer` (lane.ts:1418-1420).
    pub async fn steer(
        &self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> anyhow::Result<QueueResult> {
        self.enqueue(InboxItemKind::Steer, message, images, context)
            .await
    }

    /// Upstream `followUp` (lane.ts:1422-1428).
    pub async fn follow_up(
        &self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> anyhow::Result<QueueResult> {
        self.enqueue(InboxItemKind::FollowUp, message, images, context)
            .await
    }

    /// Upstream `nextRun` (lane.ts:1430-1432).
    pub async fn next_run(
        &self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> anyhow::Result<QueueResult> {
        self.enqueue(InboxItemKind::NextRun, message, images, context)
            .await
    }

    /// Upstream `enqueue` (lane.ts:1434-1516).
    async fn enqueue(
        &self,
        kind: InboxItemKind,
        input: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> anyhow::Result<QueueResult> {
        if let Some(closed) = self.closed_result() {
            return Ok(Err(closed));
        }
        self.assert_open()?;
        let at = crate::ai::now_ms();
        let images = images.unwrap_or_default();
        let message: AgentMessage = match input {
            QueueInput::Text(text) => {
                if text.is_empty() && images.is_empty() {
                    return Ok(Err(TaggedError::InvalidMessage {
                        lane: self.name.clone(),
                        reason: "empty".to_owned(),
                        message: "Queued input must contain text or an image".to_owned(),
                    }));
                }
                let mut blocks: Vec<TextOrImageBlock> = Vec::new();
                if !text.is_empty() {
                    blocks.push(TextOrImageBlock::Text(TextContent {
                        text,
                        text_signature: None,
                    }));
                }
                blocks.extend(images.into_iter().map(TextOrImageBlock::Image));
                AgentMessage::User(UserMessage {
                    content: StringOrBlocks::Blocks(blocks),
                    timestamp: at,
                })
            }
            QueueInput::Message(message) => {
                if let AgentMessage::Assistant(assistant) = &message {
                    if assistant.stop_reason == StopReason::Pending {
                        return Ok(Err(TaggedError::InvalidMessage {
                            lane: self.name.clone(),
                            reason: "pending_assistant".to_owned(),
                            message: "Cannot queue a pending assistant message".to_owned(),
                        }));
                    }
                }
                if !images.is_empty() && !matches!(message, AgentMessage::User(_)) {
                    return Ok(Err(TaggedError::InvalidMessage {
                        lane: self.name.clone(),
                        reason: "images_with_non_user".to_owned(),
                        message: "Images can be added only to queued user messages".to_owned(),
                    }));
                }
                match (message, images.is_empty()) {
                    (AgentMessage::User(mut user), false) => {
                        let mut blocks = match user.content {
                            StringOrBlocks::Text(text) if !text.is_empty() => {
                                vec![TextOrImageBlock::Text(TextContent {
                                    text,
                                    text_signature: None,
                                })]
                            }
                            StringOrBlocks::Text(_) => Vec::new(),
                            StringOrBlocks::Blocks(blocks) => blocks,
                        };
                        blocks.extend(images.into_iter().map(TextOrImageBlock::Image));
                        user.content = StringOrBlocks::Blocks(blocks);
                        AgentMessage::User(user)
                    }
                    (message, _) => message,
                }
            }
        };
        let entry_id = self.session.id_generator().next(Some(at));
        let name = self.name.clone();
        let planner_context = context.clone();
        let admission = self
            .command(
                move |state, reader| {
                    let name = name.clone();
                    let entry_id = entry_id.clone();
                    let message = message.clone();
                    let context = planner_context.clone();
                    Box::pin(async move {
                        let mut inbox = state.inbox.clone();
                        inbox.push(InboxItem {
                            entry_id: entry_id.clone(),
                            kind,
                        });
                        let mut queues =
                            read_lane_queues(reader, &state.inbox, context.clone()).await?;
                        queues.push(LaneQueuedItem::Message {
                            entry_id: entry_id.clone(),
                            kind,
                            message: message.clone(),
                        });
                        let current_operation_id = state
                            .operation
                            .as_ref()
                            .map(|operation| operation.meta.operation_id.clone());
                        let next = {
                            let mut next = state.clone();
                            next.inbox = inbox.clone();
                            next
                        };
                        Ok(LaneCommand::Commit {
                            writes: vec![
                                set_value(
                                    &pending_entry(&entry_id),
                                    serde_json::to_value(PendingEntry::Message {
                                        payload: message.clone(),
                                    })?,
                                ),
                                set_value(
                                    &lane_state(&name),
                                    serde_json::to_value(durable_lane_state(
                                        state,
                                        current_operation_id.as_deref(),
                                        Some(&inbox),
                                        None,
                                    ))?,
                                ),
                            ],
                            next,
                            materialize: Box::new(move |_commit: &CommitResult| {
                                Ok(QueueOutcome { entry_id })
                            }),
                            events: Some(Box::new(move |_commit: &CommitResult| {
                                Ok(vec![HarnessEvent::QueueUpdate {
                                    queues,
                                    lane: name.clone(),
                                }])
                            })),
                        })
                    })
                },
                context,
            )
            .await?;
        Ok(admission.map_err(acceptance_error_to_tagged))
    }

    /// Upstream `cancelQueued` (lane.ts:1518-1551).
    pub async fn cancel_queued(
        &self,
        entry_id: &str,
        context: Context,
    ) -> anyhow::Result<CancelQueuedResult> {
        if let Some(closed) = self.closed_result() {
            return Ok(Err(closed));
        }
        self.assert_open()?;
        let name = self.name.clone();
        let entry_id = entry_id.to_owned();
        let planner_context = context.clone();
        let outcome: CancelQueuedOutcome = self
            .command(
                move |state, reader| {
                    let name = name.clone();
                    let entry_id = entry_id.clone();
                    let context = planner_context.clone();
                    Box::pin(async move {
                        let queued = state
                            .inbox
                            .iter()
                            .find(|item| item.entry_id == entry_id)
                            .cloned();
                        let Some(queued) = queued else {
                            let consumed = reader
                                .get_entries(std::slice::from_ref(&entry_id), context.clone())
                                .await?
                                .contains_key(&entry_id);
                            return Ok(LaneCommand::Return {
                                result: if consumed {
                                    CancelQueuedOutcome::AlreadyConsumed
                                } else {
                                    CancelQueuedOutcome::NotFound
                                },
                            });
                        };
                        if reader
                            .get_value(&pending_entry(&entry_id), context.clone())
                            .await?
                            .is_none()
                        {
                            return Err(invariant(format!(
                                "Queued {} entry {} is missing its payload",
                                inbox_kind_name(queued.kind),
                                entry_id
                            )));
                        }
                        let inbox: Vec<InboxItem> = state
                            .inbox
                            .iter()
                            .filter(|item| item.entry_id != entry_id)
                            .cloned()
                            .collect();
                        let queues = read_lane_queues(reader, &inbox, context.clone()).await?;
                        let current_operation_id = state
                            .operation
                            .as_ref()
                            .map(|operation| operation.meta.operation_id.clone());
                        let next = {
                            let mut next = state.clone();
                            next.inbox = inbox.clone();
                            next
                        };
                        Ok(LaneCommand::Commit {
                            writes: vec![
                                delete_value(&pending_entry(&entry_id)),
                                set_value(
                                    &lane_state(&name),
                                    serde_json::to_value(durable_lane_state(
                                        state,
                                        current_operation_id.as_deref(),
                                        Some(&inbox),
                                        None,
                                    ))?,
                                ),
                            ],
                            next,
                            materialize: Box::new(move |_commit: &CommitResult| {
                                CancelQueuedOutcome::Cancelled
                            }),
                            events: Some(Box::new(move |_commit: &CommitResult| {
                                Ok(vec![HarnessEvent::QueueUpdate {
                                    queues,
                                    lane: name.clone(),
                                }])
                            })),
                        })
                    })
                },
                context,
            )
            .await?;
        Ok(Ok(outcome))
    }

    /// Upstream `appendMessage` (lane.ts:1916-1918).
    pub async fn append_message(
        &self,
        message: AgentMessage,
        context: Context,
    ) -> anyhow::Result<String> {
        self.append(PendingEntry::Message { payload: message }, context)
            .await
    }

    /// Upstream `appendCustomEntry` (lane.ts:1920-1922).
    pub async fn append_custom_entry(
        &self,
        custom_type: String,
        data: Option<serde_json::Value>,
        context: Context,
    ) -> anyhow::Result<String> {
        self.append(
            PendingEntry::Custom {
                custom_type,
                payload: data,
            },
            context,
        )
        .await
    }

    /// Upstream `append` (lane.ts:1924-1991): an idle append commits at the
    /// tip after flushing queued writes; an operation append stages the
    /// entry in the lane inbox.
    async fn append(&self, pending: PendingEntry, context: Context) -> anyhow::Result<String> {
        self.assert_open()?;
        if let PendingEntry::Message {
            payload: AgentMessage::Assistant(assistant),
        } = &pending
        {
            if assistant.stop_reason == StopReason::Pending {
                return Err(anyhow::Error::new(SessionPendingAssistantMessageError));
            }
        }
        let id = self.session.id_generator().next(None);
        let name = self.name.clone();
        let planner_context = context.clone();
        self.command(
            move |state, reader| {
                let name = name.clone();
                let pending = pending.clone();
                let id = id.clone();
                let context = planner_context.clone();
                Box::pin(async move {
                    match &state.operation {
                        None => {
                            let queued: Vec<InboxItem> = state
                                .inbox
                                .iter()
                                .filter(|item| item.kind == InboxItemKind::Write)
                                .cloned()
                                .collect();
                            let mut captured: Vec<NewEntry> = Vec::with_capacity(queued.len());
                            for item in &queued {
                                let stored = reader
                                    .get_value(&pending_entry(&item.entry_id), context.clone())
                                    .await?
                                    .ok_or_else(|| {
                                        invariant(format!(
                                            "Pending write {} is missing its payload",
                                            item.entry_id
                                        ))
                                    })?;
                                let staged = serde_json::from_value::<PendingEntry>(stored.value)?;
                                captured.push(pending_entry_write(&item.entry_id, &staged));
                            }
                            let inbox = without_inbox_items(&state.inbox, &queued);
                            let queues = if queued.is_empty() {
                                None
                            } else {
                                Some(read_lane_queues(reader, &inbox, context.clone()).await?)
                            };
                            captured.push(pending_entry_write(&id, &pending));
                            let entries = chain_entries(state.tip_id.as_deref(), &captured);
                            let mut writes: Vec<Write> =
                                entries.iter().cloned().map(insert_entry).collect();
                            writes.extend(
                                queued
                                    .iter()
                                    .map(|item| delete_value(&pending_entry(&item.entry_id))),
                            );
                            writes.push(set_value(
                                &crate::agent_core::harness::session::branch_tip(&name),
                                serde_json::Value::String(id.clone()),
                            ));
                            writes.push(set_value(
                                &lane_state(&name),
                                serde_json::to_value(durable_lane_state(
                                    state,
                                    None,
                                    Some(&inbox),
                                    None,
                                ))?,
                            ));
                            let next = {
                                let mut next = state.clone();
                                next.tip_id = Some(id.clone());
                                next.inbox = inbox;
                                next
                            };
                            Ok(LaneCommand::Commit {
                                writes,
                                next,
                                materialize: Box::new(move |_commit: &CommitResult| id),
                                events: Some(Box::new(move |commit: &CommitResult| {
                                    let mut events: Vec<HarnessEvent> =
                                        committed_entry_events(&entries, commit, &name, None, 0)?
                                            .into_iter()
                                            .map(HarnessEvent::from)
                                            .collect();
                                    if let Some(queues) = queues {
                                        events.push(HarnessEvent::QueueUpdate {
                                            queues,
                                            lane: name.clone(),
                                        });
                                    }
                                    Ok(events)
                                })),
                            })
                        }
                        Some(operation) => {
                            let mut inbox = state.inbox.clone();
                            inbox.push(InboxItem {
                                entry_id: id.clone(),
                                kind: InboxItemKind::Write,
                            });
                            let mut queues =
                                read_lane_queues(reader, &state.inbox, context.clone()).await?;
                            queues.push(match &pending {
                                PendingEntry::Message { payload } => LaneQueuedItem::Message {
                                    entry_id: id.clone(),
                                    kind: InboxItemKind::Write,
                                    message: payload.clone(),
                                },
                                PendingEntry::Custom {
                                    custom_type,
                                    payload,
                                } => LaneQueuedItem::Custom {
                                    entry_id: id.clone(),
                                    kind: WriteKind::Write,
                                    custom_type: custom_type.clone(),
                                    data: payload.clone(),
                                },
                            });
                            let next = {
                                let mut next = state.clone();
                                next.inbox = inbox.clone();
                                next
                            };
                            Ok(LaneCommand::Commit {
                                writes: vec![
                                    set_value(&pending_entry(&id), serde_json::to_value(&pending)?),
                                    set_value(
                                        &lane_state(&name),
                                        serde_json::to_value(durable_lane_state(
                                            state,
                                            Some(&operation.meta.operation_id),
                                            Some(&inbox),
                                            None,
                                        ))?,
                                    ),
                                ],
                                next,
                                materialize: Box::new(move |_commit: &CommitResult| id),
                                events: Some(Box::new(move |_commit: &CommitResult| {
                                    Ok(vec![HarnessEvent::QueueUpdate {
                                        queues,
                                        lane: name.clone(),
                                    }])
                                })),
                            })
                        }
                    }
                })
            },
            context,
        )
        .await
    }

    /// Upstream `recordUsage` (lane.ts:1553-1586): one caller-adjustment row
    /// in the usage ledger with its committed totals event.
    pub async fn record_usage(
        &self,
        usage: Usage,
        options: Option<RecordUsageOptions>,
        context: Context,
    ) -> anyhow::Result<RecordUsageResult> {
        if let Some(closed) = self.closed_result() {
            return Ok(Err(closed));
        }
        self.assert_open()?;
        let usage_id = self.session.id_generator().next(None);
        let row = NewUsageRow {
            id: usage_id.clone(),
            usage,
            entry_id: options
                .as_ref()
                .and_then(|options| options.entry_id.clone()),
            adjustment: true,
            details: options.and_then(|options| options.details),
        };
        let name = self.name.clone();
        let outcome: RecordUsageOutcome = self
            .command(
                move |state, _reader| {
                    let name = name.clone();
                    let row = row.clone();
                    let usage_id = usage_id.clone();
                    Box::pin(async move {
                        Ok(LaneCommand::Commit {
                            writes: vec![insert_usage(row.clone())],
                            next: state.clone(),
                            materialize: Box::new(move |_commit: &CommitResult| {
                                RecordUsageOutcome { usage_id }
                            }),
                            events: Some(Box::new(move |commit: &CommitResult| {
                                let seq = *commit.seqs.first().ok_or_else(|| {
                                    SessionInvariantError(
                                        "Usage commit produced no sequence".to_owned(),
                                    )
                                })?;
                                let row = row.clone();
                                Ok(vec![HarnessEvent::Usage {
                                    lane: name.clone(),
                                    row: UsageRow {
                                        id: row.id,
                                        seq,
                                        usage: row.usage,
                                        entry_id: row.entry_id,
                                        adjustment: row.adjustment,
                                        details: row.details,
                                    },
                                    totals: commit.stats.usage,
                                }])
                            })),
                        })
                    })
                },
                context,
            )
            .await?;
        Ok(Ok(outcome))
    }

    /// Upstream `waitForIdle` (lane.ts:1588-1606).
    pub async fn wait_for_idle(&self, context: Context) -> anyhow::Result<()> {
        loop {
            let lease = self.lease();
            let observation: IdleObservation = self
                .command(
                    move |state, _reader| {
                        let lease = lease.clone();
                        Box::pin(async move {
                            let (drive, idle) = {
                                let shared = lease.lock_shared();
                                (
                                    shared.active_drive.clone(),
                                    state.operation.is_none() && shared.active_drive.is_none(),
                                )
                            };
                            Ok(LaneCommand::Return {
                                result: if idle {
                                    IdleObservation::Idle
                                } else {
                                    IdleObservation::Wait { drive }
                                },
                            })
                        })
                    },
                    context.clone(),
                )
                .await?;
            match observation {
                IdleObservation::Idle => return Ok(()),
                IdleObservation::Wait { drive } => match drive {
                    Some(drive) => {
                        let completion = drive.completion();
                        wait_drive_outcome(self, &completion, &context).await?;
                    }
                    None => {
                        let mut receiver = self.state_change.subscribe();
                        Self::wait_state_change(&mut receiver).await;
                    }
                },
            }
        }
    }

    /// Upstream `runWhenIdle` (lane.ts:1608-1646): claim the idle window,
    /// run the callback, release.
    pub async fn run_when_idle(
        &self,
        callback: crate::agent_core::harness::agent_harness::IdleCallback,
        context: Context,
    ) -> anyhow::Result<()> {
        let owner: CancellationToken = loop {
            let lease = self.lease();
            let observation: IdleClaimObservation = self
                .command(
                    move |state, _reader| {
                        let lease = lease.clone();
                        Box::pin(async move {
                            let drive = {
                                let shared = lease.lock_shared();
                                if state.operation.is_some() || shared.active_drive.is_some() {
                                    shared.active_drive.clone()
                                } else {
                                    drop(shared);
                                    let owner = CancellationToken::new();
                                    lease.lock_shared().idle_owner = Some(owner.clone());
                                    lease.signal_state_change();
                                    return Ok(LaneCommand::Return {
                                        result: IdleClaimObservation::Claimed { owner },
                                    });
                                }
                            };
                            Ok(LaneCommand::Return {
                                result: IdleClaimObservation::Wait { drive },
                            })
                        })
                    },
                    context.clone(),
                )
                .await?;
            match observation {
                IdleClaimObservation::Claimed { owner: claimed } => {
                    break claimed;
                }
                IdleClaimObservation::Wait { drive } => match drive {
                    Some(drive) => {
                        let completion = drive.completion();
                        wait_drive_outcome(self, &completion, &context).await?;
                    }
                    None => {
                        let mut receiver = self.state_change.subscribe();
                        Self::wait_state_change(&mut receiver).await;
                    }
                },
            }
        };
        let result = async {
            self.assert_open()?;
            callback(context.clone()).await
        }
        .await;
        // Upstream `finally`: release the claim if still owned, then wake
        // waiters.
        {
            let mut shared = self.lock_shared();
            if shared
                .idle_owner
                .as_ref()
                .is_some_and(|current| *current == owner)
            {
                shared.idle_owner = None;
            }
        }
        owner.cancel();
        self.signal_state_change();
        result
    }

    /// Upstream `watch` (lane.ts:1705-1726): install a lane-filtered watcher
    /// over the harness event bus with the lane snapshot as its initial
    /// capture.
    pub async fn watch(&self, context: Context) -> anyhow::Result<LaneWatchHandle> {
        self.assert_open()?;
        let installer = self
            .lock_shared()
            .watch_install
            .clone()
            .ok_or_else(|| anyhow::Error::new(SliceNotImplemented::new("watch")))?;
        let weak = self.self_weak.clone();
        let capture: SnapshotCapture<LaneSnapshot> = Arc::new(move |context: Context| {
            let Some(lane) = weak.upgrade() else {
                let dropped: anyhow::Result<LaneSnapshot> =
                    Err(anyhow::anyhow!("Lane was dropped"));
                return Box::pin(async move { dropped })
                    as BoxFuture<'static, anyhow::Result<LaneSnapshot>>;
            };
            Box::pin(async move { lane.capture_lane_snapshot(context).await })
        });
        let name = self.name.clone();
        let filter: LaneWatchFilter = Box::new(move |event: &PublicHarnessEvent| {
            event.event_type() == "usage" || event.lane().is_none_or(|lane| lane == name)
        });
        installer(capture, filter, context).await
    }

    /// Upstream `captureLaneSnapshot` (lane.ts:1728-1881): the coherent
    /// lane snapshot read on the mutation line.
    pub async fn capture_lane_snapshot(&self, context: Context) -> anyhow::Result<LaneSnapshot> {
        let name = self.name.clone();
        let planner_context = context.clone();
        let faulted = self
            .sealed_error()
            .is_some_and(|sealed| sealed.kind == SealKind::Fault);
        self.read_lane(
            move |state, reader| {
                let name = name.clone();
                let context = planner_context.clone();
                Box::pin(async move {
                    let mut transcript = Vec::new();
                    if let Some(tip) = &state.tip_id {
                        transcript = reader
                            .scan_branch(
                                &StorageBranchScan {
                                    start: tip.clone(),
                                    stop_at_type: Some(EntryType::Compaction),
                                    order: Some(BranchScanOrder::NewestFirst),
                                    ..Default::default()
                                },
                                context.clone(),
                            )
                            .await?;
                        transcript.reverse();
                    }
                    let queues = read_lane_queues(reader, &state.inbox, context.clone()).await?;
                    let last_result = match &state.last_operation_id {
                        Some(id) => {
                            let stored = reader
                                .get_value(&operation_result(id), context.clone())
                                .await?
                                .ok_or_else(|| {
                                    SessionInvariantError(format!(
                                        "Lane {} is missing result {}",
                                        serde_json::to_string(&name).unwrap_or_default(),
                                        id
                                    ))
                                })?;
                            Some(serde_json::from_value::<OperationResultRecord>(
                                stored.value,
                            )?)
                        }
                        None => None,
                    };
                    let stats = reader.get_stats(context.clone()).await?;
                    let operation = match &state.operation {
                        None => None,
                        Some(operation) => Some(
                            capture_operation_snapshot(operation, reader, context.clone()).await?,
                        ),
                    };
                    Ok(LaneSnapshot {
                        lane: name,
                        transcript,
                        tip_id: state.tip_id.clone(),
                        last_result,
                        configuration: state.configuration.clone(),
                        stats,
                        operation,
                        queues,
                        faulted,
                    })
                })
            },
            context,
        )
        .await
    }
}

/// Upstream `readStreamingMessage` closure (`lane.ts:1759-1761`): rehydrate
/// the staged frames of a streaming assistant response and reduce them to
/// the projected message.
async fn read_streaming(
    reader: &dyn SessionMutator,
    response_entry_id: &str,
    context: Context,
    operation_id: String,
) -> anyhow::Result<Option<AgentMessage>> {
    let frames = read_assistant_frames(reader, &operation_id, response_entry_id, context).await?;
    let frames = frames
        .iter()
        .map(|value| serde_json::from_value::<AssistantMessageFrame>(value.clone()))
        .collect::<Result<Vec<AssistantMessageFrame>, _>>()?;
    Ok(reduce_assistant_message_frames(&frames)?.map(AgentMessage::Assistant))
}

/// The per-phase open-operation projection of `captureLaneSnapshot`
/// (lane.ts:1753-1868).
async fn capture_operation_snapshot(
    operation: &crate::agent_core::harness::runtime::durable::Operation,
    reader: &dyn SessionMutator,
    context: Context,
) -> anyhow::Result<LaneOperationSnapshot> {
    use crate::agent_core::harness::runtime::durable::{
        OperationIntent, OperationPhase, ToolCallState,
    };
    use crate::agent_core::harness::runtime::projection::OperationStatus;
    use crate::agent_core::harness::session::Entry;

    let operation_id = operation.meta.operation_id.clone();
    let mut running_tools: Vec<LaneSnapshotTool> = Vec::new();
    let mut streaming_message: Option<AgentMessage> = None;
    let mut retry: Option<RetrySnapshot> = None;
    let mut deferred: Option<DeferredSnapshot> = None;

    match &operation.state.phase {
        OperationPhase::AssistantRetryWait {
            generation_context,
            retry: wait,
        } => {
            retry = Some(RetrySnapshot {
                attempt: wait.next_attempt,
                max_attempts: generation_context.retry_policy.max_attempts,
                next_attempt_at: wait.not_before,
            });
        }
        OperationPhase::AssistantEffectPending {
            response_entry_id, ..
        } => {
            streaming_message = read_streaming(
                reader,
                response_entry_id,
                context.clone(),
                operation_id.clone(),
            )
            .await?;
        }
        OperationPhase::DeferredSuspended { deferred: scope } => {
            let source = reader
                .get_entries(
                    std::slice::from_ref(&scope.source_entry_id),
                    context.clone(),
                )
                .await?
                .get(&scope.source_entry_id)
                .cloned();
            let handle = match source {
                Some(Entry::Message {
                    message: AgentMessage::Assistant(assistant),
                    ..
                }) => assistant.deferred.clone().ok_or_else(|| {
                    invariant("Deferred source is missing its assistant handle".to_owned())
                })?,
                _ => {
                    return Err(invariant(
                        "Deferred source is missing its assistant handle".to_owned(),
                    ));
                }
            };
            deferred = Some(DeferredSnapshot {
                handle,
                poll: scope.poll,
            });
        }
        OperationPhase::DeferredEffectPending {
            deferred: scope,
            response_entry_id,
            ..
        } => {
            let source = reader
                .get_entries(
                    std::slice::from_ref(&scope.source_entry_id),
                    context.clone(),
                )
                .await?
                .get(&scope.source_entry_id)
                .cloned();
            let handle = match source {
                Some(Entry::Message {
                    message: AgentMessage::Assistant(assistant),
                    ..
                }) => assistant.deferred.clone().ok_or_else(|| {
                    invariant("Deferred source is missing its assistant handle".to_owned())
                })?,
                _ => {
                    return Err(invariant(
                        "Deferred source is missing its assistant handle".to_owned(),
                    ));
                }
            };
            deferred = Some(DeferredSnapshot {
                handle,
                poll: scope.poll,
            });
            streaming_message = read_streaming(
                reader,
                response_entry_id,
                context.clone(),
                operation_id.clone(),
            )
            .await?;
        }
        OperationPhase::Tools { batch } => {
            let assistant = reader
                .get_entries(
                    std::slice::from_ref(&batch.assistant_entry_id),
                    context.clone(),
                )
                .await?
                .get(&batch.assistant_entry_id)
                .cloned();
            let assistant_message = match assistant {
                Some(Entry::Message {
                    message: AgentMessage::Assistant(assistant),
                    ..
                }) => assistant,
                _ => {
                    return Err(invariant(
                        "Tool batch assistant entry is invalid".to_owned(),
                    ));
                }
            };
            for call in &batch.calls {
                let block = match assistant_message.content.get(call.source_index) {
                    Some(crate::ai::types::message::AssistantBlock::ToolCall(block)) => block,
                    _ => {
                        return Err(invariant(format!(
                            "Tool call source index {} does not name a tool-call block",
                            call.source_index
                        )));
                    }
                };
                match &call.state {
                    ToolCallState::Planned | ToolCallState::Completed { .. } => continue,
                    ToolCallState::EffectPending { .. } => {
                        let args = reader
                            .get_value(
                                &operation_tool_args(
                                    &operation_id,
                                    &batch.turn_id,
                                    call.source_index as i64,
                                ),
                                context.clone(),
                            )
                            .await?
                            .ok_or_else(|| {
                                invariant(format!(
                                    "Tool call {} is missing persisted arguments",
                                    block.id
                                ))
                            })?;
                        let checkpoint = reader
                            .get_value(
                                &pending_tool_output(&operation_id, &call.result_entry_id),
                                context.clone(),
                            )
                            .await?;
                        running_tools.push(LaneSnapshotTool {
                            tool_call_id: block.id.clone(),
                            tool_name: block.name.clone(),
                            args: args.value,
                            state: SnapshotToolState::Running {
                                result: checkpoint
                                    .and_then(|stored| serde_json::from_value(stored.value).ok()),
                            },
                        });
                    }
                    ToolCallState::OutcomeReady { terminate } => {
                        let args = reader
                            .get_value(
                                &operation_tool_args(
                                    &operation_id,
                                    &batch.turn_id,
                                    call.source_index as i64,
                                ),
                                context.clone(),
                            )
                            .await?;
                        let staged = reader
                            .get_value(&pending_entry(&call.result_entry_id), context.clone())
                            .await?;
                        let staged = match staged {
                            Some(stored) => {
                                match serde_json::from_value::<PendingEntry>(stored.value)? {
                                    PendingEntry::Message {
                                        payload: AgentMessage::ToolResult(result),
                                    } => result,
                                    _ => {
                                        return Err(invariant(format!(
                                            "Tool call {} is missing its staged result",
                                            call.result_entry_id
                                        )));
                                    }
                                }
                            }
                            None => {
                                return Err(invariant(format!(
                                    "Tool call {} is missing its staged result",
                                    call.result_entry_id
                                )));
                            }
                        };
                        if staged.tool_call_id != block.id || staged.tool_name != block.name {
                            return Err(invariant(format!(
                                "Tool call {} has a mismatched staged result",
                                call.result_entry_id
                            )));
                        }
                        running_tools.push(LaneSnapshotTool {
                            tool_call_id: block.id.clone(),
                            tool_name: block.name.clone(),
                            args: args.map(|stored| stored.value).unwrap_or_else(|| {
                                serde_json::to_value(&block.arguments)
                                    .expect("tool arguments serialize")
                            }),
                            state: SnapshotToolState::Settled {
                                result: tool_result_from_message(&staged, *terminate),
                                is_error: staged.is_error,
                            },
                        });
                    }
                }
            }
        }
        OperationPhase::SummaryRetryWait {
            task: _,
            summary_context,
            retry: wait,
        } => {
            retry = Some(RetrySnapshot {
                attempt: wait.next_attempt,
                max_attempts: summary_context.retry_policy.max_attempts,
                next_attempt_at: wait.not_before,
            });
        }
        _ => {}
    }

    Ok(LaneOperationSnapshot {
        id: operation_id.clone(),
        kind: match operation.meta.intent {
            OperationIntent::Run { .. } => OperationKind::Run,
            OperationIntent::Compaction { .. } => OperationKind::Compaction,
            OperationIntent::Navigation { .. } => OperationKind::Navigation,
        },
        started_at: operation.meta.started_at,
        from_tip_id: operation.meta.source_tip_id.clone(),
        status: if matches!(
            operation.state.scope.control,
            Control::CancelRequested { .. }
        ) {
            OperationStatus::Aborting
        } else {
            OperationStatus::Open
        },
        retry,
        deferred,
        streaming_message,
        running_tools,
    })
}

#[cfg(test)]
#[path = "lane_tests.rs"]
mod lane_tests;

#[cfg(test)]
#[path = "lane/installed_tools_tests.rs"]
mod installed_tools_tests;
