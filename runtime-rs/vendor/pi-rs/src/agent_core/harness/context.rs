//! Port of `packages/agent/src/harness/context.ts` (37 lines): the chord
//! context surface the harness subtree imports, plus the abort/cancel helpers
//! deferred from M3b Task 1 (`packages/chord/src/context/index.ts:82-124`).
//!
//! Upstream re-exports the chord context vocabulary
//! (`context.ts:14-25`); the port re-exports the same items from
//! [`crate::agent_core::chord_support::context`] and adds thin
//! constructors for the two upstream constants:
//! [`background_context`] (`BACKGROUND_CONTEXT`) and [`todo_context`]
//! (`TODO_CONTEXT`).
//!
//! `AbortSignal` binding: the port's convention is
//! [`tokio_util::sync::CancellationToken`] (same substitution as the ai
//! layer's stream surface, disclosed in `agent_core/types.rs`). This shapes
//! the four helpers:
//! - [`with_abort_signal`] — upstream combines the parent signal with
//!   `AbortSignal.any`; `CancellationToken` has no value-level union, so the
//!   port derives a fresh token and spawns a small linker task that cancels
//!   it when either source cancels. Requires a tokio runtime (all harness
//!   operation code runs inside one). The linker task lives until one of its
//!   sources cancels — bounded by the number of derivations per operation,
//!   the same lifetime an upstream `AbortSignal.any` listener set has.
//! - [`without_abort_signal`] — stores an explicit `None` over the parent
//!   chain (upstream `withContextValue(key, undefined)`), so lookups stop
//!   there.
//! - [`with_cancel`] — an independently cancellable child context; the
//!   returned token is both the stored signal and the `cancel()` trigger
//!   (upstream `AbortController`).
//! - [`await_with_context`] — observes a future until it settles or the
//!   context's signal cancels. Upstream cancellation rejects only the waiter,
//!   never the underlying promise; the port drives the future to completion
//!   in a spawned task for the same reason, so cancelling a waiter never
//!   cancels the work.
//!
//! Deferred to M3b Task 10 (telemetry module): `getTelemetryContext` /
//! `withTelemetryContext` and the `TELEMETRY_CONTEXT_KEY` — they need the
//! `pi-telemetry` typed-span subset that task ports.

use anyhow::anyhow;
use futures::future::BoxFuture;
use std::future::Future;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::context::abort_signal_key;
pub use crate::agent_core::chord_support::context::{create_context_key, Context, ContextKey};

/// Upstream `BACKGROUND_CONTEXT` re-export (`context.ts:15`,
/// `context/index.ts:55`): the chain-terminating empty context.
pub fn background_context() -> Context {
    Context::background()
}

/// Upstream `TODO_CONTEXT` re-export (`context.ts:20`,
/// `context/index.ts:56`): the empty context for call sites with no caller
/// context to pass.
pub fn todo_context() -> Context {
    Context::todo()
}

/// Upstream `withContextValue(key, value, parent)` re-export
/// (`context.ts:23`, `context/index.ts:63-65`).
pub fn with_context_value<T: Send + Sync + 'static>(
    key: &ContextKey<T>,
    value: T,
    parent: Context,
) -> Context {
    parent.with_value(key, value)
}

/// Upstream `withAbortSignal(signal, context)` (`context/index.ts:82-88`):
/// derive a context cancelled by either the parent signal or the supplied
/// signal; the parent context remains unchanged. See the module docs for the
/// linker-task substitution.
pub fn with_abort_signal(signal: CancellationToken, context: Context) -> Context {
    let combined = match context.abort_signal() {
        None => signal,
        Some(parent) => {
            if parent.is_cancelled() || signal.is_cancelled() {
                let already_cancelled = CancellationToken::new();
                already_cancelled.cancel();
                already_cancelled
            } else {
                let combined = CancellationToken::new();
                let linked = combined.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = parent.cancelled() => {}
                        _ = signal.cancelled() => {}
                    }
                    linked.cancel();
                });
                combined
            }
        }
    };
    context.with_value(abort_signal_key(), Some(combined))
}

/// Upstream `withoutAbortSignal(context)` (`context/index.ts:91-93`): derive a
/// context retaining all values except caller cancellation. Intended for
/// mandatory cleanup only.
pub fn without_abort_signal(context: Context) -> Context {
    context.with_value(abort_signal_key(), None)
}

/// The `{ context, cancel }` record upstream `withCancel` returns
/// (`context/index.ts:95-102`).
#[derive(Debug, Clone)]
pub struct CancelledContext {
    /// The derived, independently cancellable context.
    pub context: Context,
    token: CancellationToken,
}

impl CancelledContext {
    /// Upstream `cancel(reason?)` (`context/index.ts:99-101`). `CancellationToken`
    /// carries no reason; observe the context's signal for the notification.
    pub fn cancel(&self) {
        self.token.cancel();
    }
}

/// Upstream `withCancel(context)` (`context/index.ts:95-102`): derive an
/// independently cancellable child context.
pub fn with_cancel(context: Context) -> CancelledContext {
    let token = CancellationToken::new();
    CancelledContext {
        context: with_abort_signal(token.clone(), context),
        token,
    }
}

/// Upstream `awaitWithContext(promise, context)` (`context/index.ts:108-123`):
/// observe `future` until it settles or the invocation is cancelled.
/// Cancellation rejects only this waiter; it never cancels the underlying
/// future (the module docs explain the spawned-driver substitution). The
/// upstream abort `reason`/`DOMException` maps to the fixed message
/// `"the operation was aborted"`.
pub fn await_with_context<T, F>(
    future: F,
    context: Context,
) -> BoxFuture<'static, anyhow::Result<T>>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let signal = context.abort_signal();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let value = future.await;
        let _ = sender.send(value);
    });
    const ABORTED: &str = "the operation was aborted";
    match signal {
        None => Box::pin(async move {
            receiver
                .await
                .map_err(|_| anyhow!("awaitWithContext future panicked"))
        }),
        Some(signal) if signal.is_cancelled() => Box::pin(async move { Err(anyhow!(ABORTED)) }),
        Some(signal) => Box::pin(async move {
            tokio::select! {
                _ = signal.cancelled() => Err(anyhow!(ABORTED)),
                value = receiver => value.map_err(|_| anyhow!("awaitWithContext future panicked")),
            }
        }),
    }
}

#[cfg(test)]
mod tests;
