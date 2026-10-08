//! Port of `packages/client/src/types.ts` (32 lines): the shared client
//! types. Upstream callback types (`Unsubscribe`,
//! `AttachmentChangeListener`, `ListenerErrorHandler`) map onto `Arc`-based
//! handler aliases; listener callbacks are infallible in Rust, which is the
//! disclosed seam S4 (upstream wraps listener exceptions into
//! `options.onListenerError`; `onListenerError` is still honored for the
//! paths where the port can observe a failure, but a Rust closure cannot
//! throw one).

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::protocol::json::JsonValue;
use crate::protocol::protocol::{RpcTarget, SessionTarget};

use super::errors::ClientError;
use super::service::ServiceStateDecoderFactory;
use super::transport::ByteTransportFactory;

/// Upstream `ConnectionState` (`types.ts:5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

impl ConnectionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "disconnected",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Connected => "connected",
        }
    }
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Upstream `ConnectionStateChange` (`types.ts:7-10`).
#[derive(Debug, Clone)]
pub struct ConnectionStateChange {
    pub state: ConnectionState,
    pub error: Option<ClientError>,
}

/// Upstream `Unsubscribe` (`types.ts:12`).
pub type Unsubscribe = Box<dyn FnOnce() + Send>;

/// Upstream `ListenerErrorHandler` (`types.ts:13`).
pub type ListenerErrorHandler = Arc<dyn Fn(&ClientError) + Send + Sync>;

/// Upstream `AttachmentChangeListener` (`types.ts:14`); `None` is upstream's
/// `undefined` attachment.
pub type AttachmentChangeListener = Arc<dyn Fn(Option<SessionTarget>) + Send + Sync>;

/// Upstream `ServiceSubscription` (`types.ts:16-23`). `snapshot` is the
/// decoded subscription snapshot; per seam S1 it is the opaque ordered JSON
/// value (upstream: a chord `ServiceSubscriptionSnapshot`).
#[derive(Clone)]
pub struct ServiceSubscription {
    pub id: String,
    pub target: RpcTarget,
    pub snapshot: JsonValue,
    pub(crate) inner: std::sync::Arc<super::client::SubscriptionShared>,
}

impl ServiceSubscription {
    /// Begin ordered update delivery after the caller has installed the
    /// snapshot (`types.ts:21`).
    pub fn start(&self) {
        self.inner.activate();
    }

    /// `types.ts:22`; idempotent like upstream. The synchronous prefix
    /// (unsubscribe request send) runs at call time.
    pub fn dispose(&self) -> futures::future::BoxFuture<'static, Result<(), ClientError>> {
        self.inner.clone().dispose()
    }
}

/// Upstream `ClientOptions` (`types.ts:25-32`).
pub struct ClientOptions {
    pub transport_factory: ByteTransportFactory,
    /// Logical server identity expected at the physical endpoint.
    pub server_id: String,
    pub max_frame_length: Option<u64>,
    /// Reports subscriber failures without allowing them to corrupt client
    /// state.
    pub on_listener_error: Option<ListenerErrorHandler>,
    /// SEAM S1: upstream hard-imports chord's `createServiceStateDecoder`;
    /// the port takes the decoder factory as an option so the chord slice
    /// can rewire it, defaulting to the upstream-faithful port in
    /// [`super::service`].
    pub service_state_decoder_factory: ServiceStateDecoderFactory,
}

impl ClientOptions {
    pub fn new(
        transport_factory: ByteTransportFactory,
        server_id: impl Into<String>,
    ) -> ClientOptions {
        ClientOptions {
            transport_factory,
            server_id: server_id.into(),
            max_frame_length: None,
            on_listener_error: None,
            service_state_decoder_factory: super::service::default_service_state_decoder_factory(),
        }
    }
}

/// Upstream `AbortSignal` seam: the repo's cancellation-token convention (see
/// `src/agent_core/chord_support/context.rs`). Disclosed seam S3: a
/// cancellation token carries no reason payload, so abort rejections use the
/// fixed upstream DOMException text.
pub type AbortSignal = CancellationToken;
