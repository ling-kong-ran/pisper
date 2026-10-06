//! The model-catalog layer (upstream `packages/ai/src/model-catalog.ts` plus
//! the generated catalog shards `providers/*.models.ts` and the
//! `data/.manifest.json` manifest) and the `Models` collection (upstream
//! `packages/ai/src/models.ts`): the provider registry, its sync reads, auth
//! resolution ([`Models::get_auth`]/[`Models::get_auth_for_model`]), stream
//! routing ([`Models::stream`]/[`Models::stream_simple`] with the
//! [`ModelsApiStreamOptions`] transform hooks), and the refresh/publication
//! machinery for dynamic providers ([`Models::refresh`] with the
//! [`ModelsStore`] persistence surface, `store.rs`). The built-in provider
//! factories join in Task 5; `checkAuth`/`login`/`logout` (upstream
//! models.ts:187-208, 523-532, 577-638) join with their consumer task.

pub mod catalog;
pub mod faux;
pub mod provider;
pub mod providers;
pub mod store;

pub use catalog::{
    catalog_provider_ids, embedded_provider_catalog, embedded_provider_groups,
    flatten_model_catalog, model_data_manifest, model_data_structure, model_data_structure_hash,
    validate_embedded_catalog, ModelDataManifest, ModelDataStructure, MODEL_DATA_MANIFEST_FILE,
    MODEL_DATA_SCHEMA_VERSION,
};
pub use faux::{
    faux_assistant_message, faux_provider, faux_text, faux_thinking, faux_tool_call, FauxContent,
    FauxCore, FauxFactoryArgs, FauxMessageOptions, FauxModelDefinition, FauxProviderHandle,
    FauxProviderOptions, FauxProviderState, FauxResponseFactory, FauxResponseStep, FauxStateHandle,
    FauxToolCallOptions,
};
pub use provider::{
    create_provider, ApiImpls, ClassifierApiImpl, ClassifiersImpls, CreateProviderOptions,
    FetchModelsFn, FilterAllModelsFn, FilterModelsFn, ImagesApiImpl, ImagesImpls, Provider,
    StandardProvider,
};
pub use providers::{
    builtin_model, builtin_model_data_generated_at, builtin_models, builtin_models_with,
    builtin_provider_ids, builtin_providers,
};
pub use store::{
    InMemoryModelsStore, ModelsStore, ModelsStoreEntry, ModelsStoreError,
    ModelsStoreOperationOptions,
};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ai::auth::context::default_provider_auth_context;
use crate::ai::auth::credential_store::{CredentialStore, InMemoryCredentialStore, ModifyCallback};
use crate::ai::auth::resolve::{
    resolve_provider_auth, AuthResolutionOverrides, ModelsError, ModelsErrorCode,
};
use crate::ai::auth::types::{
    ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthContext, AuthError, AuthOperationOptions,
    AuthResult, AuthType, Credential,
};
use crate::ai::model_operations::{
    assert_classifier_model, assert_image_model, classifier_error_result, get_model_type,
    image_error_result, is_model_type,
};
use crate::ai::transcript::{normalize_context, TranscriptContext};
use crate::ai::types::events::{AssistantMessageEvent, ErrorReason, PartialAssistant};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::options::{ProviderEnv, ProviderHeaders, SimpleStreamOptions, StreamOptions};
use crate::ai::types::primitives::{StopReason, Usage};
use crate::ai::types::{
    AnyModel, AssistantImages, ClassifierContext, ClassifierModel, ClassifierOptions,
    ClassifierResult, ImageModel, ImagesContext, ImagesOptions, Model, ModelType,
};
use crate::ai::{now_ms, Context, ProviderConfig};

/// Upstream `CreateModelsOptions` (models.ts:244-249).
#[derive(Default)]
pub struct CreateModelsOptions {
    /// Credential store backing auth resolution (upstream `credentials`).
    /// Default: the in-memory store (upstream `InMemoryCredentialStore`).
    pub credentials: Option<Arc<dyn CredentialStore>>,
    /// Models store backing dynamic-provider catalogs (upstream
    /// `modelsStore`). Default: the in-memory store
    /// (upstream `InMemoryModelsStore`).
    pub models_store: Option<Arc<dyn ModelsStore>>,
    /// Environment access for auth resolution (upstream `authContext`).
    /// Default: the process-env context (upstream `defaultAuthContext`).
    pub auth_context: Option<Arc<dyn AuthContext>>,
}

/// Upstream `ModelsRequestTransforms.transformHeaders` (models.ts:81-83): a
/// Models-only transform over the fully assembled model/auth/request headers,
/// run once before provider dispatch. Async upstream; the port hands the
/// closure a [`BoxFuture`].
pub type TransformHeaders =
    Arc<dyn Fn(ProviderHeaders) -> BoxFuture<'static, ProviderHeaders> + Send + Sync>;

/// Upstream `ModelsApiStreamOptions` (models.ts:85): `ApiStreamOptions` plus
/// the Models-only header transform. The upstream intersection flattens to an
/// embedded base struct here, the same shape [`SimpleStreamOptions`] uses for
/// its upstream base.
#[derive(Clone, Default)]
pub struct ModelsApiStreamOptions {
    /// Base request/stream options.
    pub stream: StreamOptions,
    /// Runs once over the assembled headers (auth + model + explicit)
    /// before dispatch (upstream `transformHeaders`); its result replaces
    /// them and never reaches the provider.
    pub transform_headers: Option<TransformHeaders>,
}

/// Upstream `ModelsSimpleStreamOptions` (models.ts:86): `SimpleStreamOptions`
/// plus the Models-only header transform.
#[derive(Clone, Default)]
pub struct ModelsSimpleStreamOptions {
    /// Base request/stream options plus the simple-request extension fields.
    pub simple: SimpleStreamOptions,
    /// Runs once over the assembled headers before dispatch (upstream
    /// `transformHeaders`).
    pub transform_headers: Option<TransformHeaders>,
}

/// Upstream `ModelsImagesOptions` (models.ts): `ImagesOptions` plus the
/// Models-only header transform.
#[derive(Clone, Default)]
pub struct ModelsImagesOptions {
    /// Base image-generation request options.
    pub images: ImagesOptions,
    /// Runs once over the assembled headers before dispatch (upstream
    /// `transformHeaders`).
    pub transform_headers: Option<TransformHeaders>,
}

/// Upstream `ModelsClassifierOptions` (models.ts): `ClassifierOptions` plus
/// the Models-only header transform.
#[derive(Clone, Default)]
pub struct ModelsClassifierOptions {
    /// Base classification request options.
    pub classifier: ClassifierOptions,
    /// Runs once over the assembled headers before dispatch (upstream
    /// `transformHeaders`).
    pub transform_headers: Option<TransformHeaders>,
}

/// Upstream `ModelsPublication` (models.ts:41-46): one publication handed to
/// [`RefreshModelsContext::publish`]. The outer option is upstream
/// `persist?`-presence; the inner distinguishes delete (`None`, upstream
/// `null`) from write.
#[derive(Default)]
pub struct ModelsPublication {
    /// Provider-selected persisted catalog. `None` leaves storage unchanged;
    /// `Some(None)` deletes it; `Some(Some(entry))` writes it.
    pub persist: Option<Option<ModelsStoreEntry>>,
    /// Optional synchronous update of provider-private in-memory catalog
    /// state (upstream `update?`), run after the storage mutation — but only
    /// when the publication was not superseded or aborted.
    pub update: Option<Box<dyn FnOnce() + Send>>,
}

/// Port failure channel for one provider's refresh. Upstream rejections
/// propagate raw into `ModelsRefreshResult.errors`; the port types them as
/// [`ModelsError`]s (constructed at the failure sites) and separates
/// cancellation, which the result never records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshModelsError {
    /// The refresh's cancellation token fired (upstream `AbortError`).
    Cancelled,
    /// Provider or storage failure, recorded in
    /// [`ModelsRefreshResult::errors`].
    Failed(ModelsError),
}

/// The generation-checked publication handle behind
/// [`RefreshModelsContext::publish`] (upstream the closure built at
/// models.ts:391 over `publishProviderModels`, models.ts:350-377).
#[derive(Clone)]
struct ModelsPublisher {
    provider_id: String,
    generation: u64,
    signal: CancellationToken,
    shared: Arc<RefreshShared>,
    models_store: Arc<dyn ModelsStore>,
}

impl ModelsPublisher {
    /// Upstream `publishProviderModels` (models.ts:350-377): publications
    /// serialize per provider (upstream promise chain, here a fair FIFO
    /// tokio mutex), re-check the signal and generation after queueing and
    /// again after the storage mutation, then run the synchronous update.
    /// Resolves `Ok(false)` when superseded or aborted (upstream rejects with
    /// an abort reason instead — callers treat both as "not applied").
    async fn publish(&self, publication: ModelsPublication) -> Result<bool, RefreshModelsError> {
        let lock = self.shared.publication_lock(&self.provider_id);
        let queued = async {
            // The async lock guard is held across the store awaits by
            // design — it serializes this provider's publications. The
            // std Mutex guards in RefreshShared never cross awaits.
            let _guard = lock.lock().await;
            if self.signal.is_cancelled()
                || !self.shared.is_current(&self.provider_id, self.generation)
            {
                return Ok(false);
            }
            let options = ModelsStoreOperationOptions::new(self.signal.clone());
            match publication.persist {
                // upstream models.ts:361-365; the entry is cloned into the
                // store (upstream `structuredClone(publication.persist)`).
                Some(None) => self
                    .models_store
                    .delete(&self.provider_id, &options)
                    .await
                    .map_err(store_refresh_error)?,
                Some(Some(entry)) => self
                    .models_store
                    .write(&self.provider_id, entry, &options)
                    .await
                    .map_err(store_refresh_error)?,
                None => {}
            }
            if self.signal.is_cancelled()
                || !self.shared.is_current(&self.provider_id, self.generation)
            {
                return Ok(false);
            }
            if let Some(update) = publication.update {
                (update)();
            }
            Ok(true)
        };
        tokio::select! {
            result = queued => result,
            _ = self.signal.cancelled() => Err(RefreshModelsError::Cancelled),
        }
    }
}

/// Upstream `RefreshModelsContext` (models.ts:48-64): what a dynamic
/// provider's `refreshModels` receives for one refresh phase. A cheap
/// handle: every field is owned or `Arc`/token-backed, and the whole context
/// is [`Clone`] so implementations can hand copies to spawned work.
#[derive(Clone)]
pub struct RefreshModelsContext {
    /// Effective configured credential. The offline restore phase receives
    /// the raw stored credential; the network phase the resolved one (OAuth
    /// credentials are refreshed before network access).
    pub credential: Option<Credential>,
    /// Immutable provider-scoped catalog snapshot captured before this
    /// refresh phase. Owned (upstream `structuredClone`d) — mutating it
    /// never reaches the store.
    pub stored: Option<ModelsStoreEntry>,
    /// False during offline/cache-only initialization.
    pub allow_network: bool,
    /// Bypass provider freshness checks and fetch immediately when network
    /// access is allowed. Absent when network access is disallowed (upstream
    /// `undefined`).
    pub force: Option<bool>,
    /// Always present, including when the public refresh caller omits its
    /// optional signal.
    pub signal: CancellationToken,
    publisher: ModelsPublisher,
}

impl RefreshModelsContext {
    /// Upstream `context.publish(publication)` (models.ts:57):
    /// generation-checked publication. Persistence policy remains
    /// provider-owned; the update runs synchronously only after the selected
    /// persistence mutation. Resolves `Ok(true)` when applied, `Ok(false)`
    /// when superseded/aborted, `Err` on storage failure or cancellation.
    pub async fn publish(
        &self,
        publication: ModelsPublication,
    ) -> Result<bool, RefreshModelsError> {
        self.publisher.publish(publication).await
    }
}

/// Upstream `ModelsRefreshOptions` (models.ts:66-73).
#[derive(Default)]
pub struct ModelsRefreshOptions {
    /// Default true (upstream `allowNetwork ?? true`).
    pub allow_network: Option<bool>,
    /// Restrict refresh to these provider IDs. Unknown and static providers
    /// are ignored.
    pub providers: Option<Vec<String>>,
    /// Bypass provider freshness checks and fetch immediately when network
    /// access is allowed.
    pub force: Option<bool>,
    /// Caller cancellation (upstream `signal?`).
    pub signal: Option<CancellationToken>,
}

/// Upstream `ModelsRefreshResult` (models.ts:75-78): per-provider failures
/// and whether the caller's signal cancelled the run. Provider errors and
/// cancellation are returned without failing; static, unknown, and
/// unconfigured providers are skipped.
#[derive(Debug, Clone, Default)]
pub struct ModelsRefreshResult {
    pub aborted: bool,
    pub errors: BTreeMap<String, ModelsError>,
}

type ProviderEntries = Vec<(String, Arc<dyn Provider>)>;

/// Upstream `Models` + `MutableModels` (models.ts:163-242): runtime collection
/// of providers plus auth application and stream convenience. The upstream
/// `Models`/`MutableModels` interface split (read surface vs registry
/// mutation) is a JS capability boundary; the port is one struct. Providers
/// are held in registration order (upstream `Map` insertion order — an upsert
/// keeps the original position). `Clone` is a shared-registration clone (all
/// fields are `Arc`s — clones observe later `set_provider` calls), which the
/// harness's injected `DeferredCancelFn` closure captures.
#[derive(Clone)]
pub struct Models {
    providers: Arc<Mutex<ProviderEntries>>,
    credentials: Arc<dyn CredentialStore>,
    auth_context: Arc<dyn AuthContext>,
    models_store: Arc<dyn ModelsStore>,
    /// Refresh bookkeeping shared with in-flight publications: upstream
    /// `refreshGenerations`/`refreshControllers`/`publicationChains`
    /// (models.ts:271-273). `Mutex` guards are never held across awaits (the
    /// async publication serialization uses per-provider tokio locks).
    refresh: Arc<RefreshShared>,
}

/// Per-provider refresh state shared between the registry mutators
/// (`set_provider` supersede), [`Models::refresh`], and the publication
/// handles captured by in-flight providers.
struct RefreshShared {
    /// Upstream `refreshGenerations` (models.ts:271): one bump per
    /// supersede/begin; publications from older generations are rejected.
    generations: Mutex<HashMap<String, u64>>,
    /// Upstream `refreshControllers` (models.ts:272): the current refresh's
    /// cancellation token per provider, keyed with its generation so
    /// [`RefreshShared::release`] removes only the entry it owns. Tokens are
    /// children of the caller's token, mirroring upstream
    /// `AbortSignal.any([callerSignal, controller.signal])` (models.ts:412).
    controllers: Mutex<HashMap<String, (u64, CancellationToken)>>,
    /// Upstream `publicationChains` (models.ts:273): per-provider
    /// serialization of publications. A fair FIFO async `Mutex` replaces the
    /// promise chain; the per-provider allocation is bounded by the
    /// provider-id cardinality (the credential-store lock precedent).
    publications: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RefreshShared {
    fn new() -> Self {
        RefreshShared {
            generations: Mutex::new(HashMap::new()),
            controllers: Mutex::new(HashMap::new()),
            publications: Mutex::new(HashMap::new()),
        }
    }

    fn lock<'a, T>(guard: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Upstream `supersedeProviderRefresh` (models.ts:332-341): bump the
    /// generation and cancel the in-flight refresh token, if any. Returns the
    /// new generation.
    fn supersede(&self, provider_id: &str) -> u64 {
        let generation = {
            let mut generations = Self::lock(&self.generations);
            let generation = generations.entry(provider_id.to_string()).or_insert(0);
            *generation += 1;
            *generation
        };
        let previous = Self::lock(&self.controllers).remove(provider_id);
        if let Some((_, token)) = previous {
            token.cancel();
        }
        generation
    }

    /// Upstream `beginProviderRefresh` (models.ts:343-348): supersede any
    /// previous refresh and mint this one's token (a child of the caller's —
    /// upstream `AbortSignal.any`).
    fn begin(&self, provider_id: &str, caller: &CancellationToken) -> (u64, CancellationToken) {
        let generation = self.supersede(provider_id);
        let token = caller.child_token();
        Self::lock(&self.controllers).insert(provider_id.to_string(), (generation, token.clone()));
        (generation, token)
    }

    /// Upstream models.ts:443-446: drop the controller slot only if this
    /// refresh still owns it (a newer refresh may have replaced it).
    fn release(&self, provider_id: &str, generation: u64) {
        let mut controllers = Self::lock(&self.controllers);
        if controllers
            .get(provider_id)
            .is_some_and(|(current, _)| *current == generation)
        {
            controllers.remove(provider_id);
        }
    }

    /// Whether `generation` is still the provider's current refresh.
    fn is_current(&self, provider_id: &str, generation: u64) -> bool {
        Self::lock(&self.generations)
            .get(provider_id)
            .is_some_and(|current| *current == generation)
    }

    fn publication_lock(&self, provider_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Self::lock(&self.publications)
            .entry(provider_id.to_string())
            .or_default()
            .clone()
    }

    /// Provider ids with bookkeeping (upstream clearProviders merges provider
    /// keys with `refreshControllers` keys, models.ts:292-296).
    fn tracked_ids(&self) -> HashSet<String> {
        Self::lock(&self.controllers).keys().cloned().collect()
    }
}

/// Upstream `createModels` (models.ts:757-759).
pub fn create_models(options: CreateModelsOptions) -> Models {
    Models {
        providers: Arc::new(Mutex::new(Vec::new())),
        credentials: options.credentials.unwrap_or_else(|| {
            Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>
        }),
        auth_context: options
            .auth_context
            .unwrap_or_else(|| Arc::new(default_provider_auth_context()) as Arc<dyn AuthContext>),
        models_store: options
            .models_store
            .unwrap_or_else(|| Arc::new(InMemoryModelsStore::default()) as Arc<dyn ModelsStore>),
        refresh: Arc::new(RefreshShared::new()),
    }
}

/// Upstream `EXTENDED_THINKING_LEVELS` (models.ts:922).
const EXTENDED_THINKING_LEVELS: &[&str] =
    &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Upstream `getSupportedThinkingLevels` (models.ts:924-933): the thinking
/// levels a model accepts. Non-reasoning models accept only `"off"`;
/// `"xhigh"`/`"max"` need an explicit mapping in `thinkingLevelMap`, and a
/// `null` mapping disables its level.
pub fn get_supported_thinking_levels(model: &Model) -> Vec<&'static str> {
    if !model.reasoning {
        return vec!["off"];
    }
    EXTENDED_THINKING_LEVELS
        .iter()
        .copied()
        .filter(|level| {
            let mapped = model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(*level));
            if mapped == Some(&None) {
                return false;
            }
            if *level == "xhigh" || *level == "max" {
                return mapped.is_some();
            }
            true
        })
        .collect()
}

/// Upstream `modelsAreEqual` (models.ts): check if two models are equal by
/// comparing their type, id, and provider. `None` (upstream null/undefined)
/// on either side is false.
pub fn models_are_equal(a: Option<&AnyModel>, b: Option<&AnyModel>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return false;
    };
    get_model_type(a) == get_model_type(b) && a.id() == b.id() && a.provider() == b.provider()
}

impl Models {
    /// Upstream `MutableModels.setProvider` (models.ts:239, 281-284): upsert
    /// by provider id — ids are unique and replacement keeps the original
    /// position. Any in-flight refresh for the id is superseded first.
    pub fn set_provider(&mut self, provider: Arc<dyn Provider>) {
        // Never call provider code while holding the registry lock. Handles
        // captured by requests/lanes observe registrations through this shared map.
        let id = provider.id().to_string();
        self.refresh.supersede(&id);
        let mut providers = self.provider_entries();
        match providers.iter_mut().find(|(existing, _)| existing == &id) {
            Some((_, existing)) => *existing = provider,
            None => providers.push((id, provider)),
        }
    }

    fn provider_entries(&self) -> std::sync::MutexGuard<'_, ProviderEntries> {
        self.providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Upstream `MutableModels.deleteProvider` (models.ts:286-289): no-op for
    /// unknown ids, like `Map.delete`. Any in-flight refresh for the id is
    /// superseded first.
    pub fn delete_provider(&mut self, id: &str) {
        self.refresh.supersede(id);
        self.provider_entries()
            .retain(|(existing, _)| existing != id);
    }

    /// Upstream `MutableModels.clearProviders` (models.ts:291-296): supersede
    /// bookkeeping for both the registered providers and any provider whose
    /// refresh is still tracked, then clear.
    pub fn clear_providers(&mut self) {
        let mut ids: HashSet<String> = self.refresh.tracked_ids();
        ids.extend(self.provider_entries().iter().map(|(id, _)| id.clone()));
        for id in ids {
            self.refresh.supersede(&id);
        }
        self.provider_entries().clear();
    }

    /// Upstream `Models.getProviders` (models.ts:298-300), in provider
    /// registration order.
    pub fn get_providers(&self) -> Vec<Arc<dyn Provider>> {
        self.provider_entries()
            .iter()
            .map(|(_, provider)| Arc::clone(provider))
            .collect()
    }

    /// Upstream `Models.getProvider` (models.ts:302-304).
    pub fn get_provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.provider_entries()
            .iter()
            .find(|(existing, _)| existing == id)
            .map(|(_, provider)| Arc::clone(provider))
    }

    /// Upstream `Models.getModels` (models.ts:306-326): sync read of
    /// last-known models from one provider or all providers. Best-effort: a
    /// provider whose catalog read fails yields no models — for the
    /// single-provider filter, the all-providers concatenation, and unknown
    /// provider ids alike.
    pub fn get_models(&self, provider: Option<&str>) -> Vec<Model> {
        match provider {
            Some(id) => self
                .get_provider(id)
                .and_then(|entry| entry.get_models().ok())
                .unwrap_or_default(),
            None => self
                .get_providers()
                .iter()
                .filter_map(|entry| entry.get_models().ok())
                .flatten()
                .collect(),
        }
    }

    /// Upstream `Models.getModel` (models.ts:328-330): sync runtime lookup
    /// against last-known lists. Upstream narrows the result with `hasApi()`;
    /// here [`Model::api`] is compared directly (see `provider.rs` module
    /// docs).
    pub fn get_model(&self, provider: &str, id: &str) -> Option<Model> {
        self.get_models(Some(provider))
            .into_iter()
            .find(|model| model.id == id)
    }

    /// Upstream `Models.getAllModels` (models.ts): sync read of last-known
    /// models of every type from one provider or all providers. Best-effort
    /// like [`Models::get_models`].
    pub fn get_all_models(&self, provider: Option<&str>) -> Vec<AnyModel> {
        match provider {
            Some(id) => self
                .get_provider(id)
                .and_then(|entry| entry.get_all_models().ok())
                .unwrap_or_default(),
            None => self
                .get_providers()
                .iter()
                .filter_map(|entry| entry.get_all_models().ok())
                .flatten()
                .collect(),
        }
    }

    /// Upstream `Models.getModelsOfType` (models.ts): last-known models of
    /// one type.
    pub fn get_models_of_type(
        &self,
        model_type: ModelType,
        provider: Option<&str>,
    ) -> Vec<AnyModel> {
        self.get_all_models(provider)
            .into_iter()
            .filter(|model| is_model_type(model, model_type))
            .collect()
    }

    /// Upstream `Models.getModelOfType` (models.ts): sync runtime lookup of a
    /// model of one type against last-known lists.
    pub fn get_model_of_type(
        &self,
        model_type: ModelType,
        provider: &str,
        id: &str,
    ) -> Option<AnyModel> {
        self.get_models_of_type(model_type, Some(provider))
            .into_iter()
            .find(|model| model.id() == id)
    }

    /// Upstream `Models.refresh` (models.ts:180-184, 398-458): refresh the
    /// selected configured dynamic providers concurrently (all when
    /// `providers` is omitted). Per provider the refresh runs two
    /// `refreshModels` phases — an offline restore from the models store,
    /// then (network allowed and auth resolved) a fetch — raced against the
    /// provider's token (a child of the caller's signal), so cancellation
    /// drops the in-flight phase at its await point. Provider errors are
    /// returned in the result without failing; static, unknown, and
    /// unconfigured providers are skipped, and cancellation is never
    /// recorded as a provider error.
    pub async fn refresh(&self, options: ModelsRefreshOptions) -> ModelsRefreshResult {
        let allow_network = options.allow_network.unwrap_or(true);
        let caller = options.signal.clone().unwrap_or_default();
        let mut errors = BTreeMap::new();
        if caller.is_cancelled() {
            return ModelsRefreshResult {
                aborted: true,
                errors,
            };
        }
        let refreshable: Vec<Arc<dyn Provider>> = self
            .get_providers()
            .into_iter()
            .filter(|provider| {
                provider.is_dynamic()
                    && options
                        .providers
                        .as_ref()
                        .map(|selected| selected.iter().any(|id| id == provider.id()))
                        .unwrap_or(true)
            })
            .collect();

        // One concurrent slot per refreshable provider. Each slot selects its
        // operation against the provider token, so an abort (caller signal or
        // supersede) stops waiting even when a provider ignores its signal —
        // the abandoned future is dropped at its await point (the Rust
        // analogue of upstream's continuing-but-unobserved promise).
        let outcomes = futures::future::join_all(refreshable.into_iter().map(|provider| {
            let shared = Arc::clone(&self.refresh);
            let models_store = Arc::clone(&self.models_store);
            let credentials = Arc::clone(&self.credentials);
            let auth_context = Arc::clone(&self.auth_context);
            let caller = caller.clone();
            async move {
                let (generation, token) = shared.begin(provider.id(), &caller);
                let publisher = ModelsPublisher {
                    provider_id: provider.id().to_string(),
                    generation,
                    signal: token.clone(),
                    shared: Arc::clone(&shared),
                    models_store: Arc::clone(&models_store),
                };
                let outcome = tokio::select! {
                    outcome = run_provider_refresh(RefreshRun {
                        provider: provider.as_ref(),
                        models_store: models_store.as_ref(),
                        credentials: credentials.as_ref(),
                        auth_context: auth_context.as_ref(),
                        publisher,
                        allow_network,
                        force: options.force,
                        token: &token,
                    }) => outcome,
                    _ = token.cancelled() => Err(RefreshModelsError::Cancelled),
                };
                shared.release(provider.id(), generation);
                // Upstream models.ts:434-441: cancellation is not a provider
                // error; everything else is recorded (Errors are recorded
                // verbatim upstream — here already-typed at the failure site).
                let error = match outcome {
                    Ok(()) => None,
                    Err(RefreshModelsError::Cancelled) => None,
                    Err(RefreshModelsError::Failed(error)) => {
                        if token.is_cancelled() {
                            None
                        } else {
                            Some(error)
                        }
                    }
                };
                (provider.id().to_string(), error)
            }
        }))
        .await;

        for (id, error) in outcomes {
            if let Some(error) = error {
                errors.insert(id, error);
            }
        }
        ModelsRefreshResult {
            aborted: caller.is_cancelled(),
            errors,
        }
    }

    /// Upstream `Models.getAvailable` (models.ts:189-190, 534-554): models
    /// whose providers have complete auth configuration, with each provider's
    /// [`Provider::filter_models`] applied after the check. Auth reads run
    /// concurrently; cancellation surfaces as [`AuthError::Cancelled`].
    pub async fn get_available(
        &self,
        provider_id: Option<&str>,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<Model>, AuthError> {
        let options = options.cloned().unwrap_or_default();
        options.check()?;
        let providers: Vec<Arc<dyn Provider>> = match provider_id {
            Some(id) => self.get_provider(id).into_iter().collect(),
            None => self.get_providers(),
        };
        let checks = futures::future::join_all(providers.iter().map(|provider| {
            let credentials = Arc::clone(&self.credentials);
            let auth_context = Arc::clone(&self.auth_context);
            let options = options.clone();
            let provider = Arc::clone(provider);
            async move {
                let credential = read_refresh_credential(&*credentials, provider.id()).await?;
                let auth = check_provider_auth(
                    &*provider,
                    credential.as_ref(),
                    &*credentials,
                    &*auth_context,
                    &options,
                )
                .await?;
                Ok::<_, AuthError>((provider, credential, auth))
            }
        }));
        let checks = tokio::select! {
            checks = checks => checks,
            _ = options.cancelled() => return Err(AuthError::Cancelled),
        };
        let mut available = Vec::new();
        for check in checks {
            let (provider, credential, auth) = check?;
            if auth.is_none() {
                continue;
            }
            let models = provider.get_models().map_err(AuthError::Models)?;
            available.extend(
                provider
                    .filter_models(&models, credential.as_ref())
                    .unwrap_or(models),
            );
        }
        Ok(available)
    }

    /// Upstream `Models.getAvailableOfType` (models.ts): models of one type
    /// whose providers have complete auth configuration, via
    /// [`Models::get_all_available`].
    pub async fn get_available_of_type(
        &self,
        model_type: ModelType,
        provider_id: Option<&str>,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<AnyModel>, AuthError> {
        Ok(self
            .get_all_available(provider_id, options)
            .await?
            .into_iter()
            .filter(|model| is_model_type(model, model_type))
            .collect())
    }

    /// Upstream `Models.getAllAvailable` (models.ts): models of every type
    /// whose providers have complete auth configuration. A provider's
    /// `filterAllModels` wins; otherwise `filterModels` applies to the chat
    /// models (matched by id) and every other model is kept.
    pub async fn get_all_available(
        &self,
        provider_id: Option<&str>,
        options: Option<&AuthOperationOptions>,
    ) -> Result<Vec<AnyModel>, AuthError> {
        let options = options.cloned().unwrap_or_default();
        options.check()?;
        let providers: Vec<Arc<dyn Provider>> = match provider_id {
            Some(id) => self.get_provider(id).into_iter().collect(),
            None => self.get_providers(),
        };
        let checks = futures::future::join_all(providers.iter().map(|provider| {
            let credentials = Arc::clone(&self.credentials);
            let auth_context = Arc::clone(&self.auth_context);
            let options = options.clone();
            let provider = Arc::clone(provider);
            async move {
                let credential = read_refresh_credential(&*credentials, provider.id()).await?;
                let auth = check_provider_auth(
                    &*provider,
                    credential.as_ref(),
                    &*credentials,
                    &*auth_context,
                    &options,
                )
                .await?;
                Ok::<_, AuthError>((provider, credential, auth))
            }
        }));
        let checks = tokio::select! {
            checks = checks => checks,
            _ = options.cancelled() => return Err(AuthError::Cancelled),
        };
        let mut available = Vec::new();
        for check in checks {
            let (provider, credential, auth) = check?;
            if auth.is_none() {
                continue;
            }
            let models = provider.get_all_models().map_err(AuthError::Models)?;
            if let Some(filtered) = provider.filter_all_models(&models, credential.as_ref()) {
                available.extend(filtered);
                continue;
            }
            if !provider.has_filter_models() {
                available.extend(models);
                continue;
            }
            let chat = provider.get_models().map_err(AuthError::Models)?;
            let available_chat_ids: HashSet<String> = provider
                .filter_models(&chat, credential.as_ref())
                .unwrap_or(chat)
                .into_iter()
                .map(|model| model.id)
                .collect();
            available.extend(models.into_iter().filter(|model| {
                model.model_type() != ModelType::Chat || available_chat_ids.contains(model.id())
            }));
        }
        Ok(available)
    }

    /// Upstream `Models.getAuth(providerId)` (models.ts:201, 556-575):
    /// provider-scoped auth resolution with a source label for status UI.
    /// Resolves `Ok(None)` when the provider is unknown or unconfigured;
    /// `Err(AuthError::Models(..))` carries the [`ModelsError`] when a token
    /// refresh (code `"oauth"`, credential preserved) or api-key/credential
    /// store resolution (code `"auth"`) fails. Cancellation surfaces as
    /// [`AuthError::Cancelled`].
    pub async fn get_auth(
        &self,
        provider_id: &str,
        overrides: Option<&AuthResolutionOverrides>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let Some(provider) = self.get_provider(provider_id) else {
            return Ok(None);
        };
        resolve_provider_auth(
            provider_id,
            provider.auth(),
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            overrides,
        )
        .await
    }

    /// Upstream `Models.getAuth(model)` (models.ts:202, 556-575): the
    /// provider-scoped resolution plus the model's static headers, merged
    /// case-insensitively over the auth headers (README "Transforming
    /// Request Headers": provider auth headers -> model.headers).
    pub async fn get_auth_for_model(
        &self,
        model: &Model,
        overrides: Option<&AuthResolutionOverrides>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let Some(result) = self.get_auth(&model.provider, overrides).await? else {
            return Ok(None);
        };
        Ok(Some(merge_model_headers(result, model)))
    }

    /// Upstream `Models.stream` (models.ts:210-214, 679-693): normalize the
    /// context, then lazily resolve auth and dispatch to the owning
    /// provider's API implementation. Setup failures (unknown provider,
    /// unconfigured auth, no API implementation) terminate the stream with an
    /// error event instead of throwing (upstream `lazyStream`).
    pub fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let options = options.unwrap_or_default();
        self.route(
            model,
            context,
            RoutedOptions::Api {
                stream: options.stream,
                transform_headers: options.transform_headers,
            },
        )
    }

    /// Upstream `Models.complete` (models.ts:216-221): the stream's final
    /// message; error streams settle with `stopReason: "error"`.
    pub async fn complete(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsApiStreamOptions>,
    ) -> AssistantMessage {
        reduce_stream(self.stream(model, context, options), model).await
    }

    /// Upstream `Models.streamSimple` (models.ts:222, 703-710): the
    /// simple-request routing over [`Models::stream`]'s path.
    pub fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsSimpleStreamOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let options = options.unwrap_or_default();
        self.route(
            model,
            context,
            RoutedOptions::Simple {
                simple: options.simple,
                transform_headers: options.transform_headers,
            },
        )
    }

    /// Upstream `Models.completeSimple` (models.ts:223, 712-718).
    pub async fn complete_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<ModelsSimpleStreamOptions>,
    ) -> AssistantMessage {
        reduce_stream(self.stream_simple(model, context, options), model).await
    }

    /// Upstream `Models.streamDeferred` (models.ts:719-735): dispatch to the
    /// owning provider's optional `fetchDeferred` capability. Providers
    /// without it yield the upstream setup-error stream.
    pub fn stream_deferred(
        &self,
        model: &Model,
        handle: &crate::ai::types::options::DeferredHandle,
        context: &Context,
        options: Option<ModelsDeferredFetchOptions>,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let options = options.unwrap_or_default();
        self.route(
            model,
            context,
            RoutedOptions::Deferred {
                handle: handle.clone(),
                stream: options.stream,
                transform_headers: options.transform_headers,
            },
        )
    }

    /// Upstream `Models.fetchDeferred` (models.ts:737-741): the stream's
    /// final message.
    pub async fn fetch_deferred(
        &self,
        model: &Model,
        handle: &crate::ai::types::options::DeferredHandle,
        context: &Context,
        options: Option<ModelsDeferredFetchOptions>,
    ) -> AssistantMessage {
        reduce_stream(self.stream_deferred(model, handle, context, options), model).await
    }

    /// Upstream `Models.cancelDeferred` (models.ts:743-754): require the
    /// owning provider, reject the missing capability *before* auth
    /// resolution (models.ts:749-751), apply auth, then dispatch to the API
    /// implementation. Unlike the stream entry points the failure channel
    /// throws (upstream rejects with `ModelsError`); every production call
    /// site (`cancelDeferredBestEffort`) swallows it.
    ///
    /// Disclosed substitution: upstream carries the capability on the
    /// `Provider` object and re-dispatches per api (`apiFor`), so a
    /// capability-bearing provider with an api-less model yields the
    /// models.ts:874 "cannot cancel deferred responses for" literal; the
    /// port's capability probe lives on the [`ApiImpl`](crate::ai::api::ApiImpl)
    /// itself and an `api_for` miss keeps the "no API implementation" error
    /// every other entry point produces.
    pub async fn cancel_deferred(
        &self,
        model: &Model,
        handle: &crate::ai::types::options::DeferredHandle,
        options: Option<ModelsDeferredCancelOptions>,
    ) -> anyhow::Result<()> {
        let options = options.unwrap_or_default();
        // requireProvider (models.ts:748).
        let provider = self.get_provider(&model.provider).ok_or_else(|| {
            ModelsError::new(
                ModelsErrorCode::Provider,
                format!("Unknown provider: {}", model.provider),
            )
        })?;
        let implementation = provider.api_for(model).ok_or_else(|| {
            ModelsError::new(
                ModelsErrorCode::Stream,
                format!(
                    "Provider {} has no API implementation for \"{}\"",
                    provider.id(),
                    model.api
                ),
            )
        })?;
        // Capability check before auth resolution (models.ts:749-751).
        if !implementation.supports_deferred_cancel() {
            return Err(ModelsError::new(
                ModelsErrorCode::Provider,
                format!(
                    "Provider {} does not support deferred responses",
                    model.provider
                ),
            )
            .into());
        }
        // applyAuth (models.ts:752), then the provider dispatch (models.ts:753).
        let applied = apply_auth(
            provider.as_ref(),
            model,
            self.credentials.as_ref(),
            self.auth_context.as_ref(),
            &options.stream,
            options.transform_headers.clone(),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        let mut stream = options.stream;
        stream.api_key = applied.api_key;
        stream.headers = applied.headers;
        stream.env = applied.env;
        implementation
            .cancel_deferred(&applied.config, &applied.request_model, handle, &stream)
            .await
            .map_err(anyhow::Error::msg)
    }

    /// Upstream `Models.generateImages` (models.ts): generate images through
    /// the owning provider with auth resolved like `stream()`. Never rejects:
    /// unknown providers, unconfigured auth, non-image models, and providers
    /// without the capability all return an error `AssistantImages` (upstream
    /// `try { ... } catch (error) { return imageErrorResult(...) }`).
    pub async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        options: Option<ModelsImagesOptions>,
    ) -> AssistantImages {
        let options = options.unwrap_or_default();
        let result: Result<AssistantImages, ModelsError> = async {
            let any = AnyModel::Image(model.clone());
            assert_image_model(&any)?;
            let provider = self.get_provider(&model.provider).ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Provider,
                    format!("Unknown provider: {}", model.provider),
                )
            })?;
            let implementation = provider.images_for(&model.api).ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Provider,
                    format!(
                        "Provider {} does not support image generation",
                        model.provider
                    ),
                )
            })?;
            let applied = apply_auth_one_shot(
                provider.as_ref(),
                &any,
                self.credentials.as_ref(),
                self.auth_context.as_ref(),
                &options.images.api_key,
                &options.images.env,
                &options.images.headers,
                &options.images.signal,
                options.transform_headers.clone(),
            )
            .await?;
            let mut image_options = options.images.clone();
            image_options.api_key = applied.api_key;
            image_options.headers = applied.headers;
            image_options.env = applied.env;
            let model = match applied.request_model {
                AnyModel::Image(model) => model,
                _ => model.clone(),
            };
            implementation
                .generate_images(&applied.config, &model, context, &image_options)
                .await
        }
        .await;
        match result {
            Ok(images) => images,
            Err(error) => image_error_result(
                model,
                error,
                options.images.signal.is_some_and(|s| s.is_cancelled()),
            ),
        }
    }

    /// Upstream `Models.classify` (models.ts): classify structured state
    /// through the owning provider. Never rejects; failures are returned as
    /// an error `ClassifierResult`.
    pub async fn classify(
        &self,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: Option<ModelsClassifierOptions>,
    ) -> ClassifierResult {
        let options = options.unwrap_or_default();
        let result: Result<ClassifierResult, ModelsError> = async {
            let any = AnyModel::Classifier(model.clone());
            assert_classifier_model(&any)?;
            let provider = self.get_provider(&model.provider).ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Provider,
                    format!("Unknown provider: {}", model.provider),
                )
            })?;
            let implementation = provider.classifiers_for(&model.api).ok_or_else(|| {
                ModelsError::new(
                    ModelsErrorCode::Provider,
                    format!(
                        "Provider {} does not support classification",
                        model.provider
                    ),
                )
            })?;
            let applied = apply_auth_one_shot(
                provider.as_ref(),
                &any,
                self.credentials.as_ref(),
                self.auth_context.as_ref(),
                &options.classifier.api_key,
                &options.classifier.env,
                &options.classifier.headers,
                &options.classifier.signal,
                options.transform_headers.clone(),
            )
            .await?;
            let mut classifier_options = options.classifier.clone();
            classifier_options.api_key = applied.api_key;
            classifier_options.headers = applied.headers;
            classifier_options.env = applied.env;
            let model = match applied.request_model {
                AnyModel::Classifier(model) => model,
                _ => model.clone(),
            };
            implementation
                .classify(&applied.config, &model, context, &classifier_options)
                .await
        }
        .await;
        match result {
            Ok(result) => result,
            Err(error) => classifier_error_result(
                model,
                error,
                options.classifier.signal.is_some_and(|s| s.is_cancelled()),
            ),
        }
    }

    /// Upstream `lazyStream` (api/lazy.ts:43-60) as a channel: the routing
    /// setup runs in a spawned task behind the returned receiver; setup
    /// failures emit a single error event (upstream `createSetupErrorMessage`)
    /// and close the stream.
    fn route(
        &self,
        model: &Model,
        context: &Context,
        options: RoutedOptions,
    ) -> mpsc::Receiver<AssistantMessageEvent> {
        let transcript = normalize_context(context);
        let model = model.clone();
        let provider = self.get_provider(&model.provider);
        let credentials = Arc::clone(&self.credentials);
        let auth_context = Arc::clone(&self.auth_context);
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            match route_stream(
                provider.as_deref(),
                &model,
                transcript,
                credentials.as_ref(),
                auth_context.as_ref(),
                options,
            )
            .await
            {
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
}

/// The two option shapes [`Models::route`] accepts, mirroring the upstream
/// `ApiStreamOptions` vs `SimpleStreamOptions` dispatch target.
#[derive(Clone)]
enum RoutedOptions {
    Api {
        stream: StreamOptions,
        transform_headers: Option<TransformHeaders>,
    },
    Simple {
        simple: SimpleStreamOptions,
        transform_headers: Option<TransformHeaders>,
    },
    /// Upstream `streamDeferred(model, handle, options)` (`models.ts:224`,
    /// `719-735`): dispatch to the owning provider's optional
    /// `fetchDeferred` capability.
    Deferred {
        handle: crate::ai::types::options::DeferredHandle,
        stream: StreamOptions,
        transform_headers: Option<TransformHeaders>,
    },
}

/// Event-channel capacity for routed streams; the [`ApiImpl`] implementations
/// use the same bound for their own channels.
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// Upstream `mergeHeaders` (models.ts:250-264): case-insensitive override
/// merge; an override `None` value (upstream `null`) suppresses the base
/// header. `None` only when both sides are absent.
pub(crate) fn merge_headers(
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

/// Upstream `getAuth(model)`'s header fold (models.ts:567-574): the model's
/// static headers merge over the resolved auth headers. [`Model::headers`]
/// carries the same `Option`-valued [`ProviderHeaders`] shape, so a `None`
/// model value suppresses (deletes) an auth default of the same name, like
/// upstream `mergeHeaders`.
fn merge_model_headers(mut resolution: AuthResult, model: &Model) -> AuthResult {
    if let Some(model_headers) = model.headers.as_ref().filter(|headers| !headers.is_empty()) {
        resolution.auth.headers =
            merge_headers(resolution.auth.headers.as_ref(), Some(model_headers));
    }
    resolution
}

/// [`merge_model_headers`] over any model variant's static headers (upstream
/// `getAuth(model)` is typed over `AnyModel` since the unified catalog).
fn merge_model_headers_value(
    mut resolution: AuthResult,
    model_headers: Option<&ProviderHeaders>,
) -> AuthResult {
    if let Some(model_headers) = model_headers.filter(|headers| !headers.is_empty()) {
        resolution.auth.headers =
            merge_headers(resolution.auth.headers.as_ref(), Some(model_headers));
    }
    resolution
}

/// Upstream `createSetupErrorMessage` (api/lazy.ts:8-31): the message a
/// routing failure settles its stream with.
pub(crate) fn setup_error_message(
    model: &Model,
    message: impl std::fmt::Display,
) -> AssistantMessage {
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

/// Upstream `.result()` on an event stream (utils/event-stream.ts): reduce the
/// event sequence to its final message. Error events settle the message with
/// `stopReason: "error"` (the [`PartialAssistant`] rules), so failures resolve
/// as values, exactly like upstream.
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

/// Upstream `applyAuth`'s resolved request (models.ts:648-677): the
/// request-model rewrite plus the assembled request options, collapsed into
/// the port's routed-config channel (`ProviderConfig` carries what upstream
/// passes as `requestModel.baseUrl` + `requestOptions.apiKey`).
pub(crate) struct AppliedAuth {
    /// Upstream `requestModel` (models.ts:671): the auth-derived `baseUrl`
    /// overrides the model's.
    pub request_model: Model,
    pub config: ProviderConfig,
    pub api_key: Option<String>,
    pub headers: Option<ProviderHeaders>,
    pub env: Option<ProviderEnv>,
}

/// Upstream `applyAuth` (models.ts:648-677) — `getAuth(model)` with the
/// explicit per-field overrides (including the request signal: an abort
/// during auth resolution rejects the setup), then the per-field merge where
/// explicit request options win and the Models-only transform runs last.
/// Failures return the upstream error message verbatim.
async fn apply_auth(
    provider: &dyn Provider,
    model: &Model,
    credentials: &dyn CredentialStore,
    auth_context: &dyn AuthContext,
    request_options: &StreamOptions,
    transform_headers: Option<TransformHeaders>,
) -> Result<AppliedAuth, String> {
    let options_api_key = request_options.api_key.clone();
    let options_env = request_options.env.clone();
    let options_headers = request_options.headers.clone();
    let overrides = AuthResolutionOverrides {
        api_key: options_api_key.clone(),
        env: options_env.clone(),
        signal: request_options.signal.clone(),
        ..AuthResolutionOverrides::default()
    };
    let resolution = resolve_provider_auth(
        &model.provider,
        provider.auth(),
        credentials,
        auth_context,
        Some(&overrides),
    )
    .await
    .map_err(|error| match error {
        AuthError::Models(error) => error.message,
        other => other.to_string(),
    })?;
    let Some(resolution) = resolution else {
        return Err(ModelsError::new(
            ModelsErrorCode::Auth,
            format!("Provider is not configured: {}", model.provider),
        )
        .to_string());
    };
    // applyAuth routes through getAuth(model), so the model's static headers
    // fold in here too (models.ts:656-663).
    let resolution = merge_model_headers(resolution, model);
    let auth = &resolution.auth;

    // Explicit request options win per field; the Models-only transform runs
    // last (models.ts:666-670).
    let api_key = options_api_key.or_else(|| auth.api_key.clone());
    let mut headers = merge_headers(auth.headers.as_ref(), options_headers.as_ref());
    if let Some(transform_headers) = transform_headers {
        headers = Some(transform_headers(headers.unwrap_or_default()).await);
    }
    let env = match (resolution.env.as_ref(), options_env.as_ref()) {
        (None, None) => None,
        (resolved, explicit) => {
            let mut merged: ProviderEnv = resolved.cloned().unwrap_or_default();
            merged.extend(explicit.cloned().unwrap_or_default());
            Some(merged)
        }
    };

    // The routed config is the port's channel for what upstream passes as
    // `requestModel.baseUrl` + `requestOptions.apiKey`: the ApiImpls read
    // both from it.
    let config = ProviderConfig {
        base_url: auth
            .base_url
            .clone()
            .unwrap_or_else(|| model.base_url.clone()),
        api_key: api_key.clone().unwrap_or_default(),
        max_tokens: model.max_tokens,
    };
    // requestModel (models.ts:671): the auth-derived baseUrl overrides the
    // model's; ApiImpls that read `model.base_url` (azure resource urls) see
    // the same override.
    let mut request_model = model.clone();
    if let Some(auth_base_url) = auth.base_url.clone() {
        request_model.base_url = auth_base_url;
    }
    Ok(AppliedAuth {
        request_model,
        config,
        api_key,
        headers,
        env,
    })
}

/// The one-shot (image/classifier) resolved request: upstream
/// `applyAuth<TModel extends AnyModel>` (models.ts) is generic over the model
/// type, so the request model keeps its variant with the auth-derived
/// `baseUrl` override.
pub(crate) struct AppliedAuthOneShot {
    pub request_model: AnyModel,
    pub config: ProviderConfig,
    pub api_key: Option<String>,
    pub headers: Option<ProviderHeaders>,
    pub env: Option<ProviderEnv>,
}

/// Upstream `applyAuth` for image/classifier models: the same precedence
/// (explicit options win per field, Models-only transform last) over the
/// one-shot options shapes, which carry `apiKey`/`env`/`headers`/`signal`
/// directly instead of a `StreamOptions`.
#[allow(clippy::too_many_arguments)]
async fn apply_auth_one_shot(
    provider: &dyn Provider,
    model: &AnyModel,
    credentials: &dyn CredentialStore,
    auth_context: &dyn AuthContext,
    options_api_key: &Option<String>,
    options_env: &Option<ProviderEnv>,
    options_headers: &Option<ProviderHeaders>,
    signal: &Option<CancellationToken>,
    transform_headers: Option<TransformHeaders>,
) -> Result<AppliedAuthOneShot, ModelsError> {
    let overrides = AuthResolutionOverrides {
        api_key: options_api_key.clone(),
        env: options_env.clone(),
        signal: signal.clone(),
        ..AuthResolutionOverrides::default()
    };
    let resolution = resolve_provider_auth(
        model.provider(),
        provider.auth(),
        credentials,
        auth_context,
        Some(&overrides),
    )
    .await
    .map_err(|error| match error {
        AuthError::Models(error) => error,
        AuthError::Cancelled => {
            ModelsError::new(ModelsErrorCode::Auth, crate::ai::api::REQUEST_ABORTED)
        }
        other => ModelsError::with_cause(ModelsErrorCode::Auth, "Auth resolution failed", other),
    })?;
    let Some(resolution) = resolution else {
        return Err(ModelsError::new(
            ModelsErrorCode::Auth,
            format!("Provider is not configured: {}", model.provider()),
        ));
    };
    // getAuth(model) folds the model's static headers in (models.ts:656-663).
    let resolution = merge_model_headers_value(resolution, model.headers());
    let auth = &resolution.auth;

    let api_key = options_api_key.clone().or_else(|| auth.api_key.clone());
    let mut headers = merge_headers(auth.headers.as_ref(), options_headers.as_ref());
    if let Some(transform_headers) = transform_headers {
        headers = Some(transform_headers(headers.unwrap_or_default()).await);
    }
    let env = match (resolution.env.as_ref(), options_env.as_ref()) {
        (None, None) => None,
        (resolved, explicit) => {
            let mut merged: ProviderEnv = resolved.cloned().unwrap_or_default();
            merged.extend(explicit.cloned().unwrap_or_default());
            Some(merged)
        }
    };
    let config = ProviderConfig {
        base_url: auth
            .base_url
            .clone()
            .unwrap_or_else(|| model.base_url().to_string()),
        api_key: api_key.clone().unwrap_or_default(),
        // Image/classifier models carry no maxTokens upstream.
        max_tokens: 0,
    };
    let mut request_model = model.clone();
    if let Some(auth_base_url) = auth.base_url.clone() {
        set_base_url(&mut request_model, auth_base_url);
    }
    Ok(AppliedAuthOneShot {
        request_model,
        config,
        api_key,
        headers,
        env,
    })
}

/// The auth-derived `baseUrl` override on any model variant (upstream
/// `{ ...model, baseUrl: auth.baseUrl }`).
fn set_base_url(model: &mut AnyModel, base_url: String) {
    match model {
        AnyModel::Chat(inner) => inner.base_url = base_url,
        AnyModel::Image(inner) => inner.base_url = base_url,
        AnyModel::Classifier(inner) => inner.base_url = base_url,
    }
}

/// Upstream `requireProvider` + `applyAuth` + the provider dispatch
/// (models.ts:640-718): resolve auth through the owning provider, assemble
/// the request (config/options), then route to the model's API
/// implementation. Failures return the upstream error message verbatim; the
/// caller settles the stream with it.
async fn route_stream(
    provider: Option<&dyn Provider>,
    model: &Model,
    transcript: TranscriptContext,
    credentials: &dyn CredentialStore,
    auth_context: &dyn AuthContext,
    options: RoutedOptions,
) -> Result<mpsc::Receiver<AssistantMessageEvent>, String> {
    // requireChatProvider (models.ts): `assertChatModel(model)` first — a
    // catalog entry typed image/classifier has no chat stream — then
    // `requireProvider`. The Rust entry points are statically chat-typed;
    // this defends the parsed-model edge where `Model.r#type` is set.
    if let Some(model_type) = model.r#type {
        if model_type != ModelType::Chat {
            return Err(ModelsError::new(
                ModelsErrorCode::Provider,
                format!("Model {}/{} is not a chat model", model.provider, model.id),
            )
            .to_string());
        }
    }
    // requireProvider (models.ts:640-646).
    let provider = provider.ok_or_else(|| {
        ModelsError::new(
            ModelsErrorCode::Provider,
            format!("Unknown provider: {}", model.provider),
        )
        .to_string()
    })?;

    // applyAuth (models.ts:648-677) — getAuth(model) with the explicit
    // per-field overrides, including the request signal (the M2d
    // cancellation surface: an abort during auth resolution rejects the
    // setup, surfacing as the routing error event). The per-field overrides
    // ride on the routed StreamOptions; the Models-only header transform is
    // carried alongside.
    let transform_headers = match &options {
        RoutedOptions::Api {
            transform_headers, ..
        }
        | RoutedOptions::Simple {
            transform_headers, ..
        }
        | RoutedOptions::Deferred {
            transform_headers, ..
        } => transform_headers.clone(),
    };
    let applied = apply_auth(
        provider,
        model,
        credentials,
        auth_context,
        match &options {
            RoutedOptions::Api { stream, .. } | RoutedOptions::Deferred { stream, .. } => stream,
            RoutedOptions::Simple { simple, .. } => &simple.stream,
        },
        transform_headers,
    )
    .await?;

    // Provider dispatch (models.ts:691, 708): the api-implementation lookup
    // doubles as the upstream `apiFor` check; `None` produces the
    // "no API implementation" stream error (models.ts:808-811).
    let implementation = provider.api_for(model).ok_or_else(|| {
        ModelsError::new(
            ModelsErrorCode::Stream,
            format!(
                "Provider {} has no API implementation for \"{}\"",
                provider.id(),
                model.api
            ),
        )
        .to_string()
    })?;

    let callbacks = match &options {
        RoutedOptions::Api { stream, .. } | RoutedOptions::Deferred { stream, .. } => {
            &stream.callbacks
        }
        RoutedOptions::Simple { simple, .. } => &simple.stream.callbacks,
    };
    if !callbacks.is_empty() && !implementation.supports_request_callbacks() {
        return Err(format!(
            "API {} does not yet support request lifecycle callbacks",
            model.api
        ));
    }
    match options {
        RoutedOptions::Deferred {
            handle, mut stream, ..
        } => {
            stream.api_key = applied.api_key;
            stream.headers = applied.headers;
            stream.env = applied.env;
            Ok(implementation.stream_deferred(
                &applied.config,
                &applied.request_model,
                &handle,
                &stream,
            ))
        }
        RoutedOptions::Api { mut stream, .. } => {
            stream.api_key = applied.api_key;
            stream.headers = applied.headers;
            stream.env = applied.env;
            Ok(implementation.stream(
                &applied.config,
                &applied.request_model,
                &transcript,
                &stream,
            ))
        }
        RoutedOptions::Simple { mut simple, .. } => {
            simple.stream.api_key = applied.api_key;
            simple.stream.headers = applied.headers;
            simple.stream.env = applied.env;
            Ok(implementation.stream_simple(
                &applied.config,
                &applied.request_model,
                &transcript,
                &simple,
            ))
        }
    }
}

/// Upstream `ModelsDeferredFetchOptions`: stream-level request options for
/// deferred polls (the port keeps the transport-only subset).
#[derive(Clone, Default)]
pub struct ModelsDeferredFetchOptions {
    pub stream: StreamOptions,
    pub transform_headers: Option<TransformHeaders>,
}

/// Upstream `ModelsDeferredCancelOptions` (models.ts:88, `DeferredCancelOptions
/// & ModelsRequestTransforms`): request options for remote deferred
/// cancellation (`Models::cancel_deferred`). The port keeps the same
/// transport-only subset as [`ModelsDeferredFetchOptions`] (module docs):
/// the `StreamOptions` request fields plus the Models-only header transform.
pub type ModelsDeferredCancelOptions = ModelsDeferredFetchOptions;

/// [`ModelsStoreError`] surfaced through the refresh error channel (upstream
/// records the raw store rejection; the message is preserved verbatim).
fn store_refresh_error(error: ModelsStoreError) -> RefreshModelsError {
    match error {
        ModelsStoreError::Cancelled => RefreshModelsError::Cancelled,
        ModelsStoreError::Storage(message) => {
            RefreshModelsError::Failed(ModelsError::new(ModelsErrorCode::ModelSource, message))
        }
    }
}

/// Auth-store errors through the refresh error channel. Upstream rejections
/// propagate raw; the port keeps typed [`ModelsError`]s and renders the rest
/// through their `Display` (which carries the underlying reason).
fn auth_refresh_error(error: AuthError) -> RefreshModelsError {
    match error {
        AuthError::Cancelled => RefreshModelsError::Cancelled,
        AuthError::Models(error) => RefreshModelsError::Failed(error),
        other => {
            RefreshModelsError::Failed(ModelsError::new(ModelsErrorCode::Auth, other.to_string()))
        }
    }
}

/// Upstream `readCredential` (models.ts:489-495) as the refresh path sees it:
/// the shared [`read_credential`](crate::ai::auth::resolve::read_credential)
/// wrap (store-read failures become code-`"auth"` `ModelsError`s) with the
/// operation-less option set. Shared with `get_available`, which surfaces the
/// same wrapping through its `AuthError` channel (upstream resolve.ts:195-205).
async fn read_refresh_credential(
    credentials: &dyn CredentialStore,
    provider_id: &str,
) -> Result<Option<Credential>, AuthError> {
    crate::ai::auth::resolve::read_credential(credentials, provider_id, &AuthOperationOptions::NONE)
        .await
}

/// Per-refresh operation inputs, bundled once per provider slot (upstream
/// `refresh`'s closure captures, models.ts:410-430).
struct RefreshRun<'a> {
    provider: &'a dyn Provider,
    models_store: &'a dyn ModelsStore,
    credentials: &'a dyn CredentialStore,
    auth_context: &'a dyn AuthContext,
    publisher: ModelsPublisher,
    allow_network: bool,
    force: Option<bool>,
    token: &'a CancellationToken,
}

/// Upstream `refresh`'s per-provider operation (models.ts:413-430): read the
/// stored credential best-effort, run the offline restore phase, then — when
/// network is allowed and a credential resolved — the network fetch phase.
async fn run_provider_refresh(run: RefreshRun<'_>) -> Result<(), RefreshModelsError> {
    // Best-effort credential read (models.ts:414-420): the failure is held
    // and thrown only after the restore phase ran. The read goes through the
    // shared `read_credential` wrap, so the held error is already a
    // code-`"auth"` `ModelsError` (or `Cancelled`).
    let (stored_credential, credential_error) = {
        let options = AuthOperationOptions::new(run.token.clone());
        let read = tokio::select! {
            result = crate::ai::auth::resolve::read_credential(
                run.credentials,
                run.provider.id(),
                &options,
            ) => result.map_err(auth_refresh_error),
            _ = run.token.cancelled() => Err(RefreshModelsError::Cancelled),
        };
        match read {
            Ok(credential) => (credential, None),
            Err(error) => (None, Some(error)),
        }
    };

    // Restore cached provider state before auth resolution or network access
    // (models.ts:423).
    run_provider_refresh_phase(&run, stored_credential.clone(), false, None).await?;
    if let Some(error) = credential_error {
        return Err(error);
    }
    if !run.allow_network || run.token.is_cancelled() {
        return Ok(());
    }

    let credential = resolve_refresh_credential(
        run.provider,
        run.credentials,
        run.auth_context,
        stored_credential,
        run.token,
    )
    .await?;
    let Some(credential) = credential else {
        return Ok(());
    };
    run_provider_refresh_phase(&run, Some(credential), true, run.force).await
}

/// Upstream `withKnownModelTypes` (models.ts): drops stored models whose type
/// this version does not know ("Models from stores and remote sources may
/// have types that only newer versions know"). The [`AnyModel`] parse layer
/// falls unknown `type` values back to chat, so every parsed model is of a
/// known type — the filter is kept for upstream parity.
fn with_known_model_types(mut entry: ModelsStoreEntry) -> ModelsStoreEntry {
    entry
        .models
        .retain(crate::ai::models::provider::has_known_model_type);
    entry
}

/// Upstream `runProviderRefreshPhase` (models.ts:379-396): read the
/// provider's stored catalog, then hand the phase context to
/// `provider.refreshModels`, raced against the refresh token.
async fn run_provider_refresh_phase(
    run: &RefreshRun<'_>,
    credential: Option<Credential>,
    allow_network: bool,
    force: Option<bool>,
) -> Result<(), RefreshModelsError> {
    let token = run.token;
    let store_options = ModelsStoreOperationOptions::new(token.clone());
    let stored = tokio::select! {
        result = run.models_store.read(run.provider.id(), &store_options) => result.map_err(store_refresh_error)?,
        _ = token.cancelled() => return Err(RefreshModelsError::Cancelled),
    };
    let context = RefreshModelsContext {
        credential,
        // Store reads already return owned values, the port equivalent of
        // upstream's `structuredClone(stored)`; `withKnownModelTypes` drops
        // stored models whose type this version does not know (models.ts).
        stored: stored.map(with_known_model_types),
        allow_network,
        // Upstream `force: allowNetwork ? force : undefined` (models.ts:393).
        force: if allow_network { force } else { None },
        signal: token.clone(),
        publisher: run.publisher.clone(),
    };
    let Some(refresh) = run.provider.refresh_models(context) else {
        // refresh() filters on is_dynamic, so this is unreachable for
        // well-formed providers.
        return Ok(());
    };
    tokio::select! {
        result = refresh => result,
        _ = token.cancelled() => Err(RefreshModelsError::Cancelled),
    }
}

/// Upstream `resolveRefreshCredential` (models.ts:460-487): the network
/// phase's effective credential. Stored OAuth tokens are refreshed when
/// expired (locked through `credentials.modify` with a double-check — no
/// minimum-validity window here, unlike the getAuth flow), api-key
/// credentials resolve through the provider's handler, and `Ok(None)` means
/// the provider has no usable auth for refresh.
async fn resolve_refresh_credential(
    provider: &dyn Provider,
    credentials: &dyn CredentialStore,
    auth_context: &dyn AuthContext,
    stored: Option<Credential>,
    token: &CancellationToken,
) -> Result<Option<Credential>, RefreshModelsError> {
    if let Some(Credential::OAuth(stored)) = &stored {
        let Some(oauth) = provider.auth().oauth.clone() else {
            return Ok(None);
        };
        if now_ms() < stored.expires {
            return Ok(Some(Credential::OAuth(stored.clone())));
        }
        if token.is_cancelled() {
            return Ok(None);
        }
        // models.ts:470-478: the locked refresh. The double-check under the
        // store lock skips when another caller already rotated the token.
        let oauth_for_callback = Arc::clone(&oauth);
        let token_for_callback = token.clone();
        let callback: ModifyCallback = Box::new(move |current| {
            Box::pin(async move {
                let current = match current {
                    Some(Credential::OAuth(current)) => current,
                    // Logged out meanwhile, or the entry changed type.
                    _ => return Ok(None),
                };
                if now_ms() < current.expires {
                    return Ok(None);
                }
                let options = AuthOperationOptions::new(token_for_callback);
                // Upstream propagates refresh rejections raw (no
                // ModelsError wrap on this path — resolve.ts's wrapping is
                // getAuth-only).
                match oauth_for_callback.refresh(current, &options).await {
                    Ok(refreshed) => Ok(Some(Credential::OAuth(refreshed))),
                    Err(error) => Err(error),
                }
            })
        });
        let options = AuthOperationOptions::new(token.clone());
        let post = tokio::select! {
            result = credentials.modify(provider.id(), callback, &options) => result.map_err(auth_refresh_error)?,
            _ = token.cancelled() => return Err(RefreshModelsError::Cancelled),
        };
        return Ok(match post {
            Some(credential @ Credential::OAuth(_)) => Some(credential),
            _ => None,
        });
    }

    let Some(api_key) = provider.auth().api_key.clone() else {
        return Ok(None);
    };
    let api_key_credential = match &stored {
        Some(Credential::ApiKey(credential)) => Some(credential.clone()),
        _ => None,
    };
    let options = AuthOperationOptions::new(token.clone());
    let input = ApiKeyAuthInput {
        ctx: auth_context,
        credential: api_key_credential.as_ref(),
        options: &options,
    };
    let result = tokio::select! {
        result = api_key.resolve(input) => result.map_err(auth_refresh_error)?,
        _ = token.cancelled() => return Err(RefreshModelsError::Cancelled),
    };
    let Some(result) = result else {
        return Ok(None);
    };
    Ok(Some(Credential::ApiKey(ApiKeyCredential {
        key: result.auth.api_key,
        env: result.env,
        extra: Default::default(),
    })))
}

/// Upstream `checkProviderAuth` (models.ts:497-521): side-effect-free
/// configurability check. Stored OAuth credentials report the provider's
/// OAuth handler without refreshing; api-key providers run their `check`
/// when implemented, else resolve. `get_available` consumes this; the
/// public `checkAuth` wrapper joins with its consumer task.
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
    let resolution = resolve_provider_auth(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::api::ApiImpl;
    use crate::ai::auth::credential_store::{InMemoryCredentialStore, ModifyCallback};
    use crate::ai::auth::resolve::{AuthResolutionOverrides, ModelsError, ModelsErrorCode};
    use crate::ai::auth::types::{
        ApiKeyAuth, ApiKeyAuthInput, ApiKeyCredential, AuthCheck, AuthError, AuthOperationOptions,
        AuthResult, AuthType, Credential, CredentialInfo, ModelAuth, OAuthAuth, OAuthCredential,
        ProviderAuth, ProviderAuthInteraction,
    };
    use crate::ai::models::faux::{
        faux_assistant_message, faux_provider, FauxMessageOptions, FauxProviderOptions,
        FauxResponseStep,
    };
    use crate::ai::now_ms;
    use crate::ai::transcript::TranscriptContext;
    use crate::ai::types::events::{AssistantMessageEvent, PartialAssistant, SuccessReason};
    use crate::ai::types::message::{AssistantMessage, Message, StringOrBlocks, UserMessage};
    use crate::ai::types::options::{
        DeferredFlag, DeferredHandle, ProviderHeaders, SimpleStreamOptions, StreamOptions,
    };
    use crate::ai::types::primitives::{ModelCost, StopReason, Usage};
    use crate::ai::types::ModelInput;
    use crate::ai::{Context, ProviderConfig};
    use futures::future::BoxFuture;
    use std::collections::BTreeMap;
    use tokio::sync::mpsc;

    /// Upstream `testModel` fixture (models-runtime.test.ts:9-22): api
    /// "test-api".
    fn test_model(provider: &str, id: &str) -> Model {
        Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            name: id.to_string(),
            api: "test-api".to_string(),
            provider: provider.to_string(),
            base_url: "https://example.test/v1".to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![ModelInput::Text],
            cost: ModelCost::default(),
            context_window: 10_000,
            max_tokens: 1000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    /// Upstream `ambientAuth` fixture (models-runtime.test.ts:49-53).
    struct AmbientKeyAuth;

    impl ApiKeyAuth for AmbientKeyAuth {
        fn name(&self) -> &str {
            "Ambient"
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async { Ok(Some(AuthResult::default())) })
        }
    }

    fn ambient_auth() -> ProviderAuth {
        ProviderAuth {
            api_key: Some(Arc::new(AmbientKeyAuth)),
            oauth: None,
        }
    }

    struct StubApi;

    impl ApiImpl for StubApi {
        fn stream(
            &self,
            _cfg: &crate::ai::ProviderConfig,
            _model: &Model,
            _ctx: &TranscriptContext,
            _options: &StreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            rx
        }

        fn stream_simple(
            &self,
            _cfg: &crate::ai::ProviderConfig,
            _model: &Model,
            _ctx: &TranscriptContext,
            _options: &SimpleStreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            rx
        }
    }

    fn test_provider(id: &str, models: Vec<Model>) -> Arc<dyn Provider> {
        test_provider_with_auth_and_api(
            id,
            models,
            ambient_auth(),
            ApiImpls::Single(Arc::new(StubApi)),
        )
    }

    fn test_provider_with_auth(
        id: &str,
        models: Vec<Model>,
        auth: ProviderAuth,
    ) -> Arc<dyn Provider> {
        test_provider_with_auth_and_api(id, models, auth, ApiImpls::Single(Arc::new(StubApi)))
    }

    fn test_provider_with_auth_and_api(
        id: &str,
        models: Vec<Model>,
        auth: ProviderAuth,
        api: ApiImpls,
    ) -> Arc<dyn Provider> {
        create_provider(CreateProviderOptions {
            filter_all_models: None,
            images: crate::ai::models::provider::ImagesImpls::new(),
            classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
            id: id.to_string(),
            name: None,
            base_url: None,
            headers: None,
            auth,
            models: models
                .into_iter()
                .map(crate::ai::types::AnyModel::Chat)
                .collect(),
            fetch_models: None,
            filter_models: None,
            api,
        })
    }

    fn provider_ids(models: &Models) -> Vec<String> {
        models
            .get_providers()
            .iter()
            .map(|p| p.id().to_string())
            .collect()
    }

    fn model_ids(models: &[Model]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    /// Upstream models-runtime.test.ts "registers, replaces, and deletes
    /// providers".
    #[test]
    fn registers_replaces_and_deletes_providers() {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider("p1", vec![]));
        models.set_provider(test_provider("p2", vec![]));
        assert_eq!(provider_ids(&models), ["p1", "p2"]);

        let replacement = test_provider("p1", vec![]);
        models.set_provider(Arc::clone(&replacement));
        // The registry hands back the replacement itself.
        assert!(Arc::ptr_eq(
            &replacement,
            &models.get_provider("p1").unwrap()
        ));
        assert_eq!(models.get_providers().len(), 2);
        // Replacement keeps the original position (JS Map semantics).
        assert_eq!(provider_ids(&models), ["p1", "p2"]);

        models.delete_provider("p1");
        assert!(models.get_provider("p1").is_none());
        assert_eq!(provider_ids(&models), ["p2"]);

        models.clear_providers();
        assert!(models.get_providers().is_empty());
    }

    /// Upstream "lists and finds models per provider".
    #[test]
    fn lists_and_finds_models_per_provider() {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider(
            "p1",
            vec![test_model("p1", "m1"), test_model("p1", "m2")],
        ));
        models.set_provider(test_provider("p2", vec![test_model("p2", "m3")]));

        assert_eq!(model_ids(&models.get_models(None)), ["m1", "m2", "m3"]);
        assert_eq!(model_ids(&models.get_models(Some("p1"))), ["m1", "m2"]);
        assert!(models.get_models(Some("nope")).is_empty());
        assert_eq!(models.get_model("p2", "m3").unwrap().id, "m3");
        assert!(models.get_model("p2", "missing").is_none());

        // Upstream narrows dynamically looked-up models with hasApi(); the
        // port compares `model.api` directly.
        let found = models.get_model("p2", "m3").unwrap();
        assert_ne!(found.api, "openai-completions");
        assert_eq!(found.api, "test-api");
    }

    /// Upstream "swallows provider source failures for both all-provider and
    /// single-provider listing".
    #[test]
    fn swallows_provider_source_failures() {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::new(BrokenProvider {
            id: "broken".to_string(),
            auth: ambient_auth(),
        }));
        models.set_provider(test_provider("ok", vec![test_model("ok", "m1")]));

        assert_eq!(model_ids(&models.get_models(None)), ["m1"]);
        assert!(models.get_models(Some("broken")).is_empty());
        // Precise failures come from the provider directly (upstream: the
        // provider's own getModels() throws "boom").
        let error = models
            .get_provider("broken")
            .unwrap()
            .get_models()
            .unwrap_err();
        assert_eq!(error.message, "boom");
    }

    /// Upstream testProvider fixture with a throwing `getModels`
    /// (models-runtime.test.ts:201-217).
    struct BrokenProvider {
        id: String,
        auth: ProviderAuth,
    }

    impl Provider for BrokenProvider {
        fn id(&self) -> &str {
            &self.id
        }

        fn name(&self) -> &str {
            &self.id
        }

        fn auth(&self) -> &ProviderAuth {
            &self.auth
        }

        fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
            Err(ModelsError::new(ModelsErrorCode::ModelSource, "boom"))
        }
    }

    /// Upstream `createModels` defaults (models.ts:275-279): no options means
    /// an empty in-memory credential store behind the collection.
    #[test]
    fn create_models_starts_empty_and_registers_providers() {
        let mut models = create_models(CreateModelsOptions::default());
        assert!(models.get_models(None).is_empty());
        assert!(models.get_provider("p1").is_none());
        models.set_provider(test_provider(
            "p1",
            vec![test_model("p1", "m1"), test_model("p1", "m2")],
        ));
        assert_eq!(model_ids(&models.get_models(None)), ["m1", "m2"]);
    }

    // ===================================================================
    // Task 3: Models.getAuth + stream routing (upstream models.ts:556-575,
    // 640-718; oracle models-runtime.test.ts getAuth/stream halves).
    // ===================================================================

    /// Oracle `doneMessage` (models-runtime.test.ts:24-42): the scripted
    /// final message for fixture streams.
    fn done_message(model: &Model) -> AssistantMessage {
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
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: now_ms(),
        }
    }

    /// Oracle `context` (models-runtime.test.ts:84).
    fn user_context() -> Context {
        Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                content: StringOrBlocks::Text("hi".to_string()),
                timestamp: now_ms(),
            })],
            tools: None,
        }
    }

    fn header_map(pairs: &[(&str, &str)]) -> ProviderHeaders {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), Some((*value).to_string())))
            .collect()
    }

    /// Header map for [`Model::headers`]: the same `Option`-valued shape as
    /// options-level [`ProviderHeaders`].
    fn string_header_map(pairs: &[(&str, &str)]) -> BTreeMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), Some((*value).to_string())))
            .collect()
    }

    /// Oracle `envKeyAuth` (models-runtime.test.ts:86-95): the stored
    /// credential's key wins, otherwise the fixture key; source labels
    /// "stored" vs "env".
    struct EnvKeyAuthFixture {
        name: &'static str,
        key: Option<String>,
    }

    impl EnvKeyAuthFixture {
        fn env(key: &str) -> Arc<Self> {
            Arc::new(EnvKeyAuthFixture {
                name: "Test API key",
                key: Some(key.to_string()),
            })
        }

        fn missing() -> Arc<Self> {
            Arc::new(EnvKeyAuthFixture {
                name: "Test API key",
                key: None,
            })
        }
    }

    impl ApiKeyAuth for EnvKeyAuthFixture {
        fn name(&self) -> &str {
            self.name
        }

        fn resolve<'a>(
            &'a self,
            input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async move {
                let resolved = input
                    .credential
                    .and_then(|credential| credential.key.clone())
                    .or_else(|| self.key.clone());
                Ok(resolved.map(|key| AuthResult {
                    auth: ModelAuth {
                        api_key: Some(key),
                        ..ModelAuth::default()
                    },
                    env: None,
                    source: Some(
                        if input.credential.is_some() {
                            "stored"
                        } else {
                            "env"
                        }
                        .to_string(),
                    ),
                }))
            })
        }
    }

    /// Oracle `testOAuth` (models-runtime.test.ts:97-106): `to_auth` derives
    /// the api key from the access token; refresh passes the credential
    /// through unless configured to fail or to return a rotated credential.
    struct TestOAuth {
        refresh_error: Option<&'static str>,
        /// When set, refresh returns this rotated credential (oracle
        /// "refreshes expired OAuth before refreshing models",
        /// models-runtime.test.ts:428-460).
        rotated: Option<OAuthCredential>,
        refresh_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl TestOAuth {
        fn no_refresh() -> Arc<Self> {
            Arc::new(TestOAuth {
                refresh_error: None,
                rotated: None,
                refresh_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            })
        }

        fn failing(message: &'static str) -> Arc<Self> {
            Arc::new(TestOAuth {
                refresh_error: Some(message),
                rotated: None,
                refresh_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            })
        }

        fn rotating(credential: OAuthCredential) -> Arc<Self> {
            Arc::new(TestOAuth {
                refresh_error: None,
                rotated: Some(credential),
                refresh_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            })
        }

        fn refresh_calls(&self) -> usize {
            self.refresh_calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl OAuthAuth for TestOAuth {
        fn name(&self) -> &str {
            "Test OAuth"
        }

        fn login<'a>(
            &'a self,
            _interaction: ProviderAuthInteraction,
        ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
            Box::pin(async { Err(AuthError::Operation("not used".to_string())) })
        }

        fn refresh<'a>(
            &'a self,
            credential: OAuthCredential,
            _options: &'a AuthOperationOptions,
        ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
            Box::pin(async move {
                self.refresh_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match self.refresh_error {
                    Some(message) => Err(AuthError::Operation(message.to_string())),
                    None => Ok(self.rotated.clone().unwrap_or(credential)),
                }
            })
        }

        fn to_auth<'a>(
            &'a self,
            credential: OAuthCredential,
        ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
            Box::pin(async move {
                Ok(ModelAuth {
                    api_key: Some(credential.access),
                    ..ModelAuth::default()
                })
            })
        }
    }

    /// Oracle "Test" auth in "merges resolved auth into stream options"
    /// (models-runtime.test.ts:1078-1087): resolves fixed auth values.
    struct ResolvingKeyAuth {
        result: AuthResult,
    }

    impl ApiKeyAuth for ResolvingKeyAuth {
        fn name(&self) -> &str {
            "Test"
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async { Ok(Some(self.result.clone())) })
        }
    }

    /// Oracle failing resolver (models-runtime.test.ts:1041-1046).
    struct FailingKeyAuth;

    impl ApiKeyAuth for FailingKeyAuth {
        fn name(&self) -> &str {
            "Failing"
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async { Err(AuthError::Operation("nope".to_string())) })
        }
    }

    /// Oracle read-failing credential store (models-runtime.test.ts:992-999).
    struct ReadFailingStore;

    impl CredentialStore for ReadFailingStore {
        fn read<'a>(
            &'a self,
            _provider_id: &'a str,
            _options: &'a AuthOperationOptions,
        ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
            Box::pin(async { Err(AuthError::Storage("disk on fire".to_string())) })
        }

        fn list<'a>(
            &'a self,
            _options: &'a AuthOperationOptions,
        ) -> BoxFuture<'a, Result<Vec<CredentialInfo>, AuthError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn modify<'a>(
            &'a self,
            _provider_id: &'a str,
            _f: ModifyCallback,
            _options: &'a AuthOperationOptions,
        ) -> BoxFuture<'a, Result<Option<Credential>, AuthError>> {
            Box::pin(async { Ok(None) })
        }

        fn delete<'a>(
            &'a self,
            _provider_id: &'a str,
            _options: &'a AuthOperationOptions,
        ) -> BoxFuture<'a, Result<(), AuthError>> {
            Box::pin(async { Ok(()) })
        }
    }

    async fn store_credential(
        store: &InMemoryCredentialStore,
        provider: &str,
        credential: Credential,
    ) {
        let slot = std::sync::Mutex::new(Some(credential));
        let callback: ModifyCallback = Box::new(move |_| {
            let next = slot.lock().unwrap().take();
            Box::pin(async move { Ok(next) })
        });
        store
            .modify(provider, callback, &AuthOperationOptions::NONE)
            .await
            .unwrap();
    }

    fn api_key_credential(key: &str) -> Credential {
        Credential::ApiKey(ApiKeyCredential {
            key: Some(key.to_string()),
            env: None,
            extra: Default::default(),
        })
    }

    fn oauth_credential(access: &str, expires_in_ms: i64) -> Credential {
        Credential::OAuth(OAuthCredential {
            refresh: "r".to_string(),
            access: access.to_string(),
            expires: now_ms() + expires_in_ms,
            extra: Default::default(),
        })
    }

    /// A `RecordedCall` per ApiImpl invocation (upstream `ProviderCall`,
    /// models-runtime.test.ts:44-47): the routed config/model plus the
    /// request options as the ApiImpl received them.
    #[derive(Clone)]
    struct RecordedCall {
        config: ProviderConfig,
        model: Model,
        stream: Option<StreamOptions>,
        simple: Option<SimpleStreamOptions>,
    }

    /// Oracle `testProvider.respond` (models-runtime.test.ts:64-72) as an
    /// ApiImpl: records the call, then streams a scripted `start`+`done`.
    struct RecordingApi {
        calls: std::sync::Mutex<Vec<RecordedCall>>,
    }

    impl RecordingApi {
        fn new() -> Arc<Self> {
            Arc::new(RecordingApi {
                calls: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn recorded(&self) -> Vec<RecordedCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    fn scripted_stream(model: &Model) -> mpsc::Receiver<AssistantMessageEvent> {
        let (tx, rx) = mpsc::channel(4);
        let message = done_message(model);
        tokio::spawn(async move {
            let _ = tx
                .send(AssistantMessageEvent::Start {
                    message: message.clone(),
                })
                .await;
            let _ = tx
                .send(AssistantMessageEvent::Done {
                    reason: SuccessReason::Stop,
                    message,
                })
                .await;
        });
        rx
    }

    impl ApiImpl for RecordingApi {
        fn stream(
            &self,
            config: &ProviderConfig,
            model: &Model,
            _ctx: &TranscriptContext,
            options: &StreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            self.calls.lock().unwrap().push(RecordedCall {
                config: config.clone(),
                model: model.clone(),
                stream: Some(options.clone()),
                simple: None,
            });
            scripted_stream(model)
        }

        fn stream_simple(
            &self,
            config: &ProviderConfig,
            model: &Model,
            _ctx: &TranscriptContext,
            options: &SimpleStreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            self.calls.lock().unwrap().push(RecordedCall {
                config: config.clone(),
                model: model.clone(),
                stream: None,
                simple: Some(options.clone()),
            });
            scripted_stream(model)
        }
    }

    fn auth_with(api_key: Arc<dyn ApiKeyAuth>) -> ProviderAuth {
        ProviderAuth {
            api_key: Some(api_key),
            oauth: None,
        }
    }

    async fn collect_message(rx: mpsc::Receiver<AssistantMessageEvent>) -> AssistantMessage {
        let mut partial = PartialAssistant::new();
        let mut rx = rx;
        while let Some(event) = rx.recv().await {
            let outcome = partial.apply(&event);
            if let Err(error) = outcome {
                panic!("reducer rejected {}: {error}", event.event_type());
            }
        }
        partial.message().cloned().unwrap()
    }

    /// Oracle "resolves auth: stored credential owns the provider, ambient
    /// only when nothing stored" (models-runtime.test.ts:777-804).
    #[tokio::test]
    async fn get_auth_resolves_through_the_owning_provider() {
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            models_store: None,
            auth_context: None,
        });
        models.set_provider(test_provider_with_auth(
            "p1",
            vec![test_model("p1", "model-a")],
            ProviderAuth {
                api_key: Some(EnvKeyAuthFixture::env("env-key")),
                oauth: Some(TestOAuth::no_refresh()),
            },
        ));
        let model = test_model("p1", "model-a");

        // model and provider-id forms resolve the same provider-scoped auth
        let model_auth = models
            .get_auth_for_model(&model, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(model_auth.auth.api_key.as_deref(), Some("env-key"));
        let provider_auth = models.get_auth("p1", None).await.unwrap().unwrap();
        assert_eq!(provider_auth.auth.api_key.as_deref(), Some("env-key"));
        let overrides = AuthResolutionOverrides {
            api_key: Some("explicit-key".to_string()),
            ..AuthResolutionOverrides::default()
        };
        let explicit = models
            .get_auth_for_model(&model, Some(&overrides))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(explicit.auth.api_key.as_deref(), Some("explicit-key"));

        // stored oauth credential (persisted via the single write path):
        // beats ambient env
        store_credential(
            &credentials,
            "p1",
            oauth_credential("oauth-token", 10 * 60_000),
        )
        .await;
        let resolution = models.get_auth("p1", None).await.unwrap().unwrap();
        assert_eq!(resolution.auth.api_key.as_deref(), Some("oauth-token"));
        assert_eq!(resolution.source.as_deref(), Some("OAuth"));

        // stored api-key credential resolves through apiKey auth, beats env
        store_credential(&credentials, "p1", api_key_credential("stored-key")).await;
        let resolution = models.get_auth("p1", None).await.unwrap().unwrap();
        assert_eq!(resolution.auth.api_key.as_deref(), Some("stored-key"));
        assert_eq!(resolution.source.as_deref(), Some("stored"));
    }

    /// Oracle "adds model headers only for model auth" first half
    /// (models-runtime.test.ts:1108-1116).
    #[tokio::test]
    async fn get_auth_merges_model_headers_only_for_the_model_form() {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider_with_auth(
            "p1",
            vec![test_model("p1", "model-a")],
            ambient_auth(),
        ));
        let mut model = test_model("p1", "model-a");
        model.headers = Some(string_header_map(&[
            ("x-model", "model"),
            ("x-shared", "model"),
        ]));

        let provider_auth = models.get_auth("p1", None).await.unwrap().unwrap();
        assert!(provider_auth.auth.headers.is_none());
        let model_auth = models
            .get_auth_for_model(&model, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            model_auth.auth.headers.as_ref(),
            Some(&header_map(&[("x-model", "model"), ("x-shared", "model")]))
        );
    }

    /// Oracle "a stored credential without a matching handler blocks ambient
    /// fallback" (models-runtime.test.ts:858-866) plus the unknown-provider
    /// shape of `getAuth`.
    #[tokio::test]
    async fn get_auth_stays_undefined_for_unknown_providers_and_unhandled_credentials() {
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            models_store: None,
            auth_context: None,
        });
        models.set_provider(test_provider_with_auth(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(EnvKeyAuthFixture::env("env-key")),
        ));
        // stale oauth credential on an api-key-only provider
        store_credential(&credentials, "p1", oauth_credential("a", 0)).await;

        assert!(models.get_auth("nope", None).await.unwrap().is_none());
        assert!(models.get_auth("p1", None).await.unwrap().is_none());
    }

    /// Oracle "wraps credential store failures" read half + "wraps api-key
    /// auth failures" + "rejects with code oauth when refresh fails"
    /// (models-runtime.test.ts:990-1050) — through the `Models.getAuth`
    /// caller.
    #[tokio::test]
    async fn get_auth_wraps_store_resolver_and_refresh_failures() {
        // read failure -> code "auth"
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::new(ReadFailingStore)),
            models_store: None,
            auth_context: None,
        });
        models.set_provider(test_provider_with_auth(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(EnvKeyAuthFixture::env("env-key")),
        ));
        let error = models.get_auth("p1", None).await.unwrap_err();
        let ModelsError { code, .. } = match error {
            AuthError::Models(error) => error,
            other => panic!("expected ModelsError, got {other:?}"),
        };
        assert_eq!(code, ModelsErrorCode::Auth);

        // failing resolver -> code "auth"
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider_with_auth(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(Arc::new(FailingKeyAuth)),
        ));
        let error = models.get_auth("p1", None).await.unwrap_err();
        match error {
            AuthError::Models(error) => assert_eq!(error.code, ModelsErrorCode::Auth),
            other => panic!("expected ModelsError, got {other:?}"),
        }

        // failed oauth refresh -> code "oauth"
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let mut oauth_models = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            models_store: None,
            auth_context: None,
        });
        oauth_models.set_provider(test_provider_with_auth(
            "p1",
            vec![test_model("p1", "model-a")],
            ProviderAuth {
                api_key: None,
                oauth: Some(TestOAuth::failing("invalid_grant")),
            },
        ));
        store_credential(&credentials, "p1", oauth_credential("old", 0)).await;
        let error = oauth_models.get_auth("p1", None).await.unwrap_err();
        match error {
            AuthError::Models(error) => assert_eq!(error.code, ModelsErrorCode::OAuth),
            other => panic!("expected ModelsError, got {other:?}"),
        }
        // the credential is preserved for retry / re-login
        let stored = credentials
            .read("p1", &AuthOperationOptions::NONE)
            .await
            .unwrap()
            .unwrap();
        match stored {
            Credential::OAuth(stored) => assert_eq!(stored.access, "old"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
    }

    /// Oracle "streams through the provider" (models-runtime.test.ts:1145-1158)
    /// plus routing to the owning provider's ApiImpl with the resolved key.
    #[tokio::test]
    async fn stream_and_complete_route_through_the_owning_provider() {
        let api = RecordingApi::new();
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider_with_auth_and_api(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(EnvKeyAuthFixture::env("env-key")),
            ApiImpls::Single(Arc::clone(&api) as Arc<dyn ApiImpl>),
        ));
        let model = test_model("p1", "model-a");
        let context = user_context();

        // stream_simple forwards the scripted start/done pair and reduces to
        // the final message through complete_simple.
        let message = collect_message(models.stream_simple(&model, &context, None)).await;
        assert_eq!(message.stop_reason, StopReason::Stop);
        let message = models.complete_simple(&model, &context, None).await;
        assert_eq!(message.stop_reason, StopReason::Stop);

        let recorded = api.recorded();
        assert_eq!(recorded.len(), 2);
        // stream_simple dispatches through the simple entry point with the
        // resolved key riding both the routed config and the options.
        assert!(recorded[0].stream.is_none());
        assert!(recorded[0].simple.is_some());
        assert_eq!(recorded[0].config.api_key, "env-key");
        assert_eq!(
            recorded[0]
                .simple
                .as_ref()
                .unwrap()
                .stream
                .api_key
                .as_deref(),
            Some("env-key")
        );
    }

    /// Oracle "merges resolved auth into stream options; explicit options win
    /// per field" (models-runtime.test.ts:1076-1106). The port asserts on the
    /// routed `ProviderConfig` (the ApiImpls' base_url/api_key channel).
    #[tokio::test]
    async fn stream_merges_resolved_auth_and_explicit_options_win() {
        let api = RecordingApi::new();
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider_with_auth_and_api(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(Arc::new(ResolvingKeyAuth {
                result: AuthResult {
                    auth: ModelAuth {
                        api_key: Some("resolved-key".to_string()),
                        headers: Some(header_map(&[
                            ("Authorization", "Bearer resolved-key"),
                            ("x-a", "auth"),
                            ("x-b", "auth"),
                        ])),
                        base_url: Some("https://auth.test/v1".to_string()),
                    },
                    env: None,
                    source: None,
                },
            })),
            ApiImpls::Single(Arc::clone(&api) as Arc<dyn ApiImpl>),
        ));
        let model = test_model("p1", "model-a");
        let context = user_context();

        let options = ModelsSimpleStreamOptions {
            simple: SimpleStreamOptions {
                stream: StreamOptions {
                    api_key: Some("explicit-key".to_string()),
                    headers: Some(header_map(&[
                        ("authorization", "Explicit token"),
                        ("x-b", "explicit"),
                    ])),
                    ..StreamOptions::default()
                },
                ..SimpleStreamOptions::default()
            },
            transform_headers: None,
        };
        let message = models
            .complete_simple(&model, &context, Some(options))
            .await;
        assert_eq!(message.stop_reason, StopReason::Stop);

        let recorded = api.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].config.api_key, "explicit-key");
        assert_eq!(recorded[0].config.base_url, "https://auth.test/v1");
        // upstream `calls[0].model.baseUrl`: the request model carries the
        // auth-derived base URL for ApiImpls that read it.
        assert_eq!(recorded[0].model.base_url, "https://auth.test/v1");
        let simple = recorded[0].simple.as_ref().unwrap();
        assert_eq!(simple.stream.api_key.as_deref(), Some("explicit-key"));
        let headers = simple.stream.headers.as_ref().unwrap();
        assert_eq!(
            headers.get("authorization").unwrap(),
            &Some("Explicit token".to_string())
        );
        assert_eq!(headers.get("x-a").unwrap(), &Some("auth".to_string()));
        assert_eq!(headers.get("x-b").unwrap(), &Some("explicit".to_string()));

        // without explicit options, resolved auth applies
        models.complete_simple(&model, &context, None).await;
        let recorded = api.recorded();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[1].config.api_key, "resolved-key");
        assert_eq!(
            recorded[1]
                .simple
                .as_ref()
                .unwrap()
                .stream
                .api_key
                .as_deref(),
            Some("resolved-key")
        );
    }

    /// The request signal rides the routing: a token set on the caller's
    /// options reaches the owning provider's ApiImpl on both option shapes
    /// (upstream threads `options.signal` through `applyAuth` into the API
    /// call), and it also scopes the auth resolution itself.
    #[tokio::test]
    async fn stream_threads_the_request_signal_to_the_api_impl() {
        let api = RecordingApi::new();
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider_with_auth_and_api(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(EnvKeyAuthFixture::env("env-key")),
            ApiImpls::Single(Arc::clone(&api) as Arc<dyn ApiImpl>),
        ));
        let model = test_model("p1", "model-a");
        let context = user_context();

        let token = CancellationToken::new();
        let message = models
            .complete(
                &model,
                &context,
                Some(ModelsApiStreamOptions {
                    stream: StreamOptions {
                        signal: Some(token.clone()),
                        ..StreamOptions::default()
                    },
                    transform_headers: None,
                }),
            )
            .await;
        assert_eq!(message.stop_reason, StopReason::Stop);
        // A clone of the recorded token shares the cancellation state with
        // the caller's token (tokio-util token equality is pointer identity).
        let recorded = api.recorded();
        let routed = recorded[0]
            .stream
            .as_ref()
            .and_then(|options| options.signal.clone())
            .expect("api signal recorded");
        assert!(routed.is_cancelled() == token.is_cancelled());
        token.cancel();
        assert!(routed.is_cancelled());

        let simple_token = CancellationToken::new();
        let _ = models
            .complete_simple(
                &model,
                &context,
                Some(ModelsSimpleStreamOptions {
                    simple: SimpleStreamOptions {
                        stream: StreamOptions {
                            signal: Some(simple_token.clone()),
                            ..StreamOptions::default()
                        },
                        ..SimpleStreamOptions::default()
                    },
                    transform_headers: None,
                }),
            )
            .await;
        let recorded = api.recorded();
        let routed = recorded[1]
            .simple
            .as_ref()
            .and_then(|options| options.stream.signal.clone())
            .expect("api signal recorded");
        simple_token.cancel();
        assert!(routed.is_cancelled());
    }

    /// Oracle "adds model headers only for model auth and transforms
    /// assembled headers once" second half (models-runtime.test.ts:1118-1135).
    #[tokio::test]
    async fn stream_transforms_assembled_headers_once() {
        let api = RecordingApi::new();
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider_with_auth_and_api(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(EnvKeyAuthFixture::env("key")),
            ApiImpls::Single(Arc::clone(&api) as Arc<dyn ApiImpl>),
        ));
        let mut model = test_model("p1", "model-a");
        model.headers = Some(string_header_map(&[
            ("x-model", "model"),
            ("x-shared", "model"),
        ]));

        let transforms = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transform_counter = Arc::clone(&transforms);
        let transform_headers: TransformHeaders = Arc::new(move |headers: ProviderHeaders| {
            let counter = Arc::clone(&transform_counter);
            Box::pin(async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // The transform sees auth/model/explicit headers merged,
                // with the explicit case-different name having replaced
                // the model's.
                assert_eq!(headers.get("x-model").unwrap(), &Some("model".to_string()));
                assert_eq!(
                    headers.get("x-explicit").unwrap(),
                    &Some("explicit".to_string())
                );
                assert_eq!(
                    headers.get("X-Shared").unwrap(),
                    &Some("explicit".to_string())
                );
                let mut transformed = headers;
                transformed.insert("x-transformed".to_string(), Some("yes".to_string()));
                transformed
            }) as BoxFuture<'static, ProviderHeaders>
        });

        let options = ModelsSimpleStreamOptions {
            simple: SimpleStreamOptions {
                stream: StreamOptions {
                    headers: Some(header_map(&[
                        ("x-explicit", "explicit"),
                        ("X-Shared", "explicit"),
                    ])),
                    ..StreamOptions::default()
                },
                ..SimpleStreamOptions::default()
            },
            transform_headers: Some(transform_headers),
        };
        let message = models
            .complete_simple(&model, &user_context(), Some(options))
            .await;
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(
            transforms.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "transform runs exactly once per request"
        );

        let recorded = api.recorded();
        assert_eq!(recorded.len(), 1);
        let headers = recorded[0]
            .simple
            .as_ref()
            .unwrap()
            .stream
            .headers
            .as_ref()
            .unwrap();
        assert_eq!(headers.get("x-model").unwrap(), &Some("model".to_string()));
        assert_eq!(
            headers.get("x-explicit").unwrap(),
            &Some("explicit".to_string())
        );
        assert_eq!(
            headers.get("X-Shared").unwrap(),
            &Some("explicit".to_string())
        );
        assert_eq!(
            headers.get("x-transformed").unwrap(),
            &Some("yes".to_string())
        );
    }

    /// Oracle "produces an error stream for unknown providers instead of
    /// throwing" (models-runtime.test.ts:1138-1143), plus the unconfigured
    /// and no-API-implementation routing errors (models.ts:643, 662, 810).
    #[tokio::test]
    async fn stream_produces_error_events_for_routing_failures() {
        let mut models = create_models(CreateModelsOptions::default());
        // Upstream rejects an EMPTY api map at createProvider (the delta:
        // at least one of api/images/classifiers is required); a map keyed
        // by a different api reproduces the routing-miss error path.
        models.set_provider(test_provider_with_auth_and_api(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(EnvKeyAuthFixture::missing()),
            ApiImpls::PerApi(BTreeMap::from([(
                "other-api".to_string(),
                Arc::new(StubApi) as Arc<dyn ApiImpl>,
            )])),
        ));
        let context = user_context();

        // unknown provider
        let ghost = test_model("ghost", "model-a");
        let message = models.complete_simple(&ghost, &context, None).await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(
            message
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("Unknown provider: ghost"),
            "unexpected error message: {:?}",
            message.error_message
        );

        // unconfigured provider (resolution resolves to nothing)
        let message = models
            .complete_simple(&test_model("p1", "model-a"), &context, None)
            .await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Provider is not configured: p1")
        );

        // no API implementation for the model's api (auth resolves first)
        let mut configured = create_models(CreateModelsOptions::default());
        configured.set_provider(test_provider_with_auth_and_api(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(EnvKeyAuthFixture::env("key")),
            ApiImpls::PerApi(BTreeMap::from([(
                "other-api".to_string(),
                Arc::new(StubApi) as Arc<dyn ApiImpl>,
            )])),
        ));
        let message = configured
            .complete_simple(&test_model("p1", "model-a"), &context, None)
            .await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Provider p1 has no API implementation for \"test-api\"")
        );
    }

    /// In-memory env lookup for the wiremock round trip (the resolve.rs test
    /// pattern): avoids process-env global state.
    struct MapAuthContext {
        vars: BTreeMap<String, String>,
    }

    impl AuthContext for MapAuthContext {
        fn env<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<String>> {
            Box::pin(async move { self.vars.get(name).cloned() })
        }

        fn file_exists<'a>(&'a self, _path: &'a str) -> BoxFuture<'a, bool> {
            Box::pin(async { false })
        }
    }

    /// End-to-end routing over HTTP: a real [`OpenAiCompletions`] ApiImpl
    /// behind a wiremock server; the env-resolved key and the merged
    /// model/auth headers must reach the wire (upstream getClientApiKey /
    /// header assembly consume what `Models` merges in).
    #[tokio::test]
    async fn stream_simple_round_trips_resolved_auth_over_http() {
        use crate::ai::api::openai_completions::OpenAiCompletions;
        use wiremock::matchers::{header, method, path};

        let server = wiremock::MockServer::start().await;
        let body = format!(
            "{}{}{}",
            r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]}"#,
            "\n\n",
            format_args!(
                "{}\n\n{}\n\n",
                r#"data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                "data: [DONE]"
            )
        );
        wiremock::Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer env-key"))
            .and(header("x-model", "model"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut models = create_models(CreateModelsOptions {
            auth_context: Some(Arc::new(MapAuthContext {
                vars: [("P1_API_KEY".to_string(), "env-key".to_string())]
                    .into_iter()
                    .collect(),
            }) as Arc<dyn AuthContext>),
            credentials: None,
            models_store: None,
        });
        let mut per_api: BTreeMap<String, Arc<dyn ApiImpl>> = BTreeMap::new();
        per_api.insert(
            "openai-completions".to_string(),
            Arc::new(OpenAiCompletions),
        );
        let mut model = test_model("p1", "model-a");
        model.api = "openai-completions".to_string();
        model.base_url = format!("{}/v1", server.uri());
        model.headers = Some(string_header_map(&[("x-model", "model")]));
        models.set_provider(test_provider_with_auth_and_api(
            "p1",
            vec![model.clone()],
            auth_with(crate::ai::auth::helpers::env_api_key_auth(
                "P1",
                &["P1_API_KEY"],
            )),
            ApiImpls::PerApi(per_api),
        ));

        let message = models.complete_simple(&model, &user_context(), None).await;
        assert_eq!(message.stop_reason, StopReason::Stop);
        server.verify().await;
    }

    // ===================================================================
    // Task 4: refresh + dynamic providers + models store (upstream
    // models.ts:180-190, 332-554, 823-849; oracle models-runtime.test.ts
    // refresh halves, models-runtime.test.ts:219-615, 806-838).
    // ===================================================================

    use crate::ai::models::store::ModelsStoreOperationOptions as StoreOptions;
    use tokio::sync::Notify;

    /// Shared append-only log for fixture recording.
    #[derive(Clone, Default)]
    struct SharedLog(Arc<std::sync::Mutex<Vec<String>>>);

    impl SharedLog {
        fn push(&self, entry: impl Into<String>) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(entry.into());
        }

        fn entries(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// Behavior of one fixture provider's `refreshModels` invocation (the
    /// upstream `refreshModels` closures): receives the owned phase context.
    type FixtureBehavior = Arc<
        dyn Fn(RefreshModelsContext) -> BoxFuture<'static, Result<(), RefreshModelsError>>
            + Send
            + Sync,
    >;

    /// Oracle `testProvider` (models-runtime.test.ts:55-82) restricted to the
    /// dynamic surface: `getModels` reads a shared list, `refreshModels` runs
    /// a per-test closure, auth is injectable.
    struct DynamicFixture {
        id: String,
        auth: ProviderAuth,
        models: Arc<std::sync::RwLock<Vec<Model>>>,
        behavior: FixtureBehavior,
        filter: Option<FilterModelsFn>,
    }

    impl DynamicFixture {
        fn dynamic(
            id: &str,
            models: Vec<Model>,
            auth: ProviderAuth,
            behavior: FixtureBehavior,
        ) -> Arc<Self> {
            Arc::new(DynamicFixture {
                id: id.to_string(),
                auth,
                models: Arc::new(std::sync::RwLock::new(models)),
                behavior,
                filter: None,
            })
        }

        /// Fixture whose `getModels` reads the same shared list a behavior
        /// mutates through its publications (upstream `getModels: () => list`).
        fn shared(
            id: &str,
            models: Arc<std::sync::RwLock<Vec<Model>>>,
            auth: ProviderAuth,
            behavior: FixtureBehavior,
        ) -> Arc<Self> {
            Arc::new(DynamicFixture {
                id: id.to_string(),
                auth,
                models,
                behavior,
                filter: None,
            })
        }

        fn filtered(
            id: &str,
            models: Vec<Model>,
            auth: ProviderAuth,
            filter: FilterModelsFn,
        ) -> Arc<Self> {
            Arc::new(DynamicFixture {
                id: id.to_string(),
                auth,
                models: Arc::new(std::sync::RwLock::new(models)),
                behavior: Arc::new(|_| Box::pin(async { Ok(()) })),
                filter: Some(filter),
            })
        }
    }

    impl Provider for DynamicFixture {
        fn id(&self) -> &str {
            &self.id
        }

        fn name(&self) -> &str {
            &self.id
        }

        fn auth(&self) -> &ProviderAuth {
            &self.auth
        }

        fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
            Ok(self
                .models
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }

        fn is_dynamic(&self) -> bool {
            true
        }

        fn refresh_models(
            &self,
            context: RefreshModelsContext,
        ) -> Option<BoxFuture<'static, Result<(), RefreshModelsError>>> {
            let behavior = Arc::clone(&self.behavior);
            Some(Box::pin(async move { (behavior)(context).await }))
        }

        fn filter_models(
            &self,
            models: &[Model],
            credential: Option<&Credential>,
        ) -> Option<Vec<Model>> {
            self.filter
                .as_ref()
                .map(|filter| filter(models, credential))
        }
    }

    /// Oracle `createProvider` dynamic provider with an api-key fixture auth.
    fn dynamic_create_provider(
        id: &str,
        auth: ProviderAuth,
        fetch_models: Option<FetchModelsFn>,
    ) -> Arc<dyn Provider> {
        create_provider(CreateProviderOptions {
            filter_all_models: None,
            images: crate::ai::models::provider::ImagesImpls::new(),
            classifiers: crate::ai::models::provider::ClassifiersImpls::new(),
            id: id.to_string(),
            name: None,
            base_url: None,
            headers: None,
            auth,
            models: vec![],
            fetch_models,
            filter_models: None,
            api: ApiImpls::Single(Arc::new(StubApi)),
        })
    }

    fn fixture_error(message: &str) -> RefreshModelsError {
        RefreshModelsError::Failed(ModelsError::new(
            ModelsErrorCode::ModelSource,
            message.to_string(),
        ))
    }

    /// In-memory models store over a shared slot, for store-semantics tests
    /// (upstream custom `ModelsStore` object literals).
    struct SharedStateStore {
        state: Arc<std::sync::Mutex<Option<ModelsStoreEntry>>>,
    }

    impl ModelsStore for SharedStateStore {
        fn read<'a>(
            &'a self,
            provider_id: &'a str,
            _options: &'a StoreOptions,
        ) -> BoxFuture<'a, Result<Option<ModelsStoreEntry>, ModelsStoreError>> {
            Box::pin(async move {
                let _ = provider_id;
                Ok(self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone())
            })
        }

        fn write<'a>(
            &'a self,
            _provider_id: &'a str,
            entry: ModelsStoreEntry,
            _options: &'a StoreOptions,
        ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
            Box::pin(async move {
                *self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(entry);
                Ok(())
            })
        }

        fn delete<'a>(
            &'a self,
            _provider_id: &'a str,
            _options: &'a StoreOptions,
        ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
            Box::pin(async move {
                *self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                Ok(())
            })
        }
    }

    /// Models store recording the cancellation token each operation received
    /// (oracle "binds model-store waits to the provider refresh signal",
    /// models-runtime.test.ts:480-513).
    struct SignalRecordingStore {
        signals: std::sync::Mutex<Vec<Option<CancellationToken>>>,
    }

    impl SignalRecordingStore {
        fn new() -> Self {
            SignalRecordingStore {
                signals: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<CancellationToken> {
            self.signals
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|signal| signal.clone().unwrap())
                .collect()
        }
    }

    impl ModelsStore for SignalRecordingStore {
        fn read<'a>(
            &'a self,
            _provider_id: &'a str,
            options: &'a StoreOptions,
        ) -> BoxFuture<'a, Result<Option<ModelsStoreEntry>, ModelsStoreError>> {
            Box::pin(async move {
                self.signals
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(options.signal.clone());
                Ok(None)
            })
        }

        fn write<'a>(
            &'a self,
            _provider_id: &'a str,
            _entry: ModelsStoreEntry,
            options: &'a StoreOptions,
        ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
            Box::pin(async move {
                self.signals
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(options.signal.clone());
                Ok(())
            })
        }

        fn delete<'a>(
            &'a self,
            _provider_id: &'a str,
            options: &'a StoreOptions,
        ) -> BoxFuture<'a, Result<(), ModelsStoreError>> {
            Box::pin(async move {
                self.signals
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(options.signal.clone());
                Ok(())
            })
        }
    }

    /// Oracle "Blocked auth" (models-runtime.test.ts:294-301): resolution
    /// signals it started, then parks until released.
    struct BlockedResolveAuth {
        started: Arc<Notify>,
        finish: Arc<Notify>,
    }

    impl ApiKeyAuth for BlockedResolveAuth {
        fn name(&self) -> &str {
            "Blocked auth"
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            let started = Arc::clone(&self.started);
            let finish = Arc::clone(&self.finish);
            Box::pin(async move {
                started.notify_one();
                finish.notified().await;
                Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some("key".to_string()),
                        ..ModelAuth::default()
                    },
                    env: None,
                    source: None,
                }))
            })
        }
    }

    /// Oracle "Blocked auth" check half (models-runtime.test.ts:671-681):
    /// the availability check signals it started, then parks until released.
    struct BlockingCheckAuth {
        started: Arc<Notify>,
        finish: Arc<Notify>,
    }

    impl ApiKeyAuth for BlockingCheckAuth {
        fn name(&self) -> &str {
            "Blocked auth"
        }

        fn resolve<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> BoxFuture<'a, Result<Option<AuthResult>, AuthError>> {
            Box::pin(async {
                Ok(Some(AuthResult {
                    auth: ModelAuth {
                        api_key: Some("key".to_string()),
                        ..ModelAuth::default()
                    },
                    env: None,
                    source: None,
                }))
            })
        }

        fn check<'a>(
            &'a self,
            _input: ApiKeyAuthInput<'a>,
        ) -> Option<BoxFuture<'a, Result<Option<AuthCheck>, AuthError>>> {
            let started = Arc::clone(&self.started);
            let finish = Arc::clone(&self.finish);
            Some(Box::pin(async move {
                started.notify_one();
                finish.notified().await;
                Ok(Some(AuthCheck {
                    source: None,
                    r#type: AuthType::ApiKey,
                }))
            }))
        }
    }

    /// Oracle "refresh() updates every configured dynamic provider and
    /// reports failures" (models-runtime.test.ts:219-258).
    #[tokio::test]
    async fn refresh_updates_dynamic_providers_and_reports_failures() {
        let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let dyn_models = Arc::new(std::sync::RwLock::new(vec![test_model("dyn", "before")]));
        let behavior_models = Arc::clone(&dyn_models);
        let behavior_refreshes = Arc::clone(&refreshes);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let models = Arc::clone(&behavior_models);
            let refreshes = Arc::clone(&behavior_refreshes);
            Box::pin(async move {
                if !context.allow_network {
                    return Ok(());
                }
                refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let models_for_update = Arc::clone(&models);
                context
                    .publish(ModelsPublication {
                        persist: None,
                        update: Some(Box::new(move || {
                            *models_for_update
                                .write()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                vec![test_model("dyn", "after")];
                        })),
                    })
                    .await?;
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(DynamicFixture::shared(
            "dyn",
            Arc::clone(&dyn_models),
            ambient_auth(),
            behavior,
        ));
        models.set_provider(test_provider("static", vec![test_model("static", "s1")]));

        assert!(models.get_model("dyn", "before").is_some());
        let first = models.refresh(ModelsRefreshOptions::default()).await;
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(models.get_model("dyn", "after").is_some());
        assert!(models.get_model("dyn", "before").is_none());

        let flaky: FixtureBehavior = Arc::new(|context| {
            Box::pin(async move {
                if context.allow_network {
                    return Err(fixture_error("fetch failed"));
                }
                Ok(())
            })
        });
        models.set_provider(DynamicFixture::dynamic(
            "flaky",
            vec![],
            ambient_auth(),
            flaky,
        ));
        let second = models.refresh(ModelsRefreshOptions::default()).await;
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            second
                .errors
                .get("flaky")
                .map(|error| error.message.clone()),
            Some("fetch failed".to_string())
        );
    }

    /// Oracle "restricts refresh work to selected providers"
    /// (models-runtime.test.ts:260-278).
    #[tokio::test]
    async fn refresh_restricts_work_to_selected_providers() {
        let log = SharedLog::default();
        let mut models = create_models(CreateModelsOptions::default());
        for id in ["one", "two"] {
            let entries = log.clone();
            let behavior: FixtureBehavior = Arc::new(move |context| {
                let entries = entries.clone();
                let id = id.to_string();
                Box::pin(async move {
                    entries.push(format!(
                        "{id}:{}",
                        if context.allow_network {
                            "network"
                        } else {
                            "cache"
                        }
                    ));
                    Ok(())
                })
            });
            models.set_provider(DynamicFixture::dynamic(
                id,
                vec![],
                ambient_auth(),
                behavior,
            ));
        }

        let result = models
            .refresh(ModelsRefreshOptions {
                providers: Some(vec!["two".to_string(), "unknown".to_string()]),
                ..ModelsRefreshOptions::default()
            })
            .await;

        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(log.entries(), ["two:cache", "two:network"]);
    }

    /// Oracle "restores cached models before waiting for network auth"
    /// (models-runtime.test.ts:280-322): the cached overlay is visible before
    /// the blocked auth resolves, and aborting settles the refresh.
    #[tokio::test]
    async fn refresh_restores_cached_models_before_waiting_for_network_auth() {
        let models_store = Arc::new(InMemoryModelsStore::default());
        models_store
            .write(
                "dynamic",
                ModelsStoreEntry {
                    models: vec![crate::ai::types::AnyModel::Chat(test_model(
                        "dynamic", "cached",
                    ))],
                    ..ModelsStoreEntry::default()
                },
                &StoreOptions::NONE,
            )
            .await
            .unwrap();
        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());
        let fetch_models: FetchModelsFn = Arc::new(|_context| {
            Box::pin(async {
                Err(ModelsError::new(
                    ModelsErrorCode::ModelSource,
                    "must not fetch",
                ))
            })
        });
        let provider = dynamic_create_provider(
            "dynamic",
            ProviderAuth {
                api_key: Some(Arc::new(BlockedResolveAuth {
                    started: Arc::clone(&started),
                    finish: Arc::clone(&finish),
                })),
                oauth: None,
            },
            Some(fetch_models),
        );
        let mut models = create_models(CreateModelsOptions {
            models_store: Some(Arc::clone(&models_store) as Arc<dyn ModelsStore>),
            ..CreateModelsOptions::default()
        });
        models.set_provider(provider);
        let token = CancellationToken::new();

        let orchestrator = {
            let models = &models;
            let started = Arc::clone(&started);
            let token = token.clone();
            async move {
                started.notified().await;
                // The cached catalog is restored before auth resolution.
                assert!(models.get_model("dynamic", "cached").is_some());
                token.cancel();
            }
        };
        let (result, ()) = tokio::join!(
            models.refresh(ModelsRefreshOptions {
                providers: Some(vec!["dynamic".to_string()]),
                signal: Some(token),
                ..ModelsRefreshOptions::default()
            }),
            orchestrator
        );
        assert!(result.aborted);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        finish.notify_one();
    }

    /// Oracle "lets providers choose persistent deletion and ephemeral
    /// publication atomically" (models-runtime.test.ts:324-363).
    #[tokio::test]
    async fn refresh_deletes_persistently_and_publishes_ephemerally_atomically() {
        let state = Arc::new(std::sync::Mutex::new(Some(ModelsStoreEntry {
            models: vec![crate::ai::types::AnyModel::Chat(test_model(
                "dynamic", "stored",
            ))],
            ..ModelsStoreEntry::default()
        })));
        let store = SharedStateStore {
            state: Arc::clone(&state),
        };
        let catalog_state = Arc::new(std::sync::Mutex::new("initial".to_string()));
        let behavior_state = Arc::clone(&catalog_state);
        let behavior_entry = Arc::clone(&state);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let behavior_state = behavior_state.clone();
            let behavior_entry = behavior_entry.clone();
            Box::pin(async move {
                assert_eq!(context.stored.as_ref().unwrap().models[0].id(), "stored");
                let state_for_update = Arc::clone(&behavior_state);
                let entry_for_update = Arc::clone(&behavior_entry);
                let applied = context
                    .publish(ModelsPublication {
                        persist: Some(None),
                        update: Some(Box::new(move || {
                            assert!(entry_for_update
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .is_none());
                            *state_for_update
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                "deleted".to_string();
                        })),
                    })
                    .await?;
                assert!(applied);
                let state_for_update = Arc::clone(&behavior_state);
                context
                    .publish(ModelsPublication {
                        persist: None,
                        update: Some(Box::new(move || {
                            *state_for_update
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                "ephemeral".to_string();
                        })),
                    })
                    .await?;
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions {
            models_store: Some(Arc::new(store)),
            ..CreateModelsOptions::default()
        });
        models.set_provider(DynamicFixture::dynamic(
            "dynamic",
            vec![],
            ambient_auth(),
            behavior,
        ));

        let result = models
            .refresh(ModelsRefreshOptions {
                allow_network: Some(false),
                ..ModelsRefreshOptions::default()
            })
            .await;

        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none());
        assert_eq!(
            *catalog_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "ephemeral"
        );
    }

    /// Oracle "persists dynamic catalogs and restores them without network
    /// access" (models-runtime.test.ts:365-394).
    #[tokio::test]
    async fn refresh_persists_dynamic_catalogs_and_restores_them_offline() {
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let models_store = Arc::new(InMemoryModelsStore::default());
        store_credential(&credentials, "dynamic", api_key_credential("key")).await;

        let fetched = test_model("dynamic", "fetched");
        let fetch_models: FetchModelsFn = Arc::new(move |_context| {
            let fetched = fetched.clone();
            Box::pin(async move { Ok(vec![crate::ai::types::AnyModel::Chat(fetched)]) })
        });
        let mut online = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            models_store: Some(Arc::clone(&models_store) as Arc<dyn ModelsStore>),
            auth_context: None,
        });
        online.set_provider(dynamic_create_provider(
            "dynamic",
            auth_with(EnvKeyAuthFixture::missing()),
            Some(fetch_models),
        ));
        let result = online.refresh(ModelsRefreshOptions::default()).await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(online.get_model("dynamic", "fetched").is_some());

        let must_not_fetch: FetchModelsFn = Arc::new(|_context| {
            Box::pin(async {
                Err(ModelsError::new(
                    ModelsErrorCode::ModelSource,
                    "must not fetch",
                ))
            })
        });
        let mut offline = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            models_store: Some(Arc::clone(&models_store) as Arc<dyn ModelsStore>),
            auth_context: None,
        });
        offline.set_provider(dynamic_create_provider(
            "dynamic",
            auth_with(EnvKeyAuthFixture::missing()),
            Some(must_not_fetch),
        ));
        let result = offline
            .refresh(ModelsRefreshOptions {
                allow_network: Some(false),
                ..ModelsRefreshOptions::default()
            })
            .await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(offline.get_model("dynamic", "fetched").is_some());
    }

    /// Oracle "passes effective API-key credentials and refresh options while
    /// skipping unconfigured providers" (models-runtime.test.ts:396-426).
    #[tokio::test]
    async fn refresh_passes_effective_credential_and_force_and_skips_unconfigured() {
        let observed = Arc::new(std::sync::Mutex::new(None::<Credential>));
        let observed_force = Arc::new(std::sync::Mutex::new(None::<bool>));
        let behavior_observed = Arc::clone(&observed);
        let behavior_force = Arc::clone(&observed_force);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let observed = Arc::clone(&behavior_observed);
            let force = Arc::clone(&behavior_force);
            Box::pin(async move {
                if !context.allow_network {
                    return Ok(());
                }
                *observed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    context.credential.clone();
                *force
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = context.force;
                Ok(())
            })
        });
        let unconfigured_refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let behavior_unconfigured = Arc::clone(&unconfigured_refreshes);
        let unconfigured: FixtureBehavior = Arc::new(move |context| {
            let refreshes = Arc::clone(&behavior_unconfigured);
            Box::pin(async move {
                if context.allow_network {
                    refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(DynamicFixture::dynamic(
            "configured",
            vec![],
            auth_with(EnvKeyAuthFixture::env("ambient-key")),
            behavior,
        ));
        models.set_provider(DynamicFixture::dynamic(
            "unconfigured",
            vec![],
            auth_with(EnvKeyAuthFixture::missing()),
            unconfigured,
        ));

        models
            .refresh(ModelsRefreshOptions {
                force: Some(true),
                ..ModelsRefreshOptions::default()
            })
            .await;
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            Some(Credential::ApiKey(ApiKeyCredential {
                key: Some("ambient-key".to_string()),
                env: None,
                extra: Default::default(),
            }))
        );
        assert_eq!(
            *observed_force
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Some(true)
        );
        assert_eq!(
            unconfigured_refreshes.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    /// Oracle "refreshes expired OAuth before refreshing models"
    /// (models-runtime.test.ts:428-460).
    #[tokio::test]
    async fn refresh_refreshes_expired_oauth_before_refreshing_models() {
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let model_refresh_credential = Arc::new(std::sync::Mutex::new(None::<Credential>));
        let rotated = OAuthCredential {
            refresh: "rotated".to_string(),
            access: "fresh".to_string(),
            expires: now_ms() + 60_000,
            extra: Default::default(),
        };
        let oauth = TestOAuth::rotating(rotated.clone());
        store_credential(
            &credentials,
            "oauth-dynamic",
            oauth_credential("expired", 0),
        )
        .await;
        let behavior_observed = Arc::clone(&model_refresh_credential);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let observed = Arc::clone(&behavior_observed);
            Box::pin(async move {
                if context.allow_network {
                    *observed
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        context.credential.clone();
                }
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            ..CreateModelsOptions::default()
        });
        models.set_provider(DynamicFixture::dynamic(
            "oauth-dynamic",
            vec![],
            ProviderAuth {
                api_key: None,
                oauth: Some(Arc::clone(&oauth) as Arc<dyn OAuthAuth>),
            },
            behavior,
        ));

        let result = models.refresh(ModelsRefreshOptions::default()).await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(
            model_refresh_credential
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            Some(Credential::OAuth(rotated.clone()))
        );
        let stored = credentials
            .read("oauth-dynamic", &AuthOperationOptions::NONE)
            .await
            .unwrap()
            .unwrap();
        match stored {
            Credential::OAuth(stored) => {
                assert_eq!(stored.access, "fresh");
                assert_eq!(stored.refresh, "rotated");
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }
        let _ = oauth.refresh_calls();
    }

    /// Oracle "always gives providers a concrete signal"
    /// (models-runtime.test.ts:462-478).
    #[tokio::test]
    async fn refresh_always_gives_providers_a_concrete_signal() {
        let received = Arc::new(std::sync::Mutex::new(None::<CancellationToken>));
        let behavior_received = Arc::clone(&received);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let received = Arc::clone(&behavior_received);
            Box::pin(async move {
                *received
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(context.signal.clone());
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(DynamicFixture::dynamic(
            "dynamic",
            vec![],
            ambient_auth(),
            behavior,
        ));

        let result = models.refresh(ModelsRefreshOptions::default()).await;
        assert!(!result.aborted);
        let received = received
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("provider received a signal");
        assert!(!received.is_cancelled());
    }

    /// Oracle "binds model-store waits to the provider refresh signal"
    /// (models-runtime.test.ts:480-513). Cancellation-token identity is not
    /// directly comparable, so the port proves identity by cancelling one
    /// recorded handle and requiring every recorded handle to fire.
    #[tokio::test]
    async fn refresh_binds_model_store_waits_to_the_provider_signal() {
        let store = Arc::new(SignalRecordingStore::new());
        let provider_signal = Arc::new(std::sync::Mutex::new(None::<CancellationToken>));
        let behavior_signal = Arc::clone(&provider_signal);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let received = Arc::clone(&behavior_signal);
            Box::pin(async move {
                *received
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(context.signal.clone());
                if !context.allow_network {
                    return Ok(());
                }
                context
                    .publish(ModelsPublication {
                        persist: Some(Some(ModelsStoreEntry {
                            models: vec![crate::ai::types::AnyModel::Chat(test_model(
                                "dynamic", "fresh",
                            ))],
                            ..ModelsStoreEntry::default()
                        })),
                        update: None,
                    })
                    .await?;
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions {
            models_store: Some(Arc::clone(&store) as Arc<dyn ModelsStore>),
            ..CreateModelsOptions::default()
        });
        models.set_provider(DynamicFixture::dynamic(
            "dynamic",
            vec![],
            auth_with(EnvKeyAuthFixture::env("key")),
            behavior,
        ));

        let result = models
            .refresh(ModelsRefreshOptions {
                providers: Some(vec!["dynamic".to_string()]),
                ..ModelsRefreshOptions::default()
            })
            .await;

        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let signals = store.recorded();
        assert_eq!(signals.len(), 3);
        // Both refresh phases and the publication storage write observed the
        // provider refresh signal: cancelling it fires every recorded handle.
        provider_signal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap()
            .cancel();
        assert!(signals.iter().all(|signal| signal.is_cancelled()));
    }

    /// Oracle "returns aborted state without reporting cancellation as a
    /// provider error" (models-runtime.test.ts:515-531).
    #[tokio::test]
    async fn refresh_returns_aborted_without_reporting_cancellation_as_error() {
        let token = CancellationToken::new();
        let behavior_token = token.clone();
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let token = behavior_token.clone();
            Box::pin(async move {
                token.cancel();
                if context.signal.is_cancelled() {
                    return Ok(());
                }
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(DynamicFixture::dynamic(
            "dynamic",
            vec![],
            ambient_auth(),
            behavior,
        ));

        let result = models
            .refresh(ModelsRefreshOptions {
                signal: Some(token),
                ..ModelsRefreshOptions::default()
            })
            .await;
        assert!(result.aborted);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
    }

    /// Oracle "stops waiting on abort when a provider ignores its signal"
    /// (models-runtime.test.ts:533-568).
    #[tokio::test]
    async fn refresh_stops_waiting_on_abort_when_provider_ignores_signal() {
        let token = CancellationToken::new();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let behavior_calls = Arc::clone(&calls);
        let behavior_started = Arc::clone(&started);
        let behavior_release = Arc::clone(&release);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let calls = Arc::clone(&behavior_calls);
            let started = Arc::clone(&behavior_started);
            let release = Arc::clone(&behavior_release);
            let _ = context;
            Box::pin(async move {
                let current = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if current != 1 {
                    return Ok(());
                }
                started.notify_one();
                // Ignores the signal entirely; the refresh must not wait.
                release.notified().await;
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(DynamicFixture::dynamic(
            "dynamic",
            vec![],
            ambient_auth(),
            behavior,
        ));

        let orchestrator = {
            let started = Arc::clone(&started);
            let token = token.clone();
            async move {
                started.notified().await;
                token.cancel();
            }
        };
        let (result, ()) = tokio::join!(
            models.refresh(ModelsRefreshOptions {
                signal: Some(token),
                ..ModelsRefreshOptions::default()
            }),
            orchestrator
        );
        assert!(result.aborted);
        assert!(result.errors.is_empty(), "{:?}", result.errors);

        // The abandoned (dropped) provider phase can never contribute a late
        // failure to the already-returned result.
        release.notify_one();
        tokio::task::yield_now().await;
        assert!(result.errors.is_empty());
    }

    /// Oracle "rejects late publication from a superseded non-cooperative
    /// provider" (models-runtime.test.ts:570-615).
    #[tokio::test]
    async fn refresh_rejects_late_publication_from_a_superseded_provider() {
        let models_store = Arc::new(InMemoryModelsStore::default());
        let state = Arc::new(std::sync::Mutex::new("initial".to_string()));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let behavior_state = Arc::clone(&state);
        let behavior_calls = Arc::clone(&calls);
        let behavior_started = Arc::clone(&started);
        let behavior_release = Arc::clone(&release);
        let behavior: FixtureBehavior = Arc::new(move |context| {
            let state = Arc::clone(&behavior_state);
            let calls = Arc::clone(&behavior_calls);
            let started = Arc::clone(&behavior_started);
            let release = Arc::clone(&behavior_release);
            Box::pin(async move {
                if !context.allow_network {
                    return Ok(());
                }
                let current = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if current == 1 {
                    started.notify_one();
                    // Non-cooperative: parks until released; supersede drops
                    // this future before it can publish.
                    release.notified().await;
                }
                let value = format!("generation-{current}");
                let state_for_update = Arc::clone(&state);
                let model = test_model("dynamic", &value);
                let applied = context
                    .publish(ModelsPublication {
                        persist: Some(Some(ModelsStoreEntry {
                            models: vec![crate::ai::types::AnyModel::Chat(model)],
                            ..ModelsStoreEntry::default()
                        })),
                        update: Some(Box::new(move || {
                            *state_for_update
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
                        })),
                    })
                    .await?;
                let _ = applied;
                Ok(())
            })
        });
        let mut models = create_models(CreateModelsOptions {
            models_store: Some(Arc::clone(&models_store) as Arc<dyn ModelsStore>),
            ..CreateModelsOptions::default()
        });
        models.set_provider(DynamicFixture::dynamic(
            "dynamic",
            vec![],
            ambient_auth(),
            behavior,
        ));

        let refresh_options = || ModelsRefreshOptions {
            providers: Some(vec!["dynamic".to_string()]),
            ..ModelsRefreshOptions::default()
        };
        let orchestrator = {
            let models = &models;
            let started = Arc::clone(&started);
            async move {
                started.notified().await;
                // The second refresh supersedes the parked first one.
                models.refresh(refresh_options()).await
            }
        };
        let (first, second) = tokio::join!(models.refresh(refresh_options()), orchestrator);
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert!(second.errors.is_empty(), "{:?}", second.errors);

        assert_eq!(
            *state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "generation-2"
        );
        let stored = models_store
            .read("dynamic", &StoreOptions::NONE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.models[0].id(), "generation-2");
    }

    /// Oracle "checks provider auth without refreshing OAuth and filters
    /// available models" getAvailable half (models-runtime.test.ts:806-838).
    #[tokio::test]
    async fn get_available_filters_to_configured_providers_without_refreshing_oauth() {
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(Arc::clone(&credentials) as Arc<dyn CredentialStore>),
            ..CreateModelsOptions::default()
        });
        models.set_provider(test_provider_with_auth(
            "ambient",
            vec![test_model("ambient", "a1")],
            auth_with(EnvKeyAuthFixture::env("env-key")),
        ));
        models.set_provider(test_provider_with_auth(
            "missing",
            vec![test_model("missing", "m1")],
            auth_with(EnvKeyAuthFixture::missing()),
        ));
        let oauth = TestOAuth::no_refresh();
        models.set_provider(test_provider_with_auth(
            "oauth",
            vec![test_model("oauth", "o1")],
            ProviderAuth {
                api_key: None,
                oauth: Some(Arc::clone(&oauth) as Arc<dyn OAuthAuth>),
            },
        ));
        store_credential(&credentials, "oauth", oauth_credential("expired", 0)).await;

        let available = models.get_available(None, None).await.unwrap();
        let providers: Vec<&str> = available
            .iter()
            .map(|model| model.provider.as_str())
            .collect();
        assert_eq!(providers, ["ambient", "oauth"]);
        let available = models.get_available(Some("ambient"), None).await.unwrap();
        let providers: Vec<&str> = available
            .iter()
            .map(|model| model.provider.as_str())
            .collect();
        assert_eq!(providers, ["ambient"]);
        // The configurability check never refreshes OAuth.
        assert_eq!(oauth.refresh_calls(), 0);
    }

    /// Upstream `filterModels` wiring (models.ts:549-551, 773, 850): the
    /// provider's filter narrows `getAvailable` output, per credential.
    #[tokio::test]
    async fn get_available_applies_the_provider_filter_models() {
        let mut models = create_models(CreateModelsOptions::default());
        let filter: FilterModelsFn = Arc::new(|models, _credential| {
            models
                .iter()
                .filter(|model| model.id == "kept")
                .cloned()
                .collect()
        });
        models.set_provider(DynamicFixture::filtered(
            "filtered",
            vec![
                test_model("filtered", "kept"),
                test_model("filtered", "dropped"),
            ],
            auth_with(EnvKeyAuthFixture::env("key")),
            filter,
        ));
        // No filter on this one: the full catalog passes through.
        models.set_provider(test_provider_with_auth(
            "plain",
            vec![test_model("plain", "kept"), test_model("plain", "also")],
            auth_with(EnvKeyAuthFixture::env("key")),
        ));

        let available = models.get_available(None, None).await.unwrap();
        let ids: Vec<&str> = available.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["kept", "kept", "also"]);
    }

    /// Oracle "stops waiting for non-cooperative auth callbacks" getAvailable
    /// half (models-runtime.test.ts:688-692): aborting the operation rejects
    /// with cancellation even while a provider's `check` parks forever. (The
    /// getAuth half of the oracle belongs to the Task 3 surface.)
    #[tokio::test]
    async fn get_available_stops_waiting_for_non_cooperative_auth_callbacks() {
        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider_with_auth(
            "p1",
            vec![test_model("p1", "model-a")],
            auth_with(Arc::new(BlockingCheckAuth {
                started: Arc::clone(&started),
                finish: Arc::clone(&finish),
            })),
        ));
        let token = CancellationToken::new();

        let orchestrator = {
            let started = Arc::clone(&started);
            let token = token.clone();
            async move {
                started.notified().await;
                token.cancel();
            }
        };
        let options = AuthOperationOptions::new(token);
        let (available, ()) =
            tokio::join!(models.get_available(None, Some(&options)), orchestrator);
        assert_eq!(available, Err(AuthError::Cancelled));
        finish.notify_one();
    }

    #[tokio::test]
    async fn request_callbacks_are_explicitly_refused_by_unsupported_adapters_only() {
        let model = test_model("callback-test", "model");
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider("callback-test", vec![model.clone()]));
        let mut simple = SimpleStreamOptions::default();
        simple.stream.callbacks.on_response =
            Some(Arc::new(|_, _| panic!("unsupported callback must not run")));
        let mut stream = models.stream_simple(
            &model,
            &user_context(),
            Some(ModelsSimpleStreamOptions {
                simple,
                transform_headers: None,
            }),
        );
        let AssistantMessageEvent::Error { error, .. } =
            stream.recv().await.expect("explicit setup failure")
        else {
            panic!("setup error")
        };
        assert_eq!(
            error.error_message.as_deref(),
            Some(
                format!(
                    "API {} does not yet support request lifecycle callbacks",
                    model.api
                )
                .as_str()
            )
        );
        assert!(stream.recv().await.is_none());
        let mut original = models.stream_simple(&model, &user_context(), None);
        assert!(
            original.recv().await.is_none(),
            "callback-free StubApi path remains unchanged"
        );
    }

    // ------------------------------------------------------------------
    // Models.cancelDeferred (models.ts:743-754)
    // ------------------------------------------------------------------

    fn deferred_handle(provider: &str, model_id: &str, api: &str, id: &str) -> DeferredHandle {
        DeferredHandle {
            provider: provider.to_string(),
            model_id: model_id.to_string(),
            api: api.to_string(),
            id: id.to_string(),
            expires_at: None,
            poll_after_ms: None,
            data: None,
        }
    }

    fn deferred_submission() -> Option<ModelsSimpleStreamOptions> {
        Some(ModelsSimpleStreamOptions {
            simple: SimpleStreamOptions {
                deferred: Some(DeferredFlag::Bool(true)),
                ..SimpleStreamOptions::default()
            },
            transform_headers: None,
        })
    }

    /// Upstream providers.test.ts "records cancellation and returns deferred
    /// fetch failures in-band": the routed cancel reaches the faux provider's
    /// `cancelDeferred` capability, which records the handle and poisons
    /// later fetches.
    #[tokio::test]
    async fn cancel_deferred_routes_to_the_provider_capability() {
        let faux = faux_provider(FauxProviderOptions::default());
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(Arc::clone(&faux.provider));
        let model = faux.get_model(None).expect("faux model");
        faux.set_responses(vec![
            FauxResponseStep::Factory(Arc::new(|_args| {
                Box::pin(async { Err("deferred failed".to_owned()) })
            })),
            faux_assistant_message("cancelled", FauxMessageOptions::default()).into(),
        ]);

        let failed_submission = models
            .complete_simple(&model, &user_context(), deferred_submission())
            .await;
        let failed_handle = failed_submission
            .deferred
            .clone()
            .expect("the deferred submission carries a handle");
        let failed = models
            .fetch_deferred(&model, &failed_handle, &user_context(), None)
            .await;
        assert_eq!(failed.stop_reason, StopReason::Error);
        assert_eq!(failed.error_message.as_deref(), Some("deferred failed"));

        let cancelled_submission = models
            .complete_simple(&model, &user_context(), deferred_submission())
            .await;
        let handle = cancelled_submission
            .deferred
            .clone()
            .expect("the deferred submission carries a handle");
        models
            .cancel_deferred(&model, &handle, None)
            .await
            .expect("the routed cancel succeeds");
        let cancelled = faux
            .state()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancelled_deferred
            .clone();
        assert_eq!(cancelled, vec![handle.clone()], "the faux recorded it");
        let poisoned = models
            .fetch_deferred(&model, &handle, &user_context(), None)
            .await;
        assert_eq!(poisoned.stop_reason, StopReason::Error);
        assert!(
            poisoned
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("was cancelled"),
            "{:?}",
            poisoned.error_message
        );
    }

    /// Upstream models.ts:748 (`requireProvider`): an unregistered provider
    /// rejects with the typed provider error.
    #[tokio::test]
    async fn cancel_deferred_reports_the_unknown_provider_error() {
        let models = create_models(CreateModelsOptions::default());
        let model = test_model("missing", "model");
        let handle = deferred_handle("missing", "model", "test-api", "job-1");
        let error = models
            .cancel_deferred(&model, &handle, None)
            .await
            .expect_err("unknown provider rejected");
        let models_error = error.downcast_ref::<ModelsError>().expect("typed error");
        assert!(matches!(models_error.code, ModelsErrorCode::Provider));
        assert_eq!(models_error.message, "Unknown provider: missing");
    }

    /// Upstream models.ts:749-751: the capability check runs before auth
    /// resolution, so a configured-but-incapable provider rejects with the
    /// upstream literal.
    #[tokio::test]
    async fn cancel_deferred_reports_the_missing_capability() {
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(test_provider("stub", vec![test_model("stub", "model")]));
        let model = test_model("stub", "model");
        let handle = deferred_handle("stub", "model", "test-api", "job-1");
        let error = models
            .cancel_deferred(&model, &handle, None)
            .await
            .expect_err("capability-less provider rejected");
        let models_error = error.downcast_ref::<ModelsError>().expect("typed error");
        assert!(matches!(models_error.code, ModelsErrorCode::Provider));
        assert_eq!(
            models_error.message,
            "Provider stub does not support deferred responses"
        );
    }
}

#[cfg(test)]
mod shared_registry_tests;
