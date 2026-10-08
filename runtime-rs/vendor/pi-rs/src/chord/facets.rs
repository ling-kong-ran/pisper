//! Facet host: the private lifecycle and dependency kernel behind the atomic
//! host entry point, plus the facet loaders. Port of
//! `packages/chord/src/facets/host.ts` (upstream sha256
//! `a31983f78d4d0cb3a30cd5bbd796152078b71da37e9a69ab3c8b6d030b88f831`),
//! `facets/loader.ts` (sha256
//! `ea51215619f0a9ce6d7db5350ee2644e8522126f3cdb863e60c58d16bd0e4646`), and
//! the host/loader constructors of `api.ts` (`createFacetHost`,
//! `createStaticFacetLoader`, `combineFacetLoaders`, `defineFacet`,
//! `createRemoteServiceBinding`).
//!
//! # Divergences (disclosed)
//!
//! - **D2 (synchronous lifecycle)**: upstream activation/reload interleave
//!   promises (`Promise.all` over binding readiness, async disposals). The
//!   port is synchronous; with loopback transports the observable order
//!   (setup → dependency validation → assemble → bind → activate in
//!   topological order, disposals in reverse) is preserved.
//! - **D3 (proxy surfaces → typed handles)**: facet setups receive
//!   [`FacetEnvironment`]; provided implementations are either a classified
//!   remote [`Implementation`] or a type-erased local target; acquired
//!   services are [`ServiceHandle`]s (local downcast or remote member
//!   calls).
//! - **D6 (async setup rejection)**: upstream rejects `async setup()` with
//!   ``Facet {id} setup must be synchronous``. Rust setups are synchronous
//!   functions by construction, so the check is unrepresentable.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::chord::consumer::{
    create_remote_service_binding, AccessAssert, ErrorReporter, KeyedHandler, KeyedServiceSource,
    ObservationStop, RemoteServiceBinding, RemoteServiceBindingOptions, ServiceHandle,
    ServiceTarget,
};
use crate::chord::context::Context;
use crate::chord::handle::ServiceSlot;
use crate::chord::instances::{InstanceDirectory, InstanceDirectoryEntry};
use crate::chord::services::errors::ChordError;
use crate::chord::services::provider::{
    validate_remote_service_implementation, Implementation, ProviderEntry, RemoteServiceProvider,
};
use crate::chord::services::state::MutableReplicatedState;
use crate::chord::types::{JsonValue, Service, ServiceCatalogueEntry, ServiceMode};

pub use crate::chord::consumer::create_loopback_service_transport;

// ── facet definitions ───────────────────────────────────────────────────────

pub type Disposal = Box<dyn FnOnce() -> Result<(), ChordError> + Send>;
/// Arc'd provider operation (install / validateReplacement / replace).
pub type ProviderOp =
    Arc<dyn Fn(&Arc<RemoteServiceProvider>) -> Result<(), ChordError> + Send + Sync>;
/// Arc'd keyed connector over the local registry (upstream `connectLocal`).
pub type ConnectLocal =
    Arc<dyn Fn(&Arc<LocalKeyedServiceRegistry>) -> Result<(), ChordError> + Send + Sync>;
/// Arc'd keyed connector over the provider (upstream `connectRemote`).
pub type ConnectRemote =
    Arc<dyn Fn(&Arc<RemoteServiceProvider>) -> Result<(), ChordError> + Send + Sync>;
/// Facet setup entry point (upstream `facet.setup(env)`).
pub type FacetSetup = Arc<dyn Fn(&mut FacetEnvironment) -> Result<(), ChordError> + Send + Sync>;
type ActivationCallback = Arc<dyn Fn() -> Result<(), ChordError> + Send + Sync>;
pub type CloseHandle = Arc<dyn Fn() -> Result<(), ChordError> + Send + Sync>;

/// The setup entry point of one facet (upstream `Facet`).
#[derive(Clone)]
pub struct Facet {
    pub id: String,
    setup: FacetSetup,
}

impl std::fmt::Debug for Facet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Facet").field("id", &self.id).finish()
    }
}

/// `defineFacet({ id, setup })` (`api.ts:66-68`).
pub fn define_facet(
    id: &str,
    setup: impl Fn(&mut FacetEnvironment) -> Result<(), ChordError> + Send + Sync + 'static,
) -> Facet {
    Facet {
        id: id.to_owned(),
        setup: Arc::new(setup),
    }
}

// ── lifecycle (`facets/host.ts:59-143`) ─────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleState {
    SettingUp,
    Prepared,
    Active,
    Disposing,
    Dead,
}

impl LifecycleState {
    fn as_str(&self) -> &'static str {
        match self {
            LifecycleState::SettingUp => "setting_up",
            LifecycleState::Prepared => "prepared",
            LifecycleState::Active => "active",
            LifecycleState::Disposing => "disposing",
            LifecycleState::Dead => "dead",
        }
    }
}

struct LifecycleInner {
    effects: Vec<Disposal>,
    observations: Vec<Box<dyn FnOnce() -> Disposal + Send>>,
    activate: Vec<ActivationCallback>,
    state: LifecycleState,
    service_access: bool,
}

/// Port of upstream `FacetLifecycle` (`facets/host.ts:59-143`).
struct FacetLifecycle {
    id: String,
    inner: Mutex<LifecycleInner>,
}

impl FacetLifecycle {
    fn new(id: &str) -> Arc<Self> {
        Arc::new(FacetLifecycle {
            id: id.to_owned(),
            inner: Mutex::new(LifecycleInner {
                effects: Vec::new(),
                observations: Vec::new(),
                activate: Vec::new(),
                state: LifecycleState::SettingUp,
                service_access: false,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LifecycleInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `assertSettingUp(operation)` (`facets/host.ts:71-75`).
    fn assert_setting_up(&self, operation: &str) -> Result<(), ChordError> {
        let inner = self.lock();
        if inner.state != LifecycleState::SettingUp {
            return Err(ChordError::Type(format!(
                "Facet {} can {} only during setup",
                self.id, operation
            )));
        }
        Ok(())
    }

    /// `assertRunning(operation)` (`facets/host.ts:77-81`).
    fn assert_running(&self, operation: &str) -> Result<(), ChordError> {
        let inner = self.lock();
        if inner.state != LifecycleState::SettingUp && inner.state != LifecycleState::Active {
            return Err(ChordError::Type(format!(
                "Facet {} cannot {} while {}",
                self.id,
                operation,
                inner.state.as_str()
            )));
        }
        Ok(())
    }

    /// `assertActive(operation)` (`facets/host.ts:83-85`).
    fn assert_active(&self, operation: &str) -> Result<(), ChordError> {
        let inner = self.lock();
        if inner.state != LifecycleState::Active {
            return Err(ChordError::Type(format!(
                "Facet {} can {} only while active",
                self.id, operation
            )));
        }
        Ok(())
    }

    /// `assertServiceAccess()` (`facets/host.ts:87-91`).
    fn assert_service_access(&self) -> Result<(), ChordError> {
        let inner = self.lock();
        if !inner.service_access {
            return Err(ChordError::Type(format!(
                "Facet {} service handles cannot be used while {}",
                self.id,
                inner.state.as_str()
            )));
        }
        Ok(())
    }

    /// The [`AccessAssert`] view of [`FacetLifecycle::assert_service_access`].
    fn service_access_assert(self: &Arc<Self>) -> AccessAssert {
        let lifecycle = self.clone();
        Arc::new(move || lifecycle.assert_service_access())
    }

    /// `revoke()` (`facets/host.ts:93-95`).
    fn revoke(&self) {
        self.lock().service_access = false;
    }

    /// `own(disposal)` (`facets/host.ts:97-100`).
    fn own(&self, disposal: Disposal) -> Result<(), ChordError> {
        self.assert_running("own resources")?;
        self.lock().effects.push(disposal);
        Ok(())
    }

    /// `observe(start)` (`facets/host.ts:102-105`).
    fn observe(&self, start: Box<dyn FnOnce() -> Disposal + Send>) -> Result<(), ChordError> {
        self.assert_setting_up("observe services")?;
        self.lock().observations.push(start);
        Ok(())
    }

    /// `onActivate(callback)` (`facets/host.ts:107-110`).
    fn on_activate(&self, callback: ActivationCallback) -> Result<(), ChordError> {
        self.assert_setting_up("register activation callbacks")?;
        self.lock().activate.push(callback);
        Ok(())
    }

    /// `prepared()` (`facets/host.ts:112-115`).
    fn prepared(&self) -> Result<(), ChordError> {
        self.assert_setting_up("finish setup")?;
        self.lock().state = LifecycleState::Prepared;
        Ok(())
    }

    /// `activate()` (`facets/host.ts:117-123`).
    fn activate(&self) -> Result<(), ChordError> {
        {
            let mut inner = self.lock();
            if inner.state != LifecycleState::Prepared {
                return Err(ChordError::Type(format!(
                    "Facet {} is not prepared",
                    self.id
                )));
            }
            inner.state = LifecycleState::Active;
            inner.service_access = true;
            let observations = std::mem::take(&mut inner.observations);
            for start in observations {
                inner.effects.push(start());
            }
        }
        let callbacks = self.lock().activate.clone();
        for callback in callbacks {
            callback()?;
        }
        Ok(())
    }

    /// `dispose()` (`facets/host.ts:125-142`).
    fn dispose(&self) -> Result<(), ChordError> {
        {
            let mut inner = self.lock();
            if inner.state == LifecycleState::Dead {
                return Ok(());
            }
            inner.state = LifecycleState::Disposing;
        }
        let effects = std::mem::take(&mut self.lock().effects);
        let mut errors: Vec<ChordError> = Vec::new();
        for effect in effects.into_iter().rev() {
            if let Err(error) = effect() {
                errors.push(error);
            }
        }
        {
            let mut inner = self.lock();
            inner.observations.clear();
            inner.activate.clear();
            inner.service_access = false;
            inner.state = LifecycleState::Dead;
        }
        match errors.len() {
            0 => Ok(()),
            1 => Err(errors.into_iter().next().expect("non-empty")),
            _ => Err(ChordError::Aggregate {
                message: format!("Failed to dispose facet {}", self.id),
                errors,
            }),
        }
    }
}

// ── implementations and provisions ──────────────────────────────────────────

/// A provided implementation: classified remote members (upstream a plain
/// remotable object) or a type-erased local target (upstream an unrestricted
/// object for `local: true` services).
#[derive(Clone)]
pub enum ProvidedImplementation {
    Remote(Implementation),
    Local(Arc<dyn std::any::Any + Send + Sync>),
}

impl std::fmt::Debug for ProvidedImplementation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvidedImplementation::Remote(members) => {
                f.debug_tuple("Remote").field(&members.len()).finish()
            }
            ProvidedImplementation::Local(_) => f.debug_tuple("Local").finish(),
        }
    }
}

fn remote_members(
    service: &Service,
    implementation: &ProvidedImplementation,
) -> Result<Implementation, ChordError> {
    match implementation {
        ProvidedImplementation::Remote(members) => Ok(members.clone()),
        ProvidedImplementation::Local(_) => Err(ChordError::Type(format!(
            "Service {} implementation must be a remote contract",
            service.id
        ))),
    }
}

/// Upstream `FacetProvision` (`facets/host.ts:323-337`). `Arc` closures keep
/// the provision clonable across staged reloads.
#[derive(Clone)]
enum FacetProvision {
    Singleton {
        service: Service,
        implementation: ProvidedImplementation,
        install: ProviderOp,
        validate_replacement: ProviderOp,
        replace: ProviderOp,
    },
    Keyed {
        service: Service,
        connect_local: ConnectLocal,
        connect_remote: ConnectRemote,
    },
}

impl std::fmt::Debug for FacetProvision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FacetProvision::Singleton { service, .. } => f
                .debug_struct("Singleton")
                .field("service", &service.id)
                .finish(),
            FacetProvision::Keyed { service, .. } => f
                .debug_struct("Keyed")
                .field("service", &service.id)
                .finish(),
        }
    }
}

// ── keyed sources ───────────────────────────────────────────────────────────

struct LkrRegistration {
    generations: Mutex<HashMap<String, u64>>,
    directory: InstanceDirectory,
}

struct LkrInner {
    registrations: HashMap<String, LkrRegistration>,
    disposed: bool,
}

/// Port of upstream `LocalKeyedServiceRegistry` (`facets/host.ts:154-217`).
pub struct LocalKeyedServiceRegistry {
    inner: Mutex<LkrInner>,
}

impl std::fmt::Debug for LocalKeyedServiceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalKeyedServiceRegistry")
            .field("registrations", &self.lock().registrations.len())
            .finish()
    }
}

impl LocalKeyedServiceRegistry {
    fn new(services: &[Service], on_error: ErrorReporter) -> Result<Arc<Self>, ChordError> {
        let mut registrations = HashMap::new();
        for service in services {
            if registrations.contains_key(&service.id) {
                return Err(ChordError::Type(
                    "Local keyed service registry has duplicate IDs".to_owned(),
                ));
            }
            registrations.insert(
                service.id.clone(),
                LkrRegistration {
                    generations: Mutex::new(HashMap::new()),
                    directory: InstanceDirectory::new(true, on_error.clone()),
                },
            );
        }
        Ok(Arc::new(LocalKeyedServiceRegistry {
            inner: Mutex::new(LkrInner {
                registrations,
                disposed: false,
            }),
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LkrInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn assert_active(&self) -> Result<(), ChordError> {
        if self.lock().disposed {
            return Err(ChordError::Type(
                "Local keyed service registry is disposed".to_owned(),
            ));
        }
        Ok(())
    }

    /// `spawn(service, key, implementation)` (`facets/host.ts:169-194`).
    fn spawn(
        self: &Arc<Self>,
        service: &Service,
        key: &str,
        implementation: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<Disposal, ChordError> {
        self.assert_active()?;
        if key.is_empty() {
            return Err(ChordError::Type(
                "Local service instance key must not be empty".to_owned(),
            ));
        }
        let directory = {
            let inner = self.lock();
            let Some(registration) = inner.registrations.get(&service.id) else {
                return Err(ChordError::Type(format!(
                    "Local keyed service {} is not registered",
                    service.id
                )));
            };
            if registration.directory.generation_of(key).is_some() {
                return Err(ChordError::Type(format!(
                    "Local service {} already has a live instance with key {}",
                    service.id, key
                )));
            }
            let mut generations = registration
                .generations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let generation = generations.get(key).copied().unwrap_or(0) + 1;
            generations.insert(key.to_owned(), generation);
            let directory = registration.directory.clone();
            registration.directory.insert(InstanceDirectoryEntry::new(
                key,
                generation,
                implementation,
                Box::new(|| {}),
            ))?;
            directory
        };
        let key = key.to_owned();
        Ok(Box::new(move || {
            // A stale close leaves the re-spawned instance alone; resolve
            // the live generation at close time.
            if let Some(generation) = directory.generation_of(&key) {
                directory.remove(&key, generation);
            }
            Ok(())
        }))
    }

    fn registration_directory(&self, service_id: &str) -> Result<InstanceDirectory, ChordError> {
        let inner = self.lock();
        let Some(registration) = inner.registrations.get(service_id) else {
            return Err(ChordError::Type(format!(
                "Local keyed service {service_id} is not registered"
            )));
        };
        Ok(registration.directory.clone())
    }

    fn dispose(&self) {
        let mut inner = self.lock();
        if inner.disposed {
            return;
        }
        inner.disposed = true;
        for registration in inner.registrations.values() {
            registration.directory.dispose();
        }
        inner.registrations.clear();
    }
}

impl KeyedServiceSource for LocalKeyedServiceRegistry {
    fn observe_keyed(
        self: Arc<Self>,
        service: &Service,
        handler: crate::chord::consumer::TargetHandler,
    ) -> Result<ObservationStop, ChordError> {
        self.assert_active()?;
        let directory = self.registration_directory(&service.id)?;
        directory.observe(Arc::new(
            move |target: &crate::chord::instances::DirectoryTarget, context: &Context| {
                handler(&ServiceTarget::Local(target.clone()), context)
            },
        ))
    }
}

// ── host service slots (`facets/host.ts:219-277`) ───────────────────────────

struct SlotsInner {
    singletons: HashMap<String, Arc<ServiceSlot>>,
    keyed_sources: HashMap<String, Arc<dyn KeyedServiceSource>>,
}

/// Port of upstream `HostServiceSlots`.
pub struct HostServiceSlots {
    inner: Mutex<SlotsInner>,
}

impl std::fmt::Debug for HostServiceSlots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostServiceSlots").finish()
    }
}

use std::sync::atomic::{AtomicBool, Ordering};

impl HostServiceSlots {
    fn new() -> Self {
        HostServiceSlots {
            inner: Mutex::new(SlotsInner {
                singletons: HashMap::new(),
                keyed_sources: HashMap::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SlotsInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `getSingleton(service, assertAccess)` (`facets/host.ts:223-230`).
    fn get_singleton(&self, service: &Service, assert: AccessAssert) -> ServiceHandle {
        let slot = self
            .lock()
            .singletons
            .entry(service.id.clone())
            .or_insert_with(|| ServiceSlot::new(&service.id))
            .clone();
        ServiceHandle::new(ServiceTarget::Slot(slot), assert)
    }

    /// `hasSingleton(serviceId)` (`facets/host.ts:232-234`).
    fn has_singleton(&self, service_id: &str) -> bool {
        self.lock().singletons.contains_key(service_id)
    }

    /// `observe(service, assertAccess, handler)` (`facets/host.ts:236-262`).
    fn observe(
        &self,
        service: &Service,
        assert_base: AccessAssert,
        handler: KeyedHandler,
    ) -> Result<ObservationStop, ChordError> {
        let source = self
            .lock()
            .keyed_sources
            .get(&service.id)
            .cloned()
            .ok_or_else(|| ChordError::Type(format!("Service {} is disconnected", service.id)))?;
        let stopped = Arc::new(AtomicBool::new(false));
        let service_id = service.id.clone();
        let stopped_for_view = stopped.clone();
        let stop = source.observe_keyed(
            service,
            Arc::new(move |target, context| {
                let signal = context.abort_signal();
                let assert: AccessAssert = {
                    let assert_base = assert_base.clone();
                    let stopped = stopped_for_view.clone();
                    let service_id = service_id.clone();
                    Arc::new(move || {
                        assert_base()?;
                        if stopped.load(Ordering::SeqCst)
                            || signal.as_ref().is_some_and(|signal| signal.is_cancelled())
                        {
                            return Err(ChordError::Type(format!(
                                "Keyed service {service_id} observation is closed"
                            )));
                        }
                        Ok(())
                    })
                };
                let view = ServiceHandle::new(target.clone(), assert);
                handler(&view, context)
            }),
        )?;
        let stopped_for_stop = stopped;
        Ok(Box::new(move || {
            if stopped_for_stop.swap(true, Ordering::SeqCst) {
                return;
            }
            stop();
        }))
    }

    /// `bindSingleton(serviceId, target)` (`facets/host.ts:264-266`).
    fn bind_singleton(&self, service_id: &str, target: Arc<dyn std::any::Any + Send + Sync>) {
        if let Some(slot) = self.lock().singletons.get(service_id) {
            slot.bind(target);
        }
    }

    /// Bind a singleton slot from a resolved service handle (the port's
    /// `bindSingleton(id, services.use(service))` over remote facades).
    fn bind_singleton_handle(&self, service_id: &str, handle: &ServiceHandle) {
        match &handle.target {
            ServiceTarget::Remote(facade) => self.bind_singleton(service_id, facade.clone()),
            ServiceTarget::Local(any) => self.bind_singleton(service_id, any.clone()),
            ServiceTarget::Slot(_) => {}
        }
    }

    /// `bindKeyed(serviceId, services)` (`facets/host.ts:268-270`).
    fn bind_keyed(&self, service_id: &str, source: Arc<dyn KeyedServiceSource>) {
        self.lock()
            .keyed_sources
            .insert(service_id.to_owned(), source);
    }

    /// `dispose()` (`facets/host.ts:272-276`).
    fn dispose(&self) {
        let mut inner = self.lock();
        for slot in inner.singletons.values() {
            slot.unbind();
        }
        inner.singletons.clear();
        inner.keyed_sources.clear();
    }
}

// ── staged service spawner (`facets/host.ts:287-321`) ───────────────────────

#[derive(Clone)]
enum SpawnerInstaller {
    Local(Arc<LocalKeyedServiceRegistry>),
    Remote(Arc<RemoteServiceProvider>),
}

struct StagedInstance {
    key: String,
    implementation: ProvidedImplementation,
    release: Option<Disposal>,
}

struct SpawnerInner {
    instances: Vec<StagedInstance>,
    installer: Option<SpawnerInstaller>,
}

/// Port of upstream `StagedServiceSpawner`.
pub struct StagedServiceSpawner {
    service: Service,
    lifecycle: Arc<FacetLifecycle>,
    inner: Mutex<SpawnerInner>,
}

impl std::fmt::Debug for StagedServiceSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedServiceSpawner")
            .field("service", &self.service.id)
            .finish()
    }
}

fn installer_run(
    service: &Service,
    installer: &SpawnerInstaller,
    key: &str,
    implementation: &ProvidedImplementation,
) -> Result<Disposal, ChordError> {
    match installer {
        SpawnerInstaller::Local(registry) => {
            let ProvidedImplementation::Local(any) = implementation else {
                return Err(ChordError::Type(format!(
                    "Service {} implementation must be a local object",
                    service.id
                )));
            };
            registry.spawn(service, key, any.clone())
        }
        SpawnerInstaller::Remote(provider) => {
            let members = remote_members(service, implementation)?;
            let handle = provider.spawn(service, key, members)?;
            Ok(Box::new(move || {
                handle.close()?;
                Ok(())
            }))
        }
    }
}

impl StagedServiceSpawner {
    fn new(service: &Service, lifecycle: Arc<FacetLifecycle>) -> Arc<StagedServiceSpawner> {
        Arc::new(StagedServiceSpawner {
            service: service.clone(),
            lifecycle,
            inner: Mutex::new(SpawnerInner {
                instances: Vec::new(),
                installer: None,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SpawnerInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `connect(installer)` (`facets/host.ts:298-304`).
    fn connect(&self, installer: SpawnerInstaller) -> Result<(), ChordError> {
        let mut inner = self.lock();
        if inner.installer.is_some() {
            return Err(ChordError::Type(
                "Facet service provider is already connected".to_owned(),
            ));
        }
        inner.installer = Some(installer);
        let staged: Vec<(String, ProvidedImplementation)> = inner
            .instances
            .iter()
            .map(|instance| (instance.key.clone(), instance.implementation.clone()))
            .collect();
        let installer = inner.installer.clone().expect("just assigned");
        for (key, implementation) in staged {
            let release = installer_run(&self.service, &installer, &key, &implementation)?;
            if let Some(instance) = inner.instances.iter_mut().find(|i| i.key == key) {
                instance.release = Some(release);
            }
        }
        Ok(())
    }

    /// `spawn(key, implementation)` (`facets/host.ts:306-320`).
    pub fn spawn(
        self: &Arc<Self>,
        key: &str,
        implementation: ProvidedImplementation,
    ) -> Result<CloseHandle, ChordError> {
        self.lifecycle.assert_active("spawn service instances")?;
        if key.is_empty() {
            return Err(ChordError::Type(
                "Facet service instance key must not be empty".to_owned(),
            ));
        }
        if let ProvidedImplementation::Remote(members) = &implementation {
            if !self.service.local {
                validate_remote_service_implementation(&self.service.id, members)?;
            }
        }
        {
            let inner = self.lock();
            if inner.instances.iter().any(|instance| instance.key == key) {
                return Err(ChordError::Type(format!(
                    "Facet service already has a live instance with key {}",
                    key
                )));
            }
        }
        let release = {
            let inner = self.lock();
            match inner.installer.as_ref() {
                Some(installer) => Some(installer_run(
                    &self.service,
                    installer,
                    key,
                    &implementation,
                )?),
                None => None,
            }
        };
        let spawner = self.clone();
        let key_owned = key.to_owned();
        let close: CloseHandle = Arc::new(move || {
            let mut inner = spawner.lock();
            let Some(position) = inner
                .instances
                .iter()
                .position(|instance| instance.key == key_owned)
            else {
                return Ok(());
            };
            let instance = inner.instances.remove(position);
            drop(inner);
            if let Some(release) = instance.release {
                release()?;
            }
            Ok(())
        });
        self.lock().instances.push(StagedInstance {
            key: key.to_owned(),
            implementation,
            release,
        });
        let owned_close = close.clone();
        self.lifecycle.own(Box::new(move || owned_close()))?;
        Ok(close)
    }
}

// ── facet runtime and environment ───────────────────────────────────────────

#[derive(Clone, Debug)]
struct FacetServiceReference {
    service_id: String,
    service: Service,
    mode: ServiceMode,
}

#[derive(Debug, Default, Clone)]
struct FacetRuntimeData {
    requires: Vec<FacetServiceReference>,
    provides: Vec<FacetServiceReference>,
    provisions: Vec<FacetProvision>,
    singleton_views: HashMap<String, ServiceHandle>,
}

#[derive(Clone)]
struct FacetRuntime {
    facet_id: String,
    data: FacetRuntimeData,
    lifecycle: Arc<FacetLifecycle>,
}

/// The setup environment handed to [`define_facet`] setups (upstream
/// `FacetEnvironment`, `facets/host.ts:528-594`).
pub struct FacetEnvironment<'a> {
    runtime: &'a mut FacetRuntimeData,
    lifecycle: Arc<FacetLifecycle>,
    slots: Arc<Mutex<HostServiceSlots>>,
}

fn record_service_reference(
    target: &mut Vec<FacetServiceReference>,
    service: &Service,
    mode: ServiceMode,
) {
    if target
        .iter()
        .any(|reference| reference.service_id == service.id && reference.mode == mode)
    {
        return;
    }
    target.push(FacetServiceReference {
        service_id: service.id.clone(),
        service: service.clone(),
        mode,
    });
}

impl FacetEnvironment<'_> {
    fn lifecycle_assert(&self) -> AccessAssert {
        self.lifecycle.service_access_assert()
    }

    /// `provide(service, implementation)` (`facets/host.ts:531-546`).
    pub fn provide(
        &mut self,
        service: &Service,
        implementation: ProvidedImplementation,
    ) -> Result<(), ChordError> {
        self.lifecycle.assert_setting_up("provide services")?;
        record_service_reference(&mut self.runtime.provides, service, ServiceMode::Singleton);
        let service_for = service.clone();
        let install: ProviderOp = {
            let service = service_for.clone();
            let implementation = implementation.clone();
            Arc::new(move |provider| {
                provider.provide(&service, remote_members(&service, &implementation)?)
            })
        };
        let validate_replacement: ProviderOp = {
            let service = service_for.clone();
            let implementation = implementation.clone();
            Arc::new(move |provider| {
                let members = remote_members(&service, &implementation)?;
                provider.validate_replacement(&service, &members)
            })
        };
        let replace: ProviderOp = {
            let service = service_for.clone();
            let implementation = implementation.clone();
            Arc::new(move |provider| {
                provider.replace(&service, remote_members(&service, &implementation)?)
            })
        };
        self.runtime.provisions.push(FacetProvision::Singleton {
            service: service.clone(),
            implementation,
            install,
            validate_replacement,
            replace,
        });
        Ok(())
    }

    /// `provideMany(service)` (`facets/host.ts:547-568`).
    pub fn provide_many(
        &mut self,
        service: &Service,
    ) -> Result<StagedServiceSpawnerGuard, ChordError> {
        self.lifecycle
            .assert_setting_up("provide service instances")?;
        record_service_reference(&mut self.runtime.provides, service, ServiceMode::Keyed);
        let spawner = StagedServiceSpawner::new(service, self.lifecycle.clone());
        let connect_local_spawner = spawner.clone();
        let connect_remote_spawner = spawner.clone();
        self.runtime.provisions.push(FacetProvision::Keyed {
            service: service.clone(),
            connect_local: Arc::new(move |registry| {
                connect_local_spawner.connect(SpawnerInstaller::Local(registry.clone()))
            }),
            connect_remote: Arc::new(move |provider| {
                connect_remote_spawner.connect(SpawnerInstaller::Remote(provider.clone()))
            }),
        });
        Ok(StagedServiceSpawnerGuard { spawner })
    }

    /// `use(service)` (`facets/host.ts:569-578`).
    pub fn use_service(&mut self, service: &Service) -> Result<ServiceHandle, ChordError> {
        self.lifecycle.assert_setting_up("acquire services")?;
        record_service_reference(&mut self.runtime.requires, service, ServiceMode::Singleton);
        if let Some(view) = self.runtime.singleton_views.get(&service.id) {
            return Ok(view.clone());
        }
        let view = self
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_singleton(service, self.lifecycle_assert());
        self.runtime
            .singleton_views
            .insert(service.id.clone(), view.clone());
        Ok(view)
    }

    /// `observe(service, handler)` (`facets/host.ts:579-585`).
    pub fn observe(&mut self, service: &Service, handler: KeyedHandler) -> Result<(), ChordError> {
        self.lifecycle.assert_setting_up("observe services")?;
        record_service_reference(&mut self.runtime.requires, service, ServiceMode::Keyed);
        let slots = self.slots.clone();
        let lifecycle = self.lifecycle.clone();
        let service = service.clone();
        self.lifecycle.observe(Box::new(move || {
            let assert = lifecycle.service_access_assert();
            let observe_slots = slots.clone();
            let handler = handler.clone();
            let service = service.clone();
            Box::new(move || {
                let stop = observe_slots
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .observe(&service, assert, handler)?;
                stop();
                Ok(())
            })
        }))?;
        Ok(())
    }

    /// `replicatedState(initial)` (`facets/host.ts:586-589`).
    pub fn replicated_state(
        &self,
        initial: JsonValue,
    ) -> Result<Arc<MutableReplicatedState>, ChordError> {
        self.lifecycle.assert_running("create replicated state")?;
        Ok(MutableReplicatedState::new(initial))
    }

    /// `own(disposal)` (`facets/host.ts:590`).
    pub fn own(&mut self, disposal: Disposal) -> Result<(), ChordError> {
        self.lifecycle.own(disposal)
    }

    /// `onActivate(callback)` (`facets/host.ts:591`).
    pub fn on_activate(&mut self, callback: ActivationCallback) -> Result<(), ChordError> {
        self.lifecycle.on_activate(callback)
    }

    /// `onDeactivate(callback)` (`facets/host.ts:592`).
    pub fn on_deactivate(&mut self, callback: ActivationCallback) -> Result<(), ChordError> {
        self.lifecycle.own(Box::new(move || callback()))
    }
}

/// Owned spawner handle returned from [`FacetEnvironment::provide_many`]
/// (upstream returns the spawner object directly).
pub struct StagedServiceSpawnerGuard {
    pub spawner: Arc<StagedServiceSpawner>,
}

impl std::ops::Deref for StagedServiceSpawnerGuard {
    type Target = Arc<StagedServiceSpawner>;
    fn deref(&self) -> &Arc<StagedServiceSpawner> {
        &self.spawner
    }
}

// silence: marker removed

// ── service sources (upstream `RemoteServiceSource`) ────────────────────────

/// Options handed to [`ServiceSource::open`] (upstream
/// `RemoteServiceBindingOptions` subset used by the host).
pub struct ServiceSourceOpenOptions {
    pub services: Vec<Service>,
    pub assert_access: AccessAssert,
    pub on_error: ErrorReporter,
}

/// What a source's `open` returns (upstream `RemoteServices`).
pub trait ServiceSourceBinding: Send + Sync {
    fn use_service(&self, service: &Service) -> Result<ServiceHandle, ChordError>;
    fn observe(
        &self,
        service: &Service,
        handler: KeyedHandler,
    ) -> Result<ObservationStop, ChordError>;
    fn ready(&self, context: &Context) -> Result<(), ChordError>;
    fn dispose(&self, context: &Context) -> Result<(), ChordError>;
}

/// A [`ServiceSourceBinding`] view over an owned [`Arc<RemoteServiceBinding>`];
/// used for the internal binding and by source adapters built on bindings.
pub struct BindingSource {
    binding: Arc<RemoteServiceBinding>,
}

impl BindingSource {
    pub fn new(binding: Arc<RemoteServiceBinding>) -> Arc<Self> {
        Arc::new(BindingSource { binding })
    }
}

impl ServiceSourceBinding for BindingSource {
    fn use_service(&self, service: &Service) -> Result<ServiceHandle, ChordError> {
        self.binding.use_service(service)
    }

    fn observe(
        &self,
        service: &Service,
        handler: KeyedHandler,
    ) -> Result<ObservationStop, ChordError> {
        self.binding.observe(service, handler)
    }

    fn ready(&self, _context: &Context) -> Result<(), ChordError> {
        self.binding.ready()
    }

    fn dispose(&self, _context: &Context) -> Result<(), ChordError> {
        self.binding.dispose()
    }
}

/// The source's catalogue resolver (upstream `catalogue(context)`).
pub type CatalogueFn =
    Arc<dyn Fn(&Context) -> Result<Vec<ServiceCatalogueEntry>, ChordError> + Send + Sync>;
/// The source's binding opener (upstream `open(options)`).
pub type OpenFn = Arc<
    dyn Fn(ServiceSourceOpenOptions) -> Result<Arc<dyn ServiceSourceBinding>, ChordError>
        + Send
        + Sync,
>;

/// One external service source (upstream `RemoteServiceSource`).
pub struct ServiceSource {
    pub accepts_unavailable_services: bool,
    pub catalogue: CatalogueFn,
    pub open: OpenFn,
}

impl std::fmt::Debug for ServiceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceSource")
            .field(
                "accepts_unavailable_services",
                &self.accepts_unavailable_services,
            )
            .finish()
    }
}

// ── the kernel (`facets/host.ts:340-794`) ───────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GenerationPhase {
    Setup,
    Assembling,
    Connecting,
    Activating,
    Active,
    Reloading,
    Disposing,
    Dead,
}

impl GenerationPhase {
    fn as_str(&self) -> &'static str {
        match self {
            GenerationPhase::Setup => "setup",
            GenerationPhase::Assembling => "assembling",
            GenerationPhase::Connecting => "connecting",
            GenerationPhase::Activating => "activating",
            GenerationPhase::Active => "active",
            GenerationPhase::Reloading => "reloading",
            GenerationPhase::Disposing => "disposing",
            GenerationPhase::Dead => "dead",
        }
    }
}

struct KernelState {
    facets: Vec<(String, FacetRuntime)>,
    activation_order: Vec<String>,
}

/// Options for [`FacetKernel::new`] (upstream `FacetOptions`).
pub struct FacetOptions {
    pub facets: Vec<Facet>,
    pub service_sources: Vec<Arc<ServiceSource>>,
    pub on_error: Option<ErrorReporter>,
}

/// Port of upstream `FacetKernel` (`facets/host.ts:340-794`).
pub struct FacetKernel {
    initial_facets: Vec<Facet>,
    service_sources: Vec<Arc<ServiceSource>>,
    on_error: ErrorReporter,
    state: Mutex<KernelState>,
    phase: Arc<Mutex<GenerationPhase>>,
    slots: Arc<Mutex<HostServiceSlots>>,
    source_bindings: Mutex<Vec<(usize, Arc<dyn ServiceSourceBinding>)>>,
    provider: Mutex<Option<Arc<RemoteServiceProvider>>>,
    internal_services: Mutex<Option<Arc<RemoteServiceBinding>>>,
    local_keyed: Mutex<Option<Arc<LocalKeyedServiceRegistry>>>,
}

impl std::fmt::Debug for FacetKernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FacetKernel")
            .field(
                "phase",
                &self
                    .phase
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_str(),
            )
            .finish()
    }
}

impl FacetKernel {
    /// `new FacetKernel(options)` (`facets/host.ts:353-361`).
    pub fn new(options: FacetOptions) -> Result<FacetKernel, ChordError> {
        let ids: Vec<&str> = options
            .facets
            .iter()
            .map(|facet| facet.id.as_str())
            .collect();
        if ids.iter().any(|id| id.is_empty()) {
            return Err(ChordError::Type("Facet ID must not be empty".to_owned()));
        }
        let unique: HashSet<&str> = ids.iter().copied().collect();
        if unique.len() != ids.len() {
            return Err(ChordError::Type(
                "Facet IDs must be unique within a generation".to_owned(),
            ));
        }
        Ok(FacetKernel {
            initial_facets: options.facets,
            service_sources: options.service_sources,
            on_error: options
                .on_error
                .unwrap_or_else(crate::chord::consumer::ignore_errors),
            state: Mutex::new(KernelState {
                facets: Vec::new(),
                activation_order: Vec::new(),
            }),
            phase: Arc::new(Mutex::new(GenerationPhase::Setup)),
            slots: Arc::new(Mutex::new(HostServiceSlots::new())),
            source_bindings: Mutex::new(Vec::new()),
            provider: Mutex::new(None),
            internal_services: Mutex::new(None),
            local_keyed: Mutex::new(None),
        })
    }

    fn phase(&self) -> GenerationPhase {
        *self.phase.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_phase(&self, phase: GenerationPhase) {
        *self.phase.lock().unwrap_or_else(|p| p.into_inner()) = phase;
    }

    /// `get provider` (`facets/host.ts:363-366`).
    pub fn provider(&self) -> Result<Arc<RemoteServiceProvider>, ChordError> {
        self.provider
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| ChordError::Type("Facet service provider is not assembled".to_owned()))
    }

    /// `#assertServiceTargetAccess` (`facets/host.ts:736-745`).
    fn assert_service_target_access(&self) -> Result<(), ChordError> {
        match self.phase() {
            GenerationPhase::Activating
            | GenerationPhase::Active
            | GenerationPhase::Reloading
            | GenerationPhase::Disposing => Ok(()),
            other => Err(ChordError::Type(format!(
                "Facet service targets cannot be used during {}",
                other.as_str()
            ))),
        }
    }

    fn target_access_assert(self: &Arc<Self>) -> AccessAssert {
        let kernel = Arc::downgrade(self);
        Arc::new(move || {
            let Some(kernel) = kernel.upgrade() else {
                return Ok(());
            };
            kernel.assert_service_target_access()
        })
    }

    /// `#setupFacet(facet, record)` (`facets/host.ts:379-386`).
    fn setup_facet(
        &self,
        facet: &Facet,
        data: &mut FacetRuntimeData,
        lifecycle: Arc<FacetLifecycle>,
    ) -> Result<(), ChordError> {
        let mut environment = FacetEnvironment {
            runtime: data,
            lifecycle,
            slots: self.slots.clone(),
        };
        (facet.setup)(&mut environment)?;
        // Upstream rejects promise-returning setups (`isPromiseLike`,
        // `facets/host.ts:381-384`); the port's setups are synchronous by
        // construction (divergence D6).
        environment.lifecycle.prepared()
    }

    fn with_runtimes<T>(&self, read: impl FnOnce(&KernelState) -> T) -> T {
        read(&self.state.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// `activate()` (`facets/host.ts:388-421`).
    pub fn activate(self: &Arc<Self>) -> Result<(), ChordError> {
        let result: Result<(), ChordError> = (|| {
            let mut records: Vec<FacetRuntime> = Vec::new();
            for facet in &self.initial_facets {
                let mut runtime = FacetRuntime {
                    facet_id: facet.id.clone(),
                    data: FacetRuntimeData::default(),
                    lifecycle: FacetLifecycle::new(&facet.id),
                };
                self.setup_facet(facet, &mut runtime.data, runtime.lifecycle.clone())?;
                self.state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .facets
                    .push((facet.id.clone(), runtime.clone()));
                records.push(runtime);
            }

            self.set_phase(GenerationPhase::Assembling);
            let external_services = self.resolve_external_services(&records)?;
            let activation_order = validate_facets(&records, &external_services)?;
            self.state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .activation_order = activation_order;
            self.assemble_providers()?;
            self.bind_services(&external_services)?;

            self.set_phase(GenerationPhase::Connecting);
            // Upstream awaits every binding's `ready(context)`; the
            // synchronous port completes inline (divergence D2).
            {
                let bindings = self
                    .source_bindings
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                for (_, binding) in bindings {
                    binding.ready(&Context::background())?;
                }
            }
            {
                let internal = self
                    .internal_services
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                if let Some(internal) = internal {
                    internal.ready()?;
                }
            }

            self.set_phase(GenerationPhase::Activating);
            let order = self.with_runtimes(|state| state.activation_order.clone());
            for id in &order {
                let lifecycle = self.with_runtimes(|state| {
                    state
                        .facets
                        .iter()
                        .find(|(facet_id, _)| facet_id == id)
                        .map(|(_, runtime)| runtime.lifecycle.clone())
                });
                if let Some(lifecycle) = lifecycle {
                    lifecycle.activate()?;
                }
            }
            self.set_phase(GenerationPhase::Active);
            Ok(())
        })();
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                let cleanup_errors = self.terminate(&[]);
                if !cleanup_errors.is_empty() {
                    let mut errors = vec![error];
                    errors.extend(cleanup_errors);
                    return Err(ChordError::Aggregate {
                        message: "Facet generation startup and cleanup failed".to_owned(),
                        errors,
                    });
                }
                Err(error)
            }
        }
    }

    /// `reload(facets)` (`facets/host.ts:423-511`).
    pub fn reload(self: &Arc<Self>, facets: &[Facet]) -> Result<(), ChordError> {
        if self.phase() != GenerationPhase::Active {
            return Err(ChordError::Type(format!(
                "Facet host cannot reload while {}",
                self.phase().as_str()
            )));
        }
        let ids: Vec<&str> = facets.iter().map(|facet| facet.id.as_str()).collect();
        if ids.iter().any(|id| id.is_empty()) {
            return Err(ChordError::Type("Facet ID must not be empty".to_owned()));
        }
        let unique: HashSet<&str> = ids.iter().copied().collect();
        if unique.len() != ids.len() {
            return Err(ChordError::Type(
                "Reloaded facet IDs must be unique".to_owned(),
            ));
        }
        {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            for id in &ids {
                if !state.facets.iter().any(|(facet_id, _)| facet_id == *id) {
                    return Err(ChordError::Type(format!("Facet {id} is not active")));
                }
            }
        }
        self.set_phase(GenerationPhase::Reloading);

        // Stage the replacement records.
        let staged: Vec<FacetRuntime> = Vec::new();
        let setup_result: Result<Vec<FacetRuntime>, ChordError> = (|| {
            let mut candidates: Vec<FacetRuntime> = Vec::new();
            for facet in facets {
                let mut record = FacetRuntime {
                    facet_id: facet.id.clone(),
                    data: FacetRuntimeData::default(),
                    lifecycle: FacetLifecycle::new(&facet.id),
                };
                self.setup_facet(facet, &mut record.data, record.lifecycle.clone())?;
                let previous_shape = self.with_runtimes(|state| {
                    state
                        .facets
                        .iter()
                        .find(|(id, _)| *id == facet.id)
                        .map(|(_, runtime)| {
                            (runtime.data.requires.clone(), runtime.data.provides.clone())
                        })
                });
                let mismatch = match previous_shape {
                    Some((requires, provides)) => !same_shape(&requires, &provides, &record.data),
                    None => true,
                };
                if mismatch {
                    return Err(ChordError::Type(format!(
                        "Reloaded facet {} must preserve its service requirements and provisions",
                        facet.id
                    )));
                }
                self.validate_replacement_provisions(&record.data)?;
                candidates.push(record);
            }
            Ok(candidates)
        })();
        let candidates = match setup_result {
            Ok(candidates) => candidates,
            Err(error) => {
                let cleanup_errors = dispose_facet_records(staged.iter().rev());
                if !cleanup_errors.is_empty() {
                    let abort_errors = self.abort(&[]);
                    let mut errors = vec![error];
                    errors.extend(cleanup_errors);
                    errors.extend(abort_errors);
                    return Err(ChordError::Aggregate {
                        message: "Facet reload setup and cleanup failed".to_owned(),
                        errors,
                    });
                }
                self.set_phase(GenerationPhase::Active);
                return Err(error);
            }
        };

        // Activate candidates in the established order.
        let order = self.with_runtimes(|state| state.activation_order.clone());
        let candidate_order: Vec<FacetRuntime> = order
            .iter()
            .filter_map(|id| {
                candidates
                    .iter()
                    .find(|candidate| candidate.facet_id == *id)
                    .cloned()
            })
            .collect();
        let activation_result: Result<(), ChordError> = (|| {
            for candidate in &candidate_order {
                candidate.lifecycle.activate()?;
            }
            for candidate in &candidate_order {
                self.validate_replacement_provisions(&candidate.data)?;
            }
            Ok(())
        })();
        if let Err(error) = activation_result {
            let cleanup_errors = dispose_facet_records(candidate_order.iter().rev());
            if !cleanup_errors.is_empty() {
                let abort_errors = self.abort(&[]);
                let mut errors = vec![error];
                errors.extend(cleanup_errors);
                errors.extend(abort_errors);
                return Err(ChordError::Aggregate {
                    message: "Facet reload activation and cleanup failed".to_owned(),
                    errors,
                });
            }
            self.set_phase(GenerationPhase::Active);
            return Err(error);
        }

        // Cutover: swap records, rebind singletons, retire the previous
        // generation, then reconnect keyed provisions.
        let previous: Vec<FacetRuntime> = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            candidate_order
                .iter()
                .filter_map(|candidate| {
                    let position = state
                        .facets
                        .iter()
                        .position(|(id, _)| *id == candidate.facet_id)?;
                    Some(state.facets.remove(position).1)
                })
                .collect()
        };
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            for candidate in &candidate_order {
                state
                    .facets
                    .push((candidate.facet_id.clone(), candidate.clone()));
            }
        }
        let cutover_result: Result<(), ChordError> = (|| {
            let provider = self.provider()?;
            for candidate in &candidate_order {
                for provision in &candidate.data.provisions {
                    if let FacetProvision::Singleton {
                        service,
                        implementation,
                        replace,
                        ..
                    } = provision
                    {
                        if service.local {
                            if let ProvidedImplementation::Local(any) = implementation {
                                self.slots
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .bind_singleton(&service.id, any.clone());
                            }
                        } else {
                            replace(&provider)?;
                        }
                    }
                }
            }
            let retirement_errors = dispose_facet_records(previous.iter().rev());
            match retirement_errors.len() {
                0 => {}
                1 => return Err(retirement_errors.into_iter().next().expect("non-empty")),
                _ => {
                    return Err(ChordError::Aggregate {
                        message: "Failed to retire replaced facets".to_owned(),
                        errors: retirement_errors,
                    })
                }
            }
            for candidate in &candidate_order {
                for provision in &candidate.data.provisions {
                    if let FacetProvision::Keyed {
                        service,
                        connect_local,
                        connect_remote,
                    } = provision
                    {
                        if service.local {
                            let registry = self
                                .local_keyed
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .clone()
                                .ok_or_else(|| {
                                    ChordError::Type(
                                        "Facet keyed services are not assembled".to_owned(),
                                    )
                                })?;
                            connect_local(&registry)?;
                        } else {
                            connect_remote(&provider)?;
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = cutover_result {
            let abort_errors = self.abort(&previous);
            let mut errors = vec![error];
            errors.extend(abort_errors);
            return Err(ChordError::Aggregate {
                message: "Facet reload failed after cutover".to_owned(),
                errors,
            });
        }
        self.set_phase(GenerationPhase::Active);
        Ok(())
    }

    /// `dispose()` (`facets/host.ts:513-519`).
    pub fn dispose(self: &Arc<Self>) -> Result<(), ChordError> {
        if self.phase() == GenerationPhase::Dead {
            return Ok(());
        }
        if self.phase() != GenerationPhase::Active {
            return Err(ChordError::Type(format!(
                "Facet host cannot be disposed while {}",
                self.phase().as_str()
            )));
        }
        let errors = self.terminate(&[]);
        match errors.len() {
            0 => Ok(()),
            1 => Err(errors.into_iter().next().expect("non-empty")),
            _ => Err(ChordError::Aggregate {
                message: "Failed to dispose facet generation".to_owned(),
                errors,
            }),
        }
    }

    /// `#validateReplacementProvisions` (`facets/host.ts:521-526`).
    fn validate_replacement_provisions(&self, data: &FacetRuntimeData) -> Result<(), ChordError> {
        let provider = self.provider()?;
        for provision in &data.provisions {
            if let FacetProvision::Singleton {
                service,
                validate_replacement,
                ..
            } = provision
            {
                if service.local {
                    continue;
                }
                validate_replacement(&provider)?;
            }
        }
        Ok(())
    }

    /// `#resolveExternalServices` (`facets/host.ts:596-653`).
    fn resolve_external_services(
        self: &Arc<Self>,
        records: &[FacetRuntime],
    ) -> Result<HashMap<String, ExternalService>, ChordError> {
        let mut offered: HashMap<String, (ServiceMode, usize)> = HashMap::new();
        for (index, source) in self.service_sources.iter().enumerate() {
            let entries = (source.catalogue)(&Context::background())?;
            for entry in entries {
                if offered.contains_key(&entry.service_id) {
                    return Err(ChordError::Type(format!(
                        "Facet host service {} is offered by more than one source",
                        entry.service_id
                    )));
                }
                offered.insert(entry.service_id, (entry.mode, index));
            }
        }
        let local: HashSet<String> = records
            .iter()
            .flat_map(|runtime| {
                runtime
                    .data
                    .provides
                    .iter()
                    .map(|reference| reference.service_id.clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut external: HashMap<String, ExternalService> = HashMap::new();
        for runtime in records {
            for requirement in &runtime.data.requires {
                if local.contains(&requirement.service_id)
                    || external.contains_key(&requirement.service_id)
                {
                    continue;
                }
                let resolved = match offered.get(&requirement.service_id) {
                    Some((mode, index)) => Some((*mode, *index)),
                    None => {
                        let deferred: Vec<usize> = self
                            .service_sources
                            .iter()
                            .enumerate()
                            .filter(|(_, source)| source.accepts_unavailable_services)
                            .map(|(index, _)| index)
                            .collect();
                        if deferred.len() > 1 {
                            return Err(ChordError::Type(format!(
                                "Facet host service {} has more than one deferred source",
                                requirement.service_id
                            )));
                        }
                        deferred.first().map(|index| (requirement.mode, *index))
                    }
                };
                if let Some((mode, index)) = resolved {
                    external.insert(
                        requirement.service_id.clone(),
                        ExternalService {
                            mode,
                            source_index: index,
                            service: requirement.service.clone(),
                        },
                    );
                }
            }
        }
        let mut by_source: HashMap<usize, Vec<String>> = HashMap::new();
        for (service_id, external_entry) in &external {
            by_source
                .entry(external_entry.source_index)
                .or_default()
                .push(service_id.clone());
        }
        for (index, service_ids) in by_source {
            let source = &self.service_sources[index];
            let services: Vec<Service> = service_ids
                .iter()
                .map(|id| Service {
                    id: id.clone(),
                    local: false,
                })
                .collect();
            let binding = (source.open)(ServiceSourceOpenOptions {
                services,
                assert_access: self.target_access_assert(),
                on_error: self.on_error.clone(),
            })?;
            self.source_bindings
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((index, binding));
        }
        Ok(external)
    }

    /// `#assembleProviders` (`facets/host.ts:655-685`).
    fn assemble_providers(self: &Arc<Self>) -> Result<(), ChordError> {
        let provisions = self.provisions();
        let remote: Vec<&FacetProvision> = provisions
            .iter()
            .filter(|provision| match provision {
                FacetProvision::Singleton { service, .. } => !service.local,
                FacetProvision::Keyed { service, .. } => !service.local,
            })
            .collect();
        let entries: Vec<ProviderEntry> = remote
            .iter()
            .map(|provision| match provision {
                FacetProvision::Singleton { service, .. } => {
                    ProviderEntry::from_service(service, ServiceMode::Singleton)
                }
                FacetProvision::Keyed { service, .. } => {
                    ProviderEntry::from_service(service, ServiceMode::Keyed)
                }
            })
            .collect();
        let provider = RemoteServiceProvider::new(&entries)?;
        let remote_services: Vec<Service> = remote
            .iter()
            .map(|provision| match provision {
                FacetProvision::Singleton { service, .. } => service.clone(),
                FacetProvision::Keyed { service, .. } => service.clone(),
            })
            .collect();
        let internal_services = create_remote_service_binding(RemoteServiceBindingOptions {
            services: remote_services,
            transport: create_loopback_service_transport(provider.clone()),
            assert_access: Some(self.target_access_assert()),
            on_error: Some(self.on_error.clone()),
            bound: None,
        })?;
        let local_keyed_services: Vec<Service> = provisions
            .iter()
            .filter_map(|provision| match provision {
                FacetProvision::Keyed { service, .. } if service.local => Some(service.clone()),
                _ => None,
            })
            .collect();
        let local_keyed =
            LocalKeyedServiceRegistry::new(&local_keyed_services, self.on_error.clone())?;
        *self.provider.lock().unwrap_or_else(|p| p.into_inner()) = Some(provider.clone());
        *self
            .internal_services
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(internal_services.clone());
        *self.local_keyed.lock().unwrap_or_else(|p| p.into_inner()) = Some(local_keyed.clone());
        for provision in &provisions {
            match provision {
                FacetProvision::Singleton {
                    service, install, ..
                } => {
                    if !service.local {
                        install(&provider)?;
                    }
                }
                FacetProvision::Keyed {
                    service,
                    connect_local,
                    connect_remote,
                } => {
                    if service.local {
                        connect_local(&local_keyed)?;
                    } else {
                        connect_remote(&provider)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `#bindServices` (`facets/host.ts:687-711`).
    fn bind_services(
        self: &Arc<Self>,
        external_services: &HashMap<String, ExternalService>,
    ) -> Result<(), ChordError> {
        let provisions = self.provisions();
        let internal = self
            .internal_services
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| {
                ChordError::Type("Facet remote services are not assembled".to_owned())
            })?;
        let local_registry = self
            .local_keyed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| ChordError::Type("Facet keyed services are not assembled".to_owned()))?;
        for provision in &provisions {
            match provision {
                FacetProvision::Singleton {
                    service,
                    implementation,
                    ..
                } => {
                    if !self
                        .slots
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .has_singleton(&service.id)
                    {
                        continue;
                    }
                    if service.local {
                        if let ProvidedImplementation::Local(any) = implementation {
                            self.slots
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .bind_singleton(&service.id, any.clone());
                        }
                    } else {
                        let handle = internal.use_service(service)?;
                        self.slots
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .bind_singleton_handle(&service.id, &handle);
                    }
                }
                FacetProvision::Keyed { service, .. } => {
                    if service.local {
                        self.slots
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .bind_keyed(&service.id, local_registry.clone());
                    } else {
                        self.slots
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .bind_keyed(&service.id, internal.clone());
                    }
                }
            }
        }
        for (service_id, external) in external_services {
            let services = {
                let bindings = self
                    .source_bindings
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                let Some((_, services)) = bindings
                    .iter()
                    .find(|(index, _)| *index == external.source_index)
                else {
                    return Err(ChordError::Type(format!(
                        "Service source for {service_id} is not open"
                    )));
                };
                services.clone()
            };
            if external.mode == ServiceMode::Singleton {
                let handle = services.use_service(&external.service)?;
                self.slots
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .bind_singleton_handle(service_id, &handle);
            } else {
                self.slots
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .bind_keyed(
                        service_id,
                        Arc::new(SourceKeyedAdapter {
                            binding: services.clone(),
                        }),
                    );
            }
        }
        Ok(())
    }

    /// `#provisions()` (`facets/host.ts:713-715`).
    fn provisions(&self) -> Vec<FacetProvision> {
        self.with_runtimes(|state| {
            state
                .facets
                .iter()
                .flat_map(|(_, runtime)| runtime.data.provisions.iter().cloned())
                .collect()
        })
    }

    /// `#disposeServiceBindings` (`facets/host.ts:727-734`).
    fn dispose_service_bindings(&self) -> Vec<ChordError> {
        let mut bindings: Vec<Arc<dyn ServiceSourceBinding>> = self
            .source_bindings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
            .map(|(_, binding)| binding)
            .collect();
        if let Some(internal) = self
            .internal_services
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            bindings.push(BindingSource::new(internal));
        }
        let mut errors = Vec::new();
        for binding in bindings {
            if let Err(error) = binding.dispose(&Context::background()) {
                errors.push(error);
            }
        }
        errors
    }

    /// `#abort(extraRecords)` (`facets/host.ts:747-751`).
    fn abort(self: &Arc<Self>, extra_records: &[FacetRuntime]) -> Vec<ChordError> {
        self.with_runtimes(|state| {
            for (_, runtime) in &state.facets {
                runtime.lifecycle.revoke();
            }
        });
        for record in extra_records {
            record.lifecycle.revoke();
        }
        self.terminate(extra_records)
    }

    /// `#terminate(extraRecords)` (`facets/host.ts:753-776`).
    fn terminate(self: &Arc<Self>, extra_records: &[FacetRuntime]) -> Vec<ChordError> {
        self.set_phase(GenerationPhase::Disposing);
        let mut errors = self.dispose_lifecycles();
        errors.extend(dispose_facet_records(extra_records.iter().rev()));
        if let Some(local_keyed) = self
            .local_keyed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            local_keyed.dispose();
        }
        errors.extend(self.dispose_service_bindings());
        self.slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .dispose();
        if let Some(provider) = self
            .provider
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            if let Err(error) = provider.dispose() {
                errors.push(error);
            }
        }
        self.set_phase(GenerationPhase::Dead);
        errors
    }

    /// `#disposeLifecycles` (`facets/host.ts:778-793`).
    fn dispose_lifecycles(&self) -> Vec<ChordError> {
        let mut errors: Vec<ChordError> = Vec::new();
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let order: Vec<String> = if !state.activation_order.is_empty() {
            state.activation_order.iter().rev().cloned().collect()
        } else {
            state
                .facets
                .iter()
                .map(|(id, _)| id.clone())
                .rev()
                .collect()
        };
        for id in order {
            let Some(position) = state
                .facets
                .iter()
                .position(|(facet_id, _)| *facet_id == id)
            else {
                continue;
            };
            let (_, runtime) = state.facets.remove(position);
            if let Err(error) = runtime.lifecycle.dispose() {
                errors.push(error);
            }
        }
        errors
    }
}

#[derive(Clone, Debug)]
struct ExternalService {
    mode: ServiceMode,
    source_index: usize,
    service: Service,
}

/// `sameFacetShape` (`facets/host.ts:891-902`).
fn same_shape(
    requires: &[FacetServiceReference],
    provides: &[FacetServiceReference],
    right: &FacetRuntimeData,
) -> bool {
    same_references(requires, &right.requires) && same_references(provides, &right.provides)
}

fn same_references(left: &[FacetServiceReference], right: &[FacetServiceReference]) -> bool {
    left.len() == right.len()
        && left.iter().all(|reference| {
            right.iter().any(|other| {
                other.service_id == reference.service_id && other.mode == reference.mode
            })
        })
}

/// `disposeFacetRecords(records)` (`facets/host.ts:796-806`).
fn dispose_facet_records<'a, I>(records: I) -> Vec<ChordError>
where
    I: Iterator<Item = &'a FacetRuntime>,
{
    let mut errors: Vec<ChordError> = Vec::new();
    for record in records {
        if let Err(error) = record.lifecycle.dispose() {
            errors.push(error);
        }
    }
    errors
}

/// `validateFacets(records, externalServices)` (`facets/host.ts:808-856`).
fn validate_facets(
    records: &[FacetRuntime],
    external_services: &HashMap<String, ExternalService>,
) -> Result<Vec<String>, ChordError> {
    #[derive(Debug, Clone)]
    struct ProviderInfo {
        facet_id: Option<String>,
        mode: Option<ServiceMode>,
    }
    let mut providers: HashMap<String, ProviderInfo> = HashMap::new();
    for (service_id, external) in external_services {
        providers.insert(
            service_id.clone(),
            ProviderInfo {
                facet_id: None,
                mode: Some(external.mode),
            },
        );
    }
    for runtime in records {
        for provision in &runtime.data.provides {
            let existing = providers.get(&provision.service_id).cloned();
            if let Some(existing) = existing {
                if let Some(mode) = existing.mode {
                    if mode != provision.mode {
                        return Err(ChordError::Type(format!(
                            "Service {} is provided as both singleton and keyed",
                            provision.service_id
                        )));
                    }
                }
                if existing.facet_id.is_none() {
                    return Err(ChordError::Type(format!(
                        "Service {} is provided by both the host and {}",
                        provision.service_id, runtime.facet_id
                    )));
                }
                return Err(ChordError::Type(format!(
                    "Service {} is provided by both {} and {}",
                    provision.service_id,
                    existing.facet_id.expect("checked above"),
                    runtime.facet_id
                )));
            }
            providers.insert(
                provision.service_id.clone(),
                ProviderInfo {
                    facet_id: Some(runtime.facet_id.clone()),
                    mode: Some(provision.mode),
                },
            );
        }
    }
    let mut dependencies: HashMap<String, HashSet<String>> = records
        .iter()
        .map(|r| (r.facet_id.clone(), HashSet::new()))
        .collect();
    let mut dependents: HashMap<String, HashSet<String>> = records
        .iter()
        .map(|r| (r.facet_id.clone(), HashSet::new()))
        .collect();
    for runtime in records {
        for requirement in &runtime.data.requires {
            let provider = providers.get(&requirement.service_id).ok_or_else(|| {
                ChordError::Type(format!(
                    "Facet {} requires local/{}/{}, but no facet provides it",
                    runtime.facet_id,
                    requirement.service_id,
                    requirement.mode.as_str()
                ))
            })?;
            if let Some(mode) = provider.mode {
                if mode != requirement.mode {
                    return Err(ChordError::Type(format!(
                        "Facet {} requires {} as {}, but {} provides it as {}",
                        runtime.facet_id,
                        requirement.service_id,
                        requirement.mode.as_str(),
                        provider.facet_id.as_deref().unwrap_or("the host"),
                        mode.as_str()
                    )));
                }
            }
            match &provider.facet_id {
                None => {}
                Some(provider_id) if provider_id == &runtime.facet_id => {}
                Some(provider_id) => {
                    dependencies
                        .get_mut(&runtime.facet_id)
                        .expect("initialized above")
                        .insert(provider_id.clone());
                    dependents
                        .get_mut(provider_id)
                        .expect("initialized above")
                        .insert(runtime.facet_id.clone());
                }
            }
        }
    }
    topological_order(records, &dependencies, &dependents)
}

/// `topologicalOrder(...)` (`facets/host.ts:858-880`).
fn topological_order(
    records: &[FacetRuntime],
    dependencies: &HashMap<String, HashSet<String>>,
    dependents: &HashMap<String, HashSet<String>>,
) -> Result<Vec<String>, ChordError> {
    let mut remaining: HashMap<String, usize> = dependencies
        .iter()
        .map(|(id, values)| (id.clone(), values.len()))
        .collect();
    let mut ready: Vec<String> = records
        .iter()
        .map(|record| record.facet_id.clone())
        .filter(|id| remaining.get(id).copied().unwrap_or(0) == 0)
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    while cursor < ready.len() {
        let id = ready[cursor].clone();
        cursor += 1;
        order.push(id.clone());
        if let Some(dependent_ids) = dependents.get(&id) {
            let mut promoted: Vec<String> = Vec::new();
            for dependent in dependent_ids {
                let count = remaining.get_mut(dependent).expect("initialized above");
                *count -= 1;
                if *count == 0 {
                    promoted.push(dependent.clone());
                }
            }
            ready.extend(promoted);
        }
    }
    if order.len() != records.len() {
        let cycle: Vec<String> = records
            .iter()
            .map(|record| record.facet_id.clone())
            .filter(|id| remaining.get(id).copied().unwrap_or(0) > 0)
            .collect();
        return Err(ChordError::Type(format!(
            "Facet dependency cycle: {}",
            cycle.join(", ")
        )));
    }
    Ok(order)
}

// ── adapters ────────────────────────────────────────────────────────────────

/// Adapter exposing a [`ServiceSourceBinding`] through [`KeyedServiceSource`]
/// for external keyed sources.
struct SourceKeyedAdapter {
    binding: Arc<dyn ServiceSourceBinding>,
}

impl KeyedServiceSource for SourceKeyedAdapter {
    fn observe_keyed(
        self: Arc<Self>,
        service: &Service,
        handler: crate::chord::consumer::TargetHandler,
    ) -> Result<ObservationStop, ChordError> {
        self.binding.observe(
            service,
            Arc::new(move |handle, context| handler(&handle.target, context)),
        )
    }
}

// ── host and loaders (`api.ts` + `facets/loader.ts`) ────────────────────────

/// `createFacetHost(options)` (`api.ts:19-27`): one active host for one
/// complete set of facets.
pub fn create_facet_host(options: FacetOptions) -> Result<FacetHost, ChordError> {
    let kernel = Arc::new(FacetKernel::new(options)?);
    kernel.activate()?;
    let services = kernel.provider()?;
    Ok(FacetHost {
        services,
        kernel: Mutex::new(Some(kernel)),
    })
}

/// The frozen host surface returned by [`create_facet_host`] (upstream
/// `FacetHost`).
pub struct FacetHost {
    /// Upstream `host.services` (`kernel.provider`).
    pub services: Arc<RemoteServiceProvider>,
    kernel: Mutex<Option<Arc<FacetKernel>>>,
}

impl std::fmt::Debug for FacetHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FacetHost").finish()
    }
}

impl FacetHost {
    /// `host.reload(facets)` (`api.ts:24`).
    pub fn reload(&self, facets: Vec<Facet>) -> Result<(), ChordError> {
        let kernel = self
            .kernel
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let Some(kernel) = kernel else {
            return Err(ChordError::Type(format!(
                "Facet host cannot reload while {}",
                GenerationPhase::Dead.as_str()
            )));
        };
        kernel.reload(&facets)
    }

    /// `host.dispose()` (`api.ts:25`).
    pub fn dispose(&self) -> Result<(), ChordError> {
        let kernel = self.kernel.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(kernel) = kernel else {
            return Ok(());
        };
        kernel.dispose()
    }
}

/// Upstream `LoadedFacets` (`types.ts`).
pub struct LoadedFacets {
    pub facets: Vec<Facet>,
    dispose: Option<Box<dyn FnOnce() -> Result<(), ChordError> + Send>>,
}

impl LoadedFacets {
    pub fn new(
        facets: Vec<Facet>,
        dispose: Box<dyn FnOnce() -> Result<(), ChordError> + Send>,
    ) -> Self {
        LoadedFacets {
            facets,
            dispose: Some(dispose),
        }
    }

    pub fn dispose(&mut self) -> Result<(), ChordError> {
        match self.dispose.take() {
            Some(dispose) => dispose(),
            None => Ok(()),
        }
    }
}

/// Upstream `FacetLoader` (`types.ts`).
pub trait FacetLoader: Send + Sync {
    fn load(&self) -> Result<LoadedFacets, ChordError>;
}

/// `createStaticFacetLoader(facets)` (`api.ts:29-36`).
pub fn create_static_facet_loader(facets: Vec<Facet>) -> StaticFacetLoader {
    StaticFacetLoader { facets }
}

pub struct StaticFacetLoader {
    facets: Vec<Facet>,
}

impl FacetLoader for StaticFacetLoader {
    fn load(&self) -> Result<LoadedFacets, ChordError> {
        Ok(LoadedFacets {
            facets: self.facets.clone(),
            dispose: Some(Box::new(|| Ok(()))),
        })
    }
}

/// `combineFacetLoaders(loaders)` (`api.ts:38-64`).
pub fn combine_facet_loaders(loaders: Vec<Arc<dyn FacetLoader>>) -> CombinedFacetLoader {
    CombinedFacetLoader { loaders }
}

pub struct CombinedFacetLoader {
    loaders: Vec<Arc<dyn FacetLoader>>,
}

impl FacetLoader for CombinedFacetLoader {
    fn load(&self) -> Result<LoadedFacets, ChordError> {
        let mut loaded: Vec<LoadedFacets> = Vec::new();
        for loader in &self.loaders {
            match loader.load() {
                Ok(loaded_facets) => loaded.push(loaded_facets),
                Err(error) => {
                    // `disposeLoadedFacets(loaded.reverse())`
                    // (`facets/loader.ts:3-5`).
                    let cleanup_errors = dispose_loaded_facets(loaded.drain(..).rev().collect());
                    if !cleanup_errors.is_empty() {
                        let mut errors = vec![error];
                        errors.extend(cleanup_errors);
                        return Err(ChordError::Aggregate {
                            message: "Facet loading and cleanup failed".to_owned(),
                            errors,
                        });
                    }
                    return Err(error);
                }
            }
        }
        let facets: Vec<Facet> = loaded
            .iter()
            .flat_map(|entry| entry.facets.iter().cloned())
            .collect();
        let disposals: Vec<Box<dyn FnOnce() -> Result<(), ChordError> + Send>> = loaded
            .into_iter()
            .rev()
            .filter_map(|entry| entry.dispose)
            .collect();
        Ok(LoadedFacets {
            facets,
            dispose: Some(Box::new(move || {
                let mut errors: Vec<ChordError> = Vec::new();
                for dispose in disposals {
                    if let Err(error) = dispose() {
                        errors.push(error);
                    }
                }
                match errors.len() {
                    0 => Ok(()),
                    1 => Err(errors.into_iter().next().expect("non-empty")),
                    _ => Err(ChordError::Aggregate {
                        message: "Failed to dispose loaded facets".to_owned(),
                        errors,
                    }),
                }
            })),
        })
    }
}

/// `disposeLoadedFacets(loaded)` (`facets/loader.ts:3-6`).
pub fn dispose_loaded_facets(loaded: Vec<LoadedFacets>) -> Vec<ChordError> {
    let mut errors: Vec<ChordError> = Vec::new();
    for mut entry in loaded {
        if let Err(error) = entry.dispose() {
            errors.push(error);
        }
    }
    errors
}
