//! Remote service consumer bindings: member slots, service facades,
//! singleton/keyed binding lifecycles, and the transport surface.
//! Port of `packages/chord/src/services/consumer.ts` (upstream sha256
//! `4d9781ecca6d77845ca52734165a0748ebd3f72ed02418d55c607416050a2eea`),
//! plus the `RemoteServiceTransport` / `RemoteServices` declarations of
//! `types.ts` and the loopback transport of `services/loopback.ts` (sha256
//! `533ec7e5680090e4bcdade2b6ece9b39fba816acb72bc019ba0324af9beae9ad`).
//!
//! # Divergences (disclosed)
//!
//! - **D2 (synchronous lifecycle)**: the upstream binding is an async state
//!   machine (`starting` promises, `#bindingTransition`, cancellable
//!   observation tasks). The port flattens it onto the synchronous
//!   closure convention already established by
//!   [`crate::chord::services::provider`] (loopback transports resolve
//!   inline), so `use`/`observe`/`rebind` complete their subscriptions
//!   before returning. Observable ordering (subscribe → snapshot install →
//!   activate → ready) is preserved; readiness revisions are still tracked
//!   and `ready` re-validates them, but there are no in-flight starts to
//!   await.
//! - **D3 (proxy surfaces → typed handles)**: upstream hands out `Proxy`
//!   objects (member facades, keyed views). The port's equivalent is
//!   [`ServiceFacade`] / [`RemoteMember`] with explicit `call` / `value` /
//!   `subscribe` methods and [`ServiceHandle`] for the
//!   local-downcast-vs-remote-member duality.
//! - **D4 (trailing-Context detection)**: upstream `MemberSlot#call`
//!   inspects `args.at(-1)` for a `Context` and fails with
//!   `service_invalid_value` when absent. Rust passes the context as a
//!   parameter, so the check is unrepresentable.
//! - **D5 (listener shape checks)**: upstream `typeof listener !==
//!   "function"` TypeError guards are unrepresentable over `Fn` closures.
//! - **D11 (loopback reentrancy)**: upstream JS is single-threaded and
//!   reentrant, so an observer invoked from a provider publication can call
//!   straight back into `provider.invoke`. The synchronous loopback port
//!   publishes under the provider lock, so reentrant invokes execute on a
//!   helper thread that can re-acquire it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::chord::context::Context;
use crate::chord::delta::Op;
use crate::chord::services::errors::{ChordError, RemoteServiceErrorCode};
use crate::chord::services::provider::RemoteServiceProvider;
use crate::chord::services::state::{service_delivery_context, ReplicatedStateReplica};
use crate::chord::types::{
    JsonValue, ReplicatedStateDelivery, Service, ServiceCall, ServiceInstanceAddress,
    ServiceInstanceSnapshot, ServiceMode, ServiceProviderUpdate, ServiceSubscriptionSnapshot,
};

/// Upstream `MemberSlot#invoke` closure shape.
pub type MemberInvoker =
    dyn Fn(&[JsonValue], &Context) -> Result<Option<JsonValue>, ChordError> + Send + Sync;

/// `(error) => void` reporter (upstream `ErrorReporter`).
pub type ErrorReporter = Arc<dyn Fn(&ChordError) + Send + Sync>;

/// Upstream subscription update listener.
pub type UpdateListener =
    Arc<dyn Fn(&ServiceProviderUpdate, &Context) -> Result<(), ChordError> + Send + Sync>;

/// Access assertion shared by handles (upstream inline `assertAccess`
/// closures).
pub type AccessAssert = Arc<dyn Fn() -> Result<(), ChordError> + Send + Sync>;

pub(crate) fn ignore_errors() -> ErrorReporter {
    Arc::new(|_| {})
}

pub(crate) fn allow_all() -> AccessAssert {
    Arc::new(|| Ok(()))
}

fn remote_error(code: RemoteServiceErrorCode, message: impl Into<String>) -> ChordError {
    ChordError::remote(code, message)
}

// ── transport surface (`types.ts` RemoteServiceTransport) ───────────────────

/// The subscription wrapper consumer-side `transport.subscribe` resolves to
/// (upstream `{ snapshot, activate, close }`).
pub struct TransportSubscription {
    pub snapshot: ServiceSubscriptionSnapshot,
    activate: Box<dyn Fn() -> Result<(), ChordError> + Send + Sync>,
    close: Box<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for TransportSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportSubscription")
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

impl TransportSubscription {
    pub fn new(
        snapshot: ServiceSubscriptionSnapshot,
        activate: Box<dyn Fn() -> Result<(), ChordError> + Send + Sync>,
        close: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        TransportSubscription {
            snapshot,
            activate,
            close,
        }
    }

    /// `activate()` — replays buffered provider updates.
    pub fn activate(&self) -> Result<(), ChordError> {
        (self.activate)()
    }

    /// `close()` — detaches the consumer listener.
    pub fn close(&self) {
        (self.close)();
    }
}

/// Upstream `RemoteServiceTransport` (`types.ts`). The async surface is
/// flattened to the synchronous closure convention (divergence D2).
pub trait RemoteServiceTransport: Send + Sync {
    fn invoke(
        &self,
        call: &ServiceCall,
        context: &Context,
    ) -> Result<Option<JsonValue>, ChordError>;
    fn subscribe(
        &self,
        service_id: &str,
        mode: ServiceMode,
        listener: UpdateListener,
    ) -> Result<TransportSubscription, ChordError>;
    /// Keyed subscriptions deliver to observers that may re-enter the
    /// provider (upstream JS reentrancy); transports override this when their
    /// delivery path needs a pump thread (divergence D11). The default is a
    /// plain subscription.
    fn subscribe_keyed(
        &self,
        service_id: &str,
        listener: UpdateListener,
    ) -> Result<TransportSubscription, ChordError> {
        self.subscribe(service_id, ServiceMode::Keyed, listener)
    }
}

// ── member slots (upstream `MemberSlot`) ────────────────────────────────────

type MemberKind = &'static str;

const METHOD: MemberKind = "method";
const STATE: MemberKind = "state";

struct MemberInner {
    kind: Option<MemberKind>,
    expected_kind: Option<MemberKind>,
}

/// Port of upstream `MemberSlot` (`consumer.ts:24-138`): one service member
/// of a remote facade. [`RemoteMember::value`]/[`RemoteMember::subscribe`]
/// are the state face, [`RemoteMember::call`] the method face; the kind
/// guards reproduce the upstream two-kind detection errors.
pub struct RemoteMember {
    service_id: String,
    member: String,
    invoke: Box<MemberInvoker>,
    state: Arc<ReplicatedStateReplica>,
    is_active: Arc<dyn Fn() -> bool + Send + Sync>,
    assert_access: AccessAssert,
    inner: Mutex<MemberInner>,
}

impl std::fmt::Debug for RemoteMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteMember")
            .field("service_id", &self.service_id)
            .field("member", &self.member)
            .finish()
    }
}

impl RemoteMember {
    fn new(
        service_id: &str,
        member: &str,
        invoke: Box<MemberInvoker>,
        is_active: Arc<dyn Fn() -> bool + Send + Sync>,
        assert_access: AccessAssert,
    ) -> Arc<Self> {
        Arc::new(RemoteMember {
            service_id: service_id.to_owned(),
            member: member.to_owned(),
            invoke,
            state: ReplicatedStateReplica::new(),
            is_active,
            assert_access,
            inner: Mutex::new(MemberInner {
                kind: None,
                expected_kind: None,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemberInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `setDescription(kind)` (`consumer.ts:66-77`).
    pub fn set_description(&self, kind: MemberKind) -> Result<(), ChordError> {
        let mut inner = self.lock();
        if let Some(previous) = inner.kind {
            if previous != kind {
                return Err(ChordError::Type(format!(
                    "Remote service member {}.{} changed kind",
                    self.service_id, self.member
                )));
            }
        }
        inner.kind = Some(kind);
        if let Some(expected) = inner.expected_kind {
            if expected != kind {
                return Err(remote_error(
                    RemoteServiceErrorCode::ServiceMemberMismatch,
                    format!(
                        "Remote service member {}.{} is {}, not {}",
                        self.service_id, self.member, kind, expected
                    ),
                ));
            }
        }
        Ok(())
    }

    /// `hydrate(sequence, ops, context)` (`consumer.ts:79-82`).
    pub fn hydrate(&self, sequence: u64, ops: &[Op], context: &Context) -> Result<(), ChordError> {
        self.set_description(STATE)?;
        self.state.hydrate(sequence, ops, context)
    }

    /// `update(sequence, ops, context)` (`consumer.ts:84-87`).
    pub fn update(&self, sequence: u64, ops: &[Op], context: &Context) -> Result<(), ChordError> {
        self.set_description(STATE)?;
        self.state.update(sequence, ops, context)
    }

    /// `clear()` (`consumer.ts:89-91`).
    pub fn clear(&self) {
        self.state.clear();
    }

    /// `#expect(kind)` (`consumer.ts:102-116`).
    fn expect(&self, kind: MemberKind) -> Result<(), ChordError> {
        let mut inner = self.lock();
        if let Some(expected) = inner.expected_kind {
            if expected != kind {
                return Err(remote_error(
                    RemoteServiceErrorCode::ServiceMemberMismatch,
                    format!(
                        "Remote service member {}.{} was used as two different kinds",
                        self.service_id, self.member
                    ),
                ));
            }
        }
        inner.expected_kind = Some(kind);
        if let Some(actual) = inner.kind {
            if actual != kind {
                return Err(remote_error(
                    RemoteServiceErrorCode::ServiceMemberMismatch,
                    format!(
                        "Remote service member {}.{} is {}, not {}",
                        self.service_id, self.member, actual, kind
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The `value` property (`consumer.ts:53-57`): assert access, expect the
    /// state kind, then read the replica.
    pub fn value(&self) -> Result<Option<JsonValue>, ChordError> {
        (self.assert_access)()?;
        self.expect(STATE)?;
        Ok(self.state.value())
    }

    /// The replica behind a state member (identity-stable across updates,
    /// the upstream `this.#state.value` object identity).
    pub fn replica(&self) -> Result<Arc<ReplicatedStateReplica>, ChordError> {
        (self.assert_access)()?;
        self.expect(STATE)?;
        Ok(self.state.clone())
    }

    /// `#subscribe(listener)` (`consumer.ts:93-100`).
    pub fn subscribe<F>(&self, listener: F) -> Result<Box<dyn Fn() + Send + Sync>, ChordError>
    where
        F: Fn(&JsonValue, &Context, &ReplicatedStateDelivery) + Send + Sync + 'static,
    {
        (self.assert_access)()?;
        self.expect(STATE)?;
        Ok(self.state.subscribe(listener))
    }

    /// `#call(args)` (`consumer.ts:118-137`). The trailing-`Context`
    /// inspection is unrepresentable (divergence D4).
    pub fn call(
        &self,
        args: &[JsonValue],
        context: &Context,
    ) -> Result<Option<JsonValue>, ChordError> {
        (self.assert_access)()?;
        self.expect(METHOD)?;
        if !(self.is_active)() {
            return Err(remote_error(
                RemoteServiceErrorCode::ServiceStaleInstance,
                format!("Remote service {} binding is closed", self.service_id),
            ));
        }
        (self.invoke)(args, context)
    }
}

// ── service facades (upstream `ServiceFacade`) ──────────────────────────────

struct FacadeInner {
    slots: HashMap<String, Arc<RemoteMember>>,
    descriptions: HashMap<String, MemberKind>,
}

/// Port of upstream `ServiceFacade` (`consumer.ts:140-233`): the per-binding
/// view of one remote service. Upstream's proxy `get` lazily created slots;
/// [`ServiceFacade::member`] is that lazy slot table made explicit.
pub struct ServiceFacade {
    pub(crate) service_id: String,
    address: Option<ServiceInstanceAddress>,
    transport: Arc<dyn RemoteServiceTransport>,
    is_active: Arc<dyn Fn() -> bool + Send + Sync>,
    assert_access: AccessAssert,
    inner: Mutex<FacadeInner>,
}

impl std::fmt::Debug for ServiceFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceFacade")
            .field("service_id", &self.service_id)
            .field("address", &self.address)
            .finish()
    }
}

impl ServiceFacade {
    pub fn new(
        service_id: &str,
        address: Option<ServiceInstanceAddress>,
        transport: Arc<dyn RemoteServiceTransport>,
        is_active: Arc<dyn Fn() -> bool + Send + Sync>,
        assert_access: AccessAssert,
    ) -> Arc<Self> {
        Arc::new(ServiceFacade {
            service_id: service_id.to_owned(),
            address,
            transport,
            is_active,
            assert_access,
            inner: Mutex::new(FacadeInner {
                slots: HashMap::new(),
                descriptions: HashMap::new(),
            }),
        })
    }

    /// `#slot(member)` (`consumer.ts:208-232`).
    pub fn member(self: &Arc<Self>, member: &str) -> Arc<RemoteMember> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(slot) = inner.slots.get(member) {
            return slot.clone();
        }
        let service_id = self.service_id.clone();
        let member_name = member.to_owned();
        let address = self.address.clone();
        let transport = self.transport.clone();
        let invoke = Box::new(
            move |args: &[JsonValue], context: &Context| -> Result<Option<JsonValue>, ChordError> {
                transport.invoke(
                    &ServiceCall {
                        service_id: service_id.clone(),
                        instance: address.clone(),
                        member: member_name.clone(),
                        args: args.to_vec(),
                    },
                    context,
                )
            },
        );
        let slot = RemoteMember::new(
            &self.service_id,
            member,
            invoke,
            self.is_active.clone(),
            self.assert_access.clone(),
        );
        if let Some(kind) = inner.descriptions.get(member).copied() {
            let _ = slot.set_description(kind);
        }
        inner.slots.insert(member.to_owned(), slot.clone());
        slot
    }

    /// `install(snapshot, context)` (`consumer.ts:173-195`).
    pub fn install(
        self: &Arc<Self>,
        snapshot: &ServiceInstanceSnapshot,
        context: &Context,
    ) -> Result<(), ChordError> {
        if !same_address(snapshot.instance.as_ref(), self.address.as_ref()) {
            return Err(ChordError::Type(
                "Remote service snapshot has the wrong address".to_owned(),
            ));
        }
        let members = validate_members(&snapshot.members)?;
        {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for name in inner.slots.keys() {
                if !members.iter().any(|member| member.name() == name) {
                    return Err(remote_error(
                        RemoteServiceErrorCode::ServiceMemberNotFound,
                        format!("Unknown remote service member {}.{}", self.service_id, name),
                    ));
                }
            }
        }
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.descriptions.clear();
            for member in &members {
                let kind = match member {
                    crate::chord::types::ServiceMemberSnapshot::Method { .. } => METHOD,
                    crate::chord::types::ServiceMemberSnapshot::State { .. } => STATE,
                };
                inner.descriptions.insert(member.name().to_owned(), kind);
            }
        }
        for member in &members {
            match member {
                crate::chord::types::ServiceMemberSnapshot::State {
                    name,
                    sequence,
                    ops,
                } => {
                    self.member(name).hydrate(*sequence, ops, context)?;
                }
                crate::chord::types::ServiceMemberSnapshot::Method { name } => {
                    let existing = self
                        .inner
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .slots
                        .get(name)
                        .cloned();
                    if let Some(slot) = existing {
                        slot.set_description(METHOD)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `update(member, sequence, ops, context)` (`consumer.ts:197-202`).
    pub fn update(
        self: &Arc<Self>,
        member: &str,
        sequence: u64,
        ops: &[Op],
        context: &Context,
    ) -> Result<(), ChordError> {
        {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if inner.descriptions.get(member) != Some(&STATE) {
                return Err(ChordError::Type(format!(
                    "Remote service update targets non-state member {}.{}",
                    self.service_id, member
                )));
            }
        }
        self.member(member).update(sequence, ops, context)
    }

    /// `clear()` (`consumer.ts:204-206`).
    pub fn clear(&self) {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for slot in inner.slots.values() {
            slot.clear();
        }
    }
}

fn validate_members(
    members: &[crate::chord::types::ServiceMemberSnapshot],
) -> Result<Vec<&crate::chord::types::ServiceMemberSnapshot>, ChordError> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut result = Vec::new();
    for member in members {
        let name = member.name();
        if name.is_empty() || !seen.insert(name) {
            return Err(ChordError::Type(
                "Remote service has invalid member descriptions".to_owned(),
            ));
        }
        result.push(member);
    }
    Ok(result)
}

/// `validateResetSnapshot` (`consumer.ts:660-687`): service and mode must
/// match, singleton resets carry at most one address-less instance, keyed
/// resets carry addressed instances without repeated keys, and every state
/// member is a full root replacement.
fn validate_reset_snapshot(
    snapshot: &crate::chord::types::ServiceSubscriptionSnapshot,
    service_id: &str,
    mode: ServiceMode,
) -> Result<(), ChordError> {
    if snapshot.service_id != service_id
        || snapshot.mode != mode
        || (mode == ServiceMode::Singleton && snapshot.instances.len() > 1)
    {
        return Err(ChordError::Type(
            "Remote service reset has the wrong service or mode".to_owned(),
        ));
    }
    let mut keys: HashSet<&str> = HashSet::new();
    for instance in &snapshot.instances {
        match mode {
            ServiceMode::Singleton if instance.instance.is_some() => {
                return Err(ChordError::Type(
                    "Remote service reset has an invalid instance address".to_owned(),
                ));
            }
            ServiceMode::Keyed if instance.instance.is_none() => {
                return Err(ChordError::Type(
                    "Remote service reset has an invalid instance address".to_owned(),
                ));
            }
            _ => {}
        }
        if let Some(address) = &instance.instance {
            if !keys.insert(address.key.as_str()) {
                return Err(ChordError::Type(
                    "Remote service reset repeats an instance key".to_owned(),
                ));
            }
        }
        for member in &instance.members {
            if let crate::chord::types::ServiceMemberSnapshot::State { ops, .. } = member {
                let full_replacement =
                    ops.len() == 1 && matches!(ops.first(), Some(Op::Replace(_)));
                if !full_replacement {
                    return Err(ChordError::Type(
                        "Remote service reset must contain full root replacements".to_owned(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn same_address(
    left: Option<&ServiceInstanceAddress>,
    right: Option<&ServiceInstanceAddress>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.key == right.key && left.generation == right.generation,
        _ => false,
    }
}

// ── service handles (the local-downcast vs remote-member view duality) ──────

/// What a keyed delivery or singleton `use` hands its consumer: either a
/// host slot / local implementation target or a remote facade.
#[derive(Clone)]
pub enum ServiceTarget {
    /// A host slot whose target resolves per access (singleton views).
    Slot(Arc<super::handle::ServiceSlot>),
    /// A fixed local implementation (keyed local deliveries).
    Local(Arc<dyn std::any::Any + Send + Sync>),
    /// A remote facade (singleton `use` and remote keyed deliveries).
    Remote(Arc<ServiceFacade>),
}

/// The guarded view handed to consumers (upstream the slot/facade proxy
/// typed as `T`). Every access runs the view's access assertion first.
#[derive(Clone)]
pub struct ServiceHandle {
    pub(crate) target: ServiceTarget,
    assert: AccessAssert,
}

impl std::fmt::Debug for ServiceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.target {
            ServiceTarget::Remote(facade) => f
                .debug_struct("ServiceHandle")
                .field("target", facade)
                .finish(),
            _ => f.debug_struct("ServiceHandle").finish(),
        }
    }
}

impl ServiceHandle {
    pub fn new(target: ServiceTarget, assert: AccessAssert) -> Self {
        ServiceHandle { target, assert }
    }

    /// Upstream slot view of a local service: resolve the live target and
    /// downcast. Asserts access, then fails with the upstream disconnected
    /// message when the slot is unbound.
    pub fn local<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, ChordError> {
        let any = match &self.target {
            ServiceTarget::Slot(slot) => slot.resolve(|| (self.assert)())?,
            ServiceTarget::Local(any) => {
                (self.assert)()?;
                any.clone()
            }
            ServiceTarget::Remote(_) => {
                (self.assert)()?;
                return Err(ChordError::Type(
                    "Remote service handles expose members explicitly".to_owned(),
                ));
            }
        };
        any.downcast::<T>().map_err(|_| {
            ChordError::Type("Service handle does not hold the requested type".to_owned())
        })
    }

    /// Call a remote member (the upstream `view.member(context)` call).
    pub fn invoke(
        &self,
        member: &str,
        args: &[JsonValue],
        context: &Context,
    ) -> Result<Option<JsonValue>, ChordError> {
        self.member(member)?.call(args, context)
    }

    /// Read a remote state member (`view.state.value` upstream).
    pub fn state_value(&self, member: &str) -> Result<Option<JsonValue>, ChordError> {
        self.member(member)?.value()
    }

    /// The identity-stable replica of a remote state member.
    pub fn state_replica(&self, member: &str) -> Result<Arc<ReplicatedStateReplica>, ChordError> {
        self.member(member)?.replica()
    }

    /// Subscribe to a remote state member.
    pub fn subscribe_state<F>(
        &self,
        member: &str,
        listener: F,
    ) -> Result<Box<dyn Fn() + Send + Sync>, ChordError>
    where
        F: Fn(&JsonValue, &Context, &ReplicatedStateDelivery) + Send + Sync + 'static,
    {
        self.member(member)?.subscribe(listener)
    }

    /// The remote member slot; errors on local-only handles. Slot targets
    /// resolve through the host slot first (running the access assertion),
    /// then expose a bound remote facade.
    pub fn member(&self, member: &str) -> Result<Arc<RemoteMember>, ChordError> {
        self.remote_facade().map(|facade| facade.member(member))
    }

    /// The remote facade behind this handle, when one is bound.
    fn remote_facade(&self) -> Result<Arc<ServiceFacade>, ChordError> {
        match &self.target {
            ServiceTarget::Remote(facade) => {
                (self.assert)()?;
                Ok(facade.clone())
            }
            ServiceTarget::Slot(slot) => {
                let any = slot.resolve(|| (self.assert)())?;
                any.downcast::<ServiceFacade>().map_err(|_| {
                    ChordError::Type(
                        "Local service handles expose the implementation directly".to_owned(),
                    )
                })
            }
            ServiceTarget::Local(_) => {
                (self.assert)()?;
                Err(ChordError::Type(
                    "Local service handles expose the implementation directly".to_owned(),
                ))
            }
        }
    }
}

// ── keyed bindings (upstream `KeyedBinding`) ────────────────────────────────

/// Keyed observation handler (upstream `(service, context) => void |
/// Promise<void>`); the port is synchronous (divergence D2).
pub type KeyedHandler =
    Arc<dyn Fn(&ServiceHandle, &Context) -> Result<(), ChordError> + Send + Sync>;

/// Target-level keyed observation used by the facet host's source-agnostic
/// surface (upstream `KeyedServiceSource`, `facets/host.ts:145-147`).
pub type TargetHandler =
    Arc<dyn Fn(&ServiceTarget, &Context) -> Result<(), ChordError> + Send + Sync>;

/// A source of keyed observations: the internal binding and the local
/// registry both implement this.
pub trait KeyedServiceSource: Send + Sync {
    fn observe_keyed(
        self: Arc<Self>,
        service: &Service,
        handler: TargetHandler,
    ) -> Result<Box<dyn Fn() + Send + Sync>, ChordError>;
}

/// Keyed observation stop handle (the `() => void` upstream returns).
pub type ObservationStop = Box<dyn Fn() + Send + Sync>;

struct KeyedState {
    instances: super::instances::InstanceDirectory,
    subscription: Option<TransportSubscription>,
    bound: bool,
    revision: u64,
    closed: bool,
}

/// Port of upstream `KeyedBinding` (`consumer.ts:247-423`).
pub struct KeyedBinding {
    service_id: String,
    transport: Arc<dyn RemoteServiceTransport>,
    report_error: ErrorReporter,
    assert_access: AccessAssert,
    on_empty: Box<dyn Fn() + Send + Sync>,
    closed: Arc<AtomicBool>,
    state: Mutex<KeyedState>,
}

impl std::fmt::Debug for KeyedBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyedBinding")
            .field("service_id", &self.service_id)
            .finish()
    }
}

impl KeyedServiceSource for KeyedBinding {
    fn observe_keyed(
        self: Arc<Self>,
        _service: &Service,
        handler: TargetHandler,
    ) -> Result<ObservationStop, ChordError> {
        self.observe_target(handler)
    }
}

impl KeyedBinding {
    /// Constructor counterpart of upstream `new KeyedBinding(...)` plus the
    /// `onEmpty` closure wired at `RemoteServiceBinding.observe`
    /// (`consumer.ts:487-492`).
    fn new(
        service: &Service,
        transport: Arc<dyn RemoteServiceTransport>,
        report_error: ErrorReporter,
        assert_access: AccessAssert,
        on_empty: Box<dyn Fn() + Send + Sync>,
        bound: bool,
    ) -> Self {
        let closed = Arc::new(AtomicBool::new(false));
        KeyedBinding {
            service_id: service.id.clone(),
            transport,
            report_error: report_error.clone(),
            assert_access,
            on_empty,
            closed,
            state: Mutex::new(KeyedState {
                instances: super::instances::InstanceDirectory::new(false, report_error),
                subscription: None,
                bound,
                revision: 0,
                closed: false,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, KeyedState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `observe(handler)` (`consumer.ts:277-310`): wraps the handler's view
    /// with the observation-closure access guard, then delegates to
    /// [`KeyedBinding::observe_core`].
    pub fn observe(self: &Arc<Self>, handler: KeyedHandler) -> Result<ObservationStop, ChordError> {
        let stopped = Arc::new(AtomicBool::new(false));
        let service_id = self.service_id.clone();
        let assert_access = self.assert_access.clone();
        let stopped_for_view = stopped.clone();
        let wrapped: KeyedHandler = Arc::new(move |handle, context| {
            let signal = context.abort_signal();
            let assert: AccessAssert = {
                let stopped = stopped_for_view.clone();
                let assert_access = assert_access.clone();
                let service_id = service_id.clone();
                Arc::new(move || {
                    assert_access()?;
                    if stopped.load(Ordering::SeqCst)
                        || signal.as_ref().is_some_and(|signal| signal.is_cancelled())
                    {
                        return Err(remote_error(
                            RemoteServiceErrorCode::ServiceStaleInstance,
                            format!("Remote service {service_id} observation is closed"),
                        ));
                    }
                    Ok(())
                })
            };
            let view = ServiceHandle::new(handle.target.clone(), assert);
            handler(&view, context)
        });
        self.observe_core_with_flag(wrapped, stopped)
    }

    /// `observe_keyed` over raw facade targets (the [`KeyedServiceSource`]
    /// face used by the facet host).
    fn observe_target(
        self: &Arc<Self>,
        handler: TargetHandler,
    ) -> Result<ObservationStop, ChordError> {
        self.observe_core(Arc::new(move |handle, context| {
            handler(&handle.target, context)
        }))
    }

    fn observe_core(
        self: &Arc<Self>,
        handler: KeyedHandler,
    ) -> Result<ObservationStop, ChordError> {
        self.observe_core_with_flag(handler, Arc::new(AtomicBool::new(false)))
    }

    fn observe_core_with_flag(
        self: &Arc<Self>,
        handler: KeyedHandler,
        stopped: Arc<AtomicBool>,
    ) -> Result<ObservationStop, ChordError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ChordError::Type(
                "Remote keyed service binding is closed".to_owned(),
            ));
        }
        let service_id = self.service_id.clone();
        let assert_access = self.assert_access.clone();
        let stopped_for_view = stopped.clone();
        let directory_handler: super::instances::DirectoryHandler = Arc::new(
            move |service: &super::instances::DirectoryTarget, context: &Context| {
                let facade = service.clone().downcast::<ServiceFacade>().map_err(|_| {
                    ChordError::Type("Keyed entry is not a remote facade".to_owned())
                })?;
                let signal = context.abort_signal();
                let assert: AccessAssert = {
                    let stopped = stopped_for_view.clone();
                    let assert_access = assert_access.clone();
                    let service_id = service_id.clone();
                    Arc::new(move || {
                        assert_access()?;
                        if stopped.load(Ordering::SeqCst)
                            || signal.as_ref().is_some_and(|signal| signal.is_cancelled())
                        {
                            return Err(remote_error(
                                RemoteServiceErrorCode::ServiceStaleInstance,
                                format!("Remote service {service_id} observation is closed"),
                            ));
                        }
                        Ok(())
                    })
                };
                let view = ServiceHandle::new(ServiceTarget::Remote(facade), assert);
                handler(&view, context)
            },
        );
        let stop = self.lock().instances.observe(directory_handler)?;
        // Upstream  also starts the subscription when the binding is
        // bound (`consumer.ts:296-303`); failures are reported while the
        // binding is still current.
        let bound = self.lock().bound;
        if bound {
            let revision = self.lock().revision;
            if let Err(error) = self.start(revision) {
                let state = self.lock();
                let current = !state.closed && state.bound && state.revision == revision;
                drop(state);
                if current {
                    (self.report_error)(&error);
                }
            }
        }
        let self_arc = self.clone();
        let stopped_for_stop = stopped;
        Ok(Box::new(move || {
            if stopped_for_stop.swap(true, Ordering::SeqCst) {
                return;
            }
            stop();
            if self_arc.lock().instances.observer_count() == 0 {
                (self_arc.on_empty)();
            }
        }))
    }

    /// `rebind(bound, context)` (`consumer.ts:312-323`).
    pub fn rebind(self: &Arc<Self>, bound: bool) -> Result<(), ChordError> {
        let (revision, subscription) = {
            let mut state = self.lock();
            if state.closed {
                return Ok(());
            }
            state.bound = bound;
            state.revision += 1;
            state.instances.reset();
            (state.revision, state.subscription.take())
        };
        if let Some(subscription) = subscription {
            subscription.close();
        }
        let restart = {
            let state = self.lock();
            bound && state.instances.observer_count() > 0
        };
        if restart {
            self.start(revision)?;
        }
        Ok(())
    }

    /// `ready()` (`consumer.ts:325-327`): no in-flight starts remain in the
    /// synchronous port (divergence D2).
    pub fn ready(&self) -> Result<(), ChordError> {
        Ok(())
    }

    /// `close(context)` (`consumer.ts:329-335`).
    pub fn close(&self) -> Result<(), ChordError> {
        {
            let mut state = self.lock();
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.revision += 1;
            state.instances.reset();
        }
        self.closed.store(true, Ordering::SeqCst);
        let subscription = self.lock().subscription.take();
        if let Some(subscription) = subscription {
            subscription.close();
        }
        self.lock().instances.dispose();
        Ok(())
    }

    /// `#start(revision)` (`consumer.ts:349-369`).
    fn start(self: &Arc<Self>, revision: u64) -> Result<(), ChordError> {
        let listener: UpdateListener = {
            let binding = Arc::downgrade(self);
            Arc::new(move |update, context| {
                let Some(binding) = binding.upgrade() else {
                    return Ok(());
                };
                let current = binding.lock().revision;
                if current == revision {
                    binding.deliver(update, context);
                }
                Ok(())
            })
        };
        let subscription = self.transport.subscribe_keyed(&self.service_id, listener)?;
        let snapshot = {
            let mut state = self.lock();
            if state.closed || !state.bound || state.revision != revision {
                drop(state);
                subscription.close();
                return Ok(());
            }
            let snapshot = subscription.snapshot.clone();
            state.subscription = Some(subscription);
            snapshot
        };
        if snapshot.mode != ServiceMode::Keyed || snapshot.service_id != self.service_id {
            return Err(ChordError::Type(format!(
                "Remote service {} returned the wrong keyed snapshot",
                self.service_id
            )));
        }
        for instance in &snapshot.instances {
            self.spawn(instance, &service_delivery_context())?;
        }
        // Activate outside the state lock: buffered-update replay calls the
        // listener synchronously, which re-enters `self.lock()`.
        let activation = {
            let subscription = self.lock().subscription.take();
            let Some(subscription) = subscription else {
                return Ok(());
            };
            let result = subscription.activate();
            self.lock().subscription = Some(subscription);
            result
        };
        activation?;
        self.lock().instances.ready()?;
        Ok(())
    }

    /// `#update(update, context)` (`consumer.ts:371-397`); failures are
    /// reported, never propagated.
    fn deliver(self: &Arc<Self>, update: &ServiceProviderUpdate, context: &Context) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let result: Result<(), ChordError> = match update {
            ServiceProviderUpdate::Unavailable | ServiceProviderUpdate::Replaced { .. } => Err(
                ChordError::Type("Keyed service received a singleton lifecycle update".to_owned()),
            ),
            ServiceProviderUpdate::Reset { snapshot } => (|| -> Result<(), ChordError> {
                validate_reset_snapshot(snapshot, &self.service_id, ServiceMode::Keyed)?;
                // Drop entries the reset no longer carries
                // (`consumer.ts:374-390`).
                let removals: Vec<(String, u64)> = {
                    let state = self.lock();
                    let mut removals = Vec::new();
                    for entry in state.instances.values() {
                        let matches = snapshot.instances.iter().any(|at| {
                            at.instance.as_ref().is_some_and(|address| {
                                address.key == entry.key && address.generation == entry.generation
                            })
                        });
                        if !matches {
                            removals.push((entry.key.clone(), entry.generation));
                        }
                    }
                    removals
                };
                for (key, generation) in removals {
                    self.lock().instances.remove(&key, generation);
                }
                for instance_snapshot in &snapshot.instances {
                    let address = instance_snapshot.instance.clone().ok_or_else(|| {
                        ChordError::Type(
                            "Remote service reset has an invalid instance address".to_owned(),
                        )
                    })?;
                    if self.lock().instances.generation_of(&address.key) == Some(address.generation)
                    {
                        let service = self
                            .lock()
                            .instances
                            .service_of(&address.key, address.generation)
                            .expect("generation checked above");
                        let facade = service
                            .downcast::<ServiceFacade>()
                            .expect("keyed entries are facades");
                        facade.install(instance_snapshot, context)?;
                    } else {
                        self.spawn(instance_snapshot, context)?;
                    }
                }
                Ok(())
            })(),
            ServiceProviderUpdate::Spawned { instance } => self.spawn(instance, context),
            ServiceProviderUpdate::Closed { instance } => {
                let state = self.lock();
                if state.instances.generation_of(&instance.key) == Some(instance.generation) {
                    state.instances.remove(&instance.key, instance.generation);
                }
                Ok(())
            }
            ServiceProviderUpdate::State { instance: None, .. } => Err(ChordError::Type(
                "Keyed state update has no instance address".to_owned(),
            )),
            ServiceProviderUpdate::State {
                instance: Some(address),
                member,
                sequence,
                ops,
            } => {
                let service = {
                    let state = self.lock();
                    if state.instances.generation_of(&address.key) != Some(address.generation) {
                        return;
                    }
                    state
                        .instances
                        .service_of(&address.key, address.generation)
                        .expect("generation checked above")
                };
                let facade = service
                    .downcast::<ServiceFacade>()
                    .expect("keyed entries are facades");
                facade.update(member, *sequence, ops, context)
            }
        };
        if let Err(error) = result {
            (self.report_error)(&error);
        }
    }

    /// `#spawn(snapshot, context)` (`consumer.ts:399-422`).
    fn spawn(
        self: &Arc<Self>,
        snapshot: &ServiceInstanceSnapshot,
        context: &Context,
    ) -> Result<(), ChordError> {
        let address = snapshot.instance.clone().ok_or_else(|| {
            ChordError::Type("Keyed service instance snapshot has no address".to_owned())
        })?;
        let active = Arc::new(AtomicBool::new(true));
        let closed = self.closed.clone();
        let active_for_is_active = active.clone();
        let facade = ServiceFacade::new(
            &self.service_id,
            Some(address.clone()),
            self.transport.clone(),
            Arc::new(move || {
                active_for_is_active.load(Ordering::SeqCst) && !closed.load(Ordering::SeqCst)
            }),
            self.assert_access.clone(),
        );
        facade.install(snapshot, context)?;
        let deactivate_facade = facade.clone();
        self.lock()
            .instances
            .replace(super::instances::InstanceDirectoryEntry::new(
                address.key.clone(),
                address.generation,
                facade,
                Box::new(move || {
                    active.store(false, Ordering::SeqCst);
                    deactivate_facade.clear();
                }),
            ))
    }
}

// ── the binding itself (upstream `RemoteServiceBindingImpl`) ────────────────
struct SingletonShared {
    service_id: String,
    facade: Arc<ServiceFacade>,
    active: Arc<AtomicBool>,
    state: Mutex<SingletonLifecycle>,
}

struct SingletonLifecycle {
    subscription: Option<TransportSubscription>,
    revision: u64,
}

struct BindingInner {
    allowlist: HashSet<String>,
    modes: HashMap<String, ServiceMode>,
    /// Insertion-ordered (upstream `Map`).
    singletons: Vec<(String, Arc<SingletonShared>)>,
    keyed: Vec<(String, Arc<KeyedBinding>)>,
    readiness_revision: u64,
}

/// Options for [`create_remote_service_binding`] (upstream
/// `RemoteServiceBindingOptions`).
pub struct RemoteServiceBindingOptions {
    pub services: Vec<Service>,
    pub transport: Arc<dyn RemoteServiceTransport>,
    pub on_error: Option<ErrorReporter>,
    pub assert_access: Option<AccessAssert>,
    pub bound: Option<bool>,
}

/// Port of upstream `RemoteServiceBindingImpl` (`consumer.ts:425-634`).
pub struct RemoteServiceBinding {
    transport: Arc<dyn RemoteServiceTransport>,
    report_error: ErrorReporter,
    assert_access: AccessAssert,
    disposed: Arc<AtomicBool>,
    bound: Arc<Mutex<bool>>,
    inner: Mutex<BindingInner>,
}

/// `createRemoteServiceBinding(options)` (`api.ts:84-86`).
pub fn create_remote_service_binding(
    options: RemoteServiceBindingOptions,
) -> Result<Arc<RemoteServiceBinding>, ChordError> {
    let ids: Vec<&str> = options
        .services
        .iter()
        .map(|service| service.id.as_str())
        .collect();
    let unique: HashSet<&str> = ids.iter().copied().collect();
    if unique.len() != ids.len() {
        return Err(ChordError::Type(
            "Remote service binding has duplicate service IDs".to_owned(),
        ));
    }
    Ok(Arc::new(RemoteServiceBinding {
        transport: options.transport,
        report_error: options.on_error.unwrap_or_else(ignore_errors),
        assert_access: options.assert_access.unwrap_or_else(allow_all),
        disposed: Arc::new(AtomicBool::new(false)),
        bound: Arc::new(Mutex::new(options.bound.unwrap_or(true))),
        inner: Mutex::new(BindingInner {
            allowlist: unique.into_iter().map(str::to_owned).collect(),
            modes: HashMap::new(),
            singletons: Vec::new(),
            keyed: Vec::new(),
            readiness_revision: 0,
        }),
    }))
}

impl KeyedServiceSource for RemoteServiceBinding {
    fn observe_keyed(
        self: Arc<Self>,
        service: &Service,
        handler: TargetHandler,
    ) -> Result<ObservationStop, ChordError> {
        // Target-level observation re-enters the handle-level path with a
        // view whose assertion is the binding's own.
        let binding = self.keyed_for_source(service)?;
        binding.observe_core(Arc::new(move |handle, context| {
            handler(&handle.target, context)
        }))
    }
}

impl RemoteServiceBinding {
    fn lock(&self) -> std::sync::MutexGuard<'_, BindingInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn is_bound(&self) -> bool {
        *self
            .bound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `use(service)` (`consumer.ts:448-475`): the singleton facade handle.
    /// The upstream typed proxy collapses to [`ServiceFacade`] (divergence
    /// D3); member calls run the binding's access assertions.
    pub fn use_service(self: &Arc<Self>, service: &Service) -> Result<ServiceHandle, ChordError> {
        self.assert_remotable(service)?;
        self.assert_available(&service.id, ServiceMode::Singleton)?;
        let shared = {
            let mut inner = self.lock();
            if let Some((_, existing)) = inner.singletons.iter().find(|(id, _)| *id == service.id) {
                return Ok(ServiceHandle::new(
                    ServiceTarget::Remote(existing.facade.clone()),
                    self.handle_access(),
                ));
            }
            let active = Arc::new(AtomicBool::new(true));
            let disposed = self.disposed.clone();
            let bound = self.bound.clone();
            let facade = ServiceFacade::new(
                &service.id,
                None,
                self.transport.clone(),
                {
                    let active = active.clone();
                    Arc::new(move || {
                        active.load(Ordering::SeqCst)
                            && !disposed.load(Ordering::SeqCst)
                            && *bound
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                    })
                },
                self.handle_access(),
            );
            let shared = Arc::new(SingletonShared {
                service_id: service.id.clone(),
                facade,
                active,
                state: Mutex::new(SingletonLifecycle {
                    subscription: None,
                    revision: 0,
                }),
            });
            inner.singletons.push((service.id.clone(), shared.clone()));
            inner.readiness_revision += 1;
            shared
        };
        if !self.disposed.load(Ordering::SeqCst) && self.is_bound() {
            let revision = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .revision;
            if let Err(error) = self.start_singleton(&shared, revision) {
                // Upstream reports only when the binding is still current
                // (`consumer.ts:468-472`).
                let lifecycle = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let current = shared.active.load(Ordering::SeqCst)
                    && lifecycle.revision == revision
                    && !self.disposed.load(Ordering::SeqCst)
                    && self.is_bound();
                drop(lifecycle);
                if current {
                    (self.report_error)(&error);
                }
            }
        }
        Ok(ServiceHandle::new(
            ServiceTarget::Remote(shared.facade.clone()),
            self.handle_access(),
        ))
    }

    /// `observe(service, handler)` (`consumer.ts:477-499`).
    pub fn observe(
        self: &Arc<Self>,
        service: &Service,
        handler: KeyedHandler,
    ) -> Result<ObservationStop, ChordError> {
        self.assert_remotable(service)?;
        self.assert_available(&service.id, ServiceMode::Keyed)?;
        let binding = self.keyed_binding(service)?;
        binding.observe(handler)
    }

    /// `ready(context)` (`consumer.ts:501-519`): the synchronous port has no
    /// in-flight starts, so readiness is the disposed check (divergence D2).
    pub fn ready(&self) -> Result<(), ChordError> {
        if self.disposed.load(Ordering::SeqCst) {
            return Err(ChordError::Type(
                "Remote service binding is disposed".to_owned(),
            ));
        }
        let _revision = self.lock().readiness_revision;
        Ok(())
    }

    /// `rebind(bound, context)` (`consumer.ts:521-551`).
    pub fn rebind(self: &Arc<Self>, bound: bool) -> Result<(), ChordError> {
        if self.disposed.load(Ordering::SeqCst) {
            return Err(ChordError::Type(
                "Remote service binding is disposed".to_owned(),
            ));
        }
        *self
            .bound
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = bound;
        self.lock().readiness_revision += 1;
        let mut errors: Vec<ChordError> = Vec::new();
        let singletons: Vec<Arc<SingletonShared>> = {
            let inner = self.lock();
            for (_, shared) in inner.singletons.iter() {
                let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
                state.revision += 1;
                shared.facade.clear();
            }
            inner
                .singletons
                .iter()
                .map(|(_, shared)| shared.clone())
                .collect()
        };
        for shared in singletons {
            let previous_subscription = shared
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .subscription
                .take();
            if let Some(subscription) = previous_subscription {
                subscription.close();
            }
            if bound {
                let revision = shared
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .revision;
                if let Err(error) = self.start_singleton(&shared, revision) {
                    errors.push(error);
                }
            }
        }
        let keyed: Vec<Arc<KeyedBinding>> = {
            let inner = self.lock();
            inner.keyed.iter().map(|(_, keyed)| keyed.clone()).collect()
        };
        for keyed in keyed {
            if let Err(error) = keyed.rebind(bound) {
                errors.push(error);
            }
        }
        finish_aggregate(errors, "Failed to rebind services")
    }

    /// `dispose(context)` (`consumer.ts:553-573`).
    pub fn dispose(self: &Arc<Self>) -> Result<(), ChordError> {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let (singletons, keyed) = {
            let mut inner = self.lock();
            (
                std::mem::take(&mut inner.singletons),
                std::mem::take(&mut inner.keyed),
            )
        };
        for (_, shared) in singletons {
            shared.active.store(false, Ordering::SeqCst);
            shared.facade.clear();
            let subscription = shared
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .subscription
                .take();
            if let Some(subscription) = subscription {
                subscription.close();
            }
        }
        let mut errors: Vec<ChordError> = Vec::new();
        for (_, keyed) in keyed {
            if let Err(error) = keyed.close() {
                errors.push(error);
            }
        }
        finish_aggregate(errors, "Failed to dispose services")
    }

    /// `#startSingleton(serviceId, binding, revision)` (`consumer.ts:575-609`).
    fn start_singleton(
        self: &Arc<Self>,
        shared: &Arc<SingletonShared>,
        revision: u64,
    ) -> Result<(), ChordError> {
        let listener: UpdateListener = {
            let shared = Arc::downgrade(shared);
            let report_error = self.report_error.clone();
            Arc::new(move |update, context| {
                let Some(shared) = shared.upgrade() else {
                    return Ok(());
                };
                {
                    let state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
                    if !shared.active.load(Ordering::SeqCst) || state.revision != revision {
                        return Ok(());
                    }
                }
                let result: Result<(), ChordError> = match update {
                    ServiceProviderUpdate::Reset { snapshot } => {
                        validate_reset_snapshot(
                            snapshot,
                            &shared.service_id,
                            ServiceMode::Singleton,
                        )?;
                        match snapshot.instances.first() {
                            None => {
                                shared.facade.clear();
                                Ok(())
                            }
                            Some(instance_snapshot) => {
                                shared.facade.install(instance_snapshot, context)
                            }
                        }
                    }
                    ServiceProviderUpdate::Unavailable => {
                        shared.facade.clear();
                        Ok(())
                    }
                    ServiceProviderUpdate::Replaced { snapshot } => {
                        if snapshot.instance.is_some() {
                            Err(ChordError::Type(
                                "Singleton replacement has an instance address".to_owned(),
                            ))
                        } else {
                            shared.facade.install(snapshot, context)
                        }
                    }
                    ServiceProviderUpdate::State {
                        instance: None,
                        member,
                        sequence,
                        ops,
                    } => shared.facade.update(member, *sequence, ops, context),
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    report_error(&error);
                }
                Ok(())
            })
        };
        let subscription =
            self.transport
                .subscribe(&shared.service_id, ServiceMode::Singleton, listener)?;
        let snapshot = {
            let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
            if !shared.active.load(Ordering::SeqCst)
                || self.disposed.load(Ordering::SeqCst)
                || !self.is_bound()
                || state.revision != revision
            {
                drop(state);
                subscription.close();
                return Ok(());
            }
            let snapshot = subscription.snapshot.clone();
            state.subscription = Some(subscription);
            snapshot
        };
        if snapshot.mode != ServiceMode::Singleton
            || snapshot.service_id != shared.service_id
            || snapshot.instances.len() != 1
        {
            return Err(ChordError::Type(format!(
                "Remote service {} returned an invalid singleton snapshot",
                shared.service_id
            )));
        }
        shared
            .facade
            .install(&snapshot.instances[0], &service_delivery_context())?;
        // Activate outside the lifecycle lock: buffered-update replay calls
        // the listener synchronously, which re-enters the same lock.
        let activation = {
            let mut state = shared.state.lock().unwrap_or_else(|p| p.into_inner());
            let subscription = state.subscription.take();
            let Some(subscription) = subscription else {
                return Ok(());
            };
            let result = subscription.activate();
            state.subscription = Some(subscription);
            result
        };
        activation
    }

    pub(crate) fn handle_access(self: &Arc<Self>) -> AccessAssert {
        let weak = Arc::downgrade(self);
        Arc::new(move || {
            let Some(binding) = weak.upgrade() else {
                return Ok(());
            };
            if binding.disposed.load(Ordering::SeqCst) {
                return Err(ChordError::Type(
                    "Remote service binding is disposed".to_owned(),
                ));
            }
            (binding.assert_access)()
        })
    }

    /// The keyed binding for `service`, creating it (with the upstream
    /// on-empty closure, `consumer.ts:487-492`) when absent.
    fn keyed_binding(self: &Arc<Self>, service: &Service) -> Result<Arc<KeyedBinding>, ChordError> {
        let mut inner = self.lock();
        if let Some((_, existing)) = inner.keyed.iter().find(|(id, _)| *id == service.id) {
            return Ok(existing.clone());
        }
        let weak_binding = Arc::downgrade(self);
        let service_id = service.id.clone();
        let transport = self.transport.clone();
        let report_error = self.report_error.clone();
        let assert_access = self.handle_access();
        let bound = self.is_bound();
        let keyed: Arc<KeyedBinding> = Arc::new_cyclic(|weak_keyed| {
            let weak_keyed = weak_keyed.clone();
            let service_id_for_close = service_id.clone();
            KeyedBinding::new(
                service,
                transport,
                report_error,
                assert_access,
                Box::new(move || {
                    let Some(binding) = weak_binding.upgrade() else {
                        return;
                    };
                    let Some(keyed) = weak_keyed.upgrade() else {
                        return;
                    };
                    let still_present = {
                        let mut inner = binding.lock();
                        let still = inner.keyed.iter().any(|(id, existing)| {
                            *id == service_id_for_close && Arc::ptr_eq(existing, &keyed)
                        });
                        if still {
                            inner.keyed.retain(|(id, _)| *id != service_id_for_close);
                            inner.readiness_revision += 1;
                        }
                        still
                    };
                    if still_present {
                        let _ = keyed.close();
                    }
                }),
                bound,
            )
        });
        inner.keyed.push((service.id.clone(), keyed.clone()));
        inner.readiness_revision += 1;
        Ok(keyed)
    }

    /// The [`KeyedServiceSource`] face used by the facet host when binding
    /// keyed sources directly.
    fn keyed_for_source(
        self: &Arc<Self>,
        service: &Service,
    ) -> Result<Arc<KeyedBinding>, ChordError> {
        self.assert_remotable(service)?;
        self.assert_available(&service.id, ServiceMode::Keyed)?;
        self.keyed_binding(service)
    }

    fn assert_remotable(&self, service: &Service) -> Result<(), ChordError> {
        if service.local {
            return Err(remote_error(
                RemoteServiceErrorCode::ServiceNotAllowed,
                format!("Service {} is process-local", service.id),
            ));
        }
        Ok(())
    }

    fn assert_available(&self, service_id: &str, mode: ServiceMode) -> Result<(), ChordError> {
        let mut inner = self.lock();
        if self.disposed.load(Ordering::SeqCst) {
            return Err(ChordError::Type(
                "Remote service binding is disposed".to_owned(),
            ));
        }
        if !inner.allowlist.contains(service_id) {
            return Err(remote_error(
                RemoteServiceErrorCode::ServiceNotAllowed,
                format!("Remote service {service_id} is not allowlisted"),
            ));
        }
        if let Some(existing) = inner.modes.get(service_id) {
            if *existing != mode {
                return Err(remote_error(
                    RemoteServiceErrorCode::ServiceModeMismatch,
                    format!(
                        "Remote service {service_id} is already used as {}",
                        existing.as_str()
                    ),
                ));
            }
        }
        inner.modes.insert(service_id.to_owned(), mode);
        Ok(())
    }
}

fn finish_aggregate(errors: Vec<ChordError>, message: &str) -> Result<(), ChordError> {
    if errors.is_empty() {
        return Ok(());
    }
    if errors.len() == 1 {
        return Err(errors.into_iter().next().expect("non-empty"));
    }
    Err(ChordError::Aggregate {
        message: message.to_owned(),
        errors,
    })
}

#[cfg(test)]
#[path = "consumer_oracle.rs"]
mod consumer_oracle;

// ── loopback (`services/loopback.ts`) ───────────────────────────────────────

/// `createLoopbackServiceTransport(provider)` (`loopback.ts:5-17`): connects
/// a provider to a binding without changing remote service semantics.
pub fn create_loopback_service_transport(
    provider: Arc<RemoteServiceProvider>,
) -> Arc<dyn RemoteServiceTransport> {
    Arc::new(LoopbackServiceTransport { provider })
}

thread_local! {
    /// Marks the thread currently inside a provider publication; reentrant
    /// loopback invokes from that context run on a helper thread (D11).
    static LOOPBACK_IN_PUBLISH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

enum PumpMessage {
    Update(ServiceProviderUpdate, Context),
    Close,
}

struct LoopbackServiceTransport {
    provider: Arc<RemoteServiceProvider>,
}

impl RemoteServiceTransport for LoopbackServiceTransport {
    fn invoke(
        &self,
        call: &ServiceCall,
        context: &Context,
    ) -> Result<Option<JsonValue>, ChordError> {
        // Upstream JS is single-threaded and reentrant: an observer invoked
        // from a provider publication can call straight back into
        // `provider.invoke`. The synchronous port publishes under the
        // provider lock, so a reentrant call must execute on another thread
        // to re-acquire it (divergence D11).
        let reentrant = LOOPBACK_IN_PUBLISH.with(|flag| flag.get());
        if !reentrant {
            LOOPBACK_IN_PUBLISH.with(|flag| flag.set(true));
            let result = self.provider.invoke(call, context);
            LOOPBACK_IN_PUBLISH.with(|flag| flag.set(false));
            return result;
        }
        let provider = self.provider.clone();
        let call = call.clone();
        let context = context.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let _ = sender.send(provider.invoke(&call, &context));
            });
        });
        receiver.recv().unwrap_or_else(|_| {
            Err(ChordError::Type(
                "loopback invoke helper panicked".to_owned(),
            ))
        })
    }

    fn subscribe(
        &self,
        service_id: &str,
        mode: ServiceMode,
        listener: UpdateListener,
    ) -> Result<TransportSubscription, ChordError> {
        let subscription = Arc::new(self.provider.subscribe(
            service_id,
            mode,
            move |update, context| listener(update, context),
        )?);
        let snapshot = subscription.snapshot().clone();
        let activate_subscription = subscription.clone();
        Ok(TransportSubscription::new(
            snapshot,
            Box::new(move || activate_subscription.activate()),
            Box::new(move || subscription.close()),
        ))
    }

    /// Keyed deliveries run the observer inline under the provider lock in
    /// the synchronous port; observers may re-enter the provider (upstream
    /// JS reentrancy), so deliveries are pumped on a dedicated thread that
    /// does not hold the provider lock (divergence D11).
    fn subscribe_keyed(
        &self,
        service_id: &str,
        listener: UpdateListener,
    ) -> Result<TransportSubscription, ChordError> {
        let (sender, receiver) = std::sync::mpsc::channel::<PumpMessage>();
        let subscribe_sender = sender.clone();
        let pump_listener: UpdateListener = Arc::new(move |update, context| {
            let _ = sender.send(PumpMessage::Update(update.clone(), context.clone()));
            Ok(())
        });
        let subscription = Arc::new(self.provider.subscribe(service_id, ServiceMode::Keyed, {
            let pump_listener = pump_listener.clone();
            move |update, context| pump_listener(update, context)
        })?);
        let snapshot = subscription.snapshot().clone();
        std::thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                match message {
                    PumpMessage::Update(update, context) => {
                        let _ = listener(&update, &context);
                    }
                    PumpMessage::Close => break,
                }
            }
        });
        let activate_subscription = subscription.clone();
        let close_subscription = subscription.clone();
        Ok(TransportSubscription::new(
            snapshot,
            Box::new(move || activate_subscription.activate()),
            Box::new(move || {
                let _ = subscribe_sender.send(PumpMessage::Close);
                close_subscription.close();
            }),
        ))
    }
}
