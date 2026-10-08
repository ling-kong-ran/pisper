//! Port of `packages/client/src/transport.ts` (18 lines): the byte-transport
//! seam every connection is built on.
//!
//! Upstream `send` returns a `Promise<void>`; the port returns a boxed
//! future. The boxed future must be created (i.e. the transport's
//! bookkeeping for the chunk must run) at `send` call time, preserving the
//! documented "Calls must be delivered in invocation order" contract across
//! spawned awaits.

use std::sync::Arc;

use futures::future::BoxFuture;

use super::errors::ClientError;

/// Upstream `ByteTransport` (`transport.ts:1-6`).
pub trait ByteTransport: Send + Sync + 'static {
    /// Sends one byte chunk. Calls must be delivered in invocation order.
    fn send(&self, chunk: Vec<u8>) -> BoxFuture<'static, Result<(), ClientError>>;
    /// Closes the transport. Implementations must make repeated calls
    /// harmless.
    fn close(&self);
}

/// Upstream `ByteTransportHandlers` (`transport.ts:8-15`).
pub type OnDataCallback = Arc<dyn Fn(&[u8]) + Send + Sync>;

#[derive(Clone)]
pub struct ByteTransportHandlers {
    /// Delivers an arbitrary inbound byte chunk.
    pub on_data: OnDataCallback,
    /// Reports an orderly terminal close.
    pub on_close: Arc<dyn Fn() + Send + Sync>,
    /// Reports a terminal transport failure.
    pub on_error: Arc<dyn Fn(ClientError) + Send + Sync>,
}

/// The factory's return type: a pending transport (or its terminal failure).
pub type PendingByteTransport = BoxFuture<'static, Result<Arc<dyn ByteTransport>, ClientError>>;

/// Upstream `ByteTransportFactory` (`transport.ts:17-18`): creates a fresh
/// connected, authenticated transport. Exactly one terminal handler is
/// expected.
pub type ByteTransportFactory =
    Arc<dyn Fn(ByteTransportHandlers) -> PendingByteTransport + Send + Sync>;
