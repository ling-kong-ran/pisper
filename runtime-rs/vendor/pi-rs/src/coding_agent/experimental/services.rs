//! Port of the deterministic face of upstream `experimental/services/`
//! (worker.ts sha256 3d7f193d6d6fe3f5bd956e8f06bf4c3ba57ed4f2f941669d74f32225a01dcafd,
//! plus the concrete payload types from `agent-controller.ts`, `models.ts`,
//! `transcript.ts`, `plugins.ts`; upstream file list hashed at port time).
//!
//! Ported: the five service ids, the `SessionWorkerServices` endpoint-per-
//! scope bookkeeping (`invoke`, `removeSubscriptions`, serialized plugin
//! `reload` tail chain, `dispose` ordering with error aggregation), and the
//! agent-controller request/response payload schemas.
//!
//! D6 seam (see mod.rs docs): chord's `FacetHost`, `defineFacet`,
//! `createRemoteServiceEndpoint` and `parseServiceProviderUpdate` are not
//! ported; endpoints are the [`ServiceEndpoint`] trait and updates opaque
//! `serde_json::Value`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coding_agent::experimental::session_worker::WorkerOperationScope;

/// Upstream `defineService` ids (`services/*.ts`).
pub mod service_ids {
    /// `services/agent-controller.ts` — `pi.agent-controller`.
    pub const AGENT_CONTROLLER: &str = "pi.agent-controller";
    /// `services/models.ts` — `pi.models`.
    pub const MODELS: &str = "pi.models";
    /// `services/transcript.ts` — `pi.transcript`.
    pub const TRANSCRIPT: &str = "pi.transcript";
    /// `services/presentation-ui.ts` — `pi.presentation-plugins`.
    pub const PRESENTATION_PLUGINS: &str = "pi.presentation-plugins";
    /// `services/plugins.ts` — `pi.session-plugins`.
    pub const SESSION_PLUGINS: &str = "pi.session-plugins";
}

/// Upstream `AgentPromptImage`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "image", rename_all = "camelCase")]
pub struct AgentPromptImage {
    pub data: String,
    pub mime_type: String,
}

/// Upstream `AgentPromptRequest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptRequest {
    pub message: String,
    pub images: Option<Vec<AgentPromptImage>>,
}

/// Upstream `AgentOperationError`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOperationError {
    pub code: String,
    pub message: String,
}

/// Upstream `AgentOperationResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOperationResponse {
    pub accepted: bool,
    pub operation_id: Option<String>,
    pub error: Option<AgentOperationError>,
}

/// Upstream `AgentQueueResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQueueResponse {
    pub accepted: bool,
    pub entry_id: Option<String>,
    pub error: Option<AgentOperationError>,
}

/// Upstream `AgentCompactionRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCompactionRequest {
    pub custom_instructions: Option<String>,
}

/// Upstream `AgentNavigationRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNavigationRequest {
    pub target_id: Option<String>,
    pub summarize: bool,
    pub label: Option<String>,
    pub custom_instructions: Option<String>,
}

/// Upstream `WorkerServiceScope` alias (`services/worker.ts`).
pub type WorkerServiceScope = WorkerOperationScope;

/// D6: one scoped remote-service endpoint (upstream
/// `createRemoteServiceEndpoint(provider)`).
pub trait ServiceEndpoint: Send + Sync {
    fn invoke(
        self: Arc<Self>,
        call: crate::coding_agent::experimental::session_worker::ServiceCall,
        publish: Box<dyn FnOnce(String, Value) -> BoxFuture<'static, ()> + Send>,
    ) -> BoxFuture<'static, Result<Option<Value>, String>>;
    /// Upstream `endpoint.dispose()`.
    fn dispose(&self);
}

/// D6: upstream `pluginLoader.load()` seam (facet generations).
pub trait PluginLoader: Send + Sync {
    /// Returns the facet list handle for one generation. The `String` is the
    /// opaque facet identity used by the dispose bookkeeping.
    fn load(&self) -> BoxFuture<'static, Result<FacetGeneration, String>>;
}

/// One loaded facet generation with its dispose hook.
pub struct FacetGeneration {
    pub facets: Vec<String>,
    pub dispose: Box<dyn FnOnce() -> BoxFuture<'static, Result<(), String>> + Send>,
}

/// D6: upstream `FacetHost` seam.
pub trait FacetHostSeam: Send + Sync {
    fn reload(&self, facets: Vec<String>) -> BoxFuture<'_, Result<(), String>>;
    fn dispose(&self) -> BoxFuture<'_, Result<(), String>>;
}

/// Port of upstream `SessionWorkerServices`.
pub struct SessionWorkerServices {
    endpoints: Mutex<HashMap<String, Arc<dyn ServiceEndpoint>>>,
    endpoint_factory: Box<dyn Fn(WorkerServiceScope) -> Arc<dyn ServiceEndpoint> + Send + Sync>,
    reload_plugins: Mutex<ReloadState>,
}

struct ReloadState {
    loader: Arc<dyn PluginLoader>,
    host: Arc<dyn FacetHostSeam>,
    loaded_plugins: Arc<Mutex<Option<FacetGeneration>>>,
    /// Serializes reloads (upstream's `reloadTail` promise chain); the
    /// running reload owns the lock and later reloads queue behind it.
    tail_gate: Arc<tokio::sync::Mutex<()>>,
}

impl SessionWorkerServices {
    /// Port of upstream `createSessionWorkerServices`'s returned object.
    /// `builtins` (the static facet loader result) is aggregated into the
    /// host by the caller seam; `loaded_plugins` starts from the first
    /// `plugin_loader.load()` generation, which must be provided ready-made
    /// (upstream awaits `pluginLoader.load()` before creating the host).
    pub fn new(
        endpoint_factory: Box<dyn Fn(WorkerServiceScope) -> Arc<dyn ServiceEndpoint> + Send + Sync>,
        host: Arc<dyn FacetHostSeam>,
        plugin_loader: Arc<dyn PluginLoader>,
        initial_plugins: FacetGeneration,
    ) -> Self {
        let initial_plugins = Arc::new(Mutex::new(Some(initial_plugins)));
        SessionWorkerServices {
            endpoints: Mutex::new(HashMap::new()),
            endpoint_factory,
            reload_plugins: Mutex::new(ReloadState {
                loader: plugin_loader,
                host,
                loaded_plugins: initial_plugins,
                tail_gate: Arc::new(tokio::sync::Mutex::new(())),
            }),
        }
    }

    /// Upstream `invoke(call, scope, context)`: one endpoint per scope key.
    pub fn invoke(
        &self,
        call: crate::coding_agent::experimental::session_worker::ServiceCall,
        scope: &WorkerServiceScope,
    ) -> BoxFuture<'static, Result<Option<Value>, String>> {
        let key = service_scope_key(scope);
        let endpoint = {
            let mut endpoints = self.endpoints.lock().unwrap();
            Arc::clone(
                endpoints
                    .entry(key)
                    .or_insert_with(|| (self.endpoint_factory)(scope.clone())),
            )
        };
        let publish: Box<dyn FnOnce(String, Value) -> BoxFuture<'static, ()> + Send> =
            Box::new(|_subscription_id, _update| Box::pin(async {}));
        endpoint.invoke(call, publish)
    }

    /// Upstream `removeSubscriptions(matches)`.
    pub fn remove_subscriptions(&self, matches: impl Fn(&WorkerServiceScope) -> bool) {
        let mut endpoints = self.endpoints.lock().unwrap();
        let keys: Vec<String> = endpoints
            .iter()
            .filter(|(key, _)| {
                let scope = decode_scope_key(key);
                matches(&scope)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            if let Some(endpoint) = endpoints.remove(&key) {
                endpoint.dispose();
            }
        }
    }

    /// Upstream `reloadPlugins`: serialized through the reload tail so
    /// concurrent reloads never overlap. The returned future drives its own
    /// work once awaited (Rust futures are lazy, unlike the upstream promise
    /// chain, so serialization uses the tail gate lock).
    pub fn reload_plugins(&self) -> BoxFuture<'static, Result<(), String>> {
        let (tail_gate, loader, host, loaded_plugins) = {
            let state = self.reload_plugins.lock().unwrap();
            (
                Arc::clone(&state.tail_gate),
                Arc::clone(&state.loader),
                Arc::clone(&state.host),
                Arc::clone(&state.loaded_plugins),
            )
        };
        Box::pin(async move {
            let _gate = tail_gate.lock().await;
            let current = loaded_plugins.lock().unwrap().take();
            let (result, new_current) = reload_once(&*loader, &*host, current).await;
            *loaded_plugins.lock().unwrap() = new_current;
            result
        })
    }

    /// Upstream `dispose()`: drop every subscription, await the reload tail,
    /// then dispose the host and both facet generations, aggregating errors
    /// with the exact upstream message.
    pub async fn dispose(&self) -> Result<(), String> {
        self.remove_subscriptions(|_scope| true);
        // Upstream awaits the reload tail; the port acquires the tail gate so
        // any in-flight or queued reload finishes (or aborts) first.
        let tail_gate = Arc::clone(&self.reload_plugins.lock().unwrap().tail_gate);
        drop(tail_gate.lock().await);
        let (host, loaded_plugins) = {
            let state = self.reload_plugins.lock().unwrap();
            (Arc::clone(&state.host), Arc::clone(&state.loaded_plugins))
        };
        let mut errors: Vec<String> = Vec::new();
        if let Err(error) = host.dispose().await {
            errors.push(error);
        }
        let loaded = loaded_plugins.lock().unwrap().take();
        if let Some(loaded) = loaded {
            if let Err(error) = (loaded.dispose)().await {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            return Ok(());
        }
        if errors.len() == 1 {
            return Err(errors.remove(0));
        }
        Err(format!(
            "Failed to dispose Session facets: {}",
            errors.join("; ")
        ))
    }
}

async fn reload_once(
    loader: &dyn PluginLoader,
    host: &dyn FacetHostSeam,
    current: Option<FacetGeneration>,
) -> (Result<(), String>, Option<FacetGeneration>) {
    let candidate = match loader.load().await {
        Ok(candidate) => candidate,
        Err(error) => return (Err(error), current),
    };
    match host.reload(candidate.facets.clone()).await {
        Ok(()) => {
            if let Some(previous) = current {
                let _ = (previous.dispose)().await;
            }
            (Ok(()), Some(candidate))
        }
        Err(error) => {
            if let Err(cleanup_error) = (candidate.dispose)().await {
                return (
                    Err(format!(
                        "Session plugin reload and cleanup failed: {error}; {cleanup_error}"
                    )),
                    current,
                );
            }
            (Err(error), current)
        }
    }
}

/// Upstream `serviceScopeKey`.
pub fn service_scope_key(scope: &WorkerServiceScope) -> String {
    format!("{}\0{}", scope.server_connection_id, scope.attachment_id)
}

/// Inverse of [`service_scope_key`] (port-internal; upstream keeps the scope
/// struct on the endpoint entry instead).
fn decode_scope_key(key: &str) -> WorkerServiceScope {
    let mut parts = key.splitn(2, '\0');
    let server_connection_id = parts.next().unwrap_or_default().to_string();
    let attachment_id = parts.next().unwrap_or_default().to_string();
    WorkerServiceScope {
        server_connection_id,
        attachment_id,
    }
}

// // ── D6: real chord `FacetHost` wiring ──────────────────────────────────────
//
// Upstream `createSessionWorkerServices` builds its endpoints on the real
// chord `FacetHost` + `createRemoteServiceEndpoint`. This face ports that
// wiring over the real chord port (`crate::chord`): one host over the
// builtin + plugin facet generations, one remote-service endpoint per scope,
// the serialized reload tail, and the dispose aggregation with the exact
// upstream error strings.

use crate::chord::context::Context;
use crate::chord::facets::{
    create_facet_host, Facet, FacetHost, FacetLoader, FacetOptions, LoadedFacets,
};
use crate::chord::services::provider::{
    create_remote_service_endpoint, RemoteServiceEndpoint, RemoteServiceProvider,
};
use crate::chord::types::ServiceCall as ChordServiceCall;

/// The chord facet generation backing one [`ChordPluginLoaderSeam::load`].
pub struct ChordFacetGeneration {
    pub facets: Vec<Facet>,
    loaded: LoadedFacets,
}

impl ChordFacetGeneration {
    pub fn dispose(&mut self) -> Result<(), String> {
        self.loaded.dispose().map_err(|error| error.to_string())
    }
}

/// Upstream `pluginLoader.load()` seam over the real chord loader face.
pub trait ChordPluginLoaderSeam: Send + Sync {
    fn load(&self) -> Result<ChordFacetGeneration, String>;
}

impl<T: FacetLoader> ChordPluginLoaderSeam for T {
    fn load(&self) -> Result<ChordFacetGeneration, String> {
        let loaded = FacetLoader::load(self).map_err(|error| error.to_string())?;
        Ok(ChordFacetGeneration {
            facets: loaded.facets.clone(),
            loaded,
        })
    }
}

/// [`FacetHost`] behind a lock so the services wiring can share it.
pub struct ChordFacetHostAdapter {
    host: std::sync::Mutex<FacetHost>,
}

impl ChordFacetHostAdapter {
    /// Upstream `createFacetHost({ facets })`; error strings pass through.
    pub fn create(facets: Vec<Facet>) -> Result<Self, String> {
        let host = create_facet_host(FacetOptions {
            facets,
            service_sources: Vec::new(),
            on_error: None,
        })
        .map_err(|error| error.to_string())?;
        Ok(ChordFacetHostAdapter {
            host: std::sync::Mutex::new(host),
        })
    }

    /// Upstream `facetHost.services`.
    pub fn services(&self) -> Arc<RemoteServiceProvider> {
        Arc::clone(
            &self
                .host
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .services,
        )
    }

    /// Upstream `facetHost.reload(facets)`.
    pub fn reload(&self, facets: Vec<Facet>) -> Result<(), String> {
        self.host
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reload(facets)
            .map_err(|error| error.to_string())
    }

    /// Upstream `facetHost.dispose()`.
    pub fn dispose(&self) -> Result<(), String> {
        self.host
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .dispose()
            .map_err(|error| error.to_string())
    }
}

struct ScopedChordEndpoint {
    scope: WorkerServiceScope,
    endpoint: Mutex<RemoteServiceEndpoint>,
}

/// Upstream `publish(scope, subscriptionId, update)` callback face.
pub type ChordPublishFn = Arc<dyn Fn(&WorkerServiceScope, &str, Value) + Send + Sync>;

/// Real-chord port of upstream `SessionWorkerServices`.
pub struct ChordSessionWorkerServices {
    host: Arc<ChordFacetHostAdapter>,
    plugin_loader: Arc<dyn ChordPluginLoaderSeam>,
    endpoints: Mutex<HashMap<String, Arc<ScopedChordEndpoint>>>,
    reload: Mutex<ChordReloadState>,
}

struct ChordReloadState {
    loaded_plugins: Arc<Mutex<Option<ChordFacetGeneration>>>,
    tail_gate: Arc<tokio::sync::Mutex<()>>,
}

impl ChordSessionWorkerServices {
    #[cfg(test)]
    pub(crate) fn test_endpoint_count(&self) -> usize {
        self.endpoints.lock().unwrap().len()
    }

    /// Invokes the already-created endpoint for `scope` without the
    /// create-on-demand behavior of [`Self::invoke`] (test face: lets tests
    /// observe endpoint disposal through the chord
    /// `"Remote service endpoint is disposed"` error).
    #[cfg(test)]
    pub(crate) fn test_invoke_stored(
        &self,
        scope: &WorkerServiceScope,
        call: crate::coding_agent::experimental::session_worker::ServiceCall,
        publish: ChordPublishFn,
    ) -> Result<Option<Value>, String> {
        let key = service_scope_key(scope);
        let endpoint = self.endpoints.lock().unwrap().get(&key).cloned();
        let Some(endpoint) = endpoint else {
            return Err("no stored endpoint".to_string());
        };
        let Some(chord_call) =
            crate::coding_agent::experimental::session_worker::service_call_to_chord(&call)
        else {
            return Err("Service call is not a valid chord service call".to_string());
        };
        let publisher = endpoint_publisher(scope.clone(), publish);
        let endpoint = endpoint
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        endpoint
            .invoke(&chord_call, publisher, &Context::background())
            .map_err(|error| error.to_string())
    }
}

/// The chord update publisher handed to [`RemoteServiceEndpoint::invoke`],
/// bridging chord updates back to the experimental publish face.
type ChordEndpointPublisher =
    Arc<dyn Fn(&str, &crate::chord::types::ServiceProviderUpdate, &Context) + Send + Sync>;

fn endpoint_publisher(
    scope: WorkerServiceScope,
    publish: ChordPublishFn,
) -> ChordEndpointPublisher {
    Arc::new(
        move |subscription_id: &str,
              update: &crate::chord::types::ServiceProviderUpdate,
              _context: &Context| {
            publish(&scope, subscription_id, update.to_json());
        },
    )
}

impl ChordSessionWorkerServices {
    /// Upstream `createSessionWorkerServices`'s facet assembly: the builtin
    /// facets plus the first plugin generation go into one host. On host
    /// creation failure the plugin generation is disposed and the errors
    /// aggregate with the exact upstream `AggregateError` message; with no
    /// cleanup error the host error is re-thrown bare (upstream `throw
    /// error`). The builtin generation is owned by the host, so its
    /// failure-path disposal is chord-internal.
    pub fn create(
        builtins: Vec<Facet>,
        plugin_loader: Arc<dyn ChordPluginLoaderSeam>,
    ) -> Result<Arc<Self>, String> {
        let loaded = plugin_loader.load()?;
        let mut facets = builtins;
        facets.extend(loaded.facets.iter().cloned());
        let host = match ChordFacetHostAdapter::create(facets) {
            Ok(host) => host,
            Err(error) => {
                let mut loaded = loaded;
                return match loaded.dispose() {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(format!(
                        "Session facets failed to start and clean up: {error}; {cleanup}"
                    )),
                };
            }
        };
        Ok(Arc::new(ChordSessionWorkerServices {
            host: Arc::new(host),
            plugin_loader,
            endpoints: Mutex::new(HashMap::new()),
            reload: Mutex::new(ChordReloadState {
                loaded_plugins: Arc::new(Mutex::new(Some(loaded))),
                tail_gate: Arc::new(tokio::sync::Mutex::new(())),
            }),
        }))
    }

    /// Upstream `invoke(call, scope, context)` over
    /// `createRemoteServiceEndpoint(host.services)` per scope.
    pub fn invoke(
        self: &Arc<Self>,
        call: crate::coding_agent::experimental::session_worker::ServiceCall,
        scope: &WorkerServiceScope,
        publish: ChordPublishFn,
    ) -> BoxFuture<'static, Result<Option<Value>, String>> {
        let key = service_scope_key(scope);
        let endpoint = {
            let mut endpoints = self.endpoints.lock().unwrap();
            if let std::collections::hash_map::Entry::Vacant(entry) = endpoints.entry(key.clone()) {
                entry.insert(Arc::new(ScopedChordEndpoint {
                    scope: scope.clone(),
                    endpoint: Mutex::new(create_remote_service_endpoint(self.host.services())),
                }));
            }
            Arc::clone(endpoints.get(&key).expect("endpoint just inserted"))
        };
        let publish_scope = scope.clone();
        let chord_call: Option<ChordServiceCall> =
            crate::coding_agent::experimental::session_worker::service_call_to_chord(&call);
        Box::pin(async move {
            let Some(chord_call) = chord_call else {
                return Err("Service call is not a valid chord service call".to_string());
            };
            let publisher = endpoint_publisher(publish_scope, publish);
            let invoke: Result<
                Option<crate::chord::types::JsonValue>,
                crate::chord::services::errors::ChordError,
            > = {
                let endpoint = endpoint
                    .endpoint
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                endpoint.invoke(&chord_call, publisher, &Context::background())
            };
            match invoke {
                Ok(value) => Ok(value),
                Err(error) => Err(error.to_string()),
            }
        })
    }

    /// Upstream `removeSubscriptions(matches)`.
    pub fn remove_subscriptions(&self, matches: impl Fn(&WorkerServiceScope) -> bool) {
        let mut endpoints = self.endpoints.lock().unwrap();
        let keys: Vec<String> = endpoints
            .iter()
            .filter(|(_, entry)| matches(&entry.scope))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            if let Some(entry) = endpoints.remove(&key) {
                entry
                    .endpoint
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .dispose();
            }
        }
    }

    /// Upstream `reloadPlugins` closure: a reload serialized behind the tail
    /// gate over the real chord host.
    pub fn reload_plugins(self: &Arc<Self>) -> BoxFuture<'static, Result<(), String>> {
        let services = Arc::clone(self);
        Box::pin(async move {
            let tail_gate = Arc::clone(&services.reload.lock().unwrap().tail_gate);
            let _gate = tail_gate.lock().await;
            let loaded_plugins = Arc::clone(&services.reload.lock().unwrap().loaded_plugins);
            let mut candidate = services.plugin_loader.load()?;
            if let Err(error) = services.host.reload(candidate.facets.clone()) {
                if let Err(cleanup) = candidate.dispose() {
                    return Err(format!(
                        "Session plugin reload and cleanup failed: {error}; {cleanup}"
                    ));
                }
                return Err(error);
            }
            let mut current = loaded_plugins.lock().unwrap();
            if let Some(mut retired) = current.take() {
                let _ = retired.dispose();
            }
            *current = Some(candidate);
            Ok(())
        })
    }

    /// Upstream `dispose()`: drop subscriptions, await the reload tail, then
    /// dispose the host and the plugin generation, aggregating errors with
    /// the exact upstream message.
    pub async fn dispose(&self) -> Result<(), String> {
        self.remove_subscriptions(|_scope| true);
        let tail_gate = Arc::clone(&self.reload.lock().unwrap().tail_gate);
        drop(tail_gate.lock().await);
        let mut errors: Vec<String> = Vec::new();
        if let Err(error) = self.host.dispose() {
            errors.push(error);
        }
        let loaded = self
            .reload
            .lock()
            .unwrap()
            .loaded_plugins
            .lock()
            .unwrap()
            .take();
        if let Some(mut loaded) = loaded {
            if let Err(error) = loaded.dispose() {
                errors.push(error);
            }
        }
        match errors.len() {
            0 => Ok(()),
            1 => Err(errors.remove(0)),
            _ => Err(format!(
                "Failed to dispose Session facets: {}",
                errors.join("; ")
            )),
        }
    }
}

/// Public read for the host adapter (test face mirrors upstream's direct
/// `facetHost.services` access).
pub fn chord_host_services(host: &ChordFacetHostAdapter) -> Arc<RemoteServiceProvider> {
    host.services()
}

#[cfg(test)]
mod tests;
