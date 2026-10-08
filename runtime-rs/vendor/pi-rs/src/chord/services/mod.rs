//! Chord services: the deterministic provider/wire/codec core of
//! `packages/chord/src/services/`. See the child module docs for the per-file
//! ports; see the [`crate::chord`] module docs for the slice boundary (the
//! consumer-side binding `services/consumer.ts`, `handle.ts`, `instances.ts`
//! and `loopback.ts` are the deferred consumer seam).

pub mod errors;
pub mod provider;
pub mod state;
pub mod state_codec;
pub mod wire;

#[cfg(test)]
mod oracle_tests;
