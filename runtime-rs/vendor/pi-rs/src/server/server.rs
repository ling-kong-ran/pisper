//! Port of `packages/server/src/server.ts` (576 lines, SHA256
//! `bd428151016734ddc1532b78c22d2c2ac539117d9d13a276928ce55c7b3d938d`): the
//! protocol server — connection admission, hello handshake, framed request
//! dispatch, cancellation, subscription update gating, and lifecycle.
//!
//! JS-to-Rust structure notes (disclosed seam S-D):
//!
//! - Upstream drives the dispatch loop from the JS event loop with
//!   fire-and-forget promises (`void this.handleRequest(...)`); the port
//!   spawns the same units of work onto the tokio runtime captured at the
//!   spawn site. Message ordering is preserved: `receive`/`dispatch_message`
//!   run synchronously inside `on_data`, and requests received while still
//!   handshaking are queued and drained in arrival order when the handshake
//!   completes (upstream re-dispatches each via `handshake.then`).
//! - `AbortController` becomes `tokio_util::sync::CancellationToken`
//!   (disclosed client-slice seam S3: no reason payload; the abort reason
//!   texts are unobservable through the port's token seam).
//! - `setTimeout(...).unref()` for the handshake timeout becomes a spawned
//!   task cancelled through the per-connection token (progress requires a
//!   live runtime — the same disclosed scheduler dependence as every ported
//!   promise chain).
//! - `closed` (a promise) becomes [`Server::closed`], a future resolving
//!   once shutdown settles, carrying the identical [`ShutdownError`] value
//!   `close()` returns.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::future::{self, BoxFuture, Shared};
use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::context::with_abort_signal;
use crate::chord::services::state_codec::ServiceStateEncoder;
use crate::chord::services::wire::{
    decode_service_control_call, parse_service_call, parse_service_subscription_snapshot,
    ServiceControlCall,
};
use crate::chord::types::ServiceProviderUpdate;
use crate::protocol::codec::{
    encode_server_message, is_supported_protocol_version, ClientMessageDecoder,
};
use crate::protocol::framing::FrameDecoderOptions;
use crate::protocol::framing::DEFAULT_MAX_FRAME_LENGTH;
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{
    CancelEnvelope, ClientHello, ClientMessage, ProtocolError, RequestEnvelope, ResponseEnvelope,
    ResponseOutcome, RpcTarget, ServerHello, ServerHelloError, ServerMessage, ServiceEventEnvelope,
    PROTOCOL_VERSION,
};

use super::connection::{
    is_terminal_connection, ActiveRequest, ByteConnection, ByteConnectionAcceptor,
    ByteConnectionHandler, ConnectionStage, ConnectionState,
};
use super::errors::{OperationError, ServerError, ShutdownError};
use super::listener::ServerListener;
use super::session_router::{ClientId, RouterOptions, SessionRouter};
use super::types::{
    ConnectionCountHandler, ErrorObserver, PublishCallback, RoutedServerPresentation, ServerHost,
    ServerOptions,
};

const DEFAULT_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;
const MAX_UINT32: u64 = u32::MAX as u64;
const MAX_TIMER_DELAY_MS: u64 = 2_147_483_647;

/// The chord-side JSON value tree.
type ChordValue = serde_json::Value;

/// Shared server core (upstream `Server` private fields).
pub(crate) struct ServerCore {
    pub(crate) host: Arc<dyn ServerHost>,
    pub(crate) listeners: Vec<Arc<dyn ServerListener>>,
    pub(crate) server_id: String,
    pub(crate) max_frame_length: u64,
    pub(crate) handshake_timeout_ms: u64,
    on_connection_count_changed: Option<ConnectionCountHandler>,
    on_error: Option<ErrorObserver>,
    /// Insertion-ordered live connections (upstream `Set`).
    pub(crate) connections: Mutex<Vec<Arc<ConnectionState>>>,
    pub(crate) sessions: SessionRouter,
    next_client_id: AtomicU64,
    next_request_entry_id: AtomicU64,
    /// Lifecycle flags plus the memoized start/close futures (upstream
    /// `started`/`closing`/`startPromise`/`closePromise`).
    pub(crate) lifecycle: Mutex<ServerLifecycle>,
    closed_state: (
        tokio::sync::watch::Sender<Option<ShutdownError>>,
        tokio::sync::watch::Receiver<Option<ShutdownError>>,
    ),
}

#[derive(Default)]
pub(crate) struct ServerLifecycle {
    closing: bool,
    started: bool,
    start_future: Option<Shared<BoxFuture<'static, Result<(), ShutdownError>>>>,
    close_future: Option<Shared<BoxFuture<'static, Result<(), ShutdownError>>>>,
}

/// Upstream `Server` (`server.ts:46-545`). Cheap to clone.
#[derive(Clone)]
pub struct Server(Arc<ServerCore>);

impl Server {
    /// Upstream constructor (`server.ts:67-93`) with `resolveOptions`
    /// validation (`server.ts:555-576`). Returns the typed constructor
    /// `TypeError`s for an invalid `serverId`, `maxFrameLength`, or
    /// `handshakeTimeoutMs` (the "listeners must be an array" check is
    /// unreachable — the Rust option field is always a `Vec`, disclosed
    /// divergence D-A).
    pub fn new(
        host: Arc<dyn ServerHost>,
        options: ServerOptions,
    ) -> Result<Arc<Server>, OperationError> {
        if !crate::protocol::protocol::is_server_id(&options.server_id) {
            return Err(OperationError::Other(
                "serverId must be a canonical lowercase UUIDv4".to_string(),
            ));
        }
        let max_frame_length = options.max_frame_length.unwrap_or(DEFAULT_MAX_FRAME_LENGTH);
        // Upstream: `Number.isSafeInteger(v) && v > 0 && v <= MAX_UINT32`;
        // the safe-integer bound is unreachable for `u64` values in range.
        if max_frame_length == 0 || max_frame_length > MAX_UINT32 {
            return Err(OperationError::Other(format!(
                "Server maxFrameLength must be an integer between 1 and {MAX_UINT32}"
            )));
        }
        let handshake_timeout_ms = options
            .handshake_timeout_ms
            .unwrap_or(DEFAULT_HANDSHAKE_TIMEOUT_MS);
        if handshake_timeout_ms == 0 || handshake_timeout_ms > MAX_TIMER_DELAY_MS {
            return Err(OperationError::Other(format!(
                "Server handshakeTimeoutMs must be an integer between 1 and {MAX_TIMER_DELAY_MS}"
            )));
        }

        // The router options close over the core that does not exist yet;
        // the OnceLock is installed right after construction, before any
        // closure can fire.
        let core_slot: Arc<OnceLock<Arc<ServerCore>>> = Arc::new(OnceLock::new());
        let is_closing: Arc<dyn Fn() -> bool + Send + Sync> = {
            let slot = core_slot.clone();
            Arc::new(move || {
                slot.get()
                    .is_some_and(|core| core.lifecycle.lock().unwrap().closing)
            })
        };
        let publish_attachment = {
            let slot = core_slot.clone();
            Arc::new(
                move |client: ClientId,
                      attachment: Option<crate::protocol::protocol::SessionTarget>,
                      _context: Context|
                      -> BoxFuture<'static, Result<(), OperationError>> {
                    let core = slot.get().expect("server core installed").clone();
                    Box::pin(async move {
                        // `publishAttachment` (`server.ts:80-85`): send the
                        // attachment frame (`None` → null).
                        send_message_to(
                            &core,
                            client,
                            ServerMessage::Attachment(
                                crate::protocol::protocol::AttachmentEnvelope { attachment },
                            ),
                        )
                        .await;
                        Ok(())
                    })
                },
            )
        };
        let report_error: Arc<dyn Fn(&OperationError) + Send + Sync> = {
            let slot = core_slot.clone();
            Arc::new(move |error: &OperationError| {
                if let Some(core) = slot.get() {
                    core.report_error(error);
                }
            })
        };
        let sessions = SessionRouter::new(RouterOptions {
            host: host.clone(),
            server_id: options.server_id.clone(),
            is_closing,
            publish_attachment,
            report_error,
        });
        let (closed_sender, closed_receiver) = tokio::sync::watch::channel(None);
        let core = Arc::new(ServerCore {
            host,
            listeners: options.listeners,
            server_id: options.server_id,
            max_frame_length,
            handshake_timeout_ms,
            on_connection_count_changed: options.on_connection_count_changed,
            on_error: options.on_error,
            connections: Mutex::new(Vec::new()),
            sessions,
            next_client_id: AtomicU64::new(0),
            next_request_entry_id: AtomicU64::new(0),
            lifecycle: Mutex::new(ServerLifecycle::default()),
            closed_state: (closed_sender, closed_receiver),
        });
        let _ = core_slot.set(core.clone());
        Ok(Arc::new(Server(core)))
    }

    /// The stable logical server identity (`server.ts:47`).
    pub fn server_id(&self) -> &str {
        &self.0.server_id
    }

    /// Upstream `start()` (`server.ts:95-101`): starts every listener in
    /// order; memoized like `startPromise` with the same concurrent-call
    /// rejections.
    pub fn start(self: &Arc<Self>) -> BoxFuture<'static, Result<(), ShutdownError>> {
        let core = self.0.clone();
        let mut lifecycle = core.lifecycle.lock().unwrap();
        if lifecycle.started {
            return Box::pin(async {
                Err(ShutdownError::Single(OperationError::Other(
                    "Server is already started".to_string(),
                )))
            });
        }
        if lifecycle.start_future.is_some() {
            return Box::pin(async {
                Err(ShutdownError::Single(OperationError::Other(
                    "Server is already starting".to_string(),
                )))
            });
        }
        if lifecycle.closing {
            return Box::pin(async {
                Err(ShutdownError::Single(OperationError::Other(
                    "Server is closing or closed".to_string(),
                )))
            });
        }
        let start_core = core.clone();
        let future: Shared<BoxFuture<'static, Result<(), ShutdownError>>> =
            Box::pin(async move { start_internal(&start_core).await })
                .boxed()
                .shared();
        lifecycle.start_future = Some(future.clone());
        drop(lifecycle);
        let clear_core = core.clone();
        Box::pin(async move {
            let result = future.clone().await;
            // `finally { this.startPromise = undefined }` (`server.ts:131-133`).
            clear_core.lifecycle.lock().unwrap().start_future = None;
            result
        })
    }

    /// Upstream `close()` (`server.ts:176-181`): memoized like
    /// `closePromise`. The returned error equals the one [`Server::closed`]
    /// yields.
    pub fn close(self: &Arc<Self>) -> BoxFuture<'static, Result<(), ShutdownError>> {
        let core = self.0.clone();
        let mut lifecycle = core.lifecycle.lock().unwrap();
        if let Some(future) = &lifecycle.close_future {
            return Box::pin(future.clone());
        }
        lifecycle.closing = true;
        let close_core = core.clone();
        let future: Shared<BoxFuture<'static, Result<(), ShutdownError>>> =
            Box::pin(async move { close_internal(&close_core).await })
                .boxed()
                .shared();
        lifecycle.close_future = Some(future.clone());
        drop(lifecycle);
        Box::pin(future)
    }

    /// Upstream `closed` (`server.ts:49`): resolves after shutdown, or
    /// yields the listener/routed-Session cleanup failure.
    pub async fn closed(&self) -> Result<(), ShutdownError> {
        let mut receiver = self.0.closed_state.1.clone();
        loop {
            if let Some(error) = receiver.borrow().as_ref() {
                return Err(error.clone());
            }
            if receiver.changed().await.is_err() {
                return Ok(());
            }
        }
    }

    /// Upstream `accept` (`server.ts:136-174`).
    pub fn accept(self: &Arc<Self>, connection: Arc<dyn ByteConnection>) -> ByteConnectionHandler {
        let core = self.0.clone();
        if core.lifecycle.lock().unwrap().closing {
            let close_core = core.clone();
            let close_connection = connection.clone();
            tokio::spawn(async move {
                close_transport(&close_core, &close_connection, None).await;
            });
            let error_core = core.clone();
            return ByteConnectionHandler {
                on_data: Arc::new(|_| {}),
                on_close: Arc::new(|| {}),
                on_error: Arc::new(move |error| error_core.report_error(&error)),
            };
        }

        let client_id = ClientId(core.next_client_id.fetch_add(1, Ordering::SeqCst));
        let decoder = ClientMessageDecoder::new(Some(
            FrameDecoderOptions::default().with_max_frame_length(core.max_frame_length),
        ))
        .expect("validated max frame length");
        let handshake_timeout = CancellationToken::new();
        let state = Arc::new(ConnectionState {
            client_id,
            connection,
            decoder: Mutex::new(decoder),
            service_state_encoders: Mutex::new(HashMap::new()),
            stage: Mutex::new(ConnectionStage::AwaitingHello),
            pending_dispatch: Mutex::new(Vec::new()),
            handshake_timeout: handshake_timeout.clone(),
            server_services: Mutex::new(None),
            active_requests: Mutex::new(HashMap::new()),
        });
        core.connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(state.clone());
        core.notify_connection_count_changed();

        // Handshake timeout (`server.ts:147-152`); the upstream timer is
        // `.unref()`ed — cancellation replaces `clearTimeout`.
        let timeout_core = core.clone();
        let timeout_state = state.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = handshake_timeout.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(timeout_core.handshake_timeout_ms)) => {
                    fail_protocol(
                        &timeout_core,
                        &timeout_state,
                        ProtocolError {
                            code: "invalid_request".to_string(),
                            message: "Handshake timeout".to_string(),
                        },
                    )
                    .await;
                }
            }
        });

        let data_core = core.clone();
        let data_state = state.clone();
        let close_core = core.clone();
        let close_state = state.clone();
        let error_core = core.clone();
        let error_state = state.clone();
        ByteConnectionHandler {
            on_data: Arc::new(move |chunk: &[u8]| receive(&data_core, &data_state, chunk)),
            on_close: Arc::new(move || transport_closed(&close_core, &close_state)),
            on_error: Arc::new(move |error: OperationError| {
                error_core.report_error(&error);
                let spawn_core = error_core.clone();
                let spawn_state = error_state.clone();
                let spawn_connection = spawn_state.connection.clone();
                tokio::spawn(async move {
                    close_transport(&spawn_core, &spawn_connection, None).await;
                    disconnect(&spawn_core, &spawn_state);
                });
            }),
        }
    }
}

/// Upstream `receive` (`server.ts:208-221`): decode and dispatch one
/// inbound chunk; a decode failure fails the protocol.
fn receive(core: &Arc<ServerCore>, state: &Arc<ConnectionState>, chunk: &[u8]) {
    if is_terminal_connection(state.stage()) {
        return;
    }
    let messages = {
        let mut decoder = state
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        decoder.push(chunk)
    };
    match messages {
        Err(error) => {
            let fail_core = core.clone();
            let fail_state = state.clone();
            tokio::spawn(async move {
                fail_protocol(
                    &fail_core,
                    &fail_state,
                    to_protocol_error(
                        &fail_core,
                        &OperationError::Protocol(error.message().to_string()),
                    ),
                )
                .await;
            });
        }
        Ok(messages) => {
            for message in messages {
                if is_terminal_connection(state.stage()) {
                    return;
                }
                dispatch_message(core, state, message);
            }
        }
    }
}

/// Upstream `dispatchMessage` (`server.ts:223-260`).
fn dispatch_message(core: &Arc<ServerCore>, state: &Arc<ConnectionState>, message: ClientMessage) {
    let stage = state.stage();
    if stage == ConnectionStage::AwaitingHello {
        let ClientMessage::Hello(hello) = message else {
            let fail_core = core.clone();
            let fail_state = state.clone();
            tokio::spawn(async move {
                fail_protocol(
                    &fail_core,
                    &fail_state,
                    ProtocolError {
                        code: "invalid_request".to_string(),
                        message: "The first client message must be hello".to_string(),
                    },
                )
                .await;
            });
            return;
        };
        state.set_stage(ConnectionStage::Handshaking);
        let handshake_core = core.clone();
        let handshake_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = finish_handshake(&handshake_core, &handshake_state, hello).await {
                fail_protocol(
                    &handshake_core,
                    &handshake_state,
                    to_protocol_error(&handshake_core, &error),
                )
                .await;
            }
        });
        return;
    }

    if matches!(message, ClientMessage::Hello(_)) {
        let fail_core = core.clone();
        let fail_state = state.clone();
        tokio::spawn(async move {
            fail_protocol(
                &fail_core,
                &fail_state,
                ProtocolError {
                    code: "invalid_request".to_string(),
                    message: "hello may only be sent as the first message".to_string(),
                },
            )
            .await;
        });
        return;
    }

    if stage == ConnectionStage::Ready {
        match message {
            ClientMessage::Cancel(cancel) => handle_cancel(core, state, cancel),
            ClientMessage::Request(envelope) => {
                let request_core = core.clone();
                let request_state = state.clone();
                tokio::spawn(async move {
                    handle_request(&request_core, &request_state, envelope).await;
                });
            }
            ClientMessage::Hello(_) => {}
        }
        return;
    }
    if stage != ConnectionStage::Handshaking {
        return;
    }
    state
        .pending_dispatch
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(message);
}

impl ServerCore {
    pub(crate) fn report_error(&self, error: &OperationError) {
        // Upstream wraps the observer in try/catch (`server.ts:531-537`);
        // Rust closures cannot throw (client-slice seam S4).
        if let Some(on_error) = &self.on_error {
            on_error(error);
        }
    }

    fn notify_connection_count_changed(&self) {
        if let Some(handler) = &self.on_connection_count_changed {
            let count = self
                .connections
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len();
            handler(count);
        }
    }

    fn set_closing(&self) {
        self.lifecycle.lock().unwrap().closing = true;
    }
}

/// The publishAttachment closure's send path: route the frame to the
/// client's connection by id (`server.ts:80-85`). A missing connection is
/// already disconnected, so the frame is silently dropped.
async fn send_message_to(core: &Arc<ServerCore>, client: ClientId, message: ServerMessage) {
    let state = core
        .connections
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|candidate| candidate.client_id == client)
        .cloned();
    if let Some(state) = state {
        send_message(core, &state, message).await;
    }
}

/// Upstream `sendMessage` (`server.ts:452-472`): encode + send one frame;
/// encode or transport failures close and disconnect. `false` means the
/// frame was not delivered.
async fn send_message(
    core: &Arc<ServerCore>,
    state: &Arc<ConnectionState>,
    message: ServerMessage,
) -> bool {
    if state.stage() == ConnectionStage::Closed || state.connection.closed() {
        return false;
    }
    let frame = match encode_server_message(
        &message,
        Some(FrameDecoderOptions::default().with_max_frame_length(core.max_frame_length)),
    ) {
        Ok(frame) => frame,
        Err(error) => {
            core.report_error(&OperationError::Protocol(error.message().to_string()));
            close_transport(core, &state.connection, None).await;
            disconnect(core, state);
            return false;
        }
    };
    match state.connection.send(frame).await {
        Ok(()) => true,
        Err(error) => {
            core.report_error(&error);
            close_transport(core, &state.connection, None).await;
            disconnect(core, state);
            false
        }
    }
}

/// Upstream `disconnect` (`server.ts:417-436`).
pub(crate) fn disconnect(core: &Arc<ServerCore>, state: &Arc<ConnectionState>) {
    if state.stage() == ConnectionStage::Closed {
        return;
    }
    state.set_stage(ConnectionStage::Closed);
    state.cancel_handshake_timeout();
    let active_requests: Vec<ActiveRequest> = state
        .active_requests
        .lock()
        .unwrap()
        .drain()
        .map(|(_, request)| request)
        .collect();
    for request in active_requests {
        // `controller.abort(new Error("Client disconnected"))` — the
        // cancellation-token seam carries no reason (S3).
        request.cancellation.cancel();
    }
    state
        .service_state_encoders
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    let removed = {
        let mut connections = core
            .connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let position = connections
            .iter()
            .position(|candidate| candidate.client_id == state.client_id);
        match position {
            Some(position) => {
                connections.remove(position);
                true
            }
            None => false,
        }
    };
    if removed {
        core.notify_connection_count_changed();
    }
    let server_services = state.server_services.lock().unwrap().take();
    let cleanup_core = core.clone();
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let disconnect_result = cleanup_core
            .sessions
            .disconnect(cleanup_state.client_id, Context::background())
            .await;
        if let Err(error) = disconnect_result {
            cleanup_core.report_error(&error);
        }
        if let Some(services) = server_services {
            if let Err(error) = services.release(Context::background()).await {
                cleanup_core.report_error(&error);
            }
        }
    });
}

/// Upstream `handleCancel` (`server.ts:298-304`).
fn handle_cancel(core: &Arc<ServerCore>, state: &Arc<ConnectionState>, envelope: CancelEnvelope) {
    if rpc_target_server_id(&envelope.target) != core.server_id {
        return;
    }
    let active = state
        .active_requests
        .lock()
        .unwrap()
        .get(&envelope.id)
        .cloned();
    if let Some(active) = active {
        if same_target(&active.target, &envelope.target) {
            active.cancellation.cancel();
        }
    }
}

/// Upstream `transportClosed` (`server.ts:406-415`).
fn transport_closed(core: &Arc<ServerCore>, state: &Arc<ConnectionState>) {
    let stage = state.stage();
    if !matches!(stage, ConnectionStage::Closing | ConnectionStage::Closed) {
        let end = state
            .decoder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .end();
        if let Err(error) = end {
            core.report_error(&OperationError::Protocol(error.message().to_string()));
        }
    }
    disconnect(core, state);
}

/// Upstream `finishHandshake` (`server.ts:262-296`).
async fn finish_handshake(
    core: &Arc<ServerCore>,
    state: &Arc<ConnectionState>,
    hello: ClientHello,
) -> Result<(), OperationError> {
    if !is_supported_protocol_version(hello.version as f64) {
        fail_protocol(
            core,
            state,
            ProtocolError {
                code: "version".to_string(),
                message: format!(
                    "Unsupported protocol version {}; expected {PROTOCOL_VERSION}",
                    hello.version
                ),
            },
        )
        .await;
        return Ok(());
    }

    if handshake_aborted(core, state) {
        return Ok(());
    }
    let presentation = Arc::new(ServerPresentation {
        core: core.clone(),
        client_id: state.client_id,
    });
    let services = core
        .host
        .server_services()
        .attach_client(presentation, Context::todo())
        .await?;
    if handshake_aborted(core, state) {
        services.release(Context::todo()).await?;
        return Ok(());
    }
    *state
        .server_services
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(services.clone());
    let sent = send_message(
        core,
        state,
        ServerMessage::Hello(ServerHello {
            server_id: core.server_id.clone(),
        }),
    )
    .await;
    if sent && state.stage() == ConnectionStage::Handshaking {
        state.set_stage(ConnectionStage::Ready);
        state.cancel_handshake_timeout();
        // Drain the requests queued while handshaking, in arrival order
        // (upstream `handshake.then(...)` re-dispatch, `server.ts:252-259`).
        let pending: Vec<ClientMessage> = state
            .pending_dispatch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        for message in pending {
            if state.stage() != ConnectionStage::Ready {
                break;
            }
            match message {
                ClientMessage::Cancel(cancel) => handle_cancel(core, state, cancel),
                ClientMessage::Request(envelope) => {
                    let request_core = core.clone();
                    let request_state = state.clone();
                    tokio::spawn(async move {
                        handle_request(&request_core, &request_state, envelope).await;
                    });
                }
                ClientMessage::Hello(_) => {}
            }
        }
    }
    Ok(())
}

/// `this.closing || state.disconnected || state.stage !== "handshaking" ||
/// state.connection.closed` (`server.ts:271`, `server.ts:282`).
fn handshake_aborted(core: &Arc<ServerCore>, state: &Arc<ConnectionState>) -> bool {
    core.lifecycle.lock().unwrap().closing
        || is_terminal_connection(state.stage())
        || state.stage() != ConnectionStage::Handshaking
        || state.connection.closed()
}

/// Upstream `handleRequest` (`server.ts:306-404`).
async fn handle_request(
    core: &Arc<ServerCore>,
    state: &Arc<ConnectionState>,
    envelope: RequestEnvelope,
) {
    if state
        .active_requests
        .lock()
        .unwrap()
        .contains_key(&envelope.id)
    {
        send_message(
            core,
            state,
            response_failure(
                &envelope.id,
                ProtocolError {
                    code: "invalid_request".to_string(),
                    message: "Request ID is already active".to_string(),
                },
            ),
        )
        .await;
        return;
    }
    let call_value = envelope.call.to_serde_json();
    let call = match parse_service_call(&call_value) {
        Ok(call) => call,
        Err(_) => {
            send_message(
                core,
                state,
                response_failure(
                    &envelope.id,
                    ProtocolError {
                        code: "invalid_request".to_string(),
                        message: "Invalid service call".to_string(),
                    },
                ),
            )
            .await;
            return;
        }
    };
    let cancellation = CancellationToken::new();
    let entry_id = core.next_request_entry_id.fetch_add(1, Ordering::SeqCst);
    state.active_requests.lock().unwrap().insert(
        envelope.id.clone(),
        ActiveRequest {
            entry_id,
            cancellation: cancellation.clone(),
            target: envelope.target.clone(),
        },
    );
    let context = with_abort_signal(cancellation.clone(), Context::todo());
    let control = decode_service_control_call(&call);
    let subscribing = match &control {
        Some(control @ ServiceControlCall::Subscribe { .. }) => Some(control.clone()),
        _ => None,
    };
    let bookkeeping = Arc::new(RequestBookkeeping {
        pending_updates: Mutex::new(Vec::new()),
        subscription_ready: Mutex::new(subscribing.is_none()),
        encoder_installed: AtomicBool::new(false),
        responded: AtomicBool::new(false),
    });
    let publish: PublishCallback = {
        let core = core.clone();
        let state = state.clone();
        let bookkeeping = bookkeeping.clone();
        let gate_id = subscribing.as_ref().map(control_subscription_id);
        Arc::new(
            move |subscription_id: String,
                  update: ServiceProviderUpdate,
                  _context: Context|
                  -> BoxFuture<'static, Result<(), OperationError>> {
                let core = core.clone();
                let state = state.clone();
                let bookkeeping = bookkeeping.clone();
                let gate_id = gate_id.clone();
                Box::pin(async move {
                    if let Some(gate_id) = gate_id {
                        if subscription_id == gate_id {
                            let ready = bookkeeping
                                .subscription_ready
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if !*ready {
                                drop(ready);
                                bookkeeping
                                    .pending_updates
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .push(update);
                                return Ok(());
                            }
                        }
                    }
                    send_service_update(&core, &state, &subscription_id, update).await
                })
            },
        )
    };

    let result: Result<Option<ChordValue>, OperationError> = async {
        if rpc_target_server_id(&envelope.target) != core.server_id {
            return Err(OperationError::Server(ServerError::wrong_server()));
        }
        if let Some(control) = &subscribing {
            let subscription_id = control_subscription_id(control);
            if state
                .service_state_encoders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&subscription_id)
            {
                return Err(OperationError::Protocol(format!(
                    "Duplicate service subscription {subscription_id}"
                )));
            }
        }
        let result = match &envelope.target {
            RpcTarget::Session(_) => {
                core.sessions
                    .execute_service_call(
                        state.client_id,
                        envelope.target.clone(),
                        call.clone(),
                        publish.clone(),
                        context.clone(),
                    )
                    .await?
            }
            RpcTarget::Server(_) => {
                let services = state
                    .server_services
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                match services {
                    Some(services) => {
                        services
                            .invoke_service(call.clone(), publish.clone(), context.clone())
                            .await?
                    }
                    None => {
                        return Err(OperationError::Protocol(format!(
                            "Unknown service member {}.{}",
                            call.service_id, call.member
                        )));
                    }
                }
            }
        };
        if let Some(control) = &subscribing {
            let subscription_id = control_subscription_id(control);
            let Some(result) = result else {
                return Err(OperationError::Protocol(
                    "Service subscription did not return a snapshot".to_string(),
                ));
            };
            let mut encoder = ServiceStateEncoder::new();
            let snapshot = parse_service_subscription_snapshot(&result)?;
            let encoded = encoder.encode_snapshot(&snapshot)?;
            bookkeeping.encoder_installed.store(true, Ordering::SeqCst);
            state
                .service_state_encoders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(subscription_id, encoder);
            Ok(Some(encoded.to_json()))
        } else {
            if let Some(ServiceControlCall::Unsubscribe { subscription_id }) = &control {
                state
                    .service_state_encoders
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(subscription_id);
            }
            Ok(result)
        }
    }
    .await;

    match result {
        Ok(result) => {
            let message = match result {
                Some(value) => ServerMessage::Response(ResponseEnvelope {
                    id: envelope.id.clone(),
                    outcome: ResponseOutcome::Success {
                        result: Some(JsonValue::from_serde_json(&value)),
                    },
                }),
                None => ServerMessage::Response(ResponseEnvelope {
                    id: envelope.id.clone(),
                    outcome: ResponseOutcome::Success { result: None },
                }),
            };
            send_message(core, state, message).await;
            bookkeeping.responded.store(true, Ordering::SeqCst);
            if let Some(control) = &subscribing {
                let subscription_id = control_subscription_id(control);
                let mut drain_error: Option<OperationError> = None;
                loop {
                    let pending = bookkeeping
                        .pending_updates
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .pop();
                    let Some(update) = pending else { break };
                    if let Err(error) =
                        send_service_update(core, state, &subscription_id, update).await
                    {
                        // Upstream: the throw lands in the catch with
                        // `responded === true` — report, close, disconnect.
                        drain_error = Some(error);
                        break;
                    }
                }
                match drain_error {
                    None => {
                        *bookkeeping
                            .subscription_ready
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                    }
                    Some(error) => {
                        core.report_error(&error);
                        close_transport(core, &state.connection, None).await;
                        disconnect(core, state);
                    }
                }
            }
        }
        Err(error) => {
            if let Some(control) = &subscribing {
                let subscription_id = control_subscription_id(control);
                if bookkeeping.encoder_installed.load(Ordering::SeqCst)
                    && !bookkeeping.responded.load(Ordering::SeqCst)
                {
                    state
                        .service_state_encoders
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&subscription_id);
                }
            }
            if bookkeeping.responded.load(Ordering::SeqCst) {
                core.report_error(&error);
                close_transport(core, &state.connection, None).await;
                disconnect(core, state);
            } else {
                let protocol_error = if cancellation.is_cancelled() {
                    ProtocolError {
                        code: "cancelled".to_string(),
                        message: "RPC request cancelled".to_string(),
                    }
                } else {
                    to_protocol_error(core, &error)
                };
                send_message(core, state, response_failure(&envelope.id, protocol_error)).await;
            }
        }
    }

    // `finally`: remove the active entry when it is still ours
    // (`server.ts:401-403`).
    let mut active_requests = state.active_requests.lock().unwrap();
    if active_requests
        .get(&envelope.id)
        .is_some_and(|current| current.entry_id == entry_id)
    {
        active_requests.remove(&envelope.id);
    }
}

/// Per-request bookkeeping shared with the publish closure (upstream the
/// closure-captured `pendingUpdates`/`subscriptionReady`/`responded`
/// variables).
struct RequestBookkeeping {
    pending_updates: Mutex<Vec<ServiceProviderUpdate>>,
    subscription_ready: Mutex<bool>,
    encoder_installed: AtomicBool,
    responded: AtomicBool,
}

/// Upstream `sendServiceUpdate` (`server.ts:438-450`).
async fn send_service_update(
    core: &Arc<ServerCore>,
    state: &Arc<ConnectionState>,
    subscription_id: &str,
    update: ServiceProviderUpdate,
) -> Result<(), OperationError> {
    let encoded = {
        let mut encoders = state
            .service_state_encoders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(encoder) = encoders.get_mut(subscription_id) else {
            return Ok(());
        };
        encoder.encode_update(&update)?
    };
    send_message(
        core,
        state,
        ServerMessage::ServiceUpdate(ServiceEventEnvelope {
            subscription_id: subscription_id.to_string(),
            update: JsonValue::from_serde_json(&encoded.to_json()),
        }),
    )
    .await;
    Ok(())
}

/// Upstream `failProtocol` (`server.ts:474-487`).
async fn fail_protocol(core: &Arc<ServerCore>, state: &Arc<ConnectionState>, error: ProtocolError) {
    let stage = state.stage();
    if matches!(stage, ConnectionStage::Closing | ConnectionStage::Closed) {
        return;
    }
    state.set_stage(ConnectionStage::Closing);
    state.cancel_handshake_timeout();
    let mut final_frame: Option<Vec<u8>> = None;
    match encode_server_message(
        &ServerMessage::HelloError(ServerHelloError {
            error: error.clone(),
        }),
        Some(FrameDecoderOptions::default().with_max_frame_length(core.max_frame_length)),
    ) {
        Ok(frame) => final_frame = Some(frame),
        Err(encode_error) => core.report_error(&OperationError::Protocol(
            encode_error.message().to_string(),
        )),
    }
    close_transport(core, &state.connection, final_frame).await;
    disconnect(core, state);
}

/// Upstream `closeServerState` (`server.ts:489-502`).
async fn close_server_state(core: &Arc<ServerCore>) -> Result<(), OperationError> {
    let connections: Vec<Arc<ConnectionState>> = core
        .connections
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for connection in &connections {
        connection.set_stage(ConnectionStage::Closing);
        connection.cancel_handshake_timeout();
    }
    future::join_all(connections.iter().map(|connection| async move {
        close_transport(core, &connection.connection, None).await;
    }))
    .await;
    for connection in &connections {
        disconnect(core, connection);
    }
    let mut errors: Vec<OperationError> = Vec::new();
    if let Err(error) = core.sessions.close(Context::background()).await {
        errors.push(error);
    }
    core.connections
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    match errors.len() {
        0 => Ok(()),
        1 => Err(errors.into_iter().next().expect("one error")),
        _ => Err(OperationError::Aggregate {
            message: "Failed to close server Sessions".to_string(),
            errors,
        }),
    }
}

/// Upstream `closeConnection` (`server.ts:504-510`): errors are reported,
/// never propagated.
async fn close_transport(
    core: &ServerCore,
    connection: &Arc<dyn ByteConnection>,
    final_chunk: Option<Vec<u8>>,
) {
    if let Err(error) = connection.close(final_chunk).await {
        core.report_error(&error);
    }
}

/// Upstream `toProtocolError` (`server.ts:512-521`) with the internal-error
/// reporting side effect.
fn to_protocol_error(core: &Arc<ServerCore>, error: &OperationError) -> ProtocolError {
    if error.reports_internal() {
        core.report_error(error);
    }
    error.to_protocol_error()
}

/// Upstream `sameTarget` (`server.ts:547-553`).
fn same_target(left: &RpcTarget, right: &RpcTarget) -> bool {
    if rpc_target_server_id(left) != rpc_target_server_id(right) {
        return false;
    }
    match (left, right) {
        (RpcTarget::Session(left), RpcTarget::Session(right)) => {
            left.session_id == right.session_id && left.attachment_id == right.attachment_id
        }
        (RpcTarget::Server(_), RpcTarget::Server(_)) => true,
        _ => false,
    }
}

fn rpc_target_server_id(target: &RpcTarget) -> &str {
    match target {
        RpcTarget::Server(target) => &target.server_id,
        RpcTarget::Session(target) => &target.server_id,
    }
}

fn control_subscription_id(control: &ServiceControlCall) -> String {
    match control {
        ServiceControlCall::Subscribe {
            subscription_id, ..
        } => subscription_id.clone(),
        ServiceControlCall::Unsubscribe { subscription_id } => subscription_id.clone(),
        ServiceControlCall::Catalogue => String::new(),
    }
}

fn response_failure(id: &str, error: ProtocolError) -> ServerMessage {
    ServerMessage::Response(ResponseEnvelope {
        id: id.to_string(),
        outcome: ResponseOutcome::Failure { error },
    })
}

/// Upstream `startInternal` (`server.ts:103-134`).
async fn start_internal(core: &Arc<ServerCore>) -> Result<(), ShutdownError> {
    let mut started: Vec<Arc<dyn ServerListener>> = Vec::new();
    let acceptor = acceptor_for(core);
    let mut start_error: Option<OperationError> = None;
    for listener in &core.listeners {
        // Upstream starts sequentially, awaiting each `listener.start`.
        match listener.start(acceptor.clone()).await {
            Ok(()) => started.push(listener.clone()),
            Err(error) => {
                start_error = Some(error);
                break;
            }
        }
    }
    if let Some(error) = start_error {
        core.set_closing();
        let mut cleanup_errors: Vec<OperationError> = Vec::new();
        let closes = future::join_all(started.iter().map(|listener| listener.close())).await;
        for result in closes {
            if let Err(error) = result {
                cleanup_errors.push(error);
            }
        }
        if let Err(error) = close_server_state(core).await {
            cleanup_errors.push(error);
        }
        if !cleanup_errors.is_empty() {
            let mut errors = vec![error];
            errors.extend(cleanup_errors);
            let failure = ShutdownError::Aggregate {
                message: "Server startup and cleanup failed".to_string(),
                errors,
            };
            settle_closed(core, Some(failure.clone()));
            return Err(failure);
        }
        settle_closed(core, None);
        return Err(ShutdownError::Single(error));
    }
    core.lifecycle.lock().unwrap().started = true;
    Ok(())
}

/// The acceptor handed to listeners (`server.ts:107`): every accepted
/// connection routes through `Server::accept`.
fn acceptor_for(core: &Arc<ServerCore>) -> ByteConnectionAcceptor {
    let server_core = core.clone();
    Arc::new(move |connection: Arc<dyn ByteConnection>| {
        let server = Arc::new(Server(server_core.clone()));
        server.accept(connection)
    })
}

/// Upstream `closeInternal` (`server.ts:183-206`).
async fn close_internal(core: &Arc<ServerCore>) -> Result<(), ShutdownError> {
    let starting = core.lifecycle.lock().unwrap().start_future.clone();
    if let Some(starting) = starting {
        let _ = starting.await;
    }
    let mut errors: Vec<OperationError> = Vec::new();
    let closes = future::join_all(core.listeners.iter().map(|listener| listener.close())).await;
    for result in closes {
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if let Err(error) = close_server_state(core).await {
        errors.push(error);
    }
    core.lifecycle.lock().unwrap().started = false;
    match errors.len() {
        0 => {
            settle_closed(core, None);
            Ok(())
        }
        1 => {
            let failure = ShutdownError::Single(errors.into_iter().next().expect("one error"));
            settle_closed(core, Some(failure.clone()));
            Err(failure)
        }
        _ => {
            let failure = ShutdownError::Aggregate {
                message: "Server shutdown failed".to_string(),
                errors,
            };
            settle_closed(core, Some(failure.clone()));
            Err(failure)
        }
    }
}

/// Upstream `settleClosed` (`server.ts:539-544`).
fn settle_closed(core: &Arc<ServerCore>, error: Option<ShutdownError>) {
    let _ = core.closed_state.0.send(error);
}

/// The presentation capabilities installed for one connection's handshake
/// (`server.ts:272-279`).
struct ServerPresentation {
    core: Arc<ServerCore>,
    client_id: ClientId,
}

impl RoutedServerPresentation for ServerPresentation {
    fn attach_session(
        &self,
        session_id: String,
        context: Context,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        self.core
            .sessions
            .attach_client(self.client_id, session_id, context)
    }

    fn detach_session(&self, context: Context) -> BoxFuture<'static, Result<(), OperationError>> {
        self.core.sessions.detach_client(self.client_id, context)
    }

    fn prepare_session_removal(
        &self,
        session_id: String,
        context: Context,
    ) -> BoxFuture<'static, Result<(), OperationError>> {
        self.core.sessions.remove_session(session_id, context)
    }
}
