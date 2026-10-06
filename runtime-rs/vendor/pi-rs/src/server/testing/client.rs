//! Port of `packages/server/src/testing/client.ts` (183 lines, SHA256
//! `2d240732193839d4105183a089d6b21e7170fff6c933b9a7e76832d0c960fedf`): the
//! `ProtocolTestClient` loopback driver and the `WireChannel` seam.
//!
//! Upstream waiters are promise resolvers in a `Set`; the port registers a
//! oneshot per waiter (predicates are `Arc` closures). `next()` stays a
//! synchronous registration returning a shared future, so tests can
//! register, then send, then await — upstream's
//! `const response = this.next(...); void this.sendMessage(...)` order.
//! The send itself is dispatched eagerly at registration time (a spawned
//! task stands in for the upstream fire-and-forget promise; disclosed seam
//! S-D: progress requires a live runtime).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::oneshot;

use crate::protocol::codec::{encode_client_message, ServerMessageDecoder};
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{
    ClientHello, ClientMessage, RequestEnvelope, ResponseEnvelope, RpcTarget, ServerMessage,
    SessionTarget, PROTOCOL_VERSION,
};

use super::super::errors::OperationError;
use super::host::Deferred;

/// Upstream `WireChannel` (`client.ts:21-25`).
pub trait WireChannel: Send + Sync + 'static {
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>>;
    fn send_fragmented(
        &self,
        chunk: Vec<u8>,
        split_at: usize,
    ) -> BoxFuture<'static, Result<(), OperationError>>;
    fn close(&self) -> BoxFuture<'static, Result<(), OperationError>>;
}

/// A registered waiter predicate (upstream the closure in the waiter set).
pub type MessagePredicate = Arc<dyn Fn(&ServerMessage) -> bool + Send + Sync>;

/// The shared future returned by [`ProtocolTestClient::next`].
pub type SharedMessage = Shared<BoxFuture<'static, Result<ServerMessage, OperationError>>>;

struct Waiter {
    predicate: MessagePredicate,
    tx: oneshot::Sender<Result<ServerMessage, OperationError>>,
}

struct ClientShared {
    channel: Arc<dyn WireChannel>,
    decoder: Mutex<ServerMessageDecoder>,
    messages: Mutex<Vec<ServerMessage>>,
    waiters: Mutex<Vec<Waiter>>,
    closed: Deferred<()>,
    closed_flag: AtomicBool,
    request_sequence: AtomicU64,
    attachment: Mutex<Option<SessionTarget>>,
}

/// Upstream `ProtocolTestClient` (`client.ts:27-150`). Cheap to clone.
#[derive(Clone)]
pub struct ProtocolTestClient(Arc<ClientShared>);

impl ProtocolTestClient {
    pub fn new(channel: Arc<dyn WireChannel>) -> ProtocolTestClient {
        ProtocolTestClient(Arc::new(ClientShared {
            channel,
            decoder: Mutex::new(ServerMessageDecoder::new(None).expect("default decoder options")),
            messages: Mutex::new(Vec::new()),
            waiters: Mutex::new(Vec::new()),
            closed: Deferred::new(),
            closed_flag: AtomicBool::new(false),
            request_sequence: AtomicU64::new(0),
            attachment: Mutex::new(None),
        }))
    }

    /// `client.ts:41-43` `closed`.
    pub fn closed(&self) -> bool {
        self.0.closed_flag.load(Ordering::SeqCst)
    }

    /// `client.ts:45-49` `hello(version = PROTOCOL_VERSION)`.
    pub fn hello(&self) -> SharedMessage {
        self.hello_with_version(PROTOCOL_VERSION)
    }

    /// The `hello(version)` overload.
    pub fn hello_with_version(&self, version: u64) -> SharedMessage {
        let response = self.next(Arc::new(|message| {
            matches!(
                message,
                ServerMessage::Hello(_) | ServerMessage::HelloError(_)
            )
        }));
        let sent = tokio::spawn(self.send_message(&ClientMessage::Hello(ClientHello { version })));
        Box::pin(async move {
            sent.await.map_err(|_| {
                OperationError::Other("test client send task aborted".to_string())
            })??;
            response.await
        })
        .boxed()
        .shared()
    }

    /// `client.ts:51-61` `requestService(target, call, id)`. The returned
    /// future resolves with the response envelope message.
    pub fn request_service(
        &self,
        target: RpcTarget,
        call: JsonValue,
        id: Option<String>,
    ) -> SharedMessage {
        let id = id.unwrap_or_else(|| {
            format!(
                "request-{}",
                self.0.request_sequence.fetch_add(1, Ordering::SeqCst) + 1
            )
        });
        let response_id = id.clone();
        let response = self.next(Arc::new(move |message| match message {
            ServerMessage::Response(response) => response.id == response_id,
            _ => false,
        }));
        let sent = tokio::spawn(self.send_message(&ClientMessage::Request(RequestEnvelope {
            id,
            target,
            call,
        })));
        Box::pin(async move {
            sent.await.map_err(|_| {
                OperationError::Other("test client send task aborted".to_string())
            })??;
            response.await
        })
        .boxed()
        .shared()
    }

    /// `client.ts:63-68` `attach(serverId, sessionId)`.
    pub fn attach(&self, server_id: &str, session_id: &str) -> SharedMessage {
        self.request_service(
            RpcTarget::Server(crate::protocol::protocol::ServerTarget {
                server_id: server_id.to_string(),
            }),
            session_management_call("attach", vec![JsonValue::string(session_id)]),
            None,
        )
    }

    /// `client.ts:70-82` `requestSessionService(serverId, sessionId, call, id)`.
    pub fn request_session_service(
        &self,
        server_id: &str,
        session_id: &str,
        call: JsonValue,
        id: Option<String>,
    ) -> SharedMessage {
        let attachment = self.0.attachment.lock().unwrap().clone();
        let target = match attachment {
            Some(attachment) if attachment.session_id == session_id => {
                RpcTarget::Session(attachment)
            }
            _ => RpcTarget::Session(SessionTarget {
                server_id: server_id.to_string(),
                session_id: session_id.to_string(),
                attachment_id: "missing-attachment".to_string(),
            }),
        };
        self.request_service(target, call, id)
    }

    /// `client.ts:84-86` `sendMessage(message)`.
    pub fn send_message(
        &self,
        message: &ClientMessage,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        let frame = encode_client_message(message, None).expect("test client sends valid messages");
        self.send_bytes(&frame)
    }

    /// `client.ts:88-90` `sendBytes(chunk)`.
    pub fn send_bytes(&self, chunk: &[u8]) -> BoxFuture<'static, Result<(), OperationError>> {
        let channel = self.0.channel.clone();
        let chunk = chunk.to_vec();
        Box::pin(async move { channel.send(chunk).await })
    }

    /// `client.ts:92-94` `sendFragmentedMessage(message, splitAt)`.
    pub fn send_fragmented_message(
        &self,
        message: &ClientMessage,
        split_at: usize,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        let frame = encode_client_message(message, None).expect("test client sends valid messages");
        let channel = self.0.channel.clone();
        Box::pin(async move { channel.send_fragmented(frame, split_at).await })
    }

    /// `client.ts:96-98` `next(predicate)`.
    pub fn next(&self, predicate: MessagePredicate) -> SharedMessage {
        self.next_from(0, predicate)
    }

    /// `client.ts:100-105` `nextFrom(index, predicate)`.
    pub fn next_from(&self, index: usize, predicate: MessagePredicate) -> SharedMessage {
        {
            let messages = self
                .0
                .messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(found) = messages
                .iter()
                .skip(index)
                .find(|message| predicate(message))
            {
                let found = found.clone();
                return Box::pin(async move { Ok(found) }).boxed().shared();
            }
        }
        if self.closed() {
            return Box::pin(async move {
                Err(OperationError::Other("Wire client is closed".to_string()))
            })
            .boxed()
            .shared();
        }
        let (tx, rx) = oneshot::channel();
        self.0
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Waiter { predicate, tx });
        Box::pin(async move {
            match rx.await {
                Ok(result) => result,
                // The client failed or was dropped while waiting.
                Err(_) => Err(OperationError::Other("Wire connection closed".to_string())),
            }
        })
        .boxed()
        .shared()
    }

    /// `client.ts:107-109` `waitForClose()`.
    pub fn wait_for_close(&self) -> BoxFuture<'static, Result<(), OperationError>> {
        let promise = self.0.closed.promise();
        Box::pin(async move {
            promise.await;
            Ok(())
        })
    }

    /// `client.ts:111-113` `close()`.
    pub fn close(&self) -> BoxFuture<'static, Result<(), OperationError>> {
        let channel = self.0.channel.clone();
        Box::pin(async move { channel.close().await })
    }

    /// `client.ts:115-137` `receive(chunk)`.
    pub fn receive(&self, chunk: &[u8]) {
        let decoded = {
            let mut decoder = self
                .0
                .decoder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            decoder.push(chunk)
        };
        let messages = match decoded {
            Ok(messages) => messages,
            Err(error) => {
                self.fail(OperationError::Protocol(error.message().to_string()));
                return;
            }
        };
        for message in messages {
            if let ServerMessage::Attachment(attachment) = &message {
                *self.0.attachment.lock().unwrap() = attachment.attachment.clone();
            }
            self.0
                .messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(message.clone());
            let mut waiters = self
                .0
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut retained = Vec::with_capacity(waiters.len());
            for waiter in waiters.drain(..) {
                if (waiter.predicate)(&message) {
                    let _ = waiter.tx.send(Ok(message.clone()));
                } else {
                    retained.push(waiter);
                }
            }
            *waiters = retained;
        }
    }

    /// `client.ts:139-144` `markClosed()`.
    pub fn mark_closed(&self) {
        if self.closed() {
            return;
        }
        self.0.closed_flag.store(true, Ordering::SeqCst);
        self.0.closed.resolve(());
        self.fail(OperationError::Other("Wire connection closed".to_string()));
    }

    /// `client.ts:146-149` `fail(error)`.
    pub fn fail(&self, error: OperationError) {
        let mut waiters = self
            .0
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for waiter in waiters.drain(..) {
            let _ = waiter.tx.send(Err(error.clone()));
        }
    }

    /// The received messages so far (`messages` field).
    pub fn messages(&self) -> Vec<ServerMessage> {
        self.0
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn session_management_call(member: &str, args: Vec<JsonValue>) -> JsonValue {
    JsonValue::object(vec![
        (
            "serviceId".to_string(),
            JsonValue::string("pi.session-management"),
        ),
        ("member".to_string(), JsonValue::string(member)),
        ("args".to_string(), JsonValue::Array(args)),
    ])
}

/// A chord-shaped call value for tests (`{ serviceId, member, args }`).
pub fn session_call(member: &str, args: Vec<JsonValue>) -> JsonValue {
    JsonValue::object(vec![
        ("serviceId".to_string(), JsonValue::string("test.session")),
        ("member".to_string(), JsonValue::string(member)),
        ("args".to_string(), JsonValue::Array(args)),
    ])
}

/// The `ResponseEnvelope` view of a response waiter result (upstream
/// `requestService` returns `ResponseEnvelope`).
pub fn response_of(
    message: Result<ServerMessage, OperationError>,
) -> Result<ResponseEnvelope, OperationError> {
    match message? {
        ServerMessage::Response(response) => Ok(response),
        other => Err(OperationError::Other(format!(
            "expected response, got {}",
            message_kind(&other)
        ))),
    }
}

fn message_kind(message: &ServerMessage) -> &'static str {
    match message {
        ServerMessage::Hello(_) => "hello",
        ServerMessage::HelloError(_) => "hello_error",
        ServerMessage::Response(_) => "response",
        ServerMessage::ServiceUpdate(_) => "service_update",
        ServerMessage::Attachment(_) => "attachment",
    }
}
