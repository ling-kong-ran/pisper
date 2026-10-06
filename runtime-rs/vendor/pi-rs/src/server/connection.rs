//! Port of `packages/server/src/connection.ts` (37 lines, SHA256
//! `b13a4766dbbc2f10cbf316f1c1c003a75f854745b7c1d1f864bd0004ce1238fd`): the
//! byte-connection seam, the per-connection handler callbacks, and the
//! server's per-connection routing state.
//!
//! Upstream `ConnectionState` is a plain mutable object driven from the JS
//! event loop; the port guards its mutable fields (stage, decoder, active
//! requests, per-subscription encoders) behind mutexes and replaces the
//! `AbortController`/`NodeJS.Timeout` handles with the repo's
//! [`tokio_util::sync::CancellationToken`] convention (the same disclosed
//! substitution as the client slice: cancellation carries no reason payload).
//!
//! Disclosed folding: upstream keeps `disconnected` and `stage` as two
//! fields that are only ever set together (`disconnect()` writes
//! `disconnected = true; stage = "closed"`), so the port folds the flag into
//! [`ConnectionStage::Closed`]; `isTerminalConnection(state)` becomes a
//! stage check.

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::protocol::codec::ClientMessageDecoder;
use crate::protocol::protocol::ClientMessage;
use crate::protocol::protocol::RpcTarget;

use super::errors::OperationError;
use super::types::RoutedServerServiceAttachment;

/// Upstream `ByteConnection` (`connection.ts:8-13`): an established,
/// authorized ordered byte connection. `send`/`close` return boxed futures
/// (upstream `Promise<void>` / `MaybePromise<void>`).
pub trait ByteConnection: Send + Sync + 'static {
    /// Whether the underlying transport is known-closed.
    fn closed(&self) -> bool;
    /// Sends one framed chunk.
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), OperationError>>;
    /// Closes the connection, optionally flushing a final chunk first.
    fn close(&self, final_chunk: Option<Vec<u8>>)
        -> BoxFuture<'static, Result<(), OperationError>>;
}

/// The `on_data` callback type (`connection.ts:16`).
pub type OnDataCallback = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Upstream `ByteConnectionHandler` (`connection.ts:15-19`). The callbacks
/// are synchronous entry points (upstream event handlers); the server kicks
/// off its async work internally.
#[derive(Clone)]
pub struct ByteConnectionHandler {
    /// Delivers an inbound byte chunk.
    pub on_data: OnDataCallback,
    /// Reports an orderly terminal close.
    pub on_close: Arc<dyn Fn() + Send + Sync>,
    /// Reports a terminal transport failure.
    pub on_error: Arc<dyn Fn(OperationError) + Send + Sync>,
}

/// Upstream `ByteConnectionAcceptor` (`connection.ts:21`).
pub type ByteConnectionAcceptor =
    Arc<dyn Fn(Arc<dyn ByteConnection>) -> ByteConnectionHandler + Send + Sync>;

/// Upstream `ConnectionStage` (`connection.ts:23`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStage {
    AwaitingHello,
    Handshaking,
    Ready,
    Closing,
    Closed,
}

impl ConnectionStage {
    /// The upstream literal (test/debug surface).
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionStage::AwaitingHello => "awaitingHello",
            ConnectionStage::Handshaking => "handshaking",
            ConnectionStage::Ready => "ready",
            ConnectionStage::Closing => "closing",
            ConnectionStage::Closed => "closed",
        }
    }
}

/// One active RPC request (upstream `activeRequests` values:
/// `connection.ts:34`, `{ controller, target }`). The `AbortController`
/// becomes a cancellation token (no reason payload; disclosed seam shared
/// with the client slice S3). `entry_id` identifies the map entry so the
/// `finally` cleanup only removes its own registration.
#[derive(Clone)]
pub struct ActiveRequest {
    pub entry_id: u64,
    pub cancellation: CancellationToken,
    pub target: RpcTarget,
}

/// Per-connection server bookkeeping (upstream `ConnectionState`,
/// `connection.ts:24-35`).
///
/// Upstream's `handshake?: Promise<void>` re-dispatch becomes the
/// `pending_dispatch` queue drained when the handshake completes; the
/// `handshakeTimeout` timer handle becomes the [`CancellationToken`] that
/// cancels the timeout task.
pub(crate) struct ConnectionState {
    /// Stable identity for the router maps (upstream keys by object
    /// identity).
    pub client_id: super::session_router::ClientId,
    pub connection: Arc<dyn ByteConnection>,
    pub(crate) decoder: std::sync::Mutex<ClientMessageDecoder>,
    pub(crate) service_state_encoders:
        std::sync::Mutex<HashMap<String, crate::chord::services::state_codec::ServiceStateEncoder>>,
    pub(crate) stage: std::sync::Mutex<ConnectionStage>,
    /// Messages received while still handshaking (upstream re-dispatches
    /// each after the handshake promise resolves; the port queues and
    /// drains in arrival order).
    pub(crate) pending_dispatch: std::sync::Mutex<Vec<ClientMessage>>,
    pub(crate) handshake_timeout: CancellationToken,
    pub(crate) server_services: std::sync::Mutex<Option<Arc<dyn RoutedServerServiceAttachment>>>,
    pub(crate) active_requests: std::sync::Mutex<HashMap<String, ActiveRequest>>,
}

impl ConnectionState {
    pub(crate) fn stage(&self) -> ConnectionStage {
        *self
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn set_stage(&self, stage: ConnectionStage) {
        *self
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = stage;
    }

    /// Cancel the pending handshake timeout (upstream `clearTimeout`).
    pub(crate) fn cancel_handshake_timeout(&self) {
        self.handshake_timeout.cancel();
    }
}

/// Upstream `isTerminalConnection` (`connection.ts:37-39`): disconnected,
/// closing, or closed. The port folds the disconnected flag into
/// [`ConnectionStage::Closed`].
pub(crate) fn is_terminal_connection(stage: ConnectionStage) -> bool {
    matches!(stage, ConnectionStage::Closing | ConnectionStage::Closed)
}
