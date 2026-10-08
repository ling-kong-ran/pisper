//! Upstream `packages/ai/src/models.ts` provider half: the [`Provider`]
//! interface (models.ts:99-156) and [`create_provider`] (models.ts:761-884) —
//! the builder both the built-in provider factories (Task 5) and config
//! custom providers go through.
//!
//! Upstream `Provider<TApi extends Api>` narrows model lists by API; the port
//! erases the generic exactly like [`Model`](crate::ai::types::Model) — `api`
//! stays an open-ended string. Upstream `hasApi()` (models.ts:896-898), the
//! runtime narrowing guard for dynamically looked-up models, is therefore a
//! plain `model.api == api` comparison here; there is no type erasure to
//! undo, so no helper is ported.
//!
//! # Task seams (M2e plan)
//!
//! - Stream behavior rides on the held `ApiImpl` handles
//!   ([`Provider::api_for`], the upstream `apiFor` dispatch, models.ts:801)
//!   and is routed by the `Models` collection with auth resolution (Task 3).
//!   [`Provider::filter_models`] (models.ts:136/773) is consumed by
//!   `Models.get_available` (models.ts:534-554), alongside refresh in the
//!   same task.
//! - Upstream `fetchDeferred`/`cancelDeferred` (models.ts:150-155, attached
//!   conditionally at models.ts:856-881) are not trait methods here: the
//!   port carries both as additive `ApiImpl` capabilities
//!   (`stream_deferred`, `cancel_deferred` + `supports_deferred_cancel`)
//!   routed by the `Models` collection, mirroring how `apiFor` dispatches
//!   the upstream optional members.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;

use crate::ai::api::ApiImpl;
use crate::ai::auth::resolve::ModelsError;
use crate::ai::auth::types::{Credential, ProviderAuth};
use crate::ai::model_operations::get_model_type;
use crate::ai::now_ms;
use crate::ai::types::{
    AnyModel, AssistantImages, ClassifierContext, ClassifierModel, ClassifierOptions,
    ClassifierResult, ImageModel, ImagesContext, ImagesOptions, Model, ModelType, ProviderHeaders,
};

use super::{ModelsPublication, RefreshModelsContext, RefreshModelsError};

/// Upstream `Provider` (models.ts:99-156): the concrete runtime unit owning
/// id/name/base metadata, auth methods, and model listing. The upstream
/// stream methods (`stream`/`streamSimple`) are not on the port trait — the
/// `Models` collection routes streams through the held `ApiImpl` handles in
/// Task 3, so the trait carries only what the collection reads.
pub trait Provider: Send + Sync {
    /// Provider id, unique within a `Models` collection.
    fn id(&self) -> &str;

    /// Display name. Defaults to the id ([`create_provider`]).
    fn name(&self) -> &str;

    /// Base URL of the provider endpoint (upstream optional `baseUrl`).
    fn base_url(&self) -> Option<&str> {
        None
    }

    /// Custom HTTP headers merged over provider defaults (upstream optional
    /// `headers`).
    fn headers(&self) -> Option<&ProviderHeaders> {
        None
    }

    /// Provider auth (upstream `auth`). At least one of `api_key`/`oauth` is
    /// present — even ambient/keyless providers report configurability
    /// through api-key `resolve`.
    fn auth(&self) -> &ProviderAuth;

    /// Current known models, sync (upstream `getModels`, models.ts:121).
    /// Static providers return their chat catalog; dynamic providers the list
    /// as of the last refresh (empty before the first). Upstream requires
    /// implementations not to throw and defends anyway ("`Models` treats a
    /// throwing implementation as having no models"): the port makes that
    /// failure channel explicit as `Err` — surfaced precisely by the provider
    /// itself, swallowed to no models by the collection.
    fn get_models(&self) -> Result<Vec<Model>, ModelsError>;

    /// Upstream `getAllModels?` (models.ts): current known models of every
    /// type, sync, with the same contract as [`Provider::get_models`].
    /// Providers with only chat models may omit it; `Models` then uses
    /// `get_models()`. Model ids are unique within each type; one upstream
    /// model may have separate entries for different operations. The port
    /// default serves every model as chat, mirroring the upstream absence
    /// path.
    fn get_all_models(&self) -> Result<Vec<AnyModel>, ModelsError> {
        Ok(self.get_models()?.into_iter().map(AnyModel::Chat).collect())
    }

    /// Upstream `refreshModels?` (models.ts:124-129): dynamic providers only.
    /// Invoked once per refresh phase — first offline to restore
    /// `context.stored`, then (when network is allowed and auth resolved) to
    /// fetch a newer list. Implementations retain their previous list on
    /// failure and publish persistence and synchronous state changes through
    /// [`RefreshModelsContext::publish`]. `None` = static provider (upstream
    /// `refreshModels === undefined`); must agree with [`Provider::is_dynamic`].
    fn refresh_models(
        &self,
        context: RefreshModelsContext,
    ) -> Option<BoxFuture<'static, Result<(), RefreshModelsError>>> {
        let _ = context;
        None
    }

    /// Upstream `provider.refreshModels !== undefined` (models.ts:404-406):
    /// whether [`Models::refresh`](super::Models::refresh) includes this
    /// provider. Pair with [`Provider::refresh_models`].
    fn is_dynamic(&self) -> bool {
        false
    }

    /// Upstream `filterModels?` (models.ts:132-136): optional provider policy
    /// for credential-specific chat-model availability.
    /// [`Models::get_available`](super::Models::get_available) applies it
    /// after confirming the provider's auth is configured, over the
    /// provider's complete sync chat catalog. `None` = no filter (upstream
    /// optional method); a filter returns the kept subset.
    fn filter_models(
        &self,
        models: &[Model],
        credential: Option<&Credential>,
    ) -> Option<Vec<Model>> {
        let _ = (models, credential);
        None
    }

    /// Upstream `provider.filterModels !== undefined`.
    fn has_filter_models(&self) -> bool {
        false
    }

    /// Upstream `provider.filterAllModels !== undefined`.
    fn has_filter_all_models(&self) -> bool {
        false
    }

    /// Upstream `filterAllModels?` (models.ts): optional credential-specific
    /// availability policy across every model type. Without it,
    /// `Models::get_all_available` applies [`Provider::filter_models`] to
    /// chat models and keeps every other model.
    fn filter_all_models(
        &self,
        models: &[AnyModel],
        credential: Option<&Credential>,
    ) -> Option<Vec<AnyModel>> {
        let _ = (models, credential);
        None
    }

    /// The API implementation serving one model (upstream
    /// `Provider.stream`/`streamSimple`, models.ts:139-149: the port's
    /// `Models` collection routes streams through the held `ApiImpl` handles
    /// instead of stream methods on the trait, so the two upstream methods
    /// collapse into this lookup). `None` = no entry — upstream produces the
    /// stream error
    /// ``Provider {id} has no API implementation for "{api}"`` at dispatch
    /// time (models.ts:808-811).
    fn api_for(&self, model: &Model) -> Option<Arc<dyn ApiImpl>> {
        let _ = model;
        None
    }

    /// The image-generation implementation serving one image model (upstream
    /// `provider.generateImages`, the `images` map dispatch on `model.api`).
    /// `None` = the provider has no image-generation capability (upstream
    /// `generateImages` absent; `Models.generateImages` turns that into the
    /// error result `Provider {id} does not support image generation`).
    fn images_for(&self, api: &str) -> Option<Arc<dyn ImagesApiImpl>> {
        let _ = api;
        None
    }

    /// The classifier implementation serving one classifier model (upstream
    /// `provider.classify`, the `classifiers` map dispatch on `model.api`).
    /// `None` = the provider has no classification capability (upstream
    /// `classify` absent; `Models.classify` turns that into the error result
    /// `Provider {id} does not support classification`).
    fn classifiers_for(&self, api: &str) -> Option<Arc<dyn ClassifierApiImpl>> {
        let _ = api;
        None
    }
}

/// Upstream `ProviderImages` (types.ts): the uniform contract of an
/// image-generation API implementation module (`generateImages`). The port
/// resolves auth in the `Models` collection like the chat streams, so the
/// implementation receives the resolved [`ProviderConfig`] alongside the
/// request options (api key override, headers, env).
pub trait ImagesApiImpl: Send + Sync {
    fn generate_images(
        &self,
        config: &crate::ai::ProviderConfig,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> BoxFuture<'static, Result<AssistantImages, ModelsError>>;
}

/// Upstream `ProviderClassifier` (types.ts): the uniform contract implemented
/// by classifier API modules (`classify`).
pub trait ClassifierApiImpl: Send + Sync {
    fn classify(
        &self,
        config: &crate::ai::ProviderConfig,
        model: &ClassifierModel,
        context: &ClassifierContext,
        options: &ClassifierOptions,
    ) -> BoxFuture<'static, Result<ClassifierResult, ModelsError>>;
}

/// Upstream `CreateProviderOptions.api`
/// (`ProviderStreams | Partial<Record<TApi, ProviderStreams>>`, models.ts:775):
/// a single implementation streams all chat models, or a map dispatches on
/// `model.api` for mixed-API providers. Optional upstream when `images` or
/// `classifiers` is given; the port models that as `None`.
#[derive(Default)]
pub enum ApiImpls {
    Single(Arc<dyn ApiImpl>),
    PerApi(BTreeMap<String, Arc<dyn ApiImpl>>),
    /// Upstream `api` absent (`images`/`classifiers` carry the provider).
    #[default]
    None,
}

/// Upstream `CreateProviderOptions.images`
/// (`Partial<Record<ImageApi, ProviderImages>>`, models.ts): image-generation
/// implementations keyed by `model.api`.
pub type ImagesImpls = BTreeMap<String, Arc<dyn ImagesApiImpl>>;

/// Upstream `CreateProviderOptions.classifiers`
/// (`Partial<Record<ClassifierApi, ProviderClassifier>>`, models.ts):
/// classifier implementations keyed by `model.api`.
pub type ClassifiersImpls = BTreeMap<String, Arc<dyn ClassifierApiImpl>>;

/// Upstream `CreateProviderOptions.fetchModels` (models.ts:772): fetch a
/// dynamic model overlay of every type; [`create_provider`] restores and
/// publishes it transactionally, dropping models of unknown types. The
/// closure receives the owned phase context (upstream `context` object;
/// [`RefreshModelsContext`] is a cheap handle).
pub type FetchModelsFn = Arc<
    dyn Fn(RefreshModelsContext) -> BoxFuture<'static, Result<FetchModelsResult, ModelsError>>
        + Send
        + Sync,
>;

/// Upstream `CreateProviderOptions.filterModels` (models.ts:773):
/// credential-specific availability filter over the provider's chat catalog.
pub type FilterModelsFn = Arc<dyn Fn(&[Model], Option<&Credential>) -> Vec<Model> + Send + Sync>;

/// Upstream `CreateProviderOptions.filterAllModels` (models.ts):
/// credential-specific availability across every model type.
pub type FilterAllModelsFn =
    Arc<dyn Fn(&[AnyModel], Option<&Credential>) -> Vec<AnyModel> + Send + Sync>;

/// Upstream `CreateProviderOptions.fetchModels` return type: a dynamic model
/// overlay of every type (`readonly ProviderModel<TApi>[]`).
pub type FetchModelsResult = Vec<AnyModel>;

/// Upstream `CreateProviderOptions` (models.ts:761-776).
pub struct CreateProviderOptions {
    pub id: String,
    /// Display name. Default: `id` (upstream `name?`).
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub headers: Option<ProviderHeaders>,
    /// Required — every provider has auth semantics, even ambient/keyless
    /// ones (upstream `auth`).
    pub auth: ProviderAuth,
    /// Static baseline models of every type (upstream `models`; empty for
    /// purely dynamic providers). Models without `type` are chat models.
    pub models: Vec<AnyModel>,
    /// Fetch a dynamic model overlay of every type (upstream `fetchModels?`).
    pub fetch_models: Option<FetchModelsFn>,
    /// Credential-specific chat availability filter (upstream `filterModels?`).
    pub filter_models: Option<FilterModelsFn>,
    /// Credential-specific availability across every model type (upstream
    /// `filterAllModels?`).
    pub filter_all_models: Option<FilterAllModelsFn>,
    /// Chat implementation: a single one for all chat models, or a map keyed
    /// by `model.api` (upstream `api`). `None` (upstream absent) is legal
    /// when `images`/`classifiers` carry the provider.
    pub api: ApiImpls,
    /// Image-generation implementations keyed by `model.api` (upstream
    /// `images?`).
    pub images: ImagesImpls,
    /// Classifier implementations keyed by `model.api` (upstream
    /// `classifiers?`).
    pub classifiers: ClassifiersImpls,
}

/// The provider [`create_provider`] builds — upstream's object literal
/// (models.ts:816-854). The dynamic overlay (`dynamicModels`) starts empty
/// and is published into by refresh ([`StandardProvider::refresh_models`],
/// models.ts:823-849); until then [`Provider::get_models`] serves exactly the
/// chat slice of the baseline.
pub struct StandardProvider {
    id: String,
    name: String,
    base_url: Option<String>,
    headers: Option<ProviderHeaders>,
    auth: ProviderAuth,
    baseline: Vec<AnyModel>,
    dynamic: Arc<RwLock<Vec<AnyModel>>>,
    fetch: Option<FetchModelsFn>,
    filter: Option<FilterModelsFn>,
    filter_all: Option<FilterAllModelsFn>,
    api: ApiImpls,
    images: ImagesImpls,
    classifiers: ClassifiersImpls,
}

/// Upstream `createProvider` (models.ts:784-884).
pub fn create_provider(options: CreateProviderOptions) -> Arc<StandardProvider> {
    // Upstream models.ts:786-790: at least one concrete implementation across
    // `api`/`images`/`classifiers` is required; empty maps are rejected.
    let has_api = match &options.api {
        ApiImpls::Single(_) => true,
        ApiImpls::PerApi(map) => !map.is_empty(),
        ApiImpls::None => false,
    };
    if !has_api && options.images.is_empty() && options.classifiers.is_empty() {
        panic!(
            "Provider {}: at least one of \"api\", \"images\", or \"classifiers\" is required.",
            options.id
        );
    }
    Arc::new(StandardProvider {
        name: options.name.unwrap_or_else(|| options.id.clone()),
        id: options.id,
        base_url: options.base_url,
        headers: options.headers,
        auth: options.auth,
        baseline: options.models,
        dynamic: Arc::new(RwLock::new(Vec::new())),
        fetch: options.fetch_models,
        filter: options.filter_models,
        filter_all: options.filter_all_models,
        api: options.api,
        images: options.images,
        classifiers: options.classifiers,
    })
}

/// Upstream `currentModels` closure (models.ts:788-800): the baseline with
/// the dynamic overlay merged in — a dynamic model replaces the baseline
/// entry with the same type and id in place, otherwise it appends after the
/// baseline.
pub(crate) fn merge_catalog(baseline: &[AnyModel], dynamic: &[AnyModel]) -> Vec<AnyModel> {
    let mut merged = baseline.to_vec();
    for model in dynamic {
        let model_type = get_model_type(model);
        match merged
            .iter()
            .position(|entry| get_model_type(entry) == model_type && entry.id() == model.id())
        {
            Some(index) => merged[index] = model.clone(),
            None => merged.push(model.clone()),
        }
    }
    merged
}

impl Provider for StandardProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    fn headers(&self) -> Option<&ProviderHeaders> {
        self.headers.as_ref()
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    /// Upstream `getModels` (models.ts, createProvider): the merged catalog
    /// filtered to chat models (`isModelType(model, "chat")`).
    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        let dynamic = self
            .dynamic
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(merge_catalog(&self.baseline, &dynamic)
            .into_iter()
            .filter_map(|model| model.as_chat().cloned())
            .collect())
    }

    /// Upstream `getAllModels` (models.ts, createProvider): the merged
    /// catalog of every model type.
    fn get_all_models(&self) -> Result<Vec<AnyModel>, ModelsError> {
        let dynamic = self
            .dynamic
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(merge_catalog(&self.baseline, &dynamic))
    }

    /// Upstream `apiFor` (models.ts:801): a single implementation serves every
    /// chat model; the map form dispatches on `model.api`. The "no API
    /// implementation" stream error for a `None` result is produced by the
    /// `Models` collection at dispatch time (models.ts:808-811).
    fn api_for(&self, model: &Model) -> Option<Arc<dyn ApiImpl>> {
        match &self.api {
            ApiImpls::Single(implementation) => Some(Arc::clone(implementation)),
            ApiImpls::PerApi(map) => map.get(&model.api).cloned(),
            ApiImpls::None => None,
        }
    }

    /// Upstream's `images` map dispatch (models.ts, createProvider):
    /// `images[model.api]`.
    fn images_for(&self, api: &str) -> Option<Arc<dyn ImagesApiImpl>> {
        self.images.get(api).cloned()
    }

    /// Upstream's `classifiers` map dispatch (models.ts, createProvider):
    /// `classifiers[model.api]`.
    fn classifiers_for(&self, api: &str) -> Option<Arc<dyn ClassifierApiImpl>> {
        self.classifiers.get(api).cloned()
    }

    fn is_dynamic(&self) -> bool {
        self.fetch.is_some()
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

    fn has_filter_models(&self) -> bool {
        self.filter.is_some()
    }

    fn has_filter_all_models(&self) -> bool {
        self.filter_all.is_some()
    }

    fn filter_all_models(
        &self,
        models: &[AnyModel],
        credential: Option<&Credential>,
    ) -> Option<Vec<AnyModel>> {
        self.filter_all
            .as_ref()
            .map(|filter| filter(models, credential))
    }

    /// Upstream `createProvider`'s built-in `refreshModels` (models.ts:823-849),
    /// present only when `fetchModels` was given. Per phase: restore the
    /// provider's slice of `context.stored` through a generation-checked
    /// publication (bailing when superseded/aborted), then — network phase
    /// only — fetch and publish the refreshed overlay together with its
    /// persisted `{ models, checkedAt }` entry. Fetched models of unknown
    /// types are dropped (upstream `fetched.filter(hasKnownModelType)`).
    fn refresh_models(
        &self,
        context: RefreshModelsContext,
    ) -> Option<BoxFuture<'static, Result<(), RefreshModelsError>>> {
        let fetch = Arc::clone(self.fetch.as_ref()?);
        let id = self.id.clone();
        let dynamic = Arc::clone(&self.dynamic);
        Some(Box::pin(async move {
            // Restore `context.stored` first (models.ts:825-838): only this
            // provider's entries, overlaying whatever a previous refresh
            // published.
            if let Some(stored) = context.stored.clone() {
                let restored: Vec<AnyModel> = stored
                    .models
                    .into_iter()
                    .filter(|model| model.provider() == id)
                    .collect();
                let dynamic_for_update = Arc::clone(&dynamic);
                let applied = context
                    .publish(ModelsPublication {
                        persist: None,
                        update: Some(Box::new(move || {
                            *write_lock(&dynamic_for_update) = restored;
                        })),
                    })
                    .await?;
                if !applied {
                    return Ok(());
                }
            }
            if !context.allow_network || context.signal.is_cancelled() {
                return Ok(());
            }
            let refreshed = fetch(context.clone())
                .await
                .map_err(RefreshModelsError::Failed)?;
            if context.signal.is_cancelled() {
                return Ok(());
            }
            // Upstream: `refreshed = fetched.filter(hasKnownModelType)`.
            let refreshed: Vec<AnyModel> =
                refreshed.into_iter().filter(has_known_model_type).collect();
            let dynamic_for_update = Arc::clone(&dynamic);
            let overlay = refreshed.clone();
            context
                .publish(ModelsPublication {
                    persist: Some(Some(super::ModelsStoreEntry {
                        models: refreshed,
                        last_modified: None,
                        checked_at: Some(now_ms()),
                        etag: None,
                    })),
                    update: Some(Box::new(move || {
                        *write_lock(&dynamic_for_update) = overlay;
                    })),
                })
                .await?;
            Ok(())
        }))
    }
}

/// Upstream `hasKnownModelType` (models.ts): models from stores and remote
/// sources may have types that only newer versions know; this version knows
/// chat, image, and classifier. The [`AnyModel`] parse layer falls unknown
/// `type` values back to chat, so every model here is known — the filter is
/// kept for upstream parity.
pub(crate) fn has_known_model_type(model: &AnyModel) -> bool {
    ModelType::ALL.contains(&model.model_type())
}

/// std RwLock write access, poison-recovering (no await while held — the
/// publication update runs synchronously by contract).
fn write_lock(dynamic: &RwLock<Vec<AnyModel>>) -> std::sync::RwLockWriteGuard<'_, Vec<AnyModel>> {
    dynamic
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::types::{ApiKeyAuth, ApiKeyAuthInput, AuthError, AuthResult};
    use crate::ai::transcript::TranscriptContext;
    use crate::ai::types::events::AssistantMessageEvent;
    use crate::ai::types::options::{SimpleStreamOptions, StreamOptions};
    use crate::ai::types::primitives::ModelCost;
    use crate::ai::types::ModelInput;
    use crate::ai::ProviderConfig;
    use futures::future::BoxFuture;
    use tokio::sync::mpsc;

    /// Upstream `testModel` fixture (models-runtime.test.ts:9-22): api
    /// "test-api".
    fn test_model(provider: &str, id: &str) -> Model {
        Model {
            id: id.to_string(),
            name: id.to_string(),
            api: "test-api".to_string(),
            provider: provider.to_string(),
            base_url: "https://example.test/v1".to_string(),
            r#type: None,
            reasoning: false,
            thinking_level_map: None,
            prompt_cache: None,
            input: vec![ModelInput::Text],
            input_limits: None,
            cost: ModelCost::default(),
            context_window: 10_000,
            max_tokens: 1000,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    /// Upstream `ambientAuth` fixture (models-runtime.test.ts:49-53): reports
    /// configured with no auth values.
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

    /// Eventless API implementation for handle-identity assertions (the
    /// provider methods are never invoked by these tests).
    struct StubApi;

    impl ApiImpl for StubApi {
        fn stream(
            &self,
            _cfg: &ProviderConfig,
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
            _cfg: &ProviderConfig,
            _model: &Model,
            _ctx: &TranscriptContext,
            _options: &SimpleStreamOptions,
        ) -> mpsc::Receiver<AssistantMessageEvent> {
            let (tx, rx) = mpsc::channel(1);
            drop(tx);
            rx
        }
    }

    fn options(
        id: &str,
        name: Option<&str>,
        models: Vec<Model>,
        api: ApiImpls,
    ) -> CreateProviderOptions {
        CreateProviderOptions {
            id: id.to_string(),
            name: name.map(str::to_string),
            base_url: None,
            headers: None,
            auth: ambient_auth(),
            models: models.into_iter().map(AnyModel::Chat).collect(),
            fetch_models: None,
            filter_models: None,
            filter_all_models: None,
            api,
            images: BTreeMap::new(),
            classifiers: BTreeMap::new(),
        }
    }

    #[test]
    fn create_provider_defaults_name_to_id_and_holds_metadata() {
        let provider = create_provider(options(
            "p1",
            None,
            vec![],
            ApiImpls::Single(Arc::new(StubApi)),
        ));
        assert_eq!(provider.id(), "p1");
        assert_eq!(provider.name(), "p1");

        let named = create_provider(options(
            "p2",
            Some("P Two"),
            vec![],
            ApiImpls::Single(Arc::new(StubApi)),
        ));
        assert_eq!(named.id(), "p2");
        assert_eq!(named.name(), "P Two");
    }

    #[test]
    fn create_provider_serves_the_static_baseline() {
        let baseline = vec![test_model("p1", "m1"), test_model("p1", "m2")];
        let provider = create_provider(options(
            "p1",
            None,
            baseline.clone(),
            ApiImpls::Single(Arc::new(StubApi)),
        ));
        // Freshly built providers serve exactly the baseline: the dynamic
        // overlay starts empty (models.ts:786) until a refresh publishes
        // into it.
        assert_eq!(provider.get_models().unwrap(), baseline);

        // Purely dynamic providers ship an empty baseline (upstream radius).
        let dynamic_only = create_provider(options(
            "radius",
            None,
            vec![],
            ApiImpls::Single(Arc::new(StubApi)),
        ));
        assert!(dynamic_only.get_models().unwrap().is_empty());
    }

    /// Upstream `currentModels` merge semantics (models.ts:788-796).
    #[test]
    fn merge_catalog_overlays_dynamic_models_on_the_baseline() {
        let baseline = [test_model("p", "a"), test_model("p", "b")];
        let mut refreshed_b = test_model("p", "b");
        refreshed_b.context_window = 999;
        let dynamic = [refreshed_b, test_model("p", "c")];

        let merged = merge_catalog(
            &baseline
                .iter()
                .map(|m| crate::ai::types::AnyModel::Chat(m.clone()))
                .collect::<Vec<_>>(),
            &dynamic
                .iter()
                .cloned()
                .map(crate::ai::types::AnyModel::Chat)
                .collect::<Vec<_>>(),
        );
        let ids: Vec<String> = merged.iter().map(|model| model.id().to_string()).collect();
        // Replaced in place, new entries appended after the baseline.
        let expected: Vec<String> = ["a", "b", "c"].iter().map(|id| id.to_string()).collect();
        assert_eq!(ids, expected);
        assert_eq!(merged[1].as_chat().unwrap().context_window, 999);
        assert_eq!(merged[2].as_chat().unwrap().context_window, 10_000);

        // An empty overlay returns the baseline unchanged.
        assert_eq!(
            merge_catalog(
                &baseline
                    .iter()
                    .cloned()
                    .map(crate::ai::types::AnyModel::Chat)
                    .collect::<Vec<_>>(),
                &[],
            ),
            baseline
                .iter()
                .cloned()
                .map(crate::ai::types::AnyModel::Chat)
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn api_for_dispatches_single_and_per_api_choices() {
        let single: Arc<dyn ApiImpl> = Arc::new(StubApi);
        let provider = create_provider(options(
            "p1",
            None,
            vec![],
            ApiImpls::Single(Arc::clone(&single)),
        ));
        // A single implementation serves every model regardless of api.
        let mut model = test_model("p1", "m1");
        assert!(Arc::ptr_eq(&provider.api_for(&model).unwrap(), &single));
        model.api = "anything-else".to_string();
        assert!(Arc::ptr_eq(&provider.api_for(&model).unwrap(), &single));

        let completions: Arc<dyn ApiImpl> = Arc::new(StubApi);
        let mut per_api: BTreeMap<String, Arc<dyn ApiImpl>> = BTreeMap::new();
        per_api.insert("test-api".to_string(), Arc::clone(&single));
        per_api.insert("anthropic-messages".to_string(), Arc::clone(&completions));
        let mixed = create_provider(options("p2", None, vec![], ApiImpls::PerApi(per_api)));

        model.api = "test-api".to_string();
        assert!(Arc::ptr_eq(&mixed.api_for(&model).unwrap(), &single));
        model.api = "anthropic-messages".to_string();
        assert!(Arc::ptr_eq(&mixed.api_for(&model).unwrap(), &completions));
        // A model whose api has no entry has no implementation (upstream
        // turns this into the "no API implementation" stream error, Task 3).
        model.api = "unknown-api".to_string();
        assert!(mixed.api_for(&model).is_none());
    }

    /// The registry contract: created providers live in collections as
    /// `Arc<dyn Provider>` (the `Agent`-field pattern).
    #[test]
    fn standard_provider_is_usable_as_the_provider_trait_object() {
        let provider: Arc<dyn Provider> = create_provider(options(
            "p1",
            None,
            vec![test_model("p1", "m1")],
            ApiImpls::Single(Arc::new(StubApi)),
        ));
        assert_eq!(provider.id(), "p1");
        assert_eq!(provider.get_models().unwrap().len(), 1);
        assert_eq!(
            provider.auth().api_key.as_ref().map(|auth| auth.name()),
            Some("Ambient")
        );
        assert!(provider.base_url().is_none());
        assert!(provider.headers().is_none());
    }
}
