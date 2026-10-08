//! Server package port. Upstream: `pi/packages/server` (1966 source lines
//! across 16 files), the M6 slice hosting the protocol server: session
//! routing, request dispatch, subscription broadcasting, and the Unix
//! transport.
//!
//! # Upstream file map
//!
//! | Upstream | Port |
//! |---|---|
//! | `src/index.ts` | this module's re-exports |
//! | `src/errors.ts` | [`errors`] |
//! | `src/connection.ts` | [`connection`] |
//! | `src/listener.ts` | [`listener`] |
//! | `src/types.ts` | [`types`] |
//! | `src/session-router.ts` | [`session_router`] |
//! | `src/server.ts` | [`server`] |
//! | `src/transports/unix/*` | [`unix`] (`cfg(unix)`) |
//! | `src/testing/*` | [`testing`] + the ported test files |
//! | `test/conformance.test.ts` | [`conformance_tests`] |
//! | `test/protocol.test.ts` | [`protocol_tests`] |
//! | `test/server.test.ts` | [`server_tests`] |
//! | `test/listener.test.ts` | [`listener_tests`] |
//! | `test/unix-connection.test.ts` | [`unix_connection_tests`] (`cfg(unix)`) |
//! | `test/unix.test.ts` | [`unix_tests`] (`cfg(unix)`) |
//!
//! Deterministic behavior — hello/hello_error frames, response envelopes and
//! their full error texts, dispatch order, subscription snapshot/update
//! frames (through the real ported chord state codec), and shutdown error
//! texts — is pinned byte-for-byte against a node oracle run of the verbatim
//! copied upstream sources (`tests/fixtures/server_oracle/`, driver `oracle.mjs`,
//! captured output `oracle.out.txt`). The oracle runs the real upstream
//! server code against the real copied `pi-protocol` + `chord` packages, a
//! minimal `pi-agent-core` shim, and the `typebox` descriptor evaluator from
//! the client slice (decode-path error texts are fixed strings in the real
//! codec, so no TypeBox text reaches the captured output).
//!
//! # Upstream sources (SHA256, behavior authority)
//!
//! ```text
//! b13a4766dbbc2f10cbf316f1c1c003a75f854745b7c1d1f864bd0004ce1238fd  src/connection.ts
//! 1ad450b65d0fb3de78628ec38584e87adb2d5d8b360f6afdc0b6966b9c2b3fce  src/errors.ts
//! ebd84214dcf8626ac98b14c4ec4c866521c49328684612c751ab6bf711bdd6e9  src/index.ts
//! 5ef29dd5cf420b4119942b906dbf741bc9e20e6ea225e1292ae317c16170136c  src/listener.ts
//! bd428151016734ddc1532b78c22d2c2ac539117d9d13a276928ce55c7b3d938d  src/server.ts
//! c9ea719130708e7d8f548e5b65f23afe0c8d96f0f87ae3e611bfde6ca3dad69b  src/session-router.ts
//! cdec325901d0035ab381069e2dec9771027bde80025f4443c7fb87b145e46404  src/types.ts
//! 2d240732193839d4105183a089d6b21e7170fff6c933b9a7e76832d0c960fedf  src/testing/client.ts
//! 627979793dac2b98568c03d9ed6d57ee1d0c05cd77106808a4478a93836db14a  src/testing/host.ts
//! 17f9d7db7663715b71eb18625ba76f98708b8a3dddb6b6980dc2edf6f3c07064  src/testing/index.ts
//! e1125bb07516ea9071483dec10b35397d6729c4247b5038a1108b64fd229a372  src/testing/server.ts
//! 2d307bb684e4a6296695ad0f921547c573bdbdb843001873aa37d129a535cf13  src/transports/unix/address.ts
//! de71c68354820cede8eb7aeebb4ebe525cd6f832ce1691b60b5279816fd646eb  src/transports/unix/listener.ts
//! 6c0010fe5c9a49d2af6c2f6793cd36723061fc22b8400528ac15f970f85054e8  src/transports/unix/preset.ts
//! 067fff91eefffc09f1d7952cc8058d3333cee15f568a67711644550eba67423f  src/transports/unix/types.ts
//! 849af98dab82de8be93e62ebfd67abdce4395781d928e0d51e5d5de5192d1174  test/conformance.test.ts
//! 4cb835caa4f3d59e33dbcac31490e2d333b6741322a5f324a996a280a5bf1890  test/listener.test.ts
//! 33157c44d161baf8c4a838df7c5d852bbfdecb1b62da8cd7abc0a680cbbbac48  test/protocol.test.ts
//! 468f0cf48a78449123b87a6c66ba5a6c18a6efb4fa0d678364611cf7dc5fdcfd  test/server.test.ts
//! 93f52cbf853ea97ab39de6b5b2b16bef65e3850bb110854aeaa5967628851da3  test/unix-connection.test.ts
//! 86ca4b6d7ffc3d3e2c4eff60eaf260cc1a56aea995d46a09a9edf1a2e9f73eaf  test/unix.test.ts
//! ```
//!
//! # Disclosed seams (S-A..S-D)
//!
//! - **S-A (metadata identity)** — upstream `ServerHost<TMetadata>` is
//!   generic over the metadata record and hands the *same object* to
//!   `openSession`; the port carries the concrete
//!   `agent_core::harness::session::types::SessionMetadata` value, so the
//!   "passes concrete repository metadata" guarantee is value equality.
//! - **S-B (context/abort)** — upstream `Context`, `BACKGROUND_CONTEXT`,
//!   `TODO_CONTEXT`, and `withAbortSignal` are the existing
//!   `agent_core::chord_support`/`agent_core::harness::context` ports;
//!   `AbortController` is the repo's `tokio_util::sync::CancellationToken`
//!   convention (no reason payload — the client slice's S3).
//! - **S-C (chord consumption)** — the routed attachment/handle/host
//!   capability faces (`types.ts`) are traits over the chord value trees;
//!   service-call values cross between the protocol [`crate::protocol::json`]
//!   tree and the chord `serde_json` tree at the envelope boundary
//!   (`to_serde_json`/`from_serde_json`), the same conversion boundary the
//!   protocol module documents (its D6: key order degrades only at that
//!   consumer boundary).
//! - **S-D (scheduler)** — upstream fire-and-forget promises become tokio
//!   tasks; per-client router operations keep upstream's *registration*
//!   order (entry points are synchronous functions that splice onto the
//!   chain before returning a future), and progress requires a live
//!   runtime/poller where a detached JS promise would settle anyway.
//!
//! # Disclosed divergences (D-A..D-G; D1-D11 live in protocol/chord/client)
//!
//! - **D-A** — upstream option validation `TypeError`s for non-array
//!   `listeners` are unreachable (the Rust field is always a `Vec`); the
//!   `serverId`/`maxFrameLength`/`handshakeTimeoutMs` texts are pinned
//!   against the oracle.
//! - **D-B** — unix-connection write serialization uses one writer task
//!   (upstream: a promise tail); limits, error texts, and close ordering
//!   are identical (see [`unix`]).
//! - **D-C** — `disconnected` + `stage` fold into
//!   [`connection::ConnectionStage`] (upstream only ever sets them
//!   together in `disconnect()`).
//! - **D-D** — attachment ids come from a local pseudo-random UUIDv4
//!   formatter (upstream `node:crypto` `randomUUID`; the same disclosed
//!   substitution as `ai::uuid`). Ids are opaque protocol strings; the
//!   oracle masks their bytes.
//! - **D-E** — tracked in-flight operations remove themselves from the
//!   attachment's operation set via id (upstream: promise-set
//!   self-removal); release drains whatever remains — identical wait
//!   semantics.
//! - **D-F** — `Server::closed` is a future over a watch channel instead
//!   of a promise; `close()` and `closed()` return the identical
//!   [`errors::ShutdownError`] value.
//! - **D-G** — upstream `SessionRouter::close` settles client operations
//!   and opening futures concurrently and classifies `SessionCleanupError`
//!   rejections from both; the port drains the per-client operation queues
//!   first and the opening futures second (the same S-D registration-order
//!   scheduling), with the identical classification and aggregate error.
//! - **D-H** — the `Timeout`/unhandled-rejection JS faces for observer
//!   callbacks that throw are unrepresentable (Rust closures cannot
//!   throw; the client slice's S4).

pub mod connection;
pub mod errors;
pub mod listener;
#[allow(clippy::module_inception)]
pub mod server;
pub mod session_router;
pub mod testing;
pub mod types;
#[cfg(unix)]
pub mod unix;

pub use connection::{
    ByteConnection, ByteConnectionAcceptor, ByteConnectionHandler, ConnectionStage,
};
pub use errors::{OperationError, ServerError, ShutdownError, INTERNAL_SERVER_ERROR_MESSAGE};
pub use listener::ServerListener;
pub use server::Server;
pub use session_router::ClientId;
pub use types::{
    ConnectionCountHandler, ErrorObserver, PublishCallback, RoutedServerPresentation,
    RoutedServerServiceAttachment, RoutedServerServiceHost, RoutedSessionAttachment,
    RoutedSessionHandle, ServerHost, ServerOptions,
};

/// Re-exports of the testing doubles (upstream `testing/index.ts`).
pub use testing::{
    client::{ProtocolTestClient, WireChannel},
    host::{create_test_server_services, Deferred, OpenGate, TestHarness, TestServerHost},
    server::{create_test_server, TestServer, TestServerOptions},
};

#[cfg(test)]
#[path = "conformance_tests.rs"]
mod conformance_tests;

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;

#[cfg(test)]
#[path = "server_tests.rs"]
mod server_tests;

#[cfg(test)]
#[path = "listener_tests.rs"]
mod listener_tests;

#[cfg(unix)]
#[cfg(test)]
#[path = "unix_connection_tests.rs"]
mod unix_connection_tests;

#[cfg(unix)]
#[cfg(test)]
#[path = "unix_tests.rs"]
mod unix_tests;
