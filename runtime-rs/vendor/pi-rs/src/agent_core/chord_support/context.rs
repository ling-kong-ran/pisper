//! Chord context DI core. Port of `packages/chord/src/context/index.ts` and
//! the `Context`/`ContextKey` declarations in
//! `packages/chord/src/types.ts:8-19`.
//!
//! Upstream `Context` is an immutable linked list ("invocation-scoped values
//! passed explicitly through operations"): `BACKGROUND_CONTEXT` terminates the
//! chain and `withContextValue` prepends one keyed value. The port models the
//! chain as a cloned [`Context`] enum whose [`Context::WithValue`] variant
//! holds an `Arc` parent, so derivation is O(1) and cloning a context shares
//! the chain.
//!
//! Deferred from this subset (see the parent module docs): the abort-signal
//! helpers `withAbortSignal`, `withoutAbortSignal`, `withCancel`,
//! `awaitWithContext` (ported with `src/agent_core/harness/context.rs`), and
//! `TODO_CONTEXT` (ported here as [`Context::todo`]). The `abortSignal`
//! *lookup* is ported ([`Context::abort_signal`]); the signal-combining
//! helpers bind to the repo's [`tokio_util::sync::CancellationToken`]
//! convention.

use std::any::Any;
use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use tokio_util::sync::CancellationToken;

/// Typed identity for one value carried by a [`Context`].
///
/// Port of upstream `ContextKey<T>` (`packages/chord/src/types.ts:8-12`):
/// identity is the `token` symbol; the `valueType` marker is type-level only,
/// which `PhantomData<fn() -> T>` covers (invariance without affecting
/// `Send`/`Sync`).
///
/// Upstream creates keys with `createContextKey` (`context/index.ts:58-60`),
/// which freezes an object with a fresh `Symbol(description)`. The port's
/// equivalent is [`create_context_key`] / [`ContextKey::new`]; uniqueness comes
/// from a process-global counter instead of symbol identity.
#[derive(Debug)]
pub struct ContextKey<T> {
    id: u64,
    description: Cow<'static, str>,
    marker: PhantomData<fn() -> T>,
}

static NEXT_KEY_ID: AtomicU64 = AtomicU64::new(0);

fn next_key_id() -> u64 {
    NEXT_KEY_ID.fetch_add(1, Ordering::Relaxed)
}

impl<T> ContextKey<T> {
    /// `createContextKey(description)` (`context/index.ts:58-60`).
    pub fn new(description: impl Into<Cow<'static, str>>) -> Self {
        ContextKey {
            id: next_key_id(),
            description: description.into(),
            marker: PhantomData,
        }
    }

    /// The unique token id (upstream `key.token` symbol identity).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The token description (upstream `Symbol(description).description`),
    /// used in [`fmt::Display`] for contexts derived with this key.
    pub fn description(&self) -> &str {
        &self.description
    }
}

impl<T> Clone for ContextKey<T> {
    fn clone(&self) -> Self {
        ContextKey {
            id: self.id,
            description: self.description.clone(),
            marker: PhantomData,
        }
    }
}

impl<T> PartialEq for ContextKey<T> {
    /// Identity by token, like upstream symbol identity. Keys of different
    /// value types never compare equal because `T` must match too.
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for ContextKey<T> {}

/// `createContextKey<T>(description)` (`context/index.ts:58-60`).
pub fn create_context_key<T>(description: impl Into<Cow<'static, str>>) -> ContextKey<T> {
    ContextKey::new(description)
}

/// Immutable invocation-scoped values passed explicitly through operations.
///
/// Port of upstream `Context` (`packages/chord/src/types.ts:15-19`) and the
/// `BaseContext`/`EmptyContext`/`ContextValue` classes
/// (`context/index.ts:7-53`). The trait is a closed enum because the chain has
/// exactly two node kinds upstream.
#[derive(Clone)]
pub enum Context {
    /// Upstream `EmptyContext` (`context/index.ts:16-31`): answers no keys.
    /// Two instances exist upstream, `BACKGROUND_CONTEXT`
    /// ([`Context::background`], `context/index.ts:55`) and `TODO_CONTEXT`
    /// ([`Context::todo`], `context/index.ts:56`).
    Background {
        /// `#name`, shown by `toString` (`context/index.ts:28-30`).
        name: &'static str,
    },
    /// Upstream `ContextValue` (`context/index.ts:33-53`): one keyed value
    /// over a parent chain. Lookup by token match, then delegate to the
    /// parent.
    WithValue {
        parent: Arc<Context>,
        key_id: u64,
        /// The key's token description, for `toString`
        /// (`context/index.ts:50-52`).
        key_description: Cow<'static, str>,
        value: Arc<dyn Any + Send + Sync>,
    },
}

impl Context {
    /// Upstream `BACKGROUND_CONTEXT` (`context/index.ts:55`).
    pub fn background() -> Context {
        Context::Background {
            name: "[Context BACKGROUND_CONTEXT]",
        }
    }

    /// Upstream `TODO_CONTEXT` (`context/index.ts:56`): an empty context for
    /// call sites that have no caller context to pass. Ported with
    /// `packages/agent/src/harness/context.ts` (deferred from the original
    /// subset; see the chord module docs).
    pub fn todo() -> Context {
        Context::Background {
            name: "[Context TODO_CONTEXT]",
        }
    }

    /// Upstream `Context#value(key)` (`types.ts:17`): walk the chain for the
    /// first node whose key token matches. Returns the stored value handle, or
    /// `None` when the chain ends at [`Context::Background`].
    ///
    /// A derivation that stored `Option::None` (e.g. the deferred
    /// `withoutAbortSignal`) still terminates the walk with a match — the
    /// `Some` wrapper distinguishes "matched, value absent" from "no match".
    pub fn value_raw(&self, key_id: u64) -> Option<Arc<dyn Any + Send + Sync>> {
        let mut at = self;
        loop {
            match at {
                // EmptyContext#value: undefined (context/index.ts:24-26).
                Context::Background { .. } => return None,
                // ContextValue#value: token match stops the walk, otherwise
                // delegate to the parent (context/index.ts:45-48).
                Context::WithValue {
                    parent,
                    key_id: at_key,
                    value,
                    ..
                } => {
                    if *at_key == key_id {
                        return Some(value.clone());
                    }
                    at = parent;
                }
            }
        }
    }

    /// Typed accessor over [`Context::value_raw`], standing in for the TS
    /// call `context.value(key)`. Values are stored as `Arc`, so the returned
    /// handle clones cheaply; dereference to read.
    pub fn get<T: Send + Sync + 'static>(&self, key: &ContextKey<T>) -> Option<Arc<T>> {
        self.value_raw(key.id())
            .and_then(|value| value.downcast::<T>().ok())
    }

    /// Upstream `withContextValue(key, value, parent)`
    /// (`context/index.ts:63-65`): derive a context containing one additional
    /// or replaced value. Method form: `parent.with_value(key, value)`.
    pub fn with_value<T: Send + Sync + 'static>(self, key: &ContextKey<T>, value: T) -> Context {
        Context::WithValue {
            parent: Arc::new(self),
            key_id: key.id(),
            key_description: key.description.clone(),
            value: Arc::new(value),
        }
    }

    /// Upstream `BaseContext#get abortSignal` (`context/index.ts:11-13`):
    /// read [`ABORT_SIGNAL_KEY`] from the chain. `None` covers both "no signal
    /// anywhere" and an explicit `withoutAbortSignal`-style shadow.
    pub fn abort_signal(&self) -> Option<CancellationToken> {
        self.get(abort_signal_key())
            .and_then(|signal| (*signal).clone())
    }
}

impl fmt::Display for Context {
    /// Upstream `toString` (`context/index.ts:28-30,50-52`):
    /// `EmptyContext` prints its name; a value chain prints
    /// `{parent}.WithValue({token description})`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Context::Background { name } => write!(f, "{name}"),
            Context::WithValue {
                parent,
                key_description,
                ..
            } => write!(f, "{parent}.WithValue({key_description})"),
        }
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Same string as Display, matching upstream's default Error/toString
        // shape; the stored values are opaque.
        fmt::Display::fmt(self, f)
    }
}

/// Upstream `ABORT_SIGNAL_CONTEXT_KEY` (`context/index.ts:3-5`): the key the
/// `abortSignal` property reads. The stored type is `Option<CancellationToken>`
/// (upstream `AbortSignal | undefined`), so an explicit `None` can shadow a
/// parent signal once the deferred `withoutAbortSignal` helper is ported.
pub fn abort_signal_key() -> &'static ContextKey<Option<CancellationToken>> {
    static KEY: OnceLock<ContextKey<Option<CancellationToken>>> = OnceLock::new();
    KEY.get_or_init(|| ContextKey::new("chord.abortSignal"))
}

#[cfg(test)]
mod tests;
