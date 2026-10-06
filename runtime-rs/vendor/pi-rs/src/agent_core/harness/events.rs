//! Port of `packages/agent/src/harness/events.ts` (286 lines): the passive
//! harness event bus with isolated handler failures, plus the buffered event
//! watcher with resnapshot barriers.
//!
//! Semantics preserved from upstream, structure adapted to Rust futures:
//! - **Delivery tail.** Upstream chains every publication onto a single
//!   `deliveryTail` promise, so concurrent `emitBatch` calls serialize in
//!   process order and each batch stays contiguous with its emitting
//!   context. The port reproduces this with an explicit chain: each
//!   publication takes the previous tail's completion receiver and installs
//!   its own, so appended jobs self-serialize by construction (no locks held
//!   across delivery).
//! - **Recipient binding.** Recipients (typed listeners for the event's type,
//!   then all watch listeners) are snapshotted when a batch is emitted.
//! - **Per-listener isolation.** Upstream clones each event per listener
//!   (`structuredClone`) and catches every listener exception, converting the
//!   first failure of a non-`handler_error` event into a synthesized
//!   `handler_error` event delivered without further error reporting. The
//!   port's events are immutable `Arc` snapshots (aliasing cannot mutate what
//!   other listeners see), and listener failures are Rust panics caught with
//!   `catch_unwind` — the direct analogue of a thrown JS listener. Panic
//!   messages (from `panic!("...")`) feed `handler_error.error`; `Error.stack`
//!   has no Rust equivalent, so the stack field is always absent.
//! - **Watchers.** `BufferedEventWatcher` buffers events between `watch` and
//!   `start` (stamped with the epoch at push time and replayed through the
//!   epoch check), drops events pushed during the dropping phase of a
//!   `resnapshot`, holds events pushed after the boundary until the new
//!   snapshot is installed, and runs its listener deliveries on its own
//!   tail so a blocking listener never stalls the bus. The resnapshot
//!   boundary is a job appended to the bus tail (upstream `enqueueBarrier`),
//!   so the barrier runs only after every previously published batch was
//!   delivered.
//! - **Closed bus.** `close(error)` keeps the first error, appends the
//!   listener-map clear to the tail (already published batches drain), and
//!   later `on`/`watch` calls fail with the stored message while later
//!   `emitBatch` calls resolve without delivery.
//!
//! The bus is generic over [`BusEvent`] rather than a concrete event union:
//! upstream `events.ts` only touches `event.type`, the optional `lane`
//! string, and the `handler_error` shape, all carried by the trait. The full
//! `HarnessEvent` union lands with `agent-harness.ts` (M3b Task 10) and
//! implements the trait; the tests here use a fixture with the same variants
//! the upstream oracle tests use.

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use anyhow::anyhow;
use futures::future::BoxFuture;
use futures::FutureExt;

use crate::agent_core::chord_support::Context;

/// The `handler_error` event type discriminator (`events.ts:110`).
pub const HANDLER_ERROR_EVENT_TYPE: &str = "handler_error";

/// The event payload contract the bus relies on (upstream: the `HarnessEvent`
/// union in `agent-harness.ts:388-397`).
pub trait BusEvent: Clone + Send + Sync + 'static {
    /// The event's `type` discriminator.
    fn event_type(&self) -> &str;
    /// The optional lane string (`"lane" in event && typeof event.lane ===
    /// "string"`, `events.ts:110`).
    fn lane(&self) -> Option<&str> {
        None
    }
    /// Build the `handler_error` event for a failed delivery of
    /// `event_type` (`events.ts:110-118`: `{ type: "handler_error", kind:
    /// "event", event, error, stack?, lane? }`).
    fn handler_error(event_type: String, error: String, lane: Option<String>) -> Self;
}

/// A listener: upstream `EventListener` (`agent-harness.ts:414-417`), with
/// the event as an immutable shared snapshot and the chord context.
pub type EventListenerFn<E> = dyn Fn(Arc<E>, Context) -> BoxFuture<'static, ()> + Send + Sync;
/// A registered listener handle shared between the maps and unsubscribe
/// closures.
pub type EventListener<E> = Arc<EventListenerFn<E>>;
/// An event filter: upstream `filter: (event) => boolean` (`events.ts:50`).
pub type EventFilter<E> = dyn Fn(&E) -> bool + Send + Sync;
/// The watcher's error reporter: upstream `onError` (`events.ts:105`).
pub type WatcherOnError<E> =
    Arc<dyn Fn(anyhow::Error, Arc<E>, Context) -> BoxFuture<'static, ()> + Send + Sync>;
/// Upstream `ResnapshotCapture<T>` (`events.ts:5`): captures a new snapshot;
/// the capture must call the boundary marker exactly once.
pub type ResnapshotCapture<T> =
    Arc<dyn Fn(Context, MarkBoundary) -> BoxFuture<'static, anyhow::Result<T>> + Send + Sync>;
/// Upstream `markBoundary` (`events.ts:5, 97-101`): marks the resnapshot
/// boundary, enqueueing it after every previously published batch. Calling it
/// twice panics (upstream throws).
pub type MarkBoundary = Box<dyn FnMut() + Send>;
/// Upstream `watchFromSnapshot`'s capture (`events.ts:58`).
pub type SnapshotCapture<T> =
    Arc<dyn Fn(Context) -> BoxFuture<'static, anyhow::Result<T>> + Send + Sync>;
/// The bus-wrapped resnapshot capture stored on a watcher: the user capture
/// plus the "mark exactly once" boundary contract (`events.ts:92-104`).
type WrappedCapture<T> =
    Arc<dyn Fn(Context) -> BoxFuture<'static, anyhow::Result<T>> + Send + Sync>;

/// The `() => void` unsubscribe handle upstream `on` returns (`events.ts:14`).
/// Idempotent; callable any number of times.
pub struct Unsubscribe {
    callback: Box<dyn Fn() + Send + Sync>,
}

impl Unsubscribe {
    pub(crate) fn new(callback: Box<dyn Fn() + Send + Sync>) -> Self {
        Unsubscribe { callback }
    }

    /// Remove the listener (upstream invoking the returned function).
    pub fn unsubscribe(&self) {
        (self.callback)();
    }
}

impl fmt::Debug for Unsubscribe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Unsubscribe")
    }
}

/// Lock helper that recovers from poisoning: listener panics are caught and
/// converted to `handler_error` events, so a panic racing a lock must not
/// poison the bus permanently.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One completion slot of the delivery-tail chain: the previous job's
/// receiver (or `None` for the first) plus this job's sender.
type TailSlot = Mutex<Option<tokio::sync::oneshot::Receiver<()>>>;

/// Append `body` to the promise-chain-style tail. The take-previous and
/// install-own steps run in ONE critical section, so two concurrent
/// publishers can never both observe an empty slot and escape the chain
/// (upstream: the JS event loop makes the bind-and-chain prologue atomic).
/// Chain order follows call order regardless of task scheduling.
fn append_job<F>(slot: &TailSlot, body: F) -> tokio::task::JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let (previous, sender) = {
        let mut slot = lock(slot);
        let previous = slot.take();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *slot = Some(receiver);
        (previous, sender)
    };
    tokio::spawn(async move {
        if let Some(previous) = previous {
            let _ = previous.await;
        }
        body.await;
        let _ = sender.send(());
    })
}

/// Extract a listener-facing message from a caught panic payload (upstream
/// `error instanceof Error ? error.message : new Error(String(error))`,
/// `events.ts:107, 150`).
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "listener panicked".to_string()
    }
}

/// Invoke a listener with both failure channels caught: the synchronous
/// prologue (the closure body runs before the returned future is ever
/// polled, so its panics would otherwise escape `catch_unwind`) and the
/// polled future. Upstream `try { await listener(...) }` catches both
/// (`events.ts:145-147, 281`).
async fn catch_listener<T>(
    invoke: impl FnOnce() -> BoxFuture<'static, T>,
) -> Result<T, Box<dyn Any + Send>> {
    match std::panic::catch_unwind(AssertUnwindSafe(invoke)) {
        Err(payload) => Err(payload),
        Ok(future) => AssertUnwindSafe(future).catch_unwind().await,
    }
}

/// Await a spawned delivery job, re-panicking if the job panicked (upstream
/// delivery never rejects because listener failures are isolated; a Rust
/// panic escaping isolation is a port bug and must stay loud).
async fn join_delivery(handle: tokio::task::JoinHandle<()>) {
    match handle.await {
        Ok(()) => {}
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(_) => {}
    }
}

/// Passive harness event bus with isolated handler failures (upstream
/// `HarnessEventBus`, `events.ts:8-163`). Clone to share one bus.
pub struct HarnessEventBus<E: BusEvent> {
    inner: Arc<BusInner<E>>,
}

struct BusInner<E: BusEvent> {
    /// Typed listeners by event type (`events.ts:9`).
    listeners: Mutex<HashMap<String, Vec<EventListener<E>>>>,
    /// Watch listeners in registration order (`events.ts:10`).
    watch_listeners: Mutex<Vec<EventListener<E>>>,
    /// The global delivery tail (`events.ts:11`).
    delivery_tail: TailSlot,
    /// The first `close` error (`events.ts:12`).
    closed_error: Mutex<Option<String>>,
}

impl<E: BusEvent> Clone for HarnessEventBus<E> {
    fn clone(&self) -> Self {
        HarnessEventBus {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<E: BusEvent> Default for HarnessEventBus<E> {
    fn default() -> Self {
        Self::new()
    }
}

/// One bound event of an in-flight batch: the payload snapshot plus the
/// recipients captured at emit time (`events.ts:37-40`).
struct BoundEvent<E: BusEvent> {
    payload: Arc<E>,
    recipients: Vec<EventListener<E>>,
}

impl<E: BusEvent> BusInner<E> {
    fn snapshot_recipients(&self, event_type: &str) -> Vec<EventListener<E>> {
        let mut recipients: Vec<EventListener<E>> = lock(&self.listeners)
            .get(event_type)
            .cloned()
            .unwrap_or_default();
        recipients.extend(lock(&self.watch_listeners).iter().cloned());
        recipients
    }

    /// Upstream `deliver` (`events.ts:138-162`). Returns a boxed future so
    /// the `handler_error` recursion stays sized.
    fn deliver<'a>(
        &'a self,
        event: Arc<E>,
        recipients: &'a [EventListener<E>],
        report_errors: bool,
        context: Context,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            for listener in recipients {
                let outcome =
                    catch_listener(|| listener(Arc::clone(&event), context.clone())).await;
                match outcome {
                    Ok(()) => {}
                    Err(payload) => {
                        if !report_errors || event.event_type() == HANDLER_ERROR_EVENT_TYPE {
                            continue;
                        }
                        let message = panic_message(payload);
                        let lane = event.lane().map(str::to_string);
                        let handler_error = Arc::new(E::handler_error(
                            event.event_type().to_string(),
                            message,
                            lane,
                        ));
                        let recipients = self.snapshot_recipients(handler_error.event_type());
                        Box::pin(self.deliver(handler_error, &recipients, false, context.clone()))
                            .await;
                    }
                }
            }
        })
    }

    fn is_closed(&self) -> bool {
        lock(&self.closed_error).is_some()
    }

    fn closed_message(&self) -> anyhow::Result<()> {
        match lock(&self.closed_error).clone() {
            Some(message) => Err(anyhow!(message)),
            None => Ok(()),
        }
    }
}

impl<E: BusEvent> HarnessEventBus<E> {
    /// Create an open bus (`new HarnessEventBus()`).
    pub fn new() -> Self {
        HarnessEventBus {
            inner: Arc::new(BusInner {
                listeners: Mutex::new(HashMap::new()),
                watch_listeners: Mutex::new(Vec::new()),
                delivery_tail: Mutex::new(None),
                closed_error: Mutex::new(None),
            }),
        }
    }

    /// Upstream `on` (`events.ts:14-28`): register a listener for one event
    /// type. Fails with the stored close message after [`Self::close`].
    pub fn on<F>(&self, event_type: &str, listener: F) -> anyhow::Result<Unsubscribe>
    where
        F: Fn(Arc<E>, Context) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        self.inner.closed_message()?;
        let wrapped: EventListener<E> = Arc::new(listener);
        lock(&self.inner.listeners)
            .entry(event_type.to_string())
            .or_default()
            .push(Arc::clone(&wrapped));
        let inner = Arc::clone(&self.inner);
        let event_type = event_type.to_string();
        Ok(Unsubscribe::new(Box::new(move || {
            if let Some(listeners) = lock(&inner.listeners).get_mut(&event_type) {
                listeners.retain(|existing| !Arc::ptr_eq(existing, &wrapped));
            }
        })))
    }

    /// Upstream `emit` (`events.ts:30-32`).
    pub fn emit(&self, event: E, context: Context) -> BoxFuture<'static, ()> {
        self.emit_batch(vec![event], context)
    }

    /// Upstream `emitBatch` (`events.ts:34-46`): bind current recipients and
    /// append one contiguous batch to the global delivery tail. Resolves when
    /// the batch finished delivering; resolves without delivery when the bus
    /// is closed or the batch is empty.
    pub fn emit_batch(&self, events: Vec<E>, context: Context) -> BoxFuture<'static, ()> {
        if self.inner.is_closed() || events.is_empty() {
            return Box::pin(async {});
        }
        let bound: Vec<BoundEvent<E>> = events
            .into_iter()
            .map(|event| {
                let recipients = self.inner.snapshot_recipients(event.event_type());
                BoundEvent {
                    payload: Arc::new(event),
                    recipients,
                }
            })
            .collect();
        let inner = Arc::clone(&self.inner);
        let handle = append_job(&self.inner.delivery_tail, async move {
            for bound in &bound {
                inner
                    .deliver(
                        Arc::clone(&bound.payload),
                        &bound.recipients,
                        true,
                        context.clone(),
                    )
                    .await;
            }
        });
        Box::pin(join_delivery(handle))
    }

    /// Upstream `watch` (`events.ts:48-56`): install a watcher starting from
    /// `snapshot`. Events pass the watcher listener only when `filter` holds.
    pub fn watch<T, F>(
        &self,
        snapshot: T,
        filter: F,
        context: Context,
    ) -> anyhow::Result<WatchHandle<T, E>>
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
        T: Send + Sync + 'static,
    {
        self.watch_with_resnapshot(snapshot, filter, context, None)
    }

    /// Upstream `watch` with its optional `resnapshot` argument
    /// (`events.ts:48-56`): the capture receives the boundary marker and must
    /// call it exactly once.
    pub fn watch_with_resnapshot<T, F>(
        &self,
        snapshot: T,
        filter: F,
        context: Context,
        resnapshot: Option<ResnapshotCapture<T>>,
    ) -> anyhow::Result<WatchHandle<T, E>>
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
        T: Send + Sync + 'static,
    {
        self.inner.closed_message()?;
        let _ = context;
        Ok(self.install_watcher(Some(snapshot), Arc::new(filter), resnapshot))
    }

    /// Upstream `watchFromSnapshot` (`events.ts:58-76`): install the watcher
    /// first (so events during the capture buffer), then capture the initial
    /// snapshot; a failed capture unsubscribes and propagates.
    pub async fn watch_from_snapshot<T, F>(
        &self,
        capture: SnapshotCapture<T>,
        filter: F,
        context: Context,
    ) -> anyhow::Result<WatchHandle<T, E>>
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
        T: Send + Sync + 'static,
    {
        self.inner.closed_message()?;
        // The resnapshot capture: run the user capture, then mark the
        // boundary (`events.ts:64-68`).
        let resnapshot: ResnapshotCapture<T> = {
            let capture = Arc::clone(&capture);
            Arc::new(move |context: Context, mark_boundary: MarkBoundary| {
                let capture = Arc::clone(&capture);
                Box::pin(async move {
                    let snapshot = capture(context).await?;
                    let mut mark_boundary = mark_boundary;
                    mark_boundary();
                    Ok(snapshot)
                })
            })
        };
        let watcher = self.install_watcher(None, Arc::new(filter), Some(resnapshot));
        match capture(context.clone()).await {
            Ok(snapshot) => {
                watcher.set_snapshot(snapshot);
                Ok(watcher)
            }
            Err(error) => {
                watcher.unsubscribe();
                Err(error)
            }
        }
    }

    /// Upstream `close` (`events.ts:78-84`): keep the first error, drain the
    /// already published batches, then clear the listener maps. Later
    /// publications resolve without delivery and later registrations fail.
    pub fn close(&self, error: anyhow::Error) {
        {
            let mut closed = lock(&self.inner.closed_error);
            if closed.is_some() {
                return;
            }
            *closed = Some(error.to_string());
        }
        let listeners = Arc::clone(&self.inner);
        let watch_listeners = Arc::clone(&self.inner);
        append_job(&self.inner.delivery_tail, async move {
            lock(&listeners.listeners).clear();
            lock(&watch_listeners.watch_listeners).clear();
        });
    }

    /// Upstream `installWatcher` (`events.ts:86-127`).
    fn install_watcher<T>(
        &self,
        snapshot: Option<T>,
        filter: Arc<EventFilter<E>>,
        resnapshot: Option<ResnapshotCapture<T>>,
    ) -> WatchHandle<T, E>
    where
        T: Send + Sync + 'static,
    {
        let core = Arc::new(WatcherCore::new(snapshot, self.watcher_on_error()));
        if let Some(user) = resnapshot {
            // The capture wrapper owns the "mark exactly once" contract and
            // enqueues the boundary after the current tail (`events.ts:92-104`).
            // The core and bus are held weakly to avoid reference cycles: the
            // bus keeps only the watch listener (weak core), and the core's
            // closures upgrade on use.
            let weak_core = Arc::downgrade(&core);
            let weak_bus = Arc::downgrade(&self.inner);
            let capture: WrappedCapture<T> = Arc::new(
                move |context: Context| -> BoxFuture<'static, anyhow::Result<T>> {
                    let user = Arc::clone(&user);
                    let weak_core = weak_core.clone();
                    let weak_bus = weak_bus.clone();
                    Box::pin(async move {
                        let marked = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let mark_flag = Arc::clone(&marked);
                        let mark: MarkBoundary = Box::new(move || {
                            if mark_flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
                                panic!("Resnapshot boundary was already marked");
                            }
                            if let (Some(core), Some(bus)) =
                                (weak_core.upgrade(), weak_bus.upgrade())
                            {
                                append_job(&bus.delivery_tail, {
                                    let core = Arc::clone(&core);
                                    async move {
                                        core.mark_resnapshot_boundary();
                                    }
                                });
                            }
                        });
                        let next = user(context, mark).await?;
                        if !marked.load(std::sync::atomic::Ordering::SeqCst) {
                            anyhow::bail!("Resnapshot capture did not mark its boundary");
                        }
                        Ok(next)
                    })
                },
            );
            lock(&core.state).resnapshot_callback = Some(capture);
        }

        // The watch listener: filter, then push. Holds the core weakly — the
        // caller's WatchHandle keeps it alive (`events.ts:121-125`).
        let weak_core = Arc::downgrade(&core);
        let watch_listener: EventListener<E> = Arc::new(move |event: Arc<E>, context: Context| {
            if let Some(core) = weak_core.upgrade() {
                if filter(&event) {
                    core.push(event, context);
                }
            }
            Box::pin(async {})
        });
        lock(&self.inner.watch_listeners).push(Arc::clone(&watch_listener));
        let inner = Arc::clone(&self.inner);
        core.set_unsubscribe(Box::new(move || {
            lock(&inner.watch_listeners).retain(|listener| !Arc::ptr_eq(listener, &watch_listener));
        }));
        WatchHandle { core }
    }

    /// The watcher error reporter (`events.ts:105-120`): report every watcher
    /// delivery failure except on `handler_error` events, by emitting a
    /// `handler_error` event through the bus.
    fn watcher_on_error(&self) -> WatcherOnError<E> {
        let bus = self.clone();
        Arc::new(
            move |error: anyhow::Error, event: Arc<E>, context: Context| {
                let bus = bus.clone();
                Box::pin(async move {
                    if event.event_type() == HANDLER_ERROR_EVENT_TYPE {
                        return;
                    }
                    let lane = event.lane().map(str::to_string);
                    let handler_error =
                        E::handler_error(event.event_type().to_string(), error.to_string(), lane);
                    bus.emit(handler_error, context).await;
                })
            },
        )
    }
}

/// One event buffered between `watch` and `start` (`events.ts:169`), stamped
/// with the epoch at push time (`events.ts:273`).
struct BufferedEvent<E: BusEvent> {
    event: Arc<E>,
    context: Context,
    epoch: u64,
}

/// Resnapshot progress (`events.ts:174-181`): events are dropped while
/// `dropping`, held after the boundary until the new snapshot is installed.
struct ResnapshotState<E: BusEvent> {
    dropping: bool,
    held: Vec<(Arc<E>, Context)>,
    /// Resolved when the boundary job runs (`events.ts:213-215`); taken by
    /// `mark_resnapshot_boundary`.
    boundary: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Buffering,
    Started,
    Unsubscribed,
}

struct WatcherState<T, E: BusEvent> {
    snapshot: Option<T>,
    resnapshot_callback: Option<WrappedCapture<T>>,
    buffer: Vec<BufferedEvent<E>>,
    listener: Option<EventListener<E>>,
    unsubscribe_callback: Option<Box<dyn Fn() + Send + Sync>>,
    epoch: u64,
    resnapshot_state: Option<ResnapshotState<E>>,
    lifecycle: Lifecycle,
}

/// Upstream `BufferedEventWatcher` (`events.ts:165-286`).
struct WatcherCore<T, E: BusEvent> {
    state: Mutex<WatcherState<T, E>>,
    delivery_tail: TailSlot,
    on_error: WatcherOnError<E>,
}

impl<T: Send + Sync + 'static, E: BusEvent> WatcherCore<T, E> {
    fn new(snapshot: Option<T>, on_error: WatcherOnError<E>) -> Self {
        WatcherCore {
            state: Mutex::new(WatcherState {
                snapshot,
                resnapshot_callback: None,
                buffer: Vec::new(),
                listener: None,
                unsubscribe_callback: None,
                epoch: 0,
                resnapshot_state: None,
                lifecycle: Lifecycle::Buffering,
            }),
            delivery_tail: Mutex::new(None),
            on_error,
        }
    }

    /// Upstream `start` (`events.ts:198-207`): callable once; replays the
    /// buffer through the epoch check.
    fn start(self: &Arc<Self>, listener: EventListener<E>) {
        let buffered = {
            let mut state = lock(&self.state);
            if state.lifecycle != Lifecycle::Buffering {
                panic!("WatchHandle.start() may be called only once");
            }
            state.lifecycle = Lifecycle::Started;
            state.listener = Some(listener);
            std::mem::take(&mut state.buffer)
        };
        for buffered in buffered {
            self.enqueue(buffered.event, buffered.context, buffered.epoch);
        }
    }

    /// Upstream `resnapshot` (`events.ts:209-237`): capture a fresh snapshot,
    /// dropping events until the boundary and holding the ones after it.
    pub async fn resnapshot(self: &Arc<Self>, context: Context) -> anyhow::Result<T>
    where
        T: Clone,
    {
        let (capture, boundary) = {
            let mut state = lock(&self.state);
            if state.lifecycle == Lifecycle::Unsubscribed {
                anyhow::bail!("WatchHandle is unsubscribed");
            }
            let capture = state
                .resnapshot_callback
                .clone()
                .ok_or_else(|| anyhow!("WatchHandle does not support resnapshot"))?;
            if state.resnapshot_state.is_some() {
                anyhow::bail!("WatchHandle resnapshot is already in progress");
            }
            state.epoch += 1;
            let (sender, receiver) = tokio::sync::oneshot::channel();
            state.resnapshot_state = Some(ResnapshotState {
                dropping: true,
                held: Vec::new(),
                boundary: Some(sender),
            });
            (capture, receiver)
        };

        let result = match capture(context).await {
            Ok(snapshot) => match boundary.await {
                Ok(()) => Ok(snapshot),
                Err(_) => Err(anyhow!("resnapshot boundary sender dropped")),
            },
            Err(error) => Err(error),
        };

        // Both paths reset the resnapshot state and replay held events
        // (`events.ts:232-236`).
        let held = {
            let mut state = lock(&self.state);
            state
                .resnapshot_state
                .take()
                .map(|resnapshot| resnapshot.held)
                .unwrap_or_default()
        };
        if let Ok(snapshot) = &result {
            lock(&self.state).snapshot = Some(snapshot.clone());
        }
        for (event, context) in held {
            self.push(event, context);
        }
        result
    }

    /// Upstream `markResnapshotBoundary` (`events.ts:239-244`).
    fn mark_resnapshot_boundary(self: &Arc<Self>) {
        let mut state = lock(&self.state);
        if let Some(resnapshot) = &mut state.resnapshot_state {
            if resnapshot.dropping {
                resnapshot.dropping = false;
                if let Some(boundary) = resnapshot.boundary.take() {
                    let _ = boundary.send(());
                }
            }
        }
    }

    /// Upstream `unsubscribe` (`events.ts:246-253`).
    pub fn unsubscribe(&self) {
        let mut state = lock(&self.state);
        if state.lifecycle == Lifecycle::Unsubscribed {
            return;
        }
        state.lifecycle = Lifecycle::Unsubscribed;
        state.buffer.clear();
        state.listener = None;
        if let Some(unsubscribe) = state.unsubscribe_callback.take() {
            drop(state);
            unsubscribe();
        }
    }

    /// Upstream `push` (`events.ts:255-267`).
    pub fn push(self: &Arc<Self>, event: Arc<E>, context: Context) {
        let epoch = {
            let mut state = lock(&self.state);
            if state.lifecycle == Lifecycle::Unsubscribed {
                return;
            }
            if let Some(resnapshot) = &mut state.resnapshot_state {
                if resnapshot.dropping {
                    return;
                }
                resnapshot.held.push((event, context));
                return;
            }
            if state.lifecycle == Lifecycle::Buffering {
                let epoch = state.epoch;
                state.buffer.push(BufferedEvent {
                    event,
                    context,
                    epoch,
                });
                return;
            }
            state.epoch
        };
        self.enqueue(event, context, epoch);
    }

    fn set_unsubscribe(&self, callback: Box<dyn Fn() + Send + Sync>) {
        lock(&self.state).unsubscribe_callback = Some(callback);
    }

    /// Upstream `enqueue` (`events.ts:273-285`): chain the delivery onto the
    /// watcher's own tail through the same atomic take-and-install as the bus
    /// tail; stale epochs and unsubscribed states are skipped at run time,
    /// failures go to `onError`.
    fn enqueue(self: &Arc<Self>, event: Arc<E>, context: Context, epoch: u64) {
        let core = Arc::clone(self);
        // The returned handle is detached (the original spawns fire-and-forget
        // deliveries; nothing awaits the watcher's tail directly).
        let _handle = append_job(&self.delivery_tail, async move {
            let listener = {
                let state = lock(&core.state);
                if state.lifecycle == Lifecycle::Started && state.epoch == epoch {
                    state.listener.clone()
                } else {
                    None
                }
            };
            if let Some(listener) = listener {
                let outcome =
                    catch_listener(|| listener(Arc::clone(&event), context.clone())).await;
                if let Err(payload) = outcome {
                    let error = anyhow!(panic_message(payload));
                    // Upstream swallows onError failures (`events.ts:281-283`).
                    let _ = core.on_error.clone()(error, event, context).await;
                }
            }
        });
    }
}

/// Upstream `WatchHandle<T>` (`agent-harness.ts:181-186`): the watcher handle
/// returned by [`HarnessEventBus::watch`] / [`HarnessEventBus::watch_from_snapshot`].
/// Dropping the handle detaches the watcher from the bus (pushes upgrade a
/// weak reference); call [`WatchHandle::unsubscribe`] for the upstream
/// explicit teardown.
pub struct WatchHandle<T, E: BusEvent> {
    core: Arc<WatcherCore<T, E>>,
}

impl<T, E: BusEvent> fmt::Debug for WatchHandle<T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WatchHandle")
    }
}

impl<T, E: BusEvent> Clone for WatchHandle<T, E> {
    fn clone(&self) -> Self {
        WatchHandle {
            core: Arc::clone(&self.core),
        }
    }
}

impl<T: Send + Sync + 'static, E: BusEvent> WatchHandle<T, E> {
    /// Upstream `setSnapshot` (`events.ts:194-196`); used by
    /// [`Self::watch_from_snapshot`] once the initial capture lands.
    fn set_snapshot(&self, snapshot: T) {
        lock(&self.core.state).snapshot = Some(snapshot);
    }

    /// Upstream `start` (`events.ts:198-207`); may be called only once.
    pub fn start<F>(&self, listener: F)
    where
        F: Fn(Arc<E>, Context) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        self.core.start(Arc::new(listener));
    }

    /// Upstream `resnapshot` (`events.ts:209-237`).
    pub async fn resnapshot(&self, context: Context) -> anyhow::Result<T>
    where
        T: Clone,
    {
        self.core.resnapshot(context).await
    }

    /// Upstream `unsubscribe` (`events.ts:246-253`).
    pub fn unsubscribe(&self) {
        self.core.unsubscribe();
    }
}

impl<T: Clone + Send + Sync + 'static, E: BusEvent> WatchHandle<T, E> {
    /// Upstream `snapshot` (`agent-harness.ts:182`); panics before the
    /// initial snapshot was captured (`watch_from_snapshot` installs it
    /// before returning).
    pub fn snapshot(&self) -> T {
        lock(&self.core.state)
            .snapshot
            .clone()
            .expect("WatchHandle snapshot not installed yet")
    }
}

#[cfg(test)]
mod tests;
