//! Port of the used surface of upstream `packages/chord` (the
//! `@earendil-works/chord` package): the context DI core and the delta
//! replicated-state vocabulary that the harness subtree
//! (`packages/agent/src/harness/`) builds on.
//!
//! **This is the chord-support subset only. The full chord port (services,
//! facets, wire codec, remote bindings) remains M6.** Every ported item cites
//! its upstream source file and lines.
//!
//! Ported surface (grep of `@earendil-works/chord` under
//! `packages/agent/src/harness/`):
//!
//! | Upstream | Rust | Cited source |
//! | --- | --- | --- |
//! | `Context` (type) | [`context::Context`] | `types.ts:15-19`, `context/index.ts:7-53` |
//! | `ContextKey<T>` (type) | [`context::ContextKey`] | `types.ts:8-12` |
//! | `createContextKey` | [`context::create_context_key`] | `context/index.ts:58-60` |
//! | `withContextValue` | [`context::Context::with_value`] | `context/index.ts:63-65` |
//! | `BACKGROUND_CONTEXT` | [`context::Context::background`] | `context/index.ts:55` |
//! | `Context#abortSignal` | [`context::Context::abort_signal`] | `context/index.ts:11-13` |
//! | `Op`, `Path`, `Seg` (types) | [`delta::Op`], [`delta::Path`], [`delta::Seg`] | `delta/index.ts:12-14,30-36` |
//! | `track` / `Tracker` | [`delta::track`] / [`delta::Tracker`] | `delta/index.ts:108-127,310-1179` |
//! | `applyImmutable` | [`delta::apply_immutable`] | `delta/index.ts:1444-1494` |
//! | `isBase` | [`delta::is_base`] | `delta/index.ts:70` |
//! | `JsonValue` (type) | [`JsonValue`] | `types.ts:21` |
//! | `JsonRepresentation<T>` (type) | documented below | `types.ts:26-36` |
//!
//! # `JsonValue` and `JsonRepresentation<T>`
//!
//! Upstream `JsonValue` (`types.ts:21`) is the union of strict JSON values.
//! The port's [`JsonValue`] is `serde_json::Value`: `null`/`bool`/`number`/
//! `string` map 1:1, arrays to [`serde_json::Value::Array`], and objects to
//! [`serde_json::Value::Object`]. One disclosed difference: serde_json's
//! default object map sorts keys (BTreeMap) while a JS engine keeps insertion
//! order. Chord's own contract says object key insertion order is not
//! replicated (`packages/chord/src/delta/README.md` "Limits"), so this is
//! invisible to the delta semantics.
//!
//! Upstream `JsonRepresentation<T>` (`types.ts:26-36`) is a compile-time
//! mapping from an application type to its strict-JSON shape (unknown payloads
//! degrade to `JsonValue`). Rust has no need for the mapped type: the same
//! guarantee is expressed by the serde model, i.e. `T: serde::Serialize` with
//! `serde_json` as the representation, and `serde_json::Value` as the target
//! type for payloads whose shape is not statically known.
//!
//! # Deferred (not in the harness import list; M6 unless noted)
//!
//! - `apply` (in-place mutable applier) — the harness only imports
//!   `applyImmutable`; the port's `apply_immutable` clones the previous value
//!   per call (see the delta module docs).
//! - `encoder`/`decoder`/`WireOp` (path interning + arity omission codec) —
//!   used by chord services, not by the harness subtree.
//! - `overlap` is ported as a private helper (the harness never imports it;
//!   upstream exports it).
//! - `TODO_CONTEXT` is ported as [`context::Context::todo`]; the abort-signal
//!   helpers (`withAbortSignal`, `withoutAbortSignal`, `withCancel`,
//!   `awaitWithContext`) are ported with `src/agent_core/harness/context.rs`,
//!   which binds them to the repo's
//!   [`tokio_util::sync::CancellationToken`] convention.
//! - `isJsonValue`, `ReplicatedState`, `MutableReplicatedState`,
//!   `defineService`, facets, services — M6 / later harness tasks.

pub mod context;
pub mod delta;

pub use context::*;
pub use delta::*;

/// Upstream `JsonValue` (`packages/chord/src/types.ts:21`): the strict JSON
/// value union, mapped to `serde_json::Value`. See the module docs for the
/// key-ordering note.
pub type JsonValue = serde_json::Value;
