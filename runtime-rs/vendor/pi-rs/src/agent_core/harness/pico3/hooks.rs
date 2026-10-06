//! Port of `packages/agent/src/harness/pico3/hooks.ts` (47 lines): hook
//! registration records and the per-invocation hook runner factory.
//!
//! Disclosed substitution: upstream `handlers: object` is an untyped bag the
//! kind code narrows by shape; the port erases it as
//! `Arc<dyn Any + Send + Sync>` and the kinds downcast to their per-kind
//! handlers struct (see [`super::runtime::HookHandlers`]) — the same
//! runtime-erasure with an explicit contract.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::types::{AnyKind, Id, Namespace};

use super::runtime::HookApi;

/// Upstream `HookRegistration` (`hooks.ts:3-9`).
#[derive(Clone)]
pub struct HookRegistration {
    /// Upstream `namespace`.
    pub namespace: Namespace,
    /// Upstream `kind` — the registered kind token.
    pub kind: Arc<dyn AnyKind>,
    /// Upstream `handlers: object`.
    pub handlers: Arc<dyn std::any::Any + Send + Sync>,
    /// Upstream `conversationId?`.
    pub conversation_id: Option<Id>,
    /// Upstream `subtree?`.
    pub subtree: bool,
}

/// Upstream `HookBinding` (`types.ts:352-356`): one matching registration
/// with its api filled in.
pub struct HookBinding {
    /// Upstream `namespace`.
    pub namespace: Namespace,
    /// Upstream `api`: `{ ...info, kind: kind.name }`.
    pub api: HookApi,
    /// Upstream `handlers`, downcast by the calling kind.
    pub handlers: Arc<dyn std::any::Any + Send + Sync>,
}

/// Upstream `createHookRunners(...)`'s returned runner (`hooks.ts:16-46`):
/// one per invocation, bound to a kind and the hook info.
#[derive(Clone)]
pub struct HookRunner {
    kind: Arc<dyn AnyKind>,
    /// The info minus `kind` (`hooks.ts:16`): taskId and conversationId.
    info: (Option<Id>, Id),
    registrations: Arc<dyn Fn() -> Vec<Arc<HookRegistration>> + Send + Sync>,
    ancestors: Arc<dyn Fn(Id) -> Vec<Id> + Send + Sync>,
    on_report: Arc<dyn Fn(&anyhow::Error) + Send + Sync>,
}

impl HookRunner {
    /// Upstream `createHookRunners`'s inner closure (`hooks.ts:16`).
    pub fn new(
        kind: Arc<dyn AnyKind>,
        task_id: Option<Id>,
        conversation_id: Id,
        registrations: Arc<dyn Fn() -> Vec<Arc<HookRegistration>> + Send + Sync>,
        ancestors: Arc<dyn Fn(Id) -> Vec<Id> + Send + Sync>,
        on_report: Arc<dyn Fn(&anyhow::Error) + Send + Sync>,
    ) -> HookRunner {
        HookRunner {
            kind,
            info: (task_id, conversation_id),
            registrations,
            ancestors,
            on_report,
        }
    }

    /// The kind this runner filters registrations by.
    pub fn kind(&self) -> &Arc<dyn AnyKind> {
        &self.kind
    }

    /// The conversation the invocation runs in (`info.conversationId`).
    pub fn conversation_id(&self) -> Id {
        self.info.1
    }

    /// Upstream `handlers()` (`hooks.ts:17-31`): matching registrations,
    /// filtered by kind identity and conversation scope, in registration
    /// order. Handlers are handed out erased; the kind downcasts.
    pub fn bindings(&self) -> Vec<HookBinding> {
        let conversation_id = self.info.1;
        (self.registrations)()
            .into_iter()
            .filter(|registration| Arc::ptr_eq(&registration.kind, &self.kind))
            .filter(|registration| {
                registration.conversation_id.is_none()
                    || registration.conversation_id == Some(conversation_id)
                    || (registration.subtree
                        && (self.ancestors)(conversation_id)
                            .contains(&registration.conversation_id.expect("checked above")))
            })
            .map(|registration| HookBinding {
                namespace: registration.namespace.clone(),
                api: HookApi {
                    task_id: self.info.0,
                    conversation_id,
                    kind: self.kind.name().to_owned(),
                },
                handlers: registration.handlers.clone(),
            })
            .collect()
    }

    /// Upstream `each(ctx, fn, onValue)` (`hooks.ts:32-44`): run `f` for
    /// every binding, in order. A handler error is reported and skipped
    /// unless the context aborted — that rethrows to the caller
    /// (`hooks.ts:38-41`); the first `Some` value whose `on_value` returns
    /// true stops the chain.
    pub async fn each<H, F, V>(
        &self,
        ctx: Context,
        mut f: F,
        mut on_value: impl FnMut(V) -> bool,
    ) -> anyhow::Result<()>
    where
        H: super::runtime::HookHandlers + 'static,
        V: 'static,
        F: FnMut(&H, &HookApi) -> BoxFuture<'static, anyhow::Result<Option<V>>>,
    {
        for binding in self.bindings() {
            let Some(handlers) = binding.handlers.downcast_ref::<H>() else {
                continue;
            };
            match f(handlers, &binding.api).await {
                Ok(Some(value)) => {
                    if on_value(value) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let aborted = ctx
                        .abort_signal()
                        .is_some_and(|signal| signal.is_cancelled());
                    if aborted {
                        // `if (ctx.abortSignal?.aborted) throw error`
                        // (`hooks.ts:40`).
                        return Err(error);
                    }
                    (self.on_report)(&error);
                }
            }
        }
        Ok(())
    }
}
