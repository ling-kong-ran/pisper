//! Port of `packages/agent/src/harness/utils/adaptive-publisher.ts` (87
//! lines): publishes the latest state without queuing intermediate
//! mutations.
//!
//! Ported here (M3b Task 6) because it is a private dependency of
//! [`OutputCapture`](super::output_capture::OutputCapture); upstream exports
//! it but no other harness module imports it yet.
//!
//! Disclosed substitutions:
//! - Upstream schedules trailing publications with a `setTimeout` handle; the
//!   port spawns a tokio task per armed timer and guards it with an
//!   `armed` flag. A stale timer firing after a newer publication is a no-op
//!   (the `dirty` gate), so the observable contract — first update after idle
//!   is immediate, later publications rate-limited by encoded size with a
//!   single trailing eventual flush — is unchanged.
//! - Upstream `onError(error: unknown)` receives thrown values; the port's
//!   closure channel is infallible, so panics from
//!   `snapshot`/`update`/`measure`/`publish` are caught with
//!   `catch_unwind` and reported as the panic message (the JS throw-channel
//!   substitution used across the harness port).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::time::sleep;

/// The `snapshot` closure (`adaptive-publisher.ts:2`).
pub type SnapshotFn<TValue> = Box<dyn Fn() -> TValue + Send + Sync>;
/// The `update` diff closure (`adaptive-publisher.ts:3`).
pub type UpdateFn<TValue, TUpdate> =
    Box<dyn Fn(Option<&TValue>, &TValue) -> Option<TUpdate> + Send + Sync>;
/// The `measure` closure (`adaptive-publisher.ts:4`).
pub type MeasureFn<TUpdate> = Box<dyn Fn(&TUpdate) -> usize + Send + Sync>;
/// The `publish` closure (`adaptive-publisher.ts:5`).
pub type PublishFn<TUpdate> = Box<dyn Fn(TUpdate) + Send + Sync>;
/// The `onError` closure (`adaptive-publisher.ts:6`).
pub type OnErrorFn = Box<dyn Fn(String) + Send + Sync>;

/// Upstream `AdaptivePublisherOptions` (`adaptive-publisher.ts:1-9`).
pub struct AdaptivePublisherOptions<TValue, TUpdate> {
    /// Build the current snapshot.
    pub snapshot: SnapshotFn<TValue>,
    /// Diff the previously published snapshot against the current one;
    /// `None` skips publication.
    pub update: UpdateFn<TValue, TUpdate>,
    /// Encoded size of an update, driving the proportional delay.
    pub measure: MeasureFn<TUpdate>,
    /// Deliver one update. Called outside the state lock (upstream commits
    /// before delivery, `adaptive-publisher.ts:63-67`).
    pub publish: PublishFn<TUpdate>,
    /// Receive panics from the publish path.
    pub on_error: OnErrorFn,
    /// Minimum interval between publications (`adaptive-publisher.ts:30`,
    /// default 100).
    pub min_interval_ms: Option<u64>,
    /// Encoded bytes per second pacing budget (`adaptive-publisher.ts:31`,
    /// default 100 * 1024).
    pub target_bytes_per_second: Option<u64>,
}

impl<TValue, TUpdate> fmt::Debug for AdaptivePublisherOptions<TValue, TUpdate> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdaptivePublisherOptions")
            .finish_non_exhaustive()
    }
}

struct PublisherState<TValue> {
    published: Option<TValue>,
    dirty: bool,
    next_emit_at: Option<Instant>,
    timer_armed: bool,
    disposed: bool,
}

/// Upstream `AdaptivePublisher` (`adaptive-publisher.ts:18-87`), generic over
/// the snapshot and update types like upstream.
pub struct AdaptivePublisher<TValue, TUpdate> {
    options: Arc<AdaptivePublisherOptions<TValue, TUpdate>>,
    state: Arc<Mutex<PublisherState<TValue>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<TValue: Send + 'static, TUpdate: Send + 'static> AdaptivePublisher<TValue, TUpdate> {
    /// `new AdaptivePublisher(options)` (`adaptive-publisher.ts:28-32`).
    pub fn new(options: AdaptivePublisherOptions<TValue, TUpdate>) -> Self {
        AdaptivePublisher {
            options: Arc::new(options),
            state: Arc::new(Mutex::new(PublisherState {
                published: None,
                dirty: false,
                next_emit_at: None,
                timer_armed: false,
                disposed: false,
            })),
        }
    }

    /// Upstream `markDirty` (`adaptive-publisher.ts:34-43`): flag the state;
    /// publish immediately when the next scheduled emit is due, otherwise arm
    /// the trailing timer.
    pub fn mark_dirty(&self) {
        let wait = {
            let mut state = lock(&self.state);
            if state.disposed {
                return;
            }
            state.dirty = true;
            match state.next_emit_at {
                Some(next_emit_at) => next_emit_at.saturating_duration_since(Instant::now()),
                None => Duration::ZERO,
            }
        };
        if wait.is_zero() {
            self.flush(false);
            return;
        }
        self.arm_timer(wait);
    }

    /// Upstream `flush` (`adaptive-publisher.ts:45-68`). `force` bypasses the
    /// rate limit (upstream `flush(true)` from `OutputCapture.flush`).
    pub fn flush(&self, force: bool) {
        let update = {
            let mut state = lock(&self.state);
            if state.disposed || !state.dirty {
                return;
            }
            let now = Instant::now();
            if !force {
                if let Some(next_emit_at) = state.next_emit_at {
                    if now < next_emit_at {
                        self.arm_timer_locked(
                            &mut state,
                            next_emit_at.saturating_duration_since(now),
                        );
                        return;
                    }
                }
            }
            state.dirty = false;
            let current = (self.options.snapshot)();
            let update = (self.options.update)(state.published.as_ref(), &current);
            state.published = Some(current);
            match update {
                Some(update) => {
                    let encoded_bytes = (self.options.measure)(&update);
                    let target = self.options.target_bytes_per_second.unwrap_or(100 * 1024);
                    let min_interval = self.options.min_interval_ms.unwrap_or(100);
                    // `Math.max(minIntervalMs, (encodedBytes * 1000) /
                    // targetBytesPerSecond)` (adaptive-publisher.ts:64).
                    let delay = (encoded_bytes as u64 * 1000 / target).max(min_interval);
                    state.next_emit_at = Some(now + Duration::from_millis(delay));
                    Some(update)
                }
                None => None,
            }
        };
        // The update is computed and committed under the lock, delivered
        // after it (upstream: "commit before delivery",
        // adaptive-publisher.ts:63-67). A consumer may panic after applying;
        // the panic is reported through onError instead of unwinding into the
        // caller (upstream: the timer task catches,
        // adaptive-publisher.ts:80-85).
        if let Some(update) = update {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.options.publish)(update);
            }));
            if let Err(error) = result {
                (self.options.on_error)(panic_message(error.as_ref()));
            }
        }
    }

    /// Upstream `dispose` (`adaptive-publisher.ts:70-74`): later
    /// `markDirty`/`flush` calls are no-ops; a pending timer task exits via
    /// the same gate.
    pub fn dispose(&self) {
        lock(&self.state).disposed = true;
    }

    fn arm_timer(&self, wait: Duration) {
        let mut state = lock(&self.state);
        self.arm_timer_locked(&mut state, wait);
    }

    fn arm_timer_locked(&self, state: &mut PublisherState<TValue>, wait: Duration) {
        if state.timer_armed || state.disposed {
            return;
        }
        state.timer_armed = true;
        let state = Arc::clone(&self.state);
        let options = Arc::clone(&self.options);
        tokio::spawn(async move {
            sleep(wait).await;
            let mut guard = lock(&state);
            guard.timer_armed = false;
            if guard.disposed {
                return;
            }
            drop(guard);
            let timer = AdaptivePublisher { options, state };
            timer.flush(false);
        });
    }
}

/// The `String(error)` normalization for caught panics (upstream thrown
/// values flow as `unknown`; the port's throw channel is `catch_unwind`).
pub(crate) fn panic_message(error: &dyn std::any::Any) -> String {
    if let Some(message) = error.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = error.downcast_ref::<String>() {
        return message.clone();
    }
    "panic".to_string()
}

#[cfg(test)]
mod tests;
