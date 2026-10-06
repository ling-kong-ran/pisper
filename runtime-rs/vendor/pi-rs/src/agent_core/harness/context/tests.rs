//! Tests for the `context.ts` port: the chord-context re-export surface, the
//! `TODO_CONTEXT` constant, and the abort/cancel helpers from
//! `packages/chord/src/context/index.ts:82-124`. (The upstream
//! `test/harness/context.test.ts` oracle covers only the telemetry helpers,
//! which are deferred to the telemetry task.)

use super::*;
use std::sync::Arc;

#[test]
fn todo_context_answers_no_values_and_displays_upstream_name() {
    let context = todo_context();
    assert!(context.abort_signal().is_none());
    // context/index.ts:56: `new EmptyContext("[Context TODO_CONTEXT]")` with
    // `toString` returning `#name` (lines 28-30).
    assert_eq!(context.to_string(), "[Context TODO_CONTEXT]");
}

#[test]
fn background_context_reexport_keeps_upstream_name() {
    assert_eq!(
        background_context().to_string(),
        "[Context BACKGROUND_CONTEXT]"
    );
}

#[test]
fn with_context_value_derives_a_readable_chain() {
    let key: ContextKey<String> = create_context_key("test.source");
    let context = with_context_value(&key, "first".to_string(), background_context());
    assert_eq!(
        context.get(&key).map(|value| (*value).clone()),
        Some("first".into())
    );
    // The parent chain is unchanged.
    assert!(background_context().get(&key).is_none());
}

#[tokio::test]
async fn with_abort_signal_cancels_the_derived_context_from_the_new_signal() {
    let signal = CancellationToken::new();
    let context = with_abort_signal(signal.clone(), background_context());
    let derived = context
        .abort_signal()
        .expect("derived context has a signal");
    assert!(!derived.is_cancelled());
    signal.cancel();
    // The linker task propagates the cancellation.
    for _ in 0..100 {
        if derived.is_cancelled() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(derived.is_cancelled());
    // The parent context remains unchanged.
    assert!(background_context().abort_signal().is_none());
}

#[tokio::test]
async fn with_abort_signal_cancels_the_derived_context_from_the_parent() {
    let parent = with_abort_signal(CancellationToken::new(), background_context());
    let parent_signal = parent.abort_signal().expect("parent signal");
    let child = with_abort_signal(CancellationToken::new(), parent);
    let child_signal = child.abort_signal().expect("child signal");
    parent_signal.cancel();
    for _ in 0..100 {
        if child_signal.is_cancelled() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(child_signal.is_cancelled());
}

#[tokio::test]
async fn with_abort_signal_short_circuits_when_a_source_is_already_cancelled() {
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let parent = background_context().with_value(abort_signal_key(), Some(cancelled));
    let context = with_abort_signal(CancellationToken::new(), parent);
    let derived = context.abort_signal().expect("derived signal");
    assert!(derived.is_cancelled());
}

#[tokio::test]
async fn without_abort_signal_shadows_the_parent_signal() {
    let parent = with_abort_signal(CancellationToken::new(), background_context());
    let bare = without_abort_signal(parent);
    // An explicit empty value terminates the lookup (upstream
    // `withContextValue(key, undefined)`).
    assert!(bare.abort_signal().is_none());
}

#[tokio::test]
async fn with_cancel_yields_an_independently_cancellable_context() {
    let cancelled = with_cancel(background_context());
    let signal = cancelled
        .context
        .abort_signal()
        .expect("cancel context has a signal");
    assert!(!signal.is_cancelled());
    cancelled.cancel();
    assert!(signal.is_cancelled());
}

#[tokio::test]
async fn await_with_context_resolves_the_future_value_without_a_signal() {
    let value: anyhow::Result<u32> =
        await_with_context(async { 40 + 2 }, background_context()).await;
    assert_eq!(value.unwrap(), 42);
}

#[tokio::test(start_paused = true)]
async fn await_with_context_aborts_the_waiter_but_lets_the_work_finish() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // context/index.ts:108-123: cancellation rejects only the waiter; the
    // underlying promise still settles.
    let finished = Arc::new(AtomicBool::new(false));
    let work_flag = Arc::clone(&finished);
    let cancelled = with_cancel(background_context());
    let signal = cancelled.context.abort_signal().unwrap();
    let waiter = tokio::spawn(await_with_context(
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            work_flag.store(true, Ordering::SeqCst);
        },
        cancelled.context.clone(),
    ));

    // Let the waiter install, then cancel it.
    tokio::task::yield_now().await;
    cancelled.cancel();
    let result: anyhow::Result<()> = waiter.await.unwrap();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().to_string(), "the operation was aborted");
    assert!(signal.is_cancelled());

    // The underlying work still completes: the paused clock auto-advances
    // when every task is parked on a timer.
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    assert!(finished.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn await_with_context_returns_immediately_when_already_cancelled() {
    let signal = CancellationToken::new();
    signal.cancel();
    let context = with_abort_signal(signal, background_context());
    let result: anyhow::Result<()> = await_with_context(async {}, context).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().to_string(), "the operation was aborted");
}

#[tokio::test]
async fn await_with_context_delivers_values_through_a_signal_carrying_context() {
    let cancelled = with_cancel(background_context());
    let value: anyhow::Result<String> =
        await_with_context(async { "done".to_string() }, cancelled.context).await;
    assert_eq!(value.unwrap(), "done");
}
