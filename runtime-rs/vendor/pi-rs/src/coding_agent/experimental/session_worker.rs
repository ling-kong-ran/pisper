//! Port of upstream `experimental/session-worker.ts`
//! (sha256 f2ec55d9f48eb8ddb39f40e8681ce8424298742e7a2893db8f7d12923262b043).
//!
//! Ported: the worker protocol schema surface (`SessionWorkerMetadata`,
//! `SessionWorkerOptions`, `ServiceCall`, `WorkerOperationRequest/Response`,
//! `SessionWorkerCommand/Event`, the coordinator-input frames), the env-var
//! contract, the lifecycle grace parsing helpers, and the full
//! `WorkerLifecycle` reconciliation state machine (tokio timers replacing the
//! upstream `setTimeout(...).unref()` pairs, tokio's paused test clock
//! replacing `vi.useFakeTimers`).
//!
//! D3 seam (see mod.rs docs): the worker process main loop is ported over
//! the transport seam — `connect_control` (the `connectControl` handshake),
//! `run_command_loop` (`readCommands`) with the exact per-line destroy
//! errors, and the `run(...)` handler bodies (`handle_demand`,
//! `handle_operation`, `handle_operation_cancel`,
//! `handle_server_disconnected`). The Node-runtime-bound pieces stay behind
//! explicit embedder seams: the `proper-lockfile` session ownership, the
//! `AgentHarness`/`JsonlSessionRepo`/`NodeExecutionEnv` construction
//! (`runSessionWorkerWithHarness`'s `createHarness` callback), and the
//! SIGINT/SIGTERM registration. The frame schemas those pieces exchange are
//! byte-compatible via serde.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::coding_agent::experimental::coordinator::COORDINATOR_PROTOCOL_VERSION;

pub const SESSION_WORKER_CONTROL_ADDRESS_ENV: &str = "PI_SESSION_WORKER_CONTROL_ADDRESS";
pub const SESSION_WORKER_CONTROL_TOKEN_ENV: &str = "PI_SESSION_WORKER_CONTROL_TOKEN";
pub const SESSION_WORKER_SESSION_KEY_ENV: &str = "PI_SESSION_WORKER_SESSION_KEY_BASE64";
pub const SESSION_WORKER_PEER_ID_ENV: &str = "PI_SESSION_WORKER_PEER_ID";

pub const DEFAULT_INITIAL_DEMAND_GRACE_MS: u64 = 10_000;
pub const DEFAULT_ORPHAN_DEMAND_GRACE_MS: u64 = 30_000;
pub const SESSION_WORKER_INITIAL_DEMAND_GRACE_ENV: &str =
    "__PI_SESSION_WORKER_INITIAL_DEMAND_GRACE_MS";
pub const SESSION_WORKER_ORPHAN_DEMAND_GRACE_ENV: &str =
    "__PI_SESSION_WORKER_ORPHAN_DEMAND_GRACE_MS";

/// Upstream `SessionWorkerMetadataSchema` (strict object).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionWorkerMetadata {
    pub id: String,
    pub created_at: i64,
    pub storage_version: i64,
    pub cwd: String,
    pub path: String,
    /// Upstream `Type.Number()`; node serializes integral values without a
    /// fractional part, so the port uses `serde_json::Number` for byte
    /// compatibility.
    pub modified_at: serde_json::Number,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}

/// Upstream `SessionWorkerOptionsSchema` (strict object).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionWorkerOptions {
    pub session_dir: String,
    pub metadata: SessionWorkerMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub plugin_manifest_paths: Vec<String>,
}

/// Upstream `WorkerOperationScopeSchema`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerOperationScope {
    pub server_connection_id: String,
    pub attachment_id: String,
}

/// Upstream `ServiceCallSchema` (opaque JSON payload arrays).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceCall {
    pub service_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<ServiceCallInstance>,
    pub member: String,
    pub args: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceCallInstance {
    pub key: String,
    pub generation: u64,
}

/// Upstream `WorkerOperationRequestSchema`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerOperationRequest {
    #[serde(rename = "type")]
    pub message_type: WorkerOperationRequestType,
    pub request_id: String,
    pub scope: WorkerOperationScope,
    pub call: ServiceCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerOperationRequestType {
    #[serde(rename = "operation")]
    Operation,
}

/// Upstream `WorkerOperationResponseSchema` union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerOperationResponse {
    #[serde(rename = "operation_result", rename_all = "camelCase")]
    OperationResult {
        request_id: String,
        scope: WorkerOperationScope,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    #[serde(rename = "operation_error", rename_all = "camelCase")]
    OperationError {
        request_id: String,
        scope: WorkerOperationScope,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        message: String,
    },
}

/// Upstream `SessionWorkerCommandSchema` union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionWorkerCommand {
    #[serde(rename = "shutdown")]
    Shutdown,
    #[serde(rename = "discover_workers")]
    DiscoverWorkers,
    #[serde(rename = "session_demand", rename_all = "camelCase")]
    SessionDemand {
        server_connection_id: String,
        request_id: String,
        attachment_id: String,
        attached: bool,
    },
    #[serde(rename = "operation", rename_all = "camelCase")]
    Operation {
        request_id: String,
        scope: WorkerOperationScope,
        call: ServiceCall,
    },
    #[serde(rename = "operation_cancel", rename_all = "camelCase")]
    OperationCancel {
        request_id: String,
        scope: WorkerOperationScope,
    },
}

/// Upstream `SessionWorkerEventSchema` union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionWorkerEvent {
    #[serde(rename = "worker_ready", rename_all = "camelCase")]
    WorkerReady {
        token: String,
        session_key: String,
        session_id: String,
        pid: u64,
        metadata: SessionWorkerMetadata,
        plugin_manifest_paths: Vec<String>,
    },
    #[serde(rename = "worker_failed", rename_all = "camelCase")]
    WorkerFailed {
        token: String,
        session_key: String,
        message: String,
    },
    #[serde(rename = "demand_applied", rename_all = "camelCase")]
    DemandApplied {
        token: String,
        session_key: String,
        request_id: String,
        attachment_id: String,
        attached: bool,
    },
    #[serde(rename = "demand_rejected", rename_all = "camelCase")]
    DemandRejected {
        token: String,
        session_key: String,
        request_id: String,
        message: String,
    },
    #[serde(rename = "operation_response", rename_all = "camelCase")]
    OperationResponse {
        token: String,
        session_key: String,
        response: WorkerOperationResponse,
    },
    #[serde(rename = "service_update", rename_all = "camelCase")]
    ServiceUpdate {
        token: String,
        session_key: String,
        scope: WorkerOperationScope,
        subscription_id: String,
        update: Value,
    },
}

/// Upstream `CoordinatorInputSchema` union (coordinator -> worker frames).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorInput {
    #[serde(rename = "peer_registered", rename_all = "camelCase")]
    PeerRegistered {
        peer_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        server_connection_id: Option<String>,
    },
    #[serde(rename = "server_connected", rename_all = "camelCase")]
    ServerConnected { server_connection_id: String },
    #[serde(rename = "server_disconnected", rename_all = "camelCase")]
    ServerDisconnected { server_connection_id: String },
    #[serde(rename = "message")]
    Message { from: String, payload: Value },
}

/// Upstream validates `message.from` as `Type.Literal("server")`.
pub fn validate_coordinator_input(input: &CoordinatorInput) -> Result<(), String> {
    if let CoordinatorInput::Message { from, .. } = input {
        if from != "server" {
            return Err("Coordinator worker message must originate from the server".to_string());
        }
    }
    Ok(())
}

// ── chord service-call conversions (D5 helper face) ────────────────────────

/// Convert the experimental `ServiceCall` face to chord's. Returns `None`
/// for calls chord would reject (`instance` fields always convert; this is
/// total in practice but stays an `Option` so the D5 codec can degrade to
/// "not a control call" instead of panicking).
pub fn service_call_to_chord(call: &ServiceCall) -> Option<crate::chord::types::ServiceCall> {
    Some(crate::chord::types::ServiceCall {
        service_id: call.service_id.clone(),
        instance: call.instance.as_ref().map(|instance| {
            crate::chord::types::ServiceInstanceAddress {
                key: instance.key.clone(),
                generation: instance.generation,
            }
        }),
        member: call.member.clone(),
        args: call.args.clone(),
    })
}

/// Inverse of [`service_call_to_chord`].
pub fn service_call_from_chord(call: &crate::chord::types::ServiceCall) -> ServiceCall {
    ServiceCall {
        service_id: call.service_id.clone(),
        instance: call.instance.as_ref().map(|instance| ServiceCallInstance {
            key: instance.key.clone(),
            generation: instance.generation,
        }),
        member: call.member.clone(),
        args: call.args.clone(),
    }
}

// ── Remote service error codes (chord `services/errors.ts`) ────────────────

/// Upstream `REMOTE_SERVICE_ERROR_CODES` (worker `operation_error` codes).
pub const REMOTE_SERVICE_ERROR_CODES: [&str; 8] = [
    "service_not_allowed",
    "service_not_found",
    "service_mode_mismatch",
    "service_member_not_found",
    "service_member_mismatch",
    "service_instance_not_found",
    "service_stale_instance",
    "service_invalid_value",
];

/// Upstream `isRemoteServiceErrorCode`.
pub fn is_remote_service_error_code(candidate: &str) -> bool {
    REMOTE_SERVICE_ERROR_CODES.contains(&candidate)
}

/// Upstream `lifecycleDelay`: parse a non-negative safe-integer override from
/// the environment, exact error text.
pub fn lifecycle_delay(
    lookup: impl FnOnce(&str) -> Option<String>,
    name: &str,
    fallback: u64,
) -> Result<u64, String> {
    let Some(value) = lookup(name) else {
        return Ok(fallback);
    };
    // Upstream uses Number(value) + Number.isSafeInteger. JSON numbers beyond
    // 2^53 are the only values a f64 parse would misjudge; we mirror with an
    // f64 parse and the same bounds check.
    let parsed: f64 = value
        .trim()
        .parse()
        .map_err(|_| format!("{name} must be a non-negative safe integer"))?;
    if !parsed.is_finite()
        || parsed.fract() != 0.0
        || !(0.0..=9_007_199_254_740_991.0).contains(&parsed)
    {
        return Err(format!("{name} must be a non-negative safe integer"));
    }
    Ok(parsed as u64)
}

/// Upstream `demandKey`.
pub fn demand_key(server_connection_id: &str, attachment_id: &str) -> String {
    format!("{server_connection_id}\0{attachment_id}")
}

/// Upstream `sameScope`.
pub fn same_scope(left: &WorkerOperationScope, right: &WorkerOperationScope) -> bool {
    left.server_connection_id == right.server_connection_id
        && left.attachment_id == right.attachment_id
}

/// Upstream `WorkerLifecycle`: worker-local reconciliation of
/// server-generation demand and Harness activity. The retire callback fires
/// at most once, exactly when demand, harness activity and retirement holds
/// are all drained after initialization.
pub struct WorkerLifecycle {
    shared: Arc<LifecycleShared>,
}

struct LifecycleShared {
    initial_demand_grace_ms: u64,
    orphan_demand_grace_ms: u64,
    on_retire: Box<dyn Fn() + Send + Sync>,
    timer_serial: AtomicU64,
    state: Mutex<LifecycleState>,
}

struct LifecycleState {
    demands: HashMap<String, LifecycleDemand>,
    active_operations: HashSet<String>,
    current_server_connection_id: Option<String>,
    initial_timer: Option<u64>,
    demand_initialized: bool,
    retirement_holds: u64,
    retiring: bool,
}

struct LifecycleDemand {
    server_connection_id: String,
    timer: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleOperationKind {
    Run,
    Compaction,
    Navigation,
}

impl LifecycleOperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleOperationKind::Run => "run",
            LifecycleOperationKind::Compaction => "compaction",
            LifecycleOperationKind::Navigation => "navigation",
        }
    }
}

/// Guard returned by `hold_retirement` / `begin_request` (upstream's release
/// closure). Release is idempotent; dropping without an explicit `release`
/// also releases.
pub struct RetirementGuard {
    shared: Arc<LifecycleShared>,
    released: Arc<AtomicBool>,
}

impl std::fmt::Debug for RetirementGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RetirementGuard")
    }
}

impl RetirementGuard {
    pub fn release(self) {
        // release happens in Drop
    }
}

impl Drop for RetirementGuard {
    fn drop(&mut self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut state = self.shared.state.lock().unwrap();
        state.retirement_holds -= 1;
        let shared = Arc::clone(&self.shared);
        LifecycleShared::reconcile(&shared, &mut state);
    }
}

impl WorkerLifecycle {
    pub fn new(
        initial_server_connection_id: Option<String>,
        initial_demand_grace_ms: u64,
        orphan_demand_grace_ms: u64,
        on_retire: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        let shared = Arc::new(LifecycleShared {
            initial_demand_grace_ms,
            orphan_demand_grace_ms,
            on_retire,
            timer_serial: AtomicU64::new(1),
            state: Mutex::new(LifecycleState {
                demands: HashMap::new(),
                active_operations: HashSet::new(),
                current_server_connection_id: initial_server_connection_id,
                initial_timer: None,
                demand_initialized: false,
                retirement_holds: 0,
                retiring: false,
            }),
        });
        let initial_timer = LifecycleShared::arm_initial_timer(&shared);
        shared.state.lock().unwrap().initial_timer = Some(initial_timer);
        WorkerLifecycle { shared }
    }

    /// Upstream `serverConnected`: clear orphan timers for demands of the new
    /// generation.
    pub fn server_connected(&self, server_connection_id: &str) {
        let mut state = self.shared.state.lock().unwrap();
        state.current_server_connection_id = Some(server_connection_id.to_string());
        for demand in state.demands.values_mut() {
            if demand.server_connection_id == server_connection_id && demand.timer.is_some() {
                demand.timer = None;
            }
        }
    }

    /// Upstream `serverDisconnected`: arm the orphan grace timer for demands
    /// of the disconnected generation that have none yet.
    pub fn server_disconnected(&self, server_connection_id: &str) {
        let shared = &self.shared;
        let mut state = shared.state.lock().unwrap();
        if state.current_server_connection_id.as_deref() == Some(server_connection_id) {
            state.current_server_connection_id = None;
        }
        let mut armed: Vec<(String, u64)> = Vec::new();
        for (key, demand) in state.demands.iter_mut() {
            if demand.server_connection_id == server_connection_id && demand.timer.is_none() {
                let generation = shared.timer_serial.fetch_add(1, Ordering::SeqCst);
                demand.timer = Some(generation);
                armed.push((key.clone(), generation));
            }
        }
        for (key, generation) in armed {
            LifecycleShared::spawn_orphan_timer(Arc::clone(shared), key, generation);
        }
    }

    /// Upstream `beginRequest`: validate the request against the live
    /// generation and attachment, then hold retirement.
    pub fn begin_request(
        &self,
        server_connection_id: &str,
        attachment_id: &str,
    ) -> Result<RetirementGuard, String> {
        {
            let state = self.shared.state.lock().unwrap();
            if state.retiring {
                return Err("Session worker is retiring".to_string());
            }
            if state.current_server_connection_id.as_deref() != Some(server_connection_id) {
                return Err(
                    "Session worker received a request from a stale server generation".to_string(),
                );
            }
            let demand = state
                .demands
                .get(&demand_key(server_connection_id, attachment_id));
            match demand {
                Some(demand) if demand.timer.is_none() => {}
                _ => {
                    return Err(
                        "Session worker request does not match the active attachment".to_string(),
                    );
                }
            }
        }
        Ok(self.hold_retirement())
    }

    /// Upstream `holdRetirement`.
    pub fn hold_retirement(&self) -> RetirementGuard {
        self.shared.state.lock().unwrap().retirement_holds += 1;
        RetirementGuard {
            shared: Arc::clone(&self.shared),
            released: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Upstream `setDemand`.
    pub fn set_demand(
        &self,
        server_connection_id: &str,
        attachment_id: &str,
        attached: bool,
    ) -> Result<(), String> {
        let shared = &self.shared;
        let mut state = shared.state.lock().unwrap();
        if state.retiring {
            return Err("Session worker is retiring".to_string());
        }
        if state.current_server_connection_id.as_deref() != Some(server_connection_id) {
            return Err(
                "Session worker received demand from a stale server generation".to_string(),
            );
        }
        state.demand_initialized = true;
        if state.initial_timer.take().is_some() {
            // Cancellation is by generation: bump the serial so the armed
            // timer observes a stale token and no-ops.
            shared.timer_serial.fetch_add(1, Ordering::SeqCst);
        }
        let key = demand_key(server_connection_id, attachment_id);
        if let Some(previous) = state.demands.get(&key) {
            if previous.timer.is_some() {
                shared.timer_serial.fetch_add(1, Ordering::SeqCst);
            }
        }
        if attached {
            state.demands.insert(
                key,
                LifecycleDemand {
                    server_connection_id: server_connection_id.to_string(),
                    timer: None,
                },
            );
        } else {
            state.demands.remove(&key);
        }
        LifecycleShared::reconcile(shared, &mut state);
        Ok(())
    }

    /// Upstream `operationStarted`.
    pub fn operation_started(&self, kind: LifecycleOperationKind, lane: &str, operation_id: &str) {
        let key = format!("{}\0{lane}\0{operation_id}", kind.as_str());
        self.shared
            .state
            .lock()
            .unwrap()
            .active_operations
            .insert(key);
    }

    /// Upstream `operationStopped`.
    pub fn operation_stopped(&self, kind: LifecycleOperationKind, lane: &str, operation_id: &str) {
        let shared = &self.shared;
        let key = format!("{}\0{lane}\0{operation_id}", kind.as_str());
        let mut state = shared.state.lock().unwrap();
        state.active_operations.remove(&key);
        LifecycleShared::reconcile(shared, &mut state);
    }

    /// Upstream `close`.
    pub fn close(&self) {
        let shared = &self.shared;
        let mut state = shared.state.lock().unwrap();
        if state.initial_timer.take().is_some() {
            shared.timer_serial.fetch_add(1, Ordering::SeqCst);
        }
        let mut cleared_timer = false;
        for demand in state.demands.values_mut() {
            if demand.timer.take().is_some() {
                cleared_timer = true;
            }
        }
        if cleared_timer {
            shared.timer_serial.fetch_add(1, Ordering::SeqCst);
        }
        state.demands.clear();
    }
}

impl LifecycleShared {
    fn arm_initial_timer(shared: &Arc<LifecycleShared>) -> u64 {
        let generation = shared.timer_serial.fetch_add(1, Ordering::SeqCst);
        let task_shared = Arc::clone(shared);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(
                task_shared.initial_demand_grace_ms,
            ))
            .await;
            let mut state = task_shared.state.lock().unwrap();
            if state.initial_timer == Some(generation) {
                state.initial_timer = None;
                state.demand_initialized = true;
                LifecycleShared::reconcile(&task_shared, &mut state);
            }
        });
        generation
    }

    fn spawn_orphan_timer(shared: Arc<LifecycleShared>, key: String, generation: u64) {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(
                shared.orphan_demand_grace_ms,
            ))
            .await;
            let mut state = shared.state.lock().unwrap();
            let still_armed = state
                .demands
                .get(&key)
                .is_some_and(|demand| demand.timer == Some(generation));
            if still_armed {
                state.demands.remove(&key);
                LifecycleShared::reconcile(&shared, &mut state);
            }
        });
    }

    /// Upstream `#reconcile`.
    fn reconcile(shared: &Arc<LifecycleShared>, state: &mut LifecycleState) {
        if state.retiring
            || !state.demand_initialized
            || state.retirement_holds != 0
            || !state.active_operations.is_empty()
            || !state.demands.is_empty()
        {
            return;
        }
        state.retiring = true;
        (shared.on_retire)();
    }
}

// ── D3: worker process main loop ───────────────────────────────────────────
//
// Upstream `connectControl` / `readCommands` / the `run(...)` command wiring
// over the transport seam (`node:net` sockets become
// [`crate::coding_agent::experimental::coordinator::transport::
// ControlSocket`]); the AgentHarness/lockfile/facet-host wiring stays behind
// explicit seams so the deterministic command handling is testable end to
// end.

use crate::coding_agent::experimental::coordinator::transport::{
    read_line_capped, ControlConnector, ControlSocket,
};
use crate::coding_agent::experimental::process::encode_control_line;
use futures::future::BoxFuture;

/// Upstream's `process.env` reads, injectable for tests.
pub type WorkerEnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Upstream `connectControl`'s returned control surface.
pub struct WorkerControl {
    pub initial_server_connection_id: Option<String>,
    writer: Arc<Mutex<Box<dyn ControlSocket>>>,
    messages: Mutex<std::sync::mpsc::Receiver<Result<String, WorkerReadError>>>,
    closed: Arc<AtomicBool>,
}

/// Inbound control-stream terminal states (upstream: socket destroy/close
/// ends the `for await` loop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerReadError {
    Eof,
    Transport(String),
}

impl WorkerControl {
    /// Upstream `send(event)`: `{ type: "send", to: "server", payload }`.
    pub fn send_event(&self, event: &SessionWorkerEvent) -> Result<(), String> {
        let line = encode_control_line(&serde_json::json!({
            "type": "send",
            "to": "server",
            "payload": event,
        }))
        .map_err(|error| error.to_string())?;
        self.writer
            .lock()
            .unwrap()
            .write_line(&line)
            .map_err(|error| error.to_string())
    }

    /// Next inbound coordinator frame (raw JSON), blocking.
    pub fn next_message(&self) -> Result<String, WorkerReadError> {
        match self.messages.lock().unwrap().recv() {
            Ok(line) => line,
            Err(_) => Err(WorkerReadError::Eof),
        }
    }

    /// Upstream `control.socket.destroy(...)`.
    pub fn destroy(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.writer.lock().unwrap().shutdown();
        }
    }
}

/// Upstream `connectControl`: read the PI_* env contract, connect, send
/// `register_peer`, and require a `peer_registered` reply. The control
/// token/session key are returned to the caller (upstream re-reads them in
/// `run`).
pub fn connect_control(
    lookup: WorkerEnvLookup,
    connector: &dyn ControlConnector,
) -> Result<(WorkerControl, String, String), String> {
    let address = lookup(SESSION_WORKER_CONTROL_ADDRESS_ENV);
    let token = lookup(SESSION_WORKER_CONTROL_TOKEN_ENV);
    let encoded_session_key = lookup(SESSION_WORKER_SESSION_KEY_ENV);
    let (Some(address), Some(token), Some(encoded_session_key)) =
        (address, token, encoded_session_key)
    else {
        return Err("Session worker requires a control address".to_string());
    };
    let peer_id = lookup(SESSION_WORKER_PEER_ID_ENV)
        .filter(|peer_id| !peer_id.is_empty())
        .ok_or_else(|| "Session worker requires a peer ID".to_string())?;
    let mut socket = connector
        .connect(&address)
        .map_err(|error| error.to_string())?;
    let writer_socket = socket.try_clone().ok_or_else(|| {
        "Session worker control transport cannot split read and write halves".to_string()
    })?;
    let line = encode_control_line(&serde_json::json!({
        "type": "register_peer",
        "protocol": COORDINATOR_PROTOCOL_VERSION,
        "peerId": peer_id,
    }))
    .map_err(|error| error.to_string())?;
    socket
        .write_line(&line)
        .map_err(|error| error.to_string())?;

    let (tx, rx) = std::sync::mpsc::channel::<Result<String, WorkerReadError>>();
    let closed = Arc::new(AtomicBool::new(false));
    {
        let closed = Arc::clone(&closed);
        std::thread::Builder::new()
            .name("session-worker-control-reader".to_owned())
            .spawn(move || {
                let mut socket = socket;
                loop {
                    match read_line_capped(
                        socket.as_mut(),
                        "Session worker control message is too large",
                    ) {
                        Ok(Some(line)) => {
                            if tx.send(Ok(line)).is_err() {
                                return;
                            }
                        }
                        Ok(None) => {
                            let _ = tx.send(Err(WorkerReadError::Eof));
                            return;
                        }
                        Err(error) => {
                            let _ = tx.send(Err(WorkerReadError::Transport(error.to_string())));
                            return;
                        }
                    }
                    if closed.load(Ordering::SeqCst) {
                        return;
                    }
                }
            })
            .expect("spawn worker control reader thread");
    }
    let control = WorkerControl {
        initial_server_connection_id: None,
        writer: Arc::new(Mutex::new(writer_socket)),
        messages: Mutex::new(rx),
        closed,
    };
    // Await `peer_registered`.
    let reply = control
        .next_message()
        .map_err(|_| "Coordinator rejected the session worker registration".to_string())?;
    let parsed: CoordinatorInput = serde_json::from_str(&reply)
        .map_err(|_| "Coordinator rejected the session worker registration".to_string())?;
    if !matches!(parsed, CoordinatorInput::PeerRegistered { .. }) {
        return Err("Coordinator rejected the session worker registration".to_string());
    }
    let CoordinatorInput::PeerRegistered {
        server_connection_id,
        ..
    } = parsed
    else {
        unreachable!("matched above");
    };
    let initial = control_initial_connection(server_connection_id);
    Ok((
        WorkerControl {
            initial_server_connection_id: initial,
            ..control
        },
        token,
        decode_session_key(&encoded_session_key)?,
    ))
}

fn control_initial_connection(server_connection_id: Option<String>) -> Option<String> {
    server_connection_id.filter(|id| !id.is_empty())
}

/// `Buffer.from(key, "base64url").toString()` equivalent. Disclosure: Node's
/// base64url decoder skips invalid characters and its UTF-8 view replaces
/// malformed sequences, while this decoder rejects them; the key is always
/// produced by the manager's own base64url encoder, so in-contract bytes
/// agree (oracle: `tests/fixtures/experimental_d1236_oracle`, `sessionKey`).
fn decode_session_key(encoded: &str) -> Result<String, String> {
    const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out: Vec<u8> = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for ch in encoded.chars() {
        if ch == '=' {
            break;
        }
        let Some(value) = ALPHABET.find(ch).map(|index| index as u32) else {
            return Err("Session worker session key is not valid base64url".to_string());
        };
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    String::from_utf8(out).map_err(|_| "Session worker session key is not valid UTF-8".to_string())
}

/// Upstream `readCommands` handler surface.
pub struct SessionWorkerCommandHandlers {
    pub on_shutdown: Box<dyn Fn() + Send + Sync>,
    pub on_discovery: Box<dyn Fn() + Send + Sync>,
    pub on_demand: Arc<dyn Fn(SessionWorkerCommand) -> BoxFuture<'static, ()> + Send + Sync>,
    pub on_operation: Box<dyn Fn(WorkerOperationRequest) + Send + Sync>,
    pub on_operation_cancel: Box<dyn Fn(SessionWorkerCommand) + Send + Sync>,
    pub on_server_connected: Box<dyn Fn(String) + Send + Sync>,
    pub on_server_disconnected: Box<dyn Fn(String) + Send + Sync>,
}

/// Outcome of dispatching one inbound frame (upstream: `continue`, handler
/// call, or socket destroy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    Handled,
    Skipped,
    Destroyed,
}

/// Upstream `readCommands` body for one message: validate the
/// coordinator-input frame, then route server-connected/disconnected frames
/// and `message` payloads carrying worker commands.
pub fn dispatch_coordinator_message(
    line: &str,
    handlers: &SessionWorkerCommandHandlers,
) -> DispatchOutcome {
    let Ok(message) = serde_json::from_str::<CoordinatorInput>(line) else {
        return DispatchOutcome::Destroyed;
    };
    dispatch_coordinator_input(&message, handlers)
}

/// [`dispatch_coordinator_message`] core over a typed frame.
pub fn dispatch_coordinator_input(
    message: &CoordinatorInput,
    handlers: &SessionWorkerCommandHandlers,
) -> DispatchOutcome {
    if validate_coordinator_input(message).is_err() {
        return DispatchOutcome::Destroyed;
    }
    match message {
        CoordinatorInput::ServerConnected {
            server_connection_id,
        } => {
            (handlers.on_server_connected)(server_connection_id.clone());
            DispatchOutcome::Handled
        }
        CoordinatorInput::ServerDisconnected {
            server_connection_id,
        } => {
            (handlers.on_server_disconnected)(server_connection_id.clone());
            DispatchOutcome::Handled
        }
        CoordinatorInput::Message { payload, .. } => {
            let Ok(command) = serde_json::from_value::<SessionWorkerCommand>(payload.clone())
            else {
                return DispatchOutcome::Skipped;
            };
            dispatch_command(command, handlers)
        }
        CoordinatorInput::PeerRegistered { .. } => DispatchOutcome::Skipped,
    }
}

fn dispatch_command(
    command: SessionWorkerCommand,
    handlers: &SessionWorkerCommandHandlers,
) -> DispatchOutcome {
    match command {
        SessionWorkerCommand::Shutdown => {
            (handlers.on_shutdown)();
        }
        SessionWorkerCommand::DiscoverWorkers => {
            (handlers.on_discovery)();
        }
        SessionWorkerCommand::SessionDemand { .. } => {
            let handler = Arc::clone(&handlers.on_demand);
            tokio::spawn(handler(command));
            return DispatchOutcome::Handled;
        }
        SessionWorkerCommand::OperationCancel { .. } => {
            (handlers.on_operation_cancel)(command);
        }
        SessionWorkerCommand::Operation {
            request_id,
            scope,
            call,
        } => {
            (handlers.on_operation)(WorkerOperationRequest {
                message_type: WorkerOperationRequestType::Operation,
                request_id,
                scope,
                call,
            });
        }
    }
    DispatchOutcome::Handled
}

/// Upstream `readCommands`/`createJsonLineMessages` per-line destroy decision
/// with the exact error text: malformed JSON destroys with `"Session worker
/// received invalid control JSON"` (the messages iterable), a well-formed
/// JSON value that fails the `CoordinatorInputSchema` check destroys with
/// `"Coordinator sent an invalid worker message"` (the `readCommands` guard,
/// including the `from !== "server"` literal), and a valid frame returns
/// `None`. Oracle: `tests/fixtures/experimental_d1236_oracle`
/// (`readCommandsErrors`).
pub fn coordinator_line_destroy_error(line: &str) -> Option<String> {
    let parsed: Result<CoordinatorInput, _> = serde_json::from_str(line);
    match parsed {
        Ok(message) => {
            if validate_coordinator_input(&message).is_err() {
                Some("Coordinator sent an invalid worker message".to_string())
            } else {
                None
            }
        }
        Err(_) if serde_json::from_str::<Value>(line).is_ok() => {
            Some("Coordinator sent an invalid worker message".to_string())
        }
        Err(_) => Some("Session worker received invalid control JSON".to_string()),
    }
}

/// Terminal states of [`run_command_loop`]. Upstream `readCommands` never
/// resolves while the process lives: every terminal path funnels into
/// `closeAndExit` (cleanup then `process.exit`), so the loop's only job is to
/// report why the control stream ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerLoopStop {
    /// Socket EOF or transport failure (upstream: the `close`/`error` socket
    /// events reach `closeAndExit`).
    Exit,
    /// A frame destroyed the connection (upstream: `socket.destroy(new
    /// Error(...))` reaches `closeAndExit` through the `error` event /
    /// promise rejection). Carries the exact upstream destroy error from
    /// [`coordinator_line_destroy_error`].
    InvalidFrame(String),
}

/// Upstream `readCommands` loop: pull raw control lines from the connected
/// worker control socket and route them through the handlers until the
/// stream ends or a frame destroys the socket. Blocking by design — upstream
/// runs this detached (`void readCommands(...).catch(() => closeAndExit())`),
/// so embedders park a dedicated thread (or `spawn_blocking`) on it.
pub fn run_command_loop(
    control: &WorkerControl,
    handlers: &SessionWorkerCommandHandlers,
) -> WorkerLoopStop {
    loop {
        let line = match control.next_message() {
            Ok(line) => line,
            Err(_) => return WorkerLoopStop::Exit,
        };
        if let Some(error) = coordinator_line_destroy_error(&line) {
            control.destroy();
            return WorkerLoopStop::InvalidFrame(error);
        }
        if dispatch_coordinator_message(&line, handlers) == DispatchOutcome::Destroyed {
            control.destroy();
            return WorkerLoopStop::InvalidFrame(
                "Coordinator sent an invalid worker message".to_string(),
            );
        }
    }
}

/// Upstream `worker_ready` announce frame.
pub fn worker_ready_event(
    token: &str,
    session_key: &str,
    session_id: &str,
    pid: u64,
    metadata: &SessionWorkerMetadata,
    plugin_manifest_paths: &[String],
) -> SessionWorkerEvent {
    SessionWorkerEvent::WorkerReady {
        token: token.to_owned(),
        session_key: session_key.to_owned(),
        session_id: session_id.to_owned(),
        pid,
        metadata: metadata.clone(),
        plugin_manifest_paths: plugin_manifest_paths.to_vec(),
    }
}

/// Upstream `runSessionWorkerWithHarness`'s failure report frame.
pub fn worker_failed_event(token: &str, session_key: &str, message: &str) -> SessionWorkerEvent {
    SessionWorkerEvent::WorkerFailed {
        token: token.to_owned(),
        session_key: session_key.to_owned(),
        message: message.to_owned(),
    }
}

/// Upstream `runSessionWorkerWithHarness` argument/options validation with
/// the exact error strings (`args.length !== 1`, JSON parse,
/// strict schema + absolute-path + provider/model pairing).
pub fn parse_worker_options_args(args: &[String]) -> Result<SessionWorkerOptions, String> {
    if args.len() != 1 {
        return Err("Session worker requires one options argument".to_string());
    }
    let options: SessionWorkerOptions = serde_json::from_str(&args[0])
        .map_err(|_| "Session worker received invalid options".to_string())?;
    if options.session_dir.is_empty()
        || options.metadata.id.is_empty()
        || options
            .plugin_manifest_paths
            .iter()
            .any(|path| path.is_empty())
        || options
            .provider
            .as_ref()
            .is_some_and(|provider| provider.is_empty())
        || !is_absolute_path(&options.session_dir)
        || !is_absolute_path(&options.metadata.cwd)
        || !is_absolute_path(&options.metadata.path)
        || (options.provider.is_some() && options.model.is_none())
    {
        return Err("Session worker received invalid options".to_string());
    }
    Ok(options)
}

/// Node `path.isAbsolute` (posix face; the upstream worker contract uses
/// POSIX-absolute session paths).
fn is_absolute_path(path: &str) -> bool {
    path.starts_with('/')
}

/// Upstream `closeResources` error aggregation: one error re-raised, several
/// wrapped as `AggregateError(errors, message)` — the port formats the
/// aggregate message the same way.
pub fn aggregate_close_errors(mut errors: Vec<String>, message: &str) -> Result<(), String> {
    match errors.len() {
        0 => Ok(()),
        1 => Err(errors.remove(0)),
        _ => Err(format!("{message}: {}", errors.join("\n"))),
    }
}

// ── D3 command handlers (upstream `run(...)` wiring bodies) ────────────────

/// Event sink seam (upstream's `control.send` with token/session-key bound).
pub type EventSink =
    dyn Fn(SessionWorkerEvent) -> BoxFuture<'static, Result<(), String>> + Send + Sync;

/// Upstream `onDemand` handler body: hold retirement, drop subscriptions of
/// the detaching attachment, apply the demand, answer `demand_applied` /
/// `demand_rejected`, release retirement. `token`/`session_key` are the
/// announcement identity upstream binds from the control env (`run`).
pub async fn handle_demand(
    command: SessionWorkerCommand,
    lifecycle: &WorkerLifecycle,
    remove_subscriptions: &dyn Fn(&WorkerOperationScope),
    token: &str,
    session_key: &str,
    send: &EventSink,
) -> Result<(), String> {
    let SessionWorkerCommand::SessionDemand {
        server_connection_id,
        request_id,
        attachment_id,
        attached,
    } = command
    else {
        return Err("handle_demand received a non-demand command".to_string());
    };
    let release = lifecycle.hold_retirement();
    let outcome = {
        if !attached {
            remove_subscriptions(&WorkerOperationScope {
                server_connection_id: server_connection_id.clone(),
                attachment_id: attachment_id.clone(),
            });
        }
        lifecycle.set_demand(&server_connection_id, &attachment_id, attached)
    };
    let send_result = match outcome {
        Ok(()) => {
            send(SessionWorkerEvent::DemandApplied {
                token: token.to_owned(),
                session_key: session_key.to_owned(),
                request_id,
                attachment_id,
                attached,
            })
            .await
        }
        Err(message) => {
            send(SessionWorkerEvent::DemandRejected {
                token: token.to_owned(),
                session_key: session_key.to_owned(),
                request_id,
                message,
            })
            .await
        }
    };
    drop(release);
    send_result
}

/// Upstream `handleOperation` body: validate against the live generation,
/// invoke the service, answer `operation_result` / `operation_error`, keep
/// the cancellation token registered for `operation_cancel` and server
/// disconnects. `token`/`session_key` are the announcement identity upstream
/// binds from the control env (`run`).
pub async fn handle_operation(
    request: WorkerOperationRequest,
    lifecycle: &WorkerLifecycle,
    services: &crate::coding_agent::experimental::services::SessionWorkerServices,
    active_requests: &Mutex<HashMap<String, ActiveRequest>>,
    token: &str,
    session_key: &str,
    send: &EventSink,
) -> Result<(), String> {
    let release = lifecycle.begin_request(
        &request.scope.server_connection_id,
        &request.scope.attachment_id,
    )?;
    let cancel = tokio_util::sync::CancellationToken::new();
    active_requests.lock().unwrap().insert(
        request.request_id.clone(),
        ActiveRequest {
            scope: request.scope.clone(),
            cancel: cancel.clone(),
        },
    );
    let outcome = services.invoke(request.call.clone(), &request.scope).await;
    let response = match outcome {
        Ok(result) => WorkerOperationResponse::OperationResult {
            request_id: request.request_id.clone(),
            scope: request.scope.clone(),
            result,
        },
        Err(message) => operation_error_response(&request, &message),
    };
    let send_result = send(SessionWorkerEvent::OperationResponse {
        token: token.to_owned(),
        session_key: session_key.to_owned(),
        response,
    })
    .await;
    // Upstream removes the entry only if it still holds this operation's
    // cancel closure; request ids are unique, so an unconditional removal of
    // the (still-present) id is equivalent.
    active_requests.lock().unwrap().remove(&request.request_id);
    drop(release);
    send_result
}

/// Upstream's `activeRequests` entry (`{ scope, cancel }`).
pub struct ActiveRequest {
    pub scope: WorkerOperationScope,
    pub cancel: tokio_util::sync::CancellationToken,
}

/// Upstream error-message-to-response mapping: a message shaped like the
/// port's `WorkerOperationFailure` display carries its remote-service code
/// through (`RemoteServiceError.code` upstream).
fn operation_error_response(
    request: &WorkerOperationRequest,
    message: &str,
) -> WorkerOperationResponse {
    let code = REMOTE_SERVICE_ERROR_CODES.iter().find_map(|candidate| {
        let prefix = format!("Session worker service error {candidate}: ");
        message
            .strip_prefix(&prefix)
            .map(|rest| (candidate.to_string(), rest.to_string()))
    });
    match code {
        Some((code, inner_message)) => WorkerOperationResponse::OperationError {
            request_id: request.request_id.clone(),
            scope: request.scope.clone(),
            code: Some(code),
            message: inner_message,
        },
        None => WorkerOperationResponse::OperationError {
            request_id: request.request_id.clone(),
            scope: request.scope.clone(),
            code: None,
            message: message.to_string(),
        },
    }
}

/// Upstream `onOperationCancel`: cancel only when the id is active and the
/// scope matches.
pub fn handle_operation_cancel(
    command: &SessionWorkerCommand,
    active_requests: &Mutex<HashMap<String, ActiveRequest>>,
) {
    let SessionWorkerCommand::OperationCancel {
        request_id, scope, ..
    } = command
    else {
        return;
    };
    if let Some(entry) = active_requests.lock().unwrap().get(request_id) {
        if same_scope(&entry.scope, scope) {
            entry.cancel.cancel();
        }
    }
}

/// Upstream `onServerDisconnected`: drop the generation's subscriptions,
/// cancel its in-flight operations, then reconcile the lifecycle.
pub fn handle_server_disconnected(
    server_connection_id: &str,
    services: &crate::coding_agent::experimental::services::SessionWorkerServices,
    active_requests: &Mutex<HashMap<String, ActiveRequest>>,
    lifecycle: &WorkerLifecycle,
) {
    let matches = |scope: &WorkerOperationScope| scope.server_connection_id == server_connection_id;
    services.remove_subscriptions(matches);
    for entry in active_requests.lock().unwrap().values() {
        if matches(&entry.scope) {
            entry.cancel.cancel();
        }
    }
    lifecycle.server_disconnected(server_connection_id);
}

#[cfg(test)]
mod tests;
