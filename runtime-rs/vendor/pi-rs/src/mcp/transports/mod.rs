//! MCP transports, ported from upstream `packages/mcp/src/transports/`:
//! newline-delimited JSON-RPC 2.0 over a spawned process ([`stdio`]),
//! streamable HTTP + SSE ([`streamable_http`]), and an in-memory pair for
//! tests ([`in_memory`]).
//!
//! Upstream `McpTransport` is an interface with `onMessage`/`onError`/
//! `onClose` listener registration returning unsubscribe functions; the port
//! mirrors that shape with boxed callbacks (the client attaches its pump
//! through them, like upstream's constructor wiring in
//! `McpClient.connect`). Listener sets preserve insertion order and
//! [`TransportEvents::emit_close`] fires at most once per transport.

pub mod in_memory;
pub mod stdio;
pub mod streamable_http;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use futures::future::BoxFuture;

pub use crate::mcp::protocol::jsonrpc::{
    McpAuthRequiredError, McpHttpError, McpSessionExpiredError,
};
pub use in_memory::{create_in_memory_transport_pair, InMemoryTransport};
pub use stdio::{StdioTransport, StdioTransportOptions};
pub use streamable_http::{
    consume_sse_stream, ConsumeSseOptions, SseEvent, StreamableHttpReconnectOptions,
    StreamableHttpTransport, StreamableHttpTransportOptions,
};

/// Upstream `DEFAULT_MAX_MESSAGE_BYTES` (16 MiB): the stdio line limit, the
/// SSE per-event limit, and the pending SSE buffer limit.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

pub type TransportMessageListener =
    std::sync::Arc<dyn Fn(&crate::mcp::protocol::jsonrpc::JsonRpcMessage) + Send + Sync>;
pub type TransportErrorListener =
    std::sync::Arc<dyn Fn(&crate::mcp::protocol::jsonrpc::McpClientError) + Send + Sync>;
pub type TransportCloseListener = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Upstream listener unsubscribe handles: call to remove the listener.
#[derive(Clone)]
pub struct Unsubscribe {
    unsubscribe: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl Unsubscribe {
    pub fn new(unsubscribe: impl Fn() + Send + Sync + 'static) -> Self {
        Unsubscribe {
            unsubscribe: std::sync::Arc::new(unsubscribe),
        }
    }

    /// Upstream calling the returned `() => void`.
    pub fn unsubscribe(&self) {
        (self.unsubscribe)();
    }
}

/// Upstream `McpTransport`.
pub trait McpTransport: Send + Sync {
    fn start(&self) -> BoxFuture<'_, Result<(), crate::mcp::protocol::jsonrpc::McpClientError>>;
    fn send(
        &self,
        message: &crate::mcp::protocol::jsonrpc::JsonRpcMessage,
    ) -> BoxFuture<'_, Result<(), crate::mcp::protocol::jsonrpc::McpClientError>>;
    fn close(&self) -> BoxFuture<'_, Result<(), crate::mcp::protocol::jsonrpc::McpClientError>>;
    fn on_message(&self, listener: TransportMessageListener) -> Unsubscribe;
    fn on_error(&self, listener: TransportErrorListener) -> Unsubscribe;
    fn on_close(&self, listener: TransportCloseListener) -> Unsubscribe;
    /// Upstream optional `setProtocolVersion`; only the streamable HTTP
    /// transport implements it (it drives the `MCP-Protocol-Version` header).
    fn set_protocol_version(&self, _version: &str) {}
}

/// Listener bookkeeping shared by transports, ported from upstream
/// `TransportEvents`. `emit_close` fires at most once per transport.
#[derive(Default)]
pub struct TransportEvents {
    message_listeners: Mutex<Vec<(u64, TransportMessageListener)>>,
    error_listeners: Mutex<Vec<(u64, TransportErrorListener)>>,
    close_listeners: Mutex<Vec<(u64, TransportCloseListener)>>,
    next_listener_id: AtomicU64,
    close_emitted: AtomicBool,
}

impl TransportEvents {
    pub fn on_message(
        self: &std::sync::Arc<Self>,
        listener: TransportMessageListener,
    ) -> Unsubscribe {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.message_listeners
            .lock()
            .expect("listener list cannot be poisoned")
            .push((id, listener));
        let events = std::sync::Arc::downgrade(self);
        Unsubscribe::new(move || {
            if let Some(events) = events.upgrade() {
                events
                    .message_listeners
                    .lock()
                    .expect("listener list cannot be poisoned")
                    .retain(|(listener_id, _)| *listener_id != id);
            }
        })
    }

    pub fn on_error(self: &std::sync::Arc<Self>, listener: TransportErrorListener) -> Unsubscribe {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.error_listeners
            .lock()
            .expect("listener list cannot be poisoned")
            .push((id, listener));
        let events = std::sync::Arc::downgrade(self);
        Unsubscribe::new(move || {
            if let Some(events) = events.upgrade() {
                events
                    .error_listeners
                    .lock()
                    .expect("listener list cannot be poisoned")
                    .retain(|(listener_id, _)| *listener_id != id);
            }
        })
    }

    pub fn on_close(self: &std::sync::Arc<Self>, listener: TransportCloseListener) -> Unsubscribe {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.close_listeners
            .lock()
            .expect("listener list cannot be poisoned")
            .push((id, listener));
        let events = std::sync::Arc::downgrade(self);
        Unsubscribe::new(move || {
            if let Some(events) = events.upgrade() {
                events
                    .close_listeners
                    .lock()
                    .expect("listener list cannot be poisoned")
                    .retain(|(listener_id, _)| *listener_id != id);
            }
        })
    }

    pub fn emit_message(&self, message: &crate::mcp::protocol::jsonrpc::JsonRpcMessage) {
        let listeners = self
            .message_listeners
            .lock()
            .expect("listener list cannot be poisoned")
            .clone();
        for (_, listener) in listeners {
            listener(message);
        }
    }

    /// Upstream `emitError` normalizes with `toError` before fanning out.
    pub fn emit_error(&self, error: &crate::mcp::protocol::jsonrpc::McpClientError) {
        let listeners = self
            .error_listeners
            .lock()
            .expect("listener list cannot be poisoned")
            .clone();
        for (_, listener) in listeners {
            listener(error);
        }
    }

    pub fn emit_close(&self) {
        if self.close_emitted.swap(true, Ordering::SeqCst) {
            return;
        }
        let listeners = self
            .close_listeners
            .lock()
            .expect("listener list cannot be poisoned")
            .clone();
        for (_, listener) in listeners {
            listener();
        }
    }
}
