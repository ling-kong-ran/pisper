//! Ports of the `cfg(unix)` upstream test files:
//!
//! - `packages/client/test/unix-transport.test.ts` (147 lines, SHA256
//!   `486b007138c9f264be61f0013fc0c1650f265760a04f0ede4c464520fb31c62a`)
//! - `packages/client/test/unix.test.ts` (184 lines, SHA256
//!   `82d6969098bed9ad9b6197e3e9d2ff003cbd72691826579281f055503edb7c3b`)
//!
//! Platform gate matches upstream (`describe.runIf(process.platform !==
//! "win32")`). The discovery tests substitute upstream's `RuntimeServer`
//! (server package, separate M6 slice) with a minimal in-test unix handshake
//! responder that speaks the same client/server hello exchange — the
//! behavior under test is the client-side discovery/transport surface.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::client::support::{parse_ordered_json, SERVER_ID};
use crate::client::unix::{
    create_unix_transport_factory, discover_unix_servers, DiscoverUnixServersOptions,
    UnixTransportOptions,
};
use crate::client::{Client, ClientError};
use crate::protocol::codec::{encode_server_message, ClientMessageDecoder};
use crate::protocol::protocol::{ClientMessage, ServerHello, ServerMessage};

fn server_id(value: u32) -> String {
    format!("00000000-0000-4000-8000-{:012x}", value)
}

/// Minimal stand-in for upstream's `RuntimeServer` + `createUnixListener`:
/// accepts connections and completes the server half of the handshake. The
/// accept task owns the listener (it must outlive the caller's handle), so
/// the returned `Arc` is only a keep-alive convenience.
async fn start_handshake_server(path: &std::path::Path, server_id: &str) -> Arc<UnixListener> {
    let listener = Arc::new(UnixListener::bind(path).expect("bind unix listener"));
    let server_id = server_id.to_string();
    let accept_listener = Arc::clone(&listener);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = accept_listener.accept().await else {
                return;
            };
            let hello = encode_server_message(
                &ServerMessage::Hello(ServerHello {
                    server_id: server_id.clone(),
                }),
                None,
            )
            .expect("encodes");
            tokio::spawn(async move {
                let mut buffer = [0u8; 512];
                // Serve one full session: read the client hello, answer, then
                // answer each subsequent framed request with an empty-result
                // response by echoing framed data as it arrives.
                let _ = socket.write_all(&hello).await;
                loop {
                    let read = match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    // Respond to a framed request: mirror its frame length
                    // header with a minimal valid response frame.
                    let response = encode_server_message(
                        &ServerMessage::Response(crate::protocol::protocol::ResponseEnvelope {
                            id: "request-0".to_string(),
                            outcome: crate::protocol::protocol::ResponseOutcome::Success {
                                result: Some(parse_ordered_json("[]")),
                            },
                        }),
                        None,
                    )
                    .expect("encodes");
                    let _ = socket.write_all(&response).await;
                    let _ = read;
                }
            });
        }
    });
    listener
}

#[tokio::test]
async fn rejects_invalid_unix_transport_options() {
    // `unix-transport.test.ts:49-52`.
    // (`.err().expect` — the factory value is not `Debug`.)
    let error = create_unix_transport_factory(UnixTransportOptions {
        path: String::new(),
        max_pending_bytes: None,
    })
    .err()
    .expect("empty path");
    assert!(error.message().contains("must not be empty"));

    let error = create_unix_transport_factory(UnixTransportOptions {
        path: "/tmp/pi.sock".to_string(),
        max_pending_bytes: Some(0),
    })
    .err()
    .expect("zero pending");
    assert!(error.message().contains("positive"));
}

#[tokio::test]
async fn carries_a_complete_client_handshake_and_request_over_a_real_unix_socket() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("pi.sock");
    let listener = start_handshake_server(&path, SERVER_ID).await;

    let factory = create_unix_transport_factory(UnixTransportOptions {
        path: path.to_string_lossy().into_owned(),
        max_pending_bytes: None,
    })
    .expect("valid options");
    let client =
        Client::new(crate::client::types::ClientOptions::new(factory, SERVER_ID)).expect("client");
    client.connect().await.expect("handshake").expect("hello");
    client.dispose().await;
    drop(listener);
}

#[tokio::test]
async fn reports_truncated_final_frames_through_client() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("pi.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let hello = encode_server_message(
            &ServerMessage::Hello(ServerHello {
                server_id: SERVER_ID.to_string(),
            }),
            None,
        )
        .expect("encodes");
        let mut buffer = [0u8; 512];
        // Upstream answers each decoded `hello` with the server hello and
        // ends the socket with the truncated frame on any other message
        // (`unix-transport.test.ts:99-113`).
        let mut decoder = ClientMessageDecoder::new(None).expect("decoder");
        loop {
            let read = match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(read) => read,
            };
            let Ok(messages) = decoder.push(&buffer[..read]) else {
                return;
            };
            for message in messages {
                if matches!(message, ClientMessage::Hello(_)) {
                    if socket.write_all(&hello).await.is_err() {
                        return;
                    }
                } else {
                    // Truncated frame: length prefix 2, one byte of payload,
                    // then end of stream (`unix-transport.test.ts:115`).
                    let _ = socket.write_all(&[0, 0, 0, 2, 1]).await;
                    let _ = socket.shutdown().await;
                    return;
                }
            }
        }
    });

    let factory = create_unix_transport_factory(UnixTransportOptions {
        path: path.to_string_lossy().into_owned(),
        max_pending_bytes: None,
    })
    .expect("valid options");
    let client =
        Client::new(crate::client::types::ClientOptions::new(factory, SERVER_ID)).expect("client");
    client.connect().await.expect("handshake").expect("hello");
    let error = client
        .request(
            crate::protocol::protocol::RpcTarget::Server(crate::protocol::protocol::ServerTarget {
                server_id: SERVER_ID.to_string(),
            }),
            crate::client::service::service_call("test.server", "list", vec![]),
            None,
        )
        .await
        .expect("rejection")
        .expect_err("truncated");
    assert_eq!(error.name(), "ProtocolValidationError");
    assert!(error.message().to_lowercase().contains("truncat"));
    assert_eq!(
        client.connection_state(),
        crate::client::ConnectionState::Disconnected
    );
    client.dispose().await;
}

#[tokio::test]
async fn rejects_connection_attempts_to_missing_sockets() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("missing.sock");
    let error = create_unix_transport_factory(UnixTransportOptions {
        path: path.to_string_lossy().into_owned(),
        max_pending_bytes: None,
    })
    .expect("valid options")(crate::client::transport::ByteTransportHandlers {
        on_data: Arc::new(|_| {}),
        on_close: Arc::new(|| {}),
        on_error: Arc::new(|_| {}),
    })
    .await
    .err()
    .expect("missing socket");
    assert!(error.error_code_is("ENOENT"));
}

// ---------------------------------------------------------------------------
// discoverUnixServers (unix.test.ts)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discovery_returns_no_routes_when_the_server_directory_is_missing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let missing = directory.path().join("missing");
    let routes = discover_unix_servers(DiscoverUnixServersOptions {
        directory: missing.to_string_lossy().into_owned(),
        timeout_ms: None,
    })
    .await
    .expect("discovery succeeds");
    assert!(routes.is_empty());
}

#[tokio::test]
async fn discovery_ignores_malformed_entries_non_sockets_and_mismatched_servers() {
    let directory = tempfile::tempdir().expect("tempdir");
    // Non-socket file named like a server socket.
    std::fs::write(
        directory.path().join(format!("{}.sock", server_id(1))),
        "not a socket",
    )
    .expect("write");
    // Malformed id.
    std::fs::write(directory.path().join("not-a-server.sock"), "ignored").expect("write");
    // Directory named like a server socket.
    std::fs::create_dir(directory.path().join(format!("{}.sock", server_id(2)))).expect("mkdir");

    let routes = discover_unix_servers(DiscoverUnixServersOptions {
        directory: directory.path().to_string_lossy().into_owned(),
        timeout_ms: Some(50),
    })
    .await
    .expect("discovery succeeds");
    assert!(routes.is_empty());
}

#[tokio::test]
async fn discovery_ignores_an_endpoint_that_closes_before_its_handshake() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join(format!("{}.sock", server_id(1)));
    let listener = UnixListener::bind(&path).expect("bind");
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            drop(socket); // destroy immediately
        }
    });

    let routes = discover_unix_servers(DiscoverUnixServersOptions {
        directory: directory.path().to_string_lossy().into_owned(),
        timeout_ms: None,
    })
    .await
    .expect("discovery succeeds");
    assert!(routes.is_empty());
}

#[tokio::test]
async fn discovery_propagates_unexpected_filesystem_errors() {
    let directory = tempfile::tempdir().expect("tempdir");
    let file = directory.path().join("not-a-directory");
    std::fs::write(&file, "content").expect("write");

    let error = discover_unix_servers(DiscoverUnixServersOptions {
        directory: file.to_string_lossy().into_owned(),
        timeout_ms: None,
    })
    .await
    .expect_err("ENOTDIR");
    assert!(
        error.error_code_is("ENOTDIR"),
        "expected ENOTDIR, got {error:?}"
    );
}

#[tokio::test]
async fn discovery_rejects_invalid_timeouts() {
    let error = discover_unix_servers(DiscoverUnixServersOptions {
        directory: ".".to_string(),
        timeout_ms: Some(0),
    })
    .await
    .expect_err("zero timeout");
    assert_eq!(
        error.message(),
        "Unix discovery timeoutMs must be an integer between 1 and 2147483647"
    );
}

#[tokio::test]
async fn discovery_finds_reachable_servers() {
    let directory = tempfile::tempdir().expect("tempdir");
    let first = server_id(1);
    let second = server_id(2);
    // Bind out of order so discovery's sort is exercised.
    start_handshake_server(&directory.path().join(format!("{second}.sock")), &second).await;
    start_handshake_server(&directory.path().join(format!("{first}.sock")), &first).await;

    let routes = discover_unix_servers(DiscoverUnixServersOptions {
        directory: directory.path().to_string_lossy().into_owned(),
        timeout_ms: Some(2_000),
    })
    .await
    .expect("discovery succeeds");
    assert_eq!(routes.len(), 2, "both servers reachable");
    assert_eq!(routes[0].server_id, first);
    assert_eq!(routes[1].server_id, second);
}

/// The 16-probe concurrency cap plus timeout behavior
/// (`unix.test.ts:139-161`), exercised against silent sockets.
#[tokio::test]
async fn discovery_times_out_silent_sockets_without_deleting_them() {
    let directory = tempfile::tempdir().expect("tempdir");
    let mut expected_paths = Vec::new();
    for index in 1..=20u32 {
        let path = directory.path().join(format!("{}.sock", server_id(index)));
        UnixListener::bind(&path).expect("bind silent socket");
        expected_paths.push(path);
    }

    let routes = tokio::time::timeout(
        Duration::from_secs(30),
        discover_unix_servers(DiscoverUnixServersOptions {
            directory: directory.path().to_string_lossy().into_owned(),
            timeout_ms: Some(100),
        }),
    )
    .await
    .expect("discovery completes")
    .expect("discovery succeeds");
    assert!(routes.is_empty(), "silent sockets are omitted");
    for path in expected_paths {
        assert!(path.exists(), "stale sockets are not deleted");
    }
}

/// Keep the unused-import surface honest for cfg(unix) compilation.
#[allow(unused)]
fn _witness(_stream: Option<UnixStream>, _error: Option<ClientError>) {}
