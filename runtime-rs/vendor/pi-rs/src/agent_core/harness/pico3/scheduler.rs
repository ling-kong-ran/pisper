//! Port of `packages/agent/src/harness/pico3/scheduler.ts` (486 lines): the
//! erased-kind adapter that reserves eligible tasks on the line, dispatches
//! run/abort invocations through per-handler capability leases, validates
//! kind outputs at the boundary, and owns the durable waiters.
//!
//! Disclosed substitutions:
//! - **Drain loop.** Upstream chains `drain()` off a dirty flag on the
//!   promise tail; the port keeps the same flag protocol
//!   ([`Scheduler::kick`]/`draining`) and drives the loop on a spawned
//!   tokio task, single-flight via an atomic guard.
//! - **Invocation promises.** Upstream stores `done` promises; the port
//!   stores a `futures::future::Shared` handle over the same future so
//!   `abortTask` and `joinAll` can join in-flight invocations. The
//!   registry entry is inserted before the future is spawned, preserving
//!   upstream's synchronous `invocations.set` visibility.
//! - **AbortSignal.** `AbortController` is the repo's
//!   [`tokio_util::sync::CancellationToken`]; `withAbortSignal` is
//!   [`crate::agent_core::harness::context::with_abort_signal`]. The abort
//!   reason channel does not exist, so aborted waiters reject with the
//!   fixed upstream fallback message `"aborted"`.
//! - **Step validation.** `validateStep`'s non-object/`next`-shape checks
//!   are unrepresentable with typed [`Step`] enums (module docs in
//!   `runtime.rs`); the representable faults — completion shape, unknown
//!   checkpoint phase, in-flight phase transitions, missing phase handlers —
//!   are validated exactly where upstream does.
#![allow(clippy::type_complexity, clippy::large_enum_variant)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::with_abort_signal;
use crate::agent_core::harness::pico3::session::{
    CommitOptions, CommitRecord, Resolution, Session, Tx,
};
use crate::agent_core::harness::pico3::types::{
    is_core_kind, is_named, task_contract_fault, Completion, Id, Input, InvocationMode,
    InvocationToken, Invoker, JsonObject, NamedMessage, Outcome, Task, TaskStatus,
    TASK_CONTRACT_FAULT,
};

use super::runtime::{DoneFn, Kind, Next, Runtime, Step};

/// One in-flight invocation (`scheduler.ts:20-25`).
pub struct Invocation {
    /// Upstream `mode`.
    pub mode: InvocationMode,
    /// Upstream `controller`.
    pub controller: CancellationToken,
    /// Upstream `done`.
    pub done: Shared<BoxFuture<'static, ()>>,
}

/// Upstream `SchedulerDeps` (`scheduler.ts:27-33`).
pub struct SchedulerDeps {
    /// Upstream `session`.
    pub session: Arc<Session>,
    /// Upstream `kinds`: the registered execution kinds.
    pub kinds: Arc<std::sync::RwLock<HashMap<String, Arc<dyn Kind>>>>,
    /// Upstream `runtime(task, invoker, ctx)` (`harness.ts:189`): the
    /// runtime factory; the port binds the invoker only — contexts are
    /// per-call.
    pub make_runtime: Arc<dyn Fn(Invoker) -> Arc<Runtime> + Send + Sync>,
    /// Upstream `onReport`.
    pub on_report: Arc<dyn Fn(&anyhow::Error) + Send + Sync>,
    /// Upstream `ctx`.
    pub ctx: Context,
}

/// A resolved waiter slot (`scheduler.ts:91-93`), keyed for abort removal.
struct Waiter<T> {
    key: u64,
    sender: Option<oneshot::Sender<T>>,
}

struct IdleWaiter {
    conversation_id: Option<Id>,
    key: u64,
    sender: Option<oneshot::Sender<()>>,
}

#[derive(Default)]
struct SchedulerState {
    enabled: bool,
    holds: usize,
    dirty: bool,
    draining: bool,
    invocations: HashMap<Id, Invocation>,
    task_waiters: HashMap<Id, Vec<Waiter<Task>>>,
    input_waiters: HashMap<Id, Vec<Waiter<Input>>>,
    idle_waiters: Vec<IdleWaiter>,
}

static WAITER_KEY: AtomicU64 = AtomicU64::new(1);

fn next_waiter_key() -> u64 {
    WAITER_KEY.fetch_add(1, Ordering::SeqCst)
}

/// Upstream `Scheduler` (`scheduler.ts:84-486`).
pub struct Scheduler {
    deps: Arc<SchedulerDeps>,
    state: Mutex<SchedulerState>,
    /// Single-flight guard for the spawned drain (upstream's `draining`
    /// flag plus the promise tail gave this guarantee).
    drain_scheduled: AtomicBool,
}

impl Scheduler {
    /// Upstream `new Scheduler(deps)` (`scheduler.ts:95-115`): registers the
    /// session listener that aborts invocations, resolves waiters, and
    /// kicks the drain.
    pub fn new(deps: SchedulerDeps) -> Arc<Scheduler> {
        let deps = Arc::new(deps);
        let scheduler = Arc::new(Scheduler {
            deps: deps.clone(),
            state: Mutex::new(SchedulerState::default()),
            drain_scheduled: AtomicBool::new(false),
        });
        let listener_scheduler = scheduler.clone();
        deps.session
            .add_listener(Arc::new(move |record: &CommitRecord| {
                listener_scheduler.on_commit(record);
            }));
        scheduler
    }

    /// The session listener body (`scheduler.ts:97-114`).
    fn on_commit(self: &Arc<Self>, record: &CommitRecord) {
        {
            let mut state = self.state.lock().expect("scheduler state");
            for task in &record.changes.tasks {
                // `t.abort === true && inv?.mode === "run"` → abort
                // (`scheduler.ts:98-101`).
                if task.abort == Some(true) {
                    if let Some(invocation) = state.invocations.get(&task.id) {
                        if invocation.mode == InvocationMode::Run {
                            invocation.controller.cancel();
                        }
                    }
                }
                // Terminal tasks resolve task waiters
                // (`scheduler.ts:102-106`).
                if task.status == TaskStatus::Terminal {
                    if let Some(waiters) = state.task_waiters.get_mut(&task.id) {
                        for waiter in waiters.drain(..) {
                            if let Some(sender) = waiter.sender {
                                let _ = sender.send(task.clone());
                            }
                        }
                    }
                }
            }
            for input in &record.changes.inputs {
                // Done/unanswered inputs resolve input waiters
                // (`scheduler.ts:107-111`).
                if input.status == "done" || input.status == "unanswered" {
                    if let Some(waiters) = state.input_waiters.get_mut(&input.id) {
                        for waiter in waiters.drain(..) {
                            if let Some(sender) = waiter.sender {
                                let _ = sender.send(input.clone());
                            }
                        }
                    }
                }
            }
            let tasks_changed_out = !record.changes.tasks.is_empty();
            if tasks_changed_out {
                drop(state);
                self.kick();
            }
        }
        self.check_idle();
    }

    /// Upstream `resume` (`scheduler.ts:117-120`).
    pub fn resume(self: &Arc<Self>) {
        self.state.lock().expect("scheduler state").enabled = true;
        self.kick();
    }

    /// Upstream `stop` (`scheduler.ts:121-123`).
    pub fn stop(&self) {
        self.state.lock().expect("scheduler state").enabled = false;
    }

    /// Upstream `hold` (`scheduler.ts:124-133`): pause dispatching until the
    /// returned closure runs.
    pub fn hold(self: &Arc<Self>) -> Box<dyn FnOnce() + Send> {
        {
            let mut state = self.state.lock().expect("scheduler state");
            state.holds += 1;
        }
        let scheduler = self.clone();
        Box::new(move || {
            let should_kick = {
                let mut state = scheduler.state.lock().expect("scheduler state");
                state.holds = state.holds.saturating_sub(1);
                state.holds == 0
            };
            if should_kick {
                scheduler.kick();
            }
        })
    }

    /// Upstream `kick` (`scheduler.ts:134-138`).
    pub fn kick(self: &Arc<Self>) {
        let should_spawn = {
            let mut state = self.state.lock().expect("scheduler state");
            if !state.enabled {
                return;
            }
            state.dirty = true;
            !(state.holds > 0 || state.draining)
        };
        let scheduler = self.clone();
        if should_spawn
            && scheduler
                .drain_scheduled
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            let drain_target = scheduler.clone();
            tokio::spawn(async move {
                drain_target.drain().await;
                scheduler.drain_scheduled.store(false, Ordering::SeqCst);
            });
        }
    }

    /// Upstream `drain` (`scheduler.ts:140-156`).
    async fn drain(self: Arc<Self>) {
        self.state.lock().expect("scheduler state").draining = true;
        loop {
            let should_run = {
                let state = self.state.lock().expect("scheduler state");
                state.dirty && state.enabled && state.holds == 0
            };
            if !should_run {
                break;
            }
            self.state.lock().expect("scheduler state").dirty = false;
            let reserved = match self.reserve_eligible().await {
                Ok(reserved) => reserved,
                Err(error) => {
                    (self.deps.on_report)(&error);
                    Vec::new()
                }
            };
            let interrupted = {
                let state = self.state.lock().expect("scheduler state");
                !state.enabled || state.holds > 0
            };
            if interrupted {
                // `if (!this.enabled || this.holds > 0) { this.dirty = true;
                // break; }` (`scheduler.ts:146-149`).
                self.state.lock().expect("scheduler state").dirty = true;
                break;
            }
            for (task, mode) in reserved {
                self.dispatch(task, mode);
            }
        }
        self.state.lock().expect("scheduler state").draining = false;
        self.check_idle();
    }

    /// On the line: reserve every eligible task
    /// (`scheduler.ts:159-198`).
    async fn reserve_eligible(self: &Arc<Self>) -> anyhow::Result<Vec<(Task, InvocationMode)>> {
        let scheduler = self.clone();
        let session = self.deps.session.clone();
        let result = session
            .commit(
                Invoker::Kernel {
                    conversation_id: None,
                },
                self.deps.ctx.clone(),
                CommitOptions::default(),
                move |tx: &mut Tx, _line_ctx: Context| {
                    let scheduler = scheduler.clone();
                    async move {
                        let mut out: Vec<(Task, InvocationMode)> = Vec::new();
                        // `[...session.liveTasks.values()]`
                        // (`scheduler.ts:165`).
                        let live: Vec<Task> =
                            scheduler.deps.session.live_tasks().into_values().collect();
                        for task in &live {
                            let registered = scheduler
                                .deps
                                .kinds
                                .read()
                                .expect("kind registry")
                                .contains_key(&task.kind);
                            if !registered {
                                continue;
                            }
                            let invocation_mode = {
                                let state = scheduler.state.lock().expect("scheduler state");
                                state
                                    .invocations
                                    .get(&task.id)
                                    .map(|invocation| invocation.mode)
                            };
                            if task.abort == Some(true) {
                                match invocation_mode {
                                    // Winding down; dispatched after it
                                    // returns (`scheduler.ts:168-170`).
                                    Some(_) => continue,
                                    None => {
                                        let mut running = task.clone();
                                        if task.status == TaskStatus::Pending {
                                            running.status = TaskStatus::Running;
                                            tx.set_task(running.clone())?;
                                        }
                                        out.push((running, InvocationMode::Abort));
                                    }
                                }
                                continue;
                            }
                            if invocation_mode.is_some() {
                                continue;
                            }
                            if task.status == TaskStatus::Pending {
                                let mut ready = true;
                                for dep in &task.after {
                                    match tx.task(*dep).await? {
                                        Some(dependency)
                                            if dependency.status == TaskStatus::Terminal => {}
                                        _ => {
                                            ready = false;
                                            break;
                                        }
                                    }
                                }
                                if !ready {
                                    continue;
                                }
                                let mut running = task.clone();
                                running.status = TaskStatus::Running;
                                tx.set_task(running.clone())?;
                                out.push((running, InvocationMode::Run));
                            } else if task.status == TaskStatus::Running {
                                // Found at open: dispatch into its current
                                // phase (`scheduler.ts:190-192`).
                                out.push((task.clone(), InvocationMode::Run));
                            }
                        }
                        Ok(out)
                    }
                    .boxed()
                },
            )
            .await?;
        Ok(result.value)
    }

    /// Upstream `dispatch` (`scheduler.ts:200-305`): build the invocation
    /// entry (with its live controller token), insert it, and only then
    /// spawn the future.
    fn dispatch(self: &Arc<Self>, task: Task, mode: InvocationMode) {
        let kind = match self
            .deps
            .kinds
            .read()
            .expect("kind registry")
            .get(&task.kind)
        {
            Some(kind) => kind.clone(),
            None => {
                (self.deps.on_report)(&anyhow::anyhow!(
                    "unknown kind {} for task {}",
                    task.kind,
                    task.id
                ));
                return;
            }
        };
        let controller = CancellationToken::new();
        let ctx = with_abort_signal(controller.clone(), self.deps.ctx.clone());
        let scheduler = self.clone();
        let entry_controller = controller.clone();
        let task_id_for_entry = task.id;
        let done: Shared<BoxFuture<'static, ()>> = async move {
            scheduler
                .run_invocation(task, kind, mode, controller, ctx)
                .await;
        }
        .boxed()
        .shared();
        self.state
            .lock()
            .expect("scheduler state")
            .invocations
            .insert(
                task_id_for_entry,
                Invocation {
                    mode,
                    controller: entry_controller,
                    done: done.clone(),
                },
            );
        tokio::spawn(done);
    }

    /// The invocation body (`scheduler.ts:209-303`).
    async fn run_invocation(
        self: Arc<Self>,
        task: Task,
        kind: Arc<dyn Kind>,
        mode: InvocationMode,
        controller: CancellationToken,
        ctx: Context,
    ) {
        let session = self.deps.session.clone();
        let outcome: anyhow::Result<()> = async {
            if mode == InvocationMode::Run {
                // `const closure = await this.runPhases(...)`: `undefined`
                // means discarded.
                let closure = self
                    .run_phases(task.clone(), kind.clone(), ctx.clone())
                    .await?;
                if closure.is_none() || controller.is_cancelled() {
                    return Ok(());
                }
                let closure = closure.expect("checked above");
                let lease = self.lease(&task, &kind, mode);
                let kind_name = kind.metadata().name().to_owned();
                let session_for_commit = session.clone();
                let task_id = task.id;
                let commit_result = session
                    .commit(
                        lease.invoker.clone(),
                        ctx.clone(),
                        CommitOptions {
                            docs: Vec::new(),
                            closing: true,
                        },
                        move |tx: &mut Tx, line_ctx: Context| {
                            let kind_name = kind_name.clone();
                            async move {
                                // `const current =
                                // session.liveTasks.get(task.id); if (!current
                                // || current.abort) return`
                                // (`scheduler.ts:219-220`).
                                let current =
                                    session_for_commit.live_tasks().get(&task_id).cloned();
                                let Some(current) = current else {
                                    return Ok(());
                                };
                                if current.abort == Some(true) {
                                    return Ok(());
                                }
                                let completion = closure(tx, current.clone(), line_ctx).await?;
                                let completion = validate_completion(&kind_name, completion)?;
                                let patched = tx.task(task_id).await?.unwrap_or(current);
                                let mut final_task = patched;
                                final_task.status = TaskStatus::Terminal;
                                final_task.outcome = Some(outcome_of_completion(&completion));
                                final_task.checkpoint = None;
                                tx.set_task_for_control(final_task)?;
                                Ok(())
                            }
                            .boxed()
                        },
                    )
                    .await;
                // `finally { lease.token.revoke(); }` (`scheduler.ts:231`).
                lease.token.revoke();
                commit_result?;
            } else {
                // Abort mode (`scheduler.ts:234-265`).
                let handler_lease = self.lease(&task, &kind, mode);
                let handler_task = Arc::new(task.clone());
                let closure_result = kind
                    .abort(handler_task, handler_lease.runtime.clone(), ctx.clone())
                    .await;
                handler_lease.token.revoke();
                let closure = closure_result?;
                let closure_lease = self.lease(&task, &kind, mode);
                let session_for_commit = session.clone();
                let task_id = task.id;
                let commit_result = session
                    .commit(
                        closure_lease.invoker.clone(),
                        ctx.clone(),
                        CommitOptions {
                            docs: Vec::new(),
                            closing: true,
                        },
                        move |tx: &mut Tx, line_ctx: Context| {
                            async move {
                                let Some(current) =
                                    session_for_commit.live_tasks().get(&task_id).cloned()
                                else {
                                    return Ok(());
                                };
                                // `JSON.stringify(result)` strictness
                                // (`scheduler.ts:256`): values are serde
                                // JSON, so the check is a round-trip.
                                let result = closure(tx, current.clone(), line_ctx).await?;
                                let result: Value = serde_json::from_slice(
                                    &serde_json::to_vec(&result).expect("result serializes"),
                                )?;
                                let patched = tx.task(task_id).await?.unwrap_or(current);
                                let mut final_task = patched;
                                final_task.status = TaskStatus::Terminal;
                                final_task.outcome = Some(Outcome {
                                    status: "aborted".to_owned(),
                                    result: Some(result),
                                    failure: None,
                                    error: None,
                                });
                                final_task.checkpoint = None;
                                tx.set_task_for_control(final_task)?;
                                Ok(())
                            }
                            .boxed()
                        },
                    )
                    .await;
                closure_lease.token.revoke();
                commit_result?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = &outcome {
            if controller.is_cancelled() {
                // `if (controller.signal.aborted) return;`
                // (`scheduler.ts:268`).
            } else {
                // Contract fault or unexpected throw: the task ends
                // `faulted`, never as a declared failure
                // (`scheduler.ts:269-296`).
                (self.deps.on_report)(error);
                let fault_message = if is_named(error, TASK_CONTRACT_FAULT) {
                    error_message_of(error)
                } else {
                    format!("{}: {error}", kind.metadata().name())
                };
                let sticky_ref = crate::agent_core::harness::pico3::types::DocRef::Sticky {
                    conversation_id: task.conversation_id,
                };
                let turn_kind = kind.metadata().turn();
                let session_for_fault = session.clone();
                let task_id = task.id;
                let fault_commit = session
                    .commit(
                        Invoker::Kernel {
                            conversation_id: None,
                        },
                        self.deps.ctx.clone(),
                        CommitOptions {
                            docs: vec![sticky_ref],
                            closing: false,
                        },
                        move |tx: &mut Tx, _line_ctx: Context| {
                            let fault_message = fault_message.clone();
                            let turn_kind = turn_kind;
                            async move {
                                let Some(current) =
                                    session_for_fault.live_tasks().get(&task_id).cloned()
                                else {
                                    return Ok(());
                                };
                                let mut final_task = current.clone();
                                final_task.status = TaskStatus::Terminal;
                                final_task.outcome = Some(Outcome::faulted(fault_message.clone()));
                                final_task.checkpoint = None;
                                tx.set_task(final_task)?;
                                // A faulted turn task: its inputs would
                                // otherwise stay placed forever; the turn
                                // state is stale (`scheduler.ts:282-291`).
                                if turn_kind {
                                    if let Some(inputs) =
                                        current.input.get("inputs").and_then(Value::as_array).map(
                                            |array| {
                                                array
                                                    .iter()
                                                    .filter_map(Value::as_i64)
                                                    .collect::<Vec<_>>()
                                            },
                                        )
                                    {
                                        tx.resolve_inputs(
                                            &inputs,
                                            &Resolution::Unanswered {
                                                reason: "failed".to_owned(),
                                                detail: Some(fault_message.clone()),
                                            },
                                        )
                                        .await?;
                                    }
                                    tx.sticky_set(
                                        current.conversation_id,
                                        "turn",
                                        json!({ "tools": [] }),
                                    )?;
                                }
                                Ok(())
                            }
                            .boxed()
                        },
                    )
                    .await;
                if let Err(fault_error) = fault_commit {
                    (self.deps.on_report)(&fault_error);
                }
            }
        }
        // finally (`scheduler.ts:297-302`).
        self.state
            .lock()
            .expect("scheduler state")
            .invocations
            .remove(&task.id);
        let now = session.live_tasks().get(&task.id).cloned();
        if now.is_none() {
            if let Err(error) = session.retire(&task, self.deps.ctx.clone()).await {
                (self.deps.on_report)(&error);
            }
        }
        self.kick();
    }

    /// Upstream `lease` (`scheduler.ts:307-321`): one capability lease per
    /// handler.
    fn lease(&self, task: &Task, kind: &Arc<dyn Kind>, mode: InvocationMode) -> Lease {
        let token = InvocationToken::new();
        // Registration captures a stable metadata identity. Do not look this
        // up by name: a running kind may have been unregistered or replaced.
        let metadata = kind.metadata();
        let invoker = Invoker::Task {
            token: token.clone(),
            id: task.id,
            conversation_id: task.conversation_id,
            kind: metadata,
            core: is_core_kind(&task.kind),
            mode,
        };
        let runtime = (self.deps.make_runtime)(invoker.clone());
        Lease {
            token,
            invoker,
            runtime,
        }
    }

    /// The phase loop (`scheduler.ts:324-370`).
    async fn run_phases(
        self: &Arc<Self>,
        task: Task,
        kind: Arc<dyn Kind>,
        ctx: Context,
    ) -> anyhow::Result<Option<DoneFn>> {
        let session = self.deps.session.clone();
        let mut current = task.clone();
        loop {
            let cp = current
                .checkpoint
                .as_ref()
                .map(|checkpoint| Value::Object(checkpoint.clone()))
                .unwrap_or(Value::Null);
            let phase = cp.get("phase").and_then(Value::as_str).map(str::to_owned);
            // `handlerFor` (`scheduler.ts:41-45`): the missing-handler fault
            // surfaces through the port's `Kind::phase`/`Kind::initial`.
            let handler_lease = self.lease(&task, &kind, InvocationMode::Run);
            let handler_task = Arc::new(current.clone());
            let step_result: anyhow::Result<Step> = match &phase {
                None => {
                    kind.initial(
                        handler_task.clone(),
                        handler_lease.runtime.clone(),
                        ctx.clone(),
                    )
                    .await
                }
                Some(phase) => {
                    kind.phase(
                        phase,
                        handler_task.clone(),
                        handler_lease.runtime.clone(),
                        ctx.clone(),
                    )
                    .await
                }
            };
            // `finally { handlerLease.token.revoke(); }`
            // (`scheduler.ts:334-336`).
            handler_lease.token.revoke();
            let step = step_result?;
            let next = match step {
                Step::Done(closure) => return Ok(Some(closure)),
                Step::Next(next) => next,
            };
            if ctx
                .abort_signal()
                .is_some_and(|signal| signal.is_cancelled())
            {
                // `if (ctx.abortSignal?.aborted) return undefined;`
                // (`scheduler.ts:338`).
                return Ok(None);
            }
            let transition_lease = self.lease(&task, &kind, InvocationMode::Run);
            let session_for_commit = session.clone();
            let task_id = task.id;
            let kind_for_transition = kind.clone();
            let commit_result = session
                .commit(
                    transition_lease.invoker.clone(),
                    ctx.clone(),
                    CommitOptions::default(),
                    move |tx: &mut Tx, line_ctx: Context| {
                        let kind = kind_for_transition.clone();
                        async move {
                            // `const live = session.liveTasks.get(task.id);
                            // if (!live || live.abort) return "discard"`
                            // (`scheduler.ts:345-346`).
                            let live = session_for_commit.live_tasks().get(&task_id).cloned();
                            let Some(live) = live else {
                                return Ok::<Transition, anyhow::Error>(Transition::Discard);
                            };
                            if live.abort == Some(true) {
                                return Ok(Transition::Discard);
                            }
                            let next = match next {
                                Next::Defer(f) => f(tx, live.clone(), line_ctx).await?,
                                ready => ready,
                            };
                            match next {
                                Next::Retry => Ok(Transition::Retry),
                                Next::Completion(completion) => {
                                    let completion =
                                        validate_completion(kind.metadata().name(), completion)?;
                                    let patched = tx.task(task_id).await?.unwrap_or(live);
                                    let mut final_task = patched;
                                    final_task.status = TaskStatus::Terminal;
                                    final_task.outcome = Some(outcome_of_completion(&completion));
                                    final_task.checkpoint = None;
                                    tx.set_task_for_control(final_task)?;
                                    Ok(Transition::Terminal)
                                }
                                Next::Checkpoint(checkpoint) => {
                                    validate_checkpoint(&kind, &checkpoint)?;
                                    tx.checkpoint(Value::Object(checkpoint.clone()))?;
                                    Ok(Transition::Advanced(checkpoint))
                                }
                                Next::Defer(_) => {
                                    unreachable!("deferred transitions resolve once")
                                }
                            }
                        }
                        .boxed()
                    },
                )
                .await;
            // `finally { transitionLease.token.revoke(); }`
            // (`scheduler.ts:361-363`).
            transition_lease.token.revoke();
            match commit_result?.value {
                Transition::Retry => {
                    // `current = session.liveTasks.get(task.id) ?? current`
                    // (`scheduler.ts:365`).
                    current = session
                        .live_tasks()
                        .get(&task.id)
                        .cloned()
                        .unwrap_or(current);
                    continue;
                }
                Transition::Advanced(checkpoint) => {
                    // `current = { ...live, checkpoint }`
                    // (`scheduler.ts:356`).
                    let mut updated = session
                        .live_tasks()
                        .get(&task.id)
                        .cloned()
                        .unwrap_or(current);
                    updated.checkpoint = Some(checkpoint);
                    current = updated;
                    continue;
                }
                Transition::Terminal | Transition::Discard => return Ok(None),
            }
        }
    }

    /// Mark, revoke, signal, join (`scheduler.ts:373-394`).
    pub async fn abort_task(
        self: &Arc<Self>,
        id: Id,
        ctx: Context,
    ) -> anyhow::Result<&'static str> {
        let session = self.deps.session.clone();
        let marked = session
            .commit(
                Invoker::Kernel {
                    conversation_id: None,
                },
                ctx,
                CommitOptions::default(),
                move |tx: &mut Tx, _line_ctx: Context| {
                    async move {
                        let task = tx
                            .task(id)
                            .await?
                            .ok_or_else(|| anyhow::anyhow!("task {id} not found"))?;
                        if task.status == TaskStatus::Terminal {
                            return Ok("terminal");
                        }
                        if task.abort != Some(true) {
                            let mut marked = task;
                            marked.abort = Some(true);
                            tx.set_task(marked)?;
                        }
                        Ok("marked")
                    }
                    .boxed()
                },
            )
            .await?
            .value;
        if marked == "terminal" {
            return Ok("terminal");
        }
        let invocation = {
            let state = self.state.lock().expect("scheduler state");
            state.invocations.get(&id).map(|invocation| Invocation {
                mode: invocation.mode,
                controller: invocation.controller.clone(),
                done: invocation.done.clone(),
            })
        };
        if let Some(invocation) = invocation {
            if invocation.mode == InvocationMode::Run {
                invocation.controller.cancel();
                invocation.done.clone().await;
            }
        }
        self.kick();
        Ok("marked")
    }

    /// `waitForTask` (`scheduler.ts:398-403`): read state and install the
    /// waiter in one line operation; the wait itself happens off-line.
    pub async fn wait_for_task(self: &Arc<Self>, id: Id, ctx: Context) -> anyhow::Result<Task> {
        throw_if_aborted(&ctx)?;
        enum Step {
            Now(Task),
            Pending(oneshot::Receiver<Task>, u64),
        }
        let scheduler = self.clone();
        let session = self.deps.session.clone();
        let step = session
            .on_line(
                move |line_ctx: Context| {
                    let scheduler = scheduler.clone();
                    async move {
                        // Terminal read (`scheduler.ts:400-401`): a live
                        // task is not terminal; only the durable record
                        // resolves immediately.
                        let now = if scheduler.deps.session.live_tasks().contains_key(&id) {
                            None
                        } else {
                            scheduler.deps.session.storage().task(id, line_ctx).await?
                        };
                        if let Some(task) = now {
                            return Ok::<Step, anyhow::Error>(Step::Now(task));
                        }
                        let (sender, receiver) = oneshot::channel();
                        let key = next_waiter_key();
                        scheduler
                            .state
                            .lock()
                            .expect("scheduler state")
                            .task_waiters
                            .entry(id)
                            .or_default()
                            .push(Waiter {
                                key,
                                sender: Some(sender),
                            });
                        Ok(Step::Pending(receiver, key))
                    }
                    .boxed()
                },
                ctx.clone(),
            )
            .await?;
        match step {
            Step::Now(task) => Ok(task),
            Step::Pending(receiver, key) => {
                tokio::select! {
                    received = receiver => {
                        received.map_err(|_| anyhow::anyhow!("waiter dropped"))
                    }
                    _ = cancelled(&ctx) => {
                        // Remove this waiter (`scheduler.ts:429-431`).
                        let mut state = self.state.lock().expect("scheduler state");
                        if let Some(waiters) = state.task_waiters.get_mut(&id) {
                            waiters.retain(|waiter| waiter.key != key);
                        }
                        Err(aborted_error())
                    }
                }
            }
        }
    }

    /// `waitForInput` (`scheduler.ts:404-409`).
    pub async fn wait_for_input(self: &Arc<Self>, id: Id, ctx: Context) -> anyhow::Result<Input> {
        throw_if_aborted(&ctx)?;
        enum Step {
            Now(Input),
            Pending(oneshot::Receiver<Input>, u64),
        }
        let scheduler = self.clone();
        let session = self.deps.session.clone();
        let step = session
            .on_line(
                move |line_ctx: Context| {
                    let scheduler = scheduler.clone();
                    async move {
                        let now = scheduler.deps.session.storage().input(id, line_ctx).await?;
                        if let Some(input) = now {
                            if input.status == "done" || input.status == "unanswered" {
                                return Ok::<Step, anyhow::Error>(Step::Now(input));
                            }
                        }
                        let (sender, receiver) = oneshot::channel();
                        let key = next_waiter_key();
                        scheduler
                            .state
                            .lock()
                            .expect("scheduler state")
                            .input_waiters
                            .entry(id)
                            .or_default()
                            .push(Waiter {
                                key,
                                sender: Some(sender),
                            });
                        Ok(Step::Pending(receiver, key))
                    }
                    .boxed()
                },
                ctx.clone(),
            )
            .await?;
        match step {
            Step::Now(input) => Ok(input),
            Step::Pending(receiver, key) => {
                tokio::select! {
                    received = receiver => {
                        received.map_err(|_| anyhow::anyhow!("waiter dropped"))
                    }
                    _ = cancelled(&ctx) => {
                        let mut state = self.state.lock().expect("scheduler state");
                        if let Some(waiters) = state.input_waiters.get_mut(&id) {
                            waiters.retain(|waiter| waiter.key != key);
                        }
                        Err(aborted_error())
                    }
                }
            }
        }
    }

    /// `waitForIdle` (`scheduler.ts:440-464`).
    pub async fn wait_for_idle(
        self: &Arc<Self>,
        conversation_id: Option<Id>,
        ctx: Context,
    ) -> anyhow::Result<()> {
        throw_if_aborted(&ctx)?;
        enum Step {
            Now,
            Pending(oneshot::Receiver<()>, u64),
        }
        let scheduler = self.clone();
        let session = self.deps.session.clone();
        let step = session
            .on_line(
                move |_line_ctx: Context| {
                    let scheduler = scheduler.clone();
                    async move {
                        if scheduler.is_idle(conversation_id) {
                            return Ok::<Step, anyhow::Error>(Step::Now);
                        }
                        let (sender, receiver) = oneshot::channel();
                        let key = next_waiter_key();
                        scheduler
                            .state
                            .lock()
                            .expect("scheduler state")
                            .idle_waiters
                            .push(IdleWaiter {
                                conversation_id,
                                key,
                                sender: Some(sender),
                            });
                        Ok(Step::Pending(receiver, key))
                    }
                    .boxed()
                },
                ctx.clone(),
            )
            .await?;
        match step {
            Step::Now => Ok(()),
            Step::Pending(receiver, key) => {
                tokio::select! {
                    received = receiver => {
                        received.map_err(|_| anyhow::anyhow!("idle waiter dropped"))
                    }
                    _ = cancelled(&ctx) => {
                        let mut state = self.state.lock().expect("scheduler state");
                        state.idle_waiters.retain(|waiter| waiter.key != key);
                        Err(aborted_error())
                    }
                }
            }
        }
    }

    /// Upstream `isIdle` (`scheduler.ts:465-469`).
    pub fn is_idle(&self, conversation_id: Option<Id>) -> bool {
        for task in self.deps.session.live_tasks().values() {
            if (conversation_id.is_none() || Some(task.conversation_id) == conversation_id)
                && task.background != Some(true)
            {
                return false;
            }
        }
        true
    }

    /// Upstream `checkIdle` (`scheduler.ts:470-472`): resolve waiters only
    /// while not draining.
    fn check_idle(&self) {
        let draining = self.state.lock().expect("scheduler state").draining;
        if draining {
            return;
        }
        let mut state = self.state.lock().expect("scheduler state");
        let idle_waiters = std::mem::take(&mut state.idle_waiters);
        let mut kept = Vec::new();
        for waiter in idle_waiters {
            if self.is_idle(waiter.conversation_id) {
                if let Some(sender) = waiter.sender {
                    let _ = sender.send(());
                }
            } else {
                kept.push(waiter);
            }
        }
        state.idle_waiters = kept;
    }

    /// Signal every invocation and wait for them; nothing is written
    /// (`scheduler.ts:475-479`).
    pub async fn join_all(&self) {
        self.state.lock().expect("scheduler state").enabled = false;
        let dones: Vec<Shared<BoxFuture<'static, ()>>> = {
            let mut state = self.state.lock().expect("scheduler state");
            state
                .invocations
                .values_mut()
                .map(|invocation| {
                    invocation.controller.cancel();
                    invocation.done.clone()
                })
                .collect()
        };
        for done in dones {
            // `Promise.allSettled` (`scheduler.ts:478`).
            let _ = done.await;
        }
    }

    /// Upstream `quiescent` (`scheduler.ts:480-482`).
    pub fn quiescent(&self) -> bool {
        self.state
            .lock()
            .expect("scheduler state")
            .invocations
            .is_empty()
    }

    /// Upstream `get liveInvocations` (`scheduler.ts:483-485`).
    pub fn live_invocations(&self) -> usize {
        self.state
            .lock()
            .expect("scheduler state")
            .invocations
            .len()
    }
}

/// One capability lease (`scheduler.ts:307-321` result).
struct Lease {
    token: Arc<InvocationToken>,
    invoker: Invoker,
    runtime: Arc<Runtime>,
}

/// The transition-closure outcomes (`scheduler.ts:344-358`).
enum Transition {
    Discard,
    Retry,
    Terminal,
    Advanced(JsonObject),
}

/// Upstream `validateCompletion` (`scheduler.ts:62-69`).
fn validate_completion(kind: &str, completion: Completion) -> anyhow::Result<Completion> {
    let valid = match completion.status.as_str() {
        "completed" => completion.result.is_some(),
        "failed" => completion.failure.is_some(),
        _ => false,
    };
    if !valid {
        return Err(task_contract_fault(
            kind,
            "closure returned an invalid completion",
        ));
    }
    Ok(completion)
}

/// The `Outcome` shape of a completion (`scheduler.ts:226`).
fn outcome_of_completion(completion: &Completion) -> Outcome {
    Outcome {
        status: completion.status.clone(),
        result: completion.result.clone(),
        failure: completion.failure.clone(),
        error: None,
    }
}

/// Upstream `validateCheckpoint` (`scheduler.ts:70-82`): the phase must be
/// known and not in-flight; `JSON.stringify(cp)` strictness is a serde
/// round-trip by construction.
fn validate_checkpoint(kind: &Arc<dyn Kind>, checkpoint: &JsonObject) -> anyhow::Result<()> {
    let phase = checkpoint
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| task_contract_fault(kind.metadata().name(), "invalid checkpoint"))?;
    if !kind.phases().iter().any(|declared| declared == phase) {
        return Err(task_contract_fault(
            kind.metadata().name(),
            &format!("checkpoint names unknown phase {phase}"),
        ));
    }
    if kind
        .metadata()
        .inflight()
        .iter()
        .any(|declared| declared == phase)
    {
        return Err(task_contract_fault(
            kind.metadata().name(),
            &format!(
                "transition into in-flight phase {phase}; write it with rt.commit before the effect instead"
            ),
        ));
    }
    Ok(())
}

fn throw_if_aborted(ctx: &Context) -> anyhow::Result<()> {
    if let Some(signal) = ctx.abort_signal() {
        if signal.is_cancelled() {
            return Err(aborted_error());
        }
    }
    Ok(())
}

async fn cancelled(ctx: &Context) {
    if let Some(signal) = ctx.abort_signal() {
        signal.cancelled().await;
    } else {
        std::future::pending::<()>().await;
    }
}

/// `ctx.abortSignal?.reason ?? new Error("aborted")` (`scheduler.ts:431`);
/// the port has no reason channel, so the fallback message is fixed.
fn aborted_error() -> anyhow::Error {
    anyhow::anyhow!("aborted")
}

fn error_message_of(error: &anyhow::Error) -> String {
    for cause in error.chain() {
        if let Some(message) = cause.downcast_ref::<NamedMessage>() {
            if message.name == TASK_CONTRACT_FAULT {
                return message.message.clone();
            }
        }
    }
    format!("{error}")
}
