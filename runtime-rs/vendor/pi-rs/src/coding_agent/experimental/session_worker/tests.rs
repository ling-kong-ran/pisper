//! Tests for `session_worker.rs`: full port of upstream
//! `test/experimental-session-worker-lifecycle.test.ts`
//! (sha256 04fad274bd8d1df8dc5fe1b9460a93b2144d3027bbd5ea8b6f3921a8d7ca4de2,
//! vitest fake timers -> tokio paused clock), plus protocol-frame byte
//! oracles captured from upstream JSON.stringify.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const GENERATION: &str = "generation-1";

struct LifecycleFixture {
    lifecycle: WorkerLifecycle,
    retire_count: Arc<AtomicUsize>,
}

fn create_lifecycle() -> LifecycleFixture {
    create_lifecycle_with(100, 200)
}

fn create_lifecycle_with(
    initial_demand_grace_ms: u64,
    orphan_demand_grace_ms: u64,
) -> LifecycleFixture {
    let retire_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&retire_count);
    let lifecycle = WorkerLifecycle::new(
        Some(GENERATION.to_string()),
        initial_demand_grace_ms,
        orphan_demand_grace_ms,
        Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
    );
    LifecycleFixture {
        lifecycle,
        retire_count,
    }
}

fn retire_count(fixture: &LifecycleFixture) -> usize {
    fixture.retire_count.load(Ordering::SeqCst)
}

async fn run_all_ticks() {
    // Upstream `await vi.runAllTicks()`: let queued timer tasks finish their
    // post-fire work (retire callbacks).
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// Advance the paused tokio clock by parking the main task on a virtual
/// sleep: this runtime auto-advances paused time and drives spawned timer
/// tasks while the test body is parked (equivalent to
/// `vi.advanceTimersByTime`).
async fn advance_timers(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    run_all_ticks().await;
}

// Upstream: "retires only after client demand and Harness activity are both gone"
#[tokio::test(start_paused = true)]
async fn retires_only_after_demand_and_activity_gone() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    fixture
        .lifecycle
        .operation_started(LifecycleOperationKind::Run, "main", "operation-1");
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", false)
        .unwrap();
    assert_eq!(retire_count(&fixture), 0);

    fixture
        .lifecycle
        .operation_stopped(LifecycleOperationKind::Run, "main", "operation-1");
    run_all_ticks().await;
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream: "retains the worker until every presentation attachment is released"
#[tokio::test(start_paused = true)]
async fn retains_until_every_attachment_released() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-2", true)
        .unwrap();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", false)
        .unwrap();
    assert_eq!(retire_count(&fixture), 0);
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-2", false)
        .unwrap();
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream: "tracks a nested compaction independently from its enclosing run"
#[tokio::test(start_paused = true)]
async fn tracks_nested_compaction_independently() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    fixture
        .lifecycle
        .operation_started(LifecycleOperationKind::Run, "main", "operation-1");
    fixture
        .lifecycle
        .operation_started(LifecycleOperationKind::Compaction, "main", "operation-1");
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", false)
        .unwrap();

    fixture
        .lifecycle
        .operation_stopped(LifecycleOperationKind::Compaction, "main", "operation-1");
    assert_eq!(retire_count(&fixture), 0);
    fixture
        .lifecycle
        .operation_stopped(LifecycleOperationKind::Run, "main", "operation-1");
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream test.each(["compaction", "navigation"]): "clears suspended %s activity"
#[tokio::test(start_paused = true)]
async fn clears_suspended_compaction_activity() {
    clear_suspended_activity(LifecycleOperationKind::Compaction).await;
}

#[tokio::test(start_paused = true)]
async fn clears_suspended_navigation_activity() {
    clear_suspended_activity(LifecycleOperationKind::Navigation).await;
}

async fn clear_suspended_activity(kind: LifecycleOperationKind) {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    fixture
        .lifecycle
        .operation_started(kind, "main", "operation-1");
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", false)
        .unwrap();

    fixture
        .lifecycle
        .operation_stopped(kind, "main", "operation-1");
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream: "does not retire while a demand acknowledgement holds reconciliation"
#[tokio::test(start_paused = true)]
async fn holds_while_retirement_guard_active() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    let guard = fixture.lifecycle.hold_retirement();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", false)
        .unwrap();
    assert_eq!(retire_count(&fixture), 0);
    guard.release();
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream: "holds retirement only for requests from the active attachment"
#[tokio::test(start_paused = true)]
async fn holds_retirement_only_for_active_attachment_requests() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    let guard = fixture
        .lifecycle
        .begin_request(GENERATION, "attachment-1")
        .unwrap();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", false)
        .unwrap();
    assert_eq!(retire_count(&fixture), 0);
    guard.release();
    assert_eq!(retire_count(&fixture), 1);
    let error = fixture
        .lifecycle
        .begin_request(GENERATION, "attachment-1")
        .unwrap_err();
    assert!(error.contains("retiring"), "unexpected error: {error}");
    fixture.lifecycle.close();
}

// Upstream: "rejects requests from stale generations and attachments"
#[tokio::test(start_paused = true)]
async fn rejects_stale_generation_and_attachment_requests() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    let error = fixture
        .lifecycle
        .begin_request("stale", "attachment-1")
        .unwrap_err();
    assert!(
        error.contains("stale server generation"),
        "unexpected: {error}"
    );
    let error = fixture
        .lifecycle
        .begin_request(GENERATION, "wrong-attachment")
        .unwrap_err();
    assert!(error.contains("active attachment"), "unexpected: {error}");
    fixture.lifecycle.close();
}

// Upstream: "retains disconnected-generation demand for the orphan grace"
#[tokio::test(start_paused = true)]
async fn retains_disconnected_demand_for_orphan_grace() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    fixture.lifecycle.server_disconnected(GENERATION);

    advance_timers(199).await;
    assert_eq!(retire_count(&fixture), 0);
    advance_timers(1).await;
    run_all_ticks().await;
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream: "allows a replacement generation to retain the worker"
#[tokio::test(start_paused = true)]
async fn replacement_generation_retains_worker() {
    let fixture = create_lifecycle();
    fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap();
    fixture.lifecycle.server_disconnected(GENERATION);
    fixture.lifecycle.server_connected("generation-2");
    fixture
        .lifecycle
        .set_demand("generation-2", "attachment-2", true)
        .unwrap();

    advance_timers(200).await;
    assert_eq!(retire_count(&fixture), 0);
    fixture
        .lifecycle
        .set_demand("generation-2", "attachment-2", false)
        .unwrap();
    run_all_ticks().await;
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream: "retires a launched worker that never receives initial demand"
#[tokio::test(start_paused = true)]
async fn retires_worker_without_initial_demand() {
    let fixture = create_lifecycle();
    advance_timers(100).await;
    run_all_ticks().await;
    assert_eq!(retire_count(&fixture), 1);
    fixture.lifecycle.close();
}

// Upstream: "rejects demand after retirement has won the race"
#[tokio::test(start_paused = true)]
async fn rejects_demand_after_retirement() {
    let fixture = create_lifecycle();
    advance_timers(100).await;
    let error = fixture
        .lifecycle
        .set_demand(GENERATION, "attachment-1", true)
        .unwrap_err();
    assert!(error.contains("retiring"), "unexpected: {error}");
    fixture.lifecycle.close();
}

// Upstream: "rejects demand from a stale server generation"
#[tokio::test(start_paused = true)]
async fn rejects_demand_from_stale_generation() {
    let fixture = create_lifecycle();
    let error = fixture
        .lifecycle
        .set_demand("stale", "attachment-1", true)
        .unwrap_err();
    assert!(
        error.contains("stale server generation"),
        "unexpected: {error}"
    );
    fixture.lifecycle.close();
}

fn lifecycle_delay_lookup(value: Option<&str>) -> impl Fn(&str) -> Option<String> {
    let owned = value.map(str::to_string);
    move |_name| owned.clone()
}

#[test]
fn lifecycle_delay_matches_node_oracle() {
    // Oracle: lifecycleDelay.
    assert_eq!(
        lifecycle_delay(lifecycle_delay_lookup(None), "ENV", 42).unwrap(),
        42
    );
    assert_eq!(
        lifecycle_delay(lifecycle_delay_lookup(Some("7")), "ENV", 42).unwrap(),
        7
    );
    assert_eq!(
        lifecycle_delay(lifecycle_delay_lookup(Some("0")), "ENV", 42).unwrap(),
        0
    );
    // Oracle: lifecycleDelayError / lifecycleDelayError2.
    assert_eq!(
        lifecycle_delay(lifecycle_delay_lookup(Some("-1")), "__ENV_X", 42).unwrap_err(),
        "__ENV_X must be a non-negative safe integer"
    );
    assert_eq!(
        lifecycle_delay(lifecycle_delay_lookup(Some("1.5")), "__ENV_X", 42).unwrap_err(),
        "__ENV_X must be a non-negative safe integer"
    );
}

#[test]
fn coordinator_input_rejects_non_server_messages() {
    let input = CoordinatorInput::Message {
        from: "worker-x".to_string(),
        payload: serde_json::json!({}),
    };
    assert!(validate_coordinator_input(&input).is_err());
    let input = CoordinatorInput::Message {
        from: "server".to_string(),
        payload: serde_json::json!({ "type": "shutdown" }),
    };
    assert!(validate_coordinator_input(&input).is_ok());
}

#[test]
fn session_worker_frames_match_node_byte_oracle() {
    // Oracle: encodeControlLine[2] (worker_ready, serialized as its own
    // protocol frame rather than wrapped in a control line).
    let event = SessionWorkerEvent::WorkerReady {
        token: "worker-token".to_string(),
        session_key: "/tmp/session-1.jsonl".to_string(),
        session_id: "session-1".to_string(),
        pid: 123,
        metadata: SessionWorkerMetadata {
            id: "session-1".to_string(),
            created_at: 1,
            storage_version: 1,
            cwd: "/tmp".to_string(),
            path: "/tmp/session-1.jsonl".to_string(),
            modified_at: serde_json::Number::from(1),
            parent_session_id: Some("parent-9".to_string()),
        },
        plugin_manifest_paths: vec!["/tmp/plugin/chord-facets.json".to_string()],
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        "{\"type\":\"worker_ready\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"sessionId\":\"session-1\",\"pid\":123,\"metadata\":{\"id\":\"session-1\",\"createdAt\":1,\"storageVersion\":1,\"cwd\":\"/tmp\",\"path\":\"/tmp/session-1.jsonl\",\"modifiedAt\":1,\"parentSessionId\":\"parent-9\"},\"pluginManifestPaths\":[\"/tmp/plugin/chord-facets.json\"]}"
    );
    // Oracle: encodeControlLine[3] (operation_result response payload).
    let event = SessionWorkerEvent::OperationResponse {
        token: "worker-token".to_string(),
        session_key: "/tmp/session-1.jsonl".to_string(),
        response: WorkerOperationResponse::OperationResult {
            request_id: "req-2".to_string(),
            scope: WorkerOperationScope {
                server_connection_id: "server-generation-1".to_string(),
                attachment_id: "att-1".to_string(),
            },
            result: Some(serde_json::json!({ "accepted": true })),
        },
    };
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        "{\"type\":\"operation_response\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"response\":{\"type\":\"operation_result\",\"requestId\":\"req-2\",\"scope\":{\"serverConnectionId\":\"server-generation-1\",\"attachmentId\":\"att-1\"},\"result\":{\"accepted\":true}}}"
    );
    // Oracle: encodeControlLine[4] (operation request frame).
    let command = SessionWorkerCommand::Operation {
        request_id: "req-3".to_string(),
        scope: WorkerOperationScope {
            server_connection_id: "server-generation-1".to_string(),
            attachment_id: "att-2".to_string(),
        },
        call: ServiceCall {
            service_id: "test.session".to_string(),
            instance: Some(ServiceCallInstance {
                key: "k".to_string(),
                generation: 2,
            }),
            member: "run".to_string(),
            args: vec![
                serde_json::json!("Hello"),
                serde_json::json!(3),
                serde_json::json!(null),
                serde_json::json!({ "nested": true }),
            ],
        },
    };
    assert_eq!(
        serde_json::to_string(&command).unwrap(),
        "{\"type\":\"operation\",\"requestId\":\"req-3\",\"scope\":{\"serverConnectionId\":\"server-generation-1\",\"attachmentId\":\"att-2\"},\"call\":{\"serviceId\":\"test.session\",\"instance\":{\"key\":\"k\",\"generation\":2},\"member\":\"run\",\"args\":[\"Hello\",3,null,{\"nested\":true}]}}"
    );
    // Oracle: encodeControlLine[1] (session_demand command frame).
    let command = SessionWorkerCommand::SessionDemand {
        server_connection_id: "server-generation-1".to_string(),
        request_id: "req-1".to_string(),
        attachment_id: "att-1".to_string(),
        attached: true,
    };
    assert_eq!(
        serde_json::to_string(&command).unwrap(),
        "{\"type\":\"session_demand\",\"serverConnectionId\":\"server-generation-1\",\"requestId\":\"req-1\",\"attachmentId\":\"att-1\",\"attached\":true}"
    );
}

#[test]
fn demand_key_and_scope_helpers_match_upstream() {
    // Oracle: keys.
    assert_eq!(
        demand_key("server-generation-1", "attachment-1"),
        "server-generation-1\0attachment-1"
    );
    assert!(same_scope(
        &WorkerOperationScope {
            server_connection_id: "a".to_string(),
            attachment_id: "b".to_string(),
        },
        &WorkerOperationScope {
            server_connection_id: "a".to_string(),
            attachment_id: "b".to_string(),
        },
    ));
    assert!(!same_scope(
        &WorkerOperationScope {
            server_connection_id: "a".to_string(),
            attachment_id: "b".to_string(),
        },
        &WorkerOperationScope {
            server_connection_id: "a".to_string(),
            attachment_id: "c".to_string(),
        },
    ));
}

// ── D3: worker process main loop (`connectControl`/`readCommands`/`run`) ───
//
// Upstream authority: experimental/session-worker.ts
// (sha256 f2ec55d9f48eb8ddb39f40e8681ce8424298742e7a2893db8f7d12923262b043).
// Byte oracles: tests/fixtures/experimental_d1236_oracle/oracle_output.json
// (sha256 a820ff137137a3bb8f07c802f984ea1175741a64265c8a9b44cae3a328496188).

const ORACLE_REGISTER_PEER_LINE: &str =
    "{\"type\":\"register_peer\",\"protocol\":3,\"peerId\":\"worker-1\"}\n";
const ORACLE_SEND_WORKER_READY: &str = "{\"type\":\"send\",\"to\":\"server\",\"payload\":{\"type\":\"worker_ready\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"sessionId\":\"session-1\",\"pid\":123,\"metadata\":{\"id\":\"session-1\",\"createdAt\":1,\"storageVersion\":1,\"cwd\":\"/tmp\",\"path\":\"/tmp/session-1.jsonl\",\"modifiedAt\":1,\"parentSessionId\":\"parent-9\"},\"pluginManifestPaths\":[\"/tmp/plugin/chord-facets.json\"]}}\n";
const ORACLE_SEND_WORKER_FAILED: &str = "{\"type\":\"send\",\"to\":\"server\",\"payload\":{\"type\":\"worker_failed\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"message\":\"Session worker received invalid options\"}}\n";
const ORACLE_SEND_DEMAND_APPLIED: &str = "{\"type\":\"send\",\"to\":\"server\",\"payload\":{\"type\":\"demand_applied\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"requestId\":\"req-1\",\"attachmentId\":\"att-1\",\"attached\":true}}\n";
const TOKEN: &str = "worker-token";
const SESSION_KEY: &str = "/tmp/session-1.jsonl";

use crate::coding_agent::experimental::coordinator::transport::ControlListener;
use crate::coding_agent::experimental::coordinator::transport::{MemoryConnector, MemoryHub};

use crate::coding_agent::experimental::services::{FacetGeneration, FacetHostSeam, PluginLoader};

/// No-op facet-host stub for the seam-backed services fixture.
#[derive(Default)]
struct NoopHost;

impl FacetHostSeam for NoopHost {
    fn reload(&self, _facets: Vec<String>) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn dispose(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// No-op plugin-loader stub (one empty generation per load).
#[derive(Default)]
struct NoopLoader;

impl PluginLoader for NoopLoader {
    fn load(&self) -> BoxFuture<'static, Result<FacetGeneration, String>> {
        Box::pin(async {
            Ok(FacetGeneration {
                facets: Vec::new(),
                dispose: Box::new(|| Box::pin(async { Ok(()) })),
            })
        })
    }
}

fn ready_metadata() -> SessionWorkerMetadata {
    SessionWorkerMetadata {
        id: "session-1".to_string(),
        created_at: 1,
        storage_version: 1,
        cwd: "/tmp".to_string(),
        path: "/tmp/session-1.jsonl".to_string(),
        modified_at: serde_json::Number::from(1),
        parent_session_id: Some("parent-9".to_string()),
    }
}

fn ready_event() -> SessionWorkerEvent {
    worker_ready_event(
        TOKEN,
        SESSION_KEY,
        "session-1",
        123,
        &ready_metadata(),
        &["/tmp/plugin/chord-facets.json".to_string()],
    )
}

/// A minimal coordinator-side peer endpoint over the memory transport: binds
/// `pi-control`, and a background thread accepts the worker connection,
/// asserts the `register_peer` handshake line byte-for-byte, writes `reply`
/// and hands the raw socket to the test through the returned receiver.
fn spawn_handshake_coordinator(
    hub: &Arc<MemoryHub>,
    reply: &'static str,
) -> (
    crate::coding_agent::experimental::coordinator::transport::MemoryListener,
    std::sync::mpsc::Receiver<
        Box<dyn crate::coding_agent::experimental::coordinator::transport::ControlSocket>,
    >,
) {
    let listener = hub.bind("pi-control").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread_listener = listener.clone();
    std::thread::Builder::new()
        .name("fake-coordinator".to_owned())
        .spawn(move || {
            let mut socket = thread_listener.accept().expect("worker connected");
            let handshake = socket.read_line().expect("handshake read").expect("line");
            assert_eq!(handshake, ORACLE_REGISTER_PEER_LINE);
            socket.write_line(reply).expect("handshake reply");
            let _ = tx.send(socket);
        })
        .expect("spawn fake coordinator");
    (listener, rx)
}

fn worker_env(peer_id: &str) -> WorkerEnvLookup {
    let entries: Vec<(String, String)> = vec![
        (
            SESSION_WORKER_CONTROL_ADDRESS_ENV.to_string(),
            "pi-control".to_string(),
        ),
        (
            SESSION_WORKER_CONTROL_TOKEN_ENV.to_string(),
            TOKEN.to_string(),
        ),
        (
            SESSION_WORKER_SESSION_KEY_ENV.to_string(),
            "L3RtcC9zZXNzaW9uLTEuanNvbmw".to_string(),
        ),
        (SESSION_WORKER_PEER_ID_ENV.to_string(), peer_id.to_string()),
    ];
    Arc::new(move |key: &str| {
        entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    })
}

#[test]
fn connect_control_requires_the_env_contract() {
    let missing_everything: WorkerEnvLookup = Arc::new(|_key| None);
    assert_eq!(
        connect_control(
            missing_everything,
            &MemoryConnector::new(Arc::new(MemoryHub::new()))
        )
        .map(|_| ())
        .unwrap_err(),
        "Session worker requires a control address"
    );
    let no_peer_id: WorkerEnvLookup = Arc::new(|key| {
        if key == SESSION_WORKER_PEER_ID_ENV {
            None
        } else {
            Some("x".to_string())
        }
    });
    assert_eq!(
        connect_control(
            no_peer_id,
            &MemoryConnector::new(Arc::new(MemoryHub::new()))
        )
        .map(|_| ())
        .unwrap_err(),
        "Session worker requires a peer ID"
    );
    // An empty peer id is treated like a missing one (upstream `if (!peerId)`).
    let empty_peer_id = worker_env("");
    assert_eq!(
        connect_control(
            empty_peer_id,
            &MemoryConnector::new(Arc::new(MemoryHub::new()))
        )
        .map(|_| ())
        .unwrap_err(),
        "Session worker requires a peer ID"
    );
}

fn handshake_test(reply: &'static str) -> Result<(WorkerControl, String, String), String> {
    let hub = Arc::new(MemoryHub::new());
    let (_listener, _rx) = spawn_handshake_coordinator(&hub, reply);
    let (control, token, session_key) =
        connect_control(worker_env("worker-1"), &MemoryConnector::new(hub))?;
    Ok((control, token, session_key))
}

#[test]
fn connect_control_handshake_matches_node_byte_oracle() {
    let (control, token, session_key) = handshake_test(
        "{\"type\":\"peer_registered\",\"peerId\":\"worker-1\",\"serverConnectionId\":\"srv-1\"}\n",
    )
    .expect("handshake");
    assert_eq!(
        control.initial_server_connection_id,
        Some("srv-1".to_string())
    );
    assert_eq!(token, TOKEN);
    // Oracle: sessionKey (base64url decode of the env contract value).
    assert_eq!(session_key, SESSION_KEY);
    control.destroy();
}

#[test]
fn connect_control_without_a_server_has_no_initial_generation() {
    let (control, _token, _session_key) =
        handshake_test("{\"type\":\"peer_registered\",\"peerId\":\"worker-1\"}\n")
            .expect("handshake");
    assert_eq!(control.initial_server_connection_id, None);
    control.destroy();
}

#[test]
fn connect_control_rejects_non_peer_registered_replies() {
    // Upstream: the first frame must be a `peer_registered` CoordinatorInput
    // or the handshake throws.
    for reply in [
        "{\"type\":\"server_connected\",\"serverConnectionId\":\"srv-1\"}\n",
        "{\"type\":\"message\",\"from\":\"server\",\"payload\":{}}\n",
        "not json\n",
    ] {
        let error = handshake_test(reply).err().unwrap();
        assert_eq!(
            error,
            "Coordinator rejected the session worker registration"
        );
    }
}

#[test]
fn worker_send_event_wraps_frames_in_the_send_envelope() {
    let hub = Arc::new(MemoryHub::new());
    let (_listener, rx) = spawn_handshake_coordinator(
        &hub,
        "{\"type\":\"peer_registered\",\"peerId\":\"worker-1\",\"serverConnectionId\":\"srv-1\"}\n",
    );
    let (control, _token, _session_key) =
        connect_control(worker_env("worker-1"), &MemoryConnector::new(hub)).expect("handshake");
    control.send_event(&ready_event()).unwrap();
    control
        .send_event(&worker_failed_event(
            TOKEN,
            SESSION_KEY,
            "Session worker received invalid options",
        ))
        .unwrap();
    control
        .send_event(&SessionWorkerEvent::DemandApplied {
            token: TOKEN.to_string(),
            session_key: SESSION_KEY.to_string(),
            request_id: "req-1".to_string(),
            attachment_id: "att-1".to_string(),
            attached: true,
        })
        .unwrap();
    control.destroy();
    let mut socket = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    for expected in [
        ORACLE_SEND_WORKER_READY,
        ORACLE_SEND_WORKER_FAILED,
        ORACLE_SEND_DEMAND_APPLIED,
    ] {
        let line = socket.read_line().expect("read").expect("line");
        assert_eq!(line, expected);
    }
}

#[test]
fn worker_event_helpers_match_node_byte_oracle() {
    // The inner payloads of the send-envelope oracles above.
    assert_eq!(
        serde_json::to_string(&ready_event()).unwrap(),
        "{\"type\":\"worker_ready\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"sessionId\":\"session-1\",\"pid\":123,\"metadata\":{\"id\":\"session-1\",\"createdAt\":1,\"storageVersion\":1,\"cwd\":\"/tmp\",\"path\":\"/tmp/session-1.jsonl\",\"modifiedAt\":1,\"parentSessionId\":\"parent-9\"},\"pluginManifestPaths\":[\"/tmp/plugin/chord-facets.json\"]}"
    );
    assert_eq!(
        serde_json::to_string(&worker_failed_event(
            TOKEN,
            SESSION_KEY,
            "Session worker received invalid options"
        ))
        .unwrap(),
        "{\"type\":\"worker_failed\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"message\":\"Session worker received invalid options\"}"
    );
}

#[test]
fn decode_session_key_matches_node_base64url_oracle() {
    // Oracle: sessionKeyEncoded / sessionKey. Node's decoder is lenient about
    // malformed input (disclosed); the port is strict.
    let decoded = decode_session_key("L3RtcC9zZXNzaW9uLTEuanNvbmw").unwrap();
    assert_eq!(decoded, SESSION_KEY);
    assert_eq!(
        decode_session_key("ab!c").unwrap_err(),
        "Session worker session key is not valid base64url"
    );
    // Oracle: lenientDecodeInvalidChar ("vw" decodes to 0xbf, invalid UTF-8).
    assert_eq!(
        decode_session_key("vw").unwrap_err(),
        "Session worker session key is not valid UTF-8"
    );
}

#[test]
fn coordinator_line_destroy_error_matches_upstream_texts() {
    // Oracle: readCommandsErrors.
    assert_eq!(
        coordinator_line_destroy_error("not json"),
        Some("Session worker received invalid control JSON".to_string())
    );
    assert_eq!(
        coordinator_line_destroy_error(
            "{\"type\":\"message\",\"from\":\"worker-x\",\"payload\":{}}"
        ),
        Some("Coordinator sent an invalid worker message".to_string())
    );
    assert_eq!(
        coordinator_line_destroy_error("{\"nope\":1}"),
        Some("Coordinator sent an invalid worker message".to_string())
    );
    assert_eq!(
        coordinator_line_destroy_error(
            "{\"type\":\"message\",\"from\":\"server\",\"payload\":{\"type\":\"shutdown\"}}"
        ),
        None
    );
    assert_eq!(
        coordinator_line_destroy_error(
            "{\"type\":\"server_connected\",\"serverConnectionId\":\"srv-2\"}"
        ),
        None
    );
}

#[derive(Default)]
struct RecordedHandlers {
    events: Mutex<Vec<String>>,
    demands: Mutex<Vec<SessionWorkerCommand>>,
    operations: Mutex<Vec<WorkerOperationRequest>>,
    cancelled: Mutex<Vec<SessionWorkerCommand>>,
    servers: Mutex<Vec<String>>,
}

fn handlers_from(
    recorder: &Arc<RecordedHandlers>,
    control: Arc<WorkerControl>,
) -> SessionWorkerCommandHandlers {
    SessionWorkerCommandHandlers {
        on_shutdown: {
            let recorder = Arc::clone(recorder);
            let control = Arc::clone(&control);
            Box::new(move || {
                recorder.events.lock().unwrap().push("shutdown".to_string());
                // Upstream onShutdown = closeAndExit: the socket goes away and
                // the loop ends.
                control.destroy();
            })
        },
        on_discovery: {
            let recorder = Arc::clone(recorder);
            Box::new(move || {
                recorder
                    .events
                    .lock()
                    .unwrap()
                    .push("discovery".to_string())
            })
        },
        on_demand: {
            let recorder = Arc::clone(recorder);
            Arc::new(move |command| {
                let recorder = Arc::clone(&recorder);
                Box::pin(async move {
                    recorder.demands.lock().unwrap().push(command);
                })
            })
        },
        on_operation: {
            let recorder = Arc::clone(recorder);
            Box::new(move |request| recorder.operations.lock().unwrap().push(request))
        },
        on_operation_cancel: {
            let recorder = Arc::clone(recorder);
            Box::new(move |command| recorder.cancelled.lock().unwrap().push(command))
        },
        on_server_connected: {
            let recorder = Arc::clone(recorder);
            Box::new(move |id| {
                recorder
                    .servers
                    .lock()
                    .unwrap()
                    .push(format!("connected:{id}"))
            })
        },
        on_server_disconnected: {
            let recorder = Arc::clone(recorder);
            Box::new(move |id| {
                recorder
                    .servers
                    .lock()
                    .unwrap()
                    .push(format!("disconnected:{id}"))
            })
        },
    }
}

/// Multi-thread runtime flavor: `dispatch_command` spawns demand futures onto
/// the ambient runtime, so the blocking loop runs on the blocking pool
/// (upstream `readCommands` runs detached on the live event loop). The fake
/// coordinator hands its connection back after the handshake so the test can
/// keep framing the same socket.
async fn connected_pair() -> (
    Arc<WorkerControl>,
    crate::coding_agent::experimental::coordinator::transport::MemoryListener,
    Box<dyn crate::coding_agent::experimental::coordinator::transport::ControlSocket>,
) {
    let hub = Arc::new(MemoryHub::new());
    let (_listener, rx) = spawn_handshake_coordinator(
        &hub,
        "{\"type\":\"peer_registered\",\"peerId\":\"worker-1\",\"serverConnectionId\":\"srv-2\"}\n",
    );
    // Bounded block: the fake coordinator thread answers the handshake.
    let (control, _token, _session_key) =
        connect_control(worker_env("worker-1"), &MemoryConnector::new(hub)).expect("handshake");
    let socket = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("coordinator socket handoff");
    (Arc::new(control), _listener, socket)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_command_loop_routes_frames_until_the_stream_ends() {
    let (control, _listener, mut socket) = connected_pair().await;
    let recorder = Arc::new(RecordedHandlers::default());
    let handlers = handlers_from(&recorder, Arc::clone(&control));
    let loop_task = tokio::task::spawn_blocking({
        let control = Arc::clone(&control);
        move || run_command_loop(&control, &handlers)
    });

    socket
        .write_line("{\"type\":\"server_connected\",\"serverConnectionId\":\"srv-2\"}\n")
        .unwrap();
    socket
        .write_line("{\"type\":\"message\",\"from\":\"server\",\"payload\":{\"type\":\"discover_workers\"}}\n")
        .unwrap();
    // A non-command payload is skipped (upstream `continue`), as is a frame
    // that is not a routed message at all.
    socket
        .write_line(
            "{\"type\":\"message\",\"from\":\"server\",\"payload\":{\"type\":\"nonsense\"}}\n",
        )
        .unwrap();
    socket
        .write_line("{\"type\":\"peer_registered\",\"peerId\":\"worker-1\"}\n")
        .unwrap();
    socket
        .write_line("{\"type\":\"message\",\"from\":\"server\",\"payload\":{\"type\":\"session_demand\",\"serverConnectionId\":\"srv-2\",\"requestId\":\"req-1\",\"attachmentId\":\"att-1\",\"attached\":true}}\n")
        .unwrap();
    socket
        .write_line("{\"type\":\"message\",\"from\":\"server\",\"payload\":{\"type\":\"operation_cancel\",\"requestId\":\"req-1\",\"scope\":{\"serverConnectionId\":\"srv-2\",\"attachmentId\":\"att-1\"}}}\n")
        .unwrap();
    socket
        .write_line("{\"type\":\"server_disconnected\",\"serverConnectionId\":\"srv-2\"}\n")
        .unwrap();
    socket
        .write_line(
            "{\"type\":\"message\",\"from\":\"server\",\"payload\":{\"type\":\"shutdown\"}}\n",
        )
        .unwrap();
    // Upstream onShutdown = closeAndExit: the socket goes away, the loop sees
    // EOF and returns.
    let stop = loop_task.await.expect("loop join");
    assert_eq!(stop, WorkerLoopStop::Exit);
    // Let the spawned demand future take its tick before asserting.
    tokio::task::yield_now().await;
    assert_eq!(
        *recorder.servers.lock().unwrap(),
        vec![
            "connected:srv-2".to_string(),
            "disconnected:srv-2".to_string()
        ]
    );
    assert_eq!(
        *recorder.events.lock().unwrap(),
        vec!["discovery".to_string(), "shutdown".to_string()]
    );
    assert_eq!(recorder.demands.lock().unwrap().len(), 1);
    assert_eq!(recorder.cancelled.lock().unwrap().len(), 1);
    assert!(recorder.operations.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_command_loop_destroys_on_invalid_frames_with_upstream_error() {
    // Oracle: readCommandsErrors.invalidJson.
    {
        let (control, _listener, mut socket) = connected_pair().await;
        let recorder = Arc::new(RecordedHandlers::default());
        let handlers = handlers_from(&recorder, Arc::clone(&control));
        let loop_task = tokio::task::spawn_blocking({
            let control = Arc::clone(&control);
            move || run_command_loop(&control, &handlers)
        });
        socket.write_line("}}garbage{{\n").unwrap();
        let stop = loop_task.await.expect("loop join");
        assert_eq!(
            stop,
            WorkerLoopStop::InvalidFrame(
                "Session worker received invalid control JSON".to_string()
            )
        );
        assert!(recorder.events.lock().unwrap().is_empty());
    }
    // A non-server `from` fails the schema check instead.
    // Oracle: readCommandsErrors.invalidWorkerMessage.
    {
        let (control, _listener, mut socket) = connected_pair().await;
        let recorder = Arc::new(RecordedHandlers::default());
        let handlers = handlers_from(&recorder, Arc::clone(&control));
        let loop_task = tokio::task::spawn_blocking({
            let control = Arc::clone(&control);
            move || run_command_loop(&control, &handlers)
        });
        socket
            .write_line("{\"type\":\"message\",\"from\":\"worker-9\",\"payload\":{}}\n")
            .unwrap();
        let stop = loop_task.await.expect("loop join");
        assert_eq!(
            stop,
            WorkerLoopStop::InvalidFrame("Coordinator sent an invalid worker message".to_string())
        );
    }
}

fn recording_sink() -> (Arc<Mutex<Vec<SessionWorkerEvent>>>, Arc<EventSink>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let sink: Arc<EventSink> = Arc::new(move |event| {
        let sink_events = Arc::clone(&sink_events);
        Box::pin(async move {
            sink_events.lock().unwrap().push(event);
            Ok(())
        })
    });
    (events, sink)
}

fn lifecycle_for_generation() -> WorkerLifecycle {
    WorkerLifecycle::new(Some("srv-1".to_string()), 60_000, 60_000, Box::new(|| {}))
}

#[tokio::test]
async fn handle_demand_answers_with_bound_token_and_session_key() {
    let lifecycle = lifecycle_for_generation();
    let removed = Arc::new(Mutex::new(Vec::new()));
    let removed_for_call = Arc::clone(&removed);
    let (events, sink) = recording_sink();

    let command = SessionWorkerCommand::SessionDemand {
        server_connection_id: "srv-1".to_string(),
        request_id: "req-1".to_string(),
        attachment_id: "att-1".to_string(),
        attached: true,
    };
    handle_demand(
        command,
        &lifecycle,
        &|_scope| {},
        TOKEN,
        SESSION_KEY,
        sink.as_ref(),
    )
    .await
    .unwrap();
    // Oracle: sendDemandApplied (the send-envelope payload).
    assert_eq!(
        serde_json::to_string(&events.lock().unwrap()[0]).unwrap(),
        "{\"type\":\"demand_applied\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"requestId\":\"req-1\",\"attachmentId\":\"att-1\",\"attached\":true}"
    );
    assert!(lifecycle.begin_request("srv-1", "att-1").is_ok());

    // A detaching demand drops the scope's subscriptions before applying.
    let detach = SessionWorkerCommand::SessionDemand {
        server_connection_id: "srv-1".to_string(),
        request_id: "req-2".to_string(),
        attachment_id: "att-1".to_string(),
        attached: false,
    };
    handle_demand(
        detach,
        &lifecycle,
        &|scope: &WorkerOperationScope| {
            removed_for_call.lock().unwrap().push((
                scope.server_connection_id.clone(),
                scope.attachment_id.clone(),
            ));
        },
        TOKEN,
        SESSION_KEY,
        sink.as_ref(),
    )
    .await
    .unwrap();
    assert_eq!(
        *removed.lock().unwrap(),
        vec![("srv-1".to_string(), "att-1".to_string())]
    );
    // The detach is answered with a `demand_applied` carrying `attached:false`.
    assert_eq!(
        serde_json::to_string(&events.lock().unwrap()[1]).unwrap(),
        "{\"type\":\"demand_applied\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"requestId\":\"req-2\",\"attachmentId\":\"att-1\",\"attached\":false}"
    );
}

#[tokio::test]
async fn handle_demand_rejects_stale_generations_with_lifecycle_text() {
    let lifecycle = lifecycle_for_generation();
    let (events, sink) = recording_sink();
    let command = SessionWorkerCommand::SessionDemand {
        server_connection_id: "stale".to_string(),
        request_id: "req-3".to_string(),
        attachment_id: "att-1".to_string(),
        attached: true,
    };
    handle_demand(
        command,
        &lifecycle,
        &|_scope| {},
        TOKEN,
        SESSION_KEY,
        sink.as_ref(),
    )
    .await
    .unwrap();
    let event = events.lock().unwrap()[0].clone();
    match &event {
        SessionWorkerEvent::DemandRejected { message, .. } => {
            assert_eq!(
                message,
                "Session worker received demand from a stale server generation"
            );
        }
        other => panic!("expected demand_rejected, got {other:?}"),
    }
}

fn demand_attached() -> WorkerLifecycle {
    let lifecycle =
        WorkerLifecycle::new(Some("srv-1".to_string()), 60_000, 60_000, Box::new(|| {}));
    lifecycle.set_demand("srv-1", "att-1", true).unwrap();
    lifecycle
}

fn operation_request(call: ServiceCall) -> WorkerOperationRequest {
    WorkerOperationRequest {
        message_type: WorkerOperationRequestType::Operation,
        request_id: "req-9".to_string(),
        scope: WorkerOperationScope {
            server_connection_id: "srv-1".to_string(),
            attachment_id: "att-1".to_string(),
        },
        call,
    }
}

/// Endpoint stub for the seam-backed [`SessionWorkerServices`]: its invoke
/// result is driven by a shared slot so tests can shape outcomes.
struct StubEndpoint {
    outcome: Mutex<Result<Option<Value>, String>>,
}

impl crate::coding_agent::experimental::services::ServiceEndpoint for StubEndpoint {
    fn invoke(
        self: Arc<Self>,
        _call: ServiceCall,
        _publish: Box<dyn FnOnce(String, Value) -> BoxFuture<'static, ()> + Send>,
    ) -> BoxFuture<'static, Result<Option<Value>, String>> {
        let outcome = self.outcome.lock().unwrap().clone();
        Box::pin(async move { outcome })
    }

    fn dispose(&self) {}
}

fn stub_services(
    outcome: Result<Option<Value>, String>,
) -> crate::coding_agent::experimental::services::SessionWorkerServices {
    crate::coding_agent::experimental::services::SessionWorkerServices::new(
        Box::new(move |_scope| {
            Arc::new(StubEndpoint {
                outcome: Mutex::new(outcome.clone()),
            }) as Arc<dyn crate::coding_agent::experimental::services::ServiceEndpoint>
        }),
        Arc::new(NoopHost),
        Arc::new(NoopLoader),
        FacetGeneration {
            facets: Vec::new(),
            dispose: Box::new(|| Box::pin(async { Ok(()) })),
        },
    )
}

#[tokio::test]
async fn handle_operation_reports_results_with_bound_identity() {
    let services = stub_services(Ok(Some(serde_json::json!({ "accepted": true }))));
    let lifecycle = demand_attached();
    let active: Mutex<HashMap<String, ActiveRequest>> = Mutex::new(HashMap::new());
    let (events, sink) = recording_sink();
    let request = operation_request(ServiceCall {
        service_id: "test.session".to_string(),
        instance: None,
        member: "run".to_string(),
        args: vec![serde_json::json!("Hello")],
    });
    handle_operation(
        request,
        &lifecycle,
        &services,
        &active,
        TOKEN,
        SESSION_KEY,
        sink.as_ref(),
    )
    .await
    .unwrap();
    // Oracle: encodeControlLine[3] payload with the bound token/sessionKey.
    assert_eq!(
        serde_json::to_string(&events.lock().unwrap()[0]).unwrap(),
        "{\"type\":\"operation_response\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"response\":{\"type\":\"operation_result\",\"requestId\":\"req-9\",\"scope\":{\"serverConnectionId\":\"srv-1\",\"attachmentId\":\"att-1\"},\"result\":{\"accepted\":true}}}"
    );
    // The request slot is released once the response is sent.
    assert!(active.lock().unwrap().is_empty());
}

#[tokio::test]
async fn handle_operation_maps_remote_service_error_codes() {
    let services = stub_services(Err(
        "Session worker service error service_not_found: boom".to_string()
    ));
    let lifecycle = demand_attached();
    let active: Mutex<HashMap<String, ActiveRequest>> = Mutex::new(HashMap::new());
    let (events, sink) = recording_sink();
    handle_operation(
        operation_request(ServiceCall {
            service_id: "test.session".to_string(),
            instance: None,
            member: "run".to_string(),
            args: Vec::new(),
        }),
        &lifecycle,
        &services,
        &active,
        TOKEN,
        SESSION_KEY,
        sink.as_ref(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_string(&events.lock().unwrap()[0]).unwrap(),
        "{\"type\":\"operation_response\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"response\":{\"type\":\"operation_error\",\"requestId\":\"req-9\",\"scope\":{\"serverConnectionId\":\"srv-1\",\"attachmentId\":\"att-1\"},\"code\":\"service_not_found\",\"message\":\"boom\"}}"
    );

    // A plain failure carries no code (upstream Error without `.code`).
    let services = stub_services(Err("kaboom".to_string()));
    let lifecycle = demand_attached();
    let (events, sink) = recording_sink();
    handle_operation(
        operation_request(ServiceCall {
            service_id: "test.session".to_string(),
            instance: None,
            member: "run".to_string(),
            args: Vec::new(),
        }),
        &lifecycle,
        &services,
        &Mutex::new(HashMap::new()),
        TOKEN,
        SESSION_KEY,
        sink.as_ref(),
    )
    .await
    .unwrap();
    let event = events.lock().unwrap()[0].clone();
    match &event {
        SessionWorkerEvent::OperationResponse { response, .. } => match response {
            WorkerOperationResponse::OperationError { code, message, .. } => {
                assert_eq!(*code, None);
                assert_eq!(message, "kaboom");
            }
            other => panic!("expected operation_error, got {other:?}"),
        },
        other => panic!("expected operation_response, got {other:?}"),
    }
}

#[tokio::test]
async fn handle_operation_validates_against_the_live_generation() {
    let services = stub_services(Ok(None));
    let lifecycle =
        WorkerLifecycle::new(Some("srv-1".to_string()), 60_000, 60_000, Box::new(|| {}));
    let (events, sink) = recording_sink();
    let error = handle_operation(
        operation_request(ServiceCall {
            service_id: "test.session".to_string(),
            instance: None,
            member: "run".to_string(),
            args: Vec::new(),
        }),
        &lifecycle,
        &services,
        &Mutex::new(HashMap::new()),
        TOKEN,
        SESSION_KEY,
        sink.as_ref(),
    )
    .await
    .unwrap_err();
    // beginRequest fails before any service call or response.
    assert_eq!(
        error,
        "Session worker request does not match the active attachment"
    );
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn handle_operation_cancel_cancels_only_matching_scopes() {
    let active: Mutex<HashMap<String, ActiveRequest>> = Mutex::new(HashMap::new());
    let cancel = tokio_util::sync::CancellationToken::new();
    active.lock().unwrap().insert(
        "req-1".to_string(),
        ActiveRequest {
            scope: WorkerOperationScope {
                server_connection_id: "srv-1".to_string(),
                attachment_id: "att-1".to_string(),
            },
            cancel: cancel.clone(),
        },
    );
    // Wrong attachment: not cancelled.
    handle_operation_cancel(
        &SessionWorkerCommand::OperationCancel {
            request_id: "req-1".to_string(),
            scope: WorkerOperationScope {
                server_connection_id: "srv-1".to_string(),
                attachment_id: "att-2".to_string(),
            },
        },
        &active,
    );
    assert!(!cancel.is_cancelled());
    handle_operation_cancel(
        &SessionWorkerCommand::OperationCancel {
            request_id: "req-1".to_string(),
            scope: WorkerOperationScope {
                server_connection_id: "srv-1".to_string(),
                attachment_id: "att-1".to_string(),
            },
        },
        &active,
    );
    assert!(cancel.is_cancelled());
    // Unknown request id: a no-op.
    handle_operation_cancel(
        &SessionWorkerCommand::OperationCancel {
            request_id: "req-x".to_string(),
            scope: WorkerOperationScope {
                server_connection_id: "srv-1".to_string(),
                attachment_id: "att-1".to_string(),
            },
        },
        &active,
    );
}

#[tokio::test(start_paused = true)]
async fn handle_server_disconnected_drops_generation_state() {
    struct CountingEndpoint {
        disposed: Arc<AtomicUsize>,
    }
    impl crate::coding_agent::experimental::services::ServiceEndpoint for CountingEndpoint {
        fn invoke(
            self: Arc<Self>,
            _call: ServiceCall,
            _publish: Box<dyn FnOnce(String, Value) -> BoxFuture<'static, ()> + Send>,
        ) -> BoxFuture<'static, Result<Option<Value>, String>> {
            Box::pin(async { Ok(None) })
        }
        fn dispose(&self) {
            self.disposed.fetch_add(1, Ordering::SeqCst);
        }
    }

    let disposed = Arc::new(AtomicUsize::new(0));
    let disposed_for_factory = Arc::clone(&disposed);
    let services = crate::coding_agent::experimental::services::SessionWorkerServices::new(
        Box::new(move |_scope| {
            Arc::new(CountingEndpoint {
                disposed: Arc::clone(&disposed_for_factory),
            }) as Arc<dyn crate::coding_agent::experimental::services::ServiceEndpoint>
        }),
        Arc::new(NoopHost),
        Arc::new(NoopLoader),
        FacetGeneration {
            facets: Vec::new(),
            dispose: Box::new(|| Box::pin(async { Ok(()) })),
        },
    );
    let lifecycle = Arc::new(WorkerLifecycle::new(
        Some("srv-1".to_string()),
        60_000,
        500,
        Box::new(|| {}),
    ));
    lifecycle.set_demand("srv-1", "att-1", true).unwrap();
    let _ = services
        .invoke(
            ServiceCall {
                service_id: "test.session".to_string(),
                instance: None,
                member: "run".to_string(),
                args: Vec::new(),
            },
            &WorkerOperationScope {
                server_connection_id: "srv-1".to_string(),
                attachment_id: "att-1".to_string(),
            },
        )
        .await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let active: Mutex<HashMap<String, ActiveRequest>> = Mutex::new(HashMap::new());
    active.lock().unwrap().insert(
        "req-1".to_string(),
        ActiveRequest {
            scope: WorkerOperationScope {
                server_connection_id: "srv-1".to_string(),
                attachment_id: "att-1".to_string(),
            },
            cancel: cancel.clone(),
        },
    );

    handle_server_disconnected("srv-1", &services, &active, &lifecycle);
    assert_eq!(disposed.load(Ordering::SeqCst), 1);
    assert!(cancel.is_cancelled());
    // The orphan grace then retires the worker (paused clock).
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(lifecycle.begin_request("srv-1", "att-1").is_err());
}

#[test]
fn parse_worker_options_args_matches_node_oracle() {
    // Oracle: optionsValidation.
    assert_eq!(
        parse_worker_options_args(&[]).unwrap_err(),
        "Session worker requires one options argument"
    );
    assert_eq!(
        parse_worker_options_args(&["a".to_string(), "b".to_string()]).unwrap_err(),
        "Session worker requires one options argument"
    );
    assert_eq!(
        parse_worker_options_args(&["{".to_string()]).unwrap_err(),
        "Session worker received invalid options"
    );
    let relative = serde_json::json!({
        "sessionDir": "rel",
        "metadata": { "cwd": "rel", "path": "rel" },
    })
    .to_string();
    assert_eq!(
        parse_worker_options_args(&[relative]).unwrap_err(),
        "Session worker received invalid options"
    );
    let provider_without_model = serde_json::json!({
        "sessionDir": "/tmp",
        "metadata": { "cwd": "/tmp", "path": "/x" },
        "provider": "anthropic",
    })
    .to_string();
    assert_eq!(
        parse_worker_options_args(&[provider_without_model]).unwrap_err(),
        "Session worker received invalid options"
    );
    // Oracle: optionsValidation.accepts (null error).
    let options = parse_worker_options_args(&[serde_json::json!({
        "sessionDir": "/tmp/sessions",
        "metadata": {
            "id": "session-1",
            "createdAt": 1,
            "storageVersion": 1,
            "cwd": "/tmp",
            "path": "/tmp/session-1.jsonl",
            "modifiedAt": 1,
        },
        "provider": "anthropic",
        "model": "claude-sonnet-4-5",
        "pluginManifestPaths": ["/tmp/plugin/chord-facets.json"],
    })
    .to_string()])
    .expect("valid options");
    assert_eq!(options.session_dir, "/tmp/sessions");
    assert_eq!(options.metadata.id, "session-1");
    assert_eq!(options.provider.as_deref(), Some("anthropic"));
    assert_eq!(options.model.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(
        options.plugin_manifest_paths,
        vec!["/tmp/plugin/chord-facets.json".to_string()]
    );
}

#[test]
fn aggregate_close_errors_matches_upstream_aggregate_shape() {
    // Oracle: aggregateMessages.cleanup — one error re-raised, several
    // aggregated under the upstream AggregateError message.
    assert_eq!(
        aggregate_close_errors(Vec::new(), "Session worker cleanup failed"),
        Ok(())
    );
    assert_eq!(
        aggregate_close_errors(vec!["first".to_string()], "Session worker cleanup failed"),
        Err("first".to_string())
    );
    assert_eq!(
        aggregate_close_errors(
            vec!["a".to_string(), "b".to_string()],
            "Session worker cleanup failed"
        ),
        Err("Session worker cleanup failed: a\nb".to_string())
    );
}
