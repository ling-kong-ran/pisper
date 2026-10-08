//! Port of upstream `experimental/coordinator-entry.ts`
//! (sha256 ee772c3d86c3cb504235637593d4bf434bca8c8a470cbbc814fcbded46c28cb0).
//!
//! Ported: the entrypoint role gate and the coordinator process assembly
//! call. Upstream `consumeInternalProcessRole` is the process port's
//! [`crate::coding_agent::experimental::process::consume_internal_process_role`]
//! and `runCoordinatorProcess` is
//! [`crate::coding_agent::experimental::coordinator`]'s
//! `run_coordinator_process` — this module is the entry wiring between them.
//!
//! D14 seam (disclosed in this module's docs): process exit (`process.exit(1)` after
//! logging the error) and `console.error` are embedder-owned; the port
//! exposes the decision and the error text.

use crate::coding_agent::experimental::process::{
    consume_internal_process_role, InternalProcessRole,
};

/// Upstream entry guard: the coordinator entrypoint refuses anything but an
/// internal coordinator invocation, with the exact upstream error.
pub fn require_coordinator_role() -> Result<(), String> {
    let role = consume_internal_process_role()?;
    match role {
        Some(InternalProcessRole::Coordinator) => Ok(()),
        _ => Err("Coordinator entrypoint requires an internal coordinator invocation".to_string()),
    }
}

/// Upstream `runCoordinatorProcess(process.argv.slice(2))` argument slice.
pub fn coordinator_arguments(args: &[String]) -> Vec<String> {
    args.iter().skip(2).cloned().collect()
}

#[cfg(test)]
mod tests;
