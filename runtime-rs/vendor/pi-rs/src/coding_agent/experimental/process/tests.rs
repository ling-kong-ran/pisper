//! Tests for `process.rs` (ports upstream
//! `test/experimental-internal-process.test.ts`'s deterministic assertions
//! plus node-oracle byte comparisons; sha256
//! 278bac53861ae1e9aed7f14224ee03ef68ee7dfb8a1ba50d072cbf8f0babc3ab).
//!
//! The two upstream `describe.skipIf(process.platform === "win32")` tests
//! drive a real coordinator child over a Unix socket and are runtime-I/O;
//! the deterministic launcher assertions they encode (detached pid, SIGKILL
//! termination) are covered over the D4 seam in
//! `session_worker_manager::tests`.

use super::*;

/// Serializes the tests that mutate the real process env: the
/// `std::env::{set_var,remove_var}` calls below are process-global and race
/// under the parallel test runner (same `ENV_LOCK` convention as
/// `core::auth_guidance_tests`). The only other `INTERNAL_PROCESS_ENV` writer
/// (`coordinator::tests`) sets it on a child `Command`, not this process.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn internal_process_role_reads_and_validates() {
    let read = |role: Option<&str>| get_internal_process_role_from(|_key| role.map(OsString::from));
    assert_eq!(read(None).unwrap(), None);
    assert_eq!(
        read(Some("coordinator")).unwrap(),
        Some(InternalProcessRole::Coordinator)
    );
    assert_eq!(
        read(Some("server")).unwrap(),
        Some(InternalProcessRole::Server)
    );
    assert_eq!(
        read(Some("session-worker")).unwrap(),
        Some(InternalProcessRole::SessionWorker)
    );
    let error = read(Some("bogus")).unwrap_err();
    assert_eq!(error, "Unsupported internal process role: bogus");
}

#[test]
fn consume_internal_process_role_reads_then_removes() {
    // Mirror of upstream consumeInternalProcessRole against the real env:
    // set, read, delete.
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = std::env::var_os(INTERNAL_PROCESS_ENV);
    std::env::set_var(INTERNAL_PROCESS_ENV, "coordinator");
    let role = consume_internal_process_role().unwrap();
    assert_eq!(role, Some(InternalProcessRole::Coordinator));
    assert!(std::env::var_os(INTERNAL_PROCESS_ENV).is_none());
    assert_eq!(consume_internal_process_role().unwrap(), None);
    match previous {
        Some(value) => std::env::set_var(INTERNAL_PROCESS_ENV, value),
        None => std::env::remove_var(INTERNAL_PROCESS_ENV),
    }
}

#[test]
fn internal_entrypoint_role_guards_match_upstream_error_text() {
    // Upstream coordinator.ts/session-worker.ts direct-entry blocks. Both the
    // absent and mismatched cases report the expected role; a missing lock
    // scope is emulated by the parameterized `get_internal_process_role_from`
    // face plus the shared formatting asserted here.
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = std::env::var_os(INTERNAL_PROCESS_ENV);
    std::env::set_var(INTERNAL_PROCESS_ENV, "server");
    let error = require_internal_process_role(InternalProcessRole::Coordinator).unwrap_err();
    assert_eq!(
        error,
        "Coordinator entrypoint requires an internal coordinator invocation"
    );
    std::env::remove_var(INTERNAL_PROCESS_ENV);
    let error = require_internal_process_role(InternalProcessRole::SessionWorker).unwrap_err();
    assert_eq!(
        error,
        "Session worker entrypoint requires an internal session-worker invocation"
    );
    std::env::set_var(INTERNAL_PROCESS_ENV, "session-worker");
    require_internal_process_role(InternalProcessRole::SessionWorker).unwrap();
    assert!(std::env::var_os(INTERNAL_PROCESS_ENV).is_none());
    match previous {
        Some(value) => std::env::set_var(INTERNAL_PROCESS_ENV, value),
        None => std::env::remove_var(INTERNAL_PROCESS_ENV),
    }
}

/// D4 real-spawn regression (upstream `experimental-internal-process.test.ts`:
/// "starts the coordinator through the current runtime" + "waits for a failed
/// activation child to terminate", with the socket-connect poll replaced by
/// the process-exit face because the spawned role cannot bind a socket in a
/// test sandbox). Spawns a real detached child (the test binary listing its
/// tests), asserts a distinct pid, and covers
/// [`terminate_internal_process`]'s force-exit-and-await contract.
#[tokio::test]
async fn spawns_a_real_detached_child_and_terminates_it() {
    let exe = std::env::current_exe().expect("test executable");
    let spawner = StdProcessSpawner::with_exe(exe);
    let child = spawner
        .spawn(
            InternalProcessRole::Coordinator,
            &["--list".to_string()],
            &[],
        )
        .expect("spawn detached child");
    let pid = child.pid().expect("child pid");
    assert_ne!(
        pid,
        std::process::id(),
        "child must not reuse the parent pid"
    );
    // terminateInternalProcess: pid present, not yet exited at spawn; force
    // exit resolves and a second call is a no-op. (The D4 seam's
    // `wait_exit` polls via `tokio::time::sleep`, so the future needs a
    // tokio runtime context — same convention as the manager/service tests.)
    terminate_internal_process(child.as_ref()).await;
    terminate_internal_process(child.as_ref()).await;
    assert!(child.has_exited());
}

#[test]
fn encode_control_line_matches_node_byte_oracle() {
    // tests/fixtures/experimental_oracle/oracle_output.json: encodeControlLine[0..5]
    let frames: Vec<serde_json::Value> = vec![
        serde_json::json!({ "type": "shutdown" }),
        serde_json::json!({
            "type": "session_demand",
            "serverConnectionId": "server-generation-1",
            "requestId": "req-1",
            "attachmentId": "att-1",
            "attached": true,
        }),
        serde_json::json!({
            "type": "worker_ready",
            "token": "worker-token",
            "sessionKey": "/tmp/session-1.jsonl",
            "sessionId": "session-1",
            "pid": 123,
            "metadata": {
                "id": "session-1",
                "createdAt": 1,
                "storageVersion": 1,
                "cwd": "/tmp",
                "path": "/tmp/session-1.jsonl",
                "modifiedAt": 1,
                "parentSessionId": "parent-9",
            },
            "pluginManifestPaths": ["/tmp/plugin/chord-facets.json"],
        }),
        serde_json::json!({
            "type": "operation_response",
            "token": "worker-token",
            "sessionKey": "/tmp/session-1.jsonl",
            "response": {
                "type": "operation_result",
                "requestId": "req-2",
                "scope": {
                    "serverConnectionId": "server-generation-1",
                    "attachmentId": "att-1",
                },
                "result": { "accepted": true },
            },
        }),
        serde_json::json!({
            "type": "operation",
            "requestId": "req-3",
            "scope": {
                "serverConnectionId": "server-generation-1",
                "attachmentId": "att-2",
            },
            "call": {
                "serviceId": "test.session",
                "instance": { "key": "k", "generation": 2 },
                "member": "run",
                "args": ["Hello", 3, null, { "nested": true }],
            },
        }),
    ];
    let expected: Vec<&str> = vec![
        "{\"type\":\"shutdown\"}\n",
        "{\"type\":\"session_demand\",\"serverConnectionId\":\"server-generation-1\",\"requestId\":\"req-1\",\"attachmentId\":\"att-1\",\"attached\":true}\n",
        "{\"type\":\"worker_ready\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"sessionId\":\"session-1\",\"pid\":123,\"metadata\":{\"id\":\"session-1\",\"createdAt\":1,\"storageVersion\":1,\"cwd\":\"/tmp\",\"path\":\"/tmp/session-1.jsonl\",\"modifiedAt\":1,\"parentSessionId\":\"parent-9\"},\"pluginManifestPaths\":[\"/tmp/plugin/chord-facets.json\"]}\n",
        "{\"type\":\"operation_response\",\"token\":\"worker-token\",\"sessionKey\":\"/tmp/session-1.jsonl\",\"response\":{\"type\":\"operation_result\",\"requestId\":\"req-2\",\"scope\":{\"serverConnectionId\":\"server-generation-1\",\"attachmentId\":\"att-1\"},\"result\":{\"accepted\":true}}}\n",
        "{\"type\":\"operation\",\"requestId\":\"req-3\",\"scope\":{\"serverConnectionId\":\"server-generation-1\",\"attachmentId\":\"att-2\"},\"call\":{\"serviceId\":\"test.session\",\"instance\":{\"key\":\"k\",\"generation\":2},\"member\":\"run\",\"args\":[\"Hello\",3,null,{\"nested\":true}]}}\n",
    ];
    for (frame, want) in frames.iter().zip(expected) {
        assert_eq!(encode_control_line(frame).unwrap(), want);
    }
}

#[test]
fn encode_control_line_rejects_oversized_messages() {
    // Boundary check via the limit-parameterized core (avoids a 128 MiB
    // allocation); the oracle confirmed the same error text at the real
    // MAX_CONTROL_LINE_BYTES boundary in node.
    let message = serde_json::json!({ "big": "x".repeat(64) });
    let error = encode_control_line_with_limit(&message, 16).unwrap_err();
    assert_eq!(error, "Internal control message is too large");
    assert!(encode_control_line_with_limit(&message, 4096).is_ok());
}
