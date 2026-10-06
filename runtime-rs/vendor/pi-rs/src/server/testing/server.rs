//! Port of `packages/server/src/testing/server.ts` (28 lines, SHA256
//! `e1125bb07516ea9071483dec10b35397d6729c4247b5038a1108b64fd229a372`):
//! creates an unstarted `Server` with deterministic defaults for transport
//! conformance tests.

use std::sync::Arc;

use super::super::server::Server;
use super::super::types::{ServerHost, ServerOptions};
use super::host::TestServerHost;

/// Upstream `TestServer` (`server.ts:9-12`).
pub struct TestServer {
    pub server: Arc<Server>,
    pub host: Arc<TestServerHost>,
}

/// Upstream `TestServerOptions` (`server.ts:5-8`): everything but
/// `serverId` is optional; the host defaults to `TestServerHost`.
pub struct TestServerOptions {
    pub listeners: Vec<Arc<dyn super::super::listener::ServerListener>>,
    pub host: Option<Arc<TestServerHost>>,
    pub server_id: Option<String>,
    pub max_frame_length: Option<u64>,
    pub handshake_timeout_ms: Option<u64>,
}

/// Upstream `createTestServer` (`server.ts:16-27`).
pub fn create_test_server(
    options: TestServerOptions,
) -> Result<TestServer, super::super::errors::OperationError> {
    let host = options.host.unwrap_or_else(TestServerHost::new);
    let server = Server::new(
        host.clone() as Arc<dyn ServerHost>,
        ServerOptions::new(
            options.listeners,
            options
                .server_id
                .unwrap_or_else(|| "00000000-0000-4000-8000-000000000001".to_string()),
        )
        .max_frame_length(options.max_frame_length)
        .handshake_timeout_ms(options.handshake_timeout_ms),
    )?;
    Ok(TestServer { server, host })
}
