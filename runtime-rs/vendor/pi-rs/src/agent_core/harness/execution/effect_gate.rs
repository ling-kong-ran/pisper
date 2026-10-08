//! Port of `packages/agent/src/harness/execution/effect-gate.ts` (64 lines):
//! the synchronous effect-admission gate shared by one drive pass.
//!
//! Disclosed substitutions:
//! - **`Gate` naming.** Upstream names the procedure-facing interface `Gate`
//!   (`effect-gate.ts:13-16`); the M3b Task 4 hooks port already took that
//!   name for the trait (`hooks.rs`, which this module implements). The
//!   concrete gate here is [`EffectGate`], plus [`GateControl`] for the
//!   owner-facing view and [`create_gate`] for the `createGate` factory
//!   (`effect-gate.ts:31-64`).
//! - **`AbortSignal`/`AbortController`.** Both map onto
//!   [`tokio_util::sync::CancellationToken`] (the repo-wide convention): the
//!   gate's `signal` is a token, and `controller.abort(reason)` is
//!   `token.cancel()` — cancellation tokens carry no reason, so upstream's
//!   abort reasons (the `AbortRequested` instance / close error) are
//!   observable only through [`EffectGate::admit`], which is the same channel
//!   upstream procedures use (`check` throws before the effect runs).
//! - **`AbortRequested.cancellation: Promise<void>`**
//!   (`effect-gate.ts:3-9`): the handle the owner resolves once the
//!   cancellation procedure settles maps onto a [`CancellationToken`]; await
//!   it with `.cancelled().await`.
//! - **Typed refusals.** Upstream `check` throws `AbortRequested` or the
//!   stored close error; the port returns [`GateRejection`], which preserves
//!   both the `AbortRequested` payload (its cancellation token) and the close
//!   error's identity (`Arc` comparison). [`EffectGate`] also implements the
//!   hooks [`crate::agent_core::harness::hooks::Gate`] trait, converting the
//!   rejection into an `anyhow::Error` that downcasts back to
//!   [`GateRejection`].

use std::error::Error as StdError;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio_util::sync::CancellationToken;

use crate::agent_core::harness::hooks;

/// Upstream `AbortRequested` (`effect-gate.ts:2-10`): expected internal
/// control flow when cancellation wins effect admission.
#[derive(Debug)]
pub struct AbortRequested {
    /// Upstream `cancellation: Promise<void>`; observe with
    /// `.cancelled().await`.
    pub cancellation: CancellationToken,
}

impl fmt::Display for AbortRequested {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Upstream `super("Abort requested")`.
        write!(f, "Abort requested")
    }
}

impl StdError for AbortRequested {}

/// The two refusal kinds of [`EffectGate::admit`] (upstream `check`,
/// `effect-gate.ts:35-38`: throws `AbortRequested` for an aborting gate and
/// the stored error for a closed one).
#[derive(Debug)]
pub enum GateRejection {
    /// Upstream `new AbortRequested(state.cancellation)`.
    AbortRequested(AbortRequested),
    /// Upstream `throw state.error` — the exact error passed to
    /// [`GateControl::close`], identity preserved.
    Closed(Arc<dyn StdError + Send + Sync>),
}

impl fmt::Display for GateRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateRejection::AbortRequested(abort) => write!(f, "{abort}"),
            GateRejection::Closed(error) => write!(f, "{error}"),
        }
    }
}

impl StdError for GateRejection {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            GateRejection::AbortRequested(_) => None,
            GateRejection::Closed(error) => Some(error.as_ref()),
        }
    }
}

/// Upstream `GateState` (`effect-gate.ts:25-28`).
#[derive(Debug)]
enum GateState {
    Open,
    Aborting {
        cancellation: CancellationToken,
    },
    Closed {
        error: Arc<dyn StdError + Send + Sync>,
    },
}

struct GateInner {
    state: Mutex<GateState>,
    signal: CancellationToken,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Upstream `Gate` (`effect-gate.ts:13-16`): the procedure-facing
/// synchronous admission capability for one drive pass. Named [`EffectGate`]
/// because the hooks port owns the `Gate` trait name (see module docs).
#[derive(Clone)]
pub struct EffectGate {
    inner: Arc<GateInner>,
}

impl EffectGate {
    /// Upstream `gate.admit<T>(invoke)` (`effect-gate.ts:42-45`): check the
    /// gate synchronously, then run `invoke`. The refusal shapes are
    /// [`GateRejection`].
    pub fn admit<T>(&self, invoke: impl FnOnce() -> T) -> Result<T, GateRejection> {
        match &*lock(&self.inner.state) {
            GateState::Aborting { cancellation } => {
                return Err(GateRejection::AbortRequested(AbortRequested {
                    cancellation: cancellation.clone(),
                }));
            }
            GateState::Closed { error } => {
                return Err(GateRejection::Closed(Arc::clone(error)));
            }
            GateState::Open => {}
        }
        Ok(invoke())
    }

    /// Upstream `gate.signal` (`effect-gate.ts:14`).
    pub fn signal(&self) -> CancellationToken {
        self.inner.signal.clone()
    }
}

/// Upstream `GateControl` (`effect-gate.ts:18-23`): owner-facing lifecycle
/// controls for one drive pass.
#[derive(Clone)]
pub struct GateControl {
    inner: Arc<GateInner>,
}

impl GateControl {
    /// Upstream `beginAbort(cancellation)` (`effect-gate.ts:49-52`): refuse
    /// future admissions; only transitions from the open state.
    pub fn begin_abort(&self, cancellation: CancellationToken) {
        let mut state = lock(&self.inner.state);
        if !matches!(*state, GateState::Open) {
            return;
        }
        *state = GateState::Aborting { cancellation };
    }

    /// Upstream `signalAbort()` (`effect-gate.ts:53-56`): cancel the signal
    /// only once cancellation has committed (the aborting state) and only
    /// once.
    pub fn signal_abort(&self) {
        let state = lock(&self.inner.state);
        if !matches!(*state, GateState::Aborting { .. }) {
            return;
        }
        if self.inner.signal.is_cancelled() {
            return;
        }
        // `controller.abort(new AbortRequested(state.cancellation))`; the
        // reason is carried by the admit rejection, not the token.
        self.inner.signal.cancel();
    }

    /// Upstream `close(error)` (`effect-gate.ts:57-61`): permanently close
    /// with the error and cancel the signal if not already cancelled.
    pub fn close(&self, error: Arc<dyn StdError + Send + Sync>) {
        {
            let mut state = lock(&self.inner.state);
            if matches!(*state, GateState::Closed { .. }) {
                return;
            }
            *state = GateState::Closed { error };
        }
        if !self.inner.signal.is_cancelled() {
            self.inner.signal.cancel();
        }
    }
}

/// Upstream `createGate()` (`effect-gate.ts:31-64`): create separate
/// procedure-facing and owner-facing views of one effect gate. Returns
/// `(gate, control)` — upstream `{ gate, control }`.
pub fn create_gate() -> (EffectGate, GateControl) {
    let inner = Arc::new(GateInner {
        state: Mutex::new(GateState::Open),
        signal: CancellationToken::new(),
    });
    (
        EffectGate {
            inner: Arc::clone(&inner),
        },
        GateControl { inner },
    )
}

impl hooks::Gate for EffectGate {
    fn signal(&self) -> CancellationToken {
        EffectGate::signal(self)
    }

    fn admit(&self) -> anyhow::Result<()> {
        // `anyhow::Error::new` keeps the concrete `GateRejection` type as the
        // top-level error, so `downcast_ref::<GateRejection>` reaches it (and,
        // for the closed case, the stored error identity) from the
        // hooks-trait surface.
        EffectGate::admit(self, || ()).map_err(anyhow::Error::new)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
