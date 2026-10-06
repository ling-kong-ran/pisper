//! Port of `packages/server/test/conformance.test.ts` (502 lines, SHA256
//! `849af98dab82de8be93e62ebfd67abdce4395781d928e0d51e5d5de5192d1174`): the
//! session-protocol conformance suite over the in-memory loopback pair.
//!
//! Also hosts the shared helpers used by the sibling ported test files
//! (`protocol_tests`, `server_tests`, `listener_tests`): the loopback
//! `connect` of the upstream test head, response-envelope matchers, and the
//! oracle byte-compare helpers.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::Value;

use super::types::SessionMetadata;
use crate::agent_core::chord_support::Context;
use crate::chord::types::ServiceCall;
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{ClientMessage, ProtocolError, ResponseEnvelope, ResponseOutcome};

use super::errors::{OperationError, ServerError};
use super::server::Server;
use super::testing::{
    client::{response_of, session_call},
    connect_loopback, create_test_server_services, oracle_section, wait_until, FrameLog,
    ProtocolTestClient,
};
use super::types::{
    PublishCallback, RoutedServerPresentation, RoutedServerServiceAttachment,
    RoutedServerServiceHost, RoutedSessionAttachment, RoutedSessionHandle, ServerHost,
    ServerOptions, TerminatedSignal,
};

pub(crate) const SERVER_ID: &str = "00000000-0000-4000-8000-000000000001";

/// Drives one registered client call eagerly like upstream's
/// `void this.sendMessage(...)` microtask dispatch (S-D): spawns a clone of
/// the shared future so the request bytes go out before the next await
/// (`Shared` memoizes the outcome for the awaiting original).
pub(crate) fn fire(call: &super::testing::SharedMessage) {
    tokio::spawn(call.clone());
}

/// Yields long enough for every spawned send/dispatch task to reach its next
/// await point (the port's equivalent of the upstream microtask queue
/// draining between synchronous statements; S-D).
pub(crate) async fn settle() {
    for _ in 0..25 {
        tokio::task::yield_now().await;
    }
}

/// The upstream `createServer(host)` head helper.
pub(crate) fn create_server(host: Arc<dyn ServerHost>) -> Arc<Server> {
    create_server_with_options(host, |options| options)
}

/// `createServer` with an options mutator (upstream `new Server(host,
/// { listeners: [], serverId, ...options })`).
pub(crate) fn create_server_with_options(
    host: Arc<dyn ServerHost>,
    configure: impl FnOnce(ServerOptions) -> ServerOptions,
) -> Arc<Server> {
    let options = configure(ServerOptions::new(Vec::new(), SERVER_ID));
    Server::new(host, options).expect("valid test server options")
}

/// The upstream `connect(server)` head helper.
pub(crate) fn connect(server: &Arc<Server>) -> (Arc<ProtocolTestClient>, FrameLog) {
    connect_loopback(server)
}

/// `sessionCall(member, args)` — the chord-shaped call value.
pub(crate) fn test_session_call(member: &str, args: Vec<JsonValue>) -> JsonValue {
    session_call(member, args)
}

/// The chord-typed expectation for `sessionCall`.
pub(crate) fn expected_session_call(member: &str, args: Vec<serde_json::Value>) -> ServiceCall {
    ServiceCall {
        service_id: "test.session".to_string(),
        instance: None,
        member: member.to_string(),
        args,
    }
}

/// A request payload like the upstream `client.sendMessage({ type: "request",
/// ... })` used before the handshake.
pub(crate) fn pre_hello_request() -> ClientMessage {
    ClientMessage::Request(crate::protocol::protocol::RequestEnvelope {
        id: "request-1".to_string(),
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

/// Upstream `latestAttachmentId(client, sessionId)`.
pub(crate) fn latest_attachment_id(client: &ProtocolTestClient, session_id: &str) -> String {
    for message in client.messages().iter().rev() {
        if let crate::protocol::protocol::ServerMessage::Attachment(attachment) = message {
            if let Some(target) = &attachment.attachment {
                if target.session_id == session_id {
                    return target.attachment_id.clone();
                }
            }
        }
    }
    panic!("Missing attachment for {session_id}");
}

/// The upstream `toMatchObject({ ok: true })` on a response envelope.
pub(crate) fn assert_ok(envelope: &ResponseEnvelope) -> Option<JsonValue> {
    match &envelope.outcome {
        ResponseOutcome::Success { result } => result.clone(),
        ResponseOutcome::Failure { error } => {
            panic!(
                "expected ok response, got {}: {}",
                error.code, error.message
            )
        }
    }
}

/// The upstream `toMatchObject({ ok: false, error: { code } })`.
pub(crate) fn assert_error_code(envelope: &ResponseEnvelope, code: &str) -> ProtocolError {
    match &envelope.outcome {
        ResponseOutcome::Success { .. } => panic!("expected {code} failure, got ok"),
        ResponseOutcome::Failure { error } => {
            assert_eq!(error.code, code, "failure code");
            error.clone()
        }
    }
}

/// Awaits one `requestService`-shaped future and unwraps the envelope.
pub(crate) async fn envelope_of(
    message: Result<crate::protocol::protocol::ServerMessage, OperationError>,
) -> ResponseEnvelope {
    response_of(message).expect("response message")
}

// ---------------------------------------------------------------------------
// Oracle compare helpers (shared with server_tests / protocol_tests)
// ---------------------------------------------------------------------------

/// The oracle `frames` array for one section: either the section line is a
/// bare array of hex strings, or an object with a `frames` key.
pub(crate) fn expected_frames(section: &str) -> Vec<String> {
    let value = oracle_section(section);
    match &value {
        Value::Array(entries) => entries
            .iter()
            .map(|entry| entry.as_str().expect("hex string").to_string())
            .collect(),
        Value::Object(object) => object["frames"]
            .as_array()
            .expect("frames array")
            .iter()
            .map(|entry| entry.as_str().expect("hex string").to_string())
            .collect(),
        _ => panic!("unexpected oracle shape for {section}"),
    }
}

/// Asserts the recorded frames byte-match the oracle section.
pub(crate) fn assert_frames_match_oracle(frames: &FrameLog, section: &str) {
    assert_eq!(
        frames.frames(),
        expected_frames(section),
        "section {section}"
    );
}

/// Asserts one captured response envelope against the oracle capture
/// (`{ ok: true, value: { type: "response", ... } }`).
pub(crate) fn assert_capture_matches(capture: &Value, envelope: &ResponseEnvelope) {
    let value = &capture["value"];
    assert_eq!(value["type"], "response");
    assert_eq!(value["id"], envelope.id, "response id");
    match &envelope.outcome {
        ResponseOutcome::Success { result } => {
            assert_eq!(value["ok"], true, "expected ok capture");
            match (value.get("result"), result) {
                (None, None) => {}
                (Some(expected), Some(actual)) => {
                    assert_eq!(expected, &actual.to_serde_json(), "result value")
                }
                (expected, actual) => panic!(
                    "result presence mismatch: oracle {:?} port {:?}",
                    expected.is_some(),
                    actual.is_some()
                ),
            }
        }
        ResponseOutcome::Failure { error } => {
            assert_eq!(value["ok"], false, "expected failure capture");
            assert_eq!(value["error"]["code"], error.code, "error code");
            assert_eq!(value["error"]["message"], error.message, "error message");
        }
    }
}

/// The oracle capture object for a named key of one section.
pub(crate) fn oracle_capture(section: &str, key: &str) -> Value {
    oracle_section(section)[key].clone()
}

// ---------------------------------------------------------------------------
// Server-host doubles for the custom-host conformance scenarios
// ---------------------------------------------------------------------------

/// A `RoutedSessionHandle` that does nothing (upstream object literals with
/// `attachClient: () => ({ invokeService: async () => undefined, release() {} })`).
struct NoopSessionHandle;

impl RoutedSessionHandle for NoopSessionHandle {
    fn attach_client(
        &self,
        _context: Context,
    ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionAttachment>, OperationError>> {
        Box::pin(async { Ok(Arc::new(NoopAttachment) as Arc<dyn RoutedSessionAttachment>) })
    }

    fn terminated(&self) -> Option<TerminatedSignal> {
        None
    }

    fn close(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        Box::pin(async { Ok(()) })
    }
}

struct NoopAttachment;

impl RoutedSessionAttachment for NoopAttachment {
    fn invoke_service(
        &self,
        _call: ServiceCall,
        _publish: PublishCallback,
        _context: Context,
    ) -> BoxFuture<'static, Result<Option<serde_json::Value>, OperationError>> {
        Box::pin(async { Ok(None) })
    }

    fn release(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        Box::pin(async { Ok(()) })
    }
}

// ---------------------------------------------------------------------------
// Session protocol
// ---------------------------------------------------------------------------

fn tokio_test() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
}

#[test]
fn handshake_identifies_the_logical_server_without_listing_sessions() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);

        let hello = match client.hello().await.unwrap() {
            crate::protocol::protocol::ServerMessage::Hello(hello) => hello,
            other => panic!("expected hello, got {other:?}"),
        };
        assert_eq!(hello.server_id, SERVER_ID);
        assert_eq!(host.harness_session_count(), 0);
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_a_semantically_invalid_service_call_after_envelope_decoding() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        let server = create_server(host);
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        let response = client.next(Arc::new(|message| {
            matches!(
                message,
                crate::protocol::protocol::ServerMessage::Response(envelope)
                    if envelope.id == "invalid-call"
            )
        }));
        client
            .send_message(&ClientMessage::Request(
                crate::protocol::protocol::RequestEnvelope {
                    id: "invalid-call".to_string(),
                    target: crate::protocol::protocol::RpcTarget::Server(
                        crate::protocol::protocol::ServerTarget {
                            server_id: SERVER_ID.to_string(),
                        },
                    ),
                    call: JsonValue::object(vec![("arbitrary".to_string(), JsonValue::Bool(true))]),
                },
            ))
            .await
            .unwrap();
        let envelope = response_of(response.await).unwrap();
        assert_error_code(&envelope, "invalid_request");
        server.close().await.unwrap();
    });
}

/// Upstream scenario 3: `openSession` receives the exact repository metadata
/// (S-A: value equality of the concrete metadata record). Upstream v1.0.0
/// extends the minimal `SessionMetadata` with host-specific fields; the
/// port's concrete record has only `id`, which the equality check covers.
#[test]
fn attach_passes_concrete_repository_metadata_to_the_harness_host() {
    tokio_test().block_on(async {
        let metadata = SessionMetadata::new("session-1");
        let received: Arc<Mutex<Option<SessionMetadata>>> = Arc::new(Mutex::new(None));

        struct MetadataHost {
            metadata: SessionMetadata,
            received: Arc<Mutex<Option<SessionMetadata>>>,
        }
        impl ServerHost for MetadataHost {
            fn server_services(&self) -> Arc<dyn RoutedServerServiceHost> {
                create_test_server_services()
            }
            fn resolve_session(
                &self,
                _session_id: String,
                _context: Context,
            ) -> BoxFuture<'static, Result<SessionMetadata, OperationError>> {
                let metadata = self.metadata.clone();
                Box::pin(async move { Ok(metadata) })
            }
            fn open_session(
                &self,
                candidate: SessionMetadata,
                _context: Context,
            ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionHandle>, OperationError>>
            {
                let received = self.received.clone();
                Box::pin(async move {
                    *received.lock().unwrap() = Some(candidate);
                    Ok(Arc::new(NoopSessionHandle) as Arc<dyn RoutedSessionHandle>)
                })
            }
        }

        let host: Arc<dyn ServerHost> = Arc::new(MetadataHost {
            metadata: metadata.clone(),
            received: received.clone(),
        });
        let server = create_server(host);
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();

        let envelope = envelope_of(client.attach(SERVER_ID, "session-1").await).await;
        assert_ok(&envelope);
        assert_eq!(
            *received.lock().unwrap(),
            Some(metadata),
            "openSession receives the exact metadata value (S-A)"
        );
        server.close().await.unwrap();
    });
}

/// Upstream scenario 4: opaque server services publish attachment frames out
/// of band through the presentation seam.
#[test]
fn routes_opaque_server_services_and_publishes_attachment_changes_out_of_band() {
    tokio_test().block_on(async {
        let backing = super::testing::host::TestServerHost::new();
        backing.seed("session-1");
        let release_count = Arc::new(AtomicI64::new(0));

        struct OpaqueServices {
            release_count: Arc<AtomicI64>,
        }
        struct OpaqueAttachment {
            presentation: Arc<dyn RoutedServerPresentation>,
            release_count: Arc<AtomicI64>,
        }
        impl RoutedServerServiceHost for OpaqueServices {
            fn attach_client(
                &self,
                presentation: Arc<dyn RoutedServerPresentation>,
                _context: Context,
            ) -> BoxFuture<'static, Result<Arc<dyn RoutedServerServiceAttachment>, OperationError>>
            {
                let attachment: Arc<dyn RoutedServerServiceAttachment> =
                    Arc::new(OpaqueAttachment {
                        presentation,
                        release_count: self.release_count.clone(),
                    });
                Box::pin(async move { Ok(attachment) })
            }
        }
        impl RoutedServerServiceAttachment for OpaqueAttachment {
            fn invoke_service(
                &self,
                call: ServiceCall,
                _publish: PublishCallback,
                context: Context,
            ) -> BoxFuture<'static, Result<Option<serde_json::Value>, OperationError>> {
                let presentation = self.presentation.clone();
                Box::pin(async move {
                    if call.instance.is_some() || call.service_id != "pi.session-management" {
                        return Err(OperationError::Other("Unexpected service".to_string()));
                    }
                    if call.member == "attach" && call.args.len() == 1 && call.args[0].is_string() {
                        let session_id = call.args[0].as_str().expect("checked").to_string();
                        presentation.attach_session(session_id, context).await?;
                        return Ok(Some(serde_json::Value::Null));
                    }
                    if call.member == "detach" && call.args.is_empty() {
                        presentation.detach_session(context).await?;
                        return Ok(Some(serde_json::Value::Null));
                    }
                    Err(OperationError::Other(
                        "Unexpected service member".to_string(),
                    ))
                })
            }

            fn release(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
                let count = self.release_count.clone();
                Box::pin(async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        }

        struct DelegatingHost {
            backing: Arc<super::testing::host::TestServerHost>,
            services: Arc<dyn RoutedServerServiceHost>,
        }
        impl ServerHost for DelegatingHost {
            fn server_services(&self) -> Arc<dyn RoutedServerServiceHost> {
                self.services.clone()
            }
            fn resolve_session(
                &self,
                session_id: String,
                context: Context,
            ) -> BoxFuture<'static, Result<SessionMetadata, OperationError>> {
                let backing = self.backing.clone();
                Box::pin(async move { backing.resolve_session(session_id, context).await })
            }
            fn open_session(
                &self,
                metadata: SessionMetadata,
                context: Context,
            ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionHandle>, OperationError>>
            {
                let backing = self.backing.clone();
                Box::pin(async move { backing.open_session(metadata, context).await })
            }
        }

        let host: Arc<dyn ServerHost> = Arc::new(DelegatingHost {
            backing: backing.clone(),
            services: Arc::new(OpaqueServices {
                release_count: release_count.clone(),
            }),
        });
        let server = create_server(host);
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        let attached = client.next(Arc::new(|message| {
            matches!(
                message,
                crate::protocol::protocol::ServerMessage::Attachment(envelope)
                    if envelope.attachment.is_some()
            )
        }));
        assert_ok(&envelope_of(client.attach(SERVER_ID, "session-1").await).await);
        match attached.await.unwrap() {
            crate::protocol::protocol::ServerMessage::Attachment(envelope) => {
                let target = envelope.attachment.as_ref().expect("attached");
                assert_eq!(target.session_id, "session-1");
                assert!(!target.attachment_id.is_empty());
            }
            other => panic!("expected attachment, got {other:?}"),
        }
        assert_eq!(
            backing
                .latest_harness("session-1")
                .unwrap()
                .attached_clients(),
            1
        );

        let detached = client.next(Arc::new(|message| {
            matches!(
                message,
                crate::protocol::protocol::ServerMessage::Attachment(envelope)
                    if envelope.attachment.is_none()
            )
        }));
        assert_ok(
            &envelope_of(
                client
                    .request_service(
                        crate::protocol::protocol::RpcTarget::Server(
                            crate::protocol::protocol::ServerTarget {
                                server_id: SERVER_ID.to_string(),
                            },
                        ),
                        JsonValue::object(vec![
                            (
                                "serviceId".to_string(),
                                JsonValue::string("pi.session-management"),
                            ),
                            ("member".to_string(), JsonValue::string("detach")),
                            ("args".to_string(), JsonValue::Array(Vec::new())),
                        ]),
                        None,
                    )
                    .await,
            )
            .await,
        );
        match detached.await.unwrap() {
            crate::protocol::protocol::ServerMessage::Attachment(envelope) => {
                assert!(envelope.attachment.is_none(), "expected null attachment");
            }
            other => panic!("expected attachment, got {other:?}"),
        }
        assert_eq!(
            backing
                .latest_harness("session-1")
                .unwrap()
                .attached_clients(),
            0
        );
        client.close().await.unwrap();
        wait_until(|| release_count.load(Ordering::SeqCst) == 1).await;
        server.close().await.unwrap();
    });
}

#[test]
fn permits_multiple_client_attachments_per_session() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (first, _f1) = connect(&server);
        let (second, _f2) = connect(&server);
        first.hello().await.unwrap();
        second.hello().await.unwrap();

        assert_ok(&envelope_of(first.attach(SERVER_ID, "session-1").await).await);
        assert_ok(&envelope_of(first.attach(SERVER_ID, "session-1").await).await);
        assert_eq!(
            host.latest_harness("session-1").unwrap().attached_clients(),
            1
        );
        assert_ok(&envelope_of(second.attach(SERVER_ID, "session-1").await).await);
        assert_eq!(host.harnesses("session-1").len(), 1);
        assert_eq!(
            host.latest_harness("session-1").unwrap().attached_clients(),
            2
        );

        first.close().await.unwrap();
        wait_until(|| host.latest_harness("session-1").unwrap().attached_clients() == 1).await;
        assert_ok(&envelope_of(second.attach(SERVER_ID, "session-1").await).await);
        server.close().await.unwrap();
    });
}

#[test]
fn clears_connection_ownership_when_attachment_release_fails() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let observer = {
            let errors = errors.clone();
            Arc::new(move |error: &OperationError| {
                errors.lock().unwrap().push(error.message().to_string());
            }) as super::types::ErrorObserver
        };
        let server = create_server_with_options(host.clone(), |options| options.on_error(observer));
        let (first, _f1) = connect(&server);
        let (second, _f2) = connect(&server);
        first.hello().await.unwrap();
        second.hello().await.unwrap();
        envelope_of(first.attach(SERVER_ID, "session-1").await).await;
        let harness = host.latest_harness("session-1").unwrap();
        harness
            .set_fail_attachment_release(Some(OperationError::Other("release failed".to_string())));

        first.close().await.unwrap();
        wait_until(|| harness.attachment_release_count() == 1).await;
        wait_until(|| {
            errors
                .lock()
                .unwrap()
                .contains(&"release failed".to_string())
        })
        .await;
        harness.set_fail_attachment_release(None);
        assert_ok(&envelope_of(second.attach(SERVER_ID, "session-1").await).await);
        server.close().await.unwrap();
    });
}

#[test]
fn requires_the_requesting_client_to_hold_the_targeted_session_attachment() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        host.seed("session-2");
        let server = create_server(host.clone());
        let (attached, _fa) = connect(&server);
        let (unattached, _fu) = connect(&server);
        attached.hello().await.unwrap();
        unattached.hello().await.unwrap();

        assert_error_code(
            &envelope_of(
                unattached
                    .request_session_service(
                        SERVER_ID,
                        "session-1",
                        test_session_call("run", Vec::new()),
                        None,
                    )
                    .await,
            )
            .await,
            "session_not_attached",
        );
        envelope_of(attached.attach(SERVER_ID, "session-1").await).await;
        assert_error_code(
            &envelope_of(
                attached
                    .request_session_service(
                        SERVER_ID,
                        "session-2",
                        test_session_call("run", Vec::new()),
                        None,
                    )
                    .await,
            )
            .await,
            "session_not_attached",
        );
        let envelope = envelope_of(
            attached
                .request_session_service(
                    SERVER_ID,
                    "session-1",
                    test_session_call("run", vec![JsonValue::string("Hello")]),
                    None,
                )
                .await,
        )
        .await;
        assert_eq!(
            assert_ok(&envelope),
            Some(JsonValue::from_serde_json(
                &serde_json::json!({ "ok": true })
            ))
        );
        assert_eq!(
            host.latest_harness("session-1").unwrap().service_calls(),
            vec![expected_session_call(
                "run",
                vec![serde_json::json!("Hello")]
            )]
        );
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_a_stale_attachment_route_after_switching_sessions() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        host.seed("session-2");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        envelope_of(client.attach(SERVER_ID, "session-1").await).await;
        let first_attachment_id = latest_attachment_id(&client, "session-1");
        envelope_of(client.attach(SERVER_ID, "session-2").await).await;

        assert_error_code(
            &envelope_of(
                client
                    .request_service(
                        crate::protocol::protocol::RpcTarget::Session(
                            crate::protocol::protocol::SessionTarget {
                                server_id: SERVER_ID.to_string(),
                                session_id: "session-1".to_string(),
                                attachment_id: first_attachment_id,
                            },
                        ),
                        test_session_call("run", vec![JsonValue::string("stale")]),
                        None,
                    )
                    .await,
            )
            .await,
            "session_not_attached",
        );
        assert!(host
            .latest_harness("session-1")
            .unwrap()
            .service_calls()
            .is_empty());
        server.close().await.unwrap();
    });
}

#[test]
fn preserves_opaque_service_results_and_bounds_adapter_defects() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        envelope_of(client.attach(SERVER_ID, "session-1").await).await;
        let harness = host.latest_harness("session-1").unwrap();
        harness.set_next_service_result(Some(serde_json::json!({
            "accepted": false,
            "reason": "closed"
        })));
        let envelope = envelope_of(
            client
                .request_session_service(
                    SERVER_ID,
                    "session-1",
                    test_session_call("run", Vec::new()),
                    None,
                )
                .await,
        )
        .await;
        assert_eq!(
            assert_ok(&envelope).expect("opaque result").to_serde_json(),
            serde_json::json!({ "accepted": false, "reason": "closed" })
        );

        harness.set_next_service_error(Some(OperationError::Other(
            "private adapter detail".to_string(),
        )));
        let envelope = envelope_of(
            client
                .request_session_service(
                    SERVER_ID,
                    "session-1",
                    test_session_call("run", Vec::new()),
                    None,
                )
                .await,
        )
        .await;
        let error = assert_error_code(&envelope, "internal_error");
        assert_eq!(error.message, "Internal server error");
        server.close().await.unwrap();
    });
}

#[test]
fn admits_concurrent_service_calls_to_the_attached_session() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        envelope_of(client.attach(SERVER_ID, "session-1").await).await;
        let harness = host.latest_harness("session-1").unwrap();
        let gate = harness.gate_next_service_call();
        let first = client.request_session_service(
            SERVER_ID,
            "session-1",
            test_session_call("run", vec![JsonValue::string("first")]),
            None,
        );
        fire(&first);
        gate.entered.promise().await;
        let second = client.request_session_service(
            SERVER_ID,
            "session-1",
            test_session_call("run", vec![JsonValue::string("second")]),
            None,
        );

        assert_ok(&envelope_of(second.await).await);
        assert_eq!(
            harness.service_calls(),
            vec![
                expected_session_call("run", vec![serde_json::json!("first")]),
                expected_session_call("run", vec![serde_json::json!("second")]),
            ]
        );
        gate.release.resolve(());
        assert_ok(&envelope_of(first.await).await);
        server.close().await.unwrap();
    });
}

#[test]
fn keeps_attachment_demand_until_an_accepted_service_call_settles_after_disconnect() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        envelope_of(client.attach(SERVER_ID, "session-1").await).await;
        let harness = host.latest_harness("session-1").unwrap();
        let gate = harness.gate_next_service_call();
        let calling = client.request_session_service(
            SERVER_ID,
            "session-1",
            test_session_call("run", Vec::new()),
            None,
        );
        fire(&calling);
        gate.entered.promise().await;

        client.close().await.unwrap();
        assert_eq!(harness.attached_clients(), 1);
        gate.release.resolve(());
        let error = response_of(calling.await).unwrap_err();
        assert!(
            error.message().to_lowercase().contains("closed"),
            "expected closed rejection, got {error}"
        );
        wait_until(|| harness.attached_clients() == 0).await;
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_requests_addressed_to_another_server_before_repository_access() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();

        assert_error_code(
            &envelope_of(
                client
                    .attach("00000000-0000-4000-8000-000000000002", "session-1")
                    .await,
            )
            .await,
            "wrong_server",
        );
        assert_eq!(host.harness_session_count(), 0);
        server.close().await.unwrap();
    });
}

#[test]
fn reports_an_unknown_session_without_creating_a_harness() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();

        assert_error_code(
            &envelope_of(client.attach(SERVER_ID, "missing").await).await,
            "session_not_found",
        );
        assert_eq!(host.harness_session_count(), 0);
        server.close().await.unwrap();
    });
}

#[test]
fn rejects_an_ambiguous_session_id_without_creating_a_harness() {
    tokio_test().block_on(async {
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
                Box::pin(async { Err(OperationError::Server(ServerError::session_ambiguous())) })
            }
            fn open_session(
                &self,
                _metadata: SessionMetadata,
                _context: Context,
            ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionHandle>, OperationError>>
            {
                Box::pin(async {
                    Err(OperationError::Other(
                        "must not create a Harness for an ambiguous session".to_string(),
                    ))
                })
            }
        }
        let server = create_server(Arc::new(AmbiguousHost));
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();

        assert_error_code(
            &envelope_of(client.attach(SERVER_ID, "duplicate").await).await,
            "session_ambiguous",
        );
        server.close().await.unwrap();
    });
}

#[test]
fn invalidates_a_terminated_harness_handle_and_allows_a_later_attach() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        envelope_of(client.attach(SERVER_ID, "session-1").await).await;
        let first_harness = host.latest_harness("session-1").unwrap();

        first_harness
            .terminate(OperationError::Other("worker crashed".to_string()))
            .await;
        first_harness.terminated().await;
        wait_until(|| first_harness.attached_clients() == 0).await;
        assert_eq!(first_harness.attachment_release_count(), 1);

        assert_ok(&envelope_of(client.attach(SERVER_ID, "session-1").await).await);
        assert_eq!(host.harnesses("session-1").len(), 2);
        server.close().await.unwrap();
    });
}

#[test]
fn connection_loss_releases_its_attachment_while_server_shutdown_closes_the_harness() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        envelope_of(client.attach(SERVER_ID, "session-1").await).await;
        let harness = host.latest_harness("session-1").unwrap();

        client.close().await.unwrap();
        wait_until(|| harness.attached_clients() == 0).await;
        assert_eq!(harness.close_count(), 0);
        server.close().await.unwrap();
        assert_eq!(harness.close_count(), 1);
    });
}

// ---------------------------------------------------------------------------
// routed Session acquisition failures
// ---------------------------------------------------------------------------

/// Custom handle for the concurrent-termination scenario (upstream object
/// literal with a `terminated` promise and a gated `attachClient`).
struct GatedHandle {
    terminated: Arc<super::testing::host::Deferred<Option<OperationError>>>,
    acquiring: Arc<super::testing::host::Deferred<()>>,
    continue_acquiring: Arc<super::testing::host::Deferred<()>>,
    release_count: Arc<AtomicI64>,
}

impl RoutedSessionHandle for GatedHandle {
    fn attach_client(
        &self,
        _context: Context,
    ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionAttachment>, OperationError>> {
        let acquiring = self.acquiring.clone();
        let continue_acquiring = self.continue_acquiring.clone();
        let release_count = self.release_count.clone();
        Box::pin(async move {
            acquiring.resolve(());
            continue_acquiring.promise().await;
            Ok(Arc::new(CountingAttachment { release_count }) as Arc<dyn RoutedSessionAttachment>)
        })
    }

    fn terminated(&self) -> Option<TerminatedSignal> {
        Some(self.terminated.promise())
    }

    fn close(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        Box::pin(async { Ok(()) })
    }
}

struct CountingAttachment {
    release_count: Arc<AtomicI64>,
}

impl RoutedSessionAttachment for CountingAttachment {
    fn invoke_service(
        &self,
        _call: ServiceCall,
        _publish: PublishCallback,
        _context: Context,
    ) -> BoxFuture<'static, Result<Option<serde_json::Value>, OperationError>> {
        Box::pin(async { Ok(None) })
    }

    fn release(&self, _context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        let count = self.release_count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[test]
fn releases_a_lease_acquired_concurrently_with_harness_termination() {
    tokio_test().block_on(async {
        let metadata = SessionMetadata::new("session-1");
        let acquiring = Arc::new(super::testing::host::Deferred::new());
        let continue_acquiring = Arc::new(super::testing::host::Deferred::new());
        let terminated: Arc<super::testing::host::Deferred<Option<OperationError>>> =
            Arc::new(super::testing::host::Deferred::new());
        let release_count = Arc::new(AtomicI64::new(0));

        struct AcquisitionHost {
            metadata: SessionMetadata,
            terminated: Arc<super::testing::host::Deferred<Option<OperationError>>>,
            acquiring: Arc<super::testing::host::Deferred<()>>,
            continue_acquiring: Arc<super::testing::host::Deferred<()>>,
            release_count: Arc<AtomicI64>,
        }
        impl ServerHost for AcquisitionHost {
            fn server_services(&self) -> Arc<dyn RoutedServerServiceHost> {
                create_test_server_services()
            }
            fn resolve_session(
                &self,
                _session_id: String,
                _context: Context,
            ) -> BoxFuture<'static, Result<SessionMetadata, OperationError>> {
                let metadata = self.metadata.clone();
                Box::pin(async move { Ok(metadata) })
            }
            fn open_session(
                &self,
                _metadata: SessionMetadata,
                _context: Context,
            ) -> BoxFuture<'static, Result<Arc<dyn RoutedSessionHandle>, OperationError>>
            {
                let handle: Arc<dyn RoutedSessionHandle> = Arc::new(GatedHandle {
                    terminated: self.terminated.clone(),
                    acquiring: self.acquiring.clone(),
                    continue_acquiring: self.continue_acquiring.clone(),
                    release_count: self.release_count.clone(),
                });
                Box::pin(async move { Ok(handle) })
            }
        }

        let host: Arc<dyn ServerHost> = Arc::new(AcquisitionHost {
            metadata,
            terminated: terminated.clone(),
            acquiring: acquiring.clone(),
            continue_acquiring: continue_acquiring.clone(),
            release_count: release_count.clone(),
        });
        let server = create_server(host);
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        let attach = client.attach(SERVER_ID, "session-1");
        fire(&attach);
        acquiring.promise().await;

        terminated.resolve(Some(OperationError::Other("worker crashed".to_string())));
        continue_acquiring.resolve(());
        let error = assert_error_code(&envelope_of(attach.await).await, "server_draining");
        assert_eq!(error.code, "server_draining");
        assert_eq!(release_count.load(Ordering::SeqCst), 1);
        server.close().await.unwrap();
    });
}

#[test]
fn shares_a_harness_creation_failure_releases_the_session_and_allows_a_later_retry() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        host.set_next_open_session_error(Some(OperationError::Other(
            "Harness creation failed".to_string(),
        )));
        let server = create_server(host.clone());
        let (first, _f1) = connect(&server);
        let (second, _f2) = connect(&server);
        first.hello().await.unwrap();
        second.hello().await.unwrap();
        let gate = host.gate_next_open_session();

        let first_attach = first.attach(SERVER_ID, "session-1");
        fire(&first_attach);
        gate.entered.promise().await;
        let second_attach = second.attach(SERVER_ID, "session-1");
        fire(&second_attach);
        // The second attach must have joined the in-flight opening before it
        // settles (upstream: the sendMessage microtask precedes the release).
        settle().await;
        gate.release.resolve(());

        assert_error_code(&envelope_of(first_attach.await).await, "internal_error");
        assert_error_code(&envelope_of(second_attach.await).await, "internal_error");
        assert_eq!(host.open_session_count(), 1);

        assert_ok(&envelope_of(first.attach(SERVER_ID, "session-1").await).await);
        assert_eq!(host.open_session_count(), 2);
        assert_eq!(host.harnesses("session-1").len(), 1);
        server.close().await.unwrap();
    });
}

#[test]
fn closes_a_harness_acquired_while_server_shutdown_is_in_progress() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        let gate = host.gate_next_open_session();
        let attach = client.attach(SERVER_ID, "session-1");
        fire(&attach);
        gate.entered.promise().await;
        let closing = server.close();
        gate.release.resolve(());

        closing.await.unwrap();
        let error = response_of(attach.await).unwrap_err();
        assert!(
            error.message().to_lowercase().contains("closed"),
            "expected closed rejection, got {error}"
        );
        assert_eq!(host.latest_harness("session-1").unwrap().close_count(), 1);
    });
}

#[test]
fn fails_shutdown_when_an_in_flight_acquisition_cannot_release_its_harness() {
    tokio_test().block_on(async {
        let host = super::testing::host::TestServerHost::new();
        host.seed("session-1");
        host.set_next_harness_close_error(Some(OperationError::Other("close failed".to_string())));
        let server = create_server(host.clone());
        let (client, _frames) = connect(&server);
        client.hello().await.unwrap();
        let gate = host.gate_next_open_session();
        let attach = client.attach(SERVER_ID, "session-1");
        fire(&attach);
        gate.entered.promise().await;

        let closing = server.close();
        gate.release.resolve(());

        let error = closing.await.unwrap_err();
        assert!(
            error.message().contains("Failed to close routed Sessions"),
            "unexpected shutdown error: {error}"
        );
        let closed_error = server.closed().await.unwrap_err();
        assert!(
            closed_error
                .message()
                .contains("Failed to close routed Sessions"),
            "unexpected closed error: {closed_error}"
        );
        let attach_error = response_of(attach.await).unwrap_err();
        assert!(attach_error.message().to_lowercase().contains("closed"));
        assert_eq!(host.latest_harness("session-1").unwrap().close_count(), 1);

        // Upstream afterEach replacement: close the harness for the test double.
        host.latest_harness("session-1")
            .unwrap()
            .close(Context::background())
            .await
            .unwrap();
    });
}
