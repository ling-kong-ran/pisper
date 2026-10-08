//! In-memory transport pair, ported from upstream
//! `packages/mcp/src/transports/in-memory.ts` (exported through the
//! package's `./testing` entry).
//!
//! Port notes (disclosed divergences):
//! - Upstream `structuredClone(message)` maps to a `serde_json` deep clone
//!   (values are plain JSON).
//! - Upstream delivers with `queueMicrotask`; the port delivers from a
//!   spawned tokio task, so cross-transport ordering follows the tokio
//!   scheduler rather than the microtask queue. Scenario awaits in the
//!   oracle tests observe the same outcomes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use crate::mcp::protocol::jsonrpc::{JsonRpcMessage, McpClientError};
use crate::mcp::transports::{
    McpTransport, TransportCloseListener, TransportErrorListener, TransportEvents,
    TransportMessageListener, Unsubscribe,
};

/// Upstream `InMemoryTransport`.
pub struct InMemoryTransport {
    shared: Arc<InMemoryShared>,
}

struct InMemoryShared {
    events: Arc<TransportEvents>,
    peer: Mutex<Option<Arc<InMemoryShared>>>,
    started: AtomicBool,
    closed: AtomicBool,
}

impl Default for InMemoryTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryTransport {
    pub fn new() -> Self {
        InMemoryTransport {
            shared: Arc::new(InMemoryShared {
                events: Arc::new(TransportEvents::default()),
                peer: Mutex::new(None),
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Upstream `connectPeer`.
    pub fn connect_peer(&self, peer: &InMemoryTransport) -> Result<(), McpClientError> {
        let mut guard = self
            .shared
            .peer
            .lock()
            .expect("peer slot cannot be poisoned");
        if guard.is_some() {
            return Err(McpClientError::Other(
                "In-memory MCP transport already has a peer".into(),
            ));
        }
        *guard = Some(Arc::clone(&peer.shared));
        Ok(())
    }

    /// Upstream exposes `emitError` so tests can simulate transport-level
    /// failures.
    pub fn emit_error(&self, error: &McpClientError) {
        self.shared.events.emit_error(error);
    }
}

impl McpTransport for InMemoryTransport {
    fn start(&self) -> BoxFuture<'_, Result<(), McpClientError>> {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            if shared.closed.load(Ordering::SeqCst) {
                return Err(McpClientError::connection_closed());
            }
            shared.started.store(true, Ordering::SeqCst);
            Ok(())
        })
    }

    fn send(&self, message: &JsonRpcMessage) -> BoxFuture<'_, Result<(), McpClientError>> {
        let shared = Arc::clone(&self.shared);
        let copy = message.clone();
        Box::pin(async move {
            if !shared.started.load(Ordering::SeqCst) || shared.closed.load(Ordering::SeqCst) {
                return Err(McpClientError::connection_closed());
            }
            let peer = shared
                .peer
                .lock()
                .expect("peer slot cannot be poisoned")
                .clone();
            let Some(peer) = peer else {
                return Err(McpClientError::connection_closed_with(
                    "In-memory MCP peer is not connected",
                ));
            };
            if !peer.started.load(Ordering::SeqCst) || peer.closed.load(Ordering::SeqCst) {
                return Err(McpClientError::connection_closed_with(
                    "In-memory MCP peer is not connected",
                ));
            }
            // Upstream `queueMicrotask(() => peer.deliver(copy))`.
            tokio::spawn(async move {
                if !peer.closed.load(Ordering::SeqCst) {
                    peer.events.emit_message(&copy);
                }
            });
            Ok(())
        })
    }

    fn close(&self) -> BoxFuture<'_, Result<(), McpClientError>> {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            if shared.closed.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            shared.events.emit_close();
            let peer = shared
                .peer
                .lock()
                .expect("peer slot cannot be poisoned")
                .clone();
            if let Some(peer) = peer {
                let peer = InMemoryTransport { shared: peer };
                peer.close().await?;
            }
            Ok(())
        })
    }

    fn on_message(&self, listener: TransportMessageListener) -> Unsubscribe {
        self.shared.events.on_message(listener)
    }

    fn on_error(&self, listener: TransportErrorListener) -> Unsubscribe {
        self.shared.events.on_error(listener)
    }

    fn on_close(&self, listener: TransportCloseListener) -> Unsubscribe {
        self.shared.events.on_close(listener)
    }
}

/// Upstream `createInMemoryTransportPair`.
pub fn create_in_memory_transport_pair() -> (Arc<dyn McpTransport>, Arc<dyn McpTransport>) {
    let client = Arc::new(InMemoryTransport::new());
    let server = Arc::new(InMemoryTransport::new());
    client
        .connect_peer(&server)
        .expect("fresh transports have no peer");
    server
        .connect_peer(&client)
        .expect("fresh transports have no peer");
    // Widen to the trait objects the client drives (upstream returns the raw
    // pair; the port's client takes `Arc<dyn McpTransport>`).
    let client: Arc<dyn McpTransport> = client;
    let server: Arc<dyn McpTransport> = server;
    (client, server)
}
