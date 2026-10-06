//! Port of `src/session/observation.ts`: the Session-to-Chord bridge — one
//! [`CommittedStateSource`] or [`CommittedWatch`] per attached state, with
//! bounded pending delivery.
//!
//! Divergences (structural, disclosed): upstream drives frame delivery and
//! watch drains on microtasks, and watch listeners are async functions. The
//! port's state source delivers queued frames inline (its listeners are
//! synchronous callbacks), and the watch spawns one drain task whose listener
//! futures run one at a time in arrival order. Ordering, single-delivery,
//! retirement, overflow, and termination semantics are preserved.

use std::sync::{Arc, Mutex, Weak};

use serde_json::Value;
use tokio::sync::Notify;

use crate::agent_core::chord_support::context::Context;
use crate::chord::delta::Op;
use crate::chord::services::errors::ChordError;
use crate::chord::types::{
    ReplicatedStateSource, ReplicatedStateSourceAttachment, ReplicatedStateSourceFrame,
};

use super::super::types::{WatchEnd, WatchEndReason};

/// `ObservedDocumentValue` (`observation.ts:14`): the tracked value or `null`
/// for a retired incarnation.
pub type ObservedDocumentValue = Value;

/// Maximum exact committed frames retained behind one unavailable watch
/// listener (`observation.ts:17`).
const MAX_PENDING_WATCH_FRAMES: usize = 100;

/// Canonical terminal update for a retired document incarnation
/// (`observation.ts` `RETIREMENT_OPERATIONS`): `[["r", null]]`.
pub fn retirement_operations() -> Vec<Op> {
    vec![Op::Replace(Value::Null)]
}

/// One exact-frame observation (`observation.ts` `WatchFrame`).
#[derive(Clone)]
pub struct WatchFrame {
    pub value: ObservedDocumentValue,
    pub ops: Vec<Op>,
    pub context: Context,
}

type FrameListener = Arc<dyn Fn(WatchFrame) + Send + Sync>;

struct StateSourceCore {
    value: Option<ObservedDocumentValue>,
    cursor: usize,
    retired: bool,
    closed: bool,
    attachments: Vec<Arc<SessionSourceAttachment>>,
}

/// Session-to-Chord bridge owned one-to-one by one attached state: a document,
/// or a conversation view. A `null` value retires it (`observation.ts`
/// `CommittedStateSource`).
pub struct CommittedStateSource {
    core: Mutex<StateSourceCore>,
    release: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl CommittedStateSource {
    pub fn new(value: ObservedDocumentValue, release: Box<dyn FnOnce() + Send>) -> Arc<Self> {
        Arc::new(CommittedStateSource {
            core: Mutex::new(StateSourceCore {
                value: Some(value),
                cursor: 0,
                retired: false,
                closed: false,
                attachments: Vec::new(),
            }),
            release: Mutex::new(Some(release)),
        })
    }

    /// `attach()` (`observation.ts:37-47`): one exact-frame attachment with a
    /// bounded pending queue. Upstream throws synchronously on a closed
    /// source; the port returns `Err`.
    pub fn attach(self: &Arc<Self>) -> Result<Arc<SessionSourceAttachment>, ChordError> {
        let mut core = self
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if core.closed {
            return Err(ChordError::Type("State source is closed".to_owned()));
        }
        let snapshot = AttachmentSnapshot {
            value: core.value.clone().unwrap_or(Value::Null),
            cursor: core.cursor,
        };
        let attachment = Arc::new(SessionSourceAttachment {
            snapshot: Mutex::new(snapshot),
            core: Mutex::new(AttachmentCore {
                frames: Vec::new(),
                listener: None,
                activated: false,
                disposed: false,
            }),
            source: Mutex::new(Some(Arc::downgrade(self))),
            self_ref: Mutex::new(None),
        });
        *attachment
            .self_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::downgrade(&attachment));
        core.attachments.push(Arc::clone(&attachment));
        Ok(attachment)
    }

    /// `advance(value, ops, context)` (`observation.ts:49-63`).
    pub fn advance(&self, value: ObservedDocumentValue, ops: Vec<Op>, context: Context) {
        let targets = {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.closed || core.retired {
                return;
            }
            core.value = Some(value.clone());
            core.cursor += 1;
            if value.is_null() {
                core.retired = true;
            }
            let cursor = core.cursor;
            core.attachments
                .iter()
                .map(|attachment| {
                    (
                        Arc::downgrade(attachment),
                        (value.clone(), ops.clone(), cursor, context.clone()),
                    )
                })
                .collect::<Vec<_>>()
        };
        for (attachment, (value, ops, cursor, context)) in targets {
            if let Some(attachment) = attachment.upgrade() {
                attachment.publish(WatchFrame {
                    value,
                    ops,
                    context: with_attachment_cursor(&attachment, cursor, context),
                });
            }
        }
    }

    /// `closeSession()` (`observation.ts:65-71`).
    pub fn close_session(&self) {
        {
            let core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.closed {
                return;
            }
            let attachments = core.attachments.clone();
            for attachment in &attachments {
                attachment.dispose();
            }
        }
        self.finish_disposal();
    }

    fn remove_attachment(&self, attachment: &Arc<SessionSourceAttachment>) {
        let mut core = self
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        core.attachments
            .retain(|candidate| !Arc::ptr_eq(candidate, attachment));
        let empty = core.attachments.is_empty();
        drop(core);
        if empty {
            self.finish_disposal();
        }
    }

    /// `#finishDisposal` (`observation.ts:73-82`).
    fn finish_disposal(&self) {
        {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.closed {
                return;
            }
            core.closed = true;
            core.value = None;
        }
        let release = self
            .release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(release) = release {
            release();
        }
    }
}

/// Frames carry the advancing cursor; the port records it on the attachment
/// snapshot at delivery time.
fn with_attachment_cursor(
    attachment: &SessionSourceAttachment,
    cursor: usize,
    context: Context,
) -> Context {
    attachment
        .snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .cursor = cursor;
    context
}

/// The attachment snapshot (`observation.ts`
/// `SessionSourceAttachment.snapshot`).
#[derive(Clone)]
pub struct AttachmentSnapshot {
    pub value: ObservedDocumentValue,
    pub cursor: usize,
}

struct AttachmentCore {
    frames: Vec<WatchFrame>,
    listener: Option<FrameListener>,
    activated: bool,
    disposed: bool,
}

/// One exact-frame attachment with bounded pending delivery (`observation.ts`
/// `SessionSourceAttachment`).
pub struct SessionSourceAttachment {
    pub snapshot: Mutex<AttachmentSnapshot>,
    core: Mutex<AttachmentCore>,
    source: Mutex<Option<Weak<CommittedStateSource>>>,
    /// Handle back to the canonical `Arc` installable at `attach()` time, so
    /// `dispose()` can detach by pointer identity.
    self_ref: Mutex<Option<Weak<SessionSourceAttachment>>>,
}

impl SessionSourceAttachment {
    /// `activate(listener)` (`observation.ts:89-97`): single-use; upstream
    /// throws on a second activation or a disposed attachment, so the port
    /// returns `Err` with the same messages.
    pub fn activate(&self, listener: FrameListener) -> Result<(), ChordError> {
        {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.activated {
                return Err(ChordError::Type(
                    "State attachment is already active".to_owned(),
                ));
            }
            if core.disposed {
                return Err(ChordError::Type("State attachment is disposed".to_owned()));
            }
            core.activated = true;
            core.listener = Some(listener);
        }
        self.drain();
        Ok(())
    }

    /// `publish(frame)` (`observation.ts:99-117`): queue and drain. The
    /// upstream microtask hop only guards JS reentrancy; the port's
    /// synchronous listeners cannot reenter the publisher.
    pub fn publish(&self, frame: WatchFrame) {
        {
            let mut core = self
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.disposed {
                return;
            }
            core.frames.push(frame);
        }
        self.drain();
    }

    /// `dispose()` (`observation.ts:119-130`).
    pub fn dispose(&self) {
        let mut core = self
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if core.disposed {
            return;
        }
        core.disposed = true;
        core.frames.clear();
        core.listener = None;
        drop(core);
        let source = self
            .source
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .and_then(|weak: Weak<CommittedStateSource>| weak.upgrade());
        let self_arc = self
            .self_ref
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .and_then(|weak: Weak<SessionSourceAttachment>| weak.upgrade());
        if let (Some(source), Some(self_arc)) = (source, self_arc) {
            source.remove_attachment(&self_arc);
        }
    }

    /// `#drain()` (`observation.ts:132-150`).
    fn drain(&self) {
        loop {
            let (listener, frame) = {
                let mut core = self
                    .core
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match (&core.listener, core.frames.is_empty(), core.disposed) {
                    (Some(listener), false, false) => (listener.clone(), core.frames.remove(0)),
                    _ => return,
                }
            };
            listener(frame);
        }
    }
}

/// Watch listener (`observation.ts` `WatchHandle.start`): an async callback
/// over one frame.
pub type WatchListener = Arc<
    dyn Fn(
            ObservedDocumentValue,
            Vec<Op>,
            Context,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

struct WatchCore {
    value: ObservedDocumentValue,
    pending: Vec<WatchFrame>,
    listener: Option<WatchListener>,
    started: bool,
    running: bool,
    detached: bool,
    retired: bool,
    end: Option<WatchEnd>,
}

struct WatchInner {
    core: Mutex<WatchCore>,
    detach: Box<dyn Fn() + Send + Sync>,
    replace: Option<Box<dyn Fn() -> ObservedDocumentValue + Send + Sync>>,
    ended: Mutex<Option<WatchEnd>>,
    notify: Notify,
}

/// Serialized exact-frame watch bound to one document incarnation or
/// conversation view. A `null` value retires it (`observation.ts`
/// `CommittedWatch`).
#[derive(Clone)]
pub struct CommittedWatch {
    inner: Arc<WatchInner>,
}

impl CommittedWatch {
    pub fn new(
        value: ObservedDocumentValue,
        detach: Box<dyn Fn() + Send + Sync>,
        replace: Option<Box<dyn Fn() -> ObservedDocumentValue + Send + Sync>>,
    ) -> Self {
        CommittedWatch {
            inner: Arc::new(WatchInner {
                core: Mutex::new(WatchCore {
                    value,
                    pending: Vec::new(),
                    listener: None,
                    started: false,
                    running: false,
                    detached: false,
                    retired: false,
                    end: None,
                }),
                detach,
                replace,
                ended: Mutex::new(None),
                notify: Notify::new(),
            }),
        }
    }

    /// Acquisition revision before start; latest delivered immutable revision
    /// afterward (`WatchHandle.value`).
    pub fn value(&self) -> ObservedDocumentValue {
        self.inner
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .value
            .clone()
    }

    /// Settle when the watch terminates; an already-running callback remains
    /// caller-owned (`WatchHandle.closed`).
    pub async fn closed(&self) -> WatchEnd {
        loop {
            let notified = self.inner.notify.notified();
            if let Some(end) = self
                .inner
                .ended
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                return end;
            }
            notified.await;
        }
    }

    /// Install the sole asynchronous listener. Never invokes it inline
    /// (`WatchHandle.start`).
    pub fn start(&self, listener: WatchListener) {
        {
            let mut core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.started {
                panic!("Watch is already started");
            }
            if core.end.is_some() {
                panic!("Watch is stopped");
            }
            core.started = true;
            core.listener = Some(listener);
        }
        self.schedule();
    }

    /// Idempotently stop future callbacks (`WatchHandle.stop`).
    pub fn stop(&self) -> WatchEnd {
        self.terminate(WatchEnd::Reason(WatchEndReason::Stopped))
    }

    /// Observe one cancellation token (`observeCancellation`).
    pub fn observe_cancellation(&self, token: tokio_util::sync::CancellationToken) {
        if self.has_ended() {
            return;
        }
        let watch = self.clone();
        let cancel_token = token.clone();
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            watch.cancel();
        });
        if token.is_cancelled() {
            self.cancel();
        }
    }

    fn has_ended(&self) -> bool {
        self.inner
            .core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .end
            .is_some()
    }

    /// `cancel()` (`observation.ts:189-191`).
    pub fn cancel(&self) {
        self.terminate(WatchEnd::Reason(WatchEndReason::Cancelled));
    }

    /// `closeSession()` (`observation.ts:193-195`).
    pub fn close_session(&self) {
        self.terminate(WatchEnd::Reason(WatchEndReason::SessionClosed));
    }

    /// `advance(value, ops, context)` (`observation.ts:197-213`).
    pub fn advance(&self, value: ObservedDocumentValue, ops: Vec<Op>, context: Context) {
        let started = {
            let mut core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.end.is_some() || core.retired {
                return;
            }
            if value.is_null() {
                core.retired = true;
            }
            if core.pending.len() >= MAX_PENDING_WATCH_FRAMES {
                core.pending.clear();
                let replacement = match &self.inner.replace {
                    Some(replace) => replace(),
                    None => value.clone(),
                };
                core.pending.push(WatchFrame {
                    ops: vec![Op::Replace(replacement.clone())],
                    value: replacement,
                    context,
                });
            } else {
                core.pending.push(WatchFrame {
                    value,
                    ops,
                    context,
                });
            }
            core.started
        };
        if started {
            self.schedule();
        }
    }

    /// `#schedule()` (`observation.ts:215-223`): one drain task at a time.
    fn schedule(&self) {
        {
            let core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.running || core.end.is_some() {
                return;
            }
        }
        let watch = self.clone();
        tokio::spawn(async move {
            watch.drain().await;
        });
    }

    /// `#drain()` (`observation.ts:225-241`).
    async fn drain(self) {
        {
            let mut core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.running || core.end.is_some() || !core.started {
                return;
            }
            core.running = true;
        }
        loop {
            let (listener, frame) = {
                let mut core = self
                    .inner
                    .core
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if core.end.is_some() {
                    break;
                }
                match core.pending.is_empty() {
                    true => break,
                    false => {
                        let frame = core.pending.remove(0);
                        core.value = frame.value.clone();
                        (
                            core.listener
                                .clone()
                                .expect("started watches hold a listener"),
                            frame,
                        )
                    }
                }
            };
            let frame_null = frame.value.is_null();
            let future = listener(frame.value, frame.ops, frame.context);
            future.await;
            // `if (frame.value === null) { this.#terminate({ reason: "retired" }); break; }`
            if frame_null {
                let retired = {
                    let core = self
                        .inner
                        .core
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    core.end.is_none()
                };
                if retired {
                    self.terminate(WatchEnd::Reason(WatchEndReason::Retired));
                }
                break;
            }
        }
        {
            let mut core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            core.running = false;
        }
        // `finally` (`observation.ts:236-239`): reschedule for frames that
        // arrived while draining, then settle.
        let more = {
            let core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            !core.pending.is_empty() && core.end.is_none() && core.started
        };
        if more {
            self.schedule();
        }
        self.finish_if_ready();
    }

    /// `#terminate(end)` (`observation.ts:243-250`).
    fn terminate(&self, end: WatchEnd) -> WatchEnd {
        let terminal = {
            let mut core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &core.end {
                Some(existing) => Some(clone_end(existing)),
                None => {
                    core.end = Some(clone_end(&end));
                    core.pending.clear();
                    None
                }
            }
        };
        if let Some(existing) = terminal {
            return existing;
        }
        self.detach_now();
        self.finish_if_ready();
        end
    }

    /// `#detachNow()` (`observation.ts:252-257`).
    fn detach_now(&self) {
        {
            let mut core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if core.detached {
                return;
            }
            core.detached = true;
        }
        (self.inner.detach)();
    }

    /// `#finishIfReady()` (`observation.ts:259-267`).
    fn finish_if_ready(&self) {
        let ended = {
            let mut guard = self
                .inner
                .ended
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let core = self
                .inner
                .core
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.is_none() {
                if let Some(end) = &core.end {
                    *guard = Some(clone_end(end));
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if ended {
            self.inner.notify.notify_waiters();
        }
    }
}

fn clone_end(end: &WatchEnd) -> WatchEnd {
    match end {
        WatchEnd::Reason(reason) => WatchEnd::Reason(*reason),
        WatchEnd::ListenerError { error } => WatchEnd::ListenerError {
            error: Arc::clone(error),
        },
    }
}

/// The JSON object observed by document watches (`types.ts` `DocumentWatch`).
pub type DocumentWatchValue = serde_json::Map<String, Value>;

/// The chord source-attachment adapter over one [`SessionSourceAttachment`]:
/// the trait's `Box<Self>` methods against the port's shared handle, and the
/// frame's cursor read off the attachment snapshot at delivery time
/// (`withAttachmentCursor` puts it there).
struct SourceAttachmentAdapter(Arc<SessionSourceAttachment>);

impl ReplicatedStateSourceAttachment for SourceAttachmentAdapter {
    fn snapshot(&self) -> (crate::chord::types::JsonValue, u64) {
        let snapshot = self
            .0
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (snapshot.value.clone(), snapshot.cursor as u64)
    }

    fn activate(
        self: Box<Self>,
        listener: Box<dyn FnMut(ReplicatedStateSourceFrame) + Send>,
    ) -> Result<(), ChordError> {
        let snapshot = Arc::clone(&self.0);
        // The chord listener contract is `FnMut` (not `Sync`); a shared mutex
        // restores the shared-handle listener's `Sync` bound. Frames arrive
        // one at a time under the attachment core lock, so the lock never
        // contends.
        let listener = Mutex::new(listener);
        self.0.activate(Arc::new(move |frame: WatchFrame| {
            let cursor = snapshot
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .cursor as u64;
            (listener
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()))(
                ReplicatedStateSourceFrame {
                    cursor,
                    value: frame.value,
                    ops: frame.ops,
                    context: frame.context,
                },
            );
        }))
    }

    fn dispose(self: Box<Self>) -> Result<(), ChordError> {
        self.0.dispose();
        Ok(())
    }
}

/// A [`CommittedStateSource`] is an authoritative immutable revision source
/// for the chord `replicatedState(source)` surface (`observation.ts`
/// `CommittedStateSource` implements `ReplicatedStateSource`).
impl ReplicatedStateSource for CommittedStateSource {
    fn attach(
        self: std::sync::Arc<Self>,
    ) -> Result<Box<dyn ReplicatedStateSourceAttachment>, ChordError> {
        Ok(Box::new(SourceAttachmentAdapter(
            CommittedStateSource::attach(&self)?,
        )))
    }
}
