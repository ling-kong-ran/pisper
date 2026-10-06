//! Upstream `providers/radius-config.ts` + `providers/radius.ts`: the Radius
//! gateway provider. Radius is *purely dynamic* — models come from a gateway
//! config (`GET {gateway}/v1/config`), never a generated catalog — so unlike
//! the other factories this is a hand-rolled [`Provider`] (upstream returns a
//! hand-built object instead of calling `createProvider`): the refresh cycle
//! restores the persisted catalog, imports the legacy config cached on the
//! OAuth credential when nothing is stored, then fetches the gateway config
//! over the network and persists the result.

use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;

use crate::ai::api::http_client;
use crate::ai::api::pi_messages::PiMessages;
use crate::ai::auth::helpers::{env_api_key_auth, lazy_oauth, OAuthLoader};
use crate::ai::auth::oauth::load::load_radius_oauth;
use crate::ai::auth::oauth::radius::RadiusOAuthOptions;
use crate::ai::auth::resolve::{ModelsError, ModelsErrorCode};
use crate::ai::auth::types::{AuthError, Credential, OAuthAuth, OAuthCredential, ProviderAuth};
use crate::ai::models::{
    ModelsPublication, ModelsStoreEntry, Provider, RefreshModelsContext, RefreshModelsError,
};
use crate::ai::{now_ms, ApiImpl};

use serde::Deserialize as _;

use crate::ai::types::Model;

/// Upstream `DEFAULT_RADIUS_GATEWAY` (radius-config.ts:4).
pub const DEFAULT_RADIUS_GATEWAY: &str = "https://radius.pi.dev";

/// Upstream `RadiusGatewayConfig` (radius-config.ts:24-27). Models stay
/// raw: `sanitizeRadiusGatewayConfig` validates the upstream shape check and
/// [`get_radius_models_from_config`] merges the identity fields and
/// deserializes, so the two halves of the upstream validation are split the
/// same way here.
#[derive(Debug, Clone, PartialEq)]
pub struct RadiusGatewayConfig {
    pub base_url: String,
    pub models: Vec<serde_json::Value>,
}

/// Upstream `isRadiusGatewayModel` (radius-config.ts:29-46).
fn is_radius_gateway_model(value: &serde_json::Value) -> bool {
    let Some(model) = value.as_object() else {
        return false;
    };
    let is_number =
        |value: Option<&serde_json::Value>| value.is_some_and(serde_json::Value::is_number);
    model.get("id").is_some_and(serde_json::Value::is_string)
        && model.get("name").is_some_and(serde_json::Value::is_string)
        && model
            .get("reasoning")
            .is_some_and(serde_json::Value::is_boolean)
        && model.get("input").is_some_and(serde_json::Value::is_array)
        && is_number(model.get("contextWindow"))
        && is_number(model.get("maxTokens"))
        && model.get("cost").is_some_and(|cost| cost.is_object())
}

/// Upstream `sanitizeRadiusGatewayConfig` (radius-config.ts:48-58): require a
/// string `baseUrl` and an array `models`, keeping the shape-valid entries.
pub fn sanitize_radius_gateway_config(config: &serde_json::Value) -> Option<RadiusGatewayConfig> {
    let object = config.as_object()?;
    let base_url = object.get("baseUrl").and_then(serde_json::Value::as_str)?;
    let models = object.get("models").and_then(serde_json::Value::as_array)?;
    Some(RadiusGatewayConfig {
        base_url: base_url.to_string(),
        models: models
            .iter()
            .filter(|model| is_radius_gateway_model(model))
            .cloned()
            .collect(),
    })
}

/// Upstream `normalizeRadiusGatewayUrl` (radius-config.ts:60-63): default the
/// scheme to https and strip trailing slashes.
pub fn normalize_radius_gateway_url(value: &str) -> String {
    let has_scheme = value.starts_with("http://") || value.starts_with("https://");
    let with_scheme = if has_scheme {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// Upstream `getRadiusCredentialConfig` (radius-config.ts:65-67): the
/// sanitized `gatewayConfig` cached on an OAuth credential.
pub fn get_radius_credential_config(credential: &OAuthCredential) -> Option<RadiusGatewayConfig> {
    sanitize_radius_gateway_config(credential.extra.get("gatewayConfig")?)
}

/// Upstream `getRadiusModelsFromConfig` (radius-config.ts:75-85): stamp
/// `api`/`provider`/`baseUrl` onto every model entry. Entries that fail to
/// deserialize are skipped — the shape check above passed them, but the
/// deserializer is stricter (e.g. unknown `input` values); upstream would
/// carry the garbage into the Model and fail at request time.
pub fn get_radius_models_from_config(
    provider_id: &str,
    config: &RadiusGatewayConfig,
) -> Vec<Model> {
    config
        .models
        .iter()
        .filter_map(|model| {
            let mut object = model.as_object()?.clone();
            object.insert("api".to_string(), serde_json::json!("pi-messages"));
            object.insert("provider".to_string(), serde_json::json!(provider_id));
            object.insert("baseUrl".to_string(), serde_json::json!(config.base_url));
            Model::deserialize(&serde_json::Value::Object(object)).ok()
        })
        .collect()
}

/// Upstream `getRadiusModels` (radius-config.ts:87-91): the models cached on
/// a credential's gateway config, or an empty list.
pub fn get_radius_models(provider_id: &str, credential: Option<&OAuthCredential>) -> Vec<Model> {
    match credential.and_then(get_radius_credential_config) {
        Some(config) => get_radius_models_from_config(provider_id, &config),
        None => Vec::new(),
    }
}

fn truncate_http_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() > 512 {
        let truncated: String = trimmed.chars().take(512).collect();
        format!("{truncated}…")
    } else {
        trimmed.to_string()
    }
}

/// Upstream `loadRadiusGatewayConfig` (radius-config.ts:93-110): fetch
/// `{gateway}/v1/config` with the optional bearer key. Cancellation is not
/// threaded into the request here: the refresh caller races the whole phase
/// against its token and drops the in-flight future (the Rust abort).
pub async fn load_radius_gateway_config(
    gateway: &str,
    api_key: Option<&str>,
) -> Result<RadiusGatewayConfig, ModelsError> {
    let failure = |detail: String| {
        ModelsError::new(
            ModelsErrorCode::ModelSource,
            format!("Could not load Radius config from {gateway}: {detail}"),
        )
    };
    let mut request = http_client()
        .get(format!("{gateway}/v1/config"))
        .header("accept", "application/json");
    if let Some(api_key) = api_key {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| failure(error.to_string()))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| failure(error.to_string()))?;
    if !status.is_success() {
        return Err(failure(format!(
            "{}: {}",
            status.as_u16(),
            truncate_http_body(&body)
        )));
    }
    let config: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| failure(error.to_string()))?;
    sanitize_radius_gateway_config(&config).ok_or_else(|| {
        ModelsError::new(
            ModelsErrorCode::ModelSource,
            format!("Invalid Radius config from {gateway}"),
        )
    })
}

/// Upstream `RadiusProviderOptions` (radius.ts:13-17).
#[derive(Debug, Default, Clone)]
pub struct RadiusProviderOptions {
    pub id: Option<String>,
    pub name: Option<String>,
    pub gateway: Option<String>,
}

/// The gateway-config loader seam: the HTTP implementation by default, a
/// stub in tests. Arguments: the (normalized) gateway URL and the effective
/// api key (`None` = anonymous).
pub type RadiusConfigLoader = Arc<
    dyn Fn(String, Option<String>) -> BoxFuture<'static, Result<RadiusGatewayConfig, ModelsError>>
        + Send
        + Sync,
>;

/// The default [`RadiusConfigLoader`] over [`load_radius_gateway_config`].
fn http_config_loader() -> RadiusConfigLoader {
    Arc::new(|gateway, api_key| {
        Box::pin(async move { load_radius_gateway_config(&gateway, api_key.as_deref()).await })
    })
}

/// The Radius gateway provider (upstream `radiusProvider`, radius.ts:20-82).
pub struct RadiusProvider {
    id: String,
    name: String,
    gateway: String,
    /// Upstream `baselineModels` (radius.ts, the unified-catalog delta):
    /// the generated default-gateway catalog, chat entries with the
    /// provider overridden to the instance id; empty for a custom gateway.
    baseline: Vec<crate::ai::types::AnyModel>,
    /// The dynamic overlay (upstream's captured `dynamicModels`).
    models: Arc<RwLock<Vec<crate::ai::types::AnyModel>>>,
    auth: ProviderAuth,
    api: Arc<dyn ApiImpl>,
    load_config: RadiusConfigLoader,
}

/// Upstream `radiusProvider(options)` (radius.ts:20-82) with the default
/// HTTP config loader.
pub fn radius_provider(options: RadiusProviderOptions) -> Arc<RadiusProvider> {
    radius_provider_with_loader(options, http_config_loader())
}

/// [`radius_provider`] with an injected loader (test seam; the upstream
/// function is not parameterizable, so this is port-invented surface).
pub(crate) fn radius_provider_with_loader(
    options: RadiusProviderOptions,
    load_config: RadiusConfigLoader,
) -> Arc<RadiusProvider> {
    let id = options.id.unwrap_or_else(|| "radius".to_string());
    let name = options.name.unwrap_or_else(|| "Radius".to_string());
    let gateway = normalize_radius_gateway_url(
        &options
            .gateway
            .unwrap_or_else(|| DEFAULT_RADIUS_GATEWAY.to_string()),
    );
    // Upstream radius.ts (the unified-catalog delta): `baselineModels` is
    // the generated default-gateway catalog when the configured gateway is
    // the default, else empty; `dynamicModels = getRadiusModels(id,
    // undefined)` — no credential at build time, so the overlay starts
    // empty. The embedded catalog carries no radius shard in this snapshot
    // (data is generated), so the baseline is empty here exactly like the
    // kimi-coding drift; the merge paths are in place for generated data.
    let baseline: Vec<crate::ai::types::AnyModel> =
        crate::ai::models::catalog::embedded_provider_catalog("radius")
            .into_iter()
            .map(|mut model| {
                model.provider = id.clone();
                crate::ai::types::AnyModel::Chat(model)
            })
            .collect();
    let models: Arc<RwLock<Vec<crate::ai::types::AnyModel>>> = Arc::new(RwLock::new(Vec::new()));
    let oauth_name = name.clone();
    let oauth_gateway = gateway.clone();
    let oauth = lazy_oauth(
        name.clone(),
        false,
        None,
        Arc::new(move || {
            let name = oauth_name.clone();
            let gateway = oauth_gateway.clone();
            Box::pin(async move { Ok(load_radius_oauth(RadiusOAuthOptions { name, gateway })) })
                as BoxFuture<'static, Result<Arc<dyn OAuthAuth>, AuthError>>
        }) as Arc<OAuthLoader>,
    );
    Arc::new(RadiusProvider {
        id,
        name,
        gateway,
        baseline,
        models,
        auth: ProviderAuth {
            api_key: Some(env_api_key_auth("Radius API key", &["RADIUS_API_KEY"])),
            oauth: Some(Arc::new(oauth)),
        },
        api: Arc::new(PiMessages),
        load_config,
    })
}

/// The mutable catalog behind `getModels` (upstream's captured `models`
/// variable): replaced wholesale by the refresh update closures.
fn write_models(
    models: &Arc<RwLock<Vec<crate::ai::types::AnyModel>>>,
    updated: Vec<crate::ai::types::AnyModel>,
) {
    let mut guard = models
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = updated;
}

/// Upstream radius.ts getModels (the unified-catalog delta): the baseline
/// with the dynamic overlay merged in — a dynamic model replaces the
/// baseline entry with the same id in place, otherwise it appends.
fn merge_radius_catalog(
    baseline: &[crate::ai::types::AnyModel],
    dynamic: &[crate::ai::types::AnyModel],
) -> Vec<crate::ai::types::AnyModel> {
    let mut merged = baseline.to_vec();
    for model in dynamic {
        match merged.iter().position(|entry| entry.id() == model.id()) {
            Some(index) => merged[index] = model.clone(),
            None => merged.push(model.clone()),
        }
    }
    merged
}

impl Provider for RadiusProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    fn get_models(&self) -> Result<Vec<Model>, ModelsError> {
        let models = self
            .models
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(merge_radius_catalog(&self.baseline, &models)
            .into_iter()
            .filter_map(|model| model.as_chat().cloned())
            .collect())
    }

    fn get_all_models(&self) -> Result<Vec<crate::ai::types::AnyModel>, ModelsError> {
        let models = self
            .models
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(merge_radius_catalog(&self.baseline, &models))
    }

    fn is_dynamic(&self) -> bool {
        true
    }

    fn api_for(&self, _model: &Model) -> Option<Arc<dyn ApiImpl>> {
        Some(Arc::clone(&self.api))
    }

    /// Upstream radius.ts:35-78: restore, legacy import, fetch, publish.
    fn refresh_models(
        &self,
        context: RefreshModelsContext,
    ) -> Option<BoxFuture<'static, Result<(), RefreshModelsError>>> {
        let id = self.id.clone();
        let models = Arc::clone(&self.models);
        let gateway = self.gateway.clone();
        let load_config = Arc::clone(&self.load_config);
        Some(Box::pin(async move {
            // Restore the stored catalog first (radius.ts:37-48).
            if let Some(stored) = context.stored.clone() {
                let restored: Vec<crate::ai::types::AnyModel> = stored
                    .models
                    .into_iter()
                    .filter(|model| model.provider() == id)
                    .collect();
                let update_models = Arc::clone(&models);
                let applied = context
                    .publish(ModelsPublication {
                        persist: None,
                        update: Some(Box::new(move || {
                            write_models(&update_models, restored);
                        })),
                    })
                    .await?;
                if !applied {
                    return Ok(());
                }
            }

            // Import catalogs cached by the pre-ModelsStore Radius
            // implementation (radius.ts:50-65): only when nothing was stored
            // and the credential is OAuth.
            if context.stored.is_none() {
                if let Some(Credential::OAuth(credential)) = &context.credential {
                    let legacy: Vec<crate::ai::types::AnyModel> =
                        get_radius_models(&id, Some(credential))
                            .into_iter()
                            .map(crate::ai::types::AnyModel::Chat)
                            .collect();
                    if !legacy.is_empty() {
                        let update_models = Arc::clone(&models);
                        let applied = context
                            .publish(ModelsPublication {
                                persist: Some(Some(ModelsStoreEntry {
                                    models: legacy.clone(),
                                    last_modified: None,
                                    checked_at: Some(now_ms()),
                                    etag: None,
                                })),
                                update: Some(Box::new(move || {
                                    write_models(&update_models, legacy);
                                })),
                            })
                            .await?;
                        if !applied {
                            return Ok(());
                        }
                    }
                }
            }

            if !context.allow_network || context.signal.is_cancelled() {
                return Ok(());
            }

            // The effective api key: the OAuth access token, else the stored
            // key (radius.ts:68).
            let api_key = match &context.credential {
                Some(Credential::OAuth(credential)) => Some(credential.access.clone()),
                Some(Credential::ApiKey(credential)) => credential.key.clone(),
                None => None,
            };
            let config = (load_config)(gateway, api_key)
                .await
                .map_err(RefreshModelsError::Failed)?;
            if context.signal.is_cancelled() {
                return Ok(());
            }
            let refreshed: Vec<crate::ai::types::AnyModel> =
                get_radius_models_from_config(&id, &config)
                    .into_iter()
                    .map(crate::ai::types::AnyModel::Chat)
                    .collect();
            let update_models = Arc::clone(&models);
            context
                .publish(ModelsPublication {
                    persist: Some(Some(ModelsStoreEntry {
                        models: refreshed.clone(),
                        last_modified: None,
                        checked_at: Some(now_ms()),
                        etag: None,
                    })),
                    update: Some(Box::new(move || {
                        write_models(&update_models, refreshed);
                    })),
                })
                .await?;
            Ok(())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::credential_store::{
        CredentialStore, InMemoryCredentialStore, ModifyCallback,
    };
    use crate::ai::auth::types::{AuthOperationOptions, OAuthCredential};
    use crate::ai::models::store::{InMemoryModelsStore, ModelsStore, ModelsStoreOperationOptions};
    use crate::ai::models::{create_models, CreateModelsOptions};
    use std::collections::BTreeMap;
    use std::sync::{Arc as StdArc, Mutex};

    /// Seed a credential through the store's serialized write path (the only
    /// write surface).
    async fn seed(store: &InMemoryCredentialStore, credential: Credential) {
        let callback: ModifyCallback = Box::new(move |_current| {
            let credential = credential.clone();
            Box::pin(async move { Ok(Some(credential)) })
        });
        store
            .modify("radius", callback, &AuthOperationOptions::NONE)
            .await
            .unwrap();
    }

    fn store_options() -> ModelsStoreOperationOptions {
        ModelsStoreOperationOptions::new(tokio_util::sync::CancellationToken::new())
    }

    /// Upstream radius-config.ts normalize cases: scheme defaulting and
    /// trailing-slash stripping.
    #[test]
    fn normalizes_gateway_urls() {
        assert_eq!(
            normalize_radius_gateway_url("https://radius.example/"),
            "https://radius.example"
        );
        assert_eq!(
            normalize_radius_gateway_url("http://radius.example///"),
            "http://radius.example"
        );
        assert_eq!(
            normalize_radius_gateway_url("radius.example"),
            "https://radius.example"
        );
        assert_eq!(
            normalize_radius_gateway_url("radius.example/path/"),
            "https://radius.example/path"
        );
    }

    /// Upstream `sanitizeRadiusGatewayConfig` + `isRadiusGatewayModel`.
    #[test]
    fn sanitizes_gateway_configs() {
        let valid = serde_json::json!({
            "baseUrl": "https://gw.test",
            "models": [
                {
                    "id": "m1", "name": "M1", "reasoning": true,
                    "input": ["text", "image"],
                    "cost": {"input": 1, "output": 2, "cacheRead": 0.1, "cacheWrite": 0},
                    "contextWindow": 200000, "maxTokens": 8192,
                    "thinkingLevelMap": {"off": null, "high": "high"},
                    "samplingParams": {"topK": 40}
                },
                {"id": "bad", "reasoning": true}
            ]
        });
        let config = sanitize_radius_gateway_config(&valid).unwrap();
        assert_eq!(config.base_url, "https://gw.test");
        assert_eq!(config.models.len(), 1, "shape-invalid entries are dropped");

        // Not an object / missing fields.
        assert_eq!(
            sanitize_radius_gateway_config(&serde_json::json!(null)),
            None
        );
        assert_eq!(
            sanitize_radius_gateway_config(&serde_json::json!({"models": []})),
            None
        );
        assert_eq!(
            sanitize_radius_gateway_config(&serde_json::json!({"baseUrl": 4, "models": []})),
            None
        );
        assert_eq!(
            sanitize_radius_gateway_config(&serde_json::json!({"baseUrl": "https://x"})),
            None
        );
    }

    /// Upstream `getRadiusModelsFromConfig`: identity fields stamped, config
    /// fields (including extension fields) preserved.
    #[test]
    fn gateway_configs_map_to_models() {
        let config = RadiusGatewayConfig {
            base_url: "https://gw.test".to_string(),
            models: vec![serde_json::json!({
                "id": "m1", "name": "M1", "reasoning": true,
                "input": ["text"],
                "cost": {"input": 1, "output": 2, "cacheRead": 0.1, "cacheWrite": 0},
                "contextWindow": 200000, "maxTokens": 8192,
                "thinkingLevelMap": {"off": null},
                "samplingParams": {"topK": 40}
            })],
        };
        let models = get_radius_models_from_config("radius", &config);
        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model.id, "m1");
        assert_eq!(model.api, "pi-messages");
        assert_eq!(model.provider, "radius");
        assert_eq!(model.base_url, "https://gw.test");
        assert!(model.reasoning);
        assert_eq!(model.context_window, 200000);
        assert_eq!(model.max_tokens, 8192);
        assert_eq!(model.cost.input, 1.0);
        assert_eq!(model.cost.cache_read, 0.1);
        assert_eq!(
            model.thinking_level_map.as_ref().unwrap().get("off"),
            Some(&None)
        );
        assert_eq!(
            model.sampling_params.as_ref().unwrap().get("topK"),
            Some(&serde_json::json!(40))
        );

        // An entry the deserializer rejects is skipped.
        let broken = RadiusGatewayConfig {
            base_url: "https://gw.test".to_string(),
            models: vec![serde_json::json!({
                "id": "bad", "name": "Bad", "reasoning": true,
                "input": ["banana"],
                "cost": {"input": 1, "output": 2, "cacheRead": 0.1, "cacheWrite": 0},
                "contextWindow": 1, "maxTokens": 1
            })],
        };
        assert!(get_radius_models_from_config("radius", &broken).is_empty());
    }

    /// Upstream radius.ts:20-34 defaults: id/name/gateway, empty baseline
    /// (purely dynamic), env-key auth, Radius OAuth, pi-messages dispatch.
    #[test]
    fn radius_provider_builds_with_upstream_defaults() {
        let provider = radius_provider(RadiusProviderOptions::default());
        assert_eq!(provider.id(), "radius");
        assert_eq!(provider.name(), "Radius");
        assert_eq!(provider.gateway, "https://radius.pi.dev");
        assert!(provider.get_models().unwrap().is_empty());
        assert!(provider.is_dynamic());
        assert_eq!(
            provider.auth().api_key.as_ref().map(|auth| auth.name()),
            Some("Radius API key")
        );
        assert_eq!(
            provider.auth().oauth.as_ref().map(|oauth| oauth.name()),
            Some("Radius")
        );
        let mut model =
            crate::ai::models::providers::test_support::api_model("radius", "pi-messages");
        model.base_url = "https://gw.test".to_string();
        assert!(provider.api_for(&model).is_some());
    }

    /// Custom options flow through (radius.ts:13-17).
    #[test]
    fn radius_provider_options_override_defaults() {
        let provider = radius_provider(RadiusProviderOptions {
            id: Some("radius-eu".to_string()),
            name: Some("Radius EU".to_string()),
            gateway: Some("radius.eu.example".to_string()),
        });
        assert_eq!(provider.id(), "radius-eu");
        assert_eq!(provider.name(), "Radius EU");
        assert_eq!(provider.gateway, "https://radius.eu.example");
    }

    /// A valid gateway config, as an OAuth credential's cached gatewayConfig.
    fn gateway_config_json() -> serde_json::Value {
        serde_json::json!({
            "baseUrl": "https://gw.test",
            "models": [
                {
                    "id": "radius-large", "name": "Radius Large", "reasoning": true,
                    "input": ["text"],
                    "cost": {"input": 1, "output": 2, "cacheRead": 0.1, "cacheWrite": 0},
                    "contextWindow": 200000, "maxTokens": 8192
                },
                {
                    "id": "radius-mini", "name": "Radius Mini", "reasoning": false,
                    "input": ["text"],
                    "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
                    "contextWindow": 128000, "maxTokens": 4096
                }
            ]
        })
    }

    fn oauth_credential_with(config: Option<serde_json::Value>, expires_in_ms: i64) -> Credential {
        let mut extra = BTreeMap::new();
        if let Some(config) = config {
            extra.insert("gatewayConfig".to_string(), config);
        }
        Credential::OAuth(OAuthCredential {
            refresh: "r".to_string(),
            access: "access-token".to_string(),
            expires: now_ms() + expires_in_ms,
            extra,
        })
    }

    /// The offline refresh path (radius.ts:50-65): with no stored catalog and
    /// an OAuth credential carrying a cached gateway config, the models are
    /// imported, published into memory, and persisted.
    #[tokio::test]
    async fn offline_refresh_imports_the_legacy_credential_config() {
        let loader = Arc::new(|_gateway: String, _api_key: Option<String>| {
            Box::pin(async move {
                Err::<RadiusGatewayConfig, _>(ModelsError::new(
                    ModelsErrorCode::ModelSource,
                    "network must not be reached offline",
                ))
            }) as BoxFuture<'static, _>
        });
        let provider = radius_provider_with_loader(RadiusProviderOptions::default(), loader);

        let credentials = InMemoryCredentialStore::default();
        seed(
            &credentials,
            oauth_credential_with(Some(gateway_config_json()), 600_000),
        )
        .await;
        let store = Arc::new(InMemoryModelsStore::default());
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(StdArc::new(credentials) as Arc<dyn CredentialStore>),
            models_store: Some(Arc::clone(&store) as Arc<dyn ModelsStore>),
            auth_context: None,
        });
        models.set_provider(provider as Arc<dyn Provider>);

        let result = models
            .refresh(crate::ai::models::ModelsRefreshOptions {
                allow_network: Some(false),
                providers: Some(vec!["radius".to_string()]),
                ..Default::default()
            })
            .await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);

        let listed = models.get_models(Some("radius"));
        let ids: Vec<&str> = listed.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["radius-large", "radius-mini"]);
        assert!(listed.iter().all(|model| model.provider == "radius"));
        assert_eq!(listed[0].base_url, "https://gw.test");

        // The imported catalog was persisted.
        let entry = store
            .read("radius", &store_options())
            .await
            .unwrap()
            .unwrap();
        let stored_ids: Vec<&str> = entry.models.iter().map(|model| model.id()).collect();
        assert_eq!(stored_ids, ["radius-large", "radius-mini"]);
        assert!(entry.checked_at.is_some());
    }

    /// Unconfigured providers skip refresh entirely (radius.ts:51 guard):
    /// no credential means no legacy import and no models.
    #[tokio::test]
    async fn offline_refresh_without_a_credential_lists_nothing() {
        let provider = radius_provider_with_loader(
            RadiusProviderOptions::default(),
            Arc::new(|_: String, _: Option<String>| {
                Box::pin(async {
                    Err::<RadiusGatewayConfig, _>(ModelsError::new(
                        ModelsErrorCode::ModelSource,
                        "network must not be reached",
                    ))
                }) as BoxFuture<'static, _>
            }),
        );
        let mut models = create_models(CreateModelsOptions::default());
        models.set_provider(provider as Arc<dyn Provider>);
        let result = models
            .refresh(crate::ai::models::ModelsRefreshOptions {
                allow_network: Some(false),
                ..Default::default()
            })
            .await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(models.get_models(Some("radius")).is_empty());
    }

    /// The network phase (radius.ts:67-77): the effective api key flows to
    /// the loader (OAuth access token), and the fetched config publishes and
    /// persists.
    #[tokio::test]
    async fn network_refresh_fetches_and_persists_the_gateway_config() {
        let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_for_loader = Arc::clone(&seen);
        let loader = Arc::new(move |gateway: String, api_key: Option<String>| {
            let seen = Arc::clone(&seen_for_loader);
            Box::pin(async move {
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(api_key);
                assert_eq!(gateway, "https://radius.pi.dev");
                let value = gateway_config_json();
                sanitize_radius_gateway_config(&value).ok_or_else(|| {
                    ModelsError::new(ModelsErrorCode::ModelSource, "invalid test config")
                })
            }) as BoxFuture<'static, Result<RadiusGatewayConfig, ModelsError>>
        });
        let provider = radius_provider_with_loader(RadiusProviderOptions::default(), loader);

        let credentials = InMemoryCredentialStore::default();
        seed(&credentials, oauth_credential_with(None, 600_000)).await;
        let store = Arc::new(InMemoryModelsStore::default());
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(StdArc::new(credentials) as Arc<dyn CredentialStore>),
            models_store: Some(Arc::clone(&store) as Arc<dyn ModelsStore>),
            auth_context: None,
        });
        models.set_provider(provider as Arc<dyn Provider>);

        let result = models
            .refresh(crate::ai::models::ModelsRefreshOptions::default())
            .await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);

        // The OAuth access token was the bearer key.
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            [Some("access-token".to_string())]
        );
        let listed = models.get_models(Some("radius"));
        assert_eq!(listed.len(), 2);
        let entry = store
            .read("radius", &store_options())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.models.len(), 2);
    }

    /// The HTTP loader builds requests against the real client shape
    /// (compile-time witness; the network path itself is upstream-tested and
    /// requires a live gateway).
    #[test]
    fn http_loader_is_a_valid_seam() {
        let _loader = http_config_loader();
    }
}
