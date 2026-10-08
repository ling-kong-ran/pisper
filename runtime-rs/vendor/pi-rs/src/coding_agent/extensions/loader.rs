//! Port of upstream `coding-agent/src/core/extensions/loader.ts` (extension
//! loader; upstream SHA256 at migration time:
//! `753576eabfcdad31d6ef502ea6b1ac663879abedc866651fe0be83a69ffc14a5`).
//!
//! Port notes (disclosed):
//! - **jiti module loading seam**: upstream loads TS/JS extension modules
//!   through jiti with virtual modules / aliases. Rust has no runtime TS
//!   module system, so the "resolve path → default export factory" step is
//!   the [`ExtensionModuleLoader`] trait. The factory cache
//!   ([`clear_extension_cache`], cwd/generation tokening) is ported verbatim
//!   over that seam. The jiti-owned "Cannot find module" text for missing
//!   modules is not portable; everything after resolution (non-factory
//!   exports, factory throws, registration validation, error wrapping) is
//!   oracle-pinned.
//! - `time(...)` performance marks (timings.ts) are telemetry only and are
//!   dropped.
//! - `exec` on the API delegates to the vendored [`exec_command`], a direct
//!   port of `core/exec.ts` (spawn without shell, stdout/stderr capture,
//!   timeout/abort kill, spawn failure → `code: 1`). Upstream escalates
//!   SIGTERM → SIGKILL after 5s; the port kills outright (Windows has no
//!   SIGTERM discipline). Post-exit pipe drains are bounded by a short
//!   timeout instead of waiting out detached descendants.
//! - Factories and actions are synchronous closures (async dropped, as in the
//!   `event_bus` port).
//! - `readPiManifest` (core/pi-manifest.ts, outside the W3.3 slice) is
//!   vendored here with `stripBom` from the ported text utils.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use super::types::{
    CommandHandler, Extension, ExtensionFlag, ExtensionLoadError, FlagType, FlagValue, HandlerFn,
    LoadExtensionsResult, OrderedMap, PendingNativeProviderRegistration,
    PendingProviderRegistration, PendingVirtualModelRegistration, RegisterNativeProviderHandler,
    RegisterProviderHandler, RegisteredCommand, RegisteredTool, SendMessageOptions,
    SendUserMessageOptions, SourceInfo, ThinkingLevel, ToolDefinition, UnregisterProviderHandler,
};
use crate::coding_agent::core::event_bus::{EventBus, EventBusController, EventBusUnsubscribe};
use crate::coding_agent::core::mcp_servers::{
    mcp_namespace, validate_mcp_server_config, McpExposure, McpServerConfig, RegisteredMcpServer,
};
use crate::coding_agent::core::CONFIG_DIR_NAME;
use crate::coding_agent::utils::node_path;
use crate::coding_agent::utils::paths::{
    resolve_path_auto_base, resolve_path_with, PathInputOptions,
};

/// `McpExposure::as_str` (the core slice keeps its method private; the JSON
/// strings are pinned by the core slice's own tests).
fn mcp_exposure_as_str(exposure: McpExposure) -> &'static str {
    match exposure {
        McpExposure::Codemode => "codemode",
        McpExposure::Deferred => "deferred",
        McpExposure::Direct => "direct",
        McpExposure::Hidden => "hidden",
    }
}

/// A string field of a virtual-model definition JSON (absent → empty string,
/// which never matches a real provider/id in the unregister filter).
fn json_definition_field(definition: &Value, field: &str) -> String {
    definition
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The JSON-at-the-seam view of one validated server entry: the raw object
/// with exposure aliases resolved (upstream `validateMcpServerConfig`
/// returns exactly that object; `McpServerConfig` keeps it private, so the
/// two resolved slots are re-applied from the validated config's getters).
pub(crate) fn resolved_mcp_config_value(
    raw: &crate::ai::types::ordered_map::OrderedMap<Value>,
    validated: &McpServerConfig,
) -> Value {
    let mut resolved = raw.clone();
    if resolved.get("exposure").is_some() {
        resolved.insert(
            "exposure",
            Value::String(mcp_exposure_as_str(validated.exposure()).to_string()),
        );
    }
    if resolved.get("toolExposure").is_some() {
        let tool_exposure = validated.tool_exposure();
        let mut object = serde_json::Map::new();
        for (tool, exposure) in tool_exposure.iter() {
            object.insert(
                tool.clone(),
                Value::String(mcp_exposure_as_str(*exposure).to_string()),
            );
        }
        resolved.insert("toolExposure", Value::Object(object));
    }
    Value::Object(
        resolved
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

// ============================================================================
// Factory cache (upstream module-scope cache, ported verbatim)
// ============================================================================

/// Upstream `ExtensionFactory`: the default export of an extension module.
/// Sync closure (async dropped); a thrown `Error` is `Err(message)`.
pub type ExtensionFactory = Arc<dyn Fn(&ExtensionApi) -> Result<(), String> + Send + Sync>;

struct FactoryCacheState {
    cache: HashMap<String, ExtensionFactory>,
    cwd: Option<String>,
    generation: usize,
}

fn factory_cache() -> &'static Mutex<FactoryCacheState> {
    static CACHE: OnceLock<Mutex<FactoryCacheState>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(FactoryCacheState {
            cache: HashMap::new(),
            cwd: None,
            generation: 0,
        })
    })
}

/// Upstream `clearExtensionCache()`.
pub fn clear_extension_cache() {
    let mut state = factory_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.cache.clear();
    state.cwd = None;
    state.generation += 1;
}

/// Upstream `ExtensionCacheToken`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionCacheToken {
    cwd: String,
    generation: usize,
}

fn use_extension_cache_cwd(cwd: &str) -> ExtensionCacheToken {
    let resolved_cwd = resolve_path_with(cwd, "", &PathInputOptions::default(), cfg!(windows))
        .unwrap_or_else(|_| cwd.to_string());
    let mut state = factory_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state
        .cwd
        .as_deref()
        .is_some_and(|current| current != resolved_cwd)
    {
        state.cache.clear();
        state.generation += 1;
    }
    state.cwd = Some(resolved_cwd.clone());
    ExtensionCacheToken {
        cwd: resolved_cwd,
        generation: state.generation,
    }
}

fn is_current_cache_token(cache_token: &Option<ExtensionCacheToken>) -> bool {
    let Some(token) = cache_token else {
        return false;
    };
    let state = factory_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.cwd.as_deref() == Some(token.cwd.as_str()) && state.generation == token.generation
}

// ============================================================================
// Module loading seam
// ============================================================================

/// The jiti seam: resolve an extension path to its default-export factory.
/// `Ok(None)` = module without a valid factory export (upstream
/// `typeof factory !== "function"`); `Err` = import failure.
pub trait ExtensionModuleLoader: Send + Sync {
    fn load(&self, resolved_path: &str) -> Result<Option<ExtensionFactory>, String>;
}

/// Loader that can resolve nothing (for callers that only use explicit
/// factories). Missing modules are a jiti-owned diagnostic upstream.
pub struct NullModuleLoader;

impl ExtensionModuleLoader for NullModuleLoader {
    fn load(&self, _resolved_path: &str) -> Result<Option<ExtensionFactory>, String> {
        Err(
            "Cannot find module (no module loader installed; see the extensions module docs)"
                .to_string(),
        )
    }
}

fn load_extension_module(
    loader: &dyn ExtensionModuleLoader,
    resolved_path: &str,
    cache_token: &Option<ExtensionCacheToken>,
) -> Result<Option<ExtensionFactory>, String> {
    if is_current_cache_token(cache_token) {
        let state = factory_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cached) = state.cache.get(resolved_path) {
            return Ok(Some(Arc::clone(cached)));
        }
    }

    let factory = loader.load(resolved_path)?;
    if let (Some(factory), true) = (&factory, is_current_cache_token(cache_token)) {
        let mut state = factory_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .cache
            .insert(resolved_path.to_string(), Arc::clone(factory));
    }
    Ok(factory)
}

// ============================================================================
// Extension runtime (upstream createExtensionRuntime)
// ============================================================================

/// Post-bind provider routing installed by
/// [`super::runner::ExtensionRunner::bind_core`] (upstream replaces the
/// runtime's register/unregister closures once the model registry exists).
#[derive(Clone)]
pub(crate) struct ProviderRouting {
    pub register_provider: RegisterProviderHandler,
    pub register_native_provider: RegisterNativeProviderHandler,
    pub unregister_provider: UnregisterProviderHandler,
}

/// Post-bind virtual-model routing installed by `bind_core` (upstream
/// `runtime.registerVirtualModel` / `runtime.unregisterVirtualModel`):
/// provider actions when provided, else the model registry.
#[derive(Clone)]
pub(crate) struct VirtualModelRouting {
    pub register: super::types::RegisterVirtualModelHandler,
    pub unregister: super::types::UnregisterVirtualModelHandler,
}

/// The runner-side reaction to MCP registry changes, installed by `bind_core`
/// (upstream `runtime.mcpServers.setChangeListener(() => { void
/// this.emit(...); this.reportUnhandledMcpServers(); })`). The ported
/// registry's own listener hook cannot re-enter the locked runtime state, so
/// mutations run through [`ExtensionRuntime`] helpers that fire this sink
/// after the state lock is released — same observable behavior, different
/// mechanism (disclosed seam).
pub(crate) struct McpChangeSink {
    pub runner: super::runner::ExtensionRunner,
}

impl McpChangeSink {
    /// Upstream listener body: fire-and-forget `mcp_servers_change` emission
    /// with the full post-change server list, then the unhandled report.
    pub fn fire(&self) {
        let mut event = {
            let runtime = self.runner.runtime();
            let servers = runtime.mcp_servers_list();
            let payloads = runtime.mcp_server_payloads();
            serde_json::json!({
                "type": "mcp_servers_change",
                "servers": registered_mcp_servers_value(&servers, &payloads),
            })
        };
        self.runner.emit_detached(event);
        event = Value::Null;
        let _ = event;
        self.runner.report_unhandled_mcp_servers();
    }
}

/// `RegisteredMcpServer[]` as JSON at the seam (`{ name, config,
/// extensionPath }`); `config` is the validated entry with exposure aliases
/// resolved, snapshotted at registration time.
pub(crate) fn registered_mcp_servers_value(
    servers: &[RegisteredMcpServer],
    payloads: &OrderedMap<Value>,
) -> Value {
    Value::Array(
        servers
            .iter()
            .map(|server| {
                serde_json::json!({
                    "name": server.name,
                    "config": payloads.get(&server.name).cloned().unwrap_or(Value::Null),
                    "extensionPath": server.extension_path,
                })
            })
            .collect(),
    )
}

/// The JS pending array has reference identity: an active for-of observes
/// appended entries, but keeps the old array when unregister assigns a filter.
/// No callback runs while either this array or the runtime state is locked.
#[derive(Clone)]
pub(crate) struct PendingRegistrations<T>(Arc<Mutex<Vec<T>>>);

impl<T> Default for PendingRegistrations<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}

impl<T: Clone> PendingRegistrations<T> {
    fn push(&self, value: T) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(value);
    }

    fn snapshot(&self) -> Vec<T> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn get(&self, index: usize) -> Option<T> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(index)
            .cloned()
    }

    fn filtered(&self, mut keep: impl FnMut(&T) -> bool) -> Self {
        let values = self
            .snapshot()
            .into_iter()
            .filter(|value| keep(value))
            .collect();
        Self(Arc::new(Mutex::new(values)))
    }
}

pub(crate) struct RuntimeState {
    pub flag_values: OrderedMap<FlagValue>,
    pub pending_provider_registrations: PendingRegistrations<PendingProviderRegistration>,
    pub pending_native_provider_registrations:
        PendingRegistrations<PendingNativeProviderRegistration>,
    /// Upstream `ExtensionRuntimeState.mcpServers`: servers registered with
    /// `pi.registerMcpServer()`.
    pub mcp_servers: RegisteredMcpServers,
    /// Upstream `pendingVirtualModelRegistrations`.
    pub pending_virtual_model_registrations: PendingRegistrations<PendingVirtualModelRegistration>,
    /// `None` until `bindCore` (upstream throwing stubs).
    pub actions: Option<Arc<super::types::ExtensionActions>>,
    /// Post-bind provider routing (upstream replaced closures).
    pub provider_routing: Option<ProviderRouting>,
    /// Post-bind virtual-model routing (upstream replaced closures).
    pub virtual_model_routing: Option<VirtualModelRouting>,
    /// Upstream `runtime.createContext` (a throwing stub until `bindCore`).
    pub create_context:
        Option<Arc<dyn Fn() -> Result<super::types::ExtensionContext, String> + Send + Sync>>,
    /// Upstream MCP change listener (`setChangeListener` at bind time); fires
    /// after the state lock is released (see [`McpChangeSink`]).
    pub mcp_change_sink: Option<Arc<McpChangeSink>>,
}

/// The runtime's MCP server registrations plus their JSON-at-the-seam
/// snapshots for event payloads. `McpServerConfig` keeps its validated form
/// private (core slice), so the payload serialization (the raw entry with
/// exposure aliases resolved) is captured at registration time — the two
/// maps are only ever mutated together under the state lock.
#[derive(Default)]
pub(crate) struct RegisteredMcpServers {
    pub registry: crate::coding_agent::core::mcp_servers::McpServerRegistry,
    pub payloads: OrderedMap<Value>,
}

struct TrackedEntry {
    slot: Arc<Mutex<Option<EventBusUnsubscribe>>>,
}

impl TrackedEntry {
    fn unsubscribe(&self) {
        if let Some(handle) = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            handle.unsubscribe();
        }
    }
}

pub(crate) struct RuntimeInner {
    state: Mutex<RuntimeState>,
    stale: Mutex<Option<String>>,
    event_bus_unsubscribers: Mutex<Vec<TrackedEntry>>,
}

/// Upstream `ExtensionRuntime`: shared state created by the loader with
/// throwing action stubs, completed by `bindCore`. Clones share state.
#[derive(Clone)]
pub struct ExtensionRuntime {
    pub(crate) inner: Arc<RuntimeInner>,
}

/// Upstream throwing-stub message.
pub const RUNTIME_NOT_INITIALIZED: &str =
    "Extension runtime not initialized. Action methods cannot be called during extension loading.";
/// Upstream pre-bind `setModel` rejection message.
pub const RUNTIME_SET_MODEL_NOT_INITIALIZED: &str = "Extension runtime not initialized";

/// Upstream default `invalidate()` message.
pub const DEFAULT_STALE_MESSAGE: &str = "This extension ctx is stale after session replacement or reload. Do not use a captured pi or command ctx after ctx.newSession(), ctx.fork(), ctx.switchSession(), or ctx.reload(). For newSession, fork, and switchSession, move post-replacement work into withSession and use the ctx passed to withSession. For reload, do not use the old ctx after await ctx.reload().";

impl ExtensionRuntime {
    /// Upstream `createExtensionRuntime()`.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                state: Mutex::new(RuntimeState {
                    flag_values: OrderedMap::new(),
                    pending_provider_registrations: PendingRegistrations::default(),
                    pending_native_provider_registrations: PendingRegistrations::default(),
                    mcp_servers: RegisteredMcpServers::default(),
                    pending_virtual_model_registrations: PendingRegistrations::default(),
                    actions: None,
                    provider_routing: None,
                    virtual_model_routing: None,
                    create_context: None,
                    mcp_change_sink: None,
                }),
                stale: Mutex::new(None),
                event_bus_unsubscribers: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn flag_values(&self) -> OrderedMap<FlagValue> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flag_values
            .clone()
    }

    pub fn flag_value(&self, name: &str) -> Option<FlagValue> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flag_values
            .get(name)
            .cloned()
    }

    pub fn set_flag_value(&self, name: impl Into<String>, value: FlagValue) {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flag_values
            .set(name, value);
    }

    pub fn pending_provider_registrations(&self) -> Vec<PendingProviderRegistration> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_provider_registrations
            .snapshot()
    }

    pub fn pending_native_provider_registrations(&self) -> Vec<PendingNativeProviderRegistration> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_native_provider_registrations
            .snapshot()
    }

    /// Upstream `runtime.pendingVirtualModelRegistrations` snapshot.
    pub fn pending_virtual_model_registrations(&self) -> Vec<PendingVirtualModelRegistration> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_virtual_model_registrations
            .snapshot()
    }

    /// Upstream for-of followed by property replacement, not an early drain.
    pub(crate) fn flush_pending_providers(
        &self,
        mut register: impl FnMut(PendingProviderRegistration),
    ) {
        let pending = self.with_state(|state| state.pending_provider_registrations.clone());
        let mut index = 0;
        while let Some(registration) = pending.get(index) {
            index += 1;
            register(registration);
        }
        self.with_state(|state| {
            state.pending_provider_registrations = PendingRegistrations::default()
        });
    }

    pub(crate) fn flush_pending_native_providers(
        &self,
        mut register: impl FnMut(PendingNativeProviderRegistration),
    ) {
        let pending = self.with_state(|state| state.pending_native_provider_registrations.clone());
        let mut index = 0;
        while let Some(registration) = pending.get(index) {
            index += 1;
            register(registration);
        }
        self.with_state(|state| {
            state.pending_native_provider_registrations = PendingRegistrations::default()
        });
    }

    pub(crate) fn with_state<T>(&self, f: impl FnOnce(&mut RuntimeState) -> T) -> T {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut state)
    }

    /// Upstream `assertActive()`.
    pub fn assert_active(&self) -> Result<(), String> {
        match self
            .inner
            .stale
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            Some(message) if !message.is_empty() => Err(message),
            _ => Ok(()),
        }
    }

    /// Upstream `invalidate(message?)` — first nonempty message wins; unsubscribes
    /// tracked event-bus subscriptions.
    pub fn invalidate(&self, message: Option<&str>) {
        {
            let mut stale = self
                .inner
                .stale
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if stale.as_ref().is_some_and(|message| !message.is_empty()) {
                return;
            }
            *stale = Some(message.unwrap_or(DEFAULT_STALE_MESSAGE).to_string());
        }
        let unsubs = self
            .inner
            .event_bus_unsubscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect::<Vec<_>>();
        for entry in unsubs {
            entry.unsubscribe();
        }
    }

    /// Upstream `trackEventBusSubscription(unsubscribe)`: retain the
    /// subscription until this runtime is invalidated.
    pub fn track_event_bus_subscription(
        &self,
        handle: EventBusUnsubscribe,
    ) -> TrackedEventBusUnsubscribe {
        let slot = Arc::new(Mutex::new(Some(handle)));
        self.inner
            .event_bus_unsubscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(TrackedEntry {
                slot: Arc::clone(&slot),
            });
        TrackedEventBusUnsubscribe { slot }
    }

    fn actions(&self) -> Result<Arc<super::types::ExtensionActions>, String> {
        self.assert_active()?;
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .actions
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(|| RUNTIME_NOT_INITIALIZED.to_string())
    }

    /// Upstream `refreshTools()` — valid during extension load (no-op
    /// pre-bind).
    pub fn refresh_tools(&self) {
        if let Ok(actions) = self.actions() {
            (actions.refresh_tools)();
        }
    }

    pub fn send_message(
        &self,
        message: &Value,
        options: &SendMessageOptions,
    ) -> Result<(), String> {
        let actions = self.actions()?;
        (actions.send_message)(message, options);
        Ok(())
    }

    pub fn send_user_message(
        &self,
        content: &Value,
        options: &SendUserMessageOptions,
    ) -> Result<(), String> {
        let actions = self.actions()?;
        (actions.send_user_message)(content, options);
        Ok(())
    }

    pub fn append_entry(&self, custom_type: &str, data: Option<&Value>) -> Result<(), String> {
        let actions = self.actions()?;
        (actions.append_entry)(custom_type, data);
        Ok(())
    }

    pub fn set_session_name(&self, name: &str) -> Result<(), String> {
        let actions = self.actions()?;
        (actions.set_session_name)(name);
        Ok(())
    }

    pub fn get_session_name(&self) -> Result<Option<String>, String> {
        let actions = self.actions()?;
        Ok((actions.get_session_name)())
    }

    pub fn set_label(&self, entry_id: &str, label: Option<&str>) -> Result<(), String> {
        let actions = self.actions()?;
        (actions.set_label)(entry_id, label);
        Ok(())
    }

    pub fn get_active_tools(&self) -> Result<Vec<String>, String> {
        let actions = self.actions()?;
        Ok((actions.get_active_tools)())
    }

    pub fn get_all_tools(&self) -> Result<Vec<super::types::ToolInfo>, String> {
        let actions = self.actions()?;
        Ok((actions.get_all_tools)())
    }

    pub fn set_active_tools(&self, tool_names: &[String]) -> Result<(), String> {
        let actions = self.actions()?;
        (actions.set_active_tools)(tool_names);
        Ok(())
    }

    pub fn get_commands(&self) -> Result<Vec<Value>, String> {
        let actions = self.actions()?;
        Ok((actions.get_commands)())
    }

    pub fn set_model(&self, model: &Value) -> Result<super::types::CommandFuture<bool>, String> {
        // Upstream pre-bind: `() => Promise.reject(new Error(...))`.
        match self.actions() {
            Ok(actions) => (actions.set_model)(model),
            Err(_) => Ok(super::types::CommandFuture::rejected(
                RUNTIME_SET_MODEL_NOT_INITIALIZED.to_string(),
            )),
        }
    }

    pub fn get_thinking_level(&self) -> Result<ThinkingLevel, String> {
        let actions = self.actions()?;
        Ok((actions.get_thinking_level)())
    }

    pub fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), String> {
        let actions = self.actions()?;
        (actions.set_thinking_level)(level);
        Ok(())
    }

    /// Upstream runtime `registerProvider`: pre-bind queues; post-bind routes
    /// to the provider actions / model registry.
    pub fn register_provider(
        &self,
        name: &str,
        config: &Value,
        extension_path: &str,
    ) -> Result<(), String> {
        self.assert_active()?;
        let routing = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &state.provider_routing {
                Some(routing) => Some(routing.clone()),
                None => {
                    state
                        .pending_provider_registrations
                        .push(PendingProviderRegistration {
                            name: name.to_string(),
                            config: config.clone(),
                            extension_path: extension_path.to_string(),
                        });
                    None
                }
            }
        };
        match routing {
            Some(routing) => (routing.register_provider)(name, config),
            None => Ok(()),
        }
    }

    pub fn register_native_provider(
        &self,
        provider: &super::types::NativeProvider,
        extension_path: &str,
    ) -> Result<(), String> {
        self.assert_active()?;
        let routing = {
            let state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &state.provider_routing {
                Some(routing) => Some(routing.clone()),
                None => {
                    state.pending_native_provider_registrations.push(
                        PendingNativeProviderRegistration {
                            provider: provider.clone(),
                            extension_path: extension_path.to_string(),
                        },
                    );
                    None
                }
            }
        };
        match routing {
            Some(routing) => (routing.register_native_provider)(provider),
            None => Ok(()),
        }
    }

    pub fn unregister_provider(&self, name: &str) -> Result<(), String> {
        self.assert_active()?;
        let routing = self.with_state(|state| state.provider_routing.clone());
        if let Some(routing) = routing {
            return (routing.unregister_provider)(name);
        }
        let pending = self.with_state(|state| state.pending_provider_registrations.clone());
        let filtered = pending.filtered(|registration| registration.name != name);
        self.with_state(|state| state.pending_provider_registrations = filtered);
        let native = self.with_state(|state| state.pending_native_provider_registrations.clone());
        let filtered = native.filtered(|registration| registration.provider.id() != name);
        self.with_state(|state| state.pending_native_provider_registrations = filtered);
        Ok(())
    }

    /// Upstream `runtime.getSettings()`.
    pub fn get_settings(&self) -> Result<Value, String> {
        let actions = self.actions()?;
        Ok((actions.get_settings)())
    }

    /// Upstream `runtime.createContext()`: a throwing stub until `bindCore`.
    pub fn create_context(&self) -> Result<super::types::ExtensionContext, String> {
        let create = self
            .with_state(|state| state.create_context.clone())
            .ok_or_else(|| RUNTIME_NOT_INITIALIZED.to_string())?;
        create()
    }

    /// Every MCP server registered by extensions
    /// (`runtime.mcpServers.list()`).
    pub fn mcp_servers_list(&self) -> Vec<RegisteredMcpServer> {
        self.with_state(|state| state.mcp_servers.registry.list())
    }

    /// The JSON-at-the-seam snapshots of the registered server configs (see
    /// [`RegisteredMcpServers`]).
    pub fn mcp_server_payloads(&self) -> OrderedMap<Value> {
        self.with_state(|state| state.mcp_servers.payloads.clone())
    }

    /// One registered MCP server (`runtime.mcpServers.get(name)`).
    pub fn mcp_server(&self, name: &str) -> Option<RegisteredMcpServer> {
        self.with_state(|state| state.mcp_servers.registry.get(name).cloned())
    }

    /// Upstream `runtime.mcpServers.register(server)` plus the payload
    /// snapshot; returns whether a change sink is installed (fire it after
    /// the lock is released).
    pub(crate) fn apply_mcp_registration(
        &self,
        name: &str,
        config: McpServerConfig,
        payload: Value,
        extension_path: &str,
    ) -> bool {
        self.with_state(|state| {
            state.mcp_servers.registry.register(RegisteredMcpServer {
                name: name.to_string(),
                config,
                extension_path: extension_path.to_string(),
            });
            state.mcp_servers.payloads.set(name.to_string(), payload);
            state.mcp_change_sink.is_some()
        })
    }

    /// Upstream `runtime.mcpServers.unregister(name, extensionPath)` (a
    /// no-op for servers of other extensions).
    pub(crate) fn apply_mcp_removal(&self, name: &str, extension_path: &str) -> bool {
        self.with_state(|state| {
            let owned = state
                .mcp_servers
                .registry
                .get(name)
                .map(|server| server.extension_path == extension_path)
                .unwrap_or(false);
            if owned {
                state.mcp_servers.registry.unregister(name, extension_path);
                state.mcp_servers.payloads.delete(name);
            }
            owned && state.mcp_change_sink.is_some()
        })
    }

    /// Fire the installed MCP change sink, if any (must be called after the
    /// state lock is released; see [`McpChangeSink`]).
    pub(crate) fn fire_mcp_change(&self) {
        let sink = self.with_state(|state| state.mcp_change_sink.clone());
        if let Some(sink) = sink {
            sink.fire();
        }
    }

    /// Upstream runtime `registerVirtualModel`: pre-bind queues; post-bind
    /// routes to the virtual-model actions / model registry.
    pub fn register_virtual_model(
        &self,
        definition: super::types::VirtualModelDefinitionHandle,
    ) -> Result<(), String> {
        self.assert_active()?;
        let routing = self.with_state(|state| state.virtual_model_routing.clone());
        match routing {
            Some(routing) => (routing.register)(&definition),
            None => {
                self.with_state(|state| {
                    state
                        .pending_virtual_model_registrations
                        .push(super::types::PendingVirtualModelRegistration { definition })
                });
                Ok(())
            }
        }
    }

    /// Upstream runtime `unregisterVirtualModel(provider, id)`: pre-bind
    /// drops matching pending registrations; post-bind routes.
    pub fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String> {
        self.assert_active()?;
        let routing = self.with_state(|state| state.virtual_model_routing.clone());
        if let Some(routing) = routing {
            return (routing.unregister)(provider, id);
        }
        let pending = self.with_state(|state| state.pending_virtual_model_registrations.clone());
        let filtered = pending.filtered(|registration| {
            json_definition_field(&registration.definition.definition, "provider") != provider
                || json_definition_field(&registration.definition.definition, "id") != id
        });
        self.with_state(|state| state.pending_virtual_model_registrations = filtered);
        Ok(())
    }

    /// Upstream bind-time flush of `pendingVirtualModelRegistrations` (for-of
    /// over the live array, then replacement).
    pub(crate) fn flush_pending_virtual_models(
        &self,
        mut register: impl FnMut(super::types::PendingVirtualModelRegistration),
    ) {
        let pending = self.with_state(|state| state.pending_virtual_model_registrations.clone());
        let mut index = 0;
        while let Some(registration) = pending.get(index) {
            index += 1;
            register(registration);
        }
        self.with_state(|state| {
            state.pending_virtual_model_registrations = PendingRegistrations::default()
        });
    }
}

impl Default for ExtensionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Upstream tracked unsubscribe handle: idempotent; also driven by
/// [`ExtensionRuntime::invalidate`].
#[derive(Clone)]
pub struct TrackedEventBusUnsubscribe {
    slot: Arc<Mutex<Option<EventBusUnsubscribe>>>,
}

impl TrackedEventBusUnsubscribe {
    pub fn unsubscribe(&self) {
        if let Some(handle) = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            handle.unsubscribe();
        }
    }
}

// ============================================================================
// Extension API lifecycle (upstream createExtensionAPI)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiPhase {
    Loading,
    Active,
    Failed,
}

pub(crate) struct ApiLifecycle {
    phase: ApiPhase,
    pending_flag_values: OrderedMap<FlagValue>,
    pending_runtime_changes: Vec<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
    loading_unsubscribers: Vec<TrackedEventBusUnsubscribe>,
}

/// The extension under construction, shared between the loader and the API.
pub(crate) type SharedExtension = Arc<Mutex<Extension>>;

/// Upstream `ExtensionAPI` handed to extension factories. Registration
/// methods write to the extension object; action methods delegate to the
/// shared runtime. Factories "throw" by returning `Err`. Clones share the
/// same lifecycle state.
#[derive(Clone)]
pub struct ExtensionApi {
    pub(crate) extension: SharedExtension,
    pub(crate) runtime: ExtensionRuntime,
    pub(crate) cwd: String,
    pub(crate) event_bus: EventBus,
    pub(crate) lifecycle: Arc<Mutex<ApiLifecycle>>,
}

impl ExtensionApi {
    pub(crate) fn new(
        extension: SharedExtension,
        runtime: ExtensionRuntime,
        cwd: &str,
        event_bus: EventBus,
    ) -> Self {
        Self {
            extension,
            runtime,
            cwd: cwd.to_string(),
            event_bus,
            lifecycle: Arc::new(Mutex::new(ApiLifecycle {
                phase: ApiPhase::Loading,
                pending_flag_values: OrderedMap::new(),
                pending_runtime_changes: Vec::new(),
                loading_unsubscribers: Vec::new(),
            })),
        }
    }

    fn assert_active(&self) -> Result<(), String> {
        {
            let lifecycle = self
                .lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if lifecycle.phase == ApiPhase::Failed {
                let extension = self
                    .extension
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                return Err(format!(
                    "Extension \"{}\" failed to load and its API is no longer active.",
                    extension.path
                ));
            }
        }
        self.runtime.assert_active()
    }

    fn apply_runtime_change(
        &self,
        change: impl Fn() -> Result<(), String> + Send + Sync + 'static,
    ) -> Result<(), String> {
        let change: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(change);
        let loading = {
            let mut lifecycle = self
                .lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if lifecycle.phase == ApiPhase::Loading {
                lifecycle.pending_runtime_changes.push(Arc::clone(&change));
                true
            } else {
                false
            }
        };
        if loading {
            Ok(())
        } else {
            change()
        }
    }

    fn extension_path(&self) -> String {
        self.extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .path
            .clone()
    }

    fn source_info(&self) -> SourceInfo {
        self.extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .source_info
            .clone()
    }

    // -- Registration methods ------------------------------------------------

    /// Upstream `on(event, handler)`; returns the unsubscribe closure.
    pub fn on(&self, event: &str, handler: HandlerFn) -> Result<HandlerUnsubscribe, String> {
        self.assert_active()?;
        let extension = self
            .extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Upstream wraps each registration in a fresh `registeredHandler`
        // closure, so duplicate registrations of one function stay
        // independently removable.
        let registered: HandlerFn = {
            let handler = Arc::clone(&handler);
            Arc::new(move |event, ctx| handler(event, ctx))
        };
        let mut handlers = extension.handlers.get_cloned_list(event);
        handlers.push(Arc::clone(&registered));
        extension.handlers.set(event.to_string(), handlers);
        Ok(HandlerUnsubscribe {
            handlers: extension.handlers.clone(),
            event: event.to_string(),
            handler: registered,
        })
    }

    /// Upstream `registerTool(tool)`.
    pub fn register_tool(&self, tool: ToolDefinition) -> Result<(), String> {
        self.assert_active()?;
        if !tool.parameters.is_object() {
            let name = &tool.name;
            let extension_path = self.extension_path();
            return Err(format!(
                "Tool \"{name}\" registered by extension \"{extension_path}\" must define an object parameter schema."
            ));
        }
        let source_info = self.source_info();
        let name = tool.name.clone();
        let active = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase == ApiPhase::Active;
        let registered = RegisteredTool {
            definition: Arc::new(tool),
            source_info,
        };
        {
            let mut extension = self
                .extension
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            extension.tools.set(&name, registered.clone());
            if active {
                extension
                    .live_tools
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set(&name, registered);
            }
        }
        self.runtime.refresh_tools();
        Ok(())
    }

    /// Upstream `registerCommand(name, options)`.
    pub fn register_command(
        &self,
        name: &str,
        description: Option<String>,
        handler: CommandHandler,
    ) -> Result<(), String> {
        self.assert_active()?;
        // Upstream validates before storing: a non-string/empty name throws
        // (the port's `&str` can only be empty), and a missing handler
        // throws — unrepresentable here because `CommandHandler` is not
        // optional in this signature.
        if name.is_empty() {
            let extension_path = self.extension_path();
            return Err(format!(
                "Command registered by extension \"{extension_path}\" must have a non-empty string name. Use pi.registerCommand(\"name\", {{ description, handler }})."
            ));
        }
        let source_info = self.source_info();
        self.extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .commands
            .set(
                name,
                RegisteredCommand {
                    name: name.to_string(),
                    source_info,
                    description,
                    get_argument_completions: None,
                    handler,
                },
            );
        Ok(())
    }

    /// Upstream `registerShortcut(shortcut, options)`.
    pub fn register_shortcut(
        &self,
        shortcut: &str,
        description: Option<String>,
        handler: super::types::ShortcutHandler,
    ) -> Result<(), String> {
        self.assert_active()?;
        let extension_path = self.extension_path();
        self.extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shortcuts
            .set(
                shortcut,
                super::types::ExtensionShortcut {
                    shortcut: shortcut.to_string(),
                    description,
                    handler,
                    extension_path,
                },
            );
        Ok(())
    }

    /// Upstream `registerFlag(name, options)`.
    pub fn register_flag(
        &self,
        name: &str,
        description: Option<String>,
        flag_type: FlagType,
        default: Option<FlagValue>,
    ) -> Result<(), String> {
        self.assert_active()?;
        if let Some(value) = &default {
            if !flag_type.matches(value) {
                return Err(format!(
                    "Invalid default for flag \"{name}\": expected {expected}, got {got}",
                    expected = flag_type.as_str(),
                    got = value.type_name()
                ));
            }
        }
        let extension_path = self.extension_path();
        self.extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flags
            .set(
                name,
                ExtensionFlag {
                    name: name.to_string(),
                    description,
                    flag_type,
                    default: default.clone(),
                    extension_path,
                },
            );
        if let Some(value) = default {
            if self.runtime.flag_value(name).is_none() {
                let loading = self
                    .lifecycle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .phase
                    == ApiPhase::Loading;
                if loading {
                    let mut lifecycle = self
                        .lifecycle
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if !lifecycle.pending_flag_values.has(name) {
                        lifecycle.pending_flag_values.set(name, value);
                    }
                } else {
                    self.runtime.set_flag_value(name, value);
                }
            }
        }
        Ok(())
    }

    /// Upstream `registerMessageRenderer(customType, renderer)`.
    pub fn register_message_renderer(
        &self,
        custom_type: &str,
        renderer: super::types::MessageRenderer,
    ) -> Result<(), String> {
        self.assert_active()?;
        self.extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .message_renderers
            .set(custom_type, renderer);
        Ok(())
    }

    /// Upstream `registerMarkdownTransformer(transformer)`.
    pub fn register_markdown_transformer(
        &self,
        transformer: super::types::MarkdownTransformer,
    ) -> Result<(), String> {
        self.assert_active()?;
        self.extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .markdown_transformer = Some(transformer);
        Ok(())
    }

    /// Upstream `registerEntryRenderer(customType, renderer)`.
    pub fn register_entry_renderer(
        &self,
        custom_type: &str,
        renderer: super::types::EntryRenderer,
    ) -> Result<(), String> {
        self.assert_active()?;
        let mut extension = self
            .extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let renderers = extension
            .entry_renderers
            .get_or_insert_with(OrderedMap::new);
        renderers.set(custom_type, renderer);
        Ok(())
    }

    // -- Flag access ---------------------------------------------------------

    /// Upstream `getFlag(name)` — only flags this extension registered.
    pub fn get_flag(&self, name: &str) -> Result<Option<FlagValue>, String> {
        self.assert_active()?;
        let registered = self
            .extension
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flags
            .has(name);
        if !registered {
            return Ok(None);
        }
        if let Some(value) = self.runtime.flag_value(name) {
            return Ok(Some(value));
        }
        let lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(lifecycle.pending_flag_values.get(name).cloned())
    }

    // -- Action methods (delegate to the shared runtime) ---------------------

    pub fn send_message(
        &self,
        message: &Value,
        options: &SendMessageOptions,
    ) -> Result<(), String> {
        self.assert_active()?;
        self.runtime.send_message(message, options)
    }

    pub fn send_user_message(
        &self,
        content: &Value,
        options: &SendUserMessageOptions,
    ) -> Result<(), String> {
        self.assert_active()?;
        self.runtime.send_user_message(content, options)
    }

    pub fn append_entry(&self, custom_type: &str, data: Option<&Value>) -> Result<(), String> {
        self.assert_active()?;
        self.runtime.append_entry(custom_type, data)
    }

    pub fn set_session_name(&self, name: &str) -> Result<(), String> {
        self.assert_active()?;
        self.runtime.set_session_name(name)
    }

    pub fn get_session_name(&self) -> Result<Option<String>, String> {
        self.assert_active()?;
        self.runtime.get_session_name()
    }

    pub fn set_label(&self, entry_id: &str, label: Option<&str>) -> Result<(), String> {
        self.assert_active()?;
        self.runtime.set_label(entry_id, label)
    }

    /// Upstream `exec(command, args, options)` — delegates to [`exec_command`]
    /// with the loader cwd as the default working directory.
    pub fn exec(
        &self,
        command: &str,
        args: &[String],
        options: Option<ExecOptions>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ExecResult> + Send>> {
        let command = command.to_string();
        let args = args.to_vec();
        let cwd = options
            .as_ref()
            .and_then(|options| options.cwd.clone())
            .unwrap_or_else(|| self.cwd.clone());
        Box::pin(async move { exec_command(&command, &args, &cwd, options).await })
    }

    pub fn get_active_tools(&self) -> Result<Vec<String>, String> {
        self.assert_active()?;
        self.runtime.get_active_tools()
    }

    pub fn get_all_tools(&self) -> Result<Vec<super::types::ToolInfo>, String> {
        self.assert_active()?;
        self.runtime.get_all_tools()
    }

    /// Upstream `getSettings()`: a copy of the effective settings (global and
    /// project settings merged, with overrides), JSON at the seam.
    pub fn get_settings(&self) -> Result<Value, String> {
        self.assert_active()?;
        self.runtime.get_settings()
    }

    pub fn set_active_tools(&self, tool_names: &[String]) -> Result<(), String> {
        self.assert_active()?;
        self.runtime.set_active_tools(tool_names)
    }

    pub fn get_commands(&self) -> Result<Vec<Value>, String> {
        self.assert_active()?;
        self.runtime.get_commands()
    }

    pub fn set_model(&self, model: &Value) -> Result<super::types::CommandFuture<bool>, String> {
        self.assert_active()?;
        self.runtime.set_model(model)
    }

    pub fn get_thinking_level(&self) -> Result<ThinkingLevel, String> {
        self.assert_active()?;
        self.runtime.get_thinking_level()
    }

    pub fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), String> {
        self.assert_active()?;
        self.runtime.set_thinking_level(level)
    }

    pub fn register_provider(&self, name: &str, config: &Value) -> Result<(), String> {
        self.assert_active()?;
        let extension_path = self.extension_path();
        let runtime = self.runtime.clone();
        let name = name.to_string();
        let config = config.clone();
        self.apply_runtime_change(move || {
            runtime.register_provider(&name, &config, &extension_path)
        })
    }

    pub fn register_native_provider(
        &self,
        provider: &super::types::NativeProvider,
    ) -> Result<(), String> {
        self.assert_active()?;
        let extension_path = self.extension_path();
        let runtime = self.runtime.clone();
        let provider = provider.clone();
        self.apply_runtime_change(move || {
            runtime.register_native_provider(&provider, &extension_path)
        })
    }

    pub fn unregister_provider(&self, name: &str) -> Result<(), String> {
        self.assert_active()?;
        let runtime = self.runtime.clone();
        let name = name.to_string();
        self.apply_runtime_change(move || runtime.unregister_provider(&name))
    }

    /// Upstream `registerMcpServer(name, config)`: validate, then register
    /// for this session. The registration is not saved; a server of the same
    /// name in `mcp.json` takes precedence upstream. Throws for invalid
    /// configs and for names another extension registered.
    pub fn register_mcp_server(
        &self,
        name: &str,
        config: &crate::ai::types::ordered_map::OrderedMap<Value>,
    ) -> Result<(), String> {
        self.assert_active()?;
        let extension_path = self.extension_path();
        let validated = match validate_mcp_server_config(name, config) {
            Ok(validated) => validated,
            Err(message) => {
                return Err(format!(
                    "Invalid MCP server registered by extension \"{extension_path}\": {message}"
                ));
            }
        };
        // Ownership: registering a name again replaces THIS extension's
        // earlier registration; another extension's is an error.
        if let Some(owner) = self.runtime.mcp_server(name) {
            if owner.extension_path != extension_path {
                return Err(format!(
                    "MCP server \"{name}\" is already registered by extension \"{}\"",
                    owner.extension_path
                ));
            }
        }
        // v1.0.0: names that differ only in `-` and `_` would share a namespace.
        let clash = self.runtime.mcp_servers_list().into_iter().find(|server| {
            server.name != name && mcp_namespace(&server.name) == mcp_namespace(name)
        });
        if let Some(clash) = clash {
            return Err(format!(
                "MCP server \"{name}\" conflicts with registered server \"{}\"",
                clash.name
            ));
        }
        let payload = resolved_mcp_config_value(config, &validated);
        let runtime = self.runtime.clone();
        let name_owned = name.to_string();
        let validated_clone = validated.clone();
        let extension_path_clone = extension_path.clone();
        // The registration may be deferred to commit (during loading); the
        // change listener fires whenever the mutation actually runs, exactly
        // like the upstream registry's listener.
        self.apply_runtime_change(move || {
            let fire = runtime.apply_mcp_registration(
                &name_owned,
                validated_clone.clone(),
                payload.clone(),
                &extension_path_clone,
            );
            if fire {
                runtime.fire_mcp_change();
            }
            Ok(())
        })
    }

    /// Upstream `unregisterMcpServer(name)`: remove an MCP server this
    /// extension registered (a no-op for servers of other extensions).
    pub fn unregister_mcp_server(&self, name: &str) -> Result<(), String> {
        self.assert_active()?;
        let extension_path = self.extension_path();
        let runtime = self.runtime.clone();
        let name_owned = name.to_string();
        self.apply_runtime_change(move || {
            let fire = runtime.apply_mcp_removal(&name_owned, &extension_path);
            if fire {
                runtime.fire_mcp_change();
            }
            Ok(())
        })
    }

    /// Upstream `getMcpServers()`: every MCP server registered by extensions.
    pub fn get_mcp_servers(&self) -> Result<Vec<RegisteredMcpServer>, String> {
        self.assert_active()?;
        Ok(self.runtime.mcp_servers_list())
    }

    /// Upstream `registerVirtualModel(model)`: register a virtual model — a
    /// selectable catalog entry that routes each request to a physical model.
    /// `definition` carries the catalog fields (provider/id/name/…); `route`
    /// is the extension's routing function, wrapped here to bind
    /// `runtime.createContext()` per request (upstream closes over
    /// `runtime.createContext` at registration).
    pub fn register_virtual_model(
        &self,
        definition: Value,
        route: super::types::ExtensionVirtualModelRouteFn,
    ) -> Result<(), String> {
        self.assert_active()?;
        let runtime = self.runtime.clone();
        let route: super::types::WrappedVirtualModelRouteFn = Arc::new(move |request| {
            let runtime = runtime.clone();
            let route = Arc::clone(&route);
            Box::pin(async move {
                let ctx = runtime.create_context()?;
                route(request, ctx).await
            })
        });
        let extension_path = self.extension_path();
        let handle = Arc::new(super::types::VirtualModelDefinitionHandle {
            definition,
            route,
            extension_path,
        });
        let runtime = self.runtime.clone();
        let handle_for_change = Arc::clone(&handle);
        self.apply_runtime_change(move || {
            runtime.register_virtual_model((*handle_for_change).clone())
        })
    }

    /// Upstream `unregisterVirtualModel(provider, id)`.
    pub fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<(), String> {
        self.assert_active()?;
        let runtime = self.runtime.clone();
        let provider = provider.to_string();
        let id = id.to_string();
        self.apply_runtime_change(move || runtime.unregister_virtual_model(&provider, &id))
    }

    // -- Event bus -----------------------------------------------------------

    /// Upstream `events.emit(channel, data)`.
    pub fn emit_event(&self, channel: &str, data: &Value) -> Result<(), String> {
        self.assert_active()?;
        self.event_bus.emit(channel, data);
        Ok(())
    }

    /// Upstream `events.on(channel, handler)` — retained until invalidation.
    pub fn on_event(
        &self,
        channel: &str,
        handler: crate::coding_agent::core::event_bus::EventHandler,
    ) -> Result<TrackedEventBusUnsubscribe, String> {
        self.assert_active()?;
        let unsubscribe = self
            .runtime
            .track_event_bus_subscription(self.event_bus.on(channel, handler));
        let loading = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase
            == ApiPhase::Loading;
        if loading {
            self.lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .loading_unsubscribers
                .push(unsubscribe.clone());
        }
        Ok(unsubscribe)
    }

    // -- Commit / discard ----------------------------------------------------

    /// Upstream `commit()`.
    pub fn commit(&self) -> Result<(), String> {
        {
            let lifecycle = self
                .lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if lifecycle.phase != ApiPhase::Loading {
                return Ok(());
            }
        }
        self.runtime.assert_active()?;
        let (pending_flags, pending_changes) = {
            let mut lifecycle = self
                .lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                lifecycle.pending_flag_values.clone(),
                std::mem::take(&mut lifecycle.pending_runtime_changes),
            )
        };
        for (name, value) in pending_flags {
            if self.runtime.flag_value(&name).is_none() {
                self.runtime.set_flag_value(name, value);
            }
        }
        for apply in pending_changes {
            apply()?;
        }
        {
            let mut lifecycle = self
                .lifecycle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            lifecycle.phase = ApiPhase::Active;
            lifecycle.pending_flag_values.clear();
            lifecycle.pending_runtime_changes.clear();
            lifecycle.loading_unsubscribers.clear();
        }
        Ok(())
    }

    /// Upstream `discard()`.
    pub fn discard(&self) {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lifecycle.phase != ApiPhase::Loading {
            return;
        }
        lifecycle.phase = ApiPhase::Failed;
        let unsubs = std::mem::take(&mut lifecycle.loading_unsubscribers);
        for unsubscribe in unsubs {
            unsubscribe.unsubscribe();
        }
        lifecycle.pending_flag_values.clear();
        lifecycle.pending_runtime_changes.clear();
    }
}

/// Upstream `on()` unsubscribe closure (handler identity via `Arc::ptr_eq`).
#[derive(Clone)]
pub struct HandlerUnsubscribe {
    handlers: super::types::SharedHandlerMap,
    event: String,
    handler: HandlerFn,
}

impl HandlerUnsubscribe {
    pub fn unsubscribe(&self) {
        let mut map = self
            .handlers
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(handlers) = map.get_mut(&self.event) else {
            return;
        };
        if let Some(index) = handlers
            .iter()
            .position(|existing| Arc::ptr_eq(existing, &self.handler))
        {
            handlers.remove(index);
            if handlers.is_empty() {
                map.delete(&self.event);
            }
        }
    }
}

// ============================================================================
// exec (vendored subset of core/exec.ts)
// ============================================================================

/// Upstream `ExecOptions`.
#[derive(Default, Clone)]
pub struct ExecOptions {
    pub signal: Option<Arc<super::types::AbortSignal>>,
    pub timeout: Option<u64>,
    pub cwd: Option<String>,
}

/// Upstream `ExecResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub killed: bool,
}

/// Upstream `execCommand(command, args, cwd, options)` (exec.ts): spawn
/// without a shell, capture stdout/stderr, kill on timeout/abort, resolve
/// `code: 1` when the process could not be spawned.
pub async fn exec_command(
    command: &str,
    args: &[String],
    cwd: &str,
    options: Option<ExecOptions>,
) -> ExecResult {
    use tokio::io::AsyncReadExt;

    let options = options.unwrap_or_default();
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return ExecResult {
                code: 1,
                ..ExecResult::default()
            };
        }
    };

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = pipe.read_to_end(&mut buf).await;
        }
        buf
    });

    let deadline = options
        .timeout
        .filter(|timeout| *timeout > 0)
        .map(std::time::Duration::from_millis);
    let started = std::time::Instant::now();
    let mut killed = false;

    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                if !killed {
                    let aborted = options.signal.as_ref().is_some_and(|signal| signal.is_aborted());
                    let timed_out = deadline.is_some_and(|duration| started.elapsed() >= duration);
                    if aborted || timed_out {
                        killed = true;
                        let _ = child.start_kill();
                    }
                }
            }
        }
    };
    // Post-exit drain, bounded (upstream tolerates detached descendants).
    let drained = tokio::time::timeout(std::time::Duration::from_millis(200), async {
        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        (stdout, stderr)
    })
    .await
    .unwrap_or_default();
    let code = status.map(|status| status.code().unwrap_or(0)).unwrap_or(1);
    ExecResult {
        stdout: String::from_utf8_lossy(&drained.0).to_string(),
        stderr: String::from_utf8_lossy(&drained.1).to_string(),
        code,
        killed,
    }
}

// ============================================================================
// Extension construction
// ============================================================================

/// `path.dirname` on the host platform (inputs are resolved absolute paths).
fn path_dirname(path: &str) -> String {
    let seps: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    let trimmed = path.trim_end_matches(seps);
    match trimmed.rfind(seps) {
        Some(index) if index > 0 => trimmed[..index].to_string(),
        Some(index) => trimmed[..index + 1].to_string(),
        None => ".".to_string(),
    }
}

fn path_join2(base: &str, segment: &str) -> String {
    if cfg!(windows) {
        node_path::win32_join(&[base, segment])
    } else {
        node_path::posix_join(&[base, segment])
    }
}

/// `fs.existsSync` (follows symlinks).
fn path_exists(path: &str) -> bool {
    std::fs::metadata(path).is_ok()
}

/// Upstream `createExtension(extensionPath, resolvedPath)`.
fn create_extension(extension_path: &str, resolved_path: &str) -> Extension {
    // Upstream: `getSyntheticPathSource(extensionPath) ?? "local"`; file
    // paths carry a baseDir, synthetic ones do not.
    let source = super::types::get_synthetic_path_source(extension_path)
        .unwrap_or_else(|| "local".to_string());
    let base_dir = if super::types::is_synthetic_path(extension_path) {
        None
    } else {
        Some(path_dirname(resolved_path))
    };
    Extension {
        path: extension_path.to_string(),
        resolved_path: resolved_path.to_string(),
        hidden: false,
        replaceable: false,
        source_info: super::types::create_synthetic_source_info(
            extension_path,
            &source,
            Some(super::types::SourceScope::Temporary),
            Some(super::types::SourceOrigin::TopLevel),
            base_dir,
        ),
        handlers: super::types::SharedHandlerMap::new(),
        tools: OrderedMap::new(),
        live_tools: Arc::new(Mutex::new(OrderedMap::new())),
        message_renderers: OrderedMap::new(),
        markdown_transformer: None,
        entry_renderers: Some(OrderedMap::new()),
        commands: OrderedMap::new(),
        flags: OrderedMap::new(),
        shortcuts: OrderedMap::new(),
    }
}

/// Upstream `initializeExtension(factory, …)`.
fn initialize_extension(
    factory: &ExtensionFactory,
    extension_path: &str,
    resolved_path: &str,
    cwd: &str,
    event_bus: EventBus,
    runtime: &ExtensionRuntime,
) -> Result<Extension, String> {
    let extension: SharedExtension =
        Arc::new(Mutex::new(create_extension(extension_path, resolved_path)));
    let load = ExtensionApi::new(Arc::clone(&extension), runtime.clone(), cwd, event_bus);
    match factory(&load) {
        Ok(()) => {
            load.commit()?;
            Ok(extension
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }
        Err(error) => {
            load.discard();
            Err(error)
        }
    }
}

/// Upstream `loadExtension(extensionPath, …)` → `{ extension, error }`.
fn load_extension(
    extension_path: &str,
    cwd: &str,
    event_bus: EventBus,
    runtime: &ExtensionRuntime,
    loader: &dyn ExtensionModuleLoader,
    cache_token: &Option<ExtensionCacheToken>,
) -> (Option<Extension>, Option<String>) {
    let resolved_path = resolve_path_with(
        extension_path,
        cwd,
        &PathInputOptions {
            normalize_unicode_spaces: true,
            ..Default::default()
        },
        cfg!(windows),
    );
    let resolved_path = match resolved_path {
        Ok(resolved) => resolved,
        Err(error) => return (None, Some(format!("Failed to load extension: {error}"))),
    };

    match load_extension_module(loader, &resolved_path, cache_token) {
        Ok(Some(factory)) => {
            match initialize_extension(
                &factory,
                extension_path,
                &resolved_path,
                cwd,
                event_bus,
                runtime,
            ) {
                Ok(extension) => (Some(extension), None),
                Err(error) => (None, Some(format!("Failed to load extension: {error}"))),
            }
        }
        Ok(None) => (
            None,
            Some(format!(
                "Extension does not export a valid factory function: {extension_path}"
            )),
        ),
        Err(error) => (None, Some(format!("Failed to load extension: {error}"))),
    }
}

/// Upstream `loadExtensionFromFactory(factory, cwd, eventBus, runtime,
/// extensionPath = "<inline>")`.
pub fn load_extension_from_factory(
    factory: ExtensionFactory,
    cwd: &str,
    event_bus: EventBus,
    runtime: &ExtensionRuntime,
    extension_path: Option<&str>,
) -> Result<Extension, String> {
    let resolved_cwd = resolve_path_with(cwd, "", &PathInputOptions::default(), cfg!(windows))
        .unwrap_or_else(|_| cwd.to_string());
    let extension_path = extension_path.unwrap_or("<inline>");
    initialize_extension(
        &factory,
        extension_path,
        extension_path,
        &resolved_cwd,
        event_bus,
        runtime,
    )
}

/// Upstream `loadExtensionsInternal(paths, cwd, eventBus?, runtime?,
/// useCache?)`.
fn load_extensions_internal(
    paths: &[String],
    cwd: &str,
    event_bus: Option<EventBus>,
    runtime: Option<ExtensionRuntime>,
    use_cache: bool,
    loader: &dyn ExtensionModuleLoader,
) -> LoadExtensionsResult {
    let mut extensions = Vec::new();
    let mut errors: Vec<ExtensionLoadError> = Vec::new();
    // Nothing in this slice pushes loader warnings yet; the field mirrors the
    // upstream shape (`loadExtensionsInternal` declares and returns it).
    let warnings: Vec<super::types::ExtensionLoadWarning> = Vec::new();
    let cache_token = if use_cache {
        Some(use_extension_cache_cwd(cwd))
    } else {
        None
    };
    let resolved_cwd = cache_token
        .as_ref()
        .map(|token| token.cwd.clone())
        .unwrap_or_else(|| {
            resolve_path_with(cwd, "", &PathInputOptions::default(), cfg!(windows))
                .unwrap_or_else(|_| cwd.to_string())
        });
    let resolved_event_bus = event_bus.unwrap_or_else(|| EventBusController::new().bus().clone());
    let resolved_runtime = runtime.unwrap_or_default();

    for ext_path in paths {
        let (extension, error) = load_extension(
            ext_path,
            &resolved_cwd,
            resolved_event_bus.clone(),
            &resolved_runtime,
            loader,
            &cache_token,
        );
        if let Some(error) = error {
            errors.push(ExtensionLoadError {
                path: ext_path.clone(),
                error,
            });
            continue;
        }
        if let Some(extension) = extension {
            extensions.push(extension);
        }
    }

    LoadExtensionsResult {
        extensions,
        errors,
        warnings,
        runtime: resolved_runtime,
    }
}

/// Upstream `loadExtensions(paths, cwd, eventBus?, runtime?)`.
pub fn load_extensions(
    paths: &[String],
    cwd: &str,
    event_bus: Option<EventBus>,
    runtime: Option<ExtensionRuntime>,
    loader: &dyn ExtensionModuleLoader,
) -> LoadExtensionsResult {
    load_extensions_internal(paths, cwd, event_bus, runtime, false, loader)
}

/// Upstream `loadExtensionsCached(paths, cwd, eventBus?, runtime?)`.
pub fn load_extensions_cached(
    paths: &[String],
    cwd: &str,
    event_bus: Option<EventBus>,
    runtime: Option<ExtensionRuntime>,
    loader: &dyn ExtensionModuleLoader,
) -> LoadExtensionsResult {
    load_extensions_internal(paths, cwd, event_bus, runtime, true, loader)
}

// ============================================================================
// pi manifest (vendored from core/pi-manifest.ts)
// ============================================================================

/// Upstream `PiManifest`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PiManifest {
    pub extensions: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub themes: Option<Vec<String>>,
}

/// Upstream `readPiManifest(packageJsonPath)` (pi-manifest.ts).
pub fn read_pi_manifest(package_json_path: &str) -> Option<PiManifest> {
    let content = std::fs::read_to_string(package_json_path).ok()?;
    let stripped = crate::coding_agent::utils::text::strip_bom(&content);
    let pkg: Value = serde_json::from_str(stripped).ok()?;
    let pi = pkg.get("pi")?.as_object()?;
    let read_field = |field: &str| -> Option<Vec<String>> {
        let entries = pi.get(field)?.as_array()?;
        if entries.iter().all(|entry| entry.is_string()) {
            Some(
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect(),
            )
        } else {
            None
        }
    };
    Some(PiManifest {
        extensions: read_field("extensions"),
        skills: read_field("skills"),
        prompts: read_field("prompts"),
        themes: read_field("themes"),
    })
}

// ============================================================================
// Discovery
// ============================================================================

/// Upstream `isExtensionFile(name)`.
fn is_extension_file(name: &str) -> bool {
    name.ends_with(".ts") || name.ends_with(".js")
}

/// `path.resolve(dir, entry)` on the host platform (`dir` is absolute).
fn resolve_under(dir: &str, entry: &str) -> String {
    if cfg!(windows) {
        node_path::win32_resolve(
            &[dir, entry],
            &std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy(),
        )
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        // node's posixCwd() on a Windows host strips the drive; on POSIX it
        // is the cwd unchanged.
        let cwd = if cfg!(windows) {
            let lossy = cwd.to_string_lossy().replace('\\', "/");
            lossy
                .split_once(':')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or(lossy)
        } else {
            cwd.to_string_lossy().to_string()
        };
        node_path::posix_resolve(&[dir, entry], &cwd)
    }
}

/// Upstream `resolveExtensionEntries(dir)` — package.json `pi.extensions`
/// first, then index.ts, then index.js. `None` when no entry point exists.
fn resolve_extension_entries(dir: &str) -> Option<Vec<String>> {
    let package_json_path = path_join2(dir, "package.json");
    if path_exists(&package_json_path) {
        if let Some(manifest) = read_pi_manifest(&package_json_path) {
            if let Some(declared) = manifest.extensions {
                if !declared.is_empty() {
                    let mut entries = Vec::new();
                    for ext_path in declared {
                        let resolved_ext_path = resolve_under(dir, &ext_path);
                        if path_exists(&resolved_ext_path) {
                            entries.push(resolved_ext_path);
                        }
                    }
                    if !entries.is_empty() {
                        return Some(entries);
                    }
                }
            }
        }
    }

    let index_ts = path_join2(dir, "index.ts");
    if path_exists(&index_ts) {
        return Some(vec![index_ts]);
    }
    let index_js = path_join2(dir, "index.js");
    if path_exists(&index_js) {
        return Some(vec![index_js]);
    }
    None
}

/// Upstream `discoverExtensionsInDir(dir)`: direct `*.ts`/`*.js` files,
/// subdirectories with an entry point, no recursion beyond one level, readdir
/// errors collapse to empty, OS readdir order preserved.
fn discover_extensions_in_dir(dir: &str) -> Vec<String> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(_) => return Vec::new(),
    };

    let mut discovered = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = path_join2(dir, &name);
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        // 1. Direct files: *.ts or *.js
        if (file_type.is_file() || file_type.is_symlink()) && is_extension_file(&name) {
            discovered.push(entry_path);
            continue;
        }

        // 2 & 3. Subdirectories
        if file_type.is_dir() || file_type.is_symlink() {
            if let Some(entries) = resolve_extension_entries(&entry_path) {
                discovered.extend(entries);
            }
        }
    }
    discovered
}

/// Upstream `discoverAndLoadExtensions(configuredPaths, cwd, agentDir =
/// getAgentDir(), eventBus?)`.
pub fn discover_and_load_extensions(
    configured_paths: &[String],
    cwd: &str,
    agent_dir: &str,
    event_bus: Option<EventBus>,
    loader: &dyn ExtensionModuleLoader,
) -> LoadExtensionsResult {
    let resolved_cwd = resolve_path_with(cwd, "", &PathInputOptions::default(), cfg!(windows))
        .unwrap_or_else(|_| cwd.to_string());
    let resolved_agent_dir =
        resolve_path_with(agent_dir, "", &PathInputOptions::default(), cfg!(windows))
            .unwrap_or_else(|_| agent_dir.to_string());
    let mut all_paths: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut add_paths = |paths: Vec<String>| {
        for p in paths {
            // upstream `path.resolve(p)` — process cwd base, no tilde/unicode
            // normalization.
            let resolved = resolve_path_auto_base(&p).unwrap_or_else(|_| p.clone());
            if seen.insert(resolved) {
                all_paths.push(p);
            }
        }
    };

    // 1. Project-local extensions: cwd/${CONFIG_DIR_NAME}/extensions/
    let local_ext_dir = path_join2(&path_join2(&resolved_cwd, CONFIG_DIR_NAME), "extensions");
    add_paths(discover_extensions_in_dir(&local_ext_dir));

    // 2. Global extensions: agentDir/extensions/
    let global_ext_dir = path_join2(&resolved_agent_dir, "extensions");
    add_paths(discover_extensions_in_dir(&global_ext_dir));

    // 3. Explicitly configured paths
    for p in configured_paths {
        let resolved = resolve_path_with(
            p,
            &resolved_cwd,
            &PathInputOptions {
                normalize_unicode_spaces: true,
                ..Default::default()
            },
            cfg!(windows),
        )
        .unwrap_or_else(|_| p.clone());
        if path_exists(&resolved)
            && std::fs::metadata(&resolved)
                .map(|m| m.is_dir())
                .unwrap_or(false)
        {
            // Check for package.json with pi manifest or index.ts
            if let Some(entries) = resolve_extension_entries(&resolved) {
                add_paths(entries);
                continue;
            }
            // No explicit entries — discover individual files in directory
            add_paths(discover_extensions_in_dir(&resolved));
            continue;
        }
        add_paths(vec![resolved]);
    }

    load_extensions(&all_paths, &resolved_cwd, event_bus, None, loader)
}

/// Keep the abort-flag type referenced for downstream slices (the exec seam
/// consumes it directly).
#[allow(unused_imports)]
use std::sync::atomic::AtomicBool as _AtomicBoolUsed;

#[cfg(test)]
#[path = "loader_tests.rs"]
mod tests;
