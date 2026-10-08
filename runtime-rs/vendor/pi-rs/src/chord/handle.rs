//! Host-owned mutable service targets with consumer-owned guarded views.
//! Port of `packages/chord/src/services/handle.ts` (upstream sha256
//! `4c68a6dc8835e9ab2542523fce64076b3f63a1e6941b67c615056707ecc58f56`).
//!
//! Upstream `ServiceSlot` binds an arbitrary object and hands out `Proxy`
//! views whose `get` traps resolve the property against the *live*
//! implementation on every access (so rebinding is observed immediately) and
//! run the caller's `assertAccess` closure per access. Rust has no property
//! proxies: the port keeps the access semantics (assert → disconnect check →
//! resolve the current target) and exposes the resolved target as an
//! `Arc<T>` downcast ([`ServiceSlot::resolve`]). The deep `ValueView` chain
//! (`handle.ts:71-109`) only exists to keep property chains live through the
//! proxy; resolving the whole target per access is ownership-equivalent.

use std::sync::{Arc, Mutex};

use crate::chord::services::errors::ChordError;

type SlotTarget = Arc<dyn std::any::Any + Send + Sync>;

struct SlotInner {
    implementation: Option<SlotTarget>,
}

/// Port of upstream `ServiceSlot` (`handle.ts:9-40`).
pub struct ServiceSlot {
    service_id: String,
    inner: Mutex<SlotInner>,
}

impl ServiceSlot {
    /// `new ServiceSlot(serviceId, wrapObjects)` (`handle.ts:14-17`). The
    /// `wrapObjects` flag only shapes the upstream proxy wrapping policy and
    /// is unrepresentable over typed targets.
    pub fn new(service_id: &str) -> Arc<Self> {
        Arc::new(ServiceSlot {
            service_id: service_id.to_owned(),
            inner: Mutex::new(SlotInner {
                implementation: None,
            }),
        })
    }

    /// `bind(implementation)` (`handle.ts:23-25`).
    pub fn bind(&self, implementation: SlotTarget) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .implementation = Some(implementation);
    }

    /// `unbind()` (`handle.ts:27-29`).
    pub fn unbind(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .implementation = None;
    }

    /// `resolve(property, assertAccess)` (`handle.ts:31-39`): runs the access
    /// assertion, then resolves the current implementation. The upstream
    /// per-property resolution collapses to resolving the whole target; the
    /// typed downcast happens at the call site.
    pub fn resolve(
        &self,
        assert_access: impl FnOnce() -> Result<(), ChordError>,
    ) -> Result<SlotTarget, ChordError> {
        assert_access()?;
        let implementation = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .implementation
            .clone();
        implementation
            .ok_or_else(|| ChordError::Type(format!("Service {} is disconnected", self.service_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Value {
        n: u32,
    }

    fn ok() -> Result<(), ChordError> {
        Ok(())
    }

    fn denied() -> Result<(), ChordError> {
        Err(ChordError::Type("denied".to_owned()))
    }

    #[test]
    fn resolves_the_live_target_per_access() {
        let slot = ServiceSlot::new("svc");
        let err = slot.resolve(ok).unwrap_err();
        assert_eq!(err.message(), "Service svc is disconnected");

        let first: Arc<Value> = Arc::new(Value { n: 1 });
        slot.bind(first.clone());
        let resolved = slot.resolve(ok).unwrap();
        let value = resolved.downcast::<Value>().unwrap();
        assert_eq!(value.n, 1);
        // Rebinding is observed by later resolutions.
        slot.bind(Arc::new(Value { n: 2 }));
        assert_eq!(slot.resolve(ok).unwrap().downcast::<Value>().unwrap().n, 2);
        slot.unbind();
        assert_eq!(
            slot.resolve(ok).unwrap_err().message(),
            "Service svc is disconnected"
        );
    }

    #[test]
    fn runs_the_access_assertion_first() {
        let slot = ServiceSlot::new("svc");
        slot.bind(Arc::new(Value { n: 1 }));
        assert_eq!(slot.resolve(denied).unwrap_err().message(), "denied");
    }
}
