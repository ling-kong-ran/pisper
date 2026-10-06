//! Port of `packages/client/src/client.ts` (479 lines): the RPC client —
//! request correlation, attachment state, service subscriptions with
//! snapshot-gated update buffering, disposal, and the chord service-transport
//! adapter.
//!
//! Upstream promise plumbing maps onto oneshot resolvers; the per-subscription
//! `deliveryTail` promise chain maps onto a dedicated delivery worker that
//! serializes decoded updates behind the `start()` gate (same ordering and
//! drain semantics, disclosed seam S5). Abort signals use the repo's
//! cancellation-token seam (S3). All observable behavior — request/cancel id
//! allocation, wire frames, error texts, state listener sequences — is pinned
//! against the node oracle (`tests/fixtures/client_oracle/oracle.out.txt`).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot, watch};

use crate::agent_core::chord_support::Context;
use crate::protocol::codec::{encode_client_message, ProtocolValidationError};
use crate::protocol::framing::FrameDecoderOptions;
use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{
    is_server_id, CancelEnvelope, ClientMessage, RequestEnvelope, ResponseOutcome, RpcTarget,
    ServerHello, ServerMessage, SessionTarget,
};

use super::connection::Connection;
use super::errors::{
    to_disconnected_error, ClientDisposedError, ClientError, DisconnectedError, ServerError,
};
use super::service::{
    create_service_unsubscribe_call, parse_service_call, parse_wire_service_provider_update,
    parse_wire_service_subscription_snapshot, RemoteServiceListener, RemoteServiceSubscription,
    RemoteServiceTransport, ServiceStateDecoder, ServiceStateDecoderFactory,
};
use super::types::{
    AbortSignal, AttachmentChangeListener, ClientOptions, ConnectionState, ConnectionStateChange,
    ListenerErrorHandler, ServiceSubscription, Unsubscribe,
};

/// Upstream service-subscription listener: `(update) => void | Promise<void>`
/// (`client.ts:53`).
pub type ServiceUpdateListener = Arc<dyn Fn(JsonValue) -> BoxFuture<'static, ()> + Send + Sync>;

/// Upstream connection-state listener: `(change) => void`
/// (`client.ts:66`, `types.ts`).
pub(crate) type ConnectionStateListener = Arc<dyn Fn(&ConnectionStateChange) + Send + Sync>;

/// `client.ts:238-242`: a resolved result or a transform over it. `Null`
/// stands in for upstream `undefined` results (disclosed divergence D10).
pub type RequestHandle = oneshot::Receiver<Result<JsonValue, ClientError>>;

/// `client.ts:242`: `transform?: (result) => T`; `Err` carries the upstream
/// thrown error's message.
type Transform = Box<dyn FnOnce(JsonValue) -> Result<JsonValue, String> + Send>;

/// `client.ts:45-49`.
struct PendingRequest {
    resolve: Option<oneshot::Sender<Result<JsonValue, ClientError>>>,
    transform: Option<Transform>,
    abort_task: Option<tokio::task::AbortHandle>,
    /// Upstream `sent` (`client.ts:249`): the frame left the client.
    sent: bool,
}

/// One live subscription entry (`client.ts:51-60`). `hydrated` and
/// `wire_updates` are the upstream `hydrated` / `queuedWireUpdates`; decoded
/// updates (upstream `queued`) queue inside the delivery worker.
struct ActiveServiceListener {
    decoder: Arc<Mutex<Box<dyn ServiceStateDecoder + Send>>>,
    updates: mpsc::UnboundedSender<SubscriptionCommand>,
    hydrated: bool,
    wire_updates: Vec<JsonValue>,
}

struct ClientState {
    pending_requests: HashMap<String, PendingRequest>,
    connection_state_listeners: Vec<ConnectionStateListener>,
    attachment_listeners: Vec<AttachmentChangeListener>,
    service_listeners: HashMap<String, ActiveServiceListener>,
    request_sequence: u64,
    service_subscription_sequence: u64,
    hello: Option<ServerHello>,
    attachment: Option<SessionTarget>,
    disposed: bool,
}

/// Upstream `disconnect(reason: string | Error)` (`client.ts:138`).
#[derive(Debug, Clone)]
pub enum DisconnectReason {
    Message(String),
    Error(ClientError),
}

impl From<&str> for DisconnectReason {
    fn from(value: &str) -> DisconnectReason {
        DisconnectReason::Message(value.to_string())
    }
}

impl From<String> for DisconnectReason {
    fn from(value: String) -> DisconnectReason {
        DisconnectReason::Message(value)
    }
}

impl From<ClientError> for DisconnectReason {
    fn from(value: ClientError) -> DisconnectReason {
        DisconnectReason::Error(value)
    }
}

/// `client.ts:17` default.
impl Default for DisconnectReason {
    fn default() -> DisconnectReason {
        DisconnectReason::Message("Client disconnected".to_string())
    }
}

/// Worker plumbing behind [`ServiceSubscription`] (upstream
/// `ActiveServiceListener.deliveryTail` / `queued` / `ready` /
/// `subscription.dispose`, `client.ts:180-236, 417-421`).
pub(crate) struct SubscriptionShared {
    id: String,
    target: RpcTarget,
    commands: mpsc::UnboundedSender<SubscriptionCommand>,
    client: Weak<ClientInner>,
    state: Mutex<SubscriptionState>,
    dispose_done: watch::Sender<bool>,
}

struct SubscriptionState {
    disposed: bool,
}

enum SubscriptionCommand {
    /// `subscription.start()` (`client.ts:216-220`).
    Activate,
    /// A decoded update: delivered when ready, queued before activation.
    Update(JsonValue),
    /// `subscription.dispose()`'s final drain: clears the queue and acks once
    /// the in-flight delivery finishes (the upstream `await deliveryTail`).
    Drain(oneshot::Sender<()>),
}

impl SubscriptionShared {
    fn spawn(
        id: String,
        target: RpcTarget,
        listener: ServiceUpdateListener,
        client: Weak<ClientInner>,
    ) -> (
        mpsc::UnboundedSender<SubscriptionCommand>,
        Arc<SubscriptionShared>,
    ) {
        let (commands, mut rx) = mpsc::unbounded_channel::<SubscriptionCommand>();
        let (dispose_done, _) = watch::channel(false);
        let shared = Arc::new(SubscriptionShared {
            id,
            target,
            commands: commands.clone(),
            client,
            state: Mutex::new(SubscriptionState { disposed: false }),
            dispose_done,
        });
        let on_listener_error = shared
            .client
            .upgrade()
            .and_then(|inner| inner.on_listener_error.clone());
        tokio::spawn(async move {
            let mut ready = false;
            let mut queued: VecDeque<JsonValue> = VecDeque::new();
            while let Some(command) = rx.recv().await {
                match command {
                    SubscriptionCommand::Activate if !ready => {
                        ready = true;
                        while let Some(update) = queued.pop_front() {
                            deliver_listener(&listener, update, on_listener_error.as_ref()).await;
                        }
                    }
                    SubscriptionCommand::Activate => {}
                    SubscriptionCommand::Update(update) => {
                        if ready {
                            deliver_listener(&listener, update, on_listener_error.as_ref()).await;
                        } else {
                            queued.push_back(update);
                        }
                    }
                    SubscriptionCommand::Drain(ack) => {
                        queued.clear();
                        let _ = ack.send(());
                    }
                }
            }
        });
        (commands, shared)
    }

    /// `client.ts:216-220`.
    pub(crate) fn activate(&self) {
        {
            let state = self.state.lock().unwrap();
            if state.disposed {
                return;
            }
        }
        let _ = self.commands.send(SubscriptionCommand::Activate);
    }

    /// `client.ts:221-234`. The synchronous prefix (listener removal,
    /// unsubscribe request send, drain enqueue) runs at call time like the
    /// upstream `dispose()` body up to its first `await`.
    pub(crate) fn dispose(self: &Arc<Self>) -> BoxFuture<'static, Result<(), ClientError>> {
        let first = {
            let mut state = self.state.lock().unwrap();
            if state.disposed {
                false
            } else {
                state.disposed = true;
                true
            }
        };
        if !first {
            // Upstream returns the same `disposePromise`.
            let shared = self.clone();
            return Box::pin(async move {
                let mut done = shared.dispose_done.subscribe();
                while !*done.borrow_and_update() {
                    if done.changed().await.is_err() {
                        break;
                    }
                }
                Ok(())
            });
        }
        // Remove the listener first, like upstream (`client.ts:224`).
        let client = self.client.upgrade().map(Client);
        if let Some(client) = &client {
            client.0.remove_service_listener(&self.id);
        }
        let unsubscribe = if let Some(client) = &client {
            if client.connected() && client.0.target_is_current(&self.target) {
                Some(client.request(
                    self.target.clone(),
                    create_service_unsubscribe_call(&self.id),
                    None,
                ))
            } else {
                None
            }
        } else {
            None
        };
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self.commands.send(SubscriptionCommand::Drain(ack_tx));
        let shared = self.clone();
        // Upstream awaits the unsubscribe request, then `deliveryTail`, with
        // the queue clear in a `finally`; a request failure skips the tail
        // await but still clears.
        Box::pin(async move {
            let result: Option<Result<(), ClientError>> = match unsubscribe {
                Some(handle) => match handle.await {
                    Ok(Ok(_)) => None,
                    Ok(Err(error)) => Some(Err(error)),
                    Err(_) => Some(Err(ClientError::Disconnected(DisconnectedError::new()))),
                },
                None => None,
            };
            if result.is_none() {
                let _ = ack_rx.await;
            }
            shared.dispose_done.send_replace(true);
            match result {
                Some(Err(error)) => Err(error),
                Some(Ok(())) | None => Ok(()),
            }
        })
    }
}

/// Upstream `#deliverServiceUpdate` + `#reportListenerError`
/// (`client.ts:417-421, 437-444`): a failing listener is reported through
/// `options.onListenerError` and never breaks the delivery chain. Rust
/// closures cannot throw, so the caught failure class is a listener panic
/// (disclosed seam S4).
async fn deliver_listener(
    listener: &ServiceUpdateListener,
    update: JsonValue,
    on_listener_error: Option<&ListenerErrorHandler>,
) {
    let future = listener(update);
    match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(future)).await {
        Ok(()) => {}
        Err(panic) => {
            if let Some(handler) = on_listener_error {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|value| value.to_string()))
                    .unwrap_or_else(|| "listener panicked".to_string());
                handler(&ClientError::Transport(super::errors::TransportError::new(
                    message,
                )));
            }
        }
    }
}

struct ClientInner {
    server_id: String,
    on_listener_error: Option<ListenerErrorHandler>,
    decoder_factory: ServiceStateDecoderFactory,
    connection: Connection,
    state: Mutex<ClientState>,
    dispose_gate: tokio::sync::Mutex<()>,
    dispose_done: watch::Sender<bool>,
}

/// Upstream `Client` (`client.ts:62-445`). Cheap to clone.
#[derive(Clone)]
pub struct Client(Arc<ClientInner>);

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("server_id", &self.0.server_id)
            .field("disposed", &self.0.state.lock().unwrap().disposed)
            .finish()
    }
}

impl Client {
    /// Upstream constructor (`client.ts:76-91`): rejects a non-canonical
    /// `serverId` with the upstream `TypeError` text.
    pub fn new(options: ClientOptions) -> Result<Client, ClientError> {
        if !is_server_id(&options.server_id) {
            return Err(ClientError::Type(
                "serverId must be a canonical lowercase UUIDv4".to_string(),
            ));
        }
        let ClientOptions {
            transport_factory,
            server_id,
            max_frame_length,
            on_listener_error,
            service_state_decoder_factory,
        } = options;
        // The connection callbacks reach back into the client through a weak
        // handle installed once the `Arc` exists.
        let callback_target: Arc<OnceLock<Weak<ClientInner>>> = Arc::new(OnceLock::new());
        let handshake_target = callback_target.clone();
        let message_target = callback_target.clone();
        let state_target = callback_target.clone();
        let connection = Connection::new(super::connection::ConnectionOptions {
            transport_factory,
            server_id: server_id.clone(),
            max_frame_length,
            on_handshake: Arc::new(move |hello| {
                if let Some(inner) = handshake_target.get().and_then(Weak::upgrade) {
                    inner.state.lock().unwrap().hello = Some(hello.clone());
                }
            }),
            on_message: Arc::new(move |message| {
                if let Some(inner) = message_target.get().and_then(Weak::upgrade) {
                    inner.handle_message(message);
                }
            }),
            on_state_change: Arc::new(move |change| {
                if let Some(inner) = state_target.get().and_then(Weak::upgrade) {
                    inner.handle_connection_state_change(change);
                }
            }),
        })?;
        let (dispose_done, _) = watch::channel(false);
        let inner = Arc::new_cyclic(|weak| {
            let _ = callback_target.set(weak.clone());
            ClientInner {
                server_id,
                on_listener_error,
                decoder_factory: service_state_decoder_factory,
                connection,
                state: Mutex::new(ClientState {
                    pending_requests: HashMap::new(),
                    connection_state_listeners: Vec::new(),
                    attachment_listeners: Vec::new(),
                    service_listeners: HashMap::new(),
                    request_sequence: 0,
                    service_subscription_sequence: 0,
                    hello: None,
                    attachment: None,
                    disposed: false,
                }),
                dispose_gate: tokio::sync::Mutex::new(()),
                dispose_done,
            }
        });
        Ok(Client(inner))
    }

    /// `client.ts:117-126` (`static async connect`). Named `connect_client`
    /// because the instance method owns the upstream `connect` name.
    pub async fn connect_client(options: ClientOptions) -> Result<Client, ClientError> {
        let client = Client::new(options)?;
        let outcome = match client.connect().await {
            Ok(result) => result,
            Err(_) => Err(ClientError::Disconnected(DisconnectedError::new())),
        };
        match outcome {
            Ok(_) => Ok(client),
            Err(error) => {
                client.dispose().await;
                Err(error)
            }
        }
    }

    /// `client.ts:93-95`.
    pub fn disposed(&self) -> bool {
        self.0.state.lock().unwrap().disposed
    }

    /// `client.ts:97-99`.
    pub fn connection_state(&self) -> ConnectionState {
        self.0.connection.state()
    }

    /// `client.ts:101-103`.
    pub fn connected(&self) -> bool {
        self.connection_state() == ConnectionState::Connected
    }

    /// `client.ts:105-107`.
    pub fn server_id(&self) -> &str {
        &self.0.server_id
    }

    /// `client.ts:109-111`.
    pub fn hello(&self) -> Option<ServerHello> {
        self.0.state.lock().unwrap().hello.clone()
    }

    /// `client.ts:113-115`.
    pub fn attachment(&self) -> Option<SessionTarget> {
        self.0.state.lock().unwrap().attachment.clone()
    }

    /// `client.ts:128-132`. Returns the handshake promise (a oneshot
    /// receiver resolving to the server hello).
    pub fn connect(&self) -> oneshot::Receiver<Result<ServerHello, ClientError>> {
        if self.disposed() {
            let (resolver, pending) = oneshot::channel();
            let _ = resolver.send(Err(ClientError::Disposed(ClientDisposedError)));
            return pending;
        }
        self.0.state.lock().unwrap().hello = None;
        self.0.connection.connect()
    }

    /// `client.ts:134-136`.
    pub fn reconnect(&self) -> oneshot::Receiver<Result<ServerHello, ClientError>> {
        self.connect()
    }

    /// `client.ts:138-140`.
    pub fn disconnect(&self, reason: impl Into<DisconnectReason>) {
        match reason.into() {
            DisconnectReason::Message(message) => self.0.connection.disconnect(
                ClientError::Disconnected(DisconnectedError::with_message(message)),
            ),
            DisconnectReason::Error(error) => self.0.connection.disconnect(error),
        }
    }

    /// `client.ts:142-146`.
    pub fn on_connection_state_change(
        &self,
        listener: impl Fn(&ConnectionStateChange) + Send + Sync + 'static,
    ) -> Result<Unsubscribe, ClientError> {
        self.assert_not_disposed()?;
        let entry: Arc<dyn Fn(&ConnectionStateChange) + Send + Sync> = Arc::new(listener);
        let weak = Arc::downgrade(&self.0);
        self.0
            .state
            .lock()
            .unwrap()
            .connection_state_listeners
            .push(entry.clone());
        Ok(Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner
                    .state
                    .lock()
                    .unwrap()
                    .connection_state_listeners
                    .retain(|candidate| !Arc::ptr_eq(candidate, &entry));
            }
        }))
    }

    /// `client.ts:148-152`.
    pub fn on_attachment_change(
        &self,
        listener: AttachmentChangeListener,
    ) -> Result<Unsubscribe, ClientError> {
        self.assert_not_disposed()?;
        let weak = Arc::downgrade(&self.0);
        self.0
            .state
            .lock()
            .unwrap()
            .attachment_listeners
            .push(listener.clone());
        Ok(Box::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner
                    .state
                    .lock()
                    .unwrap()
                    .attachment_listeners
                    .retain(|candidate| !Arc::ptr_eq(candidate, &listener));
            }
        }))
    }

    /// `client.ts:154-157`: one low-level protocol call against an explicit
    /// routed target.
    pub fn request(
        &self,
        target: RpcTarget,
        call: JsonValue,
        signal: Option<AbortSignal>,
    ) -> RequestHandle {
        self.0.request_transform(target, call, signal, None)
    }

    /// `client.ts:159-170`.
    /// Synchronous prefix like upstream `serviceCatalogue` (`client.ts:159`):
    /// the request frame is sent when the call is made; the returned future
    /// is the promise.
    pub fn service_catalogue(
        &self,
        target: RpcTarget,
        signal: Option<AbortSignal>,
    ) -> impl Future<Output = Result<Vec<JsonValue>, ClientError>> + Send {
        let inner = self.0.clone();
        let handle = inner.request_transform(
            target,
            super::service::create_service_catalogue_call(),
            signal,
            None,
        );
        async move {
            let result = match handle.await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(ClientError::Disconnected(DisconnectedError::new())),
            };
            match super::service::parse_service_catalogue(&result) {
                Ok(entries) => Ok(entries),
                Err(error) => {
                    // `client.ts:161-169`: the parse failure fails the
                    // connection and surfaces as a `ProtocolValidationError`.
                    let validation = ProtocolValidationError::new(if error.message().is_empty() {
                        "Invalid service catalogue"
                    } else {
                        error.message()
                    });
                    inner
                        .connection
                        .fail(ClientError::Protocol(validation.clone()));
                    Err(ClientError::Protocol(validation))
                }
            }
        }
    }

    /// `client.ts:172-236`.
    pub fn subscribe_service(
        &self,
        target: RpcTarget,
        service_id: &str,
        mode: super::service::ServiceMode,
        listener: ServiceUpdateListener,
        signal: Option<AbortSignal>,
    ) -> impl Future<Output = Result<ServiceSubscription, ClientError>> + Send {
        let inner = self.0.clone();
        let subscription_id = {
            let mut state = inner.state.lock().unwrap();
            state.service_subscription_sequence += 1;
            format!("service-{}", state.service_subscription_sequence)
        };
        let decoder = Arc::new(Mutex::new((inner.decoder_factory)()));
        let (commands, worker) = SubscriptionShared::spawn(
            subscription_id.clone(),
            target.clone(),
            listener,
            Arc::downgrade(&inner),
        );
        {
            let mut state = inner.state.lock().unwrap();
            state.service_listeners.insert(
                subscription_id.clone(),
                ActiveServiceListener {
                    decoder: decoder.clone(),
                    updates: commands.clone(),
                    hydrated: false,
                    wire_updates: Vec::new(),
                },
            );
        }
        // The request transform (`client.ts:197-204`): decode the snapshot,
        // mark hydrated, drain the buffered wire updates through the same
        // decoder into the worker queue.
        let transform_weak = Arc::downgrade(&inner);
        let transform_decoder = decoder.clone();
        let transform_commands = commands.clone();
        let transform_id = subscription_id.clone();
        let transform: Transform = Box::new(move |result| {
            let parsed = parse_wire_service_subscription_snapshot(&result)
                .map_err(|error| error.message().to_string())?;
            let decoded = transform_decoder
                .lock()
                .unwrap()
                .decode_snapshot(parsed)
                .map_err(|error| error.message().to_string())?;
            if let Some(inner) = transform_weak.upgrade() {
                let drained: Vec<JsonValue> = {
                    let mut state = inner.state.lock().unwrap();
                    match state.service_listeners.get_mut(&transform_id) {
                        Some(active) => {
                            active.hydrated = true;
                            std::mem::take(&mut active.wire_updates)
                        }
                        None => Vec::new(),
                    }
                };
                for update in drained {
                    let parsed = parse_wire_service_provider_update(&update)
                        .map_err(|error| error.message().to_string())?;
                    let decoded_update = transform_decoder
                        .lock()
                        .unwrap()
                        .decode_update(parsed)
                        .map_err(|error| error.message().to_string())?;
                    let _ = transform_commands.send(SubscriptionCommand::Update(decoded_update));
                }
            }
            Ok(decoded)
        });
        let handle = inner.request_transform(
            target.clone(),
            super::service::create_service_subscribe_call(&subscription_id, service_id, mode),
            signal,
            Some(transform),
        );
        async move {
            let snapshot = match handle.await {
                Ok(Ok(snapshot)) => snapshot,
                Ok(Err(error)) => {
                    inner.remove_service_listener(&subscription_id);
                    return Err(error);
                }
                Err(_) => {
                    inner.remove_service_listener(&subscription_id);
                    return Err(ClientError::Disconnected(DisconnectedError::new()));
                }
            };
            // `client.ts:210`: a removed entry means the subscription died
            // mid-handshake.
            if !inner
                .state
                .lock()
                .unwrap()
                .service_listeners
                .contains_key(&subscription_id)
            {
                return Err(ClientError::Disconnected(DisconnectedError::new()));
            }
            Ok(ServiceSubscription {
                id: subscription_id,
                target,
                snapshot,
                inner: worker,
            })
        }
    }

    /// `client.ts:379-392`.
    pub async fn dispose(&self) {
        let first = {
            let mut state = self.0.state.lock().unwrap();
            if state.disposed {
                false
            } else {
                state.disposed = true;
                true
            }
        };
        if !first {
            // Upstream returns the same `disposePromise`.
            let mut done = self.0.dispose_done.subscribe();
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
            return;
        }
        let _gate = self.0.dispose_gate.lock().await;
        if *self.0.dispose_done.borrow() {
            return;
        }
        let error = ClientError::Disposed(ClientDisposedError);
        self.0.reject_pending(error.clone());
        self.0.connection.disconnect(error);
        // The disconnect state change already cleared hello/attachment and
        // the service listeners (`client.ts:386-390`).
        {
            let mut state = self.0.state.lock().unwrap();
            state.hello = None;
            state.connection_state_listeners.clear();
            state.attachment_listeners.clear();
            state.service_listeners.clear();
        }
        self.0.dispose_done.send_replace(true);
    }

    fn assert_not_disposed(&self) -> Result<(), ClientError> {
        if self.disposed() {
            return Err(ClientError::Disposed(ClientDisposedError));
        }
        Ok(())
    }
}

/// The client's internal machinery, held on the shared core so the
/// connection callbacks and subscription workers can reach it through
/// `Weak<ClientInner>` handles.
impl ClientInner {
    fn is_disposed(&self) -> bool {
        self.state.lock().unwrap().disposed
    }

    fn connected(&self) -> bool {
        self.connection.state() == ConnectionState::Connected
    }

    /// `client.ts:238-302` (`#request`).
    pub(crate) fn request_transform(
        self: &Arc<Self>,
        target: RpcTarget,
        call: JsonValue,
        signal: Option<AbortSignal>,
        transform: Option<Transform>,
    ) -> RequestHandle {
        let (resolver, pending) = oneshot::channel();
        if self.is_disposed() {
            let _ = resolver.send(Err(ClientError::Disposed(ClientDisposedError)));
            return pending;
        }
        if !self.connected() {
            let _ = resolver.send(Err(ClientError::Disconnected(DisconnectedError::new())));
            return pending;
        }
        if let Some(signal) = &signal {
            if signal.is_cancelled() {
                let _ = resolver.send(Err(ClientError::Aborted));
                return pending;
            }
        }
        let id = {
            let mut state = self.state.lock().unwrap();
            state.request_sequence += 1;
            format!("request-{}", state.request_sequence)
        };
        let abort_task = signal.map(|token| {
            let weak = Arc::downgrade(self);
            let abort_id = id.clone();
            let abort_target = target.clone();
            let task = tokio::spawn(async move {
                token.cancelled().await;
                if let Some(inner) = weak.upgrade() {
                    inner.abort_request(&abort_id, &abort_target);
                }
            });
            task.abort_handle()
        });
        self.state.lock().unwrap().pending_requests.insert(
            id.clone(),
            PendingRequest {
                resolve: Some(resolver),
                transform,
                abort_task,
                sent: false,
            },
        );
        let frame = {
            let validated = match parse_service_call(&call) {
                Ok(validated) => validated,
                Err(error) => {
                    self.take_and_reject(&id, ClientError::Type(error.message().to_string()));
                    return pending;
                }
            };
            match encode_client_message(
                &ClientMessage::Request(RequestEnvelope {
                    id: id.clone(),
                    target: target.clone(),
                    call: validated,
                }),
                self.frame_options(),
            ) {
                Ok(frame) => frame,
                Err(error) => {
                    self.take_and_reject(&id, ClientError::Protocol(error));
                    return pending;
                }
            }
        };
        if let Err(error) = self.connection.send(frame) {
            // Unreachable upstream (connected is re-checked synchronously);
            // reject rather than hang.
            self.take_and_reject(&id, error);
            return pending;
        }
        if let Some(entry) = self.state.lock().unwrap().pending_requests.get_mut(&id) {
            entry.sent = true;
        }
        pending
    }

    fn frame_options(&self) -> Option<FrameDecoderOptions> {
        Some(
            FrameDecoderOptions::default()
                .with_max_frame_length(self.connection.max_frame_length()),
        )
    }

    /// `client.ts:361-377` (`#takePendingRequest` + `#rejectPendingRequests`).
    fn take_pending(&self, id: &str) -> Option<PendingRequest> {
        let mut state = self.state.lock().unwrap();
        let mut pending = state.pending_requests.remove(id)?;
        if let Some(handle) = pending.abort_task.take() {
            handle.abort();
        }
        Some(pending)
    }

    fn reject_pending(&self, error: ClientError) {
        let requests: Vec<PendingRequest> = {
            let mut state = self.state.lock().unwrap();
            state
                .pending_requests
                .drain()
                .map(|(_, value)| value)
                .collect()
        };
        for mut pending in requests {
            if let Some(handle) = pending.abort_task.take() {
                handle.abort();
            }
            if let Some(resolver) = pending.resolve.take() {
                let _ = resolver.send(Err(error.clone()));
            }
        }
    }

    fn take_and_reject(&self, id: &str, error: ClientError) {
        if let Some(pending) = self.take_pending(id) {
            if let Some(resolver) = pending.resolve {
                let _ = resolver.send(Err(error));
            }
        }
    }

    pub(crate) fn remove_service_listener(&self, id: &str) {
        self.state.lock().unwrap().service_listeners.remove(id);
    }

    pub(crate) fn target_is_current(&self, target: &RpcTarget) -> bool {
        let state = self.state.lock().unwrap();
        match target {
            RpcTarget::Server(server) => state
                .hello
                .as_ref()
                .is_some_and(|hello| hello.server_id == server.server_id),
            RpcTarget::Session(session) => state.attachment.as_ref().is_some_and(|attachment| {
                attachment.server_id == session.server_id
                    && attachment.session_id == session.session_id
                    && attachment.attachment_id == session.attachment_id
            }),
        }
    }

    /// `client.ts:252-261, 476-479` (`sendCancel` + `abortError`).
    fn abort_request(&self, id: &str, target: &RpcTarget) {
        let sent = {
            let mut state = self.state.lock().unwrap();
            let Some(pending) = state.pending_requests.get_mut(id) else {
                return;
            };
            if let Some(resolver) = pending.resolve.take() {
                let _ = resolver.send(Err(ClientError::Aborted));
            }
            pending.abort_task = None;
            pending.sent
        };
        if !sent || !self.connected() {
            return;
        }
        let frame = match encode_client_message(
            &ClientMessage::Cancel(CancelEnvelope {
                id: id.to_string(),
                target: target.clone(),
            }),
            self.frame_options(),
        ) {
            Ok(frame) => frame,
            Err(error) => {
                self.connection.fail(ClientError::Protocol(error));
                return;
            }
        };
        if let Err(error) = self.connection.send(frame) {
            self.connection.fail(to_disconnected_error(error));
        }
    }

    /// `client.ts:304-343` (`#handleMessage`).
    fn handle_message(self: &Arc<Self>, message: ServerMessage) {
        match message {
            ServerMessage::Attachment(envelope) => {
                if let Some(attachment) = &envelope.attachment {
                    if attachment.server_id != self.server_id {
                        self.connection
                            .fail(ClientError::Protocol(ProtocolValidationError::new(
                                "Attachment update belongs to another server",
                            )));
                        return;
                    }
                }
                self.set_attachment(envelope.attachment);
            }
            ServerMessage::ServiceUpdate(event) => {
                enum Plan {
                    Buffered,
                    Decode(
                        Arc<Mutex<Box<dyn ServiceStateDecoder + Send>>>,
                        mpsc::UnboundedSender<SubscriptionCommand>,
                    ),
                }
                let plan = {
                    let mut state = self.state.lock().unwrap();
                    let Some(active) = state.service_listeners.get_mut(&event.subscription_id)
                    else {
                        // Unknown subscription (`client.ts:315`).
                        return;
                    };
                    if !active.hydrated {
                        active.wire_updates.push(event.update.clone());
                        Plan::Buffered
                    } else {
                        Plan::Decode(active.decoder.clone(), active.updates.clone())
                    }
                };
                match plan {
                    Plan::Buffered => {}
                    Plan::Decode(decoder, updates) => {
                        let decoded = parse_wire_service_provider_update(&event.update)
                            .and_then(|parsed| decoder.lock().unwrap().decode_update(parsed));
                        match decoded {
                            Ok(update) => {
                                let _ = updates.send(SubscriptionCommand::Update(update));
                            }
                            Err(error) => {
                                self.connection.fail(ClientError::Protocol(
                                    ProtocolValidationError::new(error.message().to_string()),
                                ));
                            }
                        }
                    }
                }
            }
            ServerMessage::Response(envelope) => {
                let Some(pending) = self.take_pending(&envelope.id) else {
                    self.connection
                        .fail(ClientError::Protocol(ProtocolValidationError::new(
                            "Response has no matching request",
                        )));
                    return;
                };
                let outcome = match envelope.outcome {
                    ResponseOutcome::Failure { error } => {
                        Err(ClientError::Server(ServerError::new(error)))
                    }
                    ResponseOutcome::Success { result } => {
                        let result = result.unwrap_or(JsonValue::Null);
                        match pending.transform {
                            None => Ok(result),
                            Some(transform) => match transform(result) {
                                Ok(value) => Ok(value),
                                Err(message) => {
                                    let validation = ProtocolValidationError::new(message);
                                    self.connection
                                        .fail(ClientError::Protocol(validation.clone()));
                                    Err(ClientError::Protocol(validation))
                                }
                            },
                        }
                    }
                };
                if let Some(resolver) = pending.resolve {
                    let _ = resolver.send(outcome);
                }
            }
            ServerMessage::Hello(_) | ServerMessage::HelloError(_) => {
                // Filtered by the connection state machine; the unreachable
                // path drops the message.
            }
        }
    }

    /// `client.ts:345-359` (`#handleConnectionStateChange`).
    fn handle_connection_state_change(self: &Arc<Self>, change: ConnectionStateChange) {
        let (connection_listeners, attachment_changed) = {
            let mut state = self.state.lock().unwrap();
            let mut attachment_changed = false;
            if change.state == ConnectionState::Disconnected {
                state.hello = None;
                if state.attachment.is_some() {
                    state.attachment = None;
                    attachment_changed = true;
                }
                let error = change
                    .error
                    .clone()
                    .unwrap_or_else(|| ClientError::Disconnected(DisconnectedError::new()));
                let requests: Vec<PendingRequest> = state
                    .pending_requests
                    .drain()
                    .map(|(_, value)| value)
                    .collect();
                for mut pending in requests {
                    if let Some(handle) = pending.abort_task.take() {
                        handle.abort();
                    }
                    if let Some(resolver) = pending.resolve.take() {
                        let _ = resolver.send(Err(error.clone()));
                    }
                }
                state.service_listeners.clear();
            }
            (state.connection_state_listeners.clone(), attachment_changed)
        };
        if attachment_changed {
            let attachment_listeners = self.state.lock().unwrap().attachment_listeners.clone();
            for listener in attachment_listeners {
                listener(None);
            }
        }
        let on_listener_error = self.on_listener_error.clone();
        for listener in connection_listeners {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(&change))) {
                Ok(()) => {}
                Err(panic) => {
                    if let Some(handler) = &on_listener_error {
                        let message = panic
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| panic.downcast_ref::<&str>().map(|value| value.to_string()))
                            .unwrap_or_else(|| "listener panicked".to_string());
                        handler(&ClientError::Transport(super::errors::TransportError::new(
                            message,
                        )));
                    }
                }
            }
        }
    }

    /// `client.ts:398-415` (`#setAttachment`).
    fn set_attachment(self: &Arc<Self>, attachment: Option<SessionTarget>) {
        let listeners = {
            let mut state = self.state.lock().unwrap();
            let same = match (&state.attachment, &attachment) {
                (Some(previous), Some(next)) => previous == next,
                (None, None) => true,
                _ => false,
            };
            if same {
                None
            } else {
                state.attachment = attachment.clone();
                Some(state.attachment_listeners.clone())
            }
        };
        for listener in listeners.unwrap_or_default() {
            listener(attachment.clone());
        }
    }
}

/// `client.ts:448-474`: adapts a lazily resolved routed client target to a
/// chord service transport.
pub fn create_client_service_transport(
    client: &Client,
    get_target: impl Fn() -> Option<RpcTarget> + Send + Sync + 'static,
) -> Arc<dyn RemoteServiceTransport> {
    let inner = Arc::downgrade(&client.0);
    let get_target = Arc::new(get_target);
    Arc::new(ClientServiceTransport {
        client: inner,
        get_target,
    })
}

struct ClientServiceTransport {
    client: Weak<ClientInner>,
    get_target: Arc<dyn Fn() -> Option<RpcTarget> + Send + Sync>,
}

fn unavailable_target() -> ClientError {
    // Upstream `new Error("Remote service target is unavailable")`.
    ClientError::Transport(super::errors::TransportError::new(
        "Remote service target is unavailable",
    ))
}

impl RemoteServiceTransport for ClientServiceTransport {
    fn invoke(
        &self,
        call: JsonValue,
        context: &Context,
    ) -> BoxFuture<'static, Result<JsonValue, ClientError>> {
        let Some(inner) = self.client.upgrade() else {
            return Box::pin(async move {
                Err::<JsonValue, ClientError>(ClientError::Disconnected(DisconnectedError::new()))
            });
        };
        let Some(target) = (self.get_target)() else {
            return Box::pin(async move { Err(unavailable_target()) });
        };
        let signal = context.abort_signal();
        Box::pin(async move {
            let handle = Client(inner).request(target, call, signal);
            match handle.await {
                Ok(result) => result,
                Err(_) => Err(ClientError::Disconnected(DisconnectedError::new())),
            }
        })
    }

    fn subscribe(
        &self,
        service_id: &str,
        mode: super::service::ServiceMode,
        listener: RemoteServiceListener,
        context: &Context,
    ) -> BoxFuture<'static, Result<RemoteServiceSubscription, ClientError>> {
        let Some(inner) = self.client.upgrade() else {
            return Box::pin(async move {
                Err::<RemoteServiceSubscription, ClientError>(ClientError::Disconnected(
                    DisconnectedError::new(),
                ))
            });
        };
        let Some(target) = (self.get_target)() else {
            return Box::pin(async move { Err(unavailable_target()) });
        };
        let signal = context.abort_signal();
        // `client.ts:463-465`: the chord listener always sees
        // BACKGROUND_CONTEXT.
        let service_listener: ServiceUpdateListener = Arc::new(move |update| {
            let listener = listener.clone();
            Box::pin(async move { listener(update, Context::background()).await })
        });
        // The subscribe request is sent synchronously (upstream promise
        // construction); only completion is asynchronous.
        let pending =
            Client(inner).subscribe_service(target, service_id, mode, service_listener, signal);
        Box::pin(async move {
            let subscription = pending.await?;
            Ok(RemoteServiceSubscription {
                snapshot: subscription.snapshot.clone(),
                activate: Arc::new({
                    let subscription = subscription.clone();
                    move || subscription.start()
                }),
                close: Arc::new({
                    let subscription = subscription.clone();
                    move || subscription.dispose()
                }),
            })
        })
    }
}
