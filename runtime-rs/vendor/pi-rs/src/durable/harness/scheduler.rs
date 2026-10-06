//! Port of `src/harness/scheduler.ts`: the durable task scheduler of one
//! Harness — the live-record mirror over the Session's commit publications,
//! the ownership-tree walks, reservation, invocations and their runtimes, and
//! the phase-step state machine.
//!
//! Divergences (structural, disclosed):
//! - **D18 (microtasks and timers).** Upstream schedules reconcile and drain
//!   passes with `queueMicrotask` and sleeps with `setTimeout`; the port
//!   spawns one tokio task per pass and sleeps with [`tokio::time`] (capped
//!   at the same 2^31-1 ms `MAX_TIMER_DELAY`). Ordering and the
//!   one-pass-at-a-time guarantees are preserved.
//! - **D19 (abort surfaces).** Upstream `AbortController`/`AbortSignal`
//!   become [`tokio_util::sync::CancellationToken`]; `AbortSignal.any`
//!   becomes a `tokio::select!` over the tokens, and
//!   `awaitWithContext(promise, context)` selects the wait against the
//!   context's signal.
//! - **D20 (report causes).** Upstream attaches a `cause` to the
//!   "keeps running under its old definition" report error; the port's
//!   [`ReportFn`] carries the message only (the cause's discriminator is
//!   already named by the two report texts).
//! - **D21 (missing abort handler).** Upstream `TaskDefinition.abort` is a
//!   required method; the port keeps it optional and a missing handler fails
//!   the abort invocation like a thrown one, with the upstream null-call
//!   text.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::context::{abort_signal_key, Context};

use super::super::env::ExecutionEnv;
use super::super::errors::PlainError;
use super::super::harness::types::{
    json_equal, AbortTaskResult, ApiFuture, BlockedReason, ConversationHandle, HookRegistration,
    ModelsHandle, RegistryReaderLike, RegistrySnapshotLike, ReportFn, SchedulingState,
    TaskInspection, TaskInspectionState,
};
use super::super::ids::{ConversationId, EntryId, TaskId};
use super::super::session::observation::CommittedWatch;
use super::super::session::session::Session;
use super::super::session::transaction::{Transaction, TransactionScope};
use super::super::storage::Storage;
use super::super::tasks::{NextTaskState, PhaseArgs, PlainFailure, TaskDefinition, TaskToken};
use super::super::types::{
    CommitChange as PublicationChange, CommitPublication, JoinPolicy, JsonObject, SubmissionStatus,
    SubmissionType, TableCommitChange, TaskOutcome, TaskOutcomeError, TaskQuery, TaskRecord,
    TaskState, TaskStatus,
};
use super::super::util::{closed_error, WaiterError, Waiters};
use super::context::read_context;
use super::types::{ContextView, HookScope};

/// Longest delay `setTimeout` supports; longer sleeps wait in several steps
/// (`scheduler.ts:51`).
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;
const SCAN_PAGE_SIZE: usize = 256;
/// The live (non-terminal) statuses scanned at open (`LIVE_STATUSES`).
const LIVE_STATUSES: [TaskStatus; 4] = [
    TaskStatus::Pending,
    TaskStatus::Running,
    TaskStatus::Waiting,
    TaskStatus::Completing,
];

/// Outcome of the phase that just returned, judged by the next step
/// (`PhaseResult`).
#[derive(Clone)]
struct PhaseResult {
    checkpoint: Value,
    failure: Option<PlainFailure>,
}

/// Step decision: continue with the next phase, end the invocation, or end it
/// by writing `faulted` (`Decision`).
enum Decision {
    Continue,
    End,
    Fault(String),
}

/// The immutable ownership fields of a task (`TaskNode`).
#[derive(Debug, Clone)]
struct TaskNode {
    conversation_id: ConversationId,
    owner: Option<TaskId>,
    background: bool,
}

/// Where a walk up the ownership tree continues (`Up`).
#[derive(Debug, Clone, Copy)]
enum Up {
    Task(TaskId),
    Conversation(ConversationId),
}

/// One step of a walk up (`Step`).
#[derive(Debug, Clone)]
enum Step {
    Task { task: TaskId, node: TaskNode },
    Conversation(ConversationId),
    Unknown,
}

/// Candidate records a commit staged; they override committed records in
/// ownership walks (`Overlay`).
struct Overlay {
    tasks: HashMap<TaskId, TaskRecord>,
    edges: HashMap<ConversationId, Option<TaskId>>,
}

/// Where ordinary ownership traversal starts (`Scope`).
#[derive(Debug, Clone, Copy)]
enum Scope {
    Conversation(ConversationId),
    Roots,
}

/// One in-memory execution of a task in run or abort mode (`Invocation`).
pub(crate) struct Invocation {
    pub(crate) task_id: TaskId,
    pub(crate) conversation_id: ConversationId,
    pub(crate) mode: InvocationMode,
    pub(crate) controller: CancellationToken,
    /// Context passed to handlers; cancelled by `controller`.
    pub(crate) context: Context,
    /// Watches acquired through the runtime; stopped at invocation end.
    pub(crate) watches: Mutex<Vec<Arc<CommittedWatch>>>,
    ended: AtomicBool,
    done: Arc<Done>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvocationMode {
    Run,
    Abort,
}

/// `Promise.withResolvers` pair shared by every waiter of one invocation.
struct Done {
    finished: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Done {
    fn new() -> Arc<Self> {
        Arc::new(Done {
            finished: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        })
    }

    async fn wait(&self) {
        loop {
            if self.finished.load(Ordering::SeqCst) {
                return;
            }
            let notified = self.notify.notified();
            if self.finished.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    fn finish(&self) {
        self.finished.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

impl Invocation {
    fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

/// An invocation a conversation handle is bound to: its signal, and a check
/// that fails once it ended (`InvocationBinding`).
#[derive(Clone)]
pub struct InvocationBinding {
    pub signal: CancellationToken,
    pub check: Arc<dyn Fn() -> Result<(), PlainError> + Send + Sync>,
}

struct Reservation {
    invocation: Arc<Invocation>,
    task: TaskToken,
    snapshot: Arc<dyn RegistrySnapshotLike>,
}

/// Mutable state of one run invocation, refreshed on progress (`#run`'s
/// `state`).
struct RunState {
    task: TaskToken,
    snapshot: Arc<dyn RegistrySnapshotLike>,
    /// Replacement definition already reported as unable to take over
    /// (`ReportedTask`).
    reported: Option<Option<TaskToken>>,
}

/// Why a pending task cannot be reserved under a registry snapshot; the
/// resolved task when it can (`Resolution`).
enum Resolution {
    Ready {
        task: TaskToken,
        record: Box<TaskRecord>,
    },
    Blocked(BlockedReason),
}

/// A definition that can take a record, or why none can (`Fit`).
enum Fit {
    Task {
        task: TaskToken,
        migrates: bool,
    },
    Blocked {
        reason: BlockedReason,
        error: Option<PlainError>,
    },
}

/// `settleOutcome(tx, record, outcome)`: synchronous over the port's
/// transaction (D5), async upstream.
pub type SettleOutcomeFn =
    Arc<dyn Fn(&Transaction, &TaskRecord, &TaskOutcome) -> Result<(), PlainError> + Send + Sync>;

/// `withdrawInputs(tx, conversationId)`.
pub type WithdrawInputsFn =
    Arc<dyn Fn(&Transaction, ConversationId) -> Result<(), PlainError> + Send + Sync>;

/// `conversation(id, binding, context)`.
pub type ConversationResolverFn = Arc<
    dyn Fn(
            ConversationId,
            InvocationBinding,
            Context,
        ) -> BoxFuture<'static, Result<Option<ConversationHandle>, PlainError>>
        + Send
        + Sync,
>;

/// Scheduler options (`TaskSchedulerOptions`).
pub struct TaskSchedulerOptions {
    pub session: Arc<Session>,
    pub storage: Arc<dyn Storage>,
    pub registry: Arc<dyn RegistryReaderLike>,
    pub models: Option<Arc<dyn ModelsHandle>>,
    pub env: Option<Arc<dyn ExecutionEnv>>,
    pub now: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub report: ReportFn,
    /// Harness cleanup staged in the commit that makes an outcome the
    /// scheduler wrote itself terminal (`settleOutcome`).
    pub settle_outcome: SettleOutcomeFn,
    /// Withdraw a conversation's queued inputs, for conversation abort and
    /// abort cascades (`withdrawInputs`).
    pub withdraw_inputs: WithdrawInputsFn,
    /// Invocation-bound handle of an existing conversation, for task runtimes
    /// and tools (`conversation`).
    pub conversation: ConversationResolverFn,
    /// Context for scheduler commits and invocations; carries no caller
    /// cancellation.
    pub context: Context,
}

/// Durable task scheduler of one Harness (`TaskScheduler`).
///
/// `live` mirrors every committed non-terminal task record: pending, running,
/// waiting, and completing. The synchronous commit listener updates it on the
/// Session line, so code running on the line reads exactly the committed
/// state from it.
pub struct TaskScheduler {
    options: TaskSchedulerOptions,
    self_ref: OnceLock<Weak<TaskScheduler>>,
    live: Mutex<HashMap<TaskId, TaskRecord>>,
    invocations: Mutex<HashMap<TaskId, Arc<Invocation>>>,
    task_waiters: Waiters<TaskRecord>,
    /// Idle waiters by conversation; the empty key waits for the whole
    /// Harness.
    idle_waiters: Waiters<()>,
    /// Definition whose migration failed per task; retried only once the
    /// registry resolves another definition.
    failed_migrations: Mutex<HashMap<TaskId, (TaskToken, PlainError)>>,
    /// Owner task of each loaded conversation, `None` when ownerless; absent
    /// while not loaded.
    edges: Mutex<HashMap<ConversationId, Option<TaskId>>>,
    /// Tasks that own a loaded conversation; their nodes stay in `settled`
    /// once terminal.
    conversation_owners: Mutex<HashSet<TaskId>>,
    /// Ownership fields of terminal tasks that walks pass through.
    settled: Mutex<HashMap<TaskId, TaskNode>>,
    /// `failFast` waiters the next reconcile checks for a failed task in
    /// `on`; insertion-ordered like the upstream `Set`.
    fail_fast_checks: Mutex<Vec<TaskId>>,
    reconcile_scheduled: AtomicBool,
    cascade_pending: AtomicBool,
    unsubscribe_registry: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    enabled: AtomicBool,
    closing: AtomicBool,
    dirty: AtomicBool,
    draining: AtomicBool,
}

/// Idle-scope key of the whole Harness (`undefined` upstream).
const WHOLE_HARNESS_KEY: &str = "";

impl TaskScheduler {
    /// Build the scheduler and install its self reference for spawned passes.
    pub fn new(options: TaskSchedulerOptions) -> Arc<TaskScheduler> {
        let scheduler = Arc::new(TaskScheduler {
            options,
            self_ref: OnceLock::new(),
            live: Mutex::new(HashMap::new()),
            invocations: Mutex::new(HashMap::new()),
            task_waiters: Waiters::new(),
            idle_waiters: Waiters::new(),
            failed_migrations: Mutex::new(HashMap::new()),
            edges: Mutex::new(HashMap::new()),
            conversation_owners: Mutex::new(HashSet::new()),
            settled: Mutex::new(HashMap::new()),
            fail_fast_checks: Mutex::new(Vec::new()),
            reconcile_scheduled: AtomicBool::new(false),
            cascade_pending: AtomicBool::new(false),
            unsubscribe_registry: Mutex::new(None),
            enabled: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            draining: AtomicBool::new(false),
        });
        let _ = scheduler.self_ref.set(Arc::downgrade(&scheduler));
        scheduler
    }

    fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// The session kernel (`#session`).
    fn session(&self) -> &Arc<Session> {
        &self.options.session
    }

    // ─── Live-map helpers (no lock crosses an await) ────────────────────

    fn live_snapshot(&self) -> Vec<TaskRecord> {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect()
    }

    fn live_get(&self, id: TaskId) -> Option<TaskRecord> {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .cloned()
    }

    fn live_insert(&self, record: TaskRecord) {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(record.id, record);
    }

    fn live_has(&self, id: TaskId) -> bool {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&id)
    }

    fn live_remove(&self, id: TaskId) {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
    }

    fn invocations_get(&self, id: TaskId) -> Option<Arc<Invocation>> {
        self.invocations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .cloned()
    }

    fn invocations_remove(&self, invocation: &Arc<Invocation>) {
        let mut invocations = self
            .invocations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if invocations
            .get(&invocation.task_id)
            .is_some_and(|existing| Arc::ptr_eq(existing, invocation))
        {
            invocations.remove(&invocation.task_id);
        }
    }

    fn fail_fast_add(&self, id: TaskId) {
        let mut checks = self
            .fail_fast_checks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !checks.contains(&id) {
            checks.push(id);
        }
    }

    fn fail_fast_take(&self) -> Vec<TaskId> {
        std::mem::take(
            &mut self
                .fail_fast_checks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    // ─── Lifecycle ──────────────────────────────────────────────────────

    /// Load live tasks and change surviving `running` tasks back to `pending`.
    /// Dispatches nothing (`open`).
    pub async fn open(self: &Arc<Self>, context: Context) -> Result<(), PlainError> {
        self.session().subscribe_commits(Arc::new({
            let scheduler = Arc::downgrade(self);
            move |publication: &CommitPublication, _: &Context| {
                if let Some(scheduler) = scheduler.upgrade() {
                    scheduler.observe(publication);
                }
            }
        }))?;
        self.session().subscribe_close(Arc::new({
            let scheduler = Arc::downgrade(self);
            move || {
                if let Some(scheduler) = scheduler.upgrade() {
                    scheduler.seal();
                }
            }
        }))?;
        let unsubscribe = self.options.registry.subscribe(Box::new({
            let scheduler = Arc::downgrade(self);
            move || {
                if let Some(scheduler) = scheduler.upgrade() {
                    scheduler.kick();
                }
            }
        }));
        *self
            .unsubscribe_registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(unsubscribe);
        let scheduler = Arc::clone(self);
        self.session()
            .commit(
                move |tx: Arc<Transaction>| {
                    let scheduler = scheduler.clone();
                    async move {
                        // Every table read before the first write.
                        let mut scans: Vec<Vec<TaskRecord>> = Vec::new();
                        for status in LIVE_STATUSES {
                            let mut items: Vec<TaskRecord> = Vec::new();
                            let mut cursor = None;
                            loop {
                                let page = tx.scan_tasks(
                                    TaskQuery {
                                        status: Some(status),
                                        ..Default::default()
                                    },
                                    SCAN_PAGE_SIZE,
                                    cursor.as_ref(),
                                )?;
                                let next = page.next.clone();
                                items.extend(page.items);
                                cursor = next;
                                if cursor.is_none() {
                                    break;
                                }
                            }
                            scans.push(items);
                        }
                        for records in scans {
                            for record in records {
                                scheduler.live_insert(record.clone());
                                if let TaskState::Running { checkpoint } = &record.state {
                                    tx.set_task(with_state(
                                        &record,
                                        TaskState::Pending {
                                            checkpoint: checkpoint.clone(),
                                        },
                                    ))?;
                                }
                                if let TaskState::Waiting {
                                    policy: JoinPolicy::FailFast,
                                    ..
                                } = &record.state
                                {
                                    scheduler.fail_fast_add(record.id);
                                }
                            }
                        }
                        Ok(())
                    }
                },
                context,
            )
            .await?;
        // Derive abort marks a crash left unapplied below cancelled owners,
        // and finalize held outcomes.
        self.cascade_pending.store(true, Ordering::SeqCst);
        self.schedule_reconcile();
        Ok(())
    }

    /// Enable scheduling. Idempotent; the kick does nothing once closing
    /// (`resume`).
    pub fn resume(self: &Arc<Self>) {
        self.enabled.store(true, Ordering::SeqCst);
        self.kick();
    }

    /// Wait for every invocation signalled by [`TaskScheduler::seal`]. Writes
    /// nothing (`join`).
    pub async fn join(&self) {
        let invocations: Vec<Arc<Invocation>> = self
            .invocations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        for invocation in invocations {
            invocation.done.wait().await;
        }
    }

    /// Commit the abort mark, or settle a task that no registered definition
    /// can take as `orphaned` when nothing it owns is live, then join the run
    /// invocation seen on the line; the commit listener signalled it. The
    /// abort invocation starts once the task's ordinary owned work is gone. A
    /// `completing` task is only marked (`abort`).
    pub async fn abort(
        self: &Arc<Self>,
        id: TaskId,
        context: Context,
    ) -> Result<AbortTaskResult, PlainError> {
        struct Marked {
            result: AbortTaskResult,
            run: Option<Arc<Invocation>>,
        }
        let scheduler = Arc::clone(self);
        let marked = self
            .session()
            .commit(
                move |tx: Arc<Transaction>| {
                    let scheduler = scheduler.clone();
                    async move {
                        let current = tx.task(id)?;
                        let Some(current) = current else {
                            return Err(PlainError::new(format!("Task {id} does not exist")));
                        };
                        if current.status() == TaskStatus::Terminal {
                            return Ok(Marked {
                                result: AbortTaskResult::Terminal,
                                run: None,
                            });
                        }
                        let invocation = scheduler.invocations_get(id);
                        if invocation.is_none() && current.status() != TaskStatus::Completing {
                            scheduler.load_scopes(false)?;
                            if !scheduler.owned_live(None).contains_key(&id) {
                                let snapshot = scheduler.options.registry.snapshot();
                                let resolution = scheduler.resolve(&current, snapshot.as_ref());
                                if let Resolution::Blocked(reason) = resolution {
                                    scheduler.terminate(
                                        tx.as_ref(),
                                        &current,
                                        TaskOutcome::Orphaned {
                                            reason: reason.as_str().to_owned(),
                                        },
                                    )?;
                                    return Ok(Marked {
                                        result: AbortTaskResult::Marked,
                                        run: None,
                                    });
                                }
                            }
                        }
                        if !current.abort_requested {
                            let mut marked = current.clone();
                            marked.abort_requested = true;
                            tx.set_task(marked)?;
                        }
                        Ok(Marked {
                            result: AbortTaskResult::Marked,
                            run: invocation
                                .filter(|invocation| invocation.mode == InvocationMode::Run),
                        })
                    }
                },
                context.clone(),
            )
            .await?;
        // The commit listener signalled the run; join it.
        if let Some(run) = marked.run {
            await_with_abort(run.done.wait(), &context).await?;
        }
        Ok(marked.result)
    }

    /// Resolve with the settled record of `id`, registering on the line so no
    /// terminal publication falls between the check and the registration
    /// (`waitForTask`).
    pub async fn wait_for_task(
        self: &Arc<Self>,
        id: TaskId,
        context: Context,
    ) -> Result<TaskRecord, PlainError> {
        enum Found {
            Wait(super::super::util::WaiterHandle<TaskRecord>),
            Ready(Box<TaskRecord>),
        }
        let scheduler = Arc::clone(self);
        let found = self
            .session()
            .read_on_line(|| {
                let scheduler = scheduler.clone();
                let context = context.clone();
                async move {
                    if scheduler.is_closing() {
                        return Err(PlainError::new(closed_error().to_string()));
                    }
                    if scheduler.live_has(id) {
                        let waiter = scheduler.task_waiters.register(&id.to_string(), &context)?;
                        return Ok(Found::Wait(waiter));
                    }
                    let record = scheduler
                        .options
                        .storage
                        .task(id, &context)
                        .map_err(|error| PlainError::new(error.to_string()))?;
                    let Some(record) = record else {
                        return Err(PlainError::new(format!("Task {id} does not exist")));
                    };
                    Ok(Found::Ready(Box::new(record)))
                }
            })
            .await?;
        match found {
            Found::Wait(waiter) => waiter.wait().await.map_err(waiter_error),
            Found::Ready(record) => Ok(*record),
        }
    }

    /// Resolve when ordinary traversal from the conversation, or from every
    /// ownerless conversation, reaches no live non-background task
    /// (`waitForIdle`).
    pub async fn wait_for_idle(
        self: &Arc<Self>,
        conversation_id: Option<ConversationId>,
        context: Context,
    ) -> Result<(), PlainError> {
        if self.is_closing() {
            return Err(PlainError::new(closed_error().to_string()));
        }
        if self.idle(conversation_id) {
            return Ok(());
        }
        self.schedule_reconcile();
        let key = conversation_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| WHOLE_HARNESS_KEY.to_owned());
        let waiter = self.idle_waiters.register(&key, &context)?;
        waiter.wait().await.map_err(waiter_error)
    }

    /// `Conversation.abort()`: in one commit, withdraw the queued inputs and
    /// mark every live non-background task that ordinary traversal from the
    /// conversation reaches; resolves once the scope is idle. With
    /// `background`, traversal crosses background boundaries, and the wait
    /// also covers every task it reached (`abortConversation`).
    pub async fn abort_conversation(
        self: &Arc<Self>,
        conversation_id: ConversationId,
        background: bool,
        context: Context,
    ) -> Result<(), PlainError> {
        let scheduler = Arc::clone(self);
        let reached = self
            .session()
            .commit(
                move |tx: Arc<Transaction>| {
                    let scheduler = scheduler.clone();
                    async move {
                        let queued = scheduler.load_scopes(true)?;
                        let scope = Scope::Conversation(conversation_id);
                        let mut reached: Vec<TaskId> = Vec::new();
                        for record in scheduler.live_snapshot() {
                            if record.background && !background {
                                continue;
                            }
                            if scheduler.in_scope(record_parent(&record), scope, background)
                                != Some(true)
                            {
                                continue;
                            }
                            reached.push(record.id);
                            if !record.abort_requested {
                                let mut marked = record.clone();
                                marked.abort_requested = true;
                                tx.set_task(marked)?;
                            }
                        }
                        for id in queued {
                            if scheduler.in_scope(Up::Conversation(id), scope, background)
                                == Some(true)
                            {
                                (scheduler.options.withdraw_inputs)(tx.as_ref(), id)?;
                            }
                        }
                        Ok(reached)
                    }
                },
                context.clone(),
            )
            .await?;
        if background {
            for id in reached {
                self.wait_for_task(id, context.clone()).await?;
            }
        }
        self.wait_for_idle(Some(conversation_id), context).await
    }

    // ─── Scheduling ─────────────────────────────────────────────────────

    /// The synchronous commit listener (`#observe`).
    fn observe(self: &Arc<Self>, publication: &CommitPublication) {
        let mut updated: Vec<TaskRecord> = Vec::new();
        let mut failed: Vec<TaskId> = Vec::new();
        let mut changed = false;
        for change in &publication.changes {
            let PublicationChange::Table(TableCommitChange::Task { value: record }) = change else {
                continue;
            };
            changed = true;
            let previous = self.live_get(record.id);
            let previously_failed = previous.as_ref().map(failed_outcome).unwrap_or(false);
            if failed_outcome(record) && !previously_failed {
                failed.push(record.id);
            }
            if record.status() == TaskStatus::Terminal {
                self.live_remove(record.id);
                self.failed_migrations
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&record.id);
                self.fail_fast_checks
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .retain(|id| *id != record.id);
                if self
                    .conversation_owners
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(&record.id)
                {
                    self.settled
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(record.id, node_of(record));
                }
                self.task_waiters
                    .resolve(&record.id.to_string(), record.clone());
                // Its owner may finalize now.
                self.schedule_reconcile();
                continue;
            }
            if record.abort_requested && !previous.as_ref().is_some_and(|p| p.abort_requested) {
                self.cascade_pending.store(true, Ordering::SeqCst);
                // Signal a run invocation of the newly marked task; its next
                // step ends it.
                if let Some(invocation) = self.invocations_get(record.id) {
                    if invocation.mode == InvocationMode::Run {
                        invocation.controller.cancel();
                    }
                }
            }
            let status = record.status();
            if status == TaskStatus::Completing
                && previous.as_ref().map(|p| p.status()) != Some(TaskStatus::Completing)
            {
                if cancellation_intent(record) {
                    self.cascade_pending.store(true, Ordering::SeqCst);
                }
                self.schedule_reconcile();
            }
            if let TaskState::Waiting {
                policy: JoinPolicy::FailFast,
                ..
            } = &record.state
            {
                let previously_waiting = matches!(
                    previous.as_ref().map(|p| p.state.clone()),
                    Some(TaskState::Waiting { .. })
                );
                if !previously_waiting {
                    self.fail_fast_add(record.id);
                    self.schedule_reconcile();
                }
            }
            self.live_insert(record.clone());
            updated.push(record.clone());
        }
        for id in failed {
            for record in self.live_snapshot() {
                if let TaskState::Waiting {
                    on,
                    policy: JoinPolicy::FailFast,
                    ..
                } = &record.state
                {
                    if on.contains(&id) {
                        self.fail_fast_add(record.id);
                        self.schedule_reconcile();
                    }
                }
            }
        }
        for change in &publication.changes {
            if let PublicationChange::Table(TableCommitChange::Conversation { value }) = change {
                if !self
                    .edges
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains_key(&value.id)
                {
                    self.set_edge(value.id, value.owner.as_ref().map(|owner| owner.task_id));
                }
            }
        }
        for change in &publication.changes {
            // A queued input below a cancelled owner is withdrawn, even after
            // its cascade.
            let PublicationChange::Table(TableCommitChange::Submission { value }) = change else {
                continue;
            };
            if value.status != SubmissionStatus::Queued || value.r#type != SubmissionType::Input {
                continue;
            }
            let up = Up::Conversation(value.conversation_id);
            if !self.chain_known(up, None) || self.below_cancelled(up) {
                self.cascade_pending.store(true, Ordering::SeqCst);
            }
        }
        for record in &updated {
            // Work created below a cancelled owner, even after its cascade,
            // is aborted too.
            if !self.chain_known(record_parent(record), None) {
                self.schedule_reconcile();
            } else if !record.background
                && !record.abort_requested
                && self.below_cancelled(record_parent(record))
            {
                self.cascade_pending.store(true, Ordering::SeqCst);
            }
        }
        // Also retries, with the next commit of any kind, a cascade whose
        // commit failed.
        if self.cascade_pending.load(Ordering::SeqCst) {
            self.schedule_reconcile();
        }
        if !changed {
            return;
        }
        self.resolve_idle_waiters();
        self.kick();
    }

    fn resolve_idle_waiters(self: &Arc<Self>) {
        for conversation_id in self.idle_waiters.keys() {
            let scope_id = if conversation_id == WHOLE_HARNESS_KEY {
                None
            } else {
                conversation_id.parse::<ConversationId>().ok()
            };
            if self.idle(scope_id) {
                self.idle_waiters.resolve(&conversation_id, ());
            }
        }
    }

    // ─── Ownership ──────────────────────────────────────────────────────

    fn schedule_reconcile(self: &Arc<Self>) {
        if self.reconcile_scheduled.swap(true, Ordering::SeqCst) || self.is_closing() {
            return;
        }
        let scheduler = Arc::downgrade(self);
        tokio::spawn(async move {
            if let Some(scheduler) = scheduler.upgrade() {
                scheduler.reconcile().await;
            }
        });
    }

    /// One commit that applies what committed records imply: abort marks
    /// below live owners with cancellation intent (spec §5.4), `failFast`
    /// marks (spec §5.5), withdrawn queued inputs below cancelled owners, and
    /// the final terminal record of every `completing` task whose ordinary
    /// owned work is gone (`#reconcile`).
    async fn reconcile(self: Arc<Self>) {
        self.reconcile_scheduled.store(false, Ordering::SeqCst);
        let cascade = self.cascade_pending.swap(false, Ordering::SeqCst);
        let checks = self.fail_fast_take();
        let checks_for_pass = checks.clone();
        let outcome = self
            .session()
            .commit(
                {
                    let scheduler = Arc::clone(&self);
                    let checks = checks_for_pass.clone();
                    move |tx: Arc<Transaction>| {
                        let scheduler = scheduler.clone();
                        let checks = checks.clone();
                        async move {
                            if scheduler.is_closing() {
                                return Ok(());
                            }
                            let queued = scheduler.load_scopes(cascade)?;
                            let marked: Arc<Mutex<HashSet<TaskId>>> = Arc::default();
                            // Loading edges can reveal a cancelled owner, so
                            // marks are derived on every pass.
                            for record in scheduler.live_snapshot() {
                                if !record.background
                                    && scheduler.below_cancelled(record_parent(&record))
                                {
                                    mark(tx.as_ref(), &record, &marked)?;
                                }
                            }
                            for id in &checks {
                                let waiter = scheduler.live_get(*id);
                                let TaskState::Waiting { on, .. } = waiter
                                    .map(|record| record.state)
                                    .unwrap_or(TaskState::Pending {
                                        checkpoint: Value::Null,
                                    })
                                else {
                                    continue;
                                };
                                if !scheduler.any_failed(&on)? {
                                    continue;
                                }
                                // Every other live task: the failed one keeps
                                // its own outcome.
                                for member in on {
                                    if let Some(record) = scheduler.live_get(member) {
                                        if !failed_outcome(&record) {
                                            mark(tx.as_ref(), &record, &marked)?;
                                        }
                                    }
                                }
                            }
                            for id in queued {
                                if scheduler.below_cancelled(Up::Conversation(id)) {
                                    (scheduler.options.withdraw_inputs)(tx.as_ref(), id)?;
                                }
                            }
                            scheduler.finalize(tx.as_ref())?;
                            Ok(())
                        }
                    }
                },
                self.options.context.clone(),
            )
            .await;
        if let Err(error) = outcome {
            // Any pass may have staged marks, so a failed one is retried with
            // the next commit.
            self.cascade_pending.store(true, Ordering::SeqCst);
            for id in checks {
                self.fail_fast_add(id);
            }
            if !self.is_closing() {
                (self.options.report)(&error);
            }
        }
        self.resolve_idle_waiters();
    }

    /// Whether any of `ids` holds or ended with an outcome other than
    /// `completed` (`#anyFailed`).
    fn any_failed(&self, ids: &[TaskId]) -> Result<bool, PlainError> {
        for id in ids {
            let record = match self.live_get(*id) {
                Some(record) => Some(record),
                None => self
                    .options
                    .storage
                    .task(*id, &self.options.context)
                    .map_err(|error| PlainError::new(error.to_string()))?,
            };
            if record.is_some_and(|record| failed_outcome(&record)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Write the terminal record of every `completing` task without live
    /// ordinary owned work. Finalizing one can free its owner, so this
    /// repeats over the commit's candidates until nothing changes (`#finalize`).
    fn finalize(&self, tx: &Transaction) -> Result<(), PlainError> {
        loop {
            let overlay = overlay_of(tx);
            let owned = self.owned_live(Some(&overlay));
            let done: Vec<TaskRecord> = self
                .live_records(Some(&overlay))
                .into_iter()
                .filter(|record| {
                    record.status() == TaskStatus::Completing && !owned.contains_key(&record.id)
                })
                .collect();
            if done.is_empty() {
                return Ok(());
            }
            for record in done {
                let TaskState::Completing { outcome } = record.state.clone() else {
                    continue;
                };
                tx.set_task(with_state(
                    &record,
                    TaskState::Terminal {
                        outcome: outcome.clone(),
                    },
                ))?;
                // REMINDER: only the scheduler writes `faulted` and `orphaned`
                // (spec §5.4); their cleanup waits for this commit.
                if matches!(
                    outcome,
                    TaskOutcome::Faulted { .. } | TaskOutcome::Orphaned { .. }
                ) {
                    (self.options.settle_outcome)(tx, &record, &outcome)?;
                }
            }
        }
    }

    /// Load the owner chains of every live task and, with `queued`, of every
    /// conversation with queued submissions, on the Session line; returns the
    /// latter (`#loadScopes`).
    fn load_scopes(&self, queued: bool) -> Result<Vec<ConversationId>, PlainError> {
        for record in self.live_snapshot() {
            if !self.chain_known(record_parent(&record), None) {
                self.load_chain(record_parent(&record), None)?;
            }
        }
        if !queued {
            return Ok(Vec::new());
        }
        let submissions =
            scan_all_submissions(self.options.storage.as_ref(), &self.options.context)?;
        let mut conversations: Vec<ConversationId> = Vec::new();
        let mut seen: HashSet<ConversationId> = HashSet::new();
        for submission in submissions {
            if seen.insert(submission.conversation_id) {
                conversations.push(submission.conversation_id);
            }
        }
        for id in &conversations {
            self.load_chain(Up::Conversation(*id), None)?;
        }
        Ok(conversations)
    }

    /// Load the owner edges and task nodes from `start` up to its ownerless
    /// root (`#loadChain`).
    fn load_chain(&self, start: Up, overlay: Option<&Overlay>) -> Result<(), PlainError> {
        let mut at = Some(start);
        while let Some(current) = at {
            match current {
                Up::Task(task_id) => {
                    let mut node = self.node(task_id, overlay);
                    if node.is_none() {
                        let record = self
                            .options
                            .storage
                            .task(task_id, &self.options.context)
                            .map_err(|error| PlainError::new(error.to_string()))?;
                        let Some(record) = record else {
                            return Ok(());
                        };
                        let loaded = node_of(&record);
                        if record.status() == TaskStatus::Terminal {
                            self.settled
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .insert(record.id, loaded.clone());
                        }
                        node = Some(loaded);
                    }
                    at = Some(parent_of(&node.expect("loaded above")));
                }
                Up::Conversation(conversation_id) => {
                    let mut edge = self.edge(conversation_id, overlay);
                    if edge.is_none() {
                        let record = self
                            .options
                            .storage
                            .conversation(conversation_id, &self.options.context)
                            .map_err(|error| PlainError::new(error.to_string()))?;
                        let owner = record.and_then(|record| record.owner.map(|o| o.task_id));
                        self.set_edge(conversation_id, owner);
                        edge = Some(owner);
                    }
                    at = match edge {
                        Some(Some(owner)) => Some(Up::Task(owner)),
                        _ => None,
                    };
                }
            }
        }
        Ok(())
    }

    fn set_edge(&self, conversation_id: ConversationId, owner: Option<TaskId>) {
        self.edges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(conversation_id, owner);
        if let Some(owner) = owner {
            self.conversation_owners
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(owner);
        }
    }

    /// Owner task of a conversation, `None` when ownerless, outer `None`
    /// while not loaded (`#edge`).
    fn edge(
        &self,
        conversation_id: ConversationId,
        overlay: Option<&Overlay>,
    ) -> Option<Option<TaskId>> {
        if let Some(overlay) = overlay {
            if let Some(edge) = overlay.edges.get(&conversation_id) {
                return Some(*edge);
            }
        }
        self.edges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&conversation_id)
            .copied()
    }

    fn node(&self, id: TaskId, overlay: Option<&Overlay>) -> Option<TaskNode> {
        if let Some(record) = overlay.and_then(|overlay| overlay.tasks.get(&id)) {
            return Some(node_of(record));
        }
        if let Some(record) = self.live_get(id) {
            return Some(node_of(&record));
        }
        self.settled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .cloned()
    }

    /// Walk up from `start`: owner tasks and conversations, ending at an
    /// ownerless root or an edge not loaded yet (`#above`).
    fn above(&self, start: Up, overlay: Option<&Overlay>) -> Vec<Step> {
        let mut steps = Vec::new();
        let mut at = Some(start);
        while let Some(current) = at {
            match current {
                Up::Task(task_id) => {
                    let Some(node) = self.node(task_id, overlay) else {
                        steps.push(Step::Unknown);
                        return steps;
                    };
                    steps.push(Step::Task {
                        task: task_id,
                        node: node.clone(),
                    });
                    at = Some(parent_of(&node));
                }
                Up::Conversation(conversation_id) => {
                    steps.push(Step::Conversation(conversation_id));
                    match self.edge(conversation_id, overlay) {
                        None => {
                            steps.push(Step::Unknown);
                            return steps;
                        }
                        Some(None) => at = None,
                        Some(Some(owner)) => at = Some(Up::Task(owner)),
                    }
                }
            }
        }
        steps
    }

    /// Whether every owner above `start` is loaded (`#chainKnown`).
    fn chain_known(&self, start: Up, overlay: Option<&Overlay>) -> bool {
        self.above(start, overlay)
            .iter()
            .all(|step| !matches!(step, Step::Unknown))
    }

    /// Live records, with the overlay's candidates replacing committed ones;
    /// terminal candidates are gone (`#liveRecords`).
    fn live_records(&self, overlay: Option<&Overlay>) -> Vec<TaskRecord> {
        let mut records = Vec::new();
        for record in self.live_snapshot() {
            let candidate = overlay
                .and_then(|overlay| overlay.tasks.get(&record.id))
                .cloned()
                .unwrap_or(record);
            if candidate.status() != TaskStatus::Terminal {
                records.push(candidate);
            }
        }
        if let Some(overlay) = overlay {
            for record in overlay.tasks.values() {
                if !self.live_has(record.id) && record.status() != TaskStatus::Terminal {
                    records.push(record.clone());
                }
            }
        }
        records
    }

    /// Every task with live ordinary owned work (spec §5.5), mapped to that
    /// work (`#ownedLive`). Owner chains must be loaded.
    fn owned_live(&self, overlay: Option<&Overlay>) -> HashMap<TaskId, Vec<TaskId>> {
        let mut owned: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
        for record in self.live_records(overlay) {
            if record.background {
                continue;
            }
            for step in self.above(record_parent(&record), overlay) {
                match step {
                    Step::Unknown => break,
                    Step::Conversation(_) => continue,
                    Step::Task { task, node } => {
                        owned.entry(task).or_default().push(record.id);
                        if node.background {
                            break;
                        }
                    }
                }
            }
        }
        owned
    }

    /// Whether ordinary traversal from `scope` reaches `start` (`#inScope`);
    /// `None` while an edge is not loaded.
    fn in_scope(&self, start: Up, scope: Scope, cross_background: bool) -> Option<bool> {
        for step in self.above(start, None) {
            match step {
                Step::Unknown => return None,
                Step::Conversation(conversation_id) => {
                    if let Scope::Conversation(scope_id) = scope {
                        if conversation_id == scope_id {
                            return Some(true);
                        }
                    }
                }
                Step::Task { node, .. } => {
                    if node.background && !cross_background {
                        return Some(false);
                    }
                }
            }
        }
        Some(matches!(scope, Scope::Roots))
    }

    /// Whether a live owner's cancellation intent reaches `start`
    /// (`#belowCancelled`). Terminal owners never cascade (spec §5.4).
    fn below_cancelled(&self, start: Up) -> bool {
        for step in self.above(start, None) {
            match step {
                Step::Unknown => return false,
                Step::Conversation(_) => continue,
                Step::Task { task, node } => {
                    if self
                        .live_get(task)
                        .is_some_and(|live| cancellation_intent(&live))
                    {
                        return true;
                    }
                    if node.background {
                        return false;
                    }
                }
            }
        }
        false
    }

    /// Close listener: runs synchronously once admission is sealed, before
    /// `join()` (`#seal`).
    fn seal(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(unsubscribe) = self
            .unsubscribe_registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            unsubscribe();
        }
        self.task_waiters.reject_all();
        self.idle_waiters.reject_all();
        for invocation in self
            .invocations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>()
        {
            invocation.controller.cancel();
        }
    }

    fn kick(self: &Arc<Self>) {
        self.dirty.store(true, Ordering::SeqCst);
        // Never commit synchronously from a commit or registry listener.
        if self.draining.swap(true, Ordering::SeqCst) {
            return;
        }
        if !self.is_enabled() || self.is_closing() {
            self.draining.store(false, Ordering::SeqCst);
            return;
        }
        let scheduler = Arc::downgrade(self);
        tokio::spawn(async move {
            if let Some(scheduler) = scheduler.upgrade() {
                scheduler.drain().await;
            }
        });
    }

    async fn drain(self: Arc<Self>) {
        let outcome: Result<(), PlainError> = loop {
            if !self.dirty.load(Ordering::SeqCst) || !self.is_enabled() || self.is_closing() {
                break Ok(());
            }
            self.dirty.store(false, Ordering::SeqCst);
            match self.reserve().await {
                Ok(reservations) => {
                    for reservation in reservations {
                        self.start(reservation);
                    }
                }
                Err(error) => break Err(error),
            }
        };
        self.draining.store(false, Ordering::SeqCst);
        if let Err(error) = outcome {
            if !self.is_closing() {
                (self.options.report)(&error);
            }
        }
        // A wakeup that arrived during a failed pass still needs its pass.
        if self.dirty.load(Ordering::SeqCst) {
            self.kick();
        }
    }

    /// Reserve every eligible task in one commit; orphan abort-marked tasks
    /// no definition can take (`#reserve`).
    async fn reserve(self: &Arc<Self>) -> Result<Vec<Reservation>, PlainError> {
        let reservations: Arc<Mutex<Vec<Reservation>>> = Arc::default();
        let outcome = self
            .session()
            .commit(
                {
                    let scheduler = Arc::clone(self);
                    let reservations = Arc::clone(&reservations);
                    move |tx: Arc<Transaction>| {
                        let scheduler = scheduler.clone();
                        let reservations = reservations.clone();
                        async move {
                            if !scheduler.is_enabled() || scheduler.is_closing() {
                                return Ok(());
                            }
                            scheduler.load_scopes(false)?;
                            let owned = scheduler.owned_live(None);
                            // Taken once per pass, and only when some task is
                            // a candidate.
                            let mut snapshot: Option<Arc<dyn RegistrySnapshotLike>> = None;
                            for record in scheduler.live_snapshot() {
                                if scheduler.invocations_get(record.id).is_some()
                                    || !scheduler.waiting_on(&record, &owned).is_empty()
                                {
                                    continue;
                                }
                                if record.status() == TaskStatus::Completing {
                                    continue;
                                }
                                let mode = if record.abort_requested {
                                    InvocationMode::Abort
                                } else {
                                    InvocationMode::Run
                                };
                                let snapshot = snapshot
                                    .get_or_insert_with(|| scheduler.options.registry.snapshot());
                                let resolution = scheduler.resolve(&record, snapshot.as_ref());
                                match resolution {
                                    Resolution::Blocked(reason) => {
                                        if mode == InvocationMode::Abort {
                                            scheduler.terminate(
                                                tx.as_ref(),
                                                &record,
                                                TaskOutcome::Orphaned {
                                                    reason: reason.as_str().to_owned(),
                                                },
                                            )?;
                                        }
                                        continue;
                                    }
                                    Resolution::Ready {
                                        task,
                                        record: runnable,
                                    } => {
                                        if *runnable != record
                                            || record.status() != TaskStatus::Running
                                        {
                                            tx.set_task(with_state(
                                                &runnable,
                                                TaskState::Running {
                                                    checkpoint: record_checkpoint(&runnable)
                                                        .cloned()
                                                        .unwrap_or(Value::Null),
                                                },
                                            ))?;
                                        }
                                        // Registered on the line, so marks and
                                        // later reservations see it and close
                                        // joins it.
                                        let invocation = scheduler.create_invocation(&record, mode);
                                        reservations
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                                            .push(Reservation {
                                                invocation,
                                                task,
                                                snapshot: Arc::clone(snapshot),
                                            });
                                    }
                                }
                            }
                            Ok(())
                        }
                    }
                },
                self.options.context.clone(),
            )
            .await;
        let mut staged = reservations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect::<Vec<_>>();
        match outcome {
            Ok(()) => Ok(staged),
            Err(error) => {
                for reservation in &staged {
                    self.invocations_remove(&reservation.invocation);
                    reservation.invocation.done.finish();
                }
                staged.clear();
                Err(error)
            }
        }
    }

    /// Live tasks a task waits for before its next invocation (`#waitingOn`).
    fn waiting_on(&self, record: &TaskRecord, owned: &HashMap<TaskId, Vec<TaskId>>) -> Vec<TaskId> {
        if record.abort_requested {
            return owned.get(&record.id).cloned().unwrap_or_default();
        }
        let TaskState::Waiting { on, .. } = &record.state else {
            return Vec::new();
        };
        on.iter()
            .filter(|id| self.live_has(**id))
            .copied()
            .collect()
    }

    /// Resolve the record's definition by kind, migrating an older stored
    /// version (`#resolve`).
    fn resolve(&self, record: &TaskRecord, snapshot: &dyn RegistrySnapshotLike) -> Resolution {
        let fit = self.fit(record, snapshot.task(&record.kind));
        let (task, migrates) = match fit {
            Fit::Blocked { reason, .. } => return Resolution::Blocked(reason),
            Fit::Task { task, migrates } => (task, migrates),
        };
        if !migrates {
            return Resolution::Ready {
                task,
                record: Box::new(record.clone()),
            };
        }
        let definition = &task.definition;
        let migrated = (|| {
            let migrate = definition
                .migrate
                .as_ref()
                .ok_or_else(|| missing_migration(record, definition))?;
            let checkpoint = record_checkpoint(record)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let (input, checkpoint) = migrate(&record.input, &checkpoint, record.version)?;
            let mut migrated = record.clone();
            migrated.version = definition.version;
            migrated.input = input;
            migrated.state = match &migrated.state {
                TaskState::Pending { .. } => TaskState::Pending {
                    checkpoint: Value::Object(checkpoint),
                },
                TaskState::Running { .. } => TaskState::Running {
                    checkpoint: Value::Object(checkpoint),
                },
                TaskState::Waiting { on, policy, .. } => TaskState::Waiting {
                    checkpoint: Value::Object(checkpoint),
                    on: on.clone(),
                    policy: *policy,
                },
                TaskState::Completing { .. } | TaskState::Terminal { .. } => {
                    return Err(PlainFailure::new(
                        "migrating a terminal task is not supported",
                    ));
                }
            };
            Ok(migrated)
        })();
        match migrated {
            Ok(migrated) => Resolution::Ready {
                task,
                record: Box::new(migrated),
            },
            Err(error) => {
                self.failed_migrations
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(record.id, (task, PlainError::new(error.message.clone())));
                (self.options.report)(&PlainError::new(error.message.clone()));
                Resolution::Blocked(BlockedReason::MigrationFailed)
            }
        }
    }

    fn fit(&self, record: &TaskRecord, task: Option<TaskToken>) -> Fit {
        let Some(task) = task else {
            return Fit::Blocked {
                reason: BlockedReason::MissingTask,
                error: None,
            };
        };
        let version = task.definition.version;
        if version == record.version {
            return Fit::Task {
                task,
                migrates: false,
            };
        }
        if version < record.version {
            return Fit::Blocked {
                reason: BlockedReason::TaskTooOld,
                error: None,
            };
        }
        if let Some((failed_task, error)) = self
            .failed_migrations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&record.id)
        {
            if Arc::ptr_eq(&failed_task.definition, &task.definition) {
                return Fit::Blocked {
                    reason: BlockedReason::MigrationFailed,
                    error: Some(error.clone()),
                };
            }
        }
        Fit::Task {
            task,
            migrates: true,
        }
    }

    /// Scheduling state and every live task with its derived state, read on
    /// the Session line (`inspect`). Runs no task code.
    pub async fn inspect(
        self: &Arc<Self>,
        snapshot: Arc<dyn RegistrySnapshotLike>,
    ) -> Result<(SchedulingState, Vec<TaskInspection>), PlainError> {
        self.load_scopes(false)?;
        let owned = self.owned_live(None);
        let tasks: Vec<TaskInspection> = self
            .live_snapshot()
            .into_iter()
            .map(|record| {
                let state = self.inspect_task(&record, snapshot.as_ref(), &owned);
                TaskInspection { record, state }
            })
            .collect();
        let scheduling = if self.is_closing() {
            SchedulingState::Closing
        } else if self.is_enabled() {
            SchedulingState::Running
        } else {
            SchedulingState::Paused
        };
        Ok((scheduling, tasks))
    }

    fn inspect_task(
        &self,
        record: &TaskRecord,
        snapshot: &dyn RegistrySnapshotLike,
        owned: &HashMap<TaskId, Vec<TaskId>>,
    ) -> TaskInspectionState {
        if self.invocations_get(record.id).is_some() {
            return TaskInspectionState::Running;
        }
        if record.status() == TaskStatus::Completing {
            return TaskInspectionState::Completing;
        }
        let on = self.waiting_on(record, owned);
        if !on.is_empty() {
            return TaskInspectionState::Waiting { on };
        }
        match self.fit(record, snapshot.task(&record.kind)) {
            Fit::Blocked { reason, error } => TaskInspectionState::Blocked { reason, error },
            Fit::Task { task, migrates } => {
                if migrates && task.definition.migrate.is_none() {
                    return TaskInspectionState::Blocked {
                        reason: BlockedReason::MigrationFailed,
                        error: Some(PlainError::new(
                            missing_migration(record, &task.definition).message,
                        )),
                    };
                }
                TaskInspectionState::Ready { migrates }
            }
        }
    }

    fn create_invocation(&self, record: &TaskRecord, mode: InvocationMode) -> Arc<Invocation> {
        let controller = CancellationToken::new();
        let invocation = Arc::new(Invocation {
            task_id: record.id,
            conversation_id: record.conversation_id,
            mode,
            controller: controller.clone(),
            context: with_abort_signal(self.options.context.clone(), controller),
            watches: Mutex::new(Vec::new()),
            ended: AtomicBool::new(false),
            done: Done::new(),
        });
        self.invocations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(record.id, Arc::clone(&invocation));
        invocation
    }

    fn start(self: &Arc<Self>, reservation: Reservation) {
        let scheduler = Arc::clone(self);
        tokio::spawn(async move {
            let invocation = Arc::clone(&reservation.invocation);
            let outcome = if invocation.mode == InvocationMode::Run {
                scheduler.run(&reservation).await
            } else {
                scheduler.run_abort(&reservation).await
            };
            if let Err(error) = outcome {
                (scheduler.options.report)(&error);
            }
            scheduler.end(&invocation);
            invocation.done.finish();
            scheduler.kick();
        });
    }

    /// Run phase handlers, each preceded by a step that decides on the line
    /// whether the invocation continues (`#run`).
    async fn run(self: &Arc<Self>, reservation: &Reservation) -> Result<(), PlainError> {
        let invocation = &reservation.invocation;
        let state = Arc::new(Mutex::new(RunState {
            task: reservation.task.clone(),
            snapshot: Arc::clone(&reservation.snapshot),
            reported: None,
        }));
        let snapshot_fn: SharedSnapshotFn = {
            let state = Arc::clone(&state);
            Arc::new(move || {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .snapshot
                    .clone()
            })
        };
        let task_fn: SharedTaskFn = {
            let state = Arc::clone(&state);
            Arc::new(move || {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .task
                    .clone()
            })
        };
        let runtime: Arc<dyn super::super::tasks::TaskRuntimeLike> = Arc::new(InvocationRuntime {
            scheduler: Arc::clone(self),
            invocation: Arc::clone(invocation),
            snapshot: snapshot_fn,
            task: task_fn,
        });
        let mut previous: Option<PhaseResult> = None;
        loop {
            let decide: StepDecide = {
                let scheduler = Arc::clone(self);
                let state = Arc::clone(&state);
                let previous = previous.clone();
                Box::new(move |tx: &Transaction, current: &TaskRecord| {
                    scheduler.decide(tx, current, previous, &state)
                })
            };
            // Close may seal between the decision and dispatch.
            let Some(current) = self.step(invocation, decide).await? else {
                return Ok(());
            };
            if self.is_closing() {
                return Ok(());
            }
            let checkpoint = record_checkpoint(&current).cloned().unwrap_or(Value::Null);
            let phase = checkpoint
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let phase_fn = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .task
                .definition
                .phases
                .get(&phase)
                .cloned();
            let outcome = match phase_fn {
                Some(phase_fn) => {
                    phase_fn(PhaseArgs {
                        record: current.clone(),
                        runtime: Arc::clone(&runtime),
                        context: invocation.context.clone(),
                    })
                    .await
                }
                None => Err(PlainFailure::new(format!(
                    "Task {kind} phases.{phase} is not a function",
                    kind = current.kind
                ))),
            };
            previous = Some(match outcome {
                Ok(()) => PhaseResult {
                    checkpoint,
                    failure: None,
                },
                Err(failure) => PhaseResult {
                    checkpoint,
                    failure: Some(failure),
                },
            });
        }
    }

    /// Precedence rules for a run invocation, on the line (`#decide`). Rules
    /// 1 (terminal, `completing`, or `waiting`) and 2 (closing) are applied
    /// by [`TaskScheduler::step`].
    fn decide(
        self: &Arc<Self>,
        tx: &Transaction,
        current: &TaskRecord,
        previous: Option<PhaseResult>,
        state: &Arc<Mutex<RunState>>,
    ) -> Decision {
        // 3. abort mark: end; a fresh abort invocation starts once the task's
        // ordinary owned work is gone.
        if current.abort_requested {
            return Decision::End;
        }
        let Some(previous) = previous else {
            return Decision::Continue;
        };
        // 4. uncaught error.
        if let Some(failure) = previous.failure {
            return Decision::Fault(failure.message);
        }
        // 6. no durable progress.
        if json_equal(
            record_checkpoint(current).unwrap_or(&Value::Null),
            &previous.checkpoint,
        ) {
            return Decision::Fault(format!(
                "Task {} phase {} returned without durable progress",
                current.kind,
                previous.phase_name()
            ));
        }
        // 5. progress: refresh the snapshot; hand over to a replacement
        // definition that can take the task.
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.snapshot = self.options.registry.snapshot();
        let next = state.snapshot.task(&current.kind);
        let replaced = !next
            .as_ref()
            .is_some_and(|next| Arc::ptr_eq(&next.definition, &state.task.definition));
        if replaced {
            if let Some(next) = &next {
                if can_reserve(next, current) {
                    let _ = tx.set_task(with_state(
                        current,
                        TaskState::Pending {
                            checkpoint: record_checkpoint(current).cloned().unwrap_or(Value::Null),
                        },
                    ));
                    return Decision::End;
                }
            }
            let same_as_reported = match (
                state.reported.as_ref().and_then(|token| token.as_ref()),
                next.as_ref(),
            ) {
                (Some(previous), Some(next)) => Arc::ptr_eq(&previous.definition, &next.definition),
                (None, None) => true,
                _ => false,
            };
            if !same_as_reported {
                state.reported = Some(next.clone());
                (self.options.report)(&PlainError::new(format!(
                    "Task {} keeps running under its old {} definition",
                    current.id, current.kind
                )));
            }
        }
        Decision::Continue
    }

    /// Run the abort handler once; rules 1, 2, and 4 apply, and returning
    /// without an outcome faults (`#runAbort`).
    async fn run_abort(self: &Arc<Self>, reservation: &Reservation) -> Result<(), PlainError> {
        let invocation = &reservation.invocation;
        let current = self
            .live_get(invocation.task_id)
            .filter(|record| record.status() == TaskStatus::Running);
        let Some(current) = current else {
            return Ok(());
        };
        if self.is_closing() {
            return Ok(());
        }
        let snapshot_fn: SharedSnapshotFn = {
            let snapshot = Arc::clone(&reservation.snapshot);
            Arc::new(move || snapshot.clone())
        };
        let task_fn: SharedTaskFn = {
            let task = reservation.task.clone();
            Arc::new(move || task.clone())
        };
        let runtime: Arc<dyn super::super::tasks::TaskRuntimeLike> = Arc::new(InvocationRuntime {
            scheduler: Arc::clone(self),
            invocation: Arc::clone(invocation),
            snapshot: snapshot_fn,
            task: task_fn,
        });
        let outcome = match reservation.task.definition.abort.as_ref() {
            Some(abort) => {
                abort(PhaseArgs {
                    record: current,
                    runtime: Arc::clone(&runtime),
                    context: invocation.context.clone(),
                })
                .await
            }
            // D21: a missing handler fails like the upstream null call.
            None => Err(PlainFailure::new("Task definition abort is not a function")),
        };
        let message = match outcome {
            Ok(()) => format!(
                "Abort handler of task {} returned without a terminal outcome",
                invocation.task_id
            ),
            Err(failure) => failure.message,
        };
        self.step(invocation, Box::new(|_, _| Decision::Fault(message)))
            .await?;
        Ok(())
    }

    /// One synchronous decision on the Session line (`#step`). A task that is
    /// no longer running (rule 1) or a closing Harness (rule 2) ends the
    /// invocation without a write; otherwise `decide` may stage a write and
    /// returns whether the invocation continues. Ending happens inside the
    /// callback, before a fault's Harness cleanup. A rejected step, such as
    /// admission after close, also ends the invocation.
    async fn step(
        self: &Arc<Self>,
        invocation: &Arc<Invocation>,
        decide: StepDecide,
    ) -> Result<Option<TaskRecord>, PlainError> {
        let scheduler = Arc::clone(self);
        let boxed = Arc::clone(invocation);
        let outcome = self
            .session()
            .commit(
                move |tx: Arc<Transaction>| {
                    let scheduler = scheduler.clone();
                    let boxed = boxed.clone();
                    async move {
                        let found = scheduler.live_get(boxed.task_id);
                        let current = found.filter(|record| record.status() == TaskStatus::Running);
                        let decision = match &current {
                            Some(current) if !scheduler.is_closing() => {
                                decide(tx.as_ref(), current)
                            }
                            _ => Decision::End,
                        };
                        match decision {
                            Decision::Continue => Ok(current),
                            Decision::End => {
                                scheduler.end(&boxed);
                                Ok(None)
                            }
                            Decision::Fault(message) => {
                                scheduler.end(&boxed);
                                let current =
                                    current.expect("a fault decides over a running record");
                                scheduler.terminate(
                                    tx.as_ref(),
                                    &current,
                                    TaskOutcome::Faulted {
                                        error: TaskOutcomeError {
                                            message,
                                            detail: None,
                                        },
                                    },
                                )?;
                                Ok(None)
                            }
                        }
                    }
                },
                self.options.context.clone(),
            )
            .await;
        match outcome {
            Ok(current) => Ok(current),
            Err(error) => {
                self.end(invocation);
                if !self.is_closing() {
                    (self.options.report)(&error);
                }
                Ok(None)
            }
        }
    }

    /// Write an outcome the scheduler decided. While the task's ordinary
    /// owned work is live it holds as `completing` and its Harness cleanup
    /// waits for the final commit (spec §5.5, rule 4); otherwise it is
    /// terminal with its cleanup (`#terminate`).
    fn terminate(
        self: &Arc<Self>,
        tx: &Transaction,
        record: &TaskRecord,
        outcome: TaskOutcome,
    ) -> Result<(), PlainError> {
        self.load_scopes(false)?;
        let overlay = overlay_of(tx);
        if self.owned_live(Some(&overlay)).contains_key(&record.id) {
            tx.set_task(with_state(
                record,
                TaskState::Completing {
                    outcome: outcome.clone(),
                },
            ))?;
            return Ok(());
        }
        tx.set_task(with_state(
            record,
            TaskState::Terminal {
                outcome: outcome.clone(),
            },
        ))?;
        (self.options.settle_outcome)(tx, record, &outcome)
    }

    /// Replace a running task's state with what it committed (`#commitState`).
    /// A terminal state holds as `completing` while ordinary owned work is
    /// live, judged on the commit's candidates. A wait is validated first.
    fn commit_state(
        self: &Arc<Self>,
        tx: &Transaction,
        invocation: &Arc<Invocation>,
        current: &TaskRecord,
        next: NextTaskState,
    ) -> Result<(), PlainError> {
        if let NextTaskState::Waiting { on, policy, .. } = &next {
            self.validate_wait(tx, invocation, current, on, *policy)?;
        }
        let checkpoint = match &next {
            NextTaskState::Pending { checkpoint }
            | NextTaskState::Running { checkpoint }
            | NextTaskState::Waiting { checkpoint, .. } => checkpoint.clone(),
            NextTaskState::Terminal { outcome } => {
                let overlay = overlay_of(tx);
                self.load_scopes(false)?;
                for record in overlay.tasks.values().collect::<Vec<_>>() {
                    self.load_chain(record_parent(record), Some(&overlay))?;
                }
                if self.owned_live(Some(&overlay)).contains_key(&current.id) {
                    tx.set_task(with_state(
                        current,
                        TaskState::Completing {
                            outcome: outcome.clone(),
                        },
                    ))?;
                    return Ok(());
                }
                // Terminal below.
                tx.set_task(with_state(
                    current,
                    TaskState::Terminal {
                        outcome: outcome.clone(),
                    },
                ))?;
                return Ok(());
            }
        };
        tx.set_task(with_state(
            current,
            match &next {
                NextTaskState::Pending { .. } => TaskState::Pending { checkpoint },
                NextTaskState::Running { .. } => TaskState::Running { checkpoint },
                NextTaskState::Waiting { on, policy, .. } => TaskState::Waiting {
                    checkpoint,
                    on: on.clone(),
                    policy: *policy,
                },
                NextTaskState::Terminal { .. } => unreachable!("terminal handled above"),
            },
        ))
    }

    /// A wait names existing tasks other than the waiter and its owners,
    /// which could never finish first; `failFast` only tasks the waiter owns.
    /// An abort handler cannot wait (`#validateWait`).
    fn validate_wait(
        self: &Arc<Self>,
        tx: &Transaction,
        invocation: &Arc<Invocation>,
        current: &TaskRecord,
        on: &[TaskId],
        policy: JoinPolicy,
    ) -> Result<(), PlainError> {
        if invocation.mode == InvocationMode::Abort {
            return Err(PlainError::new(format!(
                "Abort handler of task {} cannot wait",
                current.id
            )));
        }
        let overlay = overlay_of(tx);
        // Owner chains are loaded synchronously where storage allows; the
        // port's storage is synchronous (D3), so the upstream awaits
        // collapse.
        let owners: HashSet<TaskId> = self
            .above(record_parent(current), Some(&overlay))
            .iter()
            .filter_map(|step| match step {
                Step::Task { task, .. } => Some(*task),
                _ => None,
            })
            .collect();
        for id in on {
            if *id == current.id || owners.contains(id) {
                return Err(PlainError::new(format!(
                    "Task {} cannot wait on itself or its owner {}",
                    current.id, id
                )));
            }
            let member = overlay
                .tasks
                .get(id)
                .cloned()
                .or_else(|| self.live_get(*id))
                .or_else(|| {
                    self.options
                        .storage
                        .task(*id, &self.options.context)
                        .ok()
                        .flatten()
                });
            if member.is_none() {
                return Err(PlainError::new(format!("Task {id} does not exist")));
            }
            if policy == JoinPolicy::FailFast
                && member
                    .as_ref()
                    .is_some_and(|member| member.owner != Some(current.id))
            {
                return Err(PlainError::new(format!(
                    "Task {} can wait failFast only on tasks it owns; {} is not one",
                    current.id, id
                )));
            }
        }
        Ok(())
    }

    /// End an invocation: its runtime operations reject from now on, its
    /// signal aborts, its watches stop, and its task is free (`#end`).
    fn end(self: &Arc<Self>, invocation: &Arc<Invocation>) {
        if invocation.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        self.invocations_remove(invocation);
        for watch in invocation
            .watches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
        {
            let _ = watch.stop();
        }
        // Pending waits bound to the invocation, such as a tool's
        // waitForTask(), reject with it.
        invocation.controller.cancel();
    }

    /// No live non-background task in the scope; a task whose owner edges are
    /// not loaded yet counts as inside (`#idle`).
    fn idle(&self, conversation_id: Option<ConversationId>) -> bool {
        let scope = match conversation_id {
            None => Scope::Roots,
            Some(conversation_id) => Scope::Conversation(conversation_id),
        };
        for record in self.live_snapshot() {
            if !record.background
                && self.in_scope(record_parent(&record), scope, false) != Some(false)
            {
                return false;
            }
        }
        true
    }
}

// ─── Invocation runtime ──────────────────────────────────────────────────────

// ─── Invocation runtime ──────────────────────────────────────────────────────

/// The invocation-bound runtime the scheduler hands each phase (`#runtime`,
/// upstream `TaskRuntime`). One struct per invocation; every operation
/// rejects once the invocation has ended.
struct InvocationRuntime {
    scheduler: Arc<TaskScheduler>,
    invocation: Arc<Invocation>,
    snapshot: SharedSnapshotFn,
    task: SharedTaskFn,
}

impl InvocationRuntime {
    /// `runtime.hooks` body (`hooks.each`).
    async fn hooks_each(
        &self,
        name: &str,
        invoke: super::super::tasks::HookInvoke,
    ) -> Result<(), PlainError> {
        let invocation = &self.invocation;
        if invocation.ended() {
            return Err(ended_error(invocation));
        }
        let task_name = (self.task)().definition.name.clone();
        for registration in (self.snapshot)().hooks(&task_name) {
            let HookRegistration { handlers, scope } = registration;
            let Some(handler) = handlers.get(name) else {
                continue;
            };
            if let Some(scope) = scope {
                if !self.scheduler.hook_matches(invocation, scope)? {
                    continue;
                }
            }
            match invoke(handler).await {
                Ok(_) => {}
                Err(error) => {
                    if invocation.controller.is_cancelled() {
                        return Err(error);
                    }
                    (self.scheduler.options.report)(&error);
                }
            }
        }
        Ok(())
    }

    /// `runtime.commit` body.
    async fn commit(
        &self,
        change: super::super::tasks::CommitChange,
        context: Context,
    ) -> Result<(), PlainError> {
        let scheduler = Arc::clone(&self.scheduler);
        let invocation = Arc::clone(&self.invocation);
        self.scheduler
            .gated(
                &self.invocation,
                Box::new(move |tx, current| {
                    let scheduler = Arc::clone(&scheduler);
                    let invocation = Arc::clone(&invocation);
                    let next = change(tx, current)?;
                    if let Some(next) = next {
                        scheduler.commit_state(tx, &invocation, current, next)?;
                    }
                    Ok(())
                }),
                context,
            )
            .await
    }

    /// `runtime.memo` body: the read form and the claim form (D17).
    async fn memo(
        &self,
        name: &str,
        candidate: Option<Value>,
        context: Context,
    ) -> Result<Option<Value>, PlainError> {
        let scheduler = Arc::clone(&self.scheduler);
        let _invocation = Arc::clone(&self.invocation);
        let name = name.to_owned();
        match candidate {
            None => {
                // Read: the caller's context plays no role (D17).
                scheduler
                    .read(&self.invocation, || {
                        let current = scheduler.live_get(self.invocation.task_id);
                        async move { Ok(memo_of(current.as_ref(), &name)) }
                    })
                    .await
            }
            Some(candidate) => {
                scheduler
                    .gated(
                        &self.invocation,
                        Box::new(move |tx, current| {
                            let name = name.clone();
                            let candidate = candidate.clone();
                            if let Some(winner) = memo_of(Some(current), &name) {
                                return Ok(Some(winner));
                            }
                            let mut memos = current.memos.clone().unwrap_or_default();
                            memos.insert(name, candidate.clone());
                            let mut updated = current.clone();
                            updated.memos = Some(memos);
                            tx.set_task(updated)?;
                            Ok(Some(candidate))
                        }),
                        context,
                    )
                    .await
            }
        }
    }

    /// `runtime.sleep` body.
    async fn sleep(&self, until: f64, context: Context) -> Result<(), PlainError> {
        self.scheduler.sleep(&self.invocation, until, context).await
    }

    /// `runtime.watchDoc` body.
    async fn watch_doc(
        &self,
        definition: &super::super::documents::DocDefinition,
        owner: Option<ConversationId>,
        key: Option<&str>,
        context: Context,
    ) -> Result<Option<Arc<CommittedWatch>>, PlainError> {
        let invocation = &self.invocation;
        if invocation.ended() {
            return Err(ended_error(invocation));
        }
        let watch = self
            .scheduler
            .options
            .session
            .watch_doc(definition, owner, key, context)
            .await?;
        let Some(watch) = watch else {
            return Ok(None);
        };
        if invocation.ended() {
            let _ = watch.stop();
            return Err(ended_error(invocation));
        }
        invocation
            .watches
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Arc::clone(&watch));
        let tracked = Arc::clone(&watch);
        let invocation = Arc::clone(invocation);
        tokio::spawn(async move {
            let _ = tracked.closed().await;
            invocation
                .watches
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .retain(|candidate| !Arc::ptr_eq(candidate, &tracked));
        });
        Ok(Some(watch))
    }
}

impl super::super::tasks::TaskRuntimeLike for InvocationRuntime {
    fn task_id(&self) -> TaskId {
        self.invocation.task_id
    }

    fn conversation_id(&self) -> ConversationId {
        self.invocation.conversation_id
    }

    fn signal(&self) -> CancellationToken {
        self.invocation.controller.clone()
    }

    fn aborted(&self) -> bool {
        self.invocation.controller.is_cancelled()
    }

    fn throw_if_aborted(&self) -> Result<(), PlainFailure> {
        if self.invocation.controller.is_cancelled() {
            return Err(PlainFailure::new("The operation was aborted"));
        }
        Ok(())
    }

    fn models(&self) -> Option<Arc<dyn ModelsHandle>> {
        self.scheduler.options.models.clone()
    }

    fn env(&self) -> Option<Arc<dyn super::super::env::ExecutionEnv>> {
        self.scheduler.options.env.clone()
    }

    fn hooks(
        &self,
        name: &str,
        invoke: super::super::tasks::HookInvoke,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + '_>> {
        let name = name.to_owned();
        Box::pin(async move { self.hooks_each(&name, invoke).await })
    }

    fn registry(&self) -> Arc<dyn RegistrySnapshotLike> {
        (self.snapshot)()
    }

    fn commit(
        &self,
        change: super::super::tasks::CommitChange,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + '_>> {
        Box::pin(self.commit(change, context))
    }

    fn memo(
        &self,
        name: &str,
        candidate: Option<Value>,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Value>, PlainError>> + Send + '_>> {
        let name = name.to_owned();
        Box::pin(async move { self.memo(&name, candidate, context).await })
    }

    fn sleep(
        &self,
        until: f64,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlainError>> + Send + '_>> {
        Box::pin(self.sleep(until, context))
    }

    fn watch_doc(
        &self,
        definition: &super::super::documents::DocDefinition,
        owner: Option<ConversationId>,
        key: Option<&str>,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Arc<CommittedWatch>>, PlainError>> + Send + '_>>
    {
        let definition = definition.clone();
        let key = key.map(str::to_owned);
        Box::pin(async move {
            self.watch_doc(&definition, owner, key.as_deref(), context)
                .await
        })
    }

    fn snapshot(
        &self,
        definition: &super::super::documents::DocDefinition,
        owner: Option<ConversationId>,
        key: Option<&str>,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JsonObject>, PlainError>> + Send + '_>> {
        let session = Arc::clone(&self.scheduler.options.session);
        let definition = definition.clone();
        let key = key.map(str::to_owned);
        Box::pin(async move {
            session
                .snapshot(&definition, owner, key.as_deref(), context)
                .await
        })
    }

    fn snapshot_as_of(
        &self,
        definition: &super::super::documents::DocDefinition,
        conversation_id: ConversationId,
        key: Option<&str>,
        at: i64,
        context: Context,
    ) -> Pin<Box<dyn Future<Output = Result<Option<JsonObject>, PlainError>> + Send + '_>> {
        let session = Arc::clone(&self.scheduler.options.session);
        let definition = definition.clone();
        let key = key.map(str::to_owned);
        Box::pin(async move {
            session
                .snapshot_as_of(&definition, conversation_id, key.as_deref(), at, context)
                .await
        })
    }

    fn get_task(&self, id: TaskId, context: Context) -> ApiFuture<Option<TaskRecord>> {
        let scheduler = Arc::clone(&self.scheduler);
        let invocation = Arc::clone(&self.invocation);
        Box::pin(async move {
            if invocation.ended() {
                return Err(ended_error(&invocation));
            }
            scheduler
                .options
                .session
                .read_on_line(|| {
                    let storage = Arc::clone(&scheduler.options.storage);
                    let context = context.clone();
                    async move { storage.task(id, &context).map_err(storage_error) }
                })
                .await
        })
    }

    fn wait_for_task(&self, id: TaskId, context: Context) -> ApiFuture<TaskRecord> {
        let scheduler = Arc::clone(&self.scheduler);
        let invocation = Arc::clone(&self.invocation);
        Box::pin(async move {
            scheduler
                .read(&invocation, || {
                    let bound = with_abort_signal(context.clone(), invocation.controller.clone());
                    let scheduler = Arc::clone(&scheduler);
                    async move { scheduler.wait_for_task(id, bound).await }
                })
                .await
        })
    }

    fn outcomes(&self, ids: Vec<TaskId>, context: Context) -> ApiFuture<Vec<TaskOutcome>> {
        let scheduler = Arc::clone(&self.scheduler);
        let invocation = Arc::clone(&self.invocation);
        Box::pin(async move {
            scheduler
                .read(&invocation, || {
                    let scheduler = Arc::clone(&scheduler);
                    let context = context.clone();
                    async move {
                        scheduler
                            .options
                            .session
                            .read_on_line(|| {
                                let scheduler = Arc::clone(&scheduler);
                                let context = context.clone();
                                async move {
                                    let mut outcomes = Vec::new();
                                    for id in &ids {
                                        let record = scheduler
                                            .options
                                            .storage
                                            .task(*id, &context)
                                            .map_err(storage_error)?;
                                        if !matches!(
                                            record.as_ref().map(|record| record.status()),
                                            Some(TaskStatus::Terminal)
                                        ) {
                                            return Err(PlainError::new(format!(
                                                "Task {id} is not terminal"
                                            )));
                                        }
                                        if let Some(TaskState::Terminal { outcome }) =
                                            record.map(|record| record.state)
                                        {
                                            outcomes.push(outcome);
                                        }
                                    }
                                    Ok(outcomes)
                                }
                            })
                            .await
                    }
                })
                .await
        })
    }

    fn conversation(
        &self,
        id: ConversationId,
        context: Context,
    ) -> ApiFuture<Option<ConversationHandle>> {
        let scheduler = Arc::clone(&self.scheduler);
        let invocation = Arc::clone(&self.invocation);
        Box::pin(async move {
            scheduler
                .read(&invocation, || {
                    let scheduler = Arc::clone(&scheduler);
                    let invocation = Arc::clone(&invocation);
                    async move {
                        let binding = InvocationBinding {
                            signal: invocation.controller.clone(),
                            check: {
                                let invocation = Arc::clone(&invocation);
                                Arc::new(move || {
                                    if invocation.ended() {
                                        return Err(ended_error(&invocation));
                                    }
                                    Ok(())
                                })
                            },
                        };
                        (scheduler.options.conversation)(id, binding, context).await
                    }
                })
                .await
        })
    }

    fn entry(
        &self,
        kind: Option<String>,
        id: EntryId,
        context: Context,
    ) -> ApiFuture<Option<super::super::types::EntryRecord>> {
        let scheduler = Arc::clone(&self.scheduler);
        let invocation = Arc::clone(&self.invocation);
        Box::pin(async move {
            scheduler
                .read(&invocation, || {
                    let scheduler = Arc::clone(&scheduler);
                    let _conversation_id = invocation.conversation_id;
                    async move {
                        scheduler
                            .options
                            .session
                            .read_on_line(|| {
                                let scheduler = Arc::clone(&scheduler);
                                let context = context.clone();
                                async move {
                                    let found = scheduler
                                        .options
                                        .storage
                                        .entry(id, &context)
                                        .map_err(storage_error)?;
                                    Ok(found.map(|found| found.entry))
                                }
                            })
                            .await
                            .map(|entry| match entry {
                                Some(entry)
                                    if kind.as_deref().is_none_or(|kind| entry.kind == kind) =>
                                {
                                    Some(entry)
                                }
                                _ => None,
                            })
                    }
                })
                .await
        })
    }

    fn context(
        &self,
        conversation_id: ConversationId,
        context: Context,
        at: Option<EntryId>,
    ) -> ApiFuture<ContextView> {
        let scheduler = Arc::clone(&self.scheduler);
        let invocation = Arc::clone(&self.invocation);
        Box::pin(async move {
            scheduler
                .read(&invocation, || {
                    let scheduler = Arc::clone(&scheduler);
                    async move {
                        read_context(
                            &scheduler.options.session,
                            scheduler.options.storage.as_ref(),
                            conversation_id,
                            &context,
                            at,
                        )
                        .await
                    }
                })
                .await
        })
    }

    fn now(&self) -> f64 {
        (self.scheduler.options.now)()
    }

    fn report(&self, error: &PlainError) {
        (self.scheduler.options.report)(error);
    }

    fn session(&self) -> Arc<Session> {
        Arc::clone(&self.scheduler.options.session)
    }
}

impl TaskScheduler {
    /// Whether a scoped hook registration matches the invocation's
    /// conversation: itself, or an owner for `subtree` (`#hookMatches`).
    fn hook_matches(
        self: &Arc<Self>,
        invocation: &Arc<Invocation>,
        scope: HookScope,
    ) -> Result<bool, PlainError> {
        if scope.conversation_id == invocation.conversation_id {
            return Ok(true);
        }
        if !scope.subtree {
            return Ok(false);
        }
        let start = Up::Conversation(invocation.conversation_id);
        if !self.chain_known(start, None) {
            self.load_chain(start, None)?;
        }
        for step in self.above(start, None) {
            if let Step::Conversation(conversation_id) = step {
                if conversation_id == scope.conversation_id {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Run a committed-state read unless the invocation has ended (`#read`).
    async fn read<T, Fut, F>(&self, invocation: &Arc<Invocation>, read: F) -> Result<T, PlainError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, PlainError>> + Send + 'static,
    {
        if invocation.ended() {
            return Err(ended_error(invocation));
        }
        read().await
    }

    /// Commit after rereading the task on the line and gating the invocation
    /// (`#gated`).
    async fn gated<T>(
        self: &Arc<Self>,
        invocation: &Arc<Invocation>,
        change: GatedChange<T>,
        context: Context,
    ) -> Result<T, PlainError>
    where
        T: Send + 'static,
    {
        if invocation.ended() {
            return Err(ended_error(invocation));
        }
        let scheduler = Arc::clone(self);
        let boxed = Arc::clone(invocation);
        self.session()
            .commit_with(
                move |tx: Arc<Transaction>| {
                    let scheduler = scheduler.clone();
                    let boxed = boxed.clone();
                    async move {
                        if boxed.ended() {
                            return Err(ended_error(&boxed));
                        }
                        if scheduler.is_closing() {
                            return Err(PlainError::new(closed_error().to_string()));
                        }
                        let found = scheduler.live_get(boxed.task_id);
                        let Some(found) = found else {
                            return Err(PlainError::new(format!(
                                "Task {} is terminal",
                                boxed.task_id
                            )));
                        };
                        if found.status() != TaskStatus::Running {
                            return Err(PlainError::new(format!(
                                "Task {} is {}",
                                boxed.task_id,
                                found.status().as_str()
                            )));
                        }
                        if boxed.mode == InvocationMode::Run && found.abort_requested {
                            return Err(PlainError::new(format!(
                                "Task {} has a durable abort mark",
                                boxed.task_id
                            )));
                        }
                        change(tx.as_ref(), &found)
                    }
                },
                context,
                TransactionScope {
                    conversation_id: Some(invocation.conversation_id),
                    task_id: Some(invocation.task_id),
                },
            )
            .await
    }

    /// Wait until the Harness clock reaches `until`, rechecking it after
    /// every timer (`#sleep`).
    async fn sleep(
        self: &Arc<Self>,
        invocation: &Arc<Invocation>,
        until: f64,
        context: Context,
    ) -> Result<(), PlainError> {
        if invocation.ended() {
            return Err(ended_error(invocation));
        }
        let aborted = || PlainError::new("The operation was aborted");
        loop {
            if invocation.controller.is_cancelled()
                || context
                    .abort_signal()
                    .is_some_and(|signal| signal.is_cancelled())
            {
                return Err(aborted());
            }
            let remaining = until - (self.options.now)();
            if remaining <= 0.0 {
                return Ok(());
            }
            let delay = remaining.min(MAX_TIMER_DELAY_MS as f64);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs_f64(delay.max(0.0))) => {}
                _ = invocation.controller.cancelled() => return Err(aborted()),
                _ = wait_abort(&context) => return Err(aborted()),
            }
        }
    }
}

// ─── Shared closure types ───────────────────────────────────────────────────

type SharedSnapshotFn = Arc<dyn Fn() -> Arc<dyn RegistrySnapshotLike> + Send + Sync>;
type SharedTaskFn = Arc<dyn Fn() -> TaskToken + Send + Sync>;
type StepDecide = Box<dyn FnOnce(&Transaction, &TaskRecord) -> Decision + Send>;
type GatedChange<T> = Box<dyn FnOnce(&Transaction, &TaskRecord) -> Result<T, PlainError> + Send>;

// ─── Free functions (`scheduler.ts` module scope) ───────────────────────────

/// A live owner's durable cancellation intent: its abort mark, or a held
/// outcome other than `completed` (`cancellationIntent`).
fn cancellation_intent(record: &TaskRecord) -> bool {
    record.status() != TaskStatus::Terminal && (record.abort_requested || failed_outcome(record))
}

/// Whether the record holds or ends with an outcome other than `completed`
/// (`failedOutcome`).
fn failed_outcome(record: &TaskRecord) -> bool {
    match &record.state {
        TaskState::Completing { outcome } | TaskState::Terminal { outcome } => {
            !matches!(outcome, TaskOutcome::Completed { .. })
        }
        _ => false,
    }
}

fn parent_of(node: &TaskNode) -> Up {
    match node.owner {
        Some(owner) => Up::Task(owner),
        None => Up::Conversation(node.conversation_id),
    }
}

/// [`parent_of`] over a full record.
fn record_parent(record: &TaskRecord) -> Up {
    match record.owner {
        Some(owner) => Up::Task(owner),
        None => Up::Conversation(record.conversation_id),
    }
}

fn node_of(record: &TaskRecord) -> TaskNode {
    TaskNode {
        conversation_id: record.conversation_id,
        owner: record.owner,
        background: record.background,
    }
}

fn overlay_of(tx: &Transaction) -> Overlay {
    let tasks = tx
        .staged_tasks()
        .into_iter()
        .map(|record| (record.id, record))
        .collect();
    let edges = tx
        .staged_conversations()
        .into_iter()
        .map(|record| (record.id, record.owner.as_ref().map(|owner| owner.task_id)))
        .collect();
    Overlay { tasks, edges }
}

/// Own memo entry only; memo names such as `toString` must not resolve to
/// inherited properties (`memoOf`).
fn memo_of(record: Option<&TaskRecord>, name: &str) -> Option<Value> {
    let memos = record.and_then(|record| record.memos.as_ref())?;
    memos.get(name).cloned()
}

fn missing_migration(record: &TaskRecord, definition: &TaskDefinition) -> PlainFailure {
    PlainFailure::new(format!(
        "Task {} version {} has no migration from {}",
        record.kind, definition.version, record.version
    ))
}

fn ended_error(invocation: &Invocation) -> PlainError {
    PlainError::new(format!("Task {} invocation has ended", invocation.task_id))
}

/// Replace a live record's state; memos disappear once an outcome is decided
/// (`withState`).
fn with_state(record: &TaskRecord, state: TaskState) -> TaskRecord {
    let mut updated = record.clone();
    if matches!(
        state,
        TaskState::Terminal { .. } | TaskState::Completing { .. }
    ) {
        updated.memos = None;
    }
    updated.state = state;
    updated
}

/// The running (or resumable) checkpoint of a record.
pub(crate) fn record_checkpoint(record: &TaskRecord) -> Option<&Value> {
    match &record.state {
        TaskState::Pending { checkpoint }
        | TaskState::Running { checkpoint }
        | TaskState::Waiting { checkpoint, .. } => Some(checkpoint),
        TaskState::Completing { .. } | TaskState::Terminal { .. } => None,
    }
}

/// Whether a definition can take the task at reservation: same version, or
/// newer with a migration (`canReserve`).
fn can_reserve(task: &TaskToken, record: &TaskRecord) -> bool {
    let definition = &task.definition;
    definition.version == record.version
        || (definition.version > record.version && definition.migrate.is_some())
}

fn storage_error(error: super::super::storage::StorageError) -> PlainError {
    PlainError::new(error.to_string())
}

fn waiter_error(error: WaiterError) -> PlainError {
    let _ = error;
    PlainError::new(closed_error().to_string())
}

async fn wait_abort(context: &Context) {
    match context.abort_signal() {
        Some(signal) => signal.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn await_with_abort<F>(future: F, context: &Context) -> Result<(), PlainError>
where
    F: std::future::Future<Output = ()>,
{
    tokio::select! {
        _ = future => Ok(()),
        _ = wait_abort(context) => Err(PlainError::new("The operation was aborted")),
    }
}

fn with_abort_signal(context: Context, token: CancellationToken) -> Context {
    context.with_value(abort_signal_key(), Some(token))
}

/// Every queued submission of the storage, in page order.
fn scan_all_submissions(
    storage: &dyn Storage,
    context: &Context,
) -> Result<Vec<super::super::types::SubmissionRecord>, PlainError> {
    let mut items = Vec::new();
    let mut cursor = None;
    loop {
        let page = storage
            .scan_submissions(
                super::super::types::SubmissionQuery {
                    status: Some(SubmissionStatus::Queued),
                    ..Default::default()
                },
                SCAN_PAGE_SIZE,
                cursor.as_ref(),
                context,
            )
            .map_err(storage_error)?;
        let next = page.next.clone();
        items.extend(page.items);
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    Ok(items)
}

/// One `mark` staging helper of [`TaskScheduler::reconcile`].
fn mark(
    tx: &Transaction,
    record: &TaskRecord,
    marked: &Arc<Mutex<HashSet<TaskId>>>,
) -> Result<(), PlainError> {
    if record.abort_requested
        || marked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&record.id)
    {
        return Ok(());
    }
    marked
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(record.id);
    let mut updated = record.clone();
    updated.abort_requested = true;
    tx.set_task(updated)
}

impl PhaseResult {
    fn phase_name(&self) -> &str {
        self.checkpoint
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
}
