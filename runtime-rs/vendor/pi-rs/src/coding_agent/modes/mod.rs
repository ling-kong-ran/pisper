//! Coding-agent modes, being migrated from upstream. JSON event projection
//! is registered first; interactive ports its deterministic core (r16);
//! print and RPC use the real session runtime (including RPC async UI/transport).
pub mod interactive;
pub mod json_event;
#[cfg(test)]
mod json_event_tests;

pub mod print_mode;

pub mod rpc;
