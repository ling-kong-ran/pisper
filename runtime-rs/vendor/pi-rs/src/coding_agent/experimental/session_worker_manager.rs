//! Port of upstream `experimental/session-worker-manager.ts`
//! (sha256 880d00516909d6bbb64a7108a2bc2e04c230c98f2b532be404ad6e3bb862a140).
//!
//! Ported: the full `SessionWorkerManager` bookkeeping state machine —
//! discovery, launch accounting, demand application with timeout
//! compensation, operation correlation (identity/scope checks with the exact
//! upstream error strings), service subscription routing, worker stop with a
//! bounded shutdown window, replacement `detach` and full `shutdown`.
//!
//! D5 seams (see mod.rs docs):
//! - `CoordinatorLink` replaces the socket-backed
//!   `Pick<CoordinatorConnection, ...>`; events are delivered explicitly via
//!   [`SessionWorkerManager::handle_coordinator_event`].
//! - `WorkerSpawn` replaces `spawnInternalProcess`'s real child (tests inject
//!   a fake; the real-child adapter reuses `process::ProcessSpawner`).
//! - `@earendil-works/pi-server` `RoutedSessionHandle`/`ServerError` become
//!   the local [`RoutedSessionHandle`]/[`WorkerOperationFailure`].
//! - chord's `decodeServiceControlCall`/`createServiceUnsubscribeCall` are an
//!   injectable [`ControlCallCodec`] (default: no call is a control call).
//! - `randomUUID` becomes `crate::ai::uuid::uuid_v7` (opaque identifier
//!   format only; nothing behavioral depends on the version).
//! - `#childExited` reason formatting collapses `signal ?? code ?? "unknown"`
//!   to `unknown` (the fake-child seam carries no exit reason).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::ai::uuid::uuid_v7;
use crate::coding_agent::experimental::coordinator::CoordinatorConnectionEvent;
use crate::coding_agent::experimental::process::{
    InternalProcessChild, InternalProcessRole, ProcessSpawner,
};
use crate::coding_agent::experimental::session_worker::{
    same_scope, ServiceCall, SessionWorkerEvent, SessionWorkerMetadata, SessionWorkerOptions,
    WorkerOperationResponse, WorkerOperationScope, SESSION_WORKER_CONTROL_ADDRESS_ENV,
    SESSION_WORKER_CONTROL_TOKEN_ENV, SESSION_WORKER_PEER_ID_ENV, SESSION_WORKER_SESSION_KEY_ENV,
};

const WORKER_STARTUP_TIMEOUT_MS: u64 = 15_000;
const WORKER_SHUTDOWN_TIMEOUT_MS: u64 = 10_000;
const WORKER_DISCOVERY_TIMEOUT_MS: u64 = 5_000;
const WORKER_DEMAND_TIMEOUT_MS: u64 = 5_000;

/// D5: chord control-call decoding seam (upstream `decodeServiceControlCall`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceControlCall {
    Subscribe { subscription_id: String },
    Unsubscribe { subscription_id: String },
}

/// D5: chord control-call codec seam. The default recognizes no control
/// calls, mirroring a deployment without replicated-state services.
#[derive(Clone)]
pub struct ControlCallCodec {
    pub decode: ControlCallDecoder,
    pub encode_unsubscribe: ControlCallEncoder,
}

pub type ControlCallDecoder = Arc<dyn Fn(&ServiceCall) -> Option<ServiceControlCall> + Send + Sync>;
pub type ControlCallEncoder = Arc<dyn Fn(&str) -> ServiceCall + Send + Sync>;

impl ControlCallCodec {
    /// D5: the real chord-backed codec. Upstream imports
    /// `decodeServiceControlCall`/`createServiceUnsubscribeCall` from
    /// `@earendil-works/chord`; the port delegates to the chord wire module
    /// (`crate::chord::services::wire`), converting between the
    /// experimental and chord `ServiceCall` faces. `catalogue` control calls
    /// decode to `None` because the manager only reacts to subscribe /
    /// unsubscribe (upstream matches on `control?.type` the same way).
    pub fn chord() -> Self {
        ControlCallCodec {
            decode: Arc::new(|call| {
                let chord_call =
                    crate::coding_agent::experimental::session_worker::service_call_to_chord(call)?;
                Some(
                    match crate::chord::services::wire::decode_service_control_call(&chord_call)? {
                        crate::chord::services::wire::ServiceControlCall::Subscribe {
                            subscription_id,
                            ..
                        } => ServiceControlCall::Subscribe { subscription_id },
                        crate::chord::services::wire::ServiceControlCall::Unsubscribe {
                            subscription_id,
                        } => ServiceControlCall::Unsubscribe { subscription_id },
                        crate::chord::services::wire::ServiceControlCall::Catalogue => {
                            return None;
                        }
                    },
                )
            }),
            encode_unsubscribe: Arc::new(|subscription_id| {
                crate::coding_agent::experimental::session_worker::service_call_from_chord(
                    &crate::chord::services::wire::create_service_unsubscribe_call(subscription_id),
                )
            }),
        }
    }
}

impl Default for ControlCallCodec {
    fn default() -> Self {
        ControlCallCodec {
            decode: Arc::new(|_call| None),
            encode_unsubscribe: Arc::new(|_subscription_id| ServiceCall {
                service_id: String::new(),
                instance: None,
                member: String::new(),
                args: Vec::new(),
            }),
        }
    }
}

/// D5: upstream `CoordinatorConnection` narrow surface.
pub trait CoordinatorLink: Send + Sync {
    fn control_path(&self) -> &str;
    fn server_connection_id(&self) -> &str;
    fn was_replaced(&self) -> bool {
        false
    }
    /// Upstream `send(peerId, payload)`; the returned future completes when
    /// the transport accepted the frame (it may synchronously deliver
    /// coordinator events back into the manager).
    fn send<'a>(&'a self, peer_id: &'a str, payload: Value) -> BoxFuture<'a, Result<(), String>>;
    fn broadcast<'a>(&'a self, payload: Value) -> BoxFuture<'a, Result<(), String>>;
}

/// D5: spawn side of the worker launch.
pub trait WorkerSpawn: Send + Sync {
    fn spawn(
        &self,
        args: &[String],
        extra_env: &[(String, String)],
    ) -> Result<Box<dyn InternalProcessChild>, String>;
}

/// Adapter over the real [`ProcessSpawner`] seam.
pub struct SpawnViaProcessSpawner(pub Arc<dyn ProcessSpawner>);

impl WorkerSpawn for SpawnViaProcessSpawner {
    fn spawn(
        &self,
        args: &[String],
        extra_env: &[(String, String)],
    ) -> Result<Box<dyn InternalProcessChild>, String> {
        self.0
            .spawn(InternalProcessRole::SessionWorker, args, extra_env)
            .map_err(|error| error.to_string())
    }
}

/// Upstream `SessionPluginSelectionConflictError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPluginSelectionConflictError(pub String);

impl std::fmt::Display for SessionPluginSelectionConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SessionPluginSelectionConflictError {}

/// D5: local stand-in for pi-server `ServerError` (operation failures that
/// carry a remote service error code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerOperationFailure {
    pub code: Option<String>,
    pub message: String,
}

impl std::fmt::Display for WorkerOperationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.code {
            Some(code) => write!(f, "Session worker service error {code}: {}", self.message),
            None => write!(f, "Session worker operation failed: {}", self.message),
        }
    }
}

impl std::error::Error for WorkerOperationFailure {}

fn plain_error(message: impl Into<String>) -> WorkerOperationFailure {
    WorkerOperationFailure {
        code: None,
        message: message.into(),
    }
}

struct LaunchSlot {
    result: Mutex<Option<Result<Arc<WorkerRecord>, String>>>,
    notify: tokio::sync::Notify,
}

impl LaunchSlot {
    fn new() -> Arc<Self> {
        Arc::new(LaunchSlot {
            result: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        })
    }

    fn settle(&self, result: Result<Arc<WorkerRecord>, String>) {
        {
            let mut guard = self.result.lock().unwrap();
            if guard.is_some() {
                return;
            }
            *guard = Some(result);
        }
        self.notify.notify_waiters();
    }

    async fn wait(self: &Arc<Self>) -> Result<Arc<WorkerRecord>, String> {
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self.result.lock().unwrap().clone() {
                return result;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
struct TerminatedSlot {
    value: Arc<Mutex<Option<Option<WorkerOperationFailure>>>>,
    notify: Arc<tokio::sync::Notify>,
}

impl TerminatedSlot {
    fn new() -> Self {
        TerminatedSlot {
            value: Arc::new(Mutex::new(None)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn resolve(&self, error: Option<WorkerOperationFailure>) {
        {
            let mut guard = self.value.lock().unwrap();
            if guard.is_some() {
                return;
            }
            *guard = Some(error);
        }
        self.notify.notify_waiters();
    }

    fn error(&self) -> Option<WorkerOperationFailure> {
        self.value.lock().unwrap().clone().flatten()
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.value.lock().unwrap().is_some() {
                return;
            }
            notified.await;
        }
    }
}

struct WorkerRecord {
    peer_id: String,
    metadata: SessionWorkerMetadata,
    pid: u32,
    token: String,
    plugin_manifest_paths: Vec<String>,
    terminated: TerminatedSlot,
    attachment_ids: Mutex<HashSet<String>>,
    expected_stop: AtomicBool,
    stopping: AtomicBool,
    /// The upstream stopPromise caches failures as well as successful stops.
    stop_result: tokio::sync::OnceCell<Result<(), String>>,
}

struct PendingLaunch {
    session_key: String,
    peer_id: String,
    token: String,
    plugin_manifest_paths: Vec<String>,
    child: Arc<dyn InternalProcessChild>,
    /// Non-zero while the startup timer is armed (cancelled by setting 0).
    timer_generation: u64,
    promise: Arc<LaunchSlot>,
}

struct PendingDemand {
    attachment_id: String,
    attached: bool,
    worker: Arc<WorkerRecord>,
    settle: oneshot::Sender<Result<(), String>>,
}

struct PendingWorkerOperation {
    worker: Arc<WorkerRecord>,
    scope: WorkerOperationScope,
    settle: oneshot::Sender<Result<Option<Value>, WorkerOperationFailure>>,
}

struct WorkerServiceSubscription {
    worker: Arc<WorkerRecord>,
    scope: WorkerOperationScope,
    listener: Arc<dyn Fn(Value) -> BoxFuture<'static, ()> + Send + Sync>,
}

struct ManagerState {
    workers_by_session: HashMap<String, Arc<WorkerRecord>>,
    workers_by_peer: HashMap<String, Arc<WorkerRecord>>,
    worker_pids: HashMap<String, u32>,
    pending: HashMap<String, Arc<Mutex<PendingLaunch>>>,
    pending_demand: HashMap<String, PendingDemand>,
    pending_operations: HashMap<String, PendingWorkerOperation>,
    service_subscriptions: HashMap<String, WorkerServiceSubscription>,
    discovery_peers: Option<HashSet<String>>,
    discovery_done: Option<oneshot::Sender<()>>,
    timer_serial: u64,
    detached: bool,
    shutting_down: bool,
}

/// Upstream's optional `{ provider?, model }` launch selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkerModelSelection {
    pub provider: Option<String>,
    pub model: String,
}

/// D5: process-kill seam (upstream `process.kill(pid, "SIGKILL")`; the
/// upstream tests spy on `process.kill`). `true` means the signal was sent
/// or the process was already gone (ESRCH); `false` is a termination failure.
pub type KillFn = Arc<dyn Fn(u32) -> bool + Send + Sync>;

/// Upstream `SessionWorkerManager` constructor arguments plus the D5 seams.
pub struct SessionWorkerManagerConfig {
    pub link: Arc<dyn CoordinatorLink>,
    pub session_dir: String,
    pub model: Option<WorkerModelSelection>,
    pub on_worker_count_changed: Option<Box<dyn Fn(usize) + Send + Sync>>,
    pub spawner: Arc<dyn WorkerSpawn>,
    pub codec: ControlCallCodec,
    pub kill: Option<KillFn>,
}

/// Upstream `SessionWorkerManager`: session and process bookkeeping owned by
/// one replaceable server process.
pub struct SessionWorkerManager {
    shared: Arc<ManagerShared>,
}

/// Callback type for worker-count changes (upstream's optional
/// `onWorkerCountChanged` constructor argument).
type WorkerCountCallback = Mutex<Box<dyn Fn(usize) + Send + Sync>>;

struct ManagerShared {
    link: Arc<dyn CoordinatorLink>,
    session_dir: String,
    model: Option<WorkerModelSelection>,
    on_worker_count_changed: Option<WorkerCountCallback>,
    spawner: Arc<dyn WorkerSpawn>,
    codec: ControlCallCodec,
    kill: KillFn,
    state: Mutex<ManagerState>,
}

impl SessionWorkerManager {
    pub fn new(config: SessionWorkerManagerConfig) -> Arc<Self> {
        let shared = Arc::new(ManagerShared {
            link: config.link,
            session_dir: config.session_dir,
            model: config.model,
            on_worker_count_changed: config.on_worker_count_changed.map(Mutex::new),
            spawner: config.spawner,
            codec: config.codec,
            kill: config.kill.unwrap_or_else(|| Arc::new(kill_pid_sigkill)),
            state: Mutex::new(ManagerState {
                workers_by_session: HashMap::new(),
                workers_by_peer: HashMap::new(),
                worker_pids: HashMap::new(),
                pending: HashMap::new(),
                pending_demand: HashMap::new(),
                pending_operations: HashMap::new(),
                service_subscriptions: HashMap::new(),
                discovery_peers: None,
                discovery_done: None,
                timer_serial: 1,
                detached: false,
                shutting_down: false,
            }),
        });
        Arc::new(SessionWorkerManager { shared })
    }

    /// Upstream `workerPids` snapshot.
    pub fn worker_pids(&self) -> HashMap<String, u32> {
        self.shared.state.lock().unwrap().worker_pids.clone()
    }

    /// Upstream `trackedSessions`.
    pub fn tracked_sessions(&self) -> Vec<SessionWorkerMetadata> {
        self.shared
            .state
            .lock()
            .unwrap()
            .workers_by_session
            .values()
            .map(|worker| worker.metadata.clone())
            .collect()
    }

    /// Upstream `assertSessionPluginManifestPaths`.
    pub fn assert_session_plugin_manifest_paths(
        &self,
        metadata: &SessionWorkerMetadata,
        manifest_paths: &[String],
    ) -> Result<(), SessionPluginSelectionConflictError> {
        let state = self.shared.state.lock().unwrap();
        if let Some(existing) = state.workers_by_session.get(&metadata.path) {
            if !same_strings(&existing.plugin_manifest_paths, manifest_paths) {
                return Err(SessionPluginSelectionConflictError(format!(
                    "Session {} is active with a different plugin selection",
                    metadata.id
                )));
            }
        }
        if let Some(pending) = state.pending.get(&metadata.path) {
            let guard = pending.lock().unwrap();
            if !same_strings(&guard.plugin_manifest_paths, manifest_paths) {
                return Err(SessionPluginSelectionConflictError(format!(
                    "Session {} is starting with a different plugin selection",
                    metadata.id
                )));
            }
        }
        Ok(())
    }

    /// Upstream `discover`.
    pub async fn discover(&self, peer_ids: &HashSet<String>) {
        let rx = {
            let mut state = self.shared.state.lock().unwrap();
            if state.detached {
                return;
            }
            let undiscovered: HashSet<String> = peer_ids
                .iter()
                .filter(|peer_id| {
                    !state.workers_by_peer.contains_key(*peer_id)
                        && !state
                            .pending
                            .values()
                            .any(|pending| pending.lock().unwrap().peer_id == **peer_id)
                })
                .cloned()
                .collect();
            if undiscovered.is_empty() {
                return;
            }
            let (tx, rx) = oneshot::channel();
            state.discovery_peers = Some(undiscovered);
            state.discovery_done = Some(tx);
            rx
        };
        let _ = self
            .shared
            .link
            .broadcast(json!({ "type": "discover_workers" }))
            .await;
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(WORKER_DISCOVERY_TIMEOUT_MS),
            rx,
        )
        .await;
        let mut state = self.shared.state.lock().unwrap();
        state.discovery_peers = None;
        state.discovery_done = None;
    }

    /// Upstream `openSession`.
    pub async fn open_session(
        self: &Arc<Self>,
        metadata: &SessionWorkerMetadata,
        plugin_manifest_paths: &[String],
    ) -> Result<RoutedSessionHandle, String> {
        {
            let state = self.shared.state.lock().unwrap();
            if state.detached || state.shutting_down {
                return Err("Experimental server is shutting down".to_string());
            }
        }
        self.assert_session_plugin_manifest_paths(metadata, plugin_manifest_paths)
            .map_err(|error| error.0)?;
        let existing = self
            .shared
            .state
            .lock()
            .unwrap()
            .workers_by_session
            .get(&metadata.path)
            .cloned();
        if let Some(worker) = existing {
            return Ok(self.routed_handle(worker));
        }
        let pending = self
            .shared
            .state
            .lock()
            .unwrap()
            .pending
            .get(&metadata.path)
            .map(|pending| Arc::clone(pending.lock().unwrap().promise_slot()));
        if let Some(promise) = pending {
            let worker = promise.wait().await?;
            return Ok(self.routed_handle(worker));
        }
        let worker = self.launch(metadata, plugin_manifest_paths).await?;
        Ok(self.routed_handle(worker))
    }

    /// Upstream `closeSession`.
    pub async fn close_session(
        self: &Arc<Self>,
        metadata: &SessionWorkerMetadata,
    ) -> Result<(), String> {
        let worker = {
            let state = self.shared.state.lock().unwrap();
            match state.workers_by_session.get(&metadata.path) {
                Some(worker) => Some(ManagedWorker::Worker(Arc::clone(worker))),
                None => state.pending.get(&metadata.path).map(|pending| {
                    ManagedWorker::Pending(Arc::clone(pending.lock().unwrap().promise_slot()))
                }),
            }
        };
        let worker = match worker {
            Some(ManagedWorker::Worker(worker)) => Some(worker),
            Some(ManagedWorker::Pending(promise)) => promise.wait().await.ok(),
            None => None,
        };
        if let Some(worker) = worker {
            self.stop_worker(worker).await?;
        }
        Ok(())
    }

    fn routed_handle(self: &Arc<Self>, worker: Arc<WorkerRecord>) -> RoutedSessionHandle {
        RoutedSessionHandle {
            manager: Arc::clone(self),
            worker,
        }
    }

    /// Upstream `#attachClient`.
    async fn attach_client(
        self: &Arc<Self>,
        worker: Arc<WorkerRecord>,
    ) -> Result<RoutedSessionAttachment, String> {
        {
            let state = self.shared.state.lock().unwrap();
            if state.detached || state.shutting_down || worker.stopping.load(Ordering::SeqCst) {
                return Err("Experimental Session worker is stopping".to_string());
            }
        }
        if !self.is_registered_worker(&worker) {
            return Err("Experimental Session worker is no longer available".to_string());
        }
        let attachment_id = uuid_v7();
        worker
            .attachment_ids
            .lock()
            .unwrap()
            .insert(attachment_id.clone());
        if let Err(error) = self.apply_demand(&worker, &attachment_id, true, true).await {
            worker.attachment_ids.lock().unwrap().remove(&attachment_id);
            return Err(error);
        }
        let scope = self.operation_scope(&worker, &attachment_id)?;
        Ok(RoutedSessionAttachment {
            manager: Arc::clone(self),
            worker: Arc::clone(&worker),
            scope,
            released: Arc::new(AtomicBool::new(false)),
            attachment_id,
        })
    }

    fn is_registered_worker(&self, worker: &Arc<WorkerRecord>) -> bool {
        self.shared
            .state
            .lock()
            .unwrap()
            .workers_by_peer
            .get(&worker.peer_id)
            .map(|current| Arc::ptr_eq(current, worker))
            .unwrap_or(false)
    }

    /// Upstream `#invokeService`.
    async fn invoke_service(
        self: &Arc<Self>,
        worker: Arc<WorkerRecord>,
        scope: &WorkerOperationScope,
        call: ServiceCall,
        publish: Arc<dyn Fn(String, Value) -> BoxFuture<'static, ()> + Send + Sync>,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<Option<Value>, WorkerOperationFailure> {
        let control = (self.shared.codec.decode)(&call);
        let mut added_subscription_key: Option<String> = None;
        if let Some(ServiceControlCall::Subscribe { subscription_id }) = &control {
            let key = scoped_service_subscription_key(scope, subscription_id);
            let mut state = self.shared.state.lock().unwrap();
            if state.service_subscriptions.contains_key(&key) {
                return Err(plain_error("Service subscription ID is already active"));
            }
            let subscription_id = subscription_id.clone();
            state.service_subscriptions.insert(
                key.clone(),
                WorkerServiceSubscription {
                    worker: Arc::clone(&worker),
                    scope: scope.clone(),
                    listener: Arc::new(move |update| {
                        let publish = Arc::clone(&publish);
                        let subscription_id = subscription_id.clone();
                        Box::pin(async move { publish(subscription_id, update).await })
                    }),
                },
            );
            added_subscription_key = Some(key);
        }
        match self
            .invoke(&worker, scope, call.clone(), cancel.clone())
            .await
        {
            Err(error) => {
                if let (Some(key), Some(ServiceControlCall::Subscribe { subscription_id })) =
                    (&added_subscription_key, &control)
                {
                    self.shared
                        .state
                        .lock()
                        .unwrap()
                        .service_subscriptions
                        .remove(key);
                    let unsubscribe = (self.shared.codec.encode_unsubscribe)(subscription_id);
                    let _ = self.invoke(&worker, scope, unsubscribe, None).await;
                }
                Err(error)
            }
            Ok(response) => {
                if let Some(ServiceControlCall::Unsubscribe { subscription_id }) = &control {
                    self.shared
                        .state
                        .lock()
                        .unwrap()
                        .service_subscriptions
                        .remove(&scoped_service_subscription_key(scope, subscription_id));
                }
                Ok(response)
            }
        }
    }

    /// Upstream `#applyDemand`.
    async fn apply_demand(
        self: &Arc<Self>,
        worker: &Arc<WorkerRecord>,
        attachment_id: &str,
        attached: bool,
        compensate_on_timeout: bool,
    ) -> Result<(), String> {
        {
            if worker.stopping.load(Ordering::SeqCst) || !self.is_registered_worker(worker) {
                return Err("Experimental Session worker is stopping".to_string());
            }
        }
        let request_id = uuid_v7();
        let (settle_tx, settle_rx) = oneshot::channel();
        self.shared.state.lock().unwrap().pending_demand.insert(
            request_id.clone(),
            PendingDemand {
                attachment_id: attachment_id.to_string(),
                attached,
                worker: Arc::clone(worker),
                settle: settle_tx,
            },
        );
        self.spawn_demand_timer(request_id.clone(), compensate_on_timeout);
        let payload = json!({
            "type": "session_demand",
            "serverConnectionId": self.shared.link.server_connection_id(),
            "requestId": request_id,
            "attachmentId": attachment_id,
            "attached": attached,
        });
        if let Err(error) = self.shared.link.send(&worker.peer_id, payload).await {
            self.reject_demand(&request_id, plain_error(error));
        }
        settle_rx
            .await
            .map_err(|_| "Session worker demand channel closed".to_string())?
    }

    fn spawn_demand_timer(self: &Arc<Self>, request_id: String, compensate_on_timeout: bool) {
        let task = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(WORKER_DEMAND_TIMEOUT_MS)).await;
            if compensate_on_timeout {
                task.reconcile_demand_timeout(&request_id).await;
            } else {
                task.reject_demand(
                    &request_id,
                    plain_error("Session worker demand update timed out"),
                );
            }
        });
    }

    /// Upstream `#invoke`: worker operations have no wall-clock timeout;
    /// completion, disconnect, replacement or shutdown settles them.
    async fn invoke(
        self: &Arc<Self>,
        worker: &Arc<WorkerRecord>,
        scope: &WorkerOperationScope,
        call: ServiceCall,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<Option<Value>, WorkerOperationFailure> {
        if let Err(error) = self.operation_scope(worker, &scope.attachment_id) {
            return Err(plain_error(error));
        }
        if let Some(signal) = &cancel {
            if signal.is_cancelled() {
                return Err(plain_error("The operation was aborted"));
            }
        }
        let request_id = uuid_v7();
        let (settle_tx, settle_rx) = oneshot::channel();
        self.shared.state.lock().unwrap().pending_operations.insert(
            request_id.clone(),
            PendingWorkerOperation {
                worker: Arc::clone(worker),
                scope: scope.clone(),
                settle: settle_tx,
            },
        );
        let abort_task = cancel.map(|signal| {
            let task = Arc::clone(self);
            let request_id = request_id.clone();
            let peer_id = worker.peer_id.clone();
            let scope = scope.clone();
            tokio::spawn(async move {
                signal.cancelled().await;
                let registered = task
                    .shared
                    .state
                    .lock()
                    .unwrap()
                    .pending_operations
                    .contains_key(&request_id);
                if !registered {
                    return;
                }
                let _ = task
                    .shared
                    .link
                    .send(
                        &peer_id,
                        json!({
                            "type": "operation_cancel",
                            "requestId": request_id,
                            "scope": scope,
                        }),
                    )
                    .await;
                task.reject_operation(&request_id, plain_error("The operation was aborted"));
            })
        });
        let payload = json!({
            "type": "operation",
            "requestId": request_id,
            "scope": scope,
            "call": call,
        });
        if let Err(error) = self.shared.link.send(&worker.peer_id, payload).await {
            self.reject_operation(&request_id, plain_error(error));
        }
        let result = settle_rx
            .await
            .unwrap_or_else(|_| Err(plain_error("Session worker operation channel closed")));
        if let Some(task) = abort_task {
            task.abort();
        }
        result
    }

    /// Upstream `#operationScope`.
    fn operation_scope(
        &self,
        worker: &Arc<WorkerRecord>,
        attachment_id: &str,
    ) -> Result<WorkerOperationScope, String> {
        let state = self.shared.state.lock().unwrap();
        if state.detached || state.shutting_down || worker.stopping.load(Ordering::SeqCst) {
            return Err("Experimental Session worker is stopping".to_string());
        }
        let known = self.is_registered_worker_inner(&state, worker);
        if !known
            || !worker
                .attachment_ids
                .lock()
                .unwrap()
                .contains(attachment_id)
        {
            return Err("Experimental Session worker has no active attachment".to_string());
        }
        Ok(WorkerOperationScope {
            server_connection_id: self.shared.link.server_connection_id().to_string(),
            attachment_id: attachment_id.to_string(),
        })
    }

    fn is_registered_worker_inner(&self, state: &ManagerState, worker: &Arc<WorkerRecord>) -> bool {
        state
            .workers_by_peer
            .get(&worker.peer_id)
            .map(|current| Arc::ptr_eq(current, worker))
            .unwrap_or(false)
    }

    /// Upstream `#stopWorker` (memoized like the upstream `stopPromise`).
    /// Private in the port: stops are routed through
    /// [`RoutedSessionHandle::close`]/`shutdown`, matching the upstream
    /// call graph (the public `stopPromise` memoization is preserved).
    async fn stop_worker(self: &Arc<Self>, worker: Arc<WorkerRecord>) -> Result<(), String> {
        {
            let state = self.shared.state.lock().unwrap();
            if state.detached || !self.is_registered_worker_inner(&state, &worker) {
                return Ok(());
            }
        }
        worker
            .stop_result
            .get_or_init(|| self.stop_worker_internal(&worker))
            .await
            .clone()
    }

    async fn stop_worker_internal(
        self: &Arc<Self>,
        worker: &Arc<WorkerRecord>,
    ) -> Result<(), String> {
        self.reject_worker_operations(worker, plain_error("Session worker is stopping"));
        worker.stopping.store(true, Ordering::SeqCst);
        worker.expected_stop.store(true, Ordering::SeqCst);
        let _ = self
            .shared
            .link
            .send(&worker.peer_id, json!({ "type": "shutdown" }))
            .await;
        let timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(WORKER_SHUTDOWN_TIMEOUT_MS),
            worker.terminated.wait(),
        )
        .await
        .is_err();
        if timed_out {
            if !(self.shared.kill)(worker.pid) {
                return Err(format!(
                    "Failed to send SIGKILL to session worker {}",
                    worker.pid
                ));
            }
            self.remove_worker(worker, None);
            worker.terminated.wait().await;
        }
        Ok(())
    }

    /// Upstream `shutdown`.
    pub async fn shutdown(self: &Arc<Self>) {
        {
            let mut state = self.shared.state.lock().unwrap();
            if state.detached || state.shutting_down {
                return;
            }
            state.shutting_down = true;
        }
        let pending_workers: Vec<Arc<Mutex<PendingLaunch>>> = {
            let state = self.shared.state.lock().unwrap();
            state.pending.values().map(Arc::clone).collect()
        };
        for pending in &pending_workers {
            let (peer_id, payload) = {
                let guard = pending.lock().unwrap();
                (guard.peer_id.clone(), json!({ "type": "shutdown" }))
            };
            let _ = self.shared.link.send(&peer_id, payload).await;
        }
        let children: Vec<Arc<dyn InternalProcessChild>> = pending_workers
            .iter()
            .map(|pending| Arc::clone(&pending.lock().unwrap().child))
            .collect();
        let pending_timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(WORKER_SHUTDOWN_TIMEOUT_MS),
            wait_children_exit(&children),
        )
        .await
        .is_err();
        if pending_timed_out {
            for pending in &pending_workers {
                pending.lock().unwrap().child.kill();
            }
            wait_children_exit(&children).await;
        }
        let workers: Vec<Arc<WorkerRecord>> = {
            let state = self.shared.state.lock().unwrap();
            state.workers_by_session.values().map(Arc::clone).collect()
        };
        for worker in workers {
            let _ = Arc::clone(self).stop_worker(worker).await;
        }
        self.detach_state();
    }

    /// Upstream `detach`: forget workers without stopping them when this
    /// server is replaced.
    pub fn detach(&self) {
        let mut state = self.shared.state.lock().unwrap();
        if state.detached {
            return;
        }
        state.detached = true;
        let pending: Vec<Arc<Mutex<PendingLaunch>>> =
            state.pending.values().map(Arc::clone).collect();
        for pending in pending {
            let mut guard = pending.lock().unwrap();
            guard.timer_generation = 0;
            guard
                .promise
                .settle(Err("Experimental server was replaced".to_string()));
        }
        let request_ids: Vec<String> = state.pending_operations.keys().cloned().collect();
        for request_id in request_ids {
            Self::reject_operation_locked(
                &mut state,
                &request_id,
                plain_error("Experimental server was replaced during a worker operation"),
            );
        }
        drop(state);
        self.detach_state();
    }

    /// Upstream `#launch`.
    async fn launch(
        self: &Arc<Self>,
        metadata: &SessionWorkerMetadata,
        plugin_manifest_paths: &[String],
    ) -> Result<Arc<WorkerRecord>, String> {
        let session_key = metadata.path.clone();
        let peer_id = format!("worker-{}", uuid_v7());
        let token = uuid_v7();
        let options = SessionWorkerOptions {
            session_dir: self.shared.session_dir.clone(),
            metadata: metadata.clone(),
            provider: self
                .shared
                .model
                .as_ref()
                .and_then(|model| model.provider.clone()),
            model: self.shared.model.as_ref().map(|model| model.model.clone()),
            plugin_manifest_paths: plugin_manifest_paths.to_vec(),
        };
        let args = vec![serde_json::to_string(&options).map_err(|error| error.to_string())?];
        let extra_env = vec![
            (
                SESSION_WORKER_CONTROL_ADDRESS_ENV.to_string(),
                self.shared.link.control_path().to_string(),
            ),
            (SESSION_WORKER_CONTROL_TOKEN_ENV.to_string(), token.clone()),
            (
                SESSION_WORKER_SESSION_KEY_ENV.to_string(),
                base64url_encode(session_key.as_bytes()),
            ),
            (SESSION_WORKER_PEER_ID_ENV.to_string(), peer_id.clone()),
        ];
        let child: Arc<dyn InternalProcessChild> =
            self.shared.spawner.spawn(&args, &extra_env)?.into();
        let (_exit_tx, exit_rx) = oneshot::channel();
        let timer_generation = self.next_timer_generation();
        let pending = Arc::new(Mutex::new(PendingLaunch {
            session_key: session_key.clone(),
            peer_id: peer_id.clone(),
            token: token.clone(),
            plugin_manifest_paths: plugin_manifest_paths.to_vec(),
            child: Arc::clone(&child),
            timer_generation,
            promise: LaunchSlot::new(),
        }));
        {
            let mut state = self.shared.state.lock().unwrap();
            state
                .pending
                .insert(session_key.clone(), Arc::clone(&pending));
        }
        self.notify_worker_count_changed();
        self.spawn_startup_timer(session_key.clone());
        self.spawn_exit_watcher(Arc::clone(&pending), child, exit_rx);
        let promise = Arc::clone(pending.lock().unwrap().promise_slot());
        promise.wait().await
    }

    fn spawn_startup_timer(self: &Arc<Self>, session_key: String) {
        let task = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(WORKER_STARTUP_TIMEOUT_MS)).await;
            task.fail_pending(
                &session_key,
                plain_error("Session worker startup timed out"),
            );
        });
    }

    fn spawn_exit_watcher(
        self: &Arc<Self>,
        pending: Arc<Mutex<PendingLaunch>>,
        child: Arc<dyn InternalProcessChild>,
        mut exit_rx: oneshot::Receiver<()>,
    ) {
        let task = Arc::clone(self);
        tokio::spawn(async move {
            tokio::select! {
                _ = &mut exit_rx => {}
                _ = wait_child_exit(Arc::clone(&child)) => {}
            }
            task.child_exited(&pending);
        });
    }

    /// Upstream `#handleCoordinatorEvent`; the Rust port routes events in
    /// through this method instead of a listener subscription.
    pub fn handle_coordinator_event(&self, event: &CoordinatorConnectionEvent) {
        let mut state = self.shared.state.lock().unwrap();
        if state.detached {
            return;
        }
        match event {
            CoordinatorConnectionEvent::PeerConnected { .. } => {}
            CoordinatorConnectionEvent::PeerDisconnected { peer_id } => {
                Self::mark_discovered(&mut state, peer_id);
                if let Some(worker) = state.workers_by_peer.get(peer_id).cloned() {
                    let error = if worker.expected_stop.load(Ordering::SeqCst) {
                        None
                    } else {
                        Some(plain_error(format!(
                            "Session worker {} disconnected unexpectedly",
                            worker.metadata.id
                        )))
                    };
                    Self::remove_worker_locked(&mut state, &worker, error);
                }
                let pending = state
                    .pending
                    .values()
                    .find(|pending| pending.lock().unwrap().peer_id == *peer_id)
                    .map(Arc::clone);
                if let Some(pending) = pending {
                    let session_key = pending.lock().unwrap().session_key.clone();
                    drop(state);
                    self.fail_pending(
                        &session_key,
                        plain_error("Session worker disconnected during startup"),
                    );
                }
            }
            CoordinatorConnectionEvent::Message { from, payload } => {
                let Ok(event) = serde_json::from_value::<SessionWorkerEvent>(payload.clone())
                else {
                    // Upstream: an operation_response-shaped payload that
                    // fails schema validation rejects the worker's pending
                    // operations.
                    let is_operation_response = payload
                        .as_object()
                        .and_then(|object| object.get("type"))
                        .and_then(|value| value.as_str())
                        == Some("operation_response");
                    if is_operation_response {
                        if let Some(worker) = state.workers_by_peer.get(from).cloned() {
                            drop(state);
                            self.reject_worker_operations(
                                &worker,
                                plain_error(
                                    "Session worker returned an invalid operation response",
                                ),
                            );
                        }
                    }
                    return;
                };
                match event {
                    SessionWorkerEvent::WorkerFailed {
                        token,
                        session_key,
                        message,
                    } => {
                        let matches = match state.pending.get(&session_key) {
                            Some(pending) => {
                                let guard = pending.lock().unwrap();
                                guard.peer_id == *from && guard.token == token
                            }
                            None => false,
                        };
                        if matches {
                            drop(state);
                            self.fail_pending(
                                &session_key,
                                plain_error(format!("Session worker failed: {message}")),
                            );
                        }
                    }
                    SessionWorkerEvent::DemandApplied {
                        token,
                        session_key,
                        request_id,
                        attachment_id,
                        attached,
                    } => {
                        let Some(pending) = state.pending_demand.get(&request_id) else {
                            return;
                        };
                        let valid = pending.worker.peer_id == *from
                            && pending.worker.token == token
                            && pending.worker.metadata.path == session_key
                            && pending.attachment_id == attachment_id
                            && pending.attached == attached;
                        if !valid {
                            return;
                        }
                        let PendingDemand { settle, .. } =
                            state.pending_demand.remove(&request_id).unwrap();
                        let _ = settle.send(Ok(()));
                    }
                    SessionWorkerEvent::DemandRejected {
                        token,
                        session_key,
                        request_id,
                        message,
                        ..
                    } => {
                        let Some(pending) = state.pending_demand.get(&request_id) else {
                            return;
                        };
                        let valid = pending.worker.peer_id == *from
                            && pending.worker.token == token
                            && pending.worker.metadata.path == session_key;
                        if !valid {
                            return;
                        }
                        let PendingDemand { settle, .. } =
                            state.pending_demand.remove(&request_id).unwrap();
                        let _ =
                            settle.send(Err(format!("Session worker rejected demand: {message}")));
                        let _ = (token, session_key);
                    }
                    SessionWorkerEvent::OperationResponse {
                        token,
                        session_key,
                        response,
                        ..
                    } => {
                        drop(state);
                        self.handle_operation_response(from, &token, &session_key, &response);
                    }
                    SessionWorkerEvent::ServiceUpdate {
                        token,
                        session_key,
                        scope,
                        subscription_id,
                        update,
                        ..
                    } => {
                        Self::handle_service_event_locked(
                            &mut state,
                            from,
                            &token,
                            &session_key,
                            &scope,
                            &subscription_id,
                            &update,
                        );
                    }
                    SessionWorkerEvent::WorkerReady {
                        token,
                        session_key,
                        session_id,
                        pid,
                        metadata: ready_metadata,
                        plugin_manifest_paths,
                    } => {
                        self.handle_worker_ready(
                            state,
                            from,
                            &token,
                            &session_key,
                            &session_id,
                            pid,
                            &ready_metadata,
                            &plugin_manifest_paths,
                        );
                    }
                }
            }
        }
    }

    /// Upstream `#recordReadyWorker`.
    #[allow(clippy::too_many_arguments)]
    fn handle_worker_ready(
        &self,
        mut state: std::sync::MutexGuard<'_, ManagerState>,
        peer_id: &str,
        token: &str,
        session_key: &str,
        session_id: &str,
        pid: u64,
        ready_metadata: &SessionWorkerMetadata,
        plugin_manifest_paths: &[String],
    ) {
        let pid = u32::try_from(pid).unwrap_or(u32::MAX);
        if session_key != ready_metadata.path
            || session_id != ready_metadata.id
            || !is_absolute(&ready_metadata.cwd)
            || !is_absolute(&ready_metadata.path)
        {
            return;
        }
        Self::mark_discovered(&mut state, peer_id);
        let stale_plugins = state
            .pending
            .get(session_key)
            .map(|pending| {
                let guard = pending.lock().unwrap();
                guard.peer_id == peer_id
                    && !same_strings(plugin_manifest_paths, &guard.plugin_manifest_paths)
            })
            .unwrap_or(false);
        if stale_plugins {
            drop(state);
            self.fail_pending(
                session_key,
                plain_error("Session worker started with stale plugin packages"),
            );
            return;
        }
        if let Some(existing) = state.workers_by_session.get(session_key).cloned() {
            if existing.peer_id != peer_id {
                let payload = json!({ "type": "shutdown" });
                let peer = peer_id.to_string();
                let link = Arc::clone(&self.shared.link);
                tokio::spawn(async move {
                    let _ = link.send(&peer, payload).await;
                });
            }
            return;
        }
        if let Some(pending) = state.pending.get(session_key) {
            let guard = pending.lock().unwrap();
            if guard.peer_id != peer_id || guard.token != token || guard.child.pid() != Some(pid) {
                return;
            }
        }
        if state.workers_by_session.contains_key(session_key) {
            return;
        }
        let worker = Arc::new(WorkerRecord {
            peer_id: peer_id.to_string(),
            metadata: ready_metadata.clone(),
            pid,
            token: token.to_string(),
            plugin_manifest_paths: plugin_manifest_paths.to_vec(),
            terminated: TerminatedSlot::new(),
            attachment_ids: Mutex::new(HashSet::new()),
            expected_stop: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            stop_result: tokio::sync::OnceCell::new(),
        });
        state
            .workers_by_session
            .insert(session_key.to_string(), Arc::clone(&worker));
        state
            .workers_by_peer
            .insert(peer_id.to_string(), Arc::clone(&worker));
        state.worker_pids.insert(session_id.to_string(), pid);
        if let Some(pending) = state.pending.remove(session_key) {
            let mut guard = pending.lock().unwrap();
            guard.timer_generation = 0;
            guard.promise.settle(Ok(Arc::clone(&worker)));
        }
        drop(state);
        self.notify_worker_count_changed();
    }

    /// Upstream `#handleOperationResponse`.
    fn handle_operation_response(
        &self,
        peer_id: &str,
        token: &str,
        session_key: &str,
        response: &WorkerOperationResponse,
    ) {
        let mut state = self.shared.state.lock().unwrap();
        let request_id = operation_request_id(response).clone();
        let Some(pending) = state.pending_operations.get(&request_id) else {
            return;
        };
        let mismatch = match response {
            WorkerOperationResponse::OperationResult { scope, .. }
            | WorkerOperationResponse::OperationError { scope, .. } => {
                pending.worker.peer_id != peer_id
                    || pending.worker.token != token
                    || pending.worker.metadata.path != session_key
                    || scope.server_connection_id != pending.scope.server_connection_id
                    || scope.attachment_id != pending.scope.attachment_id
            }
        };
        if mismatch {
            drop(state);
            self.reject_operation(
                &request_id,
                plain_error("Session worker returned a mismatched operation response"),
            );
            return;
        }
        let PendingWorkerOperation { settle, .. } =
            state.pending_operations.remove(&request_id).unwrap();
        match response {
            WorkerOperationResponse::OperationError { code, message, .. } => {
                let _ = settle.send(Err(WorkerOperationFailure {
                    code: code.clone(),
                    message: message.clone(),
                }));
            }
            WorkerOperationResponse::OperationResult { result, .. } => {
                let _ = settle.send(Ok(result.clone()));
            }
        }
    }

    /// Upstream `#handleServiceEvent`.
    fn handle_service_event_locked(
        state: &mut ManagerState,
        peer_id: &str,
        token: &str,
        session_key: &str,
        scope: &WorkerOperationScope,
        subscription_id: &str,
        update: &Value,
    ) {
        let key = scoped_service_subscription_key(scope, subscription_id);
        let Some(entry) = state.service_subscriptions.get(&key) else {
            return;
        };
        if entry.worker.peer_id != peer_id
            || entry.worker.token != token
            || entry.worker.metadata.path != session_key
            || !same_scope(&entry.scope, scope)
        {
            return;
        }
        // Upstream parses with `parseServiceProviderUpdate` and drops invalid
        // updates; the port forwards the raw JSON (the chord update schema is
        // opaque at this seam).
        let listener = Arc::clone(&entry.listener);
        let update = update.clone();
        tokio::spawn(async move {
            listener(update).await;
        });
    }

    /// Upstream `#childExited`.
    fn child_exited(self: &Arc<Self>, pending: &Arc<Mutex<PendingLaunch>>) {
        let (session_key, peer_id) = {
            let guard = pending.lock().unwrap();
            (guard.session_key.clone(), guard.peer_id.clone())
        };
        let is_pending = self
            .shared
            .state
            .lock()
            .unwrap()
            .pending
            .get(&session_key)
            .map(|current| Arc::ptr_eq(current, pending))
            .unwrap_or(false);
        if is_pending {
            self.fail_pending(
                &session_key,
                plain_error("Session worker exited before readiness (unknown)"),
            );
            return;
        }
        let worker = self
            .shared
            .state
            .lock()
            .unwrap()
            .workers_by_peer
            .get(&peer_id)
            .cloned();
        if let Some(worker) = worker {
            let error = if worker.expected_stop.load(Ordering::SeqCst) {
                None
            } else {
                Some(plain_error(format!(
                    "Session worker {} exited unexpectedly (unknown)",
                    worker.metadata.id
                )))
            };
            self.remove_worker(&worker, error);
        }
    }

    /// Upstream `#failPending`.
    fn fail_pending(&self, session_key: &str, error: WorkerOperationFailure) {
        let mut state = self.shared.state.lock().unwrap();
        let Some(pending) = state.pending.get(session_key).cloned() else {
            return;
        };
        state.pending.remove(session_key);
        {
            let mut guard = pending.lock().unwrap();
            guard.timer_generation = 0;
            if !guard.child.has_exited() {
                guard.child.kill();
            }
            guard.promise.settle(Err(error.message));
        }
        drop(state);
        self.notify_worker_count_changed();
    }

    /// Upstream `#rejectDemand`.
    fn reject_demand(&self, request_id: &str, error: WorkerOperationFailure) {
        let mut state = self.shared.state.lock().unwrap();
        if let Some(PendingDemand { settle, .. }) = state.pending_demand.remove(request_id) {
            let _ = settle.send(Err(error.message));
        }
    }

    /// Upstream `#rejectOperation`.
    fn reject_operation(&self, request_id: &str, error: WorkerOperationFailure) {
        let mut state = self.shared.state.lock().unwrap();
        Self::reject_operation_locked(&mut state, request_id, error);
    }

    fn reject_operation_locked(
        state: &mut ManagerState,
        request_id: &str,
        error: WorkerOperationFailure,
    ) {
        if let Some(PendingWorkerOperation { settle, .. }) =
            state.pending_operations.remove(request_id)
        {
            let _ = settle.send(Err(error));
        }
    }

    fn reject_worker_operations(&self, worker: &Arc<WorkerRecord>, error: WorkerOperationFailure) {
        let mut state = self.shared.state.lock().unwrap();
        let request_ids: Vec<String> = state
            .pending_operations
            .iter()
            .filter(|(_, pending)| Arc::ptr_eq(&pending.worker, worker))
            .map(|(request_id, _)| request_id.clone())
            .collect();
        for request_id in request_ids {
            Self::reject_operation_locked(&mut state, &request_id, error.clone());
        }
    }

    /// Upstream `#reconcileDemandTimeout`.
    async fn reconcile_demand_timeout(self: &Arc<Self>, request_id: &str) {
        let pending = {
            let mut state = self.shared.state.lock().unwrap();
            state.pending_demand.remove(request_id)
        };
        let Some(PendingDemand {
            attachment_id,
            attached,
            worker,
            settle,
            ..
        }) = pending
        else {
            return;
        };
        let timeout_error = "Session worker demand update timed out".to_string();
        match self
            .apply_demand(&worker, &attachment_id, false, false)
            .await
        {
            Ok(()) => {
                if attached {
                    let _ = settle.send(Err(timeout_error));
                } else {
                    let _ = settle.send(Ok(()));
                }
            }
            Err(_) => {
                if self.stop_worker(worker).await.is_ok() {
                    let _ = settle.send(Err(
                        "Session worker demand reconciliation failed; worker was terminated"
                            .to_string(),
                    ));
                } else {
                    let _ = settle.send(Err(
                        "Session worker demand reconciliation and termination failed".to_string(),
                    ));
                }
            }
        }
    }

    /// Upstream `#removeWorker`.
    fn remove_worker(&self, worker: &Arc<WorkerRecord>, error: Option<WorkerOperationFailure>) {
        let mut state = self.shared.state.lock().unwrap();
        Self::remove_worker_locked(&mut state, worker, error);
    }

    fn remove_worker_locked(
        state: &mut ManagerState,
        worker: &Arc<WorkerRecord>,
        error: Option<WorkerOperationFailure>,
    ) {
        let known = state
            .workers_by_peer
            .get(&worker.peer_id)
            .map(|current| Arc::ptr_eq(current, worker))
            .unwrap_or(false);
        if !known {
            return;
        }
        let request_ids: Vec<String> = state
            .pending_operations
            .iter()
            .filter(|(_, pending)| Arc::ptr_eq(&pending.worker, worker))
            .map(|(request_id, _)| request_id.clone())
            .collect();
        for request_id in request_ids {
            Self::reject_operation_locked(
                state,
                &request_id,
                plain_error("Session worker disconnected during an operation"),
            );
        }
        let subscription_keys: Vec<String> = state
            .service_subscriptions
            .iter()
            .filter(|(_, entry)| Arc::ptr_eq(&entry.worker, worker))
            .map(|(key, _)| key.clone())
            .collect();
        for key in subscription_keys {
            state.service_subscriptions.remove(&key);
        }
        let demand_ids: Vec<String> = state
            .pending_demand
            .iter()
            .filter(|(_, pending)| Arc::ptr_eq(&pending.worker, worker))
            .map(|(request_id, _)| request_id.clone())
            .collect();
        for request_id in demand_ids {
            if let Some(PendingDemand { settle, .. }) = state.pending_demand.remove(&request_id) {
                let _ = settle.send(Err(
                    "Session worker disconnected during demand update".to_string()
                ));
            }
        }
        state.workers_by_peer.remove(&worker.peer_id);
        state.workers_by_session.remove(&worker.metadata.path);
        if state.worker_pids.get(&worker.metadata.id) == Some(&worker.pid) {
            state.worker_pids.remove(&worker.metadata.id);
        }
        worker.terminated.resolve(error);
    }

    fn notify_worker_count_changed(&self) {
        let count = {
            let state = self.shared.state.lock().unwrap();
            state.workers_by_session.len() + state.pending.len()
        };
        if let Some(callback) = &self.shared.on_worker_count_changed {
            (callback.lock().unwrap())(count);
        }
    }

    /// Upstream `#markDiscovered`.
    fn mark_discovered(state: &mut ManagerState, peer_id: &str) {
        let Some(discovery_peers) = state.discovery_peers.as_mut() else {
            return;
        };
        if !discovery_peers.remove(peer_id) || !discovery_peers.is_empty() {
            return;
        }
        if let Some(done) = state.discovery_done.take() {
            let _ = done.send(());
        }
    }

    fn next_timer_generation(&self) -> u64 {
        let mut state = self.shared.state.lock().unwrap();
        state.timer_serial += 1;
        state.timer_serial
    }

    fn detach_state(&self) {
        let mut state = self.shared.state.lock().unwrap();
        let request_ids: Vec<String> = state.pending_operations.keys().cloned().collect();
        for request_id in request_ids {
            Self::reject_operation_locked(
                &mut state,
                &request_id,
                plain_error("Experimental server detached during a worker operation"),
            );
        }
        for (_, pending) in state.pending_demand.drain() {
            let _ = pending.settle.send(Ok(()));
        }
        state.pending.clear();
        state.service_subscriptions.clear();
        state.workers_by_peer.clear();
        state.workers_by_session.clear();
        state.worker_pids.clear();
        state.discovery_peers = None;
        if let Some(done) = state.discovery_done.take() {
            let _ = done.send(());
        }
    }
}

impl PendingLaunch {
    fn promise_slot(&self) -> &Arc<LaunchSlot> {
        &self.promise
    }
}

/// Helper for `closeSession`'s pending-or-worker lookup.
enum ManagedWorker {
    Worker(Arc<WorkerRecord>),
    Pending(Arc<LaunchSlot>),
}

/// D5: upstream `RoutedSessionHandle`.
pub struct RoutedSessionHandle {
    manager: Arc<SessionWorkerManager>,
    worker: Arc<WorkerRecord>,
}

impl RoutedSessionHandle {
    /// Upstream `terminated` promise (settles with the unexpected-stop error,
    /// if any).
    pub async fn terminated(&self) -> Option<WorkerOperationFailure> {
        self.worker.terminated.wait().await;
        self.worker.terminated.error()
    }

    /// Upstream `attachClient`.
    pub async fn attach_client(&self) -> Result<RoutedSessionAttachment, String> {
        self.manager.attach_client(Arc::clone(&self.worker)).await
    }

    /// Upstream `close`.
    pub async fn close(&self) -> Result<(), String> {
        self.manager.stop_worker(Arc::clone(&self.worker)).await
    }
}

/// D5: upstream `RoutedSessionAttachment`.
pub struct RoutedSessionAttachment {
    manager: Arc<SessionWorkerManager>,
    worker: Arc<WorkerRecord>,
    scope: WorkerOperationScope,
    released: Arc<AtomicBool>,
    attachment_id: String,
}

impl std::fmt::Debug for RoutedSessionAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutedSessionAttachment")
            .field("attachment_id", &self.attachment_id)
            .finish_non_exhaustive()
    }
}

impl RoutedSessionAttachment {
    /// Upstream `invokeService`.
    pub async fn invoke_service(
        &self,
        call: ServiceCall,
        publish: Arc<dyn Fn(String, Value) -> BoxFuture<'static, ()> + Send + Sync>,
        cancel: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<Option<Value>, WorkerOperationFailure> {
        self.manager
            .invoke_service(Arc::clone(&self.worker), &self.scope, call, publish, cancel)
            .await
    }

    /// Upstream `release`.
    pub async fn release(&self) -> Result<(), String> {
        if self.released.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let detached = { self.manager.shared.state.lock().unwrap().detached };
        if detached
            || !self
                .worker
                .attachment_ids
                .lock()
                .unwrap()
                .contains(&self.attachment_id)
        {
            self.cleanup();
            return Ok(());
        }
        let result = self
            .manager
            .apply_demand(&self.worker, &self.attachment_id, false, true)
            .await;
        self.cleanup();
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                if !detached && !self.manager.shared.link.was_replaced() {
                    Err(error)
                } else {
                    Ok(())
                }
            }
        }
    }

    fn cleanup(&self) {
        self.worker
            .attachment_ids
            .lock()
            .unwrap()
            .remove(&self.attachment_id);
        let mut state = self.manager.shared.state.lock().unwrap();
        let keys: Vec<String> = state
            .service_subscriptions
            .iter()
            .filter(|(_, entry)| {
                Arc::ptr_eq(&entry.worker, &self.worker) && same_scope(&entry.scope, &self.scope)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            state.service_subscriptions.remove(&key);
        }
    }
}

async fn wait_child_exit(child: Arc<dyn InternalProcessChild>) {
    loop {
        if child.has_exited() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

async fn wait_children_exit(children: &[Arc<dyn InternalProcessChild>]) {
    let mut exits = futures::stream::FuturesUnordered::new();
    for child in children {
        if child.has_exited() {
            continue;
        }
        exits.push(wait_child_exit(Arc::clone(child)));
    }
    while exits.next().await.is_some() {}
}

fn operation_request_id(response: &WorkerOperationResponse) -> &String {
    match response {
        WorkerOperationResponse::OperationResult { request_id, .. }
        | WorkerOperationResponse::OperationError { request_id, .. } => request_id,
    }
}

/// Upstream `sameStrings`.
fn same_strings(left: &[String], right: &[String]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

/// Upstream `scopedServiceSubscriptionKey`.
fn scoped_service_subscription_key(scope: &WorkerOperationScope, subscription_id: &str) -> String {
    format!(
        "{}\0{}\0{}",
        scope.server_connection_id, scope.attachment_id, subscription_id
    )
}

/// Node `path.isAbsolute` (posix semantics: a leading `/` is absolute even
/// on Windows, matching the upstream session metadata contract).
fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

/// `Buffer.from(key).toString("base64url")` equivalent (no new dependencies).
fn base64url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(triple >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[triple as usize & 0x3f] as char);
        }
    }
    out
}

#[cfg(unix)]
fn kill_pid_sigkill(pid: u32) -> bool {
    kill_pid_sigkill_with(pid, rustix::process::kill_process)
}

#[cfg(unix)]
fn kill_pid_sigkill_with(
    pid: u32,
    send_signal: impl FnOnce(rustix::process::Pid, rustix::process::Signal) -> rustix::io::Result<()>,
) -> bool {
    // worker_ready requires a positive process ID. Reject zero and values
    // outside pid_t instead of accidentally signalling a process group after
    // an unchecked u32 -> i32 cast. Discovered workers need not be our children.
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    // Upstream process.kill(pid, "SIGKILL") ignores ESRCH only. Permission
    // and other errors must not be reported as successful termination.
    matches!(
        send_signal(pid, rustix::process::Signal::KILL),
        Ok(()) | Err(rustix::io::Errno::SRCH)
    )
}

#[cfg(not(unix))]
fn kill_pid_sigkill(_pid: u32) -> bool {
    // Windows has no SIGKILL; upstream session workers are POSIX-managed and
    // the fake-child seam covers the deterministic behavior.
    false
}

#[cfg(test)]
mod tests;
