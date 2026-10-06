//! Port of `src/harness/util.ts`: the pending-wait registry shared by the
//! submission and task wait surfaces, plus the paginated scan drain.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::context::Context;

/// `closedError()` (`harness/util.ts:57-59`): the error every wait rejects
/// with once the Harness closes.
pub fn closed_error() -> CloseError {
    CloseError
}

/// The `Error("Harness is closed")` wait rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseError;

impl std::fmt::Display for CloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Harness is closed")
    }
}

impl std::error::Error for CloseError {}

/// One registered waiter: its reply channel plus the handle `resolve` /
/// `reject_all` use to settle it. Dropping the sender rejects the wait.
struct Waiter<T> {
    tx: Mutex<Option<oneshot::Sender<T>>>,
    /// Fired when the waiter is settled by `resolve` (value already sent) or
    /// removed by `reject_all`; the parked `add` future also selects on the
    /// caller's abort signal.
    settled: CancellationToken,
}

struct WaitersInner<T> {
    sets: HashMap<String, Vec<Arc<Waiter<T>>>>,
}

/// Pending waits by key (`harness/util.ts` `Waiters`). Each settles once:
/// through [`Waiters::resolve`], [`Waiters::reject_all`], or cancellation of
/// its context.
///
/// Divergence (structural, disclosed): upstream keys are arbitrary values and
/// waiter identities are promise objects; the port hashes the key by its
/// encoded bytes (every upstream key is an ID or name) and identifies waiters
/// by handle.
pub struct Waiters<T> {
    inner: Arc<Mutex<WaitersInner<T>>>,
}

impl<T> Default for Waiters<T> {
    fn default() -> Self {
        Waiters {
            inner: Arc::new(Mutex::new(WaitersInner {
                sets: HashMap::new(),
            })),
        }
    }
}

impl<T: Clone + Send + 'static> Waiters<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `add(key, context)` (`harness/util.ts:12-33`): register one waiter.
    /// An already-aborted context returns [`WaiterError::Cancelled`]
    /// immediately (upstream rejects with `signal.reason`).
    pub async fn add(&self, key: &str, context: &Context) -> Result<T, WaiterError> {
        self.register(key, context)?.wait().await
    }

    /// The registration half of [`Waiters::add`]: enqueue the waiter without
    /// awaiting it. The scheduler and submissions register on the Session
    /// line and settle outside it, so the port splits the upstream
    /// promise-returning `add` in two (divergence disclosed in the scheduler
    /// module docs).
    pub fn register(&self, key: &str, context: &Context) -> Result<WaiterHandle<T>, WaiterError> {
        if let Some(signal) = context.abort_signal() {
            if signal.is_cancelled() {
                return Err(WaiterError::Cancelled);
            }
        }
        let (tx, rx) = oneshot::channel::<T>();
        let waiter = Arc::new(Waiter {
            tx: Mutex::new(Some(tx)),
            settled: CancellationToken::new(),
        });
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner
                .sets
                .entry(key.to_string())
                .or_default()
                .push(Arc::clone(&waiter));
        }
        let waiter_for_detach = Arc::clone(&waiter);
        let key = key.to_owned();
        let inner = Arc::clone(&self.inner);
        Ok(WaiterHandle {
            rx,
            settled: waiter.settled.clone(),
            abort: context.abort_signal(),
            detach: Box::new(move || {
                let mut inner = inner
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(set) = inner.sets.get_mut(&key) {
                    set.retain(|candidate| !Arc::ptr_eq(candidate, &waiter_for_detach));
                    if set.is_empty() {
                        inner.sets.remove(&key);
                    }
                }
            }),
        })
    }

    #[allow(dead_code)]
    fn detach(&self, key: &str, waiter: &Arc<Waiter<T>>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(set) = inner.sets.get_mut(key) {
            set.retain(|candidate| !Arc::ptr_eq(candidate, waiter));
            if set.is_empty() {
                inner.sets.remove(key);
            }
        }
    }

    /// `keys()` (`harness/util.ts:35-37`): pending keys in insertion order.
    pub fn keys(&self) -> Vec<String> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.sets.keys().cloned().collect()
    }

    /// `resolve(key, value)` (`harness/util.ts:39-44`): settle and drop every
    /// waiter of `key` with `value`.
    pub fn resolve(&self, key: &str, value: T) {
        let entries = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.sets.remove(key).unwrap_or_default()
        };
        for waiter in entries {
            let sender = waiter
                .tx
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(tx) = sender {
                let _ = tx.send(value.clone());
            }
            waiter.settled.cancel();
        }
    }

    /// `rejectAll(error)` (`harness/util.ts:46-52`): reject every pending
    /// waiter of every key.
    pub fn reject_all(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sets = std::mem::take(&mut inner.sets);
        for (_, set) in sets {
            for waiter in set {
                waiter
                    .tx
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                waiter.settled.cancel();
            }
        }
    }
}

/// One registered waiter awaiting settlement (`Waiters::add` split in two):
/// the reply channel, the shared settled flag, the caller's abort signal, and
/// the detach that removes it from its set.
pub struct WaiterHandle<T> {
    rx: oneshot::Receiver<T>,
    settled: CancellationToken,
    abort: Option<tokio_util::sync::CancellationToken>,
    detach: Box<dyn FnOnce() + Send>,
}

impl<T: Clone + Send + 'static> WaiterHandle<T> {
    /// Await settlement (`add`'s tail): the reply, the shared settle, or the
    /// caller's abort (`onAbort`: remove the waiter, drop the empty set,
    /// reject).
    pub async fn wait(self) -> Result<T, WaiterError> {
        let WaiterHandle {
            rx,
            settled,
            abort,
            detach,
        } = self;
        // Biased, value first: `resolve` sends the value and then cancels the
        // settled token, so both are ready when the waiter is woken after
        // settlement — the value must win (upstream resolves the promise).
        tokio::select! {
            biased;
            result = rx => result.map_err(|_| WaiterError::Cancelled),
            _ = settled.cancelled() => Err(WaiterError::Cancelled),
            _ = wait_for(abort) => {
                detach();
                settled.cancel();
                Err(WaiterError::Cancelled)
            }
        }
    }
}

async fn wait_for(token: Option<tokio_util::sync::CancellationToken>) {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

/// Failure of a [`Waiters`] wait: the context was cancelled or the wait was
/// dropped by [`Waiters::reject_all`] / Harness close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaiterError {
    Cancelled,
}

impl From<WaiterError> for super::errors::PlainError {
    fn from(error: WaiterError) -> Self {
        let _ = error;
        super::errors::PlainError::new(closed_error().to_string())
    }
}

/// `scanAll(scan)` (`harness/util.ts:54-63`): every item of a paginated scan,
/// in page order, following `page.next` until it is absent.
pub async fn scan_all<T, Fut, F>(mut scan: F) -> Vec<T>
where
    F: FnMut(Option<super::types::Cursor>) -> Fut,
    Fut: Future<Output = super::types::Page<T>>,
{
    let mut items = Vec::new();
    let mut cursor: Option<super::types::Cursor> = None;
    loop {
        let page = scan(cursor).await;
        let next = page.next;
        items.extend(page.items);
        cursor = next;
        if cursor.is_none() {
            break;
        }
    }
    items
}
