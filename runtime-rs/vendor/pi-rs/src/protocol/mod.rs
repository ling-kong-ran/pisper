//! Protocol package port. Upstream: `pi/packages/protocol` (869 source lines),
//! the M6 leaf dependency shared by the server, client, and evals slices.
//!
//! # Upstream file map
//!
//! | Upstream | Port |
//! |---|---|
//! | `src/index.ts` | this module's re-exports |
//! | `src/cbor/index.ts`, `options.ts`, `encoder.ts`, `decoder.ts` | [`cbor`] |
//! | `src/framing.ts` | [`framing`] |
//! | `src/protocol.ts` | [`protocol`] (schema types + runtime validation) |
//! | `src/codec.ts` | [`codec`] (validated parse/encode, framed decoders) |
//!
//! JS module semantics that have no Rust equivalent are carried by two value
//! trees: [`json::JsonValue`] (ordered strict-JSON payloads; upstream leans on
//! plain JS objects; integer-index keys enumerate before other string keys) and
//! [`cbor::CborValue`] (adds CBOR byte strings). Wire bytes, field order, and
//! error text are pinned byte-for-byte against a node oracle run of the copied
//! upstream source (`tests/fixtures/protocol_oracle/`).
//!
//! # Upstream sources (SHA256, behavior authority)
//!
//! ```text
//! cae1bd6aa5b9d449817ee53f3483a36dc6673136c364a93fbbecb12d34aaf434  src/index.ts
//! d91623d612ba23fdd2129bd58b661c7f3d5e1603030d3592f923a73263a349c3  src/protocol.ts
//! 570397ea3d5df9924b0c31841a0da68543b338670f921e3fda2cbe17e34273e2  src/codec.ts
//! 30214ce7400fc69be1a7571a3d1ebe7a25626a3f12aa73373a4bf28b0db26388  src/framing.ts
//! 604a0d1c3bfe00f4e0b19d07426c1904f2c4db12d1eac6c62ce7e413420107b2  src/cbor/index.ts
//! 3e26072fd679ae28b7b4a0bb810fbb6adf144aaf50a873f0ee0f23b0fa1d3f6d  src/cbor/options.ts
//! 0777eb437cc4f954273e818b1d22825e2fd2264eace11cdf5433dc30174a198e  src/cbor/encoder.ts
//! 47751a6b356d2b911de7887ec748a59be63d41d4a778ab5496043dd07d345744  src/cbor/decoder.ts
//! 9e1edc0e0eb3b88a8e8df03fb9e29e57c68f285a77052165e21c0a363a7152c9  test/cbor/cbor.test.ts
//! bd7a5f205ad8349123e2505fbaba526532bfca0060da8b9655044ec0e1770071  test/framing.test.ts
//! b0205487bee8361a77bcceb71e3e7ca74dfa480b4c57cb8c7adb5e0ce49e4c92  test/protocol.test.ts
//! ```
//!
//! # Disclosed divergences (D1-D9)
//!
//! - **D1** — the encoder's text round-trip check (`textDecoder.decode(bytes)
//!   !== value`, guarding lone UTF-16 surrogates) is structurally
//!   unobservable: a Rust `&str` cannot hold unpaired surrogates. The
//!   "valid Unicode scalar values" error is unreachable.
//! - **D2** — `undefined` handling is construction-side: map entries with
//!   `undefined` values cannot exist in [`json::JsonValue`], so the
//!   "arrays must not contain holes or undefined values" encoder error is
//!   unreachable. The observable wire rule (undefined object properties are
//!   omitted, falsey values kept) holds by construction and is pinned by test.
//! - **D3** — JS-only value kinds (bigint, symbol, function, Date, Map,
//!   enumerable symbol keys, cyclic references) are unrepresentable in the
//!   owned value trees, so the corresponding `CborError`s ("Unsupported CBOR
//!   value type", "must not contain cycles", symbol-key rejection) and the
//!   input `TypeError`s are unreachable.
//! - **D4** — `RangeError` cases for non-integer option values (< 0, 1.5,
//!   NaN) are type-level impossible (`u64` fields); the representable
//!   overflows (> u32::MAX, depth > 512) still fail at resolve time with the
//!   identical messages.
//! - **D5** — the `isJsonValue` gate (finite strict JSON, no cycles, plain
//!   prototypes) is inherent to the [`json::JsonValue`] tree. Non-JSON values
//!   arriving over CBOR (byte strings, non-finite floats) are rejected during
//!   [`cbor::cbor_value_to_json`] with the identical "Invalid client/server
//!   protocol message" error.
//! - **D6** — [`json::JsonValue`] objects retain their stored entry order.
//!   [`json::JsonValue::to_serde_json`] now retains it too, because
//!   serde_json/preserve_order is enabled (the former sorted-map loss is gone).
//!   JS integer-index property enumeration is not automatically implemented
//!   by either representation; protocol/CBOR boundaries still require their
//!   own audit. The modes-specific normalization does not fix this package.
//! - **D7** — [`framing::FrameDecoder`] accumulates payloads in one growable
//!   buffer instead of upstream's 64 KiB block list; emitted frames are
//!   byte-identical, only the internal allocation strategy differs.
//! - **D8** — upstream's two-variant `ResponseEnvelope` object union is
//!   modeled as one struct with a [`protocol::ResponseOutcome`] enum; the
//!   accepted/rejected message set is unchanged.
//! - **D9** — client-hello `version` values that fit "integer >= 0" upstream
//!   but exceed `u64` (e.g. `1e30`) are rejected at parse instead of accepted;
//!   both fail [`codec::is_supported_protocol_version`], so negotiation
//!   behavior is unchanged.

pub mod cbor;
pub mod codec;
pub mod framing;
pub mod json;
// Upstream file mapping keeps `protocol.ts` inside the protocol package, so
// the module shares its parent's name.
#[allow(clippy::module_inception)]
pub mod protocol;

pub use cbor::{
    cbor_value_to_json, decode_cbor, encode_cbor, json_value_to_cbor, CborError, CborFailure,
    CborOptions, CborValue, RangeError, DEFAULT_MAX_CBOR_BYTE_LENGTH,
    DEFAULT_MAX_CBOR_CONTAINER_LENGTH, DEFAULT_MAX_CBOR_DEPTH,
};
pub use codec::{
    encode_client_message, encode_server_message, is_supported_protocol_version,
    parse_client_message, parse_server_message, ClientMessageDecoder, ProtocolValidationError,
    ServerMessageDecoder,
};
pub use framing::{
    encode_frame, FrameDecoder, FrameDecoderOptions, FrameError, DEFAULT_MAX_FRAME_LENGTH,
};
pub use json::{JsonValue, Number as JsonNumber};
pub use protocol::{
    AttachmentEnvelope, CancelEnvelope, ClientHello, ClientMessage, ProtocolError,
    ProtocolErrorCode, RequestEnvelope, ResponseEnvelope, ResponseOutcome, RpcTarget, ServerHello,
    ServerHelloError, ServerId, ServerMessage, ServiceEventEnvelope, SessionTarget,
    PROTOCOL_VERSION,
};
