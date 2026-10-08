//! Client package port. Upstream: `pi/packages/client` (1135 source lines
//! across 8 files), the M6 slice that speaks the framed protocol over a
//! byte transport.
//!
//! # Upstream file map
//!
//! | Upstream | Port |
//! |---|---|
//! | `src/index.ts` | this module's re-exports |
//! | `src/client.ts` | [`client`] |
//! | `src/connection.ts` | [`connection`] |
//! | `src/errors.ts` | [`errors`] |
//! | `src/transport.ts` | [`transport`] |
//! | `src/types.ts` | [`types`] |
//! | `src/promise.ts` | folded into [`connection`]/[`client`] (oneshot resolvers; disclosed divergence D9) |
//! | `src/unix.ts` | [`unix`] (`cfg(unix)`; upstream throws on win32 too) |
//! | `test/support.ts` | [`support`] (`cfg(test)`): `MemoryByteServer` |
//!
//! Deterministic behavior — client hello/request/cancel/subscribe frames,
//! the reconnect/handshake state sequences, and every surfaced error text —
//! is pinned byte-for-byte against a node oracle run of the verbatim copied
//! upstream sources (`tests/fixtures/client_oracle/`, driver `oracle.mjs`, captured
//! output `oracle.out.txt`). The oracle runs the real upstream
//! client/connection/unix code plus the real copied `chord` package; the
//! protocol package's TypeBox schema checker is replaced by a minimal
//! descriptor evaluator (`tests/fixtures/client_oracle/vendor/typebox/`, offline
//! provenance note inside) whose only behavioral surface is Check over the
//! message schemas — every oracle scenario uses schema-valid messages, so
//! the client/connection behavior captured is upstream's own. Message codec
//! byte/parity behavior itself was pinned in the M6 protocol slice.
//!
//! # Upstream sources (SHA256, behavior authority)
//!
//! ```text
//! ce3a4173cba2295f6e765ee51b6919d57e80a2b1f954e1c45b37b21a04c28c82  src/client.ts
//! c6188a63351b2000401c87acb11beb737f681bde9171ee06a9445122209f08c0  src/connection.ts
//! 33727c683e98a2a0d4e09d7b0cbd8f9fd068f7ff8e0b7924d6511d3365c313ee  src/errors.ts
//! e0b0b6773ee864244c10c11a6e17246662faba96fbc7878a6f06c30c26a6b3b6  src/index.ts
//! 4437b54dcf657e0992ecedc7cc27dd863d21765047dfba63835b24926a20075b  src/promise.ts
//! 22ce6eea04d189d71e0037978593ae7844f024d4fed321d4bf4f2c8d7d83ef03  src/transport.ts
//! 465a753902f7827a24357f9ff2745213e5151908d5917cec6968fcba68714d90  src/types.ts
//! 1009a7dfda0f0cb946d2587cbb72a77a91ed5552aa61e8ae84771eed0dd22aec  src/unix.ts
//! 308b1bb3d0b6f498d7671865e09b83cf8f942fec357e75f04eef12bba4afc694  test/client.test.ts
//! 9864826d3bcb4812efe522b004bc9aa83bd4c04f1636d512b034b036e0eb2cd6  test/support.ts
//! 486b007138c9f264be61f0013fc0c1650f265760a04f0ede4c464520fb31c62a  test/unix-transport.test.ts
//! 82d6969098bed9ad9b6197e3e9d2ff003cbd72691826579281f055503edb7c3b  test/unix.test.ts
//! ```
//!
//! # Disclosed seams (S1-S6)
//!
//! - **S1 (chord faces)** — `client.ts` hard-imports `@earendil-works/chord`
//!   (service call constructors/parsers, the subscription snapshot/update
//!   wire validators, and `createServiceStateDecoder`). The full chord port
//!   is a separate M6 slice, so [`service`] carries exactly those faces:
//!   behavior-ported from `chord/src/services/wire.ts`,
//!   `delta/index.ts` (wire grammar + decoder), and
//!   `services/state-codec.ts`, with the state decoder behind the
//!   [`service::ServiceStateDecoder`] factory trait
//!   (`ClientOptions::service_state_decoder_factory`) so the chord slice can
//!   swap the internals. Wire validation texts, snapshot/update decoding
//!   (including `#`-interned path ids), and delivered key order are pinned
//!   against the node oracle, which runs the real upstream chord code.
//! - **S2 (chord `RemoteServiceTransport`)** — the structural type
//!   `createClientServiceTransport` returns is a [`service::RemoteServiceTransport`]
//!   trait here; chord's `BACKGROUND_CONTEXT` is the existing
//!   `agent_core::chord_support::Context::background()` port.
//! - **S3 (AbortSignal)** — the repo's `tokio_util::sync::CancellationToken`
//!   convention replaces `AbortSignal`. A cancellation token carries no
//!   reason payload, so abort rejections use the upstream fallback
//!   `DOMException("The operation was aborted", "AbortError")` text instead
//!   of a user-supplied reason (`types::AbortSignal`).
//! - **S4 (infallible listeners)** — Rust listener closures cannot throw, so
//!   upstream's listener try/catch → `options.onListenerError` paths have no
//!   producer; `onListenerError` remains an honored option surface.
//! - **S5 (delivery task)** — the per-subscription `deliveryTail` promise
//!   chain becomes a delivery worker task serializing decoded updates behind
//!   `ServiceSubscription::start()`; ordering, gating, and drain semantics
//!   match, only the scheduler context differs.
//! - **S6 (unix-only surface)** — [`unix`] is `cfg(unix)`; upstream supports
//!   the same set (it throws "Unix transport is not supported on Windows").
//!   It cannot compile on this Windows host, so the unix tests ride the
//!   upstream `describe.runIf(process.platform !== "win32")` gate and are
//!   validated on a unix target.
//!
//! # Disclosed divergences (D9-D11; D1-D8 live in the protocol module)
//!
//! - **D9** — `promise.ts` (`createPromiseResolvers`) folds into oneshot
//!   channels; there is no Rust `Promise.withResolvers()` face to port.
//! - **D10** — upstream `ok: true` responses without a `result` resolve the
//!   caller promise with `undefined`; the port materializes
//!   [`JsonValue::Null`] (the protocol value tree has no undefined).
//! - **D11** — `Symbol.asyncDispose` has no Rust surface; `dispose()` is a
//!   plain async method with the same idempotence contract.

// Upstream file mapping keeps `client.ts` inside the client package, so the
// module shares its parent's name (same pattern as `src/protocol`).
#[allow(clippy::module_inception)]
pub mod client;
pub mod connection;
pub mod errors;
pub mod service;
pub mod transport;
pub mod types;

#[cfg(unix)]
pub mod unix;

#[cfg(test)]
#[path = "support.rs"]
pub(crate) mod support;

#[cfg(test)]
#[path = "client_tests.rs"]
mod client_tests;

#[cfg(test)]
#[path = "service_tests.rs"]
mod service_tests;

#[cfg(test)]
#[path = "errors_tests.rs"]
mod errors_tests;

#[cfg(unix)]
#[cfg(test)]
#[path = "unix_tests.rs"]
mod unix_tests;

pub use client::{
    create_client_service_transport, Client, DisconnectReason, RequestHandle, ServiceUpdateListener,
};
pub use connection::Connection;
pub use errors::{ClientDisposedError, ClientError, DisconnectedError, ServerError};
pub use service::{
    parse_service_call, ChordServiceStateDecoder, RemoteServiceSubscription,
    RemoteServiceTransport, SeamError, ServiceMode, ServiceStateDecoder,
    ServiceStateDecoderFactory,
};
pub use transport::{ByteTransport, ByteTransportFactory, ByteTransportHandlers};
pub use types::{
    AbortSignal, AttachmentChangeListener, ClientOptions, ConnectionState, ConnectionStateChange,
    ListenerErrorHandler, ServiceSubscription, Unsubscribe,
};
