//! Replicated state: the tracked mutable source state published as
//! immutable revisions, publication-only states attached to authoritative
//! sources, and the cold read-only replica consumers hold. Port of
//! `packages/chord/src/services/state.ts` (upstream sha256
//! `6722c58e0816120d4f48a6d58d0d7248c3fe6dd9baf6f7cfd9181a56f2797668`) plus
//! the internals registry of `state-internals.ts` (sha256
//! `1ff3650206497e3c0863f978b358593bacaee437d7e141b80e31327c00629cea`).
//!
//! # Delivery (divergence D2, continuing)
//!
//! Upstream deliveries serialize per subscriber with promise-aware draining
//! (`StateSubscriber.drain` awaits promise-returning listeners; failures
//! are reported in isolation through `reportError`/`queueMicrotask`). The
//! port's listener convention is a synchronous closure that cannot throw,
//! so the 100-pending coalescing and hydrate-preservation on overflow are
//! reproduced over the synchronous queue; listener failure reporting is
//! reachable through the source-attachment `onError` channel, whose
//! failures are values here.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use crate::chord::context::Context;
use crate::chord::delta::{
    apply_immutable, is_base, track, Change, JsonRevisionValidator, Op, Tracker,
};
use crate::chord::types::{
    JsonValue, ReplicatedStateDelivery, ReplicatedStateDeliveryKind, ReplicatedStateSource,
    ReplicatedStateSourceAttachment, ReplicatedStateSourceFrame, ReplicatedStateSourceOptions,
    SourceErrorReporter,
};

use super::errors::ChordError;

type ValueListener = dyn Fn(&JsonValue, &Context, &ReplicatedStateDelivery) + Send + Sync;
pub(crate) type SourceListener =
    dyn Fn(&[Op], u64, &Context) -> Result<(), ChordError> + Send + Sync;

/// One queued delivery (`StateDelivery<T>`, `state.ts:14-19`).
#[derive(Clone)]
struct StateDelivery {
    value: JsonValue,
    context: Context,
    delivery: ReplicatedStateDelivery,
}

/// One public subscription, independent of producer and other subscriber
/// progress. Port of `StateSubscriber<T>` (`state.ts:22-90`).
struct StateSubscriber {
    listener: Arc<ValueListener>,
    pending: VecDeque<StateDelivery>,
    running: bool,
    started: bool,
    closed: bool,
}

const MAX_PENDING: usize = 100;

impl StateSubscriber {
    fn new(listener: Arc<ValueListener>) -> StateSubscriber {
        StateSubscriber {
            listener,
            pending: VecDeque::new(),
            running: false,
            started: false,
            closed: false,
        }
    }

    /// `push` (`state.ts:33-45`): at 100 pending, drop everything except a
    /// still-unstarted hydration.
    fn push(&mut self, frame: StateDelivery) {
        if self.closed {
            return;
        }
        if self.pending.len() == MAX_PENDING {
            // A cold replica can queue updates reentrantly before this
            // subscriber's first hydration starts.
            let hydration = if self.started {
                None
            } else {
                self.pending.front().cloned()
            };
            self.pending.clear();
            if let Some(hydration) = hydration {
                self.pending.push_back(hydration);
            }
        }
        self.pending.push_back(frame);
    }

    /// `drain` (`state.ts:47-68`) over the synchronous closure convention:
    /// the loop never suspends, so `#resume`'s re-queue is unreachable.
    fn drain(&mut self) {
        if self.running || self.closed {
            return;
        }
        self.running = true;
        while let Some(frame) = self.pending.pop_front() {
            self.started = true;
            (self.listener)(&frame.value, &frame.context, &frame.delivery);
        }
        self.running = false;
    }

    /// `clear` (`state.ts:70-72`).
    fn clear(&mut self) {
        self.pending.clear();
    }

    /// `close` (`state.ts:74-77`).
    fn close(&mut self) {
        self.closed = true;
        self.clear();
    }
}

/// A published revision awaiting delivery (`Publication<T>`,
/// `state.ts:96-102`).
struct Publication {
    value: JsonValue,
    ops: Vec<Op>,
    sequence: u64,
    context: Context,
}

/// Maintains local publication order independently of how revisions are
/// produced. Port of `ReplicatedStatePublisher<T>` (`state.ts:105-190`).
pub(crate) struct ReplicatedStatePublisher {
    /// Identity for the publication-reentrancy window (see
    /// [`publishing_snapshot`]).
    id: u64,
    /// Insertion-ordered subscriber → hydrated sequence
    /// (`#listeners: Map<StateSubscriber, number>`).
    listeners: Vec<(u64, Arc<Mutex<StateSubscriber>>, u64)>,
    /// Upstream keeps the reporter on the publisher for value-listener
    /// failures; the synchronous closure convention cannot raise them, so
    /// the field is construction-only (the attachment's `onError` carries
    /// the reachable failures).
    #[allow(dead_code)]
    report_error: SourceErrorReporter,
    source_listeners: Vec<(u64, Arc<SourceListener>)>,
    publications: VecDeque<Publication>,
    value: JsonValue,
    sequence: u64,
    delivering: bool,
}

static NEXT_PUBLISHER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    /// Upstream JS is single-threaded and reentrant: `#snapshot()` taken from
    /// inside a publication listener sees the just-published state. The port
    /// holds the publisher lock across listener invocation, so the in-flight
    /// snapshot is parked here for the duration of a publish.
    static PUBLISHING_SNAPSHOT: std::cell::RefCell<Option<(u64, JsonValue, u64)>> =
        const { std::cell::RefCell::new(None) };
}

/// The `(value, sequence)` pair a reentrant `snapshot()` observes on the
/// publishing thread, or `None` outside a publication.
fn publishing_snapshot(publisher_id: u64) -> Option<(JsonValue, u64)> {
    PUBLISHING_SNAPSHOT.with(|at| {
        at.borrow()
            .iter()
            .filter(|(id, _, _)| *id == publisher_id)
            .map(|(_, value, sequence)| (value.clone(), *sequence))
            .next()
    })
}

impl ReplicatedStatePublisher {
    pub(crate) fn new(
        initial: JsonValue,
        report_error: SourceErrorReporter,
    ) -> ReplicatedStatePublisher {
        ReplicatedStatePublisher {
            id: NEXT_PUBLISHER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            listeners: Vec::new(),
            report_error,
            source_listeners: Vec::new(),
            publications: VecDeque::new(),
            value: initial,
            sequence: 0,
            delivering: false,
        }
    }

    /// `get value` (`state.ts:124-126`).
    pub(crate) fn value(&self) -> JsonValue {
        self.value.clone()
    }

    /// `snapshot()` (`state.ts:128-130`): atomically capture the immutable
    /// value and its matching publication sequence (`state-internals.ts`).
    pub(crate) fn snapshot(&self) -> (JsonValue, u64) {
        (self.value.clone(), self.sequence)
    }

    /// `subscribe` (`state.ts:132-144`): queue the hydrate before any
    /// later publication can, then drain once.
    pub(crate) fn subscribe(&mut self, token: u64, listener: Arc<ValueListener>) {
        let (value, sequence) = self.snapshot();
        let subscriber = Arc::new(Mutex::new(StateSubscriber::new(listener)));
        self.listeners
            .push((token, Arc::clone(&subscriber), sequence));
        {
            let mut inner = subscriber
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.push(StateDelivery {
                value,
                context: service_delivery_context(),
                delivery: ReplicatedStateDelivery {
                    kind: ReplicatedStateDeliveryKind::Hydrate,
                    sequence,
                },
            });
            inner.drain();
        }
    }

    /// `subscribeSource` (`state.ts:146-149`).
    pub(crate) fn subscribe_source(&mut self, token: u64, listener: Arc<SourceListener>) {
        self.source_listeners.push((token, listener));
    }

    pub(crate) fn remove_listener(&mut self, token: u64) {
        if let Some(at) = self.listeners.iter().position(|(at, _, _)| *at == token) {
            self.listeners.remove(at);
        }
    }

    pub(crate) fn remove_source_listener(&mut self, token: u64) {
        self.source_listeners.retain(|(at, _)| *at != token);
    }

    /// `publish` (`state.ts:151-189`): publish an already-prepared
    /// immutable revision; source-listener failures are returned isolated.
    pub(crate) fn publish(
        &mut self,
        value: JsonValue,
        ops: Vec<Op>,
        context: &Context,
    ) -> Vec<ChordError> {
        self.value = value;
        self.sequence += 1;
        self.publications.push_back(Publication {
            value: self.value.clone(),
            ops,
            sequence: self.sequence,
            context: context.clone(),
        });
        if self.delivering {
            return Vec::new();
        }
        self.delivering = true;
        let token = (self.id, self.value.clone(), self.sequence);
        PUBLISHING_SNAPSHOT.with(|at| *at.borrow_mut() = Some(token));
        let mut errors: Vec<ChordError> = Vec::new();
        while let Some(publication) = self.publications.pop_front() {
            for (_, listener) in self.source_listeners.clone() {
                match listener(&publication.ops, publication.sequence, &publication.context) {
                    Ok(()) => {}
                    Err(error) => errors.push(error),
                }
            }
            let delivery = ReplicatedStateDelivery {
                kind: ReplicatedStateDeliveryKind::Update,
                sequence: publication.sequence,
            };
            for (_, subscriber, hydrated_sequence) in self.listeners.clone() {
                if publication.sequence <= hydrated_sequence {
                    continue;
                }
                let mut inner = subscriber
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                inner.push(StateDelivery {
                    value: publication.value.clone(),
                    context: publication.context.clone(),
                    delivery,
                });
                inner.drain();
            }
        }
        PUBLISHING_SNAPSHOT.with(|at| *at.borrow_mut() = None);
        self.delivering = false;
        errors
    }
}

/// `serviceDeliveryContext()` (`state.ts:429-433`): the synthetic context
/// for service deliveries without a caller.
pub fn service_delivery_context() -> Context {
    Context::background()
}

struct MutableInner {
    publisher: ReplicatedStatePublisher,
    next_listener: u64,
}

/// Port of `MutableReplicatedStateImpl` (`state.ts:193-266`). One
/// synchronous overlay mutation publishes atomically through
/// [`MutableReplicatedState::change`]; whole-value replacements through
/// [`MutableReplicatedState::replace`]. Cloned handles share one state;
/// listeners unsubscribe by closure, like the upstream `() => void`.
pub struct MutableReplicatedState {
    tracker: Tracker,
    inner: Mutex<MutableInner>,
    /// Lock-free identity for the publication-reentrancy window
    /// (see [`publishing_snapshot`]).
    publisher_id: u64,
    changing: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for MutableReplicatedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutableReplicatedState")
            .field("sequence", &self.sequence())
            .finish()
    }
}

impl MutableReplicatedState {
    /// `new MutableReplicatedStateImpl(initial)` (`state.ts:207-218`). The
    /// initial value seeds the publisher; publications happen on the first
    /// `change`/`replace` (upstream no longer flushes a base batch here —
    /// the subscription snapshot carries `["r", value]`).
    pub fn new(initial: JsonValue) -> Arc<Self> {
        let tracker = track(initial);
        let value = tracker.value();
        let publisher = ReplicatedStatePublisher::new(value, default_error_reporter());
        let publisher_id = publisher.id;
        Arc::new(MutableReplicatedState {
            tracker,
            inner: Mutex::new(MutableInner {
                publisher,
                next_listener: 0,
            }),
            publisher_id,
            changing: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MutableInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `get value` (`state.ts:220-222`): the tracker's current revision.
    pub fn value(&self) -> JsonValue {
        self.tracker.value()
    }

    /// `change(context, mutate)` (`state.ts:224-250`): atomically publish
    /// one synchronous overlay mutation. The callback receives the draft
    /// (upstream `Draft<T>`); returning `Err` aborts the change the way a
    /// thrown callback does upstream.
    pub fn change<F>(&self, context: &Context, mutate: F) -> Result<(), ChordError>
    where
        F: FnOnce(&mut Change) -> Result<(), ChordError>,
    {
        if self
            .changing
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
        {
            return Err(ChordError::Type(
                "Replicated state cannot be changed reentrantly from a change callback".to_owned(),
            ));
        }
        let result = self.change_inner(context, mutate);
        self.changing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    fn change_inner<F>(&self, context: &Context, mutate: F) -> Result<(), ChordError>
    where
        F: FnOnce(&mut Change) -> Result<(), ChordError>,
    {
        let mut change = self.tracker.begin_change();
        if let Err(error) = mutate(&mut change) {
            change.abort();
            return Err(error);
        }
        let prepared = match change.prepare() {
            Ok(prepared) => prepared,
            Err(error) => return Err(ChordError::Type(error.message())),
        };
        self.adopt_and_publish(context, prepared)
    }

    /// `replace(context, value)` (`state.ts:252-265`): atomically take
    /// immutable ownership of an alias-free strict-JSON replacement.
    pub fn replace(&self, context: &Context, value: JsonValue) -> Result<(), ChordError> {
        if self
            .changing
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_err()
        {
            return Err(ChordError::Type(
                "Replicated state cannot be replaced from a change callback".to_owned(),
            ));
        }
        let result = (|| {
            let prepared = match self.tracker.prepare_replace(value) {
                Ok(prepared) => prepared,
                Err(error) => return Err(ChordError::Type(error.message())),
            };
            self.adopt_and_publish(context, prepared)
        })();
        self.changing
            .store(false, std::sync::atomic::Ordering::SeqCst);
        result
    }

    /// `adopt` + conditional publication shared by `change`/`replace`
    /// (`state.ts:249-250, 262-264`).
    fn adopt_and_publish(
        &self,
        context: &Context,
        prepared: crate::chord::delta::Prepared,
    ) -> Result<(), ChordError> {
        let value = prepared.value.clone();
        let ops = prepared.ops.clone();
        self.tracker
            .adopt(prepared)
            .map_err(|error| ChordError::Type(error.message()))?;
        if ops.is_empty() {
            return Ok(());
        }
        let errors = self.lock().publisher.publish(value, ops, context);
        throw_collected_errors(errors, "Replicated state listeners failed")
    }

    /// `subscribe(listener)` (`state.ts:267-269`).
    pub fn subscribe<F>(self: &Arc<Self>, listener: F) -> Box<dyn Fn() + Send + Sync>
    where
        F: Fn(&JsonValue, &Context, &ReplicatedStateDelivery) + Send + Sync + 'static,
    {
        let token = {
            let mut inner = self.lock();
            let token = inner.next_listener;
            inner.next_listener += 1;
            token
        };
        self.lock().publisher.subscribe(token, Arc::new(listener));
        let weak: Weak<MutableReplicatedState> = Arc::downgrade(self);
        Box::new(move || {
            if let Some(state) = weak.upgrade() {
                state.lock().publisher.remove_listener(token);
            }
        })
    }

    // Provider-side internals (state-internals.ts).

    /// `snapshot()` (`state-internals.ts`): `(value, sequence)`.
    pub fn snapshot(&self) -> (JsonValue, u64) {
        // Reentrant snapshot from inside a publication listener: upstream's
        // single-threaded publisher already installed the new state.
        if let Some(token) = publishing_snapshot(self.publisher_id) {
            return token;
        }
        self.lock().publisher.snapshot()
    }

    /// `get sequence` — kept for diagnostics; the provider reads the
    /// snapshot pair.
    pub fn sequence(&self) -> u64 {
        self.lock().publisher.snapshot().1
    }

    /// `subscribe(listener)` over raw operation batches
    /// (`state-internals.ts`). Returns the unsubscribe closure.
    pub fn subscribe_source<F>(self: &Arc<Self>, listener: F) -> Box<dyn Fn() + Send + Sync>
    where
        F: Fn(&[Op], u64, &Context) -> Result<(), ChordError> + Send + Sync + 'static,
    {
        let token = {
            let mut inner = self.lock();
            let token = inner.next_listener;
            inner.next_listener += 1;
            inner.publisher.subscribe_source(token, Arc::new(listener));
            token
        };
        let weak: Weak<MutableReplicatedState> = Arc::downgrade(self);
        Box::new(move || {
            if let Some(state) = weak.upgrade() {
                state.lock().publisher.remove_source_listener(token);
            }
        })
    }
}

/// The upstream default `reportErrorAsync` rethrows on a microtask; the
/// port has no uncaught-exception channel, so the default reporter drops.
pub(crate) fn default_error_reporter() -> SourceErrorReporter {
    Arc::new(|_| {})
}

struct AttachedInner {
    publisher: ReplicatedStatePublisher,
    cursor: u64,
    disposed: bool,
}

/// Port of `AttachedReplicatedStateImpl` (`state.ts:269-351`): a
/// synchronously hydrated publication-only state backed by one source
/// attachment.
pub struct AttachedReplicatedState {
    attachment: Mutex<Option<Box<dyn ReplicatedStateSourceAttachment>>>,
    inner: Mutex<AttachedInner>,
    report_error: SourceErrorReporter,
    next_listener: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for AttachedReplicatedState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachedReplicatedState").finish()
    }
}

impl AttachedReplicatedState {
    fn new(
        attachment: Box<dyn ReplicatedStateSourceAttachment>,
        options: ReplicatedStateSourceOptions,
    ) -> Result<Arc<AttachedReplicatedState>, ChordError> {
        let (value, cursor) = attachment.snapshot();
        assert_cursor(cursor, "snapshot")?;
        let report_error = options.on_error.unwrap_or_else(default_error_reporter);
        Ok(Arc::new(AttachedReplicatedState {
            attachment: Mutex::new(Some(attachment)),
            inner: Mutex::new(AttachedInner {
                publisher: ReplicatedStatePublisher::new(value, Arc::clone(&report_error)),
                cursor,
                disposed: false,
            }),
            report_error,
            next_listener: std::sync::atomic::AtomicU64::new(0),
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, AttachedInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `get value` (`state.ts:292-294`).
    pub fn value(&self) -> JsonValue {
        self.lock().publisher.value()
    }

    /// `subscribe(listener)` (`state.ts:296-298`).
    pub fn subscribe<F>(self: &Arc<Self>, listener: F) -> Box<dyn Fn() + Send + Sync>
    where
        F: Fn(&JsonValue, &Context, &ReplicatedStateDelivery) + Send + Sync + 'static,
    {
        let token = self
            .next_listener
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.lock().publisher.subscribe(token, Arc::new(listener));
        let weak: Weak<AttachedReplicatedState> = Arc::downgrade(self);
        Box::new(move || {
            if let Some(state) = weak.upgrade() {
                state.lock().publisher.remove_listener(token);
            }
        })
    }

    /// `activate()` (`state.ts:300-304`): install the frame listener;
    /// buffered frames drain synchronously.
    pub fn activate(self: &Arc<Self>) -> Result<(), ChordError> {
        let attachment = self
            .attachment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or_else(|| {
                ChordError::Type(
                    "Replicated state source attachment has already been activated".to_owned(),
                )
            })?;
        let state = Arc::downgrade(self);
        attachment.activate(Box::new(move |frame| {
            let Some(state) = state.upgrade() else { return };
            state.receive(frame);
        }))
    }

    /// `dispose()` (`state.ts:306-313`): idempotently release the source
    /// attachment. The last published value remains readable.
    pub fn dispose(&self) {
        {
            let mut inner = self.lock();
            if inner.disposed {
                return;
            }
            inner.disposed = true;
        }
        if let Some(attachment) = self
            .attachment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = attachment.dispose();
        }
    }

    /// `#receive(frame)` (`state.ts:315-341`): cursor-gap enforcement and
    /// publication; contract failures dispose the state and report.
    fn receive(self: &Arc<Self>, frame: ReplicatedStateSourceFrame) {
        if self.lock().disposed {
            return;
        }
        let outcome = (|| -> Result<(), ChordError> {
            assert_cursor(frame.cursor, "frame")?;
            let expected = self.lock().cursor + 1;
            if frame.cursor != expected {
                return Err(ChordError::Type(format!(
                    "Replicated state source cursor has a gap: expected {expected}, received {}",
                    frame.cursor
                )));
            }
            self.lock().cursor = frame.cursor;
            let errors = self
                .lock()
                .publisher
                .publish(frame.value, frame.ops, &frame.context);
            if errors.len() == 1 {
                (self.report_error)(&errors[0]);
            } else if errors.len() > 1 {
                (self.report_error)(&ChordError::Aggregate {
                    message: "Replicated state listeners failed".to_owned(),
                    errors,
                });
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            self.fail(error);
        }
    }

    /// `#fail(error)` (`state.ts:343-358`).
    fn fail(self: &Arc<Self>, error: ChordError) {
        {
            let mut inner = self.lock();
            if inner.disposed {
                return;
            }
            inner.disposed = true;
        }
        if let Some(attachment) = self
            .attachment
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            match attachment.dispose() {
                Ok(()) => {}
                Err(dispose_error) => {
                    (self.report_error)(&ChordError::Aggregate {
                        message: "Replicated state source contract failed".to_owned(),
                        errors: vec![error, dispose_error],
                    });
                    return;
                }
            }
        }
        (self.report_error)(&error);
    }
}

/// `assertCursor` (`state.ts:432-436`): cursors must be safe integers —
/// the port's `u64` cursors are integers by construction, so only the
/// positive-range check remains.
fn assert_cursor(cursor: u64, kind: &str) -> Result<(), ChordError> {
    if cursor > i64::MAX as u64 {
        return Err(ChordError::Type(format!(
            "Replicated state source {kind} cursor must be a safe integer"
        )));
    }
    Ok(())
}

/// `throwCollectedErrors` (`state.ts:439-443`).
pub(crate) fn throw_collected_errors(
    errors: Vec<ChordError>,
    message: &str,
) -> Result<(), ChordError> {
    if errors.len() == 1 {
        return Err(errors.into_iter().next().expect("checked above"));
    }
    if errors.len() > 1 {
        return Err(ChordError::Aggregate {
            message: message.to_owned(),
            errors,
        });
    }
    Ok(())
}

/// `attachReplicatedStateSource(source, options)` (`state.ts:354-373`):
/// attach a publication-only replicated state to one authoritative
/// immutable source stream. Constructor/activation failures dispose the
/// attachment and aggregate, exactly like upstream.
pub fn attach_replicated_state_source(
    source: Arc<dyn ReplicatedStateSource>,
    options: ReplicatedStateSourceOptions,
) -> Result<Arc<AttachedReplicatedState>, ChordError> {
    let attachment = source.attach()?;
    let state = AttachedReplicatedState::new(attachment, options)?;
    if let Err(error) = state.activate() {
        state.dispose();
        return Err(ChordError::Aggregate {
            message: "Failed to attach replicated state source".to_owned(),
            errors: vec![error],
        });
    }
    Ok(state)
}

struct ReplicaInner {
    value: Option<JsonValue>,
    sequence: Option<u64>,
    /// Insertion-ordered subscribers (`Set<StateSubscriber>` upstream).
    subscribers: Vec<Arc<Mutex<StateSubscriber>>>,
}

/// Port of `ReplicatedStateReplica` (`state.ts:376-425`): a cold read-only
/// state used by service consumers until a complete snapshot arrives. Every
/// revision passes through the [`JsonRevisionValidator`] before
/// publication; a failed application clears the replica and fails.
pub struct ReplicatedStateReplica {
    inner: Mutex<ReplicaInner>,
}

impl std::fmt::Debug for ReplicatedStateReplica {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplicatedStateReplica").finish()
    }
}

impl ReplicatedStateReplica {
    /// `new ReplicatedStateReplica(reportError)` (`state.ts:384-386`); the
    /// report-error channel only carries listener failures, which the
    /// synchronous convention cannot raise.
    pub fn new() -> Arc<Self> {
        Arc::new(ReplicatedStateReplica {
            inner: Mutex::new(ReplicaInner {
                value: None,
                sequence: None,
                subscribers: Vec::new(),
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ReplicaInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `get value` (`state.ts:388-390`).
    pub fn value(&self) -> Option<JsonValue> {
        self.lock().value.clone()
    }

    /// `subscribe(listener)` (`state.ts:392-409`): a hydrated replica
    /// delivers its current revision as a hydrate.
    pub fn subscribe<F>(self: &Arc<Self>, listener: F) -> Box<dyn Fn() + Send + Sync>
    where
        F: Fn(&JsonValue, &Context, &ReplicatedStateDelivery) + Send + Sync + 'static,
    {
        let subscriber = {
            let mut inner = self.lock();
            let subscriber = Arc::new(Mutex::new(StateSubscriber::new(Arc::new(listener))));
            inner.subscribers.push(Arc::clone(&subscriber));
            if let (Some(value), Some(sequence)) = (&inner.value, inner.sequence) {
                subscriber
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(StateDelivery {
                        value: value.clone(),
                        context: service_delivery_context(),
                        delivery: ReplicatedStateDelivery {
                            kind: ReplicatedStateDeliveryKind::Hydrate,
                            sequence,
                        },
                    });
            }
            subscriber
        };
        subscriber
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain();
        let weak: Weak<ReplicatedStateReplica> = Arc::downgrade(self);
        Box::new(move || {
            if let Some(replica) = weak.upgrade() {
                let mut inner = replica.lock();
                inner.subscribers.retain(|at| !Arc::ptr_eq(at, &subscriber));
            }
            subscriber
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .close();
        })
    }

    /// `hydrate(sequence, ops, context)` (`state.ts:411-424`).
    pub fn hydrate(&self, sequence: u64, ops: &[Op], context: &Context) -> Result<(), ChordError> {
        // Only hydration requires a base batch (`state.ts:414-416`).
        let next = self.apply_revision(None, ops, true)?;
        {
            let mut inner = self.lock();
            inner.sequence = Some(sequence);
            inner.value = Some(next);
        }
        self.deliver_all(context, ReplicatedStateDeliveryKind::Hydrate, sequence);
        Ok(())
    }

    /// `update(sequence, ops, context)` (`state.ts:426-439`): a sequence
    /// gap clears the replica and fails, exactly like upstream.
    pub fn update(&self, sequence: u64, ops: &[Op], context: &Context) -> Result<(), ChordError> {
        let current = {
            let inner = self.lock();
            match (inner.sequence, inner.value.as_ref()) {
                (Some(current), Some(_)) => current,
                _ => {
                    return Err(ChordError::Type(
                        "Replicated state received an update before hydration".to_owned(),
                    ))
                }
            }
        };
        if sequence != current + 1 {
            self.clear();
            return Err(ChordError::Type(
                "Replicated state update sequence has a gap".to_owned(),
            ));
        }
        let previous = self.lock().value.clone();
        let next = self.apply_revision(previous.as_ref(), ops, false)?;
        {
            let mut inner = self.lock();
            inner.sequence = Some(sequence);
            inner.value = Some(next);
        }
        self.deliver_all(context, ReplicatedStateDeliveryKind::Update, sequence);
        Ok(())
    }

    /// The validated application both entry points share
    /// (`state.ts:414-418, 429-433`): a failed application clears the
    /// replica before the error propagates.
    fn apply_revision(
        &self,
        target: Option<&JsonValue>,
        ops: &[Op],
        require_base: bool,
    ) -> Result<JsonValue, ChordError> {
        let validator = JsonRevisionValidator;
        let attempt = || -> Result<JsonValue, ChordError> {
            if require_base && !is_base(ops) {
                return Err(ChordError::Type(
                    "Replicated state snapshot is not a base operation batch".to_owned(),
                ));
            }
            let applied =
                apply_immutable(target, ops).map_err(|error| ChordError::Type(error.message()))?;
            Ok(validator.validate(&applied))
        };
        match attempt() {
            Ok(value) => Ok(value),
            Err(error) => {
                self.clear();
                Err(error)
            }
        }
    }

    /// `clear()` (`state.ts:441-445`): also drops every subscriber's
    /// pending deliveries.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.value = None;
        inner.sequence = None;
        for subscriber in &inner.subscribers {
            subscriber
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
    }

    /// `#deliverAll` (`state.ts:447-456`): enqueue for everyone before user
    /// code can publish another revision reentrantly, then drain.
    fn deliver_all(&self, context: &Context, kind: ReplicatedStateDeliveryKind, sequence: u64) {
        let (frame, subscribers) = {
            let inner = self.lock();
            let Some(value) = inner.value.clone() else {
                return;
            };
            (
                StateDelivery {
                    value,
                    context: context.clone(),
                    delivery: ReplicatedStateDelivery { kind, sequence },
                },
                inner.subscribers.clone(),
            )
        };
        for subscriber in &subscribers {
            subscriber
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(frame.clone());
        }
        for subscriber in &subscribers {
            subscriber
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .drain();
        }
    }
}
