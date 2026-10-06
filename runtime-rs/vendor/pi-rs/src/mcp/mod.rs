//! Standalone Model Context Protocol client, ported from upstream
//! `@earendil-works/pi-mcp` 0.99.1 (`pi/packages/mcp` at 2bbfcca43): JSON-RPC
//! 2.0 over newline-delimited stdio and streamable HTTP transports, tools/
//! resources/prompts surfaces with timeouts, cancellation and progress, and
//! the OAuth authorization-code + PKCE machinery ([`oauth`]).
//!
//! Mirrors the upstream package's root entry point re-exports 1:1; the
//! `./testing` entry ([`testing`]) carries the in-memory transport pair.
//! The consuming coding-agent extension slice is a later port.

pub mod auth_provider;
pub mod client;
pub mod oauth;
pub mod protocol;
pub mod transports;

#[cfg(test)]
mod mcp_oauth_oracle_tests;
#[cfg(test)]
mod mcp_oracle_tests;
#[cfg(test)]
mod mcp_transports_oracle_tests;

/// Deterministic randomness seam for the oracle tests. The capture INTENDED
/// a counter stream (draw k fills byte[i] = `(k*32 + i) & 0xFF`, reset per
/// scenario), but its `Buffer.from({ length }, mapping)` stub never invoked
/// the mapping under the capturing Node, so every recorded draw is 32 zero
/// bytes — the oracle (all verifiers `AAAA…`, provider state 64 hex zeros)
/// is ground truth, and the port replays exactly that. Only live under
/// `cfg(test)`; production code uses OS entropy.
#[cfg(test)]
pub(crate) mod test_rng {
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Draw counter (one draw per 32-byte request, reset per scenario).
    fn counter() -> &'static AtomicU32 {
        static COUNTER: std::sync::OnceLock<AtomicU32> = std::sync::OnceLock::new();
        COUNTER.get_or_init(|| AtomicU32::new(0))
    }

    /// Reset the draw counter at the start of a scenario (capture parity).
    pub(crate) fn reset() {
        counter().store(0, Ordering::SeqCst);
    }

    /// One draw. As captured: 32 zero bytes regardless of the counter (see
    /// the module docs); the counter still advances for parity with the
    /// capture's draw sequence.
    pub(crate) fn draw(n: usize) -> Vec<u8> {
        let _ = counter().fetch_add(1, Ordering::SeqCst);
        vec![0u8; n]
    }
}

// -- upstream index.ts re-exports -------------------------------------------

pub use auth_provider::{AuthProvider, McpFetch, UnauthorizedContext};
pub use client::{McpClient, McpClientOptions, McpRequestOptions};
pub use protocol::content::{
    to_llm_content, BlobResourceContents, CallToolResult, ContentAnnotations, LlmContent,
    TextResourceContents,
};
pub use protocol::jsonrpc::{
    is_json_rpc_notification, is_json_rpc_request, is_json_rpc_response, parse_json_rpc_message,
    JsonRpcErrorObject, JsonRpcId, JsonRpcMessage, JsonRpcResponse, McpAbortError, McpClientError,
    McpConnectionClosedError, McpError, McpTimeoutError, JSON_RPC_ERROR_CODES_INTERNAL_ERROR,
    JSON_RPC_ERROR_CODES_INVALID_PARAMS, JSON_RPC_ERROR_CODES_INVALID_REQUEST,
    JSON_RPC_ERROR_CODES_METHOD_NOT_FOUND, JSON_RPC_ERROR_CODES_PARSE_ERROR,
};
pub use protocol::jsonrpc::{McpAuthRequiredError, McpHttpError, McpSessionExpiredError};
pub use protocol::types::{
    CancelledNotification, ClientCapabilities, Implementation, InitializeResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, ProgressNotification,
    ReadResourceResult, Resource, ResourceTemplate, Root, ServerCapabilities, Tool,
    ToolAnnotations, ToolExecution, LATEST_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS,
};
pub use transports::stdio::{StdioTransport, StdioTransportOptions};
pub use transports::streamable_http::{StreamableHttpTransport, StreamableHttpTransportOptions};

/// Upstream `./testing` entry: `createInMemoryTransportPair` and
/// `InMemoryTransport`.
pub mod testing {
    pub use crate::mcp::transports::in_memory::{
        create_in_memory_transport_pair, InMemoryTransport,
    };
}

/// Upstream `McpTransport` and the listener types from
/// `transports/transport.ts`.
pub use crate::mcp::transports::{
    McpTransport, TransportCloseListener, TransportErrorListener, TransportMessageListener,
};
