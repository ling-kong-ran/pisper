//! Port of `packages/server/test/protocol.test.ts` (168 lines, SHA256
//! `33157c44d161baf8c4a838df7c5d852bbfdecb1b62da8cd7abc0a680cbbbac48`): the
//! framed handshake and hostile-input surface.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::protocol::cbor::{encode_cbor, CborValue};
use crate::protocol::codec::{encode_client_message, encode_server_message, ServerMessageDecoder};
use crate::protocol::framing::encode_frame;
use crate::protocol::protocol::{
    ClientHello, ClientMessage, ProtocolError, ServerHelloError, ServerMessage, PROTOCOL_VERSION,
};

use super::conformance_tests::{
    connect, create_server, create_server_with_options, pre_hello_request, SERVER_ID,
};
use super::connection::{ByteConnection, ByteConnectionHandler};
use super::errors::OperationError;
use super::testing::ProtocolTestClient;
use super::types::ErrorObserver;

fn tokio_test() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

fn directory_request(id: &str) -> ClientMessage {
    use crate::protocol::json::JsonValue;
    ClientMessage::Request(crate::protocol::protocol::RequestEnvelope {
        id: id.to_string(),
        target: crate::protocol::protocol::RpcTarget::Server(
            crate::protocol::protocol::ServerTarget {
                server_id: SERVER_ID.to_string(),
            },
        ),
        call: JsonValue::object(vec![
            (
                "serviceId".to_string(),
                JsonValue::string("pi.session-directory"),
            ),
            ("member".to_string(), JsonValue::string("list")),
            ("args".to_string(), JsonValue::Array(Vec::new())),
        ]),
    })
}

fn hello_error_waiter(client: &ProtocolTestClient) -> super::testing::SharedMessage {
    client.next(Arc::new(|message| {
        matches!(message, ServerMessage::HelloError(_))
    }))
}

/// Awaits the hello_error waiter and returns its protocol error.
async fn hello_error_of(waiter: super::testing::SharedMessage) -> ProtocolError {
    match waiter.await.unwrap() {
        ServerMessage::HelloError(error) => error.error,
        other => panic!("expected hello_error, got {other:?}"),
    }
}

#[test]
fn requires_hello_as_the_first_message() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        client.send_message(&pre_hello_request()).await.unwrap();
        let error = hello_error_of(hello_error_waiter(&client)).await;
        assert_eq!(error.code, "invalid_request");
        client.wait_for_close().await.unwrap();
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_unsupported_protocol_versions() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        let error = hello_error_of(client.hello_with_version(PROTOCOL_VERSION + 1)).await;
        assert_eq!(error.code, "version");
        client.wait_for_close().await.unwrap();
        server.close().await.unwrap();
    });
}

#[test]
fn accepts_fragmented_hello_and_request_frames() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        let hello = ClientMessage::Hello(ClientHello {
            version: PROTOCOL_VERSION,
        });
        let hello_frame = encode_client_message(&hello, None).unwrap();
        let hello_response = client.next(Arc::new(|message| {
            matches!(message, ServerMessage::Hello(_))
        }));
        client
            .send_fragmented_message(&hello, hello_frame.len() / 2)
            .await
            .unwrap();
        match hello_response.await.unwrap() {
            ServerMessage::Hello(hello) => assert_eq!(hello.server_id, SERVER_ID),
            other => panic!("expected hello, got {other:?}"),
        }

        let response = client.next(Arc::new(|message| {
            matches!(message, ServerMessage::Response(_))
        }));
        let request = directory_request("request-1");
        let frame = encode_client_message(&request, None).unwrap();
        client
            .send_fragmented_message(&request, frame.len() / 2)
            .await
            .unwrap();
        match response.await.unwrap() {
            ServerMessage::Response(envelope) => {
                assert_eq!(envelope.id, "request-1");
                match &envelope.outcome {
                    crate::protocol::protocol::ResponseOutcome::Failure { error } => {
                        assert_eq!(error.code, "internal_error")
                    }
                    other => panic!("expected failure, got {other:?}"),
                }
            }
            other => panic!("expected response, got {other:?}"),
        }
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_hostile_framed_input_malformed_cbor() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        client
            .send_bytes(&encode_frame(&[0xff]).unwrap())
            .await
            .unwrap();
        let error = hello_error_of(hello_error_waiter(&client)).await;
        assert_eq!(error.code, "invalid_request");
        client.wait_for_close().await.unwrap();
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_hostile_framed_input_schema_invalid_cbor() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        let hostile = CborValue::Map(vec![
            ("type".to_string(), CborValue::Text("hello".to_string())),
            ("version".to_string(), CborValue::Uint(1)),
            ("extra".to_string(), CborValue::Bool(true)),
        ]);
        let payload = encode_cbor(&hostile, Default::default()).unwrap();
        client
            .send_bytes(&encode_frame(&payload).unwrap())
            .await
            .unwrap();
        let error = hello_error_of(hello_error_waiter(&client)).await;
        assert_eq!(error.code, "invalid_request");
        client.wait_for_close().await.unwrap();
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_hostile_framed_input_oversized_frame() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        client.send_bytes(&[1, 0, 0, 1]).await.unwrap();
        let error = hello_error_of(hello_error_waiter(&client)).await;
        assert_eq!(error.code, "invalid_request");
        client.wait_for_close().await.unwrap();
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_a_second_hello_after_completing_the_handshake() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        client
            .send_message(&ClientMessage::Hello(ClientHello {
                version: PROTOCOL_VERSION,
            }))
            .await
            .unwrap();
        let error = hello_error_of(hello_error_waiter(&client)).await;
        assert_eq!(error.code, "invalid_request");
        assert!(
            error.message.contains("first message"),
            "unexpected message: {}",
            error.message
        );
        client.wait_for_close().await.unwrap();
        server.close().await.unwrap();
    });
}

#[test]
fn processes_a_hello_and_request_coalesced_in_one_byte_chunk() {
    tokio_test().block_on(async {
        let server = create_server(super::testing::host::TestServerHost::new() as _);
        let (client, _frames) = connect(&server);
        let hello = encode_client_message(
            &ClientMessage::Hello(ClientHello {
                version: PROTOCOL_VERSION,
            }),
            None,
        )
        .unwrap();
        let request = encode_client_message(&directory_request("request-1"), None).unwrap();
        let mut wire = Vec::with_capacity(hello.len() + request.len());
        wire.extend_from_slice(&hello);
        wire.extend_from_slice(&request);

        client.send_bytes(&wire).await.unwrap();
        let hello = client.next(Arc::new(|message| {
            matches!(message, ServerMessage::Hello(_))
        }));
        match hello.await.unwrap() {
            ServerMessage::Hello(_) => {}
            other => panic!("expected hello, got {other:?}"),
        }
        let response = client.next(Arc::new(|message| {
            matches!(message, ServerMessage::Response(_))
        }));
        match response.await.unwrap() {
            ServerMessage::Response(envelope) => {
                assert_eq!(envelope.id, "request-1");
                match &envelope.outcome {
                    crate::protocol::protocol::ResponseOutcome::Failure { error } => {
                        assert_eq!(error.code, "internal_error");
                        assert_eq!(error.message, "Internal server error");
                    }
                    other => panic!("expected failure, got {other:?}"),
                }
            }
            other => panic!("expected response, got {other:?}"),
        }
        server.close().await.unwrap();
    });
}

#[test]
fn reports_a_truncated_final_frame_when_the_peer_closes() {
    tokio_test().block_on(async {
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let observer = {
            let errors = errors.clone();
            Arc::new(move |error: &OperationError| {
                errors.lock().unwrap().push(error.message().to_string());
            }) as ErrorObserver
        };
        let server = create_server_with_options(
            super::testing::host::TestServerHost::new() as _,
            |options| options.on_error(observer),
        );

        let closed = Arc::new(AtomicBool::new(false));
        struct SinkConnection(Arc<AtomicBool>);
        impl ByteConnection for SinkConnection {
            fn closed(&self) -> bool {
                self.0.load(Ordering::SeqCst)
            }
            fn send(&self, _chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>> {
                Box::pin(async { Ok(()) })
            }
            fn close(
                &self,
                _final_chunk: Option<Vec<u8>>,
            ) -> BoxFuture<'static, Result<(), OperationError>> {
                self.0.store(true, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            }
        }
        let handler: ByteConnectionHandler =
            server.accept(Arc::new(SinkConnection(closed.clone())));
        (handler.on_data)(&[0, 0, 0, 2, 1]);
        (handler.on_close)();

        assert!(!closed.load(Ordering::SeqCst));
        let recorded = errors.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1, "errors: {recorded:?}");
        assert!(
            recorded[0].to_lowercase().contains("truncated"),
            "unexpected error: {}",
            recorded[0]
        );
        server.close().await.unwrap();
    });
}

/// Guards the decoder import used by the unix-connection test file sibling
/// and keeps the final-chunk decode path exercised on every platform.
#[test]
fn final_chunk_decodes_as_one_server_message() {
    let final_message = ServerMessage::HelloError(ServerHelloError {
        error: ProtocolError {
            code: "invalid_request".to_string(),
            message: "Protocol violation".to_string(),
        },
    });
    let frame = encode_server_message(&final_message, None).unwrap();
    let mut decoder = ServerMessageDecoder::new(None).unwrap();
    let messages = decoder.push(&frame).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(matches!(messages[0], ServerMessage::HelloError(_)));
}
