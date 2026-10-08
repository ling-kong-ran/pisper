//! Semantics tests for the chord context port, derived from
//! `packages/chord/src/context/index.ts` behavior and the repo conventions in
//! `packages/chord/test/context.test.ts`.

use super::*;

#[test]
fn background_context_answers_no_keys() {
    // EmptyContext#value returns undefined (context/index.ts:24-26).
    let key: ContextKey<String> = create_context_key("k");
    assert!(Context::background().get(&key).is_none());
}

#[test]
fn background_context_display_matches_upstream_name() {
    // BACKGROUND_CONTEXT = new EmptyContext("[Context BACKGROUND_CONTEXT]")
    // (context/index.ts:55).
    assert_eq!(
        Context::background().to_string(),
        "[Context BACKGROUND_CONTEXT]"
    );
}

#[test]
fn keys_are_unique_per_creation_even_with_the_same_description() {
    // Symbols are unique per Symbol() call (context/index.ts:59).
    let a: ContextKey<u8> = create_context_key("same");
    let b: ContextKey<u8> = create_context_key("same");
    assert_ne!(a.id(), b.id());
    assert_ne!(a, b);
    assert_eq!(a.description(), "same");
    let base = Context::background().with_value(&a, 1u8);
    assert!(base.get(&b).is_none());
    assert_eq!(*base.get(&a).expect("own key resolves"), 1);
}

#[test]
fn with_value_derives_a_context_with_one_more_value() {
    // ContextValue#value resolves own key and delegates to the parent
    // (context/index.ts:45-48).
    let a: ContextKey<&'static str> = create_context_key("a");
    let b: ContextKey<&'static str> = create_context_key("b");
    let ctx = Context::background()
        .with_value(&a, "one")
        .with_value(&b, "two");
    assert_eq!(*ctx.get(&a).expect("parent value visible"), "one");
    assert_eq!(*ctx.get(&b).expect("own value"), "two");
}

#[test]
fn with_value_on_the_same_key_shadows_the_parent() {
    // A later derivation for the same key wins because the walk stops at the
    // first token match (context/index.ts:45-48).
    let key: ContextKey<u32> = create_context_key("k");
    let ctx = Context::background()
        .with_value(&key, 1u32)
        .with_value(&key, 2u32);
    assert_eq!(*ctx.get(&key).expect("shadowed value"), 2);
}

#[test]
fn sibling_derivations_are_independent() {
    let key: ContextKey<u32> = create_context_key("k");
    let base = Context::background();
    let left = base.clone().with_value(&key, 1u32);
    let right = base.with_value(&key, 2u32);
    assert_eq!(*left.get(&key).expect("left"), 1);
    assert_eq!(*right.get(&key).expect("right"), 2);
}

#[test]
fn display_shows_the_derivation_chain() {
    // ContextValue#toString: `${parent}.WithValue(${token.description})`
    // (context/index.ts:50-52).
    let key: ContextKey<bool> = create_context_key("pico3.session.line");
    let ctx = Context::background().with_value(&key, true);
    assert_eq!(
        ctx.to_string(),
        "[Context BACKGROUND_CONTEXT].WithValue(pico3.session.line)"
    );
}

#[test]
fn abort_signal_is_absent_on_background_context() {
    // BaseContext abortSignal reads the key and gets undefined
    // (context/index.ts:11-13,55).
    assert!(Context::background().abort_signal().is_none());
}

#[test]
fn abort_signal_is_readable_through_the_chain() {
    // A context derived with the abort key carries the signal
    // (context/index.ts:71-75 behavior, minus the combination helper).
    let token = CancellationToken::new();
    let ctx = Context::background().with_value(abort_signal_key(), Some(token.clone()));
    let read = ctx.abort_signal().expect("signal present");
    assert!(!read.is_cancelled());
    token.cancel();
    assert!(read.is_cancelled());
}

#[test]
fn abort_signal_is_inherited_from_the_parent_chain() {
    let token = CancellationToken::new();
    let parent = Context::background().with_value(abort_signal_key(), Some(token));
    let key: ContextKey<&'static str> = create_context_key("other");
    let child = parent.with_value(&key, "x");
    assert!(child.abort_signal().is_some());
}

#[test]
fn an_explicit_none_shadows_a_parent_abort_signal() {
    // withContextValue(ABORT_SIGNAL_CONTEXT_KEY, undefined, context)
    // (context/index.ts:78-80) terminates the walk at the first match, and the
    // match carries "no signal".
    let parent =
        Context::background().with_value(abort_signal_key(), Some(CancellationToken::new()));
    let child = parent.with_value(abort_signal_key(), None);
    assert!(child.abort_signal().is_none());
}

#[test]
fn key_ids_are_allocated_monotonically() {
    // Sanity for the counter: later creations get strictly larger ids
    // (upstream symbols are simply unique).
    let first: ContextKey<u8> = create_context_key("probe");
    let second: ContextKey<u8> = create_context_key("probe");
    assert!(second.id() > first.id());
}

#[test]
fn value_storage_survives_sending_the_context_across_threads() {
    // Context must be Send + Sync: it is passed through async task boundaries
    // in the harness (upstream passes it through promises).
    let key: ContextKey<u64> = create_context_key("t");
    let ctx = Context::background().with_value(&key, 7u64);
    let handle = std::thread::spawn(move || {
        assert_eq!(*ctx.get(&key).expect("value crosses threads"), 7);
    });
    handle.join().expect("thread succeeds");
}

#[test]
fn abort_key_counter_stays_stable() {
    // The abort key is a process-wide singleton like the upstream frozen
    // constant (context/index.ts:3-5).
    assert_eq!(abort_signal_key(), abort_signal_key());
    assert_eq!(abort_signal_key().description(), "chord.abortSignal");
}

#[test]
fn monotonic_ids_never_repeat_within_a_process() {
    let a: ContextKey<u8> = create_context_key("m");
    let b: ContextKey<u8> = create_context_key("m");
    assert_ne!(a.id(), b.id());
}
