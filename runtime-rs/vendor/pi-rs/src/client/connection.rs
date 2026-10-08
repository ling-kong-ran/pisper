//! Port of `packages/client/src/connection.ts` (245 lines): the byte-level
//! connection state machine — transport lifecycle, client/server hello
//! handshake, framed message decode pump, and failure transitions.
//!
//! Upstream drives the lifecycle from the JS event loop with promise
//! resolvers; the port keeps the same shape with a mutex-guarded lifecycle,
//! oneshot handshake resolvers, and a per-attempt transport pump. State
//! transitions, message routing, error texts, and the `send`/`close`
//! sequencing are pinned against the node oracle
//! (`tests/fixtures/client_oracle/oracle.out.txt`).

use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::protocol::codec::{
    encode_client_message, ProtocolValidationError, ServerMessageDecoder,
};
use crate::protocol::framing::{FrameDecoderOptions, DEFAULT_MAX_FRAME_LENGTH};
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{
    ClientHello, ClientMessage, ServerHello, ServerMessage, PROTOCOL_VERSION,
};

use super::errors::{to_disconnected_error, ClientError, DisconnectedError, ServerError};
use super::transport::{ByteTransport, ByteTransportFactory, ByteTransportHandlers};
use super::types::{ConnectionState, ConnectionStateChange};

/// `connection.ts:15`.
const MAX_UINT32: u64 = u32::MAX as u64;

type HandshakeResolver = oneshot::Sender<Result<ServerHello, ClientError>>;

/// `connection.ts:17-21`.
struct ActiveConnection {
    id: u64,
    decoder: ServerMessageDecoder,
    transport: Option<Arc<dyn ByteTransport>>,
}

/// `connection.ts:23-30`. The handshake resolver stays installed while the
/// lifecycle is live, so a failure during the connected transition (or the
/// handshake/state-change callbacks) rejects the pending `connect()`.
enum Lifecycle {
    Disconnected,
    Connecting(Box<ActiveConnection>, Option<HandshakeResolver>),
    Connected(Box<ActiveConnection>, Option<HandshakeResolver>),
}

/// `connection.ts:32-39`.
pub struct ConnectionOptions {
    pub transport_factory: ByteTransportFactory,
    pub server_id: String,
    pub max_frame_length: Option<u64>,
    pub on_handshake: Arc<dyn Fn(&ServerHello) + Send + Sync>,
    pub on_message: Arc<dyn Fn(ServerMessage) + Send + Sync>,
    pub on_state_change: Arc<dyn Fn(ConnectionStateChange) + Send + Sync>,
}

struct ConnectionShared {
    options: ConnectionOptions,
    max_frame_length: u64,
    lifecycle: Mutex<Lifecycle>,
    sequence: std::sync::atomic::AtomicU64,
}

/// Upstream `Connection` (`connection.ts:41-245`). Cheap to clone; spawned
/// tasks hold the shared core through weak handles.
#[derive(Clone)]
pub struct Connection(Arc<ConnectionShared>);

fn frame_options(max_frame_length: u64) -> Option<FrameDecoderOptions> {
    Some(FrameDecoderOptions::default().with_max_frame_length(max_frame_length))
}

impl Connection {
    /// Upstream constructor (`connection.ts:47-57`): validates
    /// `maxFrameLength` (upstream `TypeError`).
    pub fn new(options: ConnectionOptions) -> Result<Connection, ClientError> {
        let max_frame_length = options.max_frame_length.unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
        // Upstream: `Number.isSafeInteger(v) && v > 0 && v <= MAX_UINT32`;
        // `u32::MAX` is far below the safe-integer bound, so the
        // representable violations are 0 and `> MAX_UINT32`.
        if max_frame_length == 0 || max_frame_length > MAX_UINT32 {
            return Err(ClientError::Type(format!(
                "Client maxFrameLength must be an integer between 1 and {MAX_UINT32}"
            )));
        }
        Ok(Connection(Arc::new(ConnectionShared {
            max_frame_length,
            lifecycle: Mutex::new(Lifecycle::Disconnected),
            sequence: std::sync::atomic::AtomicU64::new(0),
            options,
        })))
    }

    /// `connection.ts:59-61`.
    pub fn state(&self) -> ConnectionState {
        self.0.state()
    }

    /// `connection.ts:63-65`.
    pub fn max_frame_length(&self) -> u64 {
        self.0.max_frame_length
    }

    /// `connection.ts:67-91`.
    pub fn connect(&self) -> oneshot::Receiver<Result<ServerHello, ClientError>> {
        self.0.connect()
    }

    /// `connection.ts:93-96`.
    pub fn disconnect(&self, reason: ClientError) {
        self.0.fail_and_close(reason);
    }

    /// `connection.ts:98-100` (upstream `fail` delegates to `#failAndClose`).
    pub fn fail(&self, error: ClientError) {
        self.0.fail_and_close(error);
    }

    /// `connection.ts:102-118`.
    pub fn send(&self, frame: Vec<u8>) -> Result<(), ClientError> {
        self.0.send(frame)
    }
}
impl ConnectionShared {
    fn state(&self) -> ConnectionState {
        let lifecycle = self.lifecycle.lock().unwrap();
        match &*lifecycle {
            Lifecycle::Disconnected => ConnectionState::Disconnected,
            Lifecycle::Connecting(..) => ConnectionState::Connecting,
            Lifecycle::Connected(..) => ConnectionState::Connected,
        }
    }

    fn connect(self: &Arc<Self>) -> oneshot::Receiver<Result<ServerHello, ClientError>> {
        let (resolver, pending) = oneshot::channel();
        {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            if !matches!(*lifecycle, Lifecycle::Disconnected) {
                let state = match &*lifecycle {
                    Lifecycle::Disconnected => ConnectionState::Disconnected,
                    Lifecycle::Connecting(..) => ConnectionState::Connecting,
                    Lifecycle::Connected(..) => ConnectionState::Connected,
                };
                let _ = resolver.send(Err(ClientError::Disconnected(
                    DisconnectedError::with_message(format!("Client is already {state}")),
                )));
                return pending;
            }
            let id = self
                .sequence
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            let decoder = match ServerMessageDecoder::new(frame_options(self.max_frame_length)) {
                Ok(decoder) => decoder,
                // Unreachable: the constructor validated the same bound the
                // frame decoder enforces.
                Err(error) => {
                    let _ = resolver.send(Err(ClientError::Type(error.message().to_string())));
                    return pending;
                }
            };
            *lifecycle = Lifecycle::Connecting(
                Box::new(ActiveConnection {
                    id,
                    decoder,
                    transport: None,
                }),
                Some(resolver),
            );
        }
        (self.options.on_state_change)(ConnectionStateChange {
            state: ConnectionState::Connecting,
            error: None,
        });
        let id = self.current_id();
        let handlers = self.handlers(id);
        let inner = Arc::downgrade(self);
        tokio::spawn(async move {
            if let Some(inner) = inner.upgrade() {
                inner.open_transport(id, handlers).await;
            }
        });
        pending
    }

    /// `connection.ts:102-118`. The transport's `send` is invoked
    /// synchronously (booking the chunk in invocation order); only the await
    /// of its result is spawned, mirroring `void sending.catch(...)`.
    fn send(self: &Arc<Self>, frame: Vec<u8>) -> Result<(), ClientError> {
        let transport = {
            let lifecycle = self.lifecycle.lock().unwrap();
            match &*lifecycle {
                Lifecycle::Connected(active, _) => active.transport.clone(),
                _ => return Err(ClientError::Disconnected(DisconnectedError::new())),
            }
        };
        let transport = transport.expect("connected lifecycle carries a transport");
        let sending = transport.send(frame);
        let inner = Arc::downgrade(self);
        tokio::spawn(async move {
            if let Err(error) = sending.await {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                let current = {
                    let lifecycle = inner.lifecycle.lock().unwrap();
                    match &*lifecycle {
                        Lifecycle::Connected(active, _) => active.transport.clone(),
                        _ => None,
                    }
                };
                // Upstream guards `current.state !== "disconnected" &&
                // current.transport === lifecycle.transport`.
                if let Some(current) = current {
                    if Arc::ptr_eq(&current, &transport) {
                        inner.fail_and_close(to_disconnected_error(error));
                    }
                }
            }
        });
        Ok(())
    }

    fn current_id(&self) -> u64 {
        let lifecycle = self.lifecycle.lock().unwrap();
        match &*lifecycle {
            Lifecycle::Disconnected => 0,
            Lifecycle::Connecting(active, _) | Lifecycle::Connected(active, _) => active.id,
        }
    }

    fn is_current(&self, id: u64) -> bool {
        let lifecycle = self.lifecycle.lock().unwrap();
        match &*lifecycle {
            Lifecycle::Disconnected => false,
            Lifecycle::Connecting(active, _) | Lifecycle::Connected(active, _) => active.id == id,
        }
    }

    fn is_connected(&self, id: u64) -> bool {
        let lifecycle = self.lifecycle.lock().unwrap();
        match &*lifecycle {
            Lifecycle::Connected(active, _) => active.id == id,
            _ => false,
        }
    }

    /// `connection.ts:80-88`: per-attempt handlers, fenced to the attempt id.
    fn handlers(self: &Arc<Self>, id: u64) -> ByteTransportHandlers {
        let data_inner = Arc::downgrade(self);
        let close_inner = Arc::downgrade(self);
        let error_inner = Arc::downgrade(self);
        ByteTransportHandlers {
            on_data: Arc::new(move |chunk| {
                if let Some(inner) = data_inner.upgrade() {
                    inner.handle_data(id, chunk);
                }
            }),
            on_close: Arc::new(move || {
                if let Some(inner) = close_inner.upgrade() {
                    if inner.is_current(id) {
                        inner.handle_close();
                    }
                }
            }),
            on_error: Arc::new(move |error| {
                if let Some(inner) = error_inner.upgrade() {
                    if inner.is_current(id) {
                        inner.fail_and_close(to_disconnected_error(error));
                    }
                }
            }),
        }
    }

    /// `connection.ts:120-141` (`#openTransport`).
    async fn open_transport(self: &Arc<Self>, id: u64, handlers: ByteTransportHandlers) {
        let transport = match (self.options.transport_factory)(handlers).await {
            Ok(transport) => transport,
            Err(error) => {
                if self.is_current(id) {
                    self.fail(to_disconnected_error(error));
                }
                return;
            }
        };
        {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            match &mut *lifecycle {
                Lifecycle::Connecting(active, _) if active.id == id => {
                    active.transport = Some(transport.clone());
                }
                _ => {
                    transport.close();
                    return;
                }
            }
        }
        let frame = match encode_client_message(
            &ClientMessage::Hello(ClientHello {
                version: PROTOCOL_VERSION,
            }),
            frame_options(self.max_frame_length),
        ) {
            Ok(frame) => frame,
            Err(error) => {
                if self.is_current(id) {
                    self.fail_and_close(to_disconnected_error(ClientError::Protocol(error)));
                }
                return;
            }
        };
        if let Err(error) = transport.send(frame).await {
            if self.is_current(id) {
                self.fail_and_close(to_disconnected_error(error));
            }
        }
    }

    /// `connection.ts:143-161` (`#handleData`).
    fn handle_data(&self, id: u64, chunk: &[u8]) {
        enum Plan {
            DataBeforeHello,
            Messages(Vec<ServerMessage>),
            DecodeFailure(ClientError),
        }
        let plan = {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            let (is_connecting, has_transport, id_match) = match &*lifecycle {
                Lifecycle::Disconnected => return,
                Lifecycle::Connecting(active, _) => {
                    (true, active.transport.is_some(), active.id == id)
                }
                Lifecycle::Connected(active, _) => (false, true, active.id == id),
            };
            if !id_match {
                return;
            }
            if is_connecting && !has_transport {
                Plan::DataBeforeHello
            } else {
                let decoder = match &mut *lifecycle {
                    Lifecycle::Connecting(active, _) | Lifecycle::Connected(active, _) => {
                        &mut active.decoder
                    }
                    Lifecycle::Disconnected => unreachable!("checked above"),
                };
                match decoder.push(chunk) {
                    Ok(messages) => Plan::Messages(messages),
                    Err(error) => Plan::DecodeFailure(ClientError::Protocol(error)),
                }
            }
        };
        match plan {
            Plan::DataBeforeHello => {
                self.fail_and_close(ClientError::Protocol(ProtocolValidationError::new(
                    "Received server data before the client hello was sent",
                )));
            }
            Plan::DecodeFailure(error) => self.fail_and_close(error),
            Plan::Messages(messages) => {
                for message in messages {
                    // Upstream checks only `disconnected` between messages
                    // (`connection.ts:157-159`).
                    if self.state() == ConnectionState::Disconnected {
                        return;
                    }
                    self.handle_message(message);
                }
            }
        }
    }

    /// `connection.ts:163-213` (`#handleMessage`).
    fn handle_message(&self, message: ServerMessage) {
        enum Entry {
            Disconnected,
            Connecting,
            Connected,
        }
        let entry = {
            let lifecycle = self.lifecycle.lock().unwrap();
            match &*lifecycle {
                Lifecycle::Disconnected => Entry::Disconnected,
                Lifecycle::Connecting(..) => Entry::Connecting,
                Lifecycle::Connected(..) => Entry::Connected,
            }
        };
        match entry {
            Entry::Disconnected => {}
            Entry::Connecting => match message {
                ServerMessage::HelloError(error) => {
                    self.fail_and_close(ClientError::Server(ServerError::new(error.error)));
                }
                ServerMessage::Hello(hello) => {
                    if hello.server_id != self.options.server_id {
                        self.fail_and_close(ClientError::Protocol(ProtocolValidationError::new(
                            format!(
                                "Connected server {} does not match {}",
                                js_quote(&hello.server_id),
                                js_quote(&self.options.server_id),
                            ),
                        )));
                        return;
                    }
                    let transport_present = {
                        let lifecycle = self.lifecycle.lock().unwrap();
                        match &*lifecycle {
                            Lifecycle::Connecting(active, _) => active.transport.is_some(),
                            _ => return,
                        }
                    };
                    if !transport_present {
                        self.fail_and_close(ClientError::Protocol(ProtocolValidationError::new(
                            "Received server hello before the client hello was sent",
                        )));
                        return;
                    }
                    // Transition to connected, keeping the same decoder and
                    // transport (`connection.ts:186-193`). The handshake
                    // resolver stays installed until the final identity
                    // check.
                    let id = self.current_id();
                    let transition = {
                        let mut lifecycle = self.lifecycle.lock().unwrap();
                        match &mut *lifecycle {
                            Lifecycle::Connecting(active, handshake) if active.id == id => {
                                let decoder = std::mem::replace(
                                    &mut active.decoder,
                                    // Placeholder swap target; the real
                                    // decoder moves into the connected
                                    // lifecycle below.
                                    ServerMessageDecoder::new(None)
                                        .expect("default decoder construction"),
                                );
                                let transport = active.transport.clone().expect("checked above");
                                Some((decoder, transport, handshake.take()))
                            }
                            _ => None,
                        }
                    };
                    let Some((decoder, transport, handshake)) = transition else {
                        return;
                    };
                    {
                        let mut lifecycle = self.lifecycle.lock().unwrap();
                        *lifecycle = Lifecycle::Connected(
                            Box::new(ActiveConnection {
                                id,
                                decoder,
                                transport: Some(transport),
                            }),
                            handshake,
                        );
                    }
                    (self.options.on_handshake)(&hello);
                    if !self.is_connected(id) {
                        // `connection.ts:196-200`: the handshake callback
                        // failed or replaced the lifecycle; `fail` already
                        // rejected the resolver.
                        return;
                    }
                    (self.options.on_state_change)(ConnectionStateChange {
                        state: ConnectionState::Connected,
                        error: None,
                    });
                    if !self.is_connected(id) {
                        return;
                    }
                    // `connection.ts:203-204`: detach the resolver, then
                    // resolve.
                    let resolver = {
                        let mut lifecycle = self.lifecycle.lock().unwrap();
                        match &mut *lifecycle {
                            Lifecycle::Connected(active, handshake) if active.id == id => {
                                handshake.take()
                            }
                            _ => None,
                        }
                    };
                    if let Some(resolver) = resolver {
                        let _ = resolver.send(Ok(hello));
                    }
                }
                _ => {
                    self.fail_and_close(ClientError::Protocol(ProtocolValidationError::new(
                        "Expected server hello as first message",
                    )));
                }
            },
            Entry::Connected => match message {
                ServerMessage::Hello(_) | ServerMessage::HelloError(_) => {
                    self.fail_and_close(ClientError::Protocol(ProtocolValidationError::new(
                        "Unexpected handshake message",
                    )));
                }
                _ => (self.options.on_message)(message),
            },
        }
    }

    /// `connection.ts:215-225` (`#handleClose`).
    fn handle_close(&self) {
        let end_result = {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            match &mut *lifecycle {
                Lifecycle::Connecting(active, _) | Lifecycle::Connected(active, _) => {
                    active.decoder.end()
                }
                Lifecycle::Disconnected => return,
            }
        };
        let error = match end_result {
            Ok(()) => {
                ClientError::Disconnected(DisconnectedError::with_message("Byte transport closed"))
            }
            Err(decoder_error) => ClientError::Protocol(decoder_error),
        };
        self.fail(error);
    }

    /// `connection.ts:234-240` (`#fail`).
    fn fail(&self, error: ClientError) {
        let resolver = {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            if matches!(*lifecycle, Lifecycle::Disconnected) {
                return;
            }
            match std::mem::replace(&mut *lifecycle, Lifecycle::Disconnected) {
                Lifecycle::Connecting(_, handshake) | Lifecycle::Connected(_, handshake) => {
                    handshake
                }
                Lifecycle::Disconnected => None,
            }
        };
        if let Some(resolver) = resolver {
            let _ = resolver.send(Err(error.clone()));
        }
        (self.options.on_state_change)(ConnectionStateChange {
            state: ConnectionState::Disconnected,
            error: Some(error),
        });
    }

    /// `connection.ts:227-232` (`#failAndClose`).
    fn fail_and_close(&self, error: ClientError) {
        let transport = {
            let lifecycle = self.lifecycle.lock().unwrap();
            match &*lifecycle {
                Lifecycle::Connecting(active, _) | Lifecycle::Connected(active, _) => {
                    active.transport.clone()
                }
                Lifecycle::Disconnected => None,
            }
        };
        self.fail(error);
        if let Some(transport) = transport {
            transport.close();
        }
    }
}

/// Upstream `JSON.stringify` for hello-mismatch messages.
fn js_quote(value: &str) -> String {
    crate::client::service::js_json_stringify(&JsonValue::String(value.to_string()))
}
