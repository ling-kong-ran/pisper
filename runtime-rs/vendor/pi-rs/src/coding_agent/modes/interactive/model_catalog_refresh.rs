//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream
//! `coding-agent/src/modes/interactive/model-catalog-refresh.ts`
//! (51 lines, sha256 `a7a10647deaba5036019737c376ab3f73eed12703dafeb8cb8920581ec8672e2`).
//!
//! Shares one concurrent interactive all-catalog refresh per runtime while
//! keeping each caller's cancellation independent: the first caller starts
//! the runtime refresh under a coordinator-owned controller; extra callers
//! join it; a caller that stops waiting never touches the shared controller;
//! the last waiter out aborts the shared refresh (upstream deletes the
//! `WeakMap` entry and aborts the controller in the same `finally`), and a
//! later refresh starts a fresh operation.
//!
//! # Seams
//!
//! - **`WeakMap<ModelCatalogRuntime, …>` identity**: JS keys the map by
//!   object identity and compares `map.get(runtime) === active` before
//!   touching a recorded entry; the port keys a registry vector by
//!   `Arc::as_ptr` plus a monotonically increasing **generation** (re-using
//!   the same pointer for a second refresh must never let a stale waiter or
//!   the settled task touch the new entry), with a `Weak` back-reference
//!   (entries whose runtime was dropped are pruned on access). Same
//!   observable semantics: one shared refresh per live runtime, dropped
//!   runtimes release their entry.
//! - **Shared promise → broadcast channel**: the upstream coordinator stores
//!   the shared `Promise`; the port spawns the refresh into a tokio task and
//!   fans the single settlement out through a `tokio::sync::broadcast`
//!   channel (each waiter subscribes before the first `recv`). The task's
//!   settle hook is the port of the shared promise's `finally` (delete the
//!   entry so a later caller starts a fresh refresh).
//! - **`signal.throwIfAborted()`**: maps to an immediate
//!   [`AbortError`](crate::coding_agent::utils::abort::AbortError) when the
//!   caller's token is already cancelled.
//! - **`AbortSignal`** is `tokio_util::sync::CancellationToken`, and the
//!   `raceWithAbortSignal` detached-promise divergence carries over from
//!   `utils/abort.rs` (Rust futures cancel at the select point).

use std::sync::{Arc, Mutex, Weak};

use futures::future::BoxFuture;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::ai::models::{ModelsRefreshOptions, ModelsRefreshResult};
use crate::coding_agent::utils::abort::{race_with_abort_signal, AbortError};

/// Upstream `Pick<ModelRuntime, "refresh">`: the refresh capability the
/// coordinator needs. Implemented by
/// [`crate::coding_agent::core::model_runtime::ModelRuntime`]; tests install
/// mock runtimes.
pub trait ModelCatalogRuntime: Send + Sync + 'static {
    /// Upstream `modelRuntime.refresh({ signal })`.
    fn refresh<'a>(
        &'a self,
        options: ModelsRefreshOptions,
    ) -> BoxFuture<'a, Result<ModelsRefreshResult, String>>;
}

/// The shared-refresh settlement fanned out to every waiter (upstream: the
/// single `ModelsRefreshResult` the shared promise resolves with; the port's
/// `ModelRuntime::refresh` also carries a transport error string).
type SharedOutcome = Result<ModelsRefreshResult, String>;

/// One in-flight shared refresh (upstream `ActiveModelCatalogRefresh`).
struct ActiveRefresh {
    /// Generation tag; stands in for the upstream `map.get(runtime) ===
    /// active` object-identity comparison.
    generation: u64,
    /// `Weak` mirror of the keyed runtime (upstream `WeakMap` key liveness).
    runtime: Weak<dyn ModelCatalogRuntime>,
    /// Upstream `controller: AbortController`.
    controller: CancellationToken,
    /// Upstream `promise`, fanned out to waiters.
    sender: broadcast::Sender<SharedOutcome>,
    /// Upstream `waiters: number`.
    waiters: u32,
}

/// Upstream `ModelCatalogRefreshCoordinator`. Cheap to clone (shared
/// registry, like the module-level upstream singleton).
#[derive(Clone, Default)]
pub struct ModelCatalogRefreshCoordinator {
    inner: Arc<CoordinatorInner>,
}

struct CoordinatorInner {
    active_by_runtime: Arc<Mutex<CoordinatorState>>,
}

#[derive(Default)]
struct CoordinatorState {
    active: Vec<ActiveRefresh>,
    next_generation: u64,
}

/// Error surfaced by [`ModelCatalogRefreshCoordinator::refresh`]: either the
/// caller's own signal aborted the wait (upstream `AbortError`) or the
/// shared operation failed (the port's `ModelRuntime::refresh` transport
/// error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCatalogRefreshError {
    /// Upstream `AbortError` (name `"AbortError"`).
    Abort(AbortError),
    /// The shared refresh operation failed.
    Runtime(String),
    /// The shared settlement was lost (the task died before publishing).
    Channel(String),
}

impl From<AbortError> for ModelCatalogRefreshError {
    fn from(error: AbortError) -> Self {
        Self::Abort(error)
    }
}

impl std::fmt::Display for ModelCatalogRefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Abort(error) => write!(f, "{error}"),
            Self::Runtime(message) => write!(f, "{message}"),
            Self::Channel(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ModelCatalogRefreshError {}

impl Default for CoordinatorInner {
    fn default() -> Self {
        Self {
            active_by_runtime: Arc::new(Mutex::new(CoordinatorState::default())),
        }
    }
}

impl ModelCatalogRefreshCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    /// The pointer identity the registry is keyed by (upstream `WeakMap` key).
    fn runtime_key(runtime: &Arc<dyn ModelCatalogRuntime>) -> usize {
        Arc::as_ptr(runtime) as *const () as usize
    }

    /// Upstream `ModelCatalogRefreshCoordinator.refresh`.
    pub async fn refresh(
        &self,
        runtime: Arc<dyn ModelCatalogRuntime>,
        signal: CancellationToken,
    ) -> Result<ModelsRefreshResult, ModelCatalogRefreshError> {
        // upstream `signal.throwIfAborted()`.
        if signal.is_cancelled() {
            return Err(AbortError.into());
        }

        let runtime_key = Self::runtime_key(&runtime);
        let (joined_generation, mut receiver) = {
            let mut state = self
                .inner
                .active_by_runtime
                .lock()
                .expect("coordinator lock");
            // Prune entries whose runtime was dropped (upstream WeakMap GC).
            state
                .active
                .retain(|entry| entry.runtime.upgrade().is_some());
            let existing = state
                .active
                .iter_mut()
                .find(|entry| entry.runtime_key_matches(runtime_key));
            if let Some(active) = existing {
                active.waiters += 1;
                (active.generation, active.sender.subscribe())
            } else {
                let generation = state.next_generation;
                state.next_generation += 1;
                let controller = CancellationToken::new();
                let (sender, receiver) = broadcast::channel(1);
                let task_runtime = Arc::clone(&runtime);
                let task_controller = controller.clone();
                let task_sender = sender.clone();
                let registry = Arc::downgrade(&self.inner.active_by_runtime);
                tokio::spawn(async move {
                    let options = ModelsRefreshOptions {
                        allow_network: None,
                        providers: None,
                        force: None,
                        signal: Some(task_controller),
                    };
                    let outcome = task_runtime.refresh(options).await;
                    let _ = task_sender.send(outcome);
                    // Upstream: the shared promise's `finally` deletes the
                    // WeakMap entry once the operation settles (only if it is
                    // still the recorded refresh), so a later caller always
                    // starts a fresh refresh.
                    if let Some(registry) = registry.upgrade() {
                        if let Ok(mut state) = registry.lock() {
                            state.active.retain(|entry| entry.generation != generation);
                        }
                    }
                });
                state.active.push(ActiveRefresh {
                    generation,
                    runtime: Arc::downgrade(&runtime),
                    controller,
                    sender,
                    waiters: 1,
                });
                (generation, receiver)
            }
        };

        // upstream `raceWithAbortSignal(active.promise, signal)`.
        let raced = race_with_abort_signal(receiver.recv(), Some(&signal)).await;
        // upstream `finally`: drop this waiter; when the last one leaves and
        // the entry is still the recorded refresh, abort the shared
        // controller.
        {
            let mut state = self
                .inner
                .active_by_runtime
                .lock()
                .expect("coordinator lock");
            if let Some(position) = state
                .active
                .iter()
                .position(|entry| entry.generation == joined_generation)
            {
                state.active[position].waiters -= 1;
                if state.active[position].waiters == 0 {
                    let active = state.active.remove(position);
                    active.controller.cancel();
                }
            }
        }

        match raced {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(error))) => Err(ModelCatalogRefreshError::Runtime(error)),
            Ok(Err(recv_error)) => Err(ModelCatalogRefreshError::Channel(recv_error.to_string())),
            Err(abort_error) => Err(ModelCatalogRefreshError::Abort(abort_error)),
        }
    }
}

impl ActiveRefresh {
    fn runtime_key_matches(&self, runtime_key: usize) -> bool {
        self.runtime
            .upgrade()
            .is_some_and(|strong| Arc::as_ptr(&strong) as *const () as usize == runtime_key)
    }
}

/// Upstream module-level `modelCatalogRefreshCoordinator` singleton.
static MODEL_CATALOG_REFRESH_COORDINATOR: std::sync::LazyLock<ModelCatalogRefreshCoordinator> =
    std::sync::LazyLock::new(ModelCatalogRefreshCoordinator::new);

/// Upstream `refreshModelCatalogs`: share concurrent interactive all-catalog
/// refreshes while keeping each caller's cancellation independent.
pub async fn refresh_model_catalogs(
    runtime: Arc<dyn ModelCatalogRuntime>,
    signal: CancellationToken,
) -> Result<ModelsRefreshResult, ModelCatalogRefreshError> {
    MODEL_CATALOG_REFRESH_COORDINATOR
        .refresh(runtime, signal)
        .await
}
