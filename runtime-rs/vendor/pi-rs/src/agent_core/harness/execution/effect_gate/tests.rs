//! Port of the `createGate` block of
//! `packages/agent/test/harness/execution-primitives.test.ts:24-52` (the
//! rest of that oracle file — hook-registry and event-bus coverage — landed
//! with M3b Tasks 2 and 4).

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::agent_core::harness::hooks;

/// The oracle's `new Error("closed")` stand-in, so close-error identity can
/// be asserted with `Arc::ptr_eq` (upstream compares thrown-object identity).
#[derive(Debug)]
struct TestError(&'static str);

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl StdError for TestError {}

/// Oracle "closes starts synchronously and signals only after cancellation
/// commits" (`execution-primitives.test.ts:25-43`).
#[tokio::test]
async fn closes_starts_synchronously_and_signals_only_after_cancellation_commits() {
    let (gate, control) = create_gate();
    let cancellation = CancellationToken::new();
    control.begin_abort(cancellation.clone());

    let refusal = gate.admit(|| ()).expect_err("admit must refuse");
    let GateRejection::AbortRequested(abort) = refusal else {
        panic!("expected AbortRequested, got {refusal:?}");
    };
    // `(refusal as AbortRequested).cancellation).toBe(cancellation.promise)`:
    // identity via behavior — cancelling the owner token resolves the
    // rejection's handle.
    cancellation.cancel();
    abort.cancellation.cancelled().await;
    assert!(!gate.signal().is_cancelled());
    control.signal_abort();
    assert!(gate.signal().is_cancelled());
}

/// Oracle "permanently closes and signals admitted work"
/// (`execution-primitives.test.ts:45-51`).
#[test]
fn permanently_closes_and_signals_admitted_work() {
    let (gate, control) = create_gate();
    let error: Arc<dyn StdError + Send + Sync> = Arc::new(TestError("closed"));
    control.close(Arc::clone(&error));
    let refusal = gate.admit(|| ()).expect_err("admit must refuse");
    match refusal {
        GateRejection::Closed(seen) => assert!(Arc::ptr_eq(&seen, &error)),
        other => panic!("expected Closed, got {other:?}"),
    }
    assert!(gate.signal().is_cancelled());
}

/// The trait the hooks registry consumes (M3b Task 4): the rejection stays
/// downcastable through `anyhow`, and closing cancels the trait signal.
#[test]
fn implements_the_hooks_gate_trait() {
    let (gate, control) = create_gate();
    let trait_gate: Arc<dyn hooks::Gate> = Arc::new(gate);
    let error: Arc<dyn StdError + Send + Sync> = Arc::new(TestError("closed"));
    control.close(error);
    let refusal = trait_gate.admit().expect_err("trait admit must refuse");
    let rejection = refusal
        .downcast_ref::<GateRejection>()
        .expect("rejection stays typed through anyhow");
    assert!(matches!(rejection, GateRejection::Closed(_)));
    assert!(trait_gate.signal().is_cancelled());
}
