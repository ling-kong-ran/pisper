//! Port of `packages/server/test/unix.test.ts` (143 lines, SHA256
//! `86ca4b6d7ffc3d3e2c4eff60eaf260cc1a56aea995d46a09a9edf1a2e9f73eaf`) plus
//! the oracle `unix-option-validation` section.
//!
//! Platform gate matches upstream (`describe.runIf(process.platform !==
//! "win32")`). The stale-socket child fixture (`fixtures/stale-socket-
//! server.mjs`) is replaced by an in-process bind-then-drop: the socket file
//! stays on disk with no listener behind it, which is the stale state the
//! fixture produces after `SIGKILL`.

use std::os::unix::fs::MetadataExt;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::protocol::codec::{encode_client_message, ServerMessageDecoder};
use crate::protocol::protocol::{ClientHello, ClientMessage, ServerMessage, PROTOCOL_VERSION};

use crate::server::errors::OperationError;
use crate::server::testing::TestServerHost;
use crate::server::testing::{oracle_section, ProtocolTestClient};
use crate::server::unix::{create_unix_server, get_unix_socket_path, UnixServerOptions};
use crate::server::{Server, ServerHost};

const SERVER_ID: &str = "00000000-0000-4000-8000-000000000001";

fn tokio_test() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

fn make_server(path: &str) -> Arc<Server> {
    create_unix_server(
        TestServerHost::new() as Arc<dyn ServerHost>,
        UnixServerOptions {
            path: path.to_string(),
            server_id: SERVER_ID.to_string(),
            mode: None,
            max_pending_bytes: None,
            graceful_close_timeout_ms: None,
            max_frame_length: None,
            handshake_timeout_ms: None,
            on_connection_count_changed: None,
            on_error: None,
        },
    )
    .expect("valid unix server options")
}

fn temp_directory(label: &str) -> String {
    let directory =
        std::env::temp_dir().join(format!("pi-server-tests-{label}-{}", std::process::id()));
    let nested = directory.join(label);
    std::fs::create_dir_all(&nested).expect("temp directory");
    directory.to_string_lossy().into_owned()
}

/// The upstream `connectUnixTestClient`: a real unix socket wired into the
/// `ProtocolTestClient` loopback seam.
async fn connect_unix_test_client(path: &str) -> Arc<ProtocolTestClient> {
    use tokio::io::split;

    let stream = UnixStream::connect(path)
        .await
        .expect("connect unix socket");
    let (mut read_half, write_half) = split(stream);
    // `tokio::io::split` halves: the write half is shared behind a tokio
    // `Mutex` so the `'static` futures may hold it across `.await`s.
    struct Channel(Arc<tokio::sync::Mutex<tokio::io::WriteHalf<UnixStream>>>);
    impl crate::server::testing::client::WireChannel for Channel {
        fn send(
            &self,
            chunk: Vec<u8>,
        ) -> futures::future::BoxFuture<'static, Result<(), OperationError>> {
            let half = Arc::clone(&self.0);
            Box::pin(async move {
                half.lock()
                    .await
                    .write_all(&chunk)
                    .await
                    .map_err(|error| OperationError::Other(error.to_string()))
            })
        }
        fn send_fragmented(
            &self,
            chunk: Vec<u8>,
            split_at: usize,
        ) -> futures::future::BoxFuture<'static, Result<(), OperationError>> {
            let half = Arc::clone(&self.0);
            Box::pin(async move {
                let (head, tail) = chunk.split_at(split_at.min(chunk.len()));
                let mut half = half.lock().await;
                half.write_all(head)
                    .await
                    .map_err(|error| OperationError::Other(error.to_string()))?;
                half.write_all(tail)
                    .await
                    .map_err(|error| OperationError::Other(error.to_string()))
            })
        }
        fn close(&self) -> futures::future::BoxFuture<'static, Result<(), OperationError>> {
            let half = Arc::clone(&self.0);
            Box::pin(async move {
                half.lock()
                    .await
                    .shutdown()
                    .await
                    .map_err(|error| OperationError::Other(error.to_string()))
            })
        }
    }
    let channel = Arc::new(Channel(Arc::new(tokio::sync::Mutex::new(write_half))));
    let client = Arc::new(ProtocolTestClient::new(channel));
    let pump = client.clone();
    tokio::spawn(async move {
        let mut buffer = [0u8; 4096];
        loop {
            match read_half.read(&mut buffer).await {
                Ok(0) | Err(_) => {
                    pump.mark_closed();
                    break;
                }
                Ok(read) => pump.receive(&buffer[..read]),
            }
        }
    });
    client
}

#[tokio::test]
async fn creates_an_in_memory_server_id_and_derives_its_explicit_unix_socket_path() {
    let directory = temp_directory("derive");
    let path = get_unix_socket_path(SERVER_ID, &directory).unwrap();
    assert_eq!(
        std::path::Path::new(&path),
        &std::path::Path::new(&directory).join(format!("{SERVER_ID}.sock"))
    );

    let first = make_server(&path);
    first.start().await.unwrap();
    let first_client = connect_unix_test_client(&path).await;
    match first_client.hello().await.unwrap() {
        ServerMessage::Hello(hello) => assert_eq!(hello.server_id, SERVER_ID),
        other => panic!("expected hello, got {other:?}"),
    }
    first_client.close().await.unwrap();
    first.close().await.unwrap();

    let replacement = make_server(&path);
    replacement.start().await.unwrap();
    let replacement_client = connect_unix_test_client(&path).await;
    match replacement_client.hello().await.unwrap() {
        ServerMessage::Hello(hello) => assert_eq!(hello.server_id, SERVER_ID),
        other => panic!("expected hello, got {other:?}"),
    }
    let _ = ClientMessage::Hello(ClientHello {
        version: PROTOCOL_VERSION,
    });
    let _ = encode_client_message; // parity with the upstream hello-frame encodes
    let _ = ServerMessageDecoder::new(None).unwrap().push(&[] as &[u8]);
    std::fs::remove_dir_all(&directory).ok();
}

#[tokio::test]
async fn rejects_a_live_listener_without_unlinking_it() {
    let directory = temp_directory("live");
    let path = std::path::Path::new(&directory).join("server.sock");
    let path = path.to_string_lossy().into_owned();
    // Upstream (`unix.test.ts:68-89`): the first server starts; a competing
    // start on the same path is rejected as "already running" without
    // unlinking the live socket, which still serves clients.
    let first = make_server(&path);
    first.start().await.unwrap();

    let identity = std::fs::metadata(&path).expect("socket file survives");
    assert_eq!(identity.mode() & 0o170000, 0o140000, "still a socket");
    let first_identity = (identity.dev(), identity.ino());

    let second = make_server(&path);
    let error = second.start().await.expect_err("second start rejected");
    assert!(
        error.message().contains("already running"),
        "unexpected error: {error}"
    );

    let identity = std::fs::metadata(&path).expect("socket file survives");
    assert_eq!(
        (identity.dev(), identity.ino()),
        first_identity,
        "live socket not unlinked"
    );

    let client = connect_unix_test_client(&path).await;
    match client.hello().await.unwrap() {
        ServerMessage::Hello(_) => {}
        other => panic!("expected hello, got {other:?}"),
    }
    client.close().await.unwrap();
    first.close().await.unwrap();
    std::fs::remove_dir_all(&directory).ok();
}

#[tokio::test]
async fn never_unlinks_a_regular_file_at_the_configured_path() {
    let directory = temp_directory("regular");
    let path = std::path::Path::new(&directory).join("server.sock");
    std::fs::write(&path, b"do not remove").expect("seed regular file");

    let server = make_server(&path.to_string_lossy());
    let error = server.start().await.unwrap_err();
    assert!(
        error.message().contains("non-socket"),
        "unexpected error: {error}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"do not remove");
    server.close().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"do not remove");
    std::fs::remove_dir_all(&directory).ok();
}

#[tokio::test]
async fn creates_nested_temp_parents_restricts_permissions_and_removes_its_own_socket() {
    let directory = temp_directory("nested");
    let path = std::path::Path::new(&directory)
        .join("p")
        .join("n")
        .join("server.sock");
    let path = path.to_string_lossy().into_owned();

    let server = make_server(&path);
    server.start().await.unwrap();
    let stats = std::fs::metadata(&path).expect("socket exists");
    assert_eq!(stats.mode() & 0o170000, 0o140000);
    assert_eq!(stats.mode() & 0o777, 0o600);
    let mut siblings: Vec<String> =
        std::fs::read_dir(std::path::Path::new(&path).parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
    siblings.sort();
    assert_eq!(siblings, vec!["server.sock".to_string()]);

    server.close().await.unwrap();
    assert!(
        std::fs::metadata(&path).is_err(),
        "socket removed on shutdown"
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[tokio::test]
async fn does_not_remove_a_replacement_inode_during_shutdown() {
    let directory = temp_directory("replace");
    let path = std::path::Path::new(&directory).join("server.sock");
    let path = path.to_string_lossy().into_owned();

    let server = make_server(&path);
    server.start().await.unwrap();
    std::fs::remove_file(&path).expect("unlink own socket");
    std::fs::write(&path, b"replacement").expect("plant replacement inode");

    server.close().await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    std::fs::remove_dir_all(&directory).ok();
}

#[tokio::test]
async fn removes_a_genuinely_stale_socket_before_binding() {
    let directory = temp_directory("stale");
    let path = std::path::Path::new(&directory).join("server.sock");
    let path = path.to_string_lossy().into_owned();

    // Produce the fixture state: a socket file whose listener is gone.
    let stale = std::os::unix::net::UnixListener::bind(&path).expect("bind stale socket");
    drop(stale);
    let stale_identity = std::fs::metadata(&path).expect("stale socket file");
    assert_eq!(stale_identity.mode() & 0o170000, 0o140000);

    let server = make_server(&path);
    server.start().await.unwrap();
    let live_identity = std::fs::metadata(&path).expect("live socket file");
    assert_eq!(live_identity.mode() & 0o170000, 0o140000);
    let client = connect_unix_test_client(&path).await;
    match client.hello().await.unwrap() {
        ServerMessage::Hello(_) => {}
        other => panic!("expected hello, got {other:?}"),
    }
    server.close().await.unwrap();
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn rejects_timeout_values_above_the_maximum_timer_delay() {
    let host = TestServerHost::new() as Arc<dyn ServerHost>;
    let error = create_unix_server(
        host.clone(),
        UnixServerOptions {
            path: "/tmp/pi-server-timeout-test.sock".to_string(),
            server_id: SERVER_ID.to_string(),
            mode: None,
            max_pending_bytes: None,
            graceful_close_timeout_ms: None,
            max_frame_length: None,
            handshake_timeout_ms: Some(2_147_483_648),
            on_connection_count_changed: None,
            on_error: None,
        },
    )
    .err()
    .expect("handshakeTimeoutMs rejected");
    assert!(error.message().contains("handshakeTimeoutMs"));
    let error = create_unix_server(
        host,
        UnixServerOptions {
            path: "/tmp/pi-server-timeout-test.sock".to_string(),
            server_id: SERVER_ID.to_string(),
            mode: None,
            max_pending_bytes: None,
            graceful_close_timeout_ms: Some(2_147_483_648),
            max_frame_length: None,
            handshake_timeout_ms: None,
            on_connection_count_changed: None,
            on_error: None,
        },
    )
    .err()
    .expect("gracefulCloseTimeoutMs rejected");
    assert!(error.message().contains("gracefulCloseTimeoutMs"));
}

#[test]
fn rejects_pending_byte_limits_smaller_than_one_maximum_frame() {
    let error = create_unix_server(
        TestServerHost::new() as Arc<dyn ServerHost>,
        UnixServerOptions {
            path: "/tmp/pi-server-pending-test.sock".to_string(),
            server_id: SERVER_ID.to_string(),
            mode: None,
            max_pending_bytes: Some(131),
            graceful_close_timeout_ms: None,
            max_frame_length: Some(128),
            handshake_timeout_ms: None,
            on_connection_count_changed: None,
            on_error: None,
        },
    )
    .err()
    .expect("maxPendingBytes rejected");
    assert!(
        error.message().contains("maxPendingBytes"),
        "{}",
        error.message()
    );
}

/// Rejects concurrent start calls without leaking the Unix listener
/// (server.test.ts) — the unix-preset scenario.
#[tokio::test]
async fn rejects_concurrent_start_calls_without_leaking_the_unix_listener() {
    let directory = temp_directory("concurrent");
    let path = std::path::Path::new(&directory).join("server.sock");
    let path = path.to_string_lossy().into_owned();
    let server = make_server(&path);
    let starting = server.start();
    let error = server.start().await.unwrap_err();
    assert!(error.message().contains("starting"), "{}", error.message());
    starting.await.unwrap();
    server.close().await.unwrap();
    assert!(
        std::fs::metadata(&path).is_err(),
        "socket unlinked after close"
    );
    std::fs::remove_dir_all(&directory).ok();
}

/// The oracle `unix-option-validation` section: constructor-time validation
/// texts, byte-for-byte against the captured node run.
#[test]
fn oracle_unix_option_validation() {
    // `impl Trait` is not allowed in closure parameters, so the option
    // builder is a local generic function.
    fn options(
        host: &Arc<dyn ServerHost>,
        mutate: impl FnOnce(&mut UnixServerOptions),
    ) -> Result<Arc<Server>, OperationError> {
        let mut options = UnixServerOptions {
            path: "/tmp/x.sock".to_string(),
            server_id: SERVER_ID.to_string(),
            mode: None,
            max_pending_bytes: None,
            graceful_close_timeout_ms: None,
            max_frame_length: None,
            handshake_timeout_ms: None,
            on_connection_count_changed: None,
            on_error: None,
        };
        mutate(&mut options);
        create_unix_server(Arc::clone(host), options)
    }
    let host = TestServerHost::new() as Arc<dyn ServerHost>;
    let mut messages: Vec<String> = Vec::new();
    let mut attempt = |result: Result<Arc<Server>, OperationError>| match result {
        Ok(_) => messages.push("(no error)".to_string()),
        Err(error) => messages.push(error.message().to_string()),
    };
    attempt(options(&host, |o| o.path = String::new()));
    attempt(options(&host, |o| o.mode = Some(0o1000)));
    attempt(options(&host, |o| o.max_frame_length = Some(0)));
    attempt(options(&host, |o| {
        o.max_frame_length = Some(128);
        o.max_pending_bytes = Some(131);
    }));
    attempt(options(&host, |o| o.graceful_close_timeout_ms = Some(0)));
    attempt(options(&host, |o| {
        o.handshake_timeout_ms = Some(2_147_483_648)
    }));
    match get_unix_socket_path("nope", "/tmp") {
        Ok(_) => messages.push("(no error)".to_string()),
        Err(error) => messages.push(error.message().to_string()),
    }
    assert_eq!(
        serde_json::Value::Array(
            messages
                .into_iter()
                .map(serde_json::Value::String)
                .collect()
        ),
        oracle_section("unix-option-validation")
    );
    let _ = tokio_test; // the sections above are synchronous by construction
}
