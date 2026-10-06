//! Chord context. Forwarding module for the already-ported subset of
//! `packages/chord/src/context/index.ts` that lives at
//! [`crate::agent_core::chord_support::context`]
//! (upstream sha256
//! `0f0925a896cf1e506d9495d929157347639e90da343bf28d7472ea645897e7d3`).
//!
//! The chord-support port covers the full deterministic surface of
//! `context/index.ts`: `Context`/`ContextKey` ([`types.ts:8-19`]),
//! `createContextKey`, `withContextValue`, `BACKGROUND_CONTEXT`,
//! `TODO_CONTEXT`, the `abortSignal` lookup, and `toString`. The
//! abort-signal helpers (`withAbortSignal`, `withoutAbortSignal`,
//! `withCancel`, `awaitWithContext`) are ported with
//! `src/agent_core/harness/context.rs`, bound to the repo's
//! [`tokio_util::sync::CancellationToken`] convention (they are
//! `AbortController`/event-loop features, not value-chain features).
//!
//! [`types.ts:8-19`]: crate::chord::types

pub use crate::agent_core::chord_support::context::*;
