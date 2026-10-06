//! Tests for `session_worker_manager.rs`: full port of upstream
//! `test/experimental-session-worker-manager.test.ts`
//! (sha256 ffe285dbbe20badd0fe33bc779318d5f26d6ab5cfc444109a7c5ff6bf62774c1,
//! FakeCoordinator/vitest fake timers -> CoordinatorLink seam + tokio paused
//! clock, `process.kill` spy -> the D5 `KillFn` seam).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use super::*;

const METADATA_PATH: &str = "/tmp/session-1.jsonl";

fn metadata() -> SessionWorkerMetadata {
    SessionWorkerMetadata {
        id: "session-1".to_string(),
        created_at: 1,
        storage_version: 1,
        cwd: "/tmp".to_string(),
        path: METADATA_PATH.to_string(),
        modified_at: serde_json::Number::from(1),
        parent_session_id: None,
    }
}

fn ready_payload() -> Value {
    json!({
        "type": "worker_ready",
        "token": "worker-token",
        "sessionKey": METADATA_PATH,
        "sessionId": "session-1",
        "pid": 123,
        "metadata": metadata(),
        "pluginManifestPaths": [],
    })
}

/// Reaction modes for the fake coordinator's `send` hook (mirrors the
/// upstream tests' `coordinator.onSend` assignments).
#[derive(Clone, Default)]
enum Reaction {
    /// `coordinator.onSend = () => {}` — swallow everything.
    #[default]
    Silent,
    /// Auto-ack every session_demand with demand_applied.
    AckDemand,
    /// Ack `attached: false` demands only (used by the compensation test).
    AckDetach,
    /// Echo every operation with an operation_result.
    EchoOperation {
        token: &'static str,
        null_scope: bool,
    },
}

type CoordinatorListeners = Mutex<Vec<Arc<dyn Fn(&CoordinatorConnectionEvent) + Send + Sync>>>;

struct FakeCoordinator {
    sent: Mutex<Vec<(String, Value)>>,
    reaction: Mutex<Reaction>,
    listeners: CoordinatorListeners,
}

impl FakeCoordinator {
    fn new() -> Arc<Self> {
        Arc::new(FakeCoordinator {
            sent: Mutex::new(Vec::new()),
            reaction: Mutex::new(Reaction::Silent),
            listeners: Mutex::new(Vec::new()),
        })
    }

    fn set_reaction(&self, reaction: Reaction) {
        *self.reaction.lock().unwrap() = reaction;
    }

    fn emit(&self, event: CoordinatorConnectionEvent) {
        for listener in self.listeners.lock().unwrap().iter() {
            listener(&event);
        }
    }

    fn payload_types(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|(_, payload)| {
                payload
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

impl CoordinatorLink for FakeCoordinator {
    fn control_path(&self) -> &str {
        "/tmp/control.sock"
    }

    fn server_connection_id(&self) -> &str {
        "server-generation-1"
    }

    fn send<'a>(&'a self, peer_id: &'a str, payload: Value) -> BoxFuture<'a, Result<(), String>> {
        self.sent
            .lock()
            .unwrap()
            .push((peer_id.to_string(), payload.clone()));
        let reaction = self.reaction.lock().unwrap().clone();
        match reaction {
            Reaction::Silent => {}
            Reaction::AckDemand | Reaction::AckDetach => {
                if payload.get("type").and_then(Value::as_str) == Some("session_demand") {
                    let attached = payload.get("attached").and_then(Value::as_bool);
                    let respond = match reaction {
                        Reaction::AckDemand => true,
                        _ => attached == Some(false),
                    };
                    if respond {
                        self.emit(CoordinatorConnectionEvent::Message {
                            from: peer_id.to_string(),
                            payload: json!({
                                "type": "demand_applied",
                                "token": "worker-token",
                                "sessionKey": METADATA_PATH,
                                "requestId": payload.get("requestId").cloned().unwrap_or(Value::Null),
                                "attachmentId": payload.get("attachmentId").cloned().unwrap_or(Value::Null),
                                "attached": attached.unwrap_or(false),
                            }),
                        });
                    }
                }
            }
            Reaction::EchoOperation { token, null_scope } => {
                if payload.get("type").and_then(Value::as_str) == Some("operation") {
                    let scope = if null_scope {
                        Value::Null
                    } else {
                        payload.get("scope").cloned().unwrap_or(Value::Null)
                    };
                    self.emit(CoordinatorConnectionEvent::Message {
                        from: peer_id.to_string(),
                        payload: json!({
                            "type": "operation_response",
                            "token": token,
                            "sessionKey": METADATA_PATH,
                            "response": {
                                "type": "operation_result",
                                "requestId": payload.get("requestId").cloned().unwrap_or(Value::Null),
                                "scope": scope,
                                "result": { "accepted": true },
                            },
                        }),
                    });
                }
            }
        }
        Box::pin(async { Ok(()) })
    }

    fn broadcast<'a>(&'a self, payload: Value) -> BoxFuture<'a, Result<(), String>> {
        if payload.get("type").and_then(Value::as_str) == Some("discover_workers") {
            self.emit(CoordinatorConnectionEvent::Message {
                from: "worker-1".to_string(),
                payload: ready_payload(),
            });
        }
        Box::pin(async { Ok(()) })
    }
}

/// Fake child (upstream real child with spied `process.kill`).
#[derive(Default)]
struct FakeChild {
    killed: AtomicBool,
}

/// Shared handle so the spawner can hand out the same fake child.
struct SharedChild(Arc<FakeChild>);

impl crate::coding_agent::experimental::process::InternalProcessChild for SharedChild {
    fn pid(&self) -> Option<u32> {
        Some(123)
    }

    fn kill(&self) {
        self.0.killed.store(true, Ordering::SeqCst);
    }

    fn has_exited(&self) -> bool {
        false
    }

    fn wait_exit(&self) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(std::future::pending())
    }
}

struct FakeSpawner {
    child: Arc<FakeChild>,
}

impl WorkerSpawn for FakeSpawner {
    fn spawn(
        &self,
        _args: &[String],
        _extra_env: &[(String, String)],
    ) -> Result<Box<dyn crate::coding_agent::experimental::process::InternalProcessChild>, String>
    {
        Ok(Box::new(SharedChild(Arc::clone(&self.child))))
    }
}

fn kill_spy() -> (Arc<AtomicUsize>, KillFn) {
    let count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&count);
    let spy: KillFn = Arc::new(move |pid: u32| {
        assert_eq!(pid, 123);
        counter.fetch_add(1, Ordering::SeqCst);
        true
    });
    (count, spy)
}

fn build_manager(coordinator: &Arc<FakeCoordinator>, manager: &Arc<SessionWorkerManager>) {
    let listener_manager = Arc::clone(manager);
    coordinator.listeners.lock().unwrap().push(Arc::new(
        move |event: &CoordinatorConnectionEvent| {
            listener_manager.handle_coordinator_event(event);
        },
    ));
}

fn build_manager_with_kill(
    coordinator: &Arc<FakeCoordinator>,
    kill: KillFn,
) -> Arc<SessionWorkerManager> {
    let link: Arc<dyn CoordinatorLink> = coordinator.clone();
    let manager = SessionWorkerManager::new(SessionWorkerManagerConfig {
        link,
        session_dir: "/tmp".to_string(),
        model: None,
        on_worker_count_changed: None,
        spawner: Arc::new(FakeSpawner {
            child: Arc::new(FakeChild::default()),
        }),
        codec: ControlCallCodec::default(),
        kill: Some(kill),
    });
    build_manager(coordinator, &manager);
    manager
}

async fn discover_default_worker(manager: &Arc<SessionWorkerManager>) {
    let mut peers = HashSet::new();
    peers.insert("worker-1".to_string());
    manager.discover(&peers).await;
}

struct AttachedFixture {
    coordinator: Arc<FakeCoordinator>,
    manager: Arc<SessionWorkerManager>,
    handle: RoutedSessionHandle,
    attachment: RoutedSessionAttachment,
}

async fn create_attached_worker_with_kill(kill: KillFn) -> AttachedFixture {
    let coordinator = FakeCoordinator::new();
    let manager = build_manager_with_kill(&coordinator, kill);
    discover_default_worker(&manager).await;
    let handle = manager
        .open_session(&metadata(), &[])
        .await
        .expect("open_session");
    coordinator.set_reaction(Reaction::AckDemand);
    let attachment = handle.attach_client().await.expect("attach_client");
    AttachedFixture {
        coordinator,
        manager,
        handle,
        attachment,
    }
}

async fn create_attached_worker() -> AttachedFixture {
    create_attached_worker_with_kill(Arc::new(|_pid| true)).await
}

fn noop_publish() -> Arc<dyn Fn(String, Value) -> BoxFuture<'static, ()> + Send + Sync> {
    Arc::new(|_subscription_id, _update| Box::pin(async {}))
}

/// Advance the paused tokio clock by parking the main task on a virtual
/// sleep (the runtime auto-advances paused time and drives spawned timers
/// while the test body is parked).
async fn advance_timers(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

fn service_call() -> ServiceCall {
    ServiceCall {
        service_id: "test.session".to_string(),
        instance: None,
        member: "run".to_string(),
        args: vec![json!("Hello")],
    }
}

#[tokio::test(start_paused = true)]
async fn adopts_discovered_worker_with_existing_plugin_selection() {
    let coordinator = FakeCoordinator::new();
    let manager = build_manager_with_kill(&coordinator, Arc::new(|_pid| true));
    discover_default_worker(&manager).await;
    assert!(!coordinator
        .sent
        .lock()
        .unwrap()
        .iter()
        .any(|(peer, payload)| {
            peer == "worker-1" && payload.get("type").and_then(Value::as_str) == Some("shutdown")
        }));
    assert_eq!(manager.worker_pids().len(), 1);
    assert!(manager
        .assert_session_plugin_manifest_paths(&metadata(), &[])
        .is_ok());
    manager.detach();
}

#[tokio::test(start_paused = true)]
async fn rejects_different_plugin_selection_for_active_session() {
    let fixture = create_attached_worker().await;
    let error = fixture
        .manager
        .assert_session_plugin_manifest_paths(
            &metadata(),
            &["/tmp/plugin/chord-facets.json".to_string()],
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Session session-1 is active with a different plugin selection"
    );
    assert_eq!(fixture.manager.worker_pids().len(), 1);
    fixture.manager.detach();
}

#[tokio::test(start_paused = true)]
async fn compensates_timed_out_attachment_before_rejecting_it() {
    let coordinator = FakeCoordinator::new();
    let manager = build_manager_with_kill(&coordinator, Arc::new(|_pid| true));
    discover_default_worker(&manager).await;
    let handle = manager
        .open_session(&metadata(), &[])
        .await
        .expect("open_session");
    coordinator.set_reaction(Reaction::AckDetach);

    let attaching = handle.attach_client();
    advance_timers(5_000).await;
    let error = attaching.await.unwrap_err();
    assert!(error.contains("timed out"), "unexpected: {error}");

    let demands: Vec<(String, bool)> = coordinator
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, payload)| {
            if payload.get("type").and_then(Value::as_str) != Some("session_demand") {
                return None;
            }
            Some((
                payload
                    .get("attachmentId")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string(),
                payload.get("attached").and_then(Value::as_bool).unwrap(),
            ))
        })
        .collect();
    assert_eq!(demands.len(), 2);
    assert!(demands[0].1);
    assert_eq!(demands[1], (demands[0].0.clone(), false));
    manager.detach();
}

#[tokio::test(start_paused = true)]
async fn kills_worker_when_timed_out_demand_cannot_be_reconciled() {
    let coordinator = FakeCoordinator::new();
    let (kills, spy) = kill_spy();
    let manager = build_manager_with_kill(&coordinator, spy);
    discover_default_worker(&manager).await;
    let handle = manager
        .open_session(&metadata(), &[])
        .await
        .expect("open_session");
    coordinator.set_reaction(Reaction::Silent);

    let attaching = handle.attach_client();
    advance_timers(5_000).await;
    advance_timers(5_000).await;
    advance_timers(10_000).await;
    let error = attaching.await.unwrap_err();
    assert!(
        error.contains("worker was terminated"),
        "unexpected: {error}"
    );
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    assert_eq!(manager.worker_pids().len(), 0);
    manager.detach();
}

#[tokio::test(start_paused = true)]
async fn bounds_harness_driven_worker_shutdown() {
    let (kills, spy) = kill_spy();
    let fixture = create_attached_worker_with_kill(spy).await;
    fixture.attachment.release().await.unwrap();
    fixture.coordinator.set_reaction(Reaction::Silent);

    let closing = fixture.handle.close();
    advance_timers(10_000).await;
    closing.await.unwrap();
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.manager.worker_pids().len(), 0);
    fixture.manager.detach();
}

#[tokio::test(start_paused = true)]
async fn correlates_service_results_to_generation_and_attachment() {
    let fixture = create_attached_worker().await;
    fixture.coordinator.set_reaction(Reaction::EchoOperation {
        token: "worker-token",
        null_scope: false,
    });
    let result = fixture
        .attachment
        .invoke_service(service_call(), noop_publish(), None)
        .await
        .expect("invoke_service");
    assert_eq!(result, Some(json!({ "accepted": true })));
    let operation = fixture
        .coordinator
        .sent
        .lock()
        .unwrap()
        .iter()
        .find(|(_, payload)| payload.get("type").and_then(Value::as_str) == Some("operation"))
        .cloned()
        .unwrap();
    assert_eq!(
        operation
            .1
            .get("scope")
            .and_then(|scope| scope.get("serverConnectionId"))
            .and_then(Value::as_str),
        Some("server-generation-1")
    );
    assert_eq!(operation.1.get("attachmentId"), None);
    assert_eq!(
        operation.1.get("call").map(|call| call.get("serviceId")),
        Some(Some(&json!("test.session")))
    );
    fixture.manager.detach();
}

#[tokio::test(start_paused = true)]
async fn rejects_correlated_response_with_mismatched_worker_identity() {
    let fixture = create_attached_worker().await;
    fixture.coordinator.set_reaction(Reaction::EchoOperation {
        token: "wrong-token",
        null_scope: false,
    });
    let error = fixture
        .attachment
        .invoke_service(service_call(), noop_publish(), None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("mismatched operation response"),
        "unexpected: {error}"
    );
    fixture.manager.detach();
}

#[tokio::test(start_paused = true)]
async fn rejects_null_request_scope() {
    let fixture = create_attached_worker().await;
    fixture.coordinator.set_reaction(Reaction::EchoOperation {
        token: "worker-token",
        null_scope: true,
    });
    let error = fixture
        .attachment
        .invoke_service(service_call(), noop_publish(), None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("invalid operation response"),
        "unexpected: {error}"
    );
    fixture.manager.detach();
}

#[tokio::test(start_paused = true)]
async fn rejects_pending_calls_on_replacement_without_stopping_worker() {
    let fixture = create_attached_worker().await;
    fixture.coordinator.set_reaction(Reaction::Silent);
    // Rust futures are lazy; drive the call and the replacement
    // concurrently so the operation is pending when detach lands (the
    // upstream test relies on promise-chain immediacy for this ordering).
    let calling = fixture
        .attachment
        .invoke_service(service_call(), noop_publish(), None);
    let (result, _) = tokio::join!(calling, async {
        fixture.manager.detach();
    });

    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("replaced during a worker operation"),
        "unexpected: {error}"
    );
    assert!(!fixture
        .coordinator
        .payload_types()
        .contains(&"shutdown".to_string()));
    assert_eq!(fixture.manager.worker_pids().len(), 0);
}

#[tokio::test(start_paused = true)]
async fn failed_sigkill_preserves_worker_and_cached_stop_failure() {
    let kills = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&kills);
    let fixture = create_attached_worker_with_kill(Arc::new(move |pid| {
        assert_eq!(pid, 123);
        counter.fetch_add(1, Ordering::SeqCst);
        false
    }))
    .await;
    fixture.attachment.release().await.unwrap();
    fixture.coordinator.set_reaction(Reaction::Silent);

    let (first, concurrent) = tokio::join!(fixture.handle.close(), fixture.handle.close());
    assert_eq!(
        first,
        Err("Failed to send SIGKILL to session worker 123".to_string())
    );
    assert_eq!(concurrent, first);
    assert_eq!(fixture.handle.close().await, first);
    assert_eq!(kills.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.manager.worker_pids().get("session-1"), Some(&123));
    assert!(fixture
        .handle
        .worker
        .terminated
        .value
        .lock()
        .unwrap()
        .is_none());
    assert_eq!(
        fixture
            .coordinator
            .payload_types()
            .iter()
            .filter(|kind| kind.as_str() == "shutdown")
            .count(),
        1
    );
    fixture.manager.detach();
}

#[tokio::test(start_paused = true)]
async fn failed_sigkill_during_demand_reconciliation_is_not_success() {
    let coordinator = FakeCoordinator::new();
    let manager = build_manager_with_kill(
        &coordinator,
        Arc::new(|pid| {
            assert_eq!(pid, 123);
            false
        }),
    );
    discover_default_worker(&manager).await;
    let handle = manager.open_session(&metadata(), &[]).await.unwrap();
    coordinator.set_reaction(Reaction::Silent);

    let error = handle.attach_client().await.unwrap_err();
    assert_eq!(
        error,
        "Session worker demand reconciliation and termination failed"
    );
    assert_eq!(manager.worker_pids().get("session-1"), Some(&123));
    assert!(handle.worker.terminated.value.lock().unwrap().is_none());
    manager.detach();
}

#[cfg(unix)]
mod unix_sigkill {
    use super::*;
    use rustix::io::Errno;
    use rustix::process::{Pid, Signal};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn sends_sigkill_to_the_requested_positive_pid() {
        // Fake only: neither PID is sent to the OS. No Child ownership is
        // required for discovered workers, and there must be no PID narrowing.
        for raw in [42, i32::MAX as u32] {
            assert!(kill_pid_sigkill_with(raw, |pid, signal| {
                assert_eq!(pid.as_raw_pid() as u32, raw);
                assert_eq!(signal, Signal::KILL);
                Ok(())
            }));
        }
    }

    #[test]
    fn only_esrch_is_treated_as_an_already_terminated_process() {
        let gone = |_: Pid, signal| {
            assert_eq!(signal, Signal::KILL);
            Err(Errno::SRCH)
        };
        assert!(kill_pid_sigkill_with(42, gone));
        for error in [Errno::PERM, Errno::ACCESS, Errno::INVAL, Errno::INTR] {
            assert!(!kill_pid_sigkill_with(42, |_, _| Err(error)));
        }
        // Deliberately do not signal a guessed/reaped PID to provoke ESRCH:
        // PID reuse could otherwise target a process not owned by this test.
    }

    #[test]
    fn rejects_group_and_overflow_pids_without_sending_a_signal() {
        for raw in [0, (i32::MAX as u32) + 1, u32::MAX] {
            assert!(!kill_pid_sigkill_with(raw, |_, _| {
                panic!("invalid worker PID must not reach kill_process")
            }));
        }
    }

    const CHILD_READY_ENV: &str = "PI_RUST_SESSION_WORKER_SIGKILL_CHILD_READY";

    // This is a fixture invoked in a separate copy of the test executable.
    // It does nothing during a regular run; only the parent sets the marker.
    #[test]
    fn controlled_child() {
        let Some(ready) = std::env::var_os(CHILD_READY_ENV) else {
            return;
        };
        std::fs::write(ready, b"ready").unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }

    struct OwnedChild(Child);

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            // Always clean up this test's child on assertion failure. Never
            // send another raw-PID signal after reaping it.
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    #[test]
    fn sigkill_terminates_only_the_controlled_child() {
        let temp = tempfile::tempdir().unwrap();
        let ready = temp.path().join("ready");
        // libtest omits the crate name from its test paths.
        let (_, module) = module_path!().split_once("::").unwrap();
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!("{module}::controlled_child"),
                    "--nocapture",
                ])
                .env(CHILD_READY_ENV, &ready)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "fixture exited before ready"
            );
            assert!(
                Instant::now() < ready_deadline,
                "fixture did not become ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child.0.try_wait().unwrap().is_none());
        // This is the production adapter, not the injected syscall seam.
        // The child is still unreaped, so its PID cannot have been reused.
        assert!(kill_pid_sigkill(child.0.id()));
        let exit_deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < exit_deadline,
                "SIGKILL did not terminate fixture"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.signal(), Some(Signal::KILL.as_raw()));
    }
}

// ── D5: chord control-call codec (`ControlCallCodec::chord`) ───────────────
//
// Upstream authority: session-worker-manager.ts
// (sha256 880d00516909d6bbb64a7108a2bc2e04c230c98f2b532be404ad6e3bb862a140)
// importing `decodeServiceControlCall`/`createServiceUnsubscribeCall` from
// `@earendil-works/chord`; the port delegates to
// `crate::chord::services::wire`. Byte oracles:
// tests/fixtures/experimental_d1236_oracle/oracle_output.json (`controlCalls`,
// `serverError`, `plainOperationError`).

use crate::coding_agent::experimental::session_worker::ServiceCallInstance;

fn chord_codec_test_call(call: crate::chord::types::ServiceCall) -> ServiceCall {
    crate::coding_agent::experimental::session_worker::service_call_from_chord(&call)
}

#[test]
fn chord_codec_decodes_subscribe_and_unsubscribe_control_calls() {
    let codec = ControlCallCodec::chord();
    // Oracle: controlCalls.decodeSubscribe.
    let subscribe = (codec.decode)(&chord_codec_test_call(
        crate::chord::services::wire::create_service_subscribe_call(
            "sub-1",
            "test.session",
            crate::chord::types::ServiceMode::Singleton,
        ),
    ));
    assert_eq!(
        subscribe,
        Some(ServiceControlCall::Subscribe {
            subscription_id: "sub-1".to_string(),
        })
    );
    let keyed = (codec.decode)(&chord_codec_test_call(
        crate::chord::services::wire::create_service_subscribe_call(
            "sub-2",
            "test.session",
            crate::chord::types::ServiceMode::Keyed,
        ),
    ));
    assert_eq!(
        keyed,
        Some(ServiceControlCall::Subscribe {
            subscription_id: "sub-2".to_string(),
        })
    );
    // Oracle: controlCalls.decodeUnsubscribe.
    let unsubscribe = (codec.decode)(&chord_codec_test_call(
        crate::chord::services::wire::create_service_unsubscribe_call("sub-9"),
    ));
    assert_eq!(
        unsubscribe,
        Some(ServiceControlCall::Unsubscribe {
            subscription_id: "sub-9".to_string(),
        })
    );
}

#[test]
fn chord_codec_ignores_catalogue_and_non_control_calls() {
    let codec = ControlCallCodec::chord();
    // Oracle: controlCalls.decodeCatalogue — the manager only reacts to
    // subscribe/unsubscribe, so catalogue decodes to None.
    assert_eq!(
        (codec.decode)(&chord_codec_test_call(
            crate::chord::services::wire::create_service_catalogue_call()
        )),
        None
    );
    // Oracle: controlCalls.decodeNonControl.
    assert_eq!(
        (codec.decode)(&ServiceCall {
            service_id: "test.session".to_string(),
            instance: None,
            member: "run".to_string(),
            args: Vec::new(),
        }),
        None
    );
    // Oracle: controlCalls.decodeInstanceControl — control ids never carry an
    // instance address.
    assert_eq!(
        (codec.decode)(&ServiceCall {
            service_id: "$chord.service".to_string(),
            instance: Some(ServiceCallInstance {
                key: "k".to_string(),
                generation: 2,
            }),
            member: "unsubscribe".to_string(),
            args: vec![json!("s")],
        }),
        None
    );
}

#[test]
fn chord_codec_encodes_unsubscribe_with_wire_bytes() {
    let codec = ControlCallCodec::chord();
    let call = (codec.encode_unsubscribe)("sub-9");
    // Round-trips through the real chord encoder/decoder.
    let chord_call =
        crate::coding_agent::experimental::session_worker::service_call_to_chord(&call).unwrap();
    assert_eq!(
        crate::chord::services::wire::decode_service_control_call(&chord_call),
        Some(
            crate::chord::services::wire::ServiceControlCall::Unsubscribe {
                subscription_id: "sub-9".to_string(),
            }
        )
    );
    // Oracle: controlCalls.unsubscribeWire (the chord `ServiceCall` JSON).
    assert_eq!(
        chord_call.to_json().to_string(),
        "{\"serviceId\":\"$chord.service\",\"member\":\"unsubscribe\",\"args\":[\"sub-9\"]}"
    );
}

#[test]
fn worker_operation_failure_display_matches_pi_server_error_face() {
    // Oracle: serverError — `new ServerError("service_not_found", "boom")`.
    let coded = WorkerOperationFailure {
        code: Some("service_not_found".to_string()),
        message: "boom".to_string(),
    };
    assert_eq!(coded.code.as_deref(), Some("service_not_found"));
    assert_eq!(coded.message, "boom");
    assert_eq!(
        coded.to_string(),
        "Session worker service error service_not_found: boom"
    );
    // Oracle: plainOperationError — the code-less fallback message.
    assert_eq!(
        plain_error("boom").to_string(),
        "Session worker operation failed: boom"
    );
}
