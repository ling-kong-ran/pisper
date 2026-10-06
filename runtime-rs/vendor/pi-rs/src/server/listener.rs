//! Port of `packages/server/src/listener.ts` (8 lines, SHA256
//! `5ef29dd5cf420b4119942b906dbf741bc9e20e6ea225e1292ae317c16170136c`):
//! supplies established byte connections after any required transport
//! authentication.

use std::sync::Arc;

use futures::future::BoxFuture;

use super::connection::ByteConnectionAcceptor;
use super::errors::OperationError;

/// Upstream `ServerListener` (`listener.ts:5-10`).
pub trait ServerListener: Send + Sync + 'static {
    /// Starts listening and passes authorized connections to `accept`.
    fn start(
        &self,
        accept: ByteConnectionAcceptor,
    ) -> BoxFuture<'static, Result<(), OperationError>>;
    fn close(&self) -> BoxFuture<'static, Result<(), OperationError>>;
}

/// Owned handle form used by [`super::types::ServerOptions`].
pub type ServerListenerHandle = Arc<dyn ServerListener>;
