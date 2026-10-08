//! Ports of `packages/client/test/client.test.ts` (384 lines, SHA256
//! `308b1bb3d0b6f498d7671865e09b83cf8f942fec357e75f04eef12bba4afc694`) plus
//! the additional oracle scenarios from `tests/fixtures/client_oracle/oracle.mjs`.
//! Byte frames, state sequences, and error texts assert against the captured
//! node oracle output; the seam divergences (S3 abort reason, D10 void
//! results) are asserted at their disclosed seam values.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::Context;
use crate::client::service::{RemoteServiceListener, ServiceMode};
use crate::client::support::{
    oracle_line, oracle_lines, parse_ordered_json, recording_factory, MemoryByteServer, SERVER_ID,
};
use crate::client::transport::{ByteTransport, ByteTransportFactory, ByteTransportHandlers};
use crate::client::types::ClientOptions;
use crate::client::{
    create_client_service_transport, Client, ClientDisposedError, ClientError, ConnectionState,
    ConnectionStateChange,
};
use crate::protocol::cbor::{encode_cbor, CborValue};
use crate::protocol::codec::encode_server_message;
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{
    AttachmentEnvelope, ClientMessage, ProtocolError, ResponseEnvelope, ResponseOutcome, RpcTarget,
    ServerHello, ServerHelloError, ServerMessage, ServerTarget, ServiceEventEnvelope,
    SessionTarget,
};
use crate::protocol::{cbor_value_to_json, encode_frame, json_value_to_cbor};

fn server_target() -> RpcTarget {
    RpcTarget::Server(ServerTarget {
        server_id: SERVER_ID.to_string(),
    })
}

fn attach_call(session_id: &str) -> JsonValue {
    crate::client::service::service_call(
        "pi.session-management",
        "attach",
        vec![JsonValue::string(session_id)],
    )
}

async fn connect_client(server: &Arc<MemoryByteServer>) -> Client {
    // Upstream `connectClient` helper: `Client.connect({...})`, which
    // disposes the client when the handshake fails.
    Client::connect_client(ClientOptions::new(server.transport_factory(), SERVER_ID))
        .await
        .expect("client connects")
}

async fn wait_until(condition: impl Fn() -> bool) {
    for _ in 0..400 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("condition not reached in time");
}

/// `client.test.ts:26-42` (`attachClient`).
async fn attach_client(client: &Client, server: &Arc<MemoryByteServer>, session_id: &str) {
    let expected_messages = server.messages().len() + 1;
    let attaching = client.request(server_target(), attach_call(session_id), None);
    server.wait_for_messages(expected_messages).await;
    let request = server.messages().last().cloned().expect("attach request");
    let ClientMessage::Request(request) = request else {
        panic!("missing attach request");
    };
    server.send(&ServerMessage::Attachment(AttachmentEnvelope {
        attachment: Some(SessionTarget {
            server_id: SERVER_ID.to_string(),
            session_id: session_id.to_string(),
            attachment_id: format!("attachment-{session_id}"),
        }),
    }));
    server.send(&ServerMessage::Response(ResponseEnvelope {
        id: request.id,
        outcome: ResponseOutcome::Success { result: None },
    }));
    let _attachment = attaching.await.expect("attach resolves");
}

fn message_json(message: &ClientMessage) -> JsonValue {
    // The typed message back to its wire JSON, for oracle comparisons.
    let cbor: CborValue = message.to_cbor();
    cbor_value_to_json(&cbor).expect("message JSON")
}

fn hex_frames(line: &str) -> Vec<String> {
    parse_ordered_json(line)
        .as_array()
        .expect("frame list")
        .iter()
        .map(|value| value.as_str().expect("hex frame").to_string())
        .collect()
}

fn transport_error(message: &str) -> ClientError {
    ClientError::Transport(crate::client::errors::TransportError::new(message))
}

fn response_ok(id: &str, result: Option<JsonValue>) -> ServerMessage {
    ServerMessage::Response(ResponseEnvelope {
        id: id.to_string(),
        outcome: ResponseOutcome::Success { result },
    })
}

fn response_error(id: &str, code: &str, message: &str) -> ServerMessage {
    ServerMessage::Response(ResponseEnvelope {
        id: id.to_string(),
        outcome: ResponseOutcome::Failure {
            error: ProtocolError {
                code: code.to_string(),
                message: message.to_string(),
            },
        },
    })
}

/// A transport that pushes a hello frame before its first send and counts
/// closes (`client.test.ts:255-280`).
struct DataBeforeHelloTransport {
    close_counter: Arc<AtomicUsize>,
    send_counter: Arc<AtomicUsize>,
}

impl ByteTransport for DataBeforeHelloTransport {
    fn send(
        &self,
        _chunk: Vec<u8>,
    ) -> futures::future::BoxFuture<'static, Result<(), ClientError>> {
        self.send_counter.fetch_add(1, Ordering::SeqCst);
        // The push already happened synchronously inside the factory.
        Box::pin(async { Ok(()) })
    }

    fn close(&self) {
        self.close_counter.fetch_add(1, Ordering::SeqCst);
    }
}

/// A transport whose first `send` delivers a `hello_error` frame and counts
/// closes (`client.test.ts:282-312`).
struct HelloErrorTransport {
    handlers: ByteTransportHandlers,
    frame: Arc<Vec<u8>>,
    close_counter: Arc<AtomicUsize>,
    delivered: std::sync::atomic::AtomicBool,
}

impl ByteTransport for HelloErrorTransport {
    fn send(
        &self,
        _chunk: Vec<u8>,
    ) -> futures::future::BoxFuture<'static, Result<(), ClientError>> {
        if !self.delivered.swap(true, Ordering::SeqCst) {
            (self.handlers.on_data)(&self.frame);
        }
        Box::pin(async { Ok(()) })
    }

    fn close(&self) {
        self.close_counter.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn requires_a_canonical_uuidv4_server_identity() {
    let factory: ByteTransportFactory =
        Arc::new(|_| Box::pin(async { unreachable!("factory never runs") }));
    let error = Client::new(ClientOptions::new(factory, "invalid-server"))
        .expect_err("constructor rejects");
    assert_eq!(error.name(), "TypeError");
    assert_eq!(
        error.message(),
        "serverId must be a canonical lowercase UUIDv4"
    );
}

#[tokio::test]
async fn connects_only_to_the_expected_logical_server() {
    let oracle = oracle_lines();
    let matching = MemoryByteServer::new();
    let client = connect_client(&matching).await;
    assert_eq!(client.hello().expect("hello").server_id, SERVER_ID);
    client.dispose().await;

    let wrong = MemoryByteServer::with_server_id(crate::client::support::OTHER_SERVER_ID);
    let error = Client::connect_client(ClientOptions::new(wrong.transport_factory(), SERVER_ID))
        .await
        .expect_err("wrong logical server fails");
    let expected = parse_ordered_json(&oracle_line(&oracle, "wrong-server", 0));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(wrong.client_close_count(), 1);
}

#[tokio::test]
async fn updates_attachment_state_from_out_of_band_server_routing() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let changes: Arc<Mutex<Vec<Option<SessionTarget>>>> = Arc::new(Mutex::new(Vec::new()));
    let _unsubscribe = client
        .on_attachment_change({
            let changes = changes.clone();
            Arc::new(move |attachment: Option<SessionTarget>| {
                changes.lock().unwrap().push(attachment);
            })
        })
        .expect("subscribe");

    attach_client(&client, &server, "session-1").await;
    let attachment = client.attachment().expect("attached");
    assert_eq!(attachment.session_id, "session-1");
    assert_eq!(attachment.attachment_id, "attachment-session-1");

    // `server.messages[1]` against the oracle attach-request line.
    let attach_request = message_json(&server.messages()[1]);
    let attach_request_line = oracle_line(&oracle, "attachment-routing", 1);
    let expected_attach = attach_request_line
        .strip_prefix("attach-request=")
        .map(parse_ordered_json)
        .expect("attach-request oracle line");
    assert_eq!(attach_request, expected_attach);

    server.send(&ServerMessage::Attachment(AttachmentEnvelope {
        attachment: None,
    }));
    assert!(client.attachment().is_none());
    let recorded = changes.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].as_ref().expect("first").session_id, "session-1");
    assert!(recorded[1].is_none());
    client.dispose().await;
}

#[tokio::test]
async fn buffers_service_updates_until_the_subscription_snapshot_arrives() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let frames: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let client = Client::new(ClientOptions::new(
        recording_factory(&server, frames.clone()),
        SERVER_ID,
    ))
    .expect("client constructs");
    client.connect().await.expect("handshake").expect("hello");
    attach_client(&client, &server, "session-1").await;
    assert!(client.attachment().is_some());

    let updates: Arc<Mutex<Vec<JsonValue>>> = Arc::new(Mutex::new(Vec::new()));
    let listener: RemoteServiceListener = {
        let updates = updates.clone();
        Arc::new(move |update, _context| {
            let updates = updates.clone();
            Box::pin(async move {
                updates.lock().unwrap().push(update);
            })
        })
    };
    let transport = create_client_service_transport(&client, {
        let client = client.clone();
        move || client.attachment().map(RpcTarget::Session)
    });

    frames.lock().unwrap().clear();
    let opening = transport.subscribe(
        "pi.models",
        ServiceMode::Singleton,
        listener,
        &Context::background(),
    );
    server.wait_for_messages(3).await;

    // The subscribe request frame bytes (oracle `subscribe-frames`).
    let subscribe_line = oracle_line(&oracle, "subscription-flow", 1);
    let expected_frames = hex_frames(subscribe_line.strip_prefix("subscribe-frames=").unwrap());
    assert_eq!(*frames.lock().unwrap(), expected_frames);

    server.send(&ServerMessage::ServiceUpdate(ServiceEventEnvelope {
        subscription_id: "service-1".to_string(),
        update: parse_ordered_json(
            "{\"type\":\"state\",\"member\":\"state\",\"sequence\":1,\"ops\":[[\"s\",[\"revision\"],1]]}",
        ),
    }));
    tokio::task::yield_now().await;
    assert!(updates.lock().unwrap().is_empty());

    server.send(&response_ok(
        "request-2",
        Some(parse_ordered_json(
            "{\"serviceId\":\"pi.models\",\"mode\":\"singleton\",\"instances\":[{\"members\":[{\"name\":\"state\",\"kind\":\"state\",\"sequence\":0,\"ops\":[[\"r\",{\"revision\":0}]]}]}]}",
        )),
    ));
    let subscription = opening.await.expect("subscribe");

    // Snapshot: identity decode for the read-only base ops (oracle `snapshot`).
    let snapshot_line = oracle_line(&oracle, "subscription-flow", 4);
    let expected_snapshot = parse_ordered_json(snapshot_line.strip_prefix("snapshot=").unwrap());
    assert_eq!(subscription.snapshot, expected_snapshot);
    assert!(updates.lock().unwrap().is_empty());

    // An update for an unknown subscription must not disturb the connection.
    server.send(&ServerMessage::ServiceUpdate(ServiceEventEnvelope {
        subscription_id: "closed-subscription".to_string(),
        update: parse_ordered_json(
            "{\"type\":\"state\",\"member\":\"state\",\"sequence\":99,\"ops\":[[\"s\",99,99]]}",
        ),
    }));
    tokio::task::yield_now().await;
    assert!(client.connected());

    (subscription.activate)();
    wait_until(|| updates.lock().unwrap().len() == 1).await;
    assert_eq!(
        updates
            .lock()
            .unwrap()
            .iter()
            .map(|update| update
                .get("type")
                .and_then(JsonValue::as_str)
                .unwrap()
                .to_string())
            .collect::<Vec<_>>(),
        vec!["state".to_string()]
    );

    // Post-activation updates deliver in order; `#` path ids resolve.
    server.send(&ServerMessage::ServiceUpdate(ServiceEventEnvelope {
        subscription_id: "service-1".to_string(),
        update: parse_ordered_json(
            "{\"type\":\"state\",\"member\":\"state\",\"sequence\":2,\"ops\":[[\"s\",[\"revision\"],2]]}",
        ),
    }));
    server.send(&ServerMessage::ServiceUpdate(ServiceEventEnvelope {
        subscription_id: "service-1".to_string(),
        update: parse_ordered_json(
            "{\"type\":\"state\",\"member\":\"state\",\"sequence\":3,\"ops\":[[\"#\",0,[\"revision\"]],[\"s\",0,3]]}",
        ),
    }));
    wait_until(|| updates.lock().unwrap().len() == 3).await;
    let updates_line = oracle_line(&oracle, "subscription-flow", 7);
    let expected_updates = parse_ordered_json(updates_line.strip_prefix("updates-final=").unwrap());
    assert_eq!(
        JsonValue::Array(updates.lock().unwrap().clone()),
        expected_updates
    );

    // dispose() sends the unsubscribe request for the still-current target.
    frames.lock().unwrap().clear();
    let disposing = (subscription.close)();
    server.wait_for_messages(4).await;
    let unsubscribe_line = oracle_line(&oracle, "subscription-flow", 8);
    let expected_unsubscribe = hex_frames(
        unsubscribe_line
            .strip_prefix("unsubscribe-frames=")
            .unwrap(),
    );
    assert_eq!(*frames.lock().unwrap(), expected_unsubscribe);
    server.send(&response_ok("request-3", None));
    disposing.await.expect("dispose");
    assert_eq!(server.client_close_count(), 0);
    client.dispose().await;
}

#[tokio::test]
async fn correlates_out_of_order_generic_service_responses() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let first = client.request(
        server_target(),
        crate::client::service::service_call("test", "first", vec![]),
        None,
    );
    let second = client.request(
        server_target(),
        crate::client::service::service_call("test", "second", vec![]),
        None,
    );
    server.wait_for_messages(3).await;
    server.send(&response_ok("request-2", Some(JsonValue::string("second"))));
    server.send(&response_ok("request-1", Some(JsonValue::string("first"))));
    let (first_result, second_result) =
        futures::future::try_join(async { first.await.unwrap() }, async {
            second.await.unwrap()
        })
        .await
        .expect("both resolve");
    let expected = parse_ordered_json(&oracle_line(&oracle, "out-of-order-responses", 0));
    let expected = expected.get("value").expect("value");
    assert_eq!(
        JsonValue::Array(vec![first_result, second_result]),
        *expected
    );
    client.dispose().await;
}

#[tokio::test]
async fn exposes_bounded_server_errors() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let pending = client.request(
        server_target(),
        crate::client::service::service_call("test", "missing", vec![]),
        None,
    );
    server.wait_for_messages(2).await;
    server.send(&response_error(
        "request-1",
        "session_not_found",
        "Unknown session",
    ));
    let error = pending.await.expect("rejection").expect_err("server error");
    let expected = parse_ordered_json(&oracle_line(&oracle, "server-error", 0));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );
    let ClientError::Server(server_error) = &error else {
        panic!("server error variant");
    };
    assert_eq!(server_error.code, "session_not_found");
    client.dispose().await;
}

#[tokio::test]
async fn does_not_send_a_pre_aborted_untyped_rpc_request() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let token = CancellationToken::new();
    token.cancel();

    let error = client
        .request(
            server_target(),
            crate::client::service::service_call("test", "noop", vec![]),
            Some(token),
        )
        .await
        .expect("rejection")
        .expect_err("aborted");
    // Seam S3: the cancellation-token seam has no reason payload, so the
    // upstream fallback DOMException text stands in.
    assert_eq!(error.name(), "AbortError");
    assert_eq!(error.message(), "The operation was aborted");
    assert_eq!(server.messages().len(), 1);
    client.dispose().await;
}

#[tokio::test]
async fn cancels_one_untyped_rpc_request_without_disconnecting() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let frames: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let client = Client::new(ClientOptions::new(
        recording_factory(&server, frames.clone()),
        SERVER_ID,
    ))
    .expect("client constructs");
    client.connect().await.expect("handshake").expect("hello");

    let token = CancellationToken::new();
    let pending = client.request(
        server_target(),
        crate::client::service::service_call(
            "test",
            "mutate",
            vec![parse_ordered_json("{\"value\":42}")],
        ),
        Some(token.clone()),
    );
    server.wait_for_messages(2).await;
    frames.lock().unwrap().clear();
    token.cancel();
    let error = pending.await.expect("rejection").expect_err("aborted");
    assert_eq!(error.name(), "AbortError");
    server.wait_for_messages(3).await;
    let cancel_line = oracle_line(&oracle, "cancel-frame-bytes", 1);
    let expected_cancel = hex_frames(cancel_line.strip_prefix("").unwrap_or(&cancel_line));
    assert_eq!(*frames.lock().unwrap(), expected_cancel);

    server.send(&response_error("request-1", "cancelled", "cancelled"));
    tokio::task::yield_now().await;
    assert!(client.connected());
    client.dispose().await;
}

#[tokio::test]
async fn rejects_pending_requests_after_disconnect_or_disposal() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let pending = client.request(
        server_target(),
        crate::client::service::service_call("test", "pending", vec![]),
        None,
    );
    server.disconnect();
    let error = pending.await.expect("rejection").expect_err("disconnected");
    assert_eq!(error.name(), "DisconnectedError");
    client.dispose().await;
    let error = client
        .request(
            server_target(),
            crate::client::service::service_call("test", "disposed", vec![]),
            None,
        )
        .await
        .expect("rejection")
        .expect_err("disposed");
    assert_eq!(error.name(), "ClientDisposedError");
    assert_eq!(error.message(), ClientDisposedError.message());
}

#[tokio::test]
async fn rejects_server_data_delivered_before_the_client_hello_is_sent() {
    let _server = MemoryByteServer::new();
    let close_count = Arc::new(AtomicUsize::new(0));
    let send_count = Arc::new(AtomicUsize::new(0));
    let hello_frame = encode_server_message(
        &ServerMessage::Hello(ServerHello {
            server_id: SERVER_ID.to_string(),
        }),
        None,
    )
    .expect("hello encodes");

    let factory: ByteTransportFactory = {
        let close_count = close_count.clone();
        let send_count = send_count.clone();
        Arc::new(move |handlers: ByteTransportHandlers| {
            // The upstream factory fires `onData` synchronously before
            // returning the transport, so the client sees server data before
            // its own hello was booked.
            (handlers.on_data)(&hello_frame);
            let transport = DataBeforeHelloTransport {
                close_counter: close_count.clone(),
                send_counter: send_count.clone(),
            };
            Box::pin(async move { Ok(Arc::new(transport) as Arc<dyn ByteTransport>) })
        })
    };
    let client = Client::new(ClientOptions::new(factory, SERVER_ID)).expect("constructs");

    let error = client
        .connect()
        .await
        .expect("rejection")
        .expect_err("protocol failure");
    assert_eq!(error.name(), "ProtocolValidationError");
    assert_eq!(
        error.message(),
        "Received server data before the client hello was sent"
    );
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    assert_eq!(send_count.load(Ordering::SeqCst), 0);
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_typed_handshake_errors_and_closes_the_transport() {
    let oracle = oracle_lines();
    let close_count = Arc::new(AtomicUsize::new(0));
    let hello_error = encode_server_message(
        &ServerMessage::HelloError(ServerHelloError {
            error: ProtocolError {
                code: "version".to_string(),
                message: "Unsupported protocol version".to_string(),
            },
        }),
        None,
    )
    .expect("encodes");

    let frame = Arc::new(hello_error);
    let factory: ByteTransportFactory = {
        let close_count = close_count.clone();
        Arc::new(move |handlers: ByteTransportHandlers| {
            let transport = HelloErrorTransport {
                handlers,
                frame: frame.clone(),
                close_counter: close_count.clone(),
                delivered: std::sync::atomic::AtomicBool::new(false),
            };
            Box::pin(async move { Ok(Arc::new(transport) as Arc<dyn ByteTransport>) })
        })
    };
    let client = Client::new(ClientOptions::new(factory, SERVER_ID)).expect("constructs");

    let error = client
        .connect()
        .await
        .expect("rejection")
        .expect_err("handshake error");
    let expected = parse_ordered_json(&oracle_line(&oracle, "hello-error", 0));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );
    let ClientError::Server(server_error) = &error else {
        panic!("server error variant");
    };
    assert_eq!(server_error.code, "version");
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_pending_requests_and_reconnects_through_a_fresh_transport() {
    let oracle = oracle_lines();
    let first = MemoryByteServer::new();
    let second = MemoryByteServer::new();
    let connection = Arc::new(AtomicUsize::new(0));
    let states: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let factory: ByteTransportFactory = {
        let first = first.clone();
        let second = second.clone();
        let connection = connection.clone();
        Arc::new(move |handlers: ByteTransportHandlers| {
            let index = connection.fetch_add(1, Ordering::SeqCst);
            let server = if index == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let inner = server.transport_factory();
            Box::pin(async move { (inner)(handlers).await })
        })
    };
    let client = Client::new(ClientOptions::new(factory, SERVER_ID)).expect("constructs");
    {
        let states = states.clone();
        let _unsubscribe = client
            .on_connection_state_change(move |change: &ConnectionStateChange| {
                states
                    .lock()
                    .unwrap()
                    .push(change.state.as_str().to_string());
            })
            .expect("subscribe");
    }
    client.connect().await.expect("handshake").expect("hello");
    attach_client(&client, &first, "session-1").await;
    let target = client.attachment().expect("attachment");
    let pending = client.request(
        RpcTarget::Session(target),
        crate::client::service::service_call("test.session", "run", vec![]),
        None,
    );
    first.wait_for_messages(3).await;
    first.disconnect();

    let error = pending.await.expect("rejection").expect_err("disconnected");
    assert_eq!(error.name(), "DisconnectedError");
    assert_eq!(error.message(), "Byte transport closed");

    let hello = client.reconnect().await.expect("handshake").expect("hello");
    assert_eq!(hello.server_id, SERVER_ID);
    assert_eq!(connection.load(Ordering::SeqCst), 2);
    assert!(client.connected());
    assert_eq!(second.messages().len(), 1);
    let states_line = oracle_line(&oracle, "reconnect-sequence", 3);
    let expected_states = hex_frames(states_line.strip_prefix("states=").unwrap());
    assert_eq!(*states.lock().unwrap(), expected_states);
    client.dispose().await;
}

#[tokio::test]
async fn reports_transport_failures_without_leaving_requests_pending() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    let pending = client.request(
        server_target(),
        crate::client::service::service_call("test", "pending", vec![]),
        None,
    );
    server.wait_for_messages(2).await;
    server.error(transport_error("read failed"));

    let error = pending
        .await
        .expect("rejection")
        .expect_err("transport failure");
    let expected = parse_ordered_json(&oracle_line(&oracle, "transport-failure", 0));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );
    let expected_cause = expected.get("cause").and_then(JsonValue::as_str).unwrap();
    let cause = error_source_message(&error).expect("cause chain");
    assert_eq!(cause, expected_cause);
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    client.dispose().await;
}

fn error_source_message(error: &ClientError) -> Option<String> {
    use std::error::Error;
    error
        .source()
        .and_then(|source| source.downcast_ref::<ClientError>())
        .map(|source| source.message().into_owned())
}

#[tokio::test]
async fn disconnects_on_invalid_or_truncated_server_framing() {
    let oracle = oracle_lines();
    // Invalid framing: a well-framed response with no matching request.
    let invalid_server = MemoryByteServer::new();
    let invalid_client = connect_client(&invalid_server).await;
    let invalid_payload = encode_cbor(
        &json_value_to_cbor(&parse_ordered_json(
            "{\"type\":\"response\",\"id\":\"unknown\",\"ok\":true,\"result\":1}",
        )),
        crate::protocol::cbor::CborOptions::default(),
    )
    .expect("cbor");
    invalid_server.send_raw(&encode_frame(&invalid_payload).expect("frame"));
    assert_eq!(
        invalid_client.connection_state(),
        ConnectionState::Disconnected
    );

    // Truncated framing.
    let truncated_server = MemoryByteServer::new();
    let truncated_client = connect_client(&truncated_server).await;
    let pending = truncated_client.request(
        server_target(),
        crate::client::service::service_call("test", "pending", vec![]),
        None,
    );
    truncated_server.wait_for_messages(2).await;
    truncated_server.send_raw(&[0, 0, 0, 2, 1]);
    truncated_server.disconnect();

    let error = pending.await.expect("rejection").expect_err("truncated");
    let expected = parse_ordered_json(&oracle_line(&oracle, "truncated-framing", 1));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        truncated_client.connection_state(),
        ConnectionState::Disconnected
    );
    truncated_client.dispose().await;
    invalid_client.dispose().await;
}

#[tokio::test]
async fn disconnects_when_a_response_has_no_matching_request() {
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    server.send(&response_ok(
        "unknown-request",
        Some(JsonValue::Array(vec![])),
    ));

    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    assert_eq!(server.client_close_count(), 1);
    client.dispose().await;
}

// ---------------------------------------------------------------------------
// Additional oracle scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hello_and_request_frame_bytes_match_the_node_oracle() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let frames: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let client = Client::new(ClientOptions::new(
        recording_factory(&server, frames.clone()),
        SERVER_ID,
    ))
    .expect("constructs");
    client.connect().await.expect("handshake").expect("hello");
    let pending = client.request(
        server_target(),
        crate::client::service::service_call(
            "test",
            "mutate",
            vec![parse_ordered_json("{\"value\":42}")],
        ),
        None,
    );
    server.wait_for_messages(2).await;

    let expected = hex_frames(&oracle_line(&oracle, "hello-and-request-bytes", 1));
    let frames_snapshot = frames.lock().unwrap().clone();
    assert_eq!(frames_snapshot.len(), expected.len());
    assert_eq!(frames_snapshot[0], expected[0]);
    assert_eq!(frames_snapshot[1], expected[1]);

    server.send(&response_ok("request-1", Some(JsonValue::string("done"))));
    let result = pending.await.expect("resolution").expect("result");
    assert_eq!(result, JsonValue::string("done"));
    client.dispose().await;
}

#[tokio::test]
async fn service_catalogue_happy_and_invalid_paths_match_the_node_oracle() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;

    let pending = client.service_catalogue(server_target(), None);
    server.wait_for_messages(2).await;
    server.send(&response_ok(
        "request-1",
        Some(parse_ordered_json(
            "[{\"serviceId\":\"pi.models\",\"mode\":\"singleton\"}]",
        )),
    ));
    let catalogue = pending.await.expect("catalogue");
    let expected = parse_ordered_json(&oracle_line(&oracle, "service-catalogue", 0));
    let expected = expected.get("value").expect("value");
    assert_eq!(JsonValue::Array(catalogue), *expected);

    let invalid = client.service_catalogue(server_target(), None);
    server.wait_for_messages(3).await;
    server.send(&response_ok(
        "request-2",
        Some(parse_ordered_json("{\"not\":\"an array\"}")),
    ));
    let error = invalid.await.expect_err("invalid catalogue");
    let expected = parse_ordered_json(&oracle_line(&oracle, "service-catalogue", 1));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    client.dispose().await;
}

#[tokio::test]
async fn connect_rejects_while_connecting_or_connected_and_after_disposal() {
    let server = MemoryByteServer::new();
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let factory: ByteTransportFactory = {
        let server = server.clone();
        let release = release.clone();
        let gate = gate.clone();
        let entered = entered.clone();
        Arc::new(move |handlers: ByteTransportHandlers| {
            let inner = server.transport_factory();
            let release = release.clone();
            let gate = gate.clone();
            let entered = entered.clone();
            Box::pin(async move {
                if entered.fetch_add(1, Ordering::SeqCst) >= 1 {
                    return (inner)(handlers).await;
                }
                gate.notify_one();
                release.notified().await;
                (inner)(handlers).await
            })
        })
    };
    let client = Client::new(ClientOptions::new(factory, SERVER_ID)).expect("constructs");

    let first = client.connect();
    gate.notified().await;
    // A second connect while the first handshake is in flight.
    let second = client
        .connect()
        .await
        .expect("rejection")
        .expect_err("already connecting");
    assert_eq!(second.message(), "Client is already connecting");

    release.notify_one();
    let hello = first.await.expect("handshake").expect("hello");
    assert_eq!(hello.server_id, SERVER_ID);
    let third = client
        .connect()
        .await
        .expect("rejection")
        .expect_err("already connected");
    assert_eq!(third.message(), "Client is already connected");

    client.dispose().await;
    let after = client
        .connect()
        .await
        .expect("rejection")
        .expect_err("disposed");
    assert_eq!(after.name(), "ClientDisposedError");
}

#[tokio::test]
async fn disconnect_clears_attachment_and_rejects_pending() {
    let oracle = oracle_lines();
    let server = MemoryByteServer::new();
    let client = connect_client(&server).await;
    attach_client(&client, &server, "session-1").await;
    let pending = client.request(
        server_target(),
        crate::client::service::service_call("test", "pending", vec![]),
        None,
    );
    client.disconnect("Client disconnected");

    let error = pending.await.expect("rejection").expect_err("disconnected");
    let expected = parse_ordered_json(&oracle_line(&oracle, "disconnect-clears-state", 0));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );
    assert!(client.attachment().is_none());
    assert!(client.hello().is_none());
    assert_eq!(client.connection_state(), ConnectionState::Disconnected);

    let states: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let states = states.clone();
        let _unsubscribe = client
            .on_connection_state_change(move |change| {
                states
                    .lock()
                    .unwrap()
                    .push(change.state.as_str().to_string());
            })
            .expect("subscribe");
    }
    client.reconnect().await.expect("handshake").expect("hello");
    let states_line = oracle_line(&oracle, "disconnect-clears-state", 2);
    let states_line = states_line.strip_prefix("reconnected-states=").unwrap();
    let states_line = states_line.split(" attachment-still-clear").next().unwrap();
    let expected_states = hex_frames(states_line);
    assert_eq!(*states.lock().unwrap(), expected_states);
    assert!(client.attachment().is_none());
    client.dispose().await;
}

#[tokio::test]
async fn constructor_and_disposal_validation_match_the_node_oracle() {
    let oracle = oracle_lines();
    // maxFrameLength: 0 is rejected with the upstream TypeError text.
    let server = MemoryByteServer::new();
    let mut options = ClientOptions::new(server.transport_factory(), SERVER_ID);
    options.max_frame_length = Some(0);
    let error = Client::new(options).expect_err("rejected");
    assert_eq!(
        format!("{}: {}", error.name(), error.message()),
        oracle_line(&oracle, "validation-and-disposal", 1)
    );

    // Double dispose is idempotent; requests afterwards fail as disposed.
    eprintln!("step: connecting");
    let client = connect_client(&server).await;
    eprintln!("step: connected, disposing");
    client.dispose().await;
    eprintln!("step: disposed once");
    client.dispose().await;
    eprintln!("step: disposed twice");
    let _ = client
        .request(
            server_target(),
            crate::client::service::service_call("test", "disposed", vec![]),
            None,
        )
        .await;
    eprintln!("step: post-dispose request done");
    client.dispose().await;
    let error = client
        .request(
            server_target(),
            crate::client::service::service_call("test", "disposed", vec![]),
            None,
        )
        .await
        .expect("rejection")
        .expect_err("disposed");
    let expected = parse_ordered_json(&oracle_line(&oracle, "validation-and-disposal", 2));
    assert_eq!(
        error.name(),
        expected.get("name").and_then(JsonValue::as_str).unwrap()
    );
    assert_eq!(
        error.message(),
        expected.get("message").and_then(JsonValue::as_str).unwrap()
    );

    // Listeners cannot be installed on a disposed client.
    let error = match client.on_connection_state_change(|_| {}) {
        Ok(_) => panic!("expected the disposed error"),
        Err(error) => error,
    };
    assert_eq!(
        format!("{}: {}", error.name(), error.message()),
        oracle_line(&oracle, "validation-and-disposal", 3)
    );
    assert!(client.disposed());
    assert_eq!(server.client_close_count(), 1);
}

/// The maxFrameLength upper bound (upstream `MAX_UINT32`) still rejects.
#[tokio::test]
async fn max_frame_length_upper_bound_is_rejected() {
    let server = MemoryByteServer::new();
    let mut options = ClientOptions::new(server.transport_factory(), SERVER_ID);
    options.max_frame_length = Some(u32::MAX as u64 + 1);
    let error = Client::new(options).expect_err("rejected");
    assert_eq!(
        error.message(),
        format!(
            "Client maxFrameLength must be an integer between 1 and {}",
            u32::MAX
        )
    );
}
