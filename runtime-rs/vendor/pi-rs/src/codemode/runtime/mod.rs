//! Codemode runtime submodules.
//!
//! [`prelude`] ports `runtime/prelude-source.ts` (byte-exact embedded JS),
//! [`protocol`] ports `runtime/protocol.ts`, and [`engine`] ports
//! `runtime/host.ts` + `runtime/worker.ts` + `wasm.ts` onto the embedded
//! rquickjs engine.

pub mod engine;
pub mod prelude;
pub mod protocol;
