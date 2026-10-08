//! Ports of `packages/server/test/server.test.ts` (127 lines, SHA256
//! `468f0cf48a78449123b87a6c66ba5a6c18a6efb4fa0d678364611cf7dc5fdcfd`,
//! non-unix scenarios) plus the oracle byte-compare contract: every
//! deterministic section of `tests/fixtures/server_oracle/oracle.out.txt` is
//! reproduced live and compared byte-for-byte (frames) and text-for-text
//! (error texts, captures).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::Value;

use super::types::SessionMetadata;
use crate::agent_core::chord_support::Context;
use crate::chord::services::wire::{
    create_service_subscribe_call, create_service_unsubscribe_call, decode_service_control_call,
    ServiceControlCall,
};
use crate::chord::types::{ServiceCall, ServiceProviderUpdate};
use crate::protocol::cbor::{encode_cbor, CborValue};
use crate::protocol::codec::{encode_client_message, ServerMessageDecoder};
use crate::protocol::framing::encode_frame;
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{
    ClientHello, ClientMessage, RpcTarget, ServerMessage, ServerTarget, PROTOCOL_VERSION,
};

use super::conformance_tests::{
    assert_capture_matches, assert_frames_match_oracle, connect, create_server,
    create_server_with_options, oracle_capture, pre_hello_request, SERVER_ID,
};
use super::connection::{ByteConnection, ByteConnectionHandler};
use super::errors::OperationError;
use super::server::Server;
use super::testing::{
    client::{response_of, session_call},
    create_test_server_services, oracle_section, wait_until, FrameLog, ProtocolTestClient,
    TestServerHost,
};
use super::types::{
    PublishCallback, RoutedServerServiceAttachment, RoutedServerServiceHost, RoutedSessionHandle,
    ServerHost, ServerOptions,
};

fn tokio_test() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

fn hello_bytes() -> Vec<u8> {
    encode_client_message(
        &ClientMessage::Hello(ClientHello {
            version: PROTOCOL_VERSION,
        }),
        None,
    )
    .unwrap()
}

fn directory_request_bytes(id: &str) -> Vec<u8> {
    encode_client_message(&pre_hello_request_with_id(id), None).unwrap()
}

fn pre_hello_request_with_id(id: &str) -> ClientMessage {
    let mut message = match pre_hello_request() {
        ClientMessage::Request(envelope) => envelope,
        other => panic!("unexpected {other:?}"),
    };
    message.id = id.to_string();
    ClientMessage::Request(message)
}

fn hello_waiter(client: &ProtocolTestClient) -> super::testing::SharedMessage {
    client.next(Arc::new(|message| {
        matches!(message, ServerMessage::Hello(_))
    }))
}

fn response_waiter(client: &ProtocolTestClient) -> super::testing::SharedMessage {
    client.next(Arc::new(|message| {
        matches!(message, ServerMessage::Response(_))
    }))
}

fn hello_error_waiter(client: &ProtocolTestClient) -> super::testing::SharedMessage {
    client.next(Arc::new(|message| {
        matches!(message, ServerMessage::HelloError(_))
    }))
}

/// Asserts one hello_error message against the oracle `error` field.
async fn assert_hello_error_matches(client: &ProtocolTestClient, section: &str) {
    let error = match hello_error_waiter(client).await.unwrap() {
        ServerMessage::HelloError(error) => error.error,
        other => panic!("expected hello_error, got {other:?}"),
    };
    let expected = &oracle_section(section)["error"];
    assert_eq!(expected["code"], error.code, "section {section} code");
    assert_eq!(
        expected["message"], error.message,
        "section {section} message"
    );
}

/// The host double used by the oracle `ambiguous-session` section: forces the
/// ambiguity at the `resolveSession` boundary exactly like the upstream
/// conformance stub.
struct AmbiguousHost;

impl ServerHost for AmbiguousHost {
    fn server_services(&self) -> Arc<dyn RoutedServerServiceHost> {
        create_test_server_services()
    }
    fn resolve_session(
        &self,
        _session_id: String,
        _context: Context,
    ) -> BoxFuture<'static, Result<SessionMetadata, OperationError>> {
        Box::pin(async {
            Err(OperationError::Server(
                super::errors::ServerError::session_ambiguous(),
            ))
        })
    }
    fn open_session(
        &self,
        _metadata: SessionMetadata,
        _context: Context,
    ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionHandle>, OperationError>> {
        Box::pin(async {
            Err(OperationError::Other(
                "must not create a Harness for an ambiguous session".to_string(),
            ))
        })
    }
}

/// The oracle `subscriptionHost()`: server services that answer chord
/// control calls, publishing one `unavailable` update per subscribe.
struct SubscriptionServices;

const SUBSCRIPTION_SNAPSHOT: &str = r#"{"serviceId":"pi.test","mode":"singleton","instances":[]}"#;

impl RoutedServerServiceHost for SubscriptionServices {
    fn attach_client(
        &self,
        _presentation: Arc<dyn super::types::RoutedServerPresentation>,
        _context: Context,
    ) -> BoxFuture<'static, Result<Arc<dyn RoutedServerServiceAttachment>, OperationError>> {
        Box::pin(async {
            Ok(Arc::new(SubscriptionAttachment) as Arc<dyn RoutedServerServiceAttachment>)
        })
    }
}

struct SubscriptionAttachment;

impl RoutedServerServiceAttachment for SubscriptionAttachment {
    fn invoke_service(
        &self,
        call: ServiceCall,
        publish: PublishCallback,
        context: Context,
    ) -> BoxFuture<'static, Result<Option<serde_json::Value>, OperationError>> {
        Box::pin(async move {
            match decode_service_control_call(&call) {
                Some(ServiceControlCall::Subscribe {
                    subscription_id, ..
                }) => {
                    publish(subscription_id, ServiceProviderUpdate::Unavailable, context).await?;
                    Ok(Some(
                        serde_json::from_str(SUBSCRIPTION_SNAPSHOT).expect("snapshot json"),
                    ))
                }
                Some(ServiceControlCall::Unsubscribe { .. }) => Ok(None),
                other => Err(OperationError::Other(format!(
                    "unexpected control call {other:?}"
                ))),
            }
        })
    }

    fn release(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        Box::pin(async { Ok(()) })
    }
}

fn subscription_host() -> Arc<TestServerHost> {
    let host = TestServerHost::new();
    host.set_server_services(Arc::new(SubscriptionServices));
    host
}

fn subscribe_call_value() -> JsonValue {
    service_call_value(&create_service_subscribe_call(
        "sub-1",
        "pi.test",
        crate::chord::types::ServiceMode::Singleton,
    ))
}

fn unsubscribe_call_value() -> JsonValue {
    service_call_value(&create_service_unsubscribe_call("sub-1"))
}

fn service_call_value(call: &ServiceCall) -> JsonValue {
    JsonValue::from_serde_json(&call.to_json())
}

fn server_target() -> RpcTarget {
    RpcTarget::Server(ServerTarget {
        server_id: SERVER_ID.to_string(),
    })
}

// ---------------------------------------------------------------------------
// server.test.ts (non-unix scenarios)
// ---------------------------------------------------------------------------

#[test]
fn requires_a_canonical_uuidv4_server_identity() {
    let host = TestServerHost::new() as Arc<TestServerHost>;
    let error = match Server::new(
        host.clone() as Arc<dyn ServerHost>,
        ServerOptions::new(Vec::new(), ""),
    ) {
        Err(error) => error,
        Ok(_) => panic!("expected a serverId error"),
    };
    assert!(error.message().contains("serverId"), "{}", error.message());
    let error = match Server::new(host, ServerOptions::new(Vec::new(), "invalid-server")) {
        Err(error) => error,
        Ok(_) => panic!("expected a serverId error"),
    };
    assert!(error.message().contains("serverId"), "{}", error.message());
}

#[test]
fn rejects_close_and_closed_when_listener_shutdown_fails() {
    tokio_test().block_on(async {
        struct FailingListener;
        impl super::listener::ServerListener for FailingListener {
            fn start(
                &self,
                _accept: super::connection::ByteConnectionAcceptor,
            ) -> BoxFuture<'static, Result<(), OperationError>> {
                Box::pin(async { Ok(()) })
            }
            fn close(&self) -> BoxFuture<'static, Result<(), OperationError>> {
                Box::pin(async { Err(OperationError::Other("listener close failed".to_string())) })
            }
        }
        let server = create_server_with_options(TestServerHost::new() as _, |options| options);
        let _ = &server;
        let server = Server::new(
            TestServerHost::new() as Arc<dyn ServerHost>,
            ServerOptions::new(
                vec![Arc::new(FailingListener) as Arc<dyn super::listener::ServerListener>],
                SERVER_ID,
            ),
        )
        .unwrap();
        server.start().await.unwrap();

        let error = server.close().await.unwrap_err();
        assert_eq!(error.message(), "listener close failed");
        let error = server.closed().await.unwrap_err();
        assert_eq!(error.message(), "listener close failed");
    });
}

// ---------------------------------------------------------------------------
// Oracle sections
// ---------------------------------------------------------------------------

#[test]
fn oracle_option_validation() {
    let messages = {
        let mut messages = Vec::new();
        let mut attempt = |result: Result<Arc<Server>, OperationError>| match result {
            Ok(_) => messages.push("(no error)".to_string()),
            Err(error) => messages.push(error.message().to_string()),
        };
        let host = TestServerHost::new() as Arc<TestServerHost>;
        attempt(Server::new(
            host.clone() as _,
            ServerOptions::new(Vec::new(), ""),
        ));
        attempt(Server::new(
            host.clone() as _,
            ServerOptions::new(Vec::new(), "invalid-server"),
        ));
        attempt(Server::new(
            host.clone() as _,
            ServerOptions::new(Vec::new(), SERVER_ID).max_frame_length(Some(0)),
        ));
        attempt(Server::new(
            host.clone() as _,
            ServerOptions::new(Vec::new(), SERVER_ID).max_frame_length(Some(4_294_967_296)),
        ));
        attempt(Server::new(
            host.clone() as _,
            ServerOptions::new(Vec::new(), SERVER_ID).handshake_timeout_ms(Some(0)),
        ));
        attempt(Server::new(
            host,
            ServerOptions::new(Vec::new(), SERVER_ID).handshake_timeout_ms(Some(2_147_483_648)),
        ));
        messages
    };
    assert_eq!(
        Value::Array(messages.into_iter().map(Value::String).collect()),
        oracle_section("option-validation")
    );
}

#[test]
fn oracle_hello_frame() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        assert_frames_match_oracle(&frames, "hello-frame");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_first_message_not_hello() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client
            .send_message(&pre_hello_request_with_id("request-1"))
            .await
            .unwrap();
        assert_hello_error_matches(&client, "first-message-not-hello").await;
        client.wait_for_close().await.unwrap();
        assert_frames_match_oracle(&frames, "first-message-not-hello");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_unsupported_version() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client
            .hello_with_version(PROTOCOL_VERSION + 1)
            .await
            .unwrap();
        assert_frames_match_oracle(&frames, "unsupported-version");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_malformed_frame() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client
            .send_bytes(&encode_frame(&[0xff]).unwrap())
            .await
            .unwrap();
        assert_hello_error_matches(&client, "malformed-frame").await;
        client.wait_for_close().await.unwrap();
        assert_frames_match_oracle(&frames, "malformed-frame");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_schema_invalid_frame() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
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
        assert_hello_error_matches(&client, "schema-invalid-frame").await;
        client.wait_for_close().await.unwrap();
        assert_frames_match_oracle(&frames, "schema-invalid-frame");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_oversized_frame() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client.send_bytes(&[1, 0, 0, 1]).await.unwrap();
        assert_hello_error_matches(&client, "oversized-frame").await;
        client.wait_for_close().await.unwrap();
        assert_frames_match_oracle(&frames, "oversized-frame");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_second_hello() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        client
            .send_message(&ClientMessage::Hello(ClientHello {
                version: PROTOCOL_VERSION,
            }))
            .await
            .unwrap();
        assert_hello_error_matches(&client, "second-hello").await;
        client.wait_for_close().await.unwrap();
        assert_frames_match_oracle(&frames, "second-hello");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_coalesced_hello_request() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        let mut wire = hello_bytes();
        wire.extend_from_slice(&directory_request_bytes("request-1"));
        client.send_bytes(&wire).await.unwrap();
        response_waiter(&client).await.unwrap();
        assert_frames_match_oracle(&frames, "coalesced-hello-request");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_fragmented_hello() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        let hello = ClientMessage::Hello(ClientHello {
            version: PROTOCOL_VERSION,
        });
        let split_at = hello_bytes().len() / 2;
        client
            .send_fragmented_message(&hello, split_at)
            .await
            .unwrap();
        hello_waiter(&client).await.unwrap();
        assert_frames_match_oracle(&frames, "fragmented-hello");
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_truncated_final_frame() {
    tokio_test().block_on(async {
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let observer = {
            let errors = errors.clone();
            Arc::new(move |error: &OperationError| {
                errors.lock().unwrap().push(error.message().to_string());
            }) as super::types::ErrorObserver
        };
        let server = create_server_with_options(TestServerHost::new() as _, |options| {
            options.on_error(observer)
        });
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

        wait_until(|| !errors.lock().unwrap().is_empty()).await;
        let expected = oracle_section("truncated-final-frame");
        assert_eq!(expected["closed"], false);
        assert_eq!(
            Value::Array(
                errors
                    .lock()
                    .unwrap()
                    .clone()
                    .into_iter()
                    .map(Value::String)
                    .collect()
            ),
            expected["errors"]
        );
        assert!(!closed.load(Ordering::SeqCst));
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_handshake_skips_sessions() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        assert_frames_match_oracle(&frames, "handshake-skips-sessions");
        let expected = oracle_section("handshake-skips-sessions");
        assert_eq!(expected["harnesses"], host.harness_session_count() as u64);
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_attach_unknown() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let response = response_of(client.attach(SERVER_ID, "missing").await).unwrap();
        assert_frames_match_oracle(&frames, "attach-unknown");
        assert_capture_matches(&oracle_capture("attach-unknown", "response"), &response);
        assert_eq!(
            oracle_section("attach-unknown")["harnesses"],
            host.harness_session_count() as u64
        );
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_wrong_server() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let response = response_of(
            client
                .attach("00000000-0000-4000-8000-000000000002", "session-1")
                .await,
        )
        .unwrap();
        assert_frames_match_oracle(&frames, "wrong-server");
        assert_capture_matches(&oracle_capture("wrong-server", "response"), &response);
        assert_eq!(
            oracle_section("wrong-server")["harnesses"],
            host.harness_session_count() as u64
        );
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_ambiguous_session() {
    tokio_test().block_on(async {
        let server = create_server(Arc::new(AmbiguousHost));
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let response = response_of(client.attach(SERVER_ID, "duplicate").await).unwrap();
        assert_frames_match_oracle(&frames, "ambiguous-session");
        assert_capture_matches(&oracle_capture("ambiguous-session", "response"), &response);
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_invalid_call() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let pending = client.next(Arc::new(|message| {
            matches!(
                message,
                ServerMessage::Response(envelope) if envelope.id == "invalid-call"
            )
        }));
        client
            .send_message(&ClientMessage::Request(
                crate::protocol::protocol::RequestEnvelope {
                    id: "invalid-call".to_string(),
                    target: server_target(),
                    call: JsonValue::object(vec![("arbitrary".to_string(), JsonValue::Bool(true))]),
                },
            ))
            .await
            .unwrap();
        let response = response_of(pending.await).unwrap();
        assert_frames_match_oracle(&frames, "invalid-call");
        assert_capture_matches(&oracle_capture("invalid-call", "response"), &response);
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_session_not_attached() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host);
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let response = response_of(
            client
                .request_session_service(
                    SERVER_ID,
                    "session-1",
                    session_call("run", Vec::new()),
                    None,
                )
                .await,
        )
        .unwrap();
        assert_frames_match_oracle(&frames, "session-not-attached");
        assert_capture_matches(
            &oracle_capture("session-not-attached", "response"),
            &response,
        );
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_session_service_ok() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let attach_response = response_of(client.attach(SERVER_ID, "session-1").await).unwrap();
        let run_response = response_of(
            client
                .request_session_service(
                    SERVER_ID,
                    "session-1",
                    session_call("run", vec![JsonValue::string("Hello")]),
                    None,
                )
                .await,
        )
        .unwrap();
        assert_frames_match_oracle(&frames, "session-service-ok");
        assert_capture_matches(
            &oracle_capture("session-service-ok", "attachResponse"),
            &attach_response,
        );
        assert_capture_matches(
            &oracle_capture("session-service-ok", "runResponse"),
            &run_response,
        );
        let expected_calls = oracle_section("session-service-ok")["serviceCalls"].clone();
        let calls = host.latest_harness("session-1").unwrap().service_calls();
        assert_eq!(calls.len(), expected_calls.as_array().unwrap().len());
        assert_eq!(calls[0].to_json(), expected_calls[0]);
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_stale_attachment() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        host.seed("session-2");
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        response_of(client.attach(SERVER_ID, "session-1").await).unwrap();
        let first_attachment_id =
            super::conformance_tests::latest_attachment_id(&client, "session-1");
        response_of(client.attach(SERVER_ID, "session-2").await).unwrap();
        frames.clear();
        let response = response_of(
            client
                .request_service(
                    RpcTarget::Session(crate::protocol::protocol::SessionTarget {
                        server_id: SERVER_ID.to_string(),
                        session_id: "session-1".to_string(),
                        attachment_id: first_attachment_id,
                    }),
                    session_call("run", vec![JsonValue::string("stale")]),
                    None,
                )
                .await,
        )
        .unwrap();
        assert_frames_match_oracle(&frames, "stale-attachment");
        assert_capture_matches(&oracle_capture("stale-attachment", "response"), &response);
        let expected_calls = oracle_section("stale-attachment")["serviceCalls"].clone();
        assert!(expected_calls.as_array().unwrap().is_empty());
        assert!(host
            .latest_harness("session-1")
            .unwrap()
            .service_calls()
            .is_empty());
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_opaque_result() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        response_of(client.attach(SERVER_ID, "session-1").await).unwrap();
        host.latest_harness("session-1")
            .unwrap()
            .set_next_service_result(Some(
                serde_json::json!({ "accepted": false, "reason": "closed" }),
            ));
        frames.clear();
        let response = response_of(
            client
                .request_session_service(
                    SERVER_ID,
                    "session-1",
                    session_call("run", Vec::new()),
                    None,
                )
                .await,
        )
        .unwrap();
        assert_frames_match_oracle(&frames, "opaque-result");
        assert_capture_matches(&oracle_capture("opaque-result", "response"), &response);
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_internal_error() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        response_of(client.attach(SERVER_ID, "session-1").await).unwrap();
        host.latest_harness("session-1")
            .unwrap()
            .set_next_service_error(Some(OperationError::Other(
                "private adapter detail".to_string(),
            )));
        frames.clear();
        let response = response_of(
            client
                .request_session_service(
                    SERVER_ID,
                    "session-1",
                    session_call("run", Vec::new()),
                    None,
                )
                .await,
        )
        .unwrap();
        assert_frames_match_oracle(&frames, "internal-error");
        assert_capture_matches(&oracle_capture("internal-error", "response"), &response);
        server.close().await.unwrap();
    });
}

#[test]
fn oracle_duplicate_request_id() {
    tokio_test().block_on(async {
        let host = TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        response_of(client.attach(SERVER_ID, "session-1").await).unwrap();
        let harness = host.latest_harness("session-1").unwrap();
        let gate = harness.gate_next_service_call();
        let first = client.request_session_service(
            SERVER_ID,
            "session-1",
            session_call("run", vec![JsonValue::string("first")]),
            None,
        );
        super::conformance_tests::fire(&first);
        gate.entered.promise().await;
        // `first` holds the auto id "request-2" (attach consumed "request-1"),
        // which is still active on the server while gated.
        frames.clear();
        let duplicate = client.request_service(
            RpcTarget::Session(crate::protocol::protocol::SessionTarget {
                server_id: SERVER_ID.to_string(),
                session_id: "session-1".to_string(),
                attachment_id: "x".to_string(),
            }),
            session_call("run", vec![JsonValue::string("second")]),
            Some("request-2".to_string()),
        );
        super::conformance_tests::fire(&duplicate);
        // The duplicate must reach the server while `first` is still active
        // (upstream: the sendMessage microtask precedes the release).
        super::conformance_tests::settle().await;
        gate.release.resolve(());
        let duplicate = response_of(duplicate.await).unwrap();
        response_of(first.await).unwrap();
        wait_until(|| frames.frames().len() == 2).await;
        assert_frames_match_oracle(&frames, "duplicate-request-id");
        assert_capture_matches(
            &oracle_capture("duplicate-request-id", "duplicate"),
            &duplicate,
        );
        server.close().await.unwrap();
    });
}

async fn run_subscription_section(
    host: Arc<TestServerHost>,
    section: &str,
    second_request: JsonValue,
    id: &str,
) {
    let server = create_server(host);
    let (client, frames) = connect(&server);
    client.hello().await.unwrap();
    if section != "subscription-flow" {
        response_of(
            client
                .request_service(
                    server_target(),
                    subscribe_call_value(),
                    Some("sub-req".to_string()),
                )
                .await,
        )
        .unwrap();
    }
    frames.clear();
    let response = response_of(
        client
            .request_service(server_target(), second_request, Some(id.to_string()))
            .await,
    )
    .unwrap();
    assert_frames_match_oracle(&frames, section);
    assert_capture_matches(&oracle_capture(section, "response"), &response);
    server.close().await.unwrap();
}

#[test]
fn oracle_subscription_flow() {
    tokio_test().block_on(async {
        let host = subscription_host();
        let server = create_server(host);
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let response = response_of(
            client
                .request_service(
                    server_target(),
                    subscribe_call_value(),
                    Some("sub-req".to_string()),
                )
                .await,
        )
        .unwrap();
        let update = client
            .next(Arc::new(|message| {
                matches!(message, ServerMessage::ServiceUpdate(_))
            }))
            .await
            .unwrap();
        assert_frames_match_oracle(&frames, "subscription-flow");
        assert_capture_matches(&oracle_capture("subscription-flow", "response"), &response);
        let expected = oracle_section("subscription-flow");
        let envelope = match &response.outcome {
            crate::protocol::protocol::ResponseOutcome::Success { result } => {
                result.clone().expect("snapshot result").to_serde_json()
            }
            other => panic!("expected snapshot result, got {other:?}"),
        };
        assert_eq!(envelope, expected["result"]);
        match update {
            ServerMessage::ServiceUpdate(event) => {
                let value = &expected["update"]["value"];
                assert_eq!(value["type"], "service_update");
                assert_eq!(value["subscriptionId"], event.subscription_id);
                assert_eq!(event.update.to_serde_json(), value["update"]);
            }
            other => panic!("expected service_update, got {other:?}"),
        }
        server.close().await.unwrap();
    });
    // unsubscribe + duplicate flows share the oracle driver shape.
    tokio_test().block_on(run_subscription_section(
        subscription_host(),
        "unsubscribe-flow",
        unsubscribe_call_value(),
        "unsub-req",
    ));
    tokio_test().block_on(run_subscription_section(
        subscription_host(),
        "duplicate-subscription",
        subscribe_call_value(),
        "dup-req",
    ));
}

#[test]
fn oracle_unknown_service_member() {
    tokio_test().block_on(async {
        let server = create_server(TestServerHost::new() as _);
        let (client, frames) = connect(&server);
        client.hello().await.unwrap();
        frames.clear();
        let response = response_of(
            client
                .request_service(
                    server_target(),
                    JsonValue::object(vec![
                        ("serviceId".to_string(), JsonValue::string("pi.models")),
                        ("member".to_string(), JsonValue::string("list")),
                        ("args".to_string(), JsonValue::Array(Vec::new())),
                    ]),
                    None,
                )
                .await,
        )
        .unwrap();
        assert_frames_match_oracle(&frames, "unknown-service-member");
        assert_capture_matches(
            &oracle_capture("unknown-service-member", "response"),
            &response,
        );
        server.close().await.unwrap();
    });
}

/// A `ByteConnection` that records the terminal close frame and rejects any
/// ordinary send (oracle `handshake-timeout` / upstream server.test.ts).
struct TimedOutConnection {
    closed: AtomicBool,
    final_chunk: Mutex<Option<Vec<u8>>>,
    frames: FrameLog,
    notify: tokio::sync::Notify,
}

impl ByteConnection for TimedOutConnection {
    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    fn send(&self, _chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>> {
        Box::pin(async {
            Err(OperationError::Other(
                "handshake timeout must use the terminal close frame".to_string(),
            ))
        })
    }
    fn close(
        &self,
        final_chunk: Option<Vec<u8>>,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        if let Some(chunk) = final_chunk {
            self.frames.push(super::testing::hex_frame(&chunk));
            *self.final_chunk.lock().unwrap() = Some(chunk);
        }
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn oracle_handshake_timeout() {
    tokio_test().block_on(async {
        let connection = Arc::new(TimedOutConnection {
            closed: AtomicBool::new(false),
            final_chunk: Mutex::new(None),
            frames: FrameLog::default(),
            notify: tokio::sync::Notify::new(),
        });
        let core = create_server_with_options(TestServerHost::new() as _, |options| {
            options
                .max_frame_length(Some(1024))
                .handshake_timeout_ms(Some(10))
        });
        core.accept(connection.clone() as Arc<dyn ByteConnection>);

        // Progress requires a live runtime (S-D): the timeout fires from a
        // spawned task while this test awaits the close notification.
        let notified = {
            let connection = connection.clone();
            async move { connection.notify.notified().await }
        };
        tokio::pin!(notified);
        loop {
            tokio::select! {
                _ = &mut notified => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    if connection.closed.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }

        assert!(connection.closed.load(Ordering::SeqCst));
        let final_chunk = connection
            .final_chunk
            .lock()
            .unwrap()
            .clone()
            .expect("terminal close frame");
        let mut decoder =
            ServerMessageDecoder::new(Some(crate::protocol::framing::FrameDecoderOptions {
                max_frame_length: Some(1024),
            }))
            .unwrap();
        let messages = decoder.push(&final_chunk).unwrap();
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            ServerMessage::HelloError(error) => {
                assert_eq!(error.error.code, "invalid_request");
                assert_eq!(error.error.message, "Handshake timeout");
            }
            other => panic!("expected hello_error, got {other:?}"),
        }
        assert_frames_match_oracle(&connection.frames, "handshake-timeout");
        core.close().await.unwrap();
    });
}

/// The upstream `handshake timeout closes with a final hello_error frame`
/// scenario (server.test.ts) decoded through the default frame budget.
#[test]
fn handshake_timeout_closes_with_a_final_hello_error_frame() {
    tokio_test().block_on(async {
        let connection = Arc::new(TimedOutConnection {
            closed: AtomicBool::new(false),
            final_chunk: Mutex::new(None),
            frames: FrameLog::default(),
            notify: tokio::sync::Notify::new(),
        });
        let core = create_server_with_options(TestServerHost::new() as _, |options| {
            options
                .max_frame_length(Some(1024))
                .handshake_timeout_ms(Some(10))
        });
        core.accept(connection.clone() as Arc<dyn ByteConnection>);
        let notified = {
            let connection = connection.clone();
            async move { connection.notify.notified().await }
        };
        tokio::pin!(notified);
        loop {
            tokio::select! {
                _ = &mut notified => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    if connection.closed.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
        assert!(connection.closed.load(Ordering::SeqCst));
        let final_chunk = connection.final_chunk.lock().unwrap().clone().unwrap();
        let mut decoder = ServerMessageDecoder::new(None).unwrap();
        let messages = decoder.push(&final_chunk).unwrap();
        assert_eq!(messages.len(), 1);
        assert!(matches!(messages[0], ServerMessage::HelloError(_)));
        core.close().await.unwrap();
    });
}
