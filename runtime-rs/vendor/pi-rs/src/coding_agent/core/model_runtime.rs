//! Port of upstream `coding-agent/src/core/model-runtime.ts`: the configured
//! pi-ai [`Models`](crate::ai::models::Models) collection used by
//! coding-agent and SDK consumers — composed providers
//! ([`super::provider_composer`]), credential synchronization with a locally
//! consistent availability snapshot, runtime API keys, and request-time
//! authentication.
//!
//! Behavior seams (upstream imports outside this slice):
//!
//! - **ai-layer surfaces vendored here.** The ported `pi-ai` layer does not
//!   yet expose `Models.checkAuth`/`login`/`logout` (its module docs defer
//!   them to the consumer task) nor `setupErrorMessage`/`reduceStream`/
//!   `mergeHeaders` (pub(crate)); the port vendors faithful copies against
//!   the existing [`CredentialStore`]/[`ProviderAuth`] types. `getAvailable`
//!   over one provider and the availability pass read the providers'
//!   own trait surface (credential read + `check` + `filterModels`), the
//!   same pipeline the ai collection runs.
//! - **auth-storage.ts** (default credential store): not ported; the default
//!   is the vendored [`AuthFileStore`] (same `Record<string, Credential>`
//!   auth.json wire format, per-provider write serialization; the document
//!   rewrite sorts keys where the JS object preserved insertion order —
//!   disclosed). The upstream `FileAuthStorage` proper-lockfile protocol is
//!   not reproduced here.
//! - **remote-catalog-provider.ts** (`withRemoteCatalog`): not ported; the
//!   create-time provider list uses the built-in factories as-is. Only
//!   dynamic-catalog freshness metadata is affected (network refresh is off
//!   by default for this slice's surface).
//! - **Sync reads become async.** Upstream `getModels`/`getProvider`/… read
//!   the JS collection synchronously; the port guards the collection with a
//!   tokio mutex (concurrent mutation + long-running refresh), so those
//!   reads are `async fn`. `getAvailableSnapshot`/`getError`/
//!   `hasConfiguredAuth`/`getProviderAuthStatus`/`isUsingOAuth` and the
//!   registered-provider reads stay synchronous (pure snapshot/state reads).
//! - **`cancelDeferred`** has no port-side transport surface (the ai-layer
//!   deferred port dropped `cancelDeferred`); disclosed as not ported.
//! - **Fire-and-forget refreshes** (`void this.refresh(…)`) become
//!   `tokio::spawn`ed tasks.
//! - **Cancel-during-store-mutation**: upstream keeps the committed
//!   mutation running and reports a `CredentialSynchronizationError`
//!   afterwards; the port awaits the mutation the same way, but the store's
//!   own cancellation contract (no write after cancel) can drop the write
//!   itself. The [`CredentialSynchronizationError`] wrapping of post-commit
//!   synchronization failures is preserved verbatim.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ai::auth::context::default_provider_auth_context;
use crate::ai::auth::credential_store::{CredentialStore, ModifyCallback};
use crate::ai::auth::resolve::{AuthResolutionOverrides, ModelsError, ModelsErrorCode};
use crate::ai::auth::types::{
    ApiKeyAuthInput, AuthCheck, AuthContext, AuthError, AuthInteraction, AuthOperationOptions,
    AuthResult, AuthType, Credential, CredentialInfo, ProviderAuthInteraction,
};
use crate::ai::model_operations::assert_chat_model;
use crate::ai::models::store::ModelsStore;
use crate::ai::models::{
    create_models, CreateModelsOptions, Models, ModelsApiStreamOptions, ModelsRefreshOptions,
    ModelsRefreshResult, ModelsSimpleStreamOptions, Provider,
};
use crate::ai::transcript::{normalize_context, TranscriptContext};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, PartialAssistant};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::options::ProviderHeaders;
use crate::ai::types::primitives::{ModelThinkingLevel, StopReason, ThinkingLevel, Usage};
use crate::ai::types::{AnyModel, Model};
use crate::ai::{now_ms, Context, ProviderConfig};

use super::model_config::ModelConfig;
use super::models_store::{FileModelsStore, InMemoryCodingAgentModelsStore};
use super::provider_composer::{
    compose_model_provider, configured_request_auth_status, resolve_compatibility_request_config,
    resolve_configured_model_headers, validate_extension_provider, AuthStatus, AuthStatusSource,
    CompatibilityRequestConfig, ProviderConfigInput,
};
use super::resolve_config_value::ConfigEnv;
use super::runtime_credentials::RuntimeCredentials;
use super::virtual_models::{
    is_virtual_model, with_virtual_models, FailedRoute, ModelRoute, ModelRouteReason,
    ModelRouteRequest, PreviousRoute, RouteFn, VirtualModelDefinition,
};

/// One provider's availability computation result
/// (`auth`, `credential`, filtered models).
type ProviderAvailability = (Option<AuthCheck>, Option<Credential>, Vec<Model>);

/// One provider's availability check outcome in the full pass
/// (provider id, read credential, auth check).
type ProviderCheck = (String, Option<Credential>, Option<AuthCheck>);

/// Upstream `ModelRuntimeSnapshot`.
#[derive(Clone, Default)]
struct ModelRuntimeSnapshot {
    all: Vec<Model>,
    available: Vec<Model>,
    configured_providers: HashSet<String>,
    stored_providers: HashSet<String>,
    auth: HashMap<String, Option<AuthCheck>>,
}

/// Upstream `CreateModelRuntimeOptions`.
#[derive(Default)]
pub struct CreateModelRuntimeOptions {
    /// Credential storage. Defaults to the file at `auth_path`
    /// (`~/.pi/agent/auth.json` when unset).
    pub credentials: Option<Arc<dyn CredentialStore>>,
    pub auth_path: Option<String>,
    /// `None` → the default path (`getAgentDir()/models.json`);
    /// `Some(None)` → upstream `modelsPath: null` (no models config);
    /// `Some(Some(path))` → an explicit path.
    pub models_path: Option<Option<String>>,
    pub models_store: Option<Arc<dyn ModelsStore>>,
    pub models_store_path: Option<String>,
    /// Allow [`ModelRuntime::create`] to refresh model catalogs over the
    /// network. Defaults to false.
    pub allow_model_network: bool,
    /// Timeout for the create-time network model refresh.
    pub model_refresh_timeout_ms: Option<u64>,
    /// Remote catalog base URL (remote-catalog seam — retained for option
    /// parity, unused without the catalog provider).
    pub catalog_base_url: Option<String>,
    /// Caller cancellation for initial cache restoration and availability
    /// checks.
    pub signal: Option<CancellationToken>,
    /// Skip initial catalog and availability refresh. Static models remain
    /// available. Defaults to true upstream (`refreshOnCreate !== false`).
    pub refresh_on_create: Option<bool>,
}

/// Upstream `ModelRuntimeAuthOverrides extends AuthOperationOptions` — the
/// port reuses the ai-layer override shape (same fields).
pub type ModelRuntimeAuthOverrides = AuthResolutionOverrides;

/// Upstream `CredentialSynchronizationOperation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSynchronizationOperation {
    Login,
    Logout,
    SetRuntimeApiKey,
    RemoveRuntimeApiKey,
}

impl CredentialSynchronizationOperation {
    /// The upstream literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialSynchronizationOperation::Login => "login",
            CredentialSynchronizationOperation::Logout => "logout",
            CredentialSynchronizationOperation::SetRuntimeApiKey => "setRuntimeApiKey",
            CredentialSynchronizationOperation::RemoveRuntimeApiKey => "removeRuntimeApiKey",
        }
    }
}

/// Upstream `CredentialSynchronizationError`: credentials changed
/// successfully, but the local model/auth snapshot could not be synchronized.
#[derive(Debug, Clone)]
pub struct CredentialSynchronizationError {
    pub provider_id: String,
    pub operation: CredentialSynchronizationOperation,
    pub credential: Option<Credential>,
    /// The underlying failure (upstream `error.cause`), rendered through its
    /// `Display`.
    pub cause: String,
}

impl CredentialSynchronizationError {
    /// Upstream `error.message`.
    pub fn message(&self) -> String {
        format!(
            "Credential {} committed for {}, but local synchronization failed",
            self.operation.as_str(),
            self.provider_id
        )
    }
}

impl std::fmt::Display for CredentialSynchronizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for CredentialSynchronizationError {}

/// A registration/reload failure (upstream throws synchronously).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRegistrationError(pub String);

impl std::fmt::Display for ProviderRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderRegistrationError {}

/// The shared runtime state behind the cloneable [`ModelRuntime`] handle.
/// Upstream `RegisteredVirtualModel`: a virtual model's catalog entry plus
/// its router.
struct RegisteredVirtualModel {
    model: Model,
    route: RouteFn,
}

/// Virtual models by provider id, then model id (insertion-ordered).
type VirtualModelRegistry = Vec<(String, Vec<(String, RegisteredVirtualModel)>)>;

struct ModelRuntimeInner {
    // Models owns a shared registry; no outer mutex may span provider callbacks.
    models: Models,
    credentials: Arc<RuntimeCredentials>,
    auth_context: Arc<dyn AuthContext>,
    default_builtins: Vec<(String, Arc<dyn Provider>)>,
    builtins: Mutex<Vec<(String, Arc<dyn Provider>)>>,
    native_extension_providers: Mutex<Vec<(String, Arc<dyn Provider>)>>,
    extension_providers: Mutex<Vec<(String, ProviderConfigInput)>>,
    /// Virtual models by provider id, then model id (insertion-ordered).
    virtual_models: Mutex<VirtualModelRegistry>,
    composition_errors: Mutex<Vec<(String, String)>>,
    models_path: Option<String>,
    model_network_enabled: bool,
    config: Mutex<ModelConfig>,
    snapshot: Mutex<ModelRuntimeSnapshot>,
    availability_refresh_seq: AtomicU64,
    availability_error_seq: AtomicU64,
    provider_availability_seq: Mutex<HashMap<String, u64>>,
    availability_error: Mutex<Option<String>>,
    credential_operations: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Upstream `ModelRuntime` (the port hands out cloneable `Arc` handles).
#[derive(Clone)]
pub struct ModelRuntime(Arc<ModelRuntimeInner>);

/// Upstream `resolveModel`'s options object.
#[derive(Debug)]
pub struct ResolveModelOptions {
    /// Why the request is being routed.
    pub reason: ModelRouteReason,
    /// The selected thinking level; its meaning is up to the router.
    pub thinking_level: ModelThinkingLevel,
    /// Caller cancellation for routing itself (e.g. a router calling models).
    pub signal: Option<CancellationToken>,
    /// Failed response the next request repeats; `messages` no longer
    /// contains it.
    pub failed: Option<AssistantMessage>,
    /// Router state stored by the caller on this session branch.
    pub state: Option<serde_json::Value>,
}

impl ModelRuntime {
    /// Upstream `ModelRuntime.create`.
    pub async fn create(options: CreateModelRuntimeOptions) -> Result<Self, String> {
        let credentials: Arc<dyn CredentialStore> = match options.credentials {
            Some(credentials) => credentials,
            None => Arc::new(AuthFileStore::new(match options.auth_path {
                Some(auth_path) => auth_path,
                None => super::path_join(&super::get_agent_dir(), "auth.json"),
            })),
        };
        let credentials = Arc::new(RuntimeCredentials::new(credentials));
        let models_path: Option<String> = match options.models_path {
            Some(None) => None,
            Some(Some(path)) => Some(path),
            None => Some(super::path_join(&super::get_agent_dir(), "models.json")),
        };
        let config = ModelConfig::load(models_path.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        let models_store: Arc<dyn ModelsStore> = match options.models_store {
            Some(store) => store,
            None => match &models_path {
                Some(models_path) => {
                    let store_path = options.models_store_path.unwrap_or_else(|| {
                        super::path_join(&dirname(models_path), "models-store.json")
                    });
                    Arc::new(FileModelsStore::new(store_path))
                }
                None => Arc::new(InMemoryCodingAgentModelsStore::default()),
            },
        };
        // withRemoteCatalog wrapping (upstream create) rides the
        // remote-catalog seam — built-in factories are used as-is.
        let providers = crate::ai::models::providers::builtin_providers();
        let model_network_enabled = std::env::var("PI_OFFLINE").is_err();
        let auth_context: Arc<dyn AuthContext> = Arc::new(default_provider_auth_context());
        let models = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            models_store: Some(Arc::clone(&models_store)),
            auth_context: Some(Arc::clone(&auth_context)),
        });
        let inner = Arc::new(ModelRuntimeInner {
            models,
            credentials,
            auth_context,
            default_builtins: providers
                .into_iter()
                .map(|provider| (provider.id().to_string(), provider))
                .collect(),
            builtins: Mutex::new(Vec::new()),
            native_extension_providers: Mutex::new(Vec::new()),
            extension_providers: Mutex::new(Vec::new()),
            virtual_models: Mutex::new(Vec::new()),
            composition_errors: Mutex::new(Vec::new()),
            models_path,
            model_network_enabled,
            config: Mutex::new(config),
            snapshot: Mutex::new(ModelRuntimeSnapshot::default()),
            availability_refresh_seq: AtomicU64::new(0),
            availability_error_seq: AtomicU64::new(0),
            provider_availability_seq: Mutex::new(HashMap::new()),
            availability_error: Mutex::new(None),
            credential_operations: Mutex::new(HashMap::new()),
        });
        let runtime = ModelRuntime(inner);
        runtime.configure_radius_providers();
        runtime.rebuild_providers();
        let refresh_from_network = runtime.0.model_network_enabled && options.allow_model_network;
        if options.refresh_on_create.unwrap_or(true) {
            // Upstream wires a modelRefreshTimeoutMs abort controller; the
            // port layers the same child token.
            let signal = options.signal.clone().map(|signal| {
                match options
                    .model_refresh_timeout_ms
                    .filter(|_| refresh_from_network)
                {
                    Some(timeout_ms) => {
                        let combined = signal.child_token();
                        let timeout_token = combined.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
                            timeout_token.cancel();
                        });
                        combined
                    }
                    None => signal,
                }
            });
            let _ = runtime
                .refresh(ModelsRefreshOptions {
                    allow_network: Some(refresh_from_network),
                    signal,
                    ..ModelsRefreshOptions::default()
                })
                .await;
        }
        Ok(runtime)
    }

    /// Upstream `configureRadiusProviders`: rebuild `builtins` from the
    /// defaults plus one radius provider per `oauth: "radius"` models.json
    /// entry with a baseUrl.
    fn configure_radius_providers(&self) {
        let mut builtins = lock(&self.0.builtins);
        builtins.clear();
        for (provider_id, provider) in &self.0.default_builtins {
            builtins.push((provider_id.clone(), Arc::clone(provider)));
        }
        let config = lock(&self.0.config);
        for provider_id in config.get_provider_ids() {
            let Some(entry) = config.get_provider(provider_id) else {
                continue;
            };
            if entry.oauth.as_deref() != Some("radius") {
                continue;
            }
            let Some(base_url) = &entry.base_url else {
                continue;
            };
            builtins.push((
                provider_id.to_string(),
                crate::ai::models::providers::radius_provider(
                    crate::ai::models::providers::RadiusProviderOptions {
                        id: Some(provider_id.to_string()),
                        name: Some(
                            entry
                                .name
                                .clone()
                                .unwrap_or_else(|| provider_id.to_string()),
                        ),
                        gateway: Some(strip_trailing_v1(base_url)),
                    },
                ),
            ));
        }
    }

    /// Upstream `providerIds()`: insertion-ordered union of the four sources.
    fn provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let builtin_ids: Vec<String> = lock(&self.0.builtins)
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let native_ids: Vec<String> = lock(&self.0.native_extension_providers)
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let config_ids: Vec<String> = lock(&self.0.config)
            .get_provider_ids()
            .into_iter()
            .map(String::from)
            .collect();
        let extension_ids: Vec<String> = lock(&self.0.extension_providers)
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        let virtual_ids: Vec<String> = lock(&self.0.virtual_models)
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        for source in [
            builtin_ids,
            native_ids,
            config_ids,
            extension_ids,
            virtual_ids,
        ] {
            for id in source {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    fn models(&self) -> Models {
        self.0.models.clone()
    }

    /// Upstream `recomposeProvider`: compose the provider, wrap it with the
    /// provider's virtual models, and set/delete it on `models`. Returns the
    /// provider without virtual models, or `None` when only virtual models
    /// define it (or nothing does).
    fn recompose_with(&self, provider_id: &str, models: &mut Models) -> Option<Arc<dyn Provider>> {
        let provider = self.compose_provider(provider_id);
        let virtual_models: Vec<Model> = self.virtual_models_of(provider_id);
        if !virtual_models.is_empty() {
            models.set_provider(with_virtual_models(
                provider_id,
                provider.clone(),
                virtual_models,
            ));
        } else if let Some(provider) = &provider {
            models.set_provider(Arc::clone(provider));
        } else {
            models.delete_provider(provider_id);
        }
        provider
    }

    /// The provider's registered virtual-model catalog entries (insertion
    /// order).
    fn virtual_models_of(&self, provider_id: &str) -> Vec<Model> {
        lock(&self.0.virtual_models)
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, models)| {
                models
                    .iter()
                    .map(|(_, entry)| entry.model.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Upstream `composeProvider`: the provider without virtual models, or
    /// `None` when nothing defines it.
    fn compose_provider(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        let base = {
            let natives = lock(&self.0.native_extension_providers);
            natives
                .iter()
                .find(|(id, _)| id == provider_id)
                .map(|(_, provider)| Arc::clone(provider))
                .or_else(|| {
                    lock(&self.0.builtins)
                        .iter()
                        .find(|(id, _)| id == provider_id)
                        .map(|(_, provider)| Arc::clone(provider))
                })
        };
        let extension = lock(&self.0.extension_providers)
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, config)| config.clone());
        let config_provider = lock(&self.0.config).get_provider(provider_id).cloned();
        if config_provider.is_none() && extension.is_none() {
            // No overlays: use the builtin untouched so its auth/login
            // behavior is exact.
            lock(&self.0.composition_errors).retain(|(id, _)| id != provider_id);
            return base;
        }
        match compose_model_provider(
            provider_id,
            base.clone(),
            config_provider.clone(),
            extension.clone(),
        ) {
            Ok(provider) => {
                lock(&self.0.composition_errors).retain(|(id, _)| id != provider_id);
                Some(provider)
            }
            Err(message) => {
                lock(&self.0.composition_errors).retain(|(id, _)| id != provider_id);
                lock(&self.0.composition_errors).push((provider_id.to_string(), message));
                base
            }
        }
    }

    /// Upstream `rebuildProviders`.
    fn rebuild_providers(&self) {
        let provider_ids = self.provider_ids();
        {
            let mut models = self.models();
            models.clear_providers();
            lock(&self.0.composition_errors).clear();
            for provider_id in &provider_ids {
                self.recompose_with(provider_id, &mut models);
            }
        }
        self.update_model_snapshot();
    }

    /// Upstream `updateModelSnapshot`.
    fn update_model_snapshot(&self) {
        let all = self.models().get_models(None);
        self.set_snapshot(move |snapshot| {
            snapshot.all = all;
            snapshot.available = snapshot
                .all
                .iter()
                .filter(|model| snapshot.configured_providers.contains(&model.provider))
                .cloned()
                .collect();
        });
    }

    fn set_snapshot(&self, update: impl FnOnce(&mut ModelRuntimeSnapshot)) {
        let mut snapshot = lock(&self.0.snapshot);
        update(&mut snapshot);
    }

    fn snapshot(&self) -> ModelRuntimeSnapshot {
        lock(&self.0.snapshot).clone()
    }

    /// Upstream `runAvailabilityRefresh`: one full availability pass. The
    /// per-provider pipeline runs against the providers' own trait surface
    /// (credential read → `check` → `getModels` → `filterModels`), the same
    /// flow as the ai collection's `getAvailable`.
    async fn run_availability_refresh(
        &self,
        seq: u64,
        error_seq: u64,
        signal: CancellationToken,
    ) -> Result<(), AuthError> {
        let options = AuthOperationOptions::new(signal.clone());
        let providers = {
            let models = self.models();
            models.get_providers()
        };
        let checks: Vec<Result<ProviderCheck, AuthError>> =
            futures::future::join_all(providers.iter().map(|provider| {
                let credentials = Arc::clone(&self.0.credentials);
                let auth_context = Arc::clone(&self.0.auth_context);
                let options = options.clone();
                async move {
                    let credential = crate::ai::auth::resolve::read_credential(
                        credentials.as_ref(),
                        provider.id(),
                        &options,
                    )
                    .await?;
                    let auth = check_provider_auth(
                        provider.as_ref(),
                        credential.as_ref(),
                        credentials.as_ref(),
                        auth_context.as_ref(),
                        &options,
                    )
                    .await?;
                    Ok((provider.id().to_string(), credential, auth))
                }
            }))
            .await;
        let credentials = self.0.credentials.list(&options).await;

        let checks = checks.into_iter().collect::<Result<Vec<_>, AuthError>>()?;
        let credentials = credentials?;
        if seq != self.0.availability_refresh_seq.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut auth: HashMap<String, Option<AuthCheck>> = HashMap::new();
        let mut configured_providers: HashSet<String> = HashSet::new();
        let mut available: Vec<Model> = Vec::new();
        for (provider_id, credential, check) in &checks {
            auth.insert(provider_id.clone(), check.clone());
            if check.is_some() {
                configured_providers.insert(provider_id.clone());
                // `provider.getModels()` + `filterModels` (models.ts:544-551).
                let provider = providers
                    .iter()
                    .find(|provider| provider.id() == *provider_id)
                    .expect("provider from the same list");
                let models = provider.get_models().map_err(AuthError::Models)?;
                if let Some(filtered) = provider.filter_models(&models, credential.as_ref()) {
                    available.extend(filtered);
                } else {
                    available.extend(models);
                }
            }
        }
        let stored_providers: HashSet<String> = credentials
            .iter()
            .map(|entry| entry.provider_id.clone())
            .collect();
        let all = self.models().get_models(None);
        self.set_snapshot(|_snapshot| {
            *_snapshot = ModelRuntimeSnapshot {
                all,
                available,
                configured_providers,
                stored_providers,
                auth,
            };
        });
        if error_seq == self.0.availability_error_seq.load(Ordering::SeqCst) {
            *lock(&self.0.availability_error) = None;
        }
        Ok(())
    }

    /// Upstream `queueAvailabilityRefresh`.
    async fn queue_availability_refresh(
        &self,
        signal: Option<CancellationToken>,
    ) -> Result<(), AuthError> {
        let seq = self
            .0
            .availability_refresh_seq
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        {
            let mut provider_seqs = lock(&self.0.provider_availability_seq);
            for provider_seq in provider_seqs.values_mut() {
                *provider_seq += 1;
            }
        }
        let error_seq = self.0.availability_error_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let effective_signal = signal.unwrap_or_default();
        let result = self
            .run_availability_refresh(seq, error_seq, effective_signal.clone())
            .await;
        if let Err(error) = &result {
            if error_seq == self.0.availability_error_seq.load(Ordering::SeqCst)
                && !effective_signal.is_cancelled()
            {
                *lock(&self.0.availability_error) = Some(error.to_string());
            }
        }
        result
    }

    /// Upstream `refreshProviderAvailability`.
    async fn refresh_provider_availability(
        &self,
        provider_id: &str,
        signal: CancellationToken,
    ) -> Result<(), AuthError> {
        // Invalidate any full availability pass that started before this
        // credential change.
        self.0
            .availability_refresh_seq
            .fetch_add(1, Ordering::SeqCst);
        let provider_seq = {
            let mut provider_seqs = lock(&self.0.provider_availability_seq);
            let entry = provider_seqs.entry(provider_id.to_string()).or_insert(0);
            *entry += 1;
            *entry
        };
        let error_seq = self.0.availability_error_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let outcome = async {
            let options = AuthOperationOptions::new(signal.clone());
            let provider = {
                let models = self.models();
                models.get_provider(provider_id)
            };
            let (auth, credential, available) = match &provider {
                Some(provider) => {
                    let credential = crate::ai::auth::resolve::read_credential(
                        self.0.credentials.as_ref(),
                        provider_id,
                        &options,
                    )
                    .await?;
                    let auth = check_provider_auth(
                        provider.as_ref(),
                        credential.as_ref(),
                        self.0.credentials.as_ref(),
                        self.0.auth_context.as_ref(),
                        &options,
                    )
                    .await?;
                    let models = if auth.is_some() {
                        provider.get_models().map_err(AuthError::Models)?
                    } else {
                        Vec::new()
                    };
                    let filtered = if auth.is_some() {
                        provider
                            .filter_models(&models, credential.as_ref())
                            .unwrap_or(models)
                    } else {
                        Vec::new()
                    };
                    (auth, credential, filtered)
                }
                None => (None, None, Vec::new()),
            };
            ensure_live(&signal)?;
            {
                let current = lock(&self.0.provider_availability_seq);
                if current.get(provider_id) != Some(&provider_seq) {
                    // Superseded: skip the snapshot update without an error.
                    return Ok(None);
                }
            }
            Ok(Some((auth, credential, available)))
        };
        let outcome: Result<Option<ProviderAvailability>, AuthError> = outcome.await;
        match outcome {
            Ok(Some((auth, credential, available))) => {
                self.set_snapshot(|snapshot| {
                    if auth.is_some() {
                        snapshot
                            .configured_providers
                            .insert(provider_id.to_string());
                        snapshot.auth.insert(provider_id.to_string(), auth.clone());
                    } else {
                        snapshot.configured_providers.remove(provider_id);
                        snapshot.auth.remove(provider_id);
                    }
                    if credential.is_some() {
                        snapshot.stored_providers.insert(provider_id.to_string());
                    } else {
                        snapshot.stored_providers.remove(provider_id);
                    }
                    let available_by_id: HashMap<String, Model> = snapshot
                        .available
                        .iter()
                        .filter(|model| model.provider != provider_id)
                        .map(|model| (format!("{}\0{}", model.provider, model.id), model.clone()))
                        .chain(available.iter().map(|model| {
                            (format!("{}\0{}", model.provider, model.id), model.clone())
                        }))
                        .collect();
                    snapshot.available = snapshot
                        .all
                        .iter()
                        .filter_map(|model| {
                            available_by_id
                                .get(&format!("{}\0{}", model.provider, model.id))
                                .cloned()
                        })
                        .collect();
                });
                if error_seq == self.0.availability_error_seq.load(Ordering::SeqCst) {
                    *lock(&self.0.availability_error) = None;
                }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => {
                let current_seq = lock(&self.0.provider_availability_seq)
                    .get(provider_id)
                    .copied();
                if current_seq == Some(provider_seq)
                    && error_seq == self.0.availability_error_seq.load(Ordering::SeqCst)
                    && !signal.is_cancelled()
                {
                    *lock(&self.0.availability_error) = Some(error.to_string());
                }
                Err(error)
            }
        }
    }

    // ------------------------------------------------------------------
    // Collection reads
    // ------------------------------------------------------------------

    /// Upstream `getProviders`.
    pub async fn get_providers(&self) -> Vec<Arc<dyn Provider>> {
        self.models().get_providers()
    }

    /// Upstream `getProvider`.
    pub async fn get_provider(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        self.models().get_provider(provider_id)
    }

    /// Upstream `getModels`.
    pub async fn get_models(&self, provider_id: Option<&str>) -> Vec<Model> {
        self.models().get_models(provider_id)
    }

    /// Upstream `getModel`.
    pub async fn get_model(&self, provider_id: &str, model_id: &str) -> Option<Model> {
        self.models().get_model(provider_id, model_id)
    }

    /// Synchronous catalog read over the shared collection (upstream
    /// `getModel` is synchronous; the port's other reads became async for the
    /// collection mutex, which this read does not need).
    pub fn get_model_sync(&self, provider_id: &str, model_id: &str) -> Option<Model> {
        self.models().get_model(provider_id, model_id)
    }

    /// Upstream `checkAuth` (vendored ai-layer surface — see the module
    /// docs).
    pub async fn check_auth(
        &self,
        provider_id: &str,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        let options = options.cloned().unwrap_or_default();
        options.check()?;
        let provider = {
            let models = self.models();
            models.get_provider(provider_id)
        };
        let Some(provider) = provider else {
            return Ok(None);
        };
        let credential = crate::ai::auth::resolve::read_credential(
            self.0.credentials.as_ref(),
            provider_id,
            &options,
        )
        .await?;
        check_provider_auth(
            provider.as_ref(),
            credential.as_ref(),
            self.0.credentials.as_ref(),
            self.0.auth_context.as_ref(),
            &options,
        )
        .await
    }

    /// Upstream `getAvailable`.
    pub async fn get_available(
        &self,
        provider_id: Option<&str>,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<Model>, AuthError> {
        if let Some(provider_id) = provider_id {
            let error_seq = self.0.availability_error_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let result = {
                let models = self.models();
                models.get_available(Some(provider_id), options).await
            };
            return match result {
                Ok(available) => {
                    if error_seq == self.0.availability_error_seq.load(Ordering::SeqCst) {
                        *lock(&self.0.availability_error) = None;
                    }
                    Ok(available)
                }
                Err(error) => {
                    let aborted = options
                        .and_then(|options| options.signal.as_ref())
                        .is_some_and(|signal| signal.is_cancelled());
                    if error_seq == self.0.availability_error_seq.load(Ordering::SeqCst) && !aborted
                    {
                        *lock(&self.0.availability_error) = Some(error.to_string());
                    }
                    Err(error)
                }
            };
        }
        self.queue_availability_refresh(options.and_then(|options| options.signal.clone()))
            .await?;
        Ok(self.snapshot().available)
    }

    /// Upstream `getAvailableSnapshot` (pure snapshot read, synchronous).
    pub fn get_available_snapshot(&self) -> Vec<Model> {
        self.snapshot().available
    }

    /// Upstream `getError` (synchronous state read).
    pub fn get_error(&self) -> Option<String> {
        let mut errors: Vec<String> = Vec::new();
        if let Some(config_error) = lock(&self.0.config).get_error() {
            errors.push(config_error.to_string());
        }
        for (provider_id, error) in lock(&self.0.composition_errors).iter() {
            errors.push(format!("Provider \"{provider_id}\": {error}"));
        }
        if let Some(availability_error) = lock(&self.0.availability_error).as_ref() {
            errors.push(format!("Availability refresh: {availability_error}"));
        }
        if errors.is_empty() {
            None
        } else {
            Some(errors.join("\n\n"))
        }
    }

    /// Upstream `getRegisteredProviderConfig`.
    pub fn get_registered_provider_config(&self, provider_id: &str) -> Option<ProviderConfigInput> {
        lock(&self.0.extension_providers)
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, config)| config.clone())
    }

    /// Upstream `getRegisteredProviderIds` (insertion order over the union).
    pub fn get_registered_provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for (id, _) in lock(&self.0.extension_providers).iter() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        for (id, _) in lock(&self.0.native_extension_providers).iter() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids
    }

    /// Upstream `getRegisteredNativeProvider`.
    pub fn get_registered_native_provider(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        lock(&self.0.native_extension_providers)
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, provider)| Arc::clone(provider))
    }

    /// Upstream `getCompatibilityRequestConfig` (compatibility fallback for
    /// the registry when provider auth is unconfigured).
    pub fn get_compatibility_request_config(
        &self,
        model: &Model,
    ) -> Result<CompatibilityRequestConfig, String> {
        let config = lock(&self.0.config).get_provider(&model.provider).cloned();
        let extension = lock(&self.0.extension_providers)
            .iter()
            .find(|(id, _)| id == &model.provider)
            .map(|(_, config)| config.clone());
        resolve_compatibility_request_config(model, config.as_ref(), extension.as_ref())
    }

    /// Upstream `isUsingOAuth`.
    pub fn is_using_oauth(&self, provider_id: &str) -> bool {
        self.snapshot()
            .auth
            .get(provider_id)
            .cloned()
            .flatten()
            .is_some_and(|check| check.r#type == AuthType::OAuth)
    }

    /// Upstream `isUsingSubscription`.
    pub async fn is_using_subscription(&self, provider_id: &str) -> bool {
        let subscription = self
            .models()
            .get_provider(provider_id)
            .and_then(|provider| {
                provider
                    .auth()
                    .oauth
                    .as_ref()
                    .map(|oauth| oauth.is_subscription())
            });
        self.is_using_oauth(provider_id) && subscription == Some(true)
    }

    /// Upstream `hasConfiguredAuth`.
    pub fn has_configured_auth(&self, provider_id: &str) -> bool {
        self.snapshot().configured_providers.contains(provider_id)
    }

    /// Upstream `getAuth(providerId | model, overrides)`.
    pub async fn get_auth(
        &self,
        provider_or_model: ProviderOrModel<'_>,
        overrides: Option<&ModelRuntimeAuthOverrides>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let resolution = match provider_or_model {
            ProviderOrModel::Provider(provider_id) => {
                let models = self.models();
                models.get_auth(provider_id, overrides).await?
            }
            ProviderOrModel::Model(model) => {
                let models = self.models();
                models.get_auth_for_model(model, overrides).await?
            }
        };
        let Some(mut resolution) = resolution else {
            return Ok(None);
        };
        let model = match provider_or_model {
            ProviderOrModel::Model(model) => model,
            ProviderOrModel::Provider(_) => return Ok(Some(resolution)),
        };
        // Layer the composed models.json/extension headers over the resolved
        // auth headers (upstream `resolveConfiguredModelHeaders` +
        // `mergeHeaders`).
        let mut configured_env: ConfigEnv = ConfigEnv::new();
        if let Some(resolution_env) = &resolution.env {
            for (name, value) in resolution_env {
                configured_env.insert(name.clone(), value.clone());
            }
        }
        if let Some(overrides_env) = overrides.and_then(|overrides| overrides.env.as_ref()) {
            for (name, value) in overrides_env {
                configured_env.insert(name.clone(), value.clone());
            }
        }
        let config = lock(&self.0.config).get_provider(&model.provider).cloned();
        let extension = lock(&self.0.extension_providers)
            .iter()
            .find(|(id, _)| id == &model.provider)
            .map(|(_, config)| config.clone());
        let configured_headers = resolve_configured_model_headers(
            model,
            config.as_ref(),
            extension.as_ref(),
            Some(&configured_env),
        )
        .map_err(AuthError::Operation)?;
        let configured_headers = configured_headers.map(|headers| {
            headers
                .into_iter()
                .map(|(name, value)| (name, Some(value)))
                .collect::<ProviderHeaders>()
        });
        resolution.auth.headers = merge_headers(
            resolution.auth.headers.as_ref(),
            configured_headers.as_ref(),
        );
        Ok(Some(resolution))
    }

    // ------------------------------------------------------------------
    // Credential operations
    // ------------------------------------------------------------------

    /// Upstream `enqueueCredentialOperation`: same-provider credential
    /// operations serialize; a cancelled signal rejects before the task
    /// starts (upstream `signal.throwIfAborted()` after queueing).
    async fn enqueue_credential_operation<T>(
        &self,
        provider_id: &str,
        signal: &CancellationToken,
        task: impl std::future::Future<Output = Result<T, AuthError>> + Send,
    ) -> Result<T, AuthError> {
        let lock = lock(&self.0.credential_operations)
            .entry(provider_id.to_string())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        ensure_live(signal)?;
        task.await
    }

    /// Upstream `synchronizeCredentialState`.
    // The error intentionally carries the full credential payload (upstream
    // shape), so `clippy::result_large_err` is accepted here rather than
    // boxing the variant and changing the public API.
    #[allow(clippy::result_large_err)]
    async fn synchronize_credential_state(
        &self,
        provider_id: &str,
        operation: CredentialSynchronizationOperation,
        credential: Option<Credential>,
        signal: &CancellationToken,
    ) -> Result<(), CredentialSynchronizationError> {
        let outcome = async {
            if let Err(error) = ensure_live(signal) {
                return Err(error.to_string());
            }
            {
                let mut models = self.models();
                self.recompose_with(provider_id, &mut models);
            }
            let composition_error = lock(&self.0.composition_errors)
                .iter()
                .find(|(id, _)| id == provider_id)
                .map(|(_, error)| error.clone());
            if let Some(composition_error) = composition_error {
                return Err(composition_error);
            }
            let result = {
                let models = self.models();
                models
                    .refresh(ModelsRefreshOptions {
                        allow_network: Some(false),
                        providers: Some(vec![provider_id.to_string()]),
                        signal: Some(signal.clone()),
                        ..ModelsRefreshOptions::default()
                    })
                    .await
            };
            if result.aborted {
                return Err("aborted".to_string());
            }
            if let Some(refresh_error) = result.errors.get(provider_id) {
                return Err(refresh_error.message.clone());
            }
            self.update_model_snapshot();
            self.refresh_provider_availability(provider_id, signal.clone())
                .await
                .map_err(|error| match error {
                    AuthError::Models(models_error) => models_error.message,
                    other => other.to_string(),
                })?;
            Ok(())
        };
        outcome
            .await
            .map_err(|cause| CredentialSynchronizationError {
                provider_id: provider_id.to_string(),
                operation,
                credential,
                cause,
            })
    }

    /// Upstream `setRuntimeApiKey`.
    pub async fn set_runtime_api_key(
        &self,
        provider_id: &str,
        api_key: &str,
        options: Option<&AuthOperationOptions>,
    ) -> Result<(), AuthError> {
        let signal = options
            .and_then(|options| options.signal.clone())
            .unwrap_or_default();
        let credentials = Arc::clone(&self.0.credentials);
        let api_key = api_key.to_string();
        let this = self.clone();
        let task_provider_id = provider_id.to_string();
        let task = {
            let provider_id = task_provider_id.clone();
            let signal = signal.clone();
            async move {
                credentials.set_runtime_api_key(&provider_id, &api_key);
                this.synchronize_credential_state(
                    &provider_id,
                    CredentialSynchronizationOperation::SetRuntimeApiKey,
                    Some(Credential::ApiKey(
                        crate::ai::auth::types::ApiKeyCredential {
                            key: Some(api_key.clone()),
                            ..Default::default()
                        },
                    )),
                    &signal,
                )
                .await
                .map_err(|error| AuthError::Operation(error.message()))
            }
        };
        self.enqueue_credential_operation(provider_id, &signal, task)
            .await
    }

    /// Upstream `removeRuntimeApiKey`.
    pub async fn remove_runtime_api_key(
        &self,
        provider_id: &str,
        options: Option<&AuthOperationOptions>,
    ) -> Result<(), AuthError> {
        let signal = options
            .and_then(|options| options.signal.clone())
            .unwrap_or_default();
        let credentials = Arc::clone(&self.0.credentials);
        let this = self.clone();
        let task = {
            let provider_id = provider_id.to_string();
            let signal = signal.clone();
            async move {
                credentials.remove_runtime_api_key(&provider_id);
                this.synchronize_credential_state(
                    &provider_id,
                    CredentialSynchronizationOperation::RemoveRuntimeApiKey,
                    None,
                    &signal,
                )
                .await
                .map_err(|error| AuthError::Operation(error.message()))
            }
        };
        self.enqueue_credential_operation(provider_id, &signal, task)
            .await
    }

    /// Upstream `listCredentials`.
    pub async fn list_credentials(
        &self,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<CredentialInfo>, AuthError> {
        self.0
            .credentials
            .list(options.unwrap_or(&AuthOperationOptions::NONE))
            .await
    }

    /// Upstream `getProviderAuthStatus` (synchronous state read).
    pub fn get_provider_auth_status(&self, provider_id: &str) -> AuthStatus {
        if self.0.credentials.has_runtime_api_key(provider_id) {
            return AuthStatus::configured(AuthStatusSource::Runtime);
        }
        let snapshot = self.snapshot();
        if snapshot.stored_providers.contains(provider_id) {
            return AuthStatus::configured(AuthStatusSource::Stored);
        }
        let configured = {
            let config = lock(&self.0.config).get_provider(provider_id).cloned();
            let extension = lock(&self.0.extension_providers)
                .iter()
                .find(|(id, _)| id == provider_id)
                .map(|(_, config)| config.clone());
            configured_request_auth_status(config.as_ref(), extension.as_ref())
        };
        if let Some(configured) = configured {
            return configured;
        }
        if let Some(Some(check)) = snapshot.auth.get(provider_id) {
            return AuthStatus {
                configured: true,
                source: Some(AuthStatusSource::Environment),
                label: check.source.clone(),
            };
        }
        AuthStatus {
            configured: false,
            source: None,
            label: None,
        }
    }

    // ------------------------------------------------------------------
    // Request preparation and dispatch
    // ------------------------------------------------------------------

    /// Upstream `prepareRequest`: resolve provider + request-time auth,
    /// merge headers, and derive the request model/options. The composed
    /// models.json/extension headers ride through [`ModelRuntime::get_auth`].
    #[allow(clippy::type_complexity)]
    async fn prepare_request(
        &self,
        model: &Model,
        options_api_key: Option<String>,
        options_env: Option<crate::ai::types::options::ProviderEnv>,
        options_headers: Option<&ProviderHeaders>,
        signal: Option<CancellationToken>,
        transform_headers: Option<crate::ai::models::TransformHeaders>,
    ) -> Result<
        (
            Arc<dyn Provider>,
            Model,
            Option<String>,
            Option<ProviderHeaders>,
            Option<crate::ai::types::options::ProviderEnv>,
        ),
        String,
    > {
        let provider = self.models().get_provider(&model.provider).ok_or_else(|| {
            ModelsError::new(
                ModelsErrorCode::Provider,
                format!("Unknown provider: {}", model.provider),
            )
            .message
        })?;
        let overrides = AuthResolutionOverrides {
            api_key: options_api_key.clone(),
            env: options_env.clone(),
            signal,
            ..AuthResolutionOverrides::default()
        };
        let resolution = self
            .get_auth(ProviderOrModel::Model(model), Some(&overrides))
            .await
            .map_err(|error| match error {
                AuthError::Models(models_error) => models_error.message,
                other => other.to_string(),
            })?
            .ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Auth,
                    format!("Provider is not configured: {}", model.provider),
                )
                .message
            })?;
        let mut headers = merge_headers(resolution.auth.headers.as_ref(), options_headers);
        if let Some(transform_headers) = &transform_headers {
            headers = Some(transform_headers(headers.unwrap_or_default()).await);
        }
        let env = match (resolution.env.as_ref(), options_env.as_ref()) {
            (None, None) => None,
            (resolved, explicit) => {
                let mut merged = resolved.cloned().unwrap_or_default();
                merged.extend(explicit.cloned().unwrap_or_default());
                Some(merged)
            }
        };
        let request_model = match &resolution.auth.base_url {
            Some(base_url) => {
                let mut request_model = model.clone();
                request_model.base_url = base_url.clone();
                request_model
            }
            None => model.clone(),
        };
        let api_key = options_api_key.or(resolution.auth.api_key.clone());
        Ok((provider, request_model, api_key, headers, env))
    }

    /// Upstream `stream`: lazily prepare the request, then dispatch through
    /// the provider's API implementation; setup failures settle the stream
    /// with an error event (the port's `lazyStream` convention).
    pub fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let options = options.unwrap_or_default();
        let options_api_key = options.stream.api_key.clone();
        let options_env = options.stream.env.clone();
        let options_headers = options.stream.headers.clone();
        let signal = options.stream.signal.clone();
        let transform_headers = options.transform_headers.clone();
        let stream_options = options.stream;
        let this = self.clone();
        let model = model.clone();
        lazy_stream(&model, context, move |model, transcript| async move {
            let (provider, request_model, api_key, headers, env) = this
                .prepare_request(
                    &model,
                    options_api_key,
                    options_env,
                    options_headers.as_ref(),
                    signal,
                    transform_headers,
                )
                .await?;
            let implementation = api_for_checked(provider.as_ref(), &model)?;
            let mut stream_options = stream_options;
            stream_options.api_key = api_key;
            stream_options.headers = headers;
            stream_options.env = env;
            let config = ProviderConfig {
                base_url: request_model.base_url.clone(),
                api_key: stream_options.api_key.clone().unwrap_or_default(),
                max_tokens: request_model.max_tokens,
            };
            Ok(implementation.stream(&config, &request_model, &transcript, &stream_options))
        })
    }

    /// Upstream `complete`.
    pub async fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> AssistantMessage {
        reduce_stream(self.stream(model, context, options), model).await
    }

    /// Upstream `streamSimple`.
    pub fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsSimpleStreamOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let options = options.unwrap_or_default();
        if is_virtual_model(model) {
            // Requests outside the agent loop are routed here. Callers sized
            // them before routing, so cap the output budget to the routed
            // model.
            let this = self.clone();
            let model = model.clone();
            let context = context.clone();
            return lazy_stream(&model, &context, move |model, transcript| async move {
                let simple_reasoning = model_thinking_level_from_level(options.simple.reasoning)
                    .unwrap_or(ModelThinkingLevel::Off);
                let route = this
                    .resolve_model(
                        &model,
                        transcript.messages(),
                        ResolveModelOptions {
                            reason: ModelRouteReason::Direct,
                            thinking_level: simple_reasoning,
                            signal: options.simple.stream.signal.clone(),
                            failed: None,
                            state: None,
                        },
                    )
                    .await?;
                let limit = route.model.max_tokens;
                let max_tokens = match options.simple.stream.max_tokens {
                    // `maxTokens && limit > 0` (0 is falsy upstream).
                    Some(max_tokens) if max_tokens > 0 && limit > 0 => Some(max_tokens.min(limit)),
                    max_tokens => max_tokens,
                };
                // Caller credentials were resolved for the virtual model's
                // provider. Another provider resolves its own, so they are
                // not sent to the wrong vendor.
                let same_provider = route.model.provider == model.provider;
                let mut routed_simple = options.simple.clone();
                if !same_provider {
                    routed_simple.stream.api_key = None;
                    routed_simple.stream.headers = None;
                    routed_simple.stream.env = None;
                }
                routed_simple.stream.max_tokens = max_tokens;
                routed_simple.reasoning = model_thinking_level_to_level(route.thinking_level);
                this.stream_simple_dispatch(
                    &route.model,
                    transcript,
                    routed_simple,
                    options.transform_headers.clone(),
                )
                .await
            });
        }
        let simple_options = options.simple;
        let transform_headers = options.transform_headers;
        let this = self.clone();
        let model = model.clone();
        lazy_stream(&model, context, move |model, transcript| async move {
            this.stream_simple_dispatch(&model, transcript, simple_options, transform_headers)
                .await
        })
    }

    /// The `streamSimple` dispatch: assert the chat model, prepare auth and
    /// the request model, then hand the request to the provider's
    /// implementation.
    async fn stream_simple_dispatch(
        &self,
        model: &Model,
        transcript: TranscriptContext,
        mut simple_options: crate::ai::types::options::SimpleStreamOptions,
        transform_headers: Option<crate::ai::models::TransformHeaders>,
    ) -> Result<mpsc::Receiver<AssistantMessageEvent>, String> {
        assert_chat_model(&AnyModel::Chat(model.clone())).map_err(|error| error.message)?;
        let (provider, request_model, api_key, headers, env) = self
            .prepare_request(
                model,
                simple_options.stream.api_key.clone(),
                simple_options.stream.env.clone(),
                simple_options.stream.headers.as_ref(),
                simple_options.stream.signal.clone(),
                transform_headers,
            )
            .await?;
        let implementation = api_for_checked(provider.as_ref(), model)?;
        simple_options.stream.api_key = api_key;
        simple_options.stream.headers = headers;
        simple_options.stream.env = env;
        let config = ProviderConfig {
            base_url: request_model.base_url.clone(),
            api_key: simple_options.stream.api_key.clone().unwrap_or_default(),
            max_tokens: request_model.max_tokens,
        };
        Ok(implementation.stream_simple(&config, &request_model, &transcript, &simple_options))
    }

    /// Upstream `completeSimple`.
    pub async fn complete_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsSimpleStreamOptions>,
    ) -> AssistantMessage {
        reduce_stream(self.stream_simple(model, context, options), model).await
    }

    /// Upstream `streamDeferred` (the transport-only deferred surface of the
    /// ported ai layer).
    pub fn stream_deferred(
        &self,
        model: &Model,
        handle: &crate::ai::types::options::DeferredHandle,
        context: &Context,
        options: Option<crate::ai::models::ModelsDeferredFetchOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let options = options.unwrap_or_default();
        let options_api_key = options.stream.api_key.clone();
        let options_env = options.stream.env.clone();
        let options_headers = options.stream.headers.clone();
        let signal = options.stream.signal.clone();
        let transform_headers = options.transform_headers.clone();
        let stream_options = options.stream;
        let handle = handle.clone();
        let this = self.clone();
        let model = model.clone();
        lazy_stream(&model, context, move |model, _transcript| async move {
            let (provider, request_model, api_key, headers, env) = this
                .prepare_request(
                    &model,
                    options_api_key,
                    options_env,
                    options_headers.as_ref(),
                    signal,
                    transform_headers,
                )
                .await?;
            let implementation = api_for_checked(provider.as_ref(), &model)?;
            let mut stream_options = stream_options;
            stream_options.api_key = api_key;
            stream_options.headers = headers;
            stream_options.env = env;
            let config = ProviderConfig {
                base_url: request_model.base_url.clone(),
                api_key: stream_options.api_key.clone().unwrap_or_default(),
                max_tokens: request_model.max_tokens,
            };
            Ok(implementation.stream_deferred(&config, &request_model, &handle, &stream_options))
        })
    }

    /// Upstream `fetchDeferred`.
    pub async fn fetch_deferred(
        &self,
        model: &Model,
        handle: &crate::ai::types::options::DeferredHandle,
        context: &Context,
        options: Option<crate::ai::models::ModelsDeferredFetchOptions>,
    ) -> AssistantMessage {
        reduce_stream(self.stream_deferred(model, handle, context, options), model).await
    }

    // ------------------------------------------------------------------
    // Login / logout (vendored ai-layer surface)
    // ------------------------------------------------------------------

    /// Upstream `login` (`Models.login` vendored — see the module docs).
    pub async fn login(
        &self,
        provider_id: &str,
        auth_type: AuthType,
        interaction: Arc<dyn AuthInteraction>,
    ) -> Result<Credential, AuthError> {
        let signal = interaction.signal().unwrap_or_default();
        ensure_live(&signal)?;
        let provider = {
            let models = self.models();
            models.get_provider(provider_id)
        };
        let Some(provider) = provider else {
            return Err(AuthError::Models(ModelsError::new(
                ModelsErrorCode::Provider,
                format!("Unknown provider: {provider_id}"),
            )));
        };
        let auth = provider.auth();
        let options = AuthOperationOptions::new(signal.clone());
        let credential = match auth_type {
            AuthType::OAuth => match auth.oauth.as_ref() {
                Some(oauth) => {
                    let interaction =
                        ProviderAuthInteraction::new(Arc::clone(&interaction), signal.clone());
                    let login = oauth.login(interaction);
                    Credential::OAuth(race_with_token(login, &signal).await?)
                }
                None => {
                    return Err(AuthError::Models(ModelsError::new(
                        ModelsErrorCode::Auth,
                        format!("{} does not support oauth login", provider.name()),
                    )))
                }
            },
            AuthType::ApiKey => {
                match auth.api_key.as_ref().and_then(|api_key| {
                    api_key.login(ProviderAuthInteraction::new(
                        Arc::clone(&interaction),
                        signal.clone(),
                    ))
                }) {
                    Some(login) => Credential::ApiKey(race_with_token(login, &signal).await?),
                    None => {
                        return Err(AuthError::Models(ModelsError::new(
                            ModelsErrorCode::Auth,
                            format!("{} does not support api_key login", provider.name()),
                        )))
                    }
                }
            }
        };
        // Store the credential (upstream `credentials.modify` with the
        // started/mutation race; see the module docs for the disclosed
        // cancel-during-mutation settlement difference).
        let stored = credential.clone();
        let callback: ModifyCallback =
            Box::new(move |_current| Box::pin(async move { Ok(Some(stored)) }));
        let mutation = self.0.credentials.modify(provider_id, callback, &options);
        let stored = match race_with_token(mutation, &signal).await {
            Ok(_) => {
                ensure_live(&signal)?;
                credential
            }
            Err(error) => {
                ensure_live(&signal)?;
                return Err(AuthError::Models(ModelsError::with_cause(
                    ModelsErrorCode::Auth,
                    format!("Credential store modify failed for {provider_id}"),
                    error,
                )));
            }
        };
        self.synchronize_credential_state(
            provider_id,
            CredentialSynchronizationOperation::Login,
            Some(stored.clone()),
            &signal,
        )
        .await
        .map_err(|error| AuthError::Operation(error.message()))?;
        Ok(stored)
    }

    /// Upstream `logout` (`Models.logout` vendored — see the module docs).
    pub async fn logout(
        &self,
        provider_id: &str,
        options: Option<&AuthOperationOptions>,
    ) -> Result<(), AuthError> {
        let options = options.cloned().unwrap_or_default();
        options.check()?;
        let signal = options.signal.clone().unwrap_or_default();
        let deletion = self.0.credentials.delete(provider_id, &options);
        match race_with_token(deletion, &signal).await {
            Ok(()) => ensure_live(&signal)?,
            Err(error) => {
                ensure_live(&signal)?;
                return Err(AuthError::Models(ModelsError::with_cause(
                    ModelsErrorCode::Auth,
                    format!("Credential store delete failed for {provider_id}"),
                    error,
                )));
            }
        }
        self.synchronize_credential_state(
            provider_id,
            CredentialSynchronizationOperation::Logout,
            None,
            &signal,
        )
        .await
        .map_err(|error| AuthError::Operation(error.message()))
    }

    // ------------------------------------------------------------------
    // Refresh and provider lifecycle
    // ------------------------------------------------------------------

    /// Upstream `refresh`.
    pub async fn refresh(
        &self,
        options: ModelsRefreshOptions,
    ) -> Result<ModelsRefreshResult, String> {
        let config = ModelConfig::load(self.0.models_path.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        *lock(&self.0.config) = config;
        self.configure_radius_providers();
        let selected_providers = options.providers.as_ref().map(|providers| {
            let mut unique: Vec<String> = Vec::new();
            for provider in providers {
                if !unique.contains(provider) {
                    unique.push(provider.clone());
                }
            }
            unique
        });
        if let Some(providers) = &selected_providers {
            {
                let mut models = self.models();
                for provider_id in providers {
                    self.recompose_with(provider_id, &mut models);
                }
            }
            self.update_model_snapshot();
        } else {
            self.rebuild_providers();
        }
        let refresh_options = ModelsRefreshOptions {
            allow_network: Some(
                options
                    .allow_network
                    .unwrap_or(self.0.model_network_enabled),
            ),
            providers: options.providers.clone(),
            force: options.force,
            signal: options.signal.clone(),
        };
        let (result, all_models) = {
            let models = self.models();
            let result = models.refresh(refresh_options).await;
            let all_models = models.get_models(None);
            (result, all_models)
        };
        let mut errors = result.errors.clone();
        self.set_snapshot(|snapshot| {
            snapshot.all = all_models;
            snapshot.available = snapshot
                .all
                .iter()
                .filter(|model| snapshot.configured_providers.contains(&model.provider))
                .cloned()
                .collect();
        });
        if let Some(providers) = &selected_providers {
            for provider_id in providers {
                if let Err(error) = self
                    .refresh_provider_availability(
                        provider_id,
                        options.signal.clone().unwrap_or_default(),
                    )
                    .await
                {
                    let aborted = options
                        .signal
                        .as_ref()
                        .is_some_and(|signal| signal.is_cancelled());
                    if !aborted {
                        errors.insert(
                            provider_id.clone(),
                            match error {
                                AuthError::Models(models_error) => models_error,
                                other => ModelsError::new(ModelsErrorCode::Auth, other.to_string()),
                            },
                        );
                    }
                }
            }
        } else if self
            .queue_availability_refresh(options.signal.clone())
            .await
            .is_err()
        {
            // Availability errors are recorded by the latest pass; refreshed
            // models remain usable.
        }
        Ok(ModelsRefreshResult {
            aborted: result.aborted
                || options
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.is_cancelled()),
            errors,
        })
    }

    /// Upstream `registerNativeProvider`.
    pub async fn register_native_provider(
        &self,
        provider: Arc<dyn Provider>,
    ) -> Result<(), ProviderRegistrationError> {
        self.register_native_provider_sync(provider)
    }

    /// Synchronous mutation, matching upstream; only the follow-up refresh is asynchronous.
    pub fn register_native_provider_sync(
        &self,
        provider: Arc<dyn Provider>,
    ) -> Result<(), ProviderRegistrationError> {
        let id = provider.id().to_string();
        if id.trim().is_empty() {
            return Err(ProviderRegistrationError(
                "Provider id must not be empty.".to_string(),
            ));
        }
        lock(&self.0.extension_providers).retain(|(existing, _)| existing != &id);
        let auth_type = if provider.auth().oauth.is_some() && provider.auth().api_key.is_none() {
            AuthType::OAuth
        } else {
            AuthType::ApiKey
        };
        {
            let mut natives = lock(&self.0.native_extension_providers);
            match natives.iter_mut().find(|(existing, _)| existing == &id) {
                Some((_, existing)) => *existing = provider,
                None => natives.push((id.clone(), provider)),
            }
        }
        {
            let mut models = self.models();
            self.recompose_with(&id, &mut models);
        }
        self.update_model_snapshot();
        let configured_status = {
            let config = lock(&self.0.config).get_provider(&id).cloned();
            configured_request_auth_status(config.as_ref(), None)
        };
        self.mark_provisionally_configured(&id, configured_status, auth_type);
        self.spawn_background_refresh();
        Ok(())
    }

    /// Upstream `markProvisionallyConfigured`: mark a newly registered
    /// provider as configured when it has a stored credential or a configured
    /// API key. Availability checks run asynchronously, and callers such as
    /// initial model selection read the snapshot before they finish. The next
    /// availability pass replaces this entry.
    fn mark_provisionally_configured(
        &self,
        provider_id: &str,
        configured_status: Option<AuthStatus>,
        auth_type: AuthType,
    ) {
        let should_mark = !self.snapshot().stored_providers.contains(provider_id)
            && !configured_status.is_some_and(|status| status.configured);
        if should_mark {
            return;
        }
        self.set_snapshot(|snapshot| {
            snapshot
                .configured_providers
                .insert(provider_id.to_string());
            // Never clobber a real check result.
            snapshot
                .auth
                .entry(provider_id.to_string())
                .or_insert(Some(AuthCheck {
                    r#type: auth_type,
                    source: Some("configured provider".to_string()),
                }));
            snapshot.available = snapshot
                .all
                .iter()
                .filter(|model| snapshot.configured_providers.contains(&model.provider))
                .cloned()
                .collect();
        });
    }

    /// Upstream `registerProvider(providerId, config)` (the by-name form).
    pub async fn register_provider(
        &self,
        provider_id: &str,
        config: ProviderConfigInput,
    ) -> Result<(), ProviderRegistrationError> {
        self.register_provider_sync(provider_id, config)
    }

    /// Synchronous mutation, matching upstream; only the follow-up refresh is asynchronous.
    pub fn register_provider_sync(
        &self,
        provider_id: &str,
        config: ProviderConfigInput,
    ) -> Result<(), ProviderRegistrationError> {
        let builtin = lock(&self.0.builtins)
            .iter()
            .find(|(id, _)| id == provider_id)
            .map(|(_, provider)| Arc::clone(provider));
        let models_config = lock(&self.0.config).get_provider(provider_id).cloned();
        // Validate the incoming registration on its own: a broken
        // re-registration must fail without touching the stored config.
        validate_extension_provider(
            provider_id,
            builtin.as_ref(),
            models_config.as_ref(),
            &config,
        )
        .map_err(ProviderRegistrationError)?;
        lock(&self.0.native_extension_providers).retain(|(id, _)| id != provider_id);
        // Re-registration merges defined values over the previous
        // registration and preserves undefined ones (the legacy
        // ModelRegistry contract).
        let previous = self.get_registered_provider_config(provider_id);
        let effective = merge_provider_config_inputs(previous.as_ref(), &config);
        {
            let mut extensions = lock(&self.0.extension_providers);
            match extensions.iter_mut().find(|(id, _)| id == provider_id) {
                Some((_, existing)) => *existing = effective.clone(),
                None => extensions.push((provider_id.to_string(), effective.clone())),
            }
        }
        {
            let mut models = self.models();
            self.recompose_with(provider_id, &mut models);
        }
        self.update_model_snapshot();
        let configured_status =
            configured_request_auth_status(models_config.as_ref(), Some(&effective));
        let auth_type = if effective.oauth.is_some() && effective.api_key.is_none() {
            AuthType::OAuth
        } else {
            AuthType::ApiKey
        };
        self.mark_provisionally_configured(provider_id, configured_status, auth_type);
        self.spawn_background_refresh();
        Ok(())
    }

    /// Upstream `unregisterProvider`.
    pub async fn unregister_provider(&self, provider_id: &str) {
        self.unregister_provider_sync(provider_id);
    }

    /// Synchronous removal used by native extension callbacks.
    pub fn unregister_provider_sync(&self, provider_id: &str) {
        lock(&self.0.extension_providers).retain(|(id, _)| id != provider_id);
        lock(&self.0.native_extension_providers).retain(|(id, _)| id != provider_id);
        {
            let mut models = self.models();
            self.recompose_with(provider_id, &mut models);
        }
        self.update_model_snapshot();
        self.spawn_background_refresh();
    }

    // ------------------------------------------------------------------
    // Virtual models
    // ------------------------------------------------------------------

    /// Upstream `registerVirtualModel`: register a virtual model under
    /// `definition.provider`, which may also list physical models or several
    /// virtual models. Re-registering the same provider and id replaces the
    /// virtual model. Errors when the id belongs to a physical model of that
    /// provider.
    pub fn register_virtual_model(
        &self,
        definition: &VirtualModelDefinition,
    ) -> Result<(), ProviderRegistrationError> {
        let provider_id = definition.provider.as_str();
        let id = definition.id.as_str();
        if provider_id.trim().is_empty() || id.trim().is_empty() {
            return Err(ProviderRegistrationError(
                "Virtual model provider and id must not be empty.".to_string(),
            ));
        }
        let existing = self.models().get_model(provider_id, id);
        if existing.is_some_and(|model| !is_virtual_model(&model)) {
            return Err(ProviderRegistrationError(format!(
                "Virtual model {provider_id}/{id} conflicts with a physical model."
            )));
        }
        let entry = RegisteredVirtualModel {
            model: super::virtual_models::create_virtual_model(
                &super::virtual_models::CreateVirtualModelOptions {
                    provider: definition.provider.clone(),
                    id: definition.id.clone(),
                    name: definition.name.clone(),
                    thinking_levels: definition.thinking_levels.clone(),
                    context_window: definition.context_window,
                    max_tokens: definition.max_tokens,
                    input: definition.input.clone(),
                },
            ),
            route: Arc::clone(&definition.route),
        };
        {
            let mut registry = lock(&self.0.virtual_models);
            let models = match registry.iter_mut().find(|(pid, _)| pid == provider_id) {
                Some((_, models)) => models,
                None => {
                    registry.push((provider_id.to_string(), Vec::new()));
                    &mut registry.last_mut().expect("pushed above").1
                }
            };
            match models.iter_mut().find(|(mid, _)| mid == id) {
                Some((_, existing)) => *existing = entry,
                None => models.push((id.to_string(), entry)),
            }
        }
        let composed = {
            let mut models = self.models();
            self.recompose_with(provider_id, &mut models)
        };
        if composed.is_none() && !self.snapshot().configured_providers.contains(provider_id) {
            // A provider of only virtual models needs no credentials. Mark it
            // configured now: session restore checks auth before the refresh
            // below lands.
            self.set_snapshot(|snapshot| {
                snapshot.auth.insert(
                    provider_id.to_string(),
                    Some(AuthCheck {
                        r#type: AuthType::ApiKey,
                        source: Some("virtual".to_string()),
                    }),
                );
                snapshot
                    .configured_providers
                    .insert(provider_id.to_string());
            });
        }
        self.update_model_snapshot();
        self.spawn_background_refresh();
        Ok(())
    }

    /// Upstream `unregisterVirtualModel`.
    pub fn unregister_virtual_model(&self, provider_id: &str, id: &str) {
        {
            let mut registry = lock(&self.0.virtual_models);
            let Some((_, models)) = registry.iter_mut().find(|(pid, _)| pid == provider_id) else {
                return;
            };
            let Some(position) = models.iter().position(|(mid, _)| mid == id) else {
                return;
            };
            models.remove(position);
            if models.is_empty() {
                let position = registry
                    .iter()
                    .position(|(pid, _)| pid == provider_id)
                    .expect("provider still present");
                registry.remove(position);
            }
        }
        {
            let mut models = self.models();
            self.recompose_with(provider_id, &mut models);
        }
        self.update_model_snapshot();
        self.spawn_background_refresh();
    }

    /// Upstream `resolveModel`: ask a virtual model's router for the model and
    /// thinking level of one request. The router must return a physical
    /// catalog model whose provider has credentials; the thinking level is
    /// clamped to that model. Errors when routing fails.
    ///
    /// `previous` reports the latest successful response in `messages`. A
    /// retry passes the failed response as `options.failed`; `messages` no
    /// longer contains it. `options.state` is the router state stored by the
    /// caller, which also stores the returned state.
    pub async fn resolve_model(
        &self,
        model: &Model,
        messages: &[crate::ai::types::Message],
        options: ResolveModelOptions,
    ) -> Result<ModelRoute, String> {
        let name = format!("Virtual model {}/{}", model.provider, model.id);
        let route = {
            let registry = lock(&self.0.virtual_models);
            registry
                .iter()
                .find(|(pid, _)| pid == model.provider.as_str())
                .and_then(|(_, models)| models.iter().find(|(mid, _)| mid == model.id.as_str()))
                .map(|(_, entry)| Arc::clone(&entry.route))
        };
        let Some(route) = route else {
            return Err(format!("{name} is not registered."));
        };
        let ResolveModelOptions {
            reason,
            thinking_level,
            signal,
            failed,
            state,
        } = options;
        let latest = find_latest_message_response(messages);
        let previous_model =
            latest.and_then(|latest| self.get_physical_model(&latest.provider, &latest.model));
        // A failed routing attempt names the virtual model; there is no
        // physical request to report.
        let failed_model = failed
            .as_ref()
            .and_then(|failed| self.get_physical_model(&failed.provider, &failed.model));
        let request = ModelRouteRequest {
            model: model.clone(),
            thinking_level,
            reason,
            previous: previous_model.map(|model| PreviousRoute {
                model,
                thinking_level: latest.and_then(message_thinking_level),
            }),
            failed: match (failed_model, failed) {
                (Some(model), Some(failed)) => Some(FailedRoute {
                    model,
                    thinking_level: message_thinking_level(&failed),
                    message: failed,
                }),
                _ => None,
            },
            state,
            messages: messages.to_vec(),
            signal,
        };
        let route = route(request).await?;
        let target = self.get_physical_model(&route.model.provider, &route.model.id);
        let routed = format!(
            "{name} routed to {}/{}",
            route.model.provider, route.model.id
        );
        let Some(target) = target else {
            return Err(format!("{routed}, which is not a physical model."));
        };
        if !self.has_configured_auth(&target.provider) {
            return Err(format!("{routed}, which has no credentials."));
        }
        Ok(ModelRoute {
            thinking_level: clamp_model_thinking_level(&target, route.thinking_level),
            state: route.state,
            model: target,
        })
    }

    /// Upstream `getPhysicalModel`: a catalog chat model that is not virtual.
    pub fn get_physical_model(&self, provider_id: &str, model_id: &str) -> Option<Model> {
        self.models()
            .get_model(provider_id, model_id)
            .filter(|model| !is_virtual_model(model))
    }

    // ------------------------------------------------------------------
    // Any-type catalog surfaces
    // ------------------------------------------------------------------

    /// Upstream `getModelsOfType`.
    pub fn get_models_of_type(
        &self,
        model_type: crate::ai::ModelType,
        provider_id: Option<&str>,
    ) -> Vec<AnyModel> {
        self.models().get_models_of_type(model_type, provider_id)
    }

    /// Upstream `getModelOfType`.
    pub fn get_model_of_type(
        &self,
        model_type: crate::ai::ModelType,
        provider_id: &str,
        model_id: &str,
    ) -> Option<AnyModel> {
        self.models()
            .get_model_of_type(model_type, provider_id, model_id)
    }

    /// Upstream `getAllModels`.
    pub fn get_all_models(&self, provider_id: Option<&str>) -> Vec<AnyModel> {
        self.models().get_all_models(provider_id)
    }

    /// Upstream `getAvailableOfType`.
    pub async fn get_available_of_type(
        &self,
        model_type: crate::ai::ModelType,
        provider_id: Option<&str>,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<AnyModel>, AuthError> {
        self.models()
            .get_available_of_type(model_type, provider_id, options)
            .await
    }

    /// Upstream `getAllAvailable`.
    pub async fn get_all_available(
        &self,
        provider_id: Option<&str>,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<AnyModel>, AuthError> {
        self.models().get_all_available(provider_id, options).await
    }

    /// Upstream `generateImages`.
    pub async fn generate_images(
        &self,
        model: &crate::ai::types::ImageModel,
        context: &crate::ai::types::ImagesContext,
        options: Option<crate::ai::models::ModelsImagesOptions>,
    ) -> crate::ai::types::AssistantImages {
        self.models().generate_images(model, context, options).await
    }

    /// Upstream `classify`.
    pub async fn classify(
        &self,
        model: &crate::ai::types::ClassifierModel,
        context: &crate::ai::types::ClassifierContext,
        options: Option<crate::ai::models::ModelsClassifierOptions>,
    ) -> crate::ai::types::ClassifierResult {
        self.models().classify(model, context, options).await
    }

    /// Upstream `void this.refresh({ allowNetwork: false })`.
    fn spawn_background_refresh(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            let _ = this
                .refresh(ModelsRefreshOptions {
                    allow_network: Some(false),
                    ..ModelsRefreshOptions::default()
                })
                .await;
        });
    }
}

/// `signal.throwIfAborted()` over a bare token.
fn ensure_live(signal: &CancellationToken) -> Result<(), AuthError> {
    if signal.is_cancelled() {
        Err(AuthError::Cancelled)
    } else {
        Ok(())
    }
}

/// Upstream `findLatestResponse` over the LLM transcript: the latest
/// successful assistant response (failed or aborted requests are skipped).
fn find_latest_message_response(
    messages: &[crate::ai::types::Message],
) -> Option<&AssistantMessage> {
    messages.iter().rev().find_map(|message| match message {
        crate::ai::types::Message::Assistant(assistant)
            if assistant.stop_reason != StopReason::Error
                && assistant.stop_reason != StopReason::Aborted =>
        {
            Some(assistant)
        }
        _ => None,
    })
}

/// The `thinkingLevel` of an assistant message (upstream
/// `AssistantMessage.thinkingLevel`; the port stores the provider-native wire
/// spelling).
fn message_thinking_level(message: &AssistantMessage) -> Option<ModelThinkingLevel> {
    model_thinking_level_from_wire(message.provider_thinking_level.as_deref()?)
}

/// Wire spelling → [`ModelThinkingLevel`].
pub(crate) fn model_thinking_level_from_wire(wire: &str) -> Option<ModelThinkingLevel> {
    Some(match wire {
        "off" => ModelThinkingLevel::Off,
        "minimal" => ModelThinkingLevel::Minimal,
        "low" => ModelThinkingLevel::Low,
        "medium" => ModelThinkingLevel::Medium,
        "high" => ModelThinkingLevel::High,
        "xhigh" => ModelThinkingLevel::Xhigh,
        "max" => ModelThinkingLevel::Max,
        _ => return None,
    })
}

/// [`ModelThinkingLevel`] → wire spelling.
fn model_thinking_level_wire(level: ModelThinkingLevel) -> &'static str {
    match level {
        ModelThinkingLevel::Off => "off",
        ModelThinkingLevel::Minimal => "minimal",
        ModelThinkingLevel::Low => "low",
        ModelThinkingLevel::Medium => "medium",
        ModelThinkingLevel::High => "high",
        ModelThinkingLevel::Xhigh => "xhigh",
        ModelThinkingLevel::Max => "max",
    }
}

/// `SimpleStreamOptions.reasoning` → [`ModelThinkingLevel`].
fn model_thinking_level_from_level(level: Option<ThinkingLevel>) -> Option<ModelThinkingLevel> {
    level.map(|level| match level {
        ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
        ThinkingLevel::Low => ModelThinkingLevel::Low,
        ThinkingLevel::Medium => ModelThinkingLevel::Medium,
        ThinkingLevel::High => ModelThinkingLevel::High,
        ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
        ThinkingLevel::Max => ModelThinkingLevel::Max,
    })
}

/// [`ModelThinkingLevel`] → `SimpleStreamOptions.reasoning` (`"off"` is
/// absent, matching the upstream `undefined`).
fn model_thinking_level_to_level(level: ModelThinkingLevel) -> Option<ThinkingLevel> {
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
        ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
        ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
        ModelThinkingLevel::High => Some(ThinkingLevel::High),
        ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
        ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
    }
}

/// Upstream `clampThinkingLevel` (pi-ai models.ts) over the ported
/// `get_supported_thinking_levels`.
fn clamp_model_thinking_level(model: &Model, level: ModelThinkingLevel) -> ModelThinkingLevel {
    const LEVELS: [ModelThinkingLevel; 7] = [
        ModelThinkingLevel::Off,
        ModelThinkingLevel::Minimal,
        ModelThinkingLevel::Low,
        ModelThinkingLevel::Medium,
        ModelThinkingLevel::High,
        ModelThinkingLevel::Xhigh,
        ModelThinkingLevel::Max,
    ];
    let available_levels = crate::ai::models::get_supported_thinking_levels(model);
    if available_levels.contains(&model_thinking_level_wire(level)) {
        return level;
    }
    let Some(requested_index) = LEVELS.iter().position(|candidate| *candidate == level) else {
        return available_levels
            .first()
            .and_then(|wire| model_thinking_level_from_wire(wire))
            .unwrap_or(ModelThinkingLevel::Off);
    };
    for candidate in LEVELS[requested_index..].iter() {
        if available_levels.contains(&model_thinking_level_wire(*candidate)) {
            return *candidate;
        }
    }
    for candidate in LEVELS[..requested_index].iter().rev() {
        if available_levels.contains(&model_thinking_level_wire(*candidate)) {
            return *candidate;
        }
    }
    available_levels
        .first()
        .and_then(|wire| model_thinking_level_from_wire(wire))
        .unwrap_or(ModelThinkingLevel::Off)
}

/// `Provider | Model` overload of upstream `getAuth`.
#[derive(Clone, Copy)]
pub enum ProviderOrModel<'a> {
    Provider(&'a str),
    Model(&'a Model),
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The port-side `apiFor` check folded into dispatch (upstream `provider.stream`
/// rejects with this message when no implementation serves the model's API).
fn api_for_checked(
    provider: &dyn Provider,
    model: &Model,
) -> Result<Arc<dyn crate::ai::ApiImpl>, String> {
    provider.api_for(model).ok_or_else(|| {
        ModelsError::new(
            ModelsErrorCode::Stream,
            format!(
                "Provider {} has no API implementation for \"{}\"",
                provider.id(),
                model.api
            ),
        )
        .message
    })
}

/// Upstream `mergeHeaders` (pi-ai models.ts:250-264): case-insensitive
/// override merge; a `None` override value (upstream `null`) suppresses the
/// base header. `None` only when both sides are absent.
fn merge_headers(
    base: Option<&ProviderHeaders>,
    override_headers: Option<&ProviderHeaders>,
) -> Option<ProviderHeaders> {
    match (base, override_headers) {
        (None, None) => None,
        (base, override_headers) => {
            let mut merged: ProviderHeaders = base.cloned().unwrap_or_default();
            for (name, value) in override_headers.into_iter().flatten() {
                let lowercase = name.to_lowercase();
                let replaced: Vec<String> = merged
                    .keys()
                    .filter(|existing| existing.to_lowercase() == lowercase)
                    .cloned()
                    .collect();
                for existing in replaced {
                    merged.remove(&existing);
                }
                merged.insert(name.clone(), value.clone());
            }
            Some(merged)
        }
    }
}

/// Upstream `raceWithAbortSignal` (utils/abort.ts) over the operation's
/// token: an abort wins over any result.
async fn race_with_token<T>(
    future: impl std::future::Future<Output = Result<T, AuthError>>,
    token: &CancellationToken,
) -> Result<T, AuthError> {
    tokio::select! {
        result = future => result,
        _ = token.cancelled() => Err(AuthError::Cancelled),
    }
}

/// Upstream `Models.checkAuth`'s provider half (`checkProviderAuth`,
/// vendored — the ai layer keeps it private).
async fn check_provider_auth(
    provider: &dyn Provider,
    credential: Option<&Credential>,
    credentials: &dyn CredentialStore,
    auth_context: &dyn AuthContext,
    options: &AuthOperationOptions,
) -> Result<Option<AuthCheck>, AuthError> {
    if let Some(Credential::OAuth(_)) = credential {
        return Ok(provider.auth().oauth.as_ref().map(|_| AuthCheck {
            source: Some("OAuth".to_string()),
            r#type: AuthType::OAuth,
        }));
    }
    let Some(api_key) = provider.auth().api_key.as_ref() else {
        return Ok(None);
    };
    let api_key_credential = match credential {
        Some(Credential::ApiKey(credential)) => Some(credential),
        _ => None,
    };
    let input = ApiKeyAuthInput {
        ctx: auth_context,
        credential: api_key_credential,
        options,
    };
    if let Some(check) = api_key.check(input) {
        return match check.await {
            Ok(result) => Ok(result),
            Err(AuthError::Cancelled) => Err(AuthError::Cancelled),
            Err(error) => Err(AuthError::Models(ModelsError::with_cause(
                ModelsErrorCode::Auth,
                format!("API key auth check failed for provider {}", provider.id()),
                error,
            ))),
        };
    }
    let overrides = AuthResolutionOverrides {
        signal: options.signal.clone(),
        ..AuthResolutionOverrides::default()
    };
    let resolution = crate::ai::auth::resolve::resolve_provider_auth(
        provider.id(),
        provider.auth(),
        credentials,
        auth_context,
        Some(&overrides),
    )
    .await?;
    Ok(resolution.map(|resolution| AuthCheck {
        source: resolution.source,
        r#type: AuthType::ApiKey,
    }))
}

/// Upstream `lazyStream` (api/lazy.ts) as a channel: the routing setup runs
/// in a spawned task behind the returned receiver; setup failures emit a
/// single error event and close the stream (the ai-layer convention).
fn lazy_stream<F, Fut>(
    model: &Model,
    context: &Context,
    setup: F,
) -> mpsc::Receiver<AssistantMessageEvent>
where
    F: FnOnce(Model, TranscriptContext) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<mpsc::Receiver<AssistantMessageEvent>, String>> + Send,
{
    let transcript = normalize_context(context);
    let (tx, rx) = mpsc::channel(64);
    let model = model.clone();
    tokio::spawn(async move {
        let setup_model = model.clone();
        match setup(setup_model, transcript).await {
            Ok(mut inner) => {
                while let Some(event) = inner.recv().await {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
            Err(message) => {
                let _ = tx
                    .send(AssistantMessageEvent::Error {
                        reason: ErrorReason::Error,
                        error: setup_error_message(&model, message),
                    })
                    .await;
            }
        }
    });
    rx
}

/// Upstream `createSetupErrorMessage` (api/lazy.ts:8-31, vendored).
fn setup_error_message(model: &Model, message: impl std::fmt::Display) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(message.to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

/// Upstream `.result()` on an event stream (vendored `reduceStream`).
async fn reduce_stream(
    mut rx: mpsc::Receiver<AssistantMessageEvent>,
    model: &Model,
) -> AssistantMessage {
    let mut partial = PartialAssistant::new();
    while let Some(event) = rx.recv().await {
        if let Err(error) = partial.apply(&event) {
            return setup_error_message(
                model,
                format!("reducer rejected {}: {error}", event.event_type()),
            );
        }
    }
    partial
        .message()
        .cloned()
        .unwrap_or_else(|| setup_error_message(model, "stream ended without events"))
}

/// `config.baseUrl.replace(/\/v1\/?$/u, "")`.
fn strip_trailing_v1(base_url: &str) -> String {
    if let Some(stripped) = base_url.strip_suffix("/v1/") {
        return stripped.to_string();
    }
    if let Some(stripped) = base_url.strip_suffix("/v1") {
        return stripped.to_string();
    }
    base_url.to_string()
}

/// `node:path.dirname` for the models-store default path (host-platform).
fn dirname(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    match normalized.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => normalized[..index].to_string(),
        None => ".".to_string(),
    }
}

/// Re-registration merge (upstream registerProvider): defined values over
/// the previous registration, undefined ones preserved.
fn merge_provider_config_inputs(
    previous: Option<&ProviderConfigInput>,
    config: &ProviderConfigInput,
) -> ProviderConfigInput {
    let mut effective = previous.cloned().unwrap_or_default();
    if config.name.is_some() {
        effective.name = config.name.clone();
    }
    if config.base_url.is_some() {
        effective.base_url = config.base_url.clone();
    }
    if config.api_key.is_some() {
        effective.api_key = config.api_key.clone();
    }
    if config.api.is_some() {
        effective.api = config.api.clone();
    }
    if config.stream_simple.is_some() {
        effective.stream_simple = config.stream_simple.clone();
    }
    if config.headers.is_some() {
        effective.headers = config.headers.clone();
    }
    if config.auth_header.is_some() {
        effective.auth_header = config.auth_header;
    }
    if config.oauth.is_some() {
        effective.oauth = config.oauth.clone();
    }
    if config.models.is_some() {
        effective.models = config.models.clone();
    }
    if config.refresh_models.is_some() {
        effective.refresh_models = config.refresh_models.clone();
    }
    effective
}

// ---------------------------------------------------------------------------
// AuthFileStore — the default credential store seam (upstream
// `AuthStorage.create`)
// ---------------------------------------------------------------------------

/// The default credential store: a `Record<string, Credential>` JSON file
/// with per-provider write serialization. Upstream `FileAuthStorage` adds a
/// proper-lockfile `<path>.lock` protocol around writes (not reproduced —
/// disclosed) and preserves document key order (the rewrite sorts keys).
pub struct AuthFileStore {
    path: String,
    locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AuthFileStore {
    pub fn new(path: String) -> Self {
        Self {
            path,
            locks: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn lock_provider(&self, provider_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(provider_id.to_string())
            .or_default()
            .clone()
    }

    fn read_document(&self) -> std::collections::BTreeMap<String, Credential> {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Default::default(),
        }
    }

    fn write_document(&self, document: &std::collections::BTreeMap<String, Credential>) {
        if let Some(parent) = std::path::Path::new(&self.path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let content = serde_json::to_string_pretty(document).unwrap_or_default();
        let _ = std::fs::write(&self.path, content);
    }
}

impl CredentialStore for AuthFileStore {
    fn read<'a>(
        &'a self,
        provider_id: &'a str,
        _options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move { Ok(self.read_document().get(provider_id).cloned()) })
    }

    fn list<'a>(
        &'a self,
        _options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
        Box::pin(async move {
            Ok(self
                .read_document()
                .into_iter()
                .map(|(provider_id, credential)| CredentialInfo {
                    provider_id,
                    r#type: credential.auth_type(),
                })
                .collect())
        })
    }

    fn modify<'a>(
        &'a self,
        provider_id: &'a str,
        f: ModifyCallback,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
        Box::pin(async move {
            options.check()?;
            let lock = self.lock_provider(provider_id).await;
            let _guard = lock.lock().await;
            let current = self.read_document().get(provider_id).cloned();
            let next = f(current).await?;
            let mut document = self.read_document();
            let applied = match next {
                Some(credential) => {
                    document.insert(provider_id.to_string(), credential.clone());
                    Some(credential)
                }
                None => None,
            };
            self.write_document(&document);
            Ok(applied)
        })
    }

    fn delete<'a>(
        &'a self,
        provider_id: &'a str,
        options: &'a AuthOperationOptions,
    ) -> BoxFuture<'a, Result<(), AuthError>> {
        Box::pin(async move {
            options.check()?;
            let lock = self.lock_provider(provider_id).await;
            let _guard = lock.lock().await;
            let mut document = self.read_document();
            document.remove(provider_id);
            self.write_document(&document);
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "model_runtime_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "model_runtime_registration_tests.rs"]
mod registration_tests;
